//! High-level `BrowserSession` facade.
//!
//! `BrowserSession` is the deepened seam that hides chromiumoxide /
//! CDP-protocol types from the rest of the crate. Callers in motion /
//! stealth / hooks talk to a small high-level API (`goto`, `eval`,
//! `screenshot`, `inject_script`, `html`, `cookies`) instead of importing
//! `chrome::Page`, `chrome::Frame`, `Target`, et al. directly.
//!
//! Why a facade:
//! * **CDP drift containment** — when Chrome 150 renames a CDP method or
//!   reshapes a parameter, the change lands here once instead of in 30
//!   call sites.
//! * **Testability** — `BrowserSessionLike` trait lets motion/stealth
//!   tests run against a `MockBrowserSession` without launching real
//!   Chrome.
//! * **AI navigability** — a reader of `motion::idle_drift` doesn't need
//!   to know the difference between `Page.evaluate` and `Runtime.evaluate`
//!   to follow the code.
//!
//! ## Status
//!
//! This commit lands the facade + the `BrowserSessionLike` trait. Existing
//! callers (pool.rs, motion, stealth.rs, hooks) continue to talk to
//! `chrome::Page` directly; migration is incremental and tracked per
//! consumer in subsequent commits.

#![cfg(feature = "cdp-backend")]

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use url::Url;

use crate::error::Result;
use crate::render::chrome::page::Page;

/// What a screenshot capture should cover. Kept as a small enum so the
/// caller doesn't reach into CDP `CaptureScreenshotParams` to set
/// `clip` / `from_surface` / `capture_beyond_viewport` themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotMode {
    /// Just the visible viewport rectangle.
    Viewport,
    /// The full scrollable page (may be much larger than the viewport).
    FullPage,
    /// Bounding rect of a single element matching a CSS selector. The
    /// selector lives outside the enum because including it would force
    /// `Copy` off; pass via `screenshot_element(selector)` instead.
    Element,
}

/// High-level operations every page-driving consumer needs. Implemented by
/// [`BrowserSession`] for real CDP traffic and by mock structs in tests.
#[async_trait]
pub trait BrowserSessionLike: Send + Sync {
    /// Navigate the page to `url`. Resolves once the navigation has
    /// committed (frame received its first response) — does NOT wait for
    /// the lifecycle event the caller cares about; pair with `wait_until`.
    async fn goto(&self, url: &Url) -> Result<()>;

    /// Evaluate a JS expression in the active execution context and return
    /// the result as JSON. Caller is responsible for `serde_json::from_value`
    /// on the typed shape they expect.
    async fn eval_json(&self, expr: &str) -> Result<serde_json::Value>;

    /// Capture a screenshot. `selector` is meaningful only when
    /// `mode == ScreenshotMode::Element`; ignored otherwise.
    async fn screenshot(
        &self,
        mode: ScreenshotMode,
        selector: Option<&str>,
    ) -> Result<Bytes>;

    /// Inject a JS source to run before any user script in every new
    /// document the page (or a child frame) loads. Used by the stealth
    /// shim and the SPA observer.
    async fn inject_script(&self, src: &str) -> Result<()>;

    /// Current URL of the main frame, or `None` if the page hasn't
    /// committed a navigation yet.
    async fn url(&self) -> Result<Option<String>>;

    /// Serialised HTML of the main frame's document
    /// (post-JS, after wait strategy has resolved).
    async fn html(&self) -> Result<String>;
}

/// Real implementation backed by `chrome::Page`. The wrapper is `Clone`
/// because every method takes `&self` and `Arc<Page>` is cheap to clone.
#[derive(Clone)]
pub struct BrowserSession {
    page: Arc<Page>,
}

impl BrowserSession {
    /// Wrap an already-launched `Page`. Callers continue to acquire pages
    /// from the existing `RenderPool`; this is purely a facade adapter.
    pub fn new(page: Arc<Page>) -> Self {
        Self { page }
    }

    /// Escape hatch: hand out the underlying `Arc<Page>` for callers that
    /// genuinely need a CDP-level operation we haven't surfaced yet.
    /// New code should not reach for this — flag the missing method
    /// instead so we can grow `BrowserSessionLike` deliberately.
    pub fn raw_page(&self) -> Arc<Page> {
        self.page.clone()
    }
}

#[async_trait]
impl BrowserSessionLike for BrowserSession {
    async fn goto(&self, url: &Url) -> Result<()> {
        use crate::render::chrome_protocol::cdp::browser_protocol::page::NavigateParams;
        let params = NavigateParams::builder()
            .url(url.to_string())
            .build()
            .map_err(|e| crate::Error::Render(format!("NavigateParams: {e}")))?;
        self.page
            .execute(params)
            .await
            .map_err(|e| crate::Error::Render(format!("navigate: {e}")))?;
        Ok(())
    }

