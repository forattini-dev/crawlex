pub mod context;
pub mod events;
#[cfg(feature = "lua-hooks")]
pub mod lua;

pub use context::HookContext;
pub use events::HookEvent;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookDecision {
    Continue,
    Skip,
    Retry,
    Abort,
}

pub type HookFn = Arc<
    dyn for<'a> Fn(&'a mut HookContext) -> futures::future::BoxFuture<'a, Result<HookDecision>>
        + Send
        + Sync,
>;

#[derive(Default)]
pub struct HookRegistry {
    inner: RwLock<HashMap<HookEvent, Vec<HookFn>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(&self, event: HookEvent, f: HookFn) {
        self.inner.write().entry(event).or_default().push(f);
    }

    pub async fn fire(&self, event: HookEvent, ctx: &mut HookContext) -> Result<HookDecision> {
        let hooks = self.inner.read().get(&event).cloned().unwrap_or_default();
        for h in hooks {
            match h(ctx).await? {
                HookDecision::Continue => continue,
                other => return Ok(other),
            }
        }
        Ok(HookDecision::Continue)
    }
}
