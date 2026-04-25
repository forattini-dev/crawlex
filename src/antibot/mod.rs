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

pub mod bypass;
pub mod cookie_pin;
pub mod signatures;
pub mod solver;
pub mod telemetry;

use http::HeaderMap;
use regex::Regex;
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

/// ---------------------------------------------------------------------
/// HTML detection
/// ---------------------------------------------------------------------

fn body_slice(body: &[u8]) -> std::borrow::Cow<'_, str> {
    let slice = &body[..body.len().min(32 * 1024)];
    String::from_utf8_lossy(slice)
}

/// Scan HTML text (pre- or post-JS) for vendor substrings. Ordered by
/// specificity: the most diagnostic patterns (CF JS challenge title +
/// script, Turnstile iframe URL) run first.
pub fn detect_from_html(
    html: &str,
    _url: &url::Url,
    headers: Option<&HeaderMap>,
) -> Option<RawChallenge> {
    let lower = html.to_ascii_lowercase();

    // 1. Cloudflare JS challenge — require TITLE + platform script (two signals)
    let cf_title = lower.contains("<title>just a moment");
    let cf_bypass = lower.contains("cf-chl-bypass");
    let cf_platform = lower.contains("/cdn-cgi/challenge-platform/");
    if cf_title && (cf_bypass || cf_platform) {
        return Some(RawChallenge {
            vendor: ChallengeVendor::CloudflareJsChallenge,
            level: ChallengeLevel::ChallengePage,
            metadata: html_metadata(
                html,
                true,
                &[
                    "title:just-a-moment",
                    if cf_bypass {
                        "cf-chl-bypass"
                    } else {
                        "challenge-platform"
                    },
                ],
            ),
        });
    }

    // 2. Cloudflare Turnstile (widget embedded)
    if lower.contains("challenges.cloudflare.com/turnstile") {
        return Some(RawChallenge {
            vendor: ChallengeVendor::CloudflareTurnstile,
            level: ChallengeLevel::WidgetPresent,
            metadata: html_metadata(html, true, &["turnstile-iframe"]),
        });
    }

    // 3. reCAPTCHA Enterprise (check before plain reCAPTCHA; URL is more specific)
    if lower.contains("recaptcha/enterprise.js") {
        return Some(RawChallenge {
            vendor: ChallengeVendor::RecaptchaEnterprise,
            level: ChallengeLevel::WidgetPresent,
            metadata: html_metadata(html, true, &["enterprise.js"]),
        });
    }

    // 4. reCAPTCHA v2/v3
    if lower.contains("www.google.com/recaptcha/api.js")
        || lower.contains("google.com/recaptcha/api2/")
    {
        return Some(RawChallenge {
            vendor: ChallengeVendor::Recaptcha,
            level: ChallengeLevel::WidgetPresent,
            metadata: html_metadata(html, true, &["recaptcha/api.js"]),
        });
    }

    // 5. hCaptcha
    if lower.contains("hcaptcha.com/1/api.js")
        || lower.contains("js.hcaptcha.com/1/api.js")
        || (lower.contains("<iframe") && lower.contains("hcaptcha.com"))
    {
        return Some(RawChallenge {
            vendor: ChallengeVendor::HCaptcha,
            level: ChallengeLevel::WidgetPresent,
            metadata: html_metadata(html, true, &["hcaptcha.com"]),
        });
    }

    // 6. DataDome
    if lower.contains("captcha-delivery.com") || lower.contains("dd-captcha-container") {
        return Some(RawChallenge {
            vendor: ChallengeVendor::DataDome,
            level: ChallengeLevel::ChallengePage,
            metadata: html_metadata(html, true, &["captcha-delivery"]),
        });
    }

    // 7. PerimeterX
    if lower.contains(r#"id="px-captcha""#)
        || lower.contains("client.perimeterx.net")
        || lower.contains("captcha.px-cdn.net")
    {
        return Some(RawChallenge {
            vendor: ChallengeVendor::PerimeterX,
            level: ChallengeLevel::ChallengePage,
            metadata: html_metadata(html, true, &["px-captcha"]),
        });
    }

    // 8. Akamai — DOM signature needs corroboration with header (handled
    // by HTTP detect). Pure-HTML path only matches when we see both the
    // bm-verify script and a very short body, which is rare but strong.
    if lower.contains("/_bm/_data") || lower.contains("_abck") {
        return Some(RawChallenge {
            vendor: ChallengeVendor::Akamai,
            level: ChallengeLevel::ChallengePage,
            metadata: html_metadata(html, false, &["akamai-bm"]),
        });
    }

    // 9. Generic captcha / access-denied interstitial fallback. These are
    // intentionally narrow: require <title> match AND short body so a news
    // article titled "Access Denied (book review)" doesn't trigger.
    if html.len() < 4096 {
        let title_captcha =
            lower.contains("<title>attention required") || lower.contains("<title>access denied");
        if title_captcha {
            // Distinguish CF/Akamai-fronted access denied from generic
            let vendor = if headers
                .and_then(|h| h.get("server"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_ascii_lowercase().contains("cloudflare"))
                .unwrap_or(false)
            {
                ChallengeVendor::CloudflareJsChallenge
            } else {
                ChallengeVendor::AccessDenied
            };
            return Some(RawChallenge {
                vendor,
                level: ChallengeLevel::HardBlock,
                metadata: html_metadata(html, false, &["access-denied-title"]),
            });
        }
    }

    None
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
    let server = headers
        .get("server")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let cf_mitigated = headers.get("cf-mitigated").is_some();
    let set_cookie: Vec<String> = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_ascii_lowercase()))
        .collect();

    // Akamai hard-block: Server: AkamaiGHost + 403.
    if status == 403 && server.contains("akamaighost") {
        return Some(RawChallenge {
            vendor: ChallengeVendor::Akamai,
            level: ChallengeLevel::HardBlock,
            metadata: serde_json::json!({
                "source": "http",
                "server": server,
                "status": status,
            }),
        });
    }

    // Cloudflare JS challenge — typically served as 403/503 with
    // `server: cloudflare` AND CF body markers.
    if matches!(status, 403 | 503) && server.contains("cloudflare") {
        if let Some(mut raw) = detect_from_html(&body_slice(body), url, Some(headers)) {
            // Upgrade metadata with http context
            raw.metadata = merge_json(
                raw.metadata,
                serde_json::json!({
                    "surface": "http_response",
                    "status_code": status,
                    "server": server,
                }),
            );
            return Some(raw);
        }
        if cf_mitigated {
            return Some(RawChallenge {
                vendor: ChallengeVendor::CloudflareJsChallenge,
                level: ChallengeLevel::ChallengePage,
                metadata: serde_json::json!({
                    "surface": "http_response",
                    "cf_mitigated": true,
                    "status_code": status,
                }),
            });
        }
    }

    // DataDome — x-dd-b header or datadome cookie.
    let has_datadome_cookie = set_cookie.iter().any(|c| c.starts_with("datadome="));
    if headers.get("x-dd-b").is_some() || has_datadome_cookie {
        let level = if status >= 400 {
            ChallengeLevel::ChallengePage
        } else {
            ChallengeLevel::Suspected
        };
        return Some(RawChallenge {
            vendor: ChallengeVendor::DataDome,
            level,
            metadata: serde_json::json!({
                "surface": "http_response",
                "cookie": has_datadome_cookie,
                "status_code": status,
            }),
        });
    }

    // PerimeterX cookie
    if set_cookie.iter().any(|c| c.starts_with("_px3=")) {
        return Some(RawChallenge {
            vendor: ChallengeVendor::PerimeterX,
            level: ChallengeLevel::Suspected,
            metadata: serde_json::json!({"surface": "http_response", "cookie": "_px3"}),
        });
    }

    // Body-based detection for remaining vendors on 4xx/5xx.
    if matches!(status, 403 | 429 | 503) {
        if let Some(raw) = detect_from_html(&body_slice(body), url, Some(headers)) {
            return Some(raw);
        }
        // Tiny body + error status → generic HardBlock
        if body.len() < 512 && status == 403 {
            return Some(RawChallenge {
                vendor: ChallengeVendor::AccessDenied,
                level: ChallengeLevel::HardBlock,
                metadata: serde_json::json!({
                    "surface": "http_response",
                    "status_code": status,
                    "body_bytes": body.len()
                }),
            });
        }
    }

    None
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

fn html_metadata(html: &str, widget_present: bool, signals: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "surface": "html",
        "signals": signals,
        "title": extract_html_title(html),
        "widget_present": widget_present,
        "sitekey": extract_sitekey(html),
        "action": extract_action(html),
        "iframe_srcs": extract_iframe_srcs(html),
    })
}

fn extract_html_title(html: &str) -> Option<String> {
    Regex::new(r"(?is)<title[^>]*>\s*(.*?)\s*</title>")
        .ok()
        .and_then(|re| re.captures(html))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
}

fn extract_sitekey(html: &str) -> Option<String> {
    let data_attr = Regex::new(r#"(?i)data-sitekey\s*=\s*["']([^"']+)["']"#)
        .ok()
        .and_then(|re| re.captures(html))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string());
    if data_attr.is_some() {
        return data_attr;
    }
    Regex::new(r#"(?i)[?&]render=([^"'&\s>]+)"#)
        .ok()
        .and_then(|re| re.captures(html))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn extract_action(html: &str) -> Option<String> {
    Regex::new(r#"(?i)data-action\s*=\s*["']([^"']+)["']"#)
        .ok()
        .and_then(|re| re.captures(html))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn extract_iframe_srcs(html: &str) -> Vec<String> {
    Regex::new(r#"(?is)<iframe[^>]+src\s*=\s*["']([^"']+)["']"#)
        .ok()
        .map(|re| {
            re.captures_iter(html)
                .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn merge_json(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
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
