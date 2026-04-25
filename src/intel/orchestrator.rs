//! Orchestrator for target-scoped intel gathering.
//!
//! Reads a `target` (registrable domain), walks the passive stages, and
//! emits a final `IntelReport` with counts per category. Every row it
//! collects is persisted via direct SQLite writes on its own connection
//! (separate from the crawler mpsc writer) so `crawlex intel` can run
//! without the full crawler lifecycle.
//!
//! This is the Fase B cut: subdomain (crt.sh) + DNS + WHOIS + TLS cert.
//! CT-logs history, reverse IP, and server fingerprint land in Fase B.2.

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::config::InfraIntelConfig;
use crate::discovery::{
    cert::PeerCert,
    dns::DnsFacts,
    network_probe::{cloud_lookup, reverse_dns, tcp_probe_ports, PortProbe, TOP_PORTS},
    subdomains::{certspotter_subdomains, crtsh_subdomains, hackertarget_subdomains},
};
use crate::impersonate::{ImpersonateClient, Profile};
use crate::{Error, Result};

/// Which stage ran and how it fared. Emitted per-domain so the operator
/// can see progress and per-stage cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IntelStage {
    Subdomains,
    Dns,
    Whois,
    Cert,
}

impl IntelStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subdomains => "subdomains",
            Self::Dns => "dns",
            Self::Whois => "whois",
            Self::Cert => "cert",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntelReport {
    pub target_root: String,
    pub subdomains: Vec<String>,
    pub dns_record_count: usize,
    pub unique_ips: Vec<String>,
    pub certs_captured: usize,
    pub whois_registrar: Option<String>,
    pub whois_created: Option<String>,
    pub whois_expires: Option<String>,
    pub errors: Vec<String>,
    pub elapsed_ms: u64,
}

/// Holds the sqlite connection + the impersonate client used for RDAP
/// and (future) HTTP banner probes. The TLS handshake reuses the same
/// impersonate client because it already owns the boringssl profile we
/// want to present to the target.
pub struct TargetIntelOrchestrator {
    conn: Connection,
    http: Arc<ImpersonateClient>,
    cfg: InfraIntelConfig,
}

impl TargetIntelOrchestrator {
    /// Open the SQLite DB at `db_path` (creating tables via the existing
    /// `SqliteStorage::open` migration path) and bring up an
    /// impersonate client. `cfg.target_domain` must be set by the
    /// caller; we don't read it here because the CLI surface passes
    /// the target as a positional arg.
    pub fn open(db_path: &Path, cfg: InfraIntelConfig) -> Result<Self> {
        // Run the SqliteStorage migration via the storage layer so
        // schema stays in sync; then drop back to a raw rusqlite conn
        // for the orchestrator's synchronous writes.
        #[cfg(feature = "sqlite")]
        {
            let _ = crate::storage::sqlite::SqliteStorage::open(db_path)
                .map_err(|e| Error::Storage(format!("open: {e}")))?;
        }
        let conn =
            Connection::open(db_path).map_err(|e| Error::Storage(format!("rusqlite open: {e}")))?;
        let http = Arc::new(
            ImpersonateClient::new(Profile::Chrome131Stable)
                .map_err(|e| Error::Config(format!("impersonate client: {e}")))?,
        );
        Ok(Self { conn, http, cfg })
    }

