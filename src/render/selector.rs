//! Best-in-class selector engine for the Chrome render path.
//!
//! Playwright-style DSL evaluated entirely in-page via a single
//! `document.evaluate` round-trip, piercing open shadow roots.
//!
//! ## Syntax
//!
//! A selector is a pipeline of **layers** separated by ` >> ` (scope). Each
//! layer is one of:
//!
//! | Engine | Example | Meaning |
//! |--------|---------|---------|
//! | `css=`         | `css=button.primary` | standard `querySelectorAll`, with shadow-DOM pierce |
//! | `text=`        | `text="Sign in"` or `text=/sign ?in/i` | element whose visible text contains/matches the literal/regex |
//! | `role=`        | `role=button[name="Login"]` | ARIA role, optional accessible-name filter |
//! | `label=`       | `label="Email"` | form control associated to `<label>` of the given text |
//! | `placeholder=` | `placeholder="Search"` | `[placeholder*=...]` |
//! | `alt=`         | `alt="Logo"` | `[alt*=...]` |
//! | `title=`       | `title="Close"` | `[title*=...]` |
//! | `testid=`      | `testid=submit-btn` | `[data-testid=...]`, fallback to `[data-test-id]`, `[data-qa]` |
//! | `xpath=`       | `xpath=//button[contains(.,'Send')]` | raw XPath |
//!
//! Default engine is `css` (no prefix required for plain CSS).
//!
//! ## Filters
//!
//! Any layer can append filters separated by `|`:
//! * `| visible`   — ignore elements with zero size or `display:none`
//! * `| enabled`   — ignore `[disabled]`
//! * `| first`     — keep only the first
//! * `| last`      — keep only the last
//! * `| nth=N`     — keep the 0-indexed Nth (N can be negative, like Python)
//!
//! ## Examples
//!
//! ```text
//! role=button[name="Accept all"]
//! form#login >> label="Email"
//! css=.card | visible | nth=0
//! text=/^Continue with/i | enabled
//! xpath=//*[@data-action='buy'] | first
//! ```

use crate::render::chrome::page::Page;
use serde::Deserialize;

use crate::render::interact::Rect;
use crate::{Error, Result};

/// Resolve a selector in the page. Returns the first element's bounding
/// rect when found, `None` when the selector yields zero elements. Uses a
/// single JS round-trip.
pub async fn resolve_rect(page: &Page, selector: &str) -> Result<Option<Rect>> {
    use crate::render::chrome_protocol::cdp::js_protocol::runtime::EvaluateParams;
    let js = build_resolver_js(selector, "rect");
    let params = EvaluateParams::builder()
        .expression(js)
        .return_by_value(true)
        .build()
        .map_err(|e| Error::Render(format!("sel params: {e}")))?;
    let res = page
        .evaluate_expression(params)
        .await
        .map_err(|e| Error::Render(format!("sel eval: {e}")))?;
    let Some(v) = res.value() else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    Ok(serde_json::from_value(v.clone()).ok())
}

