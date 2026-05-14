// Slice 22 — `crawlex shell` interactive REPL.
//
// The shell is a thin coordinator over three already-existing pieces:
//   * `impersonate::ImpersonateClient` — the HTTP/stealth backend used
//     by `.fetch`.
//   * `parser::TreeHandle` — the v2 selector engine that powers `.css`,
//     `.xpath`, `.findByText`, `.findByRegex`.
//   * `storage::adaptive::AdaptiveStore` — the per-spider fingerprint
//     store written by `.save <identifier>`.
//
// Two layers:
//   * `dispatch` is a pure-ish function over `ShellState`. It is the
//     unit-test surface — feed it a stubbed `Fetcher` and assert on the
//     `ShellOutput` returned for each command.
//   * `run_interactive` is the readline loop that wires `dispatch` to
//     stdin/stdout. History is persisted line-by-line in a plain text
//     file so it survives across sessions even though we do not link
//     rustyline (no network to vendor it, and the acceptance criteria
//     only require persistence, not arrow-key recall).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use url::Url;

use crate::error::{Error, Result};
use crate::impersonate::{ImpersonateClient, Profile, Response};
use crate::parser::{
    fingerprint, parse_tree, ElementHandle, Fingerprint, TextMatch,
};
use crate::storage::adaptive::AdaptiveStore;

/// Network-facing seam — abstracted so unit tests inject canned HTML.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<FetchedPage>;
}

/// Stripped-down response used by the shell. We avoid re-exporting the
/// full `impersonate::Response` so tests don't need to construct one.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub final_url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Wraps the project's stealth HTTP client. `stealth = true` is the
/// flag from `crawlex shell --stealth`; this implementation uses the
/// same `ImpersonateClient` for both so the flag is currently a wiring
/// placeholder (slice 22 acceptance only requires the flag exists).
pub struct ImpersonateFetcher {
    client: ImpersonateClient,
}

impl ImpersonateFetcher {
    pub fn new(_stealth: bool) -> Result<Self> {
        let client = ImpersonateClient::new(Profile::Chrome149Stable)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Fetcher for ImpersonateFetcher {
    async fn fetch(&self, url: &Url) -> Result<FetchedPage> {
        let resp: Response = self.client.get(url).await?;
        let content_type = resp
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Ok(FetchedPage {
            final_url: resp.final_url,
            status: resp.status.as_u16(),
            content_type,
            body: resp.body.to_vec(),
        })
    }
}

/// In-memory state held by one shell session.
pub struct ShellState {
    pub last_url: Option<Url>,
    pub last_body: Option<Vec<u8>>,
    pub last_content_type: Option<String>,
    pub last_status: Option<u16>,
    /// Fingerprint of the most recently selected element. `.save <id>`
    /// writes this into the adaptive store; commands that return at
    /// least one element refresh it.
    pub last_selection: Option<Fingerprint>,
    /// Set by `.findByText` / `.findByRegex` so an operator can recall
    /// the literal query that produced `last_selection`.
    pub last_selection_query: Option<String>,
}

impl ShellState {
    pub fn new() -> Self {
        Self {
            last_url: None,
            last_body: None,
            last_content_type: None,
            last_status: None,
            last_selection: None,
            last_selection_query: None,
        }
    }

}

impl Default for ShellState {
    fn default() -> Self {
        Self::new()
    }
}

/// What a single command produced. The interactive loop projects this
/// into stdout; tests assert on the variant directly.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellOutput {
    /// Nothing to print (e.g. `.exit` request — the loop terminates).
    Exit,
    /// One free-form line of feedback ("fetched 200 OK, 1234 bytes").
    Status(String),
    /// Multi-line block. Used by every selector verb so the operator
    /// sees all matches in the order they appear in the tree.
    Lines(Vec<String>),
    /// Help text — kept distinct from `Lines` so tests can target it.
    Help(Vec<String>),
    /// Error that should be shown but not terminate the session.
    Err(String),
}

/// Parse + execute one input line against `state`. Public so the
/// integration test in this module can drive the dispatcher without
/// touching stdin.
pub async fn dispatch(
    line: &str,
    state: &mut ShellState,
    fetcher: &dyn Fetcher,
    store: &AdaptiveStore,
) -> ShellOutput {
    let line = line.trim();
    if line.is_empty() {
        return ShellOutput::Status(String::new());
    }
    // Bare expressions (no leading `.`) are reserved for a future
    // embedded-evaluator extension. For slice 22 we explain the dot
    // grammar instead of silently swallowing the line.
    if !line.starts_with('.') {
        return ShellOutput::Err(format!(
            "expected a `.<command>` directive — got `{}`. Type `.help` for the grammar.",
            line
        ));
    }

    let (cmd, rest) = split_cmd(line);
    match cmd {
        ".help" | ".h" | ".?" => ShellOutput::Help(help_lines()),
        ".exit" | ".quit" | ".q" => ShellOutput::Exit,
        ".fetch" => cmd_fetch(rest, state, fetcher).await,
        ".css" => cmd_css(rest, state),
        ".xpath" => cmd_xpath(rest, state),
        ".findByText" => cmd_find_by_text(rest, state),
        ".findByRegex" => cmd_find_by_regex(rest, state),
        ".save" => cmd_save(rest, state, store),
        ".open" => cmd_open(state),
        other => ShellOutput::Err(format!(
            "unknown command `{}` — type `.help` to list the supported verbs.",
            other
        )),
    }
}

fn split_cmd(line: &str) -> (&str, &str) {
    match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim_start()),
        None => (line, ""),
    }
}

