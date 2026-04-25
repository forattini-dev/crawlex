//! IdentityBundle data model.
//!
//! A bundle is the single source of truth for the "persona" a crawler
//! session is projecting. Every surface that can leak inconsistency (UA,
//! sec-ch-ua, UA-CH data, timezone, locale, viewport, WebGL class,
//! platform) draws from the same fields.
//!
//! Construct via [`IdentityBundle::from_chromium`] — this takes the
//! installed Chromium's detected version as input, rather than letting
//! callers pick a version that might not match the render backend.

use serde::{Deserialize, Serialize};

use crate::impersonate::Profile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityBundle {
    /// UUID or deterministic hash of the field set.
    pub id: String,
    /// Bundle schema version — bumped on breaking layout changes.
    pub version: u32,

    // Browser identity -------------------------------------------------
    pub ua: String,
    pub sec_ch_ua: String,
    pub ua_brands: String,            // JSON array serialized as string
    pub ua_full_version_list: String, // JSON array serialized as string
    pub ua_full_version: String,      // "131.0.6778.85"
    pub ua_major: u32,
    pub platform: String,    // "Linux x86_64"
    pub ua_platform: String, // `sec-ch-ua-platform` value: "\"Linux\""

    // Locale / region --------------------------------------------------
    pub locale: String,          // "en-US"
    pub languages_json: String,  // r#"["en-US","en"]"#
    pub accept_language: String, // "en-US,en;q=0.9"
    pub timezone: String,        // "America/Sao_Paulo"
    pub tz_offset_min: i32,

    // Device / display -------------------------------------------------
    pub viewport_w: u32,
    pub viewport_h: u32,
    pub screen_w: u32,
    pub screen_h: u32,
    pub avail_screen_w: u32,
    pub avail_screen_h: u32,
    pub device_pixel_ratio: f32,
    pub color_depth: u32,
    pub device_memory: u32,
    pub hardware_concurrency: u32,

    // WebGL ------------------------------------------------------------
    pub webgl_vendor: String,
    pub webgl_renderer: String,
    pub webgl_unmasked_vendor: String,
    pub webgl_unmasked_renderer: String,

    // WebGPU — must carry the same vendor keyword (intel/nvidia/amd/apple)
    // as `webgl_renderer`. A cross-API mismatch (WebGL Intel vs WebGPU
    // NVIDIA) is a one-line FP-Inconsistent flag. Injected into the
    // stealth shim via {{WEBGPU_ADAPTER_DESCRIPTION}}.
    #[serde(default = "default_webgpu_adapter_description")]
    pub webgpu_adapter_description: String,

    // Launch flags this bundle suggests for Chromium. Callers merge with
    // their own list.
    pub chromium_flags: Vec<String>,

    // Canvas / audio seed — derived per-session so noise is stable
    // within a session but distinct across sessions.
    pub canvas_audio_seed: u64,

    // Shim-injection scalars (wave1 coherence). Every field below lands
    // in a `{{PLACEHOLDER}}` inside stealth_shim.js; callers that build
    // bundles through `from_chromium` get sensible Linux/Intel defaults
    // via `serde(default = ...)`, so fixtures and older persisted bundles
    // keep loading without a migration.
    /// Width (px) the OS scrollbar adds inside `window.innerWidth` —
    /// drives the shim's `outerWidth - innerWidth` reconciliation so the
    /// browser looks like a real framed window (15-17 px on Win/Linux, 0
    /// on macOS/mobile overlay scrollbars). See leak #1.
    #[serde(default = "default_scrollbar_width")]
    pub scrollbar_width: u32,
    /// `performance.memory.jsHeapSizeLimit` the shim should expose.
    /// Desktop Chrome builds pin this to 2 GiB; low-memory VPS hosts leak
    /// a true sub-GiB value that instantly flags the runtime. Leak #2.
    #[serde(default = "default_heap_size_limit")]
    pub heap_size_limit: u64,
    /// WebGL `MAX_TEXTURE_SIZE`. Integrated Intel = 16384; discrete
    /// NVIDIA/AMD desktop = 16384-32768; Apple M1 = 16384; Adreno = 8192.
    /// Part of the per-GPU WebGL table (A.3).
    #[serde(default = "default_max_texture_size")]
    pub max_texture_size: u32,
    /// WebGL `MAX_VIEWPORT_DIMS` tuple `[w,h]`. Must agree with the GPU
    /// class — a 32767x32767 cap on an Apple M1 is an instant cross-
    /// vendor tell. Serialized as two fields so older persisted bundles
    /// (pre-wave1) slot in via the default.
    #[serde(default = "default_max_viewport_w")]
    pub max_viewport_w: u32,
    #[serde(default = "default_max_viewport_h")]
    pub max_viewport_h: u32,
    /// AudioContext.sampleRate the persona should expose. 48000 is the
    /// modern default; 44100 and 22050 appear on older desktop rigs and
    /// some Bluetooth-only Android builds. Leak #44.
    #[serde(default = "default_audio_sample_rate")]
    pub audio_sample_rate: u32,
    /// Font list (JSON array literal) coherent with the OS. Shim injects
    /// it into document-font probes so Liberation Mono never shows up on
    /// macOS. Kept as a ready-to-paste JS array string.
    #[serde(default = "default_fonts_json")]
    pub fonts_json: String,

    /// `navigator.mediaDevices.enumerateDevices()` surface counts. Camoufox
    /// research: a single mic / single speaker response is itself a
    /// tell — real laptops expose built-in + headset + virtual. These
    /// drive the §23 shim. See leak #45.
    #[serde(default = "default_media_mic_count")]
    pub media_mic_count: u8,
    #[serde(default = "default_media_cam_count")]
    pub media_cam_count: u8,
    #[serde(default = "default_media_speaker_count")]
    pub media_speaker_count: u8,
}

