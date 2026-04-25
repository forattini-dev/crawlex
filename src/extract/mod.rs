//! Content extraction and link filtering.
//!
//! This module hosts logic ported from Firecrawl's `apps/api/native/src/`
//! (MIT-licensed; see `/NOTICE`). We picked the parts that directly improve
//! crawlex's own weaker implementations:
//!
//! * `link_filter` — multi-signal URL allow/deny with domain, file-ext,
//!   subdomain, social-media and robots.txt gates.
//! * `sitemap` — XML sitemap + sitemapindex parser that decides between
//!   "recurse" (follow another XML) and "process" (queue URLs).
//!
//! The originals live at:
//!   references/firecrawl/apps/api/native/src/crawler.rs
//!
//! We strip NAPI/FFI wiring, use our crate's error and URL types, and extend
//! the filter API with a `Reason` enum so callers can surface denial reasons
//! in NDJSON events (phase 3) rather than treating denied links as silent.

pub mod html_clean;
pub mod link_filter;
pub mod sitemap;
