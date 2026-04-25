//! NDJSON event bus — public contract between the crawler runtime and any
//! external consumer (CLI piping, SDKs, hooks).
//!
//! The envelope is versioned (`v: 1`) and every event carries
//! `run_id`/`session_id` when available, so multiple concurrent runs can
//! share a single stream. `data` is event-specific; `why` is a short
//! structured reason used by `decision.made` and `job.failed`.
//!
//! Consumers should treat *unknown event kinds* as forward-compatible and
//! ignore them. Event kind names are lowercase with `.` separators and are
//! part of the stable contract.

pub mod envelope;
pub mod sink;

pub use envelope::{ArtifactSavedData, Event, EventEnvelope, EventKind};
pub use sink::{EventSink, MemorySink, NdjsonStdoutSink, NullSink};