/// Resolve and focus the first matching element. Returns true on success.
pub async fn focus(page: &Page, selector: &str) -> Result<bool> {
    use crate::render::chrome_protocol::cdp::js_protocol::runtime::EvaluateParams;
    let js = build_resolver_js(selector, "focus");
    let params = EvaluateParams::builder()
        .expression(js)
        .return_by_value(true)
        .build()
        .map_err(|e| Error::Render(format!("sel focus params: {e}")))?;
    let res = page
        .evaluate_expression(params)
        .await
        .map_err(|e| Error::Render(format!("sel focus eval: {e}")))?;
    Ok(res.value().and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Count matches.
pub async fn count(page: &Page, selector: &str) -> Result<usize> {
    use crate::render::chrome_protocol::cdp::js_protocol::runtime::EvaluateParams;
    let js = build_resolver_js(selector, "count");
    let params = EvaluateParams::builder()
        .expression(js)
        .return_by_value(true)
        .build()
        .map_err(|e| Error::Render(format!("sel count params: {e}")))?;
    let res = page
        .evaluate_expression(params)
        .await
        .map_err(|e| Error::Render(format!("sel count eval: {e}")))?;
    Ok(res.value().and_then(|v| v.as_u64()).unwrap_or(0) as usize)
}

/// A thin Rect deserializer that accepts the rect keys we return from JS.
#[derive(Deserialize)]
struct RectDto {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl From<RectDto> for Rect {
    fn from(r: RectDto) -> Rect {
        Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }
}

/// Build a self-contained JS IIFE that parses `selector`, resolves matches,
/// and returns the `mode`-specific payload.
///
/// The selector is embedded verbatim as a JSON-encoded string literal, so we
/// don't need escape gymnastics.
fn build_resolver_js(selector: &str, mode: &str) -> String {
    let sel_json = serde_json::to_string(selector).unwrap();
    let mode_json = serde_json::to_string(mode).unwrap();
    format!("({JS})({sel_json}, {mode_json})", JS = RESOLVER_JS)
}

/// Full resolver, self-contained.
const RESOLVER_JS: &str = r#"
((selectorStr, mode) => {
  // ---- parse pipeline ----
  const parts = selectorStr.split(/\s*>>\s*/).filter(Boolean);
  const layers = parts.map(parseLayer);

  function parseLayer(raw) {
    const [body, ...filterSegs] = raw.split(/\s*\|\s*/);
    let engine = 'css';
    let value = body;
    const eqIdx = body.indexOf('=');
    const firstSpace = body.search(/\s/);
    if (eqIdx !== -1 && (firstSpace === -1 || eqIdx < firstSpace)) {
      const maybe = body.slice(0, eqIdx).trim().toLowerCase();
      if (['css','text','role','label','placeholder','alt','title','testid','xpath']
            .includes(maybe)) {
        engine = maybe;
        value = body.slice(eqIdx + 1).trim();
      }
    }
    // Strip surrounding quotes from value when it's a literal string form.
    if ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    const filters = filterSegs.map(f => f.trim());
    return { engine, value, filters };
  }

  // ---- shadow-aware traversal ----
  function* deepAll(root) {
    const stack = [root];
    while (stack.length) {
      const n = stack.pop();
      if (!n) continue;
      yield n;
      if (n.shadowRoot) stack.push(...Array.from(n.shadowRoot.children));
      if (n.children) stack.push(...Array.from(n.children));
    }
  }

  function queryAllDeep(scope, css) {
    const out = new Set();
    // Regular query within main tree.
    try { scope.querySelectorAll(css).forEach(e => out.add(e)); } catch (_) {}
    // Pierce shadow roots.
    for (const el of deepAll(scope)) {
      if (el.shadowRoot) {
        try { el.shadowRoot.querySelectorAll(css).forEach(e => out.add(e)); }
        catch (_) {}
      }
    }
    return Array.from(out);
  }

  function visibleText(el) {
    // innerText ignores hidden nodes in the layout sense, textContent sees
    // everything. We prefer innerText when defined.
    return (el.innerText !== undefined ? el.innerText : el.textContent) || '';
  }

  function toRegexOrLiteral(v) {
    const m = v.match(/^\/(.*)\/([gimsuy]*)$/);
    if (m) { try { return new RegExp(m[1], m[2]); } catch (_) {} }
    return v;
  }

  function accessibleName(el) {
    // Rough ARIA-name computation: aria-label, labelledby, alt, title, text.
    const al = el.getAttribute && el.getAttribute('aria-label');
    if (al) return al.trim();
    const lb = el.getAttribute && el.getAttribute('aria-labelledby');
    if (lb) {
      const t = lb.split(/\s+/).map(id => {
        const n = document.getElementById(id);
        return n ? visibleText(n) : '';
      }).join(' ').trim();
      if (t) return t;
    }
    const alt = el.getAttribute && el.getAttribute('alt');
    if (alt) return alt.trim();
    const ttl = el.getAttribute && el.getAttribute('title');
    if (ttl) return ttl.trim();
    // Native <label for=id>: look up label whose `for` points at this id.
    const id = el.id;
    if (id) {
      try {
        const lab = document.querySelector(`label[for="${CSS.escape(id)}"]`);
        if (lab) {
          const t = visibleText(lab).trim();
          if (t) return t;
        }
      } catch (_) {}
    }
    // Wrapping <label><input></label>: walk ancestors.
    let anc = el.parentNode;
    while (anc && anc !== document) {
      if (anc.tagName && anc.tagName.toLowerCase() === 'label') {
        const t = visibleText(anc).trim();
        if (t) return t;
        break;
      }
      anc = anc.parentNode;
    }
    return visibleText(el).trim();
  }

  function matchRoleLayer(scope, value) {
    // role=button[name="Login"]
    const m = value.match(/^([a-z]+)(?:\s*\[\s*name\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\]]+))\s*\])?$/i);
    if (!m) return [];
    const role = m[1].toLowerCase();
    const wantName = m[2] !== undefined ? m[2]
                   : m[3] !== undefined ? m[3]
                   : m[4] !== undefined ? m[4].trim()
                   : null;
    const els = queryAllDeep(scope, '[role], button, a, input, select, textarea, [href]');
    const roleOf = el => {
      const r = el.getAttribute && el.getAttribute('role');
      if (r) return r.toLowerCase();
      const tag = (el.tagName || '').toLowerCase();
      if (tag === 'a' && el.hasAttribute && el.hasAttribute('href')) return 'link';
      if (tag === 'button') return 'button';
      if (tag === 'input') {
        const t = (el.getAttribute('type') || 'text').toLowerCase();
        if (['button','submit','reset'].includes(t)) return 'button';
        if (t === 'checkbox') return 'checkbox';
        if (t === 'radio') return 'radio';
        return 'textbox';
      }
      if (tag === 'select') return 'combobox';
      if (tag === 'textarea') return 'textbox';
      return '';
    };
    const matcher = wantName ? toRegexOrLiteral(wantName) : null;
    return els.filter(el => {
      if (roleOf(el) !== role) return false;
      if (!matcher) return true;
      const name = accessibleName(el);
      if (matcher instanceof RegExp) return matcher.test(name);
      return name.includes(matcher);
    });
  }

  function matchTextLayer(scope, value) {
    const matcher = toRegexOrLiteral(value);
    const els = queryAllDeep(scope, '*');
    const hits = els.filter(el => {
      const t = visibleText(el);
      if (matcher instanceof RegExp) return matcher.test(t);
      return t.includes(matcher);
    });
    // Prefer the deepest matching elements (most specific).
    return hits.filter(el => !hits.some(o => o !== el && el.contains(o)));
  }

  function matchLabelLayer(scope, value) {
    const matcher = toRegexOrLiteral(value);
    const labels = queryAllDeep(scope, 'label');
    const controls = [];
    for (const l of labels) {
      const t = visibleText(l).trim();
      const ok = matcher instanceof RegExp ? matcher.test(t) : t.includes(matcher);
      if (!ok) continue;
      // <label for=id>
      const forId = l.getAttribute('for');
      if (forId) {
        const c = document.getElementById(forId);
        if (c) controls.push(c);
        continue;
      }
      // wrapped control
      const child = l.querySelector('input,select,textarea,button');
      if (child) controls.push(child);
    }
    return controls;
  }

  function matchAttrLayer(scope, attr, value) {
    const matcher = toRegexOrLiteral(value);
    const els = queryAllDeep(scope, `[${attr}]`);
    return els.filter(el => {
      const v = el.getAttribute(attr) || '';
      if (matcher instanceof RegExp) return matcher.test(v);
      return v.includes(matcher);
    });
  }

  function matchTestIdLayer(scope, value) {
    const attrs = ['data-testid','data-test-id','data-qa','data-cy'];
    for (const a of attrs) {
      const found = queryAllDeep(scope, `[${a}="${CSS.escape(value)}"]`);
      if (found.length) return found;
    }
    return [];
  }

  function matchXPathLayer(scope, value) {
    const ctx = scope.ownerDocument ? scope : document;
    const res = ctx.evaluate(value, scope,
                             null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
    const out = [];
    for (let i = 0; i < res.snapshotLength; i++) out.push(res.snapshotItem(i));
    return out;
  }

  function runLayer(scopes, layer) {
    const matches = new Set();
    for (const sc of scopes) {
      let els = [];
      switch (layer.engine) {
        case 'css':         els = queryAllDeep(sc, layer.value); break;
        case 'text':        els = matchTextLayer(sc, layer.value); break;
        case 'role':        els = matchRoleLayer(sc, layer.value); break;
        case 'label':       els = matchLabelLayer(sc, layer.value); break;
        case 'placeholder': els = matchAttrLayer(sc, 'placeholder', layer.value); break;
        case 'alt':         els = matchAttrLayer(sc, 'alt',         layer.value); break;
        case 'title':       els = matchAttrLayer(sc, 'title',       layer.value); break;
        case 'testid':      els = matchTestIdLayer(sc, layer.value); break;
        case 'xpath':       els = matchXPathLayer(sc, layer.value); break;
      }
      els.forEach(e => matches.add(e));
    }
    let arr = Array.from(matches);
    // Apply filters.
    const isVisible = el => {
      if (!(el instanceof Element)) return true;
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) return false;
      const s = getComputedStyle(el);
      if (s.visibility === 'hidden' || s.display === 'none') return false;
      return true;
    };
    const isEnabled = el => !el.hasAttribute || !el.hasAttribute('disabled');
    for (const f of layer.filters) {
      if (f === 'visible') arr = arr.filter(isVisible);
      else if (f === 'enabled') arr = arr.filter(isEnabled);
      else if (f === 'first') arr = arr.slice(0, 1);
      else if (f === 'last')  arr = arr.slice(-1);
      else if (f.startsWith('nth=')) {
        const n = parseInt(f.slice(4), 10);
        const idx = n < 0 ? arr.length + n : n;
        arr = (idx >= 0 && idx < arr.length) ? [arr[idx]] : [];
      }
    }
    return arr;
  }

  let scopes = [document];
  for (const layer of layers) {
    scopes = runLayer(scopes, layer);
    if (!scopes.length) break;
  }

  if (mode === 'count') return scopes.length;
  if (!scopes.length) return null;
  const first = scopes[0];
  if (mode === 'rect') {
    if (!first.getBoundingClientRect) return null;
    const r = first.getBoundingClientRect();
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  }
  if (mode === 'focus') {
    try { first.focus(); return true; } catch (_) { return false; }
  }
  return null;
})
"#;
