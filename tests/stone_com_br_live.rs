//! Live test against stone.com.br — Brazilian fintech home page.
//! Stack: Cloudflare edge + Kong gateway + Deno Deploy (deco.cx CMS).
//!
//! Fintech surfaces often ship tighter CF configs than marketing
//! pages: stricter WAF rules, faster Turnstile escalation, JS
//! challenges on suspect fingerprints. This test validates the
//! crawlex stealth stack against that tighter posture.
//!
//! `#[ignore]` — run with:
//! ```
//! cargo test --test stone_com_br_live -- --ignored --nocapture
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

fn detect_cf_block(title: &str, html: &str) -> Option<String> {
    if title.contains("Just a moment") || title.contains("Attention Required") {
        return Some(format!("title {title:?} → CF challenge"));
    }
    if html.contains("challenges.cloudflare.com") {
        return Some("page embeds challenges.cloudflare.com".into());
    }
    if html.contains("cf_chl_opt") {
        return Some("cf_chl_opt present".into());
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Chromium + network; run with --ignored"]
async fn stone_com_br_front_page() {
    let (browser, _tmp) = launch_stealth().await;

    let page = browser
        .new_page("https://www.stone.com.br/")
        .await
        .expect("new_page stone.com.br");
    tokio::time::sleep(Duration::from_millis(3500)).await;

    // Minimal human touch keeps us in step with tetis_io_live.rs and
    // google_images_live.rs — same behaviour stack, different targets.
    let mut pos = MousePos { x: 300.0, y: 300.0 };
    pos = interact::mouse_move_to(&page, pos, 600.0, 500.0)
        .await
        .expect("mouse move");
    tokio::time::sleep(Duration::from_millis(500)).await;
    interact::scroll_by(&page, 600.0, pos)
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
            "Array.from(document.querySelectorAll('h1,h2')).slice(0,4) \
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
        let _ = std::fs::write("/tmp/stone-blocked.html", &html);
        panic!("Cloudflare blocked: {why} (dump at /tmp/stone-blocked.html)");
    }

    let links: Vec<String> = page
        .evaluate(
            "Array.from(document.querySelectorAll('a[href]')) \
             .map(a => a.href) \
             .filter(h => h && !h.startsWith('javascript:')) \
             .slice(0, 30)",
        )
        .await
        .expect("read links")
        .into_value()
        .expect("into_value");

    eprintln!("\nlinks ({}):", links.len());
    for (i, l) in links.iter().take(20).enumerate() {
        let short: String = l.chars().take(120).collect();
        eprintln!("  [{i:>2}] {short}");
    }

    assert!(
        html.len() > 10_000,
        "stone.com.br front should render substantial HTML; got {} bytes",
        html.len()
    );
    assert!(!title.is_empty(), "title must be non-empty");
    assert!(
        links.len() >= 5,
        "at least 5 links expected, got {}",
        links.len()
    );

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

    eprintln!("\n=== SUCCESS: stone.com.br front page fully rendered ===");
}
