//! Startup-time coherence check on an `IdentityBundle`.
//!
//! Mismatches between UA ↔ sec-ch-ua ↔ major, locale ↔ languages, or
//! timezone ↔ offset are classic "bot-builder forgot to align one field"
//! tells. We refuse to start a crawl with an inconsistent bundle.
//!
//! Cross-layer checks enforce FP-Inconsistent (arxiv.org/abs/2406.07647)
//! mitigation: every attribute that can be correlated by a detector is
//! cross-verified here. A single inconsistency = reject bundle.

use crate::identity::IdentityBundle;

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("UA string does not contain major version {major}: {ua}")]
    UaMajorMismatch { major: u32, ua: String },
    #[error("sec-ch-ua does not reference major version {major}: {sec}")]
    SecChUaMissingMajor { major: u32, sec: String },
    #[error("locale {locale} absent from languages list {langs}")]
    LocaleNotInLanguages { locale: String, langs: String },
    #[error("accept-language {al} does not start with locale {locale}")]
    AcceptLanguageMismatch { locale: String, al: String },
    #[error("timezone {tz} has offset_min {declared}, expected roughly {guessed}")]
    TimezoneOffsetMismatch {
        tz: String,
        declared: i32,
        guessed: i32,
    },
    #[error("avail_screen_{axis} ({avail}) > screen_{axis} ({screen})")]
    AvailExceedsScreen {
        axis: &'static str,
        avail: u32,
        screen: u32,
    },
    #[error("viewport_{axis} ({view}) > screen_{axis} ({screen})")]
    ViewportExceedsScreen {
        axis: &'static str,
        view: u32,
        screen: u32,
    },
    #[error("UA platform token {ua_token:?} does not match platform {platform:?}")]
    UaPlatformMismatch { ua_token: String, platform: String },
    #[error("sec-ch-ua-platform {ch:?} does not match platform {platform:?}")]
    ChPlatformMismatch { ch: String, platform: String },
    #[error("WebGL renderer {renderer:?} is inconsistent with platform {platform:?}")]
    WebglPlatformMismatch { renderer: String, platform: String },
    #[error(
        "WebGL renderer {renderer:?} must mention a recognised GPU vendor keyword \
         (intel/nvidia/amd/apple)"
    )]
    WebglVendorUnrecognised { renderer: String },
    #[error(
        "WebGL unmasked vendor keyword {unmasked:?} must equal masked keyword {masked:?} \
         (renderers: {renderer:?} vs {unmasked_renderer:?})"
    )]
    WebglVendorKeywordMismatch {
        masked: String,
        unmasked: String,
        renderer: String,
        unmasked_renderer: String,
    },
    #[error("WebGL vendor keyword {keyword:?} is not valid for platform {platform:?}")]
    WebglVendorPlatformMismatch { keyword: String, platform: String },
    #[error(
        "WebGPU adapter description {description:?} must mention the same GPU vendor \
         keyword {keyword:?} as webgl_renderer {renderer:?}"
    )]
    WebgpuVendorMismatch {
        description: String,
        keyword: String,
        renderer: String,
    },
    #[error("ua_brands_json invalid JSON: {err}")]
    UaBrandsInvalidJson { err: String },
    #[error("ua_brands_json missing major version {major}")]
    UaBrandsMissingMajor { major: u32 },
    #[error("ua_full_version_list invalid JSON: {err}")]
    UaFullVersionListInvalidJson { err: String },
    #[error("ua_full_version {full} does not start with major {major}")]
    UaFullVersionMajorMismatch { full: String, major: u32 },
    #[error("languages_json invalid JSON: {err}")]
    LanguagesInvalidJson { err: String },
    #[error("device_memory {0} not in the standard Chrome bucket {{0.25,0.5,1,2,4,8}}")]
    DeviceMemoryInvalid(u32),
    #[error("hardware_concurrency {0} outside plausible desktop range [2,32]")]
    HardwareConcurrencyInvalid(u32),
    #[error("color_depth {0} not in {{24,30,48}}")]
    ColorDepthInvalid(u32),
    #[error("device_pixel_ratio {0} outside [1.0, 3.0]")]
    DprInvalid(f32),
    #[error("viewport_{axis} ({view}) > avail_screen_{axis} ({avail})")]
    ViewportExceedsAvail {
        axis: &'static str,
        view: u32,
        avail: u32,
    },
    #[error("TLS profile major {tls_major} disagrees with bundle.ua_major {ua_major}")]
    TlsProfileMajorMismatch { tls_major: u32, ua_major: u32 },
    #[error("scrollbar_width {0} outside plausible desktop/mobile range [0,24]")]
    ScrollbarWidthInvalid(u32),
    #[error("heap_size_limit {0} below 256 MiB (mobile floor)")]
    HeapSizeTooSmall(u64),
    #[error("heap_size_limit {0} above 8 GiB (not a Chrome value)")]
    HeapSizeTooLarge(u64),
    #[error("max_texture_size {0} not in plausible GPU range [4096,32768]")]
    MaxTextureSizeInvalid(u32),
    #[error(
        "max_viewport_dims ({w},{h}) incoherent with max_texture_size {mts}; \
         viewport dim must be >= max_texture_size on real drivers"
    )]
    MaxViewportDimsIncoherent { w: u32, h: u32, mts: u32 },
    #[error("audio_sample_rate {0} not in standard Chrome set {{22050,44100,48000,96000}}")]
    AudioSampleRateInvalid(u32),
    #[error("fonts_json invalid JSON: {err}")]
    FontsInvalidJson { err: String },
    #[error("fonts list contains {font:?} which is incoherent with platform {platform:?}")]
    FontsPlatformMismatch { font: String, platform: String },
}

