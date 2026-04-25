//! Outbound proxy transport.
//!
//! Three kinds are supported:
//!
//! 1. **HTTP/HTTPS proxy, HTTPS origin** — open TCP to the proxy, issue
//!    `CONNECT host:port HTTP/1.1`, wait for `200`, then hand the socket off
//!    to our BoringSSL connector just like a direct TCP would be handed off.
//! 2. **HTTP proxy, HTTP origin** — open TCP to the proxy, send the request
//!    with an **absolute-form** Request-URI (`GET http://origin/path ...`).
//!    The proxy forwards it. The request is otherwise identical to direct.
//! 3. **SOCKS5** — RFC 1928 greeting + CONNECT; works for both origin
//!    schemes. Username/password auth (RFC 1929) is supported when the proxy
//!    URL embeds credentials.
//!
//! We intentionally avoid pooling here: proxies rotate per job, and a pool
//! keyed on (proxy, host, port) would blow up memory for sticky-per-host
//! rotations. Each proxied request opens a fresh socket.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use crate::{Error, Result};

/// Parsed view of a `--proxy` URL.
pub enum ProxyKind {
    /// http://user:pass@host:port — we treat http and https here identically
    /// because the connection to the proxy itself is always TCP; the origin
    /// scheme decides whether we CONNECT-tunnel or absolute-form request.
    Http,
    Socks5,
}

pub struct ProxyInfo<'a> {
    pub kind: ProxyKind,
    pub host: &'a str,
    pub port: u16,
    pub user: Option<&'a str>,
    pub pass: Option<&'a str>,
}

pub fn parse_proxy(proxy: &Url) -> Result<ProxyInfo<'_>> {
    let kind = match proxy.scheme() {
        "http" | "https" => ProxyKind::Http,
        "socks5" | "socks5h" => ProxyKind::Socks5,
        other => return Err(Error::Http(format!("unsupported proxy scheme: {other}"))),
    };
    let host = proxy
        .host_str()
        .ok_or_else(|| Error::Http("proxy URL missing host".into()))?;
    let port = proxy.port_or_known_default().unwrap_or(match kind {
        ProxyKind::Http => 8080,
        ProxyKind::Socks5 => 1080,
    });
    let user = if proxy.username().is_empty() {
        None
    } else {
        Some(proxy.username())
    };
    let pass = proxy.password();
    Ok(ProxyInfo {
        kind,
        host,
        port,
        user,
        pass,
    })
}

pub async fn connect_proxy_tcp(info: &ProxyInfo<'_>) -> Result<TcpStream> {
    let connect_timeout = Duration::from_secs(8);
    let sock = tokio::time::timeout(connect_timeout, TcpStream::connect((info.host, info.port)))
        .await
        .map_err(|_| {
            Error::Http(format!(
                "proxy connect timeout: {}:{}",
                info.host, info.port
            ))
        })?
        .map_err(Error::Io)?;
    let _ = sock.set_nodelay(true);
    Ok(sock)
}

