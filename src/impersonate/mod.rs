pub mod cookies;
pub mod decode;
pub mod dns_cache;
pub mod doh;
pub mod headers;
pub mod ja3;
pub mod pool;
pub mod profiles;
pub mod proxy_transport;
pub mod resource_hints;
pub mod tls;

pub use profiles::Profile;

use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt, Empty};
use hyper::body::Body as HttpBody;
use hyper::rt::{Read as HyperRead, ReadBufCursor, Write as HyperWrite};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::TcpStream;
use url::Url;

/// IO stream wrapper that is either plain TCP (http://) or TLS (https://).
/// hyper's Read/Write traits are forwarded to the inner TokioIo.
pub enum MaybeTls {
    Plain(TokioIo<TcpStream>),
    Tls(TokioIo<tokio_boring::SslStream<TcpStream>>),
}

impl HyperRead for MaybeTls {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            MaybeTls::Plain(io) => Pin::new(io).poll_read(cx, buf),
            MaybeTls::Tls(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl HyperWrite for MaybeTls {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            MaybeTls::Plain(io) => Pin::new(io).poll_write(cx, buf),
            MaybeTls::Tls(io) => Pin::new(io).poll_write(cx, buf),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            MaybeTls::Plain(io) => Pin::new(io).poll_flush(cx),
            MaybeTls::Tls(io) => Pin::new(io).poll_flush(cx),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            MaybeTls::Plain(io) => Pin::new(io).poll_shutdown(cx),
            MaybeTls::Tls(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

use crate::config::HttpLimits;
use crate::{Error, Result};

async fn collect_limited<B>(
    mut body: B,
    limit: Option<usize>,
    store_truncated: bool,
) -> Result<(Bytes, bool)>
where
    B: HttpBody<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut out = BytesMut::new();
    let mut truncated = false;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| Error::Http(format!("body: {e}")))?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        if let Some(max) = limit {
            if out.len().saturating_add(data.len()) > max {
                if !store_truncated {
                    return Err(Error::BodyTooLarge { limit: max });
                }
                let remaining = max.saturating_sub(out.len());
                if remaining > 0 {
                    out.extend_from_slice(&data[..remaining]);
                }
                truncated = true;
                continue;
            }
        }
        if !truncated {
            out.extend_from_slice(data);
        }
    }
    Ok((out.freeze(), truncated))
}

fn has_non_identity_content_encoding(headers: &HeaderMap) -> bool {
    headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|enc| {
            enc.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .any(|enc| !enc.is_empty() && enc != "identity")
        })
        .unwrap_or(false)
}

async fn connect_any(addrs: &[SocketAddr], timeout: std::time::Duration) -> Result<TcpStream> {
    let mut last_err: Option<String> = None;
    for addr in addrs {
        match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(e)) => last_err = Some(format!("{addr}: {e}")),
            Err(_) => last_err = Some(format!("{addr}: tcp connect timeout")),
        }
    }
    Err(Error::Http(format!(
        "tcp connect failed for all resolved addresses: {}",
        last_err.unwrap_or_else(|| "empty address list".into())
    )))
}

pub struct ImpersonateClient {
    profile: Profile,
    proxy: Option<Url>,
    connector: boring::ssl::SslConnector,
    pool: pool::ConnPool,
    dns: dns_cache::DnsCache,
    cookies: cookies::CookieJar,
    follow_redirects: bool,
    max_redirects: u8,
    cookies_enabled: bool,
    identity_bundle: Arc<crate::identity::IdentityBundle>,
    http_limits: HttpLimits,
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub final_url: Url,
    pub alpn: Option<String>,
    pub tls_version: Option<String>,
    pub cipher: Option<String>,
    pub timings: crate::metrics::NetworkTimings,
    pub peer_cert: Option<crate::discovery::cert::PeerCert>,
    pub body_truncated: bool,
}

impl ImpersonateClient {
    pub fn new(profile: Profile) -> Result<Self> {
        let connector = tls::build_connector(profile)?;
        Ok(Self {
            profile,
            proxy: None,
            connector,
            pool: pool::ConnPool::new(),
            dns: dns_cache::DnsCache::new(std::time::Duration::from_secs(300)),
            cookies: cookies::CookieJar::new(),
            follow_redirects: true,
            max_redirects: 10,
            cookies_enabled: true,
            identity_bundle: Arc::new(crate::identity::IdentityBundle::from_chromium(
                profile.major_version(),
                0,
            )),
            http_limits: HttpLimits::default(),
        })
    }

    pub fn set_cookies_enabled(&mut self, yes: bool) {
        self.cookies_enabled = yes;
    }

    pub fn set_http_limits(&mut self, limits: HttpLimits) {
        self.http_limits = limits;
    }

