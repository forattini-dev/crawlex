//! Browser persona profiles.
//!
//! A `Profile` carries the coherent set of values needed to impersonate a
//! given browser/version/OS combination — the User-Agent string, the
//! `Sec-CH-UA` brand cluster, the JS shim version markers, AND (via the
//! TLS catalog) the ClientHello cipher list, extension order, supported
//! groups, ALPN/ALPS payload, etc.
//!
//! Detectors cross-check these signals; a mismatch (UA says Chrome 149 but
//! ClientHello matches Chrome 116) lights up bot trees. The profile is the
//! single source of truth that keeps every layer aligned.
//!
//! # API
//!
//! Three public entry points cover the modern catalog:
//!
//! ```no_run
//! use crawlex::impersonate::profiles::{Profile, BrowserOs};
//!
//! // Latest stable Chrome on Linux:
//! let p = Profile::for_chrome(149).os(BrowserOs::Linux).build().unwrap();
//!
//! // Vanilla Chromium 122 on Linux (rebrandless):
//! let p = Profile::for_chromium(122).os(BrowserOs::Linux).build().unwrap();
//!
//! // Firefox 130 on macOS:
//! let p = Profile::for_firefox(130).os(BrowserOs::MacOs).build().unwrap();
//! ```
//!
//! The legacy `Profile::Chrome131Stable` / `Chrome132Stable` /
//! `Chrome149Stable` constants are preserved as `#[doc(hidden)]` aliases
//! so existing callers keep compiling. New code should use the builder.

use serde::{Deserialize, Serialize};

pub use crate::impersonate::catalog::BrowserOs;
use crate::impersonate::catalog::{eras, Browser, TlsFingerprint};

/// A canned "this is browser X version Y on OS Z" persona.
///
/// `Profile` is `Copy` so callers can pass it cheaply through deep call
/// stacks without `Arc` plumbing. The actual TLS bytes live in the
/// `&'static TlsFingerprint` returned by `tls()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    Chrome {
        major: u16,
        os: BrowserOs,
    },
    Chromium {
        major: u16,
        os: BrowserOs,
    },
    Firefox {
        major: u16,
        os: BrowserOs,
    },
    Edge {
        major: u16,
        os: BrowserOs,
    },
    Safari {
        major: u16,
        os: BrowserOs,
    },

    // ────────────────────────────────────────────────────────────
    // Legacy named variants — kept for backward compatibility while
    // the codebase migrates to the builder API. Each one is a thin
    // alias for the equivalent `Chrome { major, os }` form.
    // ────────────────────────────────────────────────────────────
    #[doc(hidden)]
    Chrome131Stable,
    #[doc(hidden)]
    Chrome132Stable,
    #[doc(hidden)]
    Chrome149Stable,
}

impl Profile {
    /// Start a Chrome profile builder. Pin OS via `.os(...)` then `.build()`.
    pub fn for_chrome(major: u16) -> ProfileBuilder {
        ProfileBuilder {
            browser: Browser::Chrome,
            major,
            os: BrowserOs::Linux,
        }
    }

    /// Start a Chromium profile builder.
    pub fn for_chromium(major: u16) -> ProfileBuilder {
        ProfileBuilder {
            browser: Browser::Chromium,
            major,
            os: BrowserOs::Linux,
        }
    }

    /// Start a Firefox profile builder.
    pub fn for_firefox(major: u16) -> ProfileBuilder {
        ProfileBuilder {
            browser: Browser::Firefox,
            major,
            os: BrowserOs::Linux,
        }
    }

    /// Start an Edge profile builder.
    pub fn for_edge(major: u16) -> ProfileBuilder {
        ProfileBuilder {
            browser: Browser::Edge,
            major,
            os: BrowserOs::Windows,
        }
    }

    /// Start a Safari profile builder.
    pub fn for_safari(major: u16) -> ProfileBuilder {
        ProfileBuilder {
            browser: Browser::Safari,
            major,
            os: BrowserOs::MacOs,
        }
    }

