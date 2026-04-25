//! Human-handoff passthrough — SCAFFOLD (issue #37).
//!
//! When the policy engine classifies a challenge as *unsolvable by the
//! stack* (CF JS challenge post-3-retries, a sudden KYC form, bank 2FA),
//! the operator may want to pause the job and take the wheel instead of
//! dropping the URL. This scaffold defines the pause contract + a TUI
//! prompt helper. It does NOT (yet) modify `crate::policy::Decision`
//! — adding a new variant to that enum is a breaking change that belongs
//! in the "policy-engine evolution" wave, not the scaffold wave. Until
//! then, the crawler can consult `HandoffRequest::should_pause` directly
//! from a Lua hook or the Policy preset via out-of-band glue.
//!
//! Integration sketch (documented in `docs/infra-tier-operator.md`):
//! 1. Operator enables handoff mode via `CRAWLEX_HANDOFF=1` or a future
//!    `--handoff` CLI flag (disabled by default).
//! 2. Scheduler calls [`HandoffRequest::pause_and_wait`] when a job
//!    returns a `ChallengeLevel::HardBlock` the policy couldn't recover.
//! 3. The function prints the URL + screenshot path to stderr and blocks
//!    on stdin `Enter`. Operator solves the challenge in their own
//!    browser, copies cookies back via `crawlex sessions import`, then
//!    presses Enter to resume.

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::antibot::ChallengeSignal;

/// Local handoff decision — policy-adjacent but kept self-contained so the
/// main `Decision` enum can stay untouched in this wave. A future wave
/// will fold `HumanHandoff` into `crate::policy::Decision` and wire it
/// through the scheduler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HandoffDecision {
    /// No human needed — the caller keeps running the existing policy.
    Skip,
    /// Pause the job and prompt the operator. Carries the artifacts the
    /// operator needs to reproduce the challenge.
    Pause {
        reason: &'static str,
        url: url::Url,
        screenshot_path: Option<PathBuf>,
    },
}

/// Environment variable flipping handoff on. Default off.
pub const CRAWLEX_HANDOFF_ENV: &str = "CRAWLEX_HANDOFF";

