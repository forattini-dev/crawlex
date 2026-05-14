# Slice 18: ItemScraped event + spider.stream() [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Items yielded by `parse` are emitted on the existing event bus as a new `EventKind::ItemScraped { spider_id, identifier?, payload }`. The event contract version is bumped per `docs/architecture/04-events-hooks-sdk.md`. The spider exposes a `stream()` async iterator (Node) and a `Stream<Item>` (Rust) that filters items off the bus for consumers.

## Acceptance criteria

- [ ] `EventKind::ItemScraped` variant added with `spider_id`, optional `identifier`, JSON payload
- [ ] Event contract version bumped; `docs/architecture/04-events-hooks-sdk.md` and `docs/reference/events.md` updated
- [ ] `spider.stream()` (Node) returns an `AsyncIterable<Item>`
- [ ] Rust equivalent returns a `Stream<Item>` (tokio_stream)
- [ ] Backpressure: consumers can lag without crashing the bus
- [ ] Integration test asserts items appear on stream in the order yielded

## Blocked by

- Slice 17 (spider DSL emits the items)

## Implementation notes (2026-05-14)

- `src/events/envelope.rs`: `EVENT_ENVELOPE_VERSION` bumped 2 → 3, `EventKind::ItemScraped` added (wire name `item.scraped`), new `ItemScrapedData { spider_id, identifier?, payload }` struct. Re-exported from `crate::events`.
- `src/scraping/spider.rs`: `SpiderRunner` gained `with_id`, `with_event_sink(DynSink)`, `stream(buffer) -> impl Stream<Item = Value>`. The stream wraps a `tokio::sync::broadcast` channel via a tiny `unfold` adapter; `Lagged` errors are silently skipped (the bus stays alive, slow consumers drop history). Items are emitted on the event sink and broadcast in the same `emit_item` helper. `Spider::item_identifier` default extracts `id`/`url` from the payload — recipes can override.
- New Rust tests: event-sink emission with identifier extraction, `stream()` ordering, and `stream()` survives a lagging consumer with capacity 2 vs 5 items.
- sdk: `BaseEnvelope.v: 3`, new `'item.scraped'` kind + `ItemScrapedData` interface, `SpiderHandle.stream({ bufferSize })` returning a fresh broadcast subscriber. JS broadcaster mirrors the tokio semantics — bounded ring per subscriber, overflow drops oldest. Subscriber set is cleared in `outer()`'s `finally` so partial iteration closes the stream cleanly.
- Two new SDK tests cover order + lagging-consumer drop policy.
- docs: `docs/reference/events.md` and `docs/architecture/04-events-hooks-sdk.md` updated for the version bump and the new kind / payload / streaming surface.

Status: code complete. Could not run `cargo check`/`cargo test`/`git` from this session — those commands need permission. Manual verification required before merge.
