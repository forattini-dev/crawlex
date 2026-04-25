//! Synthetic telemetry generator for the `oz` field 74 client-fingerprint
//! blob. Port of `recaptchav3/emulation/telemetry.py`.
//!
//! Improvements over the reference:
//! * **Persona-aware.** When `IdentityBundle` is supplied, screen / UA / TZ
//!   come from the bundle (kept coherent with everything else our crawler
//!   emits) instead of being random per call. The Python version hardcoded
//!   Windows + Chrome 136, which trips any cross-check.
//! * **Smoothstep + Gaussian jitter** on the mouse path matches the
//!   reference. Our `MotionEngine` is more sophisticated; we keep this
//!   lighter generator for the server-side solve path where there's no
//!   browser to drive.

use rand::{Rng, RngExt};

use crate::identity::IdentityBundle;

/// Fixed list of "common third-party domains a real page would talk to"
/// that the reference solver embeds in field 74. Empirically these are
/// what reCAPTCHA's classifier expects to see; an empty list is itself a
/// signal that the telemetry was synthetic. We add a couple more to dilute
/// the static-list signature.
pub const COMMON_DOMAINS: &[&str] = &[
    "www.googletagmanager.com",
    "static.cloudflareinsights.com",
    "www.google.com",
    "www.clarity.ms",
    "www.gstatic.com",
];

/// One mouse path entry: `[1, x, y, ts]` — leading `1` is the event type
/// the reference uses (mouse move). Stored as `serde_json::Value` to keep
/// the JSON shape exactly aligned with what the reference produces.
fn mouse_path(
    rng: &mut impl Rng,
    start: (f64, f64),
    end: (f64, f64),
    steps: usize,
    base_ts_ms: i64,
) -> (Vec<serde_json::Value>, i64) {
    let mut path = Vec::with_capacity(steps);
    let (mut cx, mut cy) = start;
    let mut ts = base_ts_ms;
    for i in 0..steps {
        let t = i as f64 / (steps.max(2) - 1) as f64;
        // Smoothstep — same curve as reference (3t² - 2t³).
        let ease = t * t * (3.0 - 2.0 * t);
        let fade = 1.0 - ease;
        let tx = start.0 + (end.0 - start.0) * ease;
        let ty = start.1 + (end.1 - start.1) * ease;

        // Box-Muller via two uniforms — gives a normal distribution with
        // σ = 12 * fade. Reference uses random.gauss(0, 12*fade) directly;
        // we roll our own to avoid pulling in `rand_distr` for one call.
        let n_x = gaussian(rng) * 12.0 * fade;
        let n_y = gaussian(rng) * 12.0 * fade;
        cx += (tx - cx + n_x) * 0.6;
        cy += (ty - cy + n_y) * 0.6;

        let mut delay = rng.random_range(40..=140);
        if i == 0 || i == steps - 1 {
            delay += rng.random_range(80..=250);
        }
        ts += delay;
        path.push(serde_json::json!([
            1,
            cx.round() as i64,
            cy.round() as i64,
            ts
        ]));
    }
    (path, ts)
}

/// One Box-Muller sample. Two uniforms in `(0, 1]` → one N(0, 1) value.
fn gaussian(rng: &mut impl Rng) -> f64 {
    let u1: f64 = rng.random_range(1e-10..1.0);
    let u2: f64 = rng.random_range(0.0..1.0);
    let r: f64 = -2.0_f64 * u1.ln();
    r.sqrt() * (2.0_f64 * std::f64::consts::PI * u2).cos()
}

/// Three scroll events at increasing offsets, matching the reference's
/// `[(500, 3000), (2000, 6000), (4000, 9000)]` window pattern.
fn scroll_events(rng: &mut impl Rng, base_ts_ms: i64) -> Vec<serde_json::Value> {
    let windows = [(500i64, 3000i64), (2000, 6000), (4000, 9000)];
    windows
        .iter()
        .map(|&(lo, hi)| {
            serde_json::json!([
                2,
                rng.random_range(40..=220),
                base_ts_ms + rng.random_range(lo..=hi)
            ])
        })
        .collect()
}

/// Performance metrics blob. Shape mirrors the reference exactly — three
/// nulls then two arrays then `0, 0, 0`. The numbers don't need to be
/// realistic to pass; reCAPTCHA's classifier just needs the *shape*. We
/// jitter inside plausible browser ranges so two solves don't byte-match.
fn performance_metrics(rng: &mut impl Rng) -> Vec<serde_json::Value> {
    vec![
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::json!([
            9,
            round8(rng.random_range(5.0..12.0)),
            round8(rng.random_range(0.005..0.03)),
            rng.random_range(12..=24)
        ]),
        serde_json::json!([
            rng.random_range(80..=140),
            round8(rng.random_range(0.2..0.6)),
            round8(rng.random_range(0.003..0.01)),
            rng.random_range(3..=8)
        ]),
        serde_json::json!(0),
        serde_json::json!(0),
        serde_json::json!(0),
    ]
}

