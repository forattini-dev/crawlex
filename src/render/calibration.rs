//! Slice 32 — per-session browser fingerprint calibration for the
//! external CDP provider.
//!
//! Goal: before navigating an external CDP session to its target,
//! crawlex measures the *effective* browser fingerprint the endpoint
//! produced (UA, screen, locale, timezone, WebGL, canvas/audio sample,
//! storage quota, media, WebRTC, permissions, plugins, `window.chrome`,
//! perf-memory, WebGPU). The result is cached per-session and surfaced
//! through a concise `event="calibration.summary"` log line. The full
//! fingerprint is only emitted when the caller explicitly opts in.
//!
//! Caching is keyed on the inputs that legitimately change the
//! resulting fingerprint: external endpoint, seed, proxy, locale,
//! timezone, profile, and context identity. A change to *any* of those
//! invalidates the cache by virtue of producing a new key — entries are
//! never mutated in place.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Identity inputs that must invalidate a cached fingerprint when any
/// of them change. Two different `CalibrationKey` values produce two
/// distinct cache slots — there is no separate eviction step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CalibrationKey {
    pub endpoint: String,
    pub seed: String,
    pub proxy: String,
    pub locale: String,
    pub timezone: String,
    pub profile: String,
    pub context: String,
    /// Slice 35 — `"isolated"` vs `"persistent"`. Folded into the key
    /// so the isolated and persistent variants of the same identity
    /// never share a cached fingerprint (different cookie/storage
    /// surfaces produce different observable identity).
    pub session_mode: String,
}

impl CalibrationKey {
    /// Stable short id for telemetry. Not a security primitive — only
    /// used to correlate log lines with the cache slot.
    pub fn fingerprint_id(&self) -> String {
        let mut h = sha2::Sha256::new();
        for f in [
            &self.endpoint,
            &self.seed,
            &self.proxy,
            &self.locale,
            &self.timezone,
            &self.profile,
            &self.context,
            &self.session_mode,
        ] {
            h.update(f.as_bytes());
            // Field separator that cannot appear in any of the inputs
            // we currently compose into a key (URL / locale / TZ etc.).
            h.update(b"\x1f");
        }
        hex::encode(&h.finalize()[..16])
    }

