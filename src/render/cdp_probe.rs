//! Preflight check for an externally-managed CDP endpoint. Slice 30:
//! the `cdp` provider must surface unreachable / incompatible
//! endpoints **before** any target work begins, instead of failing at
//! the first job's WebSocket connect with a generic transport error.

use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of a successful probe: the WebSocket URL advertised by
/// the CDP host. Callers may log it; chromiumoxide rediscovers it
/// independently when it opens the actual session.
#[derive(Debug, Clone)]
pub struct ProbeOk {
    pub web_socket_debugger_url: String,
    pub browser: String,
}

#[derive(serde::Deserialize)]
struct VersionPayload {
    #[serde(default, rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: String,
    #[serde(default, rename = "Browser")]
    browser: String,
}

/// Probe an external CDP endpoint by fetching `/json/version`. Returns
/// a human-readable string on failure suitable for direct CLI surfacing.
///
/// Accepts `http(s)://` and `ws(s)://` schemes. Pure-WebSocket URLs are
/// rewritten to `http(s)://` for the probe — every Chromium-compatible
/// CDP host exposes the JSON endpoint on the same authority.
pub async fn probe(endpoint: &str) -> std::result::Result<ProbeOk, String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err("external CDP url is empty".to_string());
    }
    let probe_url = version_url_for(trimmed).map_err(|e| {
        format!("external CDP url is not a valid endpoint (`{trimmed}`): {e}")
    })?;
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build CDP probe HTTP client: {e}"))?;
    let resp = client
        .get(&probe_url)
        .header("content-type", "application/json")
        .send()
        .await
        .map_err(|e| {
            format!(
                "external CDP endpoint unreachable at `{probe_url}`: {e} — \
                 verify the browser is running and accepts remote CDP connections"
            )
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "external CDP endpoint at `{probe_url}` returned HTTP {status}; \
             expected the Chromium `/json/version` JSON payload — check the \
             endpoint URL and that DevTools is exposed on this host"
        ));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("external CDP endpoint at `{probe_url}` returned no body: {e}"))?;
    let parsed: VersionPayload = serde_json::from_slice(&body).map_err(|e| {
        format!(
            "external CDP endpoint at `{probe_url}` returned a non-CDP response \
             (could not parse `/json/version`: {e}) — endpoint looks incompatible \
             with the Chromium DevTools Protocol"
        )
    })?;
    if parsed.web_socket_debugger_url.is_empty() {
        return Err(format!(
            "external CDP endpoint at `{probe_url}` did not advertise a \
             `webSocketDebuggerUrl` — endpoint looks incompatible with the \
             Chromium DevTools Protocol"
        ));
    }
    Ok(ProbeOk {
        web_socket_debugger_url: parsed.web_socket_debugger_url,
        browser: parsed.browser,
    })
}

fn version_url_for(endpoint: &str) -> std::result::Result<String, String> {
    let base = if let Some(rest) = endpoint.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = endpoint.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        return Err("expected scheme `http`, `https`, `ws`, or `wss`".to_string());
    };
    let parsed = url::Url::parse(&base).map_err(|e| format!("invalid URL: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;
    let authority = match parsed.port() {
        Some(p) => format!("{}://{}:{}", parsed.scheme(), host, p),
        None => format!("{}://{}", parsed.scheme(), host),
    };
    Ok(format!("{authority}/json/version"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_url_strips_path_and_handles_ws_scheme() {
        assert_eq!(
            version_url_for("http://127.0.0.1:9222").unwrap(),
            "http://127.0.0.1:9222/json/version"
        );
        assert_eq!(
            version_url_for("ws://localhost:9222/devtools/browser/abc").unwrap(),
            "http://localhost:9222/json/version"
        );
        assert_eq!(
            version_url_for("wss://example.com/devtools/browser/xyz").unwrap(),
            "https://example.com/json/version"
        );
        assert_eq!(
            version_url_for("https://cdp.example.com:8443/x/y/z").unwrap(),
            "https://cdp.example.com:8443/json/version"
        );
    }

    #[test]
    fn version_url_rejects_unsupported_scheme() {
        assert!(version_url_for("file:///tmp/x").is_err());
        assert!(version_url_for("not-a-url").is_err());
        assert!(version_url_for("").is_err());
    }

    #[tokio::test]
    async fn probe_empty_endpoint_errors() {
        let err = probe("").await.unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[tokio::test]
    async fn probe_unreachable_endpoint_actionable_error() {
        // Port 1 is reserved/unused on virtually every host — the probe
        // must surface this as a connect-level failure with the
        // operator-facing hint.
        let err = probe("http://127.0.0.1:1").await.unwrap_err();
        assert!(err.contains("unreachable"), "got: {err}");
        assert!(
            err.contains("127.0.0.1:1"),
            "error should mention the probed URL, got: {err}"
        );
    }
}
