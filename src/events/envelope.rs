//! Event envelope and kind enum.
//!
//! `EventEnvelope` is the wire format — everything a consumer sees on stdout.
//! `Event` is the typed payload per kind; consumers that don't care about
//! the typed form can treat `data` as arbitrary JSON.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EVENT_ENVELOPE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub v: u32,
    pub ts: String, // ISO-8601 in UTC.
    pub event: EventKind,
    pub run_id: Option<u64>,
    pub session_id: Option<String>,
    pub url: Option<String>,
    /// Short structured reason (`proxy:bad-score`, `render:js-challenge`,
    /// `retry:5xx`, `budget:exceeded`, ...). Required on
    /// `decision.made`/`job.failed`; optional elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Event-specific payload. Free-form so new kinds can evolve without
    /// breaking consumers that only read a stable subset of fields.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    #[serde(rename = "run.started")]
    RunStarted,
    #[serde(rename = "run.completed")]
    RunCompleted,
    #[serde(rename = "session.created")]
    SessionCreated,
    #[serde(rename = "job.started")]
    JobStarted,
    #[serde(rename = "job.failed")]
    JobFailed,
    #[serde(rename = "decision.made")]
    DecisionMade,
    #[serde(rename = "fetch.completed")]
    FetchCompleted,
    #[serde(rename = "render.completed")]
    RenderCompleted,
    #[serde(rename = "extract.completed")]
    ExtractCompleted,
    #[serde(rename = "artifact.saved")]
    ArtifactSaved,
    #[serde(rename = "proxy.scored")]
    ProxyScored,
    #[serde(rename = "robots.decision")]
    RobotsDecision,
    #[serde(rename = "challenge.detected")]
    ChallengeDetected,
    /// ScriptSpec runner: emitted when a step begins execution. `data`
    /// carries `{ step_id, step_kind }`.
    #[serde(rename = "step.started")]
    StepStarted,
    /// ScriptSpec runner: emitted when a step finishes (either success or
    /// failure). `data` carries
    /// `{ step_id, step_kind, success, duration_ms, error? }`.
    #[serde(rename = "step.completed")]
    StepCompleted,
    /// Fase 6 — session lifecycle: state transitioned (e.g. Clean →
    /// Contaminated). `data` carries `{ from, to, reason? }`.
    #[serde(rename = "session.state_changed")]
    SessionStateChanged,
    /// Fase 6 — session was evicted from the registry (TTL, block,
    /// manual, run-ended). `data` carries
    /// `{ reason, urls_visited, challenges_seen }`.
    #[serde(rename = "session.evicted")]
    SessionEvicted,
    /// Fase 7 (P0-9) — observer noticed an outbound request to a known
    /// antibot-vendor telemetry endpoint. `data` carries
    /// `{ vendor, endpoint, method, payload_size, payload_shape,
    /// pattern_label }`. Emitted *passively* — the vendor may still
    /// decide the session is fine.
    #[serde(rename = "vendor.telemetry_observed")]
    VendorTelemetryObserved,
    #[serde(rename = "tech.fingerprint_detected")]
    TechFingerprintDetected,
}

/// Typed helpers to build common events. Not a closed set — any kind can
/// be produced by constructing `EventEnvelope` directly.
pub struct Event;

/// Payload shape for `artifact.saved` events — the full descriptor a
/// consumer needs to locate/reuse a persisted artifact. Kept as a
/// serialisable struct (instead of an ad-hoc `json!(...)`) so every
/// emit site is guaranteed to ship the same schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSavedData {
    pub kind: String,
    pub mime: String,
    pub size: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
}

impl Event {
    /// Construct an empty envelope for a kind; caller populates the rest.
    /// Named `of` (not `new`) because it returns `EventEnvelope`, not `Self`.
    pub fn of(kind: EventKind) -> EventEnvelope {
        EventEnvelope {
            v: EVENT_ENVELOPE_VERSION,
            ts: now_iso8601(),
            event: kind,
            run_id: None,
            session_id: None,
            url: None,
            why: None,
            data: Value::Null,
        }
    }
}

fn now_iso8601() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_utc();
    // ISO-8601 with ms precision. Falls back to `unix_timestamp` string on
    // the vanishingly rare formatting failure.
    let fmt = time::format_description::well_known::Iso8601::DEFAULT;
    now.format(&fmt)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
}

impl EventEnvelope {
    pub fn with_run(mut self, run_id: u64) -> Self {
        self.run_id = Some(run_id);
        self
    }
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    pub fn with_why(mut self, why: impl Into<String>) -> Self {
        self.why = Some(why.into());
        self
    }
    pub fn with_data<T: Serialize>(mut self, data: &T) -> Self {
        self.data = serde_json::to_value(data).unwrap_or(Value::Null);
        self
    }

    pub fn to_ndjson_line(&self) -> String {
        let mut s = serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"v":1,"event":"serialize.failed"}"#.to_string());
        s.push('\n');
        s
    }
}
