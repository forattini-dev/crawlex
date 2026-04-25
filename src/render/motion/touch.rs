//! Touch/pointer event generation for mobile personas.
//!
//! When the active `IdentityBundle` projects a mobile UA (`iPhone`, `Android`,
//! `Mobile`), dispatching `mousedown/mouseup/click` is an instant
//! FP-Inconsistent tell — real mobile browsers fire `touchstart`,
//! `touchmove`, `touchend`, and a coalesced `pointer*` pair. This module
//! maps a `TouchSequence` to the CDP `Input.dispatchTouchEvent` frames that
//! reproduce that shape.
//!
//! Pure math here — `interact` owns the CDP wiring.

use crate::render::motion::MotionProfile;

/// Classification of the active persona — drives event family selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touch,
}

/// Detect `PointerKind` from a UA string. We only look at well-known mobile
/// keywords — anything ambiguous defaults to `Mouse` so desktop flows never
/// regress.
pub fn pointer_kind_from_ua(ua: &str) -> PointerKind {
    let ua = ua.to_ascii_lowercase();
    if ua.contains("iphone")
        || ua.contains("ipad")
        || ua.contains("android") && ua.contains("mobile")
        || ua.contains("mobile safari")
    {
        return PointerKind::Touch;
    }
    PointerKind::Mouse
}

/// One frame in a touch sequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchFrame {
    pub phase: TouchPhase,
    pub x: f64,
    pub y: f64,
    pub delay_ms: u64,
    /// Simulated finger radius (px). Real fingertips are 8–15 px; we match
    /// the population mode so `touches[i].radiusX` on the page looks sane.
    pub radius_px: f64,
    /// Pressure ∈ [0, 1]. Real devices return ~0.5 for finger taps.
    pub force: f64,
}

/// Profile-aware touch tunables.
#[derive(Debug, Clone, Copy)]
pub struct TouchParams {
    pub tap_hold_ms_min: u64,
    pub tap_hold_ms_max: u64,
    pub move_step_ms_min: u64,
    pub move_step_ms_max: u64,
    pub radius_px: f64,
    pub force: f64,
}

impl TouchParams {
    pub fn for_profile(p: MotionProfile) -> Self {
        match p {
            MotionProfile::Fast => TouchParams {
                tap_hold_ms_min: 10,
                tap_hold_ms_max: 25,
                move_step_ms_min: 5,
                move_step_ms_max: 15,
                radius_px: 10.0,
                force: 0.5,
            },
            MotionProfile::Balanced => TouchParams {
                tap_hold_ms_min: 60,
                tap_hold_ms_max: 180,
                move_step_ms_min: 12,
                move_step_ms_max: 28,
                radius_px: 11.0,
                force: 0.5,
            },
            MotionProfile::Human => TouchParams {
                tap_hold_ms_min: 80,
                tap_hold_ms_max: 240,
                move_step_ms_min: 14,
                move_step_ms_max: 36,
                radius_px: 12.0,
                force: 0.55,
            },
            MotionProfile::Paranoid => TouchParams {
                tap_hold_ms_min: 140,
                tap_hold_ms_max: 400,
                move_step_ms_min: 18,
                move_step_ms_max: 48,
                radius_px: 13.0,
                force: 0.6,
            },
        }
    }
}

/// Convert a desktop-style trajectory (see `motion::MotionEngine::trajectory`)
/// into a touch sequence: `Start` at the first point, `Move` at each interior
/// point, `End` at the final point with a post-move `tap_hold` delay.
pub fn sequence_from_trajectory(
    points: &[(f64, f64, u64)],
    params: &TouchParams,
    tap_hold_ms: u64,
) -> Vec<TouchFrame> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(points.len() + 1);
    let (x0, y0, _) = points[0];
    out.push(TouchFrame {
        phase: TouchPhase::Start,
        x: x0,
        y: y0,
        delay_ms: 0,
        radius_px: params.radius_px,
        force: params.force,
    });
    for (x, y, d) in points.iter().skip(1) {
        out.push(TouchFrame {
            phase: TouchPhase::Move,
            x: *x,
            y: *y,
            delay_ms: *d,
            radius_px: params.radius_px,
            force: params.force,
        });
    }
    let (xn, yn, _) = *points.last().unwrap();
    out.push(TouchFrame {
        phase: TouchPhase::End,
        x: xn,
        y: yn,
        delay_ms: tap_hold_ms,
        radius_px: params.radius_px,
        force: 0.0,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_mobile_ua() {
        let ios = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Mobile/15E148";
        assert_eq!(pointer_kind_from_ua(ios), PointerKind::Touch);
        let android =
            "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 Mobile Safari/537.36";
        assert_eq!(pointer_kind_from_ua(android), PointerKind::Touch);
    }

    #[test]
    fn detect_desktop_ua() {
        let desktop = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        assert_eq!(pointer_kind_from_ua(desktop), PointerKind::Mouse);
    }

    #[test]
    fn sequence_has_start_move_end() {
        let p = TouchParams::for_profile(MotionProfile::Balanced);
        let pts = vec![(0.0, 0.0, 0), (5.0, 5.0, 10), (10.0, 10.0, 10)];
        let seq = sequence_from_trajectory(&pts, &p, 100);
        assert_eq!(seq.first().unwrap().phase, TouchPhase::Start);
        assert_eq!(seq.last().unwrap().phase, TouchPhase::End);
        assert!(seq.iter().any(|f| f.phase == TouchPhase::Move));
    }

    #[test]
    fn empty_trajectory_empty_sequence() {
        let p = TouchParams::for_profile(MotionProfile::Balanced);
        assert!(sequence_from_trajectory(&[], &p, 0).is_empty());
    }

    #[test]
    fn radius_matches_params() {
        let p = TouchParams::for_profile(MotionProfile::Human);
        let pts = vec![(0.0, 0.0, 0), (1.0, 1.0, 5)];
        let seq = sequence_from_trajectory(&pts, &p, 80);
        for f in seq.iter().take_while(|f| f.phase != TouchPhase::End) {
            assert!((f.radius_px - p.radius_px).abs() < 1e-9);
        }
    }
}
