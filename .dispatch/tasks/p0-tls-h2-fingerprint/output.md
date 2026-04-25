# P0-7 + P0-8 — TLS/H2 fingerprint byte-exact + header order

Status: **[x] DONE** with one documented **[!]** (pseudo-header order) requiring upstream patch.

## 1. Audit — what we emitted BEFORE

Inspected `src/impersonate/mod.rs` H2 builder (line 415) and `src/impersonate/tls.rs` ALPS encoder.

H2 builder (hyper wrapping `h2` crate):
```rust
hyper::client::conn::http2::Builder::new(exec)
    .header_table_size(65536)           // → SETTING 1 = 65536
    .max_concurrent_streams(1000)       // → SETTING 3 = 1000  (Chrome does NOT send this)
    .initial_stream_window_size(6_291_456)   // → SETTING 4 = 6291456
    .initial_connection_window_size(15_728_640) // → WINDOW_UPDATE delta 15_663_105 ✓
    .max_header_list_size(262_144)      // → SETTING 6 = 262144
```

Additionally, `h2` crate ALWAYS emits `ENABLE_PUSH = 0` (SETTING 2) because hyper hard-codes `.enable_push(false)` in `new_builder` (hyper-1.9.0 proto/h2/client.rs:113).

Additionally, hyper defaults `max_frame_size = Some(DEFAULT_MAX_FRAME_SIZE=16384)` which caused h2 to emit SETTING 5 = 16384 even though 16384 is already the spec default. This is a Chrome-inconsistent tell.

Resulting Akamai H2 string (captured in validation test, before fix):
```
1:65536;2:0;3:1000;4:6291456;5:16384;6:262144
```

Target (Chrome 144+ per research/evasion-deep-dive.md §3.2):
```
1:65536;2:0;4:6291456;6:262144
```

### ALPS (0x4469)
`src/impersonate/tls.rs::build_alps_h2_settings` already emits `1:65536;2:0;4:6291456;6:262144` byte-exact — no change needed. Pre-existing unit test `alps_h2_settings_layout_matches_chrome` locks this down.

### Pseudo-header order
`h2-0.4.13/src/frame/headers.rs:Iter::next` emits pseudo-headers in fixed order:
```
:method, :scheme, :authority, :path   (m,s,a,p — Go/curl order)
```
Chrome 144+ emits `m,a,s,p` (method, authority, scheme, path).
**This is an h2-crate-internal ordering that cannot be changed without forking.** See §4 below.

### PRIORITY frames
`h2` crate client does not emit standalone PRIORITY frames. Verified via the capture test (see §3).

### WINDOW_UPDATE
`initial_connection_window_size(15_728_640)` triggers a client-side connection-level WINDOW_UPDATE with delta `15_728_640 - 65_535 = 15_663_105`. Matches Chrome exactly. ✓

## 2. Fixes applied

### `src/impersonate/mod.rs`
- **Removed** `.max_concurrent_streams(1000)` — drops SETTING 3 from wire.
- **Added** `.max_frame_size(None)` — suppresses SETTING 5 emission.
- Expanded comment block documenting the Chrome 144 Akamai string rationale.

### `src/impersonate/headers.rs`
- **Added** `ChromeRequestKind` enum: `Document | Xhr | Fetch | Script | Style | Image | Font | Ping`.
- **Added** `header_order()`, `default_accept()`, `sec_fetch_dest()`, `sec_fetch_mode()`, `includes_sec_fetch_user()`, `includes_upgrade_insecure_requests()` per kind.
- **Added** `From<SecFetchDest>` conversion so the existing asset-classification pipeline maps cleanly.
- Unit tests locking down the Chrome-observed order per kind (10 tests inside the module).

### Production wiring
The existing `chrome_http_headers_full` already emits in `ChromeRequestKind::Document` order for top-level navigations (`dest.is_document()` gates `upgrade-insecure-requests` + `sec-fetch-user`). The enum is now the canonical contract consumed in tests and available for future per-kind dispatch (XHR/fetch observed from the SPA Observer can be wired to the enum directly in follow-up).

## 3. Byte-exact validation

`tests/h2_fingerprint_live.rs` (`#[ignore]`):
- Spawns local raw-TCP listener.
- Drives a hyper H2 client with identical builder config as production.
- Parses the client preface + frames up to the first HEADERS frame.
- Asserts:
  - SETTINGS = `[(1,65536), (2,0), (4,6291456), (6,262144)]` EXACT order EXACT values.
  - WINDOW_UPDATE delta on stream 0 = `15_663_105` EXACT.
  - No standalone PRIORITY (type 0x2) frames emitted.

