# crawlex — agent notes

Rust workspace. Use `cargo`, not `pnpm`/`npm`.

## Feedback loops

- Tests: `cargo test --all-features`
- Type/lint: `cargo check --all-targets --all-features`
- Optional: `cargo clippy --all-targets --all-features -- -D warnings`

When ralph or other agents are told to run `pnpm test` / `pnpm typecheck`, substitute the cargo equivalents above.

## Conventions

- Use `pnpm` only inside `sdk/` (TS SDK subproject).
- Don't add docs files unless asked.
- Stealth/antibot is a core feature — do not weaken it for "compatibility" with managed-crawler conventions (e.g. fixed UA, no-CAPTCHA-bypass).
- Maximum-discovery is the default — do not add `source` filtering (sitemaps-only / links-only).
