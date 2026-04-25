//! Full `crawlex` binary: HTTP spoof + Chromium render engines enabled.
//!
//! Requires `cli`, `sqlite`, `cdp-backend` features (all default).
//! Ships with `chromium-fetcher` so a fresh system downloads a pinned
//! Chromium-for-Testing on first run — no external Chrome dependency.

#[cfg(all(feature = "cli", feature = "cdp-backend"))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    crawlex::cli::run().await
}

#[cfg(not(all(feature = "cli", feature = "cdp-backend")))]
fn main() {
    eprintln!(
        "crawlex (full) requires the `cli` and `cdp-backend` features; \
         rebuild with default features or use `crawlex-mini` for HTTP-only."
    );
    std::process::exit(2);
}