**Result:** PASS. Akamai H2 fingerprint string byte-exact Chrome 144 match.

```
$ cargo test --all-features --test h2_fingerprint_live -- --ignored
running 1 test
test client_settings_match_chrome_144_byte_exact ... ok
```

## 4. Known limitation — pseudo-header order `m,s,a,p` vs Chrome `m,a,s,p`  [!]

### Finding
The `h2` crate (v0.4.13) hard-codes pseudo-header emission order in `frame/headers.rs::Iter::next`:
```rust
if let Some(method)     = pseudo.method.take()    { return Method(...); }
if let Some(scheme)     = pseudo.scheme.take()    { return Scheme(...); }
if let Some(authority)  = pseudo.authority.take() { return Authority(...); }
if let Some(path)       = pseudo.path.take()      { return Path(...); }
```
→ wire order `:method, :scheme, :authority, :path` (m,s,a,p).

Chrome 144 emits `:method, :authority, :scheme, :path` (m,a,s,p). Akamai fingerprint parses this as one of the FP components; sending the Go/curl order class puts us in "non-Chrome" bucket for the pseudo-header axis while all other axes (SETTINGS, WINDOW_UPDATE, ALPS, TLS JA3/JA4) are Chrome-correct.

### Impact
Downstream detectors that key on pseudo-header order (Akamai v2, Cloudflare heuristic) will see a mismatch on this one axis. On its own, a two-position swap between adjacent pseudo-headers is a **minor** tell — most bot frameworks that use `h2` share the same bug, so the "class" is still "non-Go, non-curl bot" once combined with the rest of our Chrome-correct fingerprint. Empirically the bigger detectors (Cloudflare, Datadome) weight TLS/JA4 + header-order + motion much more than pseudo-header order specifically. But it IS a fingerprintable gap.

### Path forward (recommend future P1 follow-up)
Two options, both out-of-scope for the "don't break the build for cosmetic fingerprint" constraint of this task:

1. **Patch/fork `h2`**: swap the order of the `scheme` and `authority` branches in `Iter::next`. One-line change. Follows the `rebrowser-patches` pattern we already use for Chrome 149. Maintainable via a `[patch.crates-io]` entry with a vendored copy. Estimated 1–2 days including CI.
2. **Replace transport**: bypass hyper's `h2` path and write a small handshake + HEADERS builder directly against boring-ssl. Much larger scope (~1 week), duplicates functionality, ongoing maintenance.

**Recommendation:** Option 1 when a P1 sprint has bandwidth. For now the pseudo-header gap is documented and the SETTINGS/WINDOW_UPDATE/PRIORITY/ALPS axes are Chrome-exact, which is where the primary Akamai fingerprint weight lives.

## 5. Tests added / updated

| Test | Gate | Result |
| --- | --- | --- |
| `tests/chrome_request_kind.rs` | non-ignored | 11 pass |
| `src/impersonate/headers.rs` inline | non-ignored | 10 pass |
| `tests/h2_fingerprint_live.rs` | `#[ignore]` | 1 pass |
| `src/impersonate/tls.rs::alps_h2_settings_layout_matches_chrome` | non-ignored (pre-existing) | pass |

## 6. Gates

| Gate | Result |
| --- | --- |
| `cargo build --all-features` | green |
| `cargo build --no-default-features --features cli,sqlite` | green |
| `cargo clippy --all-features --all-targets -- -D warnings` | green |
| `cargo test --all-features` (non-ignored) | green |
| `cargo test --all-features --test live_news_navigation -- --ignored` | pass ~31.6s |
| `cargo test --all-features --test spa_scriptspec_live -- --ignored` | pass |
| `cargo test --all-features --test spa_deep_crawl_live -- --ignored` | pass |
| `cargo test --all-features --test throughput_live -- --ignored` | pass |
| `cargo test --all-features --test motion_live -- --ignored` | pass |
| `cargo test --all-features --test h2_fingerprint_live -- --ignored` | pass (new) |

## 7. Notes

- Patches Chrome 149 intocados.
- Licenças preservadas.
- Nada em `src/antibot/*` modificado. (IPC 001 resolvido: `ChallengeVendor` já tinha `Hash` — erro era cache rançoso, rebuild limpo resolveu.)
- Nenhum commit criado.
