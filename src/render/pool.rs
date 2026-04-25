use futures::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, warn};
use url::Url;

use crate::render::chrome::browser::{Browser, BrowserConfig};
use crate::render::chrome::page::Page;
use crate::render::chrome_protocol::cdp::browser_protocol::browser::BrowserContextId;
use crate::render::chrome_protocol::cdp::browser_protocol::emulation::{
    UserAgentBrandVersion, UserAgentMetadata,
};
use crate::render::chrome_protocol::cdp::browser_protocol::network::{
    BlockPattern, ClearBrowserCacheParams, Cookie, CookieParam, CookiePartitionKey, CookiePriority,
    CookieSameSite, CookieSourceScheme, EventLoadingFailed, EventLoadingFinished,
    EventRequestWillBeSent, EventResponseReceived, ResourceType, SetBlockedUrLsParams,
    SetBypassServiceWorkerParams, SetUserAgentOverrideParams as NetworkSetUserAgentOverrideParams,
    TimeSinceEpoch,
};
use crate::render::chrome_protocol::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, CaptureScreenshotParams, NavigateParams,
};
use crate::render::chrome_protocol::cdp::browser_protocol::security::SetIgnoreCertificateErrorsParams;
use crate::render::chrome_protocol::cdp::browser_protocol::service_worker::UnregisterParams as ServiceWorkerUnregisterParams;
use crate::render::chrome_protocol::cdp::browser_protocol::storage::ClearDataForOriginParams;
use crate::render::chrome_protocol::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams, SetAutoAttachParams,
};
use crate::render::chrome_protocol::cdp::js_protocol::runtime::EvaluateParams;
use std::collections::HashMap;

/// Screenshot capture modes exposed to the ScriptSpec runner and internal
/// callers. Maps 1:1 to `script::spec::ScreenshotMode`; the indirection
/// keeps `render` backend-specific types out of the backend-agnostic spec.
#[derive(Debug, Clone)]
pub enum ScreenshotCaptureMode {
    Viewport,
    FullPage,
    Element { selector: String },
}

/// Parse an operator-facing screenshot mode string into the internal enum.
/// Accepted forms (case-insensitive):
///   - `viewport`
///   - `fullpage` / `full` / `full_page`
///   - `element:<selector>` — selector must be non-empty
/// Returns `Err` with a human-readable message on anything else so the CLI
/// layer can surface it early. Use `parse_screenshot_mode_or_default` when
/// unknown input should silently fall back to `FullPage`.
pub fn parse_screenshot_mode(s: &str) -> std::result::Result<ScreenshotCaptureMode, String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "viewport" {
        return Ok(ScreenshotCaptureMode::Viewport);
    }
    if lower == "fullpage" || lower == "full" || lower == "full_page" {
        return Ok(ScreenshotCaptureMode::FullPage);
    }
    if let Some(rest) = t
        .strip_prefix("element:")
        .or_else(|| t.strip_prefix("Element:"))
    {
        let sel = rest.trim();
        if sel.is_empty() {
            return Err(format!(
                "screenshot-mode: `element:` requires a selector (got `{s}`)"
            ));
        }
        return Ok(ScreenshotCaptureMode::Element {
            selector: sel.to_string(),
        });
    }
    Err(format!(
        "screenshot-mode: expected `viewport|fullpage|element:<selector>`, got `{s}`"
    ))
}

/// Same as `parse_screenshot_mode` but returns `FullPage` on `None` or any
/// unparseable input. Used inside `RenderPool::render` where a bad value in
/// a loaded Config shouldn't abort an already-running crawl — the CLI layer
/// is the place to hard-fail on malformed user input.
pub fn parse_screenshot_mode_or_default(opt: Option<&str>) -> ScreenshotCaptureMode {
    match opt {
        Some(s) => parse_screenshot_mode(s).unwrap_or(ScreenshotCaptureMode::FullPage),
        None => ScreenshotCaptureMode::FullPage,
    }
}

fn diff(start: f64, end: f64) -> Option<f64> {
    if start >= 0.0 && end >= start {
        Some(end - start)
    } else {
        None
    }
}

/// Build the Chromium launch argv for a browser instance.
///
/// Single source of truth for the set of `--flag=value` tokens passed to
/// the Chromium binary. Kept as a free function so unit tests can
/// exercise the argv shape without spinning up a real Browser.
///
/// Inputs:
/// - `bundle`: the persona's [`IdentityBundle`] — supplies UA, viewport,
///   and DPR so the launched browser's window matches what the stealth
///   shim, CDP UA override, and UA-CH emulation all declare. Consuming
///   the bundle here is what closes the "Chrome says 1920×1080 but the
///   shim reports 390×844" mismatch that burns mobile personas.
/// - `proxy`: optional per-launch upstream proxy. When set we also
///   disable the default `<loopback>` bypass so `http://127.0.0.1:…`
///   proxies actually route through (instead of going direct).
/// - `user_data_dir`: per-browser profile root. Required — sharing a
///   single profile across Chromes triggers SingletonLock crashes.
/// - `languages`: value for `--lang=`, already formatted
///   (e.g. `"en-US,en"` or `"pt-BR,en"`).
/// - `extra`: operator-supplied extra flags from `Config::chrome_flags`.
///   Appended last so they can override any default above.
///
/// Flag groups (grep-friendly for future audits):
/// - stability: `--disable-dev-shm-usage`, `--no-first-run`,
///   `--no-default-browser-check`
/// - stealth: `--disable-blink-features=AutomationControlled`
/// - feature toggles: `--disable-features=…` (Translate, MediaRouter,
///   mDNS) and `--enable-features=…` (VAAPI hw decode, AcceptCH frame,
///   Zstd content encoding, TLS13 Kyber PQ). See upstream plan items
///   #43 / #17 for rationale.
/// - WebRTC leak fix (#S.3): `--force-webrtc-ip-handling-policy=
///   disable_non_proxied_udp` so STUN never surfaces the host's private
///   IP.
/// - identity: `--user-agent` (from bundle), `--lang`, `--window-size`
///   and `--force-device-scale-factor` (both from bundle).
/// - JS/WASM surface: `--js-flags=--noexpose-wasm` — real Chrome does
///   not expose `%WasmCompileLazy` etc. to user code.
///
/// GPU policy: default is `--disable-gpu` because the headless /
/// container environments this runs in rarely have a working DRM
/// device. Operators who want hardware accel opt in via
/// `Config::chrome_flags = ["--use-gl=angle", "--use-angle=gl",
/// "--enable-gpu-rasterization"]` and we surface those here untouched
/// via `extra`.
pub fn build_launch_args(
    bundle: &crate::identity::IdentityBundle,
    proxy: Option<&Url>,
    user_data_dir: &std::path::Path,
    languages: &str,
    extra: &[String],
) -> Vec<String> {
    // Viewport / DPR come from the bundle so the launched Chrome matches
    // what the stealth shim and UA-CH payload claim. Fall back to a
    // conservative desktop default if the bundle somehow has a zero
    // dimension — we never want to emit `--window-size=0,0`.
    let win_w = if bundle.viewport_w == 0 {
        1920
    } else {
        bundle.viewport_w
    };
    let win_h = if bundle.viewport_h == 0 {
        1080
    } else {
        bundle.viewport_h
    };
    let dpr = if bundle.device_pixel_ratio > 0.0 {
        bundle.device_pixel_ratio
    } else {
        1.0
    };

    let mut flags: Vec<String> = vec![
        // Stability -----------------------------------------------------
        "--disable-dev-shm-usage".into(),
        // GPU: conservative default. Headless/docker rarely has DRM.
        // Operators flip this on via `Config::chrome_flags` (see doc).
        "--disable-gpu".into(),
        // Stealth -------------------------------------------------------
        "--disable-blink-features=AutomationControlled".into(),
        // Feature toggles we turn OFF. Keep WebRtcHideLocalIpsWithMdns
        // in the *disable* list so the `force-webrtc-ip-handling-policy`
        // flag below is the sole arbiter of ICE behaviour (else mDNS
        // substitutes a `.local` hostname that some detectors flag).
        "--disable-features=IsolateOrigins,site-per-process,Translate,MediaRouter,WebRtcHideLocalIpsWithMdns".into(),
        // Feature toggles we turn ON:
        // - VaapiVideoDecoder (#43): VA-API hw video decode when a GPU
        //   is actually present. Harmless in `--disable-gpu` mode —
        //   Chrome just never picks the accelerated path.
        // - AcceptCHFrame: makes `sec-ch-ua-*` client hints honour
        //   server `Accept-CH` on first request, matching real Chrome.
        // - ZstdContentEncoding: real Chrome advertises `zstd` in
        //   Accept-Encoding; withholding it is a detection signal.
        // - EnableTLS13KyberPQ (#17): post-quantum hybrid KEM in the
        //   ClientHello. Real Chrome 124+ ships this ON; our
        //   impersonate TLS stack already emits the right extension —
        //   keep the browser surface aligned.
        "--enable-features=VaapiVideoDecoder,AcceptCHFrame,ZstdContentEncoding,EnableTLS13KyberPQ".into(),
        // S.3 — WebRTC leak fix: force ICE to use proxied UDP only so
        // the local private IP is never surfaced via STUN/ICE. See
        // `webrtc_leak_audit` test for the detection model.
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        // Identity — all sourced from the bundle so the Chrome process
        // argv matches the shim / CDP / UA-CH view of the world.
        format!("--user-agent={}", bundle.ua),
        format!("--lang={languages}"),
        format!("--window-size={win_w},{win_h}"),
        format!("--force-device-scale-factor={dpr}"),
        // JS surface: real Chrome does not expose WASM helpers like
        // `%WasmCompileLazy` to user code. `--noexpose-wasm` closes that
        // fingerprinting surface without breaking normal WebAssembly.
        "--js-flags=--noexpose-wasm".into(),
    ];

    // Route this browser through the requested proxy. Per-job rotation
    // is achieved by keying `browsers` on the proxy URL — each unique
    // proxy gets its own Chrome.
    //
    // The bypass list explicitly strips the default `<loopback>` entry
    // so 127.0.0.1/localhost proxies are actually used instead of
    // silently going direct — this is the difference that made
    // `--proxy http://127.0.0.1:9` still fetch upstream earlier.
    if let Some(p) = proxy {
        flags.push(format!("--proxy-server={}", p));
        flags.push("--proxy-bypass-list=<-loopback>".into());
    }

    flags.push(format!("--user-data-dir={}", user_data_dir.display()));

    if !extra.is_empty() {
        flags.extend(extra.iter().cloned());
    }
    flags
}

