use dashmap::DashMap;
use url::Url;

#[derive(Default)]
pub struct DiscoveryGraph {
    edges: DashMap<(String, String), u32>,
}

impl DiscoveryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_edge(&self, from: &Url, to: &Url) {
        let key = (from.to_string(), to.to_string());
        *self.edges.entry(key).or_insert(0) += 1;
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}
