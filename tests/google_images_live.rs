//! Live real-world test: query Google Images for "horse" and extract
//! the first 10 image URLs without tripping the bot wall.
//!
//! **Version 2**: v1 used `page.goto()` for every nav and zero mouse/
//! keyboard input. Google's reCAPTCHA Enterprise rejected within the
//! warm-up sequence because "sending requests very quickly" + score
//! zero-behavioral = auto-fail.
//!
//! This version exercises the behaviour stack outra thread already
//! shipped (WindMouse mouse, log-normal keystrokes, wheel scroll):
//!
//!   1. `page.goto("https://www.google.com/")` — cold nav is OK for
//!      the landing itself.
//!   2. Mouse WALKS to random points in the viewport (2 moves).
//!   3. Scroll wheel 1–2 ticks to register an interaction.
//!   4. Click the search box; type `"horse"` character-by-character
//!      with inter-key jitter.
//!   5. Press Enter.
//!   6. Wait for the web SRP, then click the "Images" tab link (which
//!      originates from same-origin with the right referer).
//!   7. Wait for the image SRP, mouse around, scroll.
//!   8. Extract <img> srcs.
//!
//! `#[ignore]` — run with:
//! ```
//! cargo test --test google_images_live -- --ignored --nocapture
//! ```

#![cfg(feature = "cdp-backend")]

use std::time::Duration;

use crawlex::render::chrome::browser::{Browser, BrowserConfig, HeadlessMode};
use crawlex::render::chrome_protocol::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType,
};
use crawlex::render::chrome_protocol::cdp::browser_protocol::page::NavigateParams;
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

fn detect_block(html: &str, final_url: &str) -> Option<String> {
    if final_url.contains("/sorry/") {
        return Some(format!("redirected to Google sorry page: {final_url}"));
    }
    if html.contains("id=\"captcha-form\"") || html.contains("g-recaptcha") {
        return Some("captcha form present in response DOM".into());
    }
    if html.contains("unusual traffic") {
        return Some("'unusual traffic' text present".into());
    }
    None
}

