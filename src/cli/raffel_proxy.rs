use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::{Error, Result};

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RaffelProxyOptions {
    pub raffel_path: PathBuf,
    pub host: String,
    pub port: u16,
}

pub struct RaffelProxyHandle {
    child: Option<Child>,
    proxy_url: String,
}

impl RaffelProxyHandle {
    pub fn proxy_url(&self) -> &str {
        &self.proxy_url
    }

    pub async fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await;
    }
}

pub async fn spawn(opts: &RaffelProxyOptions) -> Result<RaffelProxyHandle> {
    ensure_raffel_dist(&opts.raffel_path)?;

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/raffel-proxy.mjs");
    if !script.exists() {
        return Err(Error::Config(format!(
            "raffel launcher script missing: {}",
            script.display()
        )));
    }

    let mut child = Command::new("node")
        .arg(&script)
        .arg("--raffel-path")
        .arg(&opts.raffel_path)
        .arg("--host")
        .arg(&opts.host)
        .arg("--port")
        .arg(opts.port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Error::Config(format!("spawn raffel proxy: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Config("raffel proxy stdout not captured".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(Error::Config(format!(
                "raffel proxy did not become ready within {}s",
                READY_TIMEOUT.as_secs()
            )));
        }
        let remaining = deadline.saturating_duration_since(now);
        tokio::select! {
            line = lines.next_line() => {
                match line.map_err(Error::Io)? {
                    Some(line) => {
                        let line = line.trim();
                        if let Some(url) = line.strip_prefix("READY ") {
                            return Ok(RaffelProxyHandle {
                                child: Some(child),
                                proxy_url: url.trim().to_string(),
                            });
                        }
                        tracing::info!(line, "raffel proxy");
                    }
                    None => {
                        let status = child
                            .wait()
                            .await
                            .map_err(|e| Error::Config(format!("wait raffel proxy: {e}")))?;
                        return Err(Error::Config(format!(
                            "raffel proxy exited before ready: {status}"
                        )));
                    }
                }
            }
            status = child.wait() => {
                let status = status.map_err(|e| Error::Config(format!("wait raffel proxy: {e}")))?;
                return Err(Error::Config(format!(
                    "raffel proxy exited before ready: {status}"
                )));
            }
            _ = tokio::time::sleep(remaining.min(Duration::from_millis(200))) => {}
        }
    }
}

fn ensure_raffel_dist(path: &Path) -> Result<()> {
    let entry = path.join("dist/index.js");
    if !entry.exists() {
        return Err(Error::Config(format!(
            "raffel dist not found at {} (run its build first)",
            entry.display()
        )));
    }
    Ok(())
}
