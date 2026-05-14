// Element fingerprint + similarity scoring (pure, no I/O).
//
// Port of Scrapling's adaptive-element comparison: a `Fingerprint`
// captures tag, attribute subsets (id / class / href / other), a text
// hash, the parent-chain tag sequence, and the sibling position. Two
// fingerprints are compared by a weighted sum of per-feature scores;
// a tag mismatch caps the final score to a small fraction so unrelated
// tags can never score "similar".
//
// No storage, no DOM mutation. Callers feed in an `ElementHandle` to
// build a fingerprint; later, a different tree's handle can be
// fingerprinted and `score()`'d against the saved one to find a
// best-matching element after the page has been redesigned.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};

use scraper::ElementRef;

use super::selectors::ElementHandle;

/// Stable feature bundle for one element. Cheap to clone; serialisable
/// in a future slice without changes to the public surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub tag: String,
    pub id: Option<String>,
    pub classes: BTreeSet<String>,
    pub href: Option<String>,
    pub other_attrs: BTreeMap<String, String>,
    /// Stable u64 hash of the normalised text content (trimmed,
    /// whitespace-collapsed, lowercased).
    pub text_hash: u64,
    /// Tokenised text used for fuzzy similarity (unicode-word split,
    /// lowercased). Kept alongside `text_hash` because the hash alone
    /// can't drive a Jaccard score.
    pub text_tokens: Vec<String>,
    /// Ancestor tag names, root first.
    pub parent_chain: Vec<String>,
    /// 1-based nth-of-type index among same-tag siblings.
    pub sibling_index: usize,
}

/// Build a fingerprint for `handle`. Pure: only reads the DOM.
pub fn fingerprint(handle: &ElementHandle<'_>) -> Fingerprint {
    let el = handle.inner();
    fingerprint_from_ref(el)
}

fn fingerprint_from_ref(el: ElementRef<'_>) -> Fingerprint {
    let tag = el.value().name().to_string();

    let mut id = None;
    let mut classes = BTreeSet::new();
    let mut href = None;
    let mut other_attrs = BTreeMap::new();

    for (k, v) in el.value().attrs() {
        match k {
            "id" => id = Some(v.to_string()),
            "class" => {
                for c in v.split_ascii_whitespace() {
                    classes.insert(c.to_string());
                }
            }
            "href" => href = Some(v.to_string()),
            _ => {
                other_attrs.insert(k.to_string(), v.to_string());
            }
        }
    }

    let raw_text: String = el.text().collect::<Vec<_>>().concat();
    let norm = normalise_text(&raw_text);
    let text_hash = stable_hash(&norm);
    let text_tokens = tokenise(&norm);

    let mut parent_chain = Vec::new();
    let mut cur = el.parent();
    while let Some(n) = cur {
        if let Some(p) = ElementRef::wrap(n) {
            parent_chain.push(p.value().name().to_string());
        }
        cur = n.parent();
    }
    parent_chain.reverse();

    let sibling_index = nth_of_type(el);

    Fingerprint {
        tag,
        id,
        classes,
        href,
        other_attrs,
        text_hash,
        text_tokens,
        parent_chain,
        sibling_index,
    }
}

fn nth_of_type(el: ElementRef<'_>) -> usize {
    let parent = match el.parent() {
        Some(p) => p,
        None => return 1,
    };
    let name = el.value().name();
    let target = el.id();
    let mut idx = 0usize;
    for sib in parent.children() {
        if let Some(s) = ElementRef::wrap(sib) {
            if s.value().name() == name {
                idx += 1;
                if sib.id() == target {
                    return idx;
                }
            }
        }
    }
    1
}

fn normalise_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            last_space = false;
        }
    }
    out.trim().to_string()
}

