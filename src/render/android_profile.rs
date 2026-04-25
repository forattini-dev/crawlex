//! Android emulator profile hook — SCAFFOLD (issue #36).
//!
//! Lets the operator tell the renderer "pretend to be a Pixel 7 Pro on
//! Android 14" by emitting the corresponding CDP `Emulation.*` commands
//! on every fresh page. The bundle of metrics + UA + touch flag is what
//! makes `navigator.userAgent`, `window.innerWidth`, and the `pointer:
//! coarse` media query all agree.
//!
//! **Scaffold status:** device presets + CDP payload builders are real
//! (no unimplemented!() here — the functions are pure and unit-tested).
//! The actual CDP wire-up (`chrome_wire::send_command("Emulation.setDeviceMetricsOverride", ...)`)
//! is *not* hooked into the render path yet — doing so would require
//! editing the existing `render/chrome` modules, which is explicitly out
//! of scope for this scaffold wave. A follow-up wave wires
//! `AndroidProfile::apply(&mut ChromeSession)` into
//! `crate::render::chrome::launch`.
//!
//! **Real ADB emulator** (running AOSP in an Android VM and driving it
//! over `adb`) is intentionally out of scope — this crate emulates the
//! device *fingerprint* only. Operators who need full Android behaviour
//! (Play Integrity, hardware attestation) spin up their own emulator and
//! point the crawler at it via the CDP endpoint; see
//! `docs/infra-tier-operator.md § Android handoff`.

use serde::{Deserialize, Serialize};

/// Supported device presets. Extend by adding a constructor below and a
/// match arm in [`AndroidProfile::preset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AndroidDevice {
    /// Pixel 7 Pro on Android 14 (Chrome stable).
    Pixel7Pro,
    /// Pixel 8 on Android 14 (Chrome stable).
    Pixel8,
    /// Samsung Galaxy S23 on Android 14.
    GalaxyS23,
}

impl AndroidDevice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pixel7Pro => "pixel-7-pro",
            Self::Pixel8 => "pixel-8",
            Self::GalaxyS23 => "galaxy-s23",
        }
    }
}

impl std::str::FromStr for AndroidDevice {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pixel-7-pro" | "pixel7pro" => Ok(Self::Pixel7Pro),
            "pixel-8" | "pixel8" => Ok(Self::Pixel8),
            "galaxy-s23" | "s23" => Ok(Self::GalaxyS23),
            _ => Err(()),
        }
    }
}

/// Viewport + UA + input flags — everything CDP needs to emulate a device.
/// Mirrors the `Emulation.setDeviceMetricsOverride` + `setUserAgentOverride`
/// + `setTouchEmulationEnabled` triple.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidProfile {
    pub device: AndroidDevice,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
    pub user_agent: String,
    pub user_agent_client_hints_platform: &'static str,
    pub touch_enabled: bool,
}

impl AndroidProfile {
    /// Resolve a preset. Stable — the UA strings here are frozen for the
    /// lifetime of a release so tests that lock on a specific UA keep
    /// passing across patch bumps.
    pub fn preset(device: AndroidDevice) -> Self {
        match device {
            AndroidDevice::Pixel7Pro => Self {
                device,
                width: 412,
                height: 915,
                device_scale_factor: 2.625,
                mobile: true,
                user_agent: "Mozilla/5.0 (Linux; Android 14; Pixel 7 Pro) \
                             AppleWebKit/537.36 (KHTML, like Gecko) \
                             Chrome/149.0.0.0 Mobile Safari/537.36"
                    .to_string(),
                user_agent_client_hints_platform: "Android",
                touch_enabled: true,
            },
            AndroidDevice::Pixel8 => Self {
                device,
                width: 412,
                height: 915,
                device_scale_factor: 2.625,
                mobile: true,
                user_agent: "Mozilla/5.0 (Linux; Android 14; Pixel 8) \
                             AppleWebKit/537.36 (KHTML, like Gecko) \
                             Chrome/149.0.0.0 Mobile Safari/537.36"
                    .to_string(),
                user_agent_client_hints_platform: "Android",
                touch_enabled: true,
            },
            AndroidDevice::GalaxyS23 => Self {
                device,
                width: 360,
                height: 780,
                device_scale_factor: 3.0,
                mobile: true,
                user_agent: "Mozilla/5.0 (Linux; Android 14; SM-S911B) \
                             AppleWebKit/537.36 (KHTML, like Gecko) \
                             Chrome/149.0.0.0 Mobile Safari/537.36"
                    .to_string(),
                user_agent_client_hints_platform: "Android",
                touch_enabled: true,
            },
        }
    }

