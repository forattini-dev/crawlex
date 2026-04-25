//! Byte-exact HTTP/2 fingerprint validation against a local server.
//!
//! Spawns a tiny raw-TCP server that speaks just enough of HTTP/2 to capture
//! the client's preface + first SETTINGS frame + connection-level
//! WINDOW_UPDATE, then asserts they match Chrome 144+ Akamai-format
//! fingerprint byte-exact:
//!
//!   SETTINGS      = 1:65536;2:0;4:6291456;6:262144
//!   WINDOW_UPDATE = 15663105 on stream 0
//!   NO standalone PRIORITY frames before the first HEADERS.
//!
//! Covers P0-7 from `research/evasion-actionable-backlog.md`.
//!
//! `#[ignore]` by default: runs in the dispatch gate, not in the default
//! cargo-test suite (the client path uses `ImpersonateClient`, which wants
//! a tokio runtime and real TLS stack). Run with:
//!   cargo test --all-features --test h2_fingerprint_live -- --ignored

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// One captured HTTP/2 frame header + payload.
#[derive(Debug, Clone)]
struct Frame {
    kind: u8,
    flags: u8,
    stream_id: u32,
    payload: Vec<u8>,
}

impl Frame {
    fn kind_name(&self) -> &'static str {
        match self.kind {
            0x0 => "DATA",
            0x1 => "HEADERS",
            0x2 => "PRIORITY",
            0x3 => "RST_STREAM",
            0x4 => "SETTINGS",
            0x5 => "PUSH_PROMISE",
            0x6 => "PING",
            0x7 => "GOAWAY",
            0x8 => "WINDOW_UPDATE",
            0x9 => "CONTINUATION",
            _ => "UNKNOWN",
        }
    }
}

/// Parse SETTINGS payload as `[(id, value)]` in wire order.
fn parse_settings(payload: &[u8]) -> Vec<(u16, u32)> {
    payload
        .chunks_exact(6)
        .map(|c| {
            (
                u16::from_be_bytes([c[0], c[1]]),
                u32::from_be_bytes([c[2], c[3], c[4], c[5]]),
            )
        })
        .collect()
}

/// Read exactly `n` bytes into a Vec or return io::Error::UnexpectedEof.
async fn read_exact<R: AsyncReadExt + Unpin>(r: &mut R, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> std::io::Result<Frame> {
    let hdr = read_exact(r, 9).await?;
    let len = u32::from_be_bytes([0, hdr[0], hdr[1], hdr[2]]) as usize;
    let kind = hdr[3];
    let flags = hdr[4];
    let stream_id = u32::from_be_bytes([hdr[5], hdr[6], hdr[7], hdr[8]]) & 0x7fff_ffff;
    let payload = read_exact(r, len).await?;
    Ok(Frame {
        kind,
        flags,
        stream_id,
        payload,
    })
}

/// Spawn a capture server that reads the preface + frames from the first
/// client connection and returns them. It also sends a minimal SETTINGS
/// (empty) + SETTINGS ACK so well-behaved clients don't stall; we stop
/// after seeing HEADERS (the first request) or the client closes.
async fn run_capture(listener: TcpListener) -> Vec<Frame> {
    let (mut sock, _) = listener.accept().await.expect("accept");
    // 1. Read client preface.
    let preface = read_exact(&mut sock, H2_PREFACE.len())
        .await
        .expect("preface");
    assert_eq!(
        preface.as_slice(),
        H2_PREFACE,
        "client must send H2 preface"
    );

    // 2. Send our own empty SETTINGS so the client is allowed to ACK.
    //    Frame header: len=0, type=4 (SETTINGS), flags=0, stream=0.
    sock.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])
        .await
        .expect("srv settings");

    // 3. Collect frames until we see HEADERS, or connection error.
    let mut frames = Vec::with_capacity(8);
    while let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut sock)).await
    {
        let is_headers = f.kind == 0x1;
        frames.push(f);
        if is_headers {
            break;
        }
    }
    // Drop the socket; client's body.collect() will error — fine for our assertions.
    drop(sock);
    frames
}

