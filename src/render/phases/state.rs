//! Shared mutable scratch space that flows between `RenderPhase`s.
//!
//! Replaces the implicit "every method on `RenderPool` writes back to a
//! local var" tangle in the old `render_with_script`. Phases populate
//! whatever they observe; the final `RenderPool::render` shape projects
//! `RenderState` into the user-facing `RenderedPage`.
//!
//! Fields are `pub` because phases need to read each other's output
//! (e.g. `CollectPhase` wants the URL `PostNavigatePhase` resolved to;
//! `CapturePhase` wants the element selector the user supplied).
//! Encapsulation here would be ceremony — the type is internal.

#![cfg(feature = "cdp-backend")]

use bytes::Bytes;
use url::Url;

/// Mutable scratch flowing through the phase pipeline.
#[derive(Default)]
pub struct RenderState {
    /// Final URL after the navigation committed. `None` until
    /// `PostNavigatePhase` resolves the wait strategy.
    pub final_url: Option<Url>,

    /// HTTP status code from the navigation response. `None` if the
    /// page never returned a response (timeout, network error caught
    /// further downstream).
    pub status: Option<u16>,

    /// Serialised post-JS HTML of the main frame's document. Populated
    /// by `CollectPhase`.
    pub html_post_js: Option<String>,

    /// Screenshot bytes (PNG). Populated by `CapturePhase` when the
    /// caller asked for one.
    pub screenshot_png: Option<Bytes>,

    /// URLs observed via `history.pushState` / `replaceState` / `popstate`
    /// / `hashchange`. Populated by the SPA observer if active.
    pub runtime_routes: Vec<Url>,

    /// URLs observed via runtime fetch + XHR wrappers.
    pub network_endpoints: Vec<Url>,

    /// Web Vitals + per-resource timings. Populated by
    /// `CollectPhase` only when `Config::collect_web_vitals` is on.
    pub vitals: crate::metrics::WebVitals,
    pub resources: Vec<crate::metrics::ResourceSample>,

    /// Detected antibot challenge, if any. Populated by `SettlePhase`
    /// running `antibot::detect_from_html` against the post-JS HTML.
    pub challenge: Option<crate::antibot::ChallengeSignal>,

    /// PWA / SPA snapshots — manifest, service workers, IndexedDB,
    /// Cache Storage, asset refs. Populated by `CollectPhase`.
    pub manifest_url: Option<Url>,
    pub service_worker_urls: Vec<Url>,
    pub indexeddb_inventory: Option<serde_json::Value>,
    pub cache_storage_inventory: Option<serde_json::Value>,

    /// Arbitrary key/value scratch for phase-specific state that doesn't
    /// merit a typed field yet. Kept loose because per-phase scratch is
    /// hard to predict ahead of time; promote to a typed field as soon
    /// as a second phase needs it.
    pub scratch: serde_json::Map<String, serde_json::Value>,
}

impl RenderState {
    /// Convenience: was a navigation actually committed?
    pub fn navigated(&self) -> bool {
        self.final_url.is_some()
    }
}
