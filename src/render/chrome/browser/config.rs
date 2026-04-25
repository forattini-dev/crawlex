use std::time::Duration;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use super::argument::{Arg, ArgConst, ArgsBuilder};
use crate::render::chrome::async_process::{self, Child, Stdio};
use crate::render::chrome::detection::{self, DetectionOptions};
use crate::render::chrome::handler::viewport::Viewport;
use crate::render::chrome::handler::REQUEST_TIMEOUT;

/// Default `Browser::launch` timeout in MS
pub const LAUNCH_TIMEOUT: u64 = 20_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HeadlessMode {
    /// The "headful" mode.
    False,
    /// The old headless mode.
    #[default]
    True,
    /// The new headless mode. See also: https://developer.chrome.com/docs/chromium/new-headless
    New,
}

#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// Determines whether to run headless version of the browser. Defaults to
    /// true.
    pub(crate) headless: HeadlessMode,

    /// Determines whether to run the browser with a sandbox.
    pub(crate) sandbox: bool,

    /// Launch the browser with a specific window width and height.
    pub(crate) window_size: Option<(u32, u32)>,

    /// Launch the browser with a specific debugging port.
    pub(crate) port: u16,

    /// Path for Chrome or Chromium.
    ///
    /// If unspecified, the create will try to automatically detect a suitable
    /// binary.
    pub(crate) executable: std::path::PathBuf,

    /// A list of Chrome extensions to load.
    ///
    /// An extension should be a path to a folder containing the extension code.
    /// CRX files cannot be used directly and must be first extracted.
    ///
    /// Note that Chrome does not support loading extensions in headless-mode.
    /// See https://bugs.chromium.org/p/chromium/issues/detail?id=706008#c5
    pub(crate) extensions: Vec<String>,

    /// Environment variables to set for the Chromium process.
    /// Passes value through to std::process::Command::envs.
    pub process_envs: Option<HashMap<String, String>>,

    /// Data dir for user data
    pub user_data_dir: Option<PathBuf>,

    /// Whether to launch the `Browser` in incognito mode
    pub(crate) incognito: bool,

    /// Timeout duration for `Browser::launch`.
    pub(crate) launch_timeout: Duration,

    /// Ignore https errors, default is true
    pub(crate) ignore_https_errors: bool,

    /// Ignore invalid messages, default is true
    pub(crate) ignore_invalid_messages: bool,

    /// Disable HTTPS-first features (HttpsUpgrades, HttpsFirstBalancedModeAutoEnable)
    pub(crate) disable_https_first: bool,

    /// The viewport of the browser
    pub(crate) viewport: Option<Viewport>,

    /// The duration after a request with no response should time out
    pub(crate) request_timeout: Duration,

    /// Additional command line arguments to pass to the browser instance.
    pub(crate) args: Vec<Arg>,

    /// Whether to disable DEFAULT_ARGS or not, default is false
    pub(crate) disable_default_args: bool,

    /// Whether to enable request interception
    pub request_intercept: bool,

    /// Whether to enable cache
    pub cache_enabled: bool,

    /// Avoid easy bot detection by setting `navigator.webdriver` to false
    pub(crate) hidden: bool,

    /// Stealth knobs — grouped so future additions
    /// (e.g. isolated-world name, Error.prepareStackTrace scrubbing,
    /// page init-script clean-up) land in one place instead of four.
    pub(crate) stealth: StealthConfig,
}

/// Options that make crawlex presence harder to detect at the CDP layer.
/// Grouped as a struct (rather than individual fields on each config) so
/// new knobs can be added without threading a new bool through
/// BrowserConfig / BrowserConfigBuilder / HandlerConfig / TargetConfig
/// every time.
#[derive(Debug, Clone, Default)]
pub struct StealthConfig {
    /// Skip `Runtime.enable` on new targets. When true, new pages come
    /// up with Page + lifecycle CDP domains enabled but the JS runtime
    /// domain stays off — this defeats brotector-style probes that
    /// sniff CDP attachment via `Error.prepareStackTrace` and the
    /// stack-trace signatures `Runtime.enable` injects. Costs:
    /// `page.evaluate()` now runs in the isolated world, so main-world
    /// JS globals are no longer shared between calls (DOM still is).
    /// See `BrowserConfigBuilder::stealth_runtime_enable_skip` for the
    /// full contract.
    ///
    /// Reference: <https://rebrowser.net/blog/how-to-fix-runtime-enable-cdp-detection-of-puppeteer-playwright-and-other-automation-libraries-61740>
    pub runtime_enable_skip: bool,

