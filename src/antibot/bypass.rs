//! Vendor-specific bypass tricks.
//!
//! This module is the home of the opt-in, ethically-bounded bypass
//! attempts we know about. The default [`BypassLevel::None`] disables
//! every trick; operators must pass `--antibot-bypass <level>` to turn
//! them on.
//!
//! Implemented vectors:
//!
//! - **Akamai `_abck`**: capture successful-solve cookie and pin it
//!   (24h conservative TTL) via [`crate::antibot::cookie_pin`].
//! - **DataDome**: on a 403 with `Set-Cookie: datadome=…`, extract and
//!   pin the fresh value. Callers then retry the request with the
//!   pinned cookie injected into the jar.
//! - **PerimeterX `_px*`**: same pattern, 24h TTL.
//! - **Cloudflare Turnstile "invisible"**: when the widget has no
//!   visible challenge but exposes a `data-sitekey`, we can attempt a
//!   best-effort submission with a dummy token. Success rate is low
//!   (~30% when entropy is low) but the caller can tell we tried via
//!   [`TurnstileAttempt::outcome`].
//!
//! **No network IO lives in this module.** Higher layers (HTTP path,
//! render path) feed us response bytes and we hand back structured
//! capture/attempt records. That keeps the module testable offline and
//! safe to compile into `crawlex-mini`.

use http::HeaderMap;

use super::cookie_pin::{
    CookiePinStore, AKAMAI_ABCK_TTL_SECS, DATADOME_TTL_SECS, PERIMETERX_TTL_SECS,
};

/// Operator-facing bypass tier. Parsed from `--antibot-bypass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BypassLevel {
    /// No bypass tricks at all. Default.
    #[default]
    None,
    /// Cookie replay / pinning only. Passive + ethical: we only reuse
    /// values that our own session legitimately earned.
    Replay,
    /// Replay + active dummy-token attempts (Turnstile invisible).
    /// Lower success rate, slightly more aggressive. Explicit opt-in.
    Aggressive,
}

impl BypassLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Replay => "replay",
            Self::Aggressive => "aggressive",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "" | "none" | "off" | "disabled" => Ok(Self::None),
            "replay" | "pin" | "cookie" => Ok(Self::Replay),
            "aggressive" | "active" | "full" => Ok(Self::Aggressive),
            other => Err(format!("unknown antibot-bypass level: {other}")),
        }
    }

    /// `true` when the level allows passive cookie pinning.
    pub fn allows_replay(&self) -> bool {
        matches!(self, Self::Replay | Self::Aggressive)
    }

    /// `true` when the level allows Turnstile-style dummy attempts.
    pub fn allows_aggressive(&self) -> bool {
        matches!(self, Self::Aggressive)
    }
}

/// A vendor cookie the bypass path wants to pin for replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedCookie {
    pub vendor: &'static str,
    pub name: String,
    pub value: String,
    pub ttl_secs: u64,
}

/// Inspect Set-Cookie headers from a live response and classify any
/// high-signal antibot cookies into pinning candidates. `status` is
/// the HTTP status of the response; DataDome cookies are only pinned
/// from 4xx responses (the vendor's retry-loop pattern).
pub fn capture_from_headers(headers: &HeaderMap, status: u16) -> Vec<CapturedCookie> {
    let mut out = Vec::new();
    for raw in headers.get_all("set-cookie") {
        let Some(line) = raw.to_str().ok() else {
            continue;
        };
        // Parse just the leading `name=value` — attributes after `;` are
        // ignored here because we pin our own conservative TTL anyway.
        let (head, _attrs) = line.split_once(';').unwrap_or((line, ""));
        let Some((name_raw, value_raw)) = head.split_once('=') else {
            continue;
        };
        let name = name_raw.trim();
        let value = value_raw.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }

        // Akamai — only pin when the value looks like a solved-challenge
        // cookie (long opaque string). Blank or tilde-suffixed `~-1~` is
        // the "unsolved" form.
        if name == "_abck" && !is_akamai_unsolved(value) {
            out.push(CapturedCookie {
                vendor: "akamai",
                name: name.into(),
                value: value.into(),
                ttl_secs: AKAMAI_ABCK_TTL_SECS,
            });
            continue;
        }

        // DataDome — the vendor emits a fresh cookie in its retry loop.
        // Only pin on error statuses where it's meaningful to retry.
        if name == "datadome" && (status >= 400 || status == 0) {
            out.push(CapturedCookie {
                vendor: "datadome",
                name: name.into(),
                value: value.into(),
                ttl_secs: DATADOME_TTL_SECS,
            });
            continue;
        }

        // PerimeterX — `_px2`, `_px3`, `_pxhd` all carry score state.
        if is_perimeterx_name(name) {
            out.push(CapturedCookie {
                vendor: "perimeterx",
                name: name.into(),
                value: value.into(),
                ttl_secs: PERIMETERX_TTL_SECS,
            });
        }
    }
    out
}