fn round8(v: f64) -> f64 {
    (v * 1e8).round() / 1e8
}

/// Build the full field-74 client blob. When `bundle` is `Some`, screen +
/// UA + timezone are pulled from the persona; otherwise we fall back to
/// reasonable Windows/Chrome defaults to keep the solver usable in
/// no-persona contexts (CLI smoke tests, local debugging).
pub fn generate(
    rng: &mut impl Rng,
    bundle: Option<&IdentityBundle>,
    canvas_hash: &str,
    webgl_renderer: &str,
) -> serde_json::Value {
    let now_ms = chrono_ms();
    let base_ts = now_ms - rng.random_range(8000..=25000);

    let mut mouse: Vec<serde_json::Value> = Vec::new();
    let segments = rng.random_range(2..=4);
    let mut ts_cursor = base_ts;
    for _ in 0..segments {
        let sx = rng.random_range(200.0..=1400.0);
        let sy = rng.random_range(150.0..=800.0);
        let ex = rng.random_range(400.0..=1600.0);
        let ey = rng.random_range(300.0..=900.0);
        let (segment, last_ts) = mouse_path(rng, (sx, sy), (ex, ey), 12, ts_cursor);
        mouse.extend(segment);
        ts_cursor = last_ts;
    }

    let scroll = scroll_events(rng, base_ts);
    let perf = performance_metrics(rng);

    let (ua, screen, tz_offset_min) = match bundle {
        Some(b) => (
            b.ua.clone(),
            vec![b.screen_w as i64, b.screen_h as i64],
            b.tz_offset_min,
        ),
        None => (
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36"
                .to_string(),
            vec![1920, 1080],
            0,
        ),
    };

    serde_json::json!({
        "mouse": mouse,
        "keyboard": [],
        "touch": [],
        "scroll": scroll,
        "resize": [],
        "ua": ua,
        "screen": screen,
        "timezone": tz_offset_min,
        "canvas": canvas_hash,
        "webgl": webgl_renderer,
        "perf": perf,
        "domains": COMMON_DOMAINS,
        "session": [rng.random_range(6..=12), rng.random_range(400..=1200)],
    })
}

fn chrono_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn mouse_path_has_requested_steps() {
        let mut rng = StdRng::seed_from_u64(1);
        let (path, _) = mouse_path(&mut rng, (0.0, 0.0), (100.0, 100.0), 12, 0);
        assert_eq!(path.len(), 12);
    }

    #[test]
    fn mouse_path_entries_have_event_type_one() {
        let mut rng = StdRng::seed_from_u64(2);
        let (path, _) = mouse_path(&mut rng, (10.0, 10.0), (50.0, 50.0), 6, 1000);
        for entry in &path {
            assert_eq!(entry[0].as_i64().unwrap(), 1);
            assert_eq!(entry.as_array().unwrap().len(), 4);
        }
    }

    #[test]
    fn scroll_events_count_is_three() {
        let mut rng = StdRng::seed_from_u64(3);
        let s = scroll_events(&mut rng, 1_000_000);
        assert_eq!(s.len(), 3);
        for e in &s {
            assert_eq!(e[0].as_i64().unwrap(), 2);
        }
    }

    #[test]
    fn generate_with_bundle_uses_persona_ua() {
        let mut rng = StdRng::seed_from_u64(4);
        let bundle = IdentityBundle::from_chromium(131, 7);
        let blob = generate(&mut rng, Some(&bundle), "1234567890", "ANGLE (Intel)");
        assert_eq!(blob["ua"].as_str().unwrap(), bundle.ua);
        // Screen reflects bundle.
        let screen = blob["screen"].as_array().unwrap();
        assert_eq!(screen[0].as_i64().unwrap() as u32, bundle.screen_w);
    }

    #[test]
    fn generate_without_bundle_uses_fallback() {
        let mut rng = StdRng::seed_from_u64(5);
        let blob = generate(&mut rng, None, "abc", "ANGLE (NVIDIA)");
        assert!(blob["ua"].as_str().unwrap().contains("Chrome/136"));
    }

    #[test]
    fn generate_includes_common_domains() {
        let mut rng = StdRng::seed_from_u64(6);
        let blob = generate(&mut rng, None, "x", "y");
        let domains = blob["domains"].as_array().unwrap();
        assert_eq!(domains.len(), COMMON_DOMAINS.len());
    }
}
