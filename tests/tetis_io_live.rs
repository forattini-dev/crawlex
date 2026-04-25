//! Live test against tetis.io — a Next.js site behind Cloudflare with
//! prerendered edge-cached content. The goal is empirical validation
//! that the crawlex stealth stack cleanly handles a "normal" target
//! (not Google-scale adversarial), end-to-end with real Chrome.
//!
//! Acceptance: home page loads, we can extract `<title>`, at least a
//! handful of links, and we do NOT see a Cloudflare "Just a moment"
//! / Turnstile wall. If Cloudflare ever dials up bot-fighting mode
//! on this origin, this test will catch it immediately.
//!
//! `#[ignore]` — run with:
//! ```
//! cargo test --test tetis_io_live -- --ignored --nocapture
//! ```

#![cfg(feature = "cdp-backend")]

use std::time::Duration;

use crawlex::render::chrome::browser::{Browser, BrowserConfig, HeadlessMode};
use crawlex::render::interact::{self, MousePos};
use futures::StreamExt;

fn system_chrome() -> Option<String> {
    for path in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ] {
        if std::path::Path::new(path).exists() {
            return Some(path.into());
        }
    }
    None
}

async fn launch_stealth() -> (Browser, tempfile::TempDir) {
    let exec = system_chrome().expect("requires system Chrome installed");
    let tmp = tempfile::tempdir().expect("tmp user_data_dir");
    let cfg = BrowserConfig::builder()
        .chrome_executable(exec)
        .headless_mode(HeadlessMode::New)
        .no_sandbox()
        .user_data_dir(tmp.path())
        .stealth_runtime_enable_skip(true)
        .hide()
        .build()
        .expect("build browser config");
    let (browser, mut handler) = Browser::launch(cfg).await.expect("launch browser");
    tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
    (browser, tmp)
}

/// Detect Cloudflare JS challenge / Turnstile walls on the rendered
/// page. Common markers across CF variants:
///   * Title contains "Just a moment"
///   * An iframe or script from `challenges.cloudflare.com`
///   * `cf_chl_opt` global present
fn detect_cf_block(title: &str, html: &str) -> Option<String> {
    if title.contains("Just a moment") {
        return Some(format!("title says {title:?} → CF JS challenge"));
    }
    if html.contains("challenges.cloudflare.com") {
        return Some("page embeds challenges.cloudflare.com (Turnstile/managed)".into());
    }
    if html.contains("cf_chl_opt") {
        return Some("page defines cf_chl_opt → CF interactive challenge".into());
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Chromium + network; run with --ignored"]
async fn tetis_io_front_page() {
    let (browser, _tmp) = launch_stealth().await;

    let page = browser
        .new_page("https://tetis.io/")
        .await
        .expect("new_page tetis.io");
    tokio::time::sleep(Duration::from_millis(3000)).await;

    // Minimal human touch: mouse wander + scroll a bit. Cheap, and
    // nothing here strictly requires it, but keeps the path uniform
    // with the Google test so we exercise the same behaviour stack.
    let mut pos = MousePos { x: 300.0, y: 300.0 };
    pos = interact::mouse_move_to(&page, pos, 600.0, 500.0)
        .await
        .expect("mouse move");
    tokio::time::sleep(Duration::from_millis(500)).await;
    interact::scroll_by(&page, 500.0, pos)
        .await
        .expect("scroll");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let title: String = page
        .evaluate("document.title")
        .await
        .expect("read title")
        .into_value()
        .expect("into_value");
    let html: String = page
        .evaluate("document.documentElement.outerHTML")
        .await
        .expect("read html")
        .into_value()
        .expect("into_value");
    let final_url: String = page
        .evaluate("location.href")
        .await
        .expect("read location")
        .into_value()
        .expect("into_value");
    let h1_text: String = page
        .evaluate(
            "Array.from(document.querySelectorAll('h1,h2')).slice(0,3) \
             .map(e => e.innerText.trim()).filter(Boolean).join(' | ')",
        )
        .await
        .expect("read h1")
        .into_value()
        .expect("into_value");

    eprintln!("final_url = {final_url}");
    eprintln!("title     = {title}");
    eprintln!("html len  = {}", html.len());
    eprintln!("h1/h2     = {h1_text}");

    if let Some(why) = detect_cf_block(&title, &html) {
        let _ = std::fs::write("/tmp/tetis-io-blocked.html", &html);
        panic!("Cloudflare blocked: {why} (dump at /tmp/tetis-io-blocked.html)");
    }

    // Extract meaningful content: links + images.
    let links: Vec<String> = {
        let js = "Array.from(document.querySelectorAll('a[href]')) \
                  .map(a => a.href) \
                  .filter(h => h && !h.startsWith('javascript:')) \
                  .slice(0, 25)";
        page.evaluate(js)
            .await
            .expect("read links")
            .into_value()
            .expect("into_value Vec<String>")
    };

    eprintln!("\nlinks ({}):", links.len());
    for (i, l) in links.iter().take(15).enumerate() {
        eprintln!("  [{i:>2}] {l}");
    }

    assert!(
        html.len() > 5_000,
        "tetis.io front page should be substantial; got only {} bytes",
        html.len()
    );
    assert!(!title.is_empty(), "title must be non-empty");
    assert!(!links.is_empty(), "at least one link expected");

    // Cloudflare observability: the test could also walk cookies and
    // log `__cf_bm` / `cf_clearance` status so the operator sees the
    // antibot fabric we're preserving.
    let cookies: String = page
        .evaluate("document.cookie")
        .await
        .expect("read cookies")
        .into_value()
        .expect("into_value");
    eprintln!(
        "\nvisible cookies (non-httpOnly): {}",
        if cookies.is_empty() {
            "<none>"
        } else {
            &cookies
        }
    );

    eprintln!("\n=== SUCCESS: tetis.io front page fully rendered ===");
}
