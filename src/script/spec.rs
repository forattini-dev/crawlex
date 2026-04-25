//! `ScriptSpec` AST — the common target of JSON/YAML/Lua script inputs.
//!
//! The enums here are intentionally *backend-agnostic*: no CDP client
//! types leak. Execution is wired from a separate executor module (phase 3
//! of the v0.2 plan) that translates AST nodes into calls on whichever
//! render backend is compiled in.
//!
//! Parse rules:
//!   * `ScriptSpec` is `Deserialize`-friendly from both JSON and YAML (no
//!     magic fields; serde_yml or serde_json both work out of the box).
//!   * Lua sources build the same AST programmatically; no custom parser.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const SCRIPT_SPEC_VERSION: u32 = 1;

/// Top-level document. `version` is checked on load; future versions must
/// stay backward-compatible or bump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptSpec {
    pub version: u32,

    #[serde(default)]
    pub defaults: Defaults,

    /// Named selector aliases. Steps/captures/exports can reference them
    /// with `@name` to keep the DSL DRY.
    #[serde(default)]
    pub selectors: IndexMap<String, String>,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub captures: Vec<Capture>,

    #[serde(default)]
    pub assertions: Vec<Assertion>,

    /// Name → extractor recipe. Output keys of the ExtractCompleted event.
    #[serde(default)]
    pub exports: IndexMap<String, Export>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Defaults {
    /// Default actionability timeout for every step that takes a locator.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// When true, a multi-match locator in a single-element action fails
    /// instead of picking the first (Playwright strict mode).
    #[serde(default = "default_strict")]
    pub strict_locators: bool,

    /// Cap on script execution wall time. `None` = no cap.
    #[serde(default)]
    pub max_wall_ms: Option<u64>,
}

fn default_timeout_ms() -> u64 {
    10_000
}
fn default_strict() -> bool {
    true
}

