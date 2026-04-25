//! Passive vendor telemetry observer.
//!
//! Inspects outbound HTTP requests the page makes to known antibot
//! vendors (see `crate::antibot::signatures`) and classifies the payload
//! **shape only** — length, top-level JSON keys, known field IDs. We
//! **never** decode obfuscated payloads; the goal is to know *what* the
//! vendor is collecting, not *what the user did*.
//!
//! Pure classifier (no IO, no feature gates). The persist + event emit
//! is wired from the CDP-backed render pool.
//!
//! Volume tracking (`TelemetryTracker`) exposes a simple ring-buffer per
//! session/vendor so the policy engine can preemptively rotate proxies
//! when a single vendor is posting aggressively, before the vendor has
//! decided to hard-block.

use super::signatures::{match_vendor_url, PxSignal, PX_SIGNALS};
use super::ChallengeVendor;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// Classified request payload. We keep the enum small + stringly-typed
/// across vendor families so it serialises cleanly into SQLite/events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayloadShape {
    /// Akamai Bot Manager v1.7 sensor_data. Top-level JSON key is
    /// `sensor_data` (plus occasional `bmak.sensor_data`).
    AkamaiSensorDataV1_7 {
        /// JSON top-level keys observed at parse time (never the values).
        keys_found: Vec<String>,
    },
    /// Akamai Bot Manager v2 (sbsd). Payload carries `sbsd_ek` + AES
    /// blob. We record only the presence of the key.
    AkamaiSensorDataV2 { has_sbsd_ek: bool },
    /// PerimeterX collector POST. Shallow scan for PX### tokens.
    PerimeterXCollector { event_ids: Vec<String> },
    /// DataDome telemetry report (navigator + probe counts).
    DataDomeReport { signal_count: usize },
    /// Cloudflare Turnstile / challenge platform post.
    CloudflareChallenge { has_tk: bool },
    /// hCaptcha execute / checkcaptcha.
    HCaptchaExecute { sitekey: Option<String> },
    /// reCAPTCHA `reload` / `anchor` endpoint.
    RecaptchaReload {
        k: Option<String>,
        v: Option<String>,
    },
    /// Imperva / F5 Shape / Kasada — we recognise the vendor but can't
    /// crack the shape without more reverse-engineering.
    OpaqueVendor { note: &'static str },
    /// Matched a vendor URL but couldn't classify the body (GET request,
    /// empty body, or unknown shape).
    Unknown,
}

/// Richer Akamai detail, only filled when v1.7/v2 parsing succeeded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AkamaiSensorInfo {
    pub version: AkamaiVersion,
    pub payload_len: usize,
    pub top_level_keys: Vec<String>,
    pub likely_fields: Vec<AkamaiField>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AkamaiVersion {
    V1_7,
    V2,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AkamaiField {
    MouseEvents,
    TouchEvents,
    Typing,
    Screen,
    Sensor,
    Fingerprint,
}

/// A single vendor-telemetry event observed on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorTelemetry {
    pub vendor: ChallengeVendor,
    pub endpoint: url::Url,
    pub method: String,
    pub payload_size: usize,
    pub payload_shape: PayloadShape,
    #[serde(with = "super::system_time_serde")]
    pub observed_at: SystemTime,
    pub session_id: String,
    /// Short static label from the matched `VendorPattern` — helps
    /// humans read SQL rows.
    pub pattern_label: &'static str,
}

/// Request context fed into `classify_request` by the render pool.
#[derive(Debug, Clone)]
pub struct ObservedRequest<'a> {
    pub url: &'a url::Url,
    pub method: &'a str,
    /// Concatenated POST body entries. May be empty (`hasPostData=false`
    /// or Chrome omitted it because it was too long).
    pub body: &'a [u8],
    pub session_id: &'a str,
}

/// Classify a request against the vendor signature table. Returns `None`
/// when the URL doesn't match any known vendor endpoint.
pub fn classify_request(req: &ObservedRequest<'_>) -> Option<VendorTelemetry> {
    let pattern = match_vendor_url(req.url)?;
    let shape = classify_shape(pattern.vendor, req.url, req.body);
    Some(VendorTelemetry {
        vendor: pattern.vendor,
        endpoint: req.url.clone(),
        method: req.method.to_string(),
        payload_size: req.body.len(),
        payload_shape: shape,
        observed_at: SystemTime::now(),
        session_id: req.session_id.to_string(),
        pattern_label: pattern.label,
    })
}