pub struct IdentityValidator;

impl IdentityValidator {
    pub fn check(b: &IdentityBundle) -> Result<(), ValidationError> {
        // UA must literally contain the declared major version.
        let major_str = b.ua_major.to_string();
        if !b.ua.contains(&major_str) {
            return Err(ValidationError::UaMajorMismatch {
                major: b.ua_major,
                ua: b.ua.clone(),
            });
        }
        if !b.sec_ch_ua.contains(&format!("v=\"{}\"", b.ua_major)) {
            return Err(ValidationError::SecChUaMissingMajor {
                major: b.ua_major,
                sec: b.sec_ch_ua.clone(),
            });
        }

        // ua_full_version must start with the major number, dot-separated.
        if !b.ua_full_version.starts_with(&format!("{}.", b.ua_major)) {
            return Err(ValidationError::UaFullVersionMajorMismatch {
                full: b.ua_full_version.clone(),
                major: b.ua_major,
            });
        }

        // ua_brands JSON parses and references the declared major.
        let ua_brands: serde_json::Value = serde_json::from_str(&b.ua_brands)
            .map_err(|e| ValidationError::UaBrandsInvalidJson { err: e.to_string() })?;
        let major_as_str = b.ua_major.to_string();
        let has_major = ua_brands
            .as_array()
            .map(|arr| {
                arr.iter().any(|e| {
                    e.get("version")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| s == major_as_str)
                })
            })
            .unwrap_or(false);
        if !has_major {
            return Err(ValidationError::UaBrandsMissingMajor { major: b.ua_major });
        }

        // ua_full_version_list JSON parses.
        serde_json::from_str::<serde_json::Value>(&b.ua_full_version_list)
            .map_err(|e| ValidationError::UaFullVersionListInvalidJson { err: e.to_string() })?;

        // languages_json parses.
        let langs: serde_json::Value = serde_json::from_str(&b.languages_json)
            .map_err(|e| ValidationError::LanguagesInvalidJson { err: e.to_string() })?;

        // Locale should appear in the languages list.
        let locale_in_langs = langs
            .as_array()
            .map(|arr| {
                arr.iter()
                    .any(|v| v.as_str().is_some_and(|s| s == b.locale))
            })
            .unwrap_or(false);
        if !locale_in_langs {
            return Err(ValidationError::LocaleNotInLanguages {
                locale: b.locale.clone(),
                langs: b.languages_json.clone(),
            });
        }

