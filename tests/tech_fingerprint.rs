use crawlex::discovery::cert::PeerCert;
use crawlex::discovery::tech_fingerprint::{analyze, analyze_with_facts, TechFingerprintFacts};
use http::{HeaderMap, HeaderValue};
use url::Url;

#[test]
fn detects_common_header_and_dom_technologies() {
    let url = Url::parse("https://shop.example/").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("server", HeaderValue::from_static("cloudflare"));
    headers.insert("cf-ray", HeaderValue::from_static("abc-SFO"));
    headers.insert("x-powered-by", HeaderValue::from_static("Express"));
    headers.insert("set-cookie", HeaderValue::from_static("__cf_bm=1; Path=/"));
    let html = r#"<!doctype html>
        <html><head>
          <meta name="generator" content="WordPress 6.5">
          <script src="/_next/static/app.js"></script>
          <script src="https://www.googletagmanager.com/gtm.js?id=GTM-X"></script>
        </head><body><div id="__NEXT_DATA__"></div></body></html>"#;

    let report = analyze(&url, &url, Some(&headers), Some(html));
    let slugs: Vec<_> = report
        .technologies
        .iter()
        .map(|t| t.slug.as_str())
        .collect();

    assert!(slugs.contains(&"cloudflare"));
    assert!(slugs.contains(&"express"));
    assert!(slugs.contains(&"wordpress"));
    assert!(slugs.contains(&"nextjs"));
    assert!(slugs.contains(&"google-tag-manager"));
    assert!(report
        .technologies
        .iter()
        .all(|t| t.confidence > 0 && !t.evidence.is_empty()));
}

#[test]
fn detects_required_framework_and_ecommerce_signatures() {
    let url = Url::parse("https://app.example/").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-powered-by", HeaderValue::from_static("Laravel"));
    headers.insert(
        "set-cookie",
        HeaderValue::from_static("_rails_session=1; csrftoken=2; woocommerce_cart_hash=3"),
    );
    let html = r#"<!doctype html>
        <html data-v-app data-server-rendered="true">
          <head>
            <meta name="generator" content="WooCommerce">
            <meta name="csrf-token" content="abc">
            <script type="module" src="/@vite/client"></script>
            <script src="/assets/rails-ujs.js"></script>
            <script src="/wp-content/plugins/woocommerce/assets/js/frontend/cart-fragments.js"></script>
            <script src="/static/version123/frontend/Magento/luma/en_US/mage/cookies.js"></script>
          </head>
          <body>
            <input type="hidden" name="csrfmiddlewaretoken" value="x">
            <div data-v-abc123></div>
          </body>
        </html>"#;

    let report = analyze(&url, &url, Some(&headers), Some(html));
    let slugs: Vec<_> = report
        .technologies
        .iter()
        .map(|t| t.slug.as_str())
        .collect();

    for expected in [
        "laravel",
        "rails",
        "django",
        "woocommerce",
        "magento",
        "vite",
        "vue",
    ] {
        assert!(slugs.contains(&expected), "missing {expected}: {slugs:?}");
    }
    assert!(report
        .technologies
        .iter()
        .filter(|t| ["rails", "woocommerce", "vite"].contains(&t.slug.as_str()))
        .all(|t| t.confidence >= 80));
}

#[test]
fn detects_infra_facts_from_dns_tls_ports_and_pwa_markers() {
    let url = Url::parse("https://infra.example/").unwrap();
    let cert = PeerCert {
        issuer_cn: Some("Cloudflare Inc ECC".to_string()),
        subject_cn: Some("sni.cloudflaressl.com".to_string()),
        sans: vec!["infra.example".to_string(), "*.myshopify.com".to_string()],
        ..PeerCert::default()
    };
    let dns_json = serde_json::json!({
        "cname": ["infra.global.ssl.fastly.net"],
        "ns": ["ns1.netlifydns.com"],
        "txt": ["v=spf1 include:_spf.google.com"],
        "cloud": ["aws:cloudfront"],
    })
    .to_string();

    let report = analyze_with_facts(
        &url,
        &url,
        None,
        None,
        TechFingerprintFacts {
            peer_cert: Some(&cert),
            dns_json: Some(&dns_json),
            open_ports: &[22, 5432],
            manifest_present: true,
            service_worker_present: true,
        },
    );
    let slugs: Vec<_> = report
        .technologies
        .iter()
        .map(|t| t.slug.as_str())
        .collect();

    for expected in [
        "cloudflare",
        "fastly",
        "netlify",
        "cloudfront",
        "shopify",
        "ssh",
        "postgresql",
        "web-app-manifest",
        "service-worker",
    ] {
        assert!(slugs.contains(&expected), "missing {expected}: {slugs:?}");
    }
}
