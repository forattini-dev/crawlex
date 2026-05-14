# Slice 16: Request.session_id + SessionManager routing [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Introduce `Request.session_id: Option<String>` and a `SessionManager` that owns a registry mapping session_id → engine backend (HTTP, render, stealth). Sessions of the same backend type maintain isolated cookie jars. Unknown session_id falls back to a default backend with a warning.

## Acceptance criteria

- [ ] `Request` struct gains `session_id: Option<String>`
- [ ] `SessionManager::register(id, backend_kind)` creates an isolated session
- [ ] Cookies set in session A are NOT visible in session B even if same backend kind
- [ ] Unknown session_id logs a warning and routes to the default backend
- [ ] Node SDK exposes `new Request(url, { sessionId })`
- [ ] Tests: cookie isolation between two `stealth` sessions, fallback on unknown id

## Blocked by

None - can start immediately

## Progress note (2026-05-14)

Implementation written but NOT committed — bash perms blocked all
`cargo` / `git` commands in this loop, matching the slice 15 pattern.

What landed in the working tree (unstaged):

- `src/scraping/mod.rs` — new module declaring `request` + `session`
  submodules, re-exports `Request`, `SessionManager`, `BackendKind`,
  `CookieJar`, `SessionEntry`.
- `src/scraping/request.rs` — `Request { url, method, session_id }`
  with `new`, `with_session`, `with_method` builders. 3 unit tests.
- `src/scraping/session.rs` — `BackendKind { Http, Render, Stealth }`,
  `CookieJar` (Arc<Mutex<HashMap>>), `SessionEntry`, `SessionManager`,
  `RouteDecision`. `register` is idempotent. `route()` returns the
  default backend with `fallback=true` and a `warn!` on unknown id.
  Unknown ids are NOT silently registered. 7 unit tests covering
  isolation, idempotency, jar Arc-sharing, all 3 routing cases.
- `src/lib.rs` — declares `pub mod scraping;`.
- `tests/scraping_session.rs` — 3 integration tests against public
  re-exports: stealth-vs-stealth cookie isolation, unknown-id fallback,
  default `session_id = None`.
- `sdk/index.d.ts` — `BackendKind`, `RequestOptions`, `class Request`
  declarations.
- `sdk/crawlex-sdk.js` — runtime `Request` class with input validation,
  added to `module.exports`. Pure JS placeholder, no native binding
  yet (matches slice 11/15 pattern — runtime dispatch lands once the
  engine binding ships).
- `sdk/test/request.test.js` — 5 node:test cases covering defaults,
  sessionId opt, method override, empty-url + empty-sessionId guards.

Acceptance criteria status (pre-verification):
- [x] `Request` gains `session_id: Option<String>`
- [x] `SessionManager::register(id, backend_kind)` creates isolated session
- [x] Cookies set in session A NOT visible in session B (same backend)
- [x] Unknown session_id logs warning + routes to default
- [x] Node SDK exposes `new Request(url, { sessionId })`
- [x] Cookie-isolation + unknown-id tests present

Next iteration must:

1. Run feedback loops once shell perms allow:
   - `cargo check --all-targets --all-features`
   - `cargo test --all-features scraping`
   - `node --test sdk/test/request.test.js`
2. Likely-fail points: `parking_lot::Mutex` already used elsewhere in
   the crate (e.g. `http::cookies`) so the dep should be available;
   `tracing::warn!` likewise — both confirmed by grep before write.
3. Stage all 8 files, commit with the message draft below, and
   `git mv issues/16-slice.md issues/done/16-slice.md`.

Commit message draft:

```
feat(scraping): Request.session_id + SessionManager routing (slice 16)

src/scraping/{mod,request,session}.rs: minimal Request descriptor and
SessionManager. BackendKind { Http, Render, Stealth }. Per-session
CookieJar guarantees no leakage between sessions of the same backend.
Unknown session_id falls back to the default backend with a warn! log;
not silently registered. SDK exposes `new Request(url, { sessionId })`
as a pure-JS placeholder. 10 Rust tests + 3 integration + 5 SDK tests.
```