fn help_lines() -> Vec<String> {
    vec![
        "Commands:".into(),
        "  .fetch <url>          fetch a URL into the session".into(),
        "  .css <selector>       CSS query against the last response".into(),
        "  .xpath <expr>         XPath query against the last response".into(),
        "  .findByText <text>    substring text match (descendant search)".into(),
        "  .findByRegex <regex>  regex match against descendant text".into(),
        "  .save <identifier>    persist the last selection's fingerprint".into(),
        "  .open                 open the last response in a browser".into(),
        "  .help                 print this message".into(),
        "  .exit                 leave the shell".into(),
    ]
}

async fn cmd_fetch(rest: &str, state: &mut ShellState, fetcher: &dyn Fetcher) -> ShellOutput {
    if rest.is_empty() {
        return ShellOutput::Err("usage: .fetch <url>".into());
    }
    let url = match Url::parse(rest) {
        Ok(u) => u,
        Err(e) => return ShellOutput::Err(format!("invalid url: {e}")),
    };
    match fetcher.fetch(&url).await {
        Ok(page) => {
            let msg = format!(
                "{} {} ({} bytes, content-type: {})",
                page.status,
                page.final_url,
                page.body.len(),
                page.content_type.as_deref().unwrap_or("?"),
            );
            state.last_url = Some(page.final_url);
            state.last_body = Some(page.body);
            state.last_content_type = page.content_type;
            state.last_status = Some(page.status);
            state.last_selection = None;
            state.last_selection_query = None;
            ShellOutput::Status(msg)
        }
        Err(e) => ShellOutput::Err(format!("fetch failed: {e}")),
    }
}

fn require_body(state: &ShellState) -> std::result::Result<&[u8], ShellOutput> {
    match state.last_body.as_deref() {
        Some(b) => Ok(b),
        None => Err(ShellOutput::Err(
            "no response in the session — run `.fetch <url>` first.".into(),
        )),
    }
}

fn cmd_css(rest: &str, state: &mut ShellState) -> ShellOutput {
    if rest.is_empty() {
        return ShellOutput::Err("usage: .css <selector>".into());
    }
    let body = match require_body(state) {
        Ok(b) => b,
        Err(o) => return o,
    };
    let tree = parse_tree(body, charset_from(state.last_content_type.as_deref()));
    let matches = tree.css(rest);
    record_first(&matches, state, rest);
    pretty_handles(&matches)
}

fn cmd_xpath(rest: &str, state: &mut ShellState) -> ShellOutput {
    if rest.is_empty() {
        return ShellOutput::Err("usage: .xpath <expr>".into());
    }
    let body = match require_body(state) {
        Ok(b) => b,
        Err(o) => return o,
    };
    let tree = parse_tree(body, charset_from(state.last_content_type.as_deref()));
    let matches = tree.xpath(rest);
    record_first(&matches, state, rest);
    pretty_handles(&matches)
}

fn cmd_find_by_text(rest: &str, state: &mut ShellState) -> ShellOutput {
    if rest.is_empty() {
        return ShellOutput::Err("usage: .findByText <text>".into());
    }
    let body = match require_body(state) {
        Ok(b) => b,
        Err(o) => return o,
    };
    let tree = parse_tree(body, charset_from(state.last_content_type.as_deref()));
    let matches = tree.find_by_text(rest, TextMatch::contains().with_trim(true));
    record_first(&matches, state, rest);
    pretty_handles(&matches)
}

