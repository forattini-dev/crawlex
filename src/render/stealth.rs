//! Stealth shim template substitution.
//!
//! `stealth_shim.js` ships with `{{PLACEHOLDER}}` tokens where we used to
//! hardcode "Chrome/131", "America/Sao_Paulo", etc. At render time we
//! substitute them from the active `IdentityBundle` so the shim, the
//! Chromium launch flags, the `Network.setUserAgentOverride` and the HTTP
//! headers all tell the same story.

use crate::identity::IdentityBundle;
use crate::impersonate::Profile;

pub const STEALTH_SHIM_TEMPLATE: &str = include_str!("stealth_shim.js");

/// Legacy shim vars driven by `Profile` + caller-supplied locale/timezone.
/// Retained for transition; prefer `render_shim_from_bundle`.
pub struct ShimVars<'a> {
    pub profile: Profile,
    pub locale: &'a str,
    pub languages_json: &'a str,
    pub timezone: &'a str,
    pub tz_offset_min: i32,
    pub platform: &'a str,
}

/// Token set the shim template expects. Centralised so the bundle-based
/// and profile-based paths can't drift on which placeholders they fill.
struct ShimSubstitutions<'a> {
    user_agent: &'a str,
    app_version: &'a str,
    ua_brands: &'a str,
    ua_full_version_list: &'a str,
    ua_full_version: &'a str,
    ua_major: &'a str,
    platform: &'a str,
    ua_platform: &'a str,
    locale: &'a str,
    languages_json: &'a str,
    timezone: &'a str,
    tz_offset_min: i32,
    /// Seed injected into `window.__crawlex_seed__` for deterministic
    /// canvas / audio / WebGL perturbation within a session. Truncated to
    /// the low 31 bits at substitution time so the JS integer literal
    /// stays within the safe bitwise-op range.
    canvas_seed: u32,
    /// WebGL unmasked vendor — also reused as the WebGPU adapter vendor so
    /// the two APIs never diverge on vendor string.
    webgl_unmasked_vendor: &'a str,
    /// WebGPU adapter description — must share the WebGL vendor keyword
    /// (validator enforces this). Defaults when absent keep the old
    /// profile-based path working.
    webgpu_adapter_description: &'a str,
    /// Wave1 coherence scalars — see `IdentityBundle` for the rationale
    /// on each. Kept as `&str` for integers too so `apply` stays one
    /// `.replace()` per placeholder and no ad-hoc conversions leak in.
    scrollbar_width: u32,
    heap_size_limit: u64,
    device_memory: u32,
    hardware_concurrency: u32,
    max_texture_size: u32,
    max_viewport_w: u32,
    max_viewport_h: u32,
    audio_sample_rate: u32,
    fonts_json: &'a str,
    gpu_vendor_keyword: &'a str,
    /// `navigator.mediaDevices.enumerateDevices()` surface counts (§23).
    /// Camoufox research: 1/1/1 is a tell — desktops expose built-in +
    /// virtual/bluetooth. Per-persona counts carried in `IdentityBundle`.
    media_mic_count: u8,
    media_cam_count: u8,
    media_speaker_count: u8,
    /// Whether `navigator.getBattery` is exposed (mobile personas only).
    /// Desktop Chrome 103+ removed the Battery API; mobile Chrome still
    /// ships it. When `true`, Section 7 installs a deterministic 24h
    /// charge/discharge curve anchored to local midnight (via the
    /// timezone in this bundle), and Section 17 leaves the API alone.
    /// When `false`, the API is deleted (current behavior).
    expose_battery: bool,
}

/// JS string literal escape — the bundle-sourced strings land inside
/// single-quoted JS literals in the shim, so a stray `'` or `\` would break
/// the whole script. Only the characters that actually escape in that
/// context are handled.
fn js_str_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn apply(s: &ShimSubstitutions<'_>) -> String {
    apply_to(STEALTH_SHIM_TEMPLATE, s)
}

