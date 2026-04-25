//! HTTP-layer helpers owned by the crawl-pattern scheduler (wave 1).
//!
//! This module is intentionally *narrow* in scope — it hosts logic that
//! extends standard HTTP semantics with fingerprint-relevant behaviour
//! without touching the impersonate client (which owns TLS/H2/headers
//! + the RFC 6265 jar). The sibling `crate::impersonate::cookies`
//! module remains the canonical cookie jar for outgoing requests; the
//! `cookies` module here adds Partitioned / CHIPS awareness that
//! operators can opt into when they need per-top-level isolation.

pub mod cookies;