        // accept-language header should lead with the locale.
        if !b.accept_language.starts_with(&b.locale) {
            return Err(ValidationError::AcceptLanguageMismatch {
                locale: b.locale.clone(),
                al: b.accept_language.clone(),
            });
        }

        // Timezone offset sanity — avoid off-by-an-hour. Unknown
        // timezones return None here: we can't guess, so we can't
        // reject. A bundle that picks an obscure tz with a plausible
        // offset should not be flagged just because our table is
        // short.
        if let Some(guessed) = guess_tz_offset_min(&b.timezone) {
            if (guessed - b.tz_offset_min).abs() > 60 {
                return Err(ValidationError::TimezoneOffsetMismatch {
                    tz: b.timezone.clone(),
                    declared: b.tz_offset_min,
                    guessed,
                });
            }
        }

        // Platform ↔ UA token ↔ sec-ch-ua-platform coherence.
        let ua_os = detect_ua_os(&b.ua);
        let declared_os = detect_platform_os(&b.platform);
        if ua_os != declared_os {
            return Err(ValidationError::UaPlatformMismatch {
                ua_token: ua_os.as_str().into(),
                platform: b.platform.clone(),
            });
        }
        let ch_os = detect_ch_platform_os(&b.ua_platform);
        if ch_os != declared_os {
            return Err(ValidationError::ChPlatformMismatch {
                ch: b.ua_platform.clone(),
                platform: b.platform.clone(),
            });
        }

        // WebGL renderer family must be consistent with the OS.
        // Linux uses GL/Vulkan, not Direct3D/Metal; macOS uses Metal, not D3D;
        // Windows uses ANGLE/Direct3D. A mismatch is a one-line giveaway.
        check_webgl_platform(&b.webgl_renderer, declared_os)?;
        check_webgl_platform(&b.webgl_unmasked_renderer, declared_os)?;

        // WebGL vendor keyword (intel/nvidia/amd/apple) must appear in
        // `webgl_renderer` and match the unmasked pair. Detectors correlate
        // the vendor string across masked/unmasked and across WebGL/WebGPU;
        // any skew is a free FP-Inconsistent flag.
        let masked_kw = detect_gpu_vendor(&b.webgl_renderer).ok_or_else(|| {
            ValidationError::WebglVendorUnrecognised {
                renderer: b.webgl_renderer.clone(),
            }
        })?;
        let unmasked_kw = detect_gpu_vendor(&b.webgl_unmasked_renderer).ok_or_else(|| {
            ValidationError::WebglVendorUnrecognised {
                renderer: b.webgl_unmasked_renderer.clone(),
            }
        })?;
        if masked_kw != unmasked_kw {
            tracing::warn!(
                masked = masked_kw.as_str(),
                unmasked = unmasked_kw.as_str(),
                "webgl masked vs unmasked GPU vendor keyword disagree",
            );
            return Err(ValidationError::WebglVendorKeywordMismatch {
                masked: masked_kw.as_str().into(),
                unmasked: unmasked_kw.as_str().into(),
                renderer: b.webgl_renderer.clone(),
                unmasked_renderer: b.webgl_unmasked_renderer.clone(),
            });
        }
        // Apple silicon only appears on macOS. NVIDIA/AMD/Intel can appear
        // on Linux or Windows, but Apple on Linux/Windows is an instant tell.
        if !masked_kw.is_valid_on(declared_os) {
            return Err(ValidationError::WebglVendorPlatformMismatch {
                keyword: masked_kw.as_str().into(),
                platform: b.platform.clone(),
            });
        }

        // WebGPU adapter description must share the same vendor keyword as
        // WebGL — otherwise "WebGL says Intel, WebGPU says NVIDIA" trips
        // cross-API checks in fingerprinters like CreepJS.
        let webgpu_kw = detect_gpu_vendor(&b.webgpu_adapter_description);
        if webgpu_kw != Some(masked_kw) {
            return Err(ValidationError::WebgpuVendorMismatch {
                description: b.webgpu_adapter_description.clone(),
                keyword: masked_kw.as_str().into(),
                renderer: b.webgl_renderer.clone(),
            });
        }

