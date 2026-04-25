//! Lua hook runtime with page interaction bindings.
//!
//! With the `send` feature mlua's `Lua` state is Send+Sync behind an internal
//! lock, so we keep a single `Arc<Mutex<Lua>>` shared across tokio tasks.
//! Per-call we:
//! 1. Drop the current Page into a shared slot.
//! 2. Run `spawn_blocking` with the Lua mutex locked.
//! 3. Registered globals (`page_click`, `page_type`, …) read the slot and
//!    bridge back to async via `Handle::current().block_on` since the blocking
//!    thread is inside a multi-thread tokio runtime.
//!
//! Conventions:
//! * Hook function names: `on_before_request`, `on_after_first_byte`,
//!   `on_after_load`, `on_after_idle`, `on_discovery`, `on_error`, `on_robots`.
//! * Return: `nil|"continue"` → continue; `"skip"`, `"retry"`, `"abort"` take
//!   effect; anything else is treated as continue.
//! * Inside the hook, globals available when rendering:
//!   - `page_click(selector)`  → bool
//!   - `page_type(selector, text)` → bool
//!   - `page_wait(selector, timeout_ms)` → bool  (legacy name)
//!   - `page_wait_for(selector, timeout_ms)` → bool  (alias of `page_wait`)
//!   - `page_eval(js)` → decoded JSON value
//!   - `page_scroll(dy)` → nil
//!   - `page_content()` → string (current DOM serialised via `page.content`)
//!   - `page_goto(url)` → bool (navigate, wait for load state)
//!   - `page_screenshot(mode?)` → string | nil (base64 PNG;
//!     mode = `"viewport"` | `"fullpage"` (default) | `"element:<selector>"`)
//!   - `page_screenshot_save(mode?, name?)` → string | nil (persists via
//!     the active Storage backend and returns the URL key used, avoiding
//!     the base64 round-trip; hash-aware via `window.location.href`)
//!   - `page_ax_snapshot()` → string | nil (captures accessibility tree,
//!     stashes `@eN` → backendDOMNodeId map, returns rendered tree text;
//!     subsequent `page_click("@e3")` / `page_type("@e5", "...")` resolve
//!     via this map without another DOM round-trip)
//!   Selectors accept the full DSL (`role=button[name="X"]`, `text="..."`,
//!   `label="Email"`, `xpath=//...`, `|visible`, chain with ` >> `), plus
//!   `@eN` refs from the AX snapshot layered on top of click/type.

#![cfg(feature = "lua-hooks")]

use crate::render::chrome::page::Page;
use mlua::{Function, Lua, LuaSerdeExt, Table, Value};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Handle;

use crate::hooks::{HookContext, HookDecision, HookEvent};
use crate::storage::Storage;
use crate::Error;

/// Ref-map stash: the most recently captured AX snapshot's
/// `@eN → backendDOMNodeId` map, populated by `page_ax_snapshot()` and
/// read by `page_click`/`page_type` when they see an `@eN` argument.
/// Replaced wholesale on each snapshot; a Lua flow that navigates should
/// re-snapshot before addressing new refs.
type RefMap = Arc<Mutex<BTreeMap<String, i64>>>;

pub struct LuaHookHost {
    lua: Arc<Mutex<Lua>>,
    /// Shared slot: Lua-bridged page helpers read from here on each call.
    /// Populated by `fire_with_page`, cleared after.
    current_page: Arc<Mutex<Option<Page>>>,
    /// Storage handle for `page_screenshot_save` — `None` when the host
    /// was built without storage wiring (old constructor path kept for
    /// tests that don't need persistence). The live reference used by
    /// Lua lives inside the registered closures; this field is retained
    /// so the Arc's refcount matches the host lifetime (closures outlive
    /// the constructor scope).
    #[allow(dead_code)]
    storage: Option<Arc<dyn Storage>>,
    /// Latest AX snapshot ref map. See [`RefMap`] docs. Same lifetime
    /// rationale as `storage`.
    #[allow(dead_code)]
    ref_map: RefMap,
}

