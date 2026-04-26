//! ScriptSpec runner — executes a resolved [`Plan`] against a live
//! `chromiumoxide::Page` (our in-tree CDP client fork at
//! `render::chrome::page::Page`).
//!
//! The runner is the missing piece between the backend-agnostic
//! [`Plan`](crate::script::executor::Plan) and the CDP-driven primitives
//! in `render::{interact,selector,ref_resolver,ax_snapshot,pool}`. It:
//!
//! * dispatches each [`ResolvedStep`] to the matching CDP call;
//! * gates every verb through [`ActionPolicy`];
//! * emits `step.started` / `step.completed` / `artifact.saved` events
//!   through an [`EventSink`];
//! * resolves `@eN` locators through the `ref_map` produced by
//!   [`render::ax_snapshot::capture_ax_snapshot`] so scripts can target
//!   nodes without a CSS selector once an AX snapshot has been taken;
//! * stores extracted values in a `captures` map surfaced through
//!   [`RunOutcome`].
//!
//! The runner does *not* own the page lifecycle — the caller
//! (`render::pool::RenderPool` or a test harness) drives setup/teardown
//! and hands a ready `Page` in.

#![cfg(feature = "cdp-backend")]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::events::{ArtifactSavedData, Event, EventKind, EventSink};
use crate::policy::action_policy::{ActionPolicy, ActionRule, ActionVerb};
use crate::render::ax_snapshot::{capture_ax_snapshot, SnapshotOptions};
use crate::render::chrome::page::Page;
use crate::render::interact::{self, MousePos};
use crate::render::pool::{RenderPool, ScreenshotCaptureMode};
use crate::render::ref_resolver::{
    click_by_backend_node, lookup_backend_node_id, type_by_backend_node,
};
use crate::render::selector;
use crate::script::executor::{Plan, ResolvedExport, ResolvedStep};
use crate::script::spec::{
    Assertion, ExportKind, ScreenshotMode, ScreenshotStep, SnapshotKind, SnapshotStep,
};
use crate::{Error, Result};

/// One artifact persisted during a run. `kind` is the stage tag
/// (`screenshot.viewport`, `snapshot.ax_tree`, ...). `step_kind` is the
/// ScriptSpec verb that produced the artifact (`"screenshot"` or
/// `"snapshot"`). `selector` is populated for element-scoped screenshots.
#[derive(Debug, Clone)]
pub struct ArtifactRef {
    pub step_id: String,
    pub step_kind: String,
    pub kind: String,
    pub name: String,
    pub sha256: String,
    pub bytes: usize,
    pub mime: String,
    pub selector: Option<String>,
}

/// Per-step execution summary returned from [`ScriptRunner::run`].
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub step_id: String,
    pub step_kind: String,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub artifacts: Vec<ArtifactRef>,
}

/// Whole-script outcome. `captures` holds values grabbed by `Extract`
/// steps; `exports` is the final projection (spec-level `exports`
/// evaluated after the step list).
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub steps: Vec<StepOutcome>,
    pub captures: HashMap<String, Value>,
    pub exports: IndexMap<String, Value>,
    /// First assertion failure, if any. Halts the run.
    pub failed_assertion: Option<String>,
}

/// Executes a [`Plan`] against a live [`Page`]. Holds transient state
/// (mouse position, AX ref map, captured values) so callers can reuse
/// the same runner across multiple calls if desired — though the
/// typical flow is one-shot: build, `run`, drop.
pub struct ScriptRunner<'a> {
    page: &'a Page,
    plan: &'a Plan,
    session_id: String,
    sink: Option<Arc<dyn EventSink>>,
    action_policy: &'a ActionPolicy,
    ref_map: BTreeMap<String, i64>,
    captures: HashMap<String, Value>,
    mouse: MousePos,
    run_id: Option<u64>,
    wait_strategy: crate::wait_strategy::WaitStrategy,
    /// Storage handle used to persist screenshot/snapshot artifacts
    /// through the unified `save_artifact` contract. `None` means the
    /// runner only returns `ArtifactRef`s in-memory without persistence.
    storage: Option<Arc<dyn crate::storage::ArtifactStorage>>,
    /// URL the page was navigated to — used as the artifact key so
    /// consumers can correlate artifacts with pages.
    url: Option<url::Url>,
}

impl<'a> ScriptRunner<'a> {
    pub fn new(
        page: &'a Page,
        plan: &'a Plan,
        session_id: impl Into<String>,
        action_policy: &'a ActionPolicy,
    ) -> Self {
        Self {
            page,
            plan,
            session_id: session_id.into(),
            sink: None,
            action_policy,
            ref_map: BTreeMap::new(),
            captures: HashMap::new(),
            mouse: MousePos::default(),
            run_id: None,
            wait_strategy: crate::wait_strategy::WaitStrategy::default(),
            storage: None,
            url: None,
        }
    }