    /// Build the JSON payload for `Emulation.setDeviceMetricsOverride`.
    /// Kept pure so tests (and anyone who wants to reuse the preset
    /// outside CDP) don't need a live Chrome.
    pub fn device_metrics_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "width": self.width,
            "height": self.height,
            "deviceScaleFactor": self.device_scale_factor,
            "mobile": self.mobile,
        })
    }

    /// Payload for `Emulation.setUserAgentOverride`.
    pub fn user_agent_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "userAgent": self.user_agent,
            "platform": self.user_agent_client_hints_platform,
            "userAgentMetadata": {
                "platform": self.user_agent_client_hints_platform,
                "platformVersion": "14.0.0",
                "architecture": "",
                "model": match self.device {
                    AndroidDevice::Pixel7Pro => "Pixel 7 Pro",
                    AndroidDevice::Pixel8 => "Pixel 8",
                    AndroidDevice::GalaxyS23 => "SM-S911B",
                },
                "mobile": self.mobile,
            }
        })
    }

    /// Payload for `Emulation.setTouchEmulationEnabled`.
    pub fn touch_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.touch_enabled,
            "maxTouchPoints": if self.touch_enabled { 5 } else { 0 },
        })
    }

    /// Ordered list of (method, params) CDP commands to send when a new
    /// page is ready. Consumer is expected to be the as-yet-unwritten
    /// wire-up in `render/chrome`.
    pub fn cdp_commands(&self) -> Vec<(&'static str, serde_json::Value)> {
        vec![
            (
                "Emulation.setDeviceMetricsOverride",
                self.device_metrics_payload(),
            ),
            ("Emulation.setUserAgentOverride", self.user_agent_payload()),
            ("Emulation.setTouchEmulationEnabled", self.touch_payload()),
        ]
    }
}

/// Parse the (future) `--mobile-profile` CLI value.
pub fn parse_mobile_profile(raw: &str) -> Option<AndroidProfile> {
    let device: AndroidDevice = raw.parse().ok()?;
    Some(AndroidProfile::preset(device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_7_pro_preset_is_stable() {
        let p = AndroidProfile::preset(AndroidDevice::Pixel7Pro);
        assert_eq!(p.width, 412);
        assert_eq!(p.height, 915);
        assert!((p.device_scale_factor - 2.625).abs() < 1e-6);
        assert!(p.mobile);
        assert!(p.touch_enabled);
        assert!(p.user_agent.contains("Android 14"));
        assert!(p.user_agent.contains("Pixel 7 Pro"));
    }

    #[test]
    fn cdp_commands_cover_the_triple() {
        let p = AndroidProfile::preset(AndroidDevice::Pixel8);
        let cmds = p.cdp_commands();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].0, "Emulation.setDeviceMetricsOverride");
        assert_eq!(cmds[1].0, "Emulation.setUserAgentOverride");
        assert_eq!(cmds[2].0, "Emulation.setTouchEmulationEnabled");
    }

    #[test]
    fn parse_mobile_profile_accepts_aliases() {
        assert!(parse_mobile_profile("pixel-7-pro").is_some());
        assert!(parse_mobile_profile("pixel8").is_some());
        assert!(parse_mobile_profile("s23").is_some());
        assert!(parse_mobile_profile("iphone").is_none());
    }
}
