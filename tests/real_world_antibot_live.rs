//! Real-world antibot/fingerprint validation. `#[ignore]` — network + Chrome.
//!
//! ```
//! cargo test --all-features --test real_world_antibot_live -- --ignored --nocapture
//! ```
//!
//! Probes 8 well-known antibot / fingerprint test pages, captures final
//! HTML + screenshot + challenge signal, and writes a verdict table to
//! `production-validation/real_world_report.md`. This is not a CI test —
//! it exists to produce an honest per-site pass/fail picture, not to
//! gate builds. Per site we do **one** request (rate-limited with a 3s
//! sleep between sites) to stay polite to third parties.
//!
//! Why the per-site criteria are bespoke: these pages don't speak a
//! common protocol. A generic "html contains target" assert would be
//! both brittle and dishonest, and the point of the task is to know
//! *what* our stealth actually bypasses on each check.

#![cfg(feature = "cdp-backend")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crawlex::config::Config;
use crawlex::render::pool::RenderPool;
use crawlex::render::Renderer;
use crawlex::render::WaitStrategy;
use crawlex::storage::Storage;

#[derive(Debug)]
struct SiteVerdict {
    host: String,
    #[allow(dead_code)]
    url: String,
    // "pass" | "partial" | "fail" | "unreachable"
    verdict: &'static str,
    http_status: Option<u16>,
    challenge: Option<String>,
    content_check: String,
    screenshot_bytes: usize,
    final_url: String,
    timing_ms: u128,
    notes: String,
}

