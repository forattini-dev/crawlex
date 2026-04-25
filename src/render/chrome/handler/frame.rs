use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::map::Entry;

use crate::render::chrome_protocol::cdp::browser_protocol::network::LoaderId;
use crate::render::chrome_protocol::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, CreateIsolatedWorldParams, EventFrameDetached,
    EventFrameStartedLoading, EventFrameStoppedLoading, EventLifecycleEvent,
    EventNavigatedWithinDocument, Frame as CdpFrame, FrameTree,
};
use crate::render::chrome_protocol::cdp::browser_protocol::target::EventAttachedToTarget;
use crate::render::chrome_protocol::cdp::js_protocol::runtime::*;
use crate::render::chrome_protocol::cdp::{
    browser_protocol::page::{self, FrameId},
    js_protocol::runtime,
};
use crate::render::chrome_wire::{Method, MethodId, Request};

use crate::render::chrome::error::DeadlineExceeded;
use crate::render::chrome::handler::domworld::DOMWorld;
use crate::render::chrome::handler::http::HttpRequest;
use crate::render::chrome::handler::{request_timeout_ms, REQUEST_TIMEOUT};
use crate::render::chrome::{cmd::CommandChain, ArcHttpRequest};

pub const UTILITY_WORLD_NAME: &str = "__ctx_world__";
const EVALUATION_SCRIPT_URL: &str = "____ctx_world___evaluation_script__";

/// Represents a frame on the page
#[derive(Debug)]
pub struct Frame {
    parent_frame: Option<FrameId>,
    /// Cdp identifier of this frame
    id: FrameId,
    main_world: DOMWorld,
    secondary_world: DOMWorld,
    loader_id: Option<LoaderId>,
    /// Current url of this frame
    url: Option<String>,
    /// The http request that loaded this with this frame
    http_request: ArcHttpRequest,
    /// The frames contained in this frame
    child_frames: HashSet<FrameId>,
    name: Option<String>,
    /// The received lifecycle events
    lifecycle_events: HashSet<MethodId>,
}

impl Frame {
    pub fn new(id: FrameId) -> Self {
        Self {
            parent_frame: None,
            id,
            main_world: Default::default(),
            secondary_world: Default::default(),
            loader_id: None,
            url: None,
            http_request: None,
            child_frames: Default::default(),
            name: None,
            lifecycle_events: Default::default(),
        }
    }

    pub fn with_parent(id: FrameId, parent: &mut Frame) -> Self {
        parent.child_frames.insert(id.clone());
        Self {
            parent_frame: Some(parent.id.clone()),
            id,
            main_world: Default::default(),
            secondary_world: Default::default(),
            loader_id: None,
            url: None,
            http_request: None,
            child_frames: Default::default(),
            name: None,
            lifecycle_events: Default::default(),
        }
    }

    pub fn parent_id(&self) -> Option<&FrameId> {
        self.parent_frame.as_ref()
    }