    /// Decompose the profile into its `(browser, major, os)` tuple.
    pub fn parts(self) -> (Browser, u16, BrowserOs) {
        match self {
            Profile::Chrome { major, os } => (Browser::Chrome, major, os),
            Profile::Chromium { major, os } => (Browser::Chromium, major, os),
            Profile::Firefox { major, os } => (Browser::Firefox, major, os),
            Profile::Edge { major, os } => (Browser::Edge, major, os),
            Profile::Safari { major, os } => (Browser::Safari, major, os),

            Profile::Chrome131Stable => (Browser::Chrome, 131, BrowserOs::Linux),
            Profile::Chrome132Stable => (Browser::Chrome, 132, BrowserOs::Linux),
            Profile::Chrome149Stable => (Browser::Chrome, 149, BrowserOs::Linux),
        }
    }

    /// Resolve the static TLS fingerprint for this profile via the catalog.
    /// Falls back to the closest era's representative when the exact
    /// `(browser, major, os)` tuple isn't yet captured (with a tracing::warn
    /// emitted from `eras::era_for`). Returns `None` only for completely
    /// unsupported browsers.
    pub fn tls(self) -> Option<&'static TlsFingerprint> {
        let (browser, major, os) = self.parts();
        eras::era_for(browser, major, os)
    }

    /// Major version string (e.g. `"149"`).
    pub fn major_version(self) -> u32 {
        self.parts().1 as u32
    }

    /// Stable User-Agent for the persona.
    pub fn user_agent(self) -> String {
        let (browser, major, os) = self.parts();
        let os_token = match os {
            BrowserOs::Linux => "X11; Linux x86_64",
            BrowserOs::Windows => "Windows NT 10.0; Win64; x64",
            BrowserOs::MacOs => "Macintosh; Intel Mac OS X 10_15_7",
            BrowserOs::Android => "Linux; Android 14; Pixel 8",
            BrowserOs::Other => "X11; Linux x86_64",
        };
        match browser {
            Browser::Chrome => format!(
                "Mozilla/5.0 ({os_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
            ),
            Browser::Chromium => format!(
                "Mozilla/5.0 ({os_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
            ),
            Browser::Edge => format!(
                "Mozilla/5.0 ({os_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0"
            ),
            Browser::Firefox => format!(
                "Mozilla/5.0 ({os_token}; rv:{major}.0) Gecko/20100101 Firefox/{major}.0"
            ),
            Browser::Safari => {
                // Safari version mapping: WebKit 605 ~= Safari 14, 615 ~= Safari 16/17.
                let webkit = if major >= 17 { "618.1.15" } else { "605.1.15" };
                format!(
                    "Mozilla/5.0 ({os_token}) AppleWebKit/{webkit} (KHTML, like Gecko) Version/{major}.0 Safari/{webkit}"
                )
            }
            Browser::Brave | Browser::Opera | Browser::Other => format!(
                "Mozilla/5.0 ({os_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
            ),
        }
    }

    /// `Sec-CH-UA` header value matching the persona's brand cluster.
    /// Only Chromium-family browsers send this header; non-Chromium returns
    /// an empty string and callers should skip the header altogether.
    pub fn sec_ch_ua(self) -> String {
        let (browser, major, _) = self.parts();
        match browser {
            Browser::Chrome => format!(
                "\"Google Chrome\";v=\"{major}\", \"Chromium\";v=\"{major}\", \"Not_A Brand\";v=\"24\""
            ),
            Browser::Chromium => format!(
                "\"Chromium\";v=\"{major}\", \"Not_A Brand\";v=\"24\""
            ),
            Browser::Edge => format!(
                "\"Microsoft Edge\";v=\"{major}\", \"Chromium\";v=\"{major}\", \"Not_A Brand\";v=\"24\""
            ),
            Browser::Brave | Browser::Opera => format!(
                "\"Chromium\";v=\"{major}\", \"Not_A Brand\";v=\"24\""
            ),
            // Firefox / Safari don't send Sec-CH-UA.
            Browser::Firefox | Browser::Safari | Browser::Other => String::new(),
        }
    }

    /// Full version string for `userAgentData.brands` high-entropy hint.
    pub fn ua_full_version(self) -> String {
        let (_, major, _) = self.parts();
        // Map major → known stable patch number when we have it; otherwise
        // synthesize a plausible "X.0.MMMM.NN" placeholder.
        match major {
            131 => "131.0.6778.85".into(),
            132 => "132.0.6834.83".into(),
            149 => "149.0.7795.2".into(),
            _ => format!("{major}.0.0.0"),
        }
    }

    /// JSON list used as `navigator.userAgentData.brands`.
    pub fn ua_brands_json(self) -> String {
        let (browser, major, _) = self.parts();
        match browser {
            Browser::Chrome => format!(
                r#"[{{"brand":"Google Chrome","version":"{major}"}},{{"brand":"Chromium","version":"{major}"}},{{"brand":"Not_A Brand","version":"24"}}]"#
            ),
            Browser::Chromium => format!(
                r#"[{{"brand":"Chromium","version":"{major}"}},{{"brand":"Not_A Brand","version":"24"}}]"#
            ),
            Browser::Edge => format!(
                r#"[{{"brand":"Microsoft Edge","version":"{major}"}},{{"brand":"Chromium","version":"{major}"}},{{"brand":"Not_A Brand","version":"24"}}]"#
            ),
            _ => String::new(),
        }
    }

    /// Same as [`ua_brands_json`] but with full version numbers.
    pub fn fullversion_brands_json(self) -> String {
        let (browser, _, _) = self.parts();
        let full = self.ua_full_version();
        match browser {
            Browser::Chrome => format!(
                r#"[{{"brand":"Google Chrome","version":"{full}"}},{{"brand":"Chromium","version":"{full}"}},{{"brand":"Not_A Brand","version":"24.0.0.0"}}]"#
            ),
            Browser::Chromium => format!(
                r#"[{{"brand":"Chromium","version":"{full}"}},{{"brand":"Not_A Brand","version":"24.0.0.0"}}]"#
            ),
            Browser::Edge => format!(
                r#"[{{"brand":"Microsoft Edge","version":"{full}"}},{{"brand":"Chromium","version":"{full}"}},{{"brand":"Not_A Brand","version":"24.0.0.0"}}]"#
            ),
            _ => String::new(),
        }
    }

    /// Legacy heuristic: pick the closest profile for a real Chrome major
    /// detected on disk. New callers should prefer the typed builders.
    pub fn from_detected_major(major: u32) -> Profile {
        Profile::Chrome {
            major: major as u16,
            os: BrowserOs::Linux,
        }
    }
}

