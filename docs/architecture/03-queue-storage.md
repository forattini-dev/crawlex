# Queue and Storage

## Queue backends

### In-memory

- zero setup
- lost on process exit
- useful for tests and short-lived runs

### SQLite

- durable queue state
- retry metadata and `not_before` scheduling
- automatic reclaim of rows left `in_flight`
- required for the `queue` subcommands

The CLI currently accepts `--queue redis`, but that backend is not implemented. Use `inmemory` or `sqlite`.

## Storage backends

### Memory

Useful for programmatic runs that only inspect results in-process.

### SQLite

Persists:

- pages
- host facts
- page metrics
- graph edges

Writes go through a dedicated writer thread to avoid per-operation mutex contention on async tasks.

### Filesystem

Stores sharded blobs on disk and appends metadata and edges as JSON Lines.

Expected top-level layout:

- `html/`
- `raw/`
- `screenshots/`
- `metadata.jsonl`
- `edges.jsonl`

## Current caveats

- `resume` exists in the CLI surface but is intentionally blocked for now.
- `--output-html-dir`, `--output-graph`, `--output-metadata` and `--screenshot-dir` are present in the CLI contract, but the reliable artifact paths today come from the selected storage backend and explicit export commands.
- `export-graph` reads graph edges from SQLite storage; it is not backed by in-memory or filesystem state.