    pub fn id(&self) -> &FrameId {
        &self.id
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn main_world(&self) -> &DOMWorld {
        &self.main_world
    }

    pub fn secondary_world(&self) -> &DOMWorld {
        &self.secondary_world
    }

    pub fn lifecycle_events(&self) -> &HashSet<MethodId> {
        &self.lifecycle_events
    }

    pub fn http_request(&self) -> Option<&Arc<HttpRequest>> {
        self.http_request.as_ref()
    }

    fn navigated(&mut self, frame: &CdpFrame) {
        self.name.clone_from(&frame.name);
        let url = if let Some(ref fragment) = frame.url_fragment {
            format!("{}{fragment}", frame.url)
        } else {
            frame.url.clone()
        };
        self.url = Some(url);
        // NOTE(crawlex vendor patch): Chrome 149+ stopped re-emitting
        // `Page.lifecycleEvent` for post-navigation lifecycle names; we
        // propagate loader_id here so NavigationWatcher can detect the
        // loader change and combine with `Page.loadEventFired` (see
        // on_page_load_event_fired).
        self.loader_id = Some(frame.loader_id.clone());
        self.lifecycle_events.clear();
    }

    fn navigated_within_url(&mut self, url: String) {
        self.url = Some(url)
    }

    fn on_loading_stopped(&mut self) {
        self.lifecycle_events.insert("DOMContentLoaded".into());
        self.lifecycle_events.insert("load".into());
    }

    fn on_loading_started(&mut self) {
        self.lifecycle_events.clear();
        self.http_request.take();
    }

    pub fn is_loaded(&self) -> bool {
        self.lifecycle_events.contains("load")
    }

    pub fn clear_contexts(&mut self) {
        self.main_world.take_context();
        self.secondary_world.take_context();
    }

    pub fn destroy_context(&mut self, ctx_unique_id: &str) {
        if self.main_world.execution_context_unique_id() == Some(ctx_unique_id) {
            self.main_world.take_context();
        } else if self.secondary_world.execution_context_unique_id() == Some(ctx_unique_id) {
            self.secondary_world.take_context();
        }
    }

    pub fn execution_context(&self) -> Option<ExecutionContextId> {
        self.main_world.execution_context()
    }

    pub fn set_request(&mut self, request: HttpRequest) {
        self.http_request = Some(Arc::new(request))
    }
}

impl From<CdpFrame> for Frame {
    fn from(frame: CdpFrame) -> Self {
        Self {
            parent_frame: frame.parent_id,
            id: frame.id,
            main_world: Default::default(),
            secondary_world: Default::default(),
            loader_id: Some(frame.loader_id),
            url: Some(frame.url),
            http_request: None,
            child_frames: Default::default(),
            name: frame.name,
            lifecycle_events: Default::default(),
        }
    }
}

/// Maintains the state of the pages frame and listens to events produced by
/// chromium targeting the `Target`. Also listens for events that indicate that
/// a navigation was completed
#[derive(Debug)]
pub struct FrameManager {
    main_frame: Option<FrameId>,
    frames: HashMap<FrameId, Frame>,
    /// The contexts mapped with their frames
    context_ids: HashMap<String, FrameId>,
    isolated_worlds: HashSet<String>,
    /// Pending isolated-world responses to correlate with frames.
    /// `ensure_isolated_world` pushes one entry per `Page.createIsolatedWorld`
    /// request it emits. On response, we pop-front and bind the returned
    /// `executionContextId` to that frame's secondary world. Enables
    /// stealth mode (Runtime.enable suppressed → no
    /// `Runtime.executionContextCreated` events) to still resolve an
    /// isolated context for evaluate calls.
    pending_isolated_world_frames: VecDeque<(u64, FrameId)>,
    /// Monotonic epoch of the current isolated-world chain. Bumped each
    /// time `reset_isolated_world_state` fires so responses from a
    /// superseded chain can be dropped rather than bound to the wrong
    /// frame.
    isolated_world_epoch: u64,
    /// Timeout after which an anticipated event (related to navigation) doesn't
    /// arrive results in an error
    request_timeout: Duration,
    /// Track currently in progress navigation
    pending_navigations: VecDeque<(FrameNavigationRequest, NavigationWatcher)>,
    /// The currently ongoing navigation
    navigation: Option<(NavigationWatcher, Instant)>,
}

impl FrameManager {
    pub fn new(request_timeout: Duration) -> Self {
        FrameManager {
            main_frame: None,
            frames: Default::default(),
            context_ids: Default::default(),
            isolated_worlds: Default::default(),
            pending_isolated_world_frames: Default::default(),
            isolated_world_epoch: 0,
            request_timeout,
            pending_navigations: Default::default(),
            navigation: None,
        }
    }

    /// The commands to execute in order to initialize this frame manager.
    ///
    /// Includes `Runtime.enable` — this is the **upstream chromiumoxide
    /// default** and the reason crawlex is detectable by brotector-style
    /// probes (Runtime.enable injects stack-trace signatures that
    /// fingerprinters read via `Error.prepareStackTrace`). Callers that
    /// want stealth should prefer [`Self::init_commands_stealth`] and
    /// resolve execution contexts via isolated worlds + bindings, per
    /// the rebrowser-patches pattern.
    pub fn init_commands(timeout: Duration) -> CommandChain {
        let enable = page::EnableParams::default();
        let get_tree = page::GetFrameTreeParams::default();
        let set_lifecycle = page::SetLifecycleEventsEnabledParams::new(true);
        let enable_runtime = runtime::EnableParams::default();
        CommandChain::new(
            vec![
                (enable.identifier(), serde_json::to_value(enable).unwrap()),
                (
                    get_tree.identifier(),
                    serde_json::to_value(get_tree).unwrap(),
                ),
                (
                    set_lifecycle.identifier(),
                    serde_json::to_value(set_lifecycle).unwrap(),
                ),
                (
                    enable_runtime.identifier(),
                    serde_json::to_value(enable_runtime).unwrap(),
                ),
            ],
            timeout,
        )
    }

    /// Stealth variant of [`Self::init_commands`] — omits `Runtime.enable`
    /// so fingerprinters can't infer CDP attachment via stack-trace
    /// signatures or `Error.prepareStackTrace` sniffing.
    ///
    /// Cost: downstream code can no longer rely on
    /// `Runtime.executionContextCreated` to populate `Frame.execution_context`
    /// automatically; the execution context must be resolved on-demand via
    /// [`Page::createIsolatedWorld`] (returns contextId directly) or by
    /// `Runtime.addBinding` + a script installed through
    /// `Page.addScriptToEvaluateOnNewDocument` that calls back with the
    /// context ID. Until that migration is complete, opt-in behind config
    /// rather than making it the default.
    ///
    /// Reference: <https://rebrowser.net/blog/how-to-fix-runtime-enable-cdp-detection-of-puppeteer-playwright-and-other-automation-libraries-61740>
    pub fn init_commands_stealth(timeout: Duration) -> CommandChain {
        let enable = page::EnableParams::default();
        let get_tree = page::GetFrameTreeParams::default();
        let set_lifecycle = page::SetLifecycleEventsEnabledParams::new(true);
        CommandChain::new(
            vec![
                (enable.identifier(), serde_json::to_value(enable).unwrap()),
                (
                    get_tree.identifier(),
                    serde_json::to_value(get_tree).unwrap(),
                ),
                (
                    set_lifecycle.identifier(),
                    serde_json::to_value(set_lifecycle).unwrap(),
                ),
            ],
            timeout,
        )
    }
}

#[cfg(test)]
mod stealth_init_tests {
    use super::FrameManager;
    use std::time::Duration;