/// A single step. Serde tags on the variants produce `{ "kind": args }`
/// shape — see the module-level example.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Goto(GotoStep),
    WaitFor(WaitForStep),
    WaitMs { ms: u64 },
    Click(ClickStep),
    Type(TypeStep),
    Press { key: String },
    Scroll { dy: f64 },
    Eval { script: String },
    Submit { locator: Locator },
    Screenshot(ScreenshotStep),
    Snapshot(SnapshotStep),
    Extract(ExtractStep),
    Assert(Assertion),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GotoStep {
    pub url: String,
    #[serde(default)]
    pub wait_until: Option<WaitUntil>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    Load,
    DomContentLoaded,
    NetworkIdle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitForStep {
    pub locator: Locator,
    #[serde(default)]
    pub state: Option<LocatorState>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocatorState {
    Attached,
    Detached,
    Visible,
    Hidden,
    Stable,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickStep {
    pub locator: Locator,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Skip actionability (stable + enabled + receiving-events). Default
    /// false; use when clicking a known-overlay or intentional flaky target.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeStep {
    pub locator: Locator,
    pub text: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// When true, clear the field before typing (select-all + delete).
    #[serde(default)]
    pub clear: bool,
}

/// `"#id"` → raw DSL; `"@name"` → lookup in `ScriptSpec::selectors`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Locator {
    Raw(String),
}

impl Locator {
    /// Resolve against the named-selector map. `@name` looks up; anything
    /// else is returned as-is (it IS the DSL).
    pub fn resolve<'a>(&'a self, named: &'a IndexMap<String, String>) -> &'a str {
        match self {
            Self::Raw(s) => {
                if let Some(name) = s.strip_prefix('@') {
                    named.get(name).map(|s| s.as_str()).unwrap_or(s)
                } else {
                    s
                }
            }
        }
    }

    /// Return the `@eN` ref when this locator points at an AX-snapshot
    /// ref rather than a named selector or raw DSL. Matches the
    /// `@e<digits>` shape emitted by `render::ax_snapshot`.
    pub fn ax_ref(&self) -> Option<&str> {
        let Self::Raw(s) = self;
        let rest = s.strip_prefix("@e")?;
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            Some(s.as_str())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotStep {
    #[serde(default)]
    pub mode: ScreenshotMode,
    /// Optional element scope when `mode = Element`.
    #[serde(default)]
    pub locator: Option<Locator>,
    /// Artifact name fragment; will be prefixed into the saved path.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub format: ScreenshotFormat,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotMode {
    #[default]
    Viewport,
    FullPage,
    Element,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    #[default]
    Png,
    Jpeg,
}

/// Content snapshot — persisted as an `artifact.saved` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotStep {
    pub kind: SnapshotKind,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    /// Raw HTTP response body before any JS ran.
    ResponseBody,
    /// DOM after initial parse but before any scripts executed.
    DomSnapshot,
    /// DOM after wait strategy + actions (our `html_post_js`).
    PostJsHtml,
    /// Current cookies + localStorage for the active origin.
    State,
    /// Full SPA/PWA state bundle for the current origin/page: manifest JSON,
    /// service workers, storage, runtime routes, network endpoints, and
    /// deep client-side stores when available.
    PwaState,
    /// Accessibility tree with `@eN` refs for interactive nodes. Compact,
    /// token-cheap view suitable for LLM-driven flows. The accompanying
    /// `ref_map` resolves refs to backend DOM node IDs.
    AxTree,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractStep {
    /// Name → where-to-pull spec. Same shape as `ScriptSpec::exports`; this
    /// is an in-step alternative for when extraction is mid-flow.
    pub fields: IndexMap<String, Export>,
}

/// Where a field value comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Export {
    /// Shorthand: bare locator → `text` of the first match.
    BareLocator(String),
    /// Full spec.
    Spec(ExportSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSpec {
    pub locator: Locator,
    #[serde(default)]
    pub kind: ExportKind,
    /// For `Attribute` only: which attribute to read.
    #[serde(default)]
    pub attr: Option<String>,
    /// For `Regex` only: pattern applied to `text` of the match.
    #[serde(default)]
    pub pattern: Option<String>,
    /// When true, return all matches as a list instead of the first.
    #[serde(default)]
    pub as_list: bool,
    /// Which content stage to pull from. Default: `rendered`.
    #[serde(default)]
    pub origin: ExportOrigin,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    #[default]
    Text,
    Html,
    Attribute,
    Links,
    JsonLd,
    Regex,
    Script,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportOrigin {
    /// Pre-JS HTML only.
    Static,
    #[default]
    /// Post-JS HTML only.
    Rendered,
    /// Static first; fall back to rendered when the locator doesn't match.
    StaticThenRendered,
    /// Rendered first; fall back to static when the locator doesn't match.
    RenderedThenStatic,
}

/// Passive check that halts the script on failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertion {
    Exists { locator: Locator },
    NotExists { locator: Locator },
    Contains { locator: Locator, text: String },
    HasUrl { pattern: String },
    HasTitle { pattern: String },
}

/// A capture is an artifact to persist after the script finishes (or at
/// specific steps; see `Step::Screenshot`/`Step::Snapshot`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capture {
    Screenshot(ScreenshotStep),
    Snapshot(SnapshotStep),
    /// Full network log (urls, status, bytes). Always emitted as NDJSON.
    Network,
    /// Browser console messages.
    Console,
    /// Collected metrics (timings + vitals + resources).
    Metrics,
    /// Host facts (robots, manifest, favicon, cert, RDAP) when enabled.
    Seo,
}

// ---------- Load helpers ----------

#[derive(Debug, thiserror::Error)]
pub enum ScriptLoadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported script format for path {0}")]
    UnsupportedFormat(String),
    #[error("unsupported spec version: got {got}, expected {expected}")]
    VersionMismatch { got: u32, expected: u32 },
}

impl ScriptSpec {
    /// Parse a `ScriptSpec` from JSON bytes. YAML/Lua loaders live in
    /// separate modules (feature-gated).
    pub fn from_json(data: &[u8]) -> Result<Self, ScriptLoadError> {
        let s: Self = serde_json::from_slice(data)?;
        s.check_version()?;
        Ok(s)
    }

    fn check_version(&self) -> Result<(), ScriptLoadError> {
        if self.version != SCRIPT_SPEC_VERSION {
            return Err(ScriptLoadError::VersionMismatch {
                got: self.version,
                expected: SCRIPT_SPEC_VERSION,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ax_ref_matches_at_e_digits_only() {
        assert_eq!(Locator::Raw("@e1".into()).ax_ref(), Some("@e1"));
        assert_eq!(Locator::Raw("@e42".into()).ax_ref(), Some("@e42"));
        assert_eq!(Locator::Raw("@email".into()).ax_ref(), None);
        assert_eq!(Locator::Raw("#id".into()).ax_ref(), None);
        assert_eq!(Locator::Raw("@e".into()).ax_ref(), None);
        assert_eq!(Locator::Raw("@e1a".into()).ax_ref(), None);
    }

    #[test]
    fn snapshot_kind_axtree_round_trips() {
        let json = serde_json::to_string(&SnapshotKind::AxTree).unwrap();
        assert_eq!(json, "\"ax_tree\"");
        let back: SnapshotKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, SnapshotKind::AxTree));
    }

    #[test]
    fn snapshot_kind_pwa_state_round_trips() {
        let json = serde_json::to_string(&SnapshotKind::PwaState).unwrap();
        assert_eq!(json, "\"pwa_state\"");
        let back: SnapshotKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, SnapshotKind::PwaState));
    }
}