    /// Worker-scope variant of the stealth shim. When set, the handler
    /// injects this source via `Runtime.evaluate` on every
    /// `Target.attachedToTarget` event whose target type is `worker`,
    /// `shared_worker`, or `service_worker` (Camoufox port Sprint 3 S3.1).
    /// Detectors increasingly move fingerprint collection off-thread; a
    /// shim that only runs in the main frame leaves `navigator.userAgent`
    /// + `AudioContext.sampleRate` + `WebGL` consistent on the page but
    /// inconsistent inside `new Worker(...)`, which is itself a bot tell.
    ///
    /// `Arc<String>` so the source is shared across all workers without
    /// cloning the ~1700-line shim per attach event.
    pub worker_init_script: Option<std::sync::Arc<String>>,
}

#[derive(Debug, Clone)]
pub struct BrowserConfigBuilder {
    headless: HeadlessMode,
    sandbox: bool,
    window_size: Option<(u32, u32)>,
    port: u16,
    executable: Option<PathBuf>,
    executation_detection: DetectionOptions,
    extensions: Vec<String>,
    process_envs: Option<HashMap<String, String>>,
    user_data_dir: Option<PathBuf>,
    incognito: bool,
    launch_timeout: Duration,
    ignore_https_errors: bool,
    ignore_invalid_events: bool,
    disable_https_first: bool,
    viewport: Option<Viewport>,
    request_timeout: Duration,
    args: Vec<Arg>,
    disable_default_args: bool,
    request_intercept: bool,
    cache_enabled: bool,
    hidden: bool,
    stealth: StealthConfig,
}

impl BrowserConfig {
    pub fn builder() -> BrowserConfigBuilder {
        BrowserConfigBuilder::default()
    }

    pub fn with_executable(path: impl AsRef<Path>) -> Self {
        Self::builder().chrome_executable(path).build().unwrap()
    }

    /// Whether `Runtime.enable` is suppressed on new targets. See
    /// [`BrowserConfigBuilder::stealth_runtime_enable_skip`] for the
    /// full tradeoff.
    pub fn stealth_runtime_enable_skip(&self) -> bool {
        self.stealth.runtime_enable_skip
    }

    /// Accessor for the full stealth block — used by downstream layers
    /// (Handler, Target) that need more than one knob.
    pub(crate) fn stealth(&self) -> &StealthConfig {
        &self.stealth
    }
}

impl Default for BrowserConfigBuilder {
    fn default() -> Self {
        Self {
            headless: HeadlessMode::True,
            sandbox: true,
            window_size: None,
            port: 0,
            executable: None,
            executation_detection: DetectionOptions::default(),
            extensions: Vec::new(),
            process_envs: None,
            user_data_dir: None,
            incognito: false,
            launch_timeout: Duration::from_millis(LAUNCH_TIMEOUT),
            ignore_https_errors: true,
            ignore_invalid_events: true,
            disable_https_first: false,
            viewport: Some(Default::default()),
            request_timeout: Duration::from_millis(REQUEST_TIMEOUT),
            args: Vec::new(),
            disable_default_args: false,
            request_intercept: false,
            cache_enabled: true,
            hidden: false,
            stealth: StealthConfig::default(),
        }
    }
}

impl BrowserConfigBuilder {
    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    pub fn no_sandbox(mut self) -> Self {
        self.sandbox = false;
        self
    }

    pub fn with_head(mut self) -> Self {
        self.headless = HeadlessMode::False;
        self
    }

    pub fn new_headless_mode(mut self) -> Self {
        self.headless = HeadlessMode::New;
        self
    }

    pub fn headless_mode(mut self, mode: HeadlessMode) -> Self {
        self.headless = mode;
        self
    }

    pub fn incognito(mut self) -> Self {
        self.incognito = true;
        self
    }

    pub fn respect_https_errors(mut self) -> Self {
        self.ignore_https_errors = false;
        self
    }

