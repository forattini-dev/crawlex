use bytes::Bytes;
use dashmap::DashMap;
use http::HeaderMap;
use url::Url;

use crate::storage::{PageMetadata, Storage};
use crate::Result;

#[derive(Default)]
pub struct MemoryStorage {
    pub raw: DashMap<String, Bytes>,
    pub rendered: DashMap<String, String>,
    pub edges: DashMap<(String, String), u32>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl Storage for MemoryStorage {
    async fn save_raw(&self, url: &Url, _headers: &HeaderMap, body: &Bytes) -> Result<()> {
        self.raw.insert(url.to_string(), body.clone());
        Ok(())
    }
    async fn save_rendered(&self, url: &Url, html: &str, _meta: &PageMetadata) -> Result<()> {
        self.rendered.insert(url.to_string(), html.to_string());
        Ok(())
    }
    async fn save_edge(&self, from: &Url, to: &Url) -> Result<()> {
        *self
            .edges
            .entry((from.to_string(), to.to_string()))
            .or_insert(0) += 1;
        Ok(())
    }

    fn as_any_ref(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
