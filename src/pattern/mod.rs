//! URL match patterns: globs that compile to anchored regexes.
//!
//! Grammar:
//!   * `*`  — matches any chars except `/`
//!   * `**` — matches any chars including `/`
//!   * `?`  — matches exactly one char except `/`
//!   * any other char is matched literally (regex metachars are escaped)
//!
//! Compilation happens once at config-load and yields a `regex::Regex` so the
//! hot path is identical to the legacy regex form. Globs are anchored at both
//! ends — callers should add explicit `**` if they want substring matching.

use regex::Regex;

/// A compiled glob, pre-converted to an anchored regex.
#[derive(Debug, Clone)]
pub struct Glob {
    src: String,
    re: Regex,
}

impl Glob {
    /// Compile a glob into an anchored regex. Returns the regex compile error
    /// only if the translated pattern is somehow invalid — under the grammar
    /// above this should never happen for arbitrary input.
    pub fn compile(pat: &str) -> Result<Self, regex::Error> {
        let re_src = glob_to_regex(pat);
        let re = Regex::new(&re_src)?;
        Ok(Self {
            src: pat.to_string(),
            re,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.src
    }

    pub fn as_regex(&self) -> &Regex {
        &self.re
    }

    pub fn matches(&self, s: &str) -> bool {
        self.re.is_match(s)
    }
}

/// Auto-detect: if `pat` contains characters that only make sense as regex
/// metachars (`^$()[]{}|+\`), treat it as a regex; otherwise treat it as a
/// glob. `*` and `?` are ambiguous and resolve to glob — the common case.
///
/// Callers who want unambiguous behavior can use [`Glob::compile`] directly
/// or hand the engine a pre-compiled [`Regex`].
pub fn compile_pattern(pat: &str) -> Result<Regex, regex::Error> {
    if looks_like_regex(pat) {
        Regex::new(pat)
    } else {
        Glob::compile(pat).map(|g| g.re)
    }
}

fn looks_like_regex(pat: &str) -> bool {
    let mut chars = pat.chars();
    while let Some(c) = chars.next() {
        match c {
            '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '+' | '\\' => return true,
            _ => {}
        }
    }
    false
}

/// Translate a glob to an anchored regex source string.
fn glob_to_regex(pat: &str) -> String {
    let bytes = pat.as_bytes();
    let mut out = String::with_capacity(pat.len() + 8);
    out.push('^');

    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(".*");
                    i += 2;
                    // Eat a trailing `/` after `**/` so `**/foo` matches `foo`
                    // at the root as well as nested.
                    if i < bytes.len() && bytes[i] == b'/' {
                        out.push_str("/?");
                        i += 1;
                    }
                } else {
                    out.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                out.push_str("[^/]");
                i += 1;
            }
            // Regex metachars that aren't part of our glob alphabet — escape.
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(c);
                i += 1;
            }
            _ => {
                out.push(c);
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

    fn m(pat: &str, s: &str) -> bool {
        Glob::compile(pat).expect("glob compiles").matches(s)
    }

    #[test]
    fn star_does_not_cross_slash() {
        assert!(m("/blog/*", "/blog/post"));
        assert!(!m("/blog/*", "/blog/2026/post"));
    }

    #[test]
    fn double_star_crosses_slash() {
        assert!(m("/blog/**", "/blog/post"));
        assert!(m("/blog/**", "/blog/2026/05/post"));
        assert!(m("/blog/**", "/blog/"));
    }

    #[test]
    fn exact_match() {
        assert!(m("/about", "/about"));
        assert!(!m("/about", "/about/team"));
    }

    #[test]
    fn leading_double_star() {
        assert!(m("**/post", "/post"));
        assert!(m("**/post", "/blog/post"));
        assert!(m("**/post", "/a/b/c/post"));
    }

    #[test]
    fn trailing_double_star() {
        assert!(m("/docs/**", "/docs/intro"));
        assert!(m("/docs/**", "/docs/"));
        // `/**` requires at least one preceding segment.
        assert!(!m("/docs/**", "/other/page"));
    }

    #[test]
    fn question_mark_matches_one_non_slash() {
        assert!(m("/p?ge", "/page"));
        assert!(!m("/p?ge", "/paige"));
        assert!(!m("/p?ge", "/p/ge"));
    }

    #[test]
    fn regex_metachars_in_glob_are_literal() {
        // Pattern has `.` which is a regex wildcard, but as a glob it is a
        // literal dot.
        assert!(m("/file.txt", "/file.txt"));
        assert!(!m("/file.txt", "/fileXtxt"));
    }

    #[test]
    fn auto_detect_picks_glob_for_star() {
        let re = compile_pattern("/blog/*").expect("compiles");
        assert!(re.is_match("/blog/post"));
        assert!(!re.is_match("/blog/a/b"));
    }

    #[test]
    fn auto_detect_picks_regex_for_metachars() {
        // `^/api/(v1|v2)/` is unambiguously a regex.
        let re = compile_pattern("^/api/(v1|v2)/").expect("compiles");
        assert!(re.is_match("/api/v1/"));
        assert!(re.is_match("/api/v2/users"));
        assert!(!re.is_match("/api/v3/"));
    }

    #[test]
    fn exclude_over_include_precedence() {
        // The precedence rule lives in callers; here we just confirm a glob
        // pair can express it.
        let inc = Glob::compile("/blog/**").unwrap();
        let exc = Glob::compile("/blog/drafts/**").unwrap();
        let url = "/blog/drafts/secret";
        assert!(inc.matches(url));
        assert!(exc.matches(url));
        // Caller logic: deny wins.
        let allowed = inc.matches(url) && !exc.matches(url);
        assert!(!allowed);
    }
}