fn host_of(u: &str) -> String {
    url::Url::parse(u)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Per-site content verdict. Returns (verdict, content_check_label, notes).
/// `verdict` is one of "pass", "partial", "fail".
fn evaluate(
    url: &str,
    html: &str,
    challenge: &Option<String>,
    final_url: &str,
) -> (&'static str, String, String) {
    let h = host_of(url);
    match h.as_str() {
        "nowsecure.nl" => {
            // CF JS challenge. Success = no /cdn-cgi/challenge in final URL
            // and body has some signal we got through (nowsecure's real
            // page says "OH YEAH" when passed).
            let on_cf = final_url.contains("/cdn-cgi/challenge")
                || final_url.contains("__cf_chl_")
                || html.contains("Just a moment")
                || html.contains("challenge-platform");
            if !on_cf && challenge.is_none() {
                let hit = html.contains("OH YEAH") || html.contains("nowSecure");
                let label = if hit {
                    "cf bypassed + content".into()
                } else {
                    "cf not triggered".into()
                };
                ("pass", label, "Cloudflare JS challenge not engaged".into())
            } else {
                (
                    "fail",
                    "cf challenge page".into(),
                    "Stuck on Cloudflare interstitial".into(),
                )
            }
        }
        "antoinevastel.com" => {
            // FP tester. Stable fragments: "Fingerprint" header, UA row.
            let hit = html.contains("Fingerprint") || html.contains("fingerprint");
            if hit {
                (
                    "pass",
                    "fp page rendered".into(),
                    "Vastel FP page loaded".into(),
                )
            } else {
                (
                    "fail",
                    "fp page missing markers".into(),
                    "Page did not render FP content".into(),
                )
            }
        }
        "arh.antoinevastel.com" => {
            // AreYouHeadless. Page shows "You are <not> Chrome headless".
            let hit =
                html.contains("You are") || html.contains("you are") || html.contains("headless");
            let not_headless = html.contains("You are not Chrome headless")
                || html.contains("not Chrome headless");
            if not_headless {
                (
                    "pass",
                    "declared not-headless".into(),
                    "AreYouHeadless says not-headless".into(),
                )
            } else if hit {
                (
                    "partial",
                    "page loaded, headless flag set".into(),
                    "Detector ran; classified as headless".into(),
                )
            } else {
                (
                    "fail",
                    "detector marker missing".into(),
                    "Detector text not found".into(),
                )
            }
        }
        "bot.sannysoft.com" => {
            // Sannysoft multi-check table. Cells contain "failed" or "passed".
            let failed = html.matches("failed").count();
            let passed = html.matches("passed").count();
            let label = format!("{}/{} pass/fail", passed, failed);
            if passed > 0 && failed == 0 {
                (
                    "pass",
                    label,
                    "All Sannysoft checks passed (text count)".into(),
                )
            } else if passed >= failed {
                (
                    "partial",
                    label,
                    format!("Sannysoft: {} failed / {} passed", failed, passed),
                )
            } else {
                (
                    "fail",
                    label,
                    format!("Sannysoft failures dominate: {}>{}", failed, passed),
                )
            }
        }
        "abrahamjuliot.github.io" => {
            // CreepJS. Score text like "trust score: 70%" or "Fingerprint".
            // Extract percent via regex.
            let re = regex::Regex::new(r"([0-9]{1,3})%").ok();
            let scored = re
                .as_ref()
                .and_then(|r| r.captures_iter(html).next())
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse::<u32>().ok());
            match scored {
                Some(pct) if pct >= 50 => (
                    "pass",
                    format!("trust {}%", pct),
                    "CreepJS reported >=50% trust".into(),
                ),
                Some(pct) => (
                    "partial",
                    format!("trust {}%", pct),
                    "CreepJS reported low trust".into(),
                ),
                None => {
                    let loaded =
                        html.contains("creep") || html.contains("CreepJS") || html.contains("FP");
                    if loaded {
                        (
                            "partial",
                            "no score extracted".into(),
                            "CreepJS page loaded but score not parsed".into(),
                        )
                    } else {
                        ("fail", "page empty".into(), "CreepJS did not render".into())
                    }
                }
            }
        }
        "browserleaks.com" => {
            // Two different pages under same host; branch on path.
            if url.contains("/canvas") {
                let has_hash = html.contains("Signature") || html.contains("hash");
                if has_hash {
                    (
                        "pass",
                        "canvas page rendered".into(),
                        "Canvas signature present".into(),
                    )
                } else {
                    (
                        "partial",
                        "canvas markers missing".into(),
                        "Canvas page loaded without explicit signature marker".into(),
                    )
                }
            } else if url.contains("/webrtc") {
                let has_rtc =
                    html.contains("WebRTC") || html.contains("Local IP") || html.contains("rtc");
                if has_rtc {
                    (
                        "pass",
                        "webrtc page rendered".into(),
                        "WebRTC page loaded".into(),
                    )
                } else {
                    (
                        "partial",
                        "webrtc markers missing".into(),
                        "WebRTC page text fragments not matched".into(),
                    )
                }
            } else {
                ("partial", "unknown browserleaks path".into(), String::new())
            }
        }
        "pixelscan.net" => {
            // Coherence check. Usually triggers its own antibot (ironic),
            // look for "coherent" or "score".
            let hit =
                html.contains("coherent") || html.contains("Coherence") || html.contains("score");
            if hit {
                (
                    "pass",
                    "coherence markers present".into(),
                    "Pixelscan reported".into(),
                )
            } else {
                (
                    "partial",
                    "page without markers".into(),
                    "Pixelscan probably antibot-blocked".into(),
                )
            }
        }
        _ => (
            "partial",
            "no evaluator".into(),
            "No per-site rule, treating as partial".into(),
        ),
    }
}