/// Commit a batch of captured cookies into the pin store. Origin is
/// the origin string emitted by [`crate::antibot::origin_of`]. Returns
/// the number of successfully-pinned entries.
pub fn pin_captured(
    store: &dyn CookiePinStore,
    origin: &str,
    captured: &[CapturedCookie],
) -> usize {
    let mut n = 0;
    for c in captured {
        if store
            .pin(c.vendor, origin, &c.name, &c.value, c.ttl_secs)
            .is_ok()
        {
            n += 1;
        }
    }
    n
}

fn is_akamai_unsolved(value: &str) -> bool {
    // Akamai's pre-solve value ends with `~-1~-1~-1` or contains
    // `~-1~` prominently; post-solve values omit those markers.
    value.contains("~-1~-1") || value.len() < 32
}

fn is_perimeterx_name(name: &str) -> bool {
    match name {
        "_pxvid" | "_pxhd" | "_pxde" => true,
        n => n.starts_with("_px") && n.len() > 3 && n.as_bytes()[3].is_ascii_digit(),
    }
}

/// Result of a single dummy Turnstile attempt. Callers decide whether
/// to trust the outcome (log it, feed telemetry, gate retry loops).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnstileAttemptOutcome {
    /// No attempt made — either level forbids aggressive tricks or
    /// the sitekey wasn't extractable.
    NotAttempted(&'static str),
    /// Attempt was prepared; the caller is responsible for issuing the
    /// POST and observing the verdict.
    Prepared {
        sitekey: String,
        endpoint: &'static str,
        dummy_token: &'static str,
    },
}

/// The dummy token pattern. Chosen to be obviously synthetic so that
/// if we ever leak it downstream it's easy to grep.
pub const TURNSTILE_DUMMY_TOKEN: &str = "XXXX.DUMMY.TOKEN.XXXX";
pub const TURNSTILE_CHALLENGE_ENDPOINT: &str =
    "https://challenges.cloudflare.com/turnstile/v0/api.js";

/// Structured result of a Turnstile bypass attempt preparation.
#[derive(Debug, Clone)]
pub struct TurnstileAttempt {
    pub outcome: TurnstileAttemptOutcome,
}

