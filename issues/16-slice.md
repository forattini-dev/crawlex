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
