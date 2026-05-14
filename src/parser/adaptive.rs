// Adaptive relocation for selectors (slice 14).
//
// First call with a new `identifier` saves the matched element's
// fingerprint to the adaptive store under (domain, identifier). On
// subsequent calls, if the supplied selector still resolves to a
// candidate scoring >= `threshold` against the saved fingerprint, that
// candidate is returned directly. Otherwise the entire DOM is walked,
// every element scored against the saved fingerprint, and the highest-
// scoring node above threshold is returned with its score exposed as
// `adaptive_confidence` on the returned handle.

use ego_tree::NodeRef;
use scraper::{node::Node, ElementRef};

use crate::storage::AdaptiveStore;

use super::selectors::ElementHandle;
use super::similarity::{fingerprint, score, Fingerprint};
use super::TreeHandle;

/// Default similarity threshold (matches Scrapling).
pub const DEFAULT_THRESHOLD: f32 = 0.2;

/// Result of an adaptive query.
#[derive(Clone, Copy)]
pub struct AdaptiveMatch<'a> {
    handle: ElementHandle<'a>,
    /// `Some(score)` when the element was relocated via fingerprint walk;
    /// `None` when the selector resolved directly (no relocation needed).
    confidence: Option<f32>,
}

impl<'a> AdaptiveMatch<'a> {
    pub fn handle(&self) -> ElementHandle<'a> {
        self.handle
    }
    /// Confidence score for a relocated match; `None` when the original
    /// selector resolved directly.
    pub fn adaptive_confidence(&self) -> Option<f32> {
        self.confidence
    }
}

/// Per-call adaptive parameters. `threshold` defaults to
/// [`DEFAULT_THRESHOLD`] when not overridden.
#[derive(Debug, Clone)]
pub struct AdaptiveOptions {
    pub identifier: String,
    pub threshold: f32,
}

impl AdaptiveOptions {
    pub fn new(identifier: impl Into<String>) -> Self {
        Self { identifier: identifier.into(), threshold: DEFAULT_THRESHOLD }
    }
    pub fn with_threshold(mut self, t: f32) -> Self {
        self.threshold = t;
        self
    }
}

impl TreeHandle {
    /// Adaptive CSS query. See module docs for semantics.
    pub fn css_adaptive<'a>(
        &'a self,
        sel: &str,
        store: &AdaptiveStore,
        domain: &str,
        opts: &AdaptiveOptions,
    ) -> Option<AdaptiveMatch<'a>> {
        let candidates = self.css(sel);
        adaptive_resolve(self.root_element(), candidates, store, domain, opts)
    }

    /// Adaptive XPath query.
    pub fn xpath_adaptive<'a>(
        &'a self,
        expr: &str,
        store: &AdaptiveStore,
        domain: &str,
        opts: &AdaptiveOptions,
    ) -> Option<AdaptiveMatch<'a>> {
        let candidates = self.xpath(expr);
        adaptive_resolve(self.root_element(), candidates, store, domain, opts)
    }
}

fn adaptive_resolve<'a>(
    root: ElementRef<'a>,
    candidates: Vec<ElementHandle<'a>>,
    store: &AdaptiveStore,
    domain: &str,
    opts: &AdaptiveOptions,
) -> Option<AdaptiveMatch<'a>> {
    let saved = store.retrieve(domain, &opts.identifier);

    match saved {
        None => {
            // First run: save the first candidate's fingerprint, no
            // relocation needed.
            let first = candidates.into_iter().next()?;
            let fp = fingerprint(&first);
            if let Err(e) = store.save(domain, &opts.identifier, fp) {
                tracing::warn!(
                    target: "adaptive",
                    domain = %domain,
                    identifier = %opts.identifier,
                    error = %e,
                    "adaptive store save failed",
                );
            }
            Some(AdaptiveMatch { handle: first, confidence: None })
        }
        Some(saved_fp) => {
            // Try direct candidates first.
            let mut best_direct: Option<(ElementHandle<'a>, f32)> = None;
            for c in &candidates {
                let s = score(&saved_fp, &fingerprint(c));
                if best_direct.as_ref().map(|(_, b)| s > *b).unwrap_or(true) {
                    best_direct = Some((*c, s));
                }
            }
            if let Some((h, s)) = best_direct {
                if s >= opts.threshold {
                    return Some(AdaptiveMatch { handle: h, confidence: None });
                }
            }
            // Relocate: walk all elements and pick the highest score
            // above threshold.
            relocate(root, &saved_fp, opts, domain)
        }
    }
}

