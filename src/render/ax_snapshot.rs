//! Accessibility-tree snapshot with stable `@eN` refs.
//!
//! Produces a token-cheap text view of the page structured for LLM
//! consumption: one interactive element per line with an opaque
//! sequential ref the LLM cites back when it wants to click/type/etc.
//! Our ScriptSpec `Locator` enum learns a `Ref("@e2")` variant that
//! resolves to `backendDOMNodeId` via this snapshot's `ref_map`.
//!
//! Design debt: the approach (AX tree walk, role partitioning,
//! `@eN` refs, compact text rendering) is inspired by
//! `vercel-labs/agent-browser::cli/src/native/snapshot.rs` (Apache-2.0).
//! Our code is original Rust — no line-for-line port. We intentionally
//! drop their `data-__ab-ci` DOM tagging trick because mutating the
//! live DOM during a stealth crawl is a detection beacon; we rely on
//! AX tree + cursor-style probe via `Runtime.evaluate` returning
//! backend node ids rather than writing attributes.

use crate::render::chrome::page::Page;
use crate::render::chrome_protocol::cdp::browser_protocol::accessibility::{
    AxNode, GetFullAxTreeParams,
};
use std::collections::{BTreeMap, HashMap};

use crate::{Error, Result};

/// Roles where the element is directly interactive — we always surface
/// these with a ref even in compact mode. Curated from WAI-ARIA and
/// Chromium's `ax_enums.idl`; kept intentionally conservative so a
/// snapshot stays small.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "checkbox",
    "combobox",
    "link",
    "listbox",
    "menu",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "scrollbar",
    "searchbox",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "textbox",
    "tree",
    "treeitem",
    "figure", // often clickable gallery items
];

/// Roles we keep in compact mode for context, even though they aren't
/// themselves interactive. Skipping them loses the "what is this
/// section?" framing the LLM relies on for correct clicks.
pub const CONTENT_ROLES: &[&str] = &[
    "article",
    "banner",
    "cell",
    "columnheader",
    "complementary",
    "contentinfo",
    "definition",
    "dialog",
    "document",
    "form",
    "grid",
    "group",
    "heading",
    "image",
    "list",
    "listitem",
    "main",
    "navigation",
    "paragraph",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "search",
    "separator",
    "status",
    "tabpanel",
    "table",
    "term",
    "text",
    "time",
    "toolbar",
    "tooltip",
];

/// Roles dropped in compact mode. Noise nodes inserted by the AX tree
/// for layout grouping — they have no semantic meaning to an LLM.
pub const STRUCTURAL_ROLES: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "LineBreak",
    "WebArea",
    "RootWebArea",
];

/// Options controlling which nodes enter the snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotOptions {
    /// Drop structural-only nodes; keep interactive + content roles.
    /// Good default for LLM consumption — reduces token cost ~5×.
    pub compact: bool,
    /// Keep ONLY interactive nodes. Stricter than `compact`; useful
    /// when the LLM already has the text content from another source.
    pub only_interactive: bool,
    /// Maximum tree depth from root. `None` = unlimited.
    pub max_depth: Option<usize>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            compact: true,
            only_interactive: false,
            max_depth: None,
        }
    }
}

/// A node in the snapshot tree. Refs are assigned only to *interactive*
/// nodes — content roles carry context but aren't addressable.
#[derive(Debug, Clone)]
pub struct AxRefNode {
    /// `@e1`, `@e2`, ... when interactive; `None` otherwise.
    pub ref_id: Option<String>,
    pub role: String,
    pub name: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub backend_node_id: Option<i64>,
    pub children: Vec<AxRefNode>,
}

/// Full snapshot result: tree + `@eN` → `backendDOMNodeId` lookup.
#[derive(Debug, Clone)]
pub struct AxSnapshot {
    pub root: AxRefNode,
    /// Resolves a `@eN` ref back to the `backendDOMNodeId` the CDP
    /// `DOM` and `Runtime` domains use for resolving nodes without any
    /// DOM mutation.
    pub ref_map: BTreeMap<String, i64>,
}