    /// The browser handler will return [CdpError::InvalidMessage] if a received
    /// message cannot be parsed.
    pub fn surface_invalid_messages(mut self) -> Self {
        self.ignore_invalid_events = false;
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn launch_timeout(mut self, timeout: Duration) -> Self {
        self.launch_timeout = timeout;
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Configures the viewport of the browser, which defaults to `800x600`.
    /// `None` disables viewport emulation (i.e., it uses the browsers default
    /// configuration, which fills the available space. This is similar to what
    /// Playwright does when you provide `null` as the value of its `viewport`
    /// option).
    pub fn viewport(mut self, viewport: impl Into<Option<Viewport>>) -> Self {
        self.viewport = viewport.into();
        self
    }

    pub fn user_data_dir(mut self, data_dir: impl AsRef<Path>) -> Self {
        self.user_data_dir = Some(data_dir.as_ref().to_path_buf());
        self
    }

    pub fn chrome_executable(mut self, path: impl AsRef<Path>) -> Self {
        self.executable = Some(path.as_ref().to_path_buf());
        self
    }

    /// Opt-in to brotector/rebrowser-style stealth: the per-target init
    /// chain drops `Runtime.enable` so fingerprinters cannot detect CDP
    /// attachment via stack-trace signatures. Default false.
    ///
    /// # Semantics change for `page.evaluate`
    ///
    /// When this flag is on, `page.evaluate()` runs in the **isolated
    /// world** (utility world), not the main world. Isolated worlds share
    /// the DOM with main but have **separate JavaScript globals**:
    ///
    /// ```ignore
    /// // WORKS in default mode, FAILS silently under stealth:
    /// page.evaluate("window.__cached = 1").await?;
    /// let v: i32 = page.evaluate("window.__cached").await?.into_value()?;
    /// //           ^ returns undefined — the first assignment wrote to
    /// //             the isolated world's own `window`, which isn't
    /// //             visible here.
    /// ```
    ///
    /// DOM access and mutation work normally
    /// (`document.querySelector(...)`, event dispatch, cookies, URLs).
    /// If you need to stash state between calls, use your own Rust-side
    /// storage or attach it to a DOM node.
    ///
    /// The execution-context resolution path is
    /// `Frame.secondary_world.execution_context`, populated by
    /// `Page.createIsolatedWorld`'s response (see
    /// `FrameManager::on_create_isolated_world_response`).
    pub fn stealth_runtime_enable_skip(mut self, skip: bool) -> Self {
        self.stealth.runtime_enable_skip = skip;
        self
    }

    /// Install the worker-scope stealth shim (Camoufox port S3.1). The
    /// rendered source is forwarded to every `worker`/`shared_worker`/
    /// `service_worker` target the page spawns, before the worker exits
    /// the `waitForDebuggerOnStart` pause — so persona coherence is
    /// established before any user script in the worker runs.
    pub fn stealth_worker_shim(mut self, src: impl Into<String>) -> Self {
        self.stealth.worker_init_script = Some(std::sync::Arc::new(src.into()));
        self
    }

    /// Replace the full stealth block at once — convenient when loading
    /// a serialized config; for individual knobs prefer the dedicated
    /// setters so callers don't rely on unspecified field defaults.
    pub fn stealth(mut self, stealth: StealthConfig) -> Self {
        self.stealth = stealth;
        self
    }

    pub fn chrome_detection(mut self, options: DetectionOptions) -> Self {
        self.executation_detection = options;
        self
    }

    pub fn extension(mut self, extension: impl Into<String>) -> Self {
        self.extensions.push(extension.into());
        self
    }

    pub fn extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for ext in extensions {
            self.extensions.push(ext.into());
        }
        self
    }

    pub fn env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.process_envs
            .get_or_insert(HashMap::new())
            .insert(key.into(), val.into());
        self
    }

