//! Static catalog: expected JA3 / JA4 / h2_fp per `impersonate::Profile`.
//!
//! Slice B11 of PRD forattini-dev/crawlex#25. Drift between live
//! capture and catalog populates `SelfFingerprint.drift_signals` —
//! alarms when BoringSSL regresses or a proxy / middlebox alters
//! our handshake.
//!
//! Hash values are placeholder until the live capture pipeline
//! (B14) records authoritative bytes for each profile. The catalog
//! schema is the load-bearing piece here; values get updated as the
//! capture pipeline produces real measurements.

use super::ProfileExpected;

/// Returns the catalog entry for a profile name string. Profile
/// matching is by `Debug` representation today (e.g.
/// `"Chrome131Stable"`) — moves to the typed `impersonate::Profile`
/// enum once the call sites land in B14.
pub fn lookup_by_name(profile_name: &str) -> Option<ProfileExpected> {
    match profile_name {
        "Chrome131Stable" => Some(ProfileExpected {
            profile_name: "Chrome131Stable".into(),
            ja3_hash: Some("PLACEHOLDER_chrome131_ja3_md5".into()),
            ja4: Some("t13d1517h2_PLACEHOLDER_chrome131_ja4".into()),
            h2_settings_fp: Some("PLACEHOLDER_chrome131_h2_settings".into()),
        }),
        "Chrome132Stable" => Some(ProfileExpected {
            profile_name: "Chrome132Stable".into(),
            ja3_hash: Some("PLACEHOLDER_chrome132_ja3_md5".into()),
            ja4: Some("t13d1517h2_PLACEHOLDER_chrome132_ja4".into()),
            h2_settings_fp: Some("PLACEHOLDER_chrome132_h2_settings".into()),
        }),
        "Chrome149Stable" => Some(ProfileExpected {
            profile_name: "Chrome149Stable".into(),
            ja3_hash: Some("PLACEHOLDER_chrome149_ja3_md5".into()),
            ja4: Some("t13d1517h2_PLACEHOLDER_chrome149_ja4".into()),
            h2_settings_fp: Some("PLACEHOLDER_chrome149_h2_settings".into()),
        }),
        _ => None,
    }
}

/// Compare a measured fingerprint against catalog. Returns true when
/// every populated catalog field matches; collects drift descriptions
/// into `out`.
pub fn diff_against(
    measured_ja3_hash: Option<&str>,
    measured_ja4: Option<&str>,
    measured_h2_fp: Option<&str>,
    expected: &ProfileExpected,
    out: &mut Vec<String>,
) -> bool {
    let mut ok = true;
    if let (Some(exp), Some(got)) = (expected.ja3_hash.as_deref(), measured_ja3_hash) {
        if exp != got {
            out.push(format!("ja3 drift: expected={exp} got={got}"));
            ok = false;
        }
    }
    if let (Some(exp), Some(got)) = (expected.ja4.as_deref(), measured_ja4) {
        if exp != got {
            out.push(format!("ja4 drift: expected={exp} got={got}"));
            ok = false;
        }
    }
    if let (Some(exp), Some(got)) = (expected.h2_settings_fp.as_deref(), measured_h2_fp) {
        if exp != got {
            out.push(format!("h2_settings drift: expected={exp} got={got}"));
            ok = false;
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_chrome131() {
        assert!(lookup_by_name("Chrome131Stable").is_some());
    }

    #[test]
    fn catalog_unknown_profile_is_none() {
        assert!(lookup_by_name("FooBar").is_none());
    }

    #[test]
    fn diff_clean_when_all_match() {
        let expected = lookup_by_name("Chrome131Stable").unwrap();
        let mut drift: Vec<String> = Vec::new();
        let ok = diff_against(
            Some("PLACEHOLDER_chrome131_ja3_md5"),
            Some("t13d1517h2_PLACEHOLDER_chrome131_ja4"),
            Some("PLACEHOLDER_chrome131_h2_settings"),
            &expected,
            &mut drift,
        );
        assert!(ok);
        assert!(drift.is_empty());
    }

    #[test]
    fn diff_drifts_when_ja3_diverges() {
        let expected = lookup_by_name("Chrome131Stable").unwrap();
        let mut drift: Vec<String> = Vec::new();
        let ok = diff_against(
            Some("different_hash"),
            None,
            None,
            &expected,
            &mut drift,
        );
        assert!(!ok);
        assert_eq!(drift.len(), 1);
        assert!(drift[0].starts_with("ja3 drift"));
    }
}