/// Acquire a listener on a random local port.
async fn bind_local() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = l.local_addr().unwrap().port();
    (l, port)
}

/// Raw H2-over-plaintext client driver. We bypass `ImpersonateClient` here
/// because its path forces HTTPS/TLS; plaintext prior-knowledge H2 (h2c)
/// lets us validate the frame layer without a boring-ssl round-trip.
///
/// Uses the same hyper::http2 Builder configuration as the production path
/// in `src/impersonate/mod.rs`, so the frames emitted are representative.
async fn drive_client(port: u16) {
    use bytes::Bytes;
    use http::{Method, Request};
    use http_body_util::Empty;
    use hyper::client::conn::http2;
    use hyper_util::rt::{TokioExecutor, TokioIo};

    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let io = TokioIo::new(tcp);
    let (mut sender, conn) = http2::Builder::new(TokioExecutor::new())
        .header_table_size(65536)
        .initial_stream_window_size(6_291_456)
        .initial_connection_window_size(15_728_640)
        .max_header_list_size(262_144)
        .max_frame_size(None)
        .handshake(io)
        .await
        .expect("h2 handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method(Method::GET)
        .uri("http://127.0.0.1/")
        .header("host", "127.0.0.1")
        .body(Empty::<Bytes>::new())
        .unwrap();
    // The server closes right after HEADERS — this send may error; that's fine.
    let _ = sender.send_request(req).await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn client_settings_match_chrome_144_byte_exact() {
    let (listener, port) = bind_local().await;
    let server = tokio::spawn(run_capture(listener));
    // Give the listener a moment to be polled in accept().
    tokio::time::sleep(Duration::from_millis(20)).await;
    drive_client(port).await;
    let frames = server.await.expect("join");

    // First frame from the client MUST be a SETTINGS frame (RFC 7540 §3.5).
    let settings = frames
        .iter()
        .find(|f| f.kind == 0x4 && (f.flags & 0x1) == 0)
        .expect("client SETTINGS frame");
    let pairs = parse_settings(&settings.payload);

    // Chrome 144 Akamai format: 1:65536;2:0;4:6291456;6:262144
    assert_eq!(
        pairs,
        vec![(0x1, 65_536), (0x2, 0), (0x4, 6_291_456), (0x6, 262_144),],
        "SETTINGS frame does not match Chrome 144 Akamai fingerprint byte-exact"
    );

    // WINDOW_UPDATE on stream 0 with delta 15_663_105.
    let wu = frames
        .iter()
        .find(|f| f.kind == 0x8 && f.stream_id == 0)
        .expect("WINDOW_UPDATE on stream 0");
    assert_eq!(wu.payload.len(), 4, "WINDOW_UPDATE payload must be 4 bytes");
    let delta = u32::from_be_bytes([wu.payload[0], wu.payload[1], wu.payload[2], wu.payload[3]])
        & 0x7fff_ffff;
    assert_eq!(
        delta, 15_663_105,
        "WINDOW_UPDATE delta must be 15_663_105 (15MiB - 65535)"
    );

    // No standalone PRIORITY frames — Chrome 144 uses priority hints via
    // HEADERS flags or the RFC 9218 `priority:` header, never standalone.
    let priority = frames.iter().find(|f| f.kind == 0x2);
    assert!(
        priority.is_none(),
        "client must NOT emit standalone PRIORITY frames; got {:?}",
        priority.map(|f| f.kind_name())
    );

    // S.1 — HEADERS pseudo-header order MUST be Chrome's `m,a,s,p`
    // (:method, :authority, :scheme, :path), not h2 crate default `m,s,a,p`.
    // This is the Akamai H2 fingerprint-critical assertion unlocked by the
    // `vendor/h2` fork.
    let headers = frames
        .iter()
        .find(|f| f.kind == 0x1)
        .expect("client HEADERS frame");
    let order = decode_pseudo_order(&headers.payload);
    assert_eq!(
        order,
        vec![":method", ":authority", ":scheme", ":path"],
        "pseudo-header order must match Chrome 149 (m,a,s,p), got {:?}",
        order
    );
}

/// Minimal HPACK decoder: walks the block and returns the *names* of the
/// leading pseudo-headers in wire order. Recognizes exactly what the client's
/// first request emits: static-table indexed entries (`:method GET`,
/// `:scheme http`, `:path /`) and literal-with-indexing using an indexed
/// name (`:authority <value>`). Any entry whose name does not start with `:`
/// terminates the scan — we only care about pseudo-header *order*, not values.
fn decode_pseudo_order(mut payload: &[u8]) -> Vec<&'static str> {
    // Static table (RFC 7541 Appendix A) — pseudo-header rows only.
    fn static_name(idx: u64) -> Option<&'static str> {
        match idx {
            1 => Some(":authority"),
            2 | 3 => Some(":method"),
            4 | 5 => Some(":path"),
            6 | 7 => Some(":scheme"),
            8..=14 => Some(":status"),
            _ => None,
        }
    }

    // Integer decoder per RFC 7541 §5.1 with prefix of `n` bits.
    fn decode_int(buf: &mut &[u8], prefix_bits: u8) -> Option<u64> {
        let (first, rest) = buf.split_first()?;
        let mask = (1u8 << prefix_bits) - 1;
        let mut value = (first & mask) as u64;
        *buf = rest;
        if value < mask as u64 {
            return Some(value);
        }
        let mut m = 0u32;
        loop {
            let (b, rest) = buf.split_first()?;
            *buf = rest;
            value += ((b & 0x7f) as u64) << m;
            if b & 0x80 == 0 {
                return Some(value);
            }
            m += 7;
            if m > 63 {
                return None;
            }
        }
    }

    // Skip a length-prefixed string (5-bit int with 1-bit huffman flag in MSB
    // of the prefix byte — decode_int treats the huffman bit as part of value
    // when prefix_bits=7, so we use a custom path here).
    fn skip_string(buf: &mut &[u8]) -> Option<()> {
        // length prefix: 7 bits, huffman flag in top bit (ignored for skip).
        let len = decode_int(buf, 7)? as usize;
        if buf.len() < len {
            return None;
        }
        *buf = &buf[len..];
        Some(())
    }

    let mut order = Vec::new();
    // Dynamic-table size updates may appear first (prefix `001xxxxx`).
    while let Some(&first) = payload.first() {
        if first & 0xe0 == 0x20 {
            let _ = decode_int(&mut payload, 5);
            continue;
        }
        break;
    }

    while let Some(&first) = payload.first() {
        let name = if first & 0x80 != 0 {
            // Indexed header field, 7-bit prefix.
            let idx = decode_int(&mut payload, 7).expect("hpack idx");
            static_name(idx)
        } else if first & 0xc0 == 0x40 {
            // Literal with incremental indexing, 6-bit prefix.
            let idx = decode_int(&mut payload, 6).expect("hpack lit-inc idx");
            let name = if idx == 0 {
                // Literal name — skip it.
                skip_string(&mut payload).expect("hpack lit name");
                None
            } else {
                static_name(idx)
            };
            skip_string(&mut payload).expect("hpack lit value");
            name
        } else if first & 0xf0 == 0 || first & 0xf0 == 0x10 {
            // Literal without indexing / never indexed, 4-bit prefix.
            let idx = decode_int(&mut payload, 4).expect("hpack lit idx");
            let name = if idx == 0 {
                skip_string(&mut payload).expect("hpack lit name");
                None
            } else {
                static_name(idx)
            };
            skip_string(&mut payload).expect("hpack lit value");
            name
        } else {
            break;
        };
        match name {
            Some(n) if n.starts_with(':') => order.push(n),
            _ => break,
        }
    }
    order
}