fn tokenise(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn stable_hash(s: &str) -> u64 {
    // FNV-1a so the hash is platform-stable (DefaultHasher isn't).
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ---------- scoring ----------

// Weights sum to 1.0. Tuned to mirror Scrapling's adaptive weights:
// tag/id/class dominate; text + parent chain are tie-breakers; sibling
// position is the lightest signal.
const W_TAG: f32 = 0.20;
const W_ID: f32 = 0.15;
const W_CLASS: f32 = 0.15;
const W_HREF: f32 = 0.10;
const W_OTHER: f32 = 0.10;
const W_TEXT: f32 = 0.15;
const W_PARENT: f32 = 0.10;
const W_SIBLING: f32 = 0.05;

/// Tag-mismatch cap: regardless of how well other features align,
/// different tags cannot exceed this score. Set deliberately below
/// the recall threshold mentioned in the slice (0.2).
const TAG_MISMATCH_CAP: f32 = 0.15;

/// Symmetric similarity in [0.0, 1.0].
///
/// Invariants:
///   * `score(a, a) == 1.0`
///   * `score(a, b) == score(b, a)`
///   * `a.tag != b.tag` ⇒ `score(a, b) <= TAG_MISMATCH_CAP`
pub fn score(a: &Fingerprint, b: &Fingerprint) -> f32 {
    let tag_eq = a.tag == b.tag;

    let id_s = opt_eq(&a.id, &b.id);
    let class_s = jaccard_set(&a.classes, &b.classes);
    let href_s = opt_string_similarity(&a.href, &b.href);
    let other_s = jaccard_kv(&a.other_attrs, &b.other_attrs);
    let text_s = text_similarity(a, b);
    let parent_s = parent_chain_similarity(&a.parent_chain, &b.parent_chain);
    let sibling_s = sibling_similarity(a.sibling_index, b.sibling_index);
    let tag_s = if tag_eq { 1.0 } else { 0.0 };

    let total = W_TAG * tag_s
        + W_ID * id_s
        + W_CLASS * class_s
        + W_HREF * href_s
        + W_OTHER * other_s
        + W_TEXT * text_s
        + W_PARENT * parent_s
        + W_SIBLING * sibling_s;

    let clamped = total.clamp(0.0, 1.0);
    if tag_eq {
        clamped
    } else {
        clamped.min(TAG_MISMATCH_CAP)
    }
}

fn opt_eq<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> f32 {
    match (a, b) {
        (None, None) => 1.0,
        (Some(x), Some(y)) if x == y => 1.0,
        _ => 0.0,
    }
}

fn opt_string_similarity(a: &Option<String>, b: &Option<String>) -> f32 {
    match (a, b) {
        (None, None) => 1.0,
        (Some(x), Some(y)) => {
            if x == y {
                return 1.0;
            }
            // URLs / hrefs: compare token sets so a `?utm=...` rewrite
            // still scores high.
            let ta: HashSet<&str> = x.split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty()).collect();
            let tb: HashSet<&str> = y.split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty()).collect();
            jaccard_refs(&ta, &tb)
        }
        _ => 0.0,
    }
}

fn jaccard_set<T: Ord>(a: &BTreeSet<T>, b: &BTreeSet<T>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 { 1.0 } else { inter / union }
}

fn jaccard_refs<T: Eq + Hash>(a: &HashSet<T>, b: &HashSet<T>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 { 1.0 } else { inter / union }
}

fn jaccard_kv(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    keys.extend(a.keys().map(|s| s.as_str()));
    keys.extend(b.keys().map(|s| s.as_str()));
    let mut inter = 0usize;
    for k in &keys {
        if a.get(*k) == b.get(*k) && a.contains_key(*k) {
            inter += 1;
        }
    }
    inter as f32 / keys.len() as f32
}

fn text_similarity(a: &Fingerprint, b: &Fingerprint) -> f32 {
    if a.text_hash == b.text_hash {
        return 1.0;
    }
    if a.text_tokens.is_empty() && b.text_tokens.is_empty() {
        return 1.0;
    }
    let sa: HashSet<&str> = a.text_tokens.iter().map(|s| s.as_str()).collect();
    let sb: HashSet<&str> = b.text_tokens.iter().map(|s| s.as_str()).collect();
    jaccard_refs(&sa, &sb)
}

fn parent_chain_similarity(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    // Compare suffixes (closest ancestors matter more than the root).
    let n = a.len().min(b.len());
    let mut matched = 0usize;
    for i in 0..n {
        let ai = &a[a.len() - 1 - i];
        let bi = &b[b.len() - 1 - i];
        if ai == bi {
            matched += 1;
        } else {
            break;
        }
    }
    let denom = a.len().max(b.len()) as f32;
    matched as f32 / denom
}

fn sibling_similarity(a: usize, b: usize) -> f32 {
    let lo = a.min(b) as f32;
    let hi = a.max(b) as f32;
    if hi == 0.0 { 1.0 } else { lo / hi }
}

#[cfg(test)]
mod tests {
    use super::super::parse_tree;
    use super::*;

    fn fp(html: &[u8], pick: &str) -> Fingerprint {
        let t = parse_tree(html, None);
        let h = t.css(pick).into_iter().next().expect("pick");
        fingerprint(&h)
    }

    #[test]
    fn identity_score_is_one() {
        let html = br#"<html><body><div id="x" class="a b"><p>hello</p></div></body></html>"#;
        let f = fp(html, "#x");
        let s = score(&f, &f);
        assert!((s - 1.0).abs() < 1e-6, "score(a,a) = {} != 1.0", s);
    }

