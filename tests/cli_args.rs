//! Tests for the new v0.2 CLI flags: `--emit`, `--policy`, `--config`,
//! `--explain`. We don't drive a full crawl — that's covered elsewhere —
//! we only assert the args parse with the expected defaults and accept
//! the canonical preset names.

use clap::Parser;
use crawlex::cli::args::{Cli, Command};

fn parse(argv: &[&str]) -> Cli {
    let mut full = vec!["crawlex"];
    full.extend_from_slice(argv);
    Cli::parse_from(full)
}

#[test]
fn defaults_are_emit_none_policy_balanced_no_explain() {
    let cli = parse(&["pages", "run", "--seed", "https://example.com/"]);
    let Command::Pages(crawlex::cli::args::PagesVerb::Run(c)) = cli.command else {
        panic!("not a pages run command")
    };
    assert_eq!(c.emit, "none");
    assert_eq!(c.policy, "balanced");
    assert!(!c.explain);
    assert!(c.config.is_none());
}

#[test]
fn emit_ndjson_accepted() {
    let cli = parse(&["pages", "run", "--seed", "x", "--emit", "ndjson"]);
    let Command::Pages(crawlex::cli::args::PagesVerb::Run(c)) = cli.command else {
        panic!()
    };
    assert_eq!(c.emit, "ndjson");
}

#[test]
fn all_four_policy_presets_accepted_by_clap() {
    for preset in ["fast", "balanced", "deep", "forensics"] {
        let cli = parse(&["pages", "run", "--seed", "x", "--policy", preset]);
        let Command::Pages(crawlex::cli::args::PagesVerb::Run(c)) = cli.command else {
            panic!()
        };
        assert_eq!(c.policy, preset);
    }
}

#[test]
fn explain_is_a_flag() {
    let cli = parse(&["pages", "run", "--seed", "x", "--explain"]);
    let Command::Pages(crawlex::cli::args::PagesVerb::Run(c)) = cli.command else {
        panic!()
    };
    assert!(c.explain);
}

#[test]
fn config_path_or_dash_accepted() {
    let cli = parse(&["pages", "run", "--config", "/tmp/foo.json"]);
    let Command::Pages(crawlex::cli::args::PagesVerb::Run(c)) = cli.command else {
        panic!()
    };
    assert_eq!(c.config.as_deref(), Some("/tmp/foo.json"));

    let cli = parse(&["pages", "run", "--config", "-"]);
    let Command::Pages(crawlex::cli::args::PagesVerb::Run(c)) = cli.command else {
        panic!()
    };
    assert_eq!(c.config.as_deref(), Some("-"));
}