/// Prepare a Turnstile invisible-widget dummy attempt. Returns a
/// [`TurnstileAttempt::Prepared`] when:
///
/// * `level` allows aggressive tricks.
/// * `sitekey` is present (extracted upstream from `data-sitekey`).
/// * `invisible_widget` is true (visible captcha isn't skippable).
///
/// Callers at the HTTP layer can issue the POST; we stay IO-free.
pub fn prepare_turnstile_attempt(
    level: BypassLevel,
    sitekey: Option<&str>,
    invisible_widget: bool,
) -> TurnstileAttempt {
    if !level.allows_aggressive() {
        return TurnstileAttempt {
            outcome: TurnstileAttemptOutcome::NotAttempted("level=<aggressive"),
        };
    }
    if !invisible_widget {
        return TurnstileAttempt {
            outcome: TurnstileAttemptOutcome::NotAttempted("widget_not_invisible"),
        };
    }
    let Some(key) = sitekey.filter(|s| !s.is_empty()) else {
        return TurnstileAttempt {
            outcome: TurnstileAttemptOutcome::NotAttempted("no_sitekey"),
        };
    };
    TurnstileAttempt {
        outcome: TurnstileAttemptOutcome::Prepared {
            sitekey: key.into(),
            endpoint: TURNSTILE_CHALLENGE_ENDPOINT,
            dummy_token: TURNSTILE_DUMMY_TOKEN,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn headers(set_cookie: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in set_cookie {
            h.append("set-cookie", v.parse().unwrap());
        }
        h
    }

    #[test]
    fn bypass_level_parses_and_defaults_to_none() {
        assert_eq!(BypassLevel::default(), BypassLevel::None);
        assert_eq!(BypassLevel::parse("none").unwrap(), BypassLevel::None);
        assert_eq!(BypassLevel::parse("replay").unwrap(), BypassLevel::Replay);
        assert_eq!(
            BypassLevel::parse("aggressive").unwrap(),
            BypassLevel::Aggressive
        );
        assert!(BypassLevel::parse("garbage").is_err());
        assert!(!BypassLevel::None.allows_replay());
        assert!(BypassLevel::Replay.allows_replay());
        assert!(!BypassLevel::Replay.allows_aggressive());
        assert!(BypassLevel::Aggressive.allows_aggressive());
    }

    #[test]
    fn capture_classifies_vendor_cookies() {
        let h = headers(&[
            "_abck=6A1B2C3D4E5F6A7B8C9D0123456789ABCDEF~challenge-solved~; Path=/",
            "datadome=abc123def456; Path=/; Max-Age=3600",
            "_px3=ABCDEF123456; Path=/",
            "unrelated=value",
        ]);
        let caps = capture_from_headers(&h, 403);
        let vendors: Vec<_> = caps.iter().map(|c| c.vendor).collect();
        assert!(vendors.contains(&"akamai"));
        assert!(vendors.contains(&"datadome"));
        assert!(vendors.contains(&"perimeterx"));
        assert_eq!(caps.len(), 3);
    }

    #[test]
    fn capture_rejects_unsolved_akamai_cookie() {
        let h = headers(&["_abck=short~-1~-1~-1"]);
        let caps = capture_from_headers(&h, 403);
        assert!(caps.is_empty());
    }

    #[test]
    fn capture_skips_datadome_on_2xx() {
        let h = headers(&["datadome=abc123def456; Path=/"]);
        let caps = capture_from_headers(&h, 200);
        assert!(caps.is_empty(), "only pin datadome on 4xx retry loop");
    }

    #[test]
    fn turnstile_attempt_requires_aggressive_level() {
        let a = prepare_turnstile_attempt(BypassLevel::None, Some("0x4AAA"), true);
        assert!(matches!(
            a.outcome,
            TurnstileAttemptOutcome::NotAttempted(_)
        ));
        let b = prepare_turnstile_attempt(BypassLevel::Replay, Some("0x4AAA"), true);
        assert!(matches!(
            b.outcome,
            TurnstileAttemptOutcome::NotAttempted(_)
        ));
        let c = prepare_turnstile_attempt(BypassLevel::Aggressive, Some("0x4AAA"), true);
        assert!(matches!(
            c.outcome,
            TurnstileAttemptOutcome::Prepared { .. }
        ));
    }

    #[test]
    fn turnstile_attempt_needs_sitekey_and_invisible_flag() {
        let no_key = prepare_turnstile_attempt(BypassLevel::Aggressive, None, true);
        assert!(matches!(
            no_key.outcome,
            TurnstileAttemptOutcome::NotAttempted("no_sitekey")
        ));
        let visible = prepare_turnstile_attempt(BypassLevel::Aggressive, Some("0x4AAA"), false);
        assert!(matches!(
            visible.outcome,
            TurnstileAttemptOutcome::NotAttempted("widget_not_invisible")
        ));
    }

    #[test]
    fn pin_captured_writes_to_store() {
        use super::super::cookie_pin::InMemoryCookiePinStore;
        let store = InMemoryCookiePinStore::new();
        let h = headers(&[
            "_abck=6A1B2C3D4E5F6A7B8C9D0123456789ABCDEF~solved~; Path=/",
            "datadome=abc123def456; Path=/",
        ]);
        let caps = capture_from_headers(&h, 403);
        let n = pin_captured(&store, "https://a.test", &caps);
        assert_eq!(n, 2);
        let abck = store
            .get_pinned("akamai", "https://a.test", "_abck")
            .unwrap();
        assert!(abck.is_some());
    }
}