    pub fn set_locale(&mut self, locale: Option<&str>) {
        if let Some(locale) = locale {
            Arc::make_mut(&mut self.identity_bundle).apply_locale(locale);
        }
    }

    pub fn set_user_agent_override(&mut self, ua: Option<String>) -> Result<()> {
        if let Some(s) = ua.as_deref() {
            Arc::make_mut(&mut self.identity_bundle)
                .apply_user_agent_override(s)
                .map_err(Error::Config)?;
        }
        Ok(())
    }

    pub fn set_identity_bundle(&mut self, bundle: crate::identity::IdentityBundle) {
        self.profile = bundle.profile();
        self.identity_bundle = Arc::new(bundle);
    }

    pub fn identity_bundle(&self) -> &crate::identity::IdentityBundle {
        &self.identity_bundle
    }

    pub fn with_proxy(mut self, proxy: Url) -> Self {
        self.proxy = Some(proxy);
        self
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub fn set_follow_redirects(&mut self, yes: bool) {
        self.follow_redirects = yes;
    }

    pub fn set_max_redirects(&mut self, n: u8) {
        self.max_redirects = n;
    }

    pub fn cookies(&self) -> &cookies::CookieJar {
        &self.cookies
    }

    /// Fetch without measuring — the hot path. Defaults to Document dest.
    pub async fn get(&self, url: &Url) -> Result<Response> {
        self.get_with_redirects(
            url,
            false,
            crate::discovery::assets::SecFetchDest::Document,
            None,
        )
        .await
    }

    /// Fetch with full per-phase timing.
    pub async fn get_timed(&self, url: &Url) -> Result<Response> {
        self.get_with_redirects(
            url,
            true,
            crate::discovery::assets::SecFetchDest::Document,
            None,
        )
        .await
    }

    pub async fn get_timed_with_dest(
        &self,
        url: &Url,
        dest: crate::discovery::assets::SecFetchDest,
    ) -> Result<Response> {
        self.get_with_redirects(url, true, dest, None).await
    }

    /// Fetch using an explicit Sec-Fetch-Dest so subresource requests (js,
    /// css, images, xhr) look like the right Chrome subresource rather than
    /// a top-level navigation.
    pub async fn get_with_dest(
        &self,
        url: &Url,
        dest: crate::discovery::assets::SecFetchDest,
    ) -> Result<Response> {
        self.get_with_redirects(url, false, dest, None).await
    }

    /// Same as `get_with_dest` but forces the request through the provided
    /// proxy URL (http(s)/socks5). Bypasses the connection pool.
    pub async fn get_via(
        &self,
        url: &Url,
        proxy: Option<&Url>,
        dest: crate::discovery::assets::SecFetchDest,
    ) -> Result<Response> {
        self.get_with_redirects(url, false, dest, proxy.cloned())
            .await
    }

    pub async fn get_timed_via(
        &self,
        url: &Url,
        proxy: Option<&Url>,
        dest: crate::discovery::assets::SecFetchDest,
    ) -> Result<Response> {
        self.get_with_redirects(url, true, dest, proxy.cloned())
            .await
    }

    async fn get_with_redirects(
        &self,
        url: &Url,
        timed: bool,
        dest: crate::discovery::assets::SecFetchDest,
        proxy: Option<Url>,
    ) -> Result<Response> {
        tokio::time::timeout(
            self.http_limits.request_timeout,
            self.get_with_redirects_inner(url, timed, dest, proxy),
        )
        .await
        .map_err(|_| Error::RequestTimeout {
            timeout_ms: self.http_limits.request_timeout.as_millis(),
        })?
    }

    async fn get_with_redirects_inner(
        &self,
        url: &Url,
        timed: bool,
        dest: crate::discovery::assets::SecFetchDest,
        proxy: Option<Url>,
    ) -> Result<Response> {
        let mut current = url.clone();
        let mut hops: u8 = 0;
        loop {
            let resp = self
                .get_inner(&current, timed, dest, proxy.as_ref())
                .await?;
            if !self.follow_redirects {
                return Ok(resp);
            }
            let code = resp.status.as_u16();
            if !matches!(code, 301 | 302 | 303 | 307 | 308) {
                return Ok(resp);
            }
            let Some(loc) = resp.headers.get("location").and_then(|v| v.to_str().ok()) else {
                return Ok(resp);
            };
            let Ok(next) = current.join(loc) else {
                return Ok(resp);
            };
            if hops >= self.max_redirects {
                return Ok(Response {
                    final_url: next,
                    ..resp
                });
            }
            hops += 1;
            current = next;
        }
    }

    async fn get_inner(
        &self,
        url: &Url,
        timed: bool,
        dest: crate::discovery::assets::SecFetchDest,
        proxy: Option<&Url>,
    ) -> Result<Response> {
        // Client-level proxy (legacy, with_proxy setter) takes over when no
        // per-request proxy was supplied.
        let effective_proxy = proxy.cloned().or_else(|| self.proxy.clone());
        let scheme = url.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(Error::Http(format!("unsupported scheme: {url}")));
        }
        let is_https = scheme == "https";
        let host = url
            .host_str()
            .ok_or_else(|| Error::Http(format!("missing host: {url}")))?;
        let port = url
            .port_or_known_default()
            .unwrap_or(if is_https { 443 } else { 80 });

        let t0 = timed.then(std::time::Instant::now);

        // Fast-path: reuse live h2/h1 connection for this host. Skips DNS, TCP,
        // TLS, h2 handshake entirely. Disabled when a proxy is active so we
        // don't leak pooled connections across proxy rotations.
        // Pool key now includes the proxy: a connection opened via proxy A
        // must NOT be reused for a request that wants proxy B (or none),
        // otherwise we'd leak the upstream IP. Hot path with no proxy
        // shares the empty-proxy bucket — that's the common case.
        let pool_key = pool::ConnKey::new(
            if is_https { "https" } else { "http" },
            host.to_string(),
            port,
            effective_proxy.as_ref(),
        );
        // Pool reuse now works for proxy-bound connections too (sub-pool
        // per (scheme,host,port,proxy_key)) — the only thing we still
        // refuse is reusing across proxies.
        {
            if is_https {
                if let Some(p) = self.pool.get_live(&pool_key) {
                    return self
                        .send_on_sender(url, p.sender, host, port, timed, t0, dest)
                        .await;
                }
            } else if let Some(p) = self.pool.h1_get_live(&pool_key) {
                return self.send_on_h1(url, p, host, port, timed, t0, dest).await;
            }
        }

        let connect_timeout = std::time::Duration::from_secs(8);
        let (tcp, dns_ms, tcp_connect_ms) = if let Some(pxy) = effective_proxy.as_ref() {
            // Route through proxy: TCP goes to the proxy, not the origin.
            let info = proxy_transport::parse_proxy(pxy)?;
            let tcp_started = std::time::Instant::now();
            let raw = proxy_transport::connect_proxy_tcp(&info).await?;
            let tcp = match info.kind {
                proxy_transport::ProxyKind::Http if is_https => {
                    proxy_transport::http_connect(raw, &info, host, port).await?
                }
                proxy_transport::ProxyKind::Http => {
                    // HTTP origin via HTTP proxy: keep raw socket; the request
                    // builder below will emit absolute-form Request-URI.
                    raw
                }
                proxy_transport::ProxyKind::Socks5 => {
                    proxy_transport::socks5_connect(raw, &info, host, port).await?
                }
            };
            let tcp_connect_ms = tcp_started.elapsed().as_millis() as u64;
            (tcp, timed.then_some(0u64), timed.then_some(tcp_connect_ms))
        } else if timed {
            let dns_started = std::time::Instant::now();
            let addrs = self.dns.resolve(host, port).await?;
            let dns_ms = dns_started.elapsed().as_millis() as u64;
            let tcp_started = std::time::Instant::now();
            let tcp = connect_any(&addrs, connect_timeout).await?;
            let tcp_connect_ms = tcp_started.elapsed().as_millis() as u64;
            (tcp, Some(dns_ms), Some(tcp_connect_ms))
        } else {
            let addrs = self.dns.resolve(host, port).await?;
            let tcp = connect_any(&addrs, connect_timeout).await?;
            (tcp, None, None)
        };
        let _ = tcp.set_nodelay(true);
        // When proxy + plain HTTP, the request URI must be absolute; we
        // record that flag so the h1 branch below can use absolute-form.
        let absolute_form = effective_proxy.is_some() && !is_https;

        let mut peer_cert: Option<crate::discovery::cert::PeerCert> = None;
        let (io, alpn, tls_version, cipher, tls_handshake_ms, is_h2) = if is_https {
            let mut config = self
                .connector
                .configure()
                .map_err(|e| Error::Tls(format!("configure: {e}")))?;
            config.set_verify_hostname(true);
            config.set_use_server_name_indication(true);
            tls::configure_ssl(&mut *config)?;

            // Resume a previous session for this host:port if we have a
            // cached ticket — saves one round trip on reconnect, exactly
            // like real Chrome does. `set_session` is best-effort; an
            // expired ticket on the server side just falls back to a
            // full handshake.
            if let Some(prev) = tls::lookup_ticket(host, port) {
                // SAFETY: we own `prev`, it's not shared, and `set_session`
                // copies the underlying SSL_SESSION ref internally.
                unsafe {
                    let _ = config.set_session(&prev);
                }
            }
            // Pin the host/port so the new-session callback knows where
            // to stash the ticket the server will issue.
            tls::pin_host_for_session(&config, host, port);

            let tls_started = timed.then(std::time::Instant::now);
            let tls = tokio_boring::connect(config, host, tcp)
                .await
                .map_err(|e| Error::Tls(format!("handshake: {e}")))?;
            let tls_handshake_ms = tls_started.map(|t| t.elapsed().as_millis() as u64);

            let alpn = tls
                .ssl()
                .selected_alpn_protocol()
                .map(|p| String::from_utf8_lossy(p).into_owned());
            let tls_version = Some(tls.ssl().version_str().to_string());
            let cipher = tls.ssl().current_cipher().map(|c| c.name().to_string());
            let is_h2 = matches!(alpn.as_deref(), Some("h2"));
            peer_cert = crate::discovery::cert::extract(tls.ssl());
            (
                MaybeTls::Tls(TokioIo::new(tls)),
                alpn,
                tls_version,
                cipher,
                tls_handshake_ms,
                is_h2,
            )
        } else {
            // Plain HTTP: HTTP/1.1 only (no ALPN nego).
            (
                MaybeTls::Plain(TokioIo::new(tcp)),
                None,
                None,
                None,
                None,
                false,
            )
        };

        let authority = match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_string(),
        };
        let scheme_for_uri = if is_https { "https" } else { "http" };
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_string(),
        };

        let mut ttfb_ms: Option<u64> = None;
        let req_started = timed.then(std::time::Instant::now);
        let (status, hdrs, body_bytes, body_truncated) = if is_h2 {
            // Chrome 144+ emits exactly four client SETTINGS on the wire, in
            // numeric id order (matching the h2 crate's own encode order):
            //   1 HEADER_TABLE_SIZE     = 65536
            //   2 ENABLE_PUSH           = 0
            //   4 INITIAL_WINDOW_SIZE   = 6_291_456
            //   6 MAX_HEADER_LIST_SIZE  = 262_144
            // and a connection WINDOW_UPDATE with delta 15_663_105 on stream 0
            // (default window 65_535 → 15 MiB = 15_728_640).
            //
            // Chrome does NOT include MAX_CONCURRENT_STREAMS (id 3) in the
            // client-side SETTINGS; adding it produces an Akamai H2 string
            // "1:..;2:..;3:..;4:..;6:.." that is a distinct non-Chrome class.
            // We therefore deliberately omit `max_concurrent_streams` here.
            let (sender, conn) =
                hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .header_table_size(65536)
                    .initial_stream_window_size(6_291_456)
                    .initial_connection_window_size(15_728_640)
                    .max_header_list_size(262_144)
                    // Hyper defaults max_frame_size to Some(16384), which makes the
                    // h2 crate emit SETTING id 5 (MAX_FRAME_SIZE=16384) even though
                    // 16384 is already the spec default. Chrome does NOT emit
                    // setting 5; setting max_frame_size(None) suppresses it and
                    // keeps the Akamai H2 string at 1:65536;2:0;4:6291456;6:262144.
                    .max_frame_size(None)
                    .handshake(io)
                    .await
                    .map_err(|e| Error::Http(format!("h2 handshake: {e}")))?;
            let pool = self.pool.clone();
            let pool_key_for_task = pool_key.clone();
            tokio::spawn(async move {
                let _ = conn.await;
                pool.invalidate(&pool_key_for_task);
            });
            if is_https && effective_proxy.is_none() {
                self.pool.store(
                    pool_key.clone(),
                    pool::PooledH2 {
                        sender: sender.clone(),
                    },
                );
            }
            let mut sender = sender;

            let uri_full = format!("{scheme_for_uri}://{authority}{path}");
            let req = Request::builder()
                .method(Method::GET)
                .uri(uri_full)
                .body(Empty::<Bytes>::new())
                .map_err(|e| Error::Http(format!("build req: {e}")))?;

            let cookie = if self.cookies_enabled {
                self.cookies.cookie_header(url)
            } else {
                None
            };
            let req = chrome_http_headers_full(
                &self.identity_bundle,
                req,
                &authority,
                cookie.as_deref(),
                dest,
            )?;
            let resp = sender
                .send_request(req)
                .await
                .map_err(|e| Error::Http(format!("h2 send: {e}")))?;
            if let Some(t) = req_started {
                ttfb_ms = Some(t.elapsed().as_millis() as u64);
            }
            let (parts, body) = resp.into_parts();
            if self.cookies_enabled {
                self.cookies.ingest(url, &parts.headers);
            }
            let collected = collect_limited(
                body,
                self.http_limits.max_encoded_body_bytes,
                self.http_limits.store_truncated_bodies,
            )
            .await?;
            (parts.status, parts.headers, collected.0, collected.1)
        } else {
            let (sender, conn) = hyper::client::conn::http1::handshake(io)
                .await
                .map_err(|e| Error::Http(format!("h1 handshake: {e}")))?;
            let pool = self.pool.clone();
            let pool_key_for_task = pool_key.clone();
            tokio::spawn(async move {
                let _ = conn.await;
                pool.h1_invalidate(&pool_key_for_task);
            });
            let pooled_sender = Arc::new(tokio::sync::Mutex::new(sender));
            if effective_proxy.is_none() {
                self.pool.h1_store(
                    pool_key.clone(),
                    pool::PooledH1 {
                        sender: pooled_sender.clone(),
                    },
                );
            }
            let mut sender_guard = pooled_sender.lock().await;
            let sender = &mut *sender_guard;

            let req_uri = if absolute_form {
                // Absolute-form Request-URI for HTTP origins via proxy.
                format!("http://{authority}{path}")
            } else {
                path.clone()
            };
            let req = Request::builder()
                .method(Method::GET)
                .uri(req_uri)
                .header("host", &authority)
                .body(Empty::<Bytes>::new())
                .map_err(|e| Error::Http(format!("build req: {e}")))?;
            let cookie = if self.cookies_enabled {
                self.cookies.cookie_header(url)
            } else {
                None
            };
            let req = chrome_http_headers_full(
                &self.identity_bundle,
                req,
                &authority,
                cookie.as_deref(),
                dest,
            )?;

            let resp = sender
                .send_request(req)
                .await
                .map_err(|e| Error::Http(format!("h1 send: {e}")))?;
            if let Some(t) = req_started {
                ttfb_ms = Some(t.elapsed().as_millis() as u64);
            }
            let (parts, body) = resp.into_parts();
            if self.cookies_enabled {
                self.cookies.ingest(url, &parts.headers);
            }
            let collected = collect_limited(
                body,
                self.http_limits.max_encoded_body_bytes,
                self.http_limits.store_truncated_bodies,
            )
            .await?;
            (parts.status, parts.headers, collected.0, collected.1)
        };

        let download_ms = match (ttfb_ms, req_started) {
            (Some(ttfb), Some(t)) => Some((t.elapsed().as_millis() as u64).saturating_sub(ttfb)),
            _ => None,
        };
        let total_ms = t0.map(|t| t.elapsed().as_millis() as u64);

        // Body decompression is CPU-bound (brotli/zstd especially); move it
        // off the async worker so other sockets keep being polled.
        let hdrs_for_decode = hdrs.clone();
        let decode_limits = self.http_limits.clone();
        if body_truncated && has_non_identity_content_encoding(&hdrs_for_decode) {
            return Err(Error::BodyTooLarge {
                limit: decode_limits
                    .max_encoded_body_bytes
                    .unwrap_or(body_bytes.len()),
            });
        }
        let decoded = tokio::task::spawn_blocking(move || {
            decode::decode_body_limited(
                &hdrs_for_decode,
                body_bytes,
                decode_limits.max_decoded_body_bytes,
                decode_limits.max_decompression_ratio,
            )
        })
        .await
        .map_err(|e| Error::Http(format!("decode join: {e}")))??;
        let mut hdrs = hdrs;
        hdrs.remove("content-encoding");
        hdrs.remove("content-length");

        let timings = if timed {
            crate::metrics::NetworkTimings {
                dns_ms,
                tcp_connect_ms,
                tls_handshake_ms,
                ttfb_ms,
                download_ms,
                total_ms,
                status: Some(status.as_u16()),
                bytes: Some(decoded.len() as u64),
                alpn: alpn.clone(),
                tls_version: tls_version.clone(),
                cipher: cipher.clone(),
            }
        } else {
            crate::metrics::NetworkTimings::default()
        };

        Ok(Response {
            status,
            headers: hdrs,
            body: decoded,
            final_url: url.clone(),
            alpn,
            tls_version,
            cipher,
            timings,
            peer_cert,
            body_truncated,
        })
    }
}