fn apply_to(template: &str, s: &ShimSubstitutions<'_>) -> String {
    template
        .replace("{{USER_AGENT}}", s.user_agent)
        .replace("{{APP_VERSION}}", s.app_version)
        .replace("{{UA_BRANDS}}", s.ua_brands)
        .replace("{{UA_FULL_VERSION_LIST}}", s.ua_full_version_list)
        .replace("{{UA_FULL_VERSION}}", s.ua_full_version)
        .replace("{{UA_MAJOR}}", s.ua_major)
        .replace("{{PLATFORM}}", s.platform)
        .replace("{{UA_PLATFORM}}", s.ua_platform)
        .replace("{{LOCALE}}", s.locale)
        .replace("{{LANGUAGES_JSON}}", s.languages_json)
        .replace("{{TIMEZONE}}", s.timezone)
        .replace("{{TZ_OFFSET_MIN}}", &s.tz_offset_min.to_string())
        .replace("{{CANVAS_SEED}}", &s.canvas_seed.to_string())
        .replace(
            "{{WEBGL_UNMASKED_VENDOR}}",
            &js_str_escape(s.webgl_unmasked_vendor),
        )
        .replace(
            "{{WEBGPU_ADAPTER_DESCRIPTION}}",
            &js_str_escape(s.webgpu_adapter_description),
        )
        .replace("{{SCROLLBAR_WIDTH}}", &s.scrollbar_width.to_string())
        .replace("{{HEAP_SIZE_LIMIT}}", &s.heap_size_limit.to_string())
        .replace("{{DEVICE_MEMORY}}", &s.device_memory.to_string())
        .replace("{{HW_CONCURRENCY}}", &s.hardware_concurrency.to_string())
        .replace("{{MAX_TEXTURE_SIZE}}", &s.max_texture_size.to_string())
        .replace("{{MAX_VIEWPORT_W}}", &s.max_viewport_w.to_string())
        .replace("{{MAX_VIEWPORT_H}}", &s.max_viewport_h.to_string())
        .replace("{{AUDIO_SAMPLE_RATE}}", &s.audio_sample_rate.to_string())
        .replace("{{FONTS_JSON}}", s.fonts_json)
        .replace(
            "{{GPU_VENDOR_KEYWORD}}",
            &js_str_escape(s.gpu_vendor_keyword),
        )
        .replace("{{MEDIA_MIC_COUNT}}", &s.media_mic_count.to_string())
        .replace("{{MEDIA_CAM_COUNT}}", &s.media_cam_count.to_string())
        .replace(
            "{{MEDIA_SPEAKER_COUNT}}",
            &s.media_speaker_count.to_string(),
        )
        .replace(
            "{{EXPOSE_BATTERY}}",
            if s.expose_battery { "true" } else { "false" },
        )
}

/// Derive the 31-bit JS-safe canvas/audio seed from the bundle's
/// `canvas_audio_seed`. Masking to 31 bits keeps the integer positive
/// when cast through JS `| 0` and avoids sign-bit surprises in the
/// bitwise ops used inside the shim.
/// Recover a short lowercase vendor keyword (intel/nvidia/amd/apple/adreno)
/// from a WebGL/WebGPU renderer string. Mirrors the validator's helper but
/// is local to this module to avoid plumbing the private enum across
/// crate boundaries. Falls back to `"intel"` so legacy callers produce
/// a coherent string even on exotic input.
fn gpu_keyword_from_renderer(r: &str) -> &'static str {
    let l = r.to_ascii_lowercase();
    if l.contains("apple") {
        "apple"
    } else if l.contains("nvidia") {
        "nvidia"
    } else if l.contains("amd") || l.contains("radeon") {
        "amd"
    } else if l.contains("adreno") || l.contains("qualcomm") {
        "adreno"
    } else {
        "intel"
    }
}

fn seed_u31(raw: u64) -> u32 {
    let mixed = raw ^ (raw >> 32);
    (mixed as u32) & 0x7fff_ffff
}

