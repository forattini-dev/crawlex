//! DNS record enumeration for discovered hosts.
//!
//! Queries A, AAAA, CNAME, MX, TXT, NS, CAA via hickory-resolver (async).
//! Extracts actionable leaks:
//! * CNAME target → candidate subdomain to crawl (often reveals CDN, SaaS).
//! * MX host → related infrastructure domain.
//! * TXT SPF `include:` → allied sending infra.
//! * CAA `iodef:mailto:` → owner hint.
//! * NS host → candidate to probe for zone config.

use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::TokioResolver;
use std::net::IpAddr;

#[derive(Debug, Default, Clone)]
pub struct DnsFacts {
    pub a: Vec<IpAddr>,
    pub aaaa: Vec<IpAddr>,
    pub cname: Vec<String>,
    pub mx: Vec<String>,
    pub txt: Vec<String>,
    pub ns: Vec<String>,
    pub caa: Vec<String>,
    pub related_hosts: Vec<String>,
}

pub async fn lookup(host: &str) -> DnsFacts {
    let mut builder = match TokioResolver::builder_tokio() {
        Ok(b) => b,
        Err(_) => return DnsFacts::default(),
    };
    {
        let opts = builder.options_mut();
        opts.timeout = std::time::Duration::from_secs(3);
        opts.attempts = 1;
    }
    let resolver = match builder.build() {
        Ok(r) => r,
        Err(_) => return DnsFacts::default(),
    };

    let mut facts = DnsFacts::default();

    if let Ok(r) = resolver.ipv4_lookup(host).await {
        for rec in r.answers() {
            if let RData::A(a) = &rec.data {
                facts.a.push(IpAddr::V4(a.0));
            }
        }
    }
    if let Ok(r) = resolver.ipv6_lookup(host).await {
        for rec in r.answers() {
            if let RData::AAAA(a) = &rec.data {
                facts.aaaa.push(IpAddr::V6(a.0));
            }
        }
    }
    if let Ok(r) = resolver.lookup(host, RecordType::CNAME).await {
        for rec in r.answers() {
            if let RData::CNAME(cn) = &rec.data {
                let t = cn.to_ascii().trim_end_matches('.').to_string();
                facts.cname.push(t.clone());
                facts.related_hosts.push(t);
            }
        }
    }
    if let Ok(r) = resolver.mx_lookup(host).await {
        for rec in r.answers() {
            if let RData::MX(mx) = &rec.data {
                let h = mx.exchange.to_ascii().trim_end_matches('.').to_string();
                facts.mx.push(h.clone());
                facts.related_hosts.push(h);
            }
        }
    }
    if let Ok(r) = resolver.txt_lookup(host).await {
        for rec in r.answers() {
            if let RData::TXT(txt) = &rec.data {
                let s: String = txt
                    .txt_data
                    .iter()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .collect::<Vec<_>>()
                    .join("");
                for tok in s.split_whitespace() {
                    if let Some(rest) = tok.strip_prefix("include:") {
                        facts.related_hosts.push(rest.to_string());
                    } else if let Some(rest) = tok.strip_prefix("redirect=") {
                        facts.related_hosts.push(rest.to_string());
                    }
                }
                facts.txt.push(s);
            }
        }
    }
    if let Ok(r) = resolver.lookup(host, RecordType::NS).await {
        for rec in r.answers() {
            if let RData::NS(ns) = &rec.data {
                let t = ns.to_ascii().trim_end_matches('.').to_string();
                facts.ns.push(t.clone());
                facts.related_hosts.push(t);
            }
        }
    }
    if let Ok(r) = resolver.lookup(host, RecordType::CAA).await {
        for rec in r.answers() {
            if let RData::CAA(caa) = &rec.data {
                facts.caa.push(format!("{:?}", caa));
            }
        }
    }

    facts.related_hosts.sort();
    facts.related_hosts.dedup();
    facts
}
