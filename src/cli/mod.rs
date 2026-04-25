pub mod args;
mod raffel_proxy;

use std::io::Read as _;
use std::sync::Arc;

use clap::Parser;

use crate::config::{
    ChallengeMode, Config, ProxyConfig, QueueBackend, RenderSessionScope, StorageBackend,
};
use crate::events::{EventEnvelope, EventKind, EventSink, NdjsonStdoutSink, NullSink};
use crate::impersonate::Profile;
use crate::policy::PolicyProfile;
use crate::proxy::RotationStrategy;
use crate::wait_strategy::WaitStrategy;
use crate::{Crawler, Result};

/// Parse `--policy <preset>` into a `PolicyProfile`. Returns
/// `Error::Config` on unknown preset so the operator gets a clear message
/// instead of a silent fallback.
fn parse_policy_profile(s: &str) -> Result<PolicyProfile> {
    match s.to_ascii_lowercase().as_str() {
        "fast" => Ok(PolicyProfile::Fast),
        "balanced" | "" => Ok(PolicyProfile::Balanced),
        "deep" => Ok(PolicyProfile::Deep),
        "forensics" => Ok(PolicyProfile::Forensics),
        other => Err(crate::Error::Config(format!(
            "unknown --policy `{other}`; expected fast|balanced|deep|forensics"
        ))),
    }
}

fn parse_render_session_scope(s: &str) -> Result<RenderSessionScope> {
    match s.to_ascii_lowercase().as_str() {
        "registrable_domain" | "domain" | "" => Ok(RenderSessionScope::RegistrableDomain),
        "host" => Ok(RenderSessionScope::Host),
        "origin" => Ok(RenderSessionScope::Origin),
        "url" => Ok(RenderSessionScope::Url),
        other => Err(crate::Error::Config(format!(
            "unknown --render-session-scope `{other}`; expected registrable_domain|host|origin|url"
        ))),
    }
}

fn parse_challenge_mode(s: &str) -> Result<ChallengeMode> {
    match s.to_ascii_lowercase().as_str() {
        "avoidance" | "avoid" => Ok(ChallengeMode::Avoidance),
        "solver_ready" | "solver-ready" | "" => Ok(ChallengeMode::SolverReady),
        other => Err(crate::Error::Config(format!(
            "unknown --challenge-mode `{other}`; expected avoidance|solver-ready"
        ))),
    }
}

/// Compose an `EventSink` based on `--emit` and `--explain`. When both
/// `ndjson` and `explain` are on, every event is written to stdout AND a
/// human-readable line is written to stderr via the `ExplainSink`.
fn build_event_sink(emit: &str, explain: bool) -> Result<Arc<dyn EventSink>> {
    let primary: Arc<dyn EventSink> = match emit.to_ascii_lowercase().as_str() {
        "none" | "" => Arc::new(NullSink),
        "ndjson" => Arc::new(NdjsonStdoutSink::create()),
        other => {
            return Err(crate::Error::Config(format!(
                "unknown --emit `{other}`; expected ndjson|none"
            )));
        }
    };
    if explain {
        Ok(Arc::new(TeeSink {
            primary,
            secondary: Arc::new(ExplainSink),
        }))
    } else {
        Ok(primary)
    }
}

/// Parse `--action-policy`: `permissive`, `strict`, `default`, or a path
/// to a JSON file. `None` → permissive (legacy default, no gating).
fn parse_action_policy(s: Option<&str>) -> Result<crate::policy::ActionPolicy> {
    let Some(s) = s else {
        return Ok(crate::policy::ActionPolicy::permissive());
    };
    match s {
        "permissive" => Ok(crate::policy::ActionPolicy::permissive()),
        "strict" => Ok(crate::policy::ActionPolicy::strict()),
        "default" => Ok(crate::policy::ActionPolicy::default()),
        path => {
            let bytes = std::fs::read(path).map_err(crate::Error::Io)?;
            serde_json::from_slice::<crate::policy::ActionPolicy>(&bytes)
                .map_err(|e| crate::Error::Config(format!("action_policy JSON: {e}")))
        }
    }
}

/// Load `Config` from a JSON file path, or stdin when `path == "-"`.
fn load_config_from_path_or_stdin(path: &str) -> Result<Config> {
    let bytes = if path == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(crate::Error::Io)?;
        buf
    } else {
        std::fs::read(path).map_err(crate::Error::Io)?
    };
    serde_json::from_slice::<Config>(&bytes)
        .map_err(|e| crate::Error::Config(format!("config parse: {e}")))
}

/// Forwards every event to two underlying sinks. Used when `--explain`
/// composes stderr alongside whatever primary sink is in play.
struct TeeSink {
    primary: Arc<dyn EventSink>,
    secondary: Arc<dyn EventSink>,
}
impl EventSink for TeeSink {
    fn emit(&self, ev: &EventEnvelope) {
        self.primary.emit(ev);
        self.secondary.emit(ev);
    }
    fn flush(&self) {
        self.primary.flush();
        self.secondary.flush();
    }
}

/// Sink that prints a human-readable line to stderr for events worth
/// explaining (decisions, failures, run boundaries). Quiet on the noisy
/// kinds (`job.started`, `fetch.completed`) so `--explain` stays useful.
struct ExplainSink;
impl EventSink for ExplainSink {
    fn emit(&self, ev: &EventEnvelope) {
        let kind = match ev.event {
            EventKind::DecisionMade
            | EventKind::JobFailed
            | EventKind::RunStarted
            | EventKind::RunCompleted
            | EventKind::RobotsDecision => ev.event,
            _ => return,
        };
        let url = ev.url.as_deref().unwrap_or("-");
        let why = ev.why.as_deref().unwrap_or("-");
        eprintln!("[crawlex] {:?} url={} why={}", kind, url, why);
    }
    fn flush(&self) {
        use std::io::Write as _;
        let _ = std::io::stderr().flush();
    }
}

pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init()
        .ok();

    let args = args::Cli::parse();
    match args.command {
        args::Command::Pages(v) => match v {
            args::PagesVerb::Run(a) => cmd_crawl(a).await?,
        },
        args::Command::Crawl(v) => match v {
            args::CrawlVerb::Resume(a) => cmd_resume(a).await?,
        },
        args::Command::Fingerprint(v) => match v {
            args::FingerprintVerb::Run(a) => cmd_intel(a).await?,
            args::FingerprintVerb::Show(a) => cmd_intel_show(a).await?,
            args::FingerprintVerb::Export(a) => cmd_intel_export(a).await?,
        },
        args::Command::Graph(v) => match v {
            args::GraphVerb::Export(a) => cmd_export_graph(a).await?,
        },
        args::Command::Queue(v) => match v {
            args::QueueVerb::Stats(a) => {
                cmd_queue(args::QueueCmd::Stats {
                    queue_path: a.queue_path,
                })
                .await?
            }
            args::QueueVerb::Purge(a) => {
                cmd_queue(args::QueueCmd::Purge {
                    queue_path: a.queue_path,
                })
                .await?
            }
            args::QueueVerb::Export(a) => {
                cmd_queue(args::QueueCmd::Export {
                    queue_path: a.queue_path,
                    out: a.out,
                })
                .await?
            }
        },
        args::Command::Sessions(v) => match v {
            args::SessionsVerb::List(a) => {
                cmd_sessions(args::SessionsCmd::List {
                    storage_path: a.storage_path,
                    state: a.state,
                })
                .await?
            }
        },
        args::Command::Session(v) => match v {
            args::SessionVerb::Drop(a) => {
                cmd_sessions(args::SessionsCmd::Drop {
                    storage_path: a.storage_path,
                    id: a.id,
                })
                .await?
            }
        },
        args::Command::Telemetry(v) => match v {
            args::TelemetryVerb::Show(a) => {
                cmd_telemetry(args::TelemetryCmd::Show {
                    db: a.db,
                    top: a.top,
                })
                .await?
            }
        },
        args::Command::Stealth(v) => match v {
            args::StealthVerb::Test => cmd_test_stealth().await?,
            args::StealthVerb::Inspect(a) => cmd_inspect(a).await?,
            args::StealthVerb::Catalog(cv) => match cv {
                args::CatalogVerb::List(a) => cmd_catalog_list(a)?,
                args::CatalogVerb::Show(a) => cmd_catalog_show(a)?,
            },
        },
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// `crawlex stealth catalog list` and `... show <profile>`
// ─────────────────────────────────────────────────────────────────────

fn cmd_catalog_list(args: args::CatalogListArgs) -> anyhow::Result<()> {
    use crate::impersonate::catalog::{all, Browser};
    let filter = args.filter.as_deref().map(str::to_ascii_lowercase);
    let want = |b: Browser| -> bool {
        match filter.as_deref() {
            None => true,
            Some("chrome") => b == Browser::Chrome,
            Some("chromium") => b == Browser::Chromium,
            Some("firefox") => b == Browser::Firefox,
            Some("edge") => b == Browser::Edge,
            Some("safari") => b == Browser::Safari,
            Some(_) => true,
        }
    };

    let entries: Vec<_> = all().filter(|fp| want(fp.browser)).collect();

    if args.json {
        for fp in &entries {
            let line = serde_json::json!({
                "name": fp.name,
                "browser": fp.browser_name,
                "major": fp.major,
                "version": fp.version,
                "os": fp.os_name,
                "ja3": fp.ja3_string(),
                "ciphers": fp.ciphers_no_grease().len(),
                "extensions": fp.extension_ids_no_grease().len(),
                "alpn": fp.alpn,
                "alps_alpn": fp.alps_alpn,
                "pq_groups": fp.supported_groups_no_grease(),
                "has_ech_grease": fp.has_ech_grease,
            });
            println!("{}", line);
        }
        eprintln!("\n# {} profile(s) emitted", entries.len());
        return Ok(());
    }

    println!(
        "{:<40} {:<8} {:<5} {:<8} {:<8} {:<8} {:<6}",
        "NAME", "BROWSER", "MAJOR", "OS", "CIPHERS", "EXT", "ECH"
    );
    println!("{}", "─".repeat(88));
    for fp in &entries {
        println!(
            "{:<40} {:<8} {:<5} {:<8} {:<8} {:<8} {:<6}",
            fp.name,
            fp.browser_name,
            fp.major,
            fp.os_name,
            fp.ciphers_no_grease().len(),
            fp.extension_ids_no_grease().len(),
            if fp.has_ech_grease { "yes" } else { "no" },
        );
    }
    eprintln!(
        "\n# {} profile(s) listed{}",
        entries.len(),
        match filter.as_deref() {
            Some(f) => format!(" (filter={f})"),
            None => String::new(),
        }
    );
    Ok(())
}

fn cmd_catalog_show(args: args::CatalogShowArgs) -> anyhow::Result<()> {
    use crate::impersonate::catalog::{lookup, ExtensionEntry, NumericEntry};

    // Resolve: try direct name first, else parse as <browser>-<major>-<os>.
    let fp = if let Some(fp) = lookup(&args.profile) {
        fp
    } else {
        use std::str::FromStr;
        let p = crate::impersonate::Profile::from_str(&args.profile).map_err(|e| {
            anyhow::anyhow!(
                "profile `{}` not in catalog and not a valid spec: {e}\n\
                 Try: `crawlex stealth catalog list` to see available names.",
                args.profile
            )
        })?;
        p.tls()
            .ok_or_else(|| anyhow::anyhow!("profile `{}` resolves to no fingerprint", args.profile))?
    };

    if args.json {
        let extensions: Vec<serde_json::Value> = fp
            .extensions
            .iter()
            .map(|e| match e {
                ExtensionEntry::Greased => serde_json::json!({"type": "GREASE"}),
                ExtensionEntry::Named { id, name } => serde_json::json!({
                    "type": name,
                    "id": id,
                }),
            })
            .collect();
        let ciphers: Vec<serde_json::Value> = fp
            .ciphersuites
            .iter()
            .map(|e| match e {
                NumericEntry::Greased => serde_json::json!("GREASE"),
                NumericEntry::Value(v) => serde_json::json!(format!("0x{:04x}", v)),
            })
            .collect();
        let report = serde_json::json!({
            "name": fp.name,
            "browser": fp.browser_name,
            "major": fp.major,
            "version": fp.version,
            "os": fp.os_name,
            "record_version": format!("0x{:04x}", fp.record_version),
            "handshake_version": format!("0x{:04x}", fp.handshake_version),
            "session_id_length": fp.session_id_length,
            "ciphersuites": ciphers,
            "extensions": extensions,
            "alpn": fp.alpn,
            "alps_alpn": fp.alps_alpn,
            "supported_groups": fp.supported_groups_no_grease(),
            "ec_point_formats": fp.ec_point_formats,
            "sig_hash_algs": fp.sig_hash_algs.iter().map(|a| format!("0x{:04x}", a)).collect::<Vec<_>>(),
            "supported_versions": fp.supported_versions.iter().map(|v| match v {
                NumericEntry::Greased => "GREASE".to_string(),
                NumericEntry::Value(n) => format!("0x{:04x}", n),
            }).collect::<Vec<_>>(),
            "cert_compress_algs": fp.cert_compress_algs,
            "psk_ke_modes": fp.psk_ke_modes,
            "ja3": fp.ja3_string(),
            "has_ech_grease": fp.has_ech_grease,
            "has_extended_master_secret": fp.has_extended_master_secret,
            "has_renegotiation_info": fp.has_renegotiation_info,
            "has_session_ticket": fp.has_session_ticket,
            "has_signed_certificate_timestamp": fp.has_signed_certificate_timestamp,
            "has_status_request": fp.has_status_request,
            "has_padding": fp.has_padding,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("=== TLS fingerprint: {} ===", fp.name);
    println!("  browser   : {} {}", fp.browser_name, fp.version);
    println!("  os        : {}", fp.os_name);
    println!(
        "  versions  : record=0x{:04x} handshake=0x{:04x}",
        fp.record_version, fp.handshake_version
    );
    println!("  session_id_len  : {}", fp.session_id_length);
    println!("  ciphersuites   ({}):", fp.ciphers_no_grease().len());
    for entry in fp.ciphersuites {
        match entry {
            NumericEntry::Greased => println!("    - GREASE"),
            NumericEntry::Value(v) => {
                let name = crate::impersonate::catalog::cipher_id_to_openssl_name(*v)
                    .unwrap_or("<unknown>");
                println!("    - 0x{:04x}  {}", v, name);
            }
        }
    }
    println!("  extensions ({}):", fp.extension_ids_no_grease().len());
    for entry in fp.extensions {
        match entry {
            ExtensionEntry::Greased => println!("    - GREASE"),
            ExtensionEntry::Named { id, name } => {
                println!("    - 0x{:04x} ({}) {}", id, id, name);
            }
        }
    }
    println!("  alpn        : {:?}", fp.alpn);
    if !fp.alps_alpn.is_empty() {
        println!("  alps_alpn   : {:?}", fp.alps_alpn);
    }
    println!("  supported_groups: {:?}", fp.supported_groups_no_grease());
    if !fp.cert_compress_algs.is_empty() {
        println!("  cert_compress: {:?}", fp.cert_compress_algs);
    }
    println!("  ja3 (raw)   : {}", fp.ja3_string());
    println!(
        "  flags       : ech_grease={} ems={} rnegi={} stkt={} sct={} status={} padding={}",
        fp.has_ech_grease,
        fp.has_extended_master_secret,
        fp.has_renegotiation_info,
        fp.has_session_ticket,
        fp.has_signed_certificate_timestamp,
        fp.has_status_request,
        fp.has_padding,
    );
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn cmd_intel_export(cmd: args::IntelExportArgs) -> Result<()> {
    use rusqlite::Connection;
    use serde_json::{json, Value};

    let target = normalise_target(&cmd.target);

    // HTML output wins over JSON when both flags are set. The HTML branch
    // doesn't reuse the JSON `rows` helper because its SQL is shaped for
    // table-rendering (grouping, rollups) rather than a flat payload.
    if let Some(html_path) = cmd.html.as_deref() {
        let db_path = std::path::Path::new(&cmd.db);
        let html = crate::intel::report_html::render(&target, db_path)?;
        std::fs::write(html_path, &html).map_err(crate::Error::Io)?;
        eprintln!("[intel-export] wrote {} bytes → {}", html.len(), html_path);
        return Ok(());
    }

    let conn = Connection::open(&cmd.db).map_err(|e| crate::Error::Storage(e.to_string()))?;
    let like = format!("%{}%", target);

    // Helper closures that turn each query into `Vec<serde_json::Value>`
    // so the final `json!({...})` block reads like a schema.
    let rows = |sql: &str, params: &[&dyn rusqlite::ToSql], cols: &[&str]| -> Result<Vec<Value>> {
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        let column_names: Vec<String> = cols.iter().map(|s| (*s).to_string()).collect();
        let iter = stmt
            .query_map(params, |r| {
                let mut obj = serde_json::Map::new();
                for (i, name) in column_names.iter().enumerate() {
                    let v: Value = match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Real(f) => json!(f),
                        rusqlite::types::ValueRef::Text(bs) => {
                            json!(String::from_utf8_lossy(bs).to_string())
                        }
                        rusqlite::types::ValueRef::Blob(bs) => json!(hex::encode(bs)),
                    };
                    obj.insert(name.clone(), v);
                }
                Ok(Value::Object(obj))
            })
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        Ok(iter.filter_map(|r| r.ok()).collect())
    };

    let target_summary = rows(
        "SELECT target_root, domains, subdomains, wildcard_dns, unique_ips, certs_seen \
         FROM v_target_intel WHERE target_root = ?1",
        &[&target],
        &[
            "target_root",
            "domains",
            "subdomains",
            "wildcard_dns",
            "unique_ips",
            "certs_seen",
        ],
    )?;

    let domains_list = rows(
        "SELECT domain, is_subdomain, is_wildcard_dns, first_seen, last_probed \
         FROM domains WHERE target_root = ?1 ORDER BY domain",
        &[&target],
        &[
            "domain",
            "is_subdomain",
            "is_wildcard_dns",
            "first_seen",
            "last_probed",
        ],
    )?;

    let dns = rows(
        "SELECT domain, record_type, rdata, ttl, observed_at \
         FROM dns_records WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1) \
         ORDER BY domain, record_type, rdata",
        &[&target],
        &["domain", "record_type", "rdata", "ttl", "observed_at"],
    )?;

    let ips = rows(
        "SELECT ip, asn, asn_name, reverse_ptr, cloud_provider, cdn, country, first_seen \
         FROM ip_addresses WHERE ip IN \
             (SELECT ip FROM domain_ips WHERE domain IN \
                 (SELECT domain FROM domains WHERE target_root = ?1)) \
         ORDER BY ip",
        &[&target],
        &[
            "ip",
            "asn",
            "asn_name",
            "reverse_ptr",
            "cloud_provider",
            "cdn",
            "country",
            "first_seen",
        ],
    )?;

    let whois_rows = rows(
        "SELECT domain, registrar, registrant_org, created_at, expires_at, updated_at, \
                nameservers_json, status_json, abuse_email \
         FROM whois_records WHERE domain = ?1",
        &[&target],
        &[
            "domain",
            "registrar",
            "registrant_org",
            "created_at",
            "expires_at",
            "updated_at",
            "nameservers_json",
            "status_json",
            "abuse_email",
        ],
    )?;

    let certs = rows(
        "SELECT DISTINCT c.sha256_fingerprint, c.subject_cn, c.issuer_cn, c.issuer_org, \
                c.not_before, c.not_after, c.sans_json, c.is_wildcard, c.is_self_signed, \
                c.sig_algo, c.pubkey_algo, c.pubkey_bits, c.source, c.first_seen \
         FROM certs c \
         JOIN cert_seen_on s ON s.cert_sha256 = c.sha256_fingerprint \
         WHERE s.domain IN (SELECT domain FROM domains WHERE target_root = ?1) \
         ORDER BY c.issuer_cn, c.subject_cn",
        &[&target],
        &[
            "sha256_fingerprint",
            "subject_cn",
            "issuer_cn",
            "issuer_org",
            "not_before",
            "not_after",
            "sans_json",
            "is_wildcard",
            "is_self_signed",
            "sig_algo",
            "pubkey_algo",
            "pubkey_bits",
            "source",
            "first_seen",
        ],
    )?;

    let port_probes = rows(
        "SELECT ip, port, state, banner, service, service_version, observed_at \
         FROM port_probes WHERE ip IN \
             (SELECT ip FROM domain_ips WHERE domain IN \
                 (SELECT domain FROM domains WHERE target_root = ?1)) \
         ORDER BY ip, port",
        &[&target],
        &[
            "ip",
            "port",
            "state",
            "banner",
            "service",
            "service_version",
            "observed_at",
        ],
    )?;

    let external_domains = rows(
        "SELECT to_domain, COUNT(*) AS ref_count \
         FROM asset_refs \
         WHERE from_page_url LIKE ?1 AND is_internal = 0 \
         GROUP BY to_domain ORDER BY ref_count DESC",
        &[&like],
        &["to_domain", "ref_count"],
    )?;

    let asset_kind_rollup = rows(
        "SELECT kind, is_internal, COUNT(*) AS n FROM asset_refs \
         WHERE from_page_url LIKE ?1 \
         GROUP BY kind, is_internal ORDER BY kind",
        &[&like],
        &["kind", "is_internal", "n"],
    )?;

    let payload = json!({
        "target_root": target,
        "generated_at": chrono_now(),
        "summary": target_summary.into_iter().next().unwrap_or(Value::Null),
        "domains": domains_list,
        "dns_records": dns,
        "ip_addresses": ips,
        "whois": whois_rows.into_iter().next().unwrap_or(Value::Null),
        "certs": certs,
        "port_probes": port_probes,
        "external_domains": external_domains,
        "asset_ref_rollup": asset_kind_rollup,
    });

    let text = if cmd.pretty {
        serde_json::to_string_pretty(&payload).map_err(|e| crate::Error::Config(e.to_string()))?
    } else {
        serde_json::to_string(&payload).map_err(|e| crate::Error::Config(e.to_string()))?
    };
    match cmd.out.as_deref() {
        Some(path) => {
            std::fs::write(path, &text).map_err(crate::Error::Io)?;
            eprintln!("[intel-export] wrote {} bytes → {}", text.len(), path);
        }
        None => println!("{text}"),
    }
    Ok(())
}

/// Normalise an operator-supplied target into a registrable domain.
/// Accepts `www.example.com`, `https://example.com/path`, or the raw
/// `example.com`. Keeps the lowercased registrable so downstream
/// queries against `domains.target_root` stay consistent across
/// every verb.
pub(crate) fn normalise_target(input: &str) -> String {
    let trimmed = input.trim();
    // Strip scheme + path if the operator pasted a URL.
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    crate::discovery::subdomains::registrable_domain(&host).unwrap_or(host)
}

/// Cheap ISO-8601 timestamp without pulling in chrono. Uses the `time`
/// crate already in tree.
fn chrono_now() -> String {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(not(feature = "sqlite"))]
async fn cmd_intel_export(_cmd: args::IntelExportArgs) -> Result<()> {
    Err(crate::Error::Config(
        "`crawlex intel-export` requires the sqlite feature".into(),
    ))
}

#[cfg(feature = "sqlite")]
async fn cmd_intel_show(mut cmd: args::IntelShowArgs) -> Result<()> {
    use rusqlite::Connection;
    cmd.target = normalise_target(&cmd.target);
    let conn = Connection::open(&cmd.db).map_err(|e| crate::Error::Storage(e.to_string()))?;

    println!("\n=== intel fingerprint show === target: {}", cmd.target);
    println!("db     : {}", cmd.db);

    // --- Target summary ---
    let summary: Option<(i64, i64, i64, i64, i64)> = conn
        .query_row(
            "SELECT domains, subdomains, wildcard_dns, unique_ips, certs_seen \
             FROM v_target_intel WHERE target_root = ?1",
            rusqlite::params![cmd.target],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();
    if let Some((d, s, w, i, c)) = summary {
        println!("summary: domains={d}  subdomains={s}  wildcard_dns={w}  ips={i}  certs_seen={c}");
    } else {
        println!(
            "summary: (no rows — run `crawlex intel {}` first)",
            cmd.target
        );
        return Ok(());
    }

    // --- WHOIS ---
    if let Ok((reg, org, created, expires, ns_json)) = conn.query_row(
        "SELECT registrar, registrant_org, \
             datetime(created_at,'unixepoch'), \
             datetime(expires_at,'unixepoch'), \
             COALESCE(nameservers_json,'[]') \
         FROM whois_records WHERE domain = ?1",
        rusqlite::params![cmd.target],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    ) {
        println!("\n-- whois --");
        if let Some(r) = reg {
            println!("  registrar     : {r}");
        }
        if let Some(o) = org {
            println!("  registrant_org: {o}");
        }
        if let Some(c) = created {
            println!("  created       : {c}");
        }
        if let Some(e) = expires {
            println!("  expires       : {e}");
        }
        if let Ok(ns) = serde_json::from_str::<Vec<String>>(&ns_json) {
            if !ns.is_empty() {
                println!("  nameservers   : {}", ns.join(", "));
            }
        }
    }

    // --- DNS roll-up per record type ---
    let mut stmt = conn
        .prepare(
            "SELECT record_type, COUNT(*) FROM dns_records \
                 WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1) \
                 GROUP BY record_type ORDER BY record_type",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    let dns_rows: Vec<(String, i64)> = stmt
        .query_map(rusqlite::params![cmd.target], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !dns_rows.is_empty() {
        println!("\n-- dns records --");
        for (t, n) in &dns_rows {
            println!("  {t:<6}: {n}");
        }
    }

    // --- IPs + cloud tagging ---
    let mut stmt = conn
        .prepare(
            "SELECT ip, reverse_ptr, cloud_provider, cdn \
                 FROM ip_addresses \
                 WHERE ip IN (SELECT ip FROM domain_ips \
                     WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1)) \
                 ORDER BY ip",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    #[allow(clippy::type_complexity)]
    let ip_rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = stmt
        .query_map(rusqlite::params![cmd.target], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !ip_rows.is_empty() {
        println!("\n-- ip addresses ({}) --", ip_rows.len());
        let cap = if cmd.limit == 0 {
            usize::MAX
        } else {
            cmd.limit
        };
        for (ip, ptr, cloud, cdn) in ip_rows.iter().take(cap) {
            let cloud_str = match (cloud, cdn) {
                (Some(c), Some(s)) => format!(" [{c}/{s}]"),
                (Some(c), None) => format!(" [{c}]"),
                _ => String::new(),
            };
            let ptr_str = ptr.as_deref().unwrap_or("-");
            println!("  {ip:<40} ptr={ptr_str}{cloud_str}");
        }
    }

    // --- Open ports ---
    let mut stmt = conn
        .prepare(
            "SELECT ip, port, state, service \
                 FROM port_probes \
                 WHERE ip IN (SELECT ip FROM domain_ips \
                     WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1)) \
                   AND state = 'open' \
                 ORDER BY ip, port",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    let port_rows: Vec<(String, i64, String, Option<String>)> = stmt
        .query_map(rusqlite::params![cmd.target], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !port_rows.is_empty() {
        println!("\n-- open ports --");
        for (ip, port, _state, svc) in &port_rows {
            let svc = svc.as_deref().unwrap_or("?");
            println!("  {ip}:{port:<5} [{svc}]");
        }
    }

    // --- Cert summary (deduplicated by sha256) ---
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT c.subject_cn, c.issuer_cn, c.is_wildcard, c.is_self_signed, \
                     substr(c.sha256_fingerprint, 1, 16) \
                 FROM certs c \
                 JOIN cert_seen_on s ON s.cert_sha256 = c.sha256_fingerprint \
                 WHERE s.domain IN (SELECT domain FROM domains WHERE target_root = ?1) \
                 ORDER BY c.issuer_cn, c.subject_cn",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    #[allow(clippy::type_complexity)]
    let certs: Vec<(Option<String>, Option<String>, i64, i64, String)> = stmt
        .query_map(rusqlite::params![cmd.target], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !certs.is_empty() {
        println!("\n-- certs ({}) --", certs.len());
        let cap = if cmd.limit == 0 {
            usize::MAX
        } else {
            cmd.limit
        };
        for (subject, issuer, wild, selfs, sha) in certs.iter().take(cap) {
            let sub = subject.as_deref().unwrap_or("-");
            let iss = issuer.as_deref().unwrap_or("-");
            let flags = match (*wild, *selfs) {
                (1, 1) => " [wildcard,self-signed]",
                (1, 0) => " [wildcard]",
                (0, 1) => " [self-signed]",
                _ => "",
            };
            println!("  sha:{sha}  subj={sub:<40}  issuer={iss}{flags}");
        }
    }

    // --- External domain rollup (from asset_refs) ---
    // Fenced on target_root so an operator running multiple recons in
    // one DB still gets a per-target view. `from_page_url LIKE` picks
    // any page whose URL contains the target — coarse but robust
    // against subdomain-varying hosts.
    let like = format!("%{}%", cmd.target);
    // LEFT JOIN to pull each domain's heuristic categories (when known).
    // The `external_domains` table is populated lazily by SaveAssetRefs
    // so older DBs may have NULL categories — we just skip the suffix in
    // that case rather than backfill on read.
    let mut stmt = conn
        .prepare(
            "SELECT a.to_domain, COUNT(*) as refs, e.categories_json \
             FROM asset_refs a \
             LEFT JOIN external_domains e ON e.domain = a.to_domain \
             WHERE a.from_page_url LIKE ?1 AND a.is_internal = 0 \
             GROUP BY a.to_domain, e.categories_json \
             ORDER BY refs DESC",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    let ext_rows: Vec<(String, i64, Option<String>)> = stmt
        .query_map(rusqlite::params![like], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !ext_rows.is_empty() {
        let cap = if cmd.limit == 0 {
            usize::MAX
        } else {
            cmd.limit
        };
        println!(
            "\n-- external domains ({} unique, top {} shown) --",
            ext_rows.len(),
            ext_rows.len().min(cap)
        );
        for (d, n, cats_json) in ext_rows.iter().take(cap) {
            // Parse the JSON tail back into a flat tag list. Tolerant of
            // malformed rows (treat as missing) so a corrupt column
            // never blocks the rest of the rollup.
            let suffix = cats_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                .filter(|v| !v.is_empty())
                .map(|v| format!("  [{}]", v.join(",")))
                .unwrap_or_default();
            println!("  {n:>6}  {d}{suffix}");
        }
    }

    // --- Asset-ref kind rollup (distinct between internal/external) ---
    let mut stmt = conn
        .prepare(
            "SELECT kind, is_internal, COUNT(*) \
             FROM asset_refs \
             WHERE from_page_url LIKE ?1 \
             GROUP BY kind, is_internal \
             ORDER BY kind",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    let kind_rows: Vec<(String, i64, i64)> = stmt
        .query_map(rusqlite::params![like], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !kind_rows.is_empty() {
        println!("\n-- asset refs by kind --");
        // Fold (kind, is_internal, n) → a two-column table.
        use std::collections::BTreeMap;
        let mut by_kind: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for (k, internal, n) in &kind_rows {
            let slot = by_kind.entry(k.clone()).or_insert((0, 0));
            if *internal == 1 {
                slot.0 += n;
            } else {
                slot.1 += n;
            }
        }
        println!("  {:<10} {:>8} {:>8}", "kind", "internal", "external");
        for (k, (i, e)) in &by_kind {
            println!("  {k:<10} {i:>8} {e:>8}");
        }
    }

    // --- Subdomains (cap to limit) ---
    let mut stmt = conn
        .prepare(
            "SELECT domain FROM domains \
                 WHERE target_root = ?1 AND is_subdomain = 1 \
                 ORDER BY domain",
        )
        .map_err(|e| crate::Error::Storage(e.to_string()))?;
    let subs: Vec<String> = stmt
        .query_map(rusqlite::params![cmd.target], |r| {
            let s: String = r.get(0)?;
            Ok(s)
        })
        .map_err(|e| crate::Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    if !subs.is_empty() {
        let cap = if cmd.limit == 0 {
            usize::MAX
        } else {
            cmd.limit
        };
        let shown = subs.len().min(cap);
        println!("\n-- subdomains ({shown} of {} shown) --", subs.len());
        for s in subs.iter().take(cap) {
            println!("  {s}");
        }
    }

    Ok(())
}

#[cfg(not(feature = "sqlite"))]
async fn cmd_intel_show(_cmd: args::IntelShowArgs) -> Result<()> {
    Err(crate::Error::Config(
        "`crawlex intel-show` requires the sqlite feature".into(),
    ))
}

#[cfg(feature = "sqlite")]
async fn cmd_intel(cmd: args::IntelArgs) -> Result<()> {
    let infra_cfg = crate::config::InfraIntelConfig {
        subdomains: !cmd.no_subdomains,
        dns: !cmd.no_dns,
        whois: !cmd.no_whois,
        cert: !cmd.no_cert,
        network_probe: cmd.network_probe,
        ..Default::default()
    };
    // Accept `www.stone.com.br`, `https://stone.com.br/`, or raw
    // `stone.com.br` — normalise to the registrable domain so the
    // recon runs against the right scope key and downstream queries
    // against `domains.target_root` hit the expected rows.
    let target = normalise_target(&cmd.target);
    let mut orch =
        crate::intel::TargetIntelOrchestrator::open(std::path::Path::new(&cmd.db), infra_cfg)?;
    eprintln!("[intel] target = {}  db = {}", target, cmd.db);
    let report = orch.run(&target).await?;

    println!("\n=== crawlex intel report ===");
    println!("target      : {}", report.target_root);
    println!("subdomains  : {}", report.subdomains.len());
    println!("dns records : {}", report.dns_record_count);
    println!("unique ips  : {}", report.unique_ips.len());
    println!("certs       : {}", report.certs_captured);
    if let Some(r) = &report.whois_registrar {
        println!("registrar   : {r}");
    }
    if let Some(c) = &report.whois_created {
        println!("created     : {c}");
    }
    if let Some(e) = &report.whois_expires {
        println!("expires     : {e}");
    }
    println!("elapsed_ms  : {}", report.elapsed_ms);
    if !report.errors.is_empty() {
        println!("\nerrors ({}):", report.errors.len());
        for e in &report.errors {
            println!("  - {e}");
        }
    }
    if !report.subdomains.is_empty() {
        println!("\nsubdomains (first 25):");
        for s in report.subdomains.iter().take(25) {
            println!("  - {s}");
        }
    }
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
async fn cmd_intel(_cmd: args::IntelArgs) -> Result<()> {
    Err(crate::Error::Config(
        "`crawlex intel` requires the sqlite feature".into(),
    ))
}

async fn cmd_sessions(cmd: args::SessionsCmd) -> Result<()> {
    #[cfg(feature = "sqlite")]
    {
        match cmd {
            args::SessionsCmd::List {
                storage_path,
                state,
            } => {
                let storage = crate::storage::sqlite::SqliteStorage::open(&storage_path)?;
                let rows = storage.list_archived_sessions(state.clone()).await?;
                println!(
                    "id                                   state          scope_key                      urls  chal  reason"
                );
                println!(
                    "-----------------------------------  -------------  ---------------------------  ----  ----  ---------"
                );
                for r in rows {
                    println!(
                        "{:<36} {:<14} {:<28} {:>4}  {:>4}  {}",
                        r.session_id, r.state, r.scope_key, r.urls_visited, r.challenges, r.reason
                    );
                }
                Ok(())
            }
            args::SessionsCmd::Drop { storage_path, id } => {
                // Without a running pool, we can only mark the archive
                // row as "manual" — actually disposing the BrowserContext
                // requires the in-process pool. Emit a row regardless so
                // the archive reflects operator intent.
                let storage = crate::storage::sqlite::SqliteStorage::open(&storage_path)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let row = crate::storage::sqlite::ArchivedSessionRow {
                    session_id: id.clone(),
                    scope: "unknown".to_string(),
                    scope_key: String::new(),
                    state: "dropped".to_string(),
                    bundle_id: None,
                    created_at: now,
                    ended_at: now,
                    urls_visited: 0,
                    challenges: 0,
                    final_proxy: None,
                    reason: crate::identity::EvictionReason::Manual.as_str().to_string(),
                };
                storage.archive_session_row(row).await?;
                println!("archived session {id} as manual eviction");
                Ok(())
            }
        }
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = cmd;
        Err(crate::Error::Config(
            "`sessions` subcommand requires the `sqlite` feature".into(),
        ))
    }
}

async fn cmd_crawl(mut c: args::CrawlArgs) -> Result<()> {
    #[cfg(not(feature = "cdp-backend"))]
    reject_browser_only_flags(&c)?;

    // --- Wave 2 infra-scaffold wire-ups ---------------------------------
    // Validate the scaffold flags up-front so typos surface at CLI parse
    // time instead of deep inside a render job. The factories currently
    // return adapters that log + refuse (solver) or return `None`
    // (residential), so wiring is non-invasive: default `none` ⇒ no
    // behaviour change vs pre-wave-2.
    {
        use crate::antibot::solver::{build_solver, SolverKind};
        use crate::proxy::residential::{build_provider, ResidentialProviderKind};
        use std::str::FromStr;
        let res_kind = ResidentialProviderKind::from_str(&c.residential_provider)
            .map_err(|e| crate::Error::Config(format!("--residential-provider: {e}")))?;
        let _res = build_provider(res_kind);
        if !matches!(res_kind, ResidentialProviderKind::None) {
            tracing::info!(
                provider = res_kind.as_str(),
                "residential-provider scaffold selected (stub adapter — not yet routing traffic)"
            );
        }

        let solver_kind = SolverKind::from_str(&c.captcha_solver)
            .map_err(|e| crate::Error::Config(format!("--captcha-solver: {e}")))?;
        let _solver = build_solver(solver_kind);
        if !matches!(solver_kind, SolverKind::None) {
            tracing::info!(
                adapter = solver_kind.as_str(),
                "captcha-solver scaffold selected (stub adapter — AdapterNotConfigured until credentials wired)"
            );
        }
    }
    if let Some(raw) = c.mobile_profile.as_deref() {
        match crate::render::android_profile::parse_mobile_profile(raw) {
            Some(profile) => tracing::info!(
                width = profile.width,
                height = profile.height,
                ua = %profile.user_agent,
                "--mobile-profile scaffold selected (CDP emulation wire-up pending)"
            ),
            None => {
                return Err(crate::Error::Config(format!(
                    "unknown --mobile-profile `{raw}`; accepted aliases include \
                     pixel-7-pro, pixel8, s23, android"
                )))
            }
        }
    }

    let mut raffel = maybe_spawn_raffel_proxy(&mut c).await?;
    let result = async {
        // Config from --config wins as a base, then individual flags
        // override (CLI flags = explicit operator intent).
        let mut config = if let Some(path) = c.config.as_deref() {
            load_config_from_path_or_stdin(path)?
        } else {
            build_config_from_args(&c)?
        };
        #[cfg(not(feature = "cdp-backend"))]
        reject_browser_only_config(&config)?;
        if c.method != "spoof" && c.max_concurrent_render.is_none() {
            config.max_concurrent_render = 1;
        }
        // Install the motion engine preset into the process-wide ambient
        // slot read by `interact::*`. Doing it here rather than in the
        // render pool ctor keeps the hook in front of every render path
        // (Crawler, ScriptSpec, Lua, ref_resolver).
        #[cfg(feature = "cdp-backend")]
        config.motion_profile.set_active();
        #[cfg_attr(not(feature = "lua-hooks"), allow(unused_variables))]
        let render_enabled = config.max_concurrent_render > 0;

        // Resolve `--policy <fast|balanced|deep|forensics>`.
        let policy_profile = parse_policy_profile(&c.policy)?;

        // Resolve `--emit ndjson|none` → sink.
        let sink = build_event_sink(&c.emit, c.explain)?;

        #[allow(unused_mut)]
        let mut crawler = Crawler::new(config)?
            .with_events(sink.clone())
            .with_policy_profile(policy_profile);

        let mut seeds: Vec<String> = c.seed.clone();
        if let Some(path) = c.seeds_file.as_ref() {
            let content = std::fs::read_to_string(path).map_err(crate::Error::Io)?;
            for line in content.lines() {
                let l = line.trim();
                if !l.is_empty() && !l.starts_with('#') {
                    seeds.push(l.to_string());
                }
            }
        }
        let method = match c.method.as_str() {
            "render" => crate::queue::FetchMethod::Render,
            "auto" => crate::queue::FetchMethod::Auto,
            _ => crate::queue::FetchMethod::HttpSpoof,
        };
        #[cfg(feature = "lua-hooks")]
        {
            let scripts: Vec<std::path::PathBuf> =
                c.hook_script.iter().map(std::path::PathBuf::from).collect();
            if !scripts.is_empty() && render_enabled {
                crawler.set_lua_scripts(scripts)?;
            }
        }
        crawler.seed_with(seeds, method).await?;
        crawler.run().await?;
        // Final `crawl done` line only when stdout isn't already busy
        // streaming NDJSON — mixing them would corrupt the JSON lines.
        if c.emit.eq_ignore_ascii_case("none") {
            let storage = crawler.storage();
            let edges = crawler.graph().edge_count();
            if let Some(mem) = storage
                .as_any_ref()
                .and_then(|a| a.downcast_ref::<crate::storage::memory::MemoryStorage>())
            {
                println!(
                    "crawl done: raw={} rendered={} edges={}",
                    mem.raw.len(),
                    mem.rendered.len(),
                    edges,
                );
            } else {
                println!("crawl done: edges={}", edges);
            }
        }
        Ok(())
    }
    .await;

    if let Some(handle) = raffel.as_mut() {
        handle.shutdown().await;
    }
    result
}

async fn cmd_resume(_r: args::ResumeArgs) -> Result<()> {
    Err(crate::Error::Config(
        "`resume` not yet implemented — use `crawl --queue-path <existing.db>` \
         (with no --seed) to resume a persisted SQLite queue."
            .into(),
    ))
}

async fn cmd_inspect(i: args::InspectArgs) -> Result<()> {
    use std::str::FromStr;
    let profile = match i.profile.as_deref() {
        Some(spec) => Profile::from_str(spec).map_err(|e| {
            crate::Error::Config(format!(
                "invalid --profile `{spec}`: {e}. Examples: \
                 `chrome-149-linux`, `firefox-130-macos`."
            ))
        })?,
        None => Profile::Chrome149Stable,
    };
    let url = url::Url::parse(&i.url).map_err(crate::Error::UrlParse)?;
    let client = crate::impersonate::ImpersonateClient::new(profile)?;
    let resp = client.get(&url).await?;
    println!("status: {}", resp.status);
    println!("alpn: {:?}", resp.alpn);
    println!("tls: {:?}", resp.tls_version);
    println!("cipher: {:?}", resp.cipher);
    println!("body_bytes: {}", resp.body.len());
    if resp
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("json") || s.contains("text"))
        .unwrap_or(false)
    {
        let body = String::from_utf8_lossy(&resp.body);
        println!("---\n{}", body);
    }
    Ok(())
}

async fn cmd_test_stealth() -> Result<()> {
    use crate::impersonate::ImpersonateClient;
    let targets = [
        ("tls.peet.ws JA4", "https://tls.peet.ws/api/clean"),
        ("tls.browserleaks.com", "https://tls.browserleaks.com/json"),
    ];
    let client = ImpersonateClient::new(Profile::Chrome131Stable)?;
    let mut overall_ok = true;
    for (name, url) in targets {
        let u = url::Url::parse(url).map_err(crate::Error::UrlParse)?;
        match client.get(&u).await {
            Ok(r) => {
                let body_txt = String::from_utf8_lossy(&r.body).into_owned();
                let report = stealth_assertions(&r, &body_txt);
                println!(
                    "[{name}] status={} alpn={:?} cipher={:?} bytes={}",
                    r.status,
                    r.alpn,
                    r.cipher,
                    r.body.len()
                );
                for (pass, label) in &report.checks {
                    let mark = if *pass { "PASS" } else { "FAIL" };
                    println!("  {mark} {label}");
                    if !*pass {
                        overall_ok = false;
                    }
                }
                if let Some(ct) = r.headers.get("content-type").and_then(|v| v.to_str().ok()) {
                    if ct.contains("json") || ct.contains("text") {
                        let snippet: String = body_txt.chars().take(500).collect();
                        println!("{}", snippet);
                    }
                }
            }
            Err(e) => {
                println!("[{name}] error: {e}");
                overall_ok = false;
            }
        }
    }
    if !overall_ok {
        return Err(crate::Error::Config(
            "one or more stealth checks failed — see PASS/FAIL lines above".into(),
        ));
    }
    Ok(())
}

/// Grouped pass/fail view for `cmd_test_stealth`. Pure over the response
/// so it's unit-testable without hitting the network.
pub struct StealthReport {
    pub checks: Vec<(bool, String)>,
}

/// Assert the Chrome-shaped invariants that should hold on any healthy
/// TLS handshake from this crate: ALPN = h2, cipher is AEAD (no SHA1
/// suite sneaking back in), and — when the canary endpoint returns a
/// JA4-formatted blob — JA4 starts with `t13d` (TLS 1.3 ClientHello,
/// direct). Also surfaces the expected internal fingerprint summary so
/// operators can diff against what external services report.
pub fn stealth_assertions(resp: &crate::impersonate::Response, body: &str) -> StealthReport {
    let mut checks = Vec::new();

    // Informational: always-pass line with the fingerprint summary our
    // TLS stack is declared to produce. Reading it in production tells
    // operators exactly what we're pretending to be.
    // Use the newest stable profile for the summary so the line reflects
    // current TLS-era markers (X25519MLKEM768 for Chrome 132+, ECH grease).
    // Older profiles still validate via the catalog but their summary
    // legitimately reads X25519Kyber768.
    let expected = crate::impersonate::ja3::current_chrome_fingerprint_summary(
        crate::impersonate::Profile::Chrome149Stable,
    );
    checks.push((
        true,
        format!("internal TLS fingerprint summary = {expected}"),
    ));

    let alpn_ok = resp.alpn.as_deref() == Some("h2");
    checks.push((
        alpn_ok,
        format!(
            "ALPN negotiated `h2` (got {})",
            resp.alpn.as_deref().unwrap_or("<none>")
        ),
    ));

    let cipher = resp.cipher.as_deref().unwrap_or("");
    let sha1_tell =
        cipher.ends_with("-SHA") && !cipher.contains("SHA256") && !cipher.contains("SHA384");
    checks.push((
        !sha1_tell,
        format!("cipher `{cipher}` is not a legacy SHA1 suite"),
    ));

    // tls.peet.ws returns JA4 inside a JSON blob like `"ja4":"t13d..."`.
    // When the field exists, validate it; when it doesn't, skip rather
    // than failing so the same CLI works against endpoints that don't
    // compute a JA4.
    if let Some(start) = body.find("\"ja4\":\"") {
        let rest = &body[start + "\"ja4\":\"".len()..];
        if let Some(end) = rest.find('"') {
            let ja4 = &rest[..end];
            let prefix = ja4.starts_with("t13d");
            checks.push((
                prefix,
                format!("JA4 `{ja4}` starts with `t13d` (TLS 1.3, direct, Chrome-class)"),
            ));
        }
    }

    StealthReport { checks }
}

async fn cmd_queue(q: args::QueueCmd) -> Result<()> {
    #[cfg(feature = "sqlite")]
    {
        match q {
            args::QueueCmd::Stats { queue_path } => queue_stats(&queue_path),
            args::QueueCmd::Purge { queue_path } => queue_purge(&queue_path),
            args::QueueCmd::Export { queue_path, out } => queue_export(&queue_path, &out),
        }
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = q;
        Err(crate::Error::Config(
            "`queue` subcommand requires the `sqlite` feature".into(),
        ))
    }
}

#[cfg(feature = "sqlite")]
fn queue_stats(queue_path: &str) -> Result<()> {
    let conn = rusqlite::Connection::open(queue_path)
        .map_err(|e| crate::Error::Queue(format!("open {queue_path}: {e}")))?;
    let mut stmt = conn
        .prepare("SELECT state, count(*) FROM jobs GROUP BY state ORDER BY state")
        .map_err(|e| crate::Error::Queue(format!("prepare: {e}")))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| crate::Error::Queue(format!("query: {e}")))?;
    println!("state        count");
    println!("----------   -----");
    for row in rows {
        let (state, count) = row.map_err(|e| crate::Error::Queue(format!("row: {e}")))?;
        println!("{:<12} {}", state, count);
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn queue_purge(queue_path: &str) -> Result<()> {
    let conn = rusqlite::Connection::open(queue_path)
        .map_err(|e| crate::Error::Queue(format!("open {queue_path}: {e}")))?;
    let n = conn
        .execute("DELETE FROM jobs WHERE state IN ('done','failed')", [])
        .map_err(|e| crate::Error::Queue(format!("delete: {e}")))?;
    println!("purged {n} rows");
    Ok(())
}

#[cfg(feature = "sqlite")]
fn queue_export(queue_path: &str, out: &str) -> Result<()> {
    use std::io::Write;
    let conn = rusqlite::Connection::open(queue_path)
        .map_err(|e| crate::Error::Queue(format!("open: {e}")))?;
    let mut stmt = conn
        .prepare("SELECT id, url, state, attempts, last_error FROM jobs")
        .map_err(|e| crate::Error::Queue(format!("prepare: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({
                "id":         r.get::<_, i64>(0)?,
                "url":        r.get::<_, String>(1)?,
                "state":      r.get::<_, String>(2)?,
                "attempts":   r.get::<_, i64>(3)?,
                "last_error": r.get::<_, Option<String>>(4)?,
            }))
        })
        .map_err(|e| crate::Error::Queue(format!("query: {e}")))?;
    let mut f = std::fs::File::create(out).map_err(crate::Error::Io)?;
    let mut n = 0u64;
    for row in rows {
        let v = row.map_err(|e| crate::Error::Queue(format!("row: {e}")))?;
        writeln!(f, "{}", v).map_err(crate::Error::Io)?;
        n += 1;
    }
    println!("wrote {n} rows to {out}");
    Ok(())
}

/// Aggregated challenge-rate dashboard — reads the `v_challenge_rate_*`
/// views written by the storage layer's init_db. CLI is feature-gated by
/// `cli`, storage by `sqlite`; returns a clear error when compiled without
/// the latter so operators don't see a silent no-op.
async fn cmd_telemetry(t: args::TelemetryCmd) -> Result<()> {
    #[cfg(feature = "sqlite")]
    {
        match t {
            args::TelemetryCmd::Show { db, top } => telemetry_challenge(&db, top),
        }
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = t;
        Err(crate::Error::Config(
            "`telemetry` subcommand requires the `sqlite` feature".into(),
        ))
    }
}

#[cfg(feature = "sqlite")]
fn telemetry_challenge(db_path: &str, top: usize) -> Result<()> {
    // Open the storage DB via SqliteStorage so the views exist even on a
    // freshly-created file — init_db runs CREATE VIEW IF NOT EXISTS. The
    // handle is dropped immediately; we read through a fresh read-only
    // Connection to bypass the writer thread.
    let _ = crate::storage::sqlite::SqliteStorage::open(db_path)?;
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| crate::Error::Storage(format!("open {db_path}: {e}")))?;

    let by_vendor = query_challenge_rate(&conn, "v_challenge_rate_by_vendor", None)?;
    let by_proxy = query_challenge_rate(&conn, "v_challenge_rate_by_proxy", None)?;
    let by_session = query_challenge_rate(&conn, "v_challenge_rate_by_session", Some(top))?;

    print!("{}", format_challenge_table("vendor", &by_vendor));
    println!();
    print!("{}", format_challenge_table("proxy", &by_proxy));
    println!();
    print!(
        "{}",
        format_challenge_table(&format!("session_id (top {top})"), &by_session)
    );
    Ok(())
}

#[cfg(feature = "sqlite")]
fn query_challenge_rate(
    conn: &rusqlite::Connection,
    view: &str,
    limit: Option<usize>,
) -> Result<Vec<ChallengeRateRow>> {
    // View names are static literals (not user-controlled) so interpolation
    // here is safe; binding a table name is not supported by SQL anyway.
    let sql = match limit {
        Some(n) => format!(
            "SELECT {0}, total, last_24h FROM {1} LIMIT {2}",
            match view {
                "v_challenge_rate_by_vendor" => "vendor",
                "v_challenge_rate_by_proxy" => "proxy",
                "v_challenge_rate_by_session" => "session_id",
                _ => return Err(crate::Error::Storage(format!("unknown view {view}"))),
            },
            view,
            n
        ),
        None => format!(
            "SELECT {0}, total, last_24h FROM {1}",
            match view {
                "v_challenge_rate_by_vendor" => "vendor",
                "v_challenge_rate_by_proxy" => "proxy",
                "v_challenge_rate_by_session" => "session_id",
                _ => return Err(crate::Error::Storage(format!("unknown view {view}"))),
            },
            view
        ),
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| crate::Error::Storage(format!("prepare {view}: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ChallengeRateRow {
                key: r.get::<_, String>(0)?,
                total: r.get::<_, i64>(1)?,
                last_24h: r.get::<_, i64>(2)?,
            })
        })
        .map_err(|e| crate::Error::Storage(format!("query {view}: {e}")))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| crate::Error::Storage(format!("row {view}: {e}")))?);
    }
    Ok(out)
}

/// One row of a `v_challenge_rate_*` view. Kept ad-hoc here rather than in
/// `storage/` because it exists only to drive the operator CLI; the
/// authoritative schema is the view itself.
#[cfg_attr(not(any(feature = "sqlite", test)), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChallengeRateRow {
    pub key: String,
    pub total: i64,
    pub last_24h: i64,
}

/// Pure-formatting helper so the test suite can exercise output shaping
/// without spinning a real SQLite file. Columns are sized for a 120-col
/// terminal with room to spare for the widest realistic key (proxy URLs).
#[cfg_attr(not(any(feature = "sqlite", test)), allow(dead_code))]
pub(crate) fn format_challenge_table(header: &str, rows: &[ChallengeRateRow]) -> String {
    const KEY_W: usize = 64;
    const NUM_W: usize = 10;
    let mut out = String::new();
    out.push_str(&format!(
        "{:<KEY_W$}  {:>NUM_W$}  {:>NUM_W$}\n",
        header, "total", "last_24h"
    ));
    out.push_str(&format!(
        "{:-<KEY_W$}  {:-<NUM_W$}  {:-<NUM_W$}\n",
        "", "", ""
    ));
    if rows.is_empty() {
        out.push_str("(no data)\n");
        return out;
    }
    for r in rows {
        // Truncate overlong keys so wrapping never breaks alignment.
        // Operators scan columns; wrapping destroys the grid.
        let key = if r.key.chars().count() > KEY_W {
            let truncated: String = r.key.chars().take(KEY_W - 1).collect();
            format!("{truncated}~")
        } else {
            r.key.clone()
        };
        out.push_str(&format!(
            "{:<KEY_W$}  {:>NUM_W$}  {:>NUM_W$}\n",
            key, r.total, r.last_24h
        ));
    }
    out
}

async fn cmd_export_graph(g: args::ExportGraphArgs) -> Result<()> {
    #[cfg(feature = "sqlite")]
    {
        use std::io::Write;
        let conn = rusqlite::Connection::open(&g.storage_path)
            .map_err(|e| crate::Error::Storage(format!("open: {e}")))?;
        let mut stmt = conn
            .prepare("SELECT src, dst, weight FROM edges")
            .map_err(|e| crate::Error::Storage(format!("prepare: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(serde_json::json!({
                    "src":    r.get::<_, String>(0)?,
                    "dst":    r.get::<_, String>(1)?,
                    "weight": r.get::<_, i64>(2)?,
                }))
            })
            .map_err(|e| crate::Error::Storage(format!("query: {e}")))?;
        let mut f = std::fs::File::create(&g.out).map_err(crate::Error::Io)?;
        let mut n = 0u64;
        for row in rows {
            let v = row.map_err(|e| crate::Error::Storage(format!("row: {e}")))?;
            writeln!(f, "{}", v).map_err(crate::Error::Io)?;
            n += 1;
        }
        println!("exported {n} edges to {}", g.out);
        Ok(())
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = g;
        Err(crate::Error::Config(
            "`export-graph` requires the `sqlite` feature".into(),
        ))
    }
}

/// Run `<chrome> --version` and map to the closest Profile. Returns None if
/// the binary can't be invoked — caller uses a sensible default.
fn detect_chrome_profile(chrome_path: Option<&str>) -> Option<Profile> {
    let bin = chrome_path.unwrap_or("google-chrome");
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for tok in text.split_whitespace() {
        if let Some(head) = tok.split('.').next() {
            if let Ok(v) = head.parse::<u32>() {
                if v >= 90 {
                    return Some(Profile::from_detected_major(v));
                }
            }
        }
    }
    None
}

fn build_config_from_args(c: &args::CrawlArgs) -> Result<Config> {
    // CLI → env wiring for the render-side timeout / lifecycle gates.
    // The values are read once via `OnceLock` inside the chrome handler
    // (`render::chrome::handler::request_timeout_ms`), so we set them
    // BEFORE any handler/browser is created. CLI flag wins over a
    // pre-existing env value so `--render-request-timeout-ms 60000`
    // overrides a stale `CRAWLEX_REQUEST_TIMEOUT_MS=30000` from the
    // shell's profile.
    if let Some(ms) = c.render_request_timeout_ms {
        // SAFETY: Rust 1.91 marks set_var as unsafe; we accept the call
        // is sound here because it runs before any other thread reads env.
        std::env::set_var("CRAWLEX_REQUEST_TIMEOUT_MS", ms.to_string());
    }
    if let Some(lc) = c.navigation_lifecycle.as_deref() {
        std::env::set_var("CRAWLEX_NAVIGATION_LIFECYCLE", lc);
    }

    // Profile priority:
    // 1. Explicit --profile flag (honours user intent). Accepts any
    //    `<browser>-<major>-<os>` form via `Profile::from_str` — e.g.
    //    `chrome-149-linux`, `firefox-130-macos`, `chromium-122-linux`.
    //    Legacy `chrome-131-stable` / `chrome-149-stable` aliases still work.
    // 2. Auto-detect from `<chrome-path> --version` — keeps spoof UA in
    //    lockstep with the browser the render pool will actually launch.
    // 3. Fall back to a sensible recent default.
    let profile = match c.profile.as_deref() {
        Some(spec) => {
            use std::str::FromStr;
            Profile::from_str(spec).map_err(|e| {
                crate::Error::Config(format!(
                    "invalid --profile `{spec}`: {e}. Examples: \
                     `chrome-149-linux`, `firefox-130-macos`, \
                     `chromium-122-linux`"
                ))
            })?
        }
        None => detect_chrome_profile(c.chrome_path.as_deref()).unwrap_or(Profile::Chrome131Stable),
    };

    let wait_strategy = match c.wait_strategy.as_deref().unwrap_or("networkidle") {
        "load" => WaitStrategy::Load,
        "domcontentloaded" => WaitStrategy::DomContentLoaded,
        "fixed" => WaitStrategy::Fixed {
            ms: c.wait_idle_ms.unwrap_or(1000),
        },
        _ => WaitStrategy::NetworkIdle {
            idle_ms: c.wait_idle_ms.unwrap_or(500),
        },
    };

    let queue_backend = match c.queue.as_deref().unwrap_or("inmemory") {
        "sqlite" => QueueBackend::Sqlite {
            path: c.queue_path.clone().unwrap_or_else(|| "queue.db".into()),
        },
        "redis" => {
            return Err(crate::Error::Config(
                "redis queue backend not implemented — use `sqlite` or `inmemory`".into(),
            ));
        }
        _ => QueueBackend::InMemory,
    };

    let storage_backend = match c.storage.as_deref().unwrap_or("memory") {
        "sqlite" => StorageBackend::Sqlite {
            path: c.storage_path.clone().unwrap_or_else(|| "crawl.db".into()),
        },
        "filesystem" => StorageBackend::Filesystem {
            root: c.storage_path.clone().unwrap_or_else(|| "crawl-out".into()),
        },
        _ => StorageBackend::Memory,
    };

    let strategy = match c.proxy_strategy.as_deref().unwrap_or("round-robin") {
        "sequential" => RotationStrategy::Sequential,
        "random" => RotationStrategy::Random,
        "sticky-per-host" => RotationStrategy::StickyPerHost,
        _ => RotationStrategy::RoundRobin,
    };

    let mut proxies = c.proxy.clone();
    if let Some(file) = c.proxy_file.as_ref() {
        let content = if file == "-" {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(crate::Error::Io)?;
            s
        } else {
            std::fs::read_to_string(file).map_err(crate::Error::Io)?
        };
        for line in content.lines() {
            let l = line.trim();
            if !l.is_empty() && !l.starts_with('#') {
                proxies.push(l.to_string());
            }
        }
    }

    Ok(Config {
        max_concurrent_render: c.max_concurrent_render.unwrap_or(0),
        max_concurrent_http: c.max_concurrent_http.unwrap_or(500),
        max_depth: c.max_depth,
        same_host_only: c.same_host_only,
        include_subdomains: c.include_subdomains,
        // Recon target is not yet exposed as a CLI flag (Fase B wires it
        // via `crawlex intel <target>` and `crawlex crawl --target …`);
        // default None keeps the existing frontier behaviour.
        target_domain: None,
        infra_intel: crate::config::InfraIntelConfig::default(),
        identity_preset: c.identity_preset,
        respect_robots_txt: c.respect_robots_txt.unwrap_or(true),
        user_agent_profile: profile,
        chrome_path: c.chrome_path.clone(),
        chrome_flags: c.chrome_flag.clone(),
        block_resources: c
            .block_resource
            .as_deref()
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default(),
        wait_strategy,
        rate_per_host_rps: c.rate_per_host_rps,
        retry_max: c.retry_max.unwrap_or(3),
        retry_backoff: std::time::Duration::from_millis(c.retry_backoff_ms.unwrap_or(500)),
        queue_backend,
        storage_backend,
        output: crate::config::OutputConfig {
            html_dir: c.output_html_dir.clone(),
            graph_path: c.output_graph.clone(),
            metadata_path: c.output_metadata.clone(),
            screenshot_dir: c.screenshot_dir.clone(),
            screenshot: c.screenshot,
            screenshot_mode: {
                // Validate up-front so a bad value fails before we start
                // Chrome. Only enforces when the backend exists; mini build
                // rejects `--screenshot` entirely elsewhere.
                if let Some(s) = c.screenshot_mode.as_deref() {
                    #[cfg(feature = "cdp-backend")]
                    {
                        crate::render::pool::parse_screenshot_mode(s)
                            .map_err(crate::Error::Config)?;
                    }
                    Some(s.to_string())
                } else {
                    None
                }
            },
        },
        proxy: ProxyConfig {
            proxies,
            proxy_file: c.proxy_file.clone(),
            strategy,
            sticky_per_host: c.proxy_sticky_per_host,
            health_check_interval: c
                .proxy_health_check_interval_secs
                .map(std::time::Duration::from_secs),
        },
        locale: c.locale.clone(),
        timezone: c.timezone.clone(),
        metrics_prometheus_port: c.metrics_prometheus_port,
        hook_scripts: c.hook_script.clone(),
        discovery_filter_regex: c.on_discovery_filter_regex.clone(),
        follow_pages_only: !c.follow_all_assets,
        crtsh_enabled: c.crtsh,
        robots_paths_enabled: !c.no_robots_paths,
        well_known_enabled: !c.no_well_known,
        pwa_enabled: !c.no_pwa,
        wayback_enabled: c.wayback,
        favicon_enabled: !c.no_favicon,
        dns_enabled: c.dns,
        collect_net_timings: c.metrics || c.metrics_net,
        collect_web_vitals: c.metrics || c.metrics_vitals,
        collect_peer_cert: c.peer_cert,
        rdap_enabled: c.rdap,
        cookies_enabled: !c.no_cookies,
        render_session_scope: c
            .render_session_scope
            .as_deref()
            .map(parse_render_session_scope)
            .transpose()?
            .unwrap_or(RenderSessionScope::RegistrableDomain),
        follow_redirects: !c.no_follow_redirects,
        max_redirects: c.max_redirects.unwrap_or(10),
        profile_autodetect: true,
        user_agent_override: c.user_agent_override.clone(),
        auto_fetch_chromium: !c.no_fetch_chromium,
        action_policy: parse_action_policy(c.action_policy.as_deref())?,
        challenge_mode: c
            .challenge_mode
            .as_deref()
            .map(parse_challenge_mode)
            .transpose()?
            .unwrap_or(ChallengeMode::SolverReady),
        collect_runtime_routes: !c.no_spa_observer,
        collect_network_endpoints: !c.no_spa_observer,
        collect_indexeddb: c.collect_indexeddb || c.collect_spa_state,
        collect_cache_storage: c.collect_cache_storage || c.collect_spa_state,
        collect_manifest: !c.no_spa_observer,
        collect_service_workers: !c.no_spa_observer,
        max_browsers: c.max_browsers.unwrap_or(4),
        max_pages_per_context: c.max_pages_per_context.unwrap_or(4),
        render_budgets: crate::scheduler::BudgetLimits {
            max_per_host: c.max_per_host_inflight.unwrap_or(4),
            max_per_origin: c.max_per_origin_inflight.unwrap_or(2),
            max_per_proxy: c.max_per_proxy_inflight.unwrap_or(8),
            max_per_session: c.max_per_session_inflight.unwrap_or(1),
            ..Default::default()
        },
        session_ttl_secs: c
            .session_ttl_secs
            .unwrap_or(crate::identity::DEFAULT_SESSION_TTL_SECS),
        drop_session_on_block: !c.keep_blocked_sessions,
        session_scope_auto: c.session_scope_auto,
        #[cfg(feature = "cdp-backend")]
        motion_profile: match c.motion_profile.as_deref() {
            Some(s) => crate::render::motion::MotionProfile::from_str_ci(s).ok_or_else(|| {
                crate::Error::Config(format!(
                    "invalid --motion-profile `{s}`: want fast|balanced|human|paranoid"
                ))
            })?,
            None => crate::render::motion::MotionProfile::default(),
        },
        #[cfg(feature = "cdp-backend")]
        actions: None,
        #[cfg(feature = "cdp-backend")]
        script_spec: match (c.script_spec.as_deref(), c.actions_file.as_deref()) {
            (Some(p), None) => Some(load_script_spec(p)?),
            (None, Some(p)) => Some(load_actions_file_as_script_spec(p)?),
            (None, None) => None,
            (Some(_), Some(_)) => unreachable!("clap enforces conflicts_with"),
        },
        // Warmup stays opt-in from the CLI path — operators enable it via
        // config file. Keeping the default here avoids a new CLI surface
        // until we see demand for one.
        warmup: crate::config::WarmupPolicy::default(),
        // Reading dwell is off unless `--reading-dwell` is passed — we
        // don't want a silent ~seconds-per-page throughput hit for users
        // upgrading past this commit.
        reading_dwell: if c.reading_dwell {
            Some(crate::config::ReadingDwellConfig {
                enabled: true,
                wpm: c.reading_dwell_wpm,
                jitter_ms: c.reading_dwell_jitter_ms,
                ..Default::default()
            })
        } else {
            None
        },
        http_limits: crate::config::HttpLimits::default(),
        content_store: crate::config::ContentStoreConfig::default(),
    })
}

#[cfg(feature = "cdp-backend")]
fn load_script_spec(path: &str) -> Result<crate::script::ScriptSpec> {
    let data = std::fs::read(path).map_err(crate::Error::Io)?;
    crate::script::ScriptSpec::from_json(&data)
        .map_err(|e| crate::Error::Config(format!("script-spec: {e}")))
}

#[cfg(feature = "cdp-backend")]
fn load_actions_file_as_script_spec(path: &str) -> Result<crate::script::ScriptSpec> {
    let data = std::fs::read_to_string(path).map_err(crate::Error::Io)?;
    let parsed: Vec<crate::render::actions::Action> = serde_json::from_str(&data)
        .map_err(|e| crate::Error::Config(format!("actions file: {e}")))?;
    Ok(crate::script::actions_to_script_spec(&parsed))
}

/// Guard used by the mini build: when an `--actions-file` was passed
/// but the binary has no browser backend, we refuse at CLI time rather
/// than silently dropping the file. `crawlex-mini` users get a stable
/// error early — no mystery about why their form-fill script didn't run.
#[cfg(not(feature = "cdp-backend"))]
#[allow(dead_code)]
fn reject_browser_only_flags(c: &args::CrawlArgs) -> Result<()> {
    let render_requested = c.method.eq_ignore_ascii_case("render")
        || c.max_concurrent_render.unwrap_or(0) > 0
        || c.actions_file.is_some()
        || c.script_spec.is_some()
        || c.screenshot
        || c.screenshot_dir.is_some()
        || c.screenshot_mode.is_some()
        || c.chrome_path.is_some()
        || !c.chrome_flag.is_empty()
        || c.block_resource.is_some()
        || c.wait_strategy.is_some()
        || c.wait_idle_ms.is_some()
        || c.metrics
        || c.metrics_vitals
        || !c.hook_script.is_empty();
    if render_requested {
        return Err(crate::Error::RenderDisabled(
            "render-disabled: this build has no browser backend. \
             Use `crawlex` (full) for render, screenshots, actions, vitals, \
             Chrome flags, or `--method render`."
                .into(),
        ));
    }
    Ok(())
}

#[cfg(not(feature = "cdp-backend"))]
fn reject_browser_only_config(config: &Config) -> Result<()> {
    let render_requested = config.max_concurrent_render > 0
        || config.output.screenshot
        || config.output.screenshot_dir.is_some()
        || config.collect_web_vitals
        || config.chrome_path.is_some()
        || !config.chrome_flags.is_empty()
        || !config.block_resources.is_empty();
    if render_requested {
        return Err(crate::Error::RenderDisabled(
            "render-disabled: this build has no browser backend. \
             Use `crawlex` (full) for render-related config fields."
                .into(),
        ));
    }
    Ok(())
}

async fn maybe_spawn_raffel_proxy(
    c: &mut args::CrawlArgs,
) -> Result<Option<raffel_proxy::RaffelProxyHandle>> {
    if !c.raffel_proxy {
        return Ok(None);
    }
    if !c.proxy.is_empty() || c.proxy_file.is_some() {
        return Err(crate::Error::Config(
            "`--raffel-proxy` currently owns the proxy list; don't combine it \
             with `--proxy`/`--proxy-file` yet"
                .into(),
        ));
    }

    let opts = raffel_proxy::RaffelProxyOptions {
        raffel_path: std::path::PathBuf::from(&c.raffel_proxy_path),
        host: c.raffel_proxy_host.clone(),
        port: c.raffel_proxy_port,
    };
    let handle = raffel_proxy::spawn(&opts).await?;
    tracing::info!(proxy = handle.proxy_url(), "launched local raffel proxy");
    c.proxy.push(handle.proxy_url().to_string());
    Ok(Some(handle))
}

#[cfg(test)]
mod stealth_report_tests {
    use super::stealth_assertions;
    use crate::impersonate::Response;
    use bytes::Bytes;
    use http::{HeaderMap, StatusCode};

    fn fake_response(alpn: Option<&str>, cipher: Option<&str>) -> Response {
        Response {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            final_url: "https://example.test/".parse().unwrap(),
            alpn: alpn.map(str::to_string),
            tls_version: Some("TLS1.3".into()),
            cipher: cipher.map(str::to_string),
            timings: crate::metrics::NetworkTimings::default(),
            peer_cert: None,
            body_truncated: false,
        }
    }

    #[test]
    fn summary_line_always_present() {
        let resp = fake_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"));
        let report = stealth_assertions(&resp, "{}");
        // First check is the informational summary; must always pass and
        // mention the MLKEM / ECH / cert_comp signature.
        let (ok, msg) = &report.checks[0];
        assert!(*ok, "summary line must be an always-pass info check");
        assert!(
            msg.contains("MLKEM768"),
            "summary must reference X25519MLKEM768: {msg}"
        );
        assert!(
            msg.contains("ech=1"),
            "summary must declare ECH grease: {msg}"
        );
        assert!(
            msg.contains("cert_comp=[2,1,3]"),
            "summary must list cert_compression [brotli,zlib,zstd]: {msg}"
        );
    }

    #[test]
    fn alpn_mismatch_flags_fail() {
        let resp = fake_response(Some("http/1.1"), Some("TLS_AES_128_GCM_SHA256"));
        let report = stealth_assertions(&resp, "{}");
        // One check must fail: ALPN != h2.
        let any_fail = report
            .checks
            .iter()
            .any(|(ok, msg)| !ok && msg.contains("ALPN"));
        assert!(any_fail, "stealth report should flag non-h2 ALPN");
    }

    #[test]
    fn legacy_sha1_cipher_flagged() {
        let resp = fake_response(Some("h2"), Some("ECDHE-RSA-AES128-SHA"));
        let report = stealth_assertions(&resp, "{}");
        let sha1_flag = report
            .checks
            .iter()
            .any(|(ok, msg)| !ok && msg.contains("SHA1"));
        assert!(sha1_flag, "stealth report should flag SHA1 cipher");
    }

    #[test]
    fn ja4_from_external_body_validated() {
        let resp = fake_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"));
        let body = r#"{"ja4":"t13d1516h2_8daaf6152771_b186095e22b6","something":"else"}"#;
        let report = stealth_assertions(&resp, body);
        let ja4_pass = report
            .checks
            .iter()
            .any(|(ok, msg)| *ok && msg.contains("t13d"));
        assert!(
            ja4_pass,
            "external t13d JA4 should pass: {:?}",
            report.checks
        );
    }
}

#[cfg(test)]
mod telemetry_format_tests {
    // Pure-string shaping only — no DB, no async. Lets the operator-facing
    // layout regress loudly without requiring a live SQLite file.
    use super::{format_challenge_table, ChallengeRateRow};

    fn row(key: &str, total: i64, last_24h: i64) -> ChallengeRateRow {
        ChallengeRateRow {
            key: key.to_string(),
            total,
            last_24h,
        }
    }

    #[test]
    fn header_and_separator_rendered() {
        let s = format_challenge_table("vendor", &[row("cloudflare", 42, 7)]);
        let mut lines = s.lines();
        let header = lines.next().unwrap();
        let sep = lines.next().unwrap();
        assert!(header.starts_with("vendor"), "header: {header}");
        assert!(header.contains("total"), "header missing total: {header}");
        assert!(
            header.contains("last_24h"),
            "header missing last_24h: {header}"
        );
        assert!(sep.starts_with("---"), "separator: {sep}");
    }

    #[test]
    fn empty_rows_emit_placeholder() {
        let s = format_challenge_table("proxy", &[]);
        assert!(s.contains("(no data)"), "missing placeholder: {s}");
    }

    #[test]
    fn row_values_present_and_right_aligned() {
        let s = format_challenge_table("vendor", &[row("akamai", 1234, 56), row("datadome", 7, 7)]);
        assert!(s.contains("akamai"), "missing row key: {s}");
        assert!(s.contains("1234"), "missing total: {s}");
        assert!(s.contains("datadome"), "missing second row: {s}");
        // Every row must fit the 120-col operator budget.
        for line in s.lines() {
            assert!(line.len() <= 120, "line over 120 cols: {line:?}");
        }
    }

    #[test]
    fn overlong_key_truncated_with_marker() {
        let long = "http://".to_string() + &"a".repeat(200) + ".example:8080";
        let s = format_challenge_table("proxy", &[row(&long, 1, 1)]);
        // The truncation marker preserves grid alignment — widest row still
        // fits the column width.
        assert!(s.contains('~'), "expected truncation marker: {s}");
        for line in s.lines() {
            assert!(line.len() <= 120, "line over 120 cols: {line:?}");
        }
    }
}
