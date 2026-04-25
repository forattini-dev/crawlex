//! Single-file HTML intel report for the `fingerprint export --html` CLI.
//!
//! Given a target + sqlite path, `render()` returns a fully self-contained
//! HTML5 document (inline `<style>`, no external JS/CSS) summarising every
//! intel surface the terminal `fingerprint show` command exposes: WHOIS,
//! DNS rollup, IPs with cloud/CDN badges, open ports grouped by IP, certs,
//! external domain pills (D1 categories), asset-ref kind rollup, and
//! subdomains collapsed into `<details>`.
//!
//! The module reuses the same SQL query shapes as `cmd_intel_show` /
//! `cmd_intel_export` so operators get a 1:1 mapping between the terminal
//! view and the dashboard. Numbers that disagree would be a bug.
//!
//! Constraints the implementation honours:
//!   * **No external crate deps** — everything builds on `rusqlite`,
//!     `serde_json`, and `std::fmt::Write`, all already in tree.
//!   * **No JavaScript** — the dashboard has to open cleanly from a file
//!     URL in any browser, including offline viewers and air-gapped boxes.
//!   * **Every interpolated value is HTML-escaped** via the local `h()`
//!     helper. Untrusted values (WHOIS fields, domain strings, certs)
//!     never reach the output raw.

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// `(registrar, registrant_org, created_at, expires_at, nameservers_json)`
/// — shape returned by the WHOIS lookup query in `write_whois`.
type WhoisRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

/// `(port, product?, version?)` — open-port detail under one IP, used by
/// `write_open_ports_by_ip`.
type PortDetail = (i64, Option<String>, Option<String>);

/// Build the HTML report for `target` against the sqlite db at `db_path`.
///
/// Returns the full HTML document as a `String`. Caller is responsible for
/// writing it to disk (the CLI layer does that). All queries are best-effort:
/// an empty table just skips its section, and the top-level summary falls
/// back to `(no rows)` when the recon hasn't run yet.
pub fn render(target: &str, db_path: &Path) -> Result<String> {
    let conn = Connection::open(db_path).map_err(|e| Error::Storage(e.to_string()))?;
    let target = target.trim();

    let mut out = String::with_capacity(16 * 1024);
    write_header(&mut out, target);
    write_style(&mut out);
    write_body(&mut out, &conn, target)?;
    write_footer(&mut out);
    Ok(out)
}

fn write_header(out: &mut String, target: &str) {
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\"><head>");
    out.push_str("<meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    let _ = write!(out, "<title>crawlex intel — {}</title>", h(target));
}

