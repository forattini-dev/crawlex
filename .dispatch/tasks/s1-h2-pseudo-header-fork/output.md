# S.1 — h2 pseudo-header fork — OUTPUT

Fechado. Fechado por dispatcher (worker abortou antes de escrever `.done`).

## Entregue
- `vendor/h2/` clonado
- `vendor/h2/src/frame/headers.rs::Iter::next` — ordem `method → authority → scheme → path` (linhas 705/711/715/719)
- `Cargo.toml` raiz: `[patch.crates-io] h2 = { path = "vendor/h2" }`

## Gates
- `cargo build --all-features`: ✅ 14m15s cold
- `cargo build --no-default-features --features cli,sqlite`: ✅ 6m55s
- `cargo clippy --all-features --all-targets -- -D warnings`: ✅ 31s
- `cargo test --all-features --test h2_fingerprint_live -- --ignored`: ✅ **PASS 0.03s** (pseudo-header `:method,:authority,:scheme,:path` byte-exact)
- `cargo test --all-features --test live_news_navigation -- --ignored`: ✅ **PASS 33.50s** (sem regressão)

Akamai H2 fingerprint unblocked.
