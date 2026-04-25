# Vendored chromiumoxide + Chrome 149 CDP drift fix — Fase 3 fecho

## TL;DR

**HN live test PASSES.** `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` →
`test result: ok. 1 passed; 0 failed` em ~33s. Front-page screenshot 248721 bytes, story
screenshot 37596 bytes.

Fase 3 destravada. Celebramos.

## Root cause real

Não era o `Message` enum. Eram DUAS regressões do Chrome 149 batendo no chromiumoxide 0.9.1 /
master:

1. **`ClientSecurityState` mudou schema.** Chrome 149 removeu o campo obrigatório
   `privateNetworkRequestPolicy` e adicionou `localNetworkAccessRequestPolicy`. Serde bate
   `missing field` ao tentar deserializar `Network.requestWillBeSentExtraInfo`, o handler
   dropa o frame inteiro (incluindo responses legítimas que vinham no mesmo pipeline) e
   `Page.navigate` nunca conclui.

2. **`Page.lifecycleEvent` parou de emitir pós-navegação.** Chrome 149 só emite
   `Page.lifecycleEvent` UMA vez (durante init da página about:blank). Após `Page.navigate`,
   apenas `Page.domContentEventFired` e `Page.loadEventFired` fluem — o
   `NavigationWatcher` espera `frame.lifecycle_events.contains("load")` que nunca é populado,
   e bate o timeout de 30s.

   Ademais, mesmo quando a lifecycleEvent sai, Chrome 149 renomeou o evento seminal de
   `init` pra `commit`. O código do chromiumoxide que resetava `loader_id` procurava
   literalmente `event.name == "init"`.

3. **`FrameManager::navigated()` nunca atualizava `loader_id`.** Bug latente de longa data:
   mesmo que lifecycle events chegassem, o watcher comparava `frame.loader_id ==
   watcher.loader_id` e, como `navigated()` só tocava url/name, a condição
   `frame.loader_id != watcher.loader_id` (necessária pra detectar nova navegação) nunca
   virava verdadeira.

## Patches aplicados no vendor

Clone em `vendor/chromiumoxide/` (sem submódulo — clone direto, mais simples pra CI),
pinned em rev `afcc3a4313f2087249b4490d94e54bf8e3bfaccf`.

### 1. `vendor/chromiumoxide/chromiumoxide_cdp/src/cdp.rs` — `ClientSecurityState`

```diff
 pub struct ClientSecurityState {
     #[serde(rename = "initiatorIsSecureContext")]
     pub initiator_is_secure_context: bool,
     #[serde(rename = "initiatorIPAddressSpace")]
     #[serde(deserialize_with = "super::super::de::deserialize_from_str")]
     pub initiator_ip_address_space: IpAddressSpace,
+    // NOTE(crawlex vendor patch): Chrome 149+ replaced this field with
+    // `localNetworkAccessRequestPolicy`. Making both optional so real
+    // browsers of either generation deserialize cleanly.
     #[serde(rename = "privateNetworkRequestPolicy")]
-    #[serde(deserialize_with = "super::super::de::deserialize_from_str")]
-    pub private_network_request_policy: PrivateNetworkRequestPolicy,
+    #[serde(default)]
+    #[serde(skip_serializing_if = "Option::is_none")]
+    pub private_network_request_policy: Option<PrivateNetworkRequestPolicy>,
+    #[serde(rename = "localNetworkAccessRequestPolicy")]
+    #[serde(default)]
+    #[serde(skip_serializing_if = "Option::is_none")]
+    pub local_network_access_request_policy: Option<String>,
 }
```

Construtor `new()` e `ClientSecurityStateBuilder::build()` ajustados pra embrulhar o valor
em `Some(...)` e default `None` pra `local_network_access_request_policy` (mantém
compatibilidade com quem usar builder API).

### 2. `vendor/chromiumoxide/src/handler/frame.rs`

**`Frame::navigated()`** agora propaga `loader_id` e limpa `lifecycle_events`:

```diff
 fn navigated(&mut self, frame: &CdpFrame) {
     self.name.clone_from(&frame.name);
     let url = if let Some(ref fragment) = frame.url_fragment {
         format!("{}{fragment}", frame.url)
     } else {
         frame.url.clone()
     };
     self.url = Some(url);
+    // NOTE(crawlex vendor patch): Chrome 149+ stopped re-emitting
+    // `Page.lifecycleEvent` for post-navigation lifecycle names; we
+    // propagate loader_id here so NavigationWatcher can detect the
+    // loader change and combine with `Page.loadEventFired` (see
+    // on_page_load_event_fired).
+    self.loader_id = Some(frame.loader_id.clone());
+    self.lifecycle_events.clear();
 }
```

**`on_page_lifecycle_event`** aceita `commit` como equivalente a `init`:

