//! Phase 1 antibot/stealth module — rich vendor detection, per-session
//! `ChallengeState`, and pure `detect_from_*` functions.
//!
//! Design goals:
//! * **Pure** — no IO, no async, no feature gates. Callable from both the
//!   mini (HTTP-only) build and the full CDP build.
//! * **Conservative** — substring/regex checks ordered most-specific-first,
//!   require multiple co-occurrences for the common false-positive vendors
//!   (Cloudflare "Just a moment" appears in news articles; Akamai headers
//!   are everywhere). Prefer false-negative over false-positive on crawl.
//! * **Extensible** — new vendors plug in at the `VENDOR_HTML_RULES` table
//!   without touching detect-fn control flow.
//!
//! Legacy `crate::escalation::detect_antibot_vendor` is kept for backward
//! compatibility with the existing policy engine; this module is the richer
//! path that provides `ChallengeSignal` (vendor + level + url + metadata)
//! consumed by `policy::decide_post_challenge` + SQLite telemetry.

pub mod block_detector;
pub mod bypass;
pub mod cookie_pin;
pub mod recaptcha;
pub mod signatures;
pub mod solver;
pub mod telemetry;

use http::HeaderMap;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Severity bucket for a detected antibot signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeLevel {
    /// Weak signal (rate-limit header, isolated cookie). Worth rotating
    /// proxy but the page may still be usable.
    Suspected,
    /// Full interstitial — CF JS challenge, DataDome captcha page, etc.
    ChallengePage,
    /// A captcha widget is embedded in an otherwise-loaded page.
    WidgetPresent,
    /// 403/429 definitive, vendor identified, body unrecoverable.
    HardBlock,
}

impl ChallengeLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Suspected => "suspected",
            Self::ChallengePage => "challenge_page",
            Self::WidgetPresent => "widget_present",
            Self::HardBlock => "hard_block",
        }
    }
}

/// Vendor identity. Narrower than `escalation::AntibotVendor` — splits CF JS
/// challenge from Turnstile, reCAPTCHA v2 from Enterprise, etc.
///
/// Superseded by [`crate::fingerprint::detection::Vendor`] (slice B7
/// of PRD forattini-dev/crawlex#25). Kept until B15 removes the
/// legacy antibot detection modules; `From<ChallengeVendor>` for
/// `Vendor` is provided for migration.
#[deprecated(
    since = "1.0.5",
    note = "use crate::fingerprint::detection::Vendor; ChallengeVendor is removed in B15"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeVendor {
    CloudflareJsChallenge,
    CloudflareTurnstile,
    Recaptcha,
    RecaptchaEnterprise,
    HCaptcha,
    DataDome,
    PerimeterX,
    Akamai,
    GenericCaptcha,
    AccessDenied,
}

#[allow(deprecated)]
impl ChallengeVendor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CloudflareJsChallenge => "cloudflare_js_challenge",
            Self::CloudflareTurnstile => "cloudflare_turnstile",
            Self::Recaptcha => "recaptcha",
            Self::RecaptchaEnterprise => "recaptcha_enterprise",
            Self::HCaptcha => "hcaptcha",
            Self::DataDome => "datadome",
            Self::PerimeterX => "perimeterx",
            Self::Akamai => "akamai",
            Self::GenericCaptcha => "generic_captcha",
            Self::AccessDenied => "access_denied",
        }
    }
}

/// Lossless conversion to the consolidated `fingerprint::Vendor`.
/// Used during the deprecation window.
#[allow(deprecated)]
impl From<ChallengeVendor> for crate::fingerprint::detection::Vendor {
    fn from(v: ChallengeVendor) -> Self {
        use crate::fingerprint::detection::Vendor;
        match v {
            ChallengeVendor::CloudflareJsChallenge => Vendor::Cloudflare,
            ChallengeVendor::CloudflareTurnstile => Vendor::CloudflareTurnstile,
            ChallengeVendor::Recaptcha | ChallengeVendor::RecaptchaEnterprise => {
                Vendor::Recaptcha
            }
            ChallengeVendor::HCaptcha => Vendor::HCaptcha,
            ChallengeVendor::DataDome => Vendor::DataDome,
            ChallengeVendor::PerimeterX => Vendor::PerimeterX,
            ChallengeVendor::Akamai => Vendor::AkamaiBotManager,
            ChallengeVendor::GenericCaptcha | ChallengeVendor::AccessDenied => Vendor::Unknown,
        }
    }
}

/// Partial challenge record returned by the pure `detect_*` functions.
/// Caller enriches with session_id / proxy / url / origin / first_seen.
#[derive(Debug, Clone)]
pub struct RawChallenge {
    pub vendor: ChallengeVendor,
    pub level: ChallengeLevel,
    pub metadata: serde_json::Value,
}

impl RawChallenge {
    pub fn into_signal(
        self,
        url: &url::Url,
        session_id: String,
        proxy: Option<url::Url>,
    ) -> ChallengeSignal {
        let origin = origin_of(url);
        ChallengeSignal {
            vendor: self.vendor,
            level: self.level,
            url: url.clone(),
            origin,
            proxy,
            session_id,
            first_seen: SystemTime::now(),
            metadata: self.metadata,
        }
    }
}