impl IdentityBundle {
    /// Build a bundle coherent with the installed Chromium major version.
    pub fn from_chromium(major: u32, session_seed: u64) -> Self {
        let profile = Profile::from_detected_major(major);
        let ua = profile.user_agent().to_string();
        let sec_ch_ua = profile.sec_ch_ua().to_string();
        let ua_brands = profile.ua_brands_json().to_string();
        let ua_full_version_list = profile.fullversion_brands_json().to_string();
        let ua_full_version = profile.ua_full_version().to_string();
        let ua_major = profile.major_version();
        let id = format!("ib-{}-{}", ua_major, session_seed);
        Self {
            id,
            version: 1,
            ua,
            sec_ch_ua,
            ua_brands,
            ua_full_version_list,
            ua_full_version,
            ua_major,
            platform: "Linux x86_64".into(),
            ua_platform: "\"Linux\"".into(),
            locale: "en-US".into(),
            languages_json: r#"["en-US","en"]"#.into(),
            accept_language: "en-US,en;q=0.9".into(),
            timezone: "America/Sao_Paulo".into(),
            tz_offset_min: 180,
            // Real desktop Chrome: viewport < avail (address bar + tabs
            // eat ~120px vertically, ~0px horizontally). avail < screen on
            // most Linux WMs (taskbar/dock takes ~30px). Screen-height ==
            // viewport-height is impossible in a real window and a
            // classic FP-Inconsistent tell.
            viewport_w: 1920,
            viewport_h: 960,
            screen_w: 1920,
            screen_h: 1080,
            avail_screen_w: 1920,
            avail_screen_h: 1050,
            device_pixel_ratio: 1.0,
            color_depth: 24,
            device_memory: 8,
            hardware_concurrency: 8,
            // Linux Chrome with ANGLE over OpenGL — Mesa Intel is the
            // plurality renderer on Linux desktop and keeps the string
            // coherent with an X11 UA. Direct3D/Metal on Linux is an
            // instant FP-Inconsistent flag.
            webgl_vendor: "Google Inc. (Intel)".into(),
            webgl_renderer: "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)"
                .into(),
            webgl_unmasked_vendor: "Google Inc. (Intel)".into(),
            webgl_unmasked_renderer:
                "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)".into(),
            // Keep WebGPU vendor coherent with webgl_renderer (Intel here).
            webgpu_adapter_description: default_webgpu_adapter_description(),
            chromium_flags: default_chromium_flags(&profile),
            canvas_audio_seed: session_seed,
            scrollbar_width: default_scrollbar_width(),
            heap_size_limit: default_heap_size_limit(),
            max_texture_size: default_max_texture_size(),
            max_viewport_w: default_max_viewport_w(),
            max_viewport_h: default_max_viewport_h(),
            audio_sample_rate: default_audio_sample_rate(),
            fonts_json: default_fonts_json(),
            media_mic_count: default_media_mic_count(),
            media_cam_count: default_media_cam_count(),
            media_speaker_count: default_media_speaker_count(),
        }
    }

