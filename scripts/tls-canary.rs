//! TLS canary — captures raw ClientHello bytes from incoming TCP connections.
//!
//! Run via:
//!   cargo run --bin tls-canary -- --listen 127.0.0.1:8443 --out /tmp/captures
//!   cargo run --bin tls-canary -- --listen 127.0.0.1:8443 --label chrome_149_linux
//!
//! Listens for TCP connections, reads the first TLS record (handshake type
//! 0x16 = ClientHello), writes the raw bytes to a `.bin` file, then closes.
//! No reply is sent — the browser will time out after a few seconds, which
//! is fine. We only need the ClientHello bytes.
//!
//! For each connection we emit:
//!   <out>/<label>_<unix_ts>.bin       — raw TLS record bytes (ClientHello)
//!   <out>/<label>_<unix_ts>.meta.json — { remote_addr, sni, captured_at }
//!
//! Companion script: `scripts/yaml-from-capture.mjs` consumes `.bin` files
//! and emits curl-impersonate-compatible YAML signatures.
//!
//! NOT a Cargo binary — invoke directly with `rustc scripts/tls-canary.rs`
//! or via `cargo +nightly -Zscript run --manifest-path scripts/tls-canary.rs`
//! once stable. Self-contained zero-dep code below.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Args {
    listen: String,
    out: PathBuf,
    label: String,
}

fn parse_args() -> Args {
    let mut listen = String::from("127.0.0.1:8443");
    let mut out = PathBuf::from("/tmp/crawlex-tls-captures");
    let mut label = String::from("anonymous");

    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--listen" => {
                if let Some(v) = iter.next() {
                    listen = v;
                }
            }
            "--out" => {
                if let Some(v) = iter.next() {
                    out = PathBuf::from(v);
                }
            }
            "--label" => {
                if let Some(v) = iter.next() {
                    label = v;
                }
            }
            "--help" | "-h" => {
                eprintln!(
                    "tls-canary — capture raw ClientHello bytes\n\n\
                     Usage:\n  \
                       tls-canary --listen 127.0.0.1:8443 --out /tmp/captures --label chrome_149_linux\n\n\
                     Options:\n  \
                       --listen <ADDR:PORT>  Listen address (default 127.0.0.1:8443)\n  \
                       --out <DIR>           Output directory (default /tmp/crawlex-tls-captures)\n  \
                       --label <NAME>        Label prefix for output files (default 'anonymous')\n"
                );
                std::process::exit(0);
            }
            _ => eprintln!("unknown arg: {}", arg),
        }
    }
    Args { listen, out, label }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Try to extract the SNI hostname from a TLS ClientHello byte slice.
/// Returns None if the bytes don't look like a valid ClientHello with SNI.
fn extract_sni(bytes: &[u8]) -> Option<String> {
    // Skip TLS record header (5 bytes: type + version[2] + length[2]).
    if bytes.len() < 5 || bytes[0] != 0x16 {
        return None;
    }
    let payload = &bytes[5..];
    // Skip handshake header (4 bytes: type + length[3]).
    if payload.len() < 4 || payload[0] != 0x01 {
        return None;
    }
    let body = &payload[4..];
    // Skip: legacy_version (2) + random (32) + session_id_length (1).
    if body.len() < 35 {
        return None;
    }
    let mut p = 35;
    let session_id_len = body[34] as usize;
    p += session_id_len;
    if p + 2 > body.len() {
        return None;
    }
    // Cipher suites length (2) + cipher suites.
    let cipher_len = ((body[p] as usize) << 8) | body[p + 1] as usize;
    p += 2 + cipher_len;
    if p + 1 > body.len() {
        return None;
    }
    // Compression methods length (1) + methods.
    let comp_len = body[p] as usize;
    p += 1 + comp_len;
    if p + 2 > body.len() {
        return None;
    }
    // Extensions length (2) + extensions.
    let ext_total = ((body[p] as usize) << 8) | body[p + 1] as usize;
    p += 2;
    let ext_end = p + ext_total;
    while p + 4 <= ext_end && p + 4 <= body.len() {
        let ext_type = ((body[p] as u16) << 8) | body[p + 1] as u16;
        let ext_len = ((body[p + 2] as usize) << 8) | body[p + 3] as usize;
        p += 4;
        if ext_type == 0x0000 {
            // server_name extension. Layout:
            //   list_length (2) + name_type (1=host_name) + name_length (2) + name.
            if ext_len < 5 {
                return None;
            }
            let name_off = p + 5;
            let name_len_off = p + 3;
            if name_off >= body.len() || name_len_off + 2 > body.len() {
                return None;
            }
            let name_len =
                ((body[name_len_off] as usize) << 8) | body[name_len_off + 1] as usize;
            if name_off + name_len > body.len() {
                return None;
            }
            return Some(
                String::from_utf8_lossy(&body[name_off..name_off + name_len]).to_string(),
            );
        }
        p += ext_len;
    }
    None
}

fn handle_connection(
    args: &Args,
    mut stream: std::net::TcpStream,
    remote: SocketAddr,
) -> std::io::Result<()> {
    // Read up to 8 KiB — TLS ClientHello max is theoretical 16 KiB but in
    // practice always under 2 KiB. 8 KiB covers padding-heavy variants.
    let mut buf = vec![0u8; 8 * 1024];
    let n = stream.read(&mut buf)?;
    buf.truncate(n);

    if n == 0 {
        eprintln!("[canary] {} sent zero bytes — skipping", remote);
        return Ok(());
    }

    let sni = extract_sni(&buf).unwrap_or_else(|| "?".into());
    let ts = now_unix();
    let stem = format!("{}_{}", args.label, ts);
    fs::create_dir_all(&args.out)?;
    let bin_path = args.out.join(format!("{}.bin", stem));
    let meta_path = args.out.join(format!("{}.meta.json", stem));

    fs::write(&bin_path, &buf)?;
    fs::write(
        &meta_path,
        format!(
            r#"{{"label":"{label}","remote_addr":"{remote}","sni":"{sni}","captured_at":{ts},"bytes":{bytes}}}"#,
            label = args.label,
            remote = remote,
            sni = sni,
            ts = ts,
            bytes = n
        ),
    )?;
    eprintln!(
        "[canary] captured {} bytes from {} (sni={}) → {}",
        n,
        remote,
        sni,
        bin_path.display()
    );

    // Send a TLS alert (handshake_failure = 40) so the browser drops the
    // connection cleanly instead of hanging on read timeout. Wire format:
    //   record_type=21 (alert), version=0x0303, length=2, level=2, desc=40.
    let _ = stream.write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28]);
    Ok(())
}

fn main() -> std::io::Result<()> {
    let args = parse_args();
    eprintln!(
        "[canary] listening on {} → out {}",
        args.listen,
        args.out.display()
    );
    let listener = TcpListener::bind(&args.listen)?;
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let remote = stream
                    .peer_addr()
                    .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
                if let Err(e) = handle_connection(&args, stream, remote) {
                    eprintln!("[canary] connection error: {}", e);
                }
            }
            Err(e) => eprintln!("[canary] accept error: {}", e),
        }
    }
    Ok(())
}
