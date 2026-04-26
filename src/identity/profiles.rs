//! Persona catalog — OS × locale × timezone × screen × hardware × GPU × fonts.
//!
//! Single source-of-truth table for plausible desktop/mobile personas the
//! bundle builder can snap to. Every row is an internally coherent vector:
//! if you pick row `i`, every field on row `i` agrees with every other
//! (e.g. macOS row never pairs with Direct3D renderer or Liberation Mono
//! fonts). The validator in `validator.rs` enforces the coherence
//! invariants that hold *across* rows (OS ↔ UA, WebGL vendor ↔ platform,
//! etc); this file just provides plausible raw material.
//!
//! Pool membership references (what a detector expects to see):
//!   - Windows 10 en-US Intel UHD
//!   - Windows 10 pt-BR NVIDIA GTX 1060
//!   - macOS en-US Apple M1
//!   - Linux en-US Intel UHD (the historical default)
//!   - Mobile Android pt-BR Adreno
//!
//! Not every field is currently consumed by the shim — fields like `fonts`
//! and `audio_sample_rate` ride along so the follow-up work that wires
//! them through doesn't have to reopen this file.

/// Operating-system family a persona row belongs to. Mirrors the private
/// `Os` enum in `validator.rs`; kept public here because the profile
/// catalog is public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaOs {
    Windows,
    MacOs,
    Linux,
    AndroidMobile,
}

impl PersonaOs {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
            Self::AndroidMobile => "Android",
        }
    }
}

/// GPU vendor keyword. Mirrors the private `GpuVendor` in `validator.rs`;
/// public here so the catalog can be browsed from tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaGpu {
    Intel,
    Nvidia,
    Amd,
    Apple,
    AdrenoMobile,
}

impl PersonaGpu {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Intel => "intel",
            Self::Nvidia => "nvidia",
            Self::Amd => "amd",
            Self::Apple => "apple",
            Self::AdrenoMobile => "adreno",
        }
    }
}

/// Single coherent persona row.
#[derive(Debug, Clone)]
pub struct PersonaProfile {
    /// Short codename usable on the CLI (`--persona tux`) and in
    /// configuration files. Stable across releases — renaming a persona
    /// breaks downstream automation.
    pub name: &'static str,
    /// One-line human description shown by `crawlex stealth catalog list`
    /// and similar discovery surfaces.
    pub description: &'static str,
    pub os: PersonaOs,
    pub platform: &'static str,    // navigator.platform
    pub ua_platform: &'static str, // Sec-CH-UA-Platform (bare, no quotes)
    pub locale: &'static str,
    pub languages_json: &'static str,
    pub accept_language: &'static str,
    pub timezone: &'static str,
    pub tz_offset_min: i32,

    pub screen_w: u32,
    pub screen_h: u32,
    pub avail_screen_w: u32,
    pub avail_screen_h: u32,
    pub viewport_w: u32,
    pub viewport_h: u32,
    pub device_pixel_ratio: f32,
    pub color_depth: u32,
    /// Number of px the OS-owned scrollbar adds to innerWidth on this
    /// persona. Desktop Chrome on Win/Linux overlays ~15-17 px; macOS
    /// auto-hides so 0 is realistic. Mobile is 0.
    pub scrollbar_width: u32,

    pub device_memory: u32,
    pub hardware_concurrency: u32,
    /// jsHeapSizeLimit value the persona should expose. Desktop Chrome
    /// ships a stable 2 GiB; mobile caps at 512 MiB on many builds.
    pub heap_size_limit: u64,

    pub gpu: PersonaGpu,
    pub webgl_vendor: &'static str,
    pub webgl_renderer: &'static str,
    pub webgl_unmasked_vendor: &'static str,
    pub webgl_unmasked_renderer: &'static str,
    pub webgpu_adapter_description: &'static str,
    /// MAX_TEXTURE_SIZE value the GPU class reports. Intel integrated = 16384,
    /// discrete NVIDIA/AMD desktop = 16384-32768, Apple M1 = 16384, Adreno = 8192.
    pub max_texture_size: u32,
    /// MAX_VIEWPORT_DIMS tuple.
    pub max_viewport_dims: (u32, u32),