    /// Run the full pipeline against `target_root` (already validated
    /// as a registrable domain). Returns the populated report.
    pub async fn run(&mut self, target_root: &str) -> Result<IntelReport> {
        let start = std::time::Instant::now();
        let mut report = IntelReport {
            target_root: target_root.to_string(),
            ..Default::default()
        };

        // ---- Stage 1: enumerate subdomains ----
        let mut all_domains: BTreeSet<String> = BTreeSet::new();
        all_domains.insert(target_root.to_string());
        if self.cfg.subdomains {
            // Run all three sources and union the results. Each is
            // independent — a 502 from crt.sh (common) doesn't stop the
            // other two from contributing. Per-source errors are
            // collected so the operator can tell which failed.
            match crtsh_subdomains(&self.http, target_root).await {
                Ok(subs) => {
                    for s in subs {
                        all_domains.insert(s);
                    }
                }
                Err(e) => report
                    .errors
                    .push(format!("[subdomains] crt.sh failed: {e}")),
            }
            match certspotter_subdomains(&self.http, target_root).await {
                Ok(subs) => {
                    for s in subs {
                        all_domains.insert(s);
                    }
                }
                Err(e) => report
                    .errors
                    .push(format!("[subdomains] certspotter failed: {e}")),
            }
            match hackertarget_subdomains(&self.http, target_root).await {
                Ok(subs) => {
                    for s in subs {
                        all_domains.insert(s);
                    }
                }
                Err(e) => report
                    .errors
                    .push(format!("[subdomains] hackertarget failed: {e}")),
            }
        }
        report.subdomains = all_domains
            .iter()
            .filter(|d| d.as_str() != target_root)
            .cloned()
            .collect();

        // Persist `domains` rows. `is_subdomain` is true when the full
        // name differs from the target root; `is_wildcard_dns` updates
        // in stage 2 if a nonce probe confirms wildcard behaviour.
        for d in &all_domains {
            let is_sub = if d == target_root { 0 } else { 1 };
            self.conn
                .execute(
                    "INSERT INTO domains (domain, target_root, is_subdomain) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(domain) DO UPDATE SET last_probed = strftime('%s','now')",
                    params![d, target_root, is_sub],
                )
                .map_err(|e| Error::Storage(format!("domains insert: {e}")))?;
        }

        // ---- Stage 2: DNS per domain + wildcard probe on target_root ----
        let mut seen_ips: HashMap<String, BTreeSet<IpAddr>> = HashMap::new();
        let mut total_records = 0usize;
        if self.cfg.dns {
            for d in &all_domains {
                let facts = crate::discovery::dns::lookup(d).await;
                total_records += self.persist_dns(d, &facts)?;
                let mut ips: BTreeSet<IpAddr> = BTreeSet::new();
                ips.extend(facts.a.iter().copied());
                ips.extend(facts.aaaa.iter().copied());
                seen_ips.insert(d.clone(), ips);
            }
            // Wildcard probe on target_root: resolve 3 nonces; if all
            // three return the SAME non-empty IP set, flag wildcard DNS.
            if self.is_wildcard_dns(target_root).await {
                self.conn
                    .execute(
                        "UPDATE domains SET is_wildcard_dns = 1 WHERE domain = ?1",
                        params![target_root],
                    )
                    .map_err(|e| Error::Storage(format!("wildcard flag: {e}")))?;
            }
        }
        report.dns_record_count = total_records;

        // Persist IPs + domain_ips N:N.
        let mut all_ips: BTreeSet<IpAddr> = BTreeSet::new();
        for (domain, ips) in &seen_ips {
            for ip in ips {
                all_ips.insert(*ip);
                self.conn
                    .execute(
                        "INSERT OR IGNORE INTO ip_addresses (ip) VALUES (?1)",
                        params![ip.to_string()],
                    )
                    .map_err(|e| Error::Storage(format!("ip insert: {e}")))?;
                self.conn
                    .execute(
                        "INSERT OR IGNORE INTO domain_ips (domain, ip) VALUES (?1, ?2)",
                        params![domain, ip.to_string()],
                    )
                    .map_err(|e| Error::Storage(format!("domain_ips insert: {e}")))?;
            }
        }
        report.unique_ips = all_ips.iter().map(|ip| ip.to_string()).collect();

        // ---- Stage 3: WHOIS/RDAP on target_root (parent) ----
        if self.cfg.whois {
            match crate::discovery::whois::lookup(&self.http, target_root).await {
                Ok(reg) => {
                    report.whois_registrar = reg.registrar.clone();
                    report.whois_created = reg.created.clone();
                    report.whois_expires = reg.expires.clone();
                    let created = parse_iso_seconds(reg.created.as_deref());
                    let expires = parse_iso_seconds(reg.expires.as_deref());
                    let updated = parse_iso_seconds(reg.last_changed.as_deref());
                    let ns_json =
                        serde_json::to_string(&reg.name_servers).unwrap_or_else(|_| "[]".into());
                    let status_json =
                        serde_json::to_string(&reg.status).unwrap_or_else(|_| "[]".into());
                    self.conn
                        .execute(
                            "INSERT INTO whois_records \
                             (domain, registrar, registrant_org, created_at, expires_at, \
                              updated_at, nameservers_json, status_json, abuse_email, raw_json) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                             ON CONFLICT(domain) DO UPDATE SET \
                               registrar=excluded.registrar, \
                               registrant_org=excluded.registrant_org, \
                               expires_at=excluded.expires_at, \
                               updated_at=excluded.updated_at, \
                               nameservers_json=excluded.nameservers_json, \
                               status_json=excluded.status_json, \
                               abuse_email=excluded.abuse_email, \
                               observed_at=strftime('%s','now')",
                            params![
                                target_root,
                                reg.registrar,
                                reg.registrant_org,
                                created,
                                expires,
                                updated,
                                ns_json,
                                status_json,
                                reg.abuse_emails.first().cloned(),
                                serde_json::to_string(&reg).ok()
                            ],
                        )
                        .map_err(|e| Error::Storage(format!("whois insert: {e}")))?;
                }
                Err(e) => report.errors.push(format!("[whois] {e}")),
            }
        }

        // ---- Stage 4: TLS handshake + cert per subdomain with an A record ----
        if self.cfg.cert {
            for d in &all_domains {
                // Skip domains with no A/AAAA — handshake would fail
                // on DNS; save time.
                let has_ip = seen_ips.get(d).map(|s| !s.is_empty()).unwrap_or(false);
                if !has_ip {
                    continue;
                }
                match self.grab_cert(d).await {
                    Ok(Some(cert)) => {
                        self.persist_cert(d, &cert)?;
                        report.certs_captured += 1;
                    }
                    Ok(None) => {}
                    Err(e) => report.errors.push(format!("[cert {d}] {e}")),
                }
            }
        }

        // ---- Stage 5: active network probes (reverse DNS + cloud tag +
        // TCP-connect port scan). Fully unprivileged — no raw sockets,
        // no CAP_NET_RAW. Gated behind `cfg.network_probe` because even
        // TCP-connect is visible to the target server.
        if self.cfg.network_probe {
            let ips: Vec<IpAddr> = seen_ips.values().flatten().copied().collect();
            let mut unique: BTreeSet<IpAddr> = BTreeSet::new();
            unique.extend(ips);
            for ip in &unique {
                // Reverse DNS + cloud tag are cheap — always run.
                let ptr = reverse_dns(*ip).await;
                let cloud = cloud_lookup(*ip);
                let cloud_provider = cloud.as_ref().map(|c| c.provider);
                let cloud_service = cloud.and_then(|c| c.service);
                self.conn
                    .execute(
                        "UPDATE ip_addresses SET reverse_ptr = ?2, \
                             cloud_provider = ?3, cdn = ?4, \
                             last_updated = strftime('%s','now') \
                         WHERE ip = ?1",
                        params![ip.to_string(), ptr, cloud_provider, cloud_service],
                    )
                    .map_err(|e| Error::Storage(format!("ip_addresses update: {e}")))?;
                // Port scan the top ports. 800 ms per connect timeout
                // × 20 ports in parallel ≈ <2 s per IP.
                let probes = tcp_probe_ports(*ip, TOP_PORTS, Duration::from_millis(800)).await;
                self.persist_port_probes(*ip, &probes)?;
            }
        }

        report.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(report)
    }

