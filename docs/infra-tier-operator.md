# Infrastructure-tier operator guide

Crawlex's default build ships "datacenter-tier" capabilities: an HTTP spoof
engine with a curated proxy list, a single bundled Chromium build, and
prevention-first antibot telemetry.  Several use-cases need infrastructure
the crate deliberately does **not** bundle — rented residential IPs,
commercial captcha-solve services, Android emulators, or a human in the
loop. This doc is the operator's reference for plugging that infra in.

All features documented here are **disabled by default**. Nothing in this
file opts you in; you have to set the env var / CLI flag on every run.

The matching scaffold code lives at:

| Concern               | Module                        |
| --------------------- | ----------------------------- |
| Residential proxies   | `src/proxy/residential.rs`    |
| Account warming       | `src/identity/warmup.rs`      |
| Android fingerprint   | `src/render/android_profile.rs` |
| Human handoff         | `src/render/handoff.rs`       |
| Captcha solver        | `src/antibot/solver.rs`       |

> **Scaffold status.** Every adapter below currently returns
> `…NotConfigured`. The operator is expected to implement the TODOs in
> the relevant module OR wait for a follow-up wave that lands real impls
> for the provider(s) they use. The public types (`ResidentialProvider`,
> `CaptchaSolver`, `AndroidProfile`, `SessionWarmup`, `HandoffRequest`)
> are already stable and safe to depend on.

---

## 1. Residential proxy pools (issue #34)

### Why
Datacenter IPs get classified by most CDNs within hours. Residential IPs
(rented from end-user devices) evade that filter at the cost of ~$5–15/GB
from providers like BrightData, Oxylabs, IPRoyal.

### Wire-up
1. Obtain an account with one provider. Scaffold supports:
   * `brightdata` — gateway `brd.superproxy.io:22225`
   * `oxylabs` — gateway `pr.oxylabs.io:7777`
   * `iproyal` — gateway `geo.iproyal.com:12321`
2. Export credentials:
   ```bash
   export CRAWLEX_RES_PROVIDER=brightdata
   export CRAWLEX_RES_PROXY_BRIGHTDATA_USER=brd-customer-xxx-zone-resi
   export CRAWLEX_RES_PROXY_BRIGHTDATA_PASS=yyy
   export CRAWLEX_RES_PROXY_BRIGHTDATA_ZONE=resi
   ```
3. Implement the `rotate` method on the adapter stub
   (`src/proxy/residential.rs::BrightDataStub`). Template:
   ```rust
   fn rotate(&self, host: &str) -> Result<Url, ResidentialError> {
       let user = std::env::var(env::CRAWLEX_RES_PROXY_BRIGHTDATA_USER)
           .map_err(|_| ResidentialError::ProviderNotConfigured("brightdata"))?;
       let pass = std::env::var(env::CRAWLEX_RES_PROXY_BRIGHTDATA_PASS)
           .map_err(|_| ResidentialError::ProviderNotConfigured("brightdata"))?;
       let session = format!("session-{}", uuid::Uuid::new_v4());
       Url::parse(&format!(
           "http://{user}-session-{session}:{pass}@brd.superproxy.io:22225"
       )).map_err(|e| ResidentialError::Upstream(e.to_string().into()))
   }
   ```
4. Feed the provider to `ProxyRouter` as a new proxy source. The router
   treats residential URLs identically to the static list — scoring,
   quarantine, sticky-per-host all just work.

### CLI flag (future)
The flag `--residential-provider <brightdata|oxylabs|iproyal|none>` will
land in a follow-up wave; this scaffold does not touch
`src/cli/args.rs` to avoid conflicts with parallel waves. Until then,
drive the provider via the env var above.

### Safety net
* Cap monthly spend via provider dashboard, not via crawlex.
* `ProxyRouter::record_outcome(ChallengeHit)` is already plumbed —
  residential adapters should override `report_outcome` to retire the
  session on hard-block so the next rotation mints a fresh exit IP.

---

## 2. Account warming (issue #35)

### Why
Login-gated targets (Instagram, LinkedIn, many banks) flag a session as
bot-like the moment it authenticates without prior browsing. Warming = do
a bit of benign crawling before firing the login action so the session
looks lived-in.

### Wire-up
1. Attach a `SessionWarmup` to each `SessionIdentity`:
   ```rust
   use crawlex::identity::warmup::{SessionWarmup, WarmupPolicy};

   let policy = WarmupPolicy {
       min_visits: 5,
       min_depth: 2,
       min_elapsed_secs: 600,
   };
   let mut warmup = SessionWarmup::new(policy);
   ```
2. On every successful non-login fetch for that session, call
   `warmup.record_visit(depth)`.
3. Before any login action (ScriptSpec's `type` into a password field,
   Lua hook emitting `HookDecision::Login`, etc.) call:
   ```rust
   warmup.gate_login()?;
   ```
   The gate returns `Err("warmup:cold")` or `Err("warmup:insufficient")`
   if the budget is not yet met. The scheduler should requeue the job
   with a delay equal to `min_elapsed - elapsed`.
4. Expose `warmup.phase()` on the session snapshot so the CLI dashboard
   shows progress.

### Defaults
```
min_visits       = 5
min_depth        = 2
min_elapsed_secs = 600  (10 minutes)
```

### Escape hatch
If you imported a cookie jar from a real human session, call
`warmup.force_warm()` once at session creation.

---

## 3. Android emulator profile (issue #36)

### Why
Some sites ship different backends to mobile UAs (M-pages, lite APIs) or
relax antibot checks for touch devices. Shipping a Pixel UA is not
enough — the viewport, DPR, touch flag, and UA-CH all have to agree.

