//! Self introspection (FP-B) — what crawlex looks like outbound.
//!
//! Slice B10 of PRD forattini-dev/crawlex#25. Captures our own JA3,
//! JA4, and h2 SETTINGS fingerprint by computing hashes from the
//! ClientHello / SETTINGS bytes we send. The result feeds the
//! coherence cross-check in B13.

use serde::{Deserialize, Serialize};

pub mod ja3;
pub mod ja4;
pub mod h2_fp;

pub use h2_fp::compute_h2_settings_fingerprint;
pub use ja3::compute_ja3;
pub use ja4::compute_ja4;

/// A snapshot of our outbound identity.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SelfFingerprint {
    pub ja3: Option<String>,
    pub ja3_hash: Option<String>,
    pub ja4: Option<String>,
    pub h2_settings_fp: Option<String>,
    pub header_order: Vec<String>,
    pub sec_ch_ua: Option<String>,
    pub user_agent: Option<String>,

    /// Catalog comparison populated in B11.
    pub profile_expected: Option<ProfileExpected>,
    pub matches_profile: Option<bool>,
    pub drift_signals: Vec<String>,
}

/// Catalog entry — what we expect a given `Profile` to produce.
/// Populated in B11; placeholder here so the field exists.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProfileExpected {
    pub profile_name: String,
    pub ja3_hash: Option<String>,
    pub ja4: Option<String>,
    pub h2_settings_fp: Option<String>,
}
