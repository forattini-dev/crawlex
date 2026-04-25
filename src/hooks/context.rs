use bytes::Bytes;
use http::HeaderMap;
use std::collections::HashMap;
use url::Url;

use crate::impersonate::headers::OrderedHeaders;

pub struct HookContext {
    pub url: Url,
    pub depth: u32,
    pub request_headers: Option<OrderedHeaders>,
    pub response_status: Option<u16>,
    pub response_headers: Option<HeaderMap>,
    pub body: Option<Bytes>,
    pub html_post_js: Option<String>,
    pub captured_urls: Vec<Url>,
    pub proxy: Option<Url>,
    pub retry_count: u32,
    pub allow_retry: bool,
    pub robots_allowed: Option<bool>,
    pub user_data: HashMap<String, serde_json::Value>,
    pub error: Option<String>,
}

impl HookContext {
    pub fn new(url: Url, depth: u32) -> Self {
        Self {
            url,
            depth,
            request_headers: None,
            response_status: None,
            response_headers: None,
            body: None,
            html_post_js: None,
            captured_urls: Vec::new(),
            proxy: None,
            retry_count: 0,
            allow_retry: true,
            robots_allowed: None,
            user_data: HashMap::new(),
            error: None,
        }
    }
}
