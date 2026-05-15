# PRD: Native Stealth Browser Providers

Status: needs-triage
Labels: needs-triage

## Problem Statement

crawlex currently gets work done by controlling a Chromium-family browser with launch flags, CDP discipline, human motion, and a comprehensive stealth shim. This gives strong default behavior, but it still depends on a stock browser runtime whose native fingerprint surfaces are not under crawlex's control.

New native stealth engines such as CloakBrowser and Camoufox patch browser behavior below the JavaScript layer. CloakBrowser exposes a Chromium/CDP-compatible path with native fingerprint flags and a CDP multiplexer, while Camoufox exposes a Firefox/Playwright-oriented path with C++-level fingerprint injection. crawlex needs a pragmatic integration strategy that uses these engines when available without turning the public API into vendor-specific flags, without weakening crawlex's own stealth layer, and without forcing a full browser-agnostic rewrite.

The user wants crawlex to get the job done: keep the current stock Chromium path working, but allow stronger external browser engines to be used when configured, calibrate against their effective fingerprint, and adapt crawlex's stealth layer so the combined result is coherent.

## Solution

Add a neutral browser provider layer focused on external CDP endpoints first. The first deliverable connects crawlex to a user-provided CDP endpoint, detects whether it behaves like a native stealth multiplexer such as cloakserve, passes high-level identity constraints when supported, runs a local fingerprint calibration page before visiting the target, and adapts crawlex's own stealth shim to the effective fingerprint measured from the running browser.

The public interface stays vendor-neutral. Users select a provider mode such as stock, cdp, or auto, and optionally provide a browser endpoint. crawlex may detect CloakBrowser capabilities internally, but the CLI does not grow Cloak-specific flags. For external CDP providers, calibration is mandatory. The default policy adapts to mismatches and logs them; a strict policy can fail before target navigation when a critical mismatch cannot be reconciled.

crawlex's stealth layer is never disabled. Native engines may generate the base identity, but crawlex still applies its own operational stealth, CDP discipline, human interaction, worker handling, collection hooks, and shim behavior. The shim becomes capability-aware and calibration-aware so it does not overwrite native engine output with contradictory values.

## User Stories

