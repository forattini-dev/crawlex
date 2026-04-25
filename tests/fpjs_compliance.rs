//! Static compliance checks for the stealth shim source.
//!
//! These are pure string inspections on `STEALTH_SHIM_TEMPLATE`. No browser
//! is launched; we assert the guards listed in backlog P2.3 (performance.memory
//! heap spoof) and P2.6 (Battery / Sensors absence) are physically present in
//! the shim so a regression that drops them is caught at `cargo test --lib`-
//! adjacent cost. Keeping these offline-only avoids flake from Chromium
//! availability and lets the check run in every CI lane.

use crawlex::render::stealth::STEALTH_SHIM_TEMPLATE;

const SHIM: &str = STEALTH_SHIM_TEMPLATE;

// --------------------------------------------------------------
// P2.3 — performance.memory spoof.
// --------------------------------------------------------------

#[test]
fn shim_defines_performance_memory_section() {
    assert!(
        SHIM.contains("16. performance.memory spoof"),
        "section header for performance.memory spoof missing"
    );
}

#[test]
fn shim_pins_desktop_heap_limit() {
    // 2 GiB limit is the constant defining "desktop Chrome" class.
    assert!(
        SHIM.contains("2147483648"),
        "jsHeapSizeLimit must be pinned to 2 GiB (2147483648)"
    );
    assert!(
        SHIM.contains("jsHeapSizeLimit"),
        "jsHeapSizeLimit property missing from memory tuple"
    );
    assert!(
        SHIM.contains("totalJSHeapSize"),
        "totalJSHeapSize property missing from memory tuple"
    );
    assert!(
        SHIM.contains("usedJSHeapSize"),
        "usedJSHeapSize property missing from memory tuple"
    );
}

#[test]
fn shim_heap_jitter_derives_from_canvas_seed() {
    // The noise in totalJSHeapSize/usedJSHeapSize MUST reuse the session seed
    // already driving canvas/audio perturbation, so fingerprint vectors stay
    // in agreement across surfaces. The limit stays a constant (2 GiB).
    assert!(
        SHIM.contains("{{CANVAS_SEED}}"),
        "CANVAS_SEED placeholder must still exist (seed source for jitter)"
    );
    // Jitter derives from __crawlex_seed__ (populated from CANVAS_SEED at
    // section 11). The performance.memory section must reference it.
    let mem_idx = SHIM
        .find("16. performance.memory spoof")
        .expect("section 16 header must exist");
    let tail = &SHIM[mem_idx..];
    assert!(
        tail.contains("__crawlex_seed__"),
        "performance.memory section must reuse __crawlex_seed__ for jitter"
    );
    assert!(
        tail.contains("BASE_LIMIT"),
        "heap limit must be a named constant, not a seeded value"
    );
}

#[test]
fn shim_installs_memory_on_prototype() {
    // Property lives on Performance.prototype in real Chrome; installing on
    // the instance leaks via Object.getOwnPropertyDescriptor checks.
    let mem_idx = SHIM.find("16. performance.memory spoof").unwrap();
    let tail = &SHIM[mem_idx..];
    assert!(
        tail.contains("Performance.prototype") || tail.contains("Performance !== 'undefined'"),
        "performance.memory must install on Performance.prototype"
    );
    assert!(
        tail.contains("defineProperty"),
        "performance.memory override must use defineProperty"
    );
}

#[test]
fn shim_memory_spoof_is_idempotent() {
    let mem_idx = SHIM.find("16. performance.memory spoof").unwrap();
    let tail = &SHIM[mem_idx..];
    assert!(
        tail.contains("__crawlex_memory_installed__"),
        "performance.memory install must be guarded by an idempotence sentinel"
    );
}

// --------------------------------------------------------------
// P2.6 — Battery / Sensors absence on Desktop UA.
// --------------------------------------------------------------

#[test]
fn shim_defines_sensors_battery_section() {
    assert!(
        SHIM.contains("17. Sensors/Battery absence"),
        "section header for Sensors/Battery absence missing"
    );
}