    /// AudioContext.sampleRate the persona should expose. Varies by OS +
    /// attached audio device class. 48000 is the modern default; 44100
    /// still appears on older desktop rigs.
    pub audio_sample_rate: u32,

    /// Font list coherent with the OS. Injected into the shim so
    /// font-based fingerprint probes don't see Liberation Mono on macOS.
    /// JSON array literal.
    pub fonts_json: &'static str,

    /// `navigator.mediaDevices.enumerateDevices()` surface counts. Real
    /// desktops commonly expose multiple microphones (built-in + headset +
    /// virtual/bluetooth) — our previous 1 mic / 1 cam / 1 speaker default
    /// is itself a tell. Camoufox ships 3/1/1 as default; we let each
    /// persona override so mobile can ship 1/1/1 and high-end workstations
    /// can ship higher counts without a new code path.
    pub media_mic_count: u8,
    pub media_cam_count: u8,
    pub media_speaker_count: u8,
}

/// Canonical catalog. Kept as a function (not a static) so the literal
/// lives in one place and the compiler enforces no-unused-fields via
/// construction.
pub fn catalog() -> &'static [PersonaProfile] {
    &PERSONA_CATALOG
}

/// Pick a persona by deterministic hash of the session seed. Callers that
/// already carry a 64-bit seed (IdentityBundle::canvas_audio_seed) can use
/// this to pin the persona for the session.
pub fn pick(seed: u64) -> &'static PersonaProfile {
    let idx = (seed as usize) % PERSONA_CATALOG.len();
    &PERSONA_CATALOG[idx]
}

/// Lookup the first catalog row with the given OS. Useful when the caller
/// wants a specific platform but doesn't care which sub-variant.
pub fn first_for(os: PersonaOs) -> Option<&'static PersonaProfile> {
    PERSONA_CATALOG.iter().find(|p| p.os == os)
}

/// Resolve a persona by its short name (`tux`, `office`, `gamer`, `atlas`,
/// `pixel`). Case-insensitive. Used by `--persona <name>` on the CLI and
/// by config-file consumers.
pub fn lookup_by_name(name: &str) -> Option<&'static PersonaProfile> {
    let lower = name.to_ascii_lowercase();
    PERSONA_CATALOG.iter().find(|p| p.name == lower)
}

/// Stable codenames as a flat list. The order matches `catalog()` indices,
/// so `name_for_index(idx)` gives the same string as
/// `catalog()[idx].name`. Useful for surfacing every persona in
/// `--help` output without iterating the full catalog struct.
pub fn names() -> &'static [&'static str] {
    &["tux", "office", "gamer", "atlas", "pixel"]
}

// Linux fonts: Liberation/DejaVu/Noto cluster — what every mainstream
// distro ships. Windows core set: Arial/Segoe UI/Calibri. macOS: Helvetica
// Neue / SF Pro / Menlo. Mobile Android: Roboto / Noto Sans.
const FONTS_LINUX: &str = r#"["DejaVu Sans","DejaVu Serif","DejaVu Sans Mono","Liberation Sans","Liberation Serif","Liberation Mono","Noto Sans","Noto Serif","Ubuntu","Ubuntu Mono","FreeSans","FreeMono","Droid Sans"]"#;
const FONTS_WINDOWS: &str = r#"["Arial","Arial Black","Calibri","Cambria","Candara","Comic Sans MS","Consolas","Courier New","Georgia","Impact","Lucida Console","Lucida Sans Unicode","Microsoft Sans Serif","Palatino Linotype","Segoe UI","Tahoma","Times New Roman","Trebuchet MS","Verdana","Webdings"]"#;
const FONTS_MACOS: &str = r#"["Apple Color Emoji","Helvetica","Helvetica Neue","Lucida Grande","Menlo","Monaco","Optima","SF Pro","SF Pro Display","SF Pro Text","SF Mono","Courier","Courier New","Geneva","Georgia","Times","Times New Roman"]"#;
const FONTS_ANDROID: &str = r#"["Roboto","Noto Sans","Noto Serif","Droid Sans","Droid Serif","Droid Sans Mono","sans-serif","serif","monospace"]"#;