fn classify_shape(vendor: ChallengeVendor, url: &url::Url, body: &[u8]) -> PayloadShape {
    match vendor {
        ChallengeVendor::Akamai => classify_akamai(body),
        ChallengeVendor::PerimeterX => classify_perimeterx(body),
        ChallengeVendor::DataDome => classify_datadome(body),
        ChallengeVendor::CloudflareJsChallenge | ChallengeVendor::CloudflareTurnstile => {
            classify_cloudflare(body)
        }
        ChallengeVendor::HCaptcha => classify_hcaptcha(url, body),
        ChallengeVendor::Recaptcha | ChallengeVendor::RecaptchaEnterprise => {
            classify_recaptcha(url, body)
        }
        ChallengeVendor::GenericCaptcha | ChallengeVendor::AccessDenied => {
            PayloadShape::OpaqueVendor { note: "generic" }
        }
    }
}

/// Akamai: try to parse the body as JSON; detect v1.7 vs v2 by the
/// presence of `sensor_data` vs `sbsd_ek` keys. We **never** decode the
/// string value (it's obfuscated/encrypted).
fn classify_akamai(body: &[u8]) -> PayloadShape {
    if body.is_empty() {
        return PayloadShape::Unknown;
    }
    // Strings-only: look for known fixed markers without full JSON parse
    // so we stay cheap even on 50KB payloads.
    let head = &body[..body.len().min(4096)];
    let as_str = std::str::from_utf8(head).unwrap_or("");
    if as_str.contains("sbsd_ek") {
        return PayloadShape::AkamaiSensorDataV2 { has_sbsd_ek: true };
    }
    if as_str.contains("sensor_data") {
        // Try to collect top-level keys if we can parse as JSON; fall
        // back to just flagging the presence.
        let mut keys = Vec::<String>::new();
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
            if let Some(obj) = v.as_object() {
                for k in obj.keys() {
                    keys.push(k.clone());
                }
            }
        }
        if keys.is_empty() {
            keys.push("sensor_data".to_string());
        }
        return PayloadShape::AkamaiSensorDataV1_7 { keys_found: keys };
    }
    PayloadShape::Unknown
}

/// Heuristic Akamai field inference from a decoded `sensor_data` string.
/// The string is delimiter-separated (commas, pipes, 2;1; runs). We look
/// for the telltale patterns vendors leak in public reverse-engineering
/// writeups; this never decrypts, it only classifies shape.
pub fn infer_akamai_fields(payload: &str) -> Vec<AkamaiField> {
    let mut out = Vec::new();
    if payload.contains("mmd") || payload.contains("mouse") {
        out.push(AkamaiField::MouseEvents);
    }
    if payload.contains("touch") || payload.contains("doa") {
        out.push(AkamaiField::TouchEvents);
    }
    if payload.contains("kact") || payload.contains("key") {
        out.push(AkamaiField::Typing);
    }
    if payload.contains("sc;") || payload.contains("screen") {
        out.push(AkamaiField::Screen);
    }
    if payload.contains("acc;") || payload.contains("gyro") {
        out.push(AkamaiField::Sensor);
    }
    if payload.contains("uaend") || payload.contains("fpValstr") {
        out.push(AkamaiField::Fingerprint);
    }
    out
}

/// PerimeterX: scan for `PX320..=PX348` tokens in the body. We do **not**
/// decrypt the base64 blob; PX payloads frequently contain the plaintext
/// field numbers in key positions before the encoded value.
fn classify_perimeterx(body: &[u8]) -> PayloadShape {
    if body.is_empty() {
        return PayloadShape::Unknown;
    }
    let head = &body[..body.len().min(8192)];
    let as_str = std::str::from_utf8(head).unwrap_or("");
    let mut found = Vec::new();
    for sig in PX_SIGNALS.iter() {
        if as_str.contains(sig.id) {
            found.push(sig.id.to_string());
        }
    }
    PayloadShape::PerimeterXCollector { event_ids: found }
}