    #[test]
    fn symmetric() {
        let a = fp(
            br#"<html><body><div><a href="/x" class="btn">go</a></div></body></html>"#,
            "a",
        );
        let b = fp(
            br#"<html><body><section><a href="/y" class="btn primary">go now</a></section></body></html>"#,
            "a",
        );
        let s1 = score(&a, &b);
        let s2 = score(&b, &a);
        assert!((s1 - s2).abs() < 1e-6, "asymmetric: {} vs {}", s1, s2);
    }

    #[test]
    fn tag_mismatch_caps_score() {
        // Same attributes, same text — but different tag.
        let a = fp(
            br#"<html><body><div class="x" id="same">hello world</div></body></html>"#,
            "#same",
        );
        let b = fp(
            br#"<html><body><span class="x" id="same">hello world</span></body></html>"#,
            "#same",
        );
        let s = score(&a, &b);
        assert!(s <= TAG_MISMATCH_CAP + 1e-6, "score {} not capped at {}", s, TAG_MISMATCH_CAP);
    }

    #[test]
    fn same_tag_differentiated_by_attrs_and_text() {
        let html = br#"<html><body>
            <li class="item" data-id="1">alpha</li>
            <li class="item" data-id="2">beta</li>
            <li class="other" data-id="3">zeta zog</li>
        </body></html>"#;
        let t = parse_tree(html, None);
        let lis = t.css("li");
        let f0 = fingerprint(&lis[0]);
        let f1 = fingerprint(&lis[1]);
        let f2 = fingerprint(&lis[2]);
        // f0 vs f1: same class, same parent, near identical → high.
        let s01 = score(&f0, &f1);
        // f0 vs f2: different class, different text → lower.
        let s02 = score(&f0, &f2);
        assert!(s01 > s02, "expected sibling-like pair to outrank dissimilar pair ({} vs {})", s01, s02);
        assert!(s01 >= 0.5, "near-identical lis should score >=0.5, got {}", s01);
    }

    // ---------- property tests ----------

    fn random_html(seed: u64) -> Vec<u8> {
        // Tiny LCG → deterministic varied HTML fragments.
        let mut x = seed.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
        let mut next = || {
            x = x.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
            x
        };
        let tag = ["div", "span", "p", "a"][(next() % 4) as usize];
        let cls = ["btn", "label", "card", "row"][(next() % 4) as usize];
        let id = (next() % 50).to_string();
        let text_a = ["price", "name", "qty", "sku"][(next() % 4) as usize];
        let text_b = ["alpha", "beta", "gamma", "delta"][(next() % 4) as usize];
        format!(
            r#"<html><body><section><{tag} id="x{id}" class="{cls}">{text_a} {text_b}</{tag}></section></body></html>"#
        )
        .into_bytes()
    }

    #[test]
    fn property_reflexive_and_symmetric() {
        for seed in 0..100u64 {
            let html_a = random_html(seed);
            let html_b = random_html(seed.wrapping_add(7919));
            let t_a = parse_tree(&html_a, None);
            let t_b = parse_tree(&html_b, None);
            let a = t_a.css("section > *").into_iter().next().unwrap();
            let b = t_b.css("section > *").into_iter().next().unwrap();
            let fa = fingerprint(&a);
            let fb = fingerprint(&b);
            let saa = score(&fa, &fa);
            assert!((saa - 1.0).abs() < 1e-6, "seed={seed} reflexive failed: {saa}");
            let sab = score(&fa, &fb);
            let sba = score(&fb, &fa);
            assert!((sab - sba).abs() < 1e-6, "seed={seed} not symmetric: {sab} vs {sba}");
            assert!((0.0..=1.0).contains(&sab), "seed={seed} out of range: {sab}");
        }
    }

    // ---------- recall on real-ish before/after fixture pairs ----------

    struct Pair {
        before: &'static [u8],
        after: &'static [u8],
        pick_before: &'static str,
        pick_after: &'static str,
    }

    // Threshold from the issue: a redesigned-twin should score >= 0.2
    // against its original even after attribute / position drift.
    const RECALL_THRESHOLD: f32 = 0.2;