#[test]
fn shim_removes_getbattery_from_navigator() {
    // Desktop Chrome 103+ removed navigator.getBattery entirely. The shim
    // must delete from both Navigator.prototype AND the instance, otherwise
    // `'getBattery' in navigator` still returns true.
    let sect = SHIM
        .find("17. Sensors/Battery absence")
        .expect("section 17 header must exist");
    let tail = &SHIM[sect..];
    assert!(
        tail.contains("delete Navigator.prototype.getBattery"),
        "must delete getBattery from Navigator.prototype"
    );
    assert!(
        tail.contains("delete navigator.getBattery"),
        "must also delete getBattery from the navigator instance"
    );
}

#[test]
fn shim_scrubs_desktop_incompatible_sensor_apis() {
    let sect = SHIM.find("17. Sensors/Battery absence").unwrap();
    let tail = &SHIM[sect..];
    // These constructors do not exist on a real desktop Chrome build. If the
    // vanilla Linux Chrome we ship leaks any of them, the shim must scrub it.
    for name in [
        "DeviceMotionEvent",
        "DeviceOrientationEvent",
        "Accelerometer",
        "Gyroscope",
        "LinearAccelerationSensor",
        "Magnetometer",
        "AbsoluteOrientationSensor",
        "RelativeOrientationSensor",
    ] {
        assert!(
            tail.contains(name),
            "sensor API `{name}` must be scrubbed in section 17"
        );
    }
}

#[test]
fn shim_nulls_motion_event_handler_slots() {
    let sect = SHIM.find("17. Sensors/Battery absence").unwrap();
    let tail = &SHIM[sect..];
    for handler in ["ondevicemotion", "ondeviceorientation"] {
        assert!(
            tail.contains(handler),
            "event-handler slot `{handler}` must be nulled"
        );
    }
}

// --------------------------------------------------------------
// B1 — HTMLIFrameElement.contentWindow Object.defineProperty override.
// --------------------------------------------------------------

#[test]
fn shim_defines_iframe_contentwindow_section() {
    assert!(
        SHIM.contains("18. HTMLIFrameElement.contentWindow"),
        "section header for HTMLIFrameElement.contentWindow missing"
    );
}

#[test]
fn shim_overrides_iframe_contentwindow_via_defineproperty() {
    assert!(
        SHIM.contains("HTMLIFrameElement.prototype, 'contentWindow'"),
        "contentWindow override not wired to HTMLIFrameElement.prototype"
    );
}

#[test]
fn shim_iframe_contentwindow_is_not_a_proxy() {
    // DataDome / PerimeterX detect Proxy-wrapped getters via toString and
    // stack-trace `Proxy.<get>` frames. The override must be a plain
    // accessor — never `new Proxy(HTMLIFrameElement...)`.
    assert!(
        !SHIM.contains("new Proxy(HTMLIFrameElement"),
        "contentWindow override must not use a Proxy variant"
    );
}

#[test]
fn shim_iframe_install_is_idempotent() {
    assert!(
        SHIM.contains("__crawlex_iframe_installed__"),
        "iframe contentWindow install must be guarded by an idempotence sentinel"
    );
}

// --------------------------------------------------------------
// C1 — Web Worker concurrency ceiling (section 27).
// --------------------------------------------------------------

#[test]
fn shim_defines_worker_concurrency_ceiling_section() {
    assert!(
        SHIM.contains("Web Worker concurrency ceiling"),
        "section header for Web Worker concurrency ceiling missing"
    );
}

#[test]
fn shim_worker_cap_install_is_idempotent() {
    assert!(
        SHIM.contains("__crawlex_worker_cap_installed__"),
        "Worker cap install must be guarded by an idempotence sentinel"
    );
}

#[test]
fn shim_worker_cap_throws_quota_exceeded_domexception() {
    // Excess-worker construction must raise a DOMException with name
    // 'QuotaExceededError' — the same shape a real resource-exhausted
    // browser emits, so detector probes see a plausible ceiling.
    assert!(
        SHIM.contains("QuotaExceededError"),
        "Worker cap must raise DOMException('QuotaExceededError') on overflow"
    );
}

// --------------------------------------------------------------

