//! Vendor URL/endpoint signatures + PerimeterX signal catalog.
//!
//! Pure lookup tables used by the telemetry observer
//! (`crate::antibot::telemetry`) to classify outbound network requests
//! the page makes to known antibot vendors. No IO, no feature gates.
//!
//! References: `research/evasion-deep-dive.md` §5 (vendor deep-dives).

use super::ChallengeVendor;

/// A URL pattern we recognise as a vendor telemetry/challenge endpoint.
///
/// Match rules — a request matches when **any** of the following holds,
/// scanned against the request URL lower-cased:
/// * host equals `host_eq`
/// * host ends with `host_suffix`
/// * path contains `path_contains`
///
/// Multiple patterns per vendor are expected — the first match wins.
#[derive(Debug, Clone, Copy)]
pub struct VendorPattern {
    pub vendor: ChallengeVendor,
    pub host_eq: Option<&'static str>,
    pub host_suffix: Option<&'static str>,
    pub path_contains: Option<&'static str>,
    /// Short label describing what this pattern catches — used in event
    /// metadata for humans diagnosing a hit.
    pub label: &'static str,
}

/// Static table of known vendor endpoints. Ordered most-specific-first.
pub const VENDOR_PATTERNS: &[VendorPattern] = &[
    // --- Akamai ----------------------------------------------------------
    VendorPattern {
        vendor: ChallengeVendor::Akamai,
        host_eq: None,
        host_suffix: Some(".akamaihd.net"),
        path_contains: None,
        label: "akamai-cdn",
    },
    VendorPattern {
        vendor: ChallengeVendor::Akamai,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/_bm/"),
        label: "akamai-bm-path",
    },
    VendorPattern {
        vendor: ChallengeVendor::Akamai,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/akam/"),
        label: "akamai-akam-path",
    },
    VendorPattern {
        vendor: ChallengeVendor::Akamai,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("sensor_data"),
        label: "akamai-sensor-data",
    },
    // --- PerimeterX -----------------------------------------------------
    VendorPattern {
        vendor: ChallengeVendor::PerimeterX,
        host_eq: Some("client.perimeterx.net"),
        host_suffix: None,
        path_contains: None,
        label: "perimeterx-client",
    },
    VendorPattern {
        vendor: ChallengeVendor::PerimeterX,
        host_eq: None,
        host_suffix: Some(".perimeterx.net"),
        path_contains: None,
        label: "perimeterx-suffix",
    },
    VendorPattern {
        vendor: ChallengeVendor::PerimeterX,
        host_eq: None,
        host_suffix: Some(".px-cloud.net"),
        path_contains: None,
        label: "perimeterx-pxcloud",
    },
    VendorPattern {
        vendor: ChallengeVendor::PerimeterX,
        host_eq: None,
        host_suffix: Some(".px-cdn.net"),
        path_contains: None,
        label: "perimeterx-pxcdn",
    },
    VendorPattern {
        vendor: ChallengeVendor::PerimeterX,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/api/v2/collector"),
        label: "perimeterx-collector",
    },
    VendorPattern {
        vendor: ChallengeVendor::PerimeterX,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/_px2/"),
        label: "perimeterx-px2",
    },
    // --- DataDome -------------------------------------------------------
    VendorPattern {
        vendor: ChallengeVendor::DataDome,
        host_eq: Some("js.datadome.co"),
        host_suffix: None,
        path_contains: None,
        label: "datadome-js",
    },
    VendorPattern {
        vendor: ChallengeVendor::DataDome,
        host_eq: Some("api.datadome.co"),
        host_suffix: None,
        path_contains: None,
        label: "datadome-api",
    },
    VendorPattern {
        vendor: ChallengeVendor::DataDome,
        host_eq: None,
        host_suffix: Some("captcha-delivery.com"),
        path_contains: None,
        label: "datadome-captcha",
    },
    VendorPattern {
        vendor: ChallengeVendor::DataDome,
        host_eq: None,
        host_suffix: Some(".datado.me"),
        path_contains: None,
        label: "datadome-short",
    },
    // --- Cloudflare Turnstile / JS challenge ----------------------------
    VendorPattern {
        vendor: ChallengeVendor::CloudflareTurnstile,
        host_eq: Some("challenges.cloudflare.com"),
        host_suffix: None,
        path_contains: Some("/turnstile/"),
        label: "cloudflare-turnstile",
    },
    VendorPattern {
        vendor: ChallengeVendor::CloudflareJsChallenge,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/cdn-cgi/challenge-platform/"),
        label: "cloudflare-challenge-platform",
    },
    // --- hCaptcha -------------------------------------------------------
    VendorPattern {
        vendor: ChallengeVendor::HCaptcha,
        host_eq: Some("js.hcaptcha.com"),
        host_suffix: None,
        path_contains: None,
        label: "hcaptcha-js",
    },
    VendorPattern {
        vendor: ChallengeVendor::HCaptcha,
        host_eq: Some("api.hcaptcha.com"),
        host_suffix: None,
        path_contains: None,
        label: "hcaptcha-api",
    },
    VendorPattern {
        vendor: ChallengeVendor::HCaptcha,
        host_eq: Some("hcaptcha.com"),
        host_suffix: None,
        path_contains: Some("/checkcaptcha/"),
        label: "hcaptcha-checkcaptcha",
    },
    VendorPattern {
        vendor: ChallengeVendor::HCaptcha,
        host_eq: None,
        host_suffix: Some(".hcaptcha.com"),
        path_contains: None,
        label: "hcaptcha-suffix",
    },
    // --- reCAPTCHA ------------------------------------------------------
    VendorPattern {
        vendor: ChallengeVendor::RecaptchaEnterprise,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/recaptcha/enterprise"),
        label: "recaptcha-enterprise",
    },
    VendorPattern {
        vendor: ChallengeVendor::Recaptcha,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/recaptcha/api2/"),
        label: "recaptcha-api2",
    },
    VendorPattern {
        vendor: ChallengeVendor::Recaptcha,
        host_eq: None,
        host_suffix: None,
        path_contains: Some("/recaptcha/api.js"),
        label: "recaptcha-api-js",
    },
    VendorPattern {
        vendor: ChallengeVendor::Recaptcha,
        host_eq: Some("www.recaptcha.net"),
        host_suffix: None,
        path_contains: None,
        label: "recaptcha-net",
    },
];