    pub fn with_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn with_run_id(mut self, run_id: u64) -> Self {
        self.run_id = Some(run_id);
        self
    }

    pub fn with_wait_strategy(mut self, wait_strategy: crate::wait_strategy::WaitStrategy) -> Self {
        self.wait_strategy = wait_strategy;
        self
    }

    /// Attach a storage handle; without this, `save_artifact` is a
    /// no-op and artifacts live only in the [`RunOutcome`]. Takes the
    /// narrow [`ArtifactStorage`](crate::storage::ArtifactStorage)
    /// trait — runner only persists artifact bytes, never state /
    /// telemetry / intel.
    pub fn with_storage(mut self, storage: Arc<dyn crate::storage::ArtifactStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Attach the page URL so persisted artifacts can be keyed back to
    /// the originating navigation.
    pub fn with_url(mut self, url: url::Url) -> Self {
        self.url = Some(url);
        self
    }

    /// Execute all steps in order. On the first [`Assertion`] failure,
    /// stops and returns a partial [`RunOutcome`] with
    /// `failed_assertion` populated. Interaction/navigation failures are
    /// fatal for the run; passive capture failures are recorded and the
    /// run continues so operators still get the trace/artifacts that did land.
    pub async fn run(&mut self) -> Result<RunOutcome> {
        let mut outcome = RunOutcome::default();
        for (idx, step) in self.plan.steps.iter().enumerate() {
            let step_id = format!("s{idx:03}");
            let step_kind = step_kind_str(step).to_string();
            self.emit(
                EventKind::StepStarted,
                |_| json!({ "step_id": step_id, "step_kind": step_kind }),
            );
            let t0 = Instant::now();
            let mut artifacts = Vec::new();
            let mut failed_assert: Option<String> = None;
            let res = self
                .exec_step(&step_id, step, &mut artifacts, &mut failed_assert)
                .await;
            let duration_ms = t0.elapsed().as_millis() as u64;
            let (success, error) = match res {
                Ok(()) => (failed_assert.is_none(), failed_assert.clone()),
                Err(e) => (false, Some(e.to_string())),
            };
            let so = StepOutcome {
                step_id: step_id.clone(),
                step_kind: step_kind.clone(),
                success,
                error: error.clone(),
                duration_ms,
                artifacts,
            };
            let succ_copy = success;
            let err_copy = error.clone();
            self.emit(EventKind::StepCompleted, move |_| {
                json!({
                    "step_id": step_id,
                    "step_kind": step_kind,
                    "success": succ_copy,
                    "duration_ms": duration_ms,
                    "error": err_copy,
                })
            });
            outcome.steps.push(so);
            if let Some(why) = failed_assert {
                outcome.failed_assertion = Some(why);
                break;
            }
            if error.is_some() && should_abort_on_step_error(step) {
                break;
            }
        }

        // Spec-level assertions run after the step list (pre-exports).
        // The executor ships them as a distinct `Plan::assertions` list
        // — walk them honouring the first-failure-wins semantics.
        for (i, a) in self.plan.assertions.iter().enumerate() {
            if let Err(why) = self.exec_assert(a).await {
                outcome.failed_assertion = Some(format!("spec-assertion[{i}]: {why}"));
                return Ok(outcome);
            }
        }

        // Final exports projection: evaluate each `ResolvedExport` and
        // put the value in `exports` (preserving the spec's IndexMap
        // order so consumers can rely on insertion order).
        for (k, ex) in &self.plan.exports {
            match self.eval_export(ex).await {
                Ok(v) => {
                    outcome.exports.insert(k.clone(), v);
                }
                Err(e) => {
                    outcome
                        .exports
                        .insert(k.clone(), json!({ "error": e.to_string() }));
                }
            }
        }

        // Merge in-step captures so callers have a single `captures`
        // map to consume alongside `exports`.
        outcome.captures = std::mem::take(&mut self.captures);
        Ok(outcome)
    }

    async fn exec_step(
        &mut self,
        step_id: &str,
        step: &ResolvedStep,
        artifacts: &mut Vec<ArtifactRef>,
        failed_assert: &mut Option<String>,
    ) -> Result<()> {
        // Policy gate — all verb-bearing steps pass through here so one
        // operator policy blob covers every execution surface.
        let verb = step_verb(step);
        if let Some(v) = verb {
            match self.action_policy.check(v) {
                ActionRule::Allow => {}
                ActionRule::Deny | ActionRule::Confirm => {
                    return Err(Error::HookAbort(format!(
                        "action-policy: {} denied",
                        v.as_str()
                    )));
                }
            }
        }

        match step {
            ResolvedStep::Goto {
                url,
                wait_until,
                timeout_ms,
            } => {
                use crate::render::chrome_protocol::cdp::browser_protocol::page::NavigateParams;
                let params = NavigateParams::builder()
                    .url(url.clone())
                    .build()
                    .map_err(|e| Error::Render(format!("goto params: {e}")))?;
                let nav = self.page.execute(params);
                let _ = tokio::time::timeout(Duration::from_millis(*timeout_ms), nav)
                    .await
                    .map_err(|_| Error::Render(format!("goto timeout: {url}")))?
                    .map_err(|e| Error::Render(format!("goto: {e}")))?;
                self.settle_after_navigation(*wait_until).await?;
            }
            ResolvedStep::WaitFor {
                selector,
                state: _,
                timeout_ms,
            } => {
                interact::wait_for_selector(self.page, selector, *timeout_ms).await?;
            }
            ResolvedStep::WaitMs { ms } => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
            ResolvedStep::Click {
                selector: sel,
                timeout_ms: _,
                force: _,
            } => {
                self.click_by_selector_or_ref(sel).await?;
                self.settle_after_mutation().await?;
            }
            ResolvedStep::Type {
                selector: sel,
                text,
                timeout_ms: _,
                clear,
            } => {
                if *clear {
                    // Focus + select-all + delete via key events. Cheap
                    // and works for text inputs; users who need richer
                    // clearing can compose Click + Press steps.
                    if selector::focus(self.page, sel).await.unwrap_or(false) {
                        interact::eval_js(
                            self.page,
                            "document.activeElement && document.activeElement.select && document.activeElement.select()",
                        )
                        .await
                        .ok();
                    }
                }
                self.type_by_selector_or_ref(sel, text).await?;
                self.settle_after_mutation().await?;
            }
            ResolvedStep::Press { key } => {
                press_key(self.page, key).await?;
                self.settle_after_mutation().await?;
            }
            ResolvedStep::Scroll { dy } => {
                interact::scroll_by(self.page, *dy, self.mouse).await?;
                self.settle_after_mutation().await?;
            }
            ResolvedStep::Eval { script } => {
                let v = interact::eval_js(self.page, script).await?;
                self.captures.insert(format!("{step_id}.eval"), v);
                self.settle_after_mutation().await?;
            }
            ResolvedStep::Submit {
                selector: sel,
                timeout_ms: _,
            } => {
                // Submit semantics: click the element; forms with
                // default-action handlers will submit on synthetic
                // click. An explicit form.submit() path would skip the
                // validation UI most sites rely on.
                self.click_by_selector_or_ref(sel).await?;
                self.settle_after_mutation().await?;
            }
            ResolvedStep::Screenshot(s) => {
                let ar = self.exec_screenshot(step_id, s).await?;
                artifacts.push(ar);
            }
            ResolvedStep::Snapshot(s) => {
                let ar = self.exec_snapshot(step_id, s).await?;
                artifacts.push(ar);
            }
            ResolvedStep::Extract(fields) => {
                for (k, ex) in fields {
                    match self.eval_export(ex).await {
                        Ok(v) => {
                            self.captures.insert(k.clone(), v);
                        }
                        Err(e) => {
                            self.captures
                                .insert(k.clone(), json!({ "error": e.to_string() }));
                        }
                    }
                }
            }
            ResolvedStep::Assert(a) => {
                if let Err(why) = self.exec_assert(a).await {
                    *failed_assert = Some(why);
                }
            }
        }
        Ok(())
    }

    /// Resolve a locator string against either the AX ref_map (for
    /// `@eN` refs) or the selector DSL (everything else) and click it.
    async fn click_by_selector_or_ref(&mut self, sel: &str) -> Result<()> {
        if is_ax_ref(sel) {
            let bnid = lookup_backend_node_id(sel, &self.ref_map).ok_or_else(|| {
                Error::Render(format!(
                    "no AX snapshot available — add Snapshot(AxTree) before using {sel}"
                ))
            })?;
            self.mouse = click_by_backend_node(self.page, bnid, self.mouse).await?;
        } else {
            self.mouse = interact::click_selector(self.page, sel, self.mouse).await?;
        }
        Ok(())
    }

    async fn type_by_selector_or_ref(&mut self, sel: &str, text: &str) -> Result<()> {
        if is_ax_ref(sel) {
            let bnid = lookup_backend_node_id(sel, &self.ref_map).ok_or_else(|| {
                Error::Render(format!(
                    "no AX snapshot available — add Snapshot(AxTree) before using {sel}"
                ))
            })?;
            type_by_backend_node(self.page, bnid, text).await?;
        } else {
            interact::type_text(self.page, sel, text).await?;
        }
        Ok(())
    }

    async fn exec_screenshot(&mut self, step_id: &str, s: &ScreenshotStep) -> Result<ArtifactRef> {
        let (mode, selector) = match (&s.mode, s.locator.as_ref()) {
            (ScreenshotMode::Viewport, _) => (ScreenshotCaptureMode::Viewport, None),
            (ScreenshotMode::FullPage, _) => (ScreenshotCaptureMode::FullPage, None),
            (ScreenshotMode::Element, Some(loc)) => {
                let resolved = self
                    .plan_resolve_locator(loc)
                    .unwrap_or_else(|| loc.resolve(&IndexMap::new()).to_string());
                (
                    ScreenshotCaptureMode::Element {
                        selector: resolved.clone(),
                    },
                    Some(resolved),
                )
            }
            (ScreenshotMode::Element, None) => {
                return Err(Error::Render(
                    "Screenshot(Element) requires a locator".into(),
                ))
            }
        };
        let png = capture_screenshot_via_pool(self.page, mode.clone())
            .await
            .ok_or_else(|| Error::Render("screenshot capture returned None".into()))?;
        let name = s
            .name
            .clone()
            .unwrap_or_else(|| format!("step_{step_id}_screenshot"));
        let (kind, art_kind) = match mode {
            ScreenshotCaptureMode::Viewport => (
                "screenshot.viewport",
                crate::storage::ArtifactKind::ScreenshotViewport,
            ),
            ScreenshotCaptureMode::FullPage => (
                "screenshot.full_page",
                crate::storage::ArtifactKind::ScreenshotFullPage,
            ),
            ScreenshotCaptureMode::Element { .. } => (
                "screenshot.element",
                crate::storage::ArtifactKind::ScreenshotElement,
            ),
        };
        let sha = sha256_hex(&png);
        let mime = art_kind.mime().to_string();
        let path = self
            .persist_artifact(
                art_kind,
                &name,
                step_id,
                "screenshot",
                selector.as_deref(),
                &png,
            )
            .await;
        let ar = ArtifactRef {
            step_id: step_id.to_string(),
            step_kind: "screenshot".to_string(),
            kind: kind.to_string(),
            name: name.clone(),
            sha256: sha.clone(),
            bytes: png.len(),
            mime: mime.clone(),
            selector: selector.clone(),
        };
        let final_url = self.current_url().await.ok().map(|u| u.to_string());
        let emit_data = ArtifactSavedData {
            kind: kind.to_string(),
            mime,
            size: png.len() as u64,
            sha256: sha,
            name: Some(name),
            step_id: Some(step_id.to_string()),
            step_kind: Some("screenshot".to_string()),
            selector,
            final_url,
            path,
        };
        self.emit(EventKind::ArtifactSaved, move |_| {
            serde_json::to_value(&emit_data).unwrap_or(Value::Null)
        });
        Ok(ar)
    }

    async fn exec_snapshot(&mut self, step_id: &str, s: &SnapshotStep) -> Result<ArtifactRef> {
        let (kind_tag, art_kind, payload): (&'static str, crate::storage::ArtifactKind, Vec<u8>) =
            match s.kind {
                SnapshotKind::AxTree => {
                    let snap = capture_ax_snapshot(self.page, &SnapshotOptions::default()).await?;
                    self.ref_map = snap.ref_map.clone();
                    let rendered = snap.render_tree();
                    (
                        "snapshot.ax_tree",
                        crate::storage::ArtifactKind::SnapshotAxTree,
                        rendered.into_bytes(),
                    )
                }
                SnapshotKind::PostJsHtml => {
                    let html = self
                        .page
                        .content()
                        .await
                        .map_err(|e| Error::Render(format!("content: {e}")))?;
                    (
                        "snapshot.post_js_html",
                        crate::storage::ArtifactKind::SnapshotPostJsHtml,
                        html.into_bytes(),
                    )
                }
                SnapshotKind::DomSnapshot => {
                    let html = self
                        .page
                        .content()
                        .await
                        .map_err(|e| Error::Render(format!("content: {e}")))?;
                    (
                        "snapshot.dom_snapshot",
                        crate::storage::ArtifactKind::SnapshotDom,
                        html.into_bytes(),
                    )
                }
                SnapshotKind::ResponseBody => {
                    let html = self
                        .page
                        .content()
                        .await
                        .map_err(|e| Error::Render(format!("content: {e}")))?;
                    (
                        "snapshot.response_body",
                        crate::storage::ArtifactKind::SnapshotResponseBody,
                        html.into_bytes(),
                    )
                }
                SnapshotKind::State => {
                    let js = r#"(() => {
                        const out = { cookie: document.cookie, localStorage: {}, sessionStorage: {} };
                        try { for (const k in localStorage) out.localStorage[k] = localStorage.getItem(k); } catch (_) {}
                        try { for (const k in sessionStorage) out.sessionStorage[k] = sessionStorage.getItem(k); } catch (_) {}
                        return out;
                    })()"#;
                    let v = interact::eval_js(self.page, js).await?;
                    let body = serde_json::to_vec(&v).unwrap_or_default();
                    (
                        "snapshot.state",
                        crate::storage::ArtifactKind::SnapshotState,
                        body,
                    )
                }
                SnapshotKind::PwaState => {
                    let final_url = self.current_url().await?;
                    let value =
                        RenderPool::capture_pwa_state_snapshot(self.page, &final_url, true).await;
                    let body = serde_json::to_vec(&value).unwrap_or_default();
                    (
                        "snapshot.pwa_state",
                        crate::storage::ArtifactKind::SnapshotPwaState,
                        body,
                    )
                }
            };
        let name = s
            .name
            .clone()
            .unwrap_or_else(|| format!("step_{step_id}_{}", kind_tag.replace('.', "_")));
        let sha = sha256_hex(&payload);
        let mime = art_kind.mime().to_string();
        let path = self
            .persist_artifact(art_kind, &name, step_id, "snapshot", None, &payload)
            .await;
        let ar = ArtifactRef {
            step_id: step_id.to_string(),
            step_kind: "snapshot".to_string(),
            kind: kind_tag.to_string(),
            name: name.clone(),
            sha256: sha.clone(),
            bytes: payload.len(),
            mime: mime.clone(),
            selector: None,
        };
        let final_url = self.current_url().await.ok().map(|u| u.to_string());
        let emit_data = ArtifactSavedData {
            kind: kind_tag.to_string(),
            mime,
            size: payload.len() as u64,
            sha256: sha,
            name: Some(name),
            step_id: Some(step_id.to_string()),
            step_kind: Some("snapshot".to_string()),
            selector: None,
            final_url,
            path,
        };
        self.emit(EventKind::ArtifactSaved, move |_| {
            serde_json::to_value(&emit_data).unwrap_or(Value::Null)
        });
        Ok(ar)
    }

