use http::HeaderMap;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use time::OffsetDateTime;
use url::Url;

use crate::discovery::cert::PeerCert;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TechSource {
    Header,
    Cookie,
    HtmlMeta,
    ScriptUrl,
    Dom,
    AssetDomain,
    Dns,
    Tls,
    Port,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TechEvidence {
    pub source: TechSource,
    pub key: String,
    pub value: String,
    pub confidence_delta: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TechMatch {
    pub slug: String,
    pub name: String,
    pub category: String,
    pub confidence: u8,
    pub evidence: Vec<TechEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TechFingerprintReport {
    pub url: String,
    pub final_url: String,
    pub host: String,
    pub technologies: Vec<TechMatch>,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct TechFingerprintFacts<'a> {
    pub peer_cert: Option<&'a PeerCert>,
    pub dns_json: Option<&'a str>,
    pub open_ports: &'a [u16],
    pub manifest_present: bool,
    pub service_worker_present: bool,
}

impl<'a> Default for TechFingerprintFacts<'a> {
    fn default() -> Self {
        Self {
            peer_cert: None,
            dns_json: None,
            open_ports: &[],
            manifest_present: false,
            service_worker_present: false,
        }
    }
}

#[derive(Default)]
struct Builder {
    name: &'static str,
    category: &'static str,
    evidence: Vec<TechEvidence>,
}

static META_GENERATOR_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("meta[name='generator'], meta[name='Generator']").unwrap());

pub fn analyze(
    url: &Url,
    final_url: &Url,
    headers: Option<&HeaderMap>,
    html: Option<&str>,
) -> TechFingerprintReport {
    analyze_with_facts(
        url,
        final_url,
        headers,
        html,
        TechFingerprintFacts::default(),
    )
}

pub fn analyze_with_facts(
    url: &Url,
    final_url: &Url,
    headers: Option<&HeaderMap>,
    html: Option<&str>,
    facts: TechFingerprintFacts<'_>,
) -> TechFingerprintReport {
    analyze_inner(url, final_url, headers, html, None, facts)
}

pub fn analyze_with_facts_from_document(
    url: &Url,
    final_url: &Url,
    headers: Option<&HeaderMap>,
    html: &str,
    doc: &Html,
    facts: TechFingerprintFacts<'_>,
) -> TechFingerprintReport {
    analyze_inner(url, final_url, headers, Some(html), Some(doc), facts)
}

fn analyze_inner(
    url: &Url,
    final_url: &Url,
    headers: Option<&HeaderMap>,
    html: Option<&str>,
    html_doc: Option<&Html>,
    facts: TechFingerprintFacts<'_>,
) -> TechFingerprintReport {
    let mut found: BTreeMap<&'static str, Builder> = BTreeMap::new();

    if let Some(headers) = headers {
        for (name, value) in headers.iter() {
            let key = name.as_str().to_ascii_lowercase();
            let Ok(value) = value.to_str() else {
                continue;
            };
            let lower = value.to_ascii_lowercase();
            match key.as_str() {
                "server" => {
                    if lower.contains("cloudflare") {
                        add(
                            &mut found,
                            "cloudflare",
                            "Cloudflare",
                            "cdn",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("akamai") {
                        add(
                            &mut found,
                            "akamai",
                            "Akamai",
                            "cdn",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("fastly") {
                        add(
                            &mut found,
                            "fastly",
                            "Fastly",
                            "cdn",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("cloudfront") {
                        add(
                            &mut found,
                            "cloudfront",
                            "CloudFront",
                            "cdn",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("nginx") {
                        add(
                            &mut found,
                            "nginx",
                            "nginx",
                            "server",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("apache") {
                        add(
                            &mut found,
                            "apache",
                            "Apache HTTP Server",
                            "server",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("openresty") {
                        add(
                            &mut found,
                            "openresty",
                            "OpenResty",
                            "server",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("microsoft-iis") || lower.contains("iis") {
                        add(
                            &mut found,
                            "iis",
                            "Microsoft IIS",
                            "server",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                }
                "x-powered-by" => {
                    if lower.contains("express") {
                        add(
                            &mut found,
                            "express",
                            "Express",
                            "backend",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("asp.net") {
                        add(
                            &mut found,
                            "aspnet",
                            "ASP.NET",
                            "backend",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("next.js") || lower.contains("nextjs") {
                        add(
                            &mut found,
                            "nextjs",
                            "Next.js",
                            "framework",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("laravel") {
                        add(
                            &mut found,
                            "laravel",
                            "Laravel",
                            "backend",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("rails") || lower.contains("ruby on rails") {
                        add(
                            &mut found,
                            "rails",
                            "Ruby on Rails",
                            "backend",
                            TechSource::Header,
                            &key,
                            value,
                            85,
                        );
                    }
                    if lower.contains("django") {
                        add(
                            &mut found,
                            "django",
                            "Django",
                            "backend",
                            TechSource::Header,
                            &key,
                            value,
                            85,
                        );
                    }
                }
                "x-runtime" | "x-request-id" => {
                    if key == "x-runtime" {
                        add(
                            &mut found,
                            "rails",
                            "Ruby on Rails",
                            "backend",
                            TechSource::Header,
                            &key,
                            value,
                            45,
                        );
                    }
                }
                "x-generator" => {
                    if lower.contains("wordpress") {
                        add(
                            &mut found,
                            "wordpress",
                            "WordPress",
                            "cms",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                    if lower.contains("drupal") {
                        add(
                            &mut found,
                            "drupal",
                            "Drupal",
                            "cms",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("magento") {
                        add(
                            &mut found,
                            "magento",
                            "Magento",
                            "ecommerce",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("shopify") {
                        add(
                            &mut found,
                            "shopify",
                            "Shopify",
                            "ecommerce",
                            TechSource::Header,
                            &key,
                            value,
                            95,
                        );
                    }
                }
                "x-shopid" | "x-shopify-stage" | "x-shopify-shop-api-call-limit" => {
                    add(
                        &mut found,
                        "shopify",
                        "Shopify",
                        "ecommerce",
                        TechSource::Header,
                        &key,
                        value,
                        95,
                    );
                }
                "cf-ray" | "cf-cache-status" | "cf-mitigated" => {
                    add(
                        &mut found,
                        "cloudflare",
                        "Cloudflare",
                        "cdn",
                        TechSource::Header,
                        &key,
                        value,
                        98,
                    );
                }
                "x-cache" | "via" => {
                    if lower.contains("cloudfront") {
                        add(
                            &mut found,
                            "cloudfront",
                            "CloudFront",
                            "cdn",
                            TechSource::Header,
                            &key,
                            value,
                            90,
                        );
                    }
                    if lower.contains("varnish") {
                        add(
                            &mut found,
                            "varnish",
                            "Varnish",
                            "cdn",
                            TechSource::Header,
                            &key,
                            value,
                            80,
                        );
                    }
                }
                "set-cookie" => detect_cookie(&mut found, value),
                _ => {}
            }
        }
    }

    if let Some(html) = html {
        detect_html(&mut found, html, html_doc);
    }
    detect_facts(&mut found, facts);

    let technologies = found
        .into_iter()
        .map(|(slug, b)| {
            let confidence = b
                .evidence
                .iter()
                .map(|e| e.confidence_delta as u16)
                .sum::<u16>()
                .min(100) as u8;
            TechMatch {
                slug: slug.to_string(),
                name: b.name.to_string(),
                category: b.category.to_string(),
                confidence,
                evidence: b.evidence,
            }
        })
        .filter(|m| !m.evidence.is_empty())
        .collect();

    TechFingerprintReport {
        url: url.to_string(),
        final_url: final_url.to_string(),
        host: final_url
            .host_str()
            .or_else(|| url.host_str())
            .unwrap_or("")
            .to_string(),
        technologies,
        generated_at: OffsetDateTime::now_utc().unix_timestamp(),
    }
}

fn detect_cookie(found: &mut BTreeMap<&'static str, Builder>, value: &str) {
    let lower = value.to_ascii_lowercase();
    if lower.contains("__cf_bm") || lower.contains("cf_clearance") {
        add(
            found,
            "cloudflare",
            "Cloudflare",
            "cdn",
            TechSource::Cookie,
            "set-cookie",
            value,
            95,
        );
    }
    if lower.contains("_abck") || lower.contains("bm_sz") {
        add(
            found,
            "akamai",
            "Akamai",
            "waf",
            TechSource::Cookie,
            "set-cookie",
            value,
            90,
        );
    }
    if lower.contains("datadome") {
        add(
            found,
            "datadome",
            "DataDome",
            "security",
            TechSource::Cookie,
            "set-cookie",
            value,
            95,
        );
    }
    if lower.contains("_px") {
        add(
            found,
            "perimeterx",
            "PerimeterX",
            "security",
            TechSource::Cookie,
            "set-cookie",
            value,
            90,
        );
    }
    if lower.contains("laravel_session") {
        add(
            found,
            "laravel",
            "Laravel",
            "backend",
            TechSource::Cookie,
            "set-cookie",
            value,
            90,
        );
    }
    if lower.contains("_rails_session") {
        add(
            found,
            "rails",
            "Ruby on Rails",
            "backend",
            TechSource::Cookie,
            "set-cookie",
            value,
            90,
        );
    }
    if lower.contains("csrftoken") {
        add(
            found,
            "django",
            "Django",
            "backend",
            TechSource::Cookie,
            "set-cookie",
            value,
            75,
        );
    }
    if lower.contains("wordpress_") || lower.contains("wp-settings-") {
        add(
            found,
            "wordpress",
            "WordPress",
            "cms",
            TechSource::Cookie,
            "set-cookie",
            value,
            90,
        );
    }
    if lower.contains("woocommerce_cart_hash")
        || lower.contains("woocommerce_items_in_cart")
        || lower.contains("wp_woocommerce_session_")
    {
        add(
            found,
            "woocommerce",
            "WooCommerce",
            "ecommerce",
            TechSource::Cookie,
            "set-cookie",
            value,
            90,
        );
    }
}

fn detect_html(found: &mut BTreeMap<&'static str, Builder>, html: &str, doc: Option<&Html>) {
    let lower = html.to_ascii_lowercase();
    for (needle, slug, name, category, source, score) in [
        (
            "wp-content/",
            "wordpress",
            "WordPress",
            "cms",
            TechSource::ScriptUrl,
            80,
        ),
        (
            "wp-includes/",
            "wordpress",
            "WordPress",
            "cms",
            TechSource::ScriptUrl,
            80,
        ),
        (
            "wp-content/plugins/woocommerce",
            "woocommerce",
            "WooCommerce",
            "ecommerce",
            TechSource::ScriptUrl,
            90,
        ),
        (
            "woocommerce",
            "woocommerce",
            "WooCommerce",
            "ecommerce",
            TechSource::Dom,
            55,
        ),
        (
            "wc-cart-fragments",
            "woocommerce",
            "WooCommerce",
            "ecommerce",
            TechSource::ScriptUrl,
            85,
        ),
        (
            "cdn.shopify.com",
            "shopify",
            "Shopify",
            "ecommerce",
            TechSource::AssetDomain,
            85,
        ),
        (
            "shopify-section",
            "shopify",
            "Shopify",
            "ecommerce",
            TechSource::Dom,
            70,
        ),
        (
            "vtexassets.com",
            "vtex",
            "VTEX",
            "ecommerce",
            TechSource::AssetDomain,
            85,
        ),
        (
            "vteximg.com.br",
            "vtex",
            "VTEX",
            "ecommerce",
            TechSource::AssetDomain,
            80,
        ),
        (
            "mage/cookies",
            "magento",
            "Magento",
            "ecommerce",
            TechSource::ScriptUrl,
            70,
        ),
        (
            "static/version",
            "magento",
            "Magento",
            "ecommerce",
            TechSource::ScriptUrl,
            55,
        ),
        (
            "magento",
            "magento",
            "Magento",
            "ecommerce",
            TechSource::Dom,
            55,
        ),
        (
            "googletagmanager.com/gtm.js",
            "google-tag-manager",
            "Google Tag Manager",
            "tag_manager",
            TechSource::ScriptUrl,
            90,
        ),
        (
            "google-analytics.com",
            "google-analytics",
            "Google Analytics",
            "analytics",
            TechSource::ScriptUrl,
            85,
        ),
        (
            "gtag/js",
            "google-analytics",
            "Google Analytics",
            "analytics",
            TechSource::ScriptUrl,
            80,
        ),
        (
            "segment.com/analytics.js",
            "segment",
            "Segment",
            "analytics",
            TechSource::ScriptUrl,
            85,
        ),
        (
            "cdn.segment.com/analytics.js",
            "segment",
            "Segment",
            "analytics",
            TechSource::ScriptUrl,
            90,
        ),
        (
            "laravel_session",
            "laravel",
            "Laravel",
            "backend",
            TechSource::Dom,
            75,
        ),
        (
            "csrf-token",
            "rails",
            "Ruby on Rails",
            "backend",
            TechSource::HtmlMeta,
            55,
        ),
        (
            "csrf-param",
            "rails",
            "Ruby on Rails",
            "backend",
            TechSource::HtmlMeta,
            55,
        ),
        (
            "rails-ujs",
            "rails",
            "Ruby on Rails",
            "backend",
            TechSource::ScriptUrl,
            75,
        ),
        (
            "data-turbo-track",
            "rails",
            "Ruby on Rails",
            "backend",
            TechSource::Dom,
            45,
        ),
        (
            "csrfmiddlewaretoken",
            "django",
            "Django",
            "backend",
            TechSource::Dom,
            80,
        ),
        ("django", "django", "Django", "backend", TechSource::Dom, 45),
        (
            "__next_data__",
            "nextjs",
            "Next.js",
            "framework",
            TechSource::Dom,
            90,
        ),
        (
            "/_next/static/",
            "nextjs",
            "Next.js",
            "framework",
            TechSource::ScriptUrl,
            90,
        ),
        (
            "window.__nuxt__",
            "nuxt",
            "Nuxt",
            "framework",
            TechSource::Dom,
            90,
        ),
        (
            "/_nuxt/",
            "nuxt",
            "Nuxt",
            "framework",
            TechSource::ScriptUrl,
            85,
        ),
        (
            "/@vite/client",
            "vite",
            "Vite",
            "framework",
            TechSource::ScriptUrl,
            95,
        ),
        (
            "data-vite-dev-id",
            "vite",
            "Vite",
            "framework",
            TechSource::Dom,
            85,
        ),
        (
            "vite.svg",
            "vite",
            "Vite",
            "framework",
            TechSource::AssetDomain,
            55,
        ),
        (
            "data-reactroot",
            "react",
            "React",
            "frontend",
            TechSource::Dom,
            70,
        ),
        (
            "__reactroot",
            "react",
            "React",
            "frontend",
            TechSource::Dom,
            70,
        ),
        (
            "react-dom",
            "react",
            "React",
            "frontend",
            TechSource::ScriptUrl,
            65,
        ),
        (
            "data-server-rendered=\"true\"",
            "vue",
            "Vue.js",
            "frontend",
            TechSource::Dom,
            85,
        ),
        (
            "vue.global",
            "vue",
            "Vue.js",
            "frontend",
            TechSource::ScriptUrl,
            75,
        ),
        (
            "vue-router",
            "vue",
            "Vue.js",
            "frontend",
            TechSource::ScriptUrl,
            70,
        ),
        ("data-v-", "vue", "Vue.js", "frontend", TechSource::Dom, 55),
        (
            "ng-version=",
            "angular",
            "Angular",
            "frontend",
            TechSource::Dom,
            85,
        ),
        (
            "svelte-",
            "svelte",
            "Svelte",
            "frontend",
            TechSource::Dom,
            65,
        ),
    ] {
        if lower.contains(needle) {
            add(found, slug, name, category, source, needle, needle, score);
        }
    }

    let parsed;
    let doc = if let Some(doc) = doc {
        doc
    } else {
        parsed = Html::parse_document(html);
        &parsed
    };
    for meta in doc.select(&META_GENERATOR_SELECTOR) {
        let Some(content) = meta.value().attr("content") else {
            continue;
        };
        let c = content.to_ascii_lowercase();
        if c.contains("wordpress") {
            add(
                found,
                "wordpress",
                "WordPress",
                "cms",
                TechSource::HtmlMeta,
                "generator",
                content,
                95,
            );
        }
        if c.contains("shopify") {
            add(
                found,
                "shopify",
                "Shopify",
                "ecommerce",
                TechSource::HtmlMeta,
                "generator",
                content,
                95,
            );
        }
        if c.contains("magento") {
            add(
                found,
                "magento",
                "Magento",
                "ecommerce",
                TechSource::HtmlMeta,
                "generator",
                content,
                90,
            );
        }
        if c.contains("woocommerce") {
            add(
                found,
                "woocommerce",
                "WooCommerce",
                "ecommerce",
                TechSource::HtmlMeta,
                "generator",
                content,
                90,
            );
        }
        if c.contains("vite") {
            add(
                found,
                "vite",
                "Vite",
                "framework",
                TechSource::HtmlMeta,
                "generator",
                content,
                80,
            );
        }
        if c.contains("django") {
            add(
                found,
                "django",
                "Django",
                "backend",
                TechSource::HtmlMeta,
                "generator",
                content,
                80,
            );
        }
        if c.contains("rails") {
            add(
                found,
                "rails",
                "Ruby on Rails",
                "backend",
                TechSource::HtmlMeta,
                "generator",
                content,
                80,
            );
        }
    }
}

fn detect_facts(found: &mut BTreeMap<&'static str, Builder>, facts: TechFingerprintFacts<'_>) {
    if let Some(cert) = facts.peer_cert {
        detect_tls_cert(found, cert);
    }
    if let Some(dns_json) = facts.dns_json {
        detect_dns_json(found, dns_json);
    }
    detect_ports(found, facts.open_ports);
    if facts.manifest_present {
        add(
            found,
            "web-app-manifest",
            "Web App Manifest",
            "frontend",
            TechSource::Dom,
            "manifest",
            "present",
            45,
        );
    }
    if facts.service_worker_present {
        add(
            found,
            "service-worker",
            "Service Worker",
            "frontend",
            TechSource::Dom,
            "service_worker",
            "present",
            55,
        );
    }
}

fn detect_tls_cert(found: &mut BTreeMap<&'static str, Builder>, cert: &PeerCert) {
    if let Some(issuer) = cert.issuer_cn.as_deref() {
        detect_infra_value(found, TechSource::Tls, "cert.issuer_cn", issuer, 72);
    }
    if let Some(subject) = cert.subject_cn.as_deref() {
        detect_infra_value(found, TechSource::Tls, "cert.subject_cn", subject, 78);
    }
    for san in &cert.sans {
        detect_infra_value(found, TechSource::Tls, "cert.san", san, 78);
    }
}

fn detect_dns_json(found: &mut BTreeMap<&'static str, Builder>, dns_json: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(dns_json) else {
        return;
    };
    for (field, score) in [
        ("cname", 85),
        ("ns", 72),
        ("txt", 55),
        ("mx", 50),
        ("caa", 45),
        ("cloud", 72),
    ] {
        let Some(values) = value.get(field).and_then(|v| v.as_array()) else {
            continue;
        };
        let key = format!("dns.{field}");
        for item in values {
            if let Some(s) = item.as_str() {
                detect_infra_value(found, TechSource::Dns, &key, s, score);
            }
        }
    }
}

fn detect_infra_value(
    found: &mut BTreeMap<&'static str, Builder>,
    source: TechSource,
    key: &str,
    value: &str,
    score: u8,
) {
    let lower = value.to_ascii_lowercase();
    if lower.contains("cloudflare") || lower.contains("cloudflaressl") {
        add(
            found,
            "cloudflare",
            "Cloudflare",
            "cdn",
            source,
            key,
            value,
            score.max(72),
        );
    }
    if lower.contains("akamai") || lower.contains("edgesuite") || lower.contains("edgekey") {
        add(
            found,
            "akamai",
            "Akamai",
            "cdn",
            source,
            key,
            value,
            score.max(72),
        );
    }
    if lower.contains("fastly") || lower.contains("fastlylb") {
        add(
            found,
            "fastly",
            "Fastly",
            "cdn",
            source,
            key,
            value,
            score.max(72),
        );
    }
    if lower.contains("cloudfront") {
        add(
            found,
            "cloudfront",
            "CloudFront",
            "cdn",
            source,
            key,
            value,
            score.max(75),
        );
    }
    if lower == "aws" || lower.starts_with("aws:") {
        add(
            found,
            "aws",
            "Amazon Web Services",
            "hosting",
            source,
            key,
            value,
            score.max(55),
        );
    }
    if lower == "gcp" || lower.starts_with("gcp:") || lower.contains("google cloud") {
        add(
            found,
            "google-cloud",
            "Google Cloud",
            "hosting",
            source,
            key,
            value,
            score.max(55),
        );
    }
    if lower == "azure" || lower.starts_with("azure:") || lower.contains("microsoft azure") {
        add(
            found,
            "azure",
            "Microsoft Azure",
            "hosting",
            source,
            key,
            value,
            score.max(55),
        );
    }
    if lower.contains("myshopify.com") || lower.contains("shops.myshopify.com") {
        add(
            found,
            "shopify",
            "Shopify",
            "ecommerce",
            source,
            key,
            value,
            score.max(70),
        );
    }
    if lower.contains("vtex") || lower.contains("vtexassets") || lower.contains("vteximg") {
        add(
            found,
            "vtex",
            "VTEX",
            "ecommerce",
            source,
            key,
            value,
            score.max(65),
        );
    }
    if lower.contains("vercel") {
        add(
            found,
            "vercel",
            "Vercel",
            "hosting",
            source,
            key,
            value,
            score.max(65),
        );
    }
    if lower.contains("netlify") {
        add(
            found,
            "netlify",
            "Netlify",
            "hosting",
            source,
            key,
            value,
            score.max(65),
        );
    }
}

fn detect_ports(found: &mut BTreeMap<&'static str, Builder>, open_ports: &[u16]) {
    for port in open_ports {
        let value = port.to_string();
        match *port {
            22 => add(
                found,
                "ssh",
                "SSH",
                "server",
                TechSource::Port,
                "open_port",
                &value,
                55,
            ),
            3306 => add(
                found,
                "mysql",
                "MySQL",
                "backend",
                TechSource::Port,
                "open_port",
                &value,
                60,
            ),
            5432 => add(
                found,
                "postgresql",
                "PostgreSQL",
                "backend",
                TechSource::Port,
                "open_port",
                &value,
                60,
            ),
            6379 => add(
                found,
                "redis",
                "Redis",
                "backend",
                TechSource::Port,
                "open_port",
                &value,
                60,
            ),
            8000 | 8080 | 8081 | 8888 => add(
                found,
                "http-alt",
                "Alternate HTTP",
                "server",
                TechSource::Port,
                "open_port",
                &value,
                45,
            ),
            8443 => add(
                found,
                "https-alt",
                "Alternate HTTPS",
                "server",
                TechSource::Port,
                "open_port",
                &value,
                45,
            ),
            9200 => add(
                found,
                "elasticsearch",
                "Elasticsearch",
                "backend",
                TechSource::Port,
                "open_port",
                &value,
                60,
            ),
            27017 => add(
                found,
                "mongodb",
                "MongoDB",
                "backend",
                TechSource::Port,
                "open_port",
                &value,
                60,
            ),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add(
    found: &mut BTreeMap<&'static str, Builder>,
    slug: &'static str,
    name: &'static str,
    category: &'static str,
    source: TechSource,
    key: &str,
    value: &str,
    confidence_delta: u8,
) {
    let entry = found.entry(slug).or_insert_with(|| Builder {
        name,
        category,
        evidence: Vec::new(),
    });
    if !entry
        .evidence
        .iter()
        .any(|e| e.source == source && e.key == key && e.value == value)
    {
        entry.evidence.push(TechEvidence {
            source,
            key: key.to_string(),
            value: value.chars().take(256).collect(),
            confidence_delta,
        });
    }
}
