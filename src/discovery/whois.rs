//! Registration data lookup via RDAP (RFC 7483) — a JSON replacement for
//! classic whois that runs over HTTPS, so it works with our impersonate
//! client. TLD servers are selected from a short table; anything we don't
//! know falls back to rdap.iana.org which redirects to the authoritative
//! bootstrap server.

use serde::{Deserialize, Serialize};
use url::Url;

use crate::impersonate::ImpersonateClient;
use crate::Result;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registration {
    pub tld_service: String,
    pub handle: Option<String>,
    pub status: Vec<String>,
    pub registrar: Option<String>,
    pub registrant_org: Option<String>,
    pub created: Option<String>,
    pub expires: Option<String>,
    pub last_changed: Option<String>,
    pub name_servers: Vec<String>,
    pub abuse_emails: Vec<String>,
}

/// Pick the best-known RDAP base URL for the TLD of `domain`. Bootstrap via
/// IANA handles anything we don't hard-code. We keep a tight built-in map so
/// the common case costs one request, not two.
fn rdap_base(domain: &str) -> String {
    let tld = domain.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match tld.as_str() {
        "br" => "https://rdap.registro.br/domain/".into(),
        "com" | "net" | "cc" | "tv" | "name" => "https://rdap.verisign.com/com/v1/domain/".into(),
        "org" => "https://rdap.publicinterestregistry.org/rdap/domain/".into(),
        "io" => "https://rdap.nic.io/domain/".into(),
        "us" => "https://rdap.nic.us/domain/".into(),
        "info" => "https://rdap.afilias.net/rdap/info/domain/".into(),
        _ => "https://rdap.iana.org/domain/".into(),
    }
}

pub async fn lookup(client: &ImpersonateClient, domain: &str) -> Result<Registration> {
    let url = Url::parse(&format!("{}{domain}", rdap_base(domain)))?;
    let resp = client.get(&url).await?;
    if !resp.status.is_success() {
        return Ok(Registration::default());
    }
    let body = String::from_utf8_lossy(&resp.body);
    let mut reg: Registration = Default::default();
    reg.tld_service = url.host_str().unwrap_or("").to_string();

    let Ok(v): std::result::Result<serde_json::Value, _> = serde_json::from_str(&body) else {
        return Ok(reg);
    };

    if let Some(h) = v.get("handle").and_then(|x| x.as_str()) {
        reg.handle = Some(h.to_string());
    }
    if let Some(arr) = v.get("status").and_then(|x| x.as_array()) {
        reg.status = arr
            .iter()
            .filter_map(|s| s.as_str().map(String::from))
            .collect();
    }
    if let Some(events) = v.get("events").and_then(|x| x.as_array()) {
        for ev in events {
            let action = ev.get("eventAction").and_then(|x| x.as_str()).unwrap_or("");
            let date = ev
                .get("eventDate")
                .and_then(|x| x.as_str())
                .map(String::from);
            match action {
                "registration" => reg.created = date,
                "expiration" => reg.expires = date,
                "last changed" | "last update of RDAP database" => reg.last_changed = date,
                _ => {}
            }
        }
    }
    if let Some(nss) = v.get("nameservers").and_then(|x| x.as_array()) {
        for ns in nss {
            if let Some(name) = ns.get("ldhName").and_then(|x| x.as_str()) {
                reg.name_servers.push(name.to_ascii_lowercase());
            }
        }
    }
    // RDAP entities tree: some registries (e.g. registro.br) nest the
    // registrar role inside a contact entity two levels deep instead
    // of at the top. Walk recursively so the same parser fills
    // registrar/registrant/abuse regardless of nesting.
    if let Some(entities) = v.get("entities").and_then(|x| x.as_array()) {
        walk_entities(entities, &mut reg);
    }
    Ok(reg)
}

fn walk_entities(entities: &[serde_json::Value], reg: &mut Registration) {
    for ent in entities {
        let roles: Vec<&str> = ent
            .get("roles")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        let vcard = ent.get("vcardArray").and_then(|x| x.as_array());
        if roles.contains(&"registrar") {
            reg.registrar = vcard_field(vcard, "fn").or_else(|| reg.registrar.take());
            // .br RDAP also exposes the handle as a pseudo-name when
            // vcard.fn is absent — use it so the field is never blank
            // when a registrar role entity was present.
            if reg.registrar.is_none() {
                reg.registrar = ent.get("handle").and_then(|x| x.as_str()).map(String::from);
            }
        }
        if roles.contains(&"registrant") {
            reg.registrant_org = vcard_field(vcard, "org")
                .or_else(|| vcard_field(vcard, "fn"))
                .or_else(|| reg.registrant_org.take());
        }
        if roles.contains(&"abuse") {
            if let Some(email) = vcard_field(vcard, "email") {
                if !reg.abuse_emails.contains(&email) {
                    reg.abuse_emails.push(email);
                }
            }
        }
        if let Some(nested) = ent.get("entities").and_then(|x| x.as_array()) {
            walk_entities(nested, reg);
        }
    }
}

/// jCard is an array like `["vcard",[["version",{},"text","4.0"], ["fn",{},"text","Acme"]]]`.
/// Pull the first entry matching `prop` and return its value cell.
fn vcard_field(vcard: Option<&Vec<serde_json::Value>>, prop: &str) -> Option<String> {
    let v = vcard?;
    let entries = v.get(1)?.as_array()?;
    for e in entries {
        let arr = e.as_array()?;
        if arr.first()?.as_str()? == prop {
            let val = arr.get(3)?;
            return match val {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Array(parts) => Some(
                    parts
                        .iter()
                        .filter_map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            };
        }
    }
    None
}