        // Screen geometry invariants: avail ≤ screen, viewport ≤ avail.
        if b.avail_screen_w > b.screen_w {
            return Err(ValidationError::AvailExceedsScreen {
                axis: "w",
                avail: b.avail_screen_w,
                screen: b.screen_w,
            });
        }
        if b.avail_screen_h > b.screen_h {
            return Err(ValidationError::AvailExceedsScreen {
                axis: "h",
                avail: b.avail_screen_h,
                screen: b.screen_h,
            });
        }
        if b.viewport_w > b.screen_w {
            return Err(ValidationError::ViewportExceedsScreen {
                axis: "w",
                view: b.viewport_w,
                screen: b.screen_w,
            });
        }
        if b.viewport_h > b.screen_h {
            return Err(ValidationError::ViewportExceedsScreen {
                axis: "h",
                view: b.viewport_h,
                screen: b.screen_h,
            });
        }
        // Viewport cannot exceed available screen (browser window fits inside
        // the work area once OS chrome is subtracted).
        if b.viewport_w > b.avail_screen_w {
            return Err(ValidationError::ViewportExceedsAvail {
                axis: "w",
                view: b.viewport_w,
                avail: b.avail_screen_w,
            });
        }
        if b.viewport_h > b.avail_screen_h {
            return Err(ValidationError::ViewportExceedsAvail {
                axis: "h",
                view: b.viewport_h,
                avail: b.avail_screen_h,
            });
        }

        // Device capability sanity — Chrome clamps navigator.deviceMemory to
        // the set {0.25,0.5,1,2,4,8} (stored as u32 with 0 meaning 0.25/0.5
        // is not representable here; we accept the integer subset).
        if !matches!(b.device_memory, 1 | 2 | 4 | 8) {
            return Err(ValidationError::DeviceMemoryInvalid(b.device_memory));
        }
        if !(2..=32).contains(&b.hardware_concurrency) {
            return Err(ValidationError::HardwareConcurrencyInvalid(
                b.hardware_concurrency,
            ));
        }
        if !matches!(b.color_depth, 24 | 30 | 48) {
            return Err(ValidationError::ColorDepthInvalid(b.color_depth));
        }
        if !(1.0..=3.0).contains(&b.device_pixel_ratio) {
            return Err(ValidationError::DprInvalid(b.device_pixel_ratio));
        }

        // Shim-injection scalars (wave1). Reject absurd values so a bad
        // fixture can't silently ship a 64 MiB heap limit or a 128 px
        // scrollbar into the rendered shim.
        if b.scrollbar_width > 24 {
            return Err(ValidationError::ScrollbarWidthInvalid(b.scrollbar_width));
        }
        // 256 MiB floor = low-end mobile; 8 GiB ceiling = V8 allocates
        // 4-8 GiB on 64-bit desktop and never reports more.
        if b.heap_size_limit < 268_435_456 {
            return Err(ValidationError::HeapSizeTooSmall(b.heap_size_limit));
        }
        if b.heap_size_limit > 8_589_934_592 {
            return Err(ValidationError::HeapSizeTooLarge(b.heap_size_limit));
        }
        if !(4096..=32768).contains(&b.max_texture_size) {
            return Err(ValidationError::MaxTextureSizeInvalid(b.max_texture_size));
        }
        // Real GL drivers report MAX_VIEWPORT_DIMS >= MAX_TEXTURE_SIZE.
        // Catch forged personas where someone typed 2048 for both just to
        // fill the field.
        if b.max_viewport_w < b.max_texture_size || b.max_viewport_h < b.max_texture_size {
            return Err(ValidationError::MaxViewportDimsIncoherent {
                w: b.max_viewport_w,
                h: b.max_viewport_h,
                mts: b.max_texture_size,
            });
        }
        if !matches!(b.audio_sample_rate, 22050 | 44100 | 48000 | 96000) {
            return Err(ValidationError::AudioSampleRateInvalid(b.audio_sample_rate));
        }
        // Font list must parse and match the OS. Liberation on macOS, SF
        // Pro on Linux, Segoe UI on mac — all free one-line tells.
        let fonts_parsed: serde_json::Value = serde_json::from_str(&b.fonts_json)
            .map_err(|e| ValidationError::FontsInvalidJson { err: e.to_string() })?;
        if let Some(arr) = fonts_parsed.as_array() {
            let names: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            check_fonts_platform(&names, declared_os, &b.platform)?;
        }

