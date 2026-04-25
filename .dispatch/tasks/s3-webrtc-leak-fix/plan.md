# S.3 — WebRTC IP leak fix

Meta: impedir browser de expor IP local via STUN. Crítico pra proxy cover — detectores correlacionam IP local com geo do proxy.

## Entrega

- [x] Adicionar launch flags em `src/render/pool.rs` (BrowserConfig):
  - `--disable-features=WebRtcHideLocalIpsWithMdns` (fundido na lista existente de disable-features)
  - `--force-webrtc-ip-handling-policy=disable_non_proxied_udp`
- [?] Opcional: CDP `WebRTC.setNetworkConstraints` com `ipHandlingPolicy: "disable_non_proxied_udp"` — não necessário: flag launch foi suficiente, audit test confirmou ausência de leak privado
- [x] `tests/webrtc_leak_audit.rs` `#[ignore]` — fixture `RTCPeerConnection` via data URL + onicecandidate logger → assert nenhuma candidate contém IP RFC 1918
- [x] Gates: build all + mini + clippy + test + live HN sem regressão
- [x] Output + `.done`

## Restrições

- Não quebrar sites que usam WebRTC legítimo (meet, discord, zoom) — flags devem apenas suprimir IPs privados, não desabilitar WebRTC
- Sem commits
- Patches Chrome 149 intocados
