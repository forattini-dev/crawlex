//! Tests for the ScriptSpec executor's planning phase: named selector
//! resolution, default timeout injection, error on undeclared aliases.
//!
//! Note: every JSON literal uses `r##"..."##` so the inner `#submit` etc.
//! don't terminate the raw string boundary.

use crawlex::script::{plan, PlanError, ResolvedStep, ScriptSpec};

fn parse(json: &str) -> ScriptSpec {
    ScriptSpec::from_json(json.as_bytes()).expect("spec parses")
}

#[test]
fn named_selectors_resolve_to_dsl_strings() {
    let s = parse(
        r##"{
        "version": 1,
        "selectors": { "btn": "role=button[name=\"Go\"]" },
        "steps": [
            { "click": { "locator": "@btn" } }
        ]
    }"##,
    );
    let p = plan(&s).unwrap();
    assert_eq!(p.steps.len(), 1);
    let ResolvedStep::Click { selector, .. } = &p.steps[0] else {
        panic!("expected Click")
    };
    assert_eq!(selector, "role=button[name=\"Go\"]");
}

#[test]
fn raw_selector_passes_through_unchanged() {
    let s = parse(
        r##"{
        "version": 1,
        "steps": [ { "click": { "locator": "#submit" } } ]
    }"##,
    );
    let p = plan(&s).unwrap();
    let ResolvedStep::Click { selector, .. } = &p.steps[0] else {
        panic!()
    };
    assert_eq!(selector, "#submit");
}

#[test]
fn unknown_named_selector_errs_on_plan() {
    let s = parse(
        r##"{
        "version": 1,
        "steps": [ { "click": { "locator": "@missing" } } ]
    }"##,
    );
    match plan(&s) {
        Err(PlanError::UnknownNamedSelector(name)) => assert_eq!(name, "missing"),
        other => panic!("expected UnknownNamedSelector, got ok={}", other.is_ok()),
    }
}

#[test]
fn default_timeout_propagates_when_step_omits_one() {
    let s = parse(
        r##"{
        "version": 1,
        "defaults": { "timeout_ms": 7777 },
        "steps": [
            { "click": { "locator": "#x" } },
            { "click": { "locator": "#y", "timeout_ms": 1234 } }
        ]
    }"##,
    );
    let p = plan(&s).unwrap();
    let ResolvedStep::Click { timeout_ms: a, .. } = &p.steps[0] else {
        panic!()
    };
    let ResolvedStep::Click { timeout_ms: b, .. } = &p.steps[1] else {
        panic!()
    };
    assert_eq!(*a, 7777);
    assert_eq!(*b, 1234);
}

#[test]
fn export_shorthand_string_resolves_named_alias() {
    let s = parse(
        r##"{
        "version": 1,
        "selectors": { "h1": "text=h1" },
        "exports": { "title": "@h1" }
    }"##,
    );
    let p = plan(&s).unwrap();
    assert_eq!(p.exports.len(), 1);
    let title = p.exports.get("title").unwrap();
    assert_eq!(title.selector, "text=h1");
}

#[test]
fn extract_step_resolves_inline_fields() {
    let s = parse(
        r##"{
        "version": 1,
        "selectors": { "h": "text=h1" },
        "steps": [
            { "extract": { "fields": { "title": "@h", "raw": "#main" } } }
        ]
    }"##,
    );
    let p = plan(&s).unwrap();
    let ResolvedStep::Extract(map) = &p.steps[0] else {
        panic!()
    };
    assert_eq!(map.get("title").unwrap().selector, "text=h1");
    assert_eq!(map.get("raw").unwrap().selector, "#main");
}

#[test]
fn captures_and_assertions_pass_through() {
    let s = parse(
        r##"{
        "version": 1,
        "captures": [
            { "screenshot": { "mode": "full_page" } },
            { "snapshot":   { "kind": "post_js_html" } }
        ],
        "assertions": [
            { "exists": { "locator": "#main" } }
        ]
    }"##,
    );
    let p = plan(&s).unwrap();
    assert_eq!(p.captures.len(), 2);
    assert_eq!(p.assertions.len(), 1);
}
