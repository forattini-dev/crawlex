//! Coherence cross-check between FP-A (target) and FP-B (self).
//!
//! Slice B13 of PRD forattini-dev/crawlex#25. Answers:
//! - our_ja3_matches_profile — does our live JA3 match catalog for
//!   the active Profile?
//! - their_antibot_compatible_with_our_profile — given the detected
//!   antibot vendor, is our profile plausible against it?
//! - warnings — human-readable explanations

use crate::fingerprint::detection::{Detection, Vendor};
use crate::fingerprint::introspect::SelfFingerprint;
use crate::fingerprint::report::Coherence;

/// Vendor families our current Chrome profiles are likely to fail
/// against today. Stub table — grows as we accumulate empirical data
/// (Akamai BotManager + Cloudflare Bot Management are actively
/// flagging Chrome131 JA3 hashes during Q1-Q2 2026 rollouts).
const FLAGGED_VENDORS: &[Vendor] = &[
    Vendor::AkamaiBotManager,
    Vendor::CloudflareBotManagement,
    Vendor::ShapeSecurity,
    Vendor::Kasada,
];

pub fn compute_coherence(
    antibot_detections: &[Detection],
    self_fp: Option<&SelfFingerprint>,
) -> Coherence {
    let mut out = Coherence::default();
    let Some(fp) = self_fp else {
        return out;
    };

    out.our_ja3_matches_profile = fp.matches_profile;
    if fp.matches_profile == Some(false) {
        for sig in &fp.drift_signals {
            out.warnings.push(format!("self-fingerprint drift: {sig}"));
        }
    }

    let flagged = antibot_detections
        .iter()
        .filter(|d| FLAGGED_VENDORS.contains(&d.vendor))
        .collect::<Vec<_>>();
    if flagged.is_empty() {
        out.their_antibot_compatible_with_our_profile = Some(true);
    } else {
        out.their_antibot_compatible_with_our_profile = Some(false);
        for d in flagged {
            out.warnings.push(format!(
                "target detected as {} ({}); current profiles known to be flagged by this vendor — render escalation likely required",
                d.vendor.as_str(),
                d.confidence_label()
            ));
        }
    }

    out
}

/// Add a small helper on Detection — keeps coherence reasoning self-contained.
impl crate::fingerprint::detection::Detection {
    pub fn confidence_label(&self) -> &'static str {
        match self.confidence {
            crate::fingerprint::detection::Confidence::High => "High",
            crate::fingerprint::detection::Confidence::Medium => "Medium",
            crate::fingerprint::detection::Confidence::Low => "Low",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::detection::{Category, Evidence, EvidenceSource};

    fn antibot_det(vendor: Vendor) -> Detection {
        Detection::from_single(
            Category::Antibot,
            vendor,
            Evidence::new(EvidenceSource::Header, "x", 10),
        )
    }

    #[test]
    fn no_self_fp_returns_default() {
        let c = compute_coherence(&[], None);
        assert!(c.our_ja3_matches_profile.is_none());
        assert!(c.their_antibot_compatible_with_our_profile.is_none());
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn clean_profile_no_antibot_is_compatible() {
        let mut fp = SelfFingerprint::default();
        fp.matches_profile = Some(true);
        let c = compute_coherence(&[], Some(&fp));
        assert_eq!(c.our_ja3_matches_profile, Some(true));
        assert_eq!(c.their_antibot_compatible_with_our_profile, Some(true));
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn akamai_bot_manager_flags_profile() {
        let fp = {
            let mut f = SelfFingerprint::default();
            f.matches_profile = Some(true);
            f
        };
        let dets = vec![antibot_det(Vendor::AkamaiBotManager)];
        let c = compute_coherence(&dets, Some(&fp));
        assert_eq!(c.their_antibot_compatible_with_our_profile, Some(false));
        assert!(c.warnings.iter().any(|w| w.contains("Akamai Bot Manager")));
    }

    #[test]
    fn datadome_does_not_flag_profile_in_table_today() {
        let fp = {
            let mut f = SelfFingerprint::default();
            f.matches_profile = Some(true);
            f
        };
        let dets = vec![antibot_det(Vendor::DataDome)];
        let c = compute_coherence(&dets, Some(&fp));
        // DataDome is not in FLAGGED_VENDORS today — profile considered compatible.
        assert_eq!(c.their_antibot_compatible_with_our_profile, Some(true));
    }

    #[test]
    fn ja3_drift_populates_warning() {
        let mut fp = SelfFingerprint::default();
        fp.matches_profile = Some(false);
        fp.drift_signals = vec!["ja3 drift: expected=X got=Y".to_string()];
        let c = compute_coherence(&[], Some(&fp));
        assert_eq!(c.our_ja3_matches_profile, Some(false));
        assert!(c.warnings.iter().any(|w| w.contains("ja3 drift")));
    }
}