fn relocate<'a>(
    root: ElementRef<'a>,
    saved: &Fingerprint,
    opts: &AdaptiveOptions,
    domain: &str,
) -> Option<AdaptiveMatch<'a>> {
    let mut best: Option<(ElementHandle<'a>, f32)> = None;
    let mut stack: Vec<NodeRef<'a, Node>> = vec![*root];
    while let Some(n) = stack.pop() {
        if let Some(el) = ElementRef::wrap(n) {
            let h = ElementHandle::from(el);
            let s = score(saved, &fingerprint(&h));
            if best.as_ref().map(|(_, b)| s > *b).unwrap_or(true) {
                best = Some((h, s));
            }
        }
        for c in n.children() {
            stack.push(c);
        }
    }
    let (h, s) = best?;
    if s >= opts.threshold {
        tracing::info!(
            target: "adaptive",
            domain = %domain,
            identifier = %opts.identifier,
            confidence = s,
            tag = %h.name(),
            "adaptive relocation succeeded",
        );
        Some(AdaptiveMatch { handle: h, confidence: Some(s) })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::parser::parse_tree;

    const BEFORE: &[u8] = br#"<!doctype html>
<html><body><article>
  <header><h1 class="title">Acme Widget</h1></header>
  <p class="price">$42.00</p>
</article></body></html>"#;

    const AFTER: &[u8] = br#"<!doctype html>
<html><body><article>
  <div class="card"><h1 class="product__title">Acme Widget</h1></div>
  <p class="amount">$42.00</p>
</article></body></html>"#;

    #[test]
    fn first_run_saves_fingerprint_and_returns_direct_match() {
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        let tree = parse_tree(BEFORE, None);
        let opts = AdaptiveOptions::new("title");
        let m = tree
            .css_adaptive("h1.title", &store, "example.com", &opts)
            .expect("first run hits");
        assert!(m.adaptive_confidence().is_none());
        assert_eq!(m.handle().text(), "Acme Widget");
        // Fingerprint persisted under (domain, identifier).
        assert!(store.retrieve("example.com", "title").is_some());
    }

    #[test]
    fn subsequent_run_same_dom_still_direct() {
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        let tree = parse_tree(BEFORE, None);
        let opts = AdaptiveOptions::new("title");
        tree.css_adaptive("h1.title", &store, "example.com", &opts).unwrap();
        let again = tree
            .css_adaptive("h1.title", &store, "example.com", &opts)
            .unwrap();
        assert!(again.adaptive_confidence().is_none());
    }

    #[test]
    fn mutated_dom_triggers_relocation_with_confidence() {
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        // Train on BEFORE.
        {
            let t = parse_tree(BEFORE, None);
            let opts = AdaptiveOptions::new("title");
            t.css_adaptive("h1.title", &store, "shop.example", &opts).unwrap();
        }
        // Query mutated DOM where original selector misses.
        let t2 = parse_tree(AFTER, None);
        assert!(t2.css("h1.title").is_empty(), "selector should miss on mutated DOM");
        let opts = AdaptiveOptions::new("title");
        let m = t2
            .css_adaptive("h1.title", &store, "shop.example", &opts)
            .expect("relocation finds twin");
        let conf = m.adaptive_confidence().expect("relocated => some confidence");
        assert!(conf >= 0.2, "confidence {} below threshold", conf);
        assert_eq!(m.handle().name(), "h1");
        assert_eq!(m.handle().text(), "Acme Widget");
    }

    #[test]
    fn threshold_override_can_block_relocation() {
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        {
            let t = parse_tree(BEFORE, None);
            let opts = AdaptiveOptions::new("title");
            t.css_adaptive("h1.title", &store, "shop.example", &opts).unwrap();
        }
        let t2 = parse_tree(AFTER, None);
        // Impossibly high threshold => no relocation.
        let strict = AdaptiveOptions::new("title").with_threshold(0.999);
        let m = t2.css_adaptive("h1.title", &store, "shop.example", &strict);
        assert!(m.is_none(), "high threshold should block relocation");
    }

    #[test]
    fn xpath_adaptive_relocates() {
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        {
            let t = parse_tree(BEFORE, None);
            let opts = AdaptiveOptions::new("title-x");
            t.xpath_adaptive("//h1[@class='title']", &store, "ex.com", &opts).unwrap();
        }
        let t2 = parse_tree(AFTER, None);
        assert!(t2.xpath("//h1[@class='title']").is_empty());
        let opts = AdaptiveOptions::new("title-x");
        let m = t2
            .xpath_adaptive("//h1[@class='title']", &store, "ex.com", &opts)
            .expect("xpath relocates");
        assert!(m.adaptive_confidence().is_some());
        assert_eq!(m.handle().text(), "Acme Widget");
    }

    #[test]
    fn no_saved_fingerprint_and_no_candidates_returns_none() {
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        let t = parse_tree(BEFORE, None);
        let opts = AdaptiveOptions::new("nope");
        let m = t.css_adaptive(".does-not-exist", &store, "ex.com", &opts);
        assert!(m.is_none());
        assert!(store.retrieve("ex.com", "nope").is_none());
    }

    #[test]
    fn direct_match_above_threshold_skips_relocation() {
        // Saved fp came from BEFORE; on BEFORE itself, selector hits and
        // scores 1.0 => confidence None.
        let dir = tempdir().unwrap();
        let store = AdaptiveStore::open(dir.path(), "spider1").unwrap();
        let t = parse_tree(BEFORE, None);
        let opts = AdaptiveOptions::new("price");
        t.css_adaptive("p.price", &store, "ex.com", &opts).unwrap();
        let m = t.css_adaptive("p.price", &store, "ex.com", &opts).unwrap();
        assert!(m.adaptive_confidence().is_none());
    }
}
