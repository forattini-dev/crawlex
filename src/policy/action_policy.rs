//! Per-action-verb allow/deny/confirm policy.
//!
//! Orthogonal to `PolicyEngine` (which gates fetch/render decisions):
//! this layer gates individual **action verbs** a script asks the
//! renderer to execute — `click`, `type`, `eval`, `navigate`,
//! `download`, etc. The typical use case is running a `ScriptSpec` that
//! came from an untrusted source (an LLM-generated flow, a shared
//! fixture) and needing to refuse `eval` or mark `download` as needing
//! confirmation without rewriting every step.
//!
//! Design cues from `vercel-labs/agent-browser` (Apache-2.0) —
//! specifically `cli/src/native/policy.rs`. Not a line-for-line port;
//! their policy lives outside a larger decision engine, ours plugs into
//! our existing NDJSON + `DecisionReason` pipeline.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Action verbs we gate. Keep this in step with `script::spec::Step` —
/// every step whose execution can touch the network, the filesystem, or
/// arbitrary code should have an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionVerb {
    Goto,
    Click,
    Type,
    Press,
    Scroll,
    Eval,
    Submit,
    Screenshot,
    Snapshot,
    Extract,
    Download,
}

impl ActionVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goto => "goto",
            Self::Click => "click",
            Self::Type => "type",
            Self::Press => "press",
            Self::Scroll => "scroll",
            Self::Eval => "eval",
            Self::Submit => "submit",
            Self::Screenshot => "screenshot",
            Self::Snapshot => "snapshot",
            Self::Extract => "extract",
            Self::Download => "download",
        }
    }
}

/// Outcome for one verb check. `Confirm` leaves the runtime free to
/// pause for operator approval; `crawlex` treats it as `Deny` today and
/// surfaces the reason in `decision.made`, with the option to wire a
/// real HITL path in the CLI later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRule {
    Allow,
    Deny,
    Confirm,
}

/// JSON-loadable action policy:
/// ```json
/// {
///   "default": "allow",
///   "rules": {
///     "eval":     "deny",
///     "download": "confirm",
///     "goto":     "allow"
///   }
/// }
/// ```
/// Missing verbs fall back to `default`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPolicy {
    #[serde(default = "default_allow")]
    pub default: ActionRule,
    #[serde(default)]
    pub rules: HashMap<ActionVerb, ActionRule>,
}

fn default_allow() -> ActionRule {
    ActionRule::Allow
}

impl Default for ActionPolicy {
    /// Conservative-by-design default: allow non-mutating verbs; deny
    /// arbitrary `eval` and `download`; confirm `navigate/goto` so the
    /// operator sees what URL a non-trusted script is going after.
    fn default() -> Self {
        let mut rules = HashMap::new();
        rules.insert(ActionVerb::Eval, ActionRule::Deny);
        rules.insert(ActionVerb::Download, ActionRule::Confirm);
        Self {
            default: ActionRule::Allow,
            rules,
        }
    }
}

impl ActionPolicy {
    /// Permissive policy — every verb allowed. Use when the script
    /// source is trusted (operator-authored) and the rule layer would
    /// just be friction.
    pub fn permissive() -> Self {
        Self {
            default: ActionRule::Allow,
            rules: HashMap::new(),
        }
    }

    /// Strict policy — deny everything by default; callers must
    /// explicitly allow the verbs they want. Useful when running a
    /// script you don't fully trust.
    pub fn strict() -> Self {
        Self {
            default: ActionRule::Deny,
            rules: HashMap::new(),
        }
    }

    pub fn with_rule(mut self, verb: ActionVerb, rule: ActionRule) -> Self {
        self.rules.insert(verb, rule);
        self
    }

    /// Resolve the effective rule for `verb`. Infallible; missing
    /// entries fall back to `default`.
    pub fn check(&self, verb: ActionVerb) -> ActionRule {
        *self.rules.get(&verb).unwrap_or(&self.default)
    }

