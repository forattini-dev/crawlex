# Config JSON

`--config` deserializes directly into `crawlex::config::Config`. That means the JSON shape follows Rust `serde` defaults, including externally tagged enums.

## Example

```json
{
  "max_concurrent_render": 2,
  "max_concurrent_http": 300,
  "max_depth": 4,
  "same_host_only": false,
  "include_subdomains": true,
  "respect_robots_txt": true,
  "user_agent_profile": "Chrome149Stable",
  "chrome_path": null,
  "chrome_flags": ["--disable-gpu"],
  "block_resources": ["image", "media"],
  "reject_resource_types": ["image", "font"],
  "wait_strategy": {
    "NetworkIdle": {
      "idle_ms": 750
    }
  },
  "rate_per_host_rps": 2.0,
  "retry_max": 4,
  "retry_backoff": {
    "secs": 0,
    "nanos": 750000000
  },
  "queue_backend": {
    "Sqlite": {
      "path": "state/queue.db"
    }
  },
  "storage_backend": {
    "Sqlite": {
      "path": "state/crawl.db"
    }
  },
  "output": {
    "html_dir": null,
    "graph_path": null,
    "metadata_path": null,
    "screenshot_dir": null,
    "screenshot": true
  },
  "proxy": {
    "proxies": ["http://127.0.0.1:8080"],
    "proxy_file": null,
    "strategy": "RoundRobin",
    "sticky_per_host": false,
    "health_check_interval": null
  },
  "locale": "en-US",
  "timezone": "UTC",
  "metrics_prometheus_port": 9108,
  "hook_scripts": [],
  "discovery_filter_regex": null,
  "follow_pages_only": true,
  "crtsh_enabled": true,
  "robots_paths_enabled": true,
  "well_known_enabled": true,
  "pwa_enabled": true,
  "wayback_enabled": false,
  "favicon_enabled": true,
  "dns_enabled": false,
  "collect_net_timings": true,
  "collect_web_vitals": true,
  "collect_peer_cert": false,
  "rdap_enabled": false,
  "cookies_enabled": true,
  "follow_redirects": true,
  "max_redirects": 10,
  "profile_autodetect": true,
  "user_agent_override": null,
  "auto_fetch_chromium": true,
  "cache_validation": {
    "enabled": true,
    "max_age_secs": 86400
  },
  "prefetch": false,
  "crawl_scoring": {
    "enabled": true,
    "keywords": ["docs", "api"],
    "same_host_bonus": 5,
    "keyword_bonus": 8,
    "depth_penalty": 2,
    "path_depth_penalty": 1
  },
  "fallback_fetch": null,
  "external_cdp_url": null,
  "gpu_policy": "compat",
  "dom_capture": {
    "flatten_shadow_dom": false,
    "remove_overlays": false,
    "remove_consent_popups": false
  }
}
```

## Important enum fields

| Field | Expected shape |
| --- | --- |
| `user_agent_profile` | `"Chrome131Stable"` / `"Chrome132Stable"` / `"Chrome149Stable"` |
| `wait_strategy` | `"Load"`, `"DomContentLoaded"`, `{ "NetworkIdle": { "idle_ms": 500 } }`, `{ "Fixed": { "ms": 1000 } }`, `{ "Selector": { "css": "...", "timeout_ms": 5000 } }` |
| `queue_backend` | `"InMemory"` or `{ "Sqlite": { "path": "queue.db" } }` |
| `storage_backend` | `"Memory"`, `{ "Sqlite": { "path": "crawl.db" } }`, `{ "Filesystem": { "root": "crawl-out" } }` |
| `proxy.strategy` | `"RoundRobin"`, `"Sequential"`, `"Random"`, `"StickyPerHost"` |
| `gpu_policy` | `"compat"` or `"stealth"` |

## Recent fields

| Field | Purpose | Default |
| --- | --- | --- |
| `cache_validation.enabled` | Validate stored page metadata before full processing | `false` |
| `cache_validation.max_age_secs` | Accept cache rows younger than this age without a validation probe | `null` |
| `prefetch` | Discovery-only mode: harvest links and skip heavy page analysis | `false` |
| `crawl_scoring.enabled` | Score newly discovered URLs before enqueueing | `false` |
| `crawl_scoring.keywords` | Keyword bonuses applied to host/path/query | `[]` |
| `fallback_fetch` | Last-resort external command receiving JSON stdin and returning JSON stdout | `null` |
| `external_cdp_url` | Connect to an existing Chrome/Chromium CDP endpoint | `null` |
| `gpu_policy` | Managed Chrome GPU posture | `"compat"` |
| `dom_capture.flatten_shadow_dom` | Serialize open shadow roots into captured HTML | `false` |
| `dom_capture.remove_overlays` | Remove fixed/sticky overlays before HTML capture | `false` |
| `dom_capture.remove_consent_popups` | Remove common consent/cookie banners before HTML capture | `false` |
| `reject_resource_types` | Typed CDP reject list: `image`, `media`, `font`, `stylesheet`. Auto-disabled (with a warn-level log) when the job requests a screenshot. | `[]` |

## Resource-type blocking

Two knobs feed Chrome's `Network.setBlockedURLs` so heavy assets never hit the wire:

- `block_resources: ["image", "font", "media", "stylesheet", "script", "analytics"]` —
  legacy untyped list. Accepts a broader set including `script` and `analytics`
  (vendor-domain wildcards). Kept for back-compat.
- `reject_resource_types: ["image", "media", "font", "stylesheet"]` — typed
  successor; mirrors Cloudflare's canonical set and is the recommended field for
  new configs. Auto-disabled (with a warn-level log) when the job requests a
  screenshot, so visual fidelity is preserved.

Both code paths emit identical URL patterns for the four overlapping categories,
so you can swap from one to the other without changing observable bandwidth.

CLI: `--reject-resource-type image --reject-resource-type media` (repeatable, or
comma-separated: `--reject-resource-type image,media`).

## URL match patterns (glob ↔ regex)

`crawlex::pattern` exposes a single entry point, `compile_pattern(&str) -> Regex`,
that auto-detects which dialect a string is written in and compiles it to an
anchored `regex::Regex`. Hot-path matching is identical regardless of the
input dialect.

Grammar (glob):

- `*` — matches any chars except `/`
- `**` — matches any chars including `/`; a trailing `/` after `**/` is optional
- `?` — matches exactly one char except `/`
- every other char is literal (regex metachars are escaped at compile time)

Auto-detect: a pattern is treated as a **regex** when it contains any of
`^ $ ( ) [ ] { } | + \`. Otherwise it is treated as a **glob**. `*` and `?`
are ambiguous and always resolve to the glob meaning.

When include and exclude patterns both match a URL, **exclude wins**. This
precedence is implemented by the caller (e.g. `extract::link_filter`).

Migration examples — old regex → equivalent glob:

| Regex | Glob |
| --- | --- |
| `^/blog/[^/]+$` | `/blog/*` |
| `^/docs/.*$` | `/docs/**` |
| `^/api/v[0-9]+/users$` | (keep as regex — character class) |

If you need full regex power (alternation, classes, anchors), just write a
regex; the engine will detect it and skip glob translation.

## When to prefer flags over JSON

Use CLI flags when:

- you only need one-off operator runs
- you want ergonomic enum values such as `--method auto`
- you are varying a small number of parameters between runs

Use `--config` when:

- you want the same crawl shape in CI or tests
- you are embedding `crawlex` behind another service
- you want to stream one canonical config over stdin from another process