impl ImpersonateClient {
    /// Send a GET reusing a pooled HTTP/1.1 keep-alive connection. h1 is
    /// single-request-per-socket, so this awaits the sender Mutex.
    async fn send_on_h1(
        &self,
        url: &Url,
        pooled: pool::PooledH1,
        host: &str,
        port: u16,
        timed: bool,
        t0: Option<std::time::Instant>,
        dest: crate::discovery::assets::SecFetchDest,
    ) -> Result<Response> {
        let authority = if port == 80 || port == 443 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_string(),
        };
        let req = Request::builder()
            .method(Method::GET)
            .uri(&path)
            .header("host", &authority)
            .body(Empty::<Bytes>::new())
            .map_err(|e| Error::Http(format!("build req: {e}")))?;
        let cookie = if self.cookies_enabled {
            self.cookies.cookie_header(url)
        } else {
            None
        };
        let req = chrome_http_headers_full(
            &self.identity_bundle,
            req,
            &authority,
            cookie.as_deref(),
            dest,
        )?;

        let mut sender = pooled.sender.lock().await;
        let req_started = timed.then(std::time::Instant::now);
        let resp = match sender.send_request(req).await {
            Ok(r) => r,
            Err(e) => {
                let scheme: &'static str = if url.scheme() == "https" {
                    "https"
                } else {
                    "http"
                };
                self.pool
                    .h1_invalidate(&pool::ConnKey::new(scheme, host.to_string(), port, None));
                return Err(Error::Http(format!("h1 send (pooled): {e}")));
            }
        };
        let ttfb_ms = req_started.map(|t| t.elapsed().as_millis() as u64);
        let (parts, body) = resp.into_parts();
        if self.cookies_enabled {
            self.cookies.ingest(url, &parts.headers);
        }
        let (collected, body_truncated) = collect_limited(
            body,
            self.http_limits.max_encoded_body_bytes,
            self.http_limits.store_truncated_bodies,
        )
        .await?;
        let status = parts.status;
        let hdrs = parts.headers;

        let hdrs_for_decode = hdrs.clone();
        let decode_limits = self.http_limits.clone();
        if body_truncated && has_non_identity_content_encoding(&hdrs_for_decode) {
            return Err(Error::BodyTooLarge {
                limit: decode_limits
                    .max_encoded_body_bytes
                    .unwrap_or(collected.len()),
            });
        }
        let decoded = tokio::task::spawn_blocking(move || {
            decode::decode_body_limited(
                &hdrs_for_decode,
                collected,
                decode_limits.max_decoded_body_bytes,
                decode_limits.max_decompression_ratio,
            )
        })
        .await
        .map_err(|e| Error::Http(format!("decode join: {e}")))??;
        let mut hdrs = hdrs;
        hdrs.remove("content-encoding");
        hdrs.remove("content-length");

        let total_ms = t0.map(|t| t.elapsed().as_millis() as u64);
        let download_ms = match (ttfb_ms, req_started) {
            (Some(ttfb), Some(t)) => Some((t.elapsed().as_millis() as u64).saturating_sub(ttfb)),
            _ => None,
        };
        let timings = if timed {
            crate::metrics::NetworkTimings {
                dns_ms: Some(0),
                tcp_connect_ms: Some(0),
                tls_handshake_ms: None,
                ttfb_ms,
                download_ms,
                total_ms,
                status: Some(status.as_u16()),
                bytes: Some(decoded.len() as u64),
                alpn: None,
                tls_version: None,
                cipher: None,
            }
        } else {
            crate::metrics::NetworkTimings::default()
        };

        Ok(Response {
            status,
            headers: hdrs,
            body: decoded,
            final_url: url.clone(),
            alpn: None,
            tls_version: None,
            cipher: None,
            timings,
            peer_cert: None,
            body_truncated,
        })
    }

    /// Send a GET using an already-open pooled h2 sender. Skips all setup —
    /// dns/tcp/tls timings are None. Decompression still runs.
    async fn send_on_sender(
        &self,
        url: &Url,
        mut sender: hyper::client::conn::http2::SendRequest<Empty<Bytes>>,
        host: &str,
        port: u16,
        timed: bool,
        t0: Option<std::time::Instant>,
        dest: crate::discovery::assets::SecFetchDest,
    ) -> Result<Response> {
        let authority = if port == 443 || port == 80 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_string(),
        };
        let uri_full = format!("https://{authority}{path}");
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri_full)
            .body(Empty::<Bytes>::new())
            .map_err(|e| Error::Http(format!("build req: {e}")))?;
        let cookie = if self.cookies_enabled {
            self.cookies.cookie_header(url)
        } else {
            None
        };
        let req = chrome_http_headers_full(
            &self.identity_bundle,
            req,
            &authority,
            cookie.as_deref(),
            dest,
        )?;
        let req_started = timed.then(std::time::Instant::now);
        let resp = match sender.send_request(req).await {
            Ok(r) => r,
            Err(e) => {
                // Connection is dead; drop it from the pool so the next call
                // reconnects.
                self.pool
                    .invalidate(&pool::ConnKey::new("https", host.to_string(), port, None));
                return Err(Error::Http(format!("h2 send (pooled): {e}")));
            }
        };
        let mut ttfb_ms = None;
        if let Some(t) = req_started {
            ttfb_ms = Some(t.elapsed().as_millis() as u64);
        }
        let (parts, body) = resp.into_parts();
        if self.cookies_enabled {
            self.cookies.ingest(url, &parts.headers);
        }
        let (collected, body_truncated) = collect_limited(
            body,
            self.http_limits.max_encoded_body_bytes,
            self.http_limits.store_truncated_bodies,
        )
        .await?;
        let status = parts.status;
        let hdrs = parts.headers;

        let hdrs_for_decode = hdrs.clone();
        let decode_limits = self.http_limits.clone();
        if body_truncated && has_non_identity_content_encoding(&hdrs_for_decode) {
            return Err(Error::BodyTooLarge {
                limit: decode_limits
                    .max_encoded_body_bytes
                    .unwrap_or(collected.len()),
            });
        }
        let decoded = tokio::task::spawn_blocking(move || {
            decode::decode_body_limited(
                &hdrs_for_decode,
                collected,
                decode_limits.max_decoded_body_bytes,
                decode_limits.max_decompression_ratio,
            )
        })
        .await
        .map_err(|e| Error::Http(format!("decode join: {e}")))??;
        let mut hdrs = hdrs;
        hdrs.remove("content-encoding");
        hdrs.remove("content-length");

        let total_ms = t0.map(|t| t.elapsed().as_millis() as u64);
        let download_ms = match (ttfb_ms, req_started) {
            (Some(ttfb), Some(t)) => Some((t.elapsed().as_millis() as u64).saturating_sub(ttfb)),
            _ => None,
        };
        let timings = if timed {
            crate::metrics::NetworkTimings {
                dns_ms: Some(0),
                tcp_connect_ms: Some(0),
                tls_handshake_ms: Some(0),
                ttfb_ms,
                download_ms,
                total_ms,
                status: Some(status.as_u16()),
                bytes: Some(decoded.len() as u64),
                alpn: Some("h2".into()),
                tls_version: None,
                cipher: None,
            }
        } else {
            crate::metrics::NetworkTimings::default()
        };

        Ok(Response {
            status,
            headers: hdrs,
            body: decoded,
            final_url: url.clone(),
            alpn: Some("h2".into()),
            tls_version: None,
            cipher: None,
            timings,
            peer_cert: None,
            body_truncated,
        })
    }
}

