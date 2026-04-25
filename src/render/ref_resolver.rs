//! Resolve AX-snapshot `@eN` refs to real interactions via CDP.
//!
//! The [`AxSnapshot`](super::ax_snapshot::AxSnapshot) emits a stable
//! `@e1`..`@eN` handle per interactive node plus a
//! `ref_map: BTreeMap<String, i64>` that maps each ref to the underlying
//! `backendDOMNodeId`. That id is the CDP-native address of a DOM node
//! which survives reflow, shadow-root piercing, and doesn't require any
//! DOM mutation to probe.
//!
//! This module turns a `@eN` into bbox-driven click/type through the
//! existing human-like `interact` primitives:
//!
//! 1. `DOM.getBoxModel(backendNodeId=bnid)` → quad.
//! 2. For click: sample a jittered point inside the bbox, then reuse
//!    [`interact::mouse_move_to`] + press/release.
//! 3. For type: use `DOM.resolveNode(backendNodeId=bnid)` →
//!    `Runtime.callFunctionOn(this.focus())` to focus without DOM
//!    mutation, then funnel through [`interact::type_text`]'s key
//!    dispatch loop (re-implemented here since `type_text` wants a
//!    selector, not an already-focused element).
//!
//! Keeping this layer small and separate from `interact` means the
//! happy-path selector flow stays untouched: no extra CDP round-trips
//! for the 90 % of users who never emit an AX snapshot.

#![cfg(feature = "cdp-backend")]

use std::collections::BTreeMap;

use crate::render::chrome::page::Page;
use crate::render::chrome_protocol::cdp::browser_protocol::dom::{
    BackendNodeId, GetBoxModelParams, ResolveNodeParams,
};
use crate::render::chrome_protocol::cdp::js_protocol::runtime::{
    CallFunctionOnParams, RemoteObjectId,
};
use rand::rngs::SmallRng;
use rand::RngExt;

use crate::render::interact::{click_point, dispatch_typing, MousePos, Rect};
use crate::{Error, Result};

/// Look up the `backendDOMNodeId` for an `@eN` ref in a captured snapshot's
/// map. Returns `None` when the ref was never assigned (e.g. stale snapshot,
/// typo, or the node wasn't interactive when the snapshot was taken).
pub fn lookup_backend_node_id(
    ref_id: &str,
    ref_map: &BTreeMap<String, i64>,
) -> Option<BackendNodeId> {
    ref_map.get(ref_id).copied().map(BackendNodeId::new)
}

/// Fetch the bounding rect of a node addressed by `backendDOMNodeId`.
/// Returns `None` when the node has no box (detached, `display:none`, 0×0).
pub async fn backend_node_rect(page: &Page, bnid: BackendNodeId) -> Result<Option<Rect>> {
    let params = GetBoxModelParams::builder().backend_node_id(bnid).build();
    let model = match page.execute(params).await {
        Ok(r) => r.result.model.clone(),
        // GetBoxModel fails on zero-box nodes — surface as `None`, not error.
        Err(_) => return Ok(None),
    };
    let q = model.content.inner();
    if q.len() < 8 {
        return Ok(None);
    }
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    let x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
    let x_max = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y_max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let w = (x_max - x).max(1.0);
    let h = (y_max - y).max(1.0);
    Ok(Some(Rect { x, y, w, h }))
}

/// Click the element addressed by `backendDOMNodeId`. Walks the same
/// bezier-curve mouse path as selector clicks so bot scores stay low.
pub async fn click_by_backend_node(
    page: &Page,
    bnid: BackendNodeId,
    from: MousePos,
) -> Result<MousePos> {
    let rect = backend_node_rect(page, bnid)
        .await?
        .ok_or_else(|| Error::Render(format!("no box for backend_node_id={}", bnid.inner())))?;
    let mut rng = rand::make_rng::<SmallRng>();
    let tx = rect.x + rect.w * rng.random_range(0.25..0.75);
    let ty = rect.y + rect.h * rng.random_range(0.25..0.75);
    let target_width = rect.w.min(rect.h).max(10.0);
    click_point(page, from, tx, ty, target_width).await
}

/// Resolve a backend node id to a transient JS object id for
/// `Runtime.callFunctionOn`. The `ResolveNode` call here is the only CDP
/// round-trip we need; no DOM mutation, no global scope pollution.
async fn resolve_to_object_id(page: &Page, bnid: BackendNodeId) -> Result<RemoteObjectId> {
    let params = ResolveNodeParams::builder().backend_node_id(bnid).build();
    let resp = page
        .execute(params)
        .await
        .map_err(|e| Error::Render(format!("DOM.resolveNode: {e}")))?;
    resp.result
        .object
        .object_id
        .clone()
        .ok_or_else(|| Error::Render("DOM.resolveNode returned no objectId".into()))
}

/// Focus a node by backend id via `Runtime.callFunctionOn(this.focus())`.
async fn focus_by_backend_node(page: &Page, bnid: BackendNodeId) -> Result<()> {
    let object_id = resolve_to_object_id(page, bnid).await?;
    let params = CallFunctionOnParams::builder()
        .object_id(object_id)
        .function_declaration("function() { this.focus(); }")
        .await_promise(true)
        .build()
        .map_err(|e| Error::Render(format!("callFunctionOn build: {e}")))?;
    page.execute(params)
        .await
        .map_err(|e| Error::Render(format!("callFunctionOn focus: {e}")))?;
    Ok(())
}

/// Type text into the node addressed by `backendDOMNodeId`. Focus via
/// `this.focus()`, then dispatch keys with the same cadence distribution
/// as the selector-based typing path.
pub async fn type_by_backend_node(page: &Page, bnid: BackendNodeId, text: &str) -> Result<()> {
    focus_by_backend_node(page, bnid).await?;
    dispatch_typing(page, text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::spec::Locator;

    #[test]
    fn lookup_returns_none_for_missing_ref() {
        let mut m = BTreeMap::new();
        m.insert("@e1".into(), 42i64);
        assert!(lookup_backend_node_id("@e99", &m).is_none());
    }

    #[test]
    fn lookup_returns_backend_id_for_known_ref() {
        let mut m = BTreeMap::new();
        m.insert("@e1".into(), 42i64);
        m.insert("@e2".into(), 1337i64);
        let got = lookup_backend_node_id("@e2", &m).expect("should find");
        assert_eq!(got.inner(), &1337);
    }

    #[test]
    fn locator_ax_ref_round_trips_through_map() {
        // Simulate the flow: Locator → ax_ref() → map lookup → BackendNodeId.
        let mut m = BTreeMap::new();
        m.insert("@e3".into(), 7i64);
        let loc = Locator::Raw("@e3".into());
        let ref_id = loc.ax_ref().expect("should be an ax ref");
        let bnid = lookup_backend_node_id(ref_id, &m).expect("resolve");
        assert_eq!(bnid.inner(), &7);
    }

    #[test]
    fn non_ax_selectors_return_none() {
        assert!(Locator::Raw("#login".into()).ax_ref().is_none());
        assert!(Locator::Raw("role=button".into()).ax_ref().is_none());
        assert!(Locator::Raw("@named-selector".into()).ax_ref().is_none());
    }
}
