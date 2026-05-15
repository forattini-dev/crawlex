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
        };
        match field {
            "endpoint" => k.endpoint = value.to_string(),
            "seed" => k.seed = value.to_string(),
            "proxy" => k.proxy = value.to_string(),
            "locale" => k.locale = value.to_string(),
            "timezone" => k.timezone = value.to_string(),
            "profile" => k.profile = value.to_string(),
            "context" => k.context = value.to_string(),
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
        // Each of the seven identity fields must produce a distinct
        // cache slot — otherwise we'd serve a fingerprint that no
        // longer matches the live identity.
        for field in [
            "endpoint", "seed", "proxy", "locale", "timezone", "profile", "context",
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
            "endpoint", "seed", "proxy", "locale", "timezone", "profile", "context",
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
