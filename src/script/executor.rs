//! ScriptSpec → backend translation.
//!
//! The executor is intentionally backend-agnostic: it walks a
//! `ScriptSpec` and produces a `Plan` of low-level actions + captures +
//! assertions that a *renderer-specific* runner consumes. Today the only
//! such runner is the CDP client one in `render::actions`; tomorrow a
//! second backend can plug in without touching this module.
//!
//! This module never imports `CDP client`. It speaks in
//! `script::spec` types only.

use indexmap::IndexMap;

use crate::script::spec::{
    Assertion, Capture, Defaults, Export, ExportSpec, Locator, ScriptSpec, Step,
};

/// Resolved, ready-to-execute plan derived from a `ScriptSpec`.
///
/// Selectors in steps/captures/exports/assertions are already resolved
/// (named aliases dereferenced). Defaults from the spec are propagated
/// onto each step that didn't carry an explicit timeout.
pub struct Plan {
    pub defaults: Defaults,
    pub steps: Vec<ResolvedStep>,
    pub captures: Vec<Capture>,
    pub assertions: Vec<Assertion>,
    pub exports: IndexMap<String, ResolvedExport>,
}

/// A `Step` with locators dereferenced through the spec's named map and
/// fallback timeouts injected from `defaults`.
#[derive(Debug, Clone)]
pub enum ResolvedStep {
    Goto {
        url: String,
        wait_until: Option<crate::script::spec::WaitUntil>,
        timeout_ms: u64,
    },
    WaitFor {
        selector: String,
        state: Option<crate::script::spec::LocatorState>,
        timeout_ms: u64,
    },
    WaitMs {
        ms: u64,
    },
    Click {
        selector: String,
        timeout_ms: u64,
        force: bool,
    },
    Type {
        selector: String,
        text: String,
        timeout_ms: u64,
        clear: bool,
    },
    Press {
        key: String,
    },
    Scroll {
        dy: f64,
    },
    Eval {
        script: String,
    },
    Submit {
        selector: String,
        timeout_ms: u64,
    },
    Screenshot(crate::script::spec::ScreenshotStep),
    Snapshot(crate::script::spec::SnapshotStep),
    Extract(IndexMap<String, ResolvedExport>),
    Assert(Assertion),
}

#[derive(Debug, Clone)]
pub struct ResolvedExport {
    pub selector: String,
    pub kind: crate::script::spec::ExportKind,
    pub attr: Option<String>,
    pub pattern: Option<String>,
    pub as_list: bool,
    pub origin: crate::script::spec::ExportOrigin,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("named selector `{0}` referenced but not declared in spec.selectors")]
    UnknownNamedSelector(String),
}

/// Build a `Plan` from a `ScriptSpec`. Resolves named selectors and
/// injects per-step timeouts. Fails if a `@name` reference doesn't exist
/// in `spec.selectors` — caught early instead of at runtime.
pub fn plan(spec: &ScriptSpec) -> Result<Plan, PlanError> {
    let resolve = |loc: &Locator| -> Result<String, PlanError> {
        match loc {
            Locator::Raw(s) => {
                // AX-snapshot refs (`@eN` with only digits after `@e`)
                // flow through untouched — the runner's ref_map lookup
                // resolves them at execution time. Without this
                // short-circuit, the planner would reject every AX
                // ref as an unknown named selector.
                if loc.ax_ref().is_some() {
                    return Ok(s.clone());
                }
                if let Some(name) = s.strip_prefix('@') {
                    spec.selectors
                        .get(name)
                        .cloned()
                        .ok_or_else(|| PlanError::UnknownNamedSelector(name.to_string()))
                } else {
                    Ok(s.clone())
                }
            }
        }
    };

    let resolve_export = |ex: &Export| -> Result<ResolvedExport, PlanError> {
        match ex {
            Export::BareLocator(s) => Ok(ResolvedExport {
                selector: {
                    // Pass AX refs through untouched (same reasoning as
                    // the step-locator resolver above).
                    let is_ax = s
                        .strip_prefix("@e")
                        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false);
                    if is_ax {
                        s.clone()
                    } else if let Some(name) = s.strip_prefix('@') {
                        spec.selectors
                            .get(name)
                            .cloned()
                            .ok_or_else(|| PlanError::UnknownNamedSelector(name.to_string()))?
                    } else {
                        s.clone()
                    }
                },
                kind: crate::script::spec::ExportKind::Text,
                attr: None,
                pattern: None,
                as_list: false,
                origin: crate::script::spec::ExportOrigin::Rendered,
            }),
            Export::Spec(ExportSpec {
                locator,
                kind,
                attr,
                pattern,
                as_list,
                origin,
            }) => Ok(ResolvedExport {
                selector: resolve(locator)?,
                kind: *kind,
                attr: attr.clone(),
                pattern: pattern.clone(),
                as_list: *as_list,
                origin: *origin,
            }),
        }
    };

    let default_timeout = spec.defaults.timeout_ms;

    let mut steps = Vec::with_capacity(spec.steps.len());
    for s in &spec.steps {
        steps.push(match s {
            Step::Goto(g) => ResolvedStep::Goto {
                url: g.url.clone(),
                wait_until: g.wait_until,
                timeout_ms: g.timeout_ms.unwrap_or(default_timeout),
            },
            Step::WaitFor(w) => ResolvedStep::WaitFor {
                selector: resolve(&w.locator)?,
                state: w.state,
                timeout_ms: w.timeout_ms.unwrap_or(default_timeout),
            },
            Step::WaitMs { ms } => ResolvedStep::WaitMs { ms: *ms },
            Step::Click(c) => ResolvedStep::Click {
                selector: resolve(&c.locator)?,
                timeout_ms: c.timeout_ms.unwrap_or(default_timeout),
                force: c.force,
            },
            Step::Type(t) => ResolvedStep::Type {
                selector: resolve(&t.locator)?,
                text: t.text.clone(),
                timeout_ms: t.timeout_ms.unwrap_or(default_timeout),
                clear: t.clear,
            },
            Step::Press { key } => ResolvedStep::Press { key: key.clone() },
            Step::Scroll { dy } => ResolvedStep::Scroll { dy: *dy },
            Step::Eval { script } => ResolvedStep::Eval {
                script: script.clone(),
            },
            Step::Submit { locator } => ResolvedStep::Submit {
                selector: resolve(locator)?,
                timeout_ms: default_timeout,
            },
            Step::Screenshot(s) => ResolvedStep::Screenshot(s.clone()),
            Step::Snapshot(s) => ResolvedStep::Snapshot(s.clone()),
            Step::Extract(e) => {
                let mut m = IndexMap::with_capacity(e.fields.len());
                for (k, v) in &e.fields {
                    m.insert(k.clone(), resolve_export(v)?);
                }
                ResolvedStep::Extract(m)
            }
            Step::Assert(a) => ResolvedStep::Assert(a.clone()),
        });
    }

    let mut exports = IndexMap::with_capacity(spec.exports.len());
    for (k, v) in &spec.exports {
        exports.insert(k.clone(), resolve_export(v)?);
    }

    Ok(Plan {
        defaults: spec.defaults.clone(),
        steps,
        captures: spec.captures.clone(),
        assertions: spec.assertions.clone(),
        exports,
    })
}