/// Send a CONNECT request and validate the proxy's response. Leaves the
/// socket ready for TLS handshake over the tunneled connection.
pub async fn http_connect(
    mut sock: TcpStream,
    info: &ProxyInfo<'_>,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    let auth_header = basic_auth_header(info);
    let mut req = format!(
        "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\n"
    );
    if let Some(h) = auth_header {
        req.push_str(&h);
    }
    req.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");
    sock.write_all(req.as_bytes()).await.map_err(Error::Io)?;
    sock.flush().await.map_err(Error::Io)?;

    // Read until we see the blank line ending the response status + headers.
    let mut buf = Vec::with_capacity(256);
    let mut tmp = [0u8; 256];
    loop {
        let n = sock.read(&mut tmp).await.map_err(Error::Io)?;
        if n == 0 {
            return Err(Error::Http("proxy closed connection during CONNECT".into()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let first_line = head.lines().next().unwrap_or("");
    // Expect "HTTP/1.x 200 ...".
    if !first_line.contains(" 200 ")
        && !first_line.contains(" 200\r")
        && !first_line.ends_with(" 200")
    {
        return Err(Error::Http(format!("CONNECT failed: {first_line}")));
    }
    Ok(sock)
}

/// SOCKS5 handshake + CONNECT. Handles the `no-auth` and `username/password`
/// methods (RFC 1929). Returns the socket ready to carry the target protocol
/// (TLS or plain HTTP — both work the same once SOCKS is done).
pub async fn socks5_connect(
    mut sock: TcpStream,
    info: &ProxyInfo<'_>,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    // Greeting: ver=5, nmethods, methods. Offer no-auth (0x00) and
    // username/password (0x02).
    let methods: &[u8] = if info.user.is_some() && info.pass.is_some() {
        &[0x00, 0x02]
    } else {
        &[0x00]
    };
    let mut greet = vec![0x05u8, methods.len() as u8];
    greet.extend_from_slice(methods);
    sock.write_all(&greet).await.map_err(Error::Io)?;

    let mut reply = [0u8; 2];
    sock.read_exact(&mut reply).await.map_err(Error::Io)?;
    if reply[0] != 0x05 {
        return Err(Error::Http(format!("socks5 bad version: {}", reply[0])));
    }
    match reply[1] {
        0x00 => {} // no auth
        0x02 => {
            // Username/password (RFC 1929).
            let user = info.user.unwrap_or("");
            let pass = info.pass.unwrap_or("");
            if user.len() > 255 || pass.len() > 255 {
                return Err(Error::Http("socks5 credentials too long".into()));
            }
            let mut auth = vec![0x01u8, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(pass.len() as u8);
            auth.extend_from_slice(pass.as_bytes());
            sock.write_all(&auth).await.map_err(Error::Io)?;
            let mut auth_reply = [0u8; 2];
            sock.read_exact(&mut auth_reply).await.map_err(Error::Io)?;
            if auth_reply[1] != 0x00 {
                return Err(Error::Http(format!(
                    "socks5 auth failed: {:02x}",
                    auth_reply[1]
                )));
            }
        }
        0xff => return Err(Error::Http("socks5: no acceptable method".into())),
        m => return Err(Error::Http(format!("socks5: unexpected method {m:02x}"))),
    }

    // CONNECT request: ver, cmd=1, rsv, atyp=3 (domain), len, host, port(hi,lo).
    if target_host.len() > 255 {
        return Err(Error::Http("socks5 target host too long".into()));
    }
    let mut req = vec![0x05u8, 0x01, 0x00, 0x03, target_host.len() as u8];
    req.extend_from_slice(target_host.as_bytes());
    req.push((target_port >> 8) as u8);
    req.push((target_port & 0xff) as u8);
    sock.write_all(&req).await.map_err(Error::Io)?;

    // Reply: ver, rep, rsv, atyp, bnd.addr (variable), bnd.port (2).
    let mut head = [0u8; 4];
    sock.read_exact(&mut head).await.map_err(Error::Io)?;
    if head[1] != 0x00 {
        return Err(Error::Http(format!(
            "socks5 connect reply: {:02x}",
            head[1]
        )));
    }
    let addr_len: usize = match head[3] {
        0x01 => 4,
        0x03 => {
            let mut n = [0u8; 1];
            sock.read_exact(&mut n).await.map_err(Error::Io)?;
            n[0] as usize
        }
        0x04 => 16,
        t => return Err(Error::Http(format!("socks5 bad atyp: {t:02x}"))),
    };
    let mut drain = vec![0u8; addr_len + 2];
    sock.read_exact(&mut drain).await.map_err(Error::Io)?;
    Ok(sock)
}

/// Build a `Proxy-Authorization: Basic ...` header line (including CRLF) or
/// `None` when no credentials were supplied.
fn basic_auth_header(info: &ProxyInfo<'_>) -> Option<String> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    let (u, p) = (info.user?, info.pass.unwrap_or(""));
    let raw = format!("{u}:{p}");
    let enc = STANDARD.encode(raw.as_bytes());
    Some(format!("Proxy-Authorization: Basic {enc}\r\n"))
}

#[allow(dead_code)]
fn _bind_addr(_sa: SocketAddr) {}