    #[test]
    fn default_init_includes_runtime_enable() {
        let chain = FrameManager::init_commands(Duration::from_secs(5));
        let ids = chain.method_identifiers();
        assert!(
            ids.iter().any(|m| m == "Runtime.enable"),
            "default init must emit Runtime.enable for chromiumoxide compatibility: {ids:?}"
        );
    }

    #[test]
    fn stealth_init_omits_runtime_enable() {
        let chain = FrameManager::init_commands_stealth(Duration::from_secs(5));
        let ids = chain.method_identifiers();
        assert!(
            !ids.iter().any(|m| m == "Runtime.enable"),
            "stealth init MUST NOT emit Runtime.enable (rebrowser-patches P0): {ids:?}"
        );
        // The lifecycle + frame-tree setup still needs to happen — they
        // don't expose CDP via Error.prepareStackTrace the way Runtime
        // does.
        assert!(ids.iter().any(|m| m == "Page.enable"));
        assert!(ids.iter().any(|m| m == "Page.getFrameTree"));
        assert!(ids.iter().any(|m| m == "Page.setLifecycleEventsEnabled"));
    }

    #[test]
    fn stealth_init_length_is_three_commands() {
        // Regression guard: if a future refactor re-adds Runtime.enable to
        // the stealth path without updating this test, the count check
        // catches it.
        let chain = FrameManager::init_commands_stealth(Duration::from_secs(5));
        assert_eq!(
            chain.method_identifiers().len(),
            3,
            "stealth init should fire exactly 3 commands (page.enable, getFrameTree, setLifecycle)"
        );
    }

    #[test]
    fn browser_config_default_does_not_skip_runtime_enable() {
        // The default path has to stay compatible with every caller that
        // currently depends on `Runtime.executionContextCreated` — so the
        // flag must default to false and the builder must leave it off
        // unless `stealth_runtime_enable_skip(true)` is called.
        let cfg = crate::render::chrome::browser::BrowserConfig::builder()
            .chrome_executable("/bin/false")
            .build()
            .expect("build");
        assert!(
            !cfg.stealth_runtime_enable_skip(),
            "stealth flag must default to false to keep evaluate() working"
        );
    }

    #[test]
    fn browser_config_builder_propagates_stealth_flag() {
        let cfg = crate::render::chrome::browser::BrowserConfig::builder()
            .chrome_executable("/bin/false")
            .stealth_runtime_enable_skip(true)
            .build()
            .expect("build");
        assert!(
            cfg.stealth_runtime_enable_skip(),
            "builder setter must flow into the final config"
        );
    }

    #[test]
    fn ensure_isolated_world_tracks_frame_order() {
        use crate::render::chrome::handler::frame::{Frame as InternalFrame, UTILITY_WORLD_NAME};
        use crate::render::chrome_protocol::cdp::browser_protocol::page::FrameId;
        let mut fm = super::FrameManager::new(Duration::from_secs(5));

        let make_frame = |id: &str| InternalFrame {
            parent_frame: None,
            id: FrameId::new(id.to_string()),
            main_world: Default::default(),
            secondary_world: Default::default(),
            loader_id: None,
            url: None,
            http_request: None,
            child_frames: Default::default(),
            name: None,
            lifecycle_events: Default::default(),
        };
        fm.frames
            .insert(FrameId::new("fA".to_string()), make_frame("fA"));
        fm.frames
            .insert(FrameId::new("fB".to_string()), make_frame("fB"));
        let chain = fm.ensure_isolated_world(UTILITY_WORLD_NAME).expect("chain");

        // 1 addScript + 2 createIsolatedWorld.
        let ids = chain.method_identifiers();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], "Page.addScriptToEvaluateOnNewDocument");
        assert_eq!(ids[1], "Page.createIsolatedWorld");
        assert_eq!(ids[2], "Page.createIsolatedWorld");

