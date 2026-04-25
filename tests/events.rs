//! Tests for the NDJSON event envelope and sinks.

use crawlex::events::{Event, EventKind, EventSink, MemorySink};
use serde_json::Value;

#[test]
fn envelope_serializes_in_expected_shape() {
    let ev = Event::of(EventKind::DecisionMade)
        .with_run(42)
        .with_url("https://example.com/")
        .with_session("sess_abc")
        .with_why("render:js-only-content");
    let line = ev.to_ndjson_line();
    assert!(line.ends_with('\n'));
    let parsed: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["v"], 1);
    assert_eq!(parsed["event"], "decision.made");
    assert_eq!(parsed["run_id"], 42);
    assert_eq!(parsed["url"], "https://example.com/");
    assert_eq!(parsed["session_id"], "sess_abc");
    assert_eq!(parsed["why"], "render:js-only-content");
}

#[test]
fn envelope_drops_absent_why() {
    let ev = Event::of(EventKind::FetchCompleted).with_run(1);
    let line = ev.to_ndjson_line();
    // When `why` is None, serde should skip the field entirely.
    assert!(!line.contains("\"why\""));
}

#[test]
fn memory_sink_captures_in_order() {
    let sink: &dyn EventSink = &MemorySink::create();
    for i in 0..5 {
        sink.emit(&Event::of(EventKind::JobStarted).with_run(i));
    }
    // Re-cast through MemorySink to take the buffer.
    let mem = MemorySink::create();
    for i in 0..3 {
        mem.emit(&Event::of(EventKind::ArtifactSaved).with_run(i));
    }
    let captured = mem.take();
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[0].run_id, Some(0));
    assert_eq!(captured[2].run_id, Some(2));
}

#[test]
fn event_kind_tags_match_spec() {
    use crawlex::events::envelope::EventEnvelope as EE;
    // Round-trip via JSON to confirm the wire tags.
    let ev = Event::of(EventKind::RunCompleted);
    let json = serde_json::to_string(&ev).unwrap();
    let back: EE = serde_json::from_str(&json).unwrap();
    assert_eq!(back.event, EventKind::RunCompleted);
    assert!(json.contains(r#""event":"run.completed""#));
}