impl AxSnapshot {
    /// Return a human-readable indented tree:
    ///   - button "Submit" [@e1]
    ///     - text "Click to send"
    ///   - link "Home" [@e2]
    ///
    /// Empty names are rendered as `<nameless>` so the LLM doesn't
    /// confuse two anonymous buttons.
    pub fn render_tree(&self) -> String {
        let mut out = String::new();
        render_into(&self.root, 0, &mut out);
        out
    }
}

fn render_into(node: &AxRefNode, indent: usize, out: &mut String) {
    use std::fmt::Write as _;
    let pad = "  ".repeat(indent);
    let name = if node.name.is_empty() {
        "<nameless>"
    } else {
        node.name.as_str()
    };
    let _ = write!(out, "{pad}- {} \"{}\"", node.role, name);
    if let Some(r) = &node.ref_id {
        let _ = write!(out, " [{}]", r);
    }
    if let Some(v) = &node.value {
        let _ = write!(out, " value={v:?}");
    }
    out.push('\n');
    for c in &node.children {
        render_into(c, indent + 1, out);
    }
}

/// Fetch the full AX tree from the page and transform it into an
/// [`AxSnapshot`]. Does not mutate the DOM.
pub async fn capture_ax_snapshot(page: &Page, opts: &SnapshotOptions) -> Result<AxSnapshot> {
    let params = GetFullAxTreeParams::builder().build();
    let returns = page
        .execute(params)
        .await
        .map_err(|e| Error::Render(format!("getFullAXTree: {e}")))?;
    Ok(build_snapshot(&returns.nodes, opts))
}

/// Pure: turn the flat `Vec<AxNode>` CDP gave us into the tree + ref
/// map. Extracted so tests can feed synthetic node lists without a
/// browser.
pub fn build_snapshot(nodes: &[AxNode], opts: &SnapshotOptions) -> AxSnapshot {
    if nodes.is_empty() {
        return AxSnapshot {
            root: AxRefNode {
                ref_id: None,
                role: "RootWebArea".into(),
                name: String::new(),
                description: None,
                value: None,
                backend_node_id: None,
                children: Vec::new(),
            },
            ref_map: BTreeMap::new(),
        };
    }

    // Index nodes by id for O(1) child lookup; find the root (no parent).
    let by_id: HashMap<&str, &AxNode> = nodes
        .iter()
        .map(|n| (n.node_id.inner().as_str(), n))
        .collect();
    let root = nodes
        .iter()
        .find(|n| n.parent_id.is_none())
        .unwrap_or(&nodes[0]);

    let mut ref_counter: u64 = 0;
    let mut ref_map = BTreeMap::new();
    let converted = convert(root, &by_id, opts, 0, &mut ref_counter, &mut ref_map);
    AxSnapshot {
        root: converted.unwrap_or(AxRefNode {
            ref_id: None,
            role: "RootWebArea".into(),
            name: String::new(),
            description: None,
            value: None,
            backend_node_id: None,
            children: Vec::new(),
        }),
        ref_map,
    }
}

