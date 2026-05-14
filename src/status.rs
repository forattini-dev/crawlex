//! Canonical status taxonomy (slice 1).
//!
//! `Status` is the per-URL lifecycle label written to SQLite, shipped in
//! NDJSON events, and exposed on the TS SDK. `TerminalReason` is the
//! matching per-job terminal label — populated when a run ends.
//!
//! Wire strings are stable (snake_case) and form part of the public
//! contract. Adding a variant is a minor-version event-envelope bump;
//! removing one is a major-version break.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Per-URL lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Enqueued, awaiting a worker.
    Queued,
    /// Fetched (or fetched + rendered) and persisted to storage.
    Completed,
    /// Skipped because robots.txt or an ACL forbade the fetch.
    Disallowed,
    /// Skipped for a non-policy reason (already-seen, depth budget,
    /// link-filter, host-not-allowed, ...).
    Skipped,
    /// Attempt(s) exhausted with a hard failure (network, 5xx, render).
    Errored,
    /// Cancelled mid-flight — typically by run-level budget cancellation
    /// or operator stop.
    Cancelled,
}

impl Status {
    /// Stable wire string written to SQLite `pages.crawl_status` and
    /// shipped on `EventEnvelope.status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Completed => "completed",
            Status::Disallowed => "disallowed",
            Status::Skipped => "skipped",
            Status::Errored => "errored",
            Status::Cancelled => "cancelled",
        }
    }

    /// Every variant — handy for tests and CLI `--help` rendering.
    pub fn all() -> &'static [Status] {
        &[
            Status::Queued,
            Status::Completed,
            Status::Disallowed,
            Status::Skipped,
            Status::Errored,
            Status::Cancelled,
        ]
    }
}

impl FromStr for Status {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "queued" => Status::Queued,
            "completed" => Status::Completed,
            "disallowed" => Status::Disallowed,
            "skipped" => Status::Skipped,
            "errored" => Status::Errored,
            "cancelled" => Status::Cancelled,
            other => return Err(format!("unknown status `{other}`")),
        })
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-job terminal label — populated once the run ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReason {
    /// Drained queue cleanly.
    Completed,
    /// Aborted by a hard error before drain.
    Errored,
    /// Stopped because `--timeout` elapsed.
    CancelledDueToTimeout,
    /// Stopped because a limit was hit (`--max-pages`, budget, ...).
    CancelledDueToLimits,
    /// Operator pressed Ctrl-C / sent SIGTERM.
    CancelledByUser,
}

impl TerminalReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalReason::Completed => "completed",
            TerminalReason::Errored => "errored",
            TerminalReason::CancelledDueToTimeout => "cancelled_due_to_timeout",
            TerminalReason::CancelledDueToLimits => "cancelled_due_to_limits",
            TerminalReason::CancelledByUser => "cancelled_by_user",
        }
    }

    pub fn all() -> &'static [TerminalReason] {
        &[
            TerminalReason::Completed,
            TerminalReason::Errored,
            TerminalReason::CancelledDueToTimeout,
            TerminalReason::CancelledDueToLimits,
            TerminalReason::CancelledByUser,
        ]
    }
}

impl FromStr for TerminalReason {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "completed" => TerminalReason::Completed,
            "errored" => TerminalReason::Errored,
            "cancelled_due_to_timeout" => TerminalReason::CancelledDueToTimeout,
            "cancelled_due_to_limits" => TerminalReason::CancelledDueToLimits,
            "cancelled_by_user" => TerminalReason::CancelledByUser,
            other => return Err(format!("unknown terminal_reason `{other}`")),
        })
    }
}

impl std::fmt::Display for TerminalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrip_via_wire() {
        for s in Status::all() {
            let w = s.as_str();
            assert_eq!(Status::from_str(w).unwrap(), *s);
        }
    }

    #[test]
    fn status_serde_snake_case() {
        let json = serde_json::to_string(&Status::Cancelled).unwrap();
        assert_eq!(json, "\"cancelled\"");
        let back: Status = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Status::Cancelled);
    }

    #[test]
    fn terminal_reason_roundtrip() {
        for r in TerminalReason::all() {
            assert_eq!(TerminalReason::from_str(r.as_str()).unwrap(), *r);
        }
    }

    #[test]
    fn terminal_reason_serde_snake_case() {
        let json = serde_json::to_string(&TerminalReason::CancelledDueToTimeout).unwrap();
        assert_eq!(json, "\"cancelled_due_to_timeout\"");
    }

    #[test]
    fn status_unknown_value_errors() {
        assert!(Status::from_str("nope").is_err());
        assert!(TerminalReason::from_str("nope").is_err());
    }
}
