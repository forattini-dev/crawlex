# Infra-tier TCP fingerprint (SYN window, MSS, TTL, timestamps)

Audience: operators running `crawlex` in production at scale when the target
uses TCP-layer fingerprinting (p0f-style passive fingerprints, Cloudflare
bot management signals derived from initial-SYN shape, Imperva / Akamai
L4 heuristics). The Rust crate deliberately does **not** try to reshape
these values. They are an **operator infrastructure concern, not a crate
scope**.

This document explains why, and what to do if you need them.

## Why the crate does not fake these values

All four knobs below are set by the host's TCP/IP stack before any userland
library (hickory, hyper, boring) gets to run:

| Item | Set by | Userland control |
|------|--------|------------------|
| SYN window size | kernel (`net.ipv4.tcp_rmem`, `tcp_wmem`) | `SO_RCVBUF` / `SO_SNDBUF` — hints only, kernel still owns the final value |
| MSS on SYN | kernel, derived from egress iface MTU | `TCP_MAXSEG` (Linux) — requires `CAP_NET_ADMIN` to widen |
| TTL | kernel default (`net.ipv4.ip_default_ttl` = 64 Linux / 128 Windows) | `IP_TTL` setsockopt works but is visible in `/proc` and does not change the stack's "class" |
| Timestamp option, SACK, WSCALE | kernel (`net.ipv4.tcp_timestamps`, `tcp_sack`, `tcp_window_scaling`) | none — these are negotiated by the kernel during the three-way handshake |

A Rust crate that tries to override these via raw sockets needs
`CAP_NET_RAW`, breaks on non-Linux targets, and re-implements half the
kernel's TCP state machine. That is **not** what `crawlex` is.

## Items out of scope (#13, #14)

* **#13 TCP SYN window / MSS / TTL to match Chrome on Linux.**
  Real Chrome on Linux inherits the host's stack values (wscale 7, MSS
  1460 on a 1500-MTU path, initial window ~14600, TTL 64). To impersonate
  Chrome-on-macOS (TTL 64, wscale 6) or Chrome-on-Windows (TTL 128,
  wscale 8) from a Linux worker, the operator must run inside a network
  namespace whose stack is pre-tuned, OR terminate the TCP at a reverse
  proxy that owns the outbound SYN.

* **#14 TCP timestamps enabled.**
  Chrome's tcp_timestamps is controlled by `net.ipv4.tcp_timestamps = 1`
  on the host. This is already the kernel default on Linux ≥ 4.10, so
  no action is usually needed — but **disable it inside a container**
  at your peril: some k8s images ship with `tcp_timestamps = 0`, which
  reliably flips the Linux-Chrome fingerprint into a "minimal embedded
  Linux" bucket.

## Operator checklist

If your detector shop reports "not Chrome" despite JA3/JA4/HTTP2 all
matching, the remaining divergence is almost always in this tier:

1. `sysctl net.ipv4.tcp_timestamps` — must be `1`.
2. `sysctl net.ipv4.tcp_window_scaling` — must be `1`.
3. `sysctl net.ipv4.tcp_sack` — must be `1`.
4. `ip link show dev <egress>` MTU should be 1500 (not 1450 from an
   over-eager VPN overlay, not 9000 from a jumbo-frame cluster fabric).
5. `ip rule` / `ip route` should not route egress through a tun/tap
   device whose stack quirks differ from the host.
6. Container runtime: confirm the sysctls above are not overridden by
   the runtime (`docker inspect --format '{{.HostConfig.Sysctls}}'`).

## What the crate *does* do

* TLS fingerprint: every extension Chrome M144+ emits is present
  (JA3 + JA4 + Akamai H2 all match current Chrome; see
  `tests/tls_clienthello.rs` + `src/impersonate/ja3.rs`).
* HTTP/2 fingerprint: SETTINGS frame (id order + values) + connection
  WINDOW_UPDATE delta + pseudo-header order (forked `h2` crate) all
  match Chrome (see `vendor/h2` patch).
* Header wire order per fetch kind: see
  `src/impersonate/headers.rs::ChromeRequestKind::header_order`.

The TCP-layer values (#13, #14) are the only gap, and it is the gap
the crate intentionally refuses to take on. Document it, don't fake it.

## References

* `references/curl-impersonate/chrome/` — reference fingerprint payloads.
* `net.ipv4.tcp_*` sysctls — `man 7 tcp`.
* p0f v3 signature DB — `https://github.com/p0f/p0f/blob/master/p0f.fp`
  (the canonical list of stack fingerprints detectors compare against).