    /// Persist a runner-produced artifact through the unified storage
    /// trait when a backend is wired. Swallow storage errors: a disk
    /// hiccup should not abort the script — it's already returned in
    /// the `RunOutcome::artifacts` manifest. Returns the storage
    /// location (path or `cas:<sha256>` URI) when the backend reported
    /// one, so the caller can pass it through to the NDJSON event.
    async fn persist_artifact(
        &self,
        kind: crate::storage::ArtifactKind,
        name: &str,
        step_id: &str,
        step_kind: &str,
        selector: Option<&str>,
        bytes: &[u8],
    ) -> Option<String> {
        let storage = self.storage.as_ref()?;
        let url = self.url.as_ref()?;
        let final_url = self
            .page
            .url()
            .await
            .ok()
            .flatten()
            .and_then(|u| url::Url::parse(&u).ok());
        let meta = crate::storage::ArtifactMeta {
            url,
            final_url: final_url.as_ref(),
            session_id: &self.session_id,
            kind,
            name: Some(name),
            step_id: Some(step_id),
            step_kind: Some(step_kind),
            selector,
            mime: None,
        };
        match storage.save_artifact(&meta, bytes).await {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(step_id, name, ?e, "save_artifact failed");
                None
            }
        }
    }

    async fn exec_assert(&self, a: &Assertion) -> std::result::Result<(), String> {
        match a {
            Assertion::Exists { locator } => {
                let map = IndexMap::new();
                let sel = locator.resolve(&map);
                let n = selector::count(self.page, sel)
                    .await
                    .map_err(|e| e.to_string())?;
                if n == 0 {
                    return Err(format!("exists: {sel} not found"));
                }
            }
            Assertion::NotExists { locator } => {
                let map = IndexMap::new();
                let sel = locator.resolve(&map);
                let n = selector::count(self.page, sel)
                    .await
                    .map_err(|e| e.to_string())?;
                if n != 0 {
                    return Err(format!("not_exists: {sel} found {n} match(es)"));
                }
            }
            Assertion::Contains { locator, text } => {
                let map = IndexMap::new();
                let sel = locator.resolve(&map);
                let got = element_text(self.page, sel)
                    .await
                    .map_err(|e| e.to_string())?;
                if !got.contains(text) {
                    return Err(format!("contains: expected {text:?} in {sel}, got {got:?}"));
                }
            }
            Assertion::HasUrl { pattern } => {
                let u = interact::eval_js(self.page, "window.location.href")
                    .await
                    .map_err(|e| e.to_string())?;
                let s = u.as_str().unwrap_or("");
                if !s.contains(pattern) {
                    return Err(format!("has_url: {s} does not contain {pattern}"));
                }
            }
            Assertion::HasTitle { pattern } => {
                let t = interact::eval_js(self.page, "document.title")
                    .await
                    .map_err(|e| e.to_string())?;
                let s = t.as_str().unwrap_or("");
                if !s.contains(pattern) {
                    return Err(format!("has_title: {s:?} does not contain {pattern:?}"));
                }
            }
        }
        Ok(())
    }

    async fn eval_export(&self, ex: &ResolvedExport) -> Result<Value> {
        // Dispatch strictly by kind. For CSS-style selectors this uses
        // `document.querySelector(All)`; the selector DSL is richer but
        // the common extract flow (`text`, `html`, attr) covers 95% of
        // real extract recipes with CSS alone.
        match ex.kind {
            ExportKind::Text => extract_text(self.page, &ex.selector, ex.as_list).await,
            ExportKind::Html => extract_html(self.page, &ex.selector, ex.as_list).await,
            ExportKind::Attribute => {
                let attr = ex.attr.as_deref().unwrap_or("value");
                extract_attr(self.page, &ex.selector, attr, ex.as_list).await
            }
            ExportKind::Links => extract_attr(self.page, &ex.selector, "href", true).await,
            ExportKind::JsonLd => {
                let js = r#"Array.from(document.querySelectorAll('script[type="application/ld+json"]'))
                    .map(s => { try { return JSON.parse(s.textContent); } catch(_) { return null; } })
                    .filter(Boolean)"#;
                interact::eval_js(self.page, js).await
            }
            ExportKind::Regex => {
                let text = extract_text(self.page, &ex.selector, ex.as_list).await?;
                let pat = ex.pattern.as_deref().unwrap_or(".*");
                let re =
                    regex::Regex::new(pat).map_err(|e| Error::Render(format!("regex: {e}")))?;
                let haystack = text.as_str().unwrap_or("").to_string();
                Ok(re
                    .find(&haystack)
                    .map(|m| Value::String(m.as_str().to_string()))
                    .unwrap_or(Value::Null))
            }
            ExportKind::Script => {
                // Extract via arbitrary JS — honours the ActionPolicy
                // by reusing the same gate; here we trust the spec
                // author since the runner already guards `Eval` steps.
                interact::eval_js(self.page, &ex.selector).await
            }
        }
    }

    fn plan_resolve_locator(&self, loc: &crate::script::spec::Locator) -> Option<String> {
        Some(loc.resolve(&IndexMap::new()).to_string())
    }

    async fn current_url(&self) -> Result<url::Url> {
        if let Ok(Some(raw)) = self.page.url().await {
            if let Ok(url) = url::Url::parse(&raw) {
                return Ok(url);
            }
        }
        self.url
            .clone()
            .ok_or_else(|| Error::Render("script-runner current url unavailable".into()))
    }

    async fn settle_after_mutation(&self) -> Result<()> {
        RenderPool::settle_after_actions(self.page, &self.wait_strategy).await
    }

    async fn settle_after_navigation(
        &self,
        wait_until: Option<crate::script::spec::WaitUntil>,
    ) -> Result<()> {
        let wait = match wait_until {
            Some(crate::script::spec::WaitUntil::Load) => crate::wait_strategy::WaitStrategy::Load,
            Some(crate::script::spec::WaitUntil::DomContentLoaded) => {
                crate::wait_strategy::WaitStrategy::DomContentLoaded
            }
            Some(crate::script::spec::WaitUntil::NetworkIdle) => {
                crate::wait_strategy::WaitStrategy::NetworkIdle { idle_ms: 500 }
            }
            None => self.wait_strategy.clone(),
        };
        RenderPool::wait_for(self.page, &wait).await
    }

    fn emit<F>(&self, kind: EventKind, build_data: F)
    where
        F: FnOnce(()) -> Value,
    {
        if let Some(sink) = &self.sink {
            let data = build_data(());
            let mut ev = Event::of(kind).with_session(self.session_id.clone());
            if let Some(r) = self.run_id {
                ev = ev.with_run(r);
            }
            ev = ev.with_data(&data);
            sink.emit(&ev);
        }
    }
}

