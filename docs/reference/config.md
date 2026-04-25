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
  "auto_fetch_chromium": true
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

## When to prefer flags over JSON

Use CLI flags when:

- you only need one-off operator runs
- you want ergonomic enum values such as `--method auto`
- you are varying a small number of parameters between runs

Use `--config` when:

- you want the same crawl shape in CI or tests
- you are embedding `crawlex` behind another service
- you want to stream one canonical config over stdin from another process