#[test]
fn memory_getter_registered_with_native_code_weakset() {
    // Section 13 maintains the WeakSet whose toString Proxy reports
    // '[native code]'. The new memoryGetter in section 16 must flow into
    // that set (via the late-bind `__crawlex_reg_target__` bridge) so a
    // caller doing `Object.getOwnPropertyDescriptor(Performance.prototype,
    // 'memory').get.toString()` gets the native-looking string.
    assert!(
        SHIM.contains("__crawlex_reg_target__"),
        "late-bind registrar bridge for section 16/17 hooks missing"
    );
    assert!(
        SHIM.contains("reg(memoryGetter)") || SHIM.contains("lateReg(memoryGetter)"),
        "memoryGetter must be registered into the [native code] WeakSet"
    );
}

#[test]
fn shim_is_syntactically_plausible() {
    // Basic sanity: section 17 is the tail, IIFE close `})();` still present.
    assert!(
        SHIM.trim_end().ends_with("})();"),
        "shim must remain a closed IIFE"
    );
    // No stray template tokens survived renaming.
    for tok in ["{{MISSING}}", "{{TODO}}"] {
        assert!(!SHIM.contains(tok), "unexpected placeholder {tok} in shim");
    }
}

// --------------------------------------------------------------
// Camoufox port — Sprint 1/2/3 sections.
// --------------------------------------------------------------

#[test]
fn shim_perf_now_clamps_to_chrome_non_coi_grain() {
    // Camoufox research: Chrome non-COI clamps to 100 µs (0.1 ms), not 5 µs.
    // A 5 µs grain is itself a tell.
    assert!(
        SHIM.contains("const GRAIN = 0.1;"),
        "performance.now() grain must be 100 µs (0.1 ms), not 5 µs"
    );
    assert!(
        SHIM.contains("xorshift32") || SHIM.contains("jState"),
        "performance.now() seeded jitter state must be present"
    );
}

#[test]
fn shim_audio_buffer_get_channel_data_is_shimmed() {
    // FPJS reads samples via AudioBuffer.prototype.getChannelData without
    // ever calling startRendering — §12 extension must wrap it + copyFromChannel.
    assert!(
        SHIM.contains("AudioBuffer.prototype.getChannelData")
            || SHIM.contains("AB.prototype.getChannelData"),
        "AudioBuffer.getChannelData wrapper missing from §12"
    );
    assert!(
        SHIM.contains("copyFromChannel"),
        "AudioBuffer.copyFromChannel wrapper missing from §12"
    );
}

#[test]
fn shim_canvas_preserves_zero_channels() {
    // Camoufox rule: skip alpha, walk RGB, nudge first non-zero. Guarantees
    // `clearRect + getImageData` returns all zeros.
    assert!(
        SHIM.contains("if (v === 0) continue;"),
        "canvas perturb must skip zero channels"
    );
    // Old implementation signature must not re-appear.
    assert!(
        !SHIM.contains("img.data[i] = (img.data[i] + salt) & 0xff;"),
        "legacy salt+stride canvas perturb leaked back into §11"
    );
}

#[test]
fn shim_webgl_is_enabled_pinned_defaults() {
    // §10 hook must carry the MakeIsEnabledMap default table and the touched
    // WeakMap fallthrough so caller-driven enable/disable still works.
    assert!(
        SHIM.contains("3042: false,") && SHIM.contains("3024: true,"),
        "WebGL isEnabled Chrome defaults (BLEND=false, DITHER=true) missing"
    );
    assert!(
        SHIM.contains("touched.get(this)"),
        "caller-flipped cap tracking (touched WeakMap) missing from §10"
    );
}

#[test]
fn shim_media_devices_counts_are_persona_driven() {
    // §23 must read counts from placeholders populated by IdentityBundle,
    // not hardcode 1+1+1.
    assert!(
        SHIM.contains("{{MEDIA_MIC_COUNT}}") || SHIM.contains("MIC_COUNT"),
        "media mic count must be persona-driven"
    );
    assert!(
        SHIM.contains("{{MEDIA_SPEAKER_COUNT}}") || SHIM.contains("SPEAKER_COUNT"),
        "media speaker count must be persona-driven"
    );
    assert!(
        SHIM.contains("getUserMedia"),
        "§23 must stub navigator.mediaDevices.getUserMedia"
    );
}

