//! Device-type motion profile — mouse vs trackpad vs trackball.
//!
//! The input device shapes the scroll / move signature. A trackpad scrolls
//! with momentum inertia, a mouse scrolls in discrete wheel ticks, a
//! trackball is jerkier on small-motion follow-through. Modern antibot
//! frameworks (Cloudflare, DataDome) fingerprint `wheelEvent.deltaMode`
//! + velocity autocorrelation to infer device class. When the UA says
//! "Macintosh" we bias toward trackpad; Linux/Windows bias toward mouse.

use crate::render::motion::MotionProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionDeviceProfile {
    Mouse,
    Trackpad,
    Trackball,
}

impl MotionDeviceProfile {
    /// Pick a plausible device class for the given UA. Deterministic on UA
    /// (not random) so a single persona keeps a single device across a run.
    pub fn for_ua(ua: &str) -> Self {
        let u = ua.to_ascii_lowercase();
        if u.contains("macintosh") || u.contains("mac os x") {
            // Mac desktop population: majority trackpad (laptops) +
            // sizable Magic Mouse. We bias trackpad; Magic Mouse is close
            // enough to trackpad inertia that the heuristic isn't wrong.
            return MotionDeviceProfile::Trackpad;
        }
        if u.contains("iphone") || u.contains("ipad") || u.contains("android") {
            // Mobile → handled by `touch.rs`. Callers should check
            // `pointer_kind_from_ua` first; fall back to mouse here.
            return MotionDeviceProfile::Mouse;
        }
        MotionDeviceProfile::Mouse
    }

    /// Scroll inertia flag. True = trackpad-style coasting (smooth, many
    /// tiny wheel events at decreasing velocity). False = discrete wheel
    /// ticks.
    pub fn scroll_inertia(self) -> bool {
        matches!(self, MotionDeviceProfile::Trackpad)
    }

    /// Velocity multiplier applied to base motion params. Trackballs have a
    /// jitterier follow-through; trackpads glide a bit faster; mice sit at
    /// the baseline.
    pub fn velocity_scale(self) -> f64 {
        match self {
            MotionDeviceProfile::Mouse => 1.0,
            MotionDeviceProfile::Trackpad => 1.05,
            MotionDeviceProfile::Trackball => 0.9,
        }
    }

    /// Additional OU jitter σ on top of the base motion params. Trackballs
    /// add noticeable tremor; trackpads are the smoothest.
    pub fn extra_jitter_sigma(self, profile: MotionProfile) -> f64 {
        let base = match profile {
            MotionProfile::Fast => 0.0,
            MotionProfile::Balanced => 0.1,
            MotionProfile::Human => 0.15,
            MotionProfile::Paranoid => 0.2,
        };
        match self {
            MotionDeviceProfile::Mouse => base,
            MotionDeviceProfile::Trackpad => base * 0.6,
            MotionDeviceProfile::Trackball => base * 1.8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_ua_is_trackpad() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/131";
        assert_eq!(
            MotionDeviceProfile::for_ua(ua),
            MotionDeviceProfile::Trackpad
        );
    }

    #[test]
    fn linux_ua_is_mouse() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64) Chrome/131";
        assert_eq!(MotionDeviceProfile::for_ua(ua), MotionDeviceProfile::Mouse);
    }

    #[test]
    fn scroll_inertia_matches_device() {
        assert!(MotionDeviceProfile::Trackpad.scroll_inertia());
        assert!(!MotionDeviceProfile::Mouse.scroll_inertia());
        assert!(!MotionDeviceProfile::Trackball.scroll_inertia());
    }

    #[test]
    fn velocity_scale_has_expected_ordering() {
        let m = MotionDeviceProfile::Mouse.velocity_scale();
        let tp = MotionDeviceProfile::Trackpad.velocity_scale();
        let tb = MotionDeviceProfile::Trackball.velocity_scale();
        assert!(tp > m);
        assert!(tb < m);
    }

    #[test]
    fn trackball_jitter_exceeds_mouse() {
        let m = MotionDeviceProfile::Mouse.extra_jitter_sigma(MotionProfile::Human);
        let tb = MotionDeviceProfile::Trackball.extra_jitter_sigma(MotionProfile::Human);
        assert!(tb > m);
    }

    #[test]
    fn fast_profile_has_zero_extra_jitter() {
        for d in [
            MotionDeviceProfile::Mouse,
            MotionDeviceProfile::Trackpad,
            MotionDeviceProfile::Trackball,
        ] {
            assert_eq!(d.extra_jitter_sigma(MotionProfile::Fast), 0.0);
        }
    }
}