/// Builder returned by `Profile::for_chrome`/`for_chromium`/`for_firefox`/etc.
pub struct ProfileBuilder {
    browser: Browser,
    major: u16,
    os: BrowserOs,
}

impl ProfileBuilder {
    /// Pin the OS for this profile.
    pub fn os(mut self, os: BrowserOs) -> Self {
        self.os = os;
        self
    }

    /// Materialise the profile, validating that the catalog can resolve a
    /// fingerprint for the resulting tuple.
    pub fn build(self) -> Result<Profile, ProfileError> {
        let profile = match self.browser {
            Browser::Chrome => Profile::Chrome {
                major: self.major,
                os: self.os,
            },
            Browser::Chromium => Profile::Chromium {
                major: self.major,
                os: self.os,
            },
            Browser::Firefox => Profile::Firefox {
                major: self.major,
                os: self.os,
            },
            Browser::Edge => Profile::Edge {
                major: self.major,
                os: self.os,
            },
            Browser::Safari => Profile::Safari {
                major: self.major,
                os: self.os,
            },
            Browser::Brave | Browser::Opera | Browser::Other => {
                return Err(ProfileError::UnsupportedBrowser);
            }
        };
        if profile.tls().is_none() {
            return Err(ProfileError::NoFingerprint);
        }
        Ok(profile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("unsupported browser for the catalog")]
    UnsupportedBrowser,
    #[error("no TLS fingerprint available for this browser/major/os tuple")]
    NoFingerprint,
    #[error("profile string `{0}` does not match `<browser>-<major>-<os>` (e.g. `chrome-149-linux`)")]
    BadFormat(String),
    #[error("unknown browser `{0}` — expected chrome|chromium|firefox|edge|safari")]
    UnknownBrowser(String),
    #[error("invalid major version `{0}` — expected positive integer")]
    BadMajor(String),
    #[error("unknown OS `{0}` — expected linux|windows|macos|android")]
    UnknownOs(String),
}

// ──────────────────────────────────────────────────────────────────────
// FromStr + Display: round-trip via "chrome-149-linux" form.
// ──────────────────────────────────────────────────────────────────────

impl std::str::FromStr for Profile {
    type Err = ProfileError;

