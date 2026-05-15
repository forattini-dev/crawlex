//! JsonLd source — Hot tier (B2).
//!
//! Extracts `<script type="application/ld+json">` blocks and reads
//! Schema.org `Product` / `Organization` shapes for ecommerce + brand
//! evidence. Modest contribution today; expands as more Schema.org
//! types get recognised.

use scraper::{Html, Selector};
use serde_json::Value;

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct JsonLdSource;

impl JsonLdSource {
    pub fn new() -> Self {
        Self
    }
}

impl Source for JsonLdSource {
    fn name(&self) -> &'static str {
        "json_ld"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let body_str = match std::str::from_utf8(ctx.body) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let doc = Html::parse_document(body_str);
        let sel = Selector::parse(r#"script[type="application/ld+json"]"#).unwrap();

        let mut out: Vec<Detection> = Vec::new();
        for el in doc.select(&sel) {
            let raw = el.text().collect::<String>();
            let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            collect_from_value(&v, &mut out);
        }
        out
    }
}

/// Pull useful Detections out of one parsed JSON-LD value. Supports
/// both single objects and `@graph` arrays. Recursive over arrays.
fn collect_from_value(v: &Value, out: &mut Vec<Detection>) {
    match v {
        Value::Array(items) => {
            for item in items {
                collect_from_value(item, out);
            }
        }
        Value::Object(map) => {
            // @graph wrap
            if let Some(Value::Array(graph)) = map.get("@graph") {
                for g in graph {
                    collect_from_value(g, out);
                }
            }
            let type_field = map
                .get("@type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match type_field.as_str() {
                "product" => {
                    // Product schema present — ecommerce signal. Brand
                    // info populates evidence detail when present.
                    let brand = map
                        .get("brand")
                        .and_then(|b| {
                            b.get("name").and_then(|x| x.as_str()).or_else(|| b.as_str())
                        })
                        .unwrap_or("?");
                    out.push(Detection::from_single(
                        Category::Ecommerce,
                        Vendor::Generic,
                        Evidence::new(
                            EvidenceSource::JsonLd,
                            format!("Schema.org Product (brand={brand})"),
                            6,
                        ),
                    ));
                }
                "organization" => {
                    out.push(Detection::from_single(
                        Category::Other,
                        Vendor::Generic,
                        Evidence::new(EvidenceSource::JsonLd, "Schema.org Organization", 3),
                    ));
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    fn ctx_with<'a>(headers: &'a HeaderMap, url: &'a Url, body: &'a [u8]) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, body)
    }

    #[test]
    fn detects_product_with_brand() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script type="application/ld+json">{"@type":"Product","name":"X","brand":{"name":"EMS"}}</script></html>"#;
        let dets = JsonLdSource::new().analyze(&ctx_with(&h, &u, body));
        let p = dets
            .iter()
            .find(|d| d.category == Category::Ecommerce)
            .unwrap();
        assert!(p.evidence[0].detail.contains("EMS"));
    }

    #[test]
    fn detects_organization() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body =
            br#"<html><script type="application/ld+json">{"@type":"Organization","name":"X"}</script></html>"#;
        let dets = JsonLdSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.category == Category::Other));
    }

    #[test]
    fn handles_graph_array() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script type="application/ld+json">{"@graph":[{"@type":"Product","name":"A"},{"@type":"Organization","name":"O"}]}</script></html>"#;
        let dets = JsonLdSource::new().analyze(&ctx_with(&h, &u, body));
        assert_eq!(
            dets.iter().filter(|d| d.category == Category::Ecommerce).count(),
            1
        );
        assert_eq!(
            dets.iter().filter(|d| d.category == Category::Other).count(),
            1
        );
    }

    #[test]
    fn no_detection_without_ld_json() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><body>nothing</body></html>";
        let dets = JsonLdSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.is_empty());
    }

    #[test]
    fn malformed_json_is_skipped() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script type="application/ld+json">{not json}</script></html>"#;
        let dets = JsonLdSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.is_empty());
    }
}
