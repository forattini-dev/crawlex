use http::HeaderMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{ChallengeLevel, ChallengeVendor, RawChallenge};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockSurface {
    Html,
    HttpResponse,
    Structural,
    StatusCode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockClassification {
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<ChallengeVendor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<ChallengeLevel>,
    pub confidence: BlockConfidence,
    pub reason: String,
    pub surface: BlockSurface,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl BlockClassification {
    pub fn clean() -> Self {
        Self {
            blocked: false,
            vendor: None,
            level: None,
            confidence: BlockConfidence::Low,
            reason: String::new(),
            surface: BlockSurface::Html,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn into_raw_challenge(self) -> Option<RawChallenge> {
        if !self.blocked {
            return None;
        }
        Some(RawChallenge {
            vendor: self.vendor.unwrap_or(ChallengeVendor::AccessDenied),
            level: self.level.unwrap_or(ChallengeLevel::Suspected),
            metadata: self.metadata,
        })
    }
}

pub fn classify_html(
    html: &str,
    url: &url::Url,
    headers: Option<&HeaderMap>,
) -> BlockClassification {
    classify_parts(None, html, headers, url, BlockSurface::Html)
}

pub fn classify_http_response(
    status: u16,
    body: &[u8],
    headers: &HeaderMap,
    url: &url::Url,
) -> BlockClassification {
    let html = String::from_utf8_lossy(body);
    classify_parts(
        Some(status),
        &html,
        Some(headers),
        url,
        BlockSurface::HttpResponse,
    )
}

pub fn classify_parts(
    status: Option<u16>,
    html: &str,
    headers: Option<&HeaderMap>,
    _url: &url::Url,
    surface: BlockSurface,
) -> BlockClassification {
    let lower = html.to_ascii_lowercase();
    let html_len = html.len();
    let server = header_lower(headers, "server");
    let set_cookie = headers
        .map(|h| {
            h.get_all("set-cookie")
                .iter()
                .filter_map(|v| v.to_str().ok().map(|s| s.to_ascii_lowercase()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if status == Some(429) {
        return blocked(
            ChallengeVendor::AccessDenied,
            ChallengeLevel::HardBlock,
            BlockConfidence::High,
            "HTTP 429 Too Many Requests",
            BlockSurface::StatusCode,
            metadata(html, status, headers, false, &["http-429"]),
        );
    }

    if status == Some(403) && server.contains("akamaighost") {
        return blocked(
            ChallengeVendor::Akamai,
            ChallengeLevel::HardBlock,
            BlockConfidence::High,
            "Akamai hard block",
            BlockSurface::HttpResponse,
            metadata(html, status, headers, false, &["akamaighost-403"]),
        );
    }

    if headers.is_some_and(|h| h.get("x-dd-b").is_some())
        || set_cookie.iter().any(|c| c.starts_with("datadome="))
    {
        return blocked(
            ChallengeVendor::DataDome,
            if status.is_some_and(|s| s >= 400) {
                ChallengeLevel::ChallengePage
            } else {
                ChallengeLevel::Suspected
            },
            BlockConfidence::High,
            "DataDome response/cookie signal",
            BlockSurface::HttpResponse,
            metadata(html, status, headers, true, &["datadome-header-or-cookie"]),
        );
    }

    if set_cookie.iter().any(|c| c.starts_with("_px3=")) {
        return blocked(
            ChallengeVendor::PerimeterX,
            ChallengeLevel::Suspected,
            BlockConfidence::Medium,
            "PerimeterX _px3 cookie",
            BlockSurface::HttpResponse,
            metadata(html, status, headers, false, &["_px3-cookie"]),
        );
    }

    for (pattern, vendor, level, reason, signal) in tier1_patterns() {
        if pattern.is_match(&lower) {
            return blocked(
                vendor,
                level,
                BlockConfidence::High,
                reason,
                surface,
                metadata(
                    html,
                    status,
                    headers,
                    level != ChallengeLevel::HardBlock,
                    &[signal],
                ),
            );
        }
    }

    if html_len > 15_000 {
        let stripped = strip_scripts_styles(&lower[..lower.len().min(500_000)]);
        for (pattern, vendor, level, reason, signal) in tier1_patterns() {
            if pattern.is_match(&stripped) {
                return blocked(
                    vendor,
                    level,
                    BlockConfidence::High,
                    reason,
                    surface,
                    metadata(
                        html,
                        status,
                        headers,
                        level != ChallengeLevel::HardBlock,
                        &[signal],
                    ),
                );
            }
        }
    }

    if let Some(status) = status {
        if matches!(status, 403 | 503) && !looks_like_data(html) {
            let snippet = if html_len > 10_000 {
                strip_scripts_styles(&lower[..lower.len().min(500_000)])
            } else {
                lower.clone()
            };
            for (pattern, vendor, level, reason, signal) in tier2_patterns() {
                if pattern.is_match(&snippet) {
                    return blocked(
                        vendor,
                        level,
                        BlockConfidence::High,
                        &format!("{reason} (HTTP {status})"),
                        BlockSurface::HttpResponse,
                        metadata(html, Some(status), headers, true, &[signal]),
                    );
                }
            }
            return blocked(
                ChallengeVendor::AccessDenied,
                ChallengeLevel::HardBlock,
                BlockConfidence::High,
                &format!("HTTP {status} non-data HTML response"),
                BlockSurface::StatusCode,
                metadata(html, Some(status), headers, false, &["http-block-status"]),
            );
        }
    }

    if html_len < 10_000 {
        for (pattern, vendor, level, reason, signal) in tier2_patterns() {
            if pattern.is_match(&lower) {
                return blocked(
                    vendor,
                    level,
                    BlockConfidence::Medium,
                    reason,
                    surface,
                    metadata(html, status, headers, true, &[signal]),
                );
            }
        }
    }

    if status.is_none() || status.is_some_and(|s| s >= 400) {
        if let Some(reason) = structural_integrity_reason(html) {
            return blocked(
                ChallengeVendor::AccessDenied,
                ChallengeLevel::ChallengePage,
                BlockConfidence::Medium,
                &reason,
                BlockSurface::Structural,
                metadata(html, status, headers, false, &["structural-integrity"]),
            );
        }
    }

    BlockClassification::clean()
}

fn blocked(
    vendor: ChallengeVendor,
    level: ChallengeLevel,
    confidence: BlockConfidence,
    reason: &str,
    surface: BlockSurface,
    metadata: serde_json::Value,
) -> BlockClassification {
    BlockClassification {
        blocked: true,
        vendor: Some(vendor),
        level: Some(level),
        confidence,
        reason: reason.to_string(),
        surface,
        metadata,
    }
}

fn header_lower(headers: Option<&HeaderMap>, name: &str) -> String {
    headers
        .and_then(|h| h.get(name))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

fn tier1_patterns() -> Vec<(
    Regex,
    ChallengeVendor,
    ChallengeLevel,
    &'static str,
    &'static str,
)> {
    vec![
        (
            Regex::new(r"(reference\s*(?:#|&#35;)\s*[\w.-]+|/_bm/_data|/akam/|_abck)").unwrap(),
            ChallengeVendor::Akamai,
            ChallengeLevel::HardBlock,
            "Akamai block reference",
            "akamai-reference",
        ),
        (
            Regex::new(r"pardon\s+our\s+interruption").unwrap(),
            ChallengeVendor::Akamai,
            ChallengeLevel::ChallengePage,
            "Akamai challenge page",
            "akamai-pardon",
        ),
        (
            Regex::new(r"challenge-form[\s\S]*__cf_chl_f_tk=").unwrap(),
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
            "Cloudflare challenge form",
            "cloudflare-challenge-form",
        ),
        (
            Regex::new(r#"<span\s+class=["']cf-error-code["']>\d{4}</span>"#).unwrap(),
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::HardBlock,
            "Cloudflare firewall block",
            "cloudflare-error-code",
        ),
        (
            Regex::new(r"/cdn-cgi/challenge-platform/\S*orchestrate").unwrap(),
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
            "Cloudflare JS challenge",
            "cloudflare-orchestrate",
        ),
        (
            Regex::new(r#"(window\._pxappid\s*=|id=["']px-captcha["']|client\.perimeterx\.net)"#)
                .unwrap(),
            ChallengeVendor::PerimeterX,
            ChallengeLevel::ChallengePage,
            "PerimeterX block",
            "perimeterx-appid",
        ),
        (
            Regex::new(r"captcha\.px-cdn\.net").unwrap(),
            ChallengeVendor::PerimeterX,
            ChallengeLevel::ChallengePage,
            "PerimeterX captcha",
            "perimeterx-captcha",
        ),
        (
            Regex::new(r"captcha-delivery\.com").unwrap(),
            ChallengeVendor::DataDome,
            ChallengeLevel::ChallengePage,
            "DataDome captcha",
            "datadome-captcha",
        ),
        (
            Regex::new(r"_incapsula_resource|incapsula\s+incident\s+id").unwrap(),
            ChallengeVendor::AccessDenied,
            ChallengeLevel::ChallengePage,
            "Imperva/Incapsula block",
            "imperva-incapsula",
        ),
        (
            Regex::new(r"sucuri\s+website\s+firewall").unwrap(),
            ChallengeVendor::AccessDenied,
            ChallengeLevel::HardBlock,
            "Sucuri firewall block",
            "sucuri-firewall",
        ),
        (
            Regex::new(r"kpsdk\.scriptstart\s*=\s*kpsdk\.now\(\)").unwrap(),
            ChallengeVendor::AccessDenied,
            ChallengeLevel::ChallengePage,
            "Kasada challenge",
            "kasada-kpsdk",
        ),
        (
            Regex::new(r"blocked\s+by\s+network\s+security").unwrap(),
            ChallengeVendor::AccessDenied,
            ChallengeLevel::HardBlock,
            "Network security block",
            "network-security-block",
        ),
        (
            Regex::new(r"challenges\.cloudflare\.com/turnstile").unwrap(),
            ChallengeVendor::CloudflareTurnstile,
            ChallengeLevel::WidgetPresent,
            "Cloudflare Turnstile widget",
            "turnstile-widget",
        ),
        (
            Regex::new(r"recaptcha/enterprise\.js").unwrap(),
            ChallengeVendor::RecaptchaEnterprise,
            ChallengeLevel::WidgetPresent,
            "reCAPTCHA Enterprise widget",
            "recaptcha-enterprise",
        ),
        (
            Regex::new(r"(google\.com/recaptcha/api\.js|google\.com/recaptcha/api2/)").unwrap(),
            ChallengeVendor::Recaptcha,
            ChallengeLevel::WidgetPresent,
            "reCAPTCHA widget",
            "recaptcha",
        ),
        (
            Regex::new(r"(hcaptcha\.com/1/api\.js|js\.hcaptcha\.com/1/api\.js)").unwrap(),
            ChallengeVendor::HCaptcha,
            ChallengeLevel::WidgetPresent,
            "hCaptcha widget",
            "hcaptcha",
        ),
    ]
}

fn tier2_patterns() -> Vec<(
    Regex,
    ChallengeVendor,
    ChallengeLevel,
    &'static str,
    &'static str,
)> {
    vec![
        (
            Regex::new(r"<title>\s*(access denied|attention required)|access\s+to\s+this\s+page\s+has\s+been\s+(blocked|denied)").unwrap(),
            ChallengeVendor::AccessDenied,
            ChallengeLevel::HardBlock,
            "Access Denied page",
            "access-denied",
        ),
        (
            Regex::new(r"checking\s+your\s+browser|<title>\s*just\s+a\s+moment").unwrap(),
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
            "Cloudflare browser check",
            "cloudflare-browser-check",
        ),
        (
            Regex::new(r#"class=["']g-recaptcha["']"#).unwrap(),
            ChallengeVendor::Recaptcha,
            ChallengeLevel::WidgetPresent,
            "reCAPTCHA block page",
            "g-recaptcha-class",
        ),
        (
            Regex::new(r#"class=["']h-captcha["']"#).unwrap(),
            ChallengeVendor::HCaptcha,
            ChallengeLevel::WidgetPresent,
            "hCaptcha block page",
            "h-captcha-class",
        ),
        (
            Regex::new(r"access\s+to\s+this\s+page\s+has\s+been\s+blocked").unwrap(),
            ChallengeVendor::PerimeterX,
            ChallengeLevel::ChallengePage,
            "PerimeterX block page",
            "perimeterx-block-title",
        ),
        (
            Regex::new(r"blocked\s+by\s+security|request\s+unsuccessful").unwrap(),
            ChallengeVendor::AccessDenied,
            ChallengeLevel::HardBlock,
            "Generic security block",
            "generic-security-block",
        ),
    ]
}

fn strip_scripts_styles(html: &str) -> String {
    let no_script = Regex::new(r"(?is)<script\b[\s\S]*?</script>")
        .unwrap()
        .replace_all(html, "");
    Regex::new(r"(?is)<style\b[\s\S]*?</style>")
        .unwrap()
        .replace_all(&no_script, "")
        .to_string()
}

fn looks_like_data(html: &str) -> bool {
    let stripped = html.trim_start();
    if stripped.is_empty() {
        return false;
    }
    if stripped.starts_with('{') || stripped.starts_with('[') {
        return true;
    }
    stripped.starts_with("<?xml")
}

fn structural_integrity_reason(html: &str) -> Option<String> {
    let len = html.len();
    if len > 50_000 || looks_like_data(html) || len == 0 {
        return None;
    }
    let lower = html.to_ascii_lowercase();
    if !lower.contains("<body") {
        return Some(format!("Structural: no <body> tag ({len} bytes)"));
    }
    let stripped = strip_scripts_styles(&lower);
    let visible = Regex::new(r"<[^>]+>")
        .unwrap()
        .replace_all(&stripped, "")
        .trim()
        .to_string();
    let visible_len = visible.len();
    let content_elements = Regex::new(r"<(?:p|h[1-6]|article|section|li|td|a|pre)\b")
        .unwrap()
        .find_iter(&lower)
        .count();
    let script_count = lower.matches("<script").count();
    let mut signals = Vec::new();
    if visible_len < 50 {
        signals.push("minimal_text");
    }
    if content_elements == 0 {
        signals.push("no_content_elements");
    }
    if script_count > 0 && content_elements == 0 && visible_len < 100 {
        signals.push("script_heavy_shell");
    }
    if signals.len() >= 2 || (signals.len() == 1 && len < 5_000) {
        Some(format!(
            "Structural: {} ({len} bytes, {visible_len} chars visible)",
            signals.join(", ")
        ))
    } else {
        None
    }
}

fn metadata(
    html: &str,
    status: Option<u16>,
    headers: Option<&HeaderMap>,
    widget_present: bool,
    signals: &[&str],
) -> serde_json::Value {
    let server = headers
        .and_then(|h| h.get("server"))
        .and_then(|v| v.to_str().ok());
    serde_json::json!({
        "surface": "block_detector",
        "status_code": status,
        "server": server,
        "signals": signals,
        "title": title(html),
        "widget_present": widget_present,
        "sitekey": sitekey(html),
        "action": action(html),
        "iframe_srcs": iframe_srcs(html),
        "html_bytes": html.len(),
    })
}

fn title(html: &str) -> Option<String> {
    Regex::new(r"(?is)<title[^>]*>\s*(.*?)\s*</title>")
        .ok()
        .and_then(|re| re.captures(html))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
}

fn sitekey(html: &str) -> Option<String> {
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

fn action(html: &str) -> Option<String> {
    Regex::new(r#"(?i)data-action\s*=\s*["']([^"']+)["']"#)
        .ok()
        .and_then(|re| re.captures(html))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn iframe_srcs(html: &str) -> Vec<String> {
    Regex::new(r#"(?is)<iframe[^>]+src\s*=\s*["']([^"']+)["']"#)
        .ok()
        .map(|re| {
            re.captures_iter(html)
                .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url() -> url::Url {
        url::Url::parse("https://example.com/").unwrap()
    }

    #[test]
    fn cloudflare_form_blocks() {
        let html = r#"<html><body><form class="challenge-form"><input name="__cf_chl_f_tk="></form></body></html>"#;
        let c = classify_html(html, &url(), None);
        assert!(c.blocked);
        assert_eq!(c.vendor, Some(ChallengeVendor::CloudflareJsChallenge));
    }

    #[test]
    fn access_denied_large_article_is_not_tier2_block() {
        let mut html = "<html><body><article><h1>Access Denied as a legal term</h1>".to_string();
        html.push_str(&"real content ".repeat(1200));
        html.push_str("</article></body></html>");
        let c = classify_html(&html, &url(), None);
        assert!(!c.blocked, "{c:?}");
    }

    #[test]
    fn non_data_403_html_blocks() {
        let c = classify_parts(
            Some(403),
            "<html><body><h1>Nope</h1></body></html>",
            None,
            &url(),
            BlockSurface::HttpResponse,
        );
        assert!(c.blocked);
        assert_eq!(c.level, Some(ChallengeLevel::HardBlock));
    }

    #[test]
    fn structural_shell_blocks() {
        let c = classify_html(
            "<html><body><script>app()</script></body></html>",
            &url(),
            None,
        );
        assert!(c.blocked);
        assert_eq!(c.surface, BlockSurface::Structural);
    }
}
