//! Device-type motion profile — mouse vs trackpad vs trackball (#22).

#![cfg(feature = "cdp-backend")]

use crawlex::render::motion::device::MotionDeviceProfile;
use crawlex::render::motion::MotionProfile;

#[test]
fn mac_ua_maps_to_trackpad() {
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    assert_eq!(
        MotionDeviceProfile::for_ua(ua),
        MotionDeviceProfile::Trackpad
    );
}

#[test]
fn linux_ua_maps_to_mouse() {
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    assert_eq!(MotionDeviceProfile::for_ua(ua), MotionDeviceProfile::Mouse);
}

#[test]
fn trackpad_has_scroll_inertia() {
    assert!(MotionDeviceProfile::Trackpad.scroll_inertia());
    assert!(!MotionDeviceProfile::Mouse.scroll_inertia());
}

#[test]
fn velocity_scale_respects_device_class() {
    assert!(
        MotionDeviceProfile::Trackpad.velocity_scale()
            > MotionDeviceProfile::Mouse.velocity_scale()
    );
    assert!(
        MotionDeviceProfile::Trackball.velocity_scale()
            < MotionDeviceProfile::Mouse.velocity_scale()
    );
}

#[test]
fn trackball_adds_jitter() {
    let tb = MotionDeviceProfile::Trackball.extra_jitter_sigma(MotionProfile::Human);
    let m = MotionDeviceProfile::Mouse.extra_jitter_sigma(MotionProfile::Human);
    assert!(tb > m);
}

#[test]
fn fast_profile_suppresses_extra_jitter() {
    for d in [
        MotionDeviceProfile::Mouse,
        MotionDeviceProfile::Trackpad,
        MotionDeviceProfile::Trackball,
    ] {
        assert_eq!(d.extra_jitter_sigma(MotionProfile::Fast), 0.0);
    }
}