/// Inline, dark-friendly stylesheet. Kept intentionally compact; the body
/// layout is a single-column flow with `<section>` cards and a handful of
/// table styles. Pill colours are derived per-category via `pill-{slug}`.
fn write_style(out: &mut String) {
    out.push_str(
        r#"<style>
:root{
  --bg:#0f1115;--surface:#171a21;--surface2:#1e222b;--border:#2a2f3a;
  --fg:#e6e8ee;--muted:#8a93a6;--accent:#6aa7ff;--warn:#f2b347;--bad:#ef6a6a;
}
*{box-sizing:border-box}
html,body{background:var(--bg);color:var(--fg);margin:0;padding:0;font:14px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:1100px;margin:0 auto;padding:24px}
h1{font-size:22px;margin:0 0 8px;color:var(--fg)}
h2{font-size:16px;margin:0 0 10px;color:var(--accent);letter-spacing:.02em;text-transform:uppercase}
.summary{color:var(--muted);margin:0 0 18px;font-family:ui-monospace,Menlo,Consolas,monospace;font-size:13px}
section{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:16px;margin:0 0 14px}
table{width:100%;border-collapse:collapse;font-size:13px}
th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--border);vertical-align:top}
th{color:var(--muted);font-weight:600;text-transform:uppercase;font-size:11px;letter-spacing:.05em}
tr:last-child td{border-bottom:0}
tbody tr:hover{background:var(--surface2)}
code,.mono{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:12px;color:var(--fg)}
.muted{color:var(--muted)}
.empty{color:var(--muted);font-style:italic}
.grid{display:grid;grid-template-columns:160px 1fr;gap:6px 14px;font-size:13px}
.grid dt{color:var(--muted)}
.grid dd{margin:0;word-break:break-word}
.badge{display:inline-block;padding:1px 6px;border-radius:4px;background:var(--surface2);color:var(--fg);font-size:11px;margin-right:4px;border:1px solid var(--border)}
.badge-warn{background:#3a2b14;color:var(--warn);border-color:#5a4220}
.badge-bad{background:#3a1616;color:var(--bad);border-color:#5a2020}
.pill{display:inline-block;padding:1px 8px;border-radius:999px;font-size:11px;margin:1px 3px 1px 0;background:var(--surface2);border:1px solid var(--border);color:var(--fg)}
.pill-analytics{background:#1c2a3a;border-color:#2f4b6e;color:#9cc4ff}
.pill-cdn{background:#1b2f24;border-color:#2f5a43;color:#8fe0b3}
.pill-ads{background:#3a1e1e;border-color:#5a2a2a;color:#ff9e9e}
.pill-social{background:#2b1f3a;border-color:#47325a;color:#c59bff}
.pill-font-service{background:#2e2a14;border-color:#524720;color:#ebd37a}
.pill-tag-manager{background:#1f3a35;border-color:#326558;color:#8cdbc9}
.pill-cloud-storage{background:#1f2b3a;border-color:#33466e;color:#9cbcff}
.pill-video-host{background:#3a1f2f;border-color:#5a304b;color:#ff9ecc}
.pill-payments{background:#2a3a1f;border-color:#48652f;color:#bde07b}
.pill-auth{background:#3a2b14;border-color:#5a4220;color:#f2b347}
.pill-support{background:#252a33;border-color:#3a4151;color:#c9d1e1}
.pill-maps{background:#14323a;border-color:#1f5060;color:#7fd8eb}
.pill-captcha{background:#3a1420;border-color:#5a2030;color:#f48a9c}
.pill-other{background:var(--surface2);border-color:var(--border);color:var(--muted)}
details{margin-top:8px}
details>summary{cursor:pointer;color:var(--accent);font-size:13px;user-select:none}
details[open]>summary{margin-bottom:8px}
.sub-list{columns:3;column-gap:18px;font-family:ui-monospace,Menlo,Consolas,monospace;font-size:12px;color:var(--fg)}
.sub-list div{break-inside:avoid;padding:1px 0}
footer{color:var(--muted);font-size:12px;margin-top:24px;text-align:center;padding-bottom:32px}
@media (max-width:700px){.grid{grid-template-columns:1fr}.sub-list{columns:1}}
</style></head><body><main>"#,
    );
}

fn write_body(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let _ = write!(out, "<h1>Target: {}</h1>", h(target));

    // --- Summary line from v_target_intel ----------------------------------
    let summary: Option<(i64, i64, i64, i64, i64)> = conn
        .query_row(
            "SELECT domains, subdomains, wildcard_dns, unique_ips, certs_seen \
             FROM v_target_intel WHERE target_root = ?1",
            rusqlite::params![target],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();
    match summary {
        Some((d, s, w, i, c)) => {
            let _ = write!(
                out,
                "<p class=\"summary\">summary: domains={d} subdomains={s} wildcard_dns={w} ips={i} certs_seen={c}</p>"
            );
        }
        None => {
            out.push_str("<p class=\"summary\">summary: (no rows)</p>");
        }
    }

    write_whois(out, conn, target)?;
    write_dns_rollup(out, conn, target)?;
    write_ips(out, conn, target)?;
    write_ports(out, conn, target)?;
    write_certs(out, conn, target)?;
    write_external_domains(out, conn, target)?;
    write_asset_kind_rollup(out, conn, target)?;
    write_subdomains(out, conn, target)?;
    Ok(())
}

fn write_whois(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let row: Option<WhoisRow> = conn
        .query_row(
            "SELECT registrar, registrant_org, \
                 datetime(created_at,'unixepoch'), \
                 datetime(expires_at,'unixepoch'), \
                 COALESCE(nameservers_json,'[]') \
             FROM whois_records WHERE domain = ?1",
            rusqlite::params![target],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .ok();
    out.push_str("<section><h2>WHOIS</h2>");
    match row {
        Some((reg, org, created, expires, ns_json)) => {
            out.push_str("<dl class=\"grid\">");
            dl_row(out, "registrar", reg.as_deref());
            dl_row(out, "registrant_org", org.as_deref());
            dl_row(out, "created", created.as_deref());
            dl_row(out, "expires", expires.as_deref());
            let ns: Vec<String> = serde_json::from_str(&ns_json).unwrap_or_default();
            if !ns.is_empty() {
                let joined = ns.join(", ");
                dl_row(out, "nameservers", Some(joined.as_str()));
            }
            out.push_str("</dl>");
        }
        None => out.push_str("<p class=\"empty\">No WHOIS record.</p>"),
    }
    out.push_str("</section>");
    Ok(())
}

fn dl_row(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(v) = value.filter(|s| !s.is_empty()) {
        let _ = write!(out, "<dt>{}</dt><dd>{}</dd>", h(label), h(v));
    }
}

fn write_dns_rollup(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT record_type, COUNT(*) FROM dns_records \
             WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1) \
             GROUP BY record_type ORDER BY record_type",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows: Vec<(String, i64)> = stmt
        .query_map(rusqlite::params![target], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>DNS records</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No DNS records.</p>");
    } else {
        out.push_str("<table><thead><tr><th>record_type</th><th>count</th></tr></thead><tbody>");
        for (t, n) in rows {
            let _ = write!(
                out,
                "<tr><td><code>{}</code></td><td>{}</td></tr>",
                h(&t),
                n
            );
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_ips(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT ip, reverse_ptr, cloud_provider, cdn, asn, asn_name, country \
             FROM ip_addresses \
             WHERE ip IN (SELECT ip FROM domain_ips \
                 WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1)) \
             ORDER BY ip",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
    )> = stmt
        .query_map(rusqlite::params![target], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>IP addresses</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No IPs resolved.</p>");
    } else {
        out.push_str(
            "<table><thead><tr><th>ip</th><th>reverse_ptr</th><th>asn</th><th>country</th><th>tags</th></tr></thead><tbody>",
        );
        for (ip, ptr, cloud, cdn, asn, asn_name, country) in rows {
            out.push_str("<tr>");
            let _ = write!(out, "<td><code>{}</code></td>", h(&ip));
            let _ = write!(
                out,
                "<td>{}</td>",
                ptr.as_deref()
                    .map(h)
                    .unwrap_or_else(|| "<span class=\"muted\">-</span>".into())
            );
            let asn_str = match (asn, asn_name.as_deref()) {
                (Some(n), Some(name)) => format!("AS{n} {name}"),
                (Some(n), None) => format!("AS{n}"),
                _ => "-".into(),
            };
            let _ = write!(out, "<td>{}</td>", h(&asn_str));
            let _ = write!(
                out,
                "<td>{}</td>",
                country.as_deref().map(h).unwrap_or_else(|| "-".into())
            );
            out.push_str("<td>");
            if let Some(c) = cloud.as_deref() {
                let _ = write!(out, "<span class=\"badge\">{}</span>", h(c));
            }
            if let Some(c) = cdn.as_deref() {
                let _ = write!(out, "<span class=\"badge badge-warn\">{}</span>", h(c));
            }
            out.push_str("</td></tr>");
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_ports(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT ip, port, state, service, service_version \
             FROM port_probes \
             WHERE ip IN (SELECT ip FROM domain_ips \
                 WHERE domain IN (SELECT domain FROM domains WHERE target_root = ?1)) \
               AND state = 'open' \
             ORDER BY ip, port",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, i64, String, Option<String>, Option<String>)> = stmt
        .query_map(rusqlite::params![target], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>Open ports</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No open ports observed.</p>");
    } else {
        // Group by IP to keep the dashboard scannable — one row per IP, the
        // port list lives in a compact inline span.
        use std::collections::BTreeMap;
        let mut by_ip: BTreeMap<String, Vec<PortDetail>> = BTreeMap::new();
        for (ip, port, _state, svc, ver) in rows {
            by_ip.entry(ip).or_default().push((port, svc, ver));
        }
        out.push_str("<table><thead><tr><th>ip</th><th>open ports</th></tr></thead><tbody>");
        for (ip, ports) in by_ip {
            let _ = write!(out, "<tr><td><code>{}</code></td><td>", h(&ip));
            for (port, svc, ver) in ports {
                let label = match (svc.as_deref(), ver.as_deref()) {
                    (Some(s), Some(v)) if !v.is_empty() => format!(":{port} {s}/{v}"),
                    (Some(s), _) => format!(":{port} {s}"),
                    _ => format!(":{port}"),
                };
                let _ = write!(out, "<span class=\"badge\">{}</span>", h(&label));
            }
            out.push_str("</td></tr>");
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_certs(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT c.subject_cn, c.issuer_cn, c.is_wildcard, c.is_self_signed, \
                 substr(c.sha256_fingerprint, 1, 16) \
             FROM certs c \
             JOIN cert_seen_on s ON s.cert_sha256 = c.sha256_fingerprint \
             WHERE s.domain IN (SELECT domain FROM domains WHERE target_root = ?1) \
             ORDER BY c.issuer_cn, c.subject_cn",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Option<String>, Option<String>, i64, i64, String)> = stmt
        .query_map(rusqlite::params![target], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>Certificates</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No certificates observed.</p>");
    } else {
        out.push_str(
            "<table><thead><tr><th>subject_cn</th><th>issuer_cn</th><th>flags</th><th>sha256</th></tr></thead><tbody>",
        );
        for (subject, issuer, wild, selfs, sha) in rows {
            let sub = subject.as_deref().unwrap_or("-");
            let iss = issuer.as_deref().unwrap_or("-");
            out.push_str("<tr>");
            let _ = write!(out, "<td>{}</td>", h(sub));
            let _ = write!(out, "<td>{}</td>", h(iss));
            out.push_str("<td>");
            if wild == 1 {
                out.push_str("<span class=\"badge\">wildcard</span>");
            }
            if selfs == 1 {
                out.push_str("<span class=\"badge badge-bad\">self-signed</span>");
            }
            if wild == 0 && selfs == 0 {
                out.push_str("<span class=\"muted\">-</span>");
            }
            out.push_str("</td>");
            let _ = write!(out, "<td><code>{}…</code></td>", h(&sha));
            out.push_str("</tr>");
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_external_domains(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    // Same scope fence `cmd_intel_show` uses: `from_page_url LIKE '%target%'`.
    // Left join pulls D1's `external_domains.categories_json` so each row can
    // render its category pills. Rows without a match (older DBs pre-D1) just
    // skip the pills.
    let like = format!("%{}%", target);
    let mut stmt = conn
        .prepare(
            "SELECT a.to_domain, COUNT(*) as refs, e.categories_json \
             FROM asset_refs a \
             LEFT JOIN external_domains e ON e.domain = a.to_domain \
             WHERE a.from_page_url LIKE ?1 AND a.is_internal = 0 \
             GROUP BY a.to_domain, e.categories_json \
             ORDER BY refs DESC",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows: Vec<(String, i64, Option<String>)> = stmt
        .query_map(rusqlite::params![like], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>External domains</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No external references.</p>");
    } else {
        out.push_str(
            "<table><thead><tr><th>domain</th><th>ref_count</th><th>categories</th></tr></thead><tbody>",
        );
        for (dom, n, cats_json) in rows {
            let _ = write!(
                out,
                "<tr><td><code>{}</code></td><td>{}</td><td>",
                h(&dom),
                n
            );
            if let Some(cats) = cats_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            {
                for cat in cats {
                    let _ = write!(
                        out,
                        "<span class=\"pill pill-{}\">{}</span>",
                        h(&cat),
                        h(&cat)
                    );
                }
            }
            out.push_str("</td></tr>");
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_asset_kind_rollup(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let like = format!("%{}%", target);
    let mut stmt = conn
        .prepare(
            "SELECT kind, is_internal, COUNT(*) FROM asset_refs \
             WHERE from_page_url LIKE ?1 \
             GROUP BY kind, is_internal ORDER BY kind",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows: Vec<(String, i64, i64)> = stmt
        .query_map(rusqlite::params![like], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>Asset refs by kind</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No asset references.</p>");
    } else {
        use std::collections::BTreeMap;
        let mut by_kind: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for (k, internal, n) in rows {
            let slot = by_kind.entry(k).or_insert((0, 0));
            if internal == 1 {
                slot.0 += n;
            } else {
                slot.1 += n;
            }
        }
        out.push_str(
            "<table><thead><tr><th>kind</th><th>internal</th><th>external</th></tr></thead><tbody>",
        );
        for (kind, (i, e)) in by_kind {
            let _ = write!(
                out,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                h(&kind),
                i,
                e
            );
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_subdomains(out: &mut String, conn: &Connection, target: &str) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT domain FROM domains \
             WHERE target_root = ?1 AND is_subdomain = 1 \
             ORDER BY domain",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows: Vec<String> = stmt
        .query_map(rusqlite::params![target], |r| r.get::<_, String>(0))
        .map_err(|e| Error::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    out.push_str("<section><h2>Subdomains</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No subdomains discovered.</p>");
    } else {
        let _ = write!(
            out,
            "<details><summary>{} subdomains (click to expand)</summary><div class=\"sub-list\">",
            rows.len()
        );
        for s in rows {
            let _ = write!(out, "<div>{}</div>", h(&s));
        }
        out.push_str("</div></details>");
    }
    out.push_str("</section>");
    Ok(())
}

fn write_footer(out: &mut String) {
    let _ = write!(
        out,
        "<footer>generated_at {}</footer></main></body></html>",
        h(&now_rfc3339())
    );
}

/// Minimal ISO-8601 timestamp using the `time` crate already in tree.
fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".into())
}

/// HTML-entity escape. Covers the five characters that can break out of
/// attribute or element context. Tiny by design — no regex, no crate.
pub(crate) fn h(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SqliteStorage;
    use rusqlite::Connection;

    /// Open a temp db path and run the full schema migration. Returns the
    /// path so callers can re-open with a fresh `Connection` for inserts.
    fn fresh_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("intel.db");
        // Opening SqliteStorage runs the full schema migration (domains,
        // dns_records, whois_records, asset_refs, external_domains, v_target_intel…).
        let storage = SqliteStorage::open(&path).expect("sqlite open");
        drop(storage); // release the writer thread before the readers connect
        (dir, path)
    }

    #[test]
    fn render_smoke_empty_db() {
        let (_dir, path) = fresh_db();
        let html = render("example.com", &path).expect("render");
        assert!(html.contains("<h1>Target:"), "missing h1: {html}");
        assert!(
            html.contains("(no rows)"),
            "missing summary fallback: {html}"
        );
        // Sanity: valid HTML5 document boundary markers.
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.ends_with("</html>"));
    }

    #[test]
    fn render_escapes_html_entities() {
        let (_dir, path) = fresh_db();
        let conn = Connection::open(&path).unwrap();
        // Register the target_root then an external-domain reference whose
        // domain contains a would-be script tag. The renderer must never
        // emit the raw `<script>` substring.
        conn.execute(
            "INSERT INTO domains(domain, target_root, is_subdomain) VALUES(?1, ?1, 0)",
            rusqlite::params!["evil.test"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO asset_refs(from_page_url, to_url, to_domain, kind, is_internal) \
             VALUES(?1, ?2, ?3, 'script', 0)",
            rusqlite::params![
                "https://evil.test/index",
                "https://<script>alert(1)</script>/",
                "<script>alert(1)</script>"
            ],
        )
        .unwrap();

        let html = render("evil.test", &path).expect("render");
        assert!(
            html.contains("&lt;script&gt;"),
            "expected entity-escaped tag in output"
        );
        // The only `<script>` that may appear is if we escaped incorrectly;
        // the template itself contains none.
        let lower = html.to_ascii_lowercase();
        assert!(
            !lower.contains("<script>"),
            "raw <script> escaped into output: {html}"
        );
    }

    #[test]
    fn render_includes_whois_section_when_present() {
        let (_dir, path) = fresh_db();
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO domains(domain, target_root, is_subdomain) VALUES(?1, ?1, 0)",
            rusqlite::params!["example.com"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO whois_records(domain, registrar, registrant_org, nameservers_json) \
             VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![
                "example.com",
                "Acme Registrar Inc",
                "Acme Holdings",
                "[\"ns1.acme.test\",\"ns2.acme.test\"]"
            ],
        )
        .unwrap();

        let html = render("example.com", &path).expect("render");
        assert!(
            html.contains("Acme Registrar Inc"),
            "registrar missing from output"
        );
        assert!(html.contains("Acme Holdings"), "registrant_org missing");
        assert!(html.contains("ns1.acme.test"), "nameservers missing");
    }

    #[test]
    fn render_includes_external_domains_with_categories() {
        let (_dir, path) = fresh_db();
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO domains(domain, target_root, is_subdomain) VALUES(?1, ?1, 0)",
            rusqlite::params!["mysite.test"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO asset_refs(from_page_url, to_url, to_domain, kind, is_internal) \
             VALUES(?1, ?2, ?3, 'script', 0)",
            rusqlite::params![
                "https://mysite.test/",
                "https://cdn.example.com/app.js",
                "cdn.example.com"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_domains(domain, ref_count, categories_json) \
             VALUES(?1, ?2, ?3)",
            rusqlite::params!["cdn.example.com", 1, "[\"cdn\"]"],
        )
        .unwrap();

        let html = render("mysite.test", &path).expect("render");
        assert!(
            html.contains("pill-cdn"),
            "expected pill-cdn class in output; got: {html}"
        );
        assert!(
            html.contains("cdn.example.com"),
            "external domain missing from output"
        );
    }
}