        // Pending queue length equals number of frames; order matches
        // the two create-isolated-world commands.
        assert_eq!(fm.pending_isolated_world_frames.len(), 2);
    }

    #[test]
    fn stale_epoch_response_is_dropped_not_bound() {
        // Reviewer C1: a fast double-navigation had old-chain responses
        // contaminating a freshly-reset queue. The epoch tag makes the
        // response handler walk past stale entries until it finds one
        // from the current epoch (or the queue empties).
        use crate::render::chrome::handler::frame::Frame as InternalFrame;
        use crate::render::chrome_protocol::cdp::browser_protocol::page::FrameId;
        use crate::render::chrome_protocol::cdp::js_protocol::runtime::ExecutionContextId;
        let mut fm = super::FrameManager::new(Duration::from_secs(5));

        let fid = FrameId::new("fA".to_string());
        fm.frames.insert(
            fid.clone(),
            InternalFrame {
                parent_frame: None,
                id: fid.clone(),
                main_world: Default::default(),
                secondary_world: Default::default(),
                loader_id: None,
                url: None,
                http_request: None,
                child_frames: Default::default(),
                name: None,
                lifecycle_events: Default::default(),
            },
        );

        // Stale queue entry from an older epoch.
        let stale_epoch = fm.isolated_world_epoch;
        fm.pending_isolated_world_frames
            .push_back((stale_epoch, fid.clone()));

        // Simulate a reset (navigation happened) that bumps the epoch —
        // the entry above is now stale.
        fm.reset_isolated_world_state();
        assert_ne!(fm.isolated_world_epoch, stale_epoch);

        // Re-queue a fresh pending entry for the current epoch.
        fm.pending_isolated_world_frames
            .push_back((fm.isolated_world_epoch, fid.clone()));

        // A response arriving now: the first pop is the stale entry (but
        // wait — reset_isolated_world_state cleared the queue first).
        // Simulate the race instead by pre-populating with stale + fresh.
        fm.pending_isolated_world_frames.clear();
        fm.pending_isolated_world_frames
            .push_back((stale_epoch, fid.clone()));
        fm.pending_isolated_world_frames
            .push_back((fm.isolated_world_epoch, fid.clone()));

        // Feeding one response should skip the stale and bind on the fresh.
        let bound = fm.on_create_isolated_world_response(ExecutionContextId::new(7));
        assert!(bound, "fresh-epoch entry must bind after stale was dropped");
        assert_eq!(
            fm.frames
                .get(&fid)
                .and_then(|f| f.secondary_world.execution_context()),
            Some(ExecutionContextId::new(7))
        );
    }

    #[test]
    fn on_create_isolated_world_response_binds_context() {
        use crate::render::chrome::handler::frame::Frame as InternalFrame;
        use crate::render::chrome_protocol::cdp::browser_protocol::page::FrameId;
        use crate::render::chrome_protocol::cdp::js_protocol::runtime::ExecutionContextId;
        let mut fm = super::FrameManager::new(Duration::from_secs(5));

        let fid = FrameId::new("f1".to_string());
        let frame = InternalFrame {
            parent_frame: None,
            id: fid.clone(),
            main_world: Default::default(),
            secondary_world: Default::default(),
            loader_id: None,
            url: None,
            http_request: None,
            child_frames: Default::default(),
            name: None,
            lifecycle_events: Default::default(),
        };
        fm.frames.insert(fid.clone(), frame);
        fm.pending_isolated_world_frames
            .push_back((fm.isolated_world_epoch, fid.clone()));

        // Before: frame has no secondary context.
        assert!(fm
            .frames
            .get(&fid)
            .and_then(|f| f.secondary_world.execution_context())
            .is_none());

        // Feed a synthetic response.
        let bound = fm.on_create_isolated_world_response(ExecutionContextId::new(42));
        assert!(bound, "response should bind to the pending frame");

        // After: frame.secondary_world holds the context id.
        let ctx = fm
            .frames
            .get(&fid)
            .and_then(|f| f.secondary_world.execution_context())
            .expect("secondary context should be set");
        assert_eq!(ctx, ExecutionContextId::new(42));

        // Pending queue drained.
        assert!(fm.pending_isolated_world_frames.is_empty());

        // A second spurious response returns false (nothing pending).
        let again = fm.on_create_isolated_world_response(ExecutionContextId::new(99));
        assert!(!again, "response without pending entry should be a no-op");
    }
}

impl FrameManager {
    pub fn main_frame(&self) -> Option<&Frame> {
        self.main_frame.as_ref().and_then(|id| self.frames.get(id))
    }

    pub fn main_frame_mut(&mut self) -> Option<&mut Frame> {
        if let Some(id) = self.main_frame.as_ref() {
            self.frames.get_mut(id)
        } else {
            None
        }
    }