fn event_fn_name(event: HookEvent) -> &'static str {
    use HookEvent::*;
    match event {
        BeforeEachRequest => "on_before_request",
        AfterDnsResolve => "on_after_dns",
        AfterTlsHandshake => "on_after_tls",
        AfterFirstByte => "on_after_first_byte",
        OnResponseBody => "on_response_body",
        AfterLoad => "on_after_load",
        AfterIdle => "on_after_idle",
        OnDiscovery => "on_discovery",
        OnJobStart => "on_job_start",
        OnJobEnd => "on_job_end",
        OnError => "on_error",
        OnRobotsDecision => "on_robots",
    }
}

impl LuaHookHost {
    pub fn new(scripts: Vec<PathBuf>) -> std::result::Result<Self, String> {
        Self::new_with_storage(scripts, None)
    }

    /// Construct a host with storage wiring so hooks can call
    /// `page_screenshot_save(...)` and have the bytes land in whatever
    /// backend (filesystem, sqlite, memory) the crawler was configured
    /// with. Identical to [`Self::new`] except for the storage slot.
    pub fn new_with_storage(
        scripts: Vec<PathBuf>,
        storage: Option<Arc<dyn Storage>>,
    ) -> std::result::Result<Self, String> {
        let lua = Lua::new();
        let current_page: Arc<Mutex<Option<Page>>> = Arc::new(Mutex::new(None));
        let ref_map: RefMap = Arc::new(Mutex::new(BTreeMap::new()));
        register_page_globals(&lua, current_page.clone(), storage.clone(), ref_map.clone())
            .map_err(|e| format!("register bindings: {e}"))?;
        for p in &scripts {
            let source = std::fs::read_to_string(p).map_err(|e| format!("read {p:?}: {e}"))?;
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("hook");
            lua.load(&source)
                .set_name(name)
                .exec()
                .map_err(|e| format!("{name}: {e}"))?;
        }
        Ok(Self {
            lua: Arc::new(Mutex::new(lua)),
            current_page,
            storage,
            ref_map,
        })
    }

    pub async fn fire(
        &self,
        event: HookEvent,
        ctx: &mut HookContext,
    ) -> crate::Result<HookDecision> {
        self.fire_with_page(event, ctx, None).await
    }

    pub async fn fire_with_page(
        &self,
        event: HookEvent,
        ctx: &mut HookContext,
        page: Option<Page>,
    ) -> crate::Result<HookDecision> {
        // Snapshot ctx fields we want to expose.
        let url = ctx.url.to_string();
        let depth = ctx.depth;
        let status = ctx.response_status;
        let body_bytes = ctx.body.as_ref().map(|b| b.len());
        let html = ctx.html_post_js.clone();
        let captured = ctx
            .captured_urls
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>();
        let proxy = ctx.proxy.as_ref().map(|u| u.to_string());
        let error = ctx.error.clone();
        let retry_count = ctx.retry_count;

        let lua = self.lua.clone();
        let current_page = self.current_page.clone();
        let name = event_fn_name(event);

        let (decision, extra_urls, allow_retry) =
            tokio::task::spawn_blocking(move || -> std::result::Result<_, String> {
                *current_page.lock() = page;
                let g = lua.lock();
                let r = invoke(
                    &*g,
                    name,
                    &url,
                    depth,
                    status,
                    body_bytes,
                    &html,
                    &captured,
                    proxy.as_deref(),
                    error.as_deref(),
                    retry_count,
                );
                *current_page.lock() = None;
                r
            })
            .await
            .map_err(|e| Error::HookAbort(format!("lua join: {e}")))?
            .map_err(Error::HookAbort)?;

        if let Some(v) = allow_retry {
            ctx.allow_retry = v;
        }
        for s in extra_urls {
            if let Ok(u) = url::Url::parse(&s) {
                ctx.captured_urls.push(u);
            }
        }
        Ok(decision)
    }
}

