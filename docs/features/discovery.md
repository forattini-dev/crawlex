# Discovery Enrichment

The core crawler extracts links from fetched or rendered content, but `crawlex` can also widen the frontier with auxiliary probes.

## Built-in enrichment toggles

| Feature | Flag | Default |
| --- | --- | --- |
| robots path expansion | `--no-robots-paths` to disable | on |
| `/.well-known/*` probing | `--no-well-known` to disable | on |
| PWA manifest and service worker probing | `--no-pwa` to disable | on |
| favicon hashing | `--no-favicon` to disable | on |
| Wayback seeding | `--wayback` | off |
| DNS enumeration | `--dns` | off |
| crt.sh subdomain seeding | `--crtsh` | off |
| peer certificate SAN seeding | `--peer-cert` | off |
| RDAP collection | `--rdap` | off |

## Asset following

By default the crawler only enqueues URLs classified as page, document or API-like. Other assets are still observed and stored, but not turned into frontier jobs.

Use `--follow-all-assets` when you explicitly want every discovered asset type to become a queued job.

## Suggested combinations

### Fast internet reconnaissance

```bash
--method spoof --crtsh --dns --wayback
```

### Focused application mapping

```bash
--method auto --peer-cert --rdap
```

### Conservative crawl

Leave the defaults on and avoid the opt-in probes.
