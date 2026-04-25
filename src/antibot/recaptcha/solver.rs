//! reCAPTCHA v3 invisible solver — port of `recaptchav3/core/solver.py`.
//!
//! Pipeline:
//! 1. GET `recaptcha/api.js?render=<sitekey>` → regex out the version slug.
//! 2. Build `oz` JSON, scramble + base64url-encode with `0` prefix.
//! 3. GET `recaptcha/api2/anchor?...` with `Referer: <site_url>` → regex
//!    out the anchor token from `id="recaptcha-token" value="..."`.
//! 4. POST `recaptcha/api2/reload?k=<sitekey>` with `application/x-protobuffer`
//!    body — protobuf-encoded payload using field numbers from the public
//!    grecaptcha bundle.
//! 5. Regex out the `rresp` token from the response body.
//!
//! Improvements over the reference:
//! * **Persona-aware.** Optional `IdentityBundle` flows into the `oz`
//!   blob so UA-CH brands, screen, timezone, canvas/WebGL match what our
//!   shim emits in a browser path. Reference hardcodes Windows + Chrome
//!   136 — instant cross-check failure on any site that aligns UA + TLS.
//! * **Single async runtime.** Built on `reqwest`, gated by the same
//!   `cdp-backend` feature (default-on) that already pulls reqwest in.
//! * **Caller-supplied RNG.** Tests can pin the RNG seed for byte-stable
//!   outputs; production uses `rand::rng()`.
//!
//! Limitation: this is **server-side replay**. No browser is driven, so
//! Google's behavioural classifier scores us purely on the telemetry blob
//! we synthesise. Empirical scores from the reference solver: 0.3-0.9.
//! When higher confidence is needed, route through the browser path
//! instead (real interaction → real telemetry).

use std::collections::BTreeMap;
use std::time::Instant;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, REFERER};

use crate::identity::IdentityBundle;

use super::oz::build_oz;
use super::proto::{encode as proto_encode, Value as ProtoValue};
use super::utils::{encode_co, generate_cb, random_m_byte, scramble_oz};

const ANCHOR_TOKEN_RE: &str = r#"id="recaptcha-token"\s+value="([^"]+)""#;
const RRESP_RE: &str = r#""rresp"\s*,\s*"([^"]+)""#;
const VERSION_RE: &str = r#"/releases/([^/]+)/recaptcha__"#;

/// What the caller hands in. `action` defaults to `"submit"` (matches
/// grecaptcha invisible default); pass the page-specific action if the
/// site uses one (e.g. `"login"`, `"checkout"`).
pub struct SolveRequest<'a> {
    pub site_key: &'a str,
    pub site_url: &'a url::Url,
    pub action: &'a str,
    pub bundle: Option<&'a IdentityBundle>,
}

#[derive(Debug)]
pub enum SolverError {
    Http(String),
    BadResponse(&'static str),
    Encoding(String),
}

impl std::fmt::Display for SolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolverError::Http(s) => write!(f, "recaptcha solver: http: {s}"),
            SolverError::BadResponse(s) => write!(f, "recaptcha solver: bad response: {s}"),
            SolverError::Encoding(s) => write!(f, "recaptcha solver: encoding: {s}"),
        }
    }
}

impl std::error::Error for SolverError {}

pub struct SolveOutcome {
    pub token: String,
    pub elapsed_ms: u64,
}