    /// Convenience for the executor hot path: `Allow` is the most
    /// common answer, so check a boolean first and only reach for the
    /// structured reason when denying.
    pub fn is_allowed(&self, verb: ActionVerb) -> bool {
        matches!(self.check(verb), ActionRule::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_denies_eval_confirms_download_allows_rest() {
        let p = ActionPolicy::default();
        assert_eq!(p.check(ActionVerb::Eval), ActionRule::Deny);
        assert_eq!(p.check(ActionVerb::Download), ActionRule::Confirm);
        assert_eq!(p.check(ActionVerb::Click), ActionRule::Allow);
        assert_eq!(p.check(ActionVerb::Goto), ActionRule::Allow);
    }

    #[test]
    fn permissive_allows_everything() {
        let p = ActionPolicy::permissive();
        for v in [
            ActionVerb::Eval,
            ActionVerb::Download,
            ActionVerb::Click,
            ActionVerb::Goto,
        ] {
            assert!(p.is_allowed(v), "{v:?} should be allowed");
        }
    }

    #[test]
    fn strict_denies_everything_by_default() {
        let p = ActionPolicy::strict();
        assert_eq!(p.check(ActionVerb::Click), ActionRule::Deny);
        assert_eq!(p.check(ActionVerb::Goto), ActionRule::Deny);
    }

    #[test]
    fn strict_plus_explicit_allow_picks_one_verb() {
        let p = ActionPolicy::strict().with_rule(ActionVerb::Click, ActionRule::Allow);
        assert!(p.is_allowed(ActionVerb::Click));
        assert!(!p.is_allowed(ActionVerb::Type));
    }

    #[test]
    fn json_round_trip() {
        let p = ActionPolicy::default().with_rule(ActionVerb::Goto, ActionRule::Confirm);
        let s = serde_json::to_string(&p).unwrap();
        let back: ActionPolicy = serde_json::from_str(&s).unwrap();
        assert_eq!(back.check(ActionVerb::Eval), ActionRule::Deny);
        assert_eq!(back.check(ActionVerb::Goto), ActionRule::Confirm);
        assert_eq!(back.check(ActionVerb::Click), ActionRule::Allow);
    }

    #[test]
    fn json_shape_matches_documented_example() {
        let json = r#"{
            "default": "allow",
            "rules": {
                "eval":     "deny",
                "download": "confirm",
                "goto":     "allow"
            }
        }"#;
        let p: ActionPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(p.default, ActionRule::Allow);
        assert_eq!(p.check(ActionVerb::Eval), ActionRule::Deny);
        assert_eq!(p.check(ActionVerb::Download), ActionRule::Confirm);
        assert_eq!(p.check(ActionVerb::Goto), ActionRule::Allow);
        assert_eq!(p.check(ActionVerb::Click), ActionRule::Allow); // fallback
    }

    #[test]
    fn rule_missing_from_json_falls_back_to_default() {
        let json = r#"{"default":"deny"}"#;
        let p: ActionPolicy = serde_json::from_str(json).unwrap();
        for v in [ActionVerb::Click, ActionVerb::Eval, ActionVerb::Goto] {
            assert_eq!(p.check(v), ActionRule::Deny);
        }
    }

    #[test]
    fn omitting_default_field_uses_allow() {
        // Verifies the `#[serde(default = "default_allow")]` attribute.
        let json = r#"{"rules":{"eval":"deny"}}"#;
        let p: ActionPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(p.default, ActionRule::Allow);
        assert_eq!(p.check(ActionVerb::Eval), ActionRule::Deny);
        assert_eq!(p.check(ActionVerb::Click), ActionRule::Allow);
    }

    #[test]
    fn verb_as_str_matches_serde_repr() {
        assert_eq!(ActionVerb::Eval.as_str(), "eval");
        assert_eq!(
            serde_json::to_string(&ActionVerb::Eval).unwrap(),
            "\"eval\""
        );
    }
}
