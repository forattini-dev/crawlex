# PRD: crawlex v2.0 — Full Scraping Framework

**Labels:** needs-triage, prd, breaking-change, epic
**Status:** Draft

## Problem Statement

Crawlex today is a stealth crawler — it discovers URLs, fetches with byte-perfect Chrome TLS/H2, runs a render pool, and persists results. Users who want to extract structured data still have to plug their own HTML parser, write their own retry/relocation logic when selectors break, and stitch streaming into their own pipelines.

Meanwhile, D4Vinci/Scrapling (Python) is winning mindshare with bold claims: an adaptive parser that relocates elements when pages change, a Scrapy-like spider DSL with pause/resume and streaming, a built-in MCP server for AI-assisted scraping, an interactive shell, curl-to-config conversion, ad-blocking, DoH, and "out-of-the-box" Cloudflare Turnstile bypass. Users comparing the two will see crawlex as a lower-level building block — even though crawlex already beats Scrapling on stealth (chrome-perfect TLS+H2 vs curl_cffi, native reCAPTCHA v3 solver, DoH) and performance (Rust core vs Python).

We need to close the framework gap so the comparison is on our terms.

## Solution

Ship crawlex v2.0 as a full scraping framework. Add an adaptive HTML parser with element relocation, a complete selector API (CSS/XPath/text/regex/filter/find-similar/navigation), a functional `defineSpider({...})` DSL with streaming items, an MCP server, an interactive shell, curl-to-config, dev replay cache, and ad-blocking — while keeping crawlex's stealth, polyglot SDK (Rust+Node+CLI+Lua), and persistent queue advantages.

v2.0 is a clean break from v1; legacy `crawl()` and v1 hooks API are removed. A migration guide ships with the release.

## User Stories

### Adaptive parsing
1. As a scraper maintainer, I want my CSS selectors to keep working when the target site changes its DOM, so that my pipeline doesn't break overnight.
2. As a scraper maintainer, I want to give each tracked element a stable identifier, so that future runs can relocate it via similarity matching.
3. As a scraper maintainer, I want to tune the similarity threshold per identifier, so that I trade recall vs precision for each field.
4. As a scraper maintainer, I want the adaptive store scoped per spider+domain, so that two spiders targeting overlapping domains don't collide.
5. As a scraper maintainer, I want adaptive matches logged with a confidence score, so that I can audit drift in production.

### Selector API
6. As a developer migrating from Scrapling, I want CSS selectors with the same pseudo-element semantics as Scrapy/Parsel, so that my queries port over.
7. As a developer, I want XPath support alongside CSS, so that I can query by axes not expressible in CSS.
8. As a developer, I want `findByText(...)` and `findByRegex(...)` helpers, so that I can target elements by content when attributes are unstable.
9. As a developer, I want a `filter(fn)` predicate, so that I can compose arbitrary matching logic.
10. As a developer, I want `findSimilar(element)`, so that I can locate sibling rows once I've anchored one.
11. As a developer, I want `.parent`, `.siblings`, and `.children` navigation, so that I can walk the DOM without re-querying.
12. As a developer, I want auto-generated robust selectors for any element, so that I can capture a node interactively and ship the selector.

### Spider DSL
13. As a developer, I want a functional `defineSpider({ startUrls, parse })` API, so that I don't need class inheritance and it composes with `defineHooks`.
14. As a developer, I want my `parse` callback to be an async generator yielding either items or new `Request` objects, so that link discovery and extraction live in one place.
15. As a developer, I want per-request `session_id`, so that a single spider can route some requests through fast HTTP and others through a stealth browser.
16. As a developer, I want to pause my crawl with Ctrl+C and resume from the same checkpoint, so that I survive operator interruptions and machine restarts.
17. As a developer, I want `async for item of spider.stream()` (Node) and an equivalent Rust stream, so that I can pipe items into downstream processors as they arrive.
18. As a developer, I want per-domain throttle and download delay, so that I don't get rate-limited or banned.
19. As a developer, I want optional robots.txt obedience, so that I can stay compliant without writing my own parser.
20. As a developer, I want a dev-replay mode that records responses on first run and replays them on subsequent runs, so that I can iterate on `parse` logic without re-hitting the target.

### MCP server
21. As an AI engineer, I want a `crawlex mcp` standalone binary, so that I can wire crawlex into Claude/Cursor without managing a Python runtime.
22. As an AI engineer, I want MCP tools that return pre-extracted markdown/text instead of raw HTML, so that I minimize tokens passed to the model.
23. As an AI engineer, I want MCP-managed stealth/dynamic browser sessions, so that LLM-driven scrapes can bypass Cloudflare and reCAPTCHA the same way scripted scrapes do.
24. As an AI engineer, I want MCP tools for CSS/XPath query on a fetched page, so that the model can drill into structure without me handing it all the HTML.