/// Return the first matching `VendorPattern` for this URL, or `None`.
///
/// Matching is conservative: we require the URL's host or path to
/// positively match one of the static patterns. Bare `GET` requests to
/// e.g. `example.com/foo` never match, avoiding false positives on every
/// page load.
pub fn match_vendor_url(url: &url::Url) -> Option<&'static VendorPattern> {
    let host = url.host_str()?.to_ascii_lowercase();
    let path = url.path().to_ascii_lowercase();
    for p in VENDOR_PATTERNS {
        let host_ok = match (p.host_eq, p.host_suffix) {
            (Some(eq), _) => host == eq,
            (None, Some(suf)) => host.ends_with(suf),
            (None, None) => true,
        };
        if !host_ok {
            continue;
        }
        if let Some(frag) = p.path_contains {
            if !path.contains(frag) {
                continue;
            }
        } else {
            // Without a path fragment constraint we need a positive host match.
            if p.host_eq.is_none() && p.host_suffix.is_none() {
                continue;
            }
        }
        return Some(p);
    }
    None
}

/// ---------------------------------------------------------------------
/// PerimeterX signal ID catalog (PX320–PX348).
/// ---------------------------------------------------------------------
///
/// IDs and rough meanings are compiled from public reverse-engineering
/// writeups ([antibot.blog](https://antibot.blog/posts/1741549175263),
/// PerimeterX-Reverse repos) and `research/evasion-deep-dive.md §5.4`.
/// These are **heuristic** — PerimeterX rotates field numbering per SDK
/// release, so the catalog is a best-effort reference, not a contract.
#[derive(Debug, Clone, Copy)]
pub struct PxSignal {
    /// Canonical ID, e.g. `"PX320"`.
    pub id: &'static str,
    /// Short human name.
    pub name: &'static str,
    /// What the vendor is measuring.
    pub detection: &'static str,
}