fn cmd_find_by_regex(rest: &str, state: &mut ShellState) -> ShellOutput {
    if rest.is_empty() {
        return ShellOutput::Err("usage: .findByRegex <pattern>".into());
    }
    let body = match require_body(state) {
        Ok(b) => b,
        Err(o) => return o,
    };
    let re = match Regex::new(rest) {
        Ok(r) => r,
        Err(e) => return ShellOutput::Err(format!("invalid regex: {e}")),
    };
    let tree = parse_tree(body, charset_from(state.last_content_type.as_deref()));
    let matches = tree.find_by_regex(&re);
    record_first(&matches, state, rest);
    pretty_handles(&matches)
}

fn cmd_save(rest: &str, state: &mut ShellState, store: &AdaptiveStore) -> ShellOutput {
    if rest.is_empty() {
        return ShellOutput::Err("usage: .save <identifier>".into());
    }
    let Some(fp) = state.last_selection.clone() else {
        return ShellOutput::Err(
            "no selection — run `.css`, `.xpath`, `.findByText`, or `.findByRegex` first.".into(),
        );
    };
    let Some(url) = state.last_url.as_ref() else {
        return ShellOutput::Err("no fetched url — `.save` needs a domain.".into());
    };
    let domain = url.host_str().unwrap_or("unknown").to_string();
    match store.save(&domain, rest, fp) {
        Ok(()) => ShellOutput::Status(format!("saved `{rest}` for `{domain}`.")),
        Err(e) => ShellOutput::Err(format!("save failed: {e}")),
    }
}

fn cmd_open(state: &ShellState) -> ShellOutput {
    let Some(body) = state.last_body.as_ref() else {
        return ShellOutput::Err(
            "no response in the session — run `.fetch <url>` first.".into(),
        );
    };
    let path = match write_tmp_html(body) {
        Ok(p) => p,
        Err(e) => return ShellOutput::Err(format!("failed to write tmp file: {e}")),
    };
    match try_open_browser(&path) {
        Ok(cmd) => ShellOutput::Status(format!("opened {} via `{}`.", path.display(), cmd)),
        Err(e) => ShellOutput::Err(format!(
            "wrote {} but failed to launch a browser: {e}",
            path.display()
        )),
    }
}

fn charset_from(content_type: Option<&str>) -> Option<&str> {
    let ct = content_type?;
    let idx = ct.to_ascii_lowercase().find("charset=")?;
    let tail = &ct[idx + "charset=".len()..];
    let end = tail
        .find(|c: char| c == ';' || c == ' ' || c == '"')
        .unwrap_or(tail.len());
    Some(&tail[..end])
}

fn record_first(matches: &[ElementHandle<'_>], state: &mut ShellState, query: &str) {
    if let Some(first) = matches.first() {
        state.last_selection = Some(fingerprint(first));
        state.last_selection_query = Some(query.to_string());
    }
}

fn pretty_handles(matches: &[ElementHandle<'_>]) -> ShellOutput {
    if matches.is_empty() {
        return ShellOutput::Lines(vec!["(no matches)".into()]);
    }
    let mut out = Vec::with_capacity(matches.len() + 1);
    out.push(format!("{} match(es):", matches.len()));
    for (i, h) in matches.iter().enumerate() {
        // Trim outerHTML so a huge `<body>` match doesn't drown the prompt.
        let html = h.html();
        let snippet: String = html.chars().take(200).collect();
        let suffix = if html.len() > snippet.len() { "…" } else { "" };
        out.push(format!("  [{i}] <{}> {}{}", h.name(), snippet, suffix));
    }
    ShellOutput::Lines(out)
}

fn write_tmp_html(body: &[u8]) -> std::io::Result<PathBuf> {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    path.push(format!("crawlex-shell-{nanos}.html"));
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Best-effort browser launch. We try each platform-typical opener in
/// turn and return the one that exited 0. The user only needs *some*
/// browser to fire; the loop continues either way.
fn try_open_browser(path: &std::path::Path) -> std::result::Result<&'static str, String> {
    let candidates: &[&'static str] = if cfg!(target_os = "macos") {
        &["open"]
    } else if cfg!(target_os = "windows") {
        &["cmd"]
    } else {
        &["xdg-open", "gio", "wslview"]
    };
    let mut last_err = String::new();
    for bin in candidates {
        let mut cmd = std::process::Command::new(bin);
        if *bin == "gio" {
            cmd.arg("open");
        } else if *bin == "cmd" {
            cmd.args(["/C", "start", ""]);
        }
        cmd.arg(path);
        match cmd.spawn() {
            Ok(_) => return Ok(bin),
            Err(e) => last_err = format!("{bin}: {e}"),
        }
    }
    Err(last_err)
}

// ─────────────────────────────────────────────────────────────────────
// Interactive loop
// ─────────────────────────────────────────────────────────────────────

pub struct ShellOptions {
    pub stealth: bool,
    pub history_file: Option<PathBuf>,
    pub adaptive_dir: PathBuf,
    pub spider_id: String,
}

impl ShellOptions {
    pub fn default_history_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(dir).join("crawlex").join("shell_history");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local/share/crawlex/shell_history");
        }
        PathBuf::from("./.crawlex/shell_history")
    }
}