        // TLS profile coherence — the bundle-derived Profile must map to
        // the same major version the bundle claims. Guards against a caller
        // mutating ua_major without rebuilding the bundle.
        let tls_major = b.profile().major_version();
        if tls_major != b.ua_major {
            return Err(ValidationError::TlsProfileMajorMismatch {
                tls_major,
                ua_major: b.ua_major,
            });
        }

        Ok(())
    }
}

/// Minutes west of UTC (JS `getTimezoneOffset` convention), for tz names
/// we know. Returns `None` for anything else — the caller treats unknown
/// as "don't check" rather than assuming São Paulo and spuriously
/// rejecting a bundle. Duplicate of the helper in `render/pool.rs` —
/// keep in sync; merging is phase 3's refactor work.
fn guess_tz_offset_min(tz: &str) -> Option<i32> {
    Some(match tz {
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
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Linux,
    Windows,
    MacOs,
    Unknown,
}

impl Os {
    fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::Windows => "Windows",
            Self::MacOs => "macOS",
            Self::Unknown => "Unknown",
        }
    }
}

fn detect_ua_os(ua: &str) -> Os {
    // Chrome UA tokens:
    //   Linux   -> "X11; Linux x86_64"
    //   Windows -> "Windows NT 10.0; Win64; x64"
    //   macOS   -> "Macintosh; Intel Mac OS X 10_15_7"
    if ua.contains("X11") || ua.contains("Linux") {
        Os::Linux
    } else if ua.contains("Windows NT") {
        Os::Windows
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        Os::MacOs
    } else {
        Os::Unknown
    }
}

fn detect_platform_os(platform: &str) -> Os {
    // navigator.platform values:
    //   "Linux x86_64" / "Linux armv7l" etc
    //   "Win32"
    //   "MacIntel"
    if platform.starts_with("Linux") {
        Os::Linux
    } else if platform.starts_with("Win") {
        Os::Windows
    } else if platform == "MacIntel" || platform.contains("Mac") {
        Os::MacOs
    } else {
        Os::Unknown
    }
}

fn detect_ch_platform_os(ch: &str) -> Os {
    // sec-ch-ua-platform is quoted: "\"Linux\"" / "\"Windows\"" / "\"macOS\""
    let trimmed = ch.trim_matches('"');
    match trimmed {
        "Linux" => Os::Linux,
        "Windows" => Os::Windows,
        "macOS" | "Mac OS X" => Os::MacOs,
        _ => Os::Unknown,
    }
}

/// GPU vendor keyword recognised by the validator. Restricted to the four
/// majors a desktop Chrome bundle can plausibly ship — adding more would
/// also mean expanding `is_valid_on` below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuVendor {
    Intel,
    Nvidia,
    Amd,
    Apple,
}

impl GpuVendor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Intel => "intel",
            Self::Nvidia => "nvidia",
            Self::Amd => "amd",
            Self::Apple => "apple",
        }
    }

    /// Apple Silicon GPUs only exist on macOS. The other three majors
    /// appear on both Linux and Windows desktop Chrome. `Os::Unknown`
    /// leaves the gate open because the OS-detection layer will have
    /// already flagged it separately.
    fn is_valid_on(self, os: Os) -> bool {
        match (self, os) {
            (Self::Apple, Os::MacOs) => true,
            (Self::Apple, _) => false,
            (Self::Intel | Self::Nvidia | Self::Amd, Os::Linux | Os::Windows | Os::MacOs) => true,
            (_, Os::Unknown) => true,
        }
    }
}

