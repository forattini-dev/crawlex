# `AutoFetcher` escalates spoof→render by re-queueing, not by chaining inline

When `AutoFetcher` runs the spoof path and detects an antibot challenge, it does **not** call the render path inline. Instead, the `JobRunner` returns a `JobOutcome` carrying `RetryDecision::Suggest { reason: EscalateToRender, .. }`, and the `Crawler` re-enqueues the same `Job` with `method = Render`. The render attempt is a fresh `Job` going through admission again.

We considered chaining inline: spoof → detect challenge → call render directly inside `AutoFetcher::fetch`. Rejected because: (1) inline chaining doubles the time budget of one `Job` invisibly and breaks parity with the existing NDJSON event contract; (2) re-queuing forces the render attempt back through admission, so host cooldowns, render-pool availability, and per-attempt budgets are honored without duplicate code paths; (3) every other `Fetcher` impl is a single attempt — making `AutoFetcher` the only one that internally composes would be asymmetric and would push retry/error/timing logic into the adapter layer.

The consequence: `AutoFetcher` is a thin composition that picks one path per attempt. Anyone tempted to "optimize" by chaining inside the adapter to save a queue round-trip should consult this ADR first — the latency saving is real, but the cost is uniformity of the retry/budget/event story across all three fetch methods.