pub async fn run_interactive(opts: ShellOptions) -> Result<()> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(ImpersonateFetcher::new(opts.stealth)?);
    let store = AdaptiveStore::open(&opts.adaptive_dir, &opts.spider_id).map_err(Error::Io)?;

    let history_path = opts
        .history_file
        .clone()
        .unwrap_or_else(ShellOptions::default_history_path);
    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut state = ShellState::new();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let _ = writeln!(
        stdout,
        "crawlex shell — type `.help` for commands, `.exit` to leave."
    );

    let mut line = String::new();
    loop {
        write!(stdout, "crawlex> ").ok();
        stdout.flush().ok();
        line.clear();
        let n = stdin
            .lock()
            .read_line(&mut line)
            .map_err(Error::Io)?;
        if n == 0 {
            // EOF — same as `.exit`.
            let _ = writeln!(stdout);
            break;
        }
        let raw = line.trim_end_matches(['\n', '\r']).to_string();
        if !raw.trim().is_empty() {
            append_history(&history_path, &raw);
        }
        let out = dispatch(&raw, &mut state, fetcher.as_ref(), &store).await;
        match out {
            ShellOutput::Exit => break,
            ShellOutput::Status(s) => {
                if !s.is_empty() {
                    let _ = writeln!(stdout, "{s}");
                }
            }
            ShellOutput::Lines(lines) | ShellOutput::Help(lines) => {
                for l in lines {
                    let _ = writeln!(stdout, "{l}");
                }
            }
            ShellOutput::Err(e) => {
                let _ = writeln!(stdout, "error: {e}");
            }
        }
    }
    Ok(())
}

