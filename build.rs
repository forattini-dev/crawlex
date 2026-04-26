//! TLS fingerprint catalog code generator.
//!
//! Reads curl-impersonate's per-browser YAML signatures from
//! `src/impersonate/catalog/vendored/*.yaml` (MIT-licensed, vendored from
//! lwthiker/curl-impersonate — see that dir's README for provenance) and
//! emits Rust source at `$OUT_DIR/tls_catalog_generated.rs` with one
//! `pub static <NAME>: TlsFingerprint = ...` per profile plus a flat
//! `CATALOG` slice for runtime lookup.
//!
//! Also picks up any user-captured YAMLs from
//! `src/impersonate/catalog/captured/` (Phase 3 output) and mined hash JSONs
//! from `src/impersonate/catalog/mined/` (validation oracles, JA3/JA4 only).

// curl-impersonate's signature schema includes fields we don't currently
// project into the generated `TlsFingerprint` struct (e.g. http2 pseudo-
// header order, raw extension lengths). We deserialise them anyway so a
// future codegen pass can pick them up without re-parsing — the
// `dead_code` allowance below keeps the warning ledger clean meanwhile.
#![allow(dead_code)]

use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct ProfileDoc {
    name: String,
    browser: BrowserMeta,
    signature: Signature,
}

#[derive(Debug, Deserialize)]
struct BrowserMeta {
    name: String,
    version: String,
    os: String,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Signature {
    tls_client_hello: TlsClientHello,
    #[serde(default)]
    http2: Option<Http2>,
}

#[derive(Debug, Deserialize)]
struct TlsClientHello {
    #[serde(default)]
    record_version: Option<String>,
    #[serde(default)]
    handshake_version: Option<String>,
    #[serde(default)]
    session_id_length: Option<u32>,
    ciphersuites: Vec<serde_yaml::Value>,
    #[serde(default)]
    comp_methods: Vec<u32>,
    extensions: Vec<Extension>,
}

#[derive(Debug, Deserialize)]
struct Extension {
    #[serde(rename = "type")]
    kind: serde_yaml::Value,
    #[serde(default)]
    length: Option<u32>,
    #[serde(default)]
    supported_groups: Option<Vec<serde_yaml::Value>>,
    #[serde(default)]
    ec_point_formats: Option<Vec<u32>>,
    #[serde(default)]
    alpn_list: Option<Vec<String>>,
    #[serde(default)]
    sig_hash_algs: Option<Vec<u32>>,
    #[serde(default)]
    supported_versions: Option<Vec<serde_yaml::Value>>,
    #[serde(default)]
    algorithms: Option<Vec<u32>>,
    #[serde(default)]
    alps_alpn_list: Option<Vec<String>>,
    #[serde(default)]
    psk_ke_mode: Option<u32>,
    #[serde(default)]
    status_request_type: Option<u32>,
    #[serde(default)]
    key_shares: Option<Vec<KeyShare>>,
}

#[derive(Debug, Deserialize)]
struct KeyShare {
    group: serde_yaml::Value,
    #[serde(default)]
    length: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Http2 {
    #[serde(default)]
    pseudo_headers: Vec<String>,
    #[serde(default)]
    headers: Vec<String>,
}

/// Numeric value with explicit GREASE marker. GREASE positions are stable
/// per profile (curl-impersonate captures them), but the actual GREASE byte
/// values are randomised by BoringSSL at runtime.
fn parse_numeric(v: &serde_yaml::Value) -> Option<NumericEntry> {
    match v {
        serde_yaml::Value::String(s) if s == "GREASE" => Some(NumericEntry::Greased),
        serde_yaml::Value::String(s) if s.starts_with("TLS_VERSION_") => {
            let n = match s.as_str() {
                "TLS_VERSION_1_0" => 0x0301,
                "TLS_VERSION_1_1" => 0x0302,
                "TLS_VERSION_1_2" => 0x0303,
                "TLS_VERSION_1_3" => 0x0304,
                _ => return None,
            };
            Some(NumericEntry::Value(n))
        }
        serde_yaml::Value::Number(n) => n.as_u64().map(|u| NumericEntry::Value(u as u16)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum NumericEntry {
    Greased,
    Value(u16),
}

fn render_numeric_entry(e: &NumericEntry) -> String {
    match e {
        NumericEntry::Greased => "NumericEntry::Greased".into(),
        NumericEntry::Value(v) => format!("NumericEntry::Value(0x{:04x})", v),
    }
}

fn parse_ext_kind(v: &serde_yaml::Value) -> ExtKind {
    match v {
        serde_yaml::Value::String(s) if s == "GREASE" => ExtKind::Greased,
        serde_yaml::Value::String(s) => ExtKind::Named(s.clone()),
        serde_yaml::Value::Number(n) => ExtKind::Numeric(n.as_u64().unwrap_or(0) as u16),
        _ => ExtKind::Named("unknown".into()),
    }
}

#[derive(Debug, Clone)]
enum ExtKind {
    Greased,
    Named(String),
    Numeric(u16),
}

/// Map curl-impersonate extension type names to their IANA TLS extension IDs.
/// Source: https://www.iana.org/assignments/tls-extensiontype-values/
fn ext_id_from_name(name: &str) -> Option<u16> {
    Some(match name {
        "server_name" => 0,
        "max_fragment_length" => 1,
        "status_request" => 5,
        "supported_groups" => 10,
        "ec_point_formats" => 11,
        "signature_algorithms" => 13,
        "use_srtp" => 14,
        "heartbeat" => 15,
        "application_layer_protocol_negotiation" => 16,
        "signed_certificate_timestamp" => 18,
        "padding" => 21,
        "extended_master_secret" => 23,
        "compress_certificate" => 27,
        "session_ticket" => 35,
        "supported_versions" => 43,
        "psk_key_exchange_modes" => 45,
        "keyshare" | "key_share" => 51,
        "application_settings" => 17513,
        "renegotiation_info" => 65281,
        _ => return None,
    })
}

fn parse_profile_yaml(text: &str) -> Vec<ProfileDoc> {
    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        if let Ok(p) = ProfileDoc::deserialize(doc) {
            out.push(p);
        }
    }
    out
}

fn rust_ident_from_name(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_uppercase());
        } else {
            s.push('_');
        }
    }
    s
}

fn browser_kind(name: &str) -> &'static str {
    match name {
        "chrome" => "Browser::Chrome",
        "chromium" => "Browser::Chromium",
        "firefox" => "Browser::Firefox",
        "edge" => "Browser::Edge",
        "safari" => "Browser::Safari",
        "brave" => "Browser::Brave",
        "opera" => "Browser::Opera",
        _ => "Browser::Other",
    }
}

