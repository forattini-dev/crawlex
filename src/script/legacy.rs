//! Adapters from legacy interaction inputs to `ScriptSpec`.
//!
//! The CLI still accepts `--actions-file` for backward compatibility, but
//! the render runtime now executes a single ScriptSpec path. This adapter
//! keeps the old JSON schema working while consolidating execution semantics,
//! step events, and artifact handling on the runner.

use crate::render::actions::Action;
use crate::script::spec::{ClickStep, Locator, ScriptSpec, Step, TypeStep, WaitForStep};

/// Convert a legacy `render::actions::Action` list into a ScriptSpec v1.
///
/// The adapter is intentionally mechanical: it preserves the original
/// sequence and semantics as closely as the richer ScriptSpec surface allows.
/// Legacy actions never carried top-level captures/assertions/exports, so the
/// resulting spec leaves those collections empty.
pub fn actions_to_script_spec(actions: &[Action]) -> ScriptSpec {
    let steps = actions
        .iter()
        .map(|action| match action {
            Action::WaitFor {
                selector,
                timeout_ms,
            } => Step::WaitFor(WaitForStep {
                locator: Locator::Raw(selector.clone()),
                state: None,
                timeout_ms: Some(*timeout_ms),
            }),
            Action::WaitMs { ms } => Step::WaitMs { ms: *ms },
            Action::Click { selector } => Step::Click(ClickStep {
                locator: Locator::Raw(selector.clone()),
                timeout_ms: None,
                force: false,
            }),
            Action::Type { selector, text } => Step::Type(TypeStep {
                locator: Locator::Raw(selector.clone()),
                text: text.clone(),
                timeout_ms: None,
                clear: false,
            }),
            Action::Scroll { dy } => Step::Scroll { dy: *dy },
            Action::Eval { script } => Step::Eval {
                script: script.clone(),
            },
            Action::Submit { selector } => Step::Submit {
                locator: Locator::Raw(selector.clone()),
            },
            Action::Press { key } => Step::Press { key: key.clone() },
        })
        .collect();

    ScriptSpec {
        version: crate::script::SCRIPT_SPEC_VERSION,
        defaults: Default::default(),
        selectors: Default::default(),
        steps,
        captures: Default::default(),
        assertions: Default::default(),
        exports: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapts_legacy_actions_into_ordered_script_steps() {
        let actions = vec![
            Action::WaitFor {
                selector: "#email".into(),
                timeout_ms: 5_000,
            },
            Action::Click {
                selector: "#email".into(),
            },
            Action::Type {
                selector: "#email".into(),
                text: "me@example.com".into(),
            },
            Action::Press {
                key: "Enter".into(),
            },
        ];
        let spec = actions_to_script_spec(&actions);
        assert_eq!(spec.version, crate::script::SCRIPT_SPEC_VERSION);
        assert_eq!(spec.steps.len(), 4);
        assert!(matches!(spec.steps[0], Step::WaitFor(_)));
        assert!(matches!(spec.steps[1], Step::Click(_)));
        assert!(matches!(spec.steps[2], Step::Type(_)));
        assert!(matches!(spec.steps[3], Step::Press { .. }));
    }

    #[test]
    fn adapter_does_not_inject_extra_captures_or_assertions() {
        let spec = actions_to_script_spec(&[Action::WaitMs { ms: 250 }]);
        assert!(spec.captures.is_empty());
        assert!(spec.assertions.is_empty());
        assert!(spec.exports.is_empty());
    }
}
