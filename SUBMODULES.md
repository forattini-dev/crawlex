# Git Submodules — crawlex

Reference-only checkouts used as source of truth when porting or cross-checking
behaviour. Not compiled into the crate.

| Path | Upstream | Commit | Used for |
|---|---|---|---|
| `references/curl-impersonate` | https://github.com/lwthiker/curl-impersonate | `822dbefe42e077fb9f3f16eaf0eca24944e5aadc` (v0.6.1-3) | Reference for Chrome JA4/JA3 TLS + H2 ClientHello shape — `src/impersonate/tls.rs` cross-checks cipher list and extension order against `chrome/`. |
| `references/rebrowser-patches` | https://github.com/rebrowser/rebrowser-patches | `6373894fde8379eb9b8d393e1d607706eecd8e70` (1.0.19) | Reference for puppeteer-core / playwright-core patches that defeat Chrome automation-detection side effects — inspiration for our stealth shim and render pool. |
| `references/firecrawl` | https://github.com/firecrawl/firecrawl | `b52cf19d51db99f10251792dae4649fe0487befa` | Source for the Rust-native HTML cleaner (`apps/api/native/src/html.rs`) and link filter (`apps/api/native/src/crawler.rs`) we port into `src/extract/` (Phase 1). MIT license — attribution in `NOTICE`. |
| `references/fingerprintjs` | https://github.com/fingerprintjs/fingerprintjs | `734a2f59dda3f0a290c6b03040e1852b5cb19af6` (v5.1.0-14) | Compliance test suite for our stealth shim. The 40 entropy sources in `src/sources/*.ts` are the spec our shim must satisfy coherently (Phase 4 `tests/fpjs_compliance.rs`). MIT license — attribution in `NOTICE`. |

## Updating

```bash
git submodule update --remote references/<name>
# Review diff, update the commit column above, commit both the submodule
# pointer and this file together.
```

## Not compiled

Nothing under `references/` is included in `cargo build`. Source for reference
only. If you port code, copy-with-attribution into `src/`, don't add a path
dependency.
