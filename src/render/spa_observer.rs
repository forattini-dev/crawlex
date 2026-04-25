//! SPA/PWA runtime observer injected via
//! `Page.addScriptToEvaluateOnNewDocument`. Wires four lightweight
//! wrappers onto the page globals that, together, let us reconstruct
//! the route graph and the runtime-issued network endpoints a modern
//! single-page app walks through:
//!
//!   * `history.pushState` / `replaceState`
//!   * `popstate` and `hashchange` listeners
//!   * `window.fetch`
//!   * `XMLHttpRequest.prototype.open` + `.send`
//!
//! Every sample is appended to one of two globals — both bounded with
//! a hard ceiling so a misbehaving SPA can't OOM the renderer:
//!
//!   * `window.__crawlex_runtime_routes__` — `[{type, url, at}]`
//!   * `window.__crawlex_network_endpoints__` — `[{method, url, kind, started_at, status?, ok?, duration_ms?}]`
//!
//! Wrappers MUST preserve semantics — `fetch` returns the original
//! Promise, XHR `open`/`send` stay identity-forwarding, errors
//! propagate. The prefix `__crawlex_*` is already in use elsewhere in
//! the stealth shim; the shim also neutralises generic automation
//! globals, so this is consistent with that policy.
//!
//! Injected AFTER the stealth shim so wrappers bind to the already
//! patched `fetch` / `XMLHttpRequest` prototypes — if we ever swap
//! ordering, verify this still holds.

/// Hard cap on each array to bound memory; when overflow hits, the
/// wrappers keep working but drop new entries. 2000 is plenty for a
/// typical long render and a handful of KiB on the JS side.
pub const OBSERVER_SAMPLE_CAP: usize = 2000;