/// Legacy entry — kept so existing callers keep compiling while the render
/// pool migrates to the bundle-based path. Prefer [`render_shim_from_bundle`].
///
/// **Deprecated**: this path hardcodes the Intel WebGL/WebGPU persona.
/// A caller that ships an AMD/NVIDIA identity through this function
/// will silently inject Intel strings into the shim, re-introducing
/// the cross-field FP inconsistency the validator was added to catch.
/// New code must use [`render_shim_from_bundle`] so the GPU keyword
/// flows from the same [`IdentityBundle`] the validator inspected.
#[deprecated(
    note = "hardcodes Intel GPU strings; use render_shim_from_bundle with a validated IdentityBundle"
)]
pub fn render_shim(vars: &ShimVars<'_>) -> String {
    let ua = vars.profile.user_agent();
    let app_version = ua.strip_prefix("Mozilla/").unwrap_or(&ua).to_string();
    let ua_brands = vars.profile.ua_brands_json();
    let ua_full_version_list = vars.profile.fullversion_brands_json();
    let ua_full_version = vars.profile.ua_full_version();
    let ua_major = vars.profile.major_version().to_string();
    let ua_platform = vars
        .platform
        .split_whitespace()
        .next()
        .unwrap_or("Linux")
        .to_string();
    apply(&ShimSubstitutions {
        user_agent: &ua,
        app_version: &app_version,
        ua_brands: &ua_brands,
        ua_full_version_list: &ua_full_version_list,
        ua_full_version: &ua_full_version,
        ua_major: &ua_major,
        platform: vars.platform,
        ua_platform: &ua_platform,
        locale: vars.locale,
        languages_json: vars.languages_json,
        timezone: vars.timezone,
        tz_offset_min: vars.tz_offset_min,
        // Legacy path has no bundle; fall back to a deterministic seed
        // derived from TZ offset + profile major so the shim remains
        // internally coherent (no Date.now leakage).
        canvas_seed: seed_u31(
            (vars.tz_offset_min as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (vars.profile.major_version() as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9),
        ),
        // Legacy callers don't plumb a bundle; keep the historical Intel
        // strings so the rendered shim remains valid JS. New code should
        // use `render_shim_from_bundle`.
        webgl_unmasked_vendor: "Google Inc. (Intel)",
        webgpu_adapter_description:
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)",
        scrollbar_width: 15,
        heap_size_limit: 2_147_483_648,
        device_memory: 8,
        hardware_concurrency: 8,
        max_texture_size: 16384,
        max_viewport_w: 32767,
        max_viewport_h: 32767,
        audio_sample_rate: 48000,
        fonts_json: r#"["DejaVu Sans","Liberation Sans","Noto Sans","Ubuntu"]"#,
        gpu_vendor_keyword: "intel",
        media_mic_count: 2,
        media_cam_count: 1,
        media_speaker_count: 2,
        // Legacy path defaults to desktop persona — Battery API hidden.
        expose_battery: false,
    })
}

