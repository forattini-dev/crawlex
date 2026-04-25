//! `crawlex-mini` — HTTP-only worker.
//!
//! Built with `--no-default-features --features cli,sqlite`: no
//! `CDP client` dependency, no bundled Chromium download. Same CLI
//! surface as `crawlex` full: browser-dependent subcommands/flags parse
//! normally but return `Error::RenderDisabled` at runtime with a stable
//! message operators can match on.

#[cfg(feature = "cli")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    crawlex::cli::run().await
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("crawlex-mini requires the `cli` feature");
    std::process::exit(2);
}