fn chrome_http_headers_full<B>(
    bundle: &crate::identity::IdentityBundle,
    req: Request<B>,
    _authority: &str,
    cookie: Option<&str>,
    dest: crate::discovery::assets::SecFetchDest,
) -> Result<Request<B>> {
    use crate::impersonate::headers::ChromeRequestKind;
    let (mut parts, body) = req.into_parts();
    let kind = ChromeRequestKind::from(dest);
    let ua = bundle.ua.as_str();
    let sec_ch_ua = bundle.sec_ch_ua.as_str();
    let sec_ch_ua_mobile = bundle.sec_ch_ua_mobile();
    let sec_ch_ua_platform = bundle.ua_platform.as_str();
    let accept_language = bundle.accept_language.as_str();
    let site = if kind == ChromeRequestKind::Document {
        "none"
    } else {
        "same-origin"
    };
    let h = &mut parts.headers;
    // Iterate the canonical header order for this request kind and resolve
    // each value from the active IdentityBundle. This keeps HTTP spoofing
    // aligned with the render shim and CDP Network.setUserAgentOverride.
    // HeaderValue::from_static is used wherever the value is a compile-time
    // literal — it can't fail and doesn't clone. HeaderMap preserves
    // insertion order on iter(), so the loop below IS the wire order the
    // h1/h2 emitters will emit.
    for name in kind.header_order() {
        match *name {
            "sec-ch-ua" => {
                if let Ok(v) = sec_ch_ua.parse() {
                    h.insert("sec-ch-ua", v);
                }
            }
            "sec-ch-ua-mobile" => {
                h.insert(
                    "sec-ch-ua-mobile",
                    HeaderValue::from_static(sec_ch_ua_mobile),
                );
            }
            "sec-ch-ua-platform" => {
                if let Ok(v) = sec_ch_ua_platform.parse() {
                    h.insert("sec-ch-ua-platform", v);
                }
            }
            "upgrade-insecure-requests" => {
                if kind.includes_upgrade_insecure_requests() {
                    h.insert("upgrade-insecure-requests", HeaderValue::from_static("1"));
                }
            }
            "user-agent" => {
                if let Ok(v) = ua.parse() {
                    h.insert("user-agent", v);
                }
            }
            "accept" => {
                h.insert("accept", HeaderValue::from_static(kind.default_accept()));
            }
            "sec-fetch-site" => {
                h.insert("sec-fetch-site", HeaderValue::from_static(site));
            }
            "sec-fetch-mode" => {
                h.insert(
                    "sec-fetch-mode",
                    HeaderValue::from_static(kind.sec_fetch_mode()),
                );
            }
            "sec-fetch-user" => {
                if kind.includes_sec_fetch_user() {
                    h.insert("sec-fetch-user", HeaderValue::from_static("?1"));
                }
            }
            "sec-fetch-dest" => {
                h.insert(
                    "sec-fetch-dest",
                    HeaderValue::from_static(kind.sec_fetch_dest()),
                );
            }
            "accept-encoding" => {
                h.insert(
                    "accept-encoding",
                    HeaderValue::from_static("gzip, deflate, br, zstd"),
                );
            }
            "accept-language" => {
                if let Ok(v) = accept_language.parse() {
                    h.insert("accept-language", v);
                }
            }
            "cookie" => {
                if let Some(c) = cookie {
                    if !c.is_empty() {
                        if let Ok(v) = c.parse() {
                            h.insert("cookie", v);
                        }
                    }
                }
            }
            // Names we don't populate here (referer, origin, content-type,
            // ping-from, ping-to) are emitted by higher-level callers that
            // know the referer / origin / body — skip silently so the wire
            // order stays honest for the fields we DO emit.
            _ => {}
        }
    }
    Ok(Request::from_parts(parts, body))
}