/// Strip blocks delimited by `// @worker-skip-start` / `// @worker-skip-end`.
///
/// Workers expose a different global surface than DOM windows: no `document`,
/// no `HTMLCanvasElement`, no `screen`/`matchMedia`/`requestAnimationFrame`/
/// `speechSynthesis`. Sections of the shim that touch those APIs are wrapped
/// in marker comments so the worker variant can drop them without forking
/// the template into a second `.js` file. Markers must sit on their own line.
fn strip_worker_skip_blocks(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_skip = false;
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed == "// @worker-skip-start" {
            in_skip = true;
            continue;
        }
        if trimmed == "// @worker-skip-end" {
            in_skip = false;
            continue;
        }
        if !in_skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn build_subs<'a>(bundle: &'a IdentityBundle, scratch: &'a ShimScratch) -> ShimSubstitutions<'a> {
    ShimSubstitutions {
        user_agent: &bundle.ua,
        app_version: &scratch.app_version,
        ua_brands: &bundle.ua_brands,
        ua_full_version_list: &bundle.ua_full_version_list,
        ua_full_version: &bundle.ua_full_version,
        ua_major: &scratch.ua_major,
        platform: &bundle.platform,
        ua_platform: &scratch.ua_platform,
        locale: &bundle.locale,
        languages_json: &bundle.languages_json,
        timezone: &bundle.timezone,
        tz_offset_min: bundle.tz_offset_min,
        canvas_seed: seed_u31(bundle.canvas_audio_seed),
        webgl_unmasked_vendor: &bundle.webgl_unmasked_vendor,
        webgpu_adapter_description: &bundle.webgpu_adapter_description,
        scrollbar_width: bundle.scrollbar_width,
        heap_size_limit: bundle.heap_size_limit,
        device_memory: bundle.device_memory,
        hardware_concurrency: bundle.hardware_concurrency,
        max_texture_size: bundle.max_texture_size,
        max_viewport_w: bundle.max_viewport_w,
        max_viewport_h: bundle.max_viewport_h,
        audio_sample_rate: bundle.audio_sample_rate,
        fonts_json: &bundle.fonts_json,
        gpu_vendor_keyword: gpu_keyword_from_renderer(&bundle.webgl_renderer),
        media_mic_count: bundle.media_mic_count,
        media_cam_count: bundle.media_cam_count,
        media_speaker_count: bundle.media_speaker_count,
        // Mobile personas (Adreno/Qualcomm) still ship Battery API.
        // Desktop Chrome 103+ removed it; keep hidden on those.
        expose_battery: gpu_keyword_from_renderer(&bundle.webgl_renderer) == "adreno",
    }
}

struct ShimScratch {
    app_version: String,
    ua_major: String,
    ua_platform: String,
}

impl ShimScratch {
    fn from_bundle(bundle: &IdentityBundle) -> Self {
        Self {
            app_version: bundle
                .ua
                .strip_prefix("Mozilla/")
                .unwrap_or(&bundle.ua)
                .to_string(),
            ua_major: bundle.ua_major.to_string(),
            ua_platform: bundle.ua_platform.trim_matches('"').to_string(),
        }
    }
}

/// Worker-scope variant of the stealth shim. Same persona substitutions, but
/// DOM-only sections (`@worker-skip-start`/`@worker-skip-end` blocks in
/// `stealth_shim.js`) are stripped so the script runs cleanly inside
/// dedicated/shared/service worker contexts where `document`,
/// `HTMLCanvasElement`, `window.matchMedia`, etc. don't exist.
///
/// Used by the CDP `Target.attachedToTarget` handler to inject persona
/// coherence into worker globals before any user script runs (Camoufox port
/// Sprint 3 S3.1).
pub fn render_worker_shim_from_bundle(bundle: &IdentityBundle) -> String {
    let stripped = strip_worker_skip_blocks(STEALTH_SHIM_TEMPLATE);
    let scratch = ShimScratch::from_bundle(bundle);
    let subs = build_subs(bundle, &scratch);
    apply_to(&stripped, &subs)
}

/// Build the shim from an `IdentityBundle`. Every placeholder is drawn
/// from the same struct, so (a) coherence is enforced by construction
/// and (b) the session-scoped `canvas_audio_seed` flows into the JS as
/// `window.__crawlex_seed__` via `{{TZ_OFFSET_MIN}}`-style substitution.
pub fn render_shim_from_bundle(bundle: &IdentityBundle) -> String {
    // `ua_platform` in the bundle is already wrapped in quotes
    // (`"Linux"`) because it's the Sec-CH-UA-Platform HTTP value; for the
    // JS substitution we want the bare token. Strip one layer of quotes.
    let ua_platform_raw = bundle.ua_platform.trim_matches('"').to_string();
    let ua_major = bundle.ua_major.to_string();
    let app_version = bundle
        .ua
        .strip_prefix("Mozilla/")
        .unwrap_or(&bundle.ua)
        .to_string();
    apply(&ShimSubstitutions {
        user_agent: &bundle.ua,
        app_version: &app_version,
        ua_brands: &bundle.ua_brands,
        ua_full_version_list: &bundle.ua_full_version_list,
        ua_full_version: &bundle.ua_full_version,
        ua_major: &ua_major,
        platform: &bundle.platform,
        ua_platform: &ua_platform_raw,
        locale: &bundle.locale,
        languages_json: &bundle.languages_json,
        timezone: &bundle.timezone,
        tz_offset_min: bundle.tz_offset_min,
        canvas_seed: seed_u31(bundle.canvas_audio_seed),
        webgl_unmasked_vendor: &bundle.webgl_unmasked_vendor,
        webgpu_adapter_description: &bundle.webgpu_adapter_description,
        scrollbar_width: bundle.scrollbar_width,
        heap_size_limit: bundle.heap_size_limit,
        device_memory: bundle.device_memory,
        hardware_concurrency: bundle.hardware_concurrency,
        max_texture_size: bundle.max_texture_size,
        max_viewport_w: bundle.max_viewport_w,
        max_viewport_h: bundle.max_viewport_h,
        audio_sample_rate: bundle.audio_sample_rate,
        fonts_json: &bundle.fonts_json,
        gpu_vendor_keyword: gpu_keyword_from_renderer(&bundle.webgl_renderer),
        media_mic_count: bundle.media_mic_count,
        media_cam_count: bundle.media_cam_count,
        media_speaker_count: bundle.media_speaker_count,
        expose_battery: gpu_keyword_from_renderer(&bundle.webgl_renderer) == "adreno",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityBundle;

    #[test]
    fn bundle_substitutions_fill_every_placeholder() {
        let b = IdentityBundle::from_chromium(131, 42);
        let js = render_shim_from_bundle(&b);
        // No residual `{{...}}` tokens.
        assert!(
            !js.contains("{{"),
            "template has unsubstituted tokens: {}",
            js.lines().find(|l| l.contains("{{")).unwrap_or("")
        );
        // Spot-check some critical values.
        assert!(js.contains(&b.ua), "UA missing");
        assert!(js.contains(&b.timezone), "timezone missing");
        assert!(js.contains(&b.locale), "locale missing");
        assert!(js.contains(&b.ua_full_version), "full version missing");
    }

    #[test]
    fn canvas_seed_is_deterministic_for_same_bundle() {
        // Same session_seed → same shim canvas seed literal.
        let a = IdentityBundle::from_chromium(131, 0xdead_beef);
        let b = IdentityBundle::from_chromium(131, 0xdead_beef);
        let ja = render_shim_from_bundle(&a);
        let jb = render_shim_from_bundle(&b);
        // Extract the `window.__crawlex_seed__ = <N> >>> 0` literal.
        let needle = "window.__crawlex_seed__ = (";
        let ea = ja.find(needle).expect("seed literal present");
        let eb = jb.find(needle).expect("seed literal present");
        assert_eq!(&ja[ea..ea + 80], &jb[eb..eb + 80]);
    }

    #[test]
    fn canvas_seed_differs_across_sessions() {
        let a = IdentityBundle::from_chromium(131, 1);
        let b = IdentityBundle::from_chromium(131, 2);
        let ja = render_shim_from_bundle(&a);
        let jb = render_shim_from_bundle(&b);
        // Different session seeds must land different integer literals
        // in the rendered shim.
        assert_ne!(
            ja.split("window.__crawlex_seed__ = (").nth(1).unwrap_or(""),
            jb.split("window.__crawlex_seed__ = (").nth(1).unwrap_or(""),
        );
    }

    #[test]
    fn no_date_now_in_seed_block() {
        // Guard against a regression where Date.now() sneaks back into
        // the canvas seed initialization — that would make the seed
        // non-deterministic across page loads within a session and
        // break FingerprintJS double-render equality.
        let b = IdentityBundle::from_chromium(131, 42);
        let js = render_shim_from_bundle(&b);
        let seed_block = js
            .split("typeof window.__crawlex_seed__ !== 'number'")
            .nth(1)
            .expect("seed block present");
        // Only inspect the next ~200 chars — enough to cover the init
        // expression without dragging in later canvas hooks.
        let window = &seed_block[..seed_block.len().min(400)];
        assert!(
            !window.contains("Date.now"),
            "seed init must not depend on Date.now(): {}",
            window
        );
    }

    #[test]
    fn permissions_query_handles_notifications_and_push() {
        let b = IdentityBundle::from_chromium(131, 7);
        let js = render_shim_from_bundle(&b);
        // Both leaky names should be covered.
        assert!(js.contains("leaky = { notifications: 1, push: 1 }"));
        // Coercion default → prompt must be present.
        assert!(js.contains("s === 'default' ? 'prompt' : s"));
        // Must still delegate other permission names to the original impl.
        assert!(js.contains("return orig(p);"));
    }

    #[test]
    fn notification_request_permission_is_coerced() {
        // S.4: headless Chrome returns 'denied' from
        // Notification.requestPermission without any user gesture, which
        // contradicts permissions.query (see section 3). The shim must
        // coerce 'denied' → 'default' and preserve both the callback and
        // the Promise signatures.
        let b = IdentityBundle::from_chromium(131, 11);
        let js = render_shim_from_bundle(&b);

        // Override must be installed.
        assert!(
            js.contains("Notification.requestPermission = wrapped"),
            "Notification.requestPermission override missing"
        );
        // Coercion rule present — denied → default.
        assert!(
            js.contains("(raw === 'denied') ? 'default' : raw"),
            "denied → default coercion missing"
        );
        // Both signatures preserved: callback invocation + Promise return.
        assert!(
            js.contains("callback(result)"),
            "callback invocation path missing"
        );
        assert!(
            js.contains("Promise.resolve(result)"),
            "Promise return path missing"
        );
        // toString Proxy registration: the wrapped function must be
        // handed to the section-13 WeakSet so `.toString()` looks native.
        let notif_section = js
            .split("13b. Notification.requestPermission")
            .nth(1)
            .expect("notification section present");
        // Section 13b is long enough that a 2000-byte window cut off the
        // `lateReg(wrapped)` registration line that lives ~40 lines after
        // the header. 5 KiB covers the whole block without bleeding into
        // section 14 (WebGPU) in any reasonable shim layout.
        let notif_window = &notif_section[..notif_section.len().min(5000)];
        assert!(
            notif_window.contains("__crawlex_reg_target__"),
            "wrapped function not registered with toString proxy"
        );
        assert!(
            notif_window.contains("lateReg(wrapped)"),
            "registrar not called on wrapped ref"
        );
    }

    #[test]
    fn battery_hidden_for_desktop_personas() {
        // Default Linux/Intel persona (Row 0) — desktop, EXPOSE_BATTERY=false.
        // Section 7 must NOT install the realistic curve, Section 17 must
        // delete the API.
        let b = IdentityBundle::from_chromium(131, 99);
        let js = render_shim_from_bundle(&b);
        // Placeholder substituted to literal `false`.
        assert!(
            js.contains("const EXPOSE_BATTERY = false"),
            "EXPOSE_BATTERY should be false on desktop persona"
        );
        // Section 17 must run the delete branch — gate variable also false.
        assert!(
            js.contains("const EXPOSE_BATTERY_S17 = false"),
            "Section 17 gate should be false on desktop"
        );
    }

    #[test]
    fn battery_curve_present_in_template() {
        // Even though desktop personas don't activate it at runtime, the
        // curve code must exist in the rendered shim — otherwise mobile
        // personas would receive a useless template. Sanity-check the
        // curve constants are intact post-substitution.
        let b = IdentityBundle::from_chromium(131, 7);
        let js = render_shim_from_bundle(&b);
        // The 22h discharge / 2h charge anchors.
        assert!(
            js.contains("0.85 - (0.85 - 0.20) * t"),
            "discharging formula 85→20% missing"
        );
        assert!(
            js.contains("0.20 + (0.85 - 0.20) * t"),
            "charging formula 20→85% missing"
        );
        // Hard clamps so level never exceeds 85% nor falls below 20%.
        assert!(
            js.contains("if (level > 0.85) level = 0.85"),
            "upper clamp at 85% missing — must never expose 100%"
        );
        assert!(
            js.contains("if (level < 0.20) level = 0.20"),
            "lower clamp at 20% missing"
        );
        // Local-time anchoring via Section 6 overridden getTimezoneOffset.
        assert!(
            js.contains("new Date().getTimezoneOffset()"),
            "timezone anchoring missing — curve must follow local midnight"
        );
    }

    #[test]
    #[allow(deprecated)] // intentionally exercises the legacy path
    fn legacy_profile_path_still_works() {
        let vars = ShimVars {
            profile: Profile::Chrome131Stable,
            locale: "en-US",
            languages_json: r#"["en-US","en"]"#,
            timezone: "UTC",
            tz_offset_min: 0,
            platform: "Linux x86_64",
        };
        let js = render_shim(&vars);
        assert!(!js.contains("{{"));
        assert!(js.contains("Linux"));
    }
}