/// Full challenge signal — what we persist, what we route to
/// `decide_post_challenge`, what we feed the router as `ChallengeHit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengeSignal {
    pub vendor: ChallengeVendor,
    pub level: ChallengeLevel,
    pub url: url::Url,
    pub origin: String,
    pub proxy: Option<url::Url>,
    pub session_id: String,
    #[serde(with = "system_time_serde")]
    pub first_seen: SystemTime,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Session-scoped contamination state. Policy uses this + the signal level
/// to decide whether to rotate proxy, kill the browser context, or give up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    #[default]
    Clean,
    Warm,
    Contaminated,
    Blocked,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Warm => "warm",
            Self::Contaminated => "contaminated",
            Self::Blocked => "blocked",
        }
    }
    /// Monotonic transition — a Blocked session never downgrades to Clean
    /// from challenge observations alone (operator / resume is needed).
    pub fn after_challenge(self, level: ChallengeLevel) -> SessionState {
        match (self, level) {
            (_, ChallengeLevel::HardBlock) => SessionState::Blocked,
            (SessionState::Blocked, _) => SessionState::Blocked,
            (SessionState::Contaminated, _) => SessionState::Contaminated,
            (_, ChallengeLevel::ChallengePage | ChallengeLevel::WidgetPresent) => {
                SessionState::Contaminated
            }
            (_, ChallengeLevel::Suspected) => SessionState::Warm,
        }
    }
}

/// Canonical origin string used as the pin-store key. Kept `pub` so
/// callers on the HTTP and render paths can build matching origins
/// before querying [`cookie_pin::CookiePinStore`].
pub fn origin_of_url(url: &url::Url) -> String {
    origin_of(url)
}

fn origin_of(url: &url::Url) -> String {
    match (url.scheme(), url.host_str(), url.port()) {
        (s, Some(h), Some(p)) => format!("{s}://{h}:{p}"),
        (s, Some(h), None) => format!("{s}://{h}"),
        _ => url.to_string(),
    }
}

/// Scan HTML text (pre- or post-JS) for vendor substrings. Ordered by
/// specificity: the most diagnostic patterns (CF JS challenge title +
/// script, Turnstile iframe URL) run first.
pub fn detect_from_html(
    html: &str,
    url: &url::Url,
    headers: Option<&HeaderMap>,
) -> Option<RawChallenge> {
    block_detector::classify_html(html, url, headers).into_raw_challenge()
}

/// ---------------------------------------------------------------------
/// HTTP-response detection (combines status + headers + body)
/// ---------------------------------------------------------------------

pub fn detect_from_http_response(
    status: u16,
    body: &[u8],
    headers: &HeaderMap,
    url: &url::Url,
) -> Option<RawChallenge> {
    block_detector::classify_http_response(status, body, headers, url).into_raw_challenge()
}

/// ---------------------------------------------------------------------
/// Cookie-name detection (called from render path when DOM was clean but
/// we picked up a shady cookie during load).
/// ---------------------------------------------------------------------

pub fn detect_from_cookies(cookie_names: &[&str]) -> Option<RawChallenge> {
    for name in cookie_names {
        let lower = name.to_ascii_lowercase();
        if lower == "datadome" {
            return Some(RawChallenge {
                vendor: ChallengeVendor::DataDome,
                level: ChallengeLevel::Suspected,
                metadata: serde_json::json!({"surface": "cookies", "cookie": "datadome"}),
            });
        }
        if lower == "_px3" || lower == "_pxvid" {
            return Some(RawChallenge {
                vendor: ChallengeVendor::PerimeterX,
                level: ChallengeLevel::Suspected,
                metadata: serde_json::json!({"surface": "cookies", "cookie": lower}),
            });
        }
    }
    None
}

/// SystemTime serde helper — serialize as unix millis.
pub(crate) mod system_time_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let ms = t
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        ms.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let ms = i64::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url() -> url::Url {
        url::Url::parse("https://example.com/").unwrap()
    }

    #[test]
    fn innocent_html_matches_nothing() {
        let html = "<html><head><title>Wikipedia article</title></head><body>content</body></html>";
        assert!(detect_from_html(html, &url(), None).is_none());
    }

    #[test]
    fn datadome_cookie_is_suspected() {
        let raw = detect_from_cookies(&["datadome"]).unwrap();
        assert_eq!(raw.vendor, ChallengeVendor::DataDome);
        assert_eq!(raw.level, ChallengeLevel::Suspected);
    }

    #[test]
    fn session_state_progression_monotonic() {
        let s = SessionState::Clean;
        let s = s.after_challenge(ChallengeLevel::Suspected);
        assert_eq!(s, SessionState::Warm);
        let s = s.after_challenge(ChallengeLevel::ChallengePage);
        assert_eq!(s, SessionState::Contaminated);
        let s = s.after_challenge(ChallengeLevel::HardBlock);
        assert_eq!(s, SessionState::Blocked);
        let s = s.after_challenge(ChallengeLevel::Suspected);
        assert_eq!(s, SessionState::Blocked, "blocked is sticky");
    }
}
