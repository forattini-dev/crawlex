//! Drogasil amoxicilina price scraper — JobRunner-driven demo.
//!
//! Crawls www.drogasil.com.br/search?w=amoxicilina, finds product page
//! URLs, fetches each one (capped at --max), and prints
//! `{ product, laboratory, price }`. Polite 500ms inter-request delay.
//!
//! Run: cargo run --example drogasil_amoxicilina --all-features

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crawlex::discovery::assets::SecFetchDest;
use crawlex::impersonate::{ImpersonateClient, Profile};
use crawlex::runner::SpoofFetcher;
use scraper::{Html, Selector};

const SEARCH_URL: &str = "https://www.drogasil.com.br/search?w=amoxicilina";
const MAX_PRODUCTS: usize = 20;

#[derive(Debug)]
struct Product {
    url: String,
    name: String,
    laboratory: Option<String>,
    price: Option<String>,
}

#[tokio::main]
async fn main() {
    let client = Arc::new(ImpersonateClient::new(Profile::Chrome131Stable).expect("client"));
    let spoof = Arc::new(SpoofFetcher::new(client));

    println!("→ fetch search page: {SEARCH_URL}");
    let search_url: url::Url = SEARCH_URL.parse().unwrap();
    let search_resp = spoof
        .fetch_with(&search_url, SecFetchDest::Document, None, false)
        .await
        .expect("search fetch");
    println!("  status={} bytes={}", search_resp.status, search_resp.body.len());

    let search_html = String::from_utf8_lossy(&search_resp.body).into_owned();
    let product_urls = extract_product_urls(&search_html);
    println!("  found {} amoxicilina product URLs", product_urls.len());

    let mut products: Vec<Product> = Vec::new();
    for (i, purl) in product_urls.iter().take(MAX_PRODUCTS).enumerate() {
        let url: url::Url = match purl.parse() {
            Ok(u) => u,
            Err(_) => continue,
        };
        let resp = match spoof
            .fetch_with(&url, SecFetchDest::Document, None, false)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  [{:>2}] ERR {url} {e:?}", i + 1);
                continue;
            }
        };
        let html = String::from_utf8_lossy(&resp.body).into_owned();
        let product = parse_product(&html, purl.clone());
        println!(
            "  [{:>2}] {:>3}  {:>6}B  {}",
            i + 1,
            resp.status.as_u16(),
            resp.body.len(),
            product.name
        );
        products.push(product);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!("\n=== Amoxicilina @ Drogasil ===");
    println!(
        "{:<5} {:<55} {:<25} {}",
        "#", "PRODUCT", "LABORATORY", "PRICE"
    );
    for (i, p) in products.iter().enumerate() {
        println!(
            "{:<5} {:<55} {:<25} {}",
            i + 1,
            truncate(&p.name, 55),
            truncate(p.laboratory.as_deref().unwrap_or("?"), 25),
            p.price.as_deref().unwrap_or("?")
        );
    }
}

/// Pull product detail page URLs from the search results HTML.
/// Drogasil's product pages live at `/<slug>.html?origin=search` where
/// the slug encodes drug name + lab + presentation.
fn extract_product_urls(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let sel_a = Selector::parse("a[href]").unwrap();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for a in doc.select(&sel_a) {
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let lower = href.to_ascii_lowercase();
        if !lower.contains("amoxicilin") || !lower.contains(".html") {
            continue;
        }
        let abs = if href.starts_with("http") {
            href.to_string()
        } else if href.starts_with('/') {
            format!("https://www.drogasil.com.br{href}")
        } else {
            continue;
        };
        // Strip query strings for dedup.
        let canonical = abs.split('?').next().unwrap_or(&abs).to_string();
        if seen.insert(canonical.clone()) {
            out.push(canonical);
        }
    }
    out
}

fn looks_like_drug_name(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("amoxicilin") || l.contains("clavulanato") || l.contains("clavulanic")
}

