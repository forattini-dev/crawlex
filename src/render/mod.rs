pub mod actions;
pub mod android_profile;
pub mod ax_snapshot;
#[cfg(feature = "cdp-backend")]
pub mod chrome;
#[cfg(feature = "cdp-backend")]
pub mod chrome_fetcher;
#[cfg(feature = "cdp-backend")]
pub mod chrome_protocol;
#[cfg(feature = "cdp-backend")]
pub mod chrome_wire;
pub mod handoff;
pub mod interact;
pub mod keyboard;
pub mod motion;
#[cfg(feature = "cdp-backend")]
pub mod page_pool;
#[cfg(feature = "cdp-backend")]
pub mod phases;
pub mod pool;
#[cfg(feature = "cdp-backend")]
pub mod ref_resolver;
pub mod selector;
#[cfg(feature = "cdp-backend")]
pub mod session;
pub mod spa_observer;
pub mod stealth;
pub mod wait;

pub use pool::RenderPool;
pub use wait::WaitStrategy;

use url::Url;

use crate::Result;

pub struct RenderedPage {
    pub session_id: String,
    pub final_url: Url,
    pub html_post_js: String,
    pub captured_urls: Vec<Url>,
    pub manifest_url: Option<Url>,
    pub service_worker_urls: Vec<Url>,
    pub status: u16,
    pub vitals: crate::metrics::WebVitals,
    pub resources: Vec<crate::metrics::ResourceSample>,
    pub screenshot_png: Option<Vec<u8>>,
    /// Antibot challenge detected post-render. `None` when the page is
    /// clean. Populated by running `antibot::detect_from_html` against the
    /// post-JS HTML; CDP cookie-based detection is best-effort.
    pub challenge: Option<crate::antibot::ChallengeSignal>,
    /// Absolute URLs observed via `history.pushState` / `replaceState`
    /// / `popstate` / `hashchange` during the render. Deduplicated.
    /// Empty unless `Config::collect_runtime_routes` is true AND a
    /// CDP backend is in use.
    #[cfg(feature = "cdp-backend")]
    pub runtime_routes: Vec<Url>,
    /// Absolute URLs observed via the `fetch` and `XMLHttpRequest`
    /// runtime wrappers. Deduplicated and filtered to http(s) only.
    #[cfg(feature = "cdp-backend")]
    pub network_endpoints: Vec<Url>,
    /// Heuristic SPA flag: true when the observer saw any runtime
    /// route or the final URL fragment differs from the seed URL.
    #[cfg(feature = "cdp-backend")]
    pub is_spa: bool,
}

#[async_trait::async_trait]
pub trait Renderer: Send + Sync {
    async fn render(
        &self,
        url: &Url,
        wait: &WaitStrategy,
        collect_vitals: bool,
        screenshot: bool,
        actions: Option<&[crate::render::actions::Action]>,
        proxy: Option<&Url>,
    ) -> Result<RenderedPage>;
}
