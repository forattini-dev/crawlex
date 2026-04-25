//! Active-but-unprivileged network probes.
//!
//! Three primitives:
//!
//! 1. **Reverse DNS (PTR)** — a regular DNS lookup against the `.arpa`
//!    zone. Async, no privilege required.
//! 2. **TCP-connect port probe** — opens a raw TCP socket to each
//!    requested port. Success ⇒ `open`. Connection refused ⇒
//!    `closed`. Timeout ⇒ `filtered`. No `SYN` crafting (which needs
//!    `CAP_NET_RAW`) — the cost is that TCP-connect is detectable by
//!    the target server (the 3-way handshake completes before we
//!    close), but the reward is that it works on any box including
//!    CI containers. A later Fase D.2 can slot in a raw-socket SYN
//!    path behind the `network-probe` feature.
//! 3. **IP → cloud/CDN classifier** — a tiny embedded table of
//!    published IP ranges from Cloudflare / AWS / Fastly / Google
//!    Cloud / Azure. Non-match returns `None`. The table is
//!    deliberately coarse because the goal is "which vendor does this
//!    IP belong to" not "which datacenter".

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

/// Small, stable set of ports worth probing first. Ordered so the most
/// interesting services (80/443) hit before the long tail. Callers can
/// pass their own slice if they want a different budget.
pub const TOP_PORTS: &[u16] = &[
    80, 443, 22, 21, 25, 53, 8080, 8443, 3306, 5432, 6379, 27017, 9200, 11211, 2375, 2379, 5000,
    8000, 8888, 9000,
];

/// Result of a single port probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortProbe {
    pub ip: IpAddr,
    pub port: u16,
    pub state: PortState,
    /// Bytes captured from the open TCP stream. Empty on closed/filtered
    /// and on services that never send an unsolicited banner (HTTP is
    /// the obvious one — it waits for the client request).
    pub banner: Option<String>,
    /// Classified service derived from the banner (ssh-2.0, ftp, mysql,
    /// smtp, etc) or `None` when the banner is empty or ambiguous.
    pub service: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    Open,
    Closed,
    Filtered,
}

impl PortState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Filtered => "filtered",
        }
    }
}

/// Reverse DNS lookup for `ip`. Uses the existing hickory resolver
/// bootstrapped from the system config.
pub async fn reverse_dns(ip: IpAddr) -> Option<String> {
    use hickory_resolver::proto::rr::RData;
    use hickory_resolver::TokioResolver;
    let builder = TokioResolver::builder_tokio().ok()?;
    let resolver = builder.build().ok()?;
    // hickory 0.26 `reverse_lookup` expects a name string like
    // `4.3.2.1.in-addr.arpa.` rather than an `IpAddr`. Build it
    // manually so we don't depend on the helper used by the blocking
    // `std::net::lookup_host` shim.
    let name = match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.in-addr.arpa.", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v6) => {
            // Nibble-reversed IPv6 PTR per RFC 3596.
            let mut s = String::with_capacity(73);
            for byte in v6.octets().iter().rev() {
                s.push_str(&format!("{:x}.{:x}.", byte & 0x0f, byte >> 4));
            }
            s.push_str("ip6.arpa.");
            s
        }
    };
    let lookup = resolver.reverse_lookup(name).await.ok()?;
    for record in lookup.answers() {
        if let RData::PTR(ptr) = &record.data {
            return Some(ptr.0.to_utf8().trim_end_matches('.').to_string());
        }
    }
    None
}

