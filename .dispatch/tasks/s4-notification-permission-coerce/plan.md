# S.4 — Notification.requestPermission coerção

Meta: fechar leak FPJS clássico. Headless retorna `denied` direto; Chrome real retorna `default` em certas condições.

## Entrega

- [ ] Em `src/render/stealth_shim.js`, adicionar override de `Notification.requestPermission`:
  ```js
  // Preserva a original pra fallback
  const origReq = Notification.requestPermission.bind(Notification);
  Notification.requestPermission = function(callback) {
      // Se Notification.permission é 'denied' (headless default), coage pra 'default'
      // pra não sinalizar inconsistência com permissions.query
      const result = Notification.permission === 'denied' ? 'default' : Notification.permission;
      if (typeof callback === 'function') {
          callback(result);
          return undefined;
      }
      return Promise.resolve(result);
  };
  // toString trap preserva nativeness
  ```
- [ ] Unit test em `src/render/stealth.rs` — parse JS shim + assert override presente + wrap lógica correta
- [ ] Gates: build all + mini + clippy + test + live HN sem regressão
- [ ] Output + `.done`

## Restrições

- Preserve signature da API (callback-based + Promise-based)
- toString da função sobrescrita deve bater native `function requestPermission() { [native code] }` (já temos toString Proxy trap no shim — reusar)
- Sem commits
- Patches Chrome 149 intocados