    /// Build a bundle snapped to a `PersonaProfile` from the catalog.
    /// Callers that want the catalog-driven OS/locale/GPU coherence pick
    /// the row themselves (via [`crate::identity::profiles::pick`]) and
    /// hand it in; everything else flows from the row + Chromium major.
    pub fn from_persona(
        persona: &crate::identity::profiles::PersonaProfile,
        chromium_major: u32,
        session_seed: u64,
    ) -> Self {
        let profile = Profile::from_detected_major(chromium_major);
        let ua_major = profile.major_version();
        let os_token = match persona.os {
            crate::identity::profiles::PersonaOs::Windows => "(Windows NT 10.0; Win64; x64)",
            crate::identity::profiles::PersonaOs::MacOs => "(Macintosh; Intel Mac OS X 10_15_7)",
            crate::identity::profiles::PersonaOs::Linux => "(X11; Linux x86_64)",
            crate::identity::profiles::PersonaOs::AndroidMobile => "(Linux; Android 14; Pixel 7)",
        };
        let mobile = matches!(
            persona.os,
            crate::identity::profiles::PersonaOs::AndroidMobile
        );
        let ua = format!(
            "Mozilla/5.0 {os_token} AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{ua_major}.0.0.0 {}Safari/537.36",
            if mobile { "Mobile " } else { "" }
        );
        let id = format!("ib-{}-{}", ua_major, session_seed);
        Self {
            id,
            version: 1,
            ua,
            sec_ch_ua: profile.sec_ch_ua().to_string(),
            ua_brands: profile.ua_brands_json().to_string(),
            ua_full_version_list: profile.fullversion_brands_json().to_string(),
            ua_full_version: profile.ua_full_version().to_string(),
            ua_major,
            platform: persona.platform.to_string(),
            ua_platform: format!("\"{}\"", persona.ua_platform),
            locale: persona.locale.to_string(),
            languages_json: persona.languages_json.to_string(),
            accept_language: persona.accept_language.to_string(),
            timezone: persona.timezone.to_string(),
            tz_offset_min: persona.tz_offset_min,
            viewport_w: persona.viewport_w,
            viewport_h: persona.viewport_h,
            screen_w: persona.screen_w,
            screen_h: persona.screen_h,
            avail_screen_w: persona.avail_screen_w,
            avail_screen_h: persona.avail_screen_h,
            device_pixel_ratio: persona.device_pixel_ratio,
            color_depth: persona.color_depth,
            device_memory: persona.device_memory,
            hardware_concurrency: persona.hardware_concurrency,
            webgl_vendor: persona.webgl_vendor.to_string(),
            webgl_renderer: persona.webgl_renderer.to_string(),
            webgl_unmasked_vendor: persona.webgl_unmasked_vendor.to_string(),
            webgl_unmasked_renderer: persona.webgl_unmasked_renderer.to_string(),
            webgpu_adapter_description: persona.webgpu_adapter_description.to_string(),
            chromium_flags: default_chromium_flags(&profile),
            canvas_audio_seed: session_seed,
            scrollbar_width: persona.scrollbar_width,
            heap_size_limit: persona.heap_size_limit,
            max_texture_size: persona.max_texture_size,
            max_viewport_w: persona.max_viewport_dims.0,
            max_viewport_h: persona.max_viewport_dims.1,
            audio_sample_rate: persona.audio_sample_rate,
            fonts_json: persona.fonts_json.to_string(),
            media_mic_count: persona.media_mic_count,
            media_cam_count: persona.media_cam_count,
            media_speaker_count: persona.media_speaker_count,
        }
    }

    pub fn profile(&self) -> Profile {
        Profile::from_detected_major(self.ua_major)
    }

    /// Build the effective crawl identity from config-level knobs. This is
    /// intentionally shared by HTTP spoofing and the render backend so UA,
    /// UA-CH, locale, timezone, viewport and GPU fields come from one
    /// bundle instead of parallel partial overrides.
    pub fn from_profile_with_overrides(
        profile: Profile,
        identity_preset: Option<u8>,
        locale: Option<&str>,
        timezone: Option<&str>,
        user_agent_override: Option<&str>,
        session_seed: u64,
    ) -> std::result::Result<Self, String> {
        let mut bundle = match identity_preset {
            Some(idx) => {
                let catalog = crate::identity::profiles::catalog();
                let persona = catalog.get(idx as usize).ok_or_else(|| {
                    format!(
                        "identity_preset index {idx} is out of range; catalog has {} entries",
                        catalog.len()
                    )
                })?;
                Self::from_persona(persona, profile.major_version(), session_seed)
            }
            None => Self::from_chromium(profile.major_version(), session_seed),
        };
        if let Some(locale) = locale {
            bundle.apply_locale(locale);
        }
        if let Some(timezone) = timezone {
            bundle.apply_timezone(timezone);
        }
        if let Some(ua) = user_agent_override {
            bundle.apply_user_agent_override(ua)?;
        }
        Ok(bundle)
    }

