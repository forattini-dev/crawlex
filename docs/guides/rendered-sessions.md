# Rendered Sessions

Use this flow when the target depends on client-side rendering, interaction or browser timing.

## Baseline command

```bash
cargo run --release -- crawl \
  --seed https://example.com/login \
  --method auto \
  --max-concurrent-render 2 \
  --wait-strategy networkidle \
  --metrics \
  --screenshot \
  --storage filesystem --storage-path out/rendered \
  --policy deep \
  --emit ndjson
```

## Add an action script

```json
[
  { "kind": "wait_for", "selector": "#email", "timeout_ms": 5000 },
  { "kind": "type", "selector": "#email", "text": "demo@example.com" },
  { "kind": "type", "selector": "#password", "text": "hunter2" },
  { "kind": "press", "key": "Enter" },
  { "kind": "wait_ms", "ms": 1500 }
]
```

Run it with:

```bash
--actions-file actions.json
```

## Extra stability knobs

- `--chrome-path <path>` if the wrong browser is being detected
- `--profile chrome-149-stable` if you need to pin the claimed identity
- `--external-cdp-url http://127.0.0.1:9222` if Chrome is managed outside crawlex
- `--gpu-policy stealth` when GPU-facing surfaces matter more than compatibility
- `--flatten-shadow-dom` when web components hide the content you need
- `--remove-overlays --remove-consent-popups` when modals pollute captured HTML
- `--block-resource image,media,font` to cut noise on heavy apps
- `--rate-per-host-rps 1.5` when the target rate limits aggressively

## ScriptSpec and screenshots

Prefer `--script-spec <path>` for multi-step flows. It supersedes the legacy `--actions-file` path and supports waits, interaction, extraction, assertions, exports and step-scoped screenshots.

Use `--screenshot-mode viewport`, `--screenshot-mode fullpage`, or `--screenshot-mode 'element:<css>'` to control what is captured after waits/hooks/actions settle.

## When to prefer `render` over `auto`

Use `render` only when:

- every page needs JS execution
- form interaction is required throughout
- you want one consistent browser path for the entire run
