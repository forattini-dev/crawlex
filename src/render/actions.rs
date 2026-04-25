//! Declarative interaction script for a rendered page.
//!
//! An `Action` sequence runs sequentially on a Chrome tab with human-like
//! timing baked in via the `interact` primitives. Load from JSON:
//!
//! ```json
//! [
//!   { "kind": "wait_for", "selector": "#email", "timeout_ms": 5000 },
//!   { "kind": "click",    "selector": "#email" },
//!   { "kind": "type",     "selector": "#email",    "text": "me@example.com" },
//!   { "kind": "type",     "selector": "#password", "text": "hunter2" },
//!   { "kind": "click",    "selector": "button[type=submit]" },
//!   { "kind": "wait_ms",  "ms": 2000 }
//! ]
//! ```

use crate::render::chrome::page::Page;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::policy::{ActionPolicy, ActionRule, ActionVerb};
use crate::render::interact::{
    click_selector, eval_js, scroll_by, type_text, wait_for_selector, MousePos,
};
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// CSS selector must resolve to a visible element before proceeding.
    WaitFor { selector: String, timeout_ms: u64 },
    /// Fixed delay — useful between navigation and first interaction.
    WaitMs { ms: u64 },
    /// Click a selector with bezier-curve mouse move and jitter.
    Click { selector: String },
    /// Focus and type text with gaussian keystroke timing.
    Type { selector: String, text: String },
    /// Synthesize wheel events to scroll the viewport.
    Scroll { dy: f64 },
    /// Evaluate arbitrary JS, discarding the return value.
    Eval { script: String },
    /// Submit a form by selector (clicks the matching submit button).
    Submit { selector: String },
    /// Press a single special key like "Enter" / "Tab" / "Escape".
    Press { key: String },
}

impl Action {
    /// Map each `Action` variant to the `ActionVerb` the ActionPolicy
    /// layer gates on. `WaitFor` / `WaitMs` / `Press` share their closest
    /// semantic verb (no behaviour changes gated by them individually).
    pub fn verb(&self) -> ActionVerb {
        match self {
            // Waits don't mutate anything — treat as "scroll" so they
            // inherit whatever rule the caller set for passive steps.
            Action::WaitFor { .. } | Action::WaitMs { .. } => ActionVerb::Scroll,
            Action::Click { .. } => ActionVerb::Click,
            Action::Type { .. } => ActionVerb::Type,
            Action::Scroll { .. } => ActionVerb::Scroll,
            Action::Eval { .. } => ActionVerb::Eval,
            Action::Submit { .. } => ActionVerb::Submit,
            Action::Press { .. } => ActionVerb::Press,
        }
    }
}

/// Execute `script` without an action policy — every verb is allowed.
/// Equivalent to `execute_with_policy(page, script, &ActionPolicy::permissive())`.
/// Kept as the default entry for callers that supply only operator-authored
/// scripts where gating is friction.
pub async fn execute(page: &Page, script: &[Action]) -> Result<()> {
    execute_with_policy(page, script, &ActionPolicy::permissive()).await
}

/// Execute `script` while consulting `policy` before each verb. `Deny`
/// aborts with `Error::HookAbort`; `Confirm` treats the verb as Deny
/// for now — when we wire a real HITL path in the CLI, `Confirm` will
/// suspend and await operator approval instead.
///
/// `Error::HookAbort` picks up the already-established "policy denied an
/// action" error bucket (Lua hooks use the same variant); the NDJSON
/// `job.failed` event surfaces `why=error:hook-abort` so operators can
/// grep and route these reliably.
pub async fn execute_with_policy(
    page: &Page,
    script: &[Action],
    policy: &ActionPolicy,
) -> Result<()> {
    let mut pos = MousePos { x: 100.0, y: 100.0 };
    for act in script {
        let verb = act.verb();
        match policy.check(verb) {
            ActionRule::Allow => {}
            ActionRule::Deny => {
                return Err(crate::Error::HookAbort(format!(
                    "action_policy: {verb:?} denied",
                    verb = verb.as_str()
                )));
            }
            ActionRule::Confirm => {
                // HITL wiring is operator-surface work (confirm flow over
                // stdin or a dashboard). Until that exists, `Confirm`
                // degrades to `Deny` so policy is never silently bypassed.
                return Err(crate::Error::HookAbort(format!(
                    "action_policy: {verb:?} requires confirmation (HITL unavailable in this build)",
                    verb = verb.as_str()
                )));
            }
        }
        match act {
            Action::WaitFor {
                selector,
                timeout_ms,
            } => {
                wait_for_selector(page, selector, *timeout_ms).await?;
            }
            Action::WaitMs { ms } => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
            Action::Click { selector } => {
                pos = click_selector(page, selector, pos).await?;
            }
            Action::Type { selector, text } => {
                type_text(page, selector, text).await?;
            }
            Action::Scroll { dy } => {
                scroll_by(page, *dy, pos).await?;
            }
            Action::Eval { script } => {
                eval_js(page, script).await?;
            }
            Action::Submit { selector } => {
                pos = click_selector(page, selector, pos).await?;
            }
            Action::Press { key } => {
                press_key(page, key).await?;
            }
        }
    }
    Ok(())
}

async fn press_key(page: &Page, key: &str) -> Result<()> {
    use crate::render::chrome_protocol::cdp::browser_protocol::input::{
        DispatchKeyEventParams, DispatchKeyEventType,
    };
    for ty in [DispatchKeyEventType::KeyDown, DispatchKeyEventType::KeyUp] {
        let p = DispatchKeyEventParams::builder()
            .r#type(ty)
            .key(key.to_string())
            .build()
            .map_err(|e| crate::Error::Render(format!("press params: {e}")))?;
        page.execute(p)
            .await
            .map_err(|e| crate::Error::Render(format!("press: {e}")))?;
    }
    Ok(())
}