#[cfg(test)]
mod wire_order_tests {
    use super::*;
    use crate::discovery::assets::SecFetchDest;
    use crate::impersonate::headers::ChromeRequestKind;

    fn emit(dest: SecFetchDest) -> Vec<String> {
        let req = Request::builder()
            .method(Method::GET)
            .uri("https://example.test/")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let bundle = crate::identity::IdentityBundle::from_chromium(
            Profile::Chrome131Stable.major_version(),
            1,
        );
        let out = chrome_http_headers_full(&bundle, req, "example.test", Some("id=1"), dest)
            .expect("build");
        out.headers()
            .iter()
            .map(|(k, _)| k.as_str().to_string())
            .collect()
    }

    #[test]
    fn document_wire_order_starts_with_sec_ch_ua_cluster() {
        let names = emit(SecFetchDest::Document);
        // sec-ch-ua cluster up front; upgrade-insecure-requests between
        // the cluster and user-agent (Chrome Document signature).
        assert_eq!(names[0], "sec-ch-ua");
        assert_eq!(names[1], "sec-ch-ua-mobile");
        assert_eq!(names[2], "sec-ch-ua-platform");
        assert_eq!(names[3], "upgrade-insecure-requests");
        assert_eq!(names[4], "user-agent");
        assert_eq!(names.last().map(String::as_str), Some("cookie"));
    }