fn convert(
    node: &AxNode,
    by_id: &HashMap<&str, &AxNode>,
    opts: &SnapshotOptions,
    depth: usize,
    ref_counter: &mut u64,
    ref_map: &mut BTreeMap<String, i64>,
) -> Option<AxRefNode> {
    if node.ignored {
        return None;
    }
    let role = node
        .role
        .as_ref()
        .and_then(|v| v.value.as_ref()?.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let interactive = INTERACTIVE_ROLES.contains(&role.as_str());
    let is_content = CONTENT_ROLES.contains(&role.as_str());
    let is_structural = STRUCTURAL_ROLES.contains(&role.as_str()) || role.is_empty();

    let keep = if opts.only_interactive {
        interactive
    } else if opts.compact {
        interactive || is_content
    } else {
        // Non-compact: keep everything that isn't explicitly structural
        // (we still drop `generic`/`presentation` since those carry no
        // semantic value in any mode).
        !is_structural || interactive || is_content
    };

    let ref_id = if interactive {
        *ref_counter += 1;
        let id = format!("@e{}", *ref_counter);
        if let Some(be) = node.backend_dom_node_id.as_ref() {
            ref_map.insert(id.clone(), *be.inner());
        }
        Some(id)
    } else {
        None
    };

    let below_cap = opts.max_depth.is_none_or(|max| depth < max);
    let mut children = Vec::new();
    if below_cap {
        if let Some(child_ids) = &node.child_ids {
            for cid in child_ids {
                let Some(child) = by_id.get(cid.inner().as_str()) else {
                    continue;
                };
                if let Some(c) = convert(child, by_id, opts, depth + 1, ref_counter, ref_map) {
                    children.push(c);
                }
            }
        }
    }

    // Elide a kept-but-empty structural node by hoisting its children up.
    if !keep {
        // If we're being dropped but have kept children, splice them
        // into the parent's child list. Implemented by returning the
        // first child and reassigning siblings — cleaner: return the
        // first kept descendant OR, if multiple, a synthetic "group"
        // wrapper. For a v1, just drop and return children through the
        // recursion cheat: return `None` but require caller to merge.
        //
        // The simpler-but-noisier policy is to return `None`, losing
        // the subtree. We'd rather preserve info; so return a synthetic
        // node with role='group' that holds the kept descendants.
        if children.is_empty() {
            return None;
        }
        return Some(AxRefNode {
            ref_id: None,
            role: "group".into(),
            name: String::new(),
            description: None,
            value: None,
            backend_node_id: node.backend_dom_node_id.as_ref().map(|b| *b.inner()),
            children,
        });
    }

    Some(AxRefNode {
        ref_id,
        role,
        name: node
            .name
            .as_ref()
            .and_then(|v| v.value.as_ref()?.as_str().map(|s| s.to_string()))
            .unwrap_or_default(),
        description: node
            .description
            .as_ref()
            .and_then(|v| v.value.as_ref()?.as_str().map(|s| s.to_string())),
        value: node
            .value
            .as_ref()
            .and_then(|v| v.value.as_ref()?.as_str().map(|s| s.to_string())),
        backend_node_id: node.backend_dom_node_id.as_ref().map(|b| *b.inner()),
        children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::chrome_protocol::cdp::browser_protocol::accessibility::{
        AxNode, AxNodeId, AxProperty, AxPropertyName, AxValue, AxValueType,
    };
    use crate::render::chrome_protocol::cdp::browser_protocol::dom::BackendNodeId as DomBackendNodeId;

    fn ax_str_value(s: &str) -> AxValue {
        AxValue {
            r#type: AxValueType::String,
            value: Some(serde_json::Value::String(s.into())),
            related_nodes: None,
            sources: None,
        }
    }

    fn node(
        id: &str,
        parent: Option<&str>,
        role: &str,
        name: &str,
        children: Vec<&str>,
        backend: Option<i64>,
    ) -> AxNode {
        AxNode {
            node_id: AxNodeId::from(id.to_string()),
            ignored: false,
            ignored_reasons: None,
            role: if role.is_empty() {
                None
            } else {
                Some(ax_str_value(role))
            },
            chrome_role: None,
            name: if name.is_empty() {
                None
            } else {
                Some(ax_str_value(name))
            },
            description: None,
            value: None,
            properties: None,
            parent_id: parent.map(|p| AxNodeId::from(p.to_string())),
            child_ids: if children.is_empty() {
                None
            } else {
                Some(
                    children
                        .into_iter()
                        .map(|c| AxNodeId::from(c.to_string()))
                        .collect(),
                )
            },
            backend_dom_node_id: backend.map(DomBackendNodeId::new),
            frame_id: None,
        }
    }

    fn _use_property_type() {
        // Keep AxProperty/AxPropertyName in scope so unused-import doesn't
        // bite when tests grow. No runtime effect.
        let _ = |p: AxProperty| p.name == AxPropertyName::Focusable;
    }

    #[test]
    fn empty_nodes_yields_empty_snapshot() {
        let snap = build_snapshot(&[], &SnapshotOptions::default());
        assert_eq!(snap.root.children.len(), 0);
        assert!(snap.ref_map.is_empty());
    }

    #[test]
    fn interactive_nodes_get_sequential_refs() {
        let ns = vec![
            node("1", None, "RootWebArea", "Page", vec!["2", "3"], Some(1000)),
            node("2", Some("1"), "button", "Submit", vec![], Some(2000)),
            node("3", Some("1"), "link", "Home", vec![], Some(3000)),
        ];
        let snap = build_snapshot(&ns, &SnapshotOptions::default());
        // RootWebArea is structural-ish; compact mode should route it
        // through our "keep-because-has-kept-children" path (group).
        assert!(snap.root.children.iter().any(|c| c.role == "button"));
        assert_eq!(snap.ref_map.len(), 2);
        assert!(snap.ref_map.contains_key("@e1"));
        assert!(snap.ref_map.contains_key("@e2"));
        assert_eq!(snap.ref_map["@e1"], 2000);
        assert_eq!(snap.ref_map["@e2"], 3000);
    }

    #[test]
    fn only_interactive_drops_content_roles() {
        let ns = vec![
            node("1", None, "RootWebArea", "Page", vec!["2", "3"], None),
            node("2", Some("1"), "heading", "Title", vec![], None),
            node("3", Some("1"), "button", "Go", vec![], Some(999)),
        ];
        let opts = SnapshotOptions {
            compact: true,
            only_interactive: true,
            max_depth: None,
        };
        let snap = build_snapshot(&ns, &opts);
        let txt = snap.render_tree();
        assert!(
            !txt.contains("heading"),
            "only_interactive should drop heading; got {txt}"
        );
        assert!(txt.contains("button"));
        assert_eq!(snap.ref_map.len(), 1);
    }

    #[test]
    fn render_tree_indents_children_and_shows_refs() {
        let ns = vec![
            node("1", None, "RootWebArea", "Root", vec!["2"], None),
            node("2", Some("1"), "button", "OK", vec![], Some(42)),
        ];
        let snap = build_snapshot(&ns, &SnapshotOptions::default());
        let txt = snap.render_tree();
        assert!(txt.contains("button \"OK\" [@e1]"), "got: {txt}");
    }

    #[test]
    fn max_depth_truncates_tree() {
        let ns = vec![
            node("1", None, "RootWebArea", "", vec!["2"], None),
            node("2", Some("1"), "group", "", vec!["3"], None),
            node("3", Some("2"), "button", "Deep", vec![], Some(1)),
        ];
        let opts = SnapshotOptions {
            compact: true,
            only_interactive: false,
            max_depth: Some(1),
        };
        let snap = build_snapshot(&ns, &opts);
        // At depth 1 we stop descending — button at depth 2 is gone.
        assert!(
            !snap.render_tree().contains("Deep"),
            "max_depth=1 should cut off the grandchild"
        );
    }

    #[test]
    fn ignored_nodes_are_dropped() {
        let mut ns = vec![
            node("1", None, "RootWebArea", "", vec!["2", "3"], None),
            node("2", Some("1"), "button", "Hidden", vec![], None),
            node("3", Some("1"), "button", "Visible", vec![], None),
        ];
        ns[1].ignored = true;
        let snap = build_snapshot(&ns, &SnapshotOptions::default());
        let txt = snap.render_tree();
        assert!(!txt.contains("Hidden"));
        assert!(txt.contains("Visible"));
    }
}
