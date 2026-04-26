//! `crawlex stealth catalog list [--filter <browser>] [--json]`
//!
//! Adapter port of the legacy `cmd_catalog_list` free fn in `cli/mod.rs`.
//! The logic is identical; this version exposes it through the
//! [`CliCommand`] trait so the dispatch layer can swap in a JSON
//! renderer or a human renderer based on `--json` without the command
//! itself caring about output format.

use async_trait::async_trait;

use crate::cli::command::{CliCommand, CliContext, CliOutput};
use crate::error::Result;
use crate::impersonate::catalog::{all, Browser};

pub struct CatalogListCommand {
    pub filter: Option<String>,
}

#[async_trait]
impl CliCommand for CatalogListCommand {
    fn name(&self) -> &'static str {
        "stealth.catalog.list"
    }

    async fn execute(&self, _ctx: &CliContext) -> Result<CliOutput> {
        let filter = self.filter.as_deref().map(str::to_ascii_lowercase);
        let want = |b: Browser| -> bool {
            match filter.as_deref() {
                None => true,
                Some("chrome") => b == Browser::Chrome,
                Some("chromium") => b == Browser::Chromium,
                Some("firefox") => b == Browser::Firefox,
                Some("edge") => b == Browser::Edge,
                Some("safari") => b == Browser::Safari,
                Some(_) => true,
            }
        };

        let entries: Vec<_> = all().filter(|fp| want(fp.browser)).collect();
        let headers = vec![
            "NAME".into(),
            "BROWSER".into(),
            "MAJOR".into(),
            "OS".into(),
            "CIPHERS".into(),
            "EXT".into(),
            "ECH".into(),
        ];
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|fp| {
                vec![
                    fp.name.into(),
                    fp.browser_name.into(),
                    fp.major.to_string(),
                    fp.os_name.into(),
                    fp.ciphers_no_grease().len().to_string(),
                    fp.extension_ids_no_grease().len().to_string(),
                    if fp.has_ech_grease { "yes" } else { "no" }.into(),
                ]
            })
            .collect();
        Ok(CliOutput::Table { headers, rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::render::HumanRenderer;

    fn ctx() -> CliContext {
        CliContext {
            renderer: Box::new(HumanRenderer),
        }
    }

    #[tokio::test]
    async fn lists_at_least_curl_impersonate_baseline() {
        let cmd = CatalogListCommand { filter: None };
        let out = cmd.execute(&ctx()).await.expect("command runs");
        match out {
            CliOutput::Table { rows, .. } => {
                assert!(rows.len() >= 21, "expected >=21 profiles, got {}", rows.len());
            }
            other => panic!("expected Table output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn filter_narrows_to_one_browser_family() {
        let cmd = CatalogListCommand {
            filter: Some("firefox".into()),
        };
        let out = cmd.execute(&ctx()).await.expect("command runs");
        match out {
            CliOutput::Table { rows, .. } => {
                assert!(rows.iter().all(|r| r[1] == "firefox"));
                assert!(!rows.is_empty());
            }
            other => panic!("expected Table output, got {other:?}"),
        }
    }
}
