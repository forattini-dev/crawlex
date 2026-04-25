# Wave 1 — Network fingerprint (TLS extensions + DoH + resource hints + connection coalescing)

Meta: network-level coherence. Owner: `src/impersonate/*` + `src/http/*` + `src/discovery/dns.rs`.

## Items cobertos
- C.2 DoH (DNS over HTTPS) via hickory
- #16 TLS key_share curve order audit (X25519, P-256, P-384 Chrome order)
- #27 DNS prefetch simulation on link hover
- #28 `<link rel="preconnect"/dns-prefetch"/preload">` resource hints honor
- #29 HTTP/2 connection coalescing via SAN
- #13/#14 TCP SYN/timestamps — **doc + marker only** (eBPF/kernel out of scope, document as "infra-tier")
- #17 OCSP stapling behavior audit
- Post-quantum KEM audit (`EnableTLS13KyberPQ`)

## Arquivos alvo
- `src/impersonate/tls.rs` (TLS extension audit)
- `src/impersonate/mod.rs` (header set + resource hints)
- `src/discovery/dns.rs` (DoH enable)
- `src/http/pool.rs` (SAN coalescing)
- `tests/tls_extension_order.rs`
- `tests/doh_live.rs` (#[ignore])
- `tests/connection_coalescing.rs`
- `docs/infra-tier-tcp.md` (NEW — doc #13/#14 infra requirements)

## Checklist
- [ ] Audit `src/impersonate/tls.rs` — verify key_share extension emit order matches Chrome 144+: X25519Kyber768Draft00, X25519, P-256, P-384. Fix if diverge.
- [ ] TLS ClientHello extension presence audit: GREASE, supported_versions [1.3, 1.2], signature_algorithms, status_request (OCSP), key_share, psk_key_exchange_modes, application_settings (ALPS framed), application_layer_protocol_settings (ALPS)
- [ ] DoH: enable `hickory-resolver` DoH with Cloudflare 1.1.1.1 default; CLI flag `--doh <provider>` (default=cloudflare, off=system)
- [ ] DNS prefetch: when HTML parse captures `<link rel="dns-prefetch" href>`, schedule hickory lookup pra warm cache (não resolve URL, só DNS)
- [ ] Resource hints honor: parser HTML detect `<link rel="preconnect|preload|modulepreload">` → schedule connection/fetch preemptivo
- [ ] Connection coalescing: quando já tenho H2 conn pra IP X com cert SAN incluindo host Y, reusar conn pra Y em vez de abrir nova
- [ ] OCSP stapling: verify `CertificateStatusRequest` extension emit — Chrome envia
- [ ] `tests/tls_extension_order.rs` — parse ClientHello from boring SslContextBuilder, assert extension order exato
- [ ] `tests/doh_live.rs` `#[ignore]` — resolver DoH vs system; assert DoH path used
- [ ] `tests/connection_coalescing.rs` — mock 2 hosts mesmo IP/SAN; assert conn reuse
- [ ] Doc `docs/infra-tier-tcp.md`: #13 TCP SYN window/MSS/TTL (requires raw socket or network namespace + SO_MARK), #14 TCP timestamps (kernel tunable `net.ipv4.tcp_timestamps`). Marked as "operator infrastructure concern, not Rust crate scope".
- [ ] Gates: build all + mini + clippy + test + live HN sem regressão
- [ ] Output + `.done`

## Restrições
- NÃO mexer em stealth_shim.js, motion/, pool.rs (launch flags), handler, crawler.rs, antibot/
- Chrome 149 patches intocados
- Licenças preservadas
- Sem commits
- DoH default OFF (opt-in) pra não mudar behavior existente
- hickory-resolver já é dep — aproveita