    /// True when both keys would invalidate to the same cache slot.
    /// Useful in tests to assert "this change *did* / *did not* bust
    /// the cache".
    pub fn matches(&self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    pub avail_width: u32,
    pub avail_height: u32,
    pub color_depth: u32,
    pub pixel_ratio: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub inner_width: u32,
    pub inner_height: u32,
    pub outer_width: u32,
    pub outer_height: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WebglInfo {
    pub vendor: String,
    pub renderer: String,
    pub unmasked_vendor: String,
    pub unmasked_renderer: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WebrtcInfo {
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PermissionEntry {
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PerfMemory {
    pub js_heap_size_limit: u64,
    pub total_js_heap_size: u64,
    pub used_js_heap_size: u64,
}

/// Effective per-session browser fingerprint, exactly as observed by
/// the calibration probe inside the live external CDP page. This is
/// the data model that satisfies the "calibration result is represented
/// as an effective browser fingerprint model" acceptance criterion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EffectiveFingerprint {
    pub browser_product: String,
    pub browser_version: String,
    pub platform: String,
    pub user_agent: String,
    pub locale: String,
    pub timezone: String,
    pub screen: ScreenInfo,
    pub window: WindowInfo,
    pub webgl: WebglInfo,
    pub canvas_hash: String,
    pub audio_hash: String,
    pub storage_quota: u64,
    pub media_devices: Vec<String>,
    pub webrtc: WebrtcInfo,
    pub permissions: Vec<PermissionEntry>,
    pub plugins: Vec<String>,
    pub has_window_chrome: bool,
    #[serde(default)]
    pub performance_memory: Option<PerfMemory>,
    #[serde(default)]
    pub webgpu_adapter: Option<String>,
    #[serde(default)]
    pub mismatch_count: u32,
    #[serde(default)]
    pub policy: String,
}

/// Concise structured summary emitted on every calibration. The full
/// fingerprint is *not* in this event — callers who need it must opt in
/// via [`CalibrationCache::format_full_report`].
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationSummary<'a> {
    pub browser_product: &'a str,
    pub platform: &'a str,
    pub locale: &'a str,
    pub timezone: &'a str,
    pub webgl_renderer: &'a str,
    pub mismatch_count: u32,
    pub policy: &'a str,
}

/// Per-pool cache of calibrated fingerprints keyed on `CalibrationKey`.
#[derive(Debug, Default)]
pub struct CalibrationCache {
    inner: RwLock<HashMap<CalibrationKey, Arc<EffectiveFingerprint>>>,
}

impl CalibrationCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &CalibrationKey) -> Option<Arc<EffectiveFingerprint>> {
        self.inner.read().get(key).cloned()
    }

    pub fn insert(
        &self,
        key: CalibrationKey,
        fp: EffectiveFingerprint,
    ) -> Arc<EffectiveFingerprint> {
        let arc = Arc::new(fp);
        self.inner.write().insert(key, arc.clone());
        arc
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Render a full calibration report. Only call this when the
    /// operator explicitly requested the full payload — by default the
    /// summary event is the only thing crawlex emits.
    pub fn format_full_report(fp: &EffectiveFingerprint) -> String {
        serde_json::to_string_pretty(fp).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Parse a JSON probe payload as produced by [`CALIBRATION_PROBE_JS`].
/// Errors map to operator-readable messages.
pub fn parse_probe(json: &str) -> std::result::Result<EffectiveFingerprint, String> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Err("calibration probe returned empty body".to_string());
    }
    let mut fp: EffectiveFingerprint = serde_json::from_str(trimmed)
        .map_err(|e| format!("calibration probe JSON parse: {e}"))?;
    if fp.policy.is_empty() {
        fp.policy = "report-only".to_string();
    }
    Ok(fp)
}

/// Count the number of identity constraints that the live page failed
/// to honour. Today: `locale` and `timezone`. Returns 0 when no
/// expectation was set, or when all expectations matched (case-insensitive
/// for locale).
pub fn count_mismatches(
    fp: &EffectiveFingerprint,
    expected_locale: Option<&str>,
    expected_timezone: Option<&str>,
) -> u32 {
    let mut n = 0u32;
    if let Some(l) = expected_locale {
        let l = l.trim();
        if !l.is_empty() && !fp.locale.eq_ignore_ascii_case(l) {
            n += 1;
        }
    }
    if let Some(t) = expected_timezone {
        let t = t.trim();
        if !t.is_empty() && fp.timezone != t {
            n += 1;
        }
    }
    n
}

/// Re-export from config so slice 34 callers can write
/// `calibration::MismatchPolicy` without crossing module boundaries.
pub use crate::config::MismatchPolicy;

/// Slice 34 — kinds of calibration mismatches that classification can
/// surface. The first seven entries are the critical categories the
/// PRD enumerates; `GpuClass` is reserved for non-critical drifts that
/// should be reported but never fail a strict run on their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MismatchCategory {
    BrowserFamily,
    BrowserVersion,
    ProxyIpWebrtc,
    Locale,
    Timezone,
    Platform,
    StorageProfile,
    GpuClass,
}

impl MismatchCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            MismatchCategory::BrowserFamily => "browser_family",
            MismatchCategory::BrowserVersion => "browser_version",
            MismatchCategory::ProxyIpWebrtc => "proxy_ip_webrtc",
            MismatchCategory::Locale => "locale",
            MismatchCategory::Timezone => "timezone",
            MismatchCategory::Platform => "platform",
            MismatchCategory::StorageProfile => "storage_profile",
            MismatchCategory::GpuClass => "gpu_class",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MismatchSeverity {
    Critical,
    NonCritical,
}

impl MismatchSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            MismatchSeverity::Critical => "critical",
            MismatchSeverity::NonCritical => "non_critical",
        }
    }
}

/// One classified divergence between the session's intended identity
/// and what the calibration probe actually observed. Carries enough
/// context to debug (category, severity, short expected/observed
/// strings) without dumping the full fingerprint.
#[derive(Debug, Clone, Serialize)]
pub struct Mismatch {
    pub category: MismatchCategory,
    pub severity: MismatchSeverity,
    pub expected: String,
    pub observed: String,
    /// True when the calibration-aware shim (slice 33) can rewrite the
    /// observed value at JS scope so the page sees the expected value.
    /// False when the divergence lives below the shim (network egress
    /// IP, browser engine binary, storage backing) and a strict run
    /// must refuse to proceed.
    pub reconcilable: bool,
}

/// Caller-supplied identity expectations. Every field is optional —
/// classification only flags an axis when the caller actually declared
/// an expectation for it.
#[derive(Debug, Clone, Default)]
pub struct ExpectedIdentity {
    pub browser_family: Option<String>,
    pub browser_major: Option<String>,
    pub platform: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    /// Public IPv4 the proxy is expected to egress from. When set,
    /// any observed WebRTC IPv4 that is not this value (and is a
    /// routable public address) is flagged as a proxy/IP/WebRTC
    /// coherence mismatch. Strict runs cannot reconcile this.
    pub proxy_egress_ipv4: Option<String>,
    /// Profile id the session is supposed to be running under. A
    /// contradiction here (e.g. storage backed by a different profile)
    /// is critical and not reconcilable from the shim.
    pub profile_id: Option<String>,
    /// Minimum storage quota the session expects (bytes). When set
    /// and observed quota is meaningfully lower (less than half), the
    /// storage backing is treated as a profile contradiction.
    pub min_storage_quota: Option<u64>,
}

