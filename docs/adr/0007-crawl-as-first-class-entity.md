# Crawl as a first-class, resumable entity

Resume did not really exist. `crawl resume` was declared as a subcommand but `cmd_resume` was a stub returning "not yet implemented — use `crawl --queue-path <existing.db>` (with no `--seed`)". The working resume path was implicit and queue-file-scoped: reopen the durable queue backend (SQLite / RedDB), and `run()` re-hydrates by draining whatever pending Jobs still persist. `crawl_id` existed only as a key for `CrawlStats` / `CrawlAttemptRecord` (the attempt ledger), derived per-job by `effective_crawl_id`. There was no registry of Crawls — nothing to `list`, address, or `refresh` — and "one-shot vs full crawl" was not a modelled distinction, only a difference in how you seeded (a single depth-0 seed vs a recursive seed).

That blocks the review's goal of durable, resumable full crawls with a clean one-shot/full-crawl story on the RedDB substrate.

Decision: a **Crawl** is a first-class persisted entity in the RedDB index plane. A `crawls` collection registers each run under a stable `crawl_id` with: its seeds, resolved `PolicyProfile`, scope (one-shot depth-0 vs recursive full + max depth), lifecycle `status` (`running | paused | done | failed`), `started_at` / `finished_at`, and the binding to its durable Frontier and BlobStore. This makes three operations addressable:

- `crawl list` — enumerate Crawls and their status.
- `crawl resume <id>` — implement the stub against the registry: look up the Crawl, reopen its Frontier + Seen-set, continue draining pending Jobs.
- `crawl refresh <id>` — re-seed a `done` Crawl; the Seen-set TTL (ADR-0005) means only URLs older than the TTL re-admit, so refresh is bounded, not a full recrawl.

One-shot and full crawl become the **same** Crawl entity differing only in scope, which is what unifies them on one persisted model.

Crawl vs run. A `crawl_id` identifies the persisted Crawl; the existing `run_id` (carried on `RunStarted` and friends) identifies **one execution** of it. Resuming a Crawl starts a **new `run_id` over the same `crawl_id`**. This is now explicit so event consumers and the attempt ledger do not conflate "the durable thing" with "this process's pass over it".

Consequences and open points:

- **Stale-running detection.** A Crawl whose process dies mid-run stays `status: running` with no live worker. Resume must detect this — via a heartbeat / claim token on the Crawl record — and take it over rather than refuse or double-run. Lifecycle transitions must be idempotent so a crashed-then-resumed Crawl converges. Concrete mechanism (lease TTL vs explicit heartbeat) is settled at implementation time.
- **Ownership.** `crawl_id` graduates from a derived per-job stats key to the stable registry handle; `effective_crawl_id`'s derivation stays as the fallback for Jobs enqueued outside a registered Crawl (backwards compatibility).

Alternatives rejected: keep resume implicit / queue-file-scoped (no `list` / addressable `resume` / `refresh`, and one-shot vs full stays unmodelled); registry-as-observability-only (register for `list` / status but keep resume as blind queue-drain — less unified, and `refresh` has no home).
