// Ordered header map preserving insertion order and exact casing.
// Chrome emits headers in a specific order (sec-ch-ua cluster, sec-fetch-*, etc).
// `http::HeaderMap` lowercases names, so we keep a parallel IndexMap of original casings.

use indexmap::IndexMap;

#[derive(Debug, Clone, Default)]
pub struct OrderedHeaders {
    inner: IndexMap<String, String>,
}

impl OrderedHeaders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.inner.insert(name.into(), value.into());
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Chrome-observed request kinds. Each kind drives a distinct header set
/// and emission order (Chrome M144 DevTools capture).
///
/// Chrome does NOT emit identical header orders for every request: a
/// top-level document navigation includes `upgrade-insecure-requests` and
/// `sec-fetch-user`; an XHR has neither, uses `sec-fetch-dest: empty`, and
/// commonly sets `origin`; a script/style/image load has its own
/// `accept`/`sec-fetch-dest` pairing. We surface that as an enum so the
/// request builder can pick the right canonical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeRequestKind {
    /// Top-level navigation (`sec-fetch-dest: document`, `mode: navigate`).
    Document,
    /// XMLHttpRequest (`sec-fetch-dest: empty`, `mode: cors`).
    Xhr,
    /// `fetch()` — same on-the-wire shape as XHR but semantically distinct.
    Fetch,
    /// `<script src=...>` (`sec-fetch-dest: script`, `mode: no-cors`).
    Script,
    /// `<link rel="stylesheet">` (`sec-fetch-dest: style`, `mode: no-cors`).
    Style,
    /// `<img>` / image subresource (`sec-fetch-dest: image`).
    Image,
    /// Web font (`sec-fetch-dest: font`, `mode: cors`, `credentials: same-origin`).
    Font,
    /// `navigator.sendBeacon` / ping hyperlinks (`sec-fetch-dest: empty`,
    /// `mode: no-cors`).
    Ping,
}