/// Returns the JS source injected via
/// `Page.addScriptToEvaluateOnNewDocument`. Pure function so we can
/// unit-test its shape without spinning up Chrome.
pub fn observer_js() -> String {
    format!(
        r#"(() => {{
  try {{
    if (window.__crawlex_observer_installed__) return;
    Object.defineProperty(window, '__crawlex_observer_installed__', {{
      value: true, writable: false, configurable: false, enumerable: false
    }});
    const CAP = {cap};
    window.__crawlex_runtime_routes__ = [];
    window.__crawlex_network_endpoints__ = [];
    window.__crawlex_idb_audit__ = [];
    const routes = window.__crawlex_runtime_routes__;
    const endpoints = window.__crawlex_network_endpoints__;
    const idbAudit = window.__crawlex_idb_audit__;
    const pushIdb = (rec) => {{
      try {{ if (idbAudit.length < CAP) idbAudit.push(rec); }} catch (_) {{}}
    }};
    const pushRoute = (type, url) => {{
      try {{
        if (routes.length >= CAP) return;
        const abs = (() => {{
          try {{ return new URL(url, document.baseURI).href; }}
          catch (_) {{ return String(url); }}
        }})();
        routes.push({{ type: String(type), url: abs, at: Date.now() }});
      }} catch (_) {{}}
    }};
    const pushEndpoint = (rec) => {{
      try {{
        if (endpoints.length >= CAP) return;
        endpoints.push(rec);
      }} catch (_) {{}}
    }};

    // --- History API -------------------------------------------------
    try {{
      const origPush = history.pushState;
      const origReplace = history.replaceState;
      history.pushState = function(state, title, url) {{
        const ret = origPush.apply(this, arguments);
        if (url !== undefined && url !== null) pushRoute('pushState', url);
        return ret;
      }};
      history.replaceState = function(state, title, url) {{
        const ret = origReplace.apply(this, arguments);
        if (url !== undefined && url !== null) pushRoute('replaceState', url);
        return ret;
      }};
    }} catch (_) {{}}

    // --- popstate / hashchange --------------------------------------
    try {{
      window.addEventListener('popstate', () => {{
        pushRoute('popstate', location.href);
      }}, true);
      window.addEventListener('hashchange', () => {{
        pushRoute('hashchange', location.href);
      }}, true);
    }} catch (_) {{}}

    // --- fetch wrapper ----------------------------------------------
    try {{
      const origFetch = window.fetch;
      if (typeof origFetch === 'function') {{
        window.fetch = function(input, init) {{
          let url = '';
          let method = 'GET';
          try {{
            if (typeof input === 'string') url = input;
            else if (input && typeof input.url === 'string') {{ url = input.url; method = input.method || method; }}
            else url = String(input);
            if (init && typeof init.method === 'string') method = init.method;
          }} catch (_) {{}}
          const started = Date.now();
          const rec = {{ kind: 'fetch', method: String(method).toUpperCase(), url, started_at: started }};
          pushEndpoint(rec);
          let p;
          try {{ p = origFetch.apply(this, arguments); }}
          catch (e) {{ rec.error = String(e && e.message || e); throw e; }}
          return p.then((resp) => {{
            try {{
              rec.status = resp && resp.status;
              rec.ok = resp && resp.ok;
              rec.duration_ms = Date.now() - started;
            }} catch (_) {{}}
            return resp;
          }}, (err) => {{
            try {{
              rec.error = String(err && err.message || err);
              rec.duration_ms = Date.now() - started;
            }} catch (_) {{}}
            throw err;
          }});
        }};
      }}
    }} catch (_) {{}}

    // --- XHR wrapper -------------------------------------------------
    try {{
      const XHRProto = XMLHttpRequest && XMLHttpRequest.prototype;
      if (XHRProto) {{
        const origOpen = XHRProto.open;
        const origSend = XHRProto.send;
        XHRProto.open = function(method, url) {{
          try {{
            this.__crawlex_xhr__ = {{
              method: String(method || 'GET').toUpperCase(),
              url: String(url || ''),
            }};
          }} catch (_) {{}}
          return origOpen.apply(this, arguments);
        }};
        XHRProto.send = function() {{
          try {{
            const info = this.__crawlex_xhr__ || {{ method: 'GET', url: '' }};
            const started = Date.now();
            const rec = {{
              kind: 'xhr', method: info.method, url: info.url, started_at: started,
            }};
            pushEndpoint(rec);
            const onDone = () => {{
              try {{
                rec.status = this.status;
                rec.ok = this.status >= 200 && this.status < 400;
                rec.duration_ms = Date.now() - started;
              }} catch (_) {{}}
            }};
            this.addEventListener('loadend', onDone, {{ once: true }});
          }} catch (_) {{}}
          return origSend.apply(this, arguments);
        }};
      }}
    }} catch (_) {{}}

    // --- IndexedDB transaction-order audit --------------------------
    // Wraps IDBObjectStore.put / add / delete so we record the order
    // writes were issued in. A follow-up collector can re-read the
    // store and compare order; divergence emits a
    // `VendorTelemetryObserved` host-event, log-only. Keeps semantics:
    // we return the original IDBRequest so callers see no change.
    try {{
      const OSProto = (typeof IDBObjectStore !== 'undefined') ? IDBObjectStore.prototype : null;
      if (OSProto) {{
        const origPut = OSProto.put;
        const origAdd = OSProto.add;
        const origDel = OSProto.delete;
        const wrap = (op, orig) => function(value, key) {{
          try {{
            const storeName = (this && this.name) || '<anon>';
            const keyRepr = (() => {{
              try {{
                if (arguments.length >= 2) return String(key);
                if (value && typeof value === 'object' && 'id' in value) return String(value.id);
              }} catch (_) {{}}
              return null;
            }})();
            pushIdb({{ op: op, store: String(storeName), key: keyRepr, at: Date.now() }});
          }} catch (_) {{}}
          return orig.apply(this, arguments);
        }};
        if (typeof origPut === 'function') OSProto.put = wrap('put', origPut);
        if (typeof origAdd === 'function') OSProto.add = wrap('add', origAdd);
        if (typeof origDel === 'function') OSProto.delete = wrap('delete', origDel);
      }}
    }} catch (_) {{}}
  }} catch (_) {{}}
}})();"#,
        cap = OBSERVER_SAMPLE_CAP
    )
}

/// Runtime route observation — matches the JS shape exactly so we can
/// `serde_json::from_value` with zero translation on the Rust side.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RouteObservation {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub at: Option<i64>,
}

/// Fetch/XHR observation — `kind` is either `"fetch"` or `"xhr"`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct NetworkEndpointObservation {
    pub kind: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub status: Option<i64>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// JS expression that reads both observer arrays as a single object.
/// Returned-by-value payload shape is
/// `{routes: [...], endpoints: [...]}` — safe to `serde_json::from_value`
/// straight into the type below.
pub fn collect_expression() -> &'static str {
    // JSON round-trip forces structured clone through return_by_value —
    // avoids any edge-cases where CDP mis-serialises the raw array.
    r#"JSON.parse(JSON.stringify({
  routes: (window.__crawlex_runtime_routes__ || []),
  endpoints: (window.__crawlex_network_endpoints__ || []),
  idb_audit: (window.__crawlex_idb_audit__ || []),
}))"#
}

/// One IndexedDB write captured by the observer. `key` is best-effort —
/// it's either the caller-provided key, or `value.id` if the record is
/// an object with an `id` field, otherwise null. Purely log-only — the
/// audit is a passive ordering cross-check.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct IdbAuditEntry {
    pub op: String,
    pub store: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub at: Option<i64>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CollectedObservations {
    #[serde(default)]
    pub routes: Vec<RouteObservation>,
    #[serde(default)]
    pub endpoints: Vec<NetworkEndpointObservation>,
    #[serde(default)]
    pub idb_audit: Vec<IdbAuditEntry>,
}