    pub fn envs<I, K, V>(mut self, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.process_envs
            .get_or_insert(HashMap::new())
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn arg(mut self, arg: impl Into<Arg>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Arg>,
    {
        for arg in args {
            self.args.push(arg.into());
        }
        self
    }

    pub fn disable_default_args(mut self) -> Self {
        self.disable_default_args = true;
        self
    }

    pub fn disable_https_first(mut self) -> Self {
        self.disable_https_first = true;
        self
    }

    pub fn enable_request_intercept(mut self) -> Self {
        self.request_intercept = true;
        self
    }

    pub fn disable_request_intercept(mut self) -> Self {
        self.request_intercept = false;
        self
    }

    pub fn enable_cache(mut self) -> Self {
        self.cache_enabled = true;
        self
    }

    pub fn disable_cache(mut self) -> Self {
        self.cache_enabled = false;
        self
    }

    pub fn hide(mut self) -> Self {
        self.hidden = true;
        if self.hidden && self.viewport == Some(Viewport::default()) {
            self.viewport = None;
        }
        self
    }

    pub fn build(self) -> std::result::Result<BrowserConfig, String> {
        let executable = if let Some(e) = self.executable {
            e
        } else {
            detection::default_executable(self.executation_detection)?
        };

        Ok(BrowserConfig {
            headless: self.headless,
            sandbox: self.sandbox,
            window_size: self.window_size,
            port: self.port,
            executable,
            extensions: self.extensions,
            process_envs: self.process_envs,
            user_data_dir: self.user_data_dir,
            incognito: self.incognito,
            launch_timeout: self.launch_timeout,
            ignore_https_errors: self.ignore_https_errors,
            ignore_invalid_messages: self.ignore_invalid_events,
            disable_https_first: self.disable_https_first,
            viewport: self.viewport,
            request_timeout: self.request_timeout,
            args: self.args,
            disable_default_args: self.disable_default_args,
            request_intercept: self.request_intercept,
            cache_enabled: self.cache_enabled,
            hidden: self.hidden,
            stealth: self.stealth,
        })
    }
}

impl BrowserConfig {
    pub fn launch(&self) -> io::Result<Child> {
        let mut builder = ArgsBuilder::new();

        if self.disable_default_args {
            builder.args(self.args.clone());
        } else {
            builder.args(DEFAULT_ARGS.clone()).args(self.args.clone());
        }

        if !builder.has("remote-debugging-port") {
            builder.arg(Arg::value("remote-debugging-port", self.port));
        }

        if self.extensions.is_empty() {
            builder.arg(Arg::key("disable-extensions"));
        } else {
            builder.args(
                self.extensions
                    .iter()
                    .map(|e| Arg::value("load-extension", e)),
            );
        }

        if let Some(ref user_data) = self.user_data_dir {
            builder.arg(Arg::value("user-data-dir", user_data.display()));
        } else {
            // If the user did not specify a data directory, this would default to the systems default
            // data directory. In most cases, we would rather have a fresh instance of Chromium. Specify
            // a temp dir just for the CDP client instead.
            builder.arg(Arg::value(
                "user-data-dir",
                std::env::temp_dir().join("crawlex-runner").display(),
            ));
        }

        if let Some((width, height)) = self.window_size {
            builder.arg(Arg::values("window-size", [width, height]));
        }

        if !self.sandbox {
            builder.args([Arg::key("no-sandbox"), Arg::key("disable-setuid-sandbox")]);
        }

        match self.headless {
            HeadlessMode::False => (),
            HeadlessMode::True => {
                builder.args([
                    Arg::key("headless"),
                    Arg::key("hide-scrollbars"),
                    Arg::key("mute-audio"),
                ]);
            }
            HeadlessMode::New => {
                builder.args([
                    Arg::value("headless", "new"),
                    Arg::key("hide-scrollbars"),
                    Arg::key("mute-audio"),
                ]);
            }
        }

        if self.incognito {
            builder.arg(Arg::key("incognito"));
        }

        // `AutomationControlled` is the blink feature that wires up
        // `navigator.webdriver = true` and the `document.$cdc_*` handles
        // fingerprinters scan for. Hide it whenever ANY stealth signal is
        // active (`.hide()` or stealth runtime knobs). Previously this
        // only fired on explicit `.hide()`, so a caller going through
        // `Browser::launch` with only `stealth_runtime_enable_skip(true)`
        // still leaked `navigator.webdriver` — the "real coverage gap"
        // flagged by the stealth_runtime_live probe.
        if self.hidden || self.stealth.runtime_enable_skip {
            builder.arg(Arg::value("disable-blink-features", "AutomationControlled"));
        }

        if self.disable_https_first {
            builder.arg(Arg::values(
                "disable-features",
                ["HttpsUpgrades", "HttpsFirstBalancedModeAutoEnable"],
            ));
        }

        let mut cmd = async_process::Command::new(&self.executable);

        let args = builder.into_iter().collect::<Vec<String>>();
        cmd.args(args);

        if let Some(ref envs) = self.process_envs {
            cmd.envs(envs);
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
    }
}

/// These are passed to the Chrome binary by default.
/// Via https://github.com/puppeteer/puppeteer/blob/4846b8723cf20d3551c0d755df394cc5e0c82a94/src/node/Launcher.ts#L157
static DEFAULT_ARGS: [ArgConst; 24] = [
    ArgConst::key("disable-background-networking"),
    ArgConst::values(
        "enable-features",
        &["NetworkService", "NetworkServiceInProcess"],
    ),
    ArgConst::key("disable-background-timer-throttling"),
    ArgConst::key("disable-backgrounding-occluded-windows"),
    ArgConst::key("disable-breakpad"),
    ArgConst::key("disable-client-side-phishing-detection"),
    ArgConst::key("disable-component-extensions-with-background-pages"),
    ArgConst::key("disable-default-apps"),
    ArgConst::key("disable-dev-shm-usage"),
    ArgConst::values("disable-features", &["TranslateUI"]),
    ArgConst::key("disable-hang-monitor"),
    ArgConst::key("disable-ipc-flooding-protection"),
    ArgConst::key("disable-popup-blocking"),
    ArgConst::key("disable-prompt-on-repost"),
    ArgConst::key("disable-renderer-backgrounding"),
    ArgConst::key("disable-sync"),
    ArgConst::values("force-color-profile", &["srgb"]),
    ArgConst::key("metrics-recording-only"),
    ArgConst::key("no-first-run"),
    ArgConst::key("enable-automation"),
    ArgConst::values("password-store", &["basic"]),
    ArgConst::key("use-mock-keychain"),
    ArgConst::values("enable-blink-features", &["IdleDetection"]),
    ArgConst::values("lang", &["en_US"]),
];