impl ChromeRequestKind {
    /// Canonical order Chrome uses for this request kind, HTTP/1 or HTTP/2
    /// (pseudo-headers are always prepended by the H2 layer separately).
    ///
    /// Names are lowercase on purpose: HTTP/2 mandates lowercase, and our
    /// HTTP/1 path emits lowercase for consistency with the rest of the
    /// browser-parity plumbing.
    pub fn header_order(self) -> &'static [&'static str] {
        use ChromeRequestKind::*;
        match self {
            // Document: sec-ch-ua cluster → UIR → UA → accept → sec-fetch-*
            // (with -user + -dest: document) → accept-encoding →
            // accept-language → cookie.
            Document => &[
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "upgrade-insecure-requests",
                "user-agent",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-user",
                "sec-fetch-dest",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
            // XHR/fetch: no UIR, no sec-fetch-user. `origin` appears between
            // `accept` and sec-fetch cluster; `content-type` only on POST
            // bodies. `sec-fetch-dest: empty`.
            Xhr | Fetch => &[
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "accept",
                "content-type",
                "origin",
                "user-agent",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
            // Script: sec-fetch-dest: script, mode: no-cors, accept: */*.
            Script => &[
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "user-agent",
                "sec-ch-ua-platform",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
            // Style: sec-fetch-dest: style, accept: text/css,*/*;q=0.1.
            Style => &[
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "user-agent",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
            // Image: sec-fetch-dest: image, accept: image/*.
            Image => &[
                "sec-ch-ua",
                "user-agent",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
            // Font: Chrome ALWAYS sends font fetches CORS (mode: cors) with
            // an `origin` header, even for same-origin.
            Font => &[
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "user-agent",
                "accept",
                "origin",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
            // Ping (sendBeacon / <a ping>): minimal, POST text/ping, no-cors.
            Ping => &[
                "content-type",
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "user-agent",
                "accept",
                "ping-from",
                "ping-to",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "accept-encoding",
                "accept-language",
                "cookie",
            ],
        }
    }

    /// Default Chrome `Accept` header for this request kind.
    pub fn default_accept(self) -> &'static str {
        use ChromeRequestKind::*;
        match self {
            Document => {
                "text/html,application/xhtml+xml,application/xml;q=0.9,\
                 image/avif,image/webp,image/apng,*/*;q=0.8,\
                 application/signed-exchange;v=b3;q=0.7"
            }
            Xhr | Fetch | Ping => "*/*",
            Script => "*/*",
            Style => "text/css,*/*;q=0.1",
            Image => "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
            Font => "*/*",
        }
    }

    /// Value for `Sec-Fetch-Dest`.
    pub fn sec_fetch_dest(self) -> &'static str {
        use ChromeRequestKind::*;
        match self {
            Document => "document",
            Xhr | Fetch | Ping => "empty",
            Script => "script",
            Style => "style",
            Image => "image",
            Font => "font",
        }
    }

    /// Value for `Sec-Fetch-Mode`.
    pub fn sec_fetch_mode(self) -> &'static str {
        use ChromeRequestKind::*;
        match self {
            Document => "navigate",
            Xhr | Fetch | Font => "cors",
            Script | Style | Image | Ping => "no-cors",
        }
    }

    /// Whether this kind includes `sec-fetch-user: ?1` (only top-level nav).
    pub fn includes_sec_fetch_user(self) -> bool {
        matches!(self, ChromeRequestKind::Document)
    }

    /// Whether this kind includes `upgrade-insecure-requests: 1`.
    pub fn includes_upgrade_insecure_requests(self) -> bool {
        matches!(self, ChromeRequestKind::Document)
    }
}

impl From<crate::discovery::assets::SecFetchDest> for ChromeRequestKind {
    fn from(dest: crate::discovery::assets::SecFetchDest) -> Self {
        use crate::discovery::assets::SecFetchDest as D;
        match dest {
            D::Document => ChromeRequestKind::Document,
            D::Empty => ChromeRequestKind::Xhr,
            D::Image => ChromeRequestKind::Image,
            D::Script => ChromeRequestKind::Script,
            D::Style => ChromeRequestKind::Style,
            D::Font => ChromeRequestKind::Font,
            // No dedicated kinds for audio/video; they look like fetches.
            D::Audio | D::Video => ChromeRequestKind::Fetch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_order_starts_with_sec_ch_ua_cluster() {
        let order = ChromeRequestKind::Document.header_order();
        assert_eq!(order[0], "sec-ch-ua");
        assert_eq!(order[1], "sec-ch-ua-mobile");
        assert_eq!(order[2], "sec-ch-ua-platform");
    }

    #[test]
    fn document_includes_upgrade_insecure_and_fetch_user() {
        let order = ChromeRequestKind::Document.header_order();
        assert!(order.contains(&"upgrade-insecure-requests"));
        assert!(order.contains(&"sec-fetch-user"));
        assert!(ChromeRequestKind::Document.includes_sec_fetch_user());
        assert!(ChromeRequestKind::Document.includes_upgrade_insecure_requests());
    }

    #[test]
    fn xhr_excludes_upgrade_insecure_and_sec_fetch_user() {
        let order = ChromeRequestKind::Xhr.header_order();
        assert!(!order.contains(&"upgrade-insecure-requests"));
        assert!(!order.contains(&"sec-fetch-user"));
        assert!(!ChromeRequestKind::Xhr.includes_sec_fetch_user());
        assert!(!ChromeRequestKind::Xhr.includes_upgrade_insecure_requests());
    }

    #[test]
    fn xhr_and_fetch_share_same_order() {
        assert_eq!(
            ChromeRequestKind::Xhr.header_order(),
            ChromeRequestKind::Fetch.header_order()
        );
    }

    #[test]
    fn all_kinds_end_with_cookie_last() {
        for kind in [
            ChromeRequestKind::Document,
            ChromeRequestKind::Xhr,
            ChromeRequestKind::Fetch,
            ChromeRequestKind::Script,
            ChromeRequestKind::Style,
            ChromeRequestKind::Image,
            ChromeRequestKind::Font,
            ChromeRequestKind::Ping,
        ] {
            let order = kind.header_order();
            assert_eq!(
                order.last().copied(),
                Some("cookie"),
                "{kind:?} must end with cookie"
            );
        }
    }

    #[test]
    fn sec_fetch_mode_is_navigate_for_document() {
        assert_eq!(ChromeRequestKind::Document.sec_fetch_mode(), "navigate");
        assert_eq!(ChromeRequestKind::Xhr.sec_fetch_mode(), "cors");
        assert_eq!(ChromeRequestKind::Fetch.sec_fetch_mode(), "cors");
        assert_eq!(ChromeRequestKind::Script.sec_fetch_mode(), "no-cors");
        assert_eq!(ChromeRequestKind::Style.sec_fetch_mode(), "no-cors");
        assert_eq!(ChromeRequestKind::Image.sec_fetch_mode(), "no-cors");
        assert_eq!(ChromeRequestKind::Font.sec_fetch_mode(), "cors");
    }

    #[test]
    fn sec_fetch_dest_values_match_chrome() {
        assert_eq!(ChromeRequestKind::Document.sec_fetch_dest(), "document");
        assert_eq!(ChromeRequestKind::Xhr.sec_fetch_dest(), "empty");
        assert_eq!(ChromeRequestKind::Fetch.sec_fetch_dest(), "empty");
        assert_eq!(ChromeRequestKind::Script.sec_fetch_dest(), "script");
        assert_eq!(ChromeRequestKind::Style.sec_fetch_dest(), "style");
        assert_eq!(ChromeRequestKind::Image.sec_fetch_dest(), "image");
        assert_eq!(ChromeRequestKind::Font.sec_fetch_dest(), "font");
        assert_eq!(ChromeRequestKind::Ping.sec_fetch_dest(), "empty");
    }

    #[test]
    fn default_accept_for_document_matches_chrome() {
        let accept = ChromeRequestKind::Document.default_accept();
        assert!(accept.starts_with("text/html,application/xhtml+xml"));
        assert!(accept.contains("image/avif"));
        assert!(accept.contains("application/signed-exchange"));
    }

    #[test]
    fn default_accept_style_script_image() {
        assert_eq!(
            ChromeRequestKind::Style.default_accept(),
            "text/css,*/*;q=0.1"
        );
        assert_eq!(ChromeRequestKind::Script.default_accept(), "*/*");
        assert!(ChromeRequestKind::Image
            .default_accept()
            .contains("image/avif"));
    }

    #[test]
    fn from_sec_fetch_dest_round_trip() {
        use crate::discovery::assets::SecFetchDest;
        assert_eq!(
            ChromeRequestKind::from(SecFetchDest::Document),
            ChromeRequestKind::Document
        );
        assert_eq!(
            ChromeRequestKind::from(SecFetchDest::Empty),
            ChromeRequestKind::Xhr
        );
        assert_eq!(
            ChromeRequestKind::from(SecFetchDest::Image),
            ChromeRequestKind::Image
        );
        assert_eq!(
            ChromeRequestKind::from(SecFetchDest::Script),
            ChromeRequestKind::Script
        );
        assert_eq!(
            ChromeRequestKind::from(SecFetchDest::Style),
            ChromeRequestKind::Style
        );
        assert_eq!(
            ChromeRequestKind::from(SecFetchDest::Font),
            ChromeRequestKind::Font
        );
    }
}