    #[test]
    fn xhr_wire_order_has_no_upgrade_no_sec_fetch_user() {
        let names = emit(SecFetchDest::Empty);
        assert!(!names.iter().any(|h| h == "upgrade-insecure-requests"));
        assert!(!names.iter().any(|h| h == "sec-fetch-user"));
        // Xhr order per ChromeRequestKind: sec-ch-ua cluster → accept → UA.
        assert_eq!(names[0], "sec-ch-ua");
        assert_eq!(names.last().map(String::as_str), Some("cookie"));
    }

    #[test]
    fn script_wire_order_matches_kind() {
        let names = emit(SecFetchDest::Script);
        let expected: Vec<&str> = ChromeRequestKind::Script
            .header_order()
            .iter()
            .copied()
            // Headers we do not populate (referer, origin, content-type) are
            // skipped by the emitter; filter them out of the expectation so
            // the comparison reflects what is actually on the wire.
            .filter(|n| !matches!(*n, "referer" | "origin" | "content-type"))
            .collect();
        let got: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(got, expected, "script wire order mismatch");
    }

    #[test]
    fn image_wire_order_matches_kind() {
        let names = emit(SecFetchDest::Image);
        let expected: Vec<&str> = ChromeRequestKind::Image
            .header_order()
            .iter()
            .copied()
            .filter(|n| !matches!(*n, "referer" | "origin" | "content-type"))
            .collect();
        let got: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(got, expected, "image wire order mismatch");
    }

    #[test]
    fn font_wire_order_matches_kind() {
        let names = emit(SecFetchDest::Font);
        let expected: Vec<&str> = ChromeRequestKind::Font
            .header_order()
            .iter()
            .copied()
            .filter(|n| !matches!(*n, "referer" | "origin" | "content-type"))
            .collect();
        let got: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(got, expected, "font wire order mismatch");
    }

    #[test]
    fn sec_fetch_values_match_kind() {
        let headers = {
            let req = Request::builder()
                .method(Method::GET)
                .uri("https://example.test/")
                .body(Empty::<Bytes>::new())
                .unwrap();
            let bundle = crate::identity::IdentityBundle::from_chromium(
                Profile::Chrome131Stable.major_version(),
                1,
            );
            chrome_http_headers_full(&bundle, req, "example.test", None, SecFetchDest::Empty)
                .unwrap()
        };
        let h = headers.headers();
        assert_eq!(h.get("sec-fetch-dest").unwrap(), "empty");
        assert_eq!(h.get("sec-fetch-mode").unwrap(), "cors");
        assert!(h.get("sec-fetch-user").is_none());
    }
}