#[tokio::test]
#[ignore = "requires Chromium + network + third-party sites; run with --ignored"]
async fn real_world_antibot_suite() {
    let sites: Vec<&str> = vec![
        "https://nowsecure.nl/",
        "https://antoinevastel.com/bots/",
        "https://arh.antoinevastel.com/bots/areyouheadless",
        "https://bot.sannysoft.com/",
        "https://abrahamjuliot.github.io/creepjs/",
        "https://browserleaks.com/canvas",
        "https://browserleaks.com/webrtc",
        "https://pixelscan.net/",
    ];

    let system_chrome = [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ]
    .iter()
    .find(|p| std::path::Path::new(p).exists())
    .map(|s| s.to_string());

    let tmp = tempfile::tempdir().expect("tmpdir");
    let cfg = Config {
        max_concurrent_render: 1,
        auto_fetch_chromium: system_chrome.is_none(),
        chrome_path: system_chrome.clone(),
        motion_profile: crawlex::render::motion::MotionProfile::Balanced,
        output: crawlex::config::OutputConfig {
            screenshot_mode: Some("fullpage".into()),
            ..Default::default()
        },
        ..Config::default()
    };
    cfg.motion_profile.set_active();
    let storage: Arc<dyn Storage> = Arc::new(
        crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).expect("fs storage"),
    );
    let pool = RenderPool::new(Arc::new(cfg), storage);

    // Screenshot output dir — sibling to Cargo.toml.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shots_dir = manifest_dir.join("production-validation/screenshots");
    std::fs::create_dir_all(&shots_dir).expect("create screenshots dir");

    let mut verdicts: Vec<SiteVerdict> = Vec::new();

    for (idx, site) in sites.iter().enumerate() {
        if idx > 0 {
            // Politeness — 3s between sites.
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        let host = host_of(site);
        eprintln!("\n=== [{}/{}] {} ===", idx + 1, sites.len(), site);
        let url = match url::Url::parse(site) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("  bad url: {e}");
                continue;
            }
        };
        let wait = WaitStrategy::NetworkIdle { idle_ms: 1_500 };
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(75),
            pool.render(&url, &wait, false, true, None, None),
        )
        .await;
        let elapsed = started.elapsed().as_millis();

        let verdict = match result {
            Ok(Ok(rendered)) => {
                let challenge_str = rendered
                    .challenge
                    .as_ref()
                    .map(|c| format!("{:?}/{:?}", c.vendor, c.level));
                let (v, label, notes) = evaluate(
                    site,
                    &rendered.html_post_js,
                    &challenge_str,
                    rendered.final_url.as_str(),
                );
                let png_bytes = rendered
                    .screenshot_png
                    .as_deref()
                    .map(|p| p.len())
                    .unwrap_or(0);
                if let Some(png) = rendered.screenshot_png.as_ref() {
                    let path = shots_dir.join(format!("{}.png", host.replace('.', "_")));
                    let _ = std::fs::write(&path, png);
                    eprintln!("  screenshot: {} bytes -> {}", png.len(), path.display());
                }
                eprintln!(
                    "  status={} final={} challenge={:?} verdict={} ({}ms)",
                    rendered.status, rendered.final_url, challenge_str, v, elapsed
                );
                SiteVerdict {
                    host: host.clone(),
                    url: site.to_string(),
                    verdict: v,
                    http_status: Some(rendered.status),
                    challenge: challenge_str,
                    content_check: label,
                    screenshot_bytes: png_bytes,
                    final_url: rendered.final_url.to_string(),
                    timing_ms: elapsed,
                    notes,
                }
            }
            Ok(Err(e)) => {
                eprintln!("  render error: {e}");
                SiteVerdict {
                    host: host.clone(),
                    url: site.to_string(),
                    verdict: "unreachable",
                    http_status: None,
                    challenge: None,
                    content_check: "n/a".into(),
                    screenshot_bytes: 0,
                    final_url: site.to_string(),
                    timing_ms: elapsed,
                    notes: format!("render error: {}", truncate(&e.to_string(), 160)),
                }
            }
            Err(_) => {
                eprintln!("  timed out after 75s");
                SiteVerdict {
                    host: host.clone(),
                    url: site.to_string(),
                    verdict: "unreachable",
                    http_status: None,
                    challenge: None,
                    content_check: "n/a".into(),
                    screenshot_bytes: 0,
                    final_url: site.to_string(),
                    timing_ms: elapsed,
                    notes: "timeout 75s".into(),
                }
            }
        };
        verdicts.push(verdict);
    }

    // Write report.
    let report = render_report(&verdicts);
    let report_path = manifest_dir.join("production-validation/real_world_report.md");
    std::fs::write(&report_path, &report).expect("write report");
    eprintln!("\nreport -> {}", report_path.display());

    // Summary row.
    let summary_path = manifest_dir.join("production-validation/summary.md");
    let summary_row = render_summary(&verdicts);
    let summary = if summary_path.exists() {
        let existing = std::fs::read_to_string(&summary_path).unwrap_or_default();
        if existing.contains("| A.1 |") {
            existing
        } else {
            format!("{}\n{}\n", existing.trim_end(), summary_row)
        }
    } else {
        format!(
            "# Production validation — claim → evidence → verdict\n\n\
             | ID | Claim | Evidence | Verdict |\n\
             |----|-------|----------|---------|\n\
             {}\n",
            summary_row
        )
    };
    std::fs::write(&summary_path, summary).expect("write summary");
    eprintln!("summary -> {}", summary_path.display());

    // Honest aggregate log.
    let pass = verdicts.iter().filter(|v| v.verdict == "pass").count();
    let partial = verdicts.iter().filter(|v| v.verdict == "partial").count();
    let fail = verdicts.iter().filter(|v| v.verdict == "fail").count();
    let unreachable = verdicts
        .iter()
        .filter(|v| v.verdict == "unreachable")
        .count();
    eprintln!(
        "\nTOTALS pass={} partial={} fail={} unreachable={} of {}",
        pass,
        partial,
        fail,
        unreachable,
        verdicts.len()
    );

    // The test itself passes as long as we produced a report. Honest
    // numbers may include fails — that's the point of the task.
    assert!(!verdicts.is_empty(), "no sites probed");
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "..."
    }
}

