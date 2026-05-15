//! RobotsTxt source — Warm tier (B8).
//!
//! Reads `robots.txt` for the host and pattern-matches the
//! `User-agent` and `Disallow` entries for vendor-management
//! signatures. Targets that explicitly block well-known bot UAs
//! (`AhrefsBot`, `SemrushBot`, `MJ12bot`) often run Cloudflare's
//! bot-management list — low-confidence evidence.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct RobotsTxtSource;

impl RobotsTxtSource {
    pub fn new() -> Self {
        Self
    }

    /// Inspect a robots.txt body and emit Detections. Exposed as a
    /// free function so callers (engine warm-tier dispatch) can pass
    /// the robots.txt body they fetched.
    pub fn analyze_body(robots_body: &str) -> Vec<Detection> {
        let lower = robots_body.to_ascii_lowercase();
        let mut out: Vec<Detection> = Vec::new();
        let blocked_ua_list: &[&str] =
            &["ahrefsbot", "semrushbot", "mj12bot", "dotbot", "petalbot"];
        let mut hits = 0;
        for ua in blocked_ua_list {
            if lower.contains(&format!("user-agent: {ua}")) {
                hits += 1;
            }
        }
        if hits >= 3 {
            out.push(Detection::from_single(
                Category::Antibot,
                Vendor::Unknown,
                Evidence::new(
                    EvidenceSource::RobotsTxt,
                    format!(
                        "robots.txt blocks {hits} commercial-bot UAs (cloudflare/imperva managed-list pattern)"
                    ),
                    4,
                ),
            ));
        }
        out
    }
}

impl Source for RobotsTxtSource {
    fn name(&self) -> &'static str {
        "robots_txt"
    }

    fn tier(&self) -> Tier {
        Tier::Warm
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        // robots.txt body lives outside TargetContext today. The
        // engine's Warm-tier dispatch will pass it in via a widened
        // context in B14.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_managed_bot_blocklist() {
        let body = "\
            User-agent: AhrefsBot\nDisallow: /\n\
            User-agent: SemrushBot\nDisallow: /\n\
            User-agent: MJ12bot\nDisallow: /\n";
        let dets = RobotsTxtSource::analyze_body(body);
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].category, Category::Antibot);
    }

    #[test]
    fn does_not_fire_on_innocent_robots() {
        let body = "User-agent: *\nDisallow: /admin\n";
        let dets = RobotsTxtSource::analyze_body(body);
        assert!(dets.is_empty());
    }
}