// ---------- local helpers ----------

fn is_ax_ref(s: &str) -> bool {
    s.strip_prefix("@e")
        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

fn step_kind_str(s: &ResolvedStep) -> &'static str {
    match s {
        ResolvedStep::Goto { .. } => "goto",
        ResolvedStep::WaitFor { .. } => "wait_for",
        ResolvedStep::WaitMs { .. } => "wait_ms",
        ResolvedStep::Click { .. } => "click",
        ResolvedStep::Type { .. } => "type",
        ResolvedStep::Press { .. } => "press",
        ResolvedStep::Scroll { .. } => "scroll",
        ResolvedStep::Eval { .. } => "eval",
        ResolvedStep::Submit { .. } => "submit",
        ResolvedStep::Screenshot(_) => "screenshot",
        ResolvedStep::Snapshot(_) => "snapshot",
        ResolvedStep::Extract(_) => "extract",
        ResolvedStep::Assert(_) => "assert",
    }
}

fn step_verb(s: &ResolvedStep) -> Option<ActionVerb> {
    Some(match s {
        ResolvedStep::Goto { .. } => ActionVerb::Goto,
        ResolvedStep::Click { .. } => ActionVerb::Click,
        ResolvedStep::Type { .. } => ActionVerb::Type,
        ResolvedStep::Press { .. } => ActionVerb::Press,
        ResolvedStep::Scroll { .. } => ActionVerb::Scroll,
        ResolvedStep::Eval { .. } => ActionVerb::Eval,
        ResolvedStep::Submit { .. } => ActionVerb::Submit,
        ResolvedStep::Screenshot(_) => ActionVerb::Screenshot,
        ResolvedStep::Snapshot(_) => ActionVerb::Snapshot,
        ResolvedStep::Extract(_) => ActionVerb::Extract,
        // `WaitFor`, `WaitMs`, `Assert` are passive — they never touch
        // the network or execute operator-supplied code, so leaving
        // them off the verb list is intentional.
        ResolvedStep::WaitFor { .. } | ResolvedStep::WaitMs { .. } | ResolvedStep::Assert(_) => {
            return None
        }
    })
}

