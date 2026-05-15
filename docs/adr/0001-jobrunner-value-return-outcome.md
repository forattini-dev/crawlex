# JobRunner returns a `JobOutcome` by value; live events go through an injected `EventSink`

The `JobRunner` interface is `async fn run(&self, job, ctx) -> JobOutcome`. The outcome is a structured value carrying response, links, signals, timings, retry suggestion, and any new `SessionState`. Live observability (NDJSON events, hook callbacks) is fired from inside `run()` through long-lived `Arc<EventSink>` and `Arc<HookRegistry>` injected at construction — not through a per-call sink parameter.

We considered the alternative of passing a `JobSink` callback into `run()` so the runner could stream events/links/signals as they happen. Rejected because: (1) tests assert on a returned value trivially, but would need a mock sink in the callback shape; (2) there is no real streaming consumer today — `crawler.rs` already buffers everything before persisting; (3) `EventSink` is already a long-lived shared dependency, so per-call sinks would be a new and asymmetric pattern. One adapter would have been a hypothetical seam.

The constraint this locks in: `JobRunner` must stay `Send + Sync` and free of per-call mutable state on `self`. Anything that mutates per attempt belongs in `SessionContext` (input) or `JobOutcome` (output). Reversing this decision means changing every `JobRunner` impl, every call site, and every test fixture — meaningful enough to record.
