# chromiumoxide git-master bump — partial / blocked

## Status
- [x] Cargo.toml bumped to `mattsse/chromiumoxide@afcc3a4313f2087249b4490d94e54bf8e3bfaccf`.
- [x] `cargo build --all-features` — clean, zero API breakage.
- [x] `cargo build --no-default-features --features cli,sqlite` — clean.
- [x] `cargo clippy --all-features --all-targets -- -D warnings` — clean.
- [x] `cargo test --all-features` (non-ignored) — all green.
- [!] Live test `live_news_navigation` — STILL FAILS identical to 0.9.1. Master has not fixed the CDP drift.

## Cargo.toml diff
```
-chromiumoxide = { version = "0.9", default-features = false, features = ["bytes"], optional = true }
-chromiumoxide_fetcher = { version = "0.9", default-features = false, features = ["rustls", "zip8"], optional = true }
+chromiumoxide = { git = "https://github.com/mattsse/chromiumoxide", rev = "afcc3a4313f2087249b4490d94e54bf8e3bfaccf", default-features = false, features = ["bytes"], optional = true }
+chromiumoxide_fetcher = { git = "https://github.com/mattsse/chromiumoxide", rev = "afcc3a4313f2087249b4490d94e54bf8e3bfaccf", default-features = false, features = ["rustls", "zip8"], optional = true }
```

## API changes found
None. Master (afcc3a4) is API-compatible with the 0.9.1 crates.io release for every call site in:
- `src/render/pool.rs`
- `src/render/ref_resolver.rs`
- `src/render/ax_snapshot.rs`
- `src/render/interact.rs`
- `src/render/selector.rs`

Recent master commits are Element Clone derive, ScreenshotParams Clone, zip8 support, async-std removal, dep bumps — nothing touches the PDL/CDP protocol JSON.

## Root cause of live test failure (still the same bug)

Chrome 149.0.7779.3 dev emits `Network.requestWillBeSentExtraInfo` events with fields the 0.9.1 / master PDL bindings don't know:

- `clientSecurityState.localNetworkAccessRequestPolicy: "PermissionBlock"` — NEW
- `siteHasCookieInOtherPartition: bool` — NEW

Serde's untagged `Message` enum rejects the whole event, log line:

```
WS Invalid message: data did not match any variant of untagged enum Message
```

Sample raw dropped WS frame (captured via `RUST_LOG=chromiumoxide=trace` inside the live test):

```
{"method":"Network.requestWillBeSentExtraInfo","params":{"requestId":"2075836.2","associatedCookies":[],"headers":{...},"connectTiming":{"requestTime":617712.619625},"clientSecurityState":{"initiatorIsSecureContext":true,"initiatorIPAddressSpace":"Public","localNetworkAccessRequestPolicy":"PermissionBlock"},"siteHasCookieInOtherPartition":false},"sessionId":"1334F2B0517D02BC258A662AC36D52F3"}
```

Verification:
```
grep -rn "localNetworkAccessRequestPolicy" ~/.cargo/git/checkouts/chromiumoxide-*/afcc3a4/
# → 0 hits
```

When enough of these drop, the Page.navigate command never completes and the 30s request_timeout fires with `navigate: Request timed out`.

## Decision
Upstream master does NOT have the fix yet. Per plan restriction ("Se o master também tiver o mesmo bug, marcar [!] e parar"), stopped here. Did not attempt:
- Local fork with `#[serde(other)]` fallback on Message enum
- Regenerating PDL bindings from a newer browser_protocol.json
- Swapping to `chromiumoxide_fork` (explicitly forbidden by plan)

Both are plan-B candidates for the next dispatch.

## Lockfile note
`Cargo.lock` now pins the git rev. Future upgrades: `cargo update -p chromiumoxide`. If reverting: restore the two `version = "0.9"` lines and run `cargo update -p chromiumoxide` — will snap back to crates.io 0.9.1.

## Recommendation for next dispatch (plan B)
Two viable paths:

1. **Local fork (smallest blast radius)**: fork `chromiumoxide` to a local vendored crate under `vendor/chromiumoxide` (path dep), add `#[serde(other)]` or a fallthrough `Unknown(serde_json::Value)` arm to the `Message` enum in `chromiumoxide_types/src/lib.rs` so unknown events are tolerated. Rebase onto master periodically.

2. **Regenerate PDL**: pull latest `browser_protocol.json` + `js_protocol.json` from https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/public/devtools_protocol/ (or `https://chromedevtools.github.io/devtools-protocol/`), replace the ones inside `chromiumoxide_pdl` in a fork, regen bindings. Heavier but fixes root cause and may surface more API breakage.

Path 1 is recommended first pass — smallest diff, hard to break.
