//! External oracle — opt-in sanity check that the TLS handshake we
//! THINK we sent matches what an external service SEES us send.
//!
//! Slice B12 of PRD forattini-dev/crawlex#25. Default endpoint is
//! `tls.peet.ws` (free public TLS echo). Operator opts in via
//! `--audit-tls`. Surfaces proxy / middlebox alteration of our
//! ClientHello.
//!
//! This slice ships the struct shapes + classify function. The
//! actual HTTP fetch + JSON parse against a specific endpoint
//! lives behind the opt-in CLI flag (wired in CLI-surface slice).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleReport {
    pub endpoint: String,
    pub ja3_from_oracle: Option<String>,
    pub ja4_from_oracle: Option<String>,
    pub matches_live: Option<bool>,
    pub fetched_at: String,
    pub error: Option<String>,
}

impl OracleReport {
    pub fn matches(
        endpoint: impl Into<String>,
        ja3_oracle: Option<String>,
        ja4_oracle: Option<String>,
        ja3_live: Option<&str>,
        ja4_live: Option<&str>,
    ) -> Self {
        let matches = match (
            ja3_oracle.as_deref(),
            ja4_oracle.as_deref(),
            ja3_live,
            ja4_live,
        ) {
            (Some(oj3), _, Some(lj3), _) => Some(oj3 == lj3),
            (_, Some(oj4), _, Some(lj4)) => Some(oj4 == lj4),
            _ => None,
        };
        Self {
            endpoint: endpoint.into(),
            ja3_from_oracle: ja3_oracle,
            ja4_from_oracle: ja4_oracle,
            matches_live: matches,
            fetched_at: now_iso8601(),
            error: None,
        }
    }

    pub fn failed(endpoint: impl Into<String>, err: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            ja3_from_oracle: None,
            ja4_from_oracle: None,
            matches_live: None,
            fetched_at: now_iso8601(),
            error: Some(err.into()),
        }
    }
}

fn now_iso8601() -> String {
    use time::OffsetDateTime;
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

pub const DEFAULT_ORACLE_ENDPOINT: &str = "https://tls.peet.ws/api/all";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_when_oracle_and_live_agree() {
        let r = OracleReport::matches(
            "https://tls.peet.ws/api/all",
            Some("abc".into()),
            Some("t13d".into()),
            Some("abc"),
            Some("t13d"),
        );
        assert_eq!(r.matches_live, Some(true));
        assert!(r.error.is_none());
    }

    #[test]
    fn does_not_match_when_oracle_disagrees() {
        let r = OracleReport::matches(
            "https://tls.peet.ws/api/all",
            Some("abc".into()),
            None,
            Some("xyz"),
            None,
        );
        assert_eq!(r.matches_live, Some(false));
    }

    #[test]
    fn matches_none_when_no_comparison_possible() {
        let r = OracleReport::matches(
            "https://tls.peet.ws/api/all",
            None,
            None,
            None,
            None,
        );
        assert!(r.matches_live.is_none());
    }

    #[test]
    fn failed_carries_error() {
        let r = OracleReport::failed("https://tls.peet.ws/api/all", "connection refused");
        assert_eq!(r.matches_live, None);
        assert_eq!(r.error.as_deref(), Some("connection refused"));
    }
}
