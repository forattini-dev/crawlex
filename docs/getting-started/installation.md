# Installation

## Runtime requirements

- Rust `1.91` if you are building from source.
- Node.js `>=20` only if you want the JavaScript wrapper in `sdk/`.
- A local Chrome or Chromium if you want render mode without auto-download.

Render-capable builds can auto-download a pinned Chromium-for-Testing binary unless you pass `--no-fetch-chromium`.

## Build from source

```bash
git clone https://github.com/forattini-dev/crawlex
cd crawlex
cargo build --release
./target/release/crawlex --help
```

The default feature set already includes:

- `cli`
- `sqlite`
- `cdp-backend`
- `chromium-fetcher`

## Optional Lua hooks build

`--hook-script` only does work when the binary is compiled with the `lua-hooks` feature.

```bash
cargo build --release --features lua-hooks
```

## JavaScript wrapper

The package surface lives in `sdk/crawlex-sdk.js` and resolves a native binary from:

1. `CRAWLEX_FORCE_BINARY`
2. `node_modules/.../.crawlex/bin/crawlex`
3. the system `PATH`

Typical usage:

```bash
npm install crawlex
npx crawlex --help
```

Or from code:

```js
const { crawl, ensureInstalled } = require('crawlex');

await ensureInstalled();

for await (const event of crawl({
  seeds: ['https://example.com'],
  args: ['--method', 'auto']
})) {
  console.log(event);
}
```

## Preview the docs locally

The docs are plain static files under `docs/`, so any static server works:

```bash
npx docsify-cli serve docs
```