/// Count comma/brace tokens as a proxy for "how many signals did they
/// send?" — the real payload is obfuscated base64 but the wrapping JSON
/// is usually intact enough to count commas.
fn classify_datadome(body: &[u8]) -> PayloadShape {
    let count = body.iter().filter(|&&b| b == b',').count();
    PayloadShape::DataDomeReport {
        signal_count: count,
    }
}

fn classify_cloudflare(body: &[u8]) -> PayloadShape {
    let has_tk = !body.is_empty() && std_bstr_contains(body, b"\"tk\"");
    PayloadShape::CloudflareChallenge { has_tk }
}

fn classify_hcaptcha(url: &url::Url, _body: &[u8]) -> PayloadShape {
    let sitekey = url
        .query_pairs()
        .find(|(k, _)| k == "sitekey" || k == "k")
        .map(|(_, v)| v.into_owned());
    PayloadShape::HCaptchaExecute { sitekey }
}

fn classify_recaptcha(url: &url::Url, _body: &[u8]) -> PayloadShape {
    let mut k = None;
    let mut v = None;
    for (name, value) in url.query_pairs() {
        if name == "k" {
            k = Some(value.into_owned());
        } else if name == "v" {
            v = Some(value.into_owned());
        }
    }
    PayloadShape::RecaptchaReload { k, v }
}

fn std_bstr_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// ---------------------------------------------------------------------
/// Volume tracker — feeds `policy::engine` for preemptive RotateProxy.
/// ---------------------------------------------------------------------

/// Rolling window of telemetry timestamps per `(session_id, vendor)`.
/// Small fixed capacity — we only need to know "did we see >N hits in
/// the last K seconds?" for a conservative rotate decision.
#[derive(Debug)]
pub struct TelemetryTracker {
    window: Duration,
    threshold: usize,
    /// Key: `(session_id, vendor)` — bounded by real session count, so
    /// not a memory concern in practice.
    buckets: std::collections::HashMap<(String, ChallengeVendor), VecDeque<SystemTime>>,
}

impl TelemetryTracker {
    /// Default threshold matches the plan: >20 vendor posts in 30s for
    /// the same session+vendor triggers a preventive rotation.
    pub fn new() -> Self {
        Self {
            window: Duration::from_secs(30),
            threshold: 20,
            buckets: std::collections::HashMap::new(),
        }
    }

    /// Custom construction (used by tests).
    pub fn with_config(window: Duration, threshold: usize) -> Self {
        Self {
            window,
            threshold,
            buckets: std::collections::HashMap::new(),
        }
    }

    /// Record a telemetry observation and report whether the session
    /// just crossed the preventive-rotate threshold.
    pub fn observe(&mut self, session_id: &str, vendor: ChallengeVendor, at: SystemTime) -> bool {
        let key = (session_id.to_string(), vendor);
        let bucket = self.buckets.entry(key).or_default();
        // Drop entries older than the window.
        while let Some(front) = bucket.front().copied() {
            if at
                .duration_since(front)
                .map(|d| d > self.window)
                .unwrap_or(false)
            {
                bucket.pop_front();
            } else {
                break;
            }
        }
        bucket.push_back(at);
        bucket.len() >= self.threshold
    }

    /// Current hit count for diagnostics.
    pub fn hits(&self, session_id: &str, vendor: ChallengeVendor) -> usize {
        self.buckets
            .get(&(session_id.to_string(), vendor))
            .map(|b| b.len())
            .unwrap_or(0)
    }
}

