//! Where events go. Default sink is stdout NDJSON; `--emit none` uses the
//! null sink; tests use the in-memory sink.

use parking_lot::Mutex;
use std::io::{self, Write};
use std::sync::Arc;

use crate::events::envelope::EventEnvelope;

pub trait EventSink: Send + Sync {
    fn emit(&self, ev: &EventEnvelope);
    fn flush(&self);
}

pub struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _ev: &EventEnvelope) {}
    fn flush(&self) {}
}

/// Writes NDJSON lines to stdout, locked so concurrent tasks don't
/// interleave.
pub struct NdjsonStdoutSink {
    lock: Mutex<()>,
}

impl Default for NdjsonStdoutSink {
    fn default() -> Self {
        Self {
            lock: Mutex::new(()),
        }
    }
}

impl NdjsonStdoutSink {
    /// Construct a new stdout sink. Equivalent to `NdjsonStdoutSink::default()`.
    pub fn create() -> Self {
        Self::default()
    }
}

impl EventSink for NdjsonStdoutSink {
    fn emit(&self, ev: &EventEnvelope) {
        let _g = self.lock.lock();
        let stdout = io::stdout();
        let mut h = stdout.lock();
        let _ = h.write_all(ev.to_ndjson_line().as_bytes());
    }
    fn flush(&self) {
        let _ = io::stdout().lock().flush();
    }
}

/// Collects events in memory. Useful for tests and for in-process
/// SDKs that consume the stream programmatically.
#[derive(Default)]
pub struct MemorySink {
    pub events: Mutex<Vec<EventEnvelope>>,
}

impl MemorySink {
    /// Construct a new in-memory sink. Equivalent to `MemorySink::default()`.
    pub fn create() -> Self {
        Self::default()
    }
    pub fn take(&self) -> Vec<EventEnvelope> {
        std::mem::take(&mut *self.events.lock())
    }
}

impl EventSink for MemorySink {
    fn emit(&self, ev: &EventEnvelope) {
        self.events.lock().push(ev.clone());
    }
    fn flush(&self) {}
}

/// Convenience alias: most code takes an `Arc<dyn EventSink>` so sinks
/// can be swapped at construction time without threading generics through
/// the whole crate.
pub type DynSink = Arc<dyn EventSink>;
