//! URL include/exclude patterns for `link_filter`.
//!
//! Supports two backends behind a single [`Pattern`] surface:
//! * [`Pattern::Glob`] — shell-style: `*` matches any chars except `/`;
//!   `**` matches any chars including `/`. Compiled once to an anchored
//!   regex so the hot path stays a single `Regex::is_match`.
//! * [`Pattern::Regex`] — legacy form. Auto-detected by the `re:` prefix in
//!   [`Pattern::compile_auto`]; matched as written (not anchored).
//!
//! `Glob` patterns are anchored at both ends; precedence (exclude wins over
//! include) is enforced by the caller (`filter_links`) — patterns themselves
//! are pure predicates.

use regex::Regex;

/// Shell-style glob pattern compiled to an anchored regex.
#[derive(Debug, Clone)]
pub struct Glob {
    re: Regex,
    source: String,
}

impl Glob {
    /// Compile a glob string.
    pub fn compile(pat: &str) -> Result<Self, regex::Error> {
        let re_src = glob_to_regex(pat);
        Ok(Self {
            re: Regex::new(&re_src)?,
            source: pat.to_string(),
        })
    }

    pub fn matches(&self, s: &str) -> bool {
        self.re.is_match(s)
    }

    pub fn matches_url(&self, u: &url::Url) -> bool {
        self.matches(u.as_str()) || self.matches(u.path())
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Unified include/exclude predicate. Either a [`Glob`] or a raw [`Regex`].
#[derive(Debug, Clone)]
pub enum Pattern {
    Regex(Regex),
    Glob(Glob),
}

impl Pattern {
    pub fn glob(s: &str) -> Result<Self, regex::Error> {
        Glob::compile(s).map(Pattern::Glob)
    }

    pub fn regex(r: Regex) -> Self {
        Pattern::Regex(r)
    }

    /// Auto-detect form: a `re:` prefix selects [`Pattern::Regex`]; anything
    /// else is treated as a glob. This is the escape hatch promised in
    /// `docs/reference/config.md` so existing regex recipes keep working.
    pub fn compile_auto(s: &str) -> Result<Self, regex::Error> {
        if let Some(rest) = s.strip_prefix("re:") {
            Ok(Pattern::Regex(Regex::new(rest)?))
        } else {
            Self::glob(s)
        }
    }

    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Pattern::Regex(r) => r.is_match(s),
            Pattern::Glob(g) => g.matches(s),
        }
    }
}

impl From<Regex> for Pattern {
    fn from(r: Regex) -> Self {
        Pattern::Regex(r)
    }
}

impl From<Glob> for Pattern {
    fn from(g: Glob) -> Self {
        Pattern::Glob(g)
    }
}

/// Translate glob syntax into an anchored regex source string.
///
/// Grammar:
/// * `**` → `.*` (crosses `/`)
/// * `*`  → `[^/]*` (does not cross `/`)
/// * `?`  → `[^/]` (single non-`/` char)
/// * regex metacharacters are escaped
fn glob_to_regex(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len() + 4);
    out.push('^');
    let bytes = pat.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(".*");
                    i += 2;
                } else {
                    out.push_str("[^/]*");
                    i += 1;
                }
            }
            b'?' => {
                out.push_str("[^/]");
                i += 1;
            }
            b'.' | b'+' | b'(' | b')' | b'|' | b'^' | b'$' | b'{' | b'}' | b'[' | b']'
            | b'\\' => {
                out.push('\\');
                out.push(c as char);
                i += 1;
            }
            _ => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(p: &str) -> Glob {
        Glob::compile(p).unwrap()
    }

    #[test]
    fn star_does_not_cross_slash() {
        let p = g("/blog/*");
        assert!(p.matches("/blog/post"));
        assert!(!p.matches("/blog/2025/post"));
    }

    #[test]
    fn double_star_crosses_slash() {
        let p = g("/blog/**");
        assert!(p.matches("/blog/post"));
        assert!(p.matches("/blog/2025/post"));
        assert!(p.matches("/blog/"));
    }

    #[test]
    fn exact_match() {
        let p = g("/about");
        assert!(p.matches("/about"));
        assert!(!p.matches("/about/team"));
        assert!(!p.matches("/about?x=1"));
    }

    #[test]
    fn leading_double_star() {
        let p = g("**/post.html");
        assert!(p.matches("/a/b/post.html"));
        assert!(p.matches("/post.html"));
        assert!(!p.matches("/post.htmlx"));
    }

    #[test]
    fn trailing_double_star() {
        let p = g("/api/**");
        assert!(p.matches("/api/"));
        assert!(p.matches("/api/v1/users"));
        assert!(!p.matches("/apix"));
    }

    #[test]
    fn question_mark_single_char() {
        let p = g("/p?ge");
        assert!(p.matches("/page"));
        assert!(!p.matches("/pge"));
        assert!(!p.matches("/p/ge"));
    }

    #[test]
    fn meta_chars_escaped() {
        let p = g("/v1.0/(beta)");
        assert!(p.matches("/v1.0/(beta)"));
        assert!(!p.matches("/v1x0/(beta)"));
    }

    #[test]
    fn auto_detect_regex_prefix() {
        let p = Pattern::compile_auto("re:/blog/\\d+").unwrap();
        assert!(p.is_match("/blog/123"));
        assert!(!p.is_match("/blog/x"));
    }

    #[test]
    fn auto_detect_glob_default() {
        let p = Pattern::compile_auto("/blog/**").unwrap();
        assert!(p.is_match("/blog/post"));
        assert!(p.is_match("/blog/2025/post"));
    }
}