fn os_kind(os: &str) -> &'static str {
    let o = os.to_ascii_lowercase();
    if o.contains("win") {
        "BrowserOs::Windows"
    } else if o.contains("mac") || o.contains("ios") || o.contains("os") && o.contains("x") {
        "BrowserOs::MacOs"
    } else if o.contains("linux") {
        "BrowserOs::Linux"
    } else if o.contains("android") {
        "BrowserOs::Android"
    } else {
        "BrowserOs::Other"
    }
}

fn parse_major(version: &str) -> u16 {
    version
        .split('.')
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0)
}

fn render_profile(doc: &ProfileDoc, ident: &str) -> String {
    let ch = &doc.signature.tls_client_hello;

    // Ciphersuites (with GREASE markers preserved at original positions).
    let cipher_entries: Vec<NumericEntry> =
        ch.ciphersuites.iter().filter_map(parse_numeric).collect();
    let cipher_lit = cipher_entries
        .iter()
        .map(render_numeric_entry)
        .collect::<Vec<_>>()
        .join(", ");

    // Compression methods.
    let comp_lit = ch
        .comp_methods
        .iter()
        .map(|n| format!("0x{:02x}", n))
        .collect::<Vec<_>>()
        .join(", ");

    // Extensions, in YAML order. Each entry: ExtensionEntry { id, name }.
    // GREASE entries get id=0 + name="GREASE".
    let mut ext_lits = Vec::with_capacity(ch.extensions.len());
    let mut alpn_list: Vec<String> = Vec::new();
    let mut alps_list: Vec<String> = Vec::new();
    let mut sig_hash_algs: Vec<u16> = Vec::new();
    let mut supported_groups: Vec<NumericEntry> = Vec::new();
    let mut ec_point_formats: Vec<u8> = Vec::new();
    let mut supported_versions: Vec<NumericEntry> = Vec::new();
    let mut cert_compress_algs: Vec<u16> = Vec::new();
    let mut psk_ke_modes: Vec<u8> = Vec::new();
    let mut key_share_groups: Vec<NumericEntry> = Vec::new();
    let mut has_status_request = false;
    let mut has_extended_master_secret = false;
    let mut has_renegotiation_info = false;
    let mut has_session_ticket = false;
    let mut has_signed_certificate_timestamp = false;
    let mut has_padding = false;
    let mut has_ech_grease = false;

    for ext in &ch.extensions {
        let kind = parse_ext_kind(&ext.kind);
        match &kind {
            ExtKind::Greased => {
                ext_lits.push("ExtensionEntry::Greased".into());
            }
            ExtKind::Named(name) => {
                let id = ext_id_from_name(name).unwrap_or(0);
                ext_lits.push(format!(
                    "ExtensionEntry::Named {{ id: 0x{:04x}, name: {:?} }}",
                    id, name
                ));
                match name.as_str() {
                    "application_layer_protocol_negotiation" => {
                        if let Some(list) = &ext.alpn_list {
                            alpn_list = list.clone();
                        }
                    }
                    "application_settings" => {
                        if let Some(list) = &ext.alps_alpn_list {
                            alps_list = list.clone();
                        }
                    }
                    "signature_algorithms" => {
                        if let Some(algs) = &ext.sig_hash_algs {
                            sig_hash_algs = algs.iter().map(|n| *n as u16).collect();
                        }
                    }
                    "supported_groups" => {
                        if let Some(groups) = &ext.supported_groups {
                            supported_groups = groups.iter().filter_map(parse_numeric).collect();
                        }
                    }
                    "ec_point_formats" => {
                        if let Some(fmts) = &ext.ec_point_formats {
                            ec_point_formats = fmts.iter().map(|n| *n as u8).collect();
                        }
                    }
                    "supported_versions" => {
                        if let Some(vers) = &ext.supported_versions {
                            supported_versions = vers.iter().filter_map(parse_numeric).collect();
                        }
                    }
                    "compress_certificate" => {
                        if let Some(algs) = &ext.algorithms {
                            cert_compress_algs = algs.iter().map(|n| *n as u16).collect();
                        }
                    }
                    "psk_key_exchange_modes" => {
                        if let Some(mode) = ext.psk_ke_mode {
                            psk_ke_modes.push(mode as u8);
                        }
                    }
                    "keyshare" | "key_share" => {
                        if let Some(shares) = &ext.key_shares {
                            for s in shares {
                                if let Some(e) = parse_numeric(&s.group) {
                                    key_share_groups.push(e);
                                }
                            }
                        }
                    }
                    "status_request" => has_status_request = true,
                    "extended_master_secret" => has_extended_master_secret = true,
                    "renegotiation_info" => has_renegotiation_info = true,
                    "session_ticket" => has_session_ticket = true,
                    "signed_certificate_timestamp" => has_signed_certificate_timestamp = true,
                    "padding" => has_padding = true,
                    _ => {}
                }
            }
            ExtKind::Numeric(id) => {
                ext_lits.push(format!(
                    "ExtensionEntry::Named {{ id: 0x{:04x}, name: \"raw_{}\" }}",
                    id, id
                ));
                if *id == 65037 {
                    has_ech_grease = true;
                }
            }
        }
    }

    let alpn_lit = alpn_list
        .iter()
        .map(|s| format!("{:?}", s))
        .collect::<Vec<_>>()
        .join(", ");
    let alps_lit = alps_list
        .iter()
        .map(|s| format!("{:?}", s))
        .collect::<Vec<_>>()
        .join(", ");
    let sig_hash_lit = sig_hash_algs
        .iter()
        .map(|n| format!("0x{:04x}", n))
        .collect::<Vec<_>>()
        .join(", ");
    let supported_groups_lit = supported_groups
        .iter()
        .map(render_numeric_entry)
        .collect::<Vec<_>>()
        .join(", ");
    let ec_point_formats_lit = ec_point_formats
        .iter()
        .map(|n| format!("0x{:02x}", n))
        .collect::<Vec<_>>()
        .join(", ");
    let supported_versions_lit = supported_versions
        .iter()
        .map(render_numeric_entry)
        .collect::<Vec<_>>()
        .join(", ");
    let cert_compress_lit = cert_compress_algs
        .iter()
        .map(|n| format!("0x{:04x}", n))
        .collect::<Vec<_>>()
        .join(", ");
    let psk_ke_modes_lit = psk_ke_modes
        .iter()
        .map(|n| format!("0x{:02x}", n))
        .collect::<Vec<_>>()
        .join(", ");
    let key_share_lit = key_share_groups
        .iter()
        .map(render_numeric_entry)
        .collect::<Vec<_>>()
        .join(", ");
    let ext_lit = ext_lits.join(",\n        ");

    let major = parse_major(&doc.browser.version);

    format!(
        r#"
pub static {ident}: TlsFingerprint = TlsFingerprint {{
    name: {name:?},
    browser: {browser},
    browser_name: {browser_name:?},
    major: {major},
    version: {version:?},
    os: {os},
    os_name: {os_name:?},
    record_version: 0x{record_version:04x},
    handshake_version: 0x{handshake_version:04x},
    session_id_length: {session_id_length},
    ciphersuites: &[{cipher_lit}],
    comp_methods: &[{comp_lit}],
    extensions: &[
        {ext_lit}
    ],
    alpn: &[{alpn_lit}],
    alps_alpn: &[{alps_lit}],
    sig_hash_algs: &[{sig_hash_lit}],
    supported_groups: &[{supported_groups_lit}],
    ec_point_formats: &[{ec_point_formats_lit}],
    supported_versions: &[{supported_versions_lit}],
    cert_compress_algs: &[{cert_compress_lit}],
    psk_ke_modes: &[{psk_ke_modes_lit}],
    key_share_groups: &[{key_share_lit}],
    has_status_request: {has_status_request},
    has_extended_master_secret: {has_extended_master_secret},
    has_renegotiation_info: {has_renegotiation_info},
    has_session_ticket: {has_session_ticket},
    has_signed_certificate_timestamp: {has_signed_certificate_timestamp},
    has_padding: {has_padding},
    has_ech_grease: {has_ech_grease},
}};
"#,
        ident = ident,
        name = doc.name,
        browser = browser_kind(&doc.browser.name),
        browser_name = doc.browser.name,
        major = major,
        version = doc.browser.version,
        os = os_kind(&doc.browser.os),
        os_name = doc.browser.os,
        record_version = ch
            .record_version
            .as_deref()
            .and_then(|s| match s {
                "TLS_VERSION_1_0" => Some(0x0301u16),
                "TLS_VERSION_1_1" => Some(0x0302),
                "TLS_VERSION_1_2" => Some(0x0303),
                "TLS_VERSION_1_3" => Some(0x0304),
                _ => None,
            })
            .unwrap_or(0x0301),
        handshake_version = ch
            .handshake_version
            .as_deref()
            .and_then(|s| match s {
                "TLS_VERSION_1_0" => Some(0x0301u16),
                "TLS_VERSION_1_1" => Some(0x0302),
                "TLS_VERSION_1_2" => Some(0x0303),
                "TLS_VERSION_1_3" => Some(0x0304),
                _ => None,
            })
            .unwrap_or(0x0303),
        session_id_length = ch.session_id_length.unwrap_or(0),
    )
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let out_path = PathBuf::from(&out_dir).join("tls_catalog_generated.rs");

    let curl_dir = Path::new(&manifest_dir).join("src/impersonate/catalog/vendored");
    let captured_dir = Path::new(&manifest_dir).join("src/impersonate/catalog/captured");
    let mined_dir = Path::new(&manifest_dir).join("src/impersonate/catalog/mined");

    let mut all_yamls: Vec<PathBuf> = Vec::new();
    for d in [&curl_dir, &captured_dir] {
        if d.is_dir() {
            for entry in fs::read_dir(d).unwrap() {
                let p = entry.unwrap().path();
                if p.extension().and_then(|e| e.to_str()) == Some("yaml") {
                    all_yamls.push(p);
                }
            }
        }
    }

    // Tell cargo to rerun build.rs when any source YAML or oracle JSON changes.
    for p in &all_yamls {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    if mined_dir.is_dir() {
        for entry in fs::read_dir(&mined_dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
    }
    println!("cargo:rerun-if-changed=build.rs");

    // Collect all profile docs across all yamls. Dedupe by name (captured
    // wins over curl-impersonate vendored when the same name appears in both,
    // because captured/ is iterated last).
    let mut profiles: BTreeMap<String, ProfileDoc> = BTreeMap::new();
    for path in &all_yamls {
        let text = fs::read_to_string(path).expect("read yaml");
        for doc in parse_profile_yaml(&text) {
            profiles.insert(doc.name.clone(), doc);
        }
    }

    // Mined hash oracles — JA3/JA4 hashes scraped from public DBs. NOT
    // ClientHello bytes; only used to validate roundtrip output.
    let mut mined: Vec<(String, String, String, String)> = Vec::new(); // (name, ja3_hash, ja4, source)
    if mined_dir.is_dir() {
        #[derive(serde::Deserialize)]
        struct MinedOracle {
            name: String,
            #[serde(default)]
            ja3_hash: Option<String>,
            #[serde(default)]
            ja4: Option<String>,
            #[serde(default)]
            source: Option<String>,
        }
        for entry in fs::read_dir(&mined_dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = match fs::read_to_string(&p) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if let Ok(o) = serde_json::from_str::<MinedOracle>(&text) {
                mined.push((
                    o.name,
                    o.ja3_hash.unwrap_or_default(),
                    o.ja4.unwrap_or_default(),
                    o.source.unwrap_or_default(),
                ));
            }
        }
    }

    let mut src = String::new();
    src.push_str(
        "// AUTO-GENERATED by build.rs from curl-impersonate yamls + capture/mined data.\n",
    );
    src.push_str("// DO NOT EDIT — regenerate by touching any signature yaml or mined json.\n\n");

    let mut catalog_entries: Vec<(String, String)> = Vec::new();
    for (name, doc) in &profiles {
        let ident = rust_ident_from_name(name);
        src.push_str(&render_profile(doc, &ident));
        catalog_entries.push((name.clone(), ident));
    }

    src.push_str("\npub static CATALOG: &[(&str, &TlsFingerprint)] = &[\n");
    for (name, ident) in &catalog_entries {
        src.push_str(&format!("    ({:?}, &{}),\n", name, ident));
    }
    src.push_str("];\n");

    // Emit mined oracle table for validation tests.
    src.push_str("\n/// JA3/JA4 hashes scraped from public databases (tls.peet.ws, ja4db.com).\n");
    src.push_str("/// Used by `tests/tls_catalog_roundtrip.rs` to cross-check our generated\n");
    src.push_str("/// fingerprints against community observations. Format:\n");
    src.push_str("///   `(name, ja3_hash, ja4, source)`\n");
    src.push_str("pub static MINED_HASHES: &[(&str, &str, &str, &str)] = &[\n");
    for (name, ja3_hash, ja4, source) in &mined {
        src.push_str(&format!(
            "    ({:?}, {:?}, {:?}, {:?}),\n",
            name, ja3_hash, ja4, source
        ));
    }
    src.push_str("];\n");

    fs::write(&out_path, src).expect("write generated.rs");

    println!(
        "cargo:warning=tls_catalog: emitted {} profiles + {} mined oracles to {}",
        catalog_entries.len(),
        mined.len(),
        out_path.display()
    );
}