fn invoke(
    lua: &Lua,
    name: &str,
    url: &str,
    depth: u32,
    status: Option<u16>,
    body_bytes: Option<usize>,
    html: &Option<String>,
    captured: &[String],
    proxy: Option<&str>,
    error: Option<&str>,
    retry_count: u32,
) -> std::result::Result<(HookDecision, Vec<String>, Option<bool>), String> {
    let globals = lua.globals();
    let func: Function = match globals.get(name) {
        Ok(f) => f,
        Err(_) => return Ok((HookDecision::Continue, Vec::new(), None)),
    };
    let ctx_tbl = lua.create_table().map_err(|e| e.to_string())?;
    ctx_tbl.set("url", url).map_err(|e| e.to_string())?;
    ctx_tbl.set("depth", depth).map_err(|e| e.to_string())?;
    ctx_tbl.set("status", status).map_err(|e| e.to_string())?;
    ctx_tbl
        .set("body_bytes", body_bytes)
        .map_err(|e| e.to_string())?;
    ctx_tbl
        .set("html", html.clone())
        .map_err(|e| e.to_string())?;
    let urls_tbl = lua.create_table().map_err(|e| e.to_string())?;
    for (i, u) in captured.iter().enumerate() {
        urls_tbl.set(i + 1, u.clone()).map_err(|e| e.to_string())?;
    }
    ctx_tbl
        .set("captured_urls", urls_tbl)
        .map_err(|e| e.to_string())?;
    ctx_tbl.set("proxy", proxy).map_err(|e| e.to_string())?;
    ctx_tbl.set("error", error).map_err(|e| e.to_string())?;
    ctx_tbl
        .set("retry_count", retry_count)
        .map_err(|e| e.to_string())?;

    let ret: Value = func
        .call(ctx_tbl.clone())
        .map_err(|e| format!("{name}: {e}"))?;

    let allow_retry = ctx_tbl.get::<Option<bool>>("allow_retry").ok().flatten();
    let mut extra = Vec::new();
    if let Ok(Some(tbl)) = ctx_tbl.get::<Option<Table>>("extra_urls") {
        for s in tbl.sequence_values::<String>().flatten() {
            extra.push(s);
        }
    }

    let decision = match ret {
        Value::Nil => HookDecision::Continue,
        Value::String(s) => {
            match s
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_default()
                .as_str()
            {
                "skip" => HookDecision::Skip,
                "retry" => HookDecision::Retry,
                "abort" => HookDecision::Abort,
                _ => HookDecision::Continue,
            }
        }
        _ => HookDecision::Continue,
    };
    Ok((decision, extra, allow_retry))
}

