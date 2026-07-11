# Host-eligibility frontier scheduler

The Frontier pops the highest-priority Job and stops there. `queue::reddb::pop()` orders by `priority` (with `DELAY {ms}` support for backoff / politeness), and `crawler::priority_for_discovered` awards a `same_host_bonus`, biasing the crawl depth-first within a host. Under concurrency that is precisely the same-domain thrash the web-crawler reference warns about: multiple workers pile onto one host, hit the `HostRateLimiter` / cadence envelope, get requeued with a delay, and burn cycles — and it also reads as a bot, hammering one domain's pages back-to-back rather than interleaving the way a human would. Separately, robots `crawl-delay` is never parsed or enforced (`discovery::robots_paths` extracts only *paths* for discovery; `HostRateLimiter` uses a default `rps`), and `last_crawled` does not drive scheduling.

This is the point where several earlier decisions converge — Seen-set TTL refresh (ADR-0005), the CadenceGovernor's per-host pacing (ADR-0009), the per-host StealthProfile (ADR-0008), and politeness — so scheduling has to become host-aware rather than a flat priority pop.

Decision: the Frontier becomes a **host-eligibility scheduler**. It pops the highest-priority Job **among the hosts whose `next_allowed_at` has arrived**, where `next_allowed_at = max(HostRateLimiter rps floor, robots crawl-delay, CadenceGovernor envelope)`. Eligible hosts are interleaved so workers stay busy without any single host being hammered. Per-host schedule state — `next_allowed_at`, `last_crawled` — is persisted in the RedDB index plane, so a resumed or refreshed Crawl preserves pacing instead of re-hammering a host it just visited. robots `crawl-delay` is parsed, persisted (into `host_facts` / the StealthProfile), and enforced as one of the inputs to `next_allowed_at`.

The `same_host_bonus` is reframed rather than deleted: same-host preference (finishing a site's coverage) becomes a **tiebreak within the eligible set**, not a global ordering bias that forces depth-first thrash.

Consequences:

- **Fairness / starvation.** A very large host, or one permanently in cooldown, must not starve others. The eligible set is served round-robin (with priority as the within-host order and aging to avoid indefinite deferral).
- **Worker idling.** When no host is eligible, workers sleep until the soonest `next_allowed_at` (a `min` over the per-host schedule) rather than busy-looping the queue.
- **Resume correctness.** Because per-host schedule state is durable, a resumed Crawl does not reset cooldowns and re-hammer hosts — the politeness/cadence memory survives the restart alongside the Frontier.

Alternatives rejected: priority-pop plus a cooldown filter (does not truly interleave — a host with many high-priority Jobs still dominates ordering); keep as-is (same-domain thrash, no `crawl-delay`, and a bot-like access pattern).
