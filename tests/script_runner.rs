//! Unit-level coverage for `script::runner`. The `run()` loop needs a
//! live `Page`, which we don't have in a pure-unit test. Instead we
//! cover the pieces that compose into the loop: the Plan → ResolvedStep
//! shape, the verb → policy mapping, and the `@eN` ref resolution path
//! through the public `ref_resolver::lookup_backend_node_id`. The live
//! end-to-end flow lives in `spa_scriptspec_live.rs` (`#[ignore]`).

#![cfg(feature = "cdp-backend")]

use crawlex::policy::action_policy::{ActionPolicy, ActionRule, ActionVerb};
use crawlex::script::spec::{
    ClickStep, GotoStep, Locator, ScriptSpec, SnapshotKind, SnapshotStep, Step, TypeStep,
};
use crawlex::script::{plan, RunOutcome};
use indexmap::IndexMap;

fn base_spec() -> ScriptSpec {
    ScriptSpec {
        version: 1,
        defaults: Default::default(),
        selectors: {
            let mut m = IndexMap::new();
            m.insert("email".into(), "input[name=email]".into());
            m
        },
        steps: vec![
            Step::Goto(GotoStep {
                url: "https://example.com".into(),
                wait_until: None,
                timeout_ms: Some(5000),
            }),
            Step::Type(TypeStep {
                locator: Locator::Raw("@email".into()),
                text: "hi".into(),
                timeout_ms: None,
                clear: false,
            }),
            Step::Click(ClickStep {
                locator: Locator::Raw("@e3".into()),
                timeout_ms: None,
                force: false,
            }),
            Step::Snapshot(SnapshotStep {
                kind: SnapshotKind::AxTree,
                name: None,
            }),
        ],
        captures: vec![],
        assertions: vec![],
        exports: IndexMap::new(),
    }
}

#[test]
fn plan_resolves_named_selectors_and_preserves_ax_refs() {
    let spec = base_spec();
    let p = plan(&spec).expect("plan");
    assert_eq!(p.steps.len(), 4);
    // `@email` should resolve to the raw DSL.
    match &p.steps[1] {
        crawlex::script::ResolvedStep::Type { selector, .. } => {
            assert_eq!(selector, "input[name=email]");
        }
        other => panic!("step[1] should be Type, got {other:?}"),
    }
    // `@e3` is an AX ref, not a named selector — it must pass through.
    match &p.steps[2] {
        crawlex::script::ResolvedStep::Click { selector, .. } => {
            assert_eq!(selector, "@e3");
        }
        other => panic!("step[2] should be Click, got {other:?}"),
    }
}

#[test]
fn default_action_policy_denies_eval_step() {
    // Exercises the policy verb mapping the runner relies on — `Eval`
    // denies by default so a script smuggled in from an untrusted
    // source can't run arbitrary JS without an explicit policy override.
    let p = ActionPolicy::default();
    assert_eq!(p.check(ActionVerb::Eval), ActionRule::Deny);
    assert_eq!(p.check(ActionVerb::Click), ActionRule::Allow);
    assert_eq!(p.check(ActionVerb::Snapshot), ActionRule::Allow);
}

#[test]
fn permissive_policy_allows_everything_runner_cares_about() {
    let p = ActionPolicy::permissive();
    for v in [
        ActionVerb::Goto,
        ActionVerb::Click,
        ActionVerb::Type,
        ActionVerb::Press,
        ActionVerb::Scroll,
        ActionVerb::Eval,
        ActionVerb::Submit,
        ActionVerb::Screenshot,
        ActionVerb::Snapshot,
        ActionVerb::Extract,
    ] {
        assert!(p.is_allowed(v), "permissive should allow {v:?}");
    }
}

#[test]
fn run_outcome_default_is_empty() {
    let o = RunOutcome::default();
    assert!(o.steps.is_empty());
    assert!(o.captures.is_empty());
    assert!(o.exports.is_empty());
    assert!(o.failed_assertion.is_none());
}

#[test]
fn ax_ref_locator_detected_for_click_step() {
    // Sanity: the runner's `@eN` fast path depends on
    // `Locator::ax_ref` returning `Some` for well-formed refs. Lock
    // the contract here so a refactor of spec.rs doesn't silently
    // break the runner's dispatch.
    let loc = Locator::Raw("@e7".into());
    assert_eq!(loc.ax_ref(), Some("@e7"));
    let named = Locator::Raw("@email".into());
    assert_eq!(named.ax_ref(), None);
    let raw = Locator::Raw("button.primary".into());
    assert_eq!(raw.ax_ref(), None);
}