const PERSONA_CATALOG: [PersonaProfile; 5] = [
    // Row 0: Linux en-US / Intel UHD — legacy default. Matches the
    // historical IdentityBundle::from_chromium output so existing fixtures
    // don't shift.
    PersonaProfile {
        name: "tux",
        description: "Linux desktop, Intel UHD 630, en-US, America/Sao_Paulo",
        os: PersonaOs::Linux,
        platform: "Linux x86_64",
        ua_platform: "Linux",
        locale: "en-US",
        languages_json: r#"["en-US","en"]"#,
        accept_language: "en-US,en;q=0.9",
        timezone: "America/Sao_Paulo",
        tz_offset_min: 180,
        screen_w: 1920,
        screen_h: 1080,
        avail_screen_w: 1920,
        avail_screen_h: 1050,
        viewport_w: 1920,
        viewport_h: 960,
        device_pixel_ratio: 1.0,
        color_depth: 24,
        scrollbar_width: 15,
        device_memory: 8,
        hardware_concurrency: 8,
        heap_size_limit: 2_147_483_648,
        gpu: PersonaGpu::Intel,
        webgl_vendor: "Google Inc. (Intel)",
        webgl_renderer: "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)",
        webgl_unmasked_vendor: "Google Inc. (Intel)",
        webgl_unmasked_renderer:
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)",
        webgpu_adapter_description:
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)",
        max_texture_size: 16384,
        max_viewport_dims: (32767, 32767),
        audio_sample_rate: 48000,
        fonts_json: FONTS_LINUX,
        media_mic_count: 2,
        media_cam_count: 1,
        media_speaker_count: 2,
    },
    // Row 1: Windows 10 en-US / Intel UHD 620 laptop.
    PersonaProfile {
        name: "office",
        description: "Windows 10 laptop, Intel UHD 620, en-US, America/New_York",
        os: PersonaOs::Windows,
        platform: "Win32",
        ua_platform: "Windows",
        locale: "en-US",
        languages_json: r#"["en-US","en"]"#,
        accept_language: "en-US,en;q=0.9",
        timezone: "America/New_York",
        tz_offset_min: 300,
        screen_w: 1920,
        screen_h: 1080,
        avail_screen_w: 1920,
        avail_screen_h: 1040,
        viewport_w: 1536,
        viewport_h: 864,
        device_pixel_ratio: 1.25,
        color_depth: 24,
        scrollbar_width: 17,
        device_memory: 8,
        hardware_concurrency: 8,
        heap_size_limit: 2_147_483_648,
        gpu: PersonaGpu::Intel,
        webgl_vendor: "Google Inc. (Intel)",
        webgl_renderer:
            "ANGLE (Intel, Intel(R) UHD Graphics 620 Direct3D11 vs_5_0 ps_5_0), or similar",
        webgl_unmasked_vendor: "Google Inc. (Intel)",
        webgl_unmasked_renderer:
            "ANGLE (Intel, Intel(R) UHD Graphics 620 Direct3D11 vs_5_0 ps_5_0), or similar",
        webgpu_adapter_description: "Intel(R) UHD Graphics 620",
        max_texture_size: 16384,
        max_viewport_dims: (32767, 32767),
        audio_sample_rate: 48000,
        fonts_json: FONTS_WINDOWS,
        media_mic_count: 3,
        media_cam_count: 1,
        media_speaker_count: 2,
    },
    // Row 2: Windows 10 pt-BR / NVIDIA GTX 1060 desktop.
    PersonaProfile {
        name: "gamer",
        description: "Windows 10 desktop, NVIDIA GTX 1060, pt-BR, America/Sao_Paulo",
        os: PersonaOs::Windows,
        platform: "Win32",
        ua_platform: "Windows",
        locale: "pt-BR",
        languages_json: r#"["pt-BR","pt","en-US","en"]"#,
        accept_language: "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7",
        timezone: "America/Sao_Paulo",
        tz_offset_min: 180,
        screen_w: 1920,
        screen_h: 1080,
        avail_screen_w: 1920,
        avail_screen_h: 1040,
        viewport_w: 1920,
        viewport_h: 969,
        device_pixel_ratio: 1.0,
        color_depth: 24,
        scrollbar_width: 17,
        device_memory: 8,
        hardware_concurrency: 12,
        heap_size_limit: 4_294_967_296,
        gpu: PersonaGpu::Nvidia,
        webgl_vendor: "Google Inc. (NVIDIA)",
        webgl_renderer:
            "ANGLE (NVIDIA, NVIDIA GeForce GTX 1060 Direct3D11 vs_5_0 ps_5_0), or similar",
        webgl_unmasked_vendor: "Google Inc. (NVIDIA)",
        webgl_unmasked_renderer:
            "ANGLE (NVIDIA, NVIDIA GeForce GTX 1060 Direct3D11 vs_5_0 ps_5_0), or similar",
        webgpu_adapter_description: "NVIDIA GeForce GTX 1060",
        max_texture_size: 32768,
        max_viewport_dims: (32768, 32768),
        audio_sample_rate: 48000,
        fonts_json: FONTS_WINDOWS,
        media_mic_count: 3,
        media_cam_count: 1,
        media_speaker_count: 2,
    },
    // Row 3: macOS en-US / Apple M1 laptop.
    PersonaProfile {
        name: "atlas",
        description: "macOS laptop, Apple M1 (8-core), en-US, America/Los_Angeles",
        os: PersonaOs::MacOs,
        platform: "MacIntel",
        ua_platform: "macOS",
        locale: "en-US",
        languages_json: r#"["en-US","en"]"#,
        accept_language: "en-US,en;q=0.9",
        timezone: "America/Los_Angeles",
        tz_offset_min: 480,
        screen_w: 1512,
        screen_h: 982,
        avail_screen_w: 1512,
        avail_screen_h: 944,
        viewport_w: 1440,
        viewport_h: 820,
        device_pixel_ratio: 2.0,
        color_depth: 30,
        scrollbar_width: 0,
        device_memory: 8,
        hardware_concurrency: 8,
        heap_size_limit: 2_147_483_648,
        gpu: PersonaGpu::Apple,
        webgl_vendor: "Google Inc. (Apple)",
        webgl_renderer: "ANGLE (Apple, Apple M1, OpenGL 4.1)",
        webgl_unmasked_vendor: "Google Inc. (Apple)",
        webgl_unmasked_renderer: "ANGLE (Apple, Apple M1, OpenGL 4.1)",
        webgpu_adapter_description: "Apple M1",
        max_texture_size: 16384,
        max_viewport_dims: (16384, 16384),
        audio_sample_rate: 48000,
        fonts_json: FONTS_MACOS,
        media_mic_count: 2,
        media_cam_count: 1,
        media_speaker_count: 1,
    },
    // Row 4: Android pt-BR / Adreno mobile. The bundle builder projects
    // this row as a mobile UA and emits `sec-ch-ua-mobile: ?1`.
    PersonaProfile {
        name: "pixel",
        description: "Android mobile (Pixel-class), Adreno 640, pt-BR, America/Sao_Paulo",
        os: PersonaOs::AndroidMobile,
        platform: "Linux armv8l",
        ua_platform: "Android",
        locale: "pt-BR",
        languages_json: r#"["pt-BR","pt","en-US","en"]"#,
        accept_language: "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7",
        timezone: "America/Sao_Paulo",
        tz_offset_min: 180,
        screen_w: 412,
        screen_h: 892,
        avail_screen_w: 412,
        avail_screen_h: 892,
        viewport_w: 412,
        viewport_h: 823,
        device_pixel_ratio: 2.625,
        color_depth: 24,
        scrollbar_width: 0,
        device_memory: 4,
        hardware_concurrency: 8,
        heap_size_limit: 536_870_912,
        gpu: PersonaGpu::AdrenoMobile,
        webgl_vendor: "Google Inc. (Qualcomm)",
        webgl_renderer: "ANGLE (Qualcomm, Adreno (TM) 640, OpenGL ES 3.2)",
        webgl_unmasked_vendor: "Google Inc. (Qualcomm)",
        webgl_unmasked_renderer: "ANGLE (Qualcomm, Adreno (TM) 640, OpenGL ES 3.2)",
        webgpu_adapter_description: "Adreno (TM) 640",
        max_texture_size: 8192,
        max_viewport_dims: (8192, 8192),
        audio_sample_rate: 48000,
        fonts_json: FONTS_ANDROID,
        media_mic_count: 1,
        media_cam_count: 2,
        media_speaker_count: 1,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_five_rows() {
        assert_eq!(catalog().len(), 5);
    }

    #[test]
    fn every_persona_has_unique_name() {
        let names: Vec<&str> = catalog().iter().map(|p| p.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            names.len(),
            sorted.len(),
            "duplicate persona name in catalog: {names:?}"
        );
        // Non-empty + lowercase + no whitespace (CLI-friendly).
        for n in &names {
            assert!(!n.is_empty(), "empty persona name");
            assert!(
                n.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "persona name `{n}` should be lowercase ASCII (with optional dashes)"
            );
        }
    }

    #[test]
    fn lookup_by_name_is_case_insensitive() {
        assert!(lookup_by_name("tux").is_some());
        assert!(lookup_by_name("TUX").is_some());
        assert!(lookup_by_name("Tux").is_some());
        assert!(lookup_by_name("nonexistent").is_none());
    }

    #[test]
    fn names_constant_matches_catalog_order() {
        let cat_names: Vec<&str> = catalog().iter().map(|p| p.name).collect();
        assert_eq!(cat_names, names());
    }

    #[test]
    fn pixel_persona_is_the_only_mobile() {
        let mobile_count = catalog()
            .iter()
            .filter(|p| p.os == PersonaOs::AndroidMobile)
            .count();
        assert_eq!(mobile_count, 1);
        let pixel = lookup_by_name("pixel").expect("pixel persona present");
        assert_eq!(pixel.os, PersonaOs::AndroidMobile);
    }

    #[test]
    fn each_row_has_avail_not_exceeding_screen() {
        for p in catalog() {
            assert!(p.avail_screen_w <= p.screen_w, "row os={:?}", p.os);
            assert!(p.avail_screen_h <= p.screen_h, "row os={:?}", p.os);
            assert!(p.viewport_w <= p.avail_screen_w, "row os={:?}", p.os);
            assert!(p.viewport_h <= p.avail_screen_h, "row os={:?}", p.os);
        }
    }

    #[test]
    fn gpu_and_os_coherent() {
        // Apple → macOS only, Adreno → Android only.
        for p in catalog() {
            match (p.gpu, p.os) {
                (PersonaGpu::Apple, PersonaOs::MacOs) => {}
                (PersonaGpu::Apple, _) => panic!("Apple GPU on non-mac row"),
                (PersonaGpu::AdrenoMobile, PersonaOs::AndroidMobile) => {}
                (PersonaGpu::AdrenoMobile, _) => panic!("Adreno on non-mobile row"),
                (PersonaGpu::Intel | PersonaGpu::Nvidia | PersonaGpu::Amd, _) => {}
            }
        }
    }

    #[test]
    fn fonts_match_os_keyword() {
        for p in catalog() {
            let f = p.fonts_json;
            match p.os {
                PersonaOs::Linux => {
                    assert!(
                        f.contains("DejaVu") || f.contains("Liberation"),
                        "linux fonts"
                    );
                    assert!(!f.contains("Segoe UI"), "windows font in linux row");
                }
                PersonaOs::Windows => {
                    assert!(
                        f.contains("Segoe UI") || f.contains("Calibri"),
                        "windows fonts"
                    );
                    assert!(!f.contains("DejaVu"), "linux font in windows row");
                    assert!(!f.contains("Helvetica Neue"), "mac font in windows row");
                }
                PersonaOs::MacOs => {
                    assert!(f.contains("Helvetica") || f.contains("SF Pro"), "mac fonts");
                    assert!(!f.contains("Segoe UI"), "windows font in mac row");
                    assert!(!f.contains("DejaVu"), "linux font in mac row");
                }
                PersonaOs::AndroidMobile => {
                    assert!(f.contains("Roboto"), "android needs Roboto");
                }
            }
        }
    }

    #[test]
    fn pick_is_deterministic() {
        let a = pick(0xdead_beef);
        let b = pick(0xdead_beef);
        assert_eq!(a.os, b.os);
        assert_eq!(a.gpu as u32, b.gpu as u32);
    }
}
