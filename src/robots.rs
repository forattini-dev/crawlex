// robots.txt fetch/cache/evaluate per host.
// Uses `texting_robots` for RFC-compliant parsing. The Content-Signal
// extension (Cloudflare-pushed) is parsed in-house since `texting_robots`
// drops unknown directives.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use texting_robots::Robot;
use url::Url;

use crate::Result;

/// What an operator is using a crawl for. Maps 1:1 onto the tokens defined
/// in Cloudflare's Content-Signals draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    Search,
    AiInput,
    AiTrain,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::AiInput => "ai-input",
            Self::AiTrain => "ai-train",
        }
    }

    pub fn all() -> [Purpose; 3] {
        [Self::Search, Self::AiInput, Self::AiTrain]
    }
}

impl std::str::FromStr for Purpose {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "search" => Ok(Self::Search),
            "ai-input" | "ai_input" => Ok(Self::AiInput),
            "ai-train" | "ai_train" => Ok(Self::AiTrain),
            other => Err(format!("unknown crawl purpose: {other}")),
        }
    }
}

/// Per-purpose permit/deny flags parsed from a `Content-Signal:` directive.
/// Absent directive => all three permitted (open by default, matches spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentSignal {
    pub search: bool,
    pub ai_input: bool,
    pub ai_train: bool,
}

impl Default for ContentSignal {
    fn default() -> Self {
        Self {
            search: true,
            ai_input: true,
            ai_train: true,
        }
    }
}

impl ContentSignal {
    /// Single interface the rest of the codebase pokes at.
    pub fn permits(self, purpose: Purpose) -> bool {
        match purpose {
            Purpose::Search => self.search,
            Purpose::AiInput => self.ai_input,
            Purpose::AiTrain => self.ai_train,
        }
    }

    /// True when *every* declared purpose is denied. Caller uses this to
    /// abort the run before the first fetch.
    pub fn fully_denies(self, declared: &[Purpose]) -> bool {
        !declared.is_empty() && declared.iter().all(|p| !self.permits(*p))
    }
}

/// Parse `Content-Signal:` directives from a robots.txt body for the given
/// user-agent. The directive lives inside the same `User-agent:` block as
/// the existing Allow/Disallow rules — most-specific match wins, falling
/// back to `*`. The value is a comma-separated list of `<name>=<yes|no>`
/// pairs (per the Cloudflare draft).
pub fn parse_content_signal(body: &str, user_agent: &str) -> ContentSignal {
    let ua_lc = user_agent.to_ascii_lowercase();
    let mut current_uas: Vec<String> = Vec::new();
    let mut last_was_agent = false;

    // best-match signal seen so far: (specificity, signal)
    // specificity 2 = exact UA match, 1 = `*`, 0 = none
    let mut best: (u8, ContentSignal) = (0, ContentSignal::default());

    for raw_line in body.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        let field_lc = field.trim().to_ascii_lowercase();
        let value = value.trim();

        match field_lc.as_str() {
            "user-agent" => {
                if !last_was_agent {
                    current_uas.clear();
                }
                current_uas.push(value.to_ascii_lowercase());
                last_was_agent = true;
            }
            "content-signal" => {
                last_was_agent = false;
                let sig = parse_signal_value(value);
                for ua in &current_uas {
                    let spec = if ua == &ua_lc {
                        2
                    } else if ua == "*" {
                        1
                    } else {
                        0
                    };
                    if spec > best.0 {
                        best = (spec, sig);
                    }
                }
            }
            _ => {
                last_was_agent = false;
            }
        }
    }

    best.1
}

fn parse_signal_value(value: &str) -> ContentSignal {
    // Start permissive; a token toggles a single field.
    let mut sig = ContentSignal::default();
    for piece in value.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (name, verdict) = match piece.split_once('=') {
            Some((n, v)) => (n.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase()),
            None => (piece.to_ascii_lowercase(), String::from("yes")),
        };
        let allowed = matches!(verdict.as_str(), "yes" | "y" | "true" | "1" | "allow");
        match name.as_str() {
            "search" => sig.search = allowed,
            "ai-input" | "ai_input" => sig.ai_input = allowed,
            "ai-train" | "ai_train" => sig.ai_train = allowed,
            _ => {}
        }
    }
    sig
}

pub struct RobotsCache {
    ttl: Duration,
    inner: DashMap<String, (Instant, Arc<Option<Robot>>, ContentSignal)>,
}