### Wire-up
1. Resolve a preset:
   ```rust
   use crawlex::render::android_profile::{AndroidDevice, AndroidProfile};
   let profile = AndroidProfile::preset(AndroidDevice::Pixel7Pro);
   ```
2. Once the `render/chrome` wave wires the hook, every CDP target will
   receive the three `Emulation.*` commands automatically. Until then,
   operators who want the behaviour now can hand-apply the payload from
   a Lua hook:
   ```rust
   for (method, params) in profile.cdp_commands() {
       chrome.send_command(method, params).await?;
   }
   ```
3. Future CLI flag: `--mobile-profile android` (Pixel 7 Pro default) or
   `--mobile-profile pixel-8 | galaxy-s23`.

### NOT in scope
This module does **not** drive a real Android VM. Play Integrity,
hardware attestation, and SafetyNet require an actual AOSP emulator.
Wire such an emulator in as a separate CDP endpoint and point crawlex
at it via the standard `--chrome-path` / remote-debugging-URL path.

---

## 4. Human handoff (issue #37)

### Why
Some challenges aren't automatable without dragging in a solver service
(see §5). Banks and KYC flows require a human touch. Handoff lets the
crawl pause, print a prompt, and wait for an operator to resolve the
challenge manually — then resume.

### Wire-up
1. Enable handoff for a run:
   ```bash
   export CRAWLEX_HANDOFF=1
   ```
2. On a hard-block outcome, the scheduler (future wave) will call:
   ```rust
   use crawlex::render::handoff::{HandoffRequest, should_handoff};
   if should_handoff(&signal) {
       let req = HandoffRequest::from_signal(&signal, screenshot_path);
       req.pause_and_wait()?;  // blocks on stdin
   }
   ```
3. Operator sees a TUI message like:
   ```
   ────────────────────────────────────────
    crawlex :: human-handoff requested
   ────────────────────────────────────────
     reason   : hard_block
     vendor   : cloudflare_js_challenge
     url      : https://target.example/login
     snapshot : /tmp/crawlex/shot-abc.png
   ────────────────────────────────────────
   ```
4. Operator solves in their own browser, imports cookies back via
   `crawlex sessions import --storage-path crawlex.db --cookies path.json`
   (command lands alongside the cookie-pin wave), presses Enter.

### Limitations
Handoff works for interactive runs. For unattended deployments, pair it
with a captcha solver (§5) or drop-on-block behaviour.

---

## 5. Captcha solver plug-in (issue #38)

### Why
When prevention fails, an operator may choose to pay for solves rather
than abandon the target. Crawlex stays prevention-first by default;
solver integration is strictly opt-in.

### Supported adapters (scaffold)
| Adapter        | Vendors                                                     | Key env var                          |
| -------------- | ----------------------------------------------------------- | ------------------------------------ |
| `2captcha`     | reCAPTCHA v2/v3/Enterprise, hCaptcha, Turnstile             | `CRAWLEX_SOLVER_2CAPTCHA_KEY`        |
| `anticaptcha`  | reCAPTCHA v2/v3/Enterprise, hCaptcha                        | `CRAWLEX_SOLVER_ANTICAPTCHA_KEY`     |
| `vlm`          | reCAPTCHA image, hCaptcha image, generic captcha            | `CRAWLEX_SOLVER_VLM_API_KEY` + `CRAWLEX_SOLVER_VLM_PROVIDER` |

### Wire-up
1. Export the adapter selector + its API key:
   ```bash
   export CRAWLEX_SOLVER=2captcha
   export CRAWLEX_SOLVER_2CAPTCHA_KEY=your_api_key
   ```
2. The scheduler (future wave) reads `CRAWLEX_SOLVER`, calls
   `build_solver(kind)`, and dispatches every `ChallengeSignal` whose
   vendor is in `solver.supported_vendors()` to `solver.solve(payload)`.
3. Returned `SolveResult.token` is injected into the DOM via CDP:
   * reCAPTCHA / hCaptcha / Turnstile — set the corresponding `<textarea>`.
   * VLM path — the adapter drives clicks via CDP directly, returns a
     sentinel token `"driven"`.

### CLI flag (future)
`--captcha-solver <2captcha|anticaptcha|vlm|none>`. Default `none`.

### Budget hygiene
2captcha / anti-captcha bill per successful solve ($0.50–3/1000). Always
cap the provider's balance. The scaffold includes no budget enforcement.

---

## 6. Example combined deployment

```bash
# Residential exit IPs
export CRAWLEX_RES_PROVIDER=oxylabs
export CRAWLEX_RES_PROXY_OXYLABS_USER=customer-xxx
export CRAWLEX_RES_PROXY_OXYLABS_PASS=yyy

# VLM solver with Anthropic Claude
export CRAWLEX_SOLVER=vlm
export CRAWLEX_SOLVER_VLM_PROVIDER=anthropic
export CRAWLEX_SOLVER_VLM_API_KEY=sk-ant-...
export CRAWLEX_SOLVER_VLM_MODEL=claude-opus-4-7

# Human-handoff as a fallback
export CRAWLEX_HANDOFF=1

# Run a warmed, Android-emulated crawl
crawlex crawl --seed https://target.example \
    --method render \
    --policy deep \
    --motion-profile human
```

## 7. Testing your wire-up

Every adapter ships a unit-test suite that pins the public contract:

```
cargo test -p crawlex --lib proxy::residential
cargo test -p crawlex --lib identity::warmup
cargo test -p crawlex --lib render::android_profile
cargo test -p crawlex --lib render::handoff
cargo test -p crawlex --lib antibot::solver
```

Integration tests — the cross-module contract the operator actually
cares about — live at `tests/infra_scaffold_interfaces.rs`.
