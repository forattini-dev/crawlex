//! Tests for ScriptSpec v1 AST parsing.

use crawlex::script::{Locator, ScriptSpec, SCRIPT_SPEC_VERSION};
use indexmap::IndexMap;

#[test]
fn minimal_spec_parses() {
    let json = r#"{
        "version": 1,
        "steps": [
            { "goto": { "url": "https://example.com/" } }
        ]
    }"#;
    let s = ScriptSpec::from_json(json.as_bytes()).unwrap();
    assert_eq!(s.version, SCRIPT_SPEC_VERSION);
    assert_eq!(s.steps.len(), 1);
}

#[test]
fn spec_with_selectors_steps_captures_exports_parses() {
    let json = r#"{
        "version": 1,
        "defaults": { "timeout_ms": 5000 },
        "selectors": {
            "email":  "role=textbox[name=\"Email\"]",
            "submit": "role=button[name=\"Sign in\"]"
        },
        "steps": [
            { "goto": { "url": "https://example.com/login" } },
            { "type": { "locator": "@email", "text": "a@b.c" } },
            { "click": { "locator": "@submit" } }
        ],
        "captures": [
            { "screenshot": { "mode": "full_page", "name": "dashboard" } },
            { "snapshot": { "kind": "post_js_html" } }
        ],
        "exports": {
            "title": "text=h1"
        }
    }"#;
    let s = ScriptSpec::from_json(json.as_bytes()).unwrap();
    assert_eq!(s.selectors.len(), 2);
    assert_eq!(s.steps.len(), 3);
    assert_eq!(s.captures.len(), 2);
    assert_eq!(s.exports.len(), 1);
}

#[test]
fn version_mismatch_rejected() {
    let json = r#"{ "version": 99 }"#;
    assert!(ScriptSpec::from_json(json.as_bytes()).is_err());
}

#[test]
fn locator_resolves_named_alias() {
    let mut map = IndexMap::new();
    map.insert(
        "login".to_string(),
        "role=button[name=\"Login\"]".to_string(),
    );
    let raw = Locator::Raw("@login".to_string());
    assert_eq!(raw.resolve(&map), "role=button[name=\"Login\"]");
    let literal = Locator::Raw("#submit".to_string());
    assert_eq!(literal.resolve(&map), "#submit");
}

#[test]
fn export_shorthand_parses_as_bare_locator() {
    let json = r#"{
        "version": 1,
        "exports": {
            "title": "text=h1"
        }
    }"#;
    let s = ScriptSpec::from_json(json.as_bytes()).unwrap();
    assert_eq!(s.exports.len(), 1);
    // The serde representation for `BareLocator` is a plain string; we
    // just assert it deserialized without error.
}