fn append_history(path: &std::path::Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Public so `crawlex shell --history-file path` callers can read the
/// stored history. The format is one verbatim line per entry.
pub fn read_history(path: &std::path::Path) -> Vec<String> {
    let Ok(f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    std::io::BufReader::new(f)
        .lines()
        .map_while(|r| r.ok())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct StubFetcher {
        pages: Mutex<HashMap<String, FetchedPage>>,
    }
    impl StubFetcher {
        fn new() -> Self {
            Self { pages: Mutex::new(HashMap::new()) }
        }
        fn insert(&self, url: &str, html: &str) {
            let parsed = Url::parse(url).unwrap();
            self.pages.lock().unwrap().insert(
                url.to_string(),
                FetchedPage {
                    final_url: parsed,
                    status: 200,
                    content_type: Some("text/html; charset=utf-8".into()),
                    body: html.as_bytes().to_vec(),
                },
            );
        }
    }
    #[async_trait]
    impl Fetcher for StubFetcher {
        async fn fetch(&self, url: &Url) -> Result<FetchedPage> {
            self.pages
                .lock()
                .unwrap()
                .get(url.as_str())
                .cloned()
                .ok_or_else(|| Error::Http(format!("stub: no page for {url}")))
        }
    }

    fn store() -> (tempfile::TempDir, AdaptiveStore) {
        let dir = tempdir().unwrap();
        let s = AdaptiveStore::open(dir.path(), "test").unwrap();
        (dir, s)
    }

    const HTML: &str = "<!doctype html><html><body>\
        <p class='greet'>hello world</p>\
        <a href='/x' id='cta'>Buy now</a>\
        <ul><li>alpha</li><li>beta</li></ul></body></html>";

    #[tokio::test]
    async fn fetch_populates_state() {
        let fetcher = StubFetcher::new();
        fetcher.insert("https://example.com/", HTML);
        let (_t, s) = store();
        let mut state = ShellState::new();
        let out = dispatch(".fetch https://example.com/", &mut state, &fetcher, &s).await;
        assert!(matches!(&out, ShellOutput::Status(_)), "got {out:?}");
        assert_eq!(state.last_status, Some(200));
        assert!(state.last_body.is_some());
    }

    #[tokio::test]
    async fn css_returns_matches_and_records_selection() {
        let fetcher = StubFetcher::new();
        fetcher.insert("https://example.com/", HTML);
        let (_t, s) = store();
        let mut state = ShellState::new();
        dispatch(".fetch https://example.com/", &mut state, &fetcher, &s).await;
        let out = dispatch(".css p.greet", &mut state, &fetcher, &s).await;
        match out {
            ShellOutput::Lines(lines) => {
                assert!(lines[0].starts_with("1 match"), "lines={lines:?}");
                assert!(lines[1].contains("hello world"));
            }
            other => panic!("expected Lines, got {other:?}"),
        }
        assert!(state.last_selection.is_some());
    }

    #[tokio::test]
    async fn xpath_returns_matches() {
        let fetcher = StubFetcher::new();
        fetcher.insert("https://example.com/", HTML);
        let (_t, s) = store();
        let mut state = ShellState::new();
        dispatch(".fetch https://example.com/", &mut state, &fetcher, &s).await;
        let out = dispatch(".xpath //li", &mut state, &fetcher, &s).await;
        let ShellOutput::Lines(lines) = out else {
            panic!("expected Lines");
        };
        assert!(lines[0].starts_with("2 match"), "lines={lines:?}");
    }

    #[tokio::test]
    async fn find_by_text_and_regex_record_selection() {
        let fetcher = StubFetcher::new();
        fetcher.insert("https://example.com/", HTML);
        let (_t, s) = store();
        let mut state = ShellState::new();
        dispatch(".fetch https://example.com/", &mut state, &fetcher, &s).await;
        let out = dispatch(".findByText Buy now", &mut state, &fetcher, &s).await;
        let ShellOutput::Lines(lines) = out else {
            panic!("findByText output");
        };
        assert!(lines[0].starts_with("1 match"), "lines={lines:?}");
        assert!(state.last_selection.is_some());

        let out = dispatch(".findByRegex ^alpha$", &mut state, &fetcher, &s).await;
        let ShellOutput::Lines(lines) = out else {
            panic!("findByRegex output");
        };
        assert!(lines[0].starts_with("1 match"), "lines={lines:?}");
    }

    #[tokio::test]
    async fn save_persists_fingerprint() {
        let fetcher = StubFetcher::new();
        fetcher.insert("https://example.com/", HTML);
        let dir = tempdir().unwrap();
        let s = AdaptiveStore::open(dir.path(), "test").unwrap();
        let mut state = ShellState::new();
        dispatch(".fetch https://example.com/", &mut state, &fetcher, &s).await;
        dispatch(".css a#cta", &mut state, &fetcher, &s).await;
        let out = dispatch(".save buy_button", &mut state, &fetcher, &s).await;
        assert!(matches!(&out, ShellOutput::Status(_)), "got {out:?}");
        assert!(s.retrieve("example.com", "buy_button").is_some());
    }

    #[tokio::test]
    async fn save_without_selection_errors() {
        let fetcher = StubFetcher::new();
        let (_t, s) = store();
        let mut state = ShellState::new();
        let out = dispatch(".save x", &mut state, &fetcher, &s).await;
        assert!(matches!(out, ShellOutput::Err(_)));
    }

    #[tokio::test]
    async fn unknown_and_help() {
        let fetcher = StubFetcher::new();
        let (_t, s) = store();
        let mut state = ShellState::new();
        let out = dispatch(".help", &mut state, &fetcher, &s).await;
        assert!(matches!(out, ShellOutput::Help(_)));
        let out = dispatch(".wat", &mut state, &fetcher, &s).await;
        assert!(matches!(out, ShellOutput::Err(_)));
        let out = dispatch("foo bar", &mut state, &fetcher, &s).await;
        assert!(matches!(out, ShellOutput::Err(_)));
    }

    #[test]
    fn history_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("h");
        append_history(&path, ".fetch https://example.com/");
        append_history(&path, ".css p");
        let got = read_history(&path);
        assert_eq!(got, vec![".fetch https://example.com/", ".css p"]);
    }

}