    fn persist_port_probes(&mut self, ip: IpAddr, probes: &[PortProbe]) -> Result<()> {
        for p in probes {
            // Only persist open + closed; filtered ports are noise
            // (they almost always mean "ISP/firewall ate the connect"
            // and would flood the table with rows that only tell us
            // our own network).
            if matches!(
                p.state,
                crate::discovery::network_probe::PortState::Filtered
            ) {
                continue;
            }
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO port_probes \
                        (ip, port, state, banner, service) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        ip.to_string(),
                        p.port as i64,
                        p.state.as_str(),
                        p.banner,
                        p.service
                    ],
                )
                .map_err(|e| Error::Storage(format!("port_probes insert: {e}")))?;
        }
        Ok(())
    }

    fn persist_dns(&mut self, domain: &str, f: &DnsFacts) -> Result<usize> {
        let mut n = 0usize;
        let insert = |rtype: &str, rdata: &str| -> Result<()> {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO dns_records (domain, record_type, rdata) \
                     VALUES (?1, ?2, ?3)",
                    params![domain, rtype, rdata],
                )
                .map_err(|e| Error::Storage(format!("dns_records insert: {e}")))?;
            Ok(())
        };
        for ip in &f.a {
            insert("A", &ip.to_string())?;
            n += 1;
        }
        for ip in &f.aaaa {
            insert("AAAA", &ip.to_string())?;
            n += 1;
        }
        for c in &f.cname {
            insert("CNAME", c)?;
            n += 1;
        }
        for mx in &f.mx {
            insert("MX", mx)?;
            n += 1;
        }
        for txt in &f.txt {
            insert("TXT", txt)?;
            n += 1;
        }
        for ns in &f.ns {
            insert("NS", ns)?;
            n += 1;
        }
        for caa in &f.caa {
            insert("CAA", caa)?;
            n += 1;
        }
        Ok(n)
    }

    async fn is_wildcard_dns(&self, target: &str) -> bool {
        let nonces = [
            format!("crawlex-nonce-a8f3.{target}"),
            format!("crawlex-nonce-b92x.{target}"),
            format!("crawlex-nonce-c41z.{target}"),
        ];
        let mut first: Option<BTreeSet<IpAddr>> = None;
        for n in &nonces {
            let facts = crate::discovery::dns::lookup(n).await;
            let ips: BTreeSet<IpAddr> = facts
                .a
                .iter()
                .copied()
                .chain(facts.aaaa.iter().copied())
                .collect();
            if ips.is_empty() {
                return false; // nonces that NXDOMAIN can't be wildcards
            }
            match &first {
                None => first = Some(ips),
                Some(prev) => {
                    if prev != &ips {
                        return false;
                    }
                }
            }
        }
        true
    }

    async fn grab_cert(&self, domain: &str) -> Result<Option<PeerCert>> {
        // Reuse the existing impersonate client; a HEAD/GET over HTTPS
        // forces the TLS handshake and `Response.peer_cert` carries the
        // extracted PeerCert.
        let url = url::Url::parse(&format!("https://{domain}/"))?;
        // Bounded timeout so a slow/filtered host doesn't stall the
        // whole recon run.
        let fut = self.http.get(&url);
        match tokio::time::timeout(Duration::from_secs(10), fut).await {
            Ok(Ok(resp)) => Ok(resp.peer_cert),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Http(format!("cert grab timeout for {domain}"))),
        }
    }

    fn persist_cert(&mut self, domain: &str, cert: &PeerCert) -> Result<()> {
        let sha = match cert.sha256.as_deref() {
            Some(s) => s.to_string(),
            None => return Ok(()), // nothing to key on
        };
        let is_wildcard = cert
            .subject_cn
            .as_deref()
            .map(|cn| cn.starts_with("*.") || cn.contains("*"))
            .unwrap_or(false)
            || cert.sans.iter().any(|s| s.starts_with("*."));
        let is_self_signed = cert
            .subject_cn
            .as_ref()
            .zip(cert.issuer_cn.as_ref())
            .map(|(s, i)| s == i)
            .unwrap_or(false);
        let sans_json = serde_json::to_string(&cert.sans).unwrap_or_else(|_| "[]".into());
        let not_before = parse_boring_asn1_time(cert.not_before.as_deref());
        let not_after = parse_boring_asn1_time(cert.not_after.as_deref());
        self.conn
            .execute(
                "INSERT INTO certs \
                 (sha256_fingerprint, subject_cn, issuer_cn, not_before, not_after, \
                  sans_json, is_wildcard, is_self_signed, source) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'tls_handshake') \
                 ON CONFLICT(sha256_fingerprint) DO NOTHING",
                params![
                    sha,
                    cert.subject_cn,
                    cert.issuer_cn,
                    not_before,
                    not_after,
                    sans_json,
                    is_wildcard as i64,
                    is_self_signed as i64,
                ],
            )
            .map_err(|e| Error::Storage(format!("certs insert: {e}")))?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO cert_seen_on (cert_sha256, domain, port) \
                 VALUES (?1, ?2, 443)",
                params![sha, domain],
            )
            .map_err(|e| Error::Storage(format!("cert_seen_on insert: {e}")))?;
        Ok(())
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ`-ish RDAP / ISO 8601 strings into Unix
/// seconds. RDAP emits timestamps in a few formats; we take whatever
/// `time` crate parses with `OffsetDateTime::parse` against RFC 3339.
fn parse_iso_seconds(s: Option<&str>) -> Option<i64> {
    let s = s?;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

/// BoringSSL's `not_before/not_after` arrive as `"Jan  1 00:00:00 2024 GMT"`.
/// Keep them as opaque ISO-ish strings in the DB until a future change
/// standardises the parsing (the downstream query consumers only need
/// raw display today).
fn parse_boring_asn1_time(_s: Option<&str>) -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel_stage_stringify() {
        assert_eq!(IntelStage::Subdomains.as_str(), "subdomains");
        assert_eq!(IntelStage::Dns.as_str(), "dns");
        assert_eq!(IntelStage::Whois.as_str(), "whois");
        assert_eq!(IntelStage::Cert.as_str(), "cert");
    }

    #[test]
    fn parse_iso_rfc3339_seconds() {
        // RDAP emits RFC 3339; make sure Z + offset forms both parse
        // and invalid inputs fall through to None.
        assert!(parse_iso_seconds(Some("1998-11-25T12:41:54Z")).is_some());
        assert!(parse_iso_seconds(Some("2024-01-15T10:00:00-03:00")).is_some());
        assert_eq!(parse_iso_seconds(None), None);
        assert_eq!(parse_iso_seconds(Some("not a date")), None);
    }

    #[test]
    fn report_default_is_empty() {
        let r = IntelReport::default();
        assert_eq!(r.subdomains.len(), 0);
        assert_eq!(r.dns_record_count, 0);
        assert_eq!(r.unique_ips.len(), 0);
        assert_eq!(r.certs_captured, 0);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn schema_opens_on_fresh_tmp_db() {
        // Smoke: the open path runs the SqliteStorage migration that
        // creates the Fase A tables. Fresh file ⇒ all tables created.
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("intel-smoke.db");
        let orch = TargetIntelOrchestrator::open(&path, InfraIntelConfig::default());
        assert!(orch.is_ok(), "open: {:?}", orch.err());
        let conn = Connection::open(&path).expect("reopen");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='domains'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