    pub fn frames(&self) -> impl Iterator<Item = &Frame> + '_ {
        self.frames.values()
    }

    pub fn frame(&self, id: &FrameId) -> Option<&Frame> {
        self.frames.get(id)
    }

    fn check_lifecycle(&self, watcher: &NavigationWatcher, frame: &Frame) -> bool {
        watcher.expected_lifecycle.iter().all(|ev| {
            frame.lifecycle_events.contains(ev)
                || (frame.url.is_none() && frame.lifecycle_events.contains("DOMContentLoaded"))
        }) && frame
            .child_frames
            .iter()
            .filter_map(|f| self.frames.get(f))
            .all(|f| self.check_lifecycle(watcher, f))
    }

    fn check_lifecycle_complete(
        &self,
        watcher: &NavigationWatcher,
        frame: &Frame,
    ) -> Option<NavigationOk> {
        if !self.check_lifecycle(watcher, frame) {
            return None;
        }
        if frame.loader_id == watcher.loader_id && !watcher.same_document_navigation {
            return None;
        }
        if watcher.same_document_navigation {
            return Some(NavigationOk::SameDocumentNavigation(watcher.id));
        }
        if frame.loader_id != watcher.loader_id {
            return Some(NavigationOk::NewDocumentNavigation(watcher.id));
        }
        None
    }

    /// Track the request in the frame
    pub fn on_http_request_finished(&mut self, request: HttpRequest) {
        if let Some(id) = request.frame.as_ref() {
            if let Some(frame) = self.frames.get_mut(id) {
                frame.set_request(request);
            }
        }
    }

    pub fn poll(&mut self, now: Instant) -> Option<FrameEvent> {
        // check if the navigation completed
        if let Some((watcher, deadline)) = self.navigation.take() {
            if now > deadline {
                // navigation request timed out
                return Some(FrameEvent::NavigationResult(Err(
                    NavigationError::Timeout {
                        err: DeadlineExceeded::new(now, deadline),
                        id: watcher.id,
                    },
                )));
            }
            if let Some(frame) = self.frames.get(&watcher.frame_id) {
                if let Some(nav) = self.check_lifecycle_complete(&watcher, frame) {
                    // request is complete if the frame's lifecycle is complete = frame received all
                    // required events
                    return Some(FrameEvent::NavigationResult(Ok(nav)));
                } else {
                    // not finished yet
                    self.navigation = Some((watcher, deadline));
                }
            } else {
                return Some(FrameEvent::NavigationResult(Err(
                    NavigationError::FrameNotFound {
                        frame: watcher.frame_id,
                        id: watcher.id,
                    },
                )));
            }
        } else if let Some((req, watcher)) = self.pending_navigations.pop_front() {
            // queue in the next navigation that is must be fulfilled until `deadline`
            let deadline = Instant::now() + req.timeout;
            self.navigation = Some((watcher, deadline));
            return Some(FrameEvent::NavigationRequest(req.id, req.req));
        }
        None
    }

    /// Entrypoint for page navigation
    pub fn goto(&mut self, req: FrameNavigationRequest) {
        if let Some(frame_id) = self.main_frame.clone() {
            self.navigate_frame(frame_id, req);
        }
    }

    /// Navigate a specific frame
    pub fn navigate_frame(&mut self, frame_id: FrameId, mut req: FrameNavigationRequest) {
        let loader_id = self.frames.get(&frame_id).and_then(|f| f.loader_id.clone());
        let watcher = NavigationWatcher::until_page_load(req.id, frame_id.clone(), loader_id);
        // insert the frame_id in the request if not present
        req.set_frame_id(frame_id);
        self.pending_navigations.push_back((req, watcher))
    }

    /// Fired when a frame moved to another session
    pub fn on_attached_to_target(&mut self, _event: &EventAttachedToTarget) {
        // _onFrameMoved
    }

    pub fn on_frame_tree(&mut self, frame_tree: FrameTree) {
        self.on_frame_attached(
            frame_tree.frame.id.clone(),
            frame_tree.frame.parent_id.clone(),
        );
        self.on_frame_navigated(&frame_tree.frame);
        if let Some(children) = frame_tree.child_frames {
            for child_tree in children {
                self.on_frame_tree(child_tree);
            }
        }
    }

    pub fn on_frame_attached(&mut self, frame_id: FrameId, parent_frame_id: Option<FrameId>) {
        if self.frames.contains_key(&frame_id) {
            return;
        }
        if let Some(parent_frame_id) = parent_frame_id {
            if let Some(parent_frame) = self.frames.get_mut(&parent_frame_id) {
                let frame = Frame::with_parent(frame_id.clone(), parent_frame);
                self.frames.insert(frame_id, frame);
            }
        }
    }

    pub fn on_frame_detached(&mut self, event: &EventFrameDetached) {
        self.remove_frames_recursively(&event.frame_id);
    }

    pub fn on_frame_navigated(&mut self, frame: &CdpFrame) {
        if frame.parent_id.is_some() {
            if let Some((id, mut f)) = self.frames.remove_entry(&frame.id) {
                for child in &f.child_frames {
                    self.remove_frames_recursively(child);
                }
                // this is necessary since we can't borrow mut and then remove recursively
                f.child_frames.clear();
                f.navigated(frame);
                self.frames.insert(id, f);
            }
        } else {
            let mut f = if let Some(main) = self.main_frame.take() {
                // update main frame
                let mut main_frame = self.frames.remove(&main).expect("Main frame is tracked.");
                for child in &main_frame.child_frames {
                    self.remove_frames_recursively(child);
                }
                // this is necessary since we can't borrow mut and then remove recursively
                main_frame.child_frames.clear();
                main_frame.id = frame.id.clone();
                main_frame
            } else {
                // initial main frame navigation
                Frame::new(frame.id.clone())
            };
            f.navigated(frame);
            self.main_frame = Some(f.id.clone());
            self.frames.insert(f.id.clone(), f);
        }
    }

    pub fn on_frame_navigated_within_document(&mut self, event: &EventNavigatedWithinDocument) {
        if let Some(frame) = self.frames.get_mut(&event.frame_id) {
            frame.navigated_within_url(event.url.clone());
        }
        if let Some((watcher, _)) = self.navigation.as_mut() {
            watcher.on_frame_navigated_within_document(event);
        }
    }

    pub fn on_frame_stopped_loading(&mut self, event: &EventFrameStoppedLoading) {
        if let Some(frame) = self.frames.get_mut(&event.frame_id) {
            frame.on_loading_stopped();
        }
    }

    /// Fired when frame has started loading.
    pub fn on_frame_started_loading(&mut self, event: &EventFrameStartedLoading) {
        if let Some(frame) = self.frames.get_mut(&event.frame_id) {
            frame.on_loading_started();
        }
    }

    /// Notification is issued every time when binding is called
    pub fn on_runtime_binding_called(&mut self, _ev: &EventBindingCalled) {}

    /// Issued when new execution context is created
    pub fn on_frame_execution_context_created(&mut self, event: &EventExecutionContextCreated) {
        if let Some(frame_id) = event
            .context
            .aux_data
            .as_ref()
            .and_then(|v| v["frameId"].as_str())
        {
            if let Some(frame) = self.frames.get_mut(frame_id) {
                if event
                    .context
                    .aux_data
                    .as_ref()
                    .and_then(|v| v["isDefault"].as_bool())
                    .unwrap_or_default()
                {
                    frame
                        .main_world
                        .set_context(event.context.id, event.context.unique_id.clone());
                } else if event.context.name == UTILITY_WORLD_NAME
                    && frame.secondary_world.execution_context().is_none()
                {
                    frame
                        .secondary_world
                        .set_context(event.context.id, event.context.unique_id.clone());
                }
                self.context_ids
                    .insert(event.context.unique_id.clone(), frame.id.clone());
            }
        }
        if event
            .context
            .aux_data
            .as_ref()
            .filter(|v| v["type"].as_str() == Some("isolated"))
            .is_some()
        {
            self.isolated_worlds.insert(event.context.name.clone());
        }
    }

    /// Issued when execution context is destroyed
    pub fn on_frame_execution_context_destroyed(&mut self, event: &EventExecutionContextDestroyed) {
        if let Some(id) = self.context_ids.remove(&event.execution_context_unique_id) {
            if let Some(frame) = self.frames.get_mut(&id) {
                frame.destroy_context(&event.execution_context_unique_id);
            }
        }
    }

    /// Issued when all executionContexts were cleared
    pub fn on_execution_contexts_cleared(&mut self) {
        for id in self.context_ids.values() {
            if let Some(frame) = self.frames.get_mut(id) {
                frame.clear_contexts();
            }
        }
        self.context_ids.clear()
    }

    /// Main-frame `Page.loadEventFired` / `Page.domContentEventFired`
    /// fallback: Chrome 149 no longer re-emits `Page.lifecycleEvent` after
    /// a navigation, so we fold these top-level events into the main
    /// frame's lifecycle bag so the `NavigationWatcher` ("load") can
    /// complete.
    pub fn on_page_load_event_fired(&mut self) {
        if let Some(main) = self.main_frame.clone() {
            if let Some(frame) = self.frames.get_mut(&main) {
                frame.lifecycle_events.insert("load".into());
            }
        }
    }

    pub fn on_page_dom_content_event_fired(&mut self) {
        if let Some(main) = self.main_frame.clone() {
            if let Some(frame) = self.frames.get_mut(&main) {
                frame.lifecycle_events.insert("DOMContentLoaded".into());
            }
        }
    }

    /// Fired for top level page lifecycle events (nav, load, paint, etc.)
    pub fn on_page_lifecycle_event(&mut self, event: &EventLifecycleEvent) {
        if let Some(frame) = self.frames.get_mut(&event.frame_id) {
            // NOTE(crawlex vendor patch): Chrome 149+ emits `commit` instead of
            // `init` as the first lifecycle event after a navigation. Accept
            // both so navigations complete on modern browsers.
            if event.name == "init" || event.name == "commit" {
                frame.loader_id = Some(event.loader_id.clone());
                frame.lifecycle_events.clear();
            }
            frame.lifecycle_events.insert(event.name.clone().into());
        }
    }

    /// Detach all child frames
    fn remove_frames_recursively(&mut self, id: &FrameId) -> Option<Frame> {
        if let Some(mut frame) = self.frames.remove(id) {
            for child in &frame.child_frames {
                self.remove_frames_recursively(child);
            }
            if let Some(parent_id) = frame.parent_frame.take() {
                if let Some(parent) = self.frames.get_mut(&parent_id) {
                    parent.child_frames.remove(&frame.id);
                }
            }
            Some(frame)
        } else {
            None
        }
    }

    pub fn ensure_isolated_world(&mut self, world_name: &str) -> Option<CommandChain> {
        if self.isolated_worlds.contains(world_name) {
            return None;
        }
        self.isolated_worlds.insert(world_name.to_string());
        let cmd = AddScriptToEvaluateOnNewDocumentParams::builder()
            .source(format!("//# sourceURL={EVALUATION_SCRIPT_URL}"))
            .world_name(world_name)
            .build()
            .unwrap();

        let mut cmds = Vec::with_capacity(self.frames.len() + 1);

        cmds.push((cmd.identifier(), serde_json::to_value(cmd).unwrap()));

        // Snapshot frame_ids in the same order we emit their
        // `Page.createIsolatedWorld` commands. CommandChain serializes
        // requests (only one `waiting` slot), so responses arrive in the
        // same order, and `on_create_isolated_world_response` can pop
        // front-first to match them up.
        let epoch = self.isolated_world_epoch;
        // Snapshot ids to avoid a borrow conflict with the loop below.
        let frame_ids: Vec<FrameId> = self.frames.keys().cloned().collect();
        for id in frame_ids {
            self.pending_isolated_world_frames
                .push_back((epoch, id.clone()));
            let cmd = CreateIsolatedWorldParams::builder()
                .frame_id(id.clone())
                .grant_univeral_access(true)
                .world_name(world_name)
                .build()
                .unwrap();
            cmds.push((cmd.identifier(), serde_json::to_value(cmd).unwrap()));
        }
        Some(CommandChain::new(cmds, self.request_timeout))
    }

    /// Bind a `Page.createIsolatedWorld` response to the oldest pending
    /// frame. The secondary (isolated) world of that frame is marked with
    /// the returned `executionContextId` so `page.evaluate()` can target
    /// it even when `Runtime.enable` was suppressed (stealth mode).
    ///
    /// Returns `true` if we had a pending frame to bind, `false` if the
    /// response arrived without a matching request (shouldn't happen in
    /// practice — log-only signal).
    /// Drop all execution-context bookkeeping and isolated-world
    /// tracking so a fresh `ensure_isolated_world` call re-emits the
    /// Page.createIsolatedWorld commands.
    ///
    /// Needed under stealth mode: navigating / reloading destroys the
    /// previous isolated world context, but `Runtime.executionContextsCleared`
    /// never fires (Runtime.enable is off), so the stale context id
    /// lingers until we clear it here. Default mode gets the same
    /// cleanup idempotently — event-driven paths already ran.
    pub fn reset_isolated_world_state(&mut self) {
        self.isolated_worlds.clear();
        self.pending_isolated_world_frames.clear();
        self.context_ids.clear();
        for frame in self.frames.values_mut() {
            frame.clear_contexts();
        }
        // Bump the epoch so any in-flight response from the superseded
        // chain is rejected by `on_create_isolated_world_response`.
        self.isolated_world_epoch = self.isolated_world_epoch.wrapping_add(1);
    }

    pub fn on_create_isolated_world_response(
        &mut self,
        execution_context_id: ExecutionContextId,
    ) -> bool {
        // Invariant: we push to `pending_isolated_world_frames` every
        // time `ensure_isolated_world` emits a `Page.createIsolatedWorld`
        // request, and `CommandChain` serialises requests one-in-flight.
        // Therefore each response corresponds to the oldest pending
        // frame entry — as long as the queue was not superseded by a
        // reset. Stale (epoch != current) entries are popped and
        // discarded so a fast-double-navigation cannot bind a dead
        // execution_context_id to a freshly-navigated frame.
        loop {
            let Some((epoch, frame_id)) = self.pending_isolated_world_frames.pop_front() else {
                tracing::warn!(
                    target: "crawlex::stealth",
                    ?execution_context_id,
                    "Page.createIsolatedWorld response with no matching pending \
                     frame — invariant broken, check CommandChain pipelining"
                );
                return false;
            };
            if epoch != self.isolated_world_epoch {
                // Response from a superseded chain — drop and keep
                // draining until we find an entry from the current
                // epoch (or the queue empties).
                tracing::debug!(
                    target: "crawlex::stealth",
                    stale_epoch = epoch,
                    current_epoch = self.isolated_world_epoch,
                    ?frame_id,
                    "dropping stale CreateIsolatedWorld response"
                );
                continue;
            }
            let unique_id = format!("stealth-isolated-{:?}-{:?}", frame_id, execution_context_id);
            if let Some(frame) = self.frames.get_mut(&frame_id) {
                if frame.secondary_world.execution_context().is_none() {
                    frame
                        .secondary_world
                        .set_context(execution_context_id, unique_id.clone());
                    self.context_ids.insert(unique_id, frame_id);
                    return true;
                }
            }
            return false;
        }
    }
}