    pub fn apply_locale(&mut self, locale: &str) {
        let locale = locale.trim();
        if locale.is_empty() {
            return;
        }
        self.locale = locale.to_string();
        self.languages_json = languages_json_for_locale(locale);
        self.accept_language = accept_language_for_locale(locale);
    }

    pub fn apply_timezone(&mut self, timezone: &str) {
        let timezone = timezone.trim();
        if timezone.is_empty() {
            return;
        }
        self.timezone = timezone.to_string();
        self.tz_offset_min = guess_tz_offset_min(timezone);
    }

    /// Override only when the UA can be projected coherently onto the
    /// bundle's existing platform. A Windows UA on a Linux persona is more
    /// detectable than rejecting the config at startup.
    pub fn apply_user_agent_override(&mut self, ua: &str) -> std::result::Result<(), String> {
        validate_header_value("user_agent_override", ua)?;
        let major = chrome_major_from_ua(ua).ok_or_else(|| {
            "user_agent_override must include a Chrome/<major> token so UA-CH stays coherent"
                .to_string()
        })?;
        let profile = Profile::from_detected_major(major);
        if profile.major_version() != major {
            return Err(format!(
                "user_agent_override Chrome/{major} is unsupported by the active TLS/header profile table"
            ));
        }
        let ua_platform = ua_ch_platform_from_ua(ua).ok_or_else(|| {
            "user_agent_override must contain a recognizable platform token".to_string()
        })?;
        if ua_platform != self.ua_platform {
            return Err(format!(
                "user_agent_override platform {ua_platform} does not match active identity platform {}",
                self.ua_platform
            ));
        }

        self.ua = ua.to_string();
        self.ua_major = major;
        self.ua_full_version = format!("{major}.0.0.0");
        self.sec_ch_ua = sec_ch_ua_for_major(major);
        self.ua_brands = ua_brands_json_for_major(major);
        self.ua_full_version_list = ua_full_version_list_json(&self.ua_full_version);
        Ok(())
    }

    pub fn is_mobile(&self) -> bool {
        self.ua.contains(" Mobile ")
            || self.ua.contains("Mobile Safari")
            || self
                .ua_platform
                .trim_matches('"')
                .eq_ignore_ascii_case("Android")
    }

    pub fn sec_ch_ua_mobile(&self) -> &'static str {
        if self.is_mobile() {
            "?1"
        } else {
            "?0"
        }
    }

    pub fn chrome_lang_arg(&self) -> String {
        if let Ok(values) = serde_json::from_str::<Vec<String>>(&self.languages_json) {
            if !values.is_empty() {
                return values.join(",");
            }
        }
        let langs: Vec<&str> = self
            .accept_language
            .split(',')
            .filter_map(|part| part.split(';').next().map(str::trim))
            .filter(|part| !part.is_empty())
            .collect();
        if langs.is_empty() {
            self.locale.clone()
        } else {
            langs.join(",")
        }
    }
}

fn validate_header_value(name: &str, value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.len() > 512
        || value
            .as_bytes()
            .iter()
            .any(|&b| b == b'\r' || b == b'\n' || b < 0x20 || b == 0x7f)
    {
        return Err(format!("invalid {name}: {value:?}"));
    }
    Ok(())
}

fn chrome_major_from_ua(ua: &str) -> Option<u32> {
    let marker = "Chrome/";
    let start = ua.find(marker)? + marker.len();
    let major = ua[start..].split('.').next()?;
    major.parse().ok()
}

fn ua_ch_platform_from_ua(ua: &str) -> Option<String> {
    if ua.contains("Windows NT") {
        Some("\"Windows\"".into())
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        Some("\"macOS\"".into())
    } else if ua.contains("Android") {
        Some("\"Android\"".into())
    } else if ua.contains("Linux") || ua.contains("X11") {
        Some("\"Linux\"".into())
    } else {
        None
    }
}

fn sec_ch_ua_for_major(major: u32) -> String {
    format!("\"Google Chrome\";v=\"{major}\", \"Chromium\";v=\"{major}\", \"Not_A Brand\";v=\"24\"")
}