```diff
 pub fn on_page_lifecycle_event(&mut self, event: &EventLifecycleEvent) {
     if let Some(frame) = self.frames.get_mut(&event.frame_id) {
-        if event.name == "init" {
+        // NOTE(crawlex vendor patch): Chrome 149+ emits `commit` instead of
+        // `init` as the first lifecycle event after a navigation. Accept
+        // both so navigations complete on modern browsers.
+        if event.name == "init" || event.name == "commit" {
             frame.loader_id = Some(event.loader_id.clone());
             frame.lifecycle_events.clear();
         }
         frame.lifecycle_events.insert(event.name.clone().into());
     }
 }
```

Dois handlers novos, fallback pra Chrome 149 que não re-emite `lifecycleEvent`:

```rust
pub fn on_page_load_event_fired(&mut self) {
    if let Some(main) = self.main_frame.clone() {
        if let Some(frame) = self.frames.get_mut(&main) {
            frame.lifecycle_events.insert("load".into());
        }
    }
}

pub fn on_page_dom_content_event_fired(&mut self) {
    if let Some(main) = self.main_frame.clone() {
        if let Some(frame) = self.frames.get_mut(&main) {
            frame.lifecycle_events.insert("DOMContentLoaded".into());
        }
    }
}
```

### 3. `vendor/chromiumoxide/src/handler/target.rs` — wiring

```diff
 CdpEvent::PageLifecycleEvent(ev) => self.frame_manager.on_page_lifecycle_event(ev),
+CdpEvent::PageLoadEventFired(_) => self.frame_manager.on_page_load_event_fired(),
+CdpEvent::PageDomContentEventFired(_) => {
+    self.frame_manager.on_page_dom_content_event_fired()
+}
```

### 4. `Cargo.toml` (raiz)

```diff
-chromiumoxide = { git = "https://github.com/mattsse/chromiumoxide", rev = "afcc3a4313f2087249b4490d94e54bf8e3bfaccf", default-features = false, features = ["bytes"], optional = true }
-chromiumoxide_fetcher = { git = "https://github.com/mattsse/chromiumoxide", rev = "afcc3a4313f2087249b4490d94e54bf8e3bfaccf", default-features = false, features = ["rustls", "zip8"], optional = true }
+chromiumoxide = { path = "vendor/chromiumoxide", default-features = false, features = ["bytes"], optional = true }
+chromiumoxide_fetcher = { path = "vendor/chromiumoxide/chromiumoxide_fetcher", default-features = false, features = ["rustls", "zip8"], optional = true }
```

Note: crate root está em `vendor/chromiumoxide/` (não em `vendor/chromiumoxide/chromiumoxide/` como o plano esperava — upstream usa layout `Cargo.toml` na raiz + sub-crates em siblings).

## Resultados de teste

| Teste | Status | Tempo | Notas |
|-------|--------|-------|-------|
| `live_news_navigation` | **PASS** | 33s | front PNG 248721B, story PNG 37596B. HN + target externo (wheelfront.com) ambos ok. |
| Non-ignored `cargo test --all-features` | PASS | ~15s agregado | Todos os crates passam, 0 regressões. |
| `cargo clippy --all-features --all-targets -- -D warnings` | PASS | — | Vendor não é varrido pelo clippy do crate pai por default. |
| `cargo build --no-default-features --features cli,sqlite` | PASS | — | Mini build intocado. |
| `spa_lua_flow_live` | FAIL (pré-existente) | 5s | Erro: `selector timeout: #dashboard`. Test usa `WaitStrategy::Selector{css:"#dashboard", timeout:5s}` como espera inicial, mas esse elemento só existe DEPOIS do click que é disparado pelo hook Lua, que por sua vez roda DEPOIS do wait no pipeline do `RenderPool`. Design bug, não regressão. |
| `spa_render_live` | FAIL (pré-existente) | 5s | Mesmo bug de ordem: wait pede `#dashboard` que só existe após a ação `Click{#go}` — ações rodam pós-wait. |

## Manutenção futura

- **Re-sincronizar com upstream chromiumoxide:** `cd vendor/chromiumoxide && git fetch
  origin && git checkout <novo-rev>`. Aplicar as 3 hunks acima de novo (ou checar se
  upstream já absorveu). Clippy do vendor não conta pro gate do `crawlex`.
- **PR upstream:** abrir PR em `mattsse/chromiumoxide` com:
  - `ClientSecurityState` → optional fields + `localNetworkAccessRequestPolicy`
  - `frame.rs` navigated() loader_id fix (é bug latente, não apenas Chrome 149)
  - `commit` como alias de `init`
  - handlers `on_page_load_event_fired` / `on_page_dom_content_event_fired`
  - opcionalmente, um `Message::Unknown(Value)` como defesa em profundidade pra próximas
    surpresas do CDP
- **Monitorar novas diffs do CDP:** `chromiumoxide_pdl` regenera protocol bindings a
  partir do browser_protocol.pdl; se upstream bumped, os structs de Network.* mudam.

## Observação final

O problema que o dispatch anterior diagnosticou como "WS Invalid message" era só o sintoma
MAIS visível do Chrome 149 drift. Na realidade, depois de consertar `ClientSecurityState`,
o enum `Message` nunca falhou parse — o problema era outro nível abaixo, no ciclo de vida
da navegação. Dois bugs empilhados. Agora ambos caem.