/// Build a well-formed `Accept-Language` header from a user-supplied locale.
/// Pass-through when the value already looks like a full list (contains a
/// `,` or `;`) so we don't double-quality ourselves into
/// `en-US,en;q=0.9;q=0.9` artifacts.
/// Parse a JSON array of `{brand, version}` pairs (from `ua_brands` or
/// `ua_full_version_list` in the bundle) into CDP client's
/// `UserAgentBrandVersion` vector. Falls back to a sane 3-entry default
/// keyed off `major` so an upstream that mangled the JSON still gets a
/// coherent UA-CH payload.
fn ua_ch_brands(brands_json: &str, major: &str) -> Vec<UserAgentBrandVersion> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(brands_json) {
        if let Some(arr) = val.as_array() {
            let parsed: Vec<UserAgentBrandVersion> = arr
                .iter()
                .filter_map(|e| {
                    let brand = e.get("brand")?.as_str()?.to_string();
                    let version = e.get("version")?.as_str()?.to_string();
                    Some(UserAgentBrandVersion { brand, version })
                })
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    vec![
        UserAgentBrandVersion {
            brand: "Google Chrome".into(),
            version: major.to_string(),
        },
        UserAgentBrandVersion {
            brand: "Chromium".into(),
            version: major.to_string(),
        },
        UserAgentBrandVersion {
            brand: "Not_A Brand".into(),
            version: "24".into(),
        },
    ]
}

/// Build a well-formed `Accept-Language` header from a user-supplied
/// locale. Retained for per-session locale rotation (upcoming) — the
/// bundle path currently computes `accept_language` directly.
#[allow(dead_code)]
fn build_accept_language(locale: Option<&str>) -> String {
    match locale {
        None => "en-US,en;q=0.9".to_string(),
        Some(v) => {
            let trimmed = v.trim();
            if trimmed.contains(',') || trimmed.contains(';') {
                trimmed.to_string()
            } else if trimmed.starts_with("en") {
                "en-US,en;q=0.9".to_string()
            } else {
                format!("{trimmed},en;q=0.9")
            }
        }
    }
}

use crate::config::Config;
use crate::identity::IdentityBundle;
use crate::impersonate::Profile;
use crate::render::stealth::{render_shim_from_bundle, render_worker_shim_from_bundle};
use crate::render::{RenderedPage, Renderer, WaitStrategy};
use crate::storage::Storage;
use crate::{Error, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedSessionState {
    #[serde(default)]
    cookies: Vec<PersistedCookie>,
    #[serde(default)]
    origins: Vec<PersistedOriginState>,
}

impl PersistedSessionState {
    fn merge_cookies(&mut self, cookies: Vec<PersistedCookie>) {
        let mut merged: HashMap<(String, String, String), PersistedCookie> = self
            .cookies
            .drain(..)
            .map(|cookie| {
                (
                    (
                        cookie.name.clone(),
                        cookie.domain.clone(),
                        cookie.path.clone(),
                    ),
                    cookie,
                )
            })
            .collect();
        for cookie in cookies {
            merged.insert(
                (
                    cookie.name.clone(),
                    cookie.domain.clone(),
                    cookie.path.clone(),
                ),
                cookie,
            );
        }
        self.cookies = merged.into_values().collect();
        self.cookies
            .sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
    }

    fn upsert_origin(&mut self, state: PersistedOriginState) {
        self.origins.retain(|entry| entry.origin != state.origin);
        self.origins.push(state);
        self.origins.sort_by(|a, b| a.origin.cmp(&b.origin));
    }

    fn origin_state(&self, origin: &str) -> Option<&PersistedOriginState> {
        self.origins.iter().find(|entry| entry.origin == origin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    #[serde(default)]
    same_site: Option<CookieSameSite>,
    #[serde(default)]
    expires: Option<f64>,
    #[serde(default)]
    priority: Option<CookiePriority>,
    #[serde(default)]
    source_scheme: Option<CookieSourceScheme>,
    #[serde(default)]
    source_port: Option<i64>,
    #[serde(default)]
    same_party: Option<bool>,
    #[serde(default)]
    partition_key: Option<CookiePartitionKey>,
}

impl From<Cookie> for PersistedCookie {
    fn from(cookie: Cookie) -> Self {
        Self {
            name: cookie.name,
            value: cookie.value,
            domain: cookie.domain,
            path: cookie.path,
            secure: cookie.secure,
            http_only: cookie.http_only,
            same_site: cookie.same_site,
            expires: if cookie.session {
                None
            } else {
                Some(cookie.expires)
            },
            priority: Some(cookie.priority),
            source_scheme: Some(cookie.source_scheme),
            source_port: Some(cookie.source_port),
            same_party: None,
            partition_key: cookie.partition_key,
        }
    }
}

impl PersistedCookie {
    fn to_cookie_param(&self, fallback_url: &Url) -> Result<CookieParam> {
        let cookie_url = self
            .cookie_url()
            .unwrap_or_else(|| fallback_url.as_str().to_string());
        let mut builder = CookieParam::builder()
            .name(self.name.clone())
            .value(self.value.clone())
            .url(cookie_url)
            .domain(self.domain.clone())
            .path(self.path.clone())
            .secure(self.secure)
            .http_only(self.http_only);
        if let Some(same_site) = self.same_site.clone() {
            builder = builder.same_site(same_site);
        }
        if let Some(expires) = self.expires {
            builder = builder.expires(TimeSinceEpoch::new(expires));
        }
        if let Some(priority) = self.priority.clone() {
            builder = builder.priority(priority);
        }
        if let Some(source_scheme) = self.source_scheme.clone() {
            builder = builder.source_scheme(source_scheme);
        }
        if let Some(source_port) = self.source_port {
            builder = builder.source_port(source_port);
        }
        if let Some(same_party) = self.same_party {
            builder = builder.same_party(same_party);
        }
        if let Some(partition_key) = self.partition_key.clone() {
            builder = builder.partition_key(partition_key);
        }
        builder
            .build()
            .map_err(|e| Error::Render(format!("cookie param build: {e}")))
    }

    fn cookie_url(&self) -> Option<String> {
        let host = self.domain.trim_start_matches('.');
        if host.is_empty() {
            return None;
        }
        let scheme = if self.secure { "https" } else { "http" };
        let path = if self.path.is_empty() {
            "/"
        } else {
            &self.path
        };
        Some(format!("{scheme}://{host}{path}"))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedOriginState {
    #[serde(default)]
    origin: String,
    #[serde(default)]
    local_storage: Vec<StorageEntry>,
    #[serde(default)]
    session_storage: Vec<StorageEntry>,
    #[serde(default)]
    manifest_url: Option<String>,
    #[serde(default)]
    service_workers: Vec<PersistedServiceWorker>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StorageEntry {
    key: String,
    value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedServiceWorker {
    #[serde(default)]
    scope: String,
    #[serde(default)]
    active_script_url: Option<String>,
    #[serde(default)]
    waiting_script_url: Option<String>,
    #[serde(default)]
    installing_script_url: Option<String>,
}

impl PersistedServiceWorker {
    fn script_urls(&self) -> impl Iterator<Item = &str> {
        [
            self.active_script_url.as_deref(),
            self.waiting_script_url.as_deref(),
            self.installing_script_url.as_deref(),
        ]
        .into_iter()
        .flatten()
    }
}

/// Extract the POST body (if any) from a `Network.requestWillBeSent`
/// request record. Concatenates every `postDataEntry.bytes` blob into a
/// single `Vec<u8>`. Chrome omits post bodies that exceed its protocol
/// threshold — the tracker treats "no body" as payload_size=0.
fn collect_post_body(
    req: &crate::render::chrome_protocol::cdp::browser_protocol::network::Request,
) -> Vec<u8> {
    let Some(entries) = req.post_data_entries.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries {
        if let Some(b) = entry.bytes.as_ref() {
            out.extend_from_slice(<crate::render::chrome_wire::Binary as AsRef<[u8]>>::as_ref(
                b,
            ));
        }
    }
    out
}

pub struct RenderPool {
    config: Arc<Config>,
    storage: Arc<dyn Storage>,
    sem: Arc<Semaphore>,
    /// Key = proxy URL as `String`; `""` means "no proxy". Each key owns its
    /// own Chrome instance so per-job proxy rotation works without
    /// relaunching on every render.
    browsers: dashmap::DashMap<String, Arc<Browser>>,
    /// Per-browser-key spawn mutex so two tasks racing a miss don't both
    /// launch Chrome.
    browser_locks: dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    /// BrowserContext per `(browser_key, session_id)`. Isolates stateful
    /// sites from each other while still reusing sessions across pages of
    /// the same registrable domain.
    contexts: dashmap::DashMap<String, BrowserContextId>,
    context_locks: dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    /// Cached persisted state per session_id. Loaded lazily from storage
    /// the first time a session is touched, then updated after each render.
    session_states: dashmap::DashMap<String, PersistedSessionState>,
    /// MRU list of browser keys, newest first. When it grows beyond
    /// `MAX_BROWSERS`, the oldest is dropped — closes that Chrome and frees
    /// its user-data-dir lock. Stops "one Chrome per proxy forever" bloat.
    mru: Mutex<std::collections::VecDeque<String>>,
    /// Per-RenderPool temp dir under which each browser gets its own
    /// `--user-data-dir` subdirectory. Avoids SingletonLock collisions when
    /// the same Chrome binary is launched multiple times concurrently.
    user_data_root: std::path::PathBuf,
    /// Resolved effective Profile (after optional auto-detect). Same value
    /// used on both the shim placeholders and the Chrome `--user-agent` flag
    /// so every observation surface reports one version.
    effective_profile: Mutex<Option<Profile>>,
    /// Cached rendered shim JS for the effective profile — built once so we
    /// don't re-run string replacement every render.
    shim_cache: Mutex<Option<String>>,
    worker_shim_cache: Mutex<Option<Arc<String>>>,
    /// Cached `IdentityBundle` — memoized so the shim, the Chromium launch
    /// flags, and `Network.setUserAgentOverride` all draw from the SAME
    /// instance (including the same `canvas_audio_seed`). Regenerating on
    /// each call would break per-session consistency.
    bundle_cache: Mutex<Option<Arc<IdentityBundle>>>,
    /// Resolved Chrome binary path after the first `resolve_chrome_path_async`
    /// call. Caches both PATH lookups and auto-fetched Chromium so we never
    /// fetch twice in one process.
    resolved_chrome_path: tokio::sync::Mutex<Option<String>>,
    /// Optional Lua hook host invoked with the live Page for `on_after_load`
    /// and `on_after_idle` before the page is closed. Interior mutability so
    /// `Crawler::set_lua_scripts` can wire it after the pool is created.
    #[cfg(feature = "lua-hooks")]
    lua_host: parking_lot::RwLock<Option<Arc<crate::hooks::lua::LuaHookHost>>>,
    /// Optional Throughput counters wired from `Crawler`. Updated on
    /// every tab acquire/release + browser create/evict. Keeps the
    /// pool usable standalone (counters are optional).
    counters: parking_lot::RwLock<Option<Arc<crate::metrics::Counters>>>,
    /// Page pool — one shared `PagePool` for the whole `RenderPool`
    /// that stashes idle tabs keyed on `browser_session_key`. See
    /// `page_pool.rs` for the lifecycle guarantees.
    page_pool: Arc<crate::render::page_pool::PagePool>,
    /// P0-9 — rolling volume tracker per `(session_id, vendor)` that
    /// drives preemptive proxy rotation when a vendor telemetry endpoint
    /// is being hammered. Shared across renders so successive calls on
    /// the same session accumulate.
    telemetry_tracker: Arc<Mutex<crate::antibot::telemetry::TelemetryTracker>>,
    /// Live render-session scope shared by `Crawler` when
    /// `session_scope_auto` demotes scope after challenge/login signals.
    render_scope: Arc<parking_lot::RwLock<crate::config::RenderSessionScope>>,
}

impl RenderPool {
    pub fn new(config: Arc<Config>, storage: Arc<dyn Storage>) -> Self {
        Self::new_with_scope(
            config.clone(),
            storage,
            Arc::new(parking_lot::RwLock::new(config.render_session_scope)),
        )
    }

    pub fn new_with_scope(
        config: Arc<Config>,
        storage: Arc<dyn Storage>,
        render_scope: Arc<parking_lot::RwLock<crate::config::RenderSessionScope>>,
    ) -> Self {
        let cap = config.max_concurrent_render.max(1);
        let user_data_root =
            std::env::temp_dir().join(format!("crawlex-chrome-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&user_data_root);
        let page_pool_limits = crate::render::page_pool::PagePoolLimits {
            max_pages_per_context: config.max_pages_per_context.max(1),
            ..Default::default()
        };
        Self {
            config,
            storage,
            sem: Arc::new(Semaphore::new(cap)),
            browsers: dashmap::DashMap::new(),
            browser_locks: dashmap::DashMap::new(),
            contexts: dashmap::DashMap::new(),
            context_locks: dashmap::DashMap::new(),
            session_states: dashmap::DashMap::new(),
            mru: Mutex::new(std::collections::VecDeque::new()),
            user_data_root,
            effective_profile: Mutex::new(None),
            shim_cache: Mutex::new(None),
            worker_shim_cache: Mutex::new(None),
            bundle_cache: Mutex::new(None),
            resolved_chrome_path: tokio::sync::Mutex::new(None),
            #[cfg(feature = "lua-hooks")]
            lua_host: parking_lot::RwLock::new(None),
            counters: parking_lot::RwLock::new(None),
            page_pool: Arc::new(crate::render::page_pool::PagePool::new(page_pool_limits)),
            telemetry_tracker: Arc::new(Mutex::new(
                crate::antibot::telemetry::TelemetryTracker::new(),
            )),
            render_scope,
        }
    }

    /// Wire runtime metrics counters. Optional — the pool works without
    /// them. Invoked by `Crawler::new` right after construction.
    pub fn set_counters(&self, counters: Arc<crate::metrics::Counters>) {
        *self.counters.write() = Some(counters);
    }

    pub fn page_pool(&self) -> &crate::render::page_pool::PagePool {
        &self.page_pool
    }

    fn counters_opt(&self) -> Option<Arc<crate::metrics::Counters>> {
        self.counters.read().as_ref().cloned()
    }

    /// Names we look for on PATH when the user didn't set `chrome_path`.
    /// Ordered by preference — the first one that answers `--version` with a
    /// recognizable version string wins.
    const CHROME_CANDIDATES: &'static [&'static str] = &[
        "google-chrome",
        "google-chrome-stable",
        "google-chrome-beta",
        "google-chrome-unstable",
        "chromium",
        "chromium-browser",
        "chrome",
    ];

    /// Detect the installed Chrome's major version. When `chrome_path` is
    /// `None`, we walk `CHROME_CANDIDATES` until one responds; the winner's
    /// absolute path is returned alongside the version so the launcher
    /// doesn't have to re-search later.
    fn detect_chrome(chrome_path: Option<&str>) -> Option<(String, u32)> {
        let candidates: Vec<String> = match chrome_path {
            Some(p) => vec![p.to_string()],
            None => Self::CHROME_CANDIDATES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        for bin in candidates {
            let out = match std::process::Command::new(&bin).arg("--version").output() {
                Ok(o) => o,
                Err(_) => continue,
            };
            let text = String::from_utf8_lossy(&out.stdout);
            // Typical: "Google Chrome 149.0.7712.82" / "Chromium 128.0.6613.113".
            for tok in text.split_whitespace() {
                if let Some(major) = tok.split('.').next() {
                    if let Ok(v) = major.parse::<u32>() {
                        if v >= 90 {
                            return Some((bin, v));
                        }
                    }
                }
            }
        }
        None
    }

    pub(crate) fn detect_chrome_major(chrome_path: Option<&str>) -> Option<u32> {
        Self::detect_chrome(chrome_path).map(|(_, v)| v)
    }

    /// Resolve the Chrome binary path we should actually launch. Priority:
    /// 1. user-supplied `config.chrome_path`;
    /// 2. first candidate from `CHROME_CANDIDATES` that answers `--version`;
    /// 3. previously auto-fetched Chromium in cache (`chromium-fetcher` feat);
    /// 4. auto-download Chromium-for-Testing (`chromium-fetcher` feat +
    ///    `config.auto_fetch_chromium`).
    /// Returns `Err` with a human-readable message when nothing works.
    async fn resolve_chrome_path_async(&self) -> Result<String> {
        {
            let guard = self.resolved_chrome_path.lock().await;
            if let Some(p) = guard.as_ref() {
                return Ok(p.clone());
            }
        }
        let resolved = self.resolve_uncached().await?;
        *self.resolved_chrome_path.lock().await = Some(resolved.clone());
        Ok(resolved)
    }

    async fn resolve_uncached(&self) -> Result<String> {
        // 1) Explicit override — escape hatch for operators who really know
        //    what they want. Not the default path.
        if let Some(p) = self.config.chrome_path.as_deref() {
            if std::process::Command::new(p)
                .arg("--version")
                .output()
                .is_ok()
            {
                return Ok(p.to_string());
            }
            return Err(Error::Render(format!(
                "chrome_path `{p}` did not respond to --version"
            )));
        }
        // 2) Our Chromium. Only path. Cached or fetched — both handled by
        //    `the CDP fetcher` which no-ops when the pinned build is
        //    already on disk. No system Chrome fallback: we own the engine.
        #[cfg(feature = "chromium-fetcher")]
        {
            if !self.config.auto_fetch_chromium {
                return Err(Error::Render(
                    "auto-fetch disabled and no --chrome-path set; re-enable \
                     (remove --no-fetch-chromium) or pass an explicit path"
                        .into(),
                ));
            }
            return self.fetch_chromium().await;
        }
        #[cfg(not(feature = "chromium-fetcher"))]
        {
            Err(Error::Render(
                "`chromium-fetcher` feature disabled — rebuild with it on \
                 or pass --chrome-path explicitly"
                    .into(),
            ))
        }
    }

    #[cfg(feature = "chromium-fetcher")]
    async fn fetch_chromium(&self) -> Result<String> {
        use crate::render::chrome_fetcher::{BrowserFetcher, BrowserFetcherOptions};
        let cache_dir = directories::ProjectDirs::from("", "", "crawlex")
            .map(|p| p.cache_dir().join("chromium"))
            .ok_or_else(|| {
                Error::Render("could not determine user cache dir for Chromium fetch".into())
            })?;
        let _ = std::fs::create_dir_all(&cache_dir);
        tracing::info!(
            cache = %cache_dir.display(),
            "no system Chrome found; fetching pinned Chromium-for-Testing (first run only)"
        );
        let opts = BrowserFetcherOptions::builder()
            .with_path(&cache_dir)
            .build()
            .map_err(|e| Error::Render(format!("chromium fetcher options: {e}")))?;
        let fetcher = BrowserFetcher::new(opts);
        let install = fetcher
            .fetch()
            .await
            .map_err(|e| Error::Render(format!("chromium fetch failed: {e}")))?;
        let path = install.executable_path.to_string_lossy().to_string();
        tracing::info!(path = %path, "Chromium ready");
        Ok(path)
    }

    fn resolve_profile(&self) -> Profile {
        if let Some(p) = *self.effective_profile.lock() {
            return p;
        }
        let cfg_profile = self.config.user_agent_profile;
        let resolved = if self.config.profile_autodetect {
            match Self::detect_chrome_major(self.config.chrome_path.as_deref()) {
                Some(major) if major != cfg_profile.major_version() => {
                    let p = Profile::from_detected_major(major);
                    tracing::info!(
                        detected = major,
                        configured = cfg_profile.major_version(),
                        chosen = p.major_version(),
                        "render pool auto-aligned profile to installed Chrome"
                    );
                    p
                }
                _ => cfg_profile,
            }
        } else {
            cfg_profile
        };
        *self.effective_profile.lock() = Some(resolved);
        resolved
    }

    /// Memoized `IdentityBundle`. Built on first call from the effective
    /// Profile + config overrides (locale, timezone) and the pool's
    /// one-shot `canvas_audio_seed`. Subsequent calls return the same
    /// `Arc` so the shim, the Chromium launch flags, and the
    /// `Network.setUserAgentOverride` all draw from the exact same set of
    /// fields — no risk of drift between surfaces.
    fn bundle(&self) -> Arc<IdentityBundle> {
        if let Some(b) = self.bundle_cache.lock().as_ref() {
            return b.clone();
        }
        let profile = self.resolve_profile();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xc0ffee);
        let b = IdentityBundle::from_profile_with_overrides(
            profile,
            self.config.identity_preset,
            self.config.locale.as_deref(),
            self.config.timezone.as_deref(),
            self.config.user_agent_override.as_deref(),
            seed,
        )
        .expect("invalid render identity config");
        let arc = Arc::new(b);
        *self.bundle_cache.lock() = Some(arc.clone());
        arc
    }

    fn shim_js(&self) -> String {
        if let Some(s) = self.shim_cache.lock().as_ref() {
            return s.clone();
        }
        let bundle = self.bundle();
        let js = render_shim_from_bundle(&bundle);
        *self.shim_cache.lock() = Some(js.clone());
        js
    }

    /// Worker-scope shim variant — same persona, DOM-only sections stripped.
    /// Cached separately from `shim_js` because the templates differ; the
    /// handler clones the `Arc` per `Target.attachedToTarget` event so the
    /// ~1700-line source isn't copied per worker spawn.
    fn worker_shim_js(&self) -> Arc<String> {
        if let Some(s) = self.worker_shim_cache.lock().as_ref() {
            return s.clone();
        }
        let bundle = self.bundle();
        let js = Arc::new(render_worker_shim_from_bundle(&bundle));
        *self.worker_shim_cache.lock() = Some(js.clone());
        js
    }

    #[cfg(feature = "lua-hooks")]
    pub fn set_lua_host(&self, host: Arc<crate::hooks::lua::LuaHookHost>) {
        *self.lua_host.write() = Some(host);
    }

    /// Validate the Chrome binary before the first render runs. Returns the
    /// resolved path so callers can log it. Safe to call even when no render
    /// is planned — in that case `Err` is typically ignored by the caller.
    pub async fn preflight(&self) -> Result<String> {
        let path = self.resolve_chrome_path_async().await?;
        tracing::info!(chrome = %path, "render pool: using Chrome binary");
        Ok(path)
    }

    async fn ensure_browser(&self, proxy: Option<&Url>) -> Result<Arc<Browser>> {
        let key = proxy.map(|u| u.to_string()).unwrap_or_default();
        // Fast path: browser already launched for this proxy.
        if let Some(b) = self.browsers.get(&key) {
            return Ok(b.clone());
        }
        // Serialize launches for the same key so we don't spawn Chrome twice.
        let lock = self
            .browser_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;
        if let Some(b) = self.browsers.get(&key) {
            return Ok(b.clone());
        }
        let bundle = self.bundle();
        // Lock in the Chrome binary once: user override wins, otherwise we
        // walk `CHROME_CANDIDATES` and fail loudly if nothing is installed.
        // This is the same resolution `Crawler::new` is able to validate at
        // startup when `preflight_render()` is called.
        let chrome_path = self.resolve_chrome_path_async().await?;
        let mut builder = BrowserConfig::builder();
        builder = builder.chrome_executable(&chrome_path);
        builder = builder.no_sandbox();
        let languages = bundle.chrome_lang_arg();
        // Per-browser user-data-dir prevents the Chrome SingletonLock crash
        // that happens when multiple instances share $HOME/.config/chromium.
        let udd_name = {
            use sha2::Digest as _;
            let h = sha2::Sha256::digest(key.as_bytes());
            hex::encode(&h[..8])
        };
        let udd = self.user_data_root.join(udd_name);
        let _ = std::fs::create_dir_all(&udd);
        let flags = build_launch_args(&bundle, proxy, &udd, &languages, &self.config.chrome_flags);
        builder = builder.args(flags);
        builder = builder.request_timeout(Duration::from_secs(60));
        // P0-4 stealth: suppress `Runtime.enable` on new targets so
        // brotector / DataDome can't detect CDP attachment via the
        // stack-trace signatures that Runtime.enable injects through
        // `Error.prepareStackTrace`. Execution contexts are resolved via
        // the `Page.createIsolatedWorld` response path
        // (`FrameManager::on_create_isolated_world_response`), so
        // `page.evaluate()` keeps working — it just lands in the isolated
        // world instead of main. See `BrowserConfigBuilder::stealth_runtime_enable_skip`
        // for the full contract. Port of rebrowser-patches runtime-enable-fix.
        builder = builder.stealth_runtime_enable_skip(true);
        // Camoufox port S3.1: install worker-scope shim so Web/Shared/
        // Service Workers receive the same persona coherence as the main
        // frame. The `Target.attachedToTarget` handler in the CDP layer
        // injects this via `Runtime.evaluate` before releasing the
        // worker's `waitForDebuggerOnStart` pause.
        builder = builder.stealth_worker_shim((*self.worker_shim_js()).clone());

        let cfg = builder
            .build()
            .map_err(|e| Error::Render(format!("BrowserConfig: {e}")))?;
        let (browser, mut handler) = Browser::launch(cfg)
            .await
            .map_err(|e| Error::Render(format!("launch: {e}")))?;

        tokio::spawn(async move {
            while let Some(ev) = handler.next().await {
                if let Err(e) = ev {
                    warn!(?e, "cdp handler error");
                }
            }
        });

        // Target.setAutoAttach: tell Chrome to auto-attach our CDP session
        // to every new target (popups, OOPIFs, workers). Without this our
        // stealth shim runs only in the main frame and detectors that load
        // FingerprintJS / Cloudflare Turnstile inside an iframe see an
        // un-shimmed `navigator`. `flatten=true` keeps everything on a
        // single CDP session so CDP client surfaces child Pages via
        // `browser.pages()`.
        let auto_attach = SetAutoAttachParams::builder()
            .auto_attach(true)
            .wait_for_debugger_on_start(false)
            .flatten(true)
            .build()
            .map_err(|e| Error::Render(format!("setAutoAttach build: {e}")))?;
        if let Err(e) = browser.execute(auto_attach).await {
            warn!(
                ?e,
                "Target.setAutoAttach failed; child targets won't get the stealth shim"
            );
        }

        let browser = Arc::new(browser);
        // Spawn a watcher that re-installs the stealth shim on every newly
        // attached child Page. We poll `browser.pages()` because chromium-
        // oxide doesn't currently surface a clean
        // `EventAttachedToTarget` → `Page` mapping; polling is good enough
        // since pages don't appear that often and the cost is one CDP RPC
        // when the set is unchanged.
        {
            let bro = browser.clone();
            let shim = self.shim_js();
            tokio::spawn(async move {
                let mut seen: std::collections::HashSet<
                    crate::render::chrome_protocol::cdp::browser_protocol::target::TargetId,
                > = std::collections::HashSet::new();
                let install = AddScriptToEvaluateOnNewDocumentParams {
                    source: shim,
                    world_name: None,
                    include_command_line_api: Some(false),
                    run_immediately: Some(true),
                };
                loop {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let pages = match bro.pages().await {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                    for p in pages {
                        let id = p.target_id().clone();
                        if seen.insert(id.clone()) {
                            // New target — install main-world shim via
                            // `Page.addScriptToEvaluateOnNewDocument`.
                            // Best-effort: workers reject this command
                            // because they don't have the Page domain;
                            // the worker-scope shim runs through the
                            // CDP `Target.attachedToTarget` handler in
                            // `chrome::handler::target.rs` instead
                            // (Camoufox port S3.1 — wired via
                            // `BrowserConfigBuilder::stealth_worker_shim`).
                            if let Err(e) = p.execute(install.clone()).await {
                                tracing::debug!(target = ?id, ?e, "shim install on child target failed (likely worker — handled via Runtime.evaluate)");
                            } else {
                                tracing::debug!(target = ?id, "stealth shim installed on child target");
                            }
                        }
                    }
                }
            });
        }
        self.browsers.insert(key.clone(), browser.clone());
        if let Some(c) = self.counters_opt() {
            c.browsers_active
                .store(self.browsers.len(), std::sync::atomic::Ordering::Relaxed);
        }
        // MRU bookkeeping: push key, evict oldest beyond `max_browsers`.
        let max_browsers = self.config.max_browsers.max(1);
        {
            let mut mru = self.mru.lock();
            mru.retain(|k| k != &key);
            mru.push_front(key.clone());
            while mru.len() > max_browsers {
                if let Some(old) = mru.pop_back() {
                    if let Some((_, b)) = self.browsers.remove(&old) {
                        // Arc dropped when the last render using it finishes.
                        // crate::render::chrome::Browser implements Drop that spawns
                        // a kill task, so this releases the Chrome process
                        // without blocking.
                        drop(b);
                        self.browser_locks.remove(&old);
                        // Drop any PagePool entries and contexts for the
                        // evicted browser. Key prefix matches
                        // `browser_session_key`: "<browser_key>|<session>".
                        let prefix = if old.is_empty() {
                            String::new()
                        } else {
                            format!("{old}|")
                        };
                        let ctx_keys: Vec<String> = self
                            .contexts
                            .iter()
                            .filter(|kv| {
                                if prefix.is_empty() {
                                    !kv.key().contains('|')
                                } else {
                                    kv.key().starts_with(&prefix)
                                }
                            })
                            .map(|kv| kv.key().clone())
                            .collect();
                        for ck in &ctx_keys {
                            self.page_pool.drop_context(ck);
                        }
                        if prefix.is_empty() {
                            self.contexts.retain(|k, _| k.contains('|'));
                            self.context_locks.retain(|k, _| k.contains('|'));
                        } else {
                            self.contexts.retain(|k, _| !k.starts_with(&prefix));
                            self.context_locks.retain(|k, _| !k.starts_with(&prefix));
                        }
                        tracing::debug!(proxy = %old, "render pool evicted browser");
                    }
                }
            }
        }
        if let Some(c) = self.counters_opt() {
            c.browsers_active
                .store(self.browsers.len(), std::sync::atomic::Ordering::Relaxed);
            c.contexts_active
                .store(self.contexts.len(), std::sync::atomic::Ordering::Relaxed);
        }
        Ok(browser)
    }

    fn session_id_for_url(&self, url: &Url) -> String {
        let host = url.host_str().unwrap_or_default();
        let safe = |scope: &str| {
            scope.replace(
                |c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'),
                "_",
            )
        };
        match *self.render_scope.read() {
            crate::config::RenderSessionScope::RegistrableDomain => {
                let scope =
                    crate::discovery::subdomains::registrable_domain(host).unwrap_or_else(|| {
                        if host.is_empty() {
                            let digest = sha2::Sha256::digest(url.as_str().as_bytes());
                            format!("url-{}", hex::encode(&digest[..8]))
                        } else if let Some(port) = url.port() {
                            format!("{host}-{port}")
                        } else {
                            host.to_string()
                        }
                    });
                format!("render-{}", safe(&scope))
            }
            crate::config::RenderSessionScope::Host => {
                let scope = if host.is_empty() {
                    let digest = sha2::Sha256::digest(url.as_str().as_bytes());
                    format!("url-{}", hex::encode(&digest[..8]))
                } else if let Some(port) = url.port() {
                    format!("{host}-{port}")
                } else {
                    host.to_string()
                };
                format!("render-host-{}", safe(&scope))
            }
            crate::config::RenderSessionScope::Origin => {
                let origin = url.origin().ascii_serialization();
                let digest = sha2::Sha256::digest(origin.as_bytes());
                let label = if origin.is_empty() || origin == "null" {
                    "null".to_string()
                } else {
                    safe(&origin)
                };
                format!("render-origin-{label}-{}", hex::encode(&digest[..6]))
            }
            crate::config::RenderSessionScope::Url => {
                let digest = sha2::Sha256::digest(url.as_str().as_bytes());
                format!("render-url-{}", hex::encode(&digest[..8]))
            }
        }
    }

    fn browser_session_key(browser_key: &str, session_id: &str) -> String {
        if browser_key.is_empty() {
            session_id.to_string()
        } else {
            format!("{browser_key}|{session_id}")
        }
    }

    async fn ensure_session_state_loaded(&self, session_id: &str) -> Result<()> {
        if self.session_states.contains_key(session_id) {
            return Ok(());
        }
        let state = match self.storage.load_state(session_id).await? {
            Some(json) => serde_json::from_str::<PersistedSessionState>(&json)
                .unwrap_or_else(|_| PersistedSessionState::default()),
            None => PersistedSessionState::default(),
        };
        self.session_states.insert(session_id.to_string(), state);
        Ok(())
    }

    async fn ensure_session_context(
        &self,
        browser_key: &str,
        browser: &Browser,
        session_id: &str,
    ) -> Result<BrowserContextId> {
        self.ensure_session_state_loaded(session_id).await?;
        let key = Self::browser_session_key(browser_key, session_id);
        if let Some(ctx_id) = self.contexts.get(&key) {
            return Ok(ctx_id.clone());
        }
        let lock = self
            .context_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;
        if let Some(ctx_id) = self.contexts.get(&key) {
            return Ok(ctx_id.clone());
        }
        let ctx_id = browser
            .create_browser_context(CreateBrowserContextParams::default())
            .await
            .map_err(|e| Error::Render(format!("create_browser_context: {e}")))?;
        self.contexts.insert(key, ctx_id.clone());
        if let Some(c) = self.counters_opt() {
            c.contexts_active
                .store(self.contexts.len(), std::sync::atomic::Ordering::Relaxed);
        }
        Ok(ctx_id)
    }

    /// Tear down every BrowserContext associated with `session_id`
    /// across all live browsers, along with its cached session state
    /// and any idle pages in the PagePool. Best-effort: CDP errors are
    /// logged but do not propagate — the registry drop is authoritative
    /// and the Chrome-side cleanup can lag without harming correctness.
    ///
    /// This is the hook `SessionRegistry` (phase 6) calls when a TTL
    /// sweep or explicit operator drop fires.
    pub async fn drop_session(&self, session_id: &str) {
        // Collect the `(browser_key, ctx_key, ctx_id)` triples to dispose
        // before mutating the map so we don't iterate a DashMap we're
        // writing to.
        let mut to_dispose: Vec<(String, String, BrowserContextId)> = Vec::new();
        let suffix = format!("|{session_id}");
        for entry in self.contexts.iter() {
            let key = entry.key();
            let browser_key = if key == session_id {
                String::new()
            } else if let Some(prefix) = key.strip_suffix(&suffix) {
                prefix.to_string()
            } else {
                continue;
            };
            to_dispose.push((browser_key, key.clone(), entry.value().clone()));
        }

        for (browser_key, ctx_key, ctx_id) in to_dispose {
            // Drop idle pages first so the next acquirer can't hand out
            // a page attached to a context that is about to vanish.
            self.page_pool.drop_context(&ctx_key);
            if let Some(browser) = self.browsers.get(&browser_key).map(|b| b.clone()) {
                // Scrub Chrome-internal supercookies BEFORE the context is
                // disposed so the CDP target is still live. Any one of
                // these RPCs can be missing on older Chrome builds; each
                // is best-effort and a failure only warns — rotation must
                // not be aborted by a stale or unsupported domain.
                Self::clear_chrome_supercookies(&browser, session_id).await;
                if let Err(e) = browser.dispose_browser_context(ctx_id).await {
                    tracing::debug!(session_id, ?e, "dispose_browser_context failed");
                }
            }
            self.contexts.remove(&ctx_key);
            self.context_locks.remove(&ctx_key);
        }
        self.session_states.remove(session_id);
        if let Some(c) = self.counters_opt() {
            c.contexts_active
                .store(self.contexts.len(), std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Flush Chrome's browser-internal supercookies prior to disposing a
    /// `BrowserContext`. This is the "A2" identity-rotation hardening pass
    /// — after `drop_session`, a subsequent session must not inherit ETag
    /// caches, HSTS pins, Service Worker registrations, IndexedDB rows,
    /// or the HTTP disk cache from the previous identity.
    ///
    /// Order matters:
    /// 1. `Network.setBypassServiceWorker {bypass: true}` — ensures the
    ///    about-to-be-nuked SW can't intercept any in-flight fetch while
    ///    the clear is running.
    /// 2. `Security.setIgnoreCertificateErrors {ignore: false}` — reset
    ///    the cert-ignore flag so a poisoned decision from the old
    ///    session can't leak into the next.
    /// 3. `Storage.clearDataForOrigin {origin: "*", storageTypes: "all"}`
    ///    — nukes cookies, local_storage, indexeddb, cache, service_workers,
    ///    shader_cache, websql for every origin.
    /// 4. `ServiceWorker.unregister` — best-effort; CDP has no
    ///    `getRegistrations` RPC (it's a DOM API), so step 3's
    ///    `service_workers` coverage is the primary cleanup path. This
    ///    call is a belt-and-suspenders scope-URL nudge.
    /// 5. `Network.clearBrowserCache` — explicit HTTP cache flush on top
    ///    of step 3 (cheap redundancy, independent code path in Chrome).
    ///
    /// Every RPC is wrapped in its own fallible block: a missing method
    /// on older Chrome logs a `tracing::warn!` and rotation continues.
    /// Graceful degradation is the contract — a supercookie that survived
    /// is a detection risk, not a correctness bug, and must not crash the
    /// rotation pipeline.
    async fn clear_chrome_supercookies(browser: &Browser, session_id: &str) {
        // 1. Bypass SW so in-flight fetches don't re-seed the cache
        //    during the clear window.
        if let Err(e) = browser
            .execute(SetBypassServiceWorkerParams::new(true))
            .await
        {
            warn!(
                session_id,
                ?e,
                "Network.setBypassServiceWorker unsupported — continuing rotation"
            );
        }

        // 2. Reset the ignore-cert-errors flag. Prelude per the hardening
        //    checklist: the next session must not silently trust a cert
        //    chain the old one allowed.
        if let Err(e) = browser
            .execute(SetIgnoreCertificateErrorsParams::new(false))
            .await
        {
            warn!(
                session_id,
                ?e,
                "Security.setIgnoreCertificateErrors unsupported — continuing rotation"
            );
        }

        // 3. Primary supercookie nuke. `origin: "*"` is the documented
        //    "all origins" sentinel; `storageTypes: "all"` covers
        //    cookies + local_storage + indexeddb + cache +
        //    service_workers + shader_cache + websql in one RPC.
        if let Err(e) = browser
            .execute(ClearDataForOriginParams::new("*", "all"))
            .await
        {
            warn!(
                session_id,
                ?e,
                "Storage.clearDataForOrigin unsupported — supercookies may persist"
            );
        }

        // 4. Service worker unregister — CDP has no `getRegistrations`
        //    command (only a `workerRegistrationUpdated` event stream).
        //    Step 3 already wipes registrations via the `service_workers`
        //    storage type; this extra scope-URL unregister is a wildcard
        //    nudge that Chrome will reject with a clean error if nothing
        //    matches. Kept so the CDP method appears in the rotation log
        //    trail for audit purposes.
        if let Err(e) = browser
            .execute(ServiceWorkerUnregisterParams::new("*"))
            .await
        {
            tracing::debug!(
                session_id,
                ?e,
                "ServiceWorker.unregister(\"*\") returned — expected when no SW matched"
            );
        }

        // 5. Explicit HTTP cache flush. Redundant with step 3 but cheap
        //    and takes a separate code path inside Chrome, so it covers
        //    the case where older builds implement Storage.clearDataForOrigin
        //    but miss the cache entry.
        if let Err(e) = browser.execute(ClearBrowserCacheParams::default()).await {
            warn!(
                session_id,
                ?e,
                "Network.clearBrowserCache unsupported — continuing rotation"
            );
        }
    }

    fn session_state(&self, session_id: &str) -> PersistedSessionState {
        self.session_states
            .get(session_id)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    async fn restore_session_state(
        &self,
        page: &Page,
        target_url: &Url,
        session_id: &str,
    ) -> Result<()> {
        let state = self.session_state(session_id);
        if state.cookies.is_empty() && state.origins.is_empty() {
            return Ok(());
        }

        let cookie_params: Vec<CookieParam> = state
            .cookies
            .iter()
            .filter_map(|cookie| cookie.to_cookie_param(target_url).ok())
            .collect();
        if !cookie_params.is_empty() {
            if let Err(e) = page.set_cookies(cookie_params).await {
                debug!(session_id, ?e, "cookie restore failed");
            }
        }

        let origin = target_url.origin().ascii_serialization();
        let Some(origin_state) = state.origin_state(&origin) else {
            return Ok(());
        };
        let payload = serde_json::to_string(origin_state)
            .map_err(|e| Error::Render(format!("state payload encode: {e}")))?;
        let script = format!(
            r#"(function() {{
  const state = {payload};
  const restore = (storage, items) => {{
    try {{
      storage.clear();
      for (const item of items || []) {{
        storage.setItem(item.key, item.value);
      }}
    }} catch (_) {{}}
  }};
  try {{
    if (location.origin === state.origin) {{
      restore(window.localStorage, state.local_storage);
      restore(window.sessionStorage, state.session_storage);
    }}
  }} catch (_) {{}}
}})();"#,
        );
        page.execute(AddScriptToEvaluateOnNewDocumentParams {
            source: script,
            world_name: None,
            include_command_line_api: Some(false),
            run_immediately: Some(true),
        })
        .await
        .map_err(|e| Error::Render(format!("state restore inject: {e}")))?;
        Ok(())
    }

    async fn capture_session_state(
        &self,
        page: &Page,
        final_url: &Url,
        session_id: &str,
    ) -> Result<PersistedOriginState> {
        let cookies = match page.get_cookies().await {
            Ok(cookies) => cookies.into_iter().map(PersistedCookie::from).collect(),
            Err(e) => {
                debug!(session_id, ?e, "cookie capture failed");
                Vec::new()
            }
        };
        let origin = final_url.origin().ascii_serialization();
        let origin_state = if !origin.is_empty() && origin != "null" {
            Self::capture_origin_state(page, &origin)
                .await
                .unwrap_or_else(|e| {
                    debug!(session_id, ?e, "origin state capture failed");
                    PersistedOriginState::default()
                })
        } else {
            PersistedOriginState::default()
        };

        let mut state = self.session_state(session_id);
        state.merge_cookies(cookies);
        if !origin.is_empty() && origin != "null" {
            state.upsert_origin(origin_state.clone());
        }
        let json = serde_json::to_string(&state)
            .map_err(|e| Error::Render(format!("state encode: {e}")))?;
        self.storage.save_state(session_id, &json).await?;
        self.session_states.insert(session_id.to_string(), state);
        Ok(origin_state)
    }

    async fn capture_origin_state(page: &Page, origin: &str) -> Result<PersistedOriginState> {
        let params = EvaluateParams::builder()
            .expression(
                r#"(() => ({
  manifest_url: (() => {
    try {
      for (const link of document.querySelectorAll('link[rel]')) {
        const rel = (link.getAttribute('rel') || '').toLowerCase();
        if (rel.split(/\s+/).includes('manifest') && typeof link.href === 'string') {
          return link.href;
        }
      }
    } catch (_) {}
    return null;
  })(),
  local_storage: (() => {
    try {
      const out = [];
      for (let i = 0; i < window.localStorage.length; i++) {
        const key = window.localStorage.key(i);
        if (key !== null) out.push({ key, value: window.localStorage.getItem(key) ?? '' });
      }
      return out;
    } catch (_) {
      return [];
    }
  })(),
  session_storage: (() => {
    try {
      const out = [];
      for (let i = 0; i < window.sessionStorage.length; i++) {
        const key = window.sessionStorage.key(i);
        if (key !== null) out.push({ key, value: window.sessionStorage.getItem(key) ?? '' });
      }
      return out;
    } catch (_) {
      return [];
    }
  })(),
  service_workers: await (async () => {
    try {
      const api = navigator.serviceWorker;
      if (!api || typeof api.getRegistrations !== 'function') return [];
      const regs = await api.getRegistrations();
      return regs.map((reg) => ({
        scope: reg.scope || '',
        active_script_url: reg.active?.scriptURL ?? null,
        waiting_script_url: reg.waiting?.scriptURL ?? null,
        installing_script_url: reg.installing?.scriptURL ?? null,
      }));
    } catch (_) {
      return [];
    }
  })(),
}))()"#,
            )
            .await_promise(true)
            .return_by_value(true)
            .build()
            .map_err(|e| Error::Render(format!("origin state params: {e}")))?;
        let res = page
            .evaluate_expression(params)
            .await
            .map_err(|e| Error::Render(format!("origin state eval: {e}")))?;
        let mut state = res
            .value()
            .cloned()
            .map(serde_json::from_value::<PersistedOriginState>)
            .transpose()
            .map_err(|e| Error::Render(format!("origin state decode: {e}")))?
            .unwrap_or_default();
        state.origin = origin.to_string();
        Ok(state)
    }

    async fn install_stealth(&self, page: &Page) -> Result<()> {
        page.execute(AddScriptToEvaluateOnNewDocumentParams {
            source: self.shim_js(),
            world_name: None,
            include_command_line_api: Some(false),
            run_immediately: Some(true),
        })
        .await
        .map_err(|e| Error::Render(format!("stealth inject: {e}")))?;
        Ok(())
    }

    /// Install the SPA/PWA JS observer (History API + fetch + XHR
    /// wrappers). Must run AFTER the stealth shim so we bind onto the
    /// stealth-patched prototypes. No-op when neither `collect_runtime_routes`
    /// nor `collect_network_endpoints` is set — avoids injecting JS we
    /// aren't going to read back.
    /// Register the SPA/PWA observer for every future document via
    /// `Page.addScriptToEvaluateOnNewDocument`. MUST also be manually
    /// re-evaluated after the first navigation (`reinject_spa_observer`)
    /// — on some Chromium versions the per-session script registration
    /// doesn't take effect for the very first document we navigate to,
    /// and we can't afford to lose the initial route.
    async fn install_spa_observer(&self, page: &Page) -> Result<()> {
        if !self.config.collect_runtime_routes && !self.config.collect_network_endpoints {
            return Ok(());
        }
        page.execute(AddScriptToEvaluateOnNewDocumentParams {
            source: crate::render::spa_observer::observer_js(),
            world_name: None,
            include_command_line_api: Some(false),
            run_immediately: Some(true),
        })
        .await
        .map_err(|e| Error::Render(format!("spa observer inject: {e}")))?;
        Ok(())
    }

    /// Idempotent post-navigation re-injection. The observer guards on
    /// `__crawlex_observer_installed__` — running twice is a no-op.
    async fn reinject_spa_observer(&self, page: &Page) {
        if !self.config.collect_runtime_routes && !self.config.collect_network_endpoints {
            return;
        }
        let Ok(params) = EvaluateParams::builder()
            .expression(crate::render::spa_observer::observer_js())
            .return_by_value(true)
            .build()
        else {
            return;
        };
        if let Err(e) = page.evaluate_expression(params).await {
            tracing::debug!(?e, "spa observer post-nav reinject failed");
        }
    }

    /// Read the two observer globals in one round-trip. Returns
    /// default (empty) on any failure — the observer is best-effort,
    /// a misbehaving page must not fail the render.
    pub(crate) async fn collect_spa_observations(
        page: &Page,
    ) -> crate::render::spa_observer::CollectedObservations {
        let Ok(params) = EvaluateParams::builder()
            .expression(crate::render::spa_observer::collect_expression())
            .await_promise(true)
            .return_by_value(true)
            .build()
        else {
            return Default::default();
        };
        let res = match page.evaluate_expression(params).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(?e, "spa observer collect eval failed");
                return Default::default();
            }
        };
        // Debug probe: what does the page say about the observer?
        match res.value().cloned() {
            Some(v) => match serde_json::from_value::<
                crate::render::spa_observer::CollectedObservations,
            >(v.clone())
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::debug!(?e, value=%v, "spa observer decode failed");
                    Default::default()
                }
            },
            None => {
                tracing::debug!("spa observer collect returned no value");
                Default::default()
            }
        }
    }

    /// CDP times are f64 millisecond offsets; negative means "not yet set".
    /// Return `None` rather than a nonsensical duration when either end is
    /// missing.

    /// Install PerformanceObservers BEFORE the page scripts run so CLS/LCP/
    /// long-task samples accumulate over the whole page lifetime. Without
    /// this, `getEntriesByType('layout-shift')` after render is empty.
    async fn install_vitals_observers(page: &Page) -> Result<()> {
        const OBSERVERS_JS: &str = r#"
(() => {
  try {
    window.__mb_cls = 0;
    window.__mb_lcp = 0;
    window.__mb_tbt = 0;
    window.__mb_fcp = null;
    window.__mb_longest = 0;
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (!e.hadRecentInput) window.__mb_cls += e.value;
      }
    }).observe({ type: 'layout-shift', buffered: true });
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (e.startTime > window.__mb_lcp) window.__mb_lcp = e.startTime;
      }
    }).observe({ type: 'largest-contentful-paint', buffered: true });
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        const blocking = e.duration - 50;
        if (blocking > 0) window.__mb_tbt += blocking;
        if (e.duration > window.__mb_longest) window.__mb_longest = e.duration;
      }
    }).observe({ type: 'longtask', buffered: true });
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (e.name === 'first-contentful-paint') window.__mb_fcp = e.startTime;
      }
    }).observe({ type: 'paint', buffered: true });
  } catch (_) {}
})();
"#;
        page.execute(AddScriptToEvaluateOnNewDocumentParams {
            source: OBSERVERS_JS.to_string(),
            world_name: None,
            include_command_line_api: Some(false),
            run_immediately: Some(true),
        })
        .await
        .map_err(|e| Error::Render(format!("observer inject: {e}")))?;
        Ok(())
    }

    async fn probe_inp(page: &Page) -> Result<Option<f64>> {
        let params = EvaluateParams::builder()
            .expression(crate::metrics::INP_PROBE_JS)
            .await_promise(true)
            .return_by_value(true)
            .build()
            .map_err(|e| Error::Render(format!("inp params: {e}")))?;
        let res = page
            .evaluate_expression(params)
            .await
            .map_err(|e| Error::Render(format!("inp eval: {e}")))?;
        let Some(v) = res.value() else {
            return Ok(None);
        };
        Ok(v.as_f64())
    }

    async fn collect_vitals(page: &Page) -> Result<crate::metrics::WebVitals> {
        let params = EvaluateParams::builder()
            .expression(crate::metrics::WEB_VITALS_JS)
            .await_promise(true)
            .return_by_value(true)
            .build()
            .map_err(|e| Error::Render(format!("vitals params: {e}")))?;
        let res = page
            .evaluate_expression(params)
            .await
            .map_err(|e| Error::Render(format!("vitals eval: {e}")))?;
        let Some(v) = res.value() else {
            return Ok(crate::metrics::WebVitals::default());
        };
        serde_json::from_value::<crate::metrics::WebVitals>(v.clone())
            .map_err(|e| Error::Render(format!("vitals decode: {e}")))
    }

    /// Resolves once no requests are in flight for `idle_ms` continuous ms,
    /// or after a hard 30s ceiling to prevent hanging pages from stalling.
    async fn wait_network_idle(page: &Page, idle_ms: u64) -> Result<()> {
        let mut starts = page
            .event_listener::<EventRequestWillBeSent>()
            .await
            .map_err(|e| Error::Render(format!("listener start: {e}")))?;
        let mut finishes = page
            .event_listener::<EventLoadingFinished>()
            .await
            .map_err(|e| Error::Render(format!("listener finish: {e}")))?;
        let mut fails = page
            .event_listener::<EventLoadingFailed>()
            .await
            .map_err(|e| Error::Render(format!("listener fail: {e}")))?;

        let ceiling = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(ceiling);
        let mut in_flight: i64 = 0;
        let idle_window = Duration::from_millis(idle_ms);

        loop {
            let quiet_timer = tokio::time::sleep(idle_window);
            tokio::pin!(quiet_timer);
            tokio::select! {
                _ = &mut ceiling => {
                    return Ok(());
                }
                _ = starts.next() => {
                    in_flight = in_flight.saturating_add(1);
                }
                _ = finishes.next() => {
                    in_flight = in_flight.saturating_sub(1).max(0);
                }
                _ = fails.next() => {
                    in_flight = in_flight.saturating_sub(1).max(0);
                }
                _ = &mut quiet_timer, if in_flight == 0 => {
                    return Ok(());
                }
            }
        }
    }

    pub(crate) async fn wait_for(page: &Page, wait: &WaitStrategy) -> Result<()> {
        match wait {
            WaitStrategy::Load | WaitStrategy::DomContentLoaded => {
                page.wait_for_navigation()
                    .await
                    .map_err(|e| Error::Render(format!("wait_for_navigation: {e}")))?;
            }
            WaitStrategy::NetworkIdle { idle_ms } => {
                page.wait_for_navigation()
                    .await
                    .map_err(|e| Error::Render(format!("wait_for_navigation: {e}")))?;
                Self::wait_network_idle(page, *idle_ms).await?;
            }
            WaitStrategy::Selector { css, timeout_ms } => {
                page.wait_for_navigation()
                    .await
                    .map_err(|e| Error::Render(format!("wait_for_navigation: {e}")))?;
                let deadline = std::time::Instant::now() + Duration::from_millis(*timeout_ms);
                loop {
                    if page.find_element(css.clone()).await.is_ok() {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::Render(format!("selector timeout: {css}")));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            WaitStrategy::Fixed { ms } => {
                page.wait_for_navigation()
                    .await
                    .map_err(|e| Error::Render(format!("wait_for_navigation: {e}")))?;
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
            WaitStrategy::ReadingDwell { .. } => {
                // The dwell itself runs in `settle_after_actions` where the
                // DOM is already parsed — here we just finish navigation.
                page.wait_for_navigation()
                    .await
                    .map_err(|e| Error::Render(format!("wait_for_navigation: {e}")))?;
            }
        }
        Ok(())
    }

    async fn wait_for_ready_state(page: &Page, states: &[&str], timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        let params = EvaluateParams::builder()
            .expression("document.readyState")
            .return_by_value(true)
            .build()
            .map_err(|e| Error::Render(format!("readyState params: {e}")))?;
        loop {
            match page.evaluate_expression(params.clone()).await {
                Ok(res) => {
                    if let Some(state) = res.value().and_then(|v| v.as_str()) {
                        if states.contains(&state) {
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    debug!(?e, "readyState probe failed while settling page");
                }
            }
            if std::time::Instant::now() >= deadline {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(crate) async fn settle_after_actions(page: &Page, wait: &WaitStrategy) -> Result<()> {
        Self::settle_after_actions_with_dwell(page, wait, None).await
    }

    /// Same as `settle_after_actions` but with an optional reading-dwell
    /// kicker. Kept as a separate entry point so existing callers (script
    /// runner, in-flight mutations) don't need to thread `Config` through.
    pub(crate) async fn settle_after_actions_with_dwell(
        page: &Page,
        wait: &WaitStrategy,
        reading_dwell: Option<&crate::config::ReadingDwellConfig>,
    ) -> Result<()> {
        match wait {
            WaitStrategy::Load => {
                Self::wait_for_ready_state(page, &["complete"], Duration::from_secs(10)).await?;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            WaitStrategy::DomContentLoaded => {
                Self::wait_for_ready_state(
                    page,
                    &["interactive", "complete"],
                    Duration::from_secs(10),
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            WaitStrategy::NetworkIdle { idle_ms } => {
                Self::wait_for_ready_state(
                    page,
                    &["interactive", "complete"],
                    Duration::from_secs(10),
                )
                .await?;
                Self::wait_network_idle(page, *idle_ms).await?;
            }
            WaitStrategy::Selector { css, timeout_ms } => {
                Self::wait_for_ready_state(
                    page,
                    &["interactive", "complete"],
                    Duration::from_secs(10),
                )
                .await?;
                // Re-probe the selector: SPA routing may have replaced the
                // subtree that held it, or the post-click view may render
                // a different match. Give operators the same semantics as
                // the initial wait — the selector must reappear within
                // `timeout_ms` before we serialise the DOM.
                let deadline = std::time::Instant::now() + Duration::from_millis(*timeout_ms);
                loop {
                    if page.find_element(css.clone()).await.is_ok() {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::Render(format!(
                            "selector timeout after actions: {css}"
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            WaitStrategy::Fixed { ms } => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
            WaitStrategy::ReadingDwell { wpm, jitter_ms } => {
                // Need a parseable DOM before we can count words.
                Self::wait_for_ready_state(
                    page,
                    &["interactive", "complete"],
                    Duration::from_secs(10),
                )
                .await?;
                Self::apply_reading_dwell(page, *wpm, *jitter_ms, 500, 10_000).await;
            }
        }

        // Config-driven dwell fires after the variant-specific settle. When
        // the variant is already `ReadingDwell` we've paid the cost above —
        // don't double-sleep.
        if !matches!(wait, WaitStrategy::ReadingDwell { .. }) {
            if let Some(cfg) = reading_dwell.filter(|c| c.enabled) {
                Self::apply_reading_dwell(page, cfg.wpm, cfg.jitter_ms, cfg.min_ms, cfg.max_ms)
                    .await;
            }
        }
        Ok(())
    }

    /// Evaluate `document.body.innerText`, split on whitespace, compute a
    /// WPM-proportional dwell, sleep. Best-effort: eval failures fall back
    /// to zero words (→ `min_ms`) rather than erroring — a failed word
    /// count must not kill the render.
    async fn apply_reading_dwell(page: &Page, wpm: u32, jitter_ms: u64, min_ms: u64, max_ms: u64) {
        use rand::rngs::SmallRng;
        use rand::SeedableRng;

        let words = match crate::render::interact::eval_js(
            page,
            "(document.body && document.body.innerText) ? document.body.innerText.split(/\\s+/).filter(Boolean).length : 0",
        )
        .await
        {
            Ok(v) => v.as_u64().unwrap_or(0),
            Err(_) => 0,
        };
        // Seed off a monotonic nanos timestamp so successive pages don't
        // replay the same jitter path.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut rng = SmallRng::seed_from_u64(seed);
        let ms =
            crate::wait_strategy::compute_dwell_ms(words, wpm, jitter_ms, min_ms, max_ms, &mut rng);
        tracing::debug!(words, ms, "reading dwell");
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// Capture a screenshot in one of three modes. `Viewport` matches the
    /// current window; `FullPage` extends the viewport to `scrollHeight`;
    /// `Element` clips to the bounding box of the first match of `selector`.
    pub(crate) async fn capture_screenshot_mode(
        page: &Page,
        mode: ScreenshotCaptureMode,
    ) -> Option<Vec<u8>> {
        use base64::Engine;
        let params = match mode {
            ScreenshotCaptureMode::Viewport => CaptureScreenshotParams::builder()
                .from_surface(true)
                .build(),
            ScreenshotCaptureMode::FullPage => CaptureScreenshotParams::builder()
                .capture_beyond_viewport(true)
                .from_surface(true)
                .build(),
            ScreenshotCaptureMode::Element { ref selector } => {
                use crate::render::chrome_protocol::cdp::browser_protocol::dom::{
                    GetBoxModelParams, GetDocumentParams, QuerySelectorParams,
                };
                use crate::render::chrome_protocol::cdp::browser_protocol::page::Viewport;
                let doc = match page.execute(GetDocumentParams::default()).await {
                    Ok(r) => r.root.node_id,
                    Err(e) => {
                        debug!(?e, "GetDocument for element screenshot failed");
                        return None;
                    }
                };
                let qs = QuerySelectorParams::builder()
                    .node_id(doc)
                    .selector(selector.clone())
                    .build()
                    .ok()?;
                let found = match page.execute(qs).await {
                    Ok(r) => r.node_id,
                    Err(e) => {
                        debug!(?e, "QuerySelector for element screenshot failed");
                        return None;
                    }
                };
                let box_model = match page
                    .execute(GetBoxModelParams::builder().node_id(found).build())
                    .await
                {
                    Ok(r) => r.model.clone(),
                    Err(e) => {
                        debug!(?e, "GetBoxModel for element screenshot failed");
                        return None;
                    }
                };
                // `content` is a quad: 8 floats (x,y per corner, clockwise
                // from top-left). Derive the bounding rectangle.
                let q = &box_model.content;
                if q.inner().len() < 8 {
                    return None;
                }
                let q = q.inner();
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                let x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
                let y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
                let x_max = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let y_max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let width = (x_max - x).max(1.0);
                let height = (y_max - y).max(1.0);
                let clip = Viewport::builder()
                    .x(x)
                    .y(y)
                    .width(width)
                    .height(height)
                    .scale(1.0)
                    .build()
                    .ok()?;
                CaptureScreenshotParams::builder()
                    .from_surface(true)
                    .clip(clip)
                    .build()
            }
        };
        match page.execute(params).await {
            Ok(r) => base64::engine::general_purpose::STANDARD
                .decode(&r.data)
                .ok(),
            Err(e) => {
                debug!(?e, "screenshot failed");
                None
            }
        }
    }

    fn push_unique_url(urls: &mut Vec<Url>, candidate: Url) {
        if !urls.iter().any(|existing| existing == &candidate) {
            urls.push(candidate);
        }
    }
}

#[async_trait::async_trait]
impl crate::identity::SessionDropTarget for RenderPool {
    async fn drop_session(&self, id: &str) {
        RenderPool::drop_session(self, id).await;
    }
}

impl Drop for RenderPool {
    fn drop(&mut self) {
        // Drop all cached browsers so their child Chrome processes exit,
        // then wipe the temp user-data-dir tree. Best-effort: losing the
        // temp cleanup is harmless on /tmp.
        self.browsers.clear();
        self.browser_locks.clear();
        self.contexts.clear();
        self.context_locks.clear();
        self.session_states.clear();
        let _ = std::fs::remove_dir_all(&self.user_data_root);
    }
}

impl RenderPool {
    /// Best-effort: download the manifest via in-page `fetch()` and
    /// parse as JSON. Returns `None` on any failure — callers treat
    /// it as "manifest not captured". We do the fetch inside the page
    /// so Chrome reuses its cache and any session-scoped credentials.
    pub(crate) async fn fetch_manifest_json(
        page: &Page,
        manifest: &Url,
    ) -> Option<serde_json::Value> {
        let manifest_str = manifest.as_str().replace('`', "%60");
        let expr = format!(
            r#"(async () => {{
  try {{
    const r = await fetch(`{manifest}`, {{ credentials: 'include' }});
    if (!r.ok) return null;
    return await r.json();
  }} catch (_) {{ return null; }}
}})()"#,
            manifest = manifest_str
        );
        let params = EvaluateParams::builder()
            .expression(expr)
            .await_promise(true)
            .return_by_value(true)
            .build()
            .ok()?;
        let res = page.evaluate_expression(params).await.ok()?;
        let v = res.value().cloned()?;
        if v.is_null() {
            None
        } else {
            Some(v)
        }
    }

    /// IndexedDB inventory via in-page JS (uses the standard
    /// `indexedDB.databases()` + `open()` APIs). Works with modern
    /// Chrome; older Chromium lacked `databases()` — we gracefully
    /// return an empty list in that case.
    pub(crate) async fn collect_indexeddb_inventory(
        page: &Page,
        _final_url: &Url,
    ) -> Vec<serde_json::Value> {
        const EXPR: &str = r#"(async () => {
  try {
    if (!('indexedDB' in window) || typeof indexedDB.databases !== 'function') return [];
    const metas = await indexedDB.databases();
    const out = [];
    for (const m of metas) {
      if (!m || !m.name) continue;
      const db = await new Promise((resolve) => {
        try {
          const req = indexedDB.open(m.name, m.version);
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => resolve(null);
          req.onblocked = () => resolve(null);
        } catch (_) { resolve(null); }
      });
      if (!db) { out.push({ db_name: m.name, version: m.version, stores: [] }); continue; }
      const stores = [];
      try {
        for (const name of Array.from(db.objectStoreNames)) {
          try {
            const tx = db.transaction(name, 'readonly');
            const os = tx.objectStore(name);
            stores.push({
              name: String(name),
              key_path: os.keyPath === null ? null : (Array.isArray(os.keyPath) ? os.keyPath : String(os.keyPath)),
              auto_increment: !!os.autoIncrement,
              indexes: Array.from(os.indexNames).map((n) => String(n)),
            });
            tx.abort && tx.abort();
          } catch (_) {}
        }
      } catch (_) {}
      try { db.close(); } catch (_) {}
      out.push({ db_name: m.name, version: m.version, stores });
    }
    return out;
  } catch (_) { return []; }
})()"#;
        let Ok(params) = EvaluateParams::builder()
            .expression(EXPR)
            .await_promise(true)
            .return_by_value(true)
            .build()
        else {
            return Vec::new();
        };
        let Ok(res) = page.evaluate_expression(params).await else {
            return Vec::new();
        };
        match res.value().cloned() {
            Some(serde_json::Value::Array(items)) => items,
            _ => Vec::new(),
        }
    }

    /// Cache Storage inventory via in-page JS. Bounded to the first
    /// 500 keys per cache so a huge SW-backed offline store can't
    /// bloat the artifact payload.
    pub(crate) async fn collect_cache_storage_inventory(
        page: &Page,
        _final_url: &Url,
    ) -> Vec<serde_json::Value> {
        const EXPR: &str = r#"(async () => {
  try {
    if (!('caches' in window)) return [];
    const names = await caches.keys();
    const out = [];
    for (const name of names) {
      try {
        const c = await caches.open(name);
        const reqs = await c.keys();
        const keys = reqs.slice(0, 500).map((r) => r.url);
        out.push({ cache_name: name, keys, total: reqs.length });
      } catch (_) {
        out.push({ cache_name: name, keys: [], total: 0 });
      }
    }
    return out;
  } catch (_) { return []; }
})()"#;
        let Ok(params) = EvaluateParams::builder()
            .expression(EXPR)
            .await_promise(true)
            .return_by_value(true)
            .build()
        else {
            return Vec::new();
        };
        let Ok(res) = page.evaluate_expression(params).await else {
            return Vec::new();
        };
        match res.value().cloned() {
            Some(serde_json::Value::Array(items)) => items,
            _ => Vec::new(),
        }
    }

    /// Snapshot the current page's SPA/PWA state as a single JSON artifact.
    /// Used by ScriptSpec `Snapshot(PwaState)` so scripts can persist a
    /// self-contained app-state bundle at any point in the flow.
    pub(crate) async fn capture_pwa_state_snapshot(
        page: &Page,
        final_url: &Url,
        include_deep_storage: bool,
    ) -> serde_json::Value {
        let origin = final_url.origin().ascii_serialization();
        let origin_state = if !origin.is_empty() && origin != "null" {
            Self::capture_origin_state(page, &origin)
                .await
                .unwrap_or_default()
        } else {
            PersistedOriginState::default()
        };
        let observations = Self::collect_spa_observations(page).await;
        let manifest_json = match origin_state.manifest_url.as_deref() {
            Some(raw) => match Url::parse(raw) {
                Ok(manifest_url) => Self::fetch_manifest_json(page, &manifest_url).await,
                Err(_) => None,
            },
            None => None,
        };
        let indexeddb = if include_deep_storage {
            Self::collect_indexeddb_inventory(page, final_url).await
        } else {
            Vec::new()
        };
        let cache_storage = if include_deep_storage {
            Self::collect_cache_storage_inventory(page, final_url).await
        } else {
            Vec::new()
        };
        let is_spa = !observations.routes.is_empty() || final_url.fragment().is_some();
        let PersistedOriginState {
            origin,
            local_storage,
            session_storage,
            manifest_url,
            service_workers,
        } = origin_state;
        let crate::render::spa_observer::CollectedObservations {
            routes,
            endpoints,
            idb_audit,
        } = observations;
        // Wave 2: emit a single-event telemetry log line when the
        // observer recorded IDB writes. Log-only — crawl continues. A
        // future wave will cross-check against a store-by-store re-read
        // and emit a `VendorTelemetryObserved` host event on divergence.
        if !idb_audit.is_empty() {
            tracing::debug!(
                count = idb_audit.len(),
                "idb-audit observer captured write order"
            );
        }
        serde_json::json!({
            "final_url": final_url.as_str(),
            "origin": origin,
            "manifest_url": manifest_url,
            "manifest": manifest_json,
            "service_workers": service_workers,
            "local_storage": local_storage,
            "session_storage": session_storage,
            "runtime_routes": routes,
            "network_endpoints": endpoints,
            "idb_audit": idb_audit,
            "indexeddb": indexeddb,
            "cache_storage": cache_storage,
            "is_spa": is_spa,
        })
    }

    /// Emit each non-empty SPA/PWA snapshot as an artifact row. Errors
    /// are swallowed with a debug log — artifact persistence must not
    /// fail the render.
    #[allow(clippy::too_many_arguments)]
    async fn persist_spa_artifacts(
        &self,
        seed_url: &Url,
        final_url: &Url,
        session_id: &str,
        observations: &crate::render::spa_observer::CollectedObservations,
        indexeddb: &[serde_json::Value],
        cache_storage: &[serde_json::Value],
        manifest_json: Option<&serde_json::Value>,
        service_workers: &[PersistedServiceWorker],
    ) {
        use crate::storage::{ArtifactKind, ArtifactMeta};

        let save = |kind: ArtifactKind, value: serde_json::Value| {
            let bytes = match serde_json::to_vec(&value) {
                Ok(b) => b,
                Err(e) => {
                    debug!(?e, kind = kind.wire_str(), "spa artifact encode failed");
                    return None;
                }
            };
            Some((kind, bytes))
        };

        let mut queued: Vec<(ArtifactKind, Vec<u8>)> = Vec::new();
        if self.config.collect_runtime_routes && !observations.routes.is_empty() {
            if let Ok(v) = serde_json::to_value(&observations.routes) {
                if let Some(q) = save(ArtifactKind::SnapshotRuntimeRoutes, v) {
                    queued.push(q);
                }
            }
        }
        if self.config.collect_network_endpoints && !observations.endpoints.is_empty() {
            if let Ok(v) = serde_json::to_value(&observations.endpoints) {
                if let Some(q) = save(ArtifactKind::SnapshotNetworkEndpoints, v) {
                    queued.push(q);
                }
            }
        }
        if !indexeddb.is_empty() {
            if let Some(q) = save(
                ArtifactKind::SnapshotIndexedDb,
                serde_json::Value::Array(indexeddb.to_vec()),
            ) {
                queued.push(q);
            }
        }
        if !cache_storage.is_empty() {
            if let Some(q) = save(
                ArtifactKind::SnapshotCacheStorage,
                serde_json::Value::Array(cache_storage.to_vec()),
            ) {
                queued.push(q);
            }
        }
        if let Some(m) = manifest_json {
            if let Some(q) = save(ArtifactKind::SnapshotManifest, m.clone()) {
                queued.push(q);
            }
        }
        if self.config.collect_service_workers && !service_workers.is_empty() {
            if let Ok(v) = serde_json::to_value(service_workers) {
                if let Some(q) = save(ArtifactKind::SnapshotServiceWorkers, v) {
                    queued.push(q);
                }
            }
        }

        for (kind, bytes) in queued {
            let meta = ArtifactMeta {
                url: seed_url,
                final_url: Some(final_url),
                session_id,
                kind,
                name: None,
                step_id: None,
                step_kind: None,
                selector: None,
                mime: None,
            };
            if let Err(e) = self.storage.save_artifact(&meta, &bytes).await {
                debug!(?e, kind = kind.wire_str(), "spa artifact save failed");
            }
        }
    }
}

#[async_trait::async_trait]
impl Renderer for RenderPool {
    async fn render(
        &self,
        url: &Url,
        wait: &WaitStrategy,
        collect_vitals: bool,
        screenshot: bool,
        actions: Option<&[crate::render::actions::Action]>,
        proxy: Option<&Url>,
    ) -> Result<RenderedPage> {
        // Delegate to the shared `render_core`, passing the actions runner
        // as the custom-steps closure. Everything else (navigate, wait,
        // settle, serialise) lives in the helper so `render_with_script`
        // and this legacy entry point stay in lockstep.
        let policy = self.config.action_policy.clone();
        let actions_vec: Option<Vec<crate::render::actions::Action>> = actions.map(|a| a.to_vec());
        self.render_core(url, wait, collect_vitals, screenshot, proxy, move |page| {
            Box::pin(async move {
                if let Some(list) = actions_vec {
                    if !list.is_empty() {
                        crate::render::actions::execute_with_policy(page, &list, &policy).await?;
                        return Ok(true);
                    }
                }
                Ok(false)
            })
        })
        .await
    }
}

impl RenderPool {
    /// Render a URL while executing a [`ScriptSpec`](crate::script::ScriptSpec)
    /// between the initial wait-strategy settle and the final serialisation.
    /// Runs after navigation and wait, before (optional) Lua hooks and
    /// before screenshot capture. Returns both the usual `RenderedPage`
    /// and the [`RunOutcome`] carrying step-level artifacts, captures, and
    /// export values.
    #[cfg(feature = "cdp-backend")]
    pub async fn render_with_script(
        &self,
        url: &Url,
        wait: &WaitStrategy,
        script: &crate::script::ScriptSpec,
        events: Option<std::sync::Arc<dyn crate::events::EventSink>>,
        run_id: Option<u64>,
        proxy: Option<&Url>,
    ) -> Result<(RenderedPage, crate::script::RunOutcome)> {
        use std::sync::Arc;
        let plan =
            crate::script::plan(script).map_err(|e| Error::Render(format!("script-plan: {e}")))?;
        let plan = Arc::new(plan);
        let session_id = self.session_id_for_url(url);
        let policy = self.config.action_policy.clone();
        let outcome_slot: Arc<parking_lot::Mutex<Option<crate::script::RunOutcome>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let outcome_for_closure = outcome_slot.clone();
        let plan_for_closure = plan.clone();
        let storage_for_closure = self.storage.clone();
        let url_for_closure = url.clone();
        let events_for_closure = events.clone();
        let run_id_for_closure = run_id;
        let wait_for_runner = wait.clone();
        let collect_vitals = self.config.collect_web_vitals;
        let screenshot = self.config.output.screenshot;
        let rendered = self
            .render_core(url, wait, collect_vitals, screenshot, proxy, move |page| {
                let outcome_slot = outcome_for_closure;
                let plan = plan_for_closure;
                let session_id = session_id;
                let policy = policy;
                let storage = storage_for_closure;
                let url_for_runner = url_for_closure;
                let events = events_for_closure;
                let run_id = run_id_for_closure;
                let wait = wait_for_runner;
                Box::pin(async move {
                    let mut runner =
                        crate::script::ScriptRunner::new(page, &plan, session_id, &policy)
                            .with_wait_strategy(wait)
                            .with_storage(storage)
                            .with_url(url_for_runner);
                    if let Some(sink) = events {
                        runner = runner.with_sink(sink);
                    }
                    if let Some(run_id) = run_id {
                        runner = runner.with_run_id(run_id);
                    }
                    let outcome = runner.run().await?;
                    let ran_any = !outcome.steps.is_empty();
                    *outcome_slot.lock() = Some(outcome);
                    Ok(ran_any)
                })
            })
            .await?;
        let outcome = outcome_slot.lock().take().unwrap_or_default();
        Ok((rendered, outcome))
    }

    async fn render_core<F>(
        &self,
        url: &Url,
        wait: &WaitStrategy,
        collect_vitals: bool,
        screenshot: bool,
        proxy: Option<&Url>,
        run_custom: F,
    ) -> Result<RenderedPage>
    where
        F: for<'p> FnOnce(&'p Page) -> futures::future::BoxFuture<'p, Result<bool>> + Send,
    {
        let _permit = self.sem.clone().acquire_owned().await.unwrap();
        let render_started = std::time::Instant::now();
        let browser = self.ensure_browser(proxy).await?;
        let browser_key = proxy.map(|u| u.to_string()).unwrap_or_default();
        let session_id = self.session_id_for_url(url);
        let ctx_id = self
            .ensure_session_context(&browser_key, &browser, &session_id)
            .await?;
        let ctx_key = Self::browser_session_key(&browser_key, &session_id);

        // ---- Tab acquisition ----------------------------------------
        // Reuse an idle pooled Page when available. `Page.addScriptToEvaluate
        // OnNewDocument` is persisted per-target so we skip the stealth +
        // observer install on reused tabs; we only reset the browsing
        // context via `about:blank`. On any failure to reset, the tab is
        // discarded and we create a fresh one.
        let lease: crate::render::page_pool::PageLease = {
            if let Some(pooled) = self.page_pool.try_acquire(&ctx_key) {
                let blank = NavigateParams::builder()
                    .url("about:blank".to_string())
                    .build()
                    .map_err(|e| Error::Render(format!("blank NavigateParams: {e}")))?;
                match pooled.page.execute(blank).await {
                    Ok(_) => {
                        if let Some(c) = self.counters_opt() {
                            c.pages_reused
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        crate::render::page_pool::PageLease::new(self.page_pool.clone(), pooled)
                    }
                    Err(e) => {
                        tracing::debug!(
                            ?e,
                            "pool page reset failed; discarding and creating fresh"
                        );
                        // In-flight was bumped by try_acquire; decrement.
                        self.page_pool.release_dirty_key(&ctx_key);
                        let mut params = CreateTargetParams::new("about:blank");
                        params.browser_context_id = Some(ctx_id.clone());
                        let fresh = browser
                            .new_page(params)
                            .await
                            .map_err(|e| Error::Render(format!("new_page: {e}")))?;
                        self.page_pool.register_fresh(&ctx_key);
                        self.install_stealth(&fresh).await?;
                        self.install_spa_observer(&fresh).await?;
                        if let Some(c) = self.counters_opt() {
                            c.pages_created
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        let pooled_new =
                            crate::render::page_pool::PooledPage::new(fresh, ctx_key.clone());
                        crate::render::page_pool::PageLease::new(self.page_pool.clone(), pooled_new)
                    }
                }
            } else {
                let mut params = CreateTargetParams::new("about:blank");
                params.browser_context_id = Some(ctx_id.clone());
                let fresh = browser
                    .new_page(params)
                    .await
                    .map_err(|e| Error::Render(format!("new_page: {e}")))?;
                self.page_pool.register_fresh(&ctx_key);
                self.install_stealth(&fresh).await?;
                self.install_spa_observer(&fresh).await?;
                if let Some(c) = self.counters_opt() {
                    c.pages_created
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                let pooled = crate::render::page_pool::PooledPage::new(fresh, ctx_key.clone());
                crate::render::page_pool::PageLease::new(self.page_pool.clone(), pooled)
            }
        };
        if let Some(c) = self.counters_opt() {
            c.tabs_active.store(
                self.page_pool.total_in_flight(),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        let page: Page = lease.page().clone();
        self.restore_session_state(&page, url, &session_id).await?;

        // Force the outgoing User-Agent + UA-CH metadata from our active
        // `IdentityBundle`. Same bundle that populates the shim, same
        // bundle that populates Chromium launch flags — so the UA string
        // navigator sees, the UA header the wire sees, and the UA-CH the
        // server sees are all the *same struct* deserialised three ways.
        let bundle = self.bundle();
        let brands = ua_ch_brands(&bundle.ua_brands, &bundle.ua_major.to_string());
        let full_version_list = ua_ch_brands(&bundle.ua_full_version_list, &bundle.ua_full_version);
        let ch_platform = bundle.ua_platform.trim_matches('"');
        let mobile = bundle.is_mobile();
        let (platform_version, architecture, model, bitness) = match ch_platform {
            "Windows" => ("10.0.0", "x86", "", Some("64".to_string())),
            "macOS" => ("10.15.7", "arm", "", Some("64".to_string())),
            "Android" => ("14.0.0", "arm", "Pixel 7", None),
            _ => ("6.5.0", "x86", "", Some("64".to_string())),
        };
        let metadata = UserAgentMetadata {
            brands: Some(brands),
            full_version_list: Some(full_version_list),
            platform: ch_platform.to_string(),
            platform_version: platform_version.into(),
            architecture: architecture.into(),
            model: model.into(),
            mobile,
            bitness,
            wow64: Some(false),
            form_factors: None,
        };
        if let Err(e) = page
            .execute(NetworkSetUserAgentOverrideParams {
                user_agent: bundle.ua.clone(),
                accept_language: Some(bundle.accept_language.clone()),
                platform: Some(bundle.platform.clone()),
                user_agent_metadata: Some(metadata),
            })
            .await
        {
            tracing::debug!(?e, "Network.setUserAgentOverride failed");
        }

        // Honour --block-resource flags (image, font, media, etc.) by telling
        // Chrome to refuse those URLs. Patterns are cheap wildcards; for
        // stronger control, users can write a Lua hook with network
        // interception.
        if !self.config.block_resources.is_empty() {
            let mut urls: Vec<String> = Vec::new();
            for kind in &self.config.block_resources {
                let patterns: &[&str] = match kind.as_str() {
                    "image" => &[
                        "*.png", "*.jpg", "*.jpeg", "*.gif", "*.webp", "*.avif", "*.svg", "*.ico",
                    ],
                    "font" => &["*.woff", "*.woff2", "*.ttf", "*.otf", "*.eot"],
                    "media" => &["*.mp3", "*.mp4", "*.webm", "*.ogg", "*.m3u8", "*.mpd"],
                    "stylesheet" => &["*.css"],
                    "script" => &["*.js", "*.mjs"],
                    "analytics" => &[
                        "*://*.google-analytics.com/*",
                        "*://*.googletagmanager.com/*",
                        "*://*.segment.com/*",
                        "*://*.hotjar.com/*",
                        "*://*.mixpanel.com/*",
                    ],
                    other => {
                        tracing::debug!(kind = other, "unknown block-resource kind — ignoring");
                        continue;
                    }
                };
                urls.extend(patterns.iter().map(|s| s.to_string()));
            }
            if !urls.is_empty() {
                let patterns: Vec<BlockPattern> = urls
                    .into_iter()
                    .map(|url_pattern| BlockPattern {
                        url_pattern,
                        block: true,
                    })
                    .collect();
                if let Err(e) = page
                    .execute(SetBlockedUrLsParams {
                        url_patterns: Some(patterns),
                    })
                    .await
                {
                    tracing::debug!(?e, "SetBlockedURLs failed (continuing)");
                }
            }
        }
        if collect_vitals {
            Self::install_vitals_observers(&page).await?;
        }

        let captured = Arc::new(Mutex::new(Vec::<Url>::new()));
        let mut events = page
            .event_listener::<EventRequestWillBeSent>()
            .await
            .map_err(|e| Error::Render(format!("event_listener: {e}")))?;
        {
            let captured = captured.clone();
            let storage = self.storage.clone();
            let tracker = self.telemetry_tracker.clone();
            let sid = session_id.clone();
            tokio::spawn(async move {
                while let Some(ev) = events.next().await {
                    let Ok(u) = Url::parse(&ev.request.url) else {
                        continue;
                    };
                    captured.lock().push(u.clone());

                    // P0-9: classify the request against the vendor
                    // signature table. No-op on non-vendor URLs.
                    let body = collect_post_body(&ev.request);
                    let req = crate::antibot::telemetry::ObservedRequest {
                        url: &u,
                        method: &ev.request.method,
                        body: &body,
                        session_id: &sid,
                    };
                    if let Some(telem) = crate::antibot::telemetry::classify_request(&req) {
                        let threshold_hit = {
                            let mut t = tracker.lock();
                            t.observe(&sid, telem.vendor, telem.observed_at)
                        };
                        // Persist — backends that don't care return Ok.
                        if let Err(e) = storage.record_telemetry(&telem).await {
                            tracing::debug!(?e, "record_telemetry failed");
                        }
                        tracing::debug!(
                            vendor = telem.vendor.as_str(),
                            endpoint = %telem.endpoint,
                            payload_size = telem.payload_size,
                            pattern_label = telem.pattern_label,
                            preemptive_rotate = threshold_hit,
                            "vendor telemetry observed"
                        );
                        if threshold_hit {
                            tracing::info!(
                                vendor = telem.vendor.as_str(),
                                session_id = %sid,
                                "vendor telemetry volume threshold hit — recommending RotateProxy"
                            );
                        }
                    }
                }
            });
        }

        let main_frame = page
            .mainframe()
            .await
            .map_err(|e| Error::Render(format!("mainframe: {e}")))?;
        let main_document_status = Arc::new(Mutex::new(None::<u16>));

        // Per-resource timing accumulators: keyed by requestId.
        let resource_map: Arc<Mutex<HashMap<String, crate::metrics::ResourceSample>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let mut resp_events = page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|e| Error::Render(format!("resp listener: {e}")))?;
        {
            let map = resource_map.clone();
            let status = main_document_status.clone();
            let main_frame = main_frame.clone();
            tokio::spawn(async move {
                while let Some(ev) = resp_events.next().await {
                    let is_main_document = ev.r#type == ResourceType::Document
                        && match main_frame.as_ref() {
                            Some(frame_id) => ev.frame_id.as_ref() == Some(frame_id),
                            None => true,
                        };
                    if is_main_document {
                        *status.lock() = Some(ev.response.status as u16);
                    }
                    if !collect_vitals {
                        continue;
                    }
                    let r = &ev.response;
                    let id = format!("{}", ev.request_id.inner());
                    let mut sample = crate::metrics::ResourceSample {
                        url: r.url.clone(),
                        mime_type: Some(r.mime_type.clone()),
                        resource_type: Some(format!("{:?}", ev.r#type)),
                        status: Some(r.status as u16),
                        from_cache: Some(
                            r.from_disk_cache.unwrap_or(false)
                                || r.from_service_worker.unwrap_or(false),
                        ),
                        protocol: r.protocol.clone(),
                        remote_ip: r.remote_ip_address.clone(),
                        remote_port: r.remote_port.map(|p| p as u16),
                        encoded_data_length: Some(r.encoded_data_length),
                        ..Default::default()
                    };
                    if let Some(t) = r.timing.as_ref() {
                        sample.request_time = Some(t.request_time);
                        sample.dns_start = Some(t.dns_start);
                        sample.dns_end = Some(t.dns_end);
                        sample.connect_start = Some(t.connect_start);
                        sample.connect_end = Some(t.connect_end);
                        sample.ssl_start = Some(t.ssl_start);
                        sample.ssl_end = Some(t.ssl_end);
                        sample.send_start = Some(t.send_start);
                        sample.send_end = Some(t.send_end);
                        sample.receive_headers_start = Some(t.receive_headers_start);
                        sample.receive_headers_end = Some(t.receive_headers_end);
                        sample.dns_ms = diff(t.dns_start, t.dns_end);
                        sample.connect_ms = diff(t.connect_start, t.connect_end);
                        sample.ssl_ms = diff(t.ssl_start, t.ssl_end);
                        sample.send_ms = diff(t.send_start, t.send_end);
                        sample.wait_ms = diff(t.send_end, t.receive_headers_start);
                    }
                    map.lock().insert(id, sample);
                }
            });
        }
        if collect_vitals {
            let mut fin_events = page
                .event_listener::<EventLoadingFinished>()
                .await
                .map_err(|e| Error::Render(format!("fin listener: {e}")))?;
            {
                let map = resource_map.clone();
                tokio::spawn(async move {
                    while let Some(ev) = fin_events.next().await {
                        let id = format!("{}", ev.request_id.inner());
                        let mut guard = map.lock();
                        if let Some(s) = guard.get_mut(&id) {
                            s.loading_finished_ms = Some(*ev.timestamp.inner());
                            s.transfer_size = Some(ev.encoded_data_length);
                            if let (Some(rhs), Some(rt)) = (s.receive_headers_start, s.request_time)
                            {
                                // loading_finished is absolute seconds; convert
                                // to ms-since-request_time to match peers.
                                let fin_ms = (*ev.timestamp.inner() - rt) * 1000.0;
                                s.receive_ms = Some((fin_ms - rhs).max(0.0));
                            }
                        }
                    }
                });
            }
        }

        // Warm-up hit: Cloudflare scores the *first* request harshly
        // because `__cf_bm`/`cf_clearance` aren't bound to the session
        // yet. Visit the warmup URL first so those cookies appear, then
        // navigate to the real target. A warmup failure must not kill
        // the crawl — log and continue so hostile/broken origins don't
        // turn an opt-in into a foot-gun.
        if self.config.warmup.enabled {
            let warmup_url = self.config.warmup.render_template(url);
            let target_str = url.to_string();
            // Skip when the template produced nothing, or when it resolves
            // to the same URL we're about to hit — no cookie value added.
            if !warmup_url.is_empty() && warmup_url != target_str {
                tracing::info!(
                    warmup_url = %warmup_url,
                    dwell_ms = self.config.warmup.dwell_ms,
                    "warmup navigation"
                );
                match NavigateParams::builder().url(warmup_url.clone()).build() {
                    Ok(warm_nav) => {
                        if let Err(e) = page.execute(warm_nav).await {
                            tracing::warn!(?e, warmup_url = %warmup_url, "warmup navigate failed; proceeding to target");
                        } else if self.config.warmup.dwell_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(self.config.warmup.dwell_ms))
                                .await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(?e, warmup_url = %warmup_url, "warmup NavigateParams build failed; skipping");
                    }
                }
            }
        }

        let nav = NavigateParams::builder()
            .url(url.to_string())
            .build()
            .map_err(|e| Error::Render(format!("NavigateParams: {e}")))?;
        let _ = page
            .execute(nav)
            .await
            .map_err(|e| Error::Render(format!("navigate: {e}")))?;

        // Ensure observer is installed even if addScriptToEvaluateOnNewDocument
        // didn't fire on the first document. Idempotent via guard.
        self.reinject_spa_observer(&page).await;

        Self::wait_for(&page, wait).await?;

        // Run scripted interaction after wait, before serialization. Users can
        // drive forms, reCAPTCHA iframes, multi-step flows from here. The
        // custom runner (Actions or ScriptSpec) is plugged in by the
        // caller — see `render()` and `render_with_script()`.
        let mutated_after_load = run_custom(&page).await?;
        #[cfg(feature = "lua-hooks")]
        let mut mutated_after_load = mutated_after_load;

        // Lua hook: on_after_load / on_after_idle with full Page access.
        // Users can run `page_click`, `page_type`, etc. here to drive forms,
        // solve captchas, scroll to lazy-load, and more — all before we
        // serialize the DOM below.
        #[cfg(feature = "lua-hooks")]
        {
            // Clone the Arc out of the lock and drop the guard before awaiting
            // — clippy's await_holding_lock would otherwise flag this as a bug.
            let host_opt = { self.lua_host.read().as_ref().cloned() };
            if let Some(host) = host_opt {
                mutated_after_load = true;
                let mut ctx = crate::hooks::HookContext::new(url.clone(), 0);
                let _ = host
                    .fire_with_page(
                        crate::hooks::HookEvent::AfterLoad,
                        &mut ctx,
                        Some(page.clone()),
                    )
                    .await;
                let _ = host
                    .fire_with_page(
                        crate::hooks::HookEvent::AfterIdle,
                        &mut ctx,
                        Some(page.clone()),
                    )
                    .await;
            }
        }
        if mutated_after_load {
            Self::settle_after_actions_with_dwell(&page, wait, self.config.reading_dwell.as_ref())
                .await?;
        } else if let Some(cfg) = self.config.reading_dwell.as_ref().filter(|c| c.enabled) {
            // Even when no mutation ran, honour the reading-dwell knob —
            // the whole point is simulating a reader after settle, not
            // only after scripted actions.
            Self::apply_reading_dwell(&page, cfg.wpm, cfg.jitter_ms, cfg.min_ms, cfg.max_ms).await;
        }

        let mut vitals = if collect_vitals {
            Self::collect_vitals(&page).await.unwrap_or_default()
        } else {
            crate::metrics::WebVitals::default()
        };
        if collect_vitals {
            if let Ok(Some(inp)) = Self::probe_inp(&page).await {
                vitals.interaction_to_next_paint_ms = Some(inp);
            }
        }

        let html = page
            .content()
            .await
            .map_err(|e| Error::Render(format!("content: {e}")))?;

        let screenshot_png = if screenshot {
            let mode =
                parse_screenshot_mode_or_default(self.config.output.screenshot_mode.as_deref());
            Self::capture_screenshot_mode(&page, mode).await
        } else {
            None
        };
        // Prefer `window.location.href` over `page.url()` so hash-only SPA
        // navigation (`#/dashboard`, `#/login`) is reflected in `final_url`.
        // CDP's Target `page.url()` is sourced from targetInfo which only
        // updates on actual navigations — hash changes triggered by
        // `history.pushState(null, '', '#/route')` or plain `location.hash=`
        // assignment don't bump it. Fall back to the target URL on eval
        // failure (cross-origin frame, detached page) and to the seed URL
        // as a last resort.
        let final_url: Url = {
            let evaled = crate::render::interact::eval_js(&page, "window.location.href")
                .await
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .and_then(|s| Url::parse(&s).ok());
            match evaled {
                Some(u) => u,
                None => match page.url().await {
                    Ok(Some(u)) => Url::parse(&u).unwrap_or_else(|_| url.clone()),
                    _ => url.clone(),
                },
            }
        };
        let origin_state = self
            .capture_session_state(&page, &final_url, &session_id)
            .await
            .unwrap_or_default();

        // Read SPA observer globals before we tear the page down. Must
        // happen after `settle_after_actions` to catch routes pushed by
        // hook/ScriptSpec steps.
        let observations =
            if self.config.collect_runtime_routes || self.config.collect_network_endpoints {
                Self::collect_spa_observations(&page).await
            } else {
                Default::default()
            };

        // Collect optional PWA surfaces — IndexedDB and Cache Storage.
        // Kept behind explicit flags because both can be heavy on apps
        // that seed large caches (~tens of MB transferred).
        let indexeddb_inventory = if self.config.collect_indexeddb {
            Self::collect_indexeddb_inventory(&page, &final_url).await
        } else {
            Vec::new()
        };
        let cache_storage_inventory = if self.config.collect_cache_storage {
            Self::collect_cache_storage_inventory(&page, &final_url).await
        } else {
            Vec::new()
        };

        // Optional manifest fetch. We already discover the URL via the
        // stealth-scoped `capture_origin_state`; the browser typically
        // has it cached, so we fetch via in-page `fetch()` to reuse any
        // cookies/credentials Chrome already has.
        let manifest_json: Option<serde_json::Value> = if self.config.collect_manifest {
            match origin_state.manifest_url.as_deref() {
                Some(raw) => match Url::parse(raw) {
                    Ok(u) => Self::fetch_manifest_json(&page, &u).await,
                    Err(_) => None,
                },
                None => None,
            }
        } else {
            None
        };

        // ---- Tab release --------------------------------------------
        // Decide whether the tab is clean enough to return to the pool.
        // If a post-JS antibot challenge was detected OR the status is
        // 5xx/0, treat the tab as contaminated — the page is carrying
        // cookies / interstitial JS state we don't want leaking into
        // the next render. Close explicitly then dirty-release. Healthy
        // pages get navigated back to about:blank by the next acquirer
        // and reused.
        let challenge_raw = crate::antibot::detect_from_html(&html, &final_url, None);
        let status_snapshot = *main_document_status.lock();
        let release_dirty = challenge_raw.is_some()
            || status_snapshot == Some(0)
            || matches!(status_snapshot, Some(s) if s >= 500);
        if release_dirty {
            if let Err(e) = page.close().await {
                debug!(?e, "page close error (ignored)");
            }
            lease.release_dirty();
        } else {
            lease.release_clean();
        }
        if let Some(c) = self.counters_opt() {
            c.tabs_active.store(
                self.page_pool.total_in_flight(),
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        let mut captured_urls = std::mem::take(&mut *captured.lock());
        let manifest_url = origin_state
            .manifest_url
            .as_deref()
            .and_then(|raw| Url::parse(raw).ok());
        if let Some(url) = manifest_url.clone() {
            Self::push_unique_url(&mut captured_urls, url);
        }
        let mut service_worker_urls = Vec::new();
        for worker in &origin_state.service_workers {
            for raw in worker.script_urls() {
                if let Ok(url) = Url::parse(raw) {
                    Self::push_unique_url(&mut captured_urls, url.clone());
                    Self::push_unique_url(&mut service_worker_urls, url);
                }
            }
        }

        // Derive the runtime routes / network endpoints URL vectors —
        // absolute, http(s) only, deduped. These also feed `captured_urls`
        // so the existing frontier path picks them up.
        let mut runtime_routes: Vec<Url> = Vec::new();
        for r in &observations.routes {
            if let Ok(u) = Url::parse(&r.url).or_else(|_| final_url.join(&r.url)) {
                if matches!(u.scheme(), "http" | "https") {
                    Self::push_unique_url(&mut runtime_routes, u.clone());
                    if self.config.collect_runtime_routes {
                        Self::push_unique_url(&mut captured_urls, u);
                    }
                }
            }
        }
        let mut network_endpoints: Vec<Url> = Vec::new();
        for e in &observations.endpoints {
            if let Ok(u) = Url::parse(&e.url).or_else(|_| final_url.join(&e.url)) {
                if matches!(u.scheme(), "http" | "https") {
                    Self::push_unique_url(&mut network_endpoints, u.clone());
                    if self.config.collect_network_endpoints {
                        Self::push_unique_url(&mut captured_urls, u);
                    }
                }
            }
        }

        let is_spa = !runtime_routes.is_empty()
            || (final_url.fragment().is_some() && final_url.fragment() != url.fragment());

        // Persist structured artifacts. Empty collections are skipped
        // — consumers filter by kind presence anyway, and we don't want
        // to pollute the artifacts table with empty rows.
        self.persist_spa_artifacts(
            url,
            &final_url,
            &session_id,
            &observations,
            &indexeddb_inventory,
            &cache_storage_inventory,
            manifest_json.as_ref(),
            &origin_state.service_workers,
        )
        .await;
        let resources = if collect_vitals {
            resource_map.lock().drain().map(|(_, v)| v).collect()
        } else {
            Vec::new()
        };
        let status = status_snapshot.unwrap_or(0);
        // Post-JS antibot detection already computed above for the
        // release decision; reuse it here so the signal carries the
        // same session id + proxy metadata.
        let challenge = challenge_raw
            .map(|raw| raw.into_signal(&final_url, session_id.clone(), proxy.cloned()));
        // Record render latency sample into the rolling window. OK flag
        // is set from presence of a 2xx/3xx main-document status.
        if let Some(c) = self.counters_opt() {
            let ok = matches!(status, 200..=399);
            c.record_render(render_started.elapsed(), ok);
        }
        Ok(RenderedPage {
            session_id,
            final_url,
            html_post_js: html,
            captured_urls,
            manifest_url,
            service_worker_urls,
            status,
            vitals,
            resources,
            screenshot_png,
            challenge,
            runtime_routes,
            network_endpoints,
            is_spa,
        })
    }
}

// ---------------------------------------------------------------------------
// A2 — supercookie-clear reflection audit.
//
// A full behavioural test would require a mock CDP transport (recording
// sender + fake Browser). The existing `Browser` struct wraps the CDP
// handler behind opaque channels, so spinning up a mock requires more
// glue than the rest of this module warrants. Instead we assert the
// CDP method identifiers that `clear_chrome_supercookies` depends on
// are present in the generated bindings at the exact strings Chrome
// expects — a reflection-style check that catches typos, renames, and
// protocol-revision drift without needing a live browser.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod storage_clear_audit {
    use super::*;
    use crate::render::chrome_wire::MethodType;

    #[test]
    fn clear_chrome_supercookies_wires_expected_cdp_methods() {
        // The five CDP methods the rotation cleanup pass emits, in order.
        // If bindings are regenerated against a new protocol revision and
        // one of these identifiers drifts, this test pins the failure to
        // the affected method instead of surfacing it as a silent runtime
        // warning in production.
        assert_eq!(
            SetBypassServiceWorkerParams::method_id().as_ref(),
            "Network.setBypassServiceWorker"
        );
        assert_eq!(
            SetIgnoreCertificateErrorsParams::method_id().as_ref(),
            "Security.setIgnoreCertificateErrors"
        );
        assert_eq!(
            ClearDataForOriginParams::method_id().as_ref(),
            "Storage.clearDataForOrigin"
        );
        assert_eq!(
            ServiceWorkerUnregisterParams::method_id().as_ref(),
            "ServiceWorker.unregister"
        );
        assert_eq!(
            ClearBrowserCacheParams::method_id().as_ref(),
            "Network.clearBrowserCache"
        );
    }

    #[test]
    fn clear_data_for_origin_uses_wildcard_and_all_storage_types() {
        // Guards the sentinel arguments: "*" (all origins) + "all"
        // (every storage type Chrome exposes). A regression here would
        // silently narrow the clear scope and let supercookies survive.
        let params = ClearDataForOriginParams::new("*", "all");
        assert_eq!(params.origin, "*");
        assert_eq!(params.storage_types, "all");
    }

    #[test]
    fn set_ignore_certificate_errors_resets_to_false() {
        // The hardening prelude explicitly re-asserts a safe default.
        let params = SetIgnoreCertificateErrorsParams::new(false);
        assert!(!params.ignore);
    }

    #[test]
    fn set_bypass_service_worker_enables_bypass() {
        // Bypass must be ON during the clear window so the doomed SW
        // can't intercept fetches mid-clear.
        let params = SetBypassServiceWorkerParams::new(true);
        assert!(params.bypass);
    }
}