/// Build the Lua globals that bridge into the Rust `Page` instance.
fn register_page_globals(
    lua: &Lua,
    slot: Arc<Mutex<Option<Page>>>,
    storage: Option<Arc<dyn Storage>>,
    ref_map: RefMap,
) -> mlua::Result<()> {
    let g = lua.globals();

    {
        let slot = slot.clone();
        let ref_map = ref_map.clone();
        g.set(
            "page_click",
            lua.create_function(move |_, sel: String| {
                let page = slot.lock().clone();
                let rmap = ref_map.clone();
                Ok(run_blocking(async move {
                    let Some(p) = page else { return false };
                    use crate::render::interact::{click_selector, MousePos};
                    // `@eN` → AX-ref path: resolve via stashed snapshot
                    // map, not the selector engine. `@eNsomething` falls
                    // through since it isn't a pure digit suffix.
                    if let Some(ref_id) = ax_ref_of(&sel) {
                        let bnid = {
                            let guard = rmap.lock();
                            crate::render::ref_resolver::lookup_backend_node_id(ref_id, &guard)
                        };
                        return match bnid {
                            Some(id) => crate::render::ref_resolver::click_by_backend_node(
                                &p,
                                id,
                                MousePos { x: 100.0, y: 100.0 },
                            )
                            .await
                            .is_ok(),
                            None => false,
                        };
                    }
                    click_selector(&p, &sel, MousePos { x: 100.0, y: 100.0 })
                        .await
                        .is_ok()
                }))
            })?,
        )?;
    }
    {
        let slot = slot.clone();
        let ref_map = ref_map.clone();
        g.set(
            "page_type",
            lua.create_function(move |_, (sel, text): (String, String)| {
                let page = slot.lock().clone();
                let rmap = ref_map.clone();
                Ok(run_blocking(async move {
                    let Some(p) = page else { return false };
                    if let Some(ref_id) = ax_ref_of(&sel) {
                        let bnid = {
                            let guard = rmap.lock();
                            crate::render::ref_resolver::lookup_backend_node_id(ref_id, &guard)
                        };
                        return match bnid {
                            Some(id) => {
                                crate::render::ref_resolver::type_by_backend_node(&p, id, &text)
                                    .await
                                    .is_ok()
                            }
                            None => false,
                        };
                    }
                    crate::render::interact::type_text(&p, &sel, &text)
                        .await
                        .is_ok()
                }))
            })?,
        )?;
    }
    {
        let slot = slot.clone();
        g.set(
            "page_wait",
            lua.create_function(move |_, (sel, timeout_ms): (String, u64)| {
                let page = slot.lock().clone();
                Ok(run_blocking(async move {
                    match page {
                        Some(p) => crate::render::interact::wait_for_selector(&p, &sel, timeout_ms)
                            .await
                            .is_ok(),
                        None => false,
                    }
                }))
            })?,
        )?;
    }
    // Alias — `page_wait_for` reads more naturally in flow scripts and
    // matches Playwright-style naming. Same semantics as `page_wait`.
    {
        let slot = slot.clone();
        g.set(
            "page_wait_for",
            lua.create_function(move |_, (sel, timeout_ms): (String, u64)| {
                let page = slot.lock().clone();
                Ok(run_blocking(async move {
                    match page {
                        Some(p) => crate::render::interact::wait_for_selector(&p, &sel, timeout_ms)
                            .await
                            .is_ok(),
                        None => false,
                    }
                }))
            })?,
        )?;
    }
    // Serialise the current DOM. Returns empty string on failure rather than
    // raising so scripts can guard with `if html == "" then ...` idioms.
    {
        let slot = slot.clone();
        g.set(
            "page_content",
            lua.create_function(move |_, ()| {
                let page = slot.lock().clone();
                Ok(run_blocking(async move {
                    match page {
                        Some(p) => p.content().await.unwrap_or_default(),
                        None => String::new(),
                    }
                }))
            })?,
        )?;
    }
    // Navigate the live page. Useful for flows that need to jump between
    // routes inside a single hook invocation (multi-step login, OAuth
    // callback handling). Returns true on success.
    {
        let slot = slot.clone();
        g.set(
            "page_goto",
            lua.create_function(move |_, target: String| {
                let page = slot.lock().clone();
                Ok(run_blocking(async move {
                    match page {
                        Some(p) => p.goto(target).await.is_ok(),
                        None => false,
                    }
                }))
            })?,
        )?;
    }
    // On-demand screenshot from Lua. Returns the base64-encoded PNG (the
    // raw bytes would round-trip through Lua as a binary string with
    // embedded NULs, which hook authors rarely want). Mode matches the CLI
    // flag: `viewport`, `fullpage` (default), `element:<css>`.
    {
        let slot = slot.clone();
        g.set(
            "page_screenshot",
            lua.create_function(move |_, mode_arg: Option<String>| {
                let page = slot.lock().clone();
                let mode =
                    crate::render::pool::parse_screenshot_mode_or_default(mode_arg.as_deref());
                Ok(run_blocking(async move {
                    match page {
                        Some(p) => {
                            match crate::render::pool::RenderPool::capture_screenshot_mode(&p, mode)
                                .await
                            {
                                Some(bytes) => {
                                    use base64::Engine;
                                    Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
                                }
                                None => None,
                            }
                        }
                        None => None,
                    }
                }))
            })?,
        )?;
    }
    // Capture an accessibility-tree snapshot, stash the `@eN` → backendNodeId
    // map for subsequent `page_click("@e3")` / `page_type("@e5", ...)` calls,
    // and return the rendered text tree so the Lua (or an LLM downstream)
    // can read it. Returns `nil` when no page is bound.
    {
        let slot = slot.clone();
        let ref_map = ref_map.clone();
        g.set(
            "page_ax_snapshot",
            lua.create_function(move |_, ()| {
                let page = slot.lock().clone();
                let rmap = ref_map.clone();
                Ok(run_blocking(async move {
                    let p = page?;
                    let opts = crate::render::ax_snapshot::SnapshotOptions::default();
                    match crate::render::ax_snapshot::capture_ax_snapshot(&p, &opts).await {
                        Ok(snap) => {
                            let text = snap.render_tree();
                            *rmap.lock() = snap.ref_map;
                            Some(text)
                        }
                        Err(_) => None,
                    }
                }))
            })?,
        )?;
    }
    // Persist a screenshot via the active `Storage` without a base64 round-trip
    // through Lua. Returns the URL string we keyed the save under, or `nil`
    // on failure / no storage. `name` is preserved as the artifact label in
    // the unified artifacts table; with no name supplied we fall back to the
    // URL as before so existing Lua scripts keep working.
    {
        let slot = slot.clone();
        let storage = storage.clone();
        g.set(
            "page_screenshot_save",
            lua.create_function(
                move |_, (mode_arg, name): (Option<String>, Option<String>)| {
                    let page = slot.lock().clone();
                    let storage = storage.clone();
                    let mode =
                        crate::render::pool::parse_screenshot_mode_or_default(mode_arg.as_deref());
                    Ok(run_blocking(async move {
                        let p = page?;
                        let store = storage?;
                        let (art_kind, selector) = match &mode {
                            crate::render::pool::ScreenshotCaptureMode::Viewport => {
                                (crate::storage::ArtifactKind::ScreenshotViewport, None)
                            }
                            crate::render::pool::ScreenshotCaptureMode::FullPage => {
                                (crate::storage::ArtifactKind::ScreenshotFullPage, None)
                            }
                            crate::render::pool::ScreenshotCaptureMode::Element { selector } => (
                                crate::storage::ArtifactKind::ScreenshotElement,
                                Some(selector.clone()),
                            ),
                        };
                        let bytes =
                            crate::render::pool::RenderPool::capture_screenshot_mode(&p, mode)
                                .await?;
                        let current_url = match crate::render::interact::eval_js(
                            &p,
                            "window.location.href",
                        )
                        .await
                        {
                            Ok(v) => v.as_str().map(|s| s.to_string()),
                            Err(_) => None,
                        };
                        let current_url = match current_url {
                            Some(u) => Some(u),
                            None => p.url().await.ok().flatten(),
                        };
                        let u = current_url.and_then(|s| url::Url::parse(&s).ok())?;
                        // Back-compat: fill the legacy screenshots table
                        // (ignored by save_artifact-only backends).
                        let _ = store.save_screenshot(&u, &bytes).await;
                        let session_id = crate::storage::session_id_for_url(&u);
                        let meta = crate::storage::ArtifactMeta {
                            url: &u,
                            final_url: None,
                            session_id: &session_id,
                            kind: art_kind,
                            name: name.as_deref(),
                            step_id: None,
                            step_kind: Some("lua"),
                            selector: selector.as_deref(),
                            mime: None,
                        };
                        match store.save_artifact(&meta, &bytes).await {
                            Ok(()) => Some(u.to_string()),
                            Err(_) => None,
                        }
                    }))
                },
            )?,
        )?;
    }
    // New: persist a non-screenshot snapshot (html, state, ax_tree) via
    // the unified `save_artifact` pipeline so Lua scripts can capture
    // all artifact kinds, not just PNGs.
    {
        let slot = slot.clone();
        let storage = storage.clone();
        let rmap_hook = ref_map.clone();
        g.set(
            "page_snapshot_save",
            lua.create_function(
                move |_, (kind_arg, name): (String, Option<String>)| {
                    let page = slot.lock().clone();
                    let storage = storage.clone();
                    let rmap = rmap_hook.clone();
                    Ok(run_blocking(async move {
                        let p = page?;
                        let store = storage?;
                        // Supported kinds: "html" (post-JS), "state"
                        // (cookies + local/sessionStorage), "ax_tree".
                        let (art_kind, payload): (
                            crate::storage::ArtifactKind,
                            Vec<u8>,
                        ) = match kind_arg.as_str() {
                            "html" | "post_js_html" => {
                                let html = p.content().await.ok()?;
                                (
                                    crate::storage::ArtifactKind::SnapshotPostJsHtml,
                                    html.into_bytes(),
                                )
                            }
                            "state" => {
                                let js = r#"(() => {
                                    const out = { cookie: document.cookie, localStorage: {}, sessionStorage: {} };
                                    try { for (const k in localStorage) out.localStorage[k] = localStorage.getItem(k); } catch (_) {}
                                    try { for (const k in sessionStorage) out.sessionStorage[k] = sessionStorage.getItem(k); } catch (_) {}
                                    return out;
                                })()"#;
                                let v = crate::render::interact::eval_js(&p, js).await.ok()?;
                                let body = serde_json::to_vec(&v).unwrap_or_default();
                                (crate::storage::ArtifactKind::SnapshotState, body)
                            }
                            "ax_tree" => {
                                let opts =
                                    crate::render::ax_snapshot::SnapshotOptions::default();
                                let snap = crate::render::ax_snapshot::capture_ax_snapshot(
                                    &p, &opts,
                                )
                                .await
                                .ok()?;
                                *rmap.lock() = snap.ref_map.clone();
                                (
                                    crate::storage::ArtifactKind::SnapshotAxTree,
                                    snap.render_tree().into_bytes(),
                                )
                            }
                            _ => return None,
                        };
                        let current_url = match crate::render::interact::eval_js(
                            &p,
                            "window.location.href",
                        )
                        .await
                        {
                            Ok(v) => v.as_str().map(|s| s.to_string()),
                            Err(_) => None,
                        };
                        let current_url = match current_url {
                            Some(u) => Some(u),
                            None => p.url().await.ok().flatten(),
                        };
                        let u = current_url.and_then(|s| url::Url::parse(&s).ok())?;
                        let session_id = crate::storage::session_id_for_url(&u);
                        let meta = crate::storage::ArtifactMeta {
                            url: &u,
                            final_url: None,
                            session_id: &session_id,
                            kind: art_kind,
                            name: name.as_deref(),
                            step_id: None,
                            step_kind: Some("lua"),
                            selector: None,
                            mime: None,
                        };
                        match store.save_artifact(&meta, &payload).await {
                            Ok(()) => Some(u.to_string()),
                            Err(_) => None,
                        }
                    }))
                },
            )?,
        )?;
    }
    {
        let slot = slot.clone();
        g.set(
            "page_eval",
            lua.create_function(move |lua, script: String| {
                let page = slot.lock().clone();
                let v = run_blocking(async move {
                    match page {
                        Some(p) => crate::render::interact::eval_js(&p, &script)
                            .await
                            .unwrap_or(serde_json::Value::Null),
                        None => serde_json::Value::Null,
                    }
                });
                lua.to_value(&v)
            })?,
        )?;
    }
    {
        let slot = slot.clone();
        g.set(
            "page_scroll",
            lua.create_function(move |_, dy: f64| {
                let page = slot.lock().clone();
                run_blocking(async move {
                    if let Some(p) = page {
                        let _ = crate::render::interact::scroll_by(
                            &p,
                            dy,
                            crate::render::interact::MousePos { x: 400.0, y: 400.0 },
                        )
                        .await;
                    }
                });
                Ok(())
            })?,
        )?;
    }
    Ok(())
}

/// Detect the `@eN` AX-ref shape the snapshot emits — digits only, no
/// suffix. We deliberately reject `@e1foo` so a future extension to the
/// selector DSL can't collide with us.
fn ax_ref_of(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("@e")?;
    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
        Some(s)
    } else {
        None
    }
}

/// Bridge sync → async. We're always invoked inside `spawn_blocking`, so a
/// multi-thread tokio runtime is running and `Handle::current` is available.
fn run_blocking<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    Handle::current().block_on(fut)
}
