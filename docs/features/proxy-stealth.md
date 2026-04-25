# Proxy and Stealth

## Identity coherence

The crawler tries to present one consistent browser identity across:

- request headers
- client hints
- TLS behavior
- render-side browser properties

That is why the profile system is explicit instead of a bag of loosely related overrides.

## Proxy rotation strategies

Available strategies:

- `round-robin`
- `sequential`
- `random`
- `sticky-per-host`

Example:

```bash
cargo run --release -- crawl \
  --seed https://example.com \
  --proxy http://127.0.0.1:8080 \
  --proxy http://127.0.0.1:8081 \
  --proxy-strategy sticky-per-host
```

`sticky-per-host` is usually the safest default when you need session continuity per origin.

## Raffel-backed local proxy

The CLI can spawn a local explicit proxy and force the crawl through it:

```bash
--raffel-proxy \
--raffel-proxy-path /path/to/raffel \
--raffel-proxy-host 127.0.0.1 \
--raffel-proxy-port 8899
```

## Verification commands

Use these when you are changing impersonation logic:

```bash
cargo run --release -- inspect-fingerprint https://tls.peet.ws/api/clean
cargo run --release -- test-stealth
```

They are the fastest way to see whether a profile or transport change drifted.
