use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use url::Url;

use crate::config::FallbackFetchConfig;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct FallbackFetchRequest {
    pub crawl_id: u64,
    pub url: Url,
    pub attempt_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Url>,
}

#[derive(Debug, Clone)]
pub struct FallbackFetchResult {
    pub final_url: Url,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug, Deserialize)]
struct CommandResponse {
    #[serde(default)]
    final_url: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

pub async fn run(
    config: &FallbackFetchConfig,
    request: &FallbackFetchRequest,
) -> Result<FallbackFetchResult> {
    let Some(program) = config.command.first() else {
        return Err(Error::Config(
            "fallback_fetch.command must include a program".into(),
        ));
    };
    let payload = serde_json::to_vec(request)
        .map_err(|e| Error::Config(format!("fallback fetch request JSON: {e}")))?;
    let mut cmd = Command::new(program);
    cmd.args(config.command.iter().skip(1));
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| Error::EngineFailed {
        engine: crate::error::Engine::FallbackFetch,
        reason: format!("fallback spawn `{program}`: {e}"),
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&payload)
            .await
            .map_err(|e| Error::EngineFailed {
                engine: crate::error::Engine::FallbackFetch,
                reason: format!("fallback stdin: {e}"),
            })?;
    }
    let timeout = Duration::from_millis(config.timeout_ms.max(1));
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| Error::RequestTimeout {
            timeout_ms: timeout.as_millis(),
        })?
        .map_err(|e| Error::EngineFailed {
            engine: crate::error::Engine::FallbackFetch,
            reason: format!("fallback wait: {e}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::EngineFailed {
            engine: crate::error::Engine::FallbackFetch,
            reason: format!(
                "fallback exited with status {}: {}",
                output.status,
                stderr.trim()
            ),
        });
    }
    if output.stdout.len() as u64 > config.max_output_bytes {
        return Err(Error::BodyTooLarge {
            limit: config.max_output_bytes as usize,
        });
    }
    let decoded: CommandResponse =
        serde_json::from_slice(&output.stdout).map_err(|e| Error::EngineFailed {
            engine: crate::error::Engine::FallbackFetch,
            reason: format!("fallback response JSON: {e}"),
        })?;
    let final_url = decoded
        .final_url
        .as_deref()
        .map(Url::parse)
        .transpose()
        .map_err(Error::UrlParse)?
        .unwrap_or_else(|| request.url.clone());
    let body = decoded
        .html
        .or(decoded.body)
        .ok_or_else(|| Error::EngineFailed {
            engine: crate::error::Engine::FallbackFetch,
            reason: "fallback response missing `html` or `body`".into(),
        })?;
    let mut headers = HeaderMap::new();
    for (name, value) in decoded.headers {
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            continue;
        };
        headers.insert(name, value);
    }
    if !headers.contains_key("content-type") {
        headers.insert(
            "content-type",
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
    }
    Ok(FallbackFetchResult {
        final_url,
        status: decoded.status.unwrap_or(200),
        headers,
        body: Bytes::from(body),
    })
}