fn ua_brands_json_for_major(major: u32) -> String {
    format!(
        r#"[{{"brand":"Google Chrome","version":"{major}"}},{{"brand":"Chromium","version":"{major}"}},{{"brand":"Not_A Brand","version":"24"}}]"#
    )
}

fn ua_full_version_list_json(full: &str) -> String {
    format!(
        r#"[{{"brand":"Google Chrome","version":"{full}"}},{{"brand":"Chromium","version":"{full}"}},{{"brand":"Not_A Brand","version":"24.0.0.0"}}]"#
    )
}

fn languages_json_for_locale(locale: &str) -> String {
    let primary = locale
        .split(['-', '_'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(locale);
    if primary.eq_ignore_ascii_case(locale) {
        format!(r#"["{locale}"]"#)
    } else if primary.eq_ignore_ascii_case("en") {
        r#"["en-US","en"]"#.into()
    } else {
        format!(r#"["{locale}","{primary}","en"]"#)
    }
}

fn accept_language_for_locale(locale: &str) -> String {
    let primary = locale
        .split(['-', '_'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(locale);
    if primary.eq_ignore_ascii_case(locale) {
        locale.to_string()
    } else if primary.eq_ignore_ascii_case("en") {
        "en-US,en;q=0.9".into()
    } else {
        format!("{locale},{primary};q=0.9,en;q=0.8")
    }
}

fn guess_tz_offset_min(tz: &str) -> i32 {
    match tz {
        "UTC" | "Etc/UTC" => 0,
        "America/Sao_Paulo" | "America/Buenos_Aires" | "America/Santiago" => 180,
        "America/New_York" => 300,
        "America/Chicago" => 360,
        "America/Denver" => 420,
        "America/Los_Angeles" => 480,
        "Europe/London" | "Europe/Lisbon" => 0,
        "Europe/Berlin" | "Europe/Paris" | "Europe/Madrid" | "Europe/Rome" => -60,
        "Europe/Moscow" => -180,
        "Asia/Tokyo" | "Asia/Seoul" => -540,
        "Asia/Shanghai" | "Asia/Taipei" | "Asia/Singapore" | "Asia/Hong_Kong" => -480,
        "Asia/Kolkata" => -330,
        "Australia/Sydney" => -600,
        _ => 180,
    }
}

/// WebGPU adapter description shipped with the default Intel-on-Linux
/// persona. Must mention the same vendor keyword as `webgl_renderer` —
/// the validator enforces this invariant.
fn default_scrollbar_width() -> u32 {
    15
}
fn default_heap_size_limit() -> u64 {
    2_147_483_648
}
fn default_max_texture_size() -> u32 {
    16384
}
fn default_max_viewport_w() -> u32 {
    32767
}
fn default_max_viewport_h() -> u32 {
    32767
}
fn default_audio_sample_rate() -> u32 {
    48000
}
fn default_fonts_json() -> String {
    // Linux cluster matches the default_from_chromium persona. Catalog
    // callers overwrite this through `from_persona`.
    r#"["DejaVu Sans","DejaVu Serif","DejaVu Sans Mono","Liberation Sans","Liberation Serif","Liberation Mono","Noto Sans","Noto Serif"]"#.into()
}
fn default_media_mic_count() -> u8 {
    2
}
fn default_media_cam_count() -> u8 {
    1
}
fn default_media_speaker_count() -> u8 {
    2
}

fn default_webgpu_adapter_description() -> String {
    "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)".into()
}

fn default_chromium_flags(profile: &Profile) -> Vec<String> {
    vec![
        "--disable-dev-shm-usage".into(),
        "--disable-blink-features=AutomationControlled".into(),
        "--disable-features=IsolateOrigins,site-per-process,Translate,MediaRouter".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        format!("--user-agent={}", profile.user_agent()),
        "--window-size=1920,1080".into(),
        "--lang=en-US,en".into(),
    ]
}

/// Per-session binding between a crawl session and an identity bundle.
/// Persisted (phase 5) so a resumed run keeps presenting the same persona.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIdentity {
    pub session_id: String,
    pub bundle_id: String,
    pub created_at_unix: i64,
    pub last_used_unix: i64,
    /// Antibot contamination state, updated as challenges are observed.
    /// Default `Clean`; transitions monotonically via
    /// [`crate::antibot::SessionState::after_challenge`].
    #[serde(default)]
    pub state: crate::antibot::SessionState,
}