1. As a crawlex operator, I want to keep using the current stock Chromium flow by default, so that existing crawls and tests keep working without new dependencies.
2. As a crawlex operator, I want to connect to a CDP endpoint, so that I can use a browser engine that is already running outside crawlex.
3. As a crawlex operator, I want the CDP endpoint interface to be vendor-neutral, so that my scripts do not depend on CloakBrowser-specific option names.
4. As a crawlex operator, I want crawlex to detect native stealth capabilities, so that it can adapt behavior without requiring me to know every backend detail.
5. As a crawlex operator, I want crawlex to detect cloakserve when pointed at its endpoint, so that CloakBrowser's per-connection fingerprint contract can be used automatically.
6. As a crawlex operator, I want crawlex to pass fingerprint seed, timezone, locale, proxy, and geoip constraints to compatible endpoints, so that native engines can generate coherent identities.
7. As a crawlex operator, I want crawlex to avoid silent local endpoint discovery by default, so that it does not connect to an unintended CDP server.
8. As a crawlex operator, I want an explicit auto mode, so that crawlex can try configured external providers only when I ask it to.
9. As a crawlex operator, I want every external CDP session calibrated before target navigation, so that crawlex does not inject an identity that contradicts the running browser.
10. As a crawlex operator, I want calibration to happen against a local HTTP origin, so that storage, permissions, WebRTC, and browser APIs behave like they would on a real site.
11. As a crawlex operator, I want calibration cached per render session, so that crawlex avoids redundant work while still recalibrating when seed, proxy, locale, timezone, profile, or endpoint changes.
12. As a crawlex operator, I want crawlex to record the effective browser fingerprint, so that I can debug why one provider passes and another provider fails.
13. As a crawlex operator, I want a concise provider-selected event, so that NDJSON and logs explain whether stock Chromium or an external CDP provider was used.
14. As a crawlex operator, I want a calibration summary event, so that I can see the effective browser product, platform, locale, timezone, WebGL renderer, and mismatch count.
15. As a crawlex operator, I want an optional full fingerprint report, so that I can compare stock, CloakBrowser, Camoufox, and future engines without dumping sensitive details by default.
16. As a crawlex operator, I want the default mismatch behavior to warn and adapt, so that crawlex still gets the crawl done when a provider is usable but imperfect.
17. As a crawlex operator, I want a strict mismatch policy, so that high-sensitivity runs can fail before spending proxy/session budget on a target.
18. As a crawlex operator, I want crawlex's stealth shim to remain active with native stealth engines, so that crawlex does not lose its own protections and collection behavior.
19. As a crawlex operator, I want crawlex's shim to use calibrated values where possible, so that it does not fight the native engine's C++ fingerprint generation.
20. As a crawlex operator, I want crawlex to create its own page or context for external CDP sessions, so that existing browser tabs and profiles are not polluted.
21. As a crawlex operator, I want session isolation to remain the default, so that cookies and storage from an external browser do not accidentally affect crawl reproducibility.
22. As a crawlex operator, I want an explicit persistent-session mode, so that I can intentionally reuse cookies, localStorage, and browser cache when a target penalizes ephemeral sessions.
23. As a crawlex operator, I want fallback between providers to be opt-in, so that crawlex does not unexpectedly spend proxy budget or change identity after a block.
24. As a crawlex operator, I want the first implementation to focus on CDP endpoints, so that the useful CloakBrowser path lands before a larger browser-service abstraction.
25. As a crawlex maintainer, I want stock Chromium tests to keep passing, so that native provider support does not regress the default crawler.
26. As a crawlex maintainer, I want endpoint detection isolated in a deep module, so that future providers can be added without spreading vendor checks through the render pipeline.
27. As a crawlex maintainer, I want calibration isolated in a deep module, so that probe logic can be tested independently from browser launch and crawl scheduling.
28. As a crawlex maintainer, I want provider capability detection to drive behavior, so that the code does not depend on brand names when a capability is what matters.
29. As a crawlex maintainer, I want external CDP failure modes to be explicit, so that bad endpoints, unreachable endpoints, and failed calibration produce actionable errors.
30. As a crawlex maintainer, I want live CloakBrowser tests to be optional, so that CI does not depend on a third-party binary or service.

## Implementation Decisions

- Keep the current stock Chromium provider as the default behavior.
- Add a neutral provider selection model with stock, cdp, and auto modes.
- Add a neutral browser endpoint setting and environment-variable equivalent for external CDP endpoints.
- Do not add vendor-specific public flags for CloakBrowser or Camoufox in the first implementation.
- Do not silently scan local ports for external CDP services unless the user explicitly selects auto behavior or configures an endpoint.
- Prioritize external CDP endpoint integration before direct binary launch integration.
- Detect cloakserve as an internal capability by probing its HTTP surface and CDP URL behavior.
- When a compatible native stealth multiplexer is detected, pass high-level identity constraints through its endpoint contract before connecting.
- Treat query-string identity injection as a provider capability, not as a universal CDP behavior.
- Always run calibration for external CDP providers before navigating to the target.
- Run calibration on a local HTTP origin served by crawlex, not on about:blank or a data URL.
- Cache calibration per render session and invalidate it when endpoint, seed, proxy, locale, timezone, profile, user data scope, or relevant context identity changes.
- Introduce an effective browser fingerprint model that records the measured browser identity and mismatch summary.
- Use the effective browser fingerprint to parameterize the stealth shim for external CDP sessions.
- Never disable crawlex's stealth layer. Instead make it calibration-aware and avoid contradictory overwrites.
- Keep crawlex as the source of high-level crawl/session intent, while allowing native engines to generate detailed browser fingerprints when their generator is more coherent.
- For native engines, prefer passing seed, proxy, timezone, locale, and geoip constraints over forcing every low-level GPU, screen, memory, and font value.
- For stock Chromium launched by crawlex, preserve the current IdentityBundle-driven behavior.
- For external CDP providers, create a crawlex-owned page or context for each render session when possible.
- Keep isolated session behavior as the default for external CDP.
- Add explicit persistent-session behavior later or behind a clear user setting when existing profile state should be reused.
- Add a default adapt policy for fingerprint mismatches and a strict policy for fail-fast runs.
- Emit provider selection and calibration events through the existing event/logging surface.
- Emit full fingerprint reports only when explicitly requested.
- Keep automatic challenge/provider fallback out of the first deliverable.
- Keep Camoufox support out of the first deliverable unless it can be reached through the same external endpoint abstraction without changing scope.
- Treat CloakBrowser licensing as bring-your-own or user-managed service in this PRD. Do not redistribute its binary as part of crawlex.

