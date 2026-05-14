//! Recipe-facing request descriptor.
//!
//! Kept intentionally minimal: a URL, an optional method override, and
//! the new `session_id` field that slice 16 plumbs end-to-end. Header /
//! body / timeout shaping land in later slices alongside the engine
//! dispatcher.

/// HTTP-ish request descriptor. The framework executes a request on the
/// backend named by the resolved session id (see
/// [`crate::scraping::SessionManager`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub method: String,
    /// Optional session id. `None` means "use the default backend".
    /// Unknown ids also resolve to the default backend with a warning
    /// emitted at routing time.
    pub session_id: Option<String>,
}

impl Request {
    /// Build a GET request with no session pinning.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "GET".to_string(),
            session_id: None,
        }
    }

    /// Pin this request to a session id. Chainable.
    pub fn with_session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_get_no_session() {
        let r = Request::new("https://example.com");
        assert_eq!(r.url, "https://example.com");
        assert_eq!(r.method, "GET");
        assert!(r.session_id.is_none());
    }

    #[test]
    fn with_session_sets_id() {
        let r = Request::new("https://example.com").with_session("s1");
        assert_eq!(r.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn with_method_overrides() {
        let r = Request::new("https://example.com").with_method("POST");
        assert_eq!(r.method, "POST");
    }
}