pub const PX_SIGNALS: &[PxSignal] = &[
    PxSignal {
        id: "PX320",
        name: "cdp_detection",
        detection: "Chrome DevTools Protocol presence (Runtime.Enable, console getters)",
    },
    PxSignal {
        id: "PX321",
        name: "device_model",
        detection: "navigator.userAgent model string",
    },
    PxSignal {
        id: "PX322",
        name: "device_name",
        detection: "Device marketing name (iOS only)",
    },
    PxSignal {
        id: "PX323",
        name: "os_name",
        detection: "OS family (Windows/macOS/Linux/iOS/Android)",
    },
    PxSignal {
        id: "PX324",
        name: "os_version",
        detection: "OS version string",
    },
    PxSignal {
        id: "PX325",
        name: "timestamp",
        detection: "Event timestamp ms",
    },
    PxSignal {
        id: "PX326",
        name: "uuid",
        detection: "Per-session UUID generated client-side",
    },
    PxSignal {
        id: "PX327",
        name: "sha1",
        detection: "SHA-1 of collected signals, used as integrity token",
    },
    PxSignal {
        id: "PX328",
        name: "sdk_version",
        detection: "PerimeterX SDK version string",
    },
    PxSignal {
        id: "PX329",
        name: "bundle_id",
        detection: "App bundle identifier (mobile) / origin (web)",
    },
    PxSignal {
        id: "PX330",
        name: "screen",
        detection: "screen.width/height/colorDepth/pixelRatio",
    },
    PxSignal {
        id: "PX331",
        name: "viewport",
        detection: "window.inner/outerWidth/Height",
    },
    PxSignal {
        id: "PX332",
        name: "timezone",
        detection: "Intl.DateTimeFormat resolvedOptions().timeZone",
    },
    PxSignal {
        id: "PX333",
        name: "webgl_vendor",
        detection: "WEBGL_debug_renderer_info vendor/renderer strings",
    },
    PxSignal {
        id: "PX334",
        name: "canvas_hash",
        detection: "Canvas 2D rendering hash (text + emoji)",
    },
    PxSignal {
        id: "PX335",
        name: "audio_fp",
        detection: "AudioContext DynamicsCompressor output hash",
    },
    PxSignal {
        id: "PX336",
        name: "fonts",
        detection: "Font enumeration via measureText width probe",
    },
    PxSignal {
        id: "PX337",
        name: "plugins",
        detection: "navigator.plugins length + names",
    },
    PxSignal {
        id: "PX338",
        name: "languages",
        detection: "navigator.language + navigator.languages array",
    },
    PxSignal {
        id: "PX339",
        name: "hw_concurrency",
        detection: "navigator.hardwareConcurrency",
    },
    PxSignal {
        id: "PX340",
        name: "device_memory",
        detection: "navigator.deviceMemory",
    },
    PxSignal {
        id: "PX341",
        name: "battery",
        detection: "navigator.getBattery() level + charging",
    },
    PxSignal {
        id: "PX342",
        name: "mouse_entropy",
        detection: "Mouse trajectory sampling — curvature, velocity, jitter",
    },
    PxSignal {
        id: "PX343",
        name: "touch_events",
        detection: "TouchEvent stream (mobile)",
    },
    PxSignal {
        id: "PX344",
        name: "keyboard_timing",
        detection: "Keystroke inter-arrival time distribution",
    },
    PxSignal {
        id: "PX345",
        name: "scroll_behavior",
        detection: "scroll deltas + pointer pressure",
    },
    PxSignal {
        id: "PX346",
        name: "webdriver_flag",
        detection: "navigator.webdriver boolean",
    },
    PxSignal {
        id: "PX347",
        name: "permissions_api",
        detection: "Permissions.query('notifications') state",
    },
    PxSignal {
        id: "PX348",
        name: "iframe_chain",
        detection: "Top-frame ancestor chain consistency check",
    },
];

/// Lookup a `PxSignal` by its ID (`"PX320"` etc.).
pub fn px_signal(id: &str) -> Option<&'static PxSignal> {
    PX_SIGNALS.iter().find(|s| s.id.eq_ignore_ascii_case(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_catalog_has_29_entries() {
        // PX320..=PX348 inclusive = 29 signals.
        assert_eq!(PX_SIGNALS.len(), 29);
    }

    #[test]
    fn px_catalog_ids_are_unique_and_ordered() {
        for (i, s) in PX_SIGNALS.iter().enumerate() {
            let expected = format!("PX{}", 320 + i);
            assert_eq!(s.id, expected, "entry {i} has wrong id");
        }
    }

    #[test]
    fn px_lookup_works() {
        assert!(px_signal("PX320").is_some());
        assert!(px_signal("px348").is_some());
        assert!(px_signal("PX999").is_none());
    }

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn match_cloudflare_challenge_platform() {
        let p = match_vendor_url(&u(
            "https://example.com/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1?ray=abc",
        ))
        .expect("cf pattern");
        assert_eq!(p.vendor, ChallengeVendor::CloudflareJsChallenge);
    }

    #[test]
    fn match_turnstile() {
        let p = match_vendor_url(&u("https://challenges.cloudflare.com/turnstile/v0/api.js"))
            .expect("turnstile");
        assert_eq!(p.vendor, ChallengeVendor::CloudflareTurnstile);
    }

    #[test]
    fn match_datadome_captcha() {
        let p = match_vendor_url(&u(
            "https://geo.captcha-delivery.com/captcha/?initialCid=xxx",
        ))
        .expect("datadome");
        assert_eq!(p.vendor, ChallengeVendor::DataDome);
    }

    #[test]
    fn match_perimeterx_collector() {
        let p = match_vendor_url(&u(
            "https://client.perimeterx.net/api/v2/collector?appId=abc",
        ))
        .expect("px");
        assert_eq!(p.vendor, ChallengeVendor::PerimeterX);
    }

    #[test]
    fn match_hcaptcha_checkcaptcha() {
        let p = match_vendor_url(&u("https://hcaptcha.com/checkcaptcha/xyz")).expect("hcap");
        assert_eq!(p.vendor, ChallengeVendor::HCaptcha);
    }

    #[test]
    fn match_recaptcha_enterprise_before_recaptcha() {
        let p = match_vendor_url(&u(
            "https://www.google.com/recaptcha/enterprise.js?render=xyz",
        ))
        .expect("enterprise");
        assert_eq!(p.vendor, ChallengeVendor::RecaptchaEnterprise);
    }

    #[test]
    fn match_akamai_sensor_path() {
        let p = match_vendor_url(&u("https://www.example.com/_bm/_data?token=1")).expect("akamai");
        assert_eq!(p.vendor, ChallengeVendor::Akamai);
    }

    #[test]
    fn innocent_urls_dont_match() {
        assert!(match_vendor_url(&u("https://example.com/index.html")).is_none());
        assert!(match_vendor_url(&u("https://news.ycombinator.com/")).is_none());
    }
}