#[test]
fn shim_text_metrics_section_present() {
    // §28 TextMetrics / measureText jitter header + FNV hash.
    assert!(
        SHIM.contains("28. TextMetrics / measureText jitter"),
        "§28 TextMetrics/measureText section header missing"
    );
    assert!(
        SHIM.contains("actualBoundingBoxAscent") && SHIM.contains("fontBoundingBoxAscent"),
        "§28 must perturb the full TextMetrics field set"
    );
    assert!(
        SHIM.contains("0x811c9dc5"),
        "§28 must use FNV-1a 32-bit hash for (string, font) determinism"
    );
}

#[test]
fn shim_webrtc_scrub_section_present() {
    // §29 WebRTC scrub: SDP filter, onicecandidate filter, getStats sanitize.
    assert!(
        SHIM.contains("29. WebRTC SDP/ICE/getStats scrub"),
        "§29 WebRTC scrub section header missing"
    );
    assert!(
        SHIM.contains("a=candidate:"),
        "§29 SDP filter must strip a=candidate lines"
    );
    assert!(
        SHIM.contains("local-candidate"),
        "§29 getStats sanitizer must target local-candidate entries"
    );
    assert!(
        SHIM.contains("192.168") && SHIM.contains("fe80:"),
        "§29 private-IP regex must cover IPv4 + IPv6 link-local"
    );
}

#[test]
fn worker_shim_uses_globalthis_safe_sections() {
    // Camoufox port S3.1: render_worker_shim_from_bundle must produce a
    // worker-scope shim that retains globalThis-accessible sections
    // (navigator, AudioContext, performance, WebRTC) but strips DOM-only
    // ones. This is a static guard; the live test exercises the runtime.
    use crawlex::identity::IdentityBundle;
    use crawlex::render::stealth::render_worker_shim_from_bundle;

    let bundle = IdentityBundle::from_chromium(131, 0xc0_ffee);
    let worker_shim = render_worker_shim_from_bundle(&bundle);

    // Worker-compatible sections must remain.
    assert!(
        worker_shim.contains("1. Navigator identity"),
        "worker shim must retain §1 navigator identity"
    );
    assert!(
        worker_shim.contains("21. performance.now() precision clamp"),
        "worker shim must retain §21 performance.now clamp"
    );
    assert!(
        worker_shim.contains("12. AudioContext"),
        "worker shim must retain §12 AudioContext"
    );
    // No leftover template tokens.
    assert!(
        !worker_shim.contains("{{"),
        "worker shim has unsubstituted placeholders"
    );
}

#[test]
fn worker_shim_strips_dom_only_sections() {
    // The marker pairs `// @worker-skip-start` / `// @worker-skip-end`
    // around §0, §5, §9, §11, §18, §19, §20, §24, §28 must be honoured —
    // the corresponding section headers MUST NOT appear in the worker
    // variant. Detector relevance: workers have no document/screen/
    // matchMedia/HTMLCanvasElement, so executing those sections would
    // throw and the safe() wrapper would swallow it silently — the
    // tell is structural, not behavioural.
    use crawlex::identity::IdentityBundle;
    use crawlex::render::stealth::render_worker_shim_from_bundle;

    let bundle = IdentityBundle::from_chromium(131, 0xdead_beef);
    let worker_shim = render_worker_shim_from_bundle(&bundle);

    for needle in [
        "0. Scrub automation tokens",
        "5. Screen — desktop",
        "9. matchMedia queries",
        "11. Canvas 2D",
        "18. HTMLIFrameElement.contentWindow",
        "19. Window outer/inner geometry",
        "20. requestAnimationFrame",
        "24. speechSynthesis.getVoices",
        "28. TextMetrics / measureText jitter",
    ] {
        assert!(
            !worker_shim.contains(needle),
            "worker shim must NOT contain DOM-only section: {needle}"
        );
    }
    // And the marker comments themselves must be consumed (not leaked
    // into the rendered output).
    assert!(
        !worker_shim.contains("@worker-skip-start"),
        "worker shim must consume @worker-skip-start markers"
    );
    assert!(
        !worker_shim.contains("@worker-skip-end"),
        "worker shim must consume @worker-skip-end markers"
    );
}
