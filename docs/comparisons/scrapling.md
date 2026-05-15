# crawlex vs Scrapling

Side-by-side coverage matrix versus [D4Vinci/Scrapling](https://github.com/D4Vinci/Scrapling). Status legend:

- **✓ parity** — crawlex implements the same surface.
- **✓+ superior** — crawlex implements it and goes further (perf, byte-fidelity, additional axis).
- **— intentionally absent** — design decision against shipping it; rationale in note.

Each row cites the crawlex slice or existing module that backs the claim. No public benchmark numbers are claimed here (see PRD out-of-scope).

## Fetchers and transport

| Scrapling claim | crawlex status | Evidence |
| --- | --- | --- |
| HTTP TLS impersonation (`curl_cffi`) | ✓+ superior | `src/impersonate/` + `src/identity/` — chrome-149 byte-exact TLS + H2 SETTINGS. Goes beyond `curl_cffi` partial impersonation. |
| Dynamic Chromium fetcher | ✓ parity | `src/render/pool.rs` — managed Chrome + CDP. |
| Stealthy browser fetcher (CF Turnstile bypass) | ✓ parity | `src/antibot/` — bypass + challenge handling. Native reCAPTCHA v3 solver under `src/antibot/recaptcha/`. |
| Session management (cookies, state) | ✓ parity | `src/antibot/cookie_pin.rs` + `src/scraping/session.rs` (slice 16). Per-`session_id` isolated cookie jars. |
| Proxy rotation | ✓ parity | `src/proxy/` — round-robin, sequential, random, sticky-per-host strategies. |
| Domain & ad blocking (~3,500 domains) | ✓ parity | `src/adblock/` (slice 20) — bundled baseline list + `crawlex update-blocklist` EasyList updater. |
| DNS leak prevention (DoH) | ✓ parity | `src/identity/` DoH integration; tested in `tests/doh_live.rs`. |
| Async support | ✓ parity | tokio-native end-to-end. |

## Spider framework

| Scrapling claim | crawlex status | Evidence |
| --- | --- | --- |
| Scrapy-like Spider API (`start_urls`, `parse`) | ✓ parity | `src/scraping/spider.rs` (slice 17). Node SDK: `defineSpider({ startUrls, parse })`. |
| Concurrent crawling + per-domain throttle | ✓ parity | `src/scheduler.rs` — origin-aware delay distribution. |
| Multi-session routing within a spider | ✓ parity | `Request.session_id` + `SessionManager` (slice 16). |
| Pause / Resume (checkpoint) | ✓ parity | `src/queue/sqlite.rs` persistent queue + checkpoint resume. |
| Streaming items (`async for item in spider.stream()`) | ✓ parity | `EventKind::ItemScraped` + `spider.stream()` (slice 18). |
| Blocked-request detection + retry | ✓ parity | `src/antibot/block_detector.rs` + scheduler retry policy. |
| Robots.txt compliance | ✓+ superior | `src/robots.rs` + Content-Signal directive (slice 6 / gh#7). Scrapling does not honor Content-Signal. |
| Development mode (replay cache) | ✓ parity | `src/scraping/replay.rs` (slice 19) — dir or sqlite backend. |
| Built-in JSON/JSONL export | ✓ parity | `src/storage/` + Node SDK helpers. |

## Parser and selectors

| Scrapling claim | crawlex status | Evidence |
| --- | --- | --- |
| HTML parser (tree) | ✓ parity | `src/parser/mod.rs` (slice 8) — `scraper`/html5ever, plus `lol_html` streaming branch. |
| CSS selectors with Scrapy/Parsel pseudo-element semantics | ✓ parity | `src/parser/selectors.rs` (slice 9). |
| XPath selectors | ✓ parity | `src/parser/selectors.rs::xpath` (slice 9). |
| `findByText` / `findByRegex` / `filter` | ✓ parity | `src/parser/selectors.rs` helpers (slice 10). |
| Navigation API (parent/siblings/children) | ✓ parity | `ElementHandle::parent/siblings/children` (slice 9). |
| Auto-selector generation | ✓ parity | `ElementHandle::generate_selector` (slice 11). |
| Adaptive element relocation on DOM change | ✓ parity | `src/parser/adaptive.rs` (slice 14). 1:1 port of Scrapling's weighted similarity (slice 12) + per-spider fingerprint store (slice 13). |
| `findSimilar` (peer-row detection) | ✓ parity | `ElementHandle::find_similar` (slice 15). |

## AI integration

| Scrapling claim | crawlex status | Evidence |
| --- | --- | --- |
| MCP server for AI-assisted scraping | ✓ parity | `crawlex mcp` subcommand (slice 24) on `rmcp`. Tools: session mgmt, get/bulk_get, fetch, stealth_fetch, css_query/xpath_query returning text/markdown to minimize tokens. |

## Devex

| Scrapling claim | crawlex status | Evidence |
| --- | --- | --- |
| Interactive shell (IPython) | ✓+ superior | `crawlex shell` Rust REPL (slice 22) **plus** `npx crawlex shell` Node REPL (slice 23). Scrapling ships only IPython. |
| Use directly from terminal | ✓ parity | `crawlex` CLI covers run/spider/mcp/shell/from-curl/update-blocklist. |
| Rich navigation API | ✓ parity | See parser section. |
| Enhanced text processing (regex, cleaning) | ✓ parity | `ElementHandle` + helpers (slice 10). |
| Auto selector generation | ✓ parity | Slice 11. |
| `curl` → request converter | ✓ parity | `crawlex from-curl` (slice 21). |
| Full type coverage | ✓ parity | Rust = type-checked by definition; SDK ships `index.d.ts`. |
| Docker image with browsers bundled | — intentionally absent | We ship a single static binary + auto-fetch Chromium on first use. Container build is a downstream packaging concern, not a core deliverable. |

## Differentiators (no Scrapling counterpart)

| Capability | crawlex evidence |
| --- | --- |
| Byte-perfect chrome-149 TLS + H2 fingerprint | `src/impersonate/` — beyond `curl_cffi`. |
| Native reCAPTCHA v3 solver | `src/antibot/recaptcha/` — Scrapling has no equivalent. |
| Polyglot SDK (Rust core, Node SDK, CLI, Lua/JS hooks) | `sdk/`, `src/hooks/`, embedded scripting. |
| Persistent SQLite queue with pause/resume + checkpoint | `src/queue/sqlite.rs`. |
| Job TTL: max-runtime watchdog + result-retention reaper | `src/storage/sqlite.rs` (slice 7 / gh#8). |
| Cursor pagination with opaque base64 versioned tokens | `src/storage/cursor.rs` (slice 8 / gh#9). |
| Canonical per-URL / per-job status taxonomy | `src/status.rs` (slice 1 / gh#2). |
| Render-mode operator switch (`auto` / `always` / `never`) | `Config.render_mode` (slice 2 / gh#3). |
| Conditional re-crawl knobs (`cache_max_age_secs`, `modified_since`) | `src/cache_validator.rs` (slice 4 / gh#5). |
| Resource-type blocking at CDP (`reject_resource_types`) | `src/render/pool.rs` (slice 5 / gh#6). |
| Content-Signal robots.txt directive + declared `crawl_purposes` | `src/robots.rs` (slice 6 / gh#7). |
| Neutral browser provider abstraction (stock / external CDP / auto) | `Config.browser_provider` + `--external-cdp-url` (slice 29). |
| External CDP provider integration (CloakBrowser et al.) with capability detection, per-session fingerprint calibration, mismatch policy, isolated/persistent session modes, and explicit fallback chain | Slices 30–36. |

## Intentionally absent

| Item | Reason |
| --- | --- |
| Python SDK | Polyglot strategy covers Rust + Node + CLI + Lua hooks; a Python wrapper is out of scope. |
| Embedding-based similarity (cosine over learned element embeddings) | v2.0 ships the 1:1 Scrapling-style weighted port. ML-based matching is a future possibility. |
| Distributed crawling (multi-machine work-stealing) | Persistent SQLite queue keeps a single-machine guarantee; distributed coordination is a separate PRD. |
| Public head-to-head benchmark numbers | Explicit PRD decision — no benchmark suite shipped, no public throughput / memory comparison claims. |
| Source filtering (sitemaps-only / links-only) | Maximum-discovery is the project's identity (`CLAUDE.md`). |
| ML-driven pagination / schema auto-detection | Roadmap follow-up. |

## Provenance

Every "parity" / "superior" row above is backed by a merged slice in `issues/done/` or a long-existing module on `main`. See `git log --grep "(slice"` for the implementation commits.

Compiled 2026-05-15. Update this file when a new slice lands that changes a row.