    fn pairs() -> Vec<Pair> {
        vec![
            // 1. price label gains a wrapper span + class rename.
            Pair {
                before: br#"<html><body><div class="product"><span class="price">$42.00</span></div></body></html>"#,
                after: br#"<html><body><div class="product-card"><span class="price-tag"><span class="amount">$42.00</span></span></div></body></html>"#,
                pick_before: "span.price",
                pick_after: "span.amount",
            },
            // 2. CTA button changes tag from <button> to <a role="button">.
            // → tags differ on purpose; we expect score below threshold.
            Pair {
                before: br#"<html><body><nav><button class="cta primary">Buy now</button></nav></body></html>"#,
                after: br#"<html><body><nav><a role="button" class="cta primary">Buy now</a></nav></body></html>"#,
                pick_before: "button",
                pick_after: "a",
            },
            // 3. product title: h2 → h2 with new wrapper, same text.
            Pair {
                before: br#"<html><body><article><h2 class="title">Acme Widget</h2></article></body></html>"#,
                after: br#"<html><body><article><header><h2 class="product__title">Acme Widget</h2></header></article></body></html>"#,
                pick_before: "h2",
                pick_after: "h2",
            },
            // 4. "Add to cart" form button — class + parent rename.
            Pair {
                before: br#"<html><body><form id="atc"><button class="add-to-cart">Add to cart</button></form></body></html>"#,
                after: br#"<html><body><form id="cartform"><button class="btn btn--add">Add to cart</button></form></body></html>"#,
                pick_before: "button",
                pick_after: "button",
            },
            // 5. Sale badge moved up a level, same text.
            Pair {
                before: br#"<html><body><div class="card"><span class="badge sale">SALE</span></div></body></html>"#,
                after: br#"<html><body><div class="card"><div class="card-meta"><span class="tag tag-sale">SALE</span></div></div></body></html>"#,
                pick_before: "span.badge",
                pick_after: "span.tag",
            },
            // 6. Star rating — class renamed, text identical.
            Pair {
                before: br#"<html><body><div class="rating">4.5</div></body></html>"#,
                after: br#"<html><body><div class="product-rating">4.5</div></body></html>"#,
                pick_before: "div",
                pick_after: "div",
            },
            // 7. Breadcrumb link href changes path but same anchor text.
            Pair {
                before: br#"<html><body><nav><a href="/cat/shoes">Shoes</a></nav></body></html>"#,
                after: br#"<html><body><nav><a href="/categories/shoes?ref=nav">Shoes</a></nav></body></html>"#,
                pick_before: "a",
                pick_after: "a",
            },
            // 8. Stock indicator — text identical, sibling position changes.
            Pair {
                before: br#"<html><body><ul><li>spec1</li><li class="stock">In stock</li></ul></body></html>"#,
                after: br#"<html><body><ul><li class="availability">In stock</li><li>spec1</li><li>spec2</li></ul></body></html>"#,
                pick_before: "li.stock",
                pick_after: "li.availability",
            },
            // 9. Search input — type attr stays, id renamed.
            Pair {
                before: br#"<html><body><form><input id="q" type="search" placeholder="Search"/></form></body></html>"#,
                after: br#"<html><body><form><input id="search-input" type="search" placeholder="Search products"/></form></body></html>"#,
                pick_before: "input",
                pick_after: "input",
            },
            // 10. Image with src change + alt preserved.
            Pair {
                before: br#"<html><body><figure><img src="/a/1.jpg" alt="Acme Widget"/></figure></body></html>"#,
                after: br#"<html><body><figure><picture><img src="https://cdn.example.com/products/1.webp" alt="Acme Widget"/></picture></figure></body></html>"#,
                pick_before: "img",
                pick_after: "img",
            },
        ]
    }

    #[test]
    fn recall_on_real_dom_pairs_at_threshold() {
        let pairs = pairs();
        let mut hits = 0usize;
        let mut considered = 0usize;
        for (i, p) in pairs.iter().enumerate() {
            let fb = fp(p.before, p.pick_before);
            let fa = fp(p.after, p.pick_after);
            let s = score(&fb, &fa);
            // Pair 2 is the cross-tag case — we *expect* it below threshold.
            let same_tag = fb.tag == fa.tag;
            considered += 1;
            if same_tag {
                if s >= RECALL_THRESHOLD {
                    hits += 1;
                } else {
                    eprintln!("pair {} below threshold: {} ({} → {})", i + 1, s, fb.tag, fa.tag);
                }
            } else {
                // Cross-tag pair: confirm cap actually kicks in.
                assert!(
                    s <= TAG_MISMATCH_CAP + 1e-6,
                    "pair {} cross-tag uncapped: {}",
                    i + 1,
                    s
                );
                hits += 1; // counts as correct behaviour
            }
        }
        assert_eq!(hits, considered, "recall failed: {hits}/{considered}");
    }

    #[test]
    fn href_token_similarity_handles_querystring_drift() {
        let a = Fingerprint {
            tag: "a".into(), id: None, classes: BTreeSet::new(),
            href: Some("/cat/shoes".into()), other_attrs: BTreeMap::new(),
            text_hash: 0, text_tokens: vec![], parent_chain: vec![], sibling_index: 1,
        };
        let b = Fingerprint {
            href: Some("/cat/shoes?utm=foo".into()),
            ..a.clone()
        };
        let s = score(&a, &b);
        // Same tag + most fields equal — must be very high.
        assert!(s > 0.9, "href drift wrecked score: {s}");
    }
}
