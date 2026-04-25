//! TLS era lookup: maps `(browser, major, os)` to a `TlsFingerprint`.
//!
//! Real browsers' ClientHello changes shape across versions in discrete
//! "eras" — Chrome 124-131 share an ALPS payload format, Chrome 132+ adds
//! MLKEM768 post-quantum, etc. Within an era, the fingerprint is identical
//! and we can return the same `TlsFingerprint` static for every major in
//! that range.
//!
//! Every catalog query goes through [`era_for`]. If we have a captured
//! fingerprint for the exact `(browser, major, os)` tuple, we return that;
//! otherwise we fall back to the closest era's representative profile and
//! emit `tracing::warn!` so operators know the fingerprint is approximated.

use super::{lookup, Browser, BrowserOs, TlsFingerprint};

/// Best-effort lookup of a TLS fingerprint for the given browser version.
///
/// Returns `Some(&'static TlsFingerprint)` when we either:
/// 1. Have a captured fingerprint for the exact `(browser, major, os)`
///    tuple in the catalog (curl-impersonate vendored or our own capture), or
/// 2. Can map the major version to a known TLS era and return that era's
///    representative profile.
///
/// Returns `None` only when the browser is wholly unsupported (e.g. iOS
/// Safari, Tor Browser, etc).
///
/// Emits `tracing::warn!` when falling back to era approximation so callers
/// know the fingerprint isn't a 1:1 capture of the requested version.
pub fn era_for(browser: Browser, major: u16, os: BrowserOs) -> Option<&'static TlsFingerprint> {
    // Try exact match first.
    if let Some(fp) = exact_match(browser, major, os) {
        return Some(fp);
    }

    // Fall back to era-based approximation.
    let representative = match browser {
        Browser::Chrome | Browser::Chromium => chrome_era_representative(major, os),
        Browser::Edge => edge_era_representative(major),
        Browser::Firefox => firefox_era_representative(major),
        Browser::Safari => safari_era_representative(major),
        Browser::Brave | Browser::Opera => {
            // Brave/Opera = Chromium with branding. Use Chrome era as base.
            chrome_era_representative(major, os)
        }
        Browser::Other => None,
    };

    if let Some(name) = representative {
        if let Some(fp) = lookup(name) {
            tracing::warn!(
                target: "crawlex::impersonate::catalog",
                browser = ?browser,
                major,
                os = ?os,
                fallback = name,
                "exact TLS fingerprint not in catalog; using era representative"
            );
            return Some(fp);
        }
    }

    None
}

/// Direct catalog hit by curl-impersonate name convention.
fn exact_match(browser: Browser, major: u16, os: BrowserOs) -> Option<&'static TlsFingerprint> {
    let browser_token = match browser {
        Browser::Chrome => "chrome",
        Browser::Chromium => "chromium",
        Browser::Firefox => "firefox",
        Browser::Edge => "edge",
        Browser::Safari => "safari",
        _ => return None,
    };
    let os_token = match os {
        BrowserOs::Windows => "win10",
        BrowserOs::MacOs => "macos",
        BrowserOs::Linux => "linux",
        BrowserOs::Android => "android",
        BrowserOs::Other => return None,
    };
    // Catalog names look like "chrome_98.0.4758.102_win10" — we don't know
    // the build patch numbers up front, so iterate prefix-matching.
    let prefix = format!("{}_{}", browser_token, major);
    let suffix = format!("_{}", os_token);
    for fp in super::all() {
        if fp.name.starts_with(&prefix)
            && fp.name.contains(&suffix)
            && (fp.name.as_bytes().get(prefix.len()) == Some(&b'.')
                || fp.name.as_bytes().get(prefix.len()) == Some(&b'_'))
        {
            return Some(fp);
        }
    }
    None
}

/// Chrome / Chromium TLS eras by major version.
///
/// Era boundaries derived from public Chromium release notes + curl-impersonate
/// captures. Within an era, ClientHello bytes are identical (same cipher list,
/// same extensions, same supported_groups, same ALPS payload).
///
/// | Era | Majors | Marker change |
/// |-----|--------|---------------|
/// | E1  | 98-99  | Pre-permute_extensions |
/// | E2  | 100-103 | permute_extensions enabled |
/// | E3  | 104-110 | post-quantum experimentation start |
/// | E4  | 111-116 | X25519Kyber768 |
/// | E5  | 117-123 | ALPS reformat |
/// | E6  | 124-131 | (current curl-impersonate frontier) |
/// | E7  | 132-141 | MLKEM768 (Kyber removed) |
/// | E8  | 142+    | ECH wider deployment |
fn chrome_era_representative(major: u16, os: BrowserOs) -> Option<&'static str> {
    // Use the closest captured Win10 representative for each era. Linux/Mac
    // fingerprints land here once we capture them in Phase 3.
    let _ = os;
    Some(match major {
        0..=98 => "chrome_98.0.4758.102_win10",
        99 => "chrome_99.0.4844.51_win10",
        100 => "chrome_100.0.4896.127_win10",
        101..=103 => "chrome_101.0.4951.67_win10",
        104..=106 => "chrome_104.0.5112.81_win10",
        107..=109 => "chrome_107.0.5304.107_win10",
        110..=115 => "chrome_110.0.5481.177_win10",
        // Era 4-8: until we capture them ourselves, use Chrome 116 as the
        // newest available baseline. tracing::warn alerts operators.
        116..=u16::MAX => "chrome_116.0.5845.180_win10",
    })
}

fn edge_era_representative(major: u16) -> Option<&'static str> {
    Some(match major {
        0..=98 => "edge_98.0.1108.62_win10",
        99..=100 => "edge_99.0.1150.30_win10",
        101..=u16::MAX => "edge_101.0.1210.47_win10",
    })
}

fn firefox_era_representative(major: u16) -> Option<&'static str> {
    Some(match major {
        0..=91 => "firefox_91.6.0esr_win10",
        92..=95 => "firefox_95.0.2_win10",
        96..=98 => "firefox_98.0_win10",
        99..=100 => "firefox_100.0_win10",
        101..=108 => "firefox_102.0_win10",
        109..=116 => "firefox_109.0_win10",
        117..=u16::MAX => "firefox_117.0.1_win10",
    })
}

fn safari_era_representative(major: u16) -> Option<&'static str> {
    // curl-impersonate has 15.3 and 15.5 only.
    Some(match major {
        0..=15 => "safari_15.5_macos12.4",
        _ => "safari_15.5_macos12.4",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_149_resolves_via_era_fallback() {
        let fp = era_for(Browser::Chrome, 149, BrowserOs::Linux).expect("era resolves");
        // Until we capture Chrome 149 directly, era fallback uses chrome_116.
        assert!(fp.name.starts_with("chrome_"), "name = {}", fp.name);
    }

    #[test]
    fn firefox_130_resolves_via_era_fallback() {
        let fp = era_for(Browser::Firefox, 130, BrowserOs::Linux).expect("era resolves");
        assert!(fp.name.starts_with("firefox_"), "name = {}", fp.name);
    }

    #[test]
    fn chromium_122_uses_chrome_era() {
        let fp = era_for(Browser::Chromium, 122, BrowserOs::Linux).expect("era resolves");
        // Chromium falls back to Chrome era representatives until we capture.
        assert!(fp.name.starts_with("chrome_"), "name = {}", fp.name);
    }

    #[test]
    fn safari_18_resolves_via_fallback() {
        let fp = era_for(Browser::Safari, 18, BrowserOs::MacOs).expect("era resolves");
        assert!(fp.name.starts_with("safari_"), "name = {}", fp.name);
    }
}