#[derive(Debug)]
pub enum FrameEvent {
    /// A previously submitted navigation has finished
    NavigationResult(Result<NavigationOk, NavigationError>),
    /// A new navigation request needs to be submitted
    NavigationRequest(NavigationId, Request),
    /* /// The initial page of the target has been loaded
     * InitialPageLoadFinished */
}

#[derive(Debug)]
pub enum NavigationError {
    Timeout {
        id: NavigationId,
        err: DeadlineExceeded,
    },
    FrameNotFound {
        id: NavigationId,
        frame: FrameId,
    },
}

impl NavigationError {
    pub fn navigation_id(&self) -> &NavigationId {
        match self {
            NavigationError::Timeout { id, .. } => id,
            NavigationError::FrameNotFound { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NavigationOk {
    SameDocumentNavigation(NavigationId),
    NewDocumentNavigation(NavigationId),
}

impl NavigationOk {
    pub fn navigation_id(&self) -> &NavigationId {
        match self {
            NavigationOk::SameDocumentNavigation(id) => id,
            NavigationOk::NewDocumentNavigation(id) => id,
        }
    }
}

/// Tracks the progress of an issued `Page.navigate` request until completion.
#[derive(Debug)]
pub struct NavigationWatcher {
    id: NavigationId,
    expected_lifecycle: HashSet<MethodId>,
    frame_id: FrameId,
    loader_id: Option<LoaderId>,
    /// Once we receive the response to the issued `Page.navigate` request we
    /// can detect whether we were navigating withing the same document or were
    /// navigating to a new document by checking if a loader was included in the
    /// response.
    same_document_navigation: bool,
}

impl NavigationWatcher {
    pub fn until_page_load(id: NavigationId, frame: FrameId, loader_id: Option<LoaderId>) -> Self {
        // Default lifecycle target is `load` (CDP). Heavy real-world targets
        // (Cloudflare-fronted SPAs with WordPress + ad scripts) routinely
        // exceed the 30s default before `load` fires. Operators override
        // via `CRAWLEX_NAVIGATION_LIFECYCLE`:
        //   - `load` (default) — wait for full window onload
        //   - `domcontentloaded` — return as soon as parser is done
        let lifecycle: MethodId = match std::env::var("CRAWLEX_NAVIGATION_LIFECYCLE")
            .ok()
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("domcontentloaded") | Some("dom_content_loaded") | Some("dcl") => {
                "DOMContentLoaded"
            }
            _ => "load",
        }
        .into();
        Self {
            id,
            expected_lifecycle: std::iter::once(lifecycle).collect(),
            loader_id,
            frame_id: frame,
            same_document_navigation: false,
        }
    }

    /// Checks whether the navigation was completed
    pub fn is_lifecycle_complete(&self) -> bool {
        self.expected_lifecycle.is_empty()
    }

    fn on_frame_navigated_within_document(&mut self, ev: &EventNavigatedWithinDocument) {
        if self.frame_id == ev.frame_id {
            self.same_document_navigation = true;
        }
    }
}

/// An identifier for an ongoing navigation
#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct NavigationId(pub usize);

/// Represents a the request for a navigation
#[derive(Debug)]
pub struct FrameNavigationRequest {
    /// The internal identifier
    pub id: NavigationId,
    /// the cdp request that will trigger the navigation
    pub req: Request,
    /// The timeout after which the request will be considered timed out
    pub timeout: Duration,
}

impl FrameNavigationRequest {
    pub fn new(id: NavigationId, req: Request) -> Self {
        Self {
            id,
            req,
            timeout: Duration::from_millis(request_timeout_ms()),
        }
    }

    /// This will set the id of the frame into the `params` `frameId` field.
    pub fn set_frame_id(&mut self, frame_id: FrameId) {
        if let Some(params) = self.req.params.as_object_mut() {
            if let Entry::Vacant(entry) = params.entry("frameId") {
                entry.insert(serde_json::Value::String(frame_id.into()));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LifecycleEvent {
    #[default]
    Load,
    DomcontentLoaded,
    NetworkIdle,
    NetworkAlmostIdle,
}

impl AsRef<str> for LifecycleEvent {
    fn as_ref(&self) -> &str {
        match self {
            LifecycleEvent::Load => "load",
            LifecycleEvent::DomcontentLoaded => "DOMContentLoaded",
            LifecycleEvent::NetworkIdle => "networkIdle",
            LifecycleEvent::NetworkAlmostIdle => "networkAlmostIdle",
        }
    }
}