async fn press_enter(page: &crawlex::render::chrome::page::Page) -> Result<(), String> {
    for ev in [DispatchKeyEventType::KeyDown, DispatchKeyEventType::KeyUp] {
        let p = DispatchKeyEventParams::builder()
            .r#type(ev)
            .key("Enter".to_string())
            .code("Enter".to_string())
            .windows_virtual_key_code(13)
            .native_virtual_key_code(13)
            .build()
            .map_err(|e| format!("build key: {e}"))?;
        page.execute(p)
            .await
            .map_err(|e| format!("dispatch key: {e}"))?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Chromium + network + residential IP; run with --ignored"]
async fn google_images_horse_top_10() {
    let (browser, _tmp) = launch_stealth().await;

    // Stage 1 — cold land on home.
    let page = browser
        .new_page("https://www.google.com/")
        .await
        .expect("new_page google.com");
    // Real-user dwell: 3-5s on landing before first interaction.
    tokio::time::sleep(Duration::from_millis(3500)).await;

    // Handle the cookie consent wall if it appears (EU + BR sometimes).
    // Best-effort; not required in every region.
    let consent_accept = interact::click_selector(
        &page,
        "button#L2AGLb, button[aria-label='Accept all']",
        MousePos { x: 100.0, y: 100.0 },
    )
    .await;
    if consent_accept.is_ok() {
        eprintln!("accepted consent wall");
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    let mut pos = MousePos { x: 200.0, y: 200.0 };

    // Stage 2 — warm the session with mouse/scroll like a real visitor.
    pos = interact::mouse_move_to(&page, pos, 640.0, 320.0)
        .await
        .expect("mouse move 1");
    tokio::time::sleep(Duration::from_millis(700)).await;
    pos = interact::mouse_move_to(&page, pos, 320.0, 420.0)
        .await
        .expect("mouse move 2");
    tokio::time::sleep(Duration::from_millis(500)).await;
    interact::scroll_by(&page, 200.0, pos)
        .await
        .expect("scroll 1");
    tokio::time::sleep(Duration::from_millis(900)).await;
    interact::scroll_by(&page, -150.0, pos)
        .await
        .expect("scroll 2");
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Stage 3 — click the search input. Google ships both a textarea
    // (desktop) and input (mobile/some A/B). Try the textarea first.
    let clicked_search =
        interact::click_selector(&page, "textarea[name='q'], input[name='q']", pos).await;
    match clicked_search {
        Ok(p) => pos = p,
        Err(e) => panic!("failed to click search box: {e}"),
    }
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Stage 4 — type the query with human rhythm (TypingEngine under
    // the hood) and submit via Enter (not button click — real users
    // press Enter).
    interact::dispatch_typing(&page, "horse")
        .await
        .expect("type horse");
    tokio::time::sleep(Duration::from_millis(800)).await;
    press_enter(&page).await.expect("press Enter");

    // Wait for web-SRP to settle.
    tokio::time::sleep(Duration::from_millis(3500)).await;

    let html_web: String = page
        .evaluate("document.documentElement.outerHTML")
        .await
        .expect("read web-SRP outerHTML")
        .into_value()
        .expect("into_value");
    let url_web: String = page
        .evaluate("location.href")
        .await
        .expect("read web-SRP location.href")
        .into_value()
        .expect("into_value");
    eprintln!("web-SRP url   = {}", url_web);
    eprintln!("web-SRP bytes = {}", html_web.len());
    if let Some(why) = detect_block(&html_web, &url_web) {
        let _ = std::fs::write("/tmp/google-web-srp-blocked.html", &html_web);
        panic!("blocked at web-SRP: {why} (dump at /tmp/google-web-srp-blocked.html)");
    }

    // Stage 5 — click "Images" (tab link). href is `/search?q=horse&udm=2`
    // or similar; match by the tab role/label text.
    let to_images = interact::click_selector(
        &page,
        "a[href*='udm=2'], a[href*='tbm=isch'], div[role='listitem'] a[aria-label*='Images']",
        pos,
    )
    .await;
    if let Err(e) = to_images {
        // Fallback: goto the Images URL directly using the query cookie
        // we already earned on the web-SRP.
        eprintln!("Images-tab click failed ({e}); falling back to direct nav");
        let nav = NavigateParams::builder()
            .url("https://www.google.com/search?q=horse&udm=2&hl=en")
            .build()
            .expect("build imgs nav");
        page.execute(nav).await.expect("nav to images");
    } else {
        pos = to_images.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(4000)).await;

    // Stage 6 — settle on image SRP.
    interact::scroll_by(&page, 400.0, pos)
        .await
        .expect("scroll on images srp");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let html: String = page
        .evaluate("document.documentElement.outerHTML")
        .await
        .expect("read image-SRP outerHTML")
        .into_value()
        .expect("into_value");
    let final_url: String = page
        .evaluate("location.href")
        .await
        .expect("read image-SRP location.href")
        .into_value()
        .expect("into_value");
    eprintln!("image-SRP url   = {}", final_url);
    eprintln!("image-SRP bytes = {}", html.len());

    if let Some(why) = detect_block(&html, &final_url) {
        let _ = std::fs::write("/tmp/google-images-blocked.html", &html);
        panic!("Google blocked at image-SRP: {why} (dump at /tmp/google-images-blocked.html)");
    }

    // Extract candidate img URLs.
    let re = regex::Regex::new(r#"<img[^>]+(?:src|data-src)="([^"]+)""#).unwrap();
    let urls: Vec<String> = re
        .captures_iter(&html)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .filter(|u| u.starts_with("http") || u.starts_with("data:image"))
        .collect();

    eprintln!("extracted {} img candidates", urls.len());
    for (i, u) in urls.iter().take(15).enumerate() {
        let short: String = u.chars().take(140).collect();
        eprintln!("  [{i:>2}] {short}");
    }

    if urls.len() < 10 {
        let _ = std::fs::write("/tmp/google-images-debug.html", &html);
    }
    assert!(
        urls.len() >= 10,
        "only {} img candidates — shell-without-tiles or markup change \
         (dump at /tmp/google-images-debug.html)",
        urls.len()
    );

    eprintln!("\n=== SUCCESS: stealth + behaviour stack cleared Google Images ===");
}