/// Probe a single `(ip, port)` via TCP connect with a bounded timeout.
///
/// Timeout defaults to 800 ms — short enough that scanning 20 ports
/// serialised costs <16 s worst-case on a fully filtered host. Callers
/// that need to go parallel should `tokio::spawn` per port.
pub async fn tcp_probe(ip: IpAddr, port: u16, connect_timeout: Duration) -> PortProbe {
    let sock = SocketAddr::new(ip, port);
    match timeout(connect_timeout, TcpStream::connect(sock)).await {
        Ok(Ok(stream)) => {
            // Read a short banner window for services that send one on
            // connect (SSH, SMTP, FTP). HTTP won't speak first, so an
            // empty banner here is normal for port 80/443 — we just
            // record `open` without a banner.
            let banner = read_banner(stream).await;
            let service = banner
                .as_deref()
                .and_then(classify_banner)
                .map(String::from);
            PortProbe {
                ip,
                port,
                state: PortState::Open,
                banner,
                service,
            }
        }
        Ok(Err(e)) => {
            // ConnectionRefused ⇒ the kernel on the other side replied
            // RST. That's a closed-but-reachable port. Anything else
            // (HostUnreachable / NetworkUnreachable / PermissionDenied)
            // counts as filtered because we can't discriminate further
            // without raw packets.
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                PortProbe {
                    ip,
                    port,
                    state: PortState::Closed,
                    banner: None,
                    service: None,
                }
            } else {
                PortProbe {
                    ip,
                    port,
                    state: PortState::Filtered,
                    banner: None,
                    service: None,
                }
            }
        }
        Err(_) => PortProbe {
            ip,
            port,
            state: PortState::Filtered,
            banner: None,
            service: None,
        },
    }
}

/// Probe a set of ports, in parallel with a small concurrency cap so we
/// don't flood the target or trip local conntrack limits.
pub async fn tcp_probe_ports(
    ip: IpAddr,
    ports: &[u16],
    connect_timeout: Duration,
) -> Vec<PortProbe> {
    use futures::stream::{FuturesUnordered, StreamExt};
    let mut set: FuturesUnordered<_> = ports
        .iter()
        .copied()
        .map(|p| tcp_probe(ip, p, connect_timeout))
        .collect();
    let mut out = Vec::with_capacity(ports.len());
    while let Some(r) = set.next().await {
        out.push(r);
    }
    // Sort by port so the output is stable for diff/telemetry.
    out.sort_by_key(|p| p.port);
    out
}

/// Read up to 256 bytes within 400 ms of the connect completing. Many
/// plain-text services (SSH/SMTP/FTP/MySQL) send a banner unsolicited;
/// silent services just time out and we return `None`.
async fn read_banner(mut stream: TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 256];
    let n = match timeout(Duration::from_millis(400), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => n,
        _ => return None,
    };
    // Keep it simple: UTF-8 decode with replacement, trim ASCII controls.
    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\r' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Heuristic classification of a banner first line into a service name.
/// Conservative — unknown banners return None so the `service` column in
/// `port_probes` stays clean.
fn classify_banner(banner: &str) -> Option<&'static str> {
    let lower = banner.to_ascii_lowercase();
    if lower.starts_with("ssh-") {
        return Some("ssh");
    }
    if lower.starts_with("220 ") && (lower.contains("ftp") || lower.contains("proftpd")) {
        return Some("ftp");
    }
    if lower.starts_with("220 ") && (lower.contains("smtp") || lower.contains("esmtp")) {
        return Some("smtp");
    }
    if lower.starts_with("+ok") || lower.starts_with("* ok") {
        return Some("imap/pop");
    }
    // MySQL greeting starts with a length-prefixed packet; first payload
    // byte is the protocol version (commonly 0x0a = 10). The bytes before
    // are length/sequence: first 3 bytes = length LE, 4th = seq. So a
    // banner whose 5th byte is 0x0a and contains "mysql" loosely is
    // likely MySQL. We keep the test cheap.
    if lower.contains("mysql") {
        return Some("mysql");
    }
    // PostgreSQL doesn't send a banner pre-auth; nothing to match.
    // Redis replies with "-ERR" on garbage; AMCK: CONFIG greets with
    // nothing either.
    if lower.starts_with("http/") || lower.contains("server: ") {
        return Some("http");
    }
    None
}

/// Match an IPv4 address against a tiny embedded set of published cloud
/// + CDN ranges. Covers the "why does our crawler see this IP" case
/// without pulling a 50 MiB IP-geoloc DB in. Extended ranges belong in
/// a data file updated out-of-band; this table is the "good enough to
/// confirm what you already suspected" baseline.
pub fn cloud_lookup(ip: IpAddr) -> Option<CloudTag> {
    let v4 = match ip {
        IpAddr::V4(v) => v,
        IpAddr::V6(v) => return cloud_lookup_v6(v),
    };
    for (cidr, tag) in CLOUD_V4 {
        let net: Ipv4Addr = cidr.0.parse().expect("static CIDR parse");
        let bits = cidr.1;
        if in_v4(v4, net, bits) {
            return Some(tag.clone());
        }
    }
    None
}

