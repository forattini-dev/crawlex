//! Ad/tracker URL gate (slice 20).
//!
//! Bundles a baseline of well-known ad/tracker hostnames via
//! `include_str!`. A local override file (populated by
//! `crawlex update-blocklist` from EasyList) is merged at runtime. Match
//! is suffix-based on hostname labels — bundling `doubleclick.net`
//! blocks `pagead.l.doubleclick.net` too.
//!
//! Opt-in only: `SpiderConfig::ad_block` defaults to `false`. Both the
//! HTTP fetch path and the browser render path consult [`is_blocked`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use url::Url;

const BASELINE_RAW: &str = include_str!("baseline.txt");

/// Holds the parsed domain set. Cheap to share — clone is a HashSet
/// clone but typical callers stash this in an `Arc` and read forever.
#[derive(Debug, Clone, Default)]
pub struct BlockList {
    domains: HashSet<String>,
}

impl BlockList {
    /// Empty list — every URL passes. Useful in tests.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Just the embedded baseline.
    pub fn baseline() -> Self {
        let mut s = Self::default();
        s.extend_from_str(BASELINE_RAW);
        s
    }

    /// Baseline + local override file (if it exists and is readable).
    /// The override file simply unions in — there is no negation syntax;
    /// once a domain is in the set it is blocked.
    pub fn baseline_with_override(override_path: &Path) -> Self {
        let mut s = Self::baseline();
        if let Ok(body) = std::fs::read_to_string(override_path) {
            s.extend_from_str(&body);
        }
        s
    }

