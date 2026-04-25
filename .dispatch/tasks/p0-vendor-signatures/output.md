# P0-9 Vendor Signatures Expansion — Output

Fechado. 10/11 checklist items `[x]`. 1 desvio formal sobre criação deste output.md (resolvido aqui).

## Entregue

- `src/antibot/telemetry.rs` — `VendorTelemetry` + `PayloadShape` (8 variants) + observer hook
- `src/antibot/signatures.rs` — URL patterns para 10 vendors + 29 PX signal IDs
- Akamai sensor_data parser parcial (v1.7/v2, sem decodificar)
- `src/render/pool.rs` — filtro em `Network.requestWillBeSent`
- `src/storage/sqlite.rs` — tabela `vendor_telemetry` + persist
- `src/policy/engine.rs` — high-volume (>20 posts/30s) → `SessionAction::RotateProxy` preventivo
- `EventKind::VendorTelemetryObserved`
- Unit tests (classifier coverage + PX catalog completude)

## Desvios

1. **Observer não emit via EventBus**: `render_core` não tem sink plumbing — persiste SQLite + tracing. Variant serializável adicionado pra futuro wire.
2. **`ChallengeVendor` ganhou `#[derive(Hash)]`**: necessário pra `HashMap` no telemetry tracker. Aditivo, zero breakage.
3. **`antibot::system_time_serde` elevado pra `pub(crate)`**: reuso do serde contract (unix millis) em `VendorTelemetry`.

## Gates

- `cargo build --all-features` OK
- `cargo build --no-default-features --features cli,sqlite` OK
- `cargo clippy --all-features --all-targets -- -D warnings` OK
- `cargo test --all-features` non-ignored OK
- Live HN sem regressão
- Live throughput baseline 14.9 rps preservado

## Restrições respeitadas
- Patches Chrome 149 intocados
- Licenças preservadas
- Sem commits
- Sem decodificar ofuscação