    async fn eval_json(&self, expr: &str) -> Result<serde_json::Value> {
        let v = self
            .page
            .evaluate(expr)
            .await
            .map_err(|e| crate::Error::Render(format!("evaluate: {e}")))?;
        Ok(v.value().cloned().unwrap_or(serde_json::Value::Null))
    }

    async fn screenshot(
        &self,
        mode: ScreenshotMode,
        _selector: Option<&str>,
    ) -> Result<Bytes> {
        use crate::render::chrome_protocol::cdp::browser_protocol::page::CaptureScreenshotParams;
        let mut params = CaptureScreenshotParams::default();
        if matches!(mode, ScreenshotMode::FullPage) {
            params.capture_beyond_viewport = Some(true);
        }
        let resp = self
            .page
            .execute(params)
            .await
            .map_err(|e| crate::Error::Render(format!("screenshot: {e}")))?;
        // CDP returns base64; our types crate stamps it as Vec<u8> already.
        let data = resp.result.data.clone();
        let decoded = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            data.as_bytes(),
        )
        .map_err(|e| crate::Error::Render(format!("screenshot decode: {e}")))?;
        Ok(Bytes::from(decoded))
    }

    async fn inject_script(&self, src: &str) -> Result<()> {
        use crate::render::chrome_protocol::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
        let params = AddScriptToEvaluateOnNewDocumentParams {
            source: src.to_string(),
            world_name: None,
            include_command_line_api: Some(false),
            run_immediately: Some(true),
        };
        self.page
            .execute(params)
            .await
            .map_err(|e| crate::Error::Render(format!("inject_script: {e}")))?;
        Ok(())
    }

    async fn url(&self) -> Result<Option<String>> {
        self.page
            .url()
            .await
            .map_err(|e| crate::Error::Render(format!("url: {e}")))
    }

    async fn html(&self) -> Result<String> {
        // `document.documentElement.outerHTML` is the canonical post-JS
        // snapshot. Callers wanting the unmodified pre-JS HTML should
        // use `ArtifactStorage::save_raw` from a fetch path instead.
        let v = self
            .page
            .evaluate("document.documentElement.outerHTML")
            .await
            .map_err(|e| crate::Error::Render(format!("html: {e}")))?;
        Ok(v.value()
            .and_then(|x| x.as_str())
            .map(String::from)
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dummy implementation used by motion/stealth tests that don't want
    /// to launch real Chrome. Records every method call so the test can
    /// assert which API the consumer touched.
    pub struct MockBrowserSession {
        pub calls: parking_lot::Mutex<Vec<String>>,
    }

    impl MockBrowserSession {
        pub fn new() -> Self {
            Self {
                calls: parking_lot::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl BrowserSessionLike for MockBrowserSession {
        async fn goto(&self, url: &Url) -> Result<()> {
            self.calls.lock().push(format!("goto({url})"));
            Ok(())
        }
        async fn eval_json(&self, expr: &str) -> Result<serde_json::Value> {
            self.calls.lock().push(format!("eval({expr})"));
            Ok(serde_json::Value::Null)
        }
        async fn screenshot(
            &self,
            mode: ScreenshotMode,
            selector: Option<&str>,
        ) -> Result<Bytes> {
            self.calls
                .lock()
                .push(format!("screenshot({:?}, {:?})", mode, selector));
            Ok(Bytes::new())
        }
        async fn inject_script(&self, src: &str) -> Result<()> {
            self.calls.lock().push(format!("inject_script({})", src.len()));
            Ok(())
        }
        async fn url(&self) -> Result<Option<String>> {
            self.calls.lock().push("url".into());
            Ok(None)
        }
        async fn html(&self) -> Result<String> {
            self.calls.lock().push("html".into());
            Ok(String::new())
        }
    }

    #[tokio::test]
    async fn mock_records_calls() {
        let mock = MockBrowserSession::new();
        let url = Url::parse("https://example.com").unwrap();
        mock.goto(&url).await.unwrap();
        let _ = mock.eval_json("1+1").await.unwrap();
        mock.inject_script("/* shim */").await.unwrap();
        let calls = mock.calls.lock();
        assert_eq!(calls.len(), 3);
        assert!(calls[0].starts_with("goto"));
        assert!(calls[1].starts_with("eval"));
        assert!(calls[2].starts_with("inject_script"));
    }
}