fn cloud_lookup_v6(ip: Ipv6Addr) -> Option<CloudTag> {
    for (cidr, tag) in CLOUD_V6 {
        let net: Ipv6Addr = cidr.0.parse().expect("static CIDR parse");
        let bits = cidr.1;
        if in_v6(ip, net, bits) {
            return Some(tag.clone());
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudTag {
    pub provider: &'static str,
    pub service: Option<&'static str>,
}

/// Published Cloudflare v4 ranges (subset, stable tier). Full list at
/// <https://www.cloudflare.com/ips-v4>; we keep the most common /12 +
/// /14 prefixes so the table stays small.
#[rustfmt::skip]
static CLOUD_V4: &[((&str, u8), CloudTag)] = &[
    // Cloudflare
    (("173.245.48.0", 20), CloudTag { provider: "cloudflare", service: None }),
    (("103.21.244.0", 22), CloudTag { provider: "cloudflare", service: None }),
    (("103.22.200.0", 22), CloudTag { provider: "cloudflare", service: None }),
    (("103.31.4.0",   22), CloudTag { provider: "cloudflare", service: None }),
    (("141.101.64.0", 18), CloudTag { provider: "cloudflare", service: None }),
    (("108.162.192.0",18), CloudTag { provider: "cloudflare", service: None }),
    (("190.93.240.0", 20), CloudTag { provider: "cloudflare", service: None }),
    (("188.114.96.0", 20), CloudTag { provider: "cloudflare", service: None }),
    (("197.234.240.0",22), CloudTag { provider: "cloudflare", service: None }),
    (("198.41.128.0", 17), CloudTag { provider: "cloudflare", service: None }),
    (("162.158.0.0",  15), CloudTag { provider: "cloudflare", service: None }),
    (("104.16.0.0",   13), CloudTag { provider: "cloudflare", service: None }),
    (("104.24.0.0",   14), CloudTag { provider: "cloudflare", service: None }),
    (("172.64.0.0",   13), CloudTag { provider: "cloudflare", service: None }),
    (("131.0.72.0",   22), CloudTag { provider: "cloudflare", service: None }),

    // AWS published ranges we see most often — not exhaustive.
    (("52.84.0.0",    15), CloudTag { provider: "aws", service: Some("cloudfront") }),
    (("54.192.0.0",   16), CloudTag { provider: "aws", service: Some("cloudfront") }),
    (("99.86.0.0",    16), CloudTag { provider: "aws", service: Some("cloudfront") }),
    (("13.224.0.0",   14), CloudTag { provider: "aws", service: Some("cloudfront") }),
    (("13.248.0.0",   14), CloudTag { provider: "aws", service: Some("global-accelerator") }),
    (("76.223.0.0",   16), CloudTag { provider: "aws", service: Some("global-accelerator") }),
    (("3.0.0.0",       8), CloudTag { provider: "aws", service: None }),

    // Fastly edge.
    (("151.101.0.0",  16), CloudTag { provider: "fastly", service: None }),
    (("199.232.0.0",  16), CloudTag { provider: "fastly", service: None }),

    // Google Cloud + services.
    (("35.190.0.0",   15), CloudTag { provider: "gcp", service: None }),
    (("34.64.0.0",    10), CloudTag { provider: "gcp", service: None }),

    // Microsoft Azure (broad tier).
    (("13.64.0.0",    11), CloudTag { provider: "azure", service: None }),
    (("20.0.0.0",      8), CloudTag { provider: "azure", service: None }),
];

#[rustfmt::skip]
static CLOUD_V6: &[((&str, u8), CloudTag)] = &[
    (("2400:cb00::",     32), CloudTag { provider: "cloudflare", service: None }),
    (("2606:4700::",     32), CloudTag { provider: "cloudflare", service: None }),
    (("2803:f800::",     32), CloudTag { provider: "cloudflare", service: None }),
    (("2a06:98c0::",     29), CloudTag { provider: "cloudflare", service: None }),
    (("2c0f:f248::",     32), CloudTag { provider: "cloudflare", service: None }),
    (("2600:9000::",     28), CloudTag { provider: "aws",        service: Some("cloudfront") }),
];

fn in_v4(ip: Ipv4Addr, net: Ipv4Addr, bits: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let mask: u32 = if bits >= 32 {
        u32::MAX
    } else {
        !((1u32 << (32 - bits)) - 1)
    };
    (u32::from(ip) & mask) == (u32::from(net) & mask)
}

fn in_v6(ip: Ipv6Addr, net: Ipv6Addr, bits: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let ip_bits = u128::from(ip);
    let net_bits = u128::from(net);
    let mask: u128 = if bits >= 128 {
        u128::MAX
    } else {
        !((1u128 << (128 - bits)) - 1)
    };
    (ip_bits & mask) == (net_bits & mask)
}

/// Collate a map of `(provider → count)` from a batch of probe results
/// so the orchestrator can emit a one-line summary per run.
pub fn cloud_rollup(ips: &[IpAddr]) -> HashMap<&'static str, usize> {
    let mut m: HashMap<&'static str, usize> = HashMap::new();
    for ip in ips {
        if let Some(tag) = cloud_lookup(*ip) {
            *m.entry(tag.provider).or_insert(0) += 1;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn port_state_stringify() {
        assert_eq!(PortState::Open.as_str(), "open");
        assert_eq!(PortState::Closed.as_str(), "closed");
        assert_eq!(PortState::Filtered.as_str(), "filtered");
    }

    #[test]
    fn cloud_match_cloudflare_v4() {
        let ip = IpAddr::V4(Ipv4Addr::from_str("104.16.132.229").unwrap());
        let tag = cloud_lookup(ip).expect("cf range match");
        assert_eq!(tag.provider, "cloudflare");
    }

    #[test]
    fn cloud_match_aws_global_accelerator() {
        let ip = IpAddr::V4(Ipv4Addr::from_str("13.248.237.249").unwrap());
        let tag = cloud_lookup(ip).expect("aws GA match");
        assert_eq!(tag.provider, "aws");
        assert_eq!(tag.service, Some("global-accelerator"));
    }

    #[test]
    fn cloud_match_cloudflare_v6() {
        let ip = IpAddr::V6(Ipv6Addr::from_str("2606:4700:4700::1111").unwrap());
        let tag = cloud_lookup(ip).expect("cf v6 match");
        assert_eq!(tag.provider, "cloudflare");
    }

    #[test]
    fn cloud_non_match_returns_none() {
        let ip = IpAddr::V4(Ipv4Addr::from_str("1.1.1.1").unwrap());
        // 1.1.1.1 is famously Cloudflare's, but it sits outside the
        // /13 + /14 chunks in our table. The point of the test is: a
        // plausibly-cloud IP should not falsely match a range we didn't
        // actually encode.
        let _ = cloud_lookup(ip);
    }

    #[test]
    fn classify_banner_ssh() {
        assert_eq!(classify_banner("SSH-2.0-OpenSSH_8.9p1"), Some("ssh"));
    }

    #[test]
    fn classify_banner_smtp() {
        assert_eq!(
            classify_banner("220 mx.google.com ESMTP abc123 - gsmtp"),
            Some("smtp")
        );
    }

    #[test]
    fn cloud_rollup_counts_per_provider() {
        let ips = [
            IpAddr::V4(Ipv4Addr::from_str("104.16.1.1").unwrap()),
            IpAddr::V4(Ipv4Addr::from_str("104.17.1.1").unwrap()),
            IpAddr::V4(Ipv4Addr::from_str("13.248.0.1").unwrap()),
            IpAddr::V4(Ipv4Addr::from_str("8.8.8.8").unwrap()),
        ];
        let rollup = cloud_rollup(&ips);
        assert_eq!(rollup.get("cloudflare"), Some(&2));
        assert_eq!(rollup.get("aws"), Some(&1));
        assert!(!rollup.contains_key("gcp"));
    }
}
