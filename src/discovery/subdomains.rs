//! Certificate-transparency-based subdomain enumeration.
//!
//! Queries crt.sh for every SAN issued against `%.<domain>`; yields each
//! unique name found, stripping wildcard prefixes. Adapted from redblue's
//! `modules/recon/crtsh.rs` but trimmed to the subset we need.

use bytes::Bytes;
use std::collections::HashSet;
use url::Url;

use crate::impersonate::ImpersonateClient;
use crate::Result;

pub async fn crtsh_subdomains(client: &ImpersonateClient, domain: &str) -> Result<Vec<String>> {
    let url = Url::parse(&format!("https://crt.sh/?q=%25.{domain}&output=json"))?;
    let resp = client.get(&url).await?;
    if !resp.status.is_success() {
        return Ok(Vec::new());
    }
    Ok(parse_crtsh_json(&resp.body, domain))
}

pub fn parse_crtsh_json(body: &Bytes, domain: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(body);
    if text.trim().is_empty() || text.trim() == "[]" {
        return Vec::new();
    }
    let lower_domain = domain.to_ascii_lowercase();
    let mut out: HashSet<String> = HashSet::new();
    // Find all "name_value": "..." fields and split on \n or literal newline.
    let key = "\"name_value\"";
    let mut cursor = 0;
    while let Some(idx) = text[cursor..].find(key) {
        let start = cursor + idx + key.len();
        let after = &text[start..];
        // Skip whitespace + colon + opening quote.
        let mut in_str_start = None;
        for (i, b) in after.bytes().enumerate() {
            match b {
                b' ' | b'\t' | b':' => continue,
                b'"' => {
                    in_str_start = Some(i + 1);
                    break;
                }
                _ => break,
            }
        }
        let Some(val_start) = in_str_start else {
            cursor = start;
            continue;
        };
        // Find closing quote (respecting backslash escapes).
        let abs_start = start + val_start;
        let rest = text[abs_start..].as_bytes();
        let mut end = 0;
        let mut escape = false;
        while end < rest.len() {
            let c = rest[end];
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                break;
            }
            end += 1;
        }
        let raw = &text[abs_start..abs_start + end];
        for chunk in raw
            .split(|c| c == '\n' || c == ',')
            .flat_map(|s| s.split("\\n"))
        {
            let name = chunk.trim().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let clean = name.strip_prefix("*.").unwrap_or(&name);
            if clean == lower_domain || clean.ends_with(&format!(".{lower_domain}")) {
                if !clean.contains('*') && !clean.contains(' ') {
                    out.insert(clean.to_string());
                }
            }
        }
        cursor = abs_start + end;
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

/// Query Cert Spotter's public JSON API as a fallback when crt.sh is
/// returning 502 Bad Gateway (common under heavy load) or rate-limiting
/// our IP. Public tier without an API key allows a few queries per
/// minute per IP, which is plenty for a target-scoped recon run.
///
/// Response shape: array of certs with a `dns_names` string array. We
/// union them into the same lowercase, wildcard-stripped set the crt.sh
/// parser emits so callers can mix both sources without dedup logic.
pub async fn certspotter_subdomains(
    client: &ImpersonateClient,
    domain: &str,
) -> Result<Vec<String>> {
    let url = Url::parse(&format!(
        "https://api.certspotter.com/v1/issuances?domain={domain}&include_subdomains=true&expand=dns_names"
    ))?;
    let resp = client.get(&url).await?;
    if !resp.status.is_success() {
        return Ok(Vec::new());
    }
    Ok(parse_certspotter_json(&resp.body, domain))
}

pub fn parse_certspotter_json(body: &Bytes, domain: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(body);
    if text.trim().is_empty() || text.trim() == "[]" {
        return Vec::new();
    }
    let lower = domain.to_ascii_lowercase();
    let mut out: HashSet<String> = HashSet::new();
    // CertSpotter JSON is well-formed — parse fully rather than the
    // streaming string walk crt.sh uses, since the response is usually
    // < 1 MB and the schema is stable.
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    for entry in arr {
        let Some(names) = entry.get("dns_names").and_then(|x| x.as_array()) else {
            continue;
        };
        for n in names {
            let Some(name) = n.as_str() else { continue };
            let name = name.trim().to_ascii_lowercase();
            let clean = name.strip_prefix("*.").unwrap_or(&name);
            if clean == lower || clean.ends_with(&format!(".{lower}")) {
                if !clean.contains('*') && !clean.contains(' ') {
                    out.insert(clean.to_string());
                }
            }
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

/// HackerTarget `hostsearch` endpoint — public reverse-DNS and
/// subdomain aggregator. Free tier = a handful of queries per day per
/// IP, plenty for one-shot recon. Response is CSV: `hostname,ip`.
pub async fn hackertarget_subdomains(
    client: &ImpersonateClient,
    domain: &str,
) -> Result<Vec<String>> {
    let url = Url::parse(&format!(
        "https://api.hackertarget.com/hostsearch/?q={domain}"
    ))?;
    let resp = client.get(&url).await?;
    if !resp.status.is_success() {
        return Ok(Vec::new());
    }
    Ok(parse_hackertarget_csv(&resp.body, domain))
}

pub fn parse_hackertarget_csv(body: &Bytes, domain: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(body);
    // HackerTarget returns plain text like `error check your search ...`
    // when rate-limited or querying an unknown domain — skip those.
    if text.to_ascii_lowercase().contains("error") && !text.contains(',') {
        return Vec::new();
    }
    let lower = domain.to_ascii_lowercase();
    let mut out: HashSet<String> = HashSet::new();
    for line in text.lines() {
        let Some((name, _ip)) = line.split_once(',') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if name == lower || name.ends_with(&format!(".{lower}")) {
            out.insert(name);
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

/// Reduce `a.b.example.com` to its registrable pair `example.com` using a
/// naïve 2-label heuristic. Good enough for crt.sh query seeding.
pub fn registrable_domain(host: &str) -> Option<String> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    // Handle simple two-label TLDs (co.uk, com.br, etc.) with a short list.
    let two_label_tlds = [
        "co.uk", "com.br", "com.au", "com.ar", "co.jp", "co.kr", "com.mx", "com.pl", "com.tr",
        "co.in", "co.za",
    ];
    let last_two = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
    if two_label_tlds.contains(&last_two.as_str()) && parts.len() >= 3 {
        return Some(format!("{}.{}", parts[parts.len() - 3], last_two));
    }
    Some(last_two)
}
