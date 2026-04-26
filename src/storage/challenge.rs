//! `ChallengeStorage` — antibot challenge telemetry sink.
//!
//! Backends that don't care about challenge telemetry inherit the
//! default no-ops; the SQLite backend writes rows to `challenge_events`
//! and exposes `session_challenges` for replay during session triage.

use crate::Result;

/// Antibot challenge telemetry sink.
#[async_trait::async_trait]
pub trait ChallengeStorage: Send + Sync {
    /// Persist a detected antibot challenge. Default no-op.
    async fn record_challenge(&self, _signal: &crate::antibot::ChallengeSignal) -> Result<()> {
        Ok(())
    }

    /// Load every challenge observed for a given session_id, ordered by
    /// observed_at ascending. Default empty.
    async fn session_challenges(
        &self,
        _session_id: &str,
    ) -> Result<Vec<crate::antibot::ChallengeSignal>> {
        Ok(Vec::new())
    }
}