impl Default for TelemetryTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Handy alias for the public catalog length — referenced by tests.
pub fn px_catalog() -> &'static [PxSignal] {
    PX_SIGNALS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn req<'a>(url: &'a url::Url, method: &'a str, body: &'a [u8]) -> ObservedRequest<'a> {
        ObservedRequest {
            url,
            method,
            body,
            session_id: "s1",
        }
    }

    #[test]
    fn classify_none_on_unknown_url() {
        let u = url::Url::parse("https://example.com/foo").unwrap();
        assert!(classify_request(&req(&u, "GET", b"")).is_none());
    }

    #[test]
    fn classify_akamai_v17() {
        let u = url::Url::parse("https://www.example.com/_bm/_data").unwrap();
        let body = br#"{"sensor_data":"1.7,-1,0,0,...garbage"}"#;
        let t = classify_request(&req(&u, "POST", body)).unwrap();
        assert_eq!(t.vendor, ChallengeVendor::Akamai);
        assert!(matches!(
            t.payload_shape,
            PayloadShape::AkamaiSensorDataV1_7 { .. }
        ));
    }

    #[test]
    fn classify_akamai_v2_sbsd() {
        let u = url::Url::parse("https://www.example.com/akam/11/abc").unwrap();
        let body = br#"{"sbsd_ek":"encblobhere","t":"..."}"#;
        let t = classify_request(&req(&u, "POST", body)).unwrap();
        assert_eq!(
            t.payload_shape,
            PayloadShape::AkamaiSensorDataV2 { has_sbsd_ek: true }
        );
    }

    #[test]
    fn classify_perimeterx_extracts_signal_ids() {
        let u = url::Url::parse("https://client.perimeterx.net/api/v2/collector?appId=PX").unwrap();
        let body = br#"{"PX320":"1","PX333":"Intel Iris","PX346":"false"}"#;
        let t = classify_request(&req(&u, "POST", body)).unwrap();
        match t.payload_shape {
            PayloadShape::PerimeterXCollector { event_ids } => {
                assert!(event_ids.contains(&"PX320".to_string()));
                assert!(event_ids.contains(&"PX333".to_string()));
                assert!(event_ids.contains(&"PX346".to_string()));
            }
            other => panic!("expected PerimeterXCollector, got {other:?}"),
        }
    }

    #[test]
    fn classify_hcaptcha_picks_sitekey() {
        let u = url::Url::parse(
            "https://hcaptcha.com/checkcaptcha/xyz?sitekey=10000000-ffff-ffff-ffff-000000000001",
        )
        .unwrap();
        let t = classify_request(&req(&u, "POST", b"")).unwrap();
        match t.payload_shape {
            PayloadShape::HCaptchaExecute { sitekey } => assert!(sitekey.is_some()),
            _ => panic!("expected HCaptchaExecute"),
        }
    }

    #[test]
    fn classify_recaptcha_reload() {
        let u =
            url::Url::parse("https://www.google.com/recaptcha/api2/reload?k=SITEKEY&v=V").unwrap();
        let t = classify_request(&req(&u, "POST", b"foo")).unwrap();
        match t.payload_shape {
            PayloadShape::RecaptchaReload { k, v } => {
                assert_eq!(k.as_deref(), Some("SITEKEY"));
                assert_eq!(v.as_deref(), Some("V"));
            }
            _ => panic!("expected RecaptchaReload"),
        }
    }

    #[test]
    fn tracker_fires_at_threshold() {
        let mut t = TelemetryTracker::with_config(Duration::from_secs(30), 5);
        let now = SystemTime::now();
        for i in 0..4 {
            assert!(
                !t.observe(
                    "s1",
                    ChallengeVendor::PerimeterX,
                    now + Duration::from_millis(i * 10)
                ),
                "should not fire at {i}"
            );
        }
        assert!(
            t.observe(
                "s1",
                ChallengeVendor::PerimeterX,
                now + Duration::from_millis(50)
            ),
            "5th hit should fire"
        );
    }

    #[test]
    fn tracker_window_expires_old_entries() {
        let mut t = TelemetryTracker::with_config(Duration::from_secs(1), 3);
        let now = SystemTime::now();
        t.observe("s1", ChallengeVendor::Akamai, now);
        t.observe("s1", ChallengeVendor::Akamai, now);
        // Two seconds later — the earlier two should expire.
        let later = now + Duration::from_secs(2);
        assert!(!t.observe("s1", ChallengeVendor::Akamai, later));
        assert_eq!(t.hits("s1", ChallengeVendor::Akamai), 1);
    }

    #[test]
    fn infer_akamai_fields_basic() {
        let s = "uaend;mmd=1;touch=no;sc;1920,1080;kact;abc;fpValstr=x";
        let fields = infer_akamai_fields(s);
        assert!(fields.contains(&AkamaiField::MouseEvents));
        assert!(fields.contains(&AkamaiField::Screen));
        assert!(fields.contains(&AkamaiField::Typing));
        assert!(fields.contains(&AkamaiField::Fingerprint));
    }
}