    /// Parse a profile spec in the form `<browser>-<major>-<os>`.
    ///
    /// Examples:
    ///   * `chrome-149-linux`
    ///   * `chromium-122-linux`
    ///   * `firefox-130-macos`
    ///   * `edge-130-windows`
    ///
    /// Also accepts the legacy aliases `chrome-131-stable`, `chrome-132-stable`,
    /// `chrome-149-stable` — the `stable` slot is treated as Linux for backward
    /// compatibility with old configs.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Legacy aliases first.
        match s {
            "chrome-131-stable" | "chrome131-stable" => return Ok(Profile::Chrome131Stable),
            "chrome-132-stable" | "chrome132-stable" => return Ok(Profile::Chrome132Stable),
            "chrome-149-stable" | "chrome149-stable" => return Ok(Profile::Chrome149Stable),
            _ => {}
        }

        let parts: Vec<&str> = s.splitn(3, '-').collect();
        if parts.len() != 3 {
            return Err(ProfileError::BadFormat(s.to_string()));
        }
        let browser = match parts[0].to_ascii_lowercase().as_str() {
            "chrome" => Browser::Chrome,
            "chromium" => Browser::Chromium,
            "firefox" => Browser::Firefox,
            "edge" => Browser::Edge,
            "safari" => Browser::Safari,
            other => return Err(ProfileError::UnknownBrowser(other.to_string())),
        };
        let major: u16 = parts[1]
            .parse()
            .map_err(|_| ProfileError::BadMajor(parts[1].to_string()))?;
        let os = match parts[2].to_ascii_lowercase().as_str() {
            "linux" => BrowserOs::Linux,
            "windows" | "win" | "win10" | "win11" => BrowserOs::Windows,
            "macos" | "mac" | "darwin" | "osx" => BrowserOs::MacOs,
            "android" => BrowserOs::Android,
            other => return Err(ProfileError::UnknownOs(other.to_string())),
        };

        let builder = ProfileBuilder {
            browser,
            major,
            os,
        };
        builder.build()
    }
}