fn render_report(verdicts: &[SiteVerdict]) -> String {
    let mut out = String::new();
    out.push_str("# Real-world antibot validation report\n\n");
    out.push_str(
        "Generated by `cargo test --all-features --test real_world_antibot_live -- --ignored`.\n\n",
    );
    out.push_str(
        "| # | Site | Verdict | HTTP | Challenge | Content | Screenshot | Time | Final URL |\n",
    );
    out.push_str(
        "|---|------|---------|------|-----------|---------|------------|------|-----------|\n",
    );
    for (i, v) in verdicts.iter().enumerate() {
        let shot = if v.screenshot_bytes > 0 {
            format!("{} KB", v.screenshot_bytes / 1024)
        } else {
            "—".to_string()
        };
        let http = v
            .http_status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".into());
        let ch = v.challenge.clone().unwrap_or_else(|| "none".into());
        out.push_str(&format!(
            "| {} | `{}` | **{}** | {} | {} | {} | {} | {}ms | `{}` |\n",
            i + 1,
            v.host,
            v.verdict,
            http,
            ch,
            v.content_check,
            shot,
            v.timing_ms,
            truncate(&v.final_url, 80)
        ));
    }
    out.push_str("\n## Per-site notes\n\n");
    for v in verdicts {
        out.push_str(&format!("- **{}** — {}: {}\n", v.host, v.verdict, v.notes));
    }
    out.push_str("\n## Legend\n\n");
    out.push_str("- **pass** — bypassed detector / loaded target content.\n");
    out.push_str("- **partial** — page loaded but detector flagged us or markers absent.\n");
    out.push_str("- **fail** — detector blocked us or content missing.\n");
    out.push_str(
        "- **unreachable** — network/render error; site may be down, NOT a stealth regression.\n\n",
    );
    out.push_str("Screenshots: `production-validation/screenshots/<host>.png` (dots replaced with underscores).\n");
    out
}

fn render_summary(verdicts: &[SiteVerdict]) -> String {
    let pass = verdicts.iter().filter(|v| v.verdict == "pass").count();
    let partial = verdicts.iter().filter(|v| v.verdict == "partial").count();
    let fail = verdicts.iter().filter(|v| v.verdict == "fail").count();
    let unreachable = verdicts
        .iter()
        .filter(|v| v.verdict == "unreachable")
        .count();
    let total = verdicts.len();
    let evidence = format!(
        "{} pass / {} partial / {} fail / {} unreachable of {}",
        pass, partial, fail, unreachable, total
    );
    let verdict = if fail == 0 && unreachable == 0 && partial == 0 {
        "pass"
    } else if pass + partial >= total - unreachable && fail == 0 {
        "partial"
    } else if pass == 0 {
        "fail"
    } else {
        "partial"
    };
    format!(
        "| A.1 | Super browser bypasses real antibot/FP pages | {} (see real_world_report.md) | {} |",
        evidence, verdict
    )
}
