# Phase 1 Antibot/Stealth — Output

## Status

All non-negotiable gates green:
- `cargo build --all-features` — OK (1m02s)
- `cargo build --no-default-features --features cli,sqlite` — OK
- `cargo clippy --all-features --all-targets -- -D warnings` — OK
- `cargo test --all-features` — all 225+ non-ignored tests pass
- `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` — PASS (~32.7s)
- Chrome 149 patches in `src/render/chrome/handler/{frame,target}.rs` untouched
- Licenses in `src/render/LICENSES/` untouched
- No commits made
- No CAPTCHA solver — detect + escalate only

## New files
- `src/antibot/mod.rs` — pure detection module (no IO, feature-gate-free). Types: `ChallengeLevel`, `ChallengeVendor`, `RawChallenge`, `ChallengeSignal`, `SessionState`. Functions: `detect_from_html`, `detect_from_http_response`, `detect_from_cookies`. 10 vendor variants, conservative signatures ordered most-specific-first.
- `tests/antibot_fixtures/` — 8 vendor fixtures (cloudflare_jschallenge, cloudflare_turnstile, recaptcha, recaptcha_enterprise, hcaptcha, datadome, perimeterx, akamai) + `innocent.html` false-positive guard.
- `tests/antibot_detection.rs` — 18 tests covering HTML/HTTP/cookie paths, false-positive guards, cross-vendor isolation.

## Modified files
- `src/lib.rs` — `pub mod antibot`.
- `src/events/envelope.rs` — `EventKind::ChallengeDetected` (wire `challenge.detected`).
- `src/identity/bundle.rs` — `SessionIdentity.state: SessionState` (serde default Clean).
- `src/policy/engine.rs` — `SessionAction` enum + `PolicyEngine::decide_post_challenge`.
- `src/policy/mod.rs` — re-export `SessionAction`.
- `src/storage/mod.rs` — `Storage` trait: default `record_challenge` + `session_challenges` (no-op).
- `src/storage/sqlite.rs` — `challenge_events` table + indexes, `Op::RecordChallenge`, impls for record/query via writer thread + read-only connection.
- `src/render/mod.rs` — `RenderedPage.challenge: Option<ChallengeSignal>`.
- `src/render/pool.rs` — post-`settle_after_actions` → `detect_from_html` fills `rp.challenge`.
- `src/crawler.rs` — new `session_states: DashMap<String, SessionState>`, helper `handle_challenge` (updates state, feeds `ProxyOutcome::ChallengeHit`, persists, picks `SessionAction`, emits event), wired in both HTTP path (after response) and render path (after rp). `write_challenge_snapshot` for `challenge_<vendor>_<session>_<ts>.html|.png` artifacts under the screenshot dir. Also gated pre-existing `write_screenshot_output` under `cdp-backend` (latent dead-code reference to `Sha256` that mini build could not resolve).
- `tests/policy_engine.rs` — added `decide_post_challenge_matrix` test (7 rule combinations).

## Design decisions (auto-approved defaults)

- **RotateProxy async**: not abortive — crawler lets current job finish; the router's `ChallengeHit` outcome is fed so subsequent picks favour other proxies. Matches existing escalation pattern (no inline swap).
- **record_challenge severity**: always persist regardless of `level`. `SqliteStorage::record_challenge` writes every hit; `session_challenges` query returns full history per session.
- **Kill-context path**: reuses the existing session scope — when `SessionAction::KillContext`/`ReopenBrowser` is produced, the crawler emits the event and flags the session `Contaminated`/`Blocked`; pool-side BrowserContext lifecycle management stays the same (session_id change at scope boundaries already drops the context).

## Observations / ponta frouxa

- `render_outcome` (4.3 leftover): `ProxyOutcome::ChallengeHit` is now wired end-to-end on both HTTP and render paths. Full render-path success/latency outcome threading (the non-challenge outcomes already flowed on HTTP) would need timing threaded from `RenderPool::render` to the crawler — left for phase 2 (runtime ScriptSpec) since render latency is dominated by `settle_after_actions` policies and not yet a router-scoring signal.
- Signatures are deliberately conservative; `innocent.html` fixture contains the literal phrases "Access denied" and "Just a moment" and still produces `None` from `detect_from_html` because the Cloudflare detector requires **both** title + platform-script co-occurrence and the AccessDenied fallback requires a small body (<4KiB).
- `SessionState::after_challenge` is monotonic — `Blocked` is sticky.
- HTTP path uses session_id `"http"` sentinel (impersonate client shares cookie jar per-host at the client level; the session contamination notion only makes sense for browser contexts). Per-host contamination tracking can be added in phase 2.

## Event payload shape

```json
{
  "v": 1,
  "event": "challenge.detected",
  "run_id": 123,
  "session_id": "ctx-abc",
  "url": "https://target.example/",
  "why": "antibot:cloudflare_js_challenge:challenge_page",
  "data": {
    "vendor": "cloudflare_js_challenge",
    "level": "challenge_page",
    "session_action": "kill_context",
    "session_state": "contaminated",
    "proxy": "http://p1:8080",
    "metadata": { "source": "html", "signals": ["..."] }
  }
}
```