impl std::fmt::Display for Profile {
    /// Inverse of `FromStr`: render a profile as `<browser>-<major>-<os>`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (browser, major, os) = self.parts();
        let browser_token = match browser {
            Browser::Chrome => "chrome",
            Browser::Chromium => "chromium",
            Browser::Firefox => "firefox",
            Browser::Edge => "edge",
            Browser::Safari => "safari",
            Browser::Brave => "brave",
            Browser::Opera => "opera",
            Browser::Other => "other",
        };
        let os_token = match os {
            BrowserOs::Linux => "linux",
            BrowserOs::Windows => "windows",
            BrowserOs::MacOs => "macos",
            BrowserOs::Android => "android",
            BrowserOs::Other => "other",
        };
        write!(f, "{}-{}-{}", browser_token, major, os_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_aliases_decompose_correctly() {
        assert_eq!(
            Profile::Chrome131Stable.parts(),
            (Browser::Chrome, 131, BrowserOs::Linux)
        );
        assert_eq!(
            Profile::Chrome149Stable.parts(),
            (Browser::Chrome, 149, BrowserOs::Linux)
        );
    }

    #[test]
    fn builder_builds_chrome_149_linux() {
        let p = Profile::for_chrome(149)
            .os(BrowserOs::Linux)
            .build()
            .expect("chrome 149 builds");
        let (b, m, o) = p.parts();
        assert_eq!(b, Browser::Chrome);
        assert_eq!(m, 149);
        assert_eq!(o, BrowserOs::Linux);
    }

    #[test]
    fn builder_resolves_tls_via_era_fallback() {
        // Until we capture Chrome 149 directly, this resolves to the
        // newest era representative. Just assert non-None + the name
        // starts with "chrome_".
        let p = Profile::for_chrome(149)
            .os(BrowserOs::Linux)
            .build()
            .unwrap();
        let fp = p.tls().expect("tls resolves");
        assert!(fp.name.starts_with("chrome_"), "name = {}", fp.name);
    }

    #[test]
    fn user_agent_renders_persona_correctly() {
        let p = Profile::for_chrome(149)
            .os(BrowserOs::Linux)
            .build()
            .unwrap();
        let ua = p.user_agent();
        assert!(ua.contains("Chrome/149"), "ua = {}", ua);
        assert!(ua.contains("Linux"), "ua = {}", ua);
    }

    #[test]
    fn firefox_does_not_send_sec_ch_ua() {
        let p = Profile::for_firefox(130)
            .os(BrowserOs::Linux)
            .build()
            .unwrap();
        assert_eq!(p.sec_ch_ua(), "");
    }

    #[test]
    fn from_str_round_trips_via_display() {
        use std::str::FromStr;
        for spec in [
            "chrome-149-linux",
            "chromium-122-linux",
            "firefox-130-macos",
            "edge-130-windows",
            "safari-17-macos",
        ] {
            let p = Profile::from_str(spec).unwrap_or_else(|e| panic!("{spec}: {e}"));
            assert_eq!(p.to_string(), spec, "round-trip {spec}");
        }
    }

    #[test]
    fn from_str_rejects_bad_format() {
        use std::str::FromStr;
        assert!(matches!(
            Profile::from_str("chrome149linux"),
            Err(ProfileError::BadFormat(_))
        ));
        assert!(matches!(
            Profile::from_str("chrome-149"),
            Err(ProfileError::BadFormat(_))
        ));
        assert!(matches!(
            Profile::from_str("opera-149-linux"),
            Err(ProfileError::UnknownBrowser(_)) | Err(ProfileError::UnsupportedBrowser)
        ));
        assert!(matches!(
            Profile::from_str("chrome-abc-linux"),
            Err(ProfileError::BadMajor(_))
        ));
        assert!(matches!(
            Profile::from_str("chrome-149-bsd"),
            Err(ProfileError::UnknownOs(_))
        ));
    }

    #[test]
    fn from_str_accepts_legacy_aliases() {
        use std::str::FromStr;
        assert_eq!(
            Profile::from_str("chrome-149-stable").unwrap(),
            Profile::Chrome149Stable
        );
    }

    #[test]
    fn from_str_accepts_os_synonyms() {
        use std::str::FromStr;
        for spec in [
            "chrome-149-windows",
            "chrome-149-win10",
            "chrome-149-mac",
            "chrome-149-darwin",
        ] {
            Profile::from_str(spec).unwrap_or_else(|e| panic!("{spec}: {e}"));
        }
    }
}