fn short(s: &str) -> String {
    const MAX: usize = 64;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

fn is_public_ipv4(addr: &str) -> bool {
    let octets: Vec<u8> = addr
        .split('.')
        .filter_map(|p| p.parse::<u8>().ok())
        .collect();
    if octets.len() != 4 {
        return false;
    }
    let (a, b) = (octets[0], octets[1]);
    if a == 10 || a == 127 || a == 0 {
        return false;
    }
    if a == 172 && (16..=31).contains(&b) {
        return false;
    }
    if a == 192 && b == 168 {
        return false;
    }
    if a == 169 && b == 254 {
        return false;
    }
    if a == 100 && (64..=127).contains(&b) {
        return false;
    }
    true
}

/// Slice 34 — compare expected session intent against the effective
/// fingerprint and produce one entry per divergence. Empty result
/// means the fingerprint honoured every declared expectation.
pub fn classify_mismatches(
    fp: &EffectiveFingerprint,
    expected: &ExpectedIdentity,
) -> Vec<Mismatch> {
    let mut out = Vec::new();
    if let Some(fam) = expected.browser_family.as_deref() {
        let fam = fam.trim();
        if !fam.is_empty() && !fp.browser_product.eq_ignore_ascii_case(fam) {
            out.push(Mismatch {
                category: MismatchCategory::BrowserFamily,
                severity: MismatchSeverity::Critical,
                expected: short(fam),
                observed: short(&fp.browser_product),
                reconcilable: false,
            });
        }
    }
    if let Some(major) = expected.browser_major.as_deref() {
        let major = major.trim();
        if !major.is_empty() {
            let observed_major = fp
                .browser_version
                .split('.')
                .next()
                .unwrap_or("")
                .trim();
            if !observed_major.is_empty() && observed_major != major {
                out.push(Mismatch {
                    category: MismatchCategory::BrowserVersion,
                    severity: MismatchSeverity::Critical,
                    expected: short(major),
                    observed: short(observed_major),
                    // UA-string + UA-CH versions can be rewritten by
                    // the shim; on-wire UA from the engine cannot.
                    // We mark reconcilable=true because the shim
                    // override layer (slice 33) covers the JS-visible
                    // surface, which is what fingerprint probes read.
                    reconcilable: true,
                });
            }
        }
    }
    if let Some(plat) = expected.platform.as_deref() {
        let plat = plat.trim();
        if !plat.is_empty()
            && !fp
                .platform
                .to_ascii_lowercase()
                .contains(&plat.to_ascii_lowercase())
        {
            out.push(Mismatch {
                category: MismatchCategory::Platform,
                severity: MismatchSeverity::Critical,
                expected: short(plat),
                observed: short(&fp.platform),
                reconcilable: true,
            });
        }
    }
    if let Some(loc) = expected.locale.as_deref() {
        let loc = loc.trim();
        if !loc.is_empty() && !fp.locale.eq_ignore_ascii_case(loc) {
            out.push(Mismatch {
                category: MismatchCategory::Locale,
                severity: MismatchSeverity::Critical,
                expected: short(loc),
                observed: short(&fp.locale),
                reconcilable: true,
            });
        }
    }
    if let Some(tz) = expected.timezone.as_deref() {
        let tz = tz.trim();
        if !tz.is_empty() && fp.timezone != tz {
            out.push(Mismatch {
                category: MismatchCategory::Timezone,
                severity: MismatchSeverity::Critical,
                expected: short(tz),
                observed: short(&fp.timezone),
                reconcilable: true,
            });
        }
    }
    if let Some(ip) = expected.proxy_egress_ipv4.as_deref() {
        let ip = ip.trim();
        if !ip.is_empty() {
            let leaked: Vec<&String> = fp
                .webrtc
                .ipv4
                .iter()
                .filter(|a| is_public_ipv4(a) && a.as_str() != ip)
                .collect();
            if !leaked.is_empty() {
                let observed_summary = leaked
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                out.push(Mismatch {
                    category: MismatchCategory::ProxyIpWebrtc,
                    severity: MismatchSeverity::Critical,
                    expected: short(ip),
                    observed: short(&observed_summary),
                    // WebRTC public-IP leak past the proxy lives at
                    // the network stack — no shim rewrite can hide it
                    // from a determined fingerprint probe.
                    reconcilable: false,
                });
            }
        }
    }
    if let Some(min_q) = expected.min_storage_quota {
        if min_q > 0 && fp.storage_quota > 0 && fp.storage_quota.saturating_mul(2) < min_q {
            out.push(Mismatch {
                category: MismatchCategory::StorageProfile,
                severity: MismatchSeverity::Critical,
                expected: format!(">= {min_q}"),
                observed: fp.storage_quota.to_string(),
                reconcilable: false,
            });
        }
    }
    if let Some(pid) = expected.profile_id.as_deref() {
        // A profile contradiction is only observable through indirect
        // signals — here we treat a zero storage quota under an
        // expected non-empty profile as a backing contradiction.
        if !pid.trim().is_empty() && fp.storage_quota == 0 {
            out.push(Mismatch {
                category: MismatchCategory::StorageProfile,
                severity: MismatchSeverity::Critical,
                expected: short(pid),
                observed: "storage_quota=0".to_string(),
                reconcilable: false,
            });
        }
    }
    out
}

/// True when at least one entry is both `Critical` and not
/// reconcilable by the shim. Strict policy uses this to decide
/// whether to fail before target navigation.
pub fn has_unreconciled_critical(mismatches: &[Mismatch]) -> bool {
    mismatches
        .iter()
        .any(|m| m.severity == MismatchSeverity::Critical && !m.reconcilable)
}

/// Compact event payload describing the classified mismatch set. Used
/// for `event="calibration.mismatch"` warning emission. The full
/// fingerprint is deliberately *not* included.
#[derive(Debug, Clone, Serialize)]
pub struct MismatchReport<'a> {
    pub policy: &'a str,
    pub critical: u32,
    pub non_critical: u32,
    pub unreconciled_critical: u32,
    pub categories: Vec<&'static str>,
    pub mismatches: &'a [Mismatch],
}