## Testing Decisions

- Good tests should assert external behavior: provider selection, endpoint detection, calibration output, mismatch policy behavior, shim parameterization, and emitted events.
- Do not test implementation details such as exact internal helper function boundaries unless they define a stable module interface.
- Unit test provider selection with combinations of provider mode, endpoint setting, environment variable, and absent configuration.
- Unit test endpoint detection using mocked HTTP responses for generic CDP endpoints and cloakserve-like endpoints.
- Unit test identity query construction for compatible native stealth endpoints, including seed, locale, timezone, proxy, and geoip behavior.
- Unit test calibration result parsing from representative browser probe payloads.
- Unit test mismatch classification for critical and non-critical mismatches.
- Unit test adapt versus strict policy behavior.
- Unit test the shim identity adapter so calibrated UA, platform, languages, timezone, screen/window, WebGL, storage, media, and WebGPU values are reflected in generated shim configuration.
- Integration test external CDP against a normal local Chromium endpoint when available.
- Add optional ignored/live tests for cloakserve that only run when the endpoint is configured.
- Preserve existing browser launch flag tests, stealth runtime tests, worker shim tests, WebRTC leak audit tests, motion tests, typing tests, and real-world antibot validation tests.
- Add event snapshot tests or structured event assertions for provider selection, calibration success, calibration failure, and provider fallback-not-enabled behavior.
- Use current live antibot validation style for manual BrowserScan/FingerprintJS comparison between stock and native CDP providers.

## Out of Scope

- Redistributing the CloakBrowser binary.
- Building or maintaining a crawlex Chromium fork.
- Implementing a Camoufox/Firefox adapter in the first deliverable.
- Adding vendor-specific public CLI flags.
- Silent local port scanning for CDP endpoints.
- Automatic challenge-driven provider fallback in the first deliverable.
- A full CreepJS-style detector embedded into crawlex.
- Public benchmark claims about passing Cloudflare, reCAPTCHA, FingerprintJS, or BrowserScan.
- CAPTCHA solving or proxy rotation changes beyond passing configured proxy information to compatible providers.
- Direct browser install managers for third-party stealth engines.

## Further Notes

- CloakBrowser's wrapper code is MIT, but its patched Chromium binary has separate terms that allow use but restrict redistribution and product bundling. The first integration should assume user-managed CloakBrowser or a user-managed cloakserve endpoint.
- CloakBrowser is the most pragmatic first native provider because it remains Chromium/CDP-compatible.
- Camoufox remains strategically interesting, but its Firefox/Playwright/Juggler shape makes it a later adapter or service integration rather than the first PR.
- The first implementation should be deliberately small: external CDP provider, cloakserve capability detection, calibration, shim adaptation, and events.
- The accepted design principle is "get the job done": capability detection and calibrated behavior matter more than browser-agnostic purity.