### Devex
25. As a developer, I want `crawlex shell` to drop me into a Rust REPL pre-loaded with crawlex helpers, so that I can prototype scrapes interactively.
26. As a JS-first developer, I want an opt-in `npx crawlex shell` Node REPL with the SDK loaded, so that I can prototype in TypeScript.
27. As a developer, I want `crawlex from-curl 'curl ...'` to generate a config, so that I can convert browser-devtools-copied requests into crawlex runs.
28. As an SRE, I want auto ad-blocking of ~3,500 known tracker domains plus an opt-in EasyList updater, so that headless renders don't waste bandwidth and time on ads.
29. As an SRE, I want DoH for DNS resolution, so that proxy-routed traffic doesn't leak DNS to the network operator.

### Migration / docs
30. As an existing crawlex 1.x user, I want a migration guide mapping every v1 API to its v2 equivalent, so that I can upgrade without guesswork.
31. As a developer evaluating crawlex vs Scrapling, I want a feature parity matrix, so that I can confirm crawlex covers my use case.

## Implementation Decisions

### Scope and release strategy
- v2.0 is a **big-bang breaking release**. v1 APIs are removed.
- No public benchmark comparison vs Scrapling. Marketing positions on parity + stealth/perf differentiators.
- Test coverage target ≥80%, internal, not a public claim.

### Deep modules (testable in isolation)

1. **parser** — HTML bytes → tree (via `scraper`/html5ever) or streaming events (via `lol_html`). The caller picks based on use case: tree for adaptive matching and navigation, streaming for high-throughput rewriting/extraction.
2. **selectors** — query (CSS, XPath, `findByText`, `findByRegex`, `filter`, navigation) → element set. Sits on top of `parser` tree mode.
3. **adaptive** — element fingerprint (tag + attribute subset + text hash + parent chain + sibling position) persisted in a per-spider reddb store, scoped by `spider+domain+identifier`. On selector failure, walks the DOM and returns the highest-scoring candidate above threshold.
4. **similarity** — pure function over two element representations, returns 0..1. Algorithm is a 1:1 port of Scrapling's weighted formula (tag, attributes split per id/class/href/other, text via sequence similarity, parent chain, sibling position).
5. **session_router** — `Request.session_id` → engine backend (HTTP / render / stealth). SessionManager owns the session_id ↔ backend registry, including cookie isolation between sessions of the same type.
6. **adblock** — URL → blocked/allow. Ships a bundled baseline list (~3,500 domains, `include_str!`) and exposes `crawlex update-blocklist` to fetch EasyList for richer coverage.
7. **replay** — record-on-first-hit, replay-on-subsequent. Backend is user-pickable: an on-disk directory of `{url-hash}.json+body` files OR the reddb store.
8. **from_curl** — curl command string → crawlex config struct.

### Shallow / integration modules
9. **spider** runtime — implements `defineSpider({ startUrls, parse: async function*(res) {...} })`, drives the crawl, marshals `Request`/`Response`, integrates with the existing scheduler/queue/checkpoint.
10. **stream** — emits a new `EventKind::ItemScraped` variant on the existing event bus; `spider.stream()` is a filtered consumer of that bus. Single bus, no parallel mpsc.
11. **mcp** — standalone `crawlex mcp` binary built on `rmcp`. Tools mirror Scrapling's surface: `open_session` / `close_session` / `list_sessions`, `get` / `bulk_get` (HTTP), `fetch` (dynamic), stealth fetch, plus CSS/XPath query on a fetched page that returns markdown/text rather than HTML.
12. **shell** — Rust REPL via `rustyline` is the default (`crawlex shell`); a Node REPL via `npx crawlex shell` is shipped as opt-in.
13. **Node SDK extensions** — TS-side `defineSpider`, selector wrappers, `for await` stream iterator. Calls into Rust core via the existing NAPI bridge.

### Architectural decisions

- **Embedded store:** adaptive fingerprints and (optionally) replay bodies live in **reddb-io/reddb** (the specific embedded DB the user named — *not* cberner/redb). One DB file per spider.
- **Adaptive scope:** identifier is keyed by `spider_id + domain + identifier`, not Scrapling's `domain + identifier`. Two spiders can reuse identifier names without collision.
- **HTML parser dual-track:** `scraper` for tree operations (selectors, adaptive matching, navigation). `lol_html` for streaming rewrite/extract paths where holding a full DOM is wasteful. Public API documents which methods use which backend.
- **Session routing:** `Request.session_id` is a free-form string. `SessionManager` maintains the session_id → engine registry. The existing `method` enum (http|render|stealth) becomes the *backend type* of a session, not a per-request switch.
- **Streaming:** items flow on the same event bus as existing events as a new `EventKind::ItemScraped` variant. No dedicated mpsc channel.
- **Stealth carry-over:** crawlex's existing antibot module (TLS+H2 fingerprint, reCAPTCHA v3 solver, CF challenge handling, cookie pinning, fingerprint signatures, DoH) is preserved unchanged and is the stealth backbone for the new spider DSL.
- **CLI surface additions:** `crawlex mcp`, `crawlex shell`, `crawlex from-curl`, `crawlex update-blocklist`.