/// Whether handoff is enabled for this process. Cheap — reads env once
/// but not cached, so tests can toggle.
pub fn handoff_enabled() -> bool {
    matches!(
        std::env::var(CRAWLEX_HANDOFF_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

/// Whether a given challenge signal qualifies for handoff. Scaffold
/// policy: only `HardBlock` levels and only when handoff is enabled.
/// Real policy integration will consult
/// `crate::policy::engine::PolicyThresholds` to allow operator-tuned
/// thresholds.
pub fn should_handoff(signal: &ChallengeSignal) -> bool {
    handoff_enabled()
        && matches!(
            signal.level,
            crate::antibot::ChallengeLevel::HardBlock
                | crate::antibot::ChallengeLevel::ChallengePage
        )
}

/// Request carrying everything the operator needs while paused. The
/// scheduler builds this and calls `pause_and_wait`.
#[derive(Debug, Clone)]
pub struct HandoffRequest {
    pub url: url::Url,
    pub screenshot_path: Option<PathBuf>,
    pub reason: &'static str,
    pub vendor: Option<crate::antibot::ChallengeVendor>,
}

impl HandoffRequest {
    pub fn from_signal(signal: &ChallengeSignal, screenshot_path: Option<PathBuf>) -> Self {
        Self {
            url: signal.url.clone(),
            screenshot_path,
            reason: signal.level.as_str(),
            vendor: Some(signal.vendor),
        }
    }

    /// Lift this request into a `Decision::HumanHandoff` variant so the
    /// scheduler and NDJSON sink can pipe it through the canonical policy
    /// path instead of the previous `HandoffDecision` side-channel.
    pub fn into_policy_decision(self) -> crate::policy::Decision {
        crate::policy::Decision::HumanHandoff {
            reason: self.reason.to_string(),
            vendor: self.vendor.map(|v| v.as_str().to_string()),
            url: self.url,
            screenshot_path: self.screenshot_path,
        }
    }

    /// True when the scheduler should block on this request. Re-runs the
    /// enabled-check so a toggle between signal and pause honours it.
    pub fn should_pause(&self) -> bool {
        handoff_enabled()
    }

    /// Emit the TUI prompt on `out`, then read a single line from `rd`.
    /// The split between writer and reader keeps the function testable
    /// without stdin.
    ///
    /// Returns `Ok(())` when the operator presses Enter; returns
    /// `Err(io::ErrorKind::Interrupted)` if the reader hit EOF before a
    /// newline (e.g. stdin closed — treat as "abort handoff").
    pub fn render_prompt<W: Write, R: BufRead>(&self, out: &mut W, rd: &mut R) -> io::Result<()> {
        writeln!(out)?;
        writeln!(out, "────────────────────────────────────────")?;
        writeln!(out, " crawlex :: human-handoff requested")?;
        writeln!(out, "────────────────────────────────────────")?;
        writeln!(out, "  reason   : {}", self.reason)?;
        if let Some(v) = self.vendor {
            writeln!(out, "  vendor   : {}", v.as_str())?;
        }
        writeln!(out, "  url      : {}", self.url)?;
        if let Some(p) = &self.screenshot_path {
            writeln!(out, "  snapshot : {}", p.display())?;
        }
        writeln!(out)?;
        writeln!(
            out,
            "  Solve the challenge in your own browser, copy session"
        )?;
        writeln!(
            out,
            "  cookies back via `crawlex sessions import`, then press"
        )?;
        writeln!(out, "  Enter here to resume the crawl.")?;
        writeln!(out, "────────────────────────────────────────")?;
        out.flush()?;

        let mut line = String::new();
        let n = rd.read_line(&mut line)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "stdin closed during handoff",
            ));
        }
        Ok(())
    }

    /// Block on the real stdin/stderr. Returns `Ok(())` on operator ack.
    /// Called from the scheduler (not yet wired — see module docs).
    pub fn pause_and_wait(&self) -> io::Result<()> {
        let stdout = io::stderr();
        let mut out = stdout.lock();
        let stdin = io::stdin();
        let mut lock = stdin.lock();
        self.render_prompt(&mut out, &mut lock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_url() -> url::Url {
        url::Url::parse("https://example.com/login").unwrap()
    }

    #[test]
    fn handoff_disabled_by_default() {
        // unset for a clean run — may leak from operator shell in CI, so
        // we assert the happy-path explicitly.
        std::env::remove_var(CRAWLEX_HANDOFF_ENV);
        assert!(!handoff_enabled());
    }

    #[test]
    fn should_handoff_requires_enabled() {
        std::env::remove_var(CRAWLEX_HANDOFF_ENV);
        let sig = ChallengeSignal {
            vendor: crate::antibot::ChallengeVendor::CloudflareJsChallenge,
            level: crate::antibot::ChallengeLevel::HardBlock,
            url: sample_url(),
            origin: "https://example.com".into(),
            proxy: None,
            session_id: "s".into(),
            first_seen: std::time::SystemTime::now(),
            metadata: serde_json::Value::Null,
        };
        assert!(!should_handoff(&sig));
    }

    #[test]
    fn prompt_writes_url_and_reason() {
        let req = HandoffRequest {
            url: sample_url(),
            screenshot_path: Some(PathBuf::from("/tmp/shot.png")),
            reason: "hard_block",
            vendor: Some(crate::antibot::ChallengeVendor::CloudflareJsChallenge),
        };
        let mut out = Vec::new();
        let mut rd = Cursor::new(b"\n".to_vec());
        req.render_prompt(&mut out, &mut rd).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("human-handoff requested"));
        assert!(s.contains("example.com/login"));
        assert!(s.contains("hard_block"));
        assert!(s.contains("/tmp/shot.png"));
    }

    #[test]
    fn eof_is_translated_to_interrupted() {
        let req = HandoffRequest {
            url: sample_url(),
            screenshot_path: None,
            reason: "hard_block",
            vendor: None,
        };
        let mut out = Vec::new();
        let mut rd = Cursor::new(Vec::<u8>::new());
        let err = req.render_prompt(&mut out, &mut rd).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }
}
