# Fetch, Render and Identity

## HTTP impersonation

The HTTP path is built around a typed `Profile`:

- `Chrome131Stable`
- `Chrome132Stable`
- `Chrome149Stable`

The goal is coherence across:

- `User-Agent`
- `sec-ch-ua*` headers
- TLS and ALPN characteristics
- browser-facing identity shims

If a real Chrome binary is present, the CLI auto-detects its major version and picks the closest matching profile unless you override it explicitly.

## Render pool

The render path stays lazy. If no job ever requires Chrome, the pool is never created.

When rendering is active, the pool can:

- wait for `load`, `domcontentloaded`, `networkidle`, a selector or a fixed delay
- execute declarative action scripts
- collect Web Vitals
- capture screenshots
- route traffic through the selected proxy

## Action scripts

Action files are JSON arrays executed sequentially on each rendered page.

```json
[
  { "kind": "wait_for", "selector": "#email", "timeout_ms": 5000 },
  { "kind": "click", "selector": "#email" },
  { "kind": "type", "selector": "#email", "text": "me@example.com" },
  { "kind": "type", "selector": "#password", "text": "hunter2" },
  { "kind": "click", "selector": "button[type=submit]" },
  { "kind": "wait_ms", "ms": 1500 }
]
```

Supported actions today:

- `wait_for`
- `wait_ms`
- `click`
- `type`
- `scroll`
- `eval`
- `submit`
- `press`

## Practical rule

Use `spoof` for coverage at scale. Use `auto` when the site occasionally requires JS. Use `render` only when you know every target page needs a browser.