### Schema / contract changes
- New `EventKind::ItemScraped { spider_id, identifier?, payload }`. Event contract version bump per `docs/architecture/04-events-hooks-sdk.md`.
- New reddb schema per spider: `fingerprints` (key: `domain|identifier`, value: serialized fingerprint), `replay` (key: `request_hash`, value: serialized response).
- `Request` gains optional `session_id: String`. `Response` gains adaptive-match metadata when selectors used `identifier`.
- v1 APIs removed: `crawl()` entrypoint, v1 hook signatures. v2 single entrypoint is `defineSpider({...}).start()` / `crawlex run`.

## Testing Decisions

### What makes a good test here
- Test the **deep module's external behavior**, not its internal data structures. Example: for `adaptive`, assert that given fixture HTML A (training) and a mutated HTML B (target), `relocate("price_label")` returns the semantically equivalent element — never assert on internal fingerprint bytes.
- Use **real-world fixtures** (snapshots from actual scraped sites at two points in time) for adaptive recall tests, not synthetic minimal HTML — drift behavior on synthetic input doesn't predict production behavior.
- Stealth/integration tests (CreepJS, peet.ws, fingerprintjs/sannysoft, CF Turnstile demo pages) carry more weight than coverage % for the antibot surface — that's already established in the existing test suite and continues.

### Modules with required test suites in v2.0
- **parser** — tree correctness vs fixtures (malformed HTML, CDATA, encoding edge cases) and streaming event ordering for `lol_html` paths.
- **selectors** — CSS/XPath parity with Scrapy semantics; `findByText`/`findByRegex`/`filter` coverage; navigation correctness (parent/siblings/children).
- **adaptive + similarity** — recall on real before/after DOM mutation pairs. Threshold sweep. Cross-spider isolation. Storage round-trip via reddb.
- **session_router** — session_id → backend resolution, cookie isolation between sessions of the same type, fallback when a session_id is not registered.
- **adblock** — URL/subdomain matching against bundled list, EasyList update flow, exempt-list behavior.
- **replay** — record-then-replay round-trip, both directory and reddb backends; behavior on cache miss; deterministic key hashing.
- **from_curl** — curl flag coverage (headers, cookies, data, method, redirects, proxy) → config struct equivalence.

### Prior art
- Existing fingerprint/antibot tests under `tests/` (e.g. `antibot_detection.rs`, `fpjs_compliance.rs`, `cookie_pinning.rs`, `doh_live.rs`) set the style for integration tests of the stealth surface — adaptive recall tests should follow that pattern (real fixtures, real assertions).
- Existing queue/checkpoint tests (`tests/queue_*.rs`, `tests/artifact_storage.rs`) set the pattern for storage round-trip tests — replay and adaptive store tests follow them.

## Out of Scope

- **Public benchmark suite vs Scrapling.** Explicit decision: no head-to-head numbers in v2.0.
- **Public test-coverage claim.** Internal target ≥80%, not a marketed number.
- **Embedding-based similarity** (cosine-sim over learned element embeddings). v2.0 ships the 1:1 Scrapling port; ML-based matching is a future possibility.
- **Distributed crawling** (multi-machine work-stealing). Single-machine persistent queue remains.
- **ML-driven pagination auto-detection** and **schema auto-detection** (Scrapling roadmap items they haven't shipped either).
- **Python SDK.** Polyglot strategy covers Rust + Node + CLI + Lua hooks; Python is intentionally not added.

## Further Notes

- **Crate selection:** `scraper` (html5ever) and `lol_html` are both established Rust dependencies; no new exotic crates. `rmcp` for MCP server, `rustyline` for shell, `reddb-io/reddb` for the per-spider store.
- **Stealth differentiators carry into v2 marketing copy:** chrome-149 byte-perfect TLS+H2, native reCAPTCHA v3 solver, DoH, persistent queue, polyglot SDK. These are the four claims where crawlex beats Scrapling rather than matches.
- **Open follow-ups not blocking publication of this PRD:**
  - Concrete release timeline (weeks vs months).
  - Docker image parity (Scrapling ships one with browsers bundled — does crawlex?).
  - Ownership of long-term adaptive-recall regressions (algorithm drift on real targets).
  - Default similarity threshold per element kind (Scrapling uses 0.2 globally; whether that holds for us under the 1:1 port).
  - Migration-guide structure for v1 → v2.
- **Reference clone:** Scrapling source is at `/tmp/scrapling-research` for direct comparison during implementation. Key files: `scrapling/parser.py` (adaptive core), `scrapling/core/storage.py` (fingerprint store), `scrapling/core/ai.py` (MCP tool surface), `scrapling/spiders/spider.py` (spider DSL shape).