impl<'a> MismatchReport<'a> {
    pub fn new(policy: MismatchPolicy, mismatches: &'a [Mismatch]) -> Self {
        let mut critical = 0u32;
        let mut non_critical = 0u32;
        let mut unreconciled_critical = 0u32;
        let mut cats: Vec<&'static str> = Vec::new();
        for m in mismatches {
            match m.severity {
                MismatchSeverity::Critical => {
                    critical += 1;
                    if !m.reconcilable {
                        unreconciled_critical += 1;
                    }
                }
                MismatchSeverity::NonCritical => non_critical += 1,
            }
            let c = m.category.as_str();
            if !cats.contains(&c) {
                cats.push(c);
            }
        }
        Self {
            policy: policy.as_str(),
            critical,
            non_critical,
            unreconciled_critical,
            categories: cats,
            mismatches,
        }
    }
}

/// JS source of the calibration probe. Evaluated as an expression that
/// resolves to a JSON string (the host parses with [`parse_probe`]).
pub const CALIBRATION_PROBE_JS: &str = include_str!("calibration_probe.js");

/// HTML served at the local origin. Trivial — every interesting bit
/// of the calibration runs from `CALIBRATION_PROBE_JS` after navigation.
pub const CALIBRATION_HTML: &str = "<!doctype html><html><head>\
<meta charset=\"utf-8\"><title>__crawlex_calibrate</title></head>\
<body><script>window.__crawlex_calibrate_ready=true;</script></body></html>";

/// URL path of the calibration document. The "origin" name in the
/// acceptance criteria lives here.
pub const CALIBRATION_PATH: &str = "/__crawlex_calibrate";

