# Crawl Pipeline

## 1. Seed intake

Seeds come from repeated `--seed` flags or `--seeds-file`. Each URL is canonicalized and deduplicated before entering the queue.

## 2. Queue selection

Each job carries:

- URL
- depth
- priority
- fetch method (`spoof`, `render`, `auto`)
- retry state

With SQLite enabled, jobs survive process crashes and `in_flight` rows are reclaimed on the next open.

## 3. Policy decision

The active policy profile shapes limits like:

- job wall clock budget
- retry cap
- render budget
- host cooldown
- proxy score floor

Profiles available today:

- `fast`
- `balanced`
- `deep`
- `forensics`

## 4. Fetch or render

- `spoof`: use the impersonation client directly.
- `render`: use Chrome for every job.
- `auto`: let the crawler decide when a render path is worth paying for.

## 5. Extraction

After a response or render completes, the crawler can:

- extract links from the document
- classify assets by URL and MIME
- persist raw or rendered bodies
- save graph edges
- emit lifecycle events

## 6. Auxiliary discovery

Per-host and per-domain enrichment can add new roots around the core crawl:

- robots path expansion
- `/.well-known/*` probing
- PWA manifest and service worker discovery
- Wayback seeding
- DNS enumeration
- favicon hashing
- peer certificate SAN seeding
- RDAP lookups

## 7. Persistence and observability

Storage backends own artifacts. Events expose the run externally. Metrics can be written into storage and served over a small Prometheus-compatible endpoint.