impl RobotsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: DashMap::new(),
        }
    }

    pub fn check(&self, url: &Url, user_agent: &str) -> Option<bool> {
        let host = url.host_str()?;
        let entry = self.inner.get(host)?;
        let (inserted, robot, _signal) = entry.value();
        if inserted.elapsed() > self.ttl {
            return None;
        }
        robot
            .as_ref()
            .as_ref()
            .map(|r| r.allowed(url.as_str()))
            .or(Some(true))
            .map(|allowed| {
                let _ = user_agent;
                allowed
            })
    }

    /// Look up the cached Content-Signal for a host. `None` when nothing
    /// has been stored yet (caller treats as "permissive" — matches the
    /// spec's default-open stance).
    pub fn content_signal(&self, host: &str) -> Option<ContentSignal> {
        let entry = self.inner.get(host)?;
        let (inserted, _, signal) = entry.value();
        if inserted.elapsed() > self.ttl {
            return None;
        }
        Some(*signal)
    }

    pub fn store(&self, host: &str, txt: Option<&str>, user_agent: &str) -> Result<()> {
        let robot = txt.and_then(|t| Robot::new(user_agent, t.as_bytes()).ok());
        let signal = txt
            .map(|t| parse_content_signal(t, user_agent))
            .unwrap_or_default();
        self.inner.insert(
            host.to_string(),
            (Instant::now(), Arc::new(robot), signal),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_permissive() {
        let s = ContentSignal::default();
        for p in Purpose::all() {
            assert!(s.permits(p));
        }
        assert!(!s.fully_denies(&Purpose::all()));
    }

    #[test]
    fn parse_permissive_body() {
        let body = "User-agent: *\nDisallow:\n";
        let s = parse_content_signal(body, "crawlex");
        assert_eq!(s, ContentSignal::default());
    }

    #[test]
    fn parse_fully_deny() {
        let body = "User-agent: *\nContent-Signal: search=no, ai-input=no, ai-train=no\n";
        let s = parse_content_signal(body, "crawlex");
        assert!(!s.search && !s.ai_input && !s.ai_train);
        assert!(s.fully_denies(&Purpose::all()));
    }

    #[test]
    fn parse_mixed_denies_ai_train_only() {
        // Cloudflare spec example — search ok, ai-train forbidden.
        let body =
            "User-agent: *\nContent-Signal: search=yes, ai-input=yes, ai-train=no\nDisallow:\n";
        let s = parse_content_signal(body, "crawlex");
        assert!(s.permits(Purpose::Search));
        assert!(s.permits(Purpose::AiInput));
        assert!(!s.permits(Purpose::AiTrain));
        assert!(!s.fully_denies(&Purpose::all()));
        assert!(s.fully_denies(&[Purpose::AiTrain]));
    }

    #[test]
    fn ua_specific_overrides_wildcard() {
        let body = "User-agent: *\nContent-Signal: search=no\n\
                    User-agent: crawlex\nContent-Signal: search=yes\n";
        let s = parse_content_signal(body, "crawlex");
        assert!(s.permits(Purpose::Search));

        let other = parse_content_signal(body, "otherbot");
        assert!(!other.permits(Purpose::Search));
    }

    #[test]
    fn stacked_user_agents_share_signal() {
        let body = "User-agent: alpha\nUser-agent: beta\nContent-Signal: ai-train=no\n";
        let a = parse_content_signal(body, "alpha");
        let b = parse_content_signal(body, "beta");
        assert!(!a.permits(Purpose::AiTrain));
        assert!(!b.permits(Purpose::AiTrain));
    }

    #[test]
    fn purpose_from_str_roundtrip() {
        for p in Purpose::all() {
            let parsed: Purpose = p.as_str().parse().unwrap();
            assert_eq!(parsed, p);
        }
        assert!("nope".parse::<Purpose>().is_err());
    }

    #[test]
    fn cache_stores_and_returns_signal() {
        let cache = RobotsCache::new(Duration::from_secs(60));
        let body = "User-agent: *\nContent-Signal: search=no, ai-input=no, ai-train=no\n";
        cache.store("example.com", Some(body), "crawlex").unwrap();
        let sig = cache.content_signal("example.com").expect("stored");
        assert!(sig.fully_denies(&Purpose::all()));
    }

    #[test]
    fn cache_missing_host_returns_none() {
        let cache = RobotsCache::new(Duration::from_secs(60));
        assert!(cache.content_signal("nope.example").is_none());
    }
}