/// Handle returned by [`serve_calibration_origin`]. Dropping it
/// terminates the local HTTP server.
pub struct CalibrationOrigin {
    pub base_url: String,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl CalibrationOrigin {
    pub fn calibrate_url(&self) -> String {
        format!("{}{}", self.base_url, CALIBRATION_PATH)
    }
}

/// Bind a tiny loopback HTTP server on `127.0.0.1:0`. Every request is
/// answered with [`CALIBRATION_HTML`] — the only thing this origin is
/// for is giving the calibration probe a real `http://` origin to run
/// from (so the same-origin policy doesn't kneecap probes that touch
/// `OfflineAudioContext`, `RTCPeerConnection`, `navigator.permissions`,
/// etc.).
pub async fn serve_calibration_origin() -> std::result::Result<CalibrationOrigin, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("calibration origin bind: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("calibration origin local_addr: {e}"))?;
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accept = listener.accept() => {
                    let (mut sock, _peer) = match accept {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1024];
                        // Read at most a small request prefix so we can
                        // close the connection cleanly. We don't need to
                        // parse the request — every path returns the
                        // same body.
                        let _ = tokio::time::timeout(
                            Duration::from_millis(500),
                            sock.read(&mut buf),
                        )
                        .await;
                        let body = CALIBRATION_HTML.as_bytes();
                        let head = format!(
                            "HTTP/1.1 200 OK\r\n\
                             Content-Type: text/html; charset=utf-8\r\n\
                             Content-Length: {}\r\n\
                             Cache-Control: no-store\r\n\
                             Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                        let _ = sock.shutdown().await;
                    });
                }
            }
        }
    });
    Ok(CalibrationOrigin {
        base_url: format!("http://{addr}"),
        _shutdown: tx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_probe_json() -> &'static str {
        r#"{
            "browser_product": "Chromium",
            "browser_version": "149",
            "platform": "Linux x86_64",
            "user_agent": "Mozilla/5.0 ... Chrome/149.0",
            "locale": "pt-PT",
            "timezone": "Europe/Lisbon",
            "screen": { "width": 1920, "height": 1080, "avail_width": 1920,
                        "avail_height": 1050, "color_depth": 24, "pixel_ratio": 1.0 },
            "window": { "inner_width": 1280, "inner_height": 720,
                        "outer_width": 1280, "outer_height": 760 },
            "webgl": { "vendor": "Google Inc.", "renderer": "ANGLE",
                       "unmasked_vendor": "NVIDIA",
                       "unmasked_renderer": "GeForce RTX 4070" },
            "canvas_hash": "deadbeef",
            "audio_hash": "0.12345678",
            "storage_quota": 1073741824,
            "media_devices": ["audioinput:", "videoinput:"],
            "webrtc": { "ipv4": ["192.0.2.1"], "ipv6": [] },
            "permissions": [{"name":"geolocation","state":"prompt"}],
            "plugins": [],
            "has_window_chrome": true,
            "performance_memory": {
                "js_heap_size_limit": 4294967296,
                "total_js_heap_size": 16777216,
                "used_js_heap_size": 8388608
            },
            "webgpu_adapter": "Mesa//llvmpipe"
        }"#
    }

    #[test]
    fn parse_probe_full_payload_round_trips_every_field() {
        let fp = parse_probe(sample_probe_json()).unwrap();
        assert_eq!(fp.browser_product, "Chromium");
        assert_eq!(fp.platform, "Linux x86_64");
        assert_eq!(fp.locale, "pt-PT");
        assert_eq!(fp.timezone, "Europe/Lisbon");
        assert_eq!(fp.screen.width, 1920);
        assert_eq!(fp.screen.pixel_ratio, 1.0);
        assert_eq!(fp.window.inner_width, 1280);
        assert_eq!(fp.webgl.unmasked_renderer, "GeForce RTX 4070");
        assert_eq!(fp.canvas_hash, "deadbeef");
        assert_eq!(fp.storage_quota, 1_073_741_824);
        assert_eq!(fp.media_devices.len(), 2);
        assert_eq!(fp.webrtc.ipv4, vec!["192.0.2.1".to_string()]);
        assert_eq!(fp.permissions[0].state, "prompt");
        assert!(fp.has_window_chrome);
        assert!(fp.performance_memory.is_some());
        assert_eq!(fp.webgpu_adapter.as_deref(), Some("Mesa//llvmpipe"));
        // Default policy applied when probe omitted it.
        assert_eq!(fp.policy, "report-only");
    }

    #[test]
    fn parse_probe_missing_optional_fields_is_ok() {
        let json = r#"{
            "browser_product": "Chromium", "browser_version": "",
            "platform": "Linux", "user_agent": "", "locale": "en-US",
            "timezone": "UTC",
            "screen": { "width":0, "height":0, "avail_width":0, "avail_height":0, "color_depth":0, "pixel_ratio":1 },
            "window": { "inner_width":0, "inner_height":0, "outer_width":0, "outer_height":0 },
            "webgl": { "vendor": "", "renderer": "", "unmasked_vendor": "", "unmasked_renderer": "" },
            "canvas_hash": "", "audio_hash": "", "storage_quota": 0,
            "media_devices": [], "webrtc": {"ipv4":[],"ipv6":[]},
            "permissions": [], "plugins": [], "has_window_chrome": false,
            "policy": "enforce"
        }"#;
        let fp = parse_probe(json).unwrap();
        assert!(fp.performance_memory.is_none());
        assert!(fp.webgpu_adapter.is_none());
        // Probe-supplied policy is preserved.
        assert_eq!(fp.policy, "enforce");
    }

    #[test]
    fn parse_probe_empty_body_errors() {
        assert!(parse_probe("").is_err());
        assert!(parse_probe("   ").is_err());
    }

    #[test]
    fn parse_probe_garbage_errors_with_actionable_message() {
        let err = parse_probe("not json").unwrap_err();
        assert!(err.contains("JSON parse"), "got: {err}");
    }

    #[test]
    fn count_mismatches_zero_when_no_expectation() {
        let fp = parse_probe(sample_probe_json()).unwrap();
        assert_eq!(count_mismatches(&fp, None, None), 0);
        assert_eq!(count_mismatches(&fp, Some(""), Some("")), 0);
    }

    #[test]
    fn count_mismatches_locale_case_insensitive() {
        let fp = parse_probe(sample_probe_json()).unwrap();
        assert_eq!(count_mismatches(&fp, Some("PT-pt"), None), 0);
        assert_eq!(count_mismatches(&fp, Some("en-US"), None), 1);
    }

    #[test]
    fn count_mismatches_timezone_strict() {
        let fp = parse_probe(sample_probe_json()).unwrap();
        assert_eq!(count_mismatches(&fp, None, Some("Europe/Lisbon")), 0);
        assert_eq!(count_mismatches(&fp, None, Some("Europe/lisbon")), 1);
        assert_eq!(count_mismatches(&fp, None, Some("UTC")), 1);
        assert_eq!(count_mismatches(&fp, Some("en-US"), Some("UTC")), 2);
    }

    fn key(field: &str, value: &str) -> CalibrationKey {
        let mut k = CalibrationKey {
            endpoint: "http://stealth.example:9222".to_string(),
            seed: "seed-1".to_string(),
            proxy: "http://proxy.example:3128".to_string(),
            locale: "pt-PT".to_string(),
            timezone: "Europe/Lisbon".to_string(),
            profile: "default".to_string(),
            context: "session-A".to_string(),
            session_mode: "isolated".to_string(),
        };
        match field {
            "endpoint" => k.endpoint = value.to_string(),
            "seed" => k.seed = value.to_string(),
            "proxy" => k.proxy = value.to_string(),
            "locale" => k.locale = value.to_string(),
            "timezone" => k.timezone = value.to_string(),
            "profile" => k.profile = value.to_string(),
            "context" => k.context = value.to_string(),
            "session_mode" => k.session_mode = value.to_string(),
            "" => {}
            other => panic!("unknown field {other}"),
        }
        k
    }

    #[test]
    fn cache_returns_inserted_value() {
        let cache = CalibrationCache::new();
        let k = key("", "");
        assert!(cache.get(&k).is_none());
        let fp = parse_probe(sample_probe_json()).unwrap();
        let arc = cache.insert(k.clone(), fp.clone());
        assert_eq!(arc.locale, "pt-PT");
        let got = cache.get(&k).unwrap();
        assert_eq!(*got, fp);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_key_invalidates_on_each_identity_field() {
        let base = key("", "");
        let fp = parse_probe(sample_probe_json()).unwrap();
        let cache = CalibrationCache::new();
        cache.insert(base.clone(), fp.clone());
        // Each identity field must produce a distinct cache slot —
        // otherwise we'd serve a fingerprint that no longer matches
        // the live identity.
        for field in [
            "endpoint",
            "seed",
            "proxy",
            "locale",
            "timezone",
            "profile",
            "context",
            "session_mode",
        ] {
            let mutated = key(field, "MUTATED-VALUE");
            assert_ne!(
                base, mutated,
                "changing `{field}` must change the cache key"
            );
            assert!(
                cache.get(&mutated).is_none(),
                "cache hit for `{field}`-mutated key — key did not invalidate"
            );
        }
    }

    #[test]
    fn cache_key_hash_is_stable_and_distinct_per_change() {
        let a = key("", "");
        let b = key("", ""); // identical
        assert_eq!(a.fingerprint_id(), b.fingerprint_id());
        for field in [
            "endpoint",
            "seed",
            "proxy",
            "locale",
            "timezone",
            "profile",
            "context",
            "session_mode",
        ] {
            let mutated = key(field, "X");
            assert_ne!(
                a.fingerprint_id(),
                mutated.fingerprint_id(),
                "fingerprint_id collision on `{field}` mutation"
            );
        }
    }

    #[test]
    fn cache_key_session_mode_isolated_and_persistent_are_distinct() {
        // Slice 35 acceptance: the calibration cache must not serve an
        // isolated-session fingerprint for a persistent-mode render
        // (or vice versa) — the underlying storage surface is
        // observably different.
        let mut iso = key("", "");
        iso.session_mode = "isolated".to_string();
        let mut pers = iso.clone();
        pers.session_mode = "persistent".to_string();
        assert_ne!(iso, pers, "session_mode must not collapse cache slots");
        assert_ne!(
            iso.fingerprint_id(),
            pers.fingerprint_id(),
            "fingerprint_id must change with session_mode"
        );

        let cache = CalibrationCache::new();
        let fp = parse_probe(sample_probe_json()).unwrap();
        cache.insert(iso.clone(), fp.clone());
        assert!(cache.get(&iso).is_some(), "isolated lookup must hit");
        assert!(
            cache.get(&pers).is_none(),
            "persistent lookup must miss the isolated slot"
        );
    }

    #[test]
    fn cache_key_field_separator_prevents_concatenation_collision() {
        // Without a separator, `("ab", "c")` and `("a", "bc")` would
        // hash identically. Verify the `\x1f` separator actually
        // distinguishes them.
        let a = CalibrationKey {
            endpoint: "ab".to_string(),
            seed: "c".to_string(),
            ..Default::default()
        };
        let b = CalibrationKey {
            endpoint: "a".to_string(),
            seed: "bc".to_string(),
            ..Default::default()
        };
        assert_ne!(a.fingerprint_id(), b.fingerprint_id());
    }

    #[test]
    fn full_report_contains_all_top_level_fields() {
        let fp = parse_probe(sample_probe_json()).unwrap();
        let report = CalibrationCache::format_full_report(&fp);
        for needle in [
            "\"browser_product\"",
            "\"timezone\"",
            "\"webgl\"",
            "\"webrtc\"",
            "\"webgpu_adapter\"",
            "\"performance_memory\"",
        ] {
            assert!(report.contains(needle), "report missing {needle}");
        }
    }

    #[test]
    fn calibration_probe_js_has_expected_surface_areas() {
        // The JS probe is a const include — assert it covers every
        // capture the slice 32 spec lists. This is a contract check on
        // the file, not a runtime check.
        let js = CALIBRATION_PROBE_JS;
        for needle in [
            "navigator",
            "screen",
            "Intl.DateTimeFormat",
            "WEBGL_debug_renderer_info",
            "OfflineAudioContext",
            "navigator.storage",
            "mediaDevices",
            "RTCPeerConnection",
            "navigator.permissions",
            "navigator.plugins",
            "window.chrome",
            "performance.memory",
            "navigator.gpu",
        ] {
            assert!(
                js.contains(needle),
                "calibration probe missing surface `{needle}`"
            );
        }
    }

    #[test]
    fn calibration_path_is_origin_marker() {
        assert_eq!(CALIBRATION_PATH, "/__crawlex_calibrate");
    }

    #[tokio::test]
    async fn local_origin_serves_calibration_html() {
        let origin = serve_calibration_origin().await.unwrap();
        let url = origin.calibrate_url();
        assert!(url.contains("/__crawlex_calibrate"));
        assert!(url.starts_with("http://127.0.0.1:"));
        let body = reqwest::get(&url).await.unwrap().text().await.unwrap();
        assert!(body.contains("__crawlex_calibrate"));
        assert!(body.contains("__crawlex_calibrate_ready"));
    }

    fn base_fp() -> EffectiveFingerprint {
        parse_probe(sample_probe_json()).unwrap()
    }

    #[test]
    fn policy_parse_round_trips() {
        assert_eq!(MismatchPolicy::parse("adapt"), Some(MismatchPolicy::Adapt));
        assert_eq!(MismatchPolicy::parse("Strict"), Some(MismatchPolicy::Strict));
        assert_eq!(MismatchPolicy::parse("nope"), None);
        assert_eq!(MismatchPolicy::default(), MismatchPolicy::Adapt);
        assert_eq!(MismatchPolicy::Strict.as_str(), "strict");
    }

    #[test]
    fn classify_no_expectations_returns_empty() {
        let fp = base_fp();
        assert!(classify_mismatches(&fp, &ExpectedIdentity::default()).is_empty());
    }

    #[test]
    fn classify_locale_and_timezone_flagged_critical_reconcilable() {
        let fp = base_fp();
        let exp = ExpectedIdentity {
            locale: Some("en-US".to_string()),
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert_eq!(m.len(), 2);
        for entry in &m {
            assert_eq!(entry.severity, MismatchSeverity::Critical);
            assert!(entry.reconcilable);
        }
        // Strict can proceed because both are reconcilable by the shim.
        assert!(!has_unreconciled_critical(&m));
    }

    #[test]
    fn classify_browser_family_critical_not_reconcilable() {
        let fp = base_fp();
        let exp = ExpectedIdentity {
            browser_family: Some("Firefox".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category, MismatchCategory::BrowserFamily);
        assert!(!m[0].reconcilable);
        assert!(has_unreconciled_critical(&m));
    }

    #[test]
    fn classify_browser_major_reconcilable() {
        let fp = base_fp();
        let exp = ExpectedIdentity {
            browser_major: Some("120".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category, MismatchCategory::BrowserVersion);
        assert!(m[0].reconcilable);
        assert!(!has_unreconciled_critical(&m));
    }

    #[test]
    fn classify_platform_reconcilable() {
        let fp = base_fp();
        let exp = ExpectedIdentity {
            platform: Some("Windows".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category, MismatchCategory::Platform);
        assert!(m[0].reconcilable);
    }

    #[test]
    fn classify_proxy_ip_leak_flagged_unreconcilable() {
        // Sample probe leaks 192.0.2.1; expected egress is 203.0.113.5.
        let fp = base_fp();
        let exp = ExpectedIdentity {
            proxy_egress_ipv4: Some("203.0.113.5".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category, MismatchCategory::ProxyIpWebrtc);
        assert!(!m[0].reconcilable);
        assert!(has_unreconciled_critical(&m));
    }

    #[test]
    fn classify_proxy_ip_private_address_is_not_a_leak() {
        let mut fp = base_fp();
        fp.webrtc.ipv4 = vec!["10.0.0.5".to_string(), "192.168.1.7".to_string()];
        let exp = ExpectedIdentity {
            proxy_egress_ipv4: Some("203.0.113.5".to_string()),
            ..Default::default()
        };
        assert!(classify_mismatches(&fp, &exp).is_empty());
    }

    #[test]
    fn classify_storage_quota_below_minimum_is_unreconcilable() {
        let mut fp = base_fp();
        fp.storage_quota = 100;
        let exp = ExpectedIdentity {
            min_storage_quota: Some(1_000_000),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category, MismatchCategory::StorageProfile);
        assert!(!m[0].reconcilable);
    }

    #[test]
    fn classify_profile_id_contradiction_when_storage_zero() {
        let mut fp = base_fp();
        fp.storage_quota = 0;
        let exp = ExpectedIdentity {
            profile_id: Some("warm-profile-A".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        assert!(m.iter().any(|x| x.category == MismatchCategory::StorageProfile));
    }

    #[test]
    fn mismatch_report_aggregates_counts_and_categories() {
        let fp = base_fp();
        let exp = ExpectedIdentity {
            browser_family: Some("Firefox".to_string()),
            locale: Some("en-US".to_string()),
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        let r = MismatchReport::new(MismatchPolicy::Adapt, &m);
        assert_eq!(r.policy, "adapt");
        assert_eq!(r.critical, 3);
        assert_eq!(r.non_critical, 0);
        assert_eq!(r.unreconciled_critical, 1);
        assert!(r.categories.contains(&"browser_family"));
        assert!(r.categories.contains(&"locale"));
        assert!(r.categories.contains(&"timezone"));
        // Serializes without dumping the full fingerprint.
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("canvas_hash"));
        assert!(!json.contains("user_agent"));
        assert!(json.contains("\"unreconciled_critical\":1"));
    }

    #[test]
    fn non_critical_does_not_trigger_strict_failure() {
        // Hand-roll a NonCritical+unreconcilable entry — strict policy
        // must NOT abort on these per AC.
        let m = vec![Mismatch {
            category: MismatchCategory::GpuClass,
            severity: MismatchSeverity::NonCritical,
            expected: "intel".into(),
            observed: "nvidia".into(),
            reconcilable: false,
        }];
        assert!(!has_unreconciled_critical(&m));
        let r = MismatchReport::new(MismatchPolicy::Strict, &m);
        assert_eq!(r.unreconciled_critical, 0);
        assert_eq!(r.non_critical, 1);
    }

    #[test]
    fn adapt_does_not_flag_strict_when_only_reconcilable_critical() {
        let fp = base_fp();
        let exp = ExpectedIdentity {
            locale: Some("en-US".to_string()),
            timezone: Some("UTC".to_string()),
            platform: Some("Windows".to_string()),
            browser_major: Some("120".to_string()),
            ..Default::default()
        };
        let m = classify_mismatches(&fp, &exp);
        let r = MismatchReport::new(MismatchPolicy::Strict, &m);
        assert!(r.critical >= 4);
        assert_eq!(r.unreconciled_critical, 0);
        assert!(!has_unreconciled_critical(&m));
    }

    #[test]
    fn summary_event_serializes_compactly() {
        let fp = parse_probe(sample_probe_json()).unwrap();
        let summary = CalibrationSummary {
            browser_product: &fp.browser_product,
            platform: &fp.platform,
            locale: &fp.locale,
            timezone: &fp.timezone,
            webgl_renderer: &fp.webgl.unmasked_renderer,
            mismatch_count: 0,
            policy: &fp.policy,
        };
        let json = serde_json::to_string(&summary).unwrap();
        // Summary must NOT carry the full fingerprint surface.
        assert!(json.contains("\"webgl_renderer\""));
        assert!(!json.contains("canvas_hash"));
        assert!(!json.contains("media_devices"));
        assert!(!json.contains("webrtc"));
    }
}