/// Drogasil product pages embed structured data in JSON-LD plus visible
/// labels. We try JSON-LD first (`<script type="application/ld+json">`)
/// since it's stable; fall back to visible text patterns if missing.
fn parse_product(html: &str, url: String) -> Product {
    let doc = Html::parse_document(html);

    // JSON-LD path — most reliable.
    let ld_sel = Selector::parse(r#"script[type="application/ld+json"]"#).unwrap();
    let mut name: Option<String> = None;
    let mut laboratory: Option<String> = None;
    let mut price: Option<String> = None;
    for s in doc.select(&ld_sel) {
        let raw = s.text().collect::<String>();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            // Schema.org Product
            let type_field = v.get("@type").and_then(|x| x.as_str()).unwrap_or("");
            if type_field.eq_ignore_ascii_case("Product") {
                if name.is_none() {
                    name = v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                }
                if laboratory.is_none() {
                    laboratory = v
                        .pointer("/brand/name")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("brand").and_then(|x| x.as_str()))
                        .or_else(|| v.get("manufacturer").and_then(|x| x.as_str()))
                        .map(|s| s.to_string());
                }
                if price.is_none() {
                    price = v
                        .pointer("/offers/price")
                        .and_then(|x| {
                            x.as_str().map(|s| s.to_string()).or_else(|| {
                                x.as_f64().map(|n| format!("{:.2}", n))
                            })
                        })
                        .or_else(|| {
                            v.pointer("/offers/0/price").and_then(|x| {
                                x.as_str().map(|s| s.to_string()).or_else(|| {
                                    x.as_f64().map(|n| format!("{:.2}", n))
                                })
                            })
                        });
                }
            }
        }
    }

    // Fallbacks from visible HTML.
    if name.is_none() {
        let h1 = Selector::parse("h1").unwrap();
        if let Some(el) = doc.select(&h1).next() {
            let t = el.text().collect::<String>().trim().to_string();
            if !t.is_empty() {
                name = Some(t);
            }
        }
    }
    if price.is_none() {
        // Drogasil uses `[data-testid="price-value"]` or similar; fall back
        // to any element with "R$" pattern in the visible page text.
        let all = doc.root_element().text().collect::<Vec<_>>().join(" ");
        if let Some(m) = regex_lite_find_price(&all) {
            price = Some(m);
        }
    }
    // JSON-LD `brand.name` is often the active ingredient on Drogasil
    // (e.g. "Amoxicilina"), not the lab. Drop those values and let the
    // structured-HTML path below win.
    if laboratory.as_deref().map(looks_like_drug_name).unwrap_or(false) {
        laboratory = None;
    }

    // Drogasil renders an attribute table on the product page where one
    // row reads `<span>Fabricante</span><span><a>EMS</a></span>`. Walk
    // every `<span>` and, when we find one whose text is exactly
    // "Fabricante" (or "Marca" / "Laboratório"), take the text of the
    // immediately-following sibling span. Same structure across all
    // generics — no laboratory list needed.
    if laboratory.is_none() {
        let sel_span = Selector::parse("span").unwrap();
        let mut prev_was_label: Option<&'static str> = None;
        for span in doc.select(&sel_span) {
            let txt = span.text().collect::<String>();
            let t = txt.trim();
            match prev_was_label {
                Some(_label) => {
                    if !t.is_empty() && !looks_like_drug_name(t) {
                        laboratory = Some(t.to_string());
                        break;
                    }
                    prev_was_label = None;
                }
                None => {
                    if t.eq_ignore_ascii_case("Fabricante")
                        || t.eq_ignore_ascii_case("Marca")
                        || t.eq_ignore_ascii_case("Laboratório")
                    {
                        prev_was_label = Some("hit");
                    }
                }
            }
        }
    }

    Product {
        url,
        name: name.unwrap_or_else(|| "?".into()),
        laboratory,
        price,
    }
}

/// Minimal "R$ X,XX" finder without pulling regex as a dep.
fn regex_lite_find_price(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        if bytes[i] == b'R' && bytes[i + 1] == b'$' {
            // Skip whitespace.
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == 0xA0) {
                j += 1;
            }
            // Collect digits, dot, comma.
            let start = j;
            while j < bytes.len()
                && (bytes[j].is_ascii_digit() || bytes[j] == b',' || bytes[j] == b'.')
            {
                j += 1;
            }
            if j > start {
                let num = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                if !num.is_empty() && num.chars().any(|c| c.is_ascii_digit()) {
                    return Some(format!("R$ {num}"));
                }
            }
        }
        i += 1;
    }
    None
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