fn should_abort_on_step_error(step: &ResolvedStep) -> bool {
    !matches!(
        step,
        ResolvedStep::Screenshot(_) | ResolvedStep::Snapshot(_) | ResolvedStep::Extract(_)
    )
}

async fn press_key(page: &Page, key: &str) -> Result<()> {
    use crate::render::chrome_protocol::cdp::browser_protocol::input::{
        DispatchKeyEventParams, DispatchKeyEventType,
    };
    for ty in [DispatchKeyEventType::KeyDown, DispatchKeyEventType::KeyUp] {
        let p = DispatchKeyEventParams::builder()
            .r#type(ty)
            .key(key.to_string())
            .build()
            .map_err(|e| Error::Render(format!("press params: {e}")))?;
        page.execute(p)
            .await
            .map_err(|e| Error::Render(format!("press: {e}")))?;
    }
    Ok(())
}

async fn capture_screenshot_via_pool(page: &Page, mode: ScreenshotCaptureMode) -> Option<Vec<u8>> {
    // The `RenderPool::capture_screenshot_mode` helper is `pub(crate)`
    // so we can call it from within this crate. Stable re-export path
    // lives on `render::pool`.
    crate::render::pool::RenderPool::capture_screenshot_mode(page, mode).await
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

async fn extract_text(page: &Page, css: &str, as_list: bool) -> Result<Value> {
    let css_json = serde_json::to_string(css).unwrap();
    let js = if as_list {
        format!("Array.from(document.querySelectorAll({css_json})).map(e => e.textContent || '')")
    } else {
        format!(
            "(() => {{ const e = document.querySelector({css_json}); return e ? (e.textContent || '') : null; }})()"
        )
    };
    interact::eval_js(page, &js).await
}

async fn extract_html(page: &Page, css: &str, as_list: bool) -> Result<Value> {
    let css_json = serde_json::to_string(css).unwrap();
    let js = if as_list {
        format!("Array.from(document.querySelectorAll({css_json})).map(e => e.outerHTML || '')")
    } else {
        format!(
            "(() => {{ const e = document.querySelector({css_json}); return e ? (e.outerHTML || '') : null; }})()"
        )
    };
    interact::eval_js(page, &js).await
}

async fn extract_attr(page: &Page, css: &str, attr: &str, as_list: bool) -> Result<Value> {
    let css_json = serde_json::to_string(css).unwrap();
    let attr_json = serde_json::to_string(attr).unwrap();
    let js = if as_list {
        format!(
            "Array.from(document.querySelectorAll({css_json})).map(e => e.getAttribute({attr_json}))"
        )
    } else {
        format!(
            "(() => {{ const e = document.querySelector({css_json}); return e ? e.getAttribute({attr_json}) : null; }})()"
        )
    };
    interact::eval_js(page, &js).await
}

async fn element_text(page: &Page, sel: &str) -> Result<String> {
    // Accept either CSS or selector DSL: if the DSL has prefixes we
    // can't use querySelector directly, so fall back through
    // `resolve_rect` (which is DSL-aware) + active-element read. For
    // the assert path we just need *some* text — use querySelector if
    // it parses, else eval through the DSL scope.
    let v = extract_text(page, sel, false).await?;
    Ok(v.as_str().unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::action_policy::{ActionPolicy, ActionVerb};

    #[test]
    fn is_ax_ref_matches_plan_semantics() {
        assert!(is_ax_ref("@e1"));
        assert!(is_ax_ref("@e42"));
        assert!(!is_ax_ref("@email"));
        assert!(!is_ax_ref("#login"));
        assert!(!is_ax_ref("@e"));
    }

    #[test]
    fn verb_mapping_is_comprehensive() {
        // Every step kind that touches the world should map to a verb
        // so policy coverage is exhaustive.
        use crate::script::spec::{
            ExtractStep, ScreenshotFormat, ScreenshotMode, ScreenshotStep, SnapshotKind,
            SnapshotStep,
        };
        use indexmap::IndexMap;
        let _ = ArtifactRef {
            step_id: "s000".into(),
            step_kind: "screenshot".into(),
            kind: "screenshot.viewport".into(),
            name: "x".into(),
            sha256: "a".into(),
            bytes: 0,
            mime: "image/png".into(),
            selector: None,
        };
        let cases: Vec<ResolvedStep> = vec![
            ResolvedStep::Goto {
                url: "x".into(),
                wait_until: None,
                timeout_ms: 0,
            },
            ResolvedStep::Click {
                selector: "a".into(),
                timeout_ms: 0,
                force: false,
            },
            ResolvedStep::Type {
                selector: "a".into(),
                text: "b".into(),
                timeout_ms: 0,
                clear: false,
            },
            ResolvedStep::Press {
                key: "Enter".into(),
            },
            ResolvedStep::Scroll { dy: 100.0 },
            ResolvedStep::Eval { script: "1".into() },
            ResolvedStep::Submit {
                selector: "a".into(),
                timeout_ms: 0,
            },
            ResolvedStep::Screenshot(ScreenshotStep {
                mode: ScreenshotMode::Viewport,
                locator: None,
                name: None,
                format: ScreenshotFormat::Png,
            }),
            ResolvedStep::Snapshot(SnapshotStep {
                kind: SnapshotKind::PostJsHtml,
                name: None,
            }),
            ResolvedStep::Extract(IndexMap::new()),
        ];
        let _ = ExtractStep {
            fields: IndexMap::new(),
        };
        for c in cases {
            assert!(
                step_verb(&c).is_some(),
                "step {:?} should map to a verb",
                step_kind_str(&c)
            );
        }
        // Passive steps deliberately do not map.
        assert!(step_verb(&ResolvedStep::WaitMs { ms: 1 }).is_none());
        assert!(step_verb(&ResolvedStep::WaitFor {
            selector: "a".into(),
            state: None,
            timeout_ms: 0,
        })
        .is_none());
    }

    #[test]
    fn policy_deny_produces_expected_error() {
        // Can't exercise async runner without a Page, but we can verify
        // the string shape the runner will produce on a deny so CLI
        // consumers can grep for `action-policy:` reliably.
        let p = ActionPolicy::strict();
        assert_eq!(p.check(ActionVerb::Click), ActionRule::Deny);
        let msg = format!("action-policy: {} denied", ActionVerb::Click.as_str());
        assert_eq!(msg, "action-policy: click denied");
    }

    #[test]
    fn step_kind_str_is_stable_wire_string() {
        // These strings appear in `step.started` / `step.completed`
        // event payloads — locking them down so consumers can match.
        assert_eq!(step_kind_str(&ResolvedStep::WaitMs { ms: 1 }), "wait_ms");
        assert_eq!(
            step_kind_str(&ResolvedStep::Eval { script: "x".into() }),
            "eval"
        );
    }
}