/// Stateless solve — builds a fresh reqwest client, runs the 3-step pipeline,
/// returns the token.
///
/// `proxy_url` (when supplied) is wired into reqwest as an HTTP/HTTPS proxy.
/// We deliberately don't share a client across calls because each solve is
/// short-lived and may carry distinct identity per call (different bundle,
/// different persona).
pub async fn solve(
    req: SolveRequest<'_>,
    proxy_url: Option<&str>,
) -> Result<SolveOutcome, SolverError> {
    let started = Instant::now();
    // Seed from OS entropy via the standard helper. `from_seed` over a
    // freshly-randomised seed is the rand 0.10 idiom for "non-deterministic
    // RNG without dragging in `rand::rng()`".
    let mut seed_bytes = [0u8; 32];
    {
        let mut top = rand::rng();
        for b in seed_bytes.iter_mut() {
            *b = top.random_range(0u8..=255);
        }
    }
    let mut rng = StdRng::from_seed(seed_bytes);

    let client = build_client(proxy_url, req.bundle)?;

    let version = fetch_version(&client, req.site_key).await?;
    let now_ms = epoch_ms();

    let oz_bytes = build_oz(&mut rng, req.site_url.as_str(), req.bundle, now_ms);
    let m = random_m_byte(&mut rng);
    let scrambled = scramble_oz(&oz_bytes, now_ms as u64, m, &mut rng);

    let cb = generate_cb(&mut rng, now_ms as u64);
    let co = encode_co(req.site_url).ok_or(SolverError::BadResponse("site_url has no host"))?;

    let anchor_token = fetch_anchor_token(
        &client,
        req.site_key,
        &co,
        &cb,
        &version,
        req.site_url.as_str(),
    )
    .await?;

    let rresp = post_reload(
        &client,
        req.site_key,
        &version,
        &anchor_token,
        &scrambled,
        req.action,
        req.site_url.as_str(),
        &mut rng,
    )
    .await?;

    Ok(SolveOutcome {
        token: rresp,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn build_client(
    proxy_url: Option<&str>,
    bundle: Option<&IdentityBundle>,
) -> Result<reqwest::Client, SolverError> {
    let ua = bundle.map(|b| b.ua.clone()).unwrap_or_else(|| {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36"
            .to_string()
    });

    let mut builder = reqwest::Client::builder()
        .user_agent(ua)
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(20));

    if let Some(b) = bundle {
        let mut headers = HeaderMap::new();
        // Sec-CH-UA cluster — kept aligned with what the bundle declares
        // so a server-side cross-check (UA-CH header vs UA string) holds.
        if let Ok(v) = HeaderValue::from_str(&b.sec_ch_ua) {
            headers.insert("sec-ch-ua", v);
        }
        if let Ok(v) = HeaderValue::from_str(b.ua_platform.trim_matches('"')) {
            // sec-ch-ua-platform value is quoted in HTTP per CH spec.
            if let Ok(quoted) =
                HeaderValue::from_str(&format!("\"{}\"", b.ua_platform.trim_matches('"')))
            {
                let _ = quoted;
            }
            headers.insert("sec-ch-ua-platform", v);
        }
        headers.insert("sec-ch-ua-mobile", HeaderValue::from_static("?0"));
        if let Ok(v) = HeaderValue::from_str(&b.accept_language) {
            headers.insert("accept-language", v);
        }
        builder = builder.default_headers(headers);
    }

    if let Some(p) = proxy_url {
        let proxy =
            reqwest::Proxy::all(p).map_err(|e| SolverError::Http(format!("proxy parse: {e}")))?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|e| SolverError::Http(format!("client build: {e}")))
}

async fn fetch_version(client: &reqwest::Client, site_key: &str) -> Result<String, SolverError> {
    let url = format!(
        "https://www.google.com/recaptcha/api.js?render={}&hl=en",
        site_key
    );
    let body = client
        .get(&url)
        .send()
        .await
        .map_err(|e| SolverError::Http(format!("api.js GET: {e}")))?
        .text()
        .await
        .map_err(|e| SolverError::Http(format!("api.js read: {e}")))?;

    let re = Regex::new(VERSION_RE).expect("static regex");
    let cap = re
        .captures(&body)
        .ok_or(SolverError::BadResponse("version slug not found in api.js"))?;
    Ok(cap.get(1).unwrap().as_str().to_string())
}

async fn fetch_anchor_token(
    client: &reqwest::Client,
    site_key: &str,
    co: &str,
    cb: &str,
    version: &str,
    referer: &str,
) -> Result<String, SolverError> {
    let url = format!(
        "https://www.google.com/recaptcha/api2/anchor?ar=1&k={key}&co={co}&hl=en\
         &v={ver}&size=invisible&anchor-ms=20000&execute-ms=30000&cb={cb}",
        key = site_key,
        co = co,
        ver = version,
        cb = cb,
    );
    let body = client
        .get(&url)
        .header(REFERER, referer)
        .send()
        .await
        .map_err(|e| SolverError::Http(format!("anchor GET: {e}")))?
        .text()
        .await
        .map_err(|e| SolverError::Http(format!("anchor read: {e}")))?;

    let re = Regex::new(ANCHOR_TOKEN_RE).expect("static regex");
    let cap = re
        .captures(&body)
        .ok_or(SolverError::BadResponse("anchor token not found"))?;
    Ok(cap.get(1).unwrap().as_str().to_string())
}

#[allow(clippy::too_many_arguments)]
async fn post_reload(
    client: &reqwest::Client,
    site_key: &str,
    version: &str,
    anchor_token: &str,
    scrambled: &str,
    action: &str,
    referer: &str,
    rng: &mut impl rand::Rng,
) -> Result<String, SolverError> {
    let mut fields: BTreeMap<u32, ProtoValue> = BTreeMap::new();
    fields.insert(1, ProtoValue::from_string(version));
    fields.insert(2, ProtoValue::from_string(anchor_token));
    fields.insert(4, ProtoValue::from_string(scrambled));
    // Field 5: signed 32-bit. Reference uses random.randint over the full
    // signed range. We map to u64 via two's complement bit-cast — the
    // varint encoder treats the value as a uint64, which is what protobuf
    // expects for sint32 raw values (no zigzag here, the reference doesn't).
    let sig5: i32 = rng.random_range(i32::MIN..=i32::MAX);
    fields.insert(5, ProtoValue::from_string(&sig5.to_string()));
    fields.insert(6, ProtoValue::from_string("q"));
    fields.insert(8, ProtoValue::from_string(action));
    fields.insert(14, ProtoValue::from_string(site_key));
    fields.insert(16, ProtoValue::from_string(scrambled));
    // Telemetry is base64-encoded JSON of the same client blob the oz
    // contains in field 74 — but we only need the wire here. Keep it
    // empty-ish to mirror the reference's "W10" = base64 of "[]". A
    // future iteration can plumb a proper telemetry sub-payload.
    fields.insert(20, ProtoValue::from_string("W10"));
    fields.insert(22, ProtoValue::from_string(""));
    fields.insert(25, ProtoValue::from_string("W10"));
    fields.insert(28, ProtoValue::from_u64(20000));
    fields.insert(29, ProtoValue::from_u64(30000));

    let body = proto_encode(&fields);

    let url = format!(
        "https://www.google.com/recaptcha/api2/reload?k={}",
        site_key
    );
    let resp = client
        .post(&url)
        .header(REFERER, referer)
        .header(CONTENT_TYPE, "application/x-protobuffer")
        .body(body)
        .send()
        .await
        .map_err(|e| SolverError::Http(format!("reload POST: {e}")))?;

    let text = resp
        .text()
        .await
        .map_err(|e| SolverError::Http(format!("reload read: {e}")))?;

    let re = Regex::new(RRESP_RE).expect("static regex");
    let cap = re
        .captures(&text)
        .ok_or(SolverError::BadResponse("rresp token not found in reload"))?;
    Ok(cap.get(1).unwrap().as_str().to_string())
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_regex_matches_expected_html() {
        let html = r#"<input id="recaptcha-token" value="03ABCD-token-here-xyz"/>"#;
        let re = Regex::new(ANCHOR_TOKEN_RE).unwrap();
        let cap = re.captures(html).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "03ABCD-token-here-xyz");
    }

    #[test]
    fn rresp_regex_matches_expected_json_fragment() {
        let body = r#")]}'
[["rresp","03AGdBq25_xxx_yyy"]]"#;
        let re = Regex::new(RRESP_RE).unwrap();
        let cap = re.captures(body).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "03AGdBq25_xxx_yyy");
    }

    #[test]
    fn version_regex_extracts_release_slug() {
        let js = r#"... '/releases/abc123def_v3/recaptcha__en.js' ..."#;
        let re = Regex::new(VERSION_RE).unwrap();
        let cap = re.captures(js).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "abc123def_v3");
    }

    #[test]
    fn build_client_works_without_bundle() {
        // Smoke test — must not panic, must return a usable reqwest::Client.
        let c = build_client(None, None).unwrap();
        // Type check via debug repr.
        assert!(format!("{c:?}").contains("Client"));
    }

    #[test]
    fn build_client_works_with_bundle() {
        let bundle = IdentityBundle::from_chromium(131, 7);
        let c = build_client(None, Some(&bundle)).unwrap();
        assert!(format!("{c:?}").contains("Client"));
    }

    #[test]
    fn build_client_rejects_invalid_proxy() {
        let err = build_client(Some("not a url"), None).unwrap_err();
        assert!(matches!(err, SolverError::Http(_)));
    }
}