    /// Parse `body` line-by-line, stripping `#`-comments and blanks.
    /// Lines that look like EasyList domain rules (`||foo.bar^`) are
    /// also accepted so the override file can be the raw EasyList dump.
    pub fn extend_from_str(&mut self, body: &str) {
        for line in body.lines() {
            if let Some(d) = parse_line(line) {
                self.domains.insert(d);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn contains_exact(&self, host: &str) -> bool {
        self.domains.contains(host)
    }

    /// Suffix-match `host` against the list. `ads.example.com` is
    /// blocked if any of `ads.example.com`, `example.com`, `com` is in
    /// the set. (`com` is filtered out in parsing so we never block
    /// every dotcom — see [`parse_line`].)
    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        if self.domains.contains(&host) {
            return true;
        }
        // Walk label suffixes: a.b.c -> b.c -> c.
        let mut rest = host.as_str();
        while let Some(idx) = rest.find('.') {
            rest = &rest[idx + 1..];
            if rest.is_empty() {
                break;
            }
            if self.domains.contains(rest) {
                return true;
            }
        }
        false
    }

    /// Convenience: parse `url`'s host and consult the list.
    pub fn matches_url(&self, url: &str) -> bool {
        match Url::parse(url) {
            Ok(u) => u.host_str().map(|h| self.matches_host(h)).unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// Public alias for [`parse_line`] — used by the
/// `crawlex update-blocklist` CLI to serialise normalised domains into
/// the override file.
pub fn extract_domain(line: &str) -> Option<String> {
    parse_line(line)
}

/// One line of the bundled baseline or the override file. Returns
/// `None` for blanks, comments, single-label TLDs (`com`), and lines we
/// can't interpret.
fn parse_line(line: &str) -> Option<String> {
    let trimmed = line.split('#').next().unwrap_or("").trim();
    if trimmed.is_empty() {
        return None;
    }
    // EasyList: `||example.com^` (with optional trailing modifiers).
    // We accept just the domain part. Lines with selector syntax
    // (`##`, `#@#`, `#?#`) or with options (`$third-party`) are kept
    // only if the domain prefix is clean.
    let candidate = if let Some(rest) = trimmed.strip_prefix("||") {
        // Stop at the first non-domain char (`^`, `/`, `$`, `*`).
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        &rest[..end]
    } else if trimmed.starts_with("0.0.0.0 ") || trimmed.starts_with("127.0.0.1 ") {
        // hosts-file format. Take field 2.
        trimmed.split_whitespace().nth(1).unwrap_or("")
    } else if trimmed.starts_with('!') || trimmed.starts_with('[') {
        // EasyList comment / header.
        return None;
    } else if trimmed.contains("##") || trimmed.contains("#@#") || trimmed.contains("#?#") {
        // Cosmetic / element-hide rule — the part before `##` names a
        // page, not a request target. Skip.
        return None;
    } else {
        // Bare host (our baseline format) — accept domain chars and a
        // trailing `/path` fragment which we discard.
        let domain_end = trimmed
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'))
            .unwrap_or(trimmed.len());
        &trimmed[..domain_end]
    };
    let host = candidate.trim_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        // Reject single-label entries — they would catch every TLD.
        return None;
    }
    Some(host)
}

/// Default user-config path for the override file. Honours
/// `CRAWLEX_BLOCKLIST` env var first, then `$XDG_CONFIG_HOME/crawlex`,
/// then `$HOME/.config/crawlex`, then `./.crawlex` as a last resort.
pub fn default_override_path() -> PathBuf {
    if let Ok(p) = std::env::var("CRAWLEX_BLOCKLIST") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("crawlex").join("blocklist.txt")
}

/// Process-wide shared list. First access loads baseline + the override
/// at [`default_override_path`]. Tests that need a clean slate should
/// build a [`BlockList`] explicitly rather than touching this.
pub fn global() -> &'static BlockList {
    static GLOBAL: OnceLock<BlockList> = OnceLock::new();
    GLOBAL.get_or_init(|| BlockList::baseline_with_override(&default_override_path()))
}

/// `is_blocked(url) -> bool` — slice 20 acceptance hook. Returns
/// `false` for unparseable URLs (callers always treat "unknown" as
/// "let it through"; the gate is opt-in anyway).
pub fn is_blocked(url: &str) -> bool {
    global().matches_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_parses_nonempty() {
        let list = BlockList::baseline();
        assert!(
            list.len() > 100,
            "baseline should contain hundreds of entries, got {}",
            list.len()
        );
        // Known canonicals must be present.
        assert!(list.contains_exact("doubleclick.net"));
        assert!(list.contains_exact("google-analytics.com"));
    }

    #[test]
    fn matches_exact_host() {
        let list = BlockList::baseline();
        assert!(list.matches_host("doubleclick.net"));
    }

    #[test]
    fn matches_subdomain_via_suffix() {
        let list = BlockList::baseline();
        // `doubleclick.net` is in the baseline; any subdomain should
        // be blocked.
        assert!(list.matches_host("pagead.l.doubleclick.net"));
        assert!(list.matches_host("foo.bar.baz.doubleclick.net"));
    }

    #[test]
    fn does_not_match_unrelated_host() {
        let list = BlockList::baseline();
        assert!(!list.matches_host("example.com"));
        assert!(!list.matches_host("rust-lang.org"));
    }

    #[test]
    fn matches_url_strips_scheme_and_path() {
        let list = BlockList::baseline();
        assert!(list.matches_url("https://pagead.l.doubleclick.net/some/path?x=1"));
        assert!(!list.matches_url("https://example.com/"));
    }

    #[test]
    fn override_unions_with_baseline() {
        let mut list = BlockList::baseline();
        list.extend_from_str("# override\nads.private.example.test\n");
        assert!(list.matches_host("ads.private.example.test"));
        // Subdomain of an override entry also matches.
        assert!(list.matches_host("foo.ads.private.example.test"));
        // Baseline still works.
        assert!(list.matches_host("doubleclick.net"));
    }

    #[test]
    fn parses_easylist_domain_rules() {
        let mut list = BlockList::empty();
        list.extend_from_str(
            r#"
[Adblock Plus 2.0]
! Title: Test list
||evil-tracker.test^
||third.party.test^$third-party
example.test##.banner
||bad.test$image
"#,
        );
        assert!(list.matches_host("evil-tracker.test"));
        assert!(list.matches_host("third.party.test"));
        assert!(list.matches_host("bad.test"));
        // Cosmetic rules are NOT domain rules — we shouldn't accidentally
        // block the cosmetic target.
        assert!(!list.matches_host("example.test"));
    }

    #[test]
    fn parses_hosts_file_format() {
        let mut list = BlockList::empty();
        list.extend_from_str("0.0.0.0 evil.test\n127.0.0.1 other.test\n");
        assert!(list.matches_host("evil.test"));
        assert!(list.matches_host("other.test"));
    }

    #[test]
    fn rejects_single_label_entries() {
        let mut list = BlockList::empty();
        list.extend_from_str("com\norg\n");
        assert!(!list.matches_host("example.com"));
        assert!(!list.matches_host("rust-lang.org"));
    }

    #[test]
    fn override_round_trip_via_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocklist.txt");
        std::fs::write(&path, "||rt-override.test^\n").unwrap();
        let list = BlockList::baseline_with_override(&path);
        assert!(list.matches_host("rt-override.test"));
        assert!(list.matches_host("x.y.rt-override.test"));
        // Baseline is preserved.
        assert!(list.matches_host("doubleclick.net"));
    }

    #[test]
    fn case_and_trailing_dot_normalised() {
        let list = BlockList::baseline();
        assert!(list.matches_host("DoubleClick.NET"));
        assert!(list.matches_host("doubleclick.net."));
    }
}
