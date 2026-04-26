# Vendored TLS signatures (from curl-impersonate)

Per-browser-version `tls_client_hello` signatures vendored from
`lwthiker/curl-impersonate` v0.6.1-3, commit `822dbefe42` — used by
`build.rs` at compile time to populate the static `TlsFingerprint`
catalog.

## Files

| File | Profiles |
|---|---|
| `chrome.yaml`  | 9  (Chrome 98, 99, 100, 101, 104, 107, 110, 116 win10 + 99 android12-pixel6) |
| `firefox.yaml` | 7  (Firefox 91esr, 95, 98, 100, 102, 109, 117 win10) |
| `edge.yaml`    | 3  (Edge 98, 99, 101 win10) |
| `safari.yaml`  | 2  (Safari 15.3, 15.5 macos) |

Total: 21 profiles. See [`SCHEMA.md`](SCHEMA.md) for the YAML grammar.

## Why vendored

These files used to live as a git submodule under
`references/curl-impersonate/tests/signatures/`. We dropped the submodule
when v1 was about to ship to keep the repo self-contained — `build.rs`
needs these files to compile, so a missing-submodule clone would break
`cargo build`. Vendoring is licensed (MIT, see
[`LICENSE-curl-impersonate`](LICENSE-curl-impersonate)) and attributed in
the root `NOTICE`.

## Updating

When curl-impersonate ships a new release with newer browser captures:

```bash
git clone --depth 1 https://github.com/lwthiker/curl-impersonate /tmp/cu
cp /tmp/cu/tests/signatures/{chrome,edge,firefox,safari}.yaml \
   src/impersonate/catalog/vendored/
# verify diff, update commit hash in this README, commit.
```

Locally-captured profiles (Phase 3) live in
`../captured/` and are picked up by `build.rs` alongside these vendored
files; the captured copy wins on name collision.
