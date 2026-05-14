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