/// Recover the GPU vendor keyword from a WebGL / WebGPU renderer string.
/// Word-boundary-ish matching avoids pathological cases like "intelligent"
/// landing as Intel — we require a surrounding non-letter or string edge.
fn detect_gpu_vendor(s: &str) -> Option<GpuVendor> {
    let lower = s.to_ascii_lowercase();
    for (needle, v) in [
        ("intel", GpuVendor::Intel),
        ("nvidia", GpuVendor::Nvidia),
        ("amd", GpuVendor::Amd),
        ("apple", GpuVendor::Apple),
    ] {
        if contains_word(&lower, needle) {
            return Some(v);
        }
    }
    None
}

fn contains_word(hay: &str, needle: &str) -> bool {
    let bytes = hay.as_bytes();
    let nlen = needle.len();
    let mut i = 0;
    while i + nlen <= bytes.len() {
        if &bytes[i..i + nlen] == needle.as_bytes() {
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphabetic();
            let after_ok = i + nlen == bytes.len() || !bytes[i + nlen].is_ascii_alphabetic();
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Font-list platform coherence. Checks a small exclusion list per OS —
/// fonts that flatly do not ship on that platform. Full membership
/// checking would be fragile (users install extra fonts); the negative
/// rule catches the bot-author-typed "I mixed the lists" mistake.
fn check_fonts_platform(fonts: &[String], os: Os, platform: &str) -> Result<(), ValidationError> {
    // Exclusion lists. Keyword matching is case-insensitive and
    // substring-based so variants like "SF Pro Display" all match.
    let lower: Vec<String> = fonts.iter().map(|f| f.to_ascii_lowercase()).collect();
    let has = |needle: &str| lower.iter().any(|f| f.contains(needle));
    match os {
        Os::Linux => {
            // Proprietary macOS / Windows fonts never ship on a vanilla
            // Linux Chrome. "Arial"/"Times New Roman" via msttcorefonts are
            // common enough to skip.
            for bad in ["sf pro", "helvetica neue", "segoe ui", "calibri"] {
                if has(bad) {
                    return Err(ValidationError::FontsPlatformMismatch {
                        font: bad.into(),
                        platform: platform.into(),
                    });
                }
            }
        }
        Os::MacOs => {
            for bad in ["segoe ui", "calibri", "liberation mono", "dejavu sans mono"] {
                if has(bad) {
                    return Err(ValidationError::FontsPlatformMismatch {
                        font: bad.into(),
                        platform: platform.into(),
                    });
                }
            }
        }
        Os::Windows => {
            for bad in [
                "sf pro",
                "helvetica neue",
                "liberation mono",
                "dejavu sans mono",
            ] {
                if has(bad) {
                    return Err(ValidationError::FontsPlatformMismatch {
                        font: bad.into(),
                        platform: platform.into(),
                    });
                }
            }
        }
        Os::Unknown => {}
    }
    Ok(())
}

fn check_webgl_platform(renderer: &str, os: Os) -> Result<(), ValidationError> {
    let r = renderer.to_ascii_lowercase();
    // Metal/M1/M2 only exist on macOS.
    let mentions_metal = r.contains("metal") || r.contains("apple m1") || r.contains("apple m2");
    // Direct3D/D3D11 is Windows (ANGLE on Win). Chrome on Linux with ANGLE
    // uses OpenGL/Vulkan backends; Direct3D never appears there.
    let mentions_d3d = r.contains("direct3d") || r.contains("d3d11") || r.contains("d3d9");
    match os {
        Os::Linux => {
            if mentions_metal || mentions_d3d {
                return Err(ValidationError::WebglPlatformMismatch {
                    renderer: renderer.into(),
                    platform: "Linux x86_64".into(),
                });
            }
        }
        Os::MacOs => {
            if mentions_d3d {
                return Err(ValidationError::WebglPlatformMismatch {
                    renderer: renderer.into(),
                    platform: "MacIntel".into(),
                });
            }
        }
        Os::Windows => {
            if mentions_metal {
                return Err(ValidationError::WebglPlatformMismatch {
                    renderer: renderer.into(),
                    platform: "Win32".into(),
                });
            }
        }
        Os::Unknown => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mut_bundle() -> IdentityBundle {
        IdentityBundle::from_chromium(131, 0xdead_beef)
    }

    #[test]
    fn defaults_are_coherent() {
        IdentityValidator::check(&mut_bundle()).expect("from_chromium must validate");
    }

    #[test]
    fn ua_platform_mismatch_rejected() {
        let mut b = mut_bundle();
        b.platform = "Win32".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::UaPlatformMismatch { .. })
        ));
    }

    #[test]
    fn ch_platform_mismatch_rejected() {
        let mut b = mut_bundle();
        b.ua_platform = "\"Windows\"".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::ChPlatformMismatch { .. })
        ));
    }

    #[test]
    fn webgl_d3d_on_linux_rejected() {
        let renderer = "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)";
        assert!(matches!(
            check_webgl_platform(renderer, Os::Linux),
            Err(ValidationError::WebglPlatformMismatch { .. })
        ));
    }

    #[test]
    fn webgl_metal_on_windows_rejected() {
        let renderer = "ANGLE (Apple M1, Metal)";
        assert!(matches!(
            check_webgl_platform(renderer, Os::Windows),
            Err(ValidationError::WebglPlatformMismatch { .. })
        ));
    }

    #[test]
    fn webgl_opengl_on_linux_ok() {
        let renderer = "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)";
        assert!(check_webgl_platform(renderer, Os::Linux).is_ok());
    }

    #[test]
    fn accept_language_mismatch_rejected() {
        let mut b = mut_bundle();
        b.accept_language = "fr-FR,fr;q=0.9".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::AcceptLanguageMismatch { .. })
        ));
    }

    #[test]
    fn device_memory_out_of_bucket_rejected() {
        let mut b = mut_bundle();
        b.device_memory = 6;
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::DeviceMemoryInvalid(6))
        ));
    }

    #[test]
    fn hardware_concurrency_out_of_range_rejected() {
        let mut b = mut_bundle();
        b.hardware_concurrency = 64;
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::HardwareConcurrencyInvalid(64))
        ));
    }

    #[test]
    fn viewport_exceeds_avail_rejected() {
        let mut b = mut_bundle();
        b.viewport_h = b.avail_screen_h + 1;
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::ViewportExceedsAvail { axis: "h", .. })
        ));
    }

    #[test]
    fn full_version_major_mismatch_rejected() {
        let mut b = mut_bundle();
        b.ua_full_version = "132.0.0.0".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::UaFullVersionMajorMismatch { .. })
        ));
    }

    #[test]
    fn unknown_timezone_does_not_spuriously_reject() {
        // Before: unknown tz fell back to São Paulo's +180 and could
        // reject valid bundles. After nit #8 fix: unknown → None →
        // skip check.
        let mut b = mut_bundle();
        b.timezone = "Pacific/Auckland".into();
        b.tz_offset_min = -720;
        IdentityValidator::check(&b).expect("unknown tz with sane offset must pass");
    }

    #[test]
    fn known_timezone_still_catches_off_by_hour() {
        let mut b = mut_bundle();
        b.timezone = "America/Sao_Paulo".into();
        b.tz_offset_min = 0; // wrong: SP is +180 min west
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::TimezoneOffsetMismatch { .. })
        ));
    }

    #[test]
    fn tls_profile_major_mismatch_rejected() {
        // Force a mismatch: claim ua_major=130 while keeping the 131
        // brands/UA. Profile::from_detected_major(130) returns Chrome131
        // (nearest-not-newer), so this also exercises the fallback path —
        // the check fires because Chrome131.major_version()=131 ≠ 130.
        let mut b = mut_bundle();
        b.ua_major = 130;
        // Keep ua/sec-ch-ua referencing 131 so earlier checks don't short-
        // circuit; only the TLS profile check should trigger.
        // Actually UA check will fire first if we leave ua at 131. Rebuild
        // ua/sec so both reference 130, but full_version still starts 131.
        b.ua = b.ua.replace("131", "130");
        b.sec_ch_ua = b.sec_ch_ua.replace("131", "130");
        b.ua_brands = b.ua_brands.replace("131", "130");
        // Leave ua_full_version as "131.0.6778.85" so UaFullVersionMajorMismatch
        // would fire — swap that too.
        b.ua_full_version = "130.0.0.0".into();
        b.ua_full_version_list = b.ua_full_version_list.replace("131", "130");
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::TlsProfileMajorMismatch {
                tls_major: 131,
                ua_major: 130
            })
        ));
    }

    #[test]
    fn ua_brands_missing_major_rejected() {
        let mut b = mut_bundle();
        b.ua_brands = r#"[{"brand":"Google Chrome","version":"130"}]"#.into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::UaBrandsMissingMajor { major: 131 })
        ));
    }

    #[test]
    fn webgl_vendor_keyword_detected_for_all_majors() {
        // Word-boundary matcher handles both ANGLE-wrapped and bare strings,
        // plus common substring traps (nothing matches "intelligent").
        assert_eq!(
            detect_gpu_vendor("ANGLE (Intel, Mesa Intel(R) UHD Graphics)"),
            Some(GpuVendor::Intel)
        );
        assert_eq!(
            detect_gpu_vendor("NVIDIA GeForce RTX 3080"),
            Some(GpuVendor::Nvidia)
        );
        assert_eq!(
            detect_gpu_vendor("ANGLE (AMD, AMD Radeon RX 6800)"),
            Some(GpuVendor::Amd)
        );
        assert_eq!(detect_gpu_vendor("Apple M1"), Some(GpuVendor::Apple));
        assert_eq!(detect_gpu_vendor("intelligent system"), None);
        assert_eq!(detect_gpu_vendor("WebKit WebGL"), None);
    }

    #[test]
    fn webgl_vendor_mismatch_between_masked_and_unmasked_rejected() {
        // Forge a bundle where masked says Intel but unmasked says NVIDIA —
        // the exact inconsistency fingerprinters hash across the two slots.
        let mut b = mut_bundle();
        b.webgl_unmasked_renderer = "NVIDIA GeForce RTX 3080".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::WebglVendorKeywordMismatch { .. })
        ));
    }

    #[test]
    fn webgl_vendor_unrecognised_rejected() {
        let mut b = mut_bundle();
        b.webgl_renderer = "WebKit WebGL".into();
        b.webgl_unmasked_renderer = "WebKit WebGL".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::WebglVendorUnrecognised { .. })
        ));
    }

    #[test]
    fn webgl_apple_on_linux_rejected() {
        // Apple Silicon GPU only exists on macOS. Linux + Apple is instant.
        let mut b = mut_bundle();
        // Use an OpenGL-flavoured string so the earlier OS/renderer check
        // (metal → macOS) doesn't fire first.
        b.webgl_renderer = "ANGLE (Apple, Apple GPU, OpenGL 4.1)".into();
        b.webgl_unmasked_renderer = "ANGLE (Apple, Apple GPU, OpenGL 4.1)".into();
        b.webgpu_adapter_description = "ANGLE (Apple, Apple GPU, OpenGL 4.1)".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::WebglVendorPlatformMismatch { .. })
        ));
    }

    #[test]
    fn webgpu_vendor_mismatch_rejected() {
        // WebGL says Intel but WebGPU adapter description says NVIDIA — the
        // precise one-line tell P1.6 was filed to close.
        let mut b = mut_bundle();
        b.webgpu_adapter_description = "NVIDIA GeForce RTX 3080".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::WebgpuVendorMismatch { .. })
        ));
    }

    #[test]
    fn webgpu_vendor_missing_keyword_rejected() {
        // A description with no vendor keyword at all fails the WebGPU
        // check — we need *some* keyword to compare against webgl.
        let mut b = mut_bundle();
        b.webgpu_adapter_description = "Generic Renderer".into();
        assert!(matches!(
            IdentityValidator::check(&b),
            Err(ValidationError::WebgpuVendorMismatch { .. })
        ));
    }
}
