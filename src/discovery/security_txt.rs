//! Structured parser for /.well-known/security.txt (RFC 9116).
//!
//! Line format: `Field-Name: value`. Lines starting with `#` or blank are
//! ignored. Some fields are repeatable (Contact, Acknowledgments, Hiring,
//! Policy, Encryption). Values that look like URLs feed the crawler's
//! frontier; emails/phones are retained as metadata only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityTxt {
    pub contacts: Vec<String>,
    pub expires: Option<String>,
    pub preferred_languages: Vec<String>,
    pub canonical: Vec<String>,
    pub policy: Vec<String>,
    pub acknowledgments: Vec<String>,
    pub hiring: Vec<String>,
    pub encryption: Vec<String>,
    pub csaf: Vec<String>,
}

pub fn parse(body: &str) -> SecurityTxt {
    let mut out = SecurityTxt::default();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim().to_string();
        if val.is_empty() {
            continue;
        }
        match key.as_str() {
            "contact" => out.contacts.push(val),
            "expires" => out.expires = Some(val),
            "preferred-languages" => {
                for lang in val.split(',') {
                    let t = lang.trim();
                    if !t.is_empty() {
                        out.preferred_languages.push(t.to_string());
                    }
                }
            }
            "canonical" => out.canonical.push(val),
            "policy" => out.policy.push(val),
            "acknowledgments" => out.acknowledgments.push(val),
            "hiring" => out.hiring.push(val),
            "encryption" => out.encryption.push(val),
            "csaf" => out.csaf.push(val),
            _ => {}
        }
    }
    out
}

/// Fields whose values are URLs the crawler should probe.
pub fn url_fields(st: &SecurityTxt) -> Vec<&str> {
    let mut v = Vec::new();
    for s in &st.contacts {
        if s.starts_with("http://") || s.starts_with("https://") {
            v.push(s.as_str());
        }
    }
    for s in [
        &st.canonical,
        &st.policy,
        &st.acknowledgments,
        &st.hiring,
        &st.encryption,
        &st.csaf,
    ]
    .iter()
    {
        for u in *s {
            if u.starts_with("http://") || u.starts_with("https://") {
                v.push(u.as_str());
            }
        }
    }
    v
}
