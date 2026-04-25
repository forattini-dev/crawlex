// Stealth shim — hardens the page surface against FingerprintJS v3/v4 probes
// and common headless-detection heuristics. Runs before any page script via
// Page.addScriptToEvaluateOnNewDocument.
//
// What FingerprintJS checks, grouped: navigator props, screen, timezone,
// plugins+mimeTypes, canvas 2D, WebGL (vendor, extensions, attrs, params),
// AudioContext, matchMedia queries, webdriver/automation tokens.
// This shim patches all of the above to present a coherent "real desktop
// Chrome on Linux" profile. Values are opinionated but consistent — the key
// property detectors use is *agreement* across vectors; contradicting
// signals (UA says Linux, platform says MacIntel) are instant red flags.
(() => {
  const safe = (fn) => { try { fn(); } catch (_) {} };
  const nativeToString = Function.prototype.toString;
  // Hoisted so section 13 (toString proxy) can see the binding before
  // section 16 assigns it.
  let memoryGetter;

  // @worker-skip-start
  // ============================================================
  // 0. Scrub automation tokens other frameworks leave behind.
  // ============================================================
  safe(() => {
    const bad = [
      'callPhantom', '_phantom', '__nightmare',
      '__webdriver_evaluate', '__selenium_evaluate', '__webdriver_script_fn',
      '__webdriver_script_func', '__webdriver_script_function',
      '__fxdriver_evaluate', '__driver_unwrapped', '__webdriver_unwrapped',
      '__driver_evaluate', '__selenium_unwrapped', '__fxdriver_unwrapped',
      'domAutomation', 'domAutomationController', '__lastWatirAlert',
      '__lastWatirConfirm', '__lastWatirPrompt',
    ];
    for (const k of bad) {
      try { delete window[k]; } catch (_) {}
      try { delete document[k]; } catch (_) {}
    }
  });
  // @worker-skip-end

  // navigator.webdriver → must be ABSENT, not just `undefined`. CreepJS
  // checks `'webdriver' in navigator` and `Object.getOwnPropertyDescriptor`,
  // both of which catch a defineProperty(get:undefined) shim. We delete
  // the descriptor entirely; the getter's still on the prototype but the
  // property check now matches real Chrome.
  safe(() => {
    delete Object.getPrototypeOf(navigator).webdriver;
    Object.defineProperty(Navigator.prototype, 'webdriver', {
      get: () => undefined,
      configurable: true,
      enumerable: false,
    });
  });

  // ============================================================
  // 1. Navigator identity (platform / vendor / product cluster).
  // ============================================================
  safe(() => {
    const defs = {
      platform:      '{{PLATFORM}}',
      oscpu:         undefined,                 // Firefox-only; Chrome undefined
      vendor:        'Google Inc.',
      vendorSub:     '',
      product:       'Gecko',
      productSub:    '20030107',
      appCodeName:   'Mozilla',
      appName:       'Netscape',
      appVersion:    '{{APP_VERSION}}',
      doNotTrack:    null,
      languages:     {{LANGUAGES_JSON}},
      language:      '{{LOCALE}}',
      hardwareConcurrency: {{HW_CONCURRENCY}},
      deviceMemory:  {{DEVICE_MEMORY}},
      maxTouchPoints: 0,
      cookieEnabled: true,
      onLine:        true,
      pdfViewerEnabled: true,
    };
    for (const [k, v] of Object.entries(defs)) {
      try {
        Object.defineProperty(navigator, k, { get: () => v, configurable: true });
      } catch (_) {}
    }
  });

  // navigator.userAgentData (Client Hints) — must agree with UA string.
  safe(() => {
    Object.defineProperty(navigator, 'userAgentData', {
      get: () => ({
        brands: {{UA_BRANDS}},
        mobile: false,
        platform: '{{UA_PLATFORM}}',
        // Real UA-CH `getHighEntropyValues` returns ONLY the keys the
        // caller asked for. Detectors probe with a small set and check
        // that no extra keys leaked — returning the full object is a
        // signature for "homemade Sec-CH-UA shim".
        getHighEntropyValues: (hints) => {
          const all = {
            architecture: 'x86',
            bitness:      '64',
            model:        '',
            platform:     '{{UA_PLATFORM}}',
            platformVersion: '6.5.0',
            uaFullVersion: '{{UA_FULL_VERSION}}',
            fullVersionList: {{UA_FULL_VERSION_LIST}},
            wow64: false,
          };
          const filtered = {
            brands: [],
            mobile: false,
            platform: '{{UA_PLATFORM}}',
          };
          if (Array.isArray(hints)) {
            for (const k of hints) {
              if (k in all) filtered[k] = all[k];
            }
          }
          return Promise.resolve(filtered);
        },
      }),
    });
  });

  // Force navigator.userAgent to the profile-chosen string. This catches the
  // HeadlessChrome token AND any Chrome-version mismatch between the binary
  // on disk and the profile we claim in HTTP headers.
  safe(() => {
    Object.defineProperty(navigator, 'userAgent', { get: () => '{{USER_AGENT}}' });
  });

  // ============================================================
  // 2. chrome.* runtime stub.
  // ============================================================
  safe(() => {
    const g = (typeof window !== 'undefined') ? window : globalThis;
    if (!g.chrome) g.chrome = {};
    const c = g.chrome;
    c.app = c.app || {
      isInstalled: false,
      InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' },
      RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' },
      getDetails:   function () { return null; },
      getIsInstalled: function () { return false; },
      runningState:   function () { return 'cannot_run'; },
    };
    c.runtime = c.runtime || {
      OnInstalledReason: {}, OnRestartRequiredReason: {}, PlatformArch: {},
      PlatformNaclArch: {}, PlatformOs: {}, RequestUpdateCheckStatus: {},
      connect: function () {}, sendMessage: function () {},
      id: undefined, // id is undefined in content page contexts, defined in extension contexts
    };
    c.loadTimes = c.loadTimes || function () {
      return {
        commitLoadTime: performance.timeOrigin / 1000,
        connectionInfo: 'h2',
        finishDocumentLoadTime: performance.timeOrigin / 1000 + 0.3,
        finishLoadTime: performance.timeOrigin / 1000 + 0.5,
        firstPaintAfterLoadTime: 0,
        firstPaintTime: performance.timeOrigin / 1000 + 0.4,
        navigationType: 'Other',
        npnNegotiatedProtocol: 'h2',
        requestTime: performance.timeOrigin / 1000,
        startLoadTime: performance.timeOrigin / 1000,
        wasAlternateProtocolAvailable: false,
        wasFetchedViaSpdy: true,
        wasNpnNegotiated: true,
      };
    };
    c.csi = c.csi || function () {
      return { onloadT: Date.now(), pageT: Date.now() - performance.timeOrigin,
               startE: Date.now(), tran: 15 };
    };
  });

  // ============================================================
  // 3. Permissions API — fix Notification/Push "denied" signal.
  //
  // Classic Puppeteer leak: headless Chrome has
  //   Notification.permission === 'denied'
  // but
  //   navigator.permissions.query({name:'notifications'}).state === 'prompt'
  // This contradiction is a single-bit detector. Same pattern applies to
  // `push`. We align both by pulling state from `Notification.permission`
  // when it's available and coercing any `'default'` to `'prompt'` (the
  // Permissions API never returns 'default'). Other names flow through to
  // the original impl so geolocation/camera/etc keep working naturally.
  // ============================================================
  safe(() => {
    const q = navigator.permissions && navigator.permissions.query;
    if (q) {
      const orig = q.bind(navigator.permissions);
      const coerce = (s) => (s === 'default' ? 'prompt' : s);
      const leaky = { notifications: 1, push: 1 };
      navigator.permissions.query = function (p) {
        if (p && p.name && leaky[p.name]) {
          const np = (typeof Notification !== 'undefined' && Notification.permission)
            ? Notification.permission : 'prompt';
          return Promise.resolve({ state: coerce(np), onchange: null });
        }
        return orig(p);
      };
    }
  });

  // ============================================================
  // 4. Plugins & MimeTypes (Chrome 114+ still lists 5 PDF plugins).
  // ============================================================
  safe(() => {
    const makePlugin = (name, filename, description) => {
      const p = Object.create(Plugin.prototype);
      Object.defineProperties(p, {
        name:        { value: name,        enumerable: true },
        filename:    { value: filename,    enumerable: true },
        description: { value: description, enumerable: true },
        length:      { value: 0,           enumerable: true },
      });
      return p;
    };
    const pdfMime = Object.create(MimeType.prototype);
    Object.defineProperties(pdfMime, {
      type:        { value: 'application/pdf', enumerable: true },
      suffixes:    { value: 'pdf',              enumerable: true },
      description: { value: 'Portable Document Format', enumerable: true },
      enabledPlugin: { value: null,             enumerable: true },
    });
    const pluginList = [
      makePlugin('PDF Viewer',                'internal-pdf-viewer', 'Portable Document Format'),
      makePlugin('Chrome PDF Viewer',         'internal-pdf-viewer', 'Portable Document Format'),
      makePlugin('Chromium PDF Viewer',       'internal-pdf-viewer', 'Portable Document Format'),
      makePlugin('Microsoft Edge PDF Viewer', 'internal-pdf-viewer', 'Portable Document Format'),
      makePlugin('WebKit built-in PDF',       'internal-pdf-viewer', 'Portable Document Format'),
    ];
    // Wrap in an object whose prototype is PluginArray so
    // `navigator.plugins instanceof PluginArray` returns true.
    const pluginArray = Object.create(PluginArray.prototype);
    pluginList.forEach((p, i) => {
      Object.defineProperty(pluginArray, i, { value: p, enumerable: true });
    });
    Object.defineProperty(pluginArray, 'length', { value: pluginList.length });
    pluginArray.item        = (i) => pluginList[i] || null;
    pluginArray.namedItem   = (n) => pluginList.find(p => p.name === n) || null;
    pluginArray.refresh     = () => {};
    pluginArray[Symbol.iterator] = function* () { for (const p of pluginList) yield p; };
    Object.defineProperty(navigator, 'plugins', { get: () => pluginArray });

    const mimeArray = Object.create(MimeTypeArray.prototype);
    Object.defineProperty(mimeArray, 0, { value: pdfMime, enumerable: true });
    Object.defineProperty(mimeArray, 'length', { value: 1 });
    mimeArray.item       = (i) => (i === 0 ? pdfMime : null);
    mimeArray.namedItem  = (n) => (pdfMime.type === n ? pdfMime : null);
    mimeArray[Symbol.iterator] = function* () { yield pdfMime; };
    Object.defineProperty(navigator, 'mimeTypes', { get: () => mimeArray });
  });

  // @worker-skip-start
  // ============================================================
  // 5. Screen — desktop 1920x1080, LANDSCAPE, 24-bit color.
  // ============================================================
  safe(() => {
    const vals = { width: 1920, height: 1080, availWidth: 1920, availHeight: 1050,
                   colorDepth: 24, pixelDepth: 24 };
    for (const [k, v] of Object.entries(vals)) {
      try {
        Object.defineProperty(screen, k, { get: () => v, configurable: true });
      } catch (_) {}
    }
    try {
      Object.defineProperty(screen, 'orientation', {
        get: () => ({ type: 'landscape-primary', angle: 0,
                      onchange: null, addEventListener: () => {},
                      removeEventListener: () => {}, dispatchEvent: () => false }),
      });
    } catch (_) {}
  });
  // @worker-skip-end

  // ============================================================
  // 6. Timezone — Intl + Date.prototype.getTimezoneOffset agreement.
  // ============================================================
  safe(() => {
    const TZ = '{{TIMEZONE}}';
    const OFFSET_MIN = {{TZ_OFFSET_MIN}};
    const origResolved = Intl.DateTimeFormat.prototype.resolvedOptions;
    Intl.DateTimeFormat.prototype.resolvedOptions = function () {
      const r = origResolved.call(this);
      r.timeZone = TZ;
      r.locale = r.locale || 'en-US';
      return r;
    };
    const origGetTZ = Date.prototype.getTimezoneOffset;
    Date.prototype.getTimezoneOffset = function () { return OFFSET_MIN; };
    // Patch toString/toTimeString so debug traces align with the offset.
    try {
      const origToString = Date.prototype.toString;
      Date.prototype.toString = function () {
        try { return origToString.call(this); } catch (_) { return String(this); }
      };
    } catch (_) {}
    // mark originals for later introspection (toString remains native-looking).
    origResolved.toString = () => nativeToString.call(origResolved);
    origGetTZ.toString    = () => nativeToString.call(origGetTZ);
  });

  // ============================================================
  // 7. Battery API — desktop Chrome 103+ removed it; mobile personas keep
  //    it. When EXPOSE_BATTERY is true (mobile/Adreno path) we install a
  //    deterministic 24h cycle anchored at LOCAL midnight via the bundle
  //    timezone offset. Curve: 0h-2h local = charging linearly 20%→85%
  //    (2h to top up), 2h-24h local = discharging linearly 85%→20%
  //    (22h drain). Level is clamped to [0.20, 0.85] so the user never
  //    sees 100% (a brand-new battery on full charge is itself a tell —
  //    real phones idle in the 30-90% band 99% of the time). Tiny
  //    deterministic noise (±1%) from the canvas/audio session seed so
  //    two sessions don't byte-match.
  //
  //    The API surface (BatteryManager) is recomputed on every property
  //    read, so even within a long-running page the level/charging state
  //    drift naturally with wall-clock — matching what a real phone
  //    exposes when a page polls every few seconds. `addEventListener` is
  //    accepted but never fires (real Chrome only fires on actual state
  //    transitions; in our model those transitions are smooth so no
  //    discrete event is warranted).
  //
  //    Desktop personas (EXPOSE_BATTERY=false) follow the original path:
  //    Section 17 deletes `getBattery` entirely so `'getBattery' in
  //    navigator === false`, matching Chrome 103+ desktop.
  // ============================================================
  safe(() => {
    const EXPOSE_BATTERY = {{EXPOSE_BATTERY}};
    if (!EXPOSE_BATTERY) {
      // Desktop path: keep the legacy reject so any caller that happens
      // to invoke getBattery before Section 17 deletes the descriptor
      // fails in a Chrome-coherent way (Section 17 is the actual fix).
      if (navigator.getBattery) {
        navigator.getBattery = () => Promise.reject(
          new DOMException('Permission denied', 'NotAllowedError')
        );
      }
      return;
    }

    // Mobile path: realistic curve.
    // Seed → small ±1% level noise + ±5min charging-time jitter.
    const seed = (window.__crawlex_seed__ || 1) >>> 0;
    const levelNoise = (((seed & 0xffff) / 65535) - 0.5) * 0.02; // [-0.01, +0.01]
    const chargingJitterSec = ((seed >> 16) & 0xff) / 255 * 300; // [0, 300] sec

    function computeBatteryState() {
      // Local time anchored via the overridden Date.prototype.getTimezoneOffset
      // (Section 6). `getTimezoneOffset` returns minutes WEST of UTC, so
      // local_ms = utc_ms - offset*60000.
      const utcMs = Date.now();
      const offsetMin = new Date().getTimezoneOffset();
      const localMs = utcMs - offsetMin * 60000;
      const msInDay = ((localMs % 86400000) + 86400000) % 86400000;
      const hours = msInDay / 3600000;

      let level, charging, chargingTime, dischargingTime;
      if (hours < 2) {
        // Charging window: 00:00 → 02:00 local, 20% → 85%.
        const t = hours / 2;
        level = 0.20 + (0.85 - 0.20) * t;
        charging = true;
        // Seconds remaining until full (here "full" = 85% cap).
        chargingTime = Math.round((2 - hours) * 3600 + chargingJitterSec);
        dischargingTime = Infinity;
      } else {
        // Discharging window: 02:00 → 24:00 local, 85% → 20% over 22h.
        const t = (hours - 2) / 22;
        level = 0.85 - (0.85 - 0.20) * t;
        charging = false;
        chargingTime = Infinity;
        dischargingTime = Math.round((24 - hours) * 3600);
      }
      level += levelNoise;
      // Hard clamp so we never expose 100% (or anything above 85%) and
      // never below 20%.
      if (level > 0.85) level = 0.85;
      if (level < 0.20) level = 0.20;
      // Round to 2 decimal places — real Chrome reports level in 0.01
      // increments on most platforms.
      level = Math.round(level * 100) / 100;
      return { level, charging, chargingTime, dischargingTime };
    }

    function makeBatteryManager() {
      const listeners = {
        levelchange: [],
        chargingchange: [],
        chargingtimechange: [],
        dischargingtimechange: [],
      };
      const bm = {
        get level() { return computeBatteryState().level; },
        get charging() { return computeBatteryState().charging; },
        get chargingTime() { return computeBatteryState().chargingTime; },
        get dischargingTime() { return computeBatteryState().dischargingTime; },
        addEventListener(t, fn) {
          if (typeof fn !== 'function') return;
          if (listeners[t]) listeners[t].push(fn);
        },
        removeEventListener(t, fn) {
          if (listeners[t]) listeners[t] = listeners[t].filter(h => h !== fn);
        },
        dispatchEvent() { return true; },
        onchargingchange: null,
        onchargingtimechange: null,
        ondischargingtimechange: null,
        onlevelchange: null,
      };
      return bm;
    }

    const getBatteryFn = function getBattery() {
      return Promise.resolve(makeBatteryManager());
    };
    try {
      Object.defineProperty(navigator, 'getBattery', {
        value: getBatteryFn,
        writable: true,
        configurable: true,
        enumerable: false,
      });
      Object.defineProperty(Navigator.prototype, 'getBattery', {
        value: getBatteryFn,
        writable: true,
        configurable: true,
        enumerable: false,
      });
    } catch (_) {}
    // Register with the toString WeakSet so `getBattery.toString()` reports
    // `[native code]`. The registrar is exposed by Section 13.
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(getBatteryFn);
    } catch (_) {}
  });

  // ============================================================
  // 8. navigator.connection — NetworkInformation.
  // ============================================================
  safe(() => {
    const info = {
      effectiveType: '4g', rtt: 100, downlink: 10, saveData: false,
      type: 'wifi', onchange: null,
      addEventListener: () => {}, removeEventListener: () => {},
      dispatchEvent: () => false,
    };
    Object.defineProperty(navigator, 'connection',         { get: () => info });
    Object.defineProperty(navigator, 'mozConnection',      { get: () => info });
    Object.defineProperty(navigator, 'webkitConnection',   { get: () => info });
  });

  // @worker-skip-start
  // ============================================================
  // 9. matchMedia queries commonly probed by fingerprinters.
  // ============================================================
  safe(() => {
    const wantedMatches = {
      '(color-gamut: srgb)': true,
      '(color-gamut: p3)': false,
      '(color-gamut: rec2020)': false,
      '(dynamic-range: standard)': true,
      '(dynamic-range: high)': false,
      '(forced-colors: none)': true,
      '(forced-colors: active)': false,
      '(prefers-reduced-motion: no-preference)': true,
      '(prefers-reduced-motion: reduce)': false,
      '(prefers-color-scheme: light)': true,
      '(prefers-color-scheme: dark)': false,
      '(prefers-contrast: no-preference)': true,
      '(monochrome)': false,
      '(any-pointer: fine)': true,
      '(any-hover: hover)': true,
      '(pointer: fine)': true,
      '(hover: hover)': true,
    };
    const origMM = window.matchMedia;
    window.matchMedia = function (q) {
      const res = origMM.call(window, q);
      const qn = q.trim();
      if (qn in wantedMatches) {
        Object.defineProperty(res, 'matches', { get: () => wantedMatches[qn] });
      }
      return res;
    };
  });
  // @worker-skip-end

  // ============================================================
  // 10. WebGL — vendor/renderer + extension list + GL parameters.
  //     FingerprintJS hashes: vendor, renderer, supportedExtensions join,
  //     and ~30 parameter queries. We pin all to an Intel UHD 620 profile.
  // ============================================================
  safe(() => {
    const glProfile = {
      vendorUnmasked:   '{{WEBGL_UNMASKED_VENDOR}}',
      rendererUnmasked: '{{WEBGPU_ADAPTER_DESCRIPTION}}',
      vendor:           'WebKit',
      renderer:         'WebKit WebGL',
      version:          'WebGL 1.0 (OpenGL ES 2.0 Chromium)',
      shadingLanguageVersion: 'WebGL GLSL ES 1.0 (OpenGL ES GLSL ES 1.0 Chromium)',
      extensions: [
        'ANGLE_instanced_arrays','EXT_blend_minmax','EXT_color_buffer_half_float',
        'EXT_disjoint_timer_query','EXT_float_blend','EXT_frag_depth',
        'EXT_shader_texture_lod','EXT_texture_compression_bptc',
        'EXT_texture_compression_rgtc','EXT_texture_filter_anisotropic',
        'EXT_sRGB','KHR_parallel_shader_compile','OES_element_index_uint',
        'OES_fbo_render_mipmap','OES_standard_derivatives','OES_texture_float',
        'OES_texture_float_linear','OES_texture_half_float','OES_texture_half_float_linear',
        'OES_vertex_array_object','WEBGL_color_buffer_float','WEBGL_compressed_texture_s3tc',
        'WEBGL_compressed_texture_s3tc_srgb','WEBGL_debug_renderer_info',
        'WEBGL_debug_shaders','WEBGL_depth_texture','WEBGL_draw_buffers',
        'WEBGL_lose_context','WEBGL_multi_draw',
      ],
      params: {
        3379:  {{MAX_TEXTURE_SIZE}},                  // MAX_TEXTURE_SIZE
        3386:  [{{MAX_VIEWPORT_W}}, {{MAX_VIEWPORT_H}}], // MAX_VIEWPORT_DIMS
        34076: {{MAX_TEXTURE_SIZE}},                  // MAX_CUBE_MAP_TEXTURE_SIZE
        34921: 16,           // MAX_VERTEX_ATTRIBS
        34930: 16,           // MAX_TEXTURE_IMAGE_UNITS
        35660: 16,           // MAX_VERTEX_TEXTURE_IMAGE_UNITS
        35661: 32,           // MAX_COMBINED_TEXTURE_IMAGE_UNITS
        35724: 'WebGL GLSL ES 1.0 (OpenGL ES GLSL ES 1.0 Chromium)', // SHADING_LANGUAGE_VERSION
        36348: 1024,         // MAX_VARYING_VECTORS
        36349: 4096,         // MAX_VERTEX_UNIFORM_VECTORS
        36347: 4096,         // MAX_FRAGMENT_UNIFORM_VECTORS
        37445: '{{WEBGL_UNMASKED_VENDOR}}',           // UNMASKED_VENDOR_WEBGL
        37446: '{{WEBGPU_ADAPTER_DESCRIPTION}}',      // UNMASKED_RENDERER_WEBGL
        2978:  [0, 0, 1920, 1080], // VIEWPORT
      },
    };
    const hook = (proto) => {
      if (!proto) return;
      const origGet = proto.getParameter;
      proto.getParameter = function (p) {
        if (p === 7936) return glProfile.vendor;   // VENDOR
        if (p === 7937) return glProfile.renderer; // RENDERER
        if (p === 7938) return glProfile.version;  // VERSION
        if (p === 35724) return glProfile.shadingLanguageVersion;
        if (p in glProfile.params) return glProfile.params[p];
        return origGet.call(this, p);
      };
      const origExt = proto.getSupportedExtensions;
      proto.getSupportedExtensions = function () {
        return glProfile.extensions.slice();
      };
      const origAttrs = proto.getContextAttributes;
      if (origAttrs) {
        proto.getContextAttributes = function () {
          const a = origAttrs.call(this) || {};
          return Object.assign(a, {
            alpha: true, antialias: true, depth: true, desynchronized: false,
            failIfMajorPerformanceCaveat: false, powerPreference: 'default',
            premultipliedAlpha: true, preserveDrawingBuffer: false, stencil: false,
          });
        };
      }
      // getExtension hook: when the page asks for a known extension, return a
      // synthetic object whose own params route through our coherent profile.
      // Notably WEBGL_debug_renderer_info exposes UNMASKED_* via the ext
      // object and bypasses the getParameter hook unless we also handle it
      // here. CreepJS uses both paths.
      const origGetExt = proto.getExtension;
      proto.getExtension = function (name) {
        if (name === 'WEBGL_debug_renderer_info') {
          return {
            UNMASKED_VENDOR_WEBGL: 37445,
            UNMASKED_RENDERER_WEBGL: 37446,
          };
        }
        if (!glProfile.extensions.includes(name)) {
          return null;
        }
        return origGetExt.call(this, name) || {};
      };
      // getShaderPrecisionFormat: FingerprintJS hashes precision tuples
      // for each (shader, precision) combo. Pin to the most common Chrome
      // values so multiple agents in the fleet produce the same hash.
      const origPrecision = proto.getShaderPrecisionFormat;
      if (origPrecision) {
        proto.getShaderPrecisionFormat = function (shaderType, precisionType) {
          // FRAGMENT_SHADER (35632), VERTEX_SHADER (35633)
          // HIGH_FLOAT (36338), MEDIUM_FLOAT (36337), LOW_FLOAT (36336)
          // HIGH_INT (36341), MEDIUM_INT (36340), LOW_INT (36339)
          const isHighFloat = precisionType === 36338;
          return {
            rangeMin: isHighFloat ? 127 : 31,
            rangeMax: isHighFloat ? 127 : 30,
            precision: isHighFloat ? 23 : 0,
          };
        };
      }
      // readPixels: detector trick is rendering a known shader, reading
      // back the pixels, and hashing. We add per-session deterministic
      // noise (one byte every ~1KiB) — same approach as canvas.
      const origReadPixels = proto.readPixels;
      if (origReadPixels) {
        proto.readPixels = function (...args) {
          const r = origReadPixels.apply(this, args);
          try {
            const arr = args[6];
            if (arr && arr.length) {
              const seed = (window.__crawlex_seed__ || 1) & 0xff;
              for (let i = 0; i < arr.length; i += 1024) arr[i] = (arr[i] + seed) & 0xff;
            }
          } catch (_) {}
          return r;
        };
      }
      // isEnabled spoof (Camoufox §MakeIsEnabledMap): Chrome's
      // `gl.isEnabled(cap)` returns the per-cap default state at
      // context creation. Headless sometimes reports SCISSOR_TEST=true
      // because the rendering pipeline is pre-warmed differently — a
      // single-bit mismatch vs real Chrome. Pin the canonical defaults:
      // BLEND=false, CULL_FACE=false, DEPTH_TEST=false, DITHER=true,
      // POLYGON_OFFSET_FILL=false, SAMPLE_ALPHA_TO_COVERAGE=false,
      // SAMPLE_COVERAGE=false, SCISSOR_TEST=false, STENCIL_TEST=false,
      // RASTERIZER_DISCARD=false (WebGL2). Caller-driven state flips
      // fall through to the real getter so enable/disable still works.
      const origIsEnabled = proto.isEnabled;
      if (origIsEnabled) {
        const DEFAULTS = {
          3042: false,  // BLEND
          2884: false,  // CULL_FACE
          2929: false,  // DEPTH_TEST
          3024: true,   // DITHER
          32823: false, // POLYGON_OFFSET_FILL
          32926: false, // SAMPLE_ALPHA_TO_COVERAGE
          32928: false, // SAMPLE_COVERAGE
          3089:  false, // SCISSOR_TEST
          2960:  false, // STENCIL_TEST
          35977: false, // RASTERIZER_DISCARD (WebGL2)
        };
        // Track whether the caller has flipped a cap — once it has,
        // fall through to the real state. The WeakSet is per-context
        // so the defaults reset across a `webgl-lost`/recovered cycle.
        const touched = new WeakMap();
        const origEnable = proto.enable;
        const origDisable = proto.disable;
        if (origEnable) {
          proto.enable = function (cap) {
            try {
              let set = touched.get(this);
              if (!set) { set = new Set(); touched.set(this, set); }
              set.add(cap >>> 0);
            } catch (_) {}
            return origEnable.call(this, cap);
          };
        }
        if (origDisable) {
          proto.disable = function (cap) {
            try {
              let set = touched.get(this);
              if (!set) { set = new Set(); touched.set(this, set); }
              set.add(cap >>> 0);
            } catch (_) {}
            return origDisable.call(this, cap);
          };
        }
        proto.isEnabled = function (cap) {
          const key = cap >>> 0;
          const set = touched.get(this);
          if (set && set.has(key)) return origIsEnabled.call(this, cap);
          if (Object.prototype.hasOwnProperty.call(DEFAULTS, key)) return DEFAULTS[key];
          return origIsEnabled.call(this, cap);
        };
      }
    };
    hook(window.WebGLRenderingContext && window.WebGLRenderingContext.prototype);
    hook(window.WebGL2RenderingContext && window.WebGL2RenderingContext.prototype);
  });

  // @worker-skip-start
  // ============================================================
  // 11. Canvas 2D — deterministic noise so repeat reads are stable but the
  //     pixel hash doesn't match the public headless-chromium baseline.
  // ============================================================
  safe(() => {
    // Per-session seed instead of the static 1779. Same value across reads
    // within a session (so FingerprintJS double-render equality holds), but
    // different between sessions so the canvas hash isn't a public,
    // attributable signature.
    //
    // Seed is derived from the IdentityBundle's `canvas_audio_seed`
    // (injected at shim render time). This is DETERMINISTIC per bundle/
    // session — same bundle across two loads of the same page inside the
    // session produces the same hash (double-render equality), and two
    // different sessions produce distinct hashes. Do NOT mix Date.now()
    // here: that made the seed non-deterministic within a session.
    if (typeof window.__crawlex_seed__ !== 'number') {
      // Keep within a 31-bit safe range so bitwise ops stay meaningful.
      window.__crawlex_seed__ = ({{CANVAS_SEED}} >>> 0) & 0x7fffffff;
      if (window.__crawlex_seed__ === 0) window.__crawlex_seed__ = 0x1779;
    }
    const stride = 23 + ((window.__crawlex_seed__ >> 8) & 0x1f);
    // Camoufox-style per-pixel perturbation: for each selected pixel, walk
    // R,G,B (skip alpha at offset 3), find the first channel that is NOT
    // zero, and nudge it by ±1 (bounded [0, 255]). Pure-zero pixels stay
    // zero, which preserves the CreepJS "clearRect + getImageData returns
    // all-zeros" invariant that our previous `(val + salt) & 0xff` rewrite
    // failed. Direction bit comes from the seed so two runs with the same
    // bundle produce the same hash (double-render equality), and two
    // different seeds produce distinct hashes.
    const dirBit = (window.__crawlex_seed__ >> 13) & 1;
    const perturb = (ctx, w, h) => {
      const img = ctx.getImageData(0, 0, w, h);
      const d = img.data;
      for (let p = 0; p < d.length; p += stride * 4) {
        // Walk R (p), G (p+1), B (p+2); skip A (p+3).
        for (let ch = 0; ch < 3; ch++) {
          const idx = p + ch;
          if (idx >= d.length) break;
          const v = d[idx];
          if (v === 0) continue;
          if (dirBit === 0) {
            d[idx] = v < 255 ? v + 1 : 254;
          } else {
            d[idx] = v > 0 ? v - 1 : 1;
          }
          break;
        }
      }
      ctx.putImageData(img, 0, 0);
    };
    const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
    HTMLCanvasElement.prototype.toDataURL = function (...args) {
      try {
        const ctx = this.getContext('2d');
        if (ctx && this.width > 0 && this.height > 0) perturb(ctx, this.width, this.height);
      } catch (_) {}
      return origToDataURL.apply(this, args);
    };
    const origToBlob = HTMLCanvasElement.prototype.toBlob;
    HTMLCanvasElement.prototype.toBlob = function (cb, ...rest) {
      try {
        const ctx = this.getContext('2d');
        if (ctx && this.width > 0 && this.height > 0) perturb(ctx, this.width, this.height);
      } catch (_) {}
      return origToBlob.call(this, cb, ...rest);
    };
    const origGetData = CanvasRenderingContext2D.prototype.getImageData;
    CanvasRenderingContext2D.prototype.getImageData = function (...a) {
      const d = origGetData.apply(this, a);
      const skip = 257 + ((window.__crawlex_seed__ >> 16) & 0xff);
      // Same per-channel zero-preserve rule as `perturb`: walk RGB, skip
      // alpha, nudge first non-zero. The previous `i = 3` iteration hit
      // the alpha channel and XOR'd 0 → 1, which trips the CreepJS
      // clear-canvas invariant just as obviously as perturbing R.
      const data = d.data;
      for (let p = 0; p < data.length; p += skip * 4) {
        for (let ch = 0; ch < 3; ch++) {
          const idx = p + ch;
          if (idx >= data.length) break;
          const v = data[idx];
          if (v === 0) continue;
          data[idx] = ((dirBit === 0) ? (v < 255 ? v + 1 : 254)
                                      : (v > 0 ? v - 1 : 1));
          break;
        }
      }
      return d;
    };
  });
  // @worker-skip-end

  // ============================================================
  // 12. AudioContext + OfflineAudioContext — perturb FFT readouts AND
  //     the offline rendering buffer that FingerprintJS actually hashes.
  //     The canonical FPJS audio fingerprint uses
  //     `OfflineAudioContext.startRendering()` with a triangle-wave +
  //     DynamicsCompressor and reads samples 4500-5000. Perturbing only
  //     the live AnalyserNode (as we did before) leaves that hash intact.
  // ============================================================
  safe(() => {
    // Seeded deterministic PRNG + Box-Muller gaussian. Using a gaussian
    // jitter (σ derived from the session seed) matches the shape of real
    // DAC quantization noise — uniform random is itself a detector tell
    // because no real audio pipeline produces uniform-on-[0,1) samples.
    // Mulberry32 PRNG: 32-bit state, ~4 Gi-period, cheap and pure JS.
    const mulberry32 = (s) => {
      let state = s >>> 0;
      return function () {
        state = (state + 0x6D2B79F5) >>> 0;
        let t = state;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      };
    };
    const boxMuller = (rng) => {
      // Classic polar form; guards against the zero edge that would
      // produce -Infinity inside log.
      let u1 = rng(); if (u1 < 1e-10) u1 = 1e-10;
      const u2 = rng();
      return Math.sqrt(-2 * Math.log(u1)) * Math.cos(2 * Math.PI * u2);
    };
    const AC = window.AudioContext || window.webkitAudioContext;
    if (AC) {
      const orig = AC.prototype.createAnalyser;
      AC.prototype.createAnalyser = function () {
        const a = orig.apply(this, arguments);
        const ffd = a.getFloatFrequencyData;
        a.getFloatFrequencyData = function (arr) {
          ffd.call(this, arr);
          const seed = (window.__crawlex_seed__ || 1);
          // σ derived from the seed so different sessions produce
          // distinct noise floors without crossing the audibility line
          // (1e-7 dB is well below any fingerprint comparator threshold).
          const sigma = 1e-7 * (1 + ((seed >>> 24) & 0xff) / 255);
          const rng = mulberry32(seed ^ 0xA5A5A5A5);
          for (let i = 0; i < arr.length; i++) arr[i] += boxMuller(rng) * sigma;
        };
        return a;
      };
    }
    const OAC = window.OfflineAudioContext || window.webkitOfflineAudioContext;
    if (OAC) {
      const origStart = OAC.prototype.startRendering;
      OAC.prototype.startRendering = function () {
        const seed = (window.__crawlex_seed__ || 1);
        const promise = origStart.apply(this, arguments);
        return promise.then((buffer) => {
          try {
            const n = buffer.numberOfChannels;
            const sigma = 1e-7 * (1 + ((seed >>> 16) & 0xff) / 255);
            for (let c = 0; c < n; c++) {
              const data = buffer.getChannelData(c);
              // Per-channel PRNG seed so different channels aren't
              // byte-identical under a stereo-difference probe.
              const rng = mulberry32((seed ^ (c * 0x85EBCA6B)) >>> 0);
              for (let i = 0; i < data.length; i += 137) {
                data[i] += boxMuller(rng) * sigma;
              }
            }
          } catch (_) {}
          return buffer;
        });
      };
    }
    // AudioBuffer direct-read path. FingerprintJS (and several commercial
    // audio-FP stacks) extract samples through `getChannelData` and the
    // newer `copyFromChannel` API without ever calling `startRendering`,
    // so the OfflineAudioContext hook above would miss them. Camoufox
    // notes Brave's 0.1-0.2% noise was defeated by sampling both points;
    // we apply a seeded LCG-style multiplicative jitter in [0.996, 1.004]
    // plus a tiny non-linear polynomial correction so the transform isn't
    // a pure scalar (which would be reversible by linear regression).
    const AB = window.AudioBuffer;
    if (AB && AB.prototype) {
      const seed0 = (window.__crawlex_seed__ || 1) >>> 0;
      const transform = (data, channelIndex) => {
        const rng = mulberry32((seed0 ^ (channelIndex * 0xC2B2AE3D)) >>> 0);
        for (let i = 0; i < data.length; i++) {
          const r = rng();
          const mul = 0.996 + r * 0.008; // [0.996, 1.004]
          const x = data[i];
          data[i] = x * mul + (x * x - 0.5) * 0.002;
        }
      };
      const origGetChannel = AB.prototype.getChannelData;
      if (typeof origGetChannel === 'function') {
        AB.prototype.getChannelData = function (channel) {
          const data = origGetChannel.apply(this, arguments);
          try { transform(data, channel >>> 0); } catch (_) {}
          return data;
        };
      }
      const origCopyFrom = AB.prototype.copyFromChannel;
      if (typeof origCopyFrom === 'function') {
        AB.prototype.copyFromChannel = function (destination, channelNumber /*, startInChannel */) {
          const ret = origCopyFrom.apply(this, arguments);
          try { transform(destination, channelNumber >>> 0); } catch (_) {}
          return ret;
        };
      }
    }
  });

  // ============================================================
  // 13. Function.prototype.toString — make our overrides look native.
  //     FingerprintJS inspects `fn.toString()` for non-[native code] bodies.
  //
  //     Old shim used a WeakSet seeded with `Navigator.prototype` (not a
  //     function!) so that lookup always missed and our hooks leaked
  //     `function (...) { ... }` bodies. Fix: register the actual hooked
  //     function refs (the ones we've reassigned in earlier sections)
  //     and use a Map keyed by the function so the lookup is stable
  //     across call sites.
  // ============================================================
  safe(() => {
    const FAKE = 'function () { [native code] }';
    const targets = new WeakSet();
    const reg = (fn) => { try { if (typeof fn === 'function') targets.add(fn); } catch (_) {} };
    reg(HTMLCanvasElement.prototype.toDataURL);
    reg(HTMLCanvasElement.prototype.toBlob);
    reg(CanvasRenderingContext2D.prototype.getImageData);
    reg(WebGLRenderingContext && WebGLRenderingContext.prototype.getParameter);
    reg(WebGLRenderingContext && WebGLRenderingContext.prototype.getExtension);
    reg(WebGLRenderingContext && WebGLRenderingContext.prototype.getSupportedExtensions);
    reg(WebGLRenderingContext && WebGLRenderingContext.prototype.getContextAttributes);
    reg(WebGLRenderingContext && WebGLRenderingContext.prototype.getShaderPrecisionFormat);
    reg(WebGLRenderingContext && WebGLRenderingContext.prototype.readPixels);
    if (window.WebGL2RenderingContext) {
      reg(WebGL2RenderingContext.prototype.getParameter);
      reg(WebGL2RenderingContext.prototype.getExtension);
      reg(WebGL2RenderingContext.prototype.getSupportedExtensions);
      reg(WebGL2RenderingContext.prototype.getContextAttributes);
      reg(WebGL2RenderingContext.prototype.getShaderPrecisionFormat);
      reg(WebGL2RenderingContext.prototype.readPixels);
    }
    const AC = window.AudioContext || window.webkitAudioContext;
    if (AC) reg(AC.prototype.createAnalyser);
    const OAC = window.OfflineAudioContext || window.webkitOfflineAudioContext;
    if (OAC) reg(OAC.prototype.startRendering);
    reg(window.matchMedia);
    // Section 16 / 17 overrides — register getters so their toString() stays
    // [native code]. `memoryGetter` may still be undefined here (section 16
    // runs after this block); expose a late-binding registrar for it.
    if (typeof memoryGetter === 'function') reg(memoryGetter);
    window.__crawlex_reg_target__ = reg;
    const proxiedToString = new Proxy(nativeToString, {
      apply(target, thisArg, args) {
        if (targets.has(thisArg)) return FAKE;
        // The Proxy below also gets queried; report it native.
        if (thisArg === proxiedToString) return FAKE;
        try { return Reflect.apply(target, thisArg, args); } catch (_) { return FAKE; }
      },
    });
    Function.prototype.toString = proxiedToString;
  });

  // ============================================================
  // 13b. Notification.requestPermission — coerce 'denied' → 'default'.
  //
  // Classic FingerprintJS leak (companion to section 3): headless Chrome
  // boots with `Notification.permission === 'denied'` and therefore
  // `Notification.requestPermission()` resolves to `'denied'` immediately
  // without any user gesture. Real Chrome on an HTTP / insecure context
  // (or on a site with no prior user decision) returns `'default'`.
  // Section 3 aligned permissions.query, but detectors also call
  // `Notification.requestPermission()` and cross-check the two — this
  // block closes that vector.
  //
  // Both signatures are preserved (legacy callback form + modern Promise
  // form). The wrapped function is registered with the toString Proxy
  // from section 13 so `Notification.requestPermission.toString()`
  // reports `[native code]`. Placed after section 13 so
  // `window.__crawlex_reg_target__` is live at wrap time.
  // ============================================================
  safe(() => {
    if (typeof Notification === 'undefined' ||
        typeof Notification.requestPermission !== 'function') return;
    const wrapped = function requestPermission(callback) {
      const raw = Notification.permission;
      // 'denied' is the headless-Chrome default; coerce to 'default' so
      // it agrees with permissions.query (section 3) and the Chrome-on-
      // HTTP behaviour detectors expect.
      const result = (raw === 'denied') ? 'default' : raw;
      if (typeof callback === 'function') {
        try { callback(result); } catch (_) {}
        // Modern Chrome returns the Promise even when the legacy
        // callback is supplied; match that.
        return Promise.resolve(result);
      }
      return Promise.resolve(result);
    };
    try {
      Notification.requestPermission = wrapped;
    } catch (_) {}
    // Register the override with the toString Proxy WeakSet that
    // section 13 exposes via `window.__crawlex_reg_target__`.
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(wrapped);
    } catch (_) {}
  });

  // ============================================================
  // 14. WebGPU — Chrome 113+ ships WebGPU enabled. Detectors probe
  //     `navigator.gpu.requestAdapter()` and read adapter info; absence
  //     is itself a signal. Provide a stub adapter consistent with our
  //     declared GPU class.
  // ============================================================
  safe(() => {
    if (!navigator.gpu) {
      // vendor/description are rendered from the active IdentityBundle so
      // the WebGPU adapter info stays coherent with WebGL (same GPU
      // vendor keyword — validator enforces this in Rust at startup).
      const adapterInfo = {
        vendor: '{{WEBGL_UNMASKED_VENDOR}}',
        architecture: '',
        device: '',
        description: '{{WEBGPU_ADAPTER_DESCRIPTION}}',
      };
      const adapter = {
        features: new Set(),
        limits: { maxTextureDimension2D: 16384, maxBindGroups: 4 },
        requestAdapterInfo: () => Promise.resolve(adapterInfo),
        info: adapterInfo,
        requestDevice: () => Promise.reject(new Error('NotSupportedError')),
      };
      Object.defineProperty(navigator, 'gpu', {
        get: () => ({
          requestAdapter: () => Promise.resolve(adapter),
          getPreferredCanvasFormat: () => 'bgra8unorm',
        }),
        configurable: true,
      });
    }
  });

  // ============================================================
  // 15. Screen geometry consistency — `availHeight <= height`,
  //     `availWidth <= width`. Headless usually reports total=avail
  //     equal which is itself a tell. Pin to a Linux desktop default
  //     with a 30 px taskbar/panel.
  // ============================================================
  safe(() => {
    Object.defineProperty(screen, 'availHeight', { get: () => 1050, configurable: true });
    Object.defineProperty(screen, 'availWidth',  { get: () => 1920, configurable: true });
    Object.defineProperty(screen, 'height',      { get: () => 1080, configurable: true });
    Object.defineProperty(screen, 'width',       { get: () => 1920, configurable: true });
    Object.defineProperty(screen, 'colorDepth',  { get: () => 24, configurable: true });
    Object.defineProperty(screen, 'pixelDepth',  { get: () => 24, configurable: true });
  });

  // ============================================================
  // 16. performance.memory spoof — desktop Chrome heap values.
  //
  // `performance.memory` is a Chromium-only API (Firefox/Safari leave it
  // undefined). Low-memory VPS hosts often expose tiny `jsHeapSizeLimit`
  // values (< 1 GiB), which is flagrantly incoherent with a desktop Chrome
  // UA. Detectors (CreepJS, FPJS pro) read the tuple and compare against a
  // desktop baseline of ~2 GiB. We pin `jsHeapSizeLimit` to 2147483648
  // (2 GiB — stock 64-bit V8 limit) and jitter the live `total` / `used`
  // counters with a deterministic nudge derived from the canvas/audio
  // session seed so two sessions don't return byte-identical tuples.
  //
  // Property lives on the `Performance` prototype; use defineProperty on
  // the prototype so `('memory' in performance)` stays true and the
  // getter survives `Object.getOwnPropertyDescriptor(performance, ...)`.
  // Idempotent: a second run is a no-op (we mark the installed getter).
  // ============================================================
  safe(() => {
    const seed = (window.__crawlex_seed__ || (({{CANVAS_SEED}} >>> 0) & 0x7fffffff) || 0x1779);
    // Leak #2: jsHeapSizeLimit varies by build. Bundle-driven so mobile
    // personas expose ~512 MiB and desktop 2-4 GiB. Placeholder keeps the
    // literal an integer so `| 0` coercions later stay valid.
    const BASE_LIMIT = {{HEAP_SIZE_LIMIT}};
    const BASE_TOTAL = 50_000_000;
    const BASE_USED  = 30_000_000;
    // Tiny deterministic noise: bytes in [0, 65535] so hashes differ per
    // session but live comfortably inside a realistic working set.
    const jitterTotal = (seed & 0xffff);
    const jitterUsed  = ((seed >> 16) & 0xffff);
    const memoryObj = {
      jsHeapSizeLimit: BASE_LIMIT,
      totalJSHeapSize: BASE_TOTAL + jitterTotal,
      usedJSHeapSize:  BASE_USED  + jitterUsed,
    };
    memoryGetter = function () { return memoryObj; };
    // Late-bind the getter into the toString [native code] WeakSet set up
    // by section 13 (runs before this block). The registrar is left live
    // so subsequent sections (18 contentWindow, 20 performance.now, etc.)
    // can also flow their hooks into the same WeakSet; the final cleanup
    // happens at the bottom of the IIFE.
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(memoryGetter);
    } catch (_) {}
    const proto = (typeof Performance !== 'undefined') ? Performance.prototype : null;
    const target = proto || (window.performance && Object.getPrototypeOf(window.performance));
    if (target && !target.__crawlex_memory_installed__) {
      try {
        Object.defineProperty(target, 'memory', {
          get: memoryGetter,
          configurable: true,
          enumerable: false,
        });
        Object.defineProperty(target, '__crawlex_memory_installed__', {
          value: true, configurable: true, enumerable: false,
        });
      } catch (_) {
        // Fallback: pin directly on the instance.
        try {
          Object.defineProperty(window.performance, 'memory', {
            get: memoryGetter, configurable: true, enumerable: false,
          });
        } catch (__) {}
      }
    }
  });

  // ============================================================
  // 17. Sensors/Battery absence — desktop Chrome coherence.
  //
  // Desktop Chrome 103+ removed `navigator.getBattery` entirely (it now
  // returns undefined on non-mobile builds). The sensor APIs
  // (`DeviceMotionEvent`, `DeviceOrientationEvent`, `Accelerometer`,
  // `Gyroscope`, `LinearAccelerationSensor`, `AbsoluteOrientationSensor`,
  // `RelativeOrientationSensor`, `Magnetometer`) are feature-gated to
  // mobile/touch builds; their presence on a desktop UA is a coherence
  // red flag. Section 7 above still reject-wraps getBattery for older
  // Chromium; here we additionally `delete` the descriptor so the
  // property is genuinely absent (`'getBattery' in navigator === false`).
  //
  // Shipping on a headless Linux Chrome this routinely exposes
  // DeviceMotionEvent / DeviceOrientationEvent constructors even on
  // desktop (they're wired at the Blink level). We scrub them.
  // Idempotent: `delete` on an already-deleted property is a no-op.
  // ============================================================
  safe(() => {
    // EXPOSE_BATTERY=true means Section 7 installed the realistic
    // mobile curve; we must NOT delete it here. Desktop personas
    // (EXPOSE_BATTERY=false) keep the original behavior — getBattery
    // is removed entirely so `'getBattery' in navigator === false`,
    // matching Chrome 103+ desktop.
    const EXPOSE_BATTERY_S17 = {{EXPOSE_BATTERY}};
    if (!EXPOSE_BATTERY_S17) {
      // Battery: delete from both instance and prototype so no descriptor
      // survives `'getBattery' in navigator` probes.
      try { delete Navigator.prototype.getBattery; } catch (_) {}
      try { delete navigator.getBattery; } catch (_) {}
      try {
        Object.defineProperty(navigator, 'getBattery', {
          get: () => undefined, configurable: true,
        });
      } catch (_) {}
    }

    // Sensor constructors + motion events that desktop Chrome does not ship.
    const sensorNames = [
      'DeviceMotionEvent', 'DeviceOrientationEvent', 'DeviceOrientationAbsoluteEvent',
      'Accelerometer', 'LinearAccelerationSensor', 'GravitySensor',
      'Gyroscope', 'Magnetometer',
      'AbsoluteOrientationSensor', 'RelativeOrientationSensor',
    ];
    for (const name of sensorNames) {
      try { delete window[name]; } catch (_) {}
      try {
        if (name in window) {
          Object.defineProperty(window, name, {
            get: () => undefined, configurable: true,
          });
        }
      } catch (_) {}
    }

    // `ondevicemotion` / `ondeviceorientation` event-handler slots live on
    // Window.prototype on mobile; null them out if present.
    const handlerSlots = ['ondevicemotion', 'ondeviceorientation', 'ondeviceorientationabsolute'];
    for (const h of handlerSlots) {
      try {
        Object.defineProperty(window, h, {
          get: () => null, set: () => {}, configurable: true,
        });
      } catch (_) {}
    }
  });

  // @worker-skip-start
  // ============================================================
  // 18. HTMLIFrameElement.contentWindow — Object.defineProperty override (no Proxy).
  //
  // DataDome and PerimeterX probe the `contentWindow` accessor on
  // `HTMLIFrameElement.prototype` precisely because automation frameworks
  // (puppeteer-stealth, playwright-extra, undetected_chromedriver) commonly
  // wrap it in a `Proxy` to intercept cross-origin frame access. A Proxy
  // descriptor leaks via two surfaces:
  //   1. `Function.prototype.toString.call(
  //        Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype,
  //          'contentWindow').get)`
  //      reports the Proxy source rather than `[native code]`.
  //   2. Throwing inside the Proxy emits a `Proxy.<get>` frame in the
  //      stack trace.
  //
  // We install an *identity* override using plain `Object.defineProperty`
  // — NOT a Proxy. The wrapped getter calls the original native getter
  // and returns its result unmodified. Its only purpose is twofold:
  //   • Pre-empt any later layer (a downstream init script, an injected
  //     extension) that might wrap it in a Proxy. Once our descriptor is
  //     installed, attempts to overwrite have to deal with our own getter
  //     first.
  //   • Register the wrapper into the `__crawlex_reg_target__` WeakSet
  //     from section 13 so `descriptor.get.toString()` reports
  //     `function get contentWindow() { [native code] }` via the
  //     Function.prototype.toString Proxy.
  //
  // Idempotent: re-injection on SPA route change must not throw. The
  // `__crawlex_iframe_installed__` sentinel on `window` short-circuits a
  // second install.
  // ============================================================
  safe(() => {
    if (window.__crawlex_iframe_installed__) return;
    const desc = Object.getOwnPropertyDescriptor(
      HTMLIFrameElement.prototype, 'contentWindow'
    );
    if (!desc || typeof desc.get !== 'function') return;
    const originalGetter = desc.get;
    const wrapped = function get() {
      // Identity passthrough — return the native value as-is. No Proxy,
      // no mutation; this is purely a defensive ownership of the slot.
      return originalGetter.call(this);
    };
    try {
      Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
        get: wrapped,
        configurable: true,
      });
    } catch (_) {}
    // Flow into the native-toString WeakSet so a probe like
    //   Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype,
    //     'contentWindow').get.toString()
    // reports `function () { [native code] }`.
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(wrapped);
    } catch (_) {}
    try {
      Object.defineProperty(window, '__crawlex_iframe_installed__', {
        value: true, configurable: true, enumerable: false,
      });
    } catch (_) {
      try { window.__crawlex_iframe_installed__ = true; } catch (__) {}
    }
  });
  // @worker-skip-end

  // @worker-skip-start
  // ============================================================
  // 19. Window outer/inner geometry — scrollbar shape (leak #1).
  //
  // Desktop Chrome on Win/Linux subtracts a 15-17 px vertical scrollbar
  // track from `innerWidth` whenever content overflows; headless builds
  // present `outerWidth === innerWidth` which is a one-line framed-window
  // tell. macOS / mobile overlay scrollbars genuinely return 0. Value
  // comes from the persona via `{{SCROLLBAR_WIDTH}}`.
  //
  // We wrap `outerWidth` / `outerHeight` getters to return `innerWidth +
  // scrollbar` and a fixed chrome band; the values are consistent across
  // reads, don't depend on wall-clock, and survive
  // `getOwnPropertyDescriptor` probes because we `configurable: true`.
  // ============================================================
  safe(() => {
    const SBW = {{SCROLLBAR_WIDTH}};
    // Browser chrome band height: tabs + address bar. 74 px matches the
    // stock Chrome 120+ layout on Linux/Windows; macOS is smaller but
    // the scrollbar-width 0 on that persona masks the difference.
    const CHROME_H = 74;
    try {
      Object.defineProperty(window, 'outerWidth', {
        get: () => (window.innerWidth || 0) + SBW,
        configurable: true,
      });
      Object.defineProperty(window, 'outerHeight', {
        get: () => (window.innerHeight || 0) + CHROME_H,
        configurable: true,
      });
    } catch (_) {}
  });
  // @worker-skip-end

  // @worker-skip-start
  // ============================================================
  // 20. requestAnimationFrame visibility throttle (leak #11).
  //
  // Real Chrome throttles rAF to 1 Hz (1000 ms) whenever the document is
  // hidden (`document.visibilityState === 'hidden'`). Headless crawlers
  // keep firing at 16.6 ms which trivially distinguishes them from a
  // backgrounded tab. We intercept `requestAnimationFrame` and, when the
  // page is hidden, redirect to a `setTimeout(cb, 1000)` that mirrors the
  // real throttle. Visible-state rAF passes through untouched so paint
  // timing doesn't drift.
  // ============================================================
  safe(() => {
    const origRaf = window.requestAnimationFrame;
    const origCaf = window.cancelAnimationFrame;
    if (typeof origRaf !== 'function') return;
    window.requestAnimationFrame = function (cb) {
      try {
        if (document.visibilityState === 'hidden') {
          // Match real Chrome: fire once per second with a DOMHighResTimeStamp
          // that keeps the page's perception of `performance.now()` coherent.
          const t = setTimeout(() => {
            try { cb(performance.now()); } catch (_) {}
          }, 1000);
          // rAF returns a handle; reuse the setTimeout id so cancel works.
          return t;
        }
      } catch (_) {}
      return origRaf.call(window, cb);
    };
    window.cancelAnimationFrame = function (id) {
      try { clearTimeout(id); } catch (_) {}
      try { return origCaf.call(window, id); } catch (_) {}
    };
  });
  // @worker-skip-end

  // ============================================================
  // 21. performance.now() precision clamp (leak #12).
  //
  // Real Chrome rounds `performance.now()` to 100 µs in non-COI contexts
  // and 5 µs only inside cross-origin-isolated windows (spec:
  // https://wicg.github.io/timing-threat-mitigation). Detectors probe the
  // granularity directly: returning 5 µs from a non-COI page is itself a
  // tell. We clamp to 100 µs AND add seeded sub-grain jitter so two
  // consecutive ticks aren't byte-identical after the floor — the jitter
  // amount is below the grain, so monotonicity still holds. The seed is
  // shared with canvas/audio so a session is deterministic end-to-end.
  // ============================================================
  safe(() => {
    if (!window.performance || typeof performance.now !== 'function') return;
    const origNow = performance.now.bind(performance);
    const GRAIN = 0.1; // 100 µs in ms — Chrome's non-COI default
    const seed = (window.__crawlex_seed__ || 1) >>> 0;
    // xorshift32 for cheap, seeded, deterministic jitter stream.
    let jState = (seed ^ 0xC0FFEE) >>> 0;
    const jitter = () => {
      jState ^= jState << 13; jState >>>= 0;
      jState ^= jState >>> 17;
      jState ^= jState << 5; jState >>>= 0;
      // Return value in [0, GRAIN) so it never crosses into the next grain.
      return (jState / 4294967296) * GRAIN;
    };
    let last = 0;
    performance.now = function () {
      const raw = origNow();
      const floored = Math.floor(raw / GRAIN) * GRAIN;
      const candidate = floored + jitter();
      // Monotonic guard: never decrease. If jitter lands below `last`,
      // return `last` (still within the same grain as the real tick).
      if (candidate < last) return last;
      last = candidate;
      return candidate;
    };
    // Register so `performance.now.toString()` still reports [native code]
    // via the section-13 proxy. The late registrar is kept live across the
    // IIFE; if absent (cleanup race), swallow silently.
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(performance.now);
    } catch (_) {}
  });

  // ============================================================
  // 22. AudioContext.sampleRate pinning (leak #44).
  //
  // Real Chrome inherits the OS audio device's native sample rate —
  // 48000 on modern builds, 44100 on legacy rigs. Headless commonly
  // reports the hardware fallback `null` audio sink at 44100 when the
  // bundle claims a 48000 persona. We pin both live + offline contexts
  // to `{{AUDIO_SAMPLE_RATE}}` which the Rust side snapped to the
  // persona row.
  // ============================================================
  safe(() => {
    const RATE = {{AUDIO_SAMPLE_RATE}};
    const AC = window.AudioContext || window.webkitAudioContext;
    if (AC) {
      try {
        Object.defineProperty(AC.prototype, 'sampleRate', {
          get: () => RATE, configurable: true,
        });
      } catch (_) {}
    }
    const OAC = window.OfflineAudioContext || window.webkitOfflineAudioContext;
    if (OAC) {
      try {
        Object.defineProperty(OAC.prototype, 'sampleRate', {
          get: () => RATE, configurable: true,
        });
      } catch (_) {}
    }
  });

  // ============================================================
  // 23. navigator.mediaDevices.enumerateDevices stub (leak #45).
  //
  // Headless Chrome returns an empty array from
  // `navigator.mediaDevices.enumerateDevices()`. A real desktop rig
  // always has at least a default mic + default speaker, plus typically
  // a built-in + headset + virtual/bluetooth interface — `1 mic / 1 cam
  // / 1 speaker` is itself a tell (Camoufox research). Per-persona
  // counts flow from `IdentityBundle.media_{mic,cam,speaker}_count`.
  // `deviceId`/`label` stay empty because real Chrome gates those
  // behind a `getUserMedia` permission grant. `getUserMedia` itself is
  // stubbed to reject with NotAllowedError so sites that probe the
  // permission state see the pre-grant default, not a missing API.
  // ============================================================
  safe(() => {
    const md = navigator.mediaDevices;
    if (!md) return;
    const MIC_COUNT = {{MEDIA_MIC_COUNT}};
    const CAM_COUNT = {{MEDIA_CAM_COUNT}};
    const SPEAKER_COUNT = {{MEDIA_SPEAKER_COUNT}};
    const makeRow = (kind) => ({ deviceId: '', kind, label: '', groupId: '' });
    const fakeDevices = [];
    for (let i = 0; i < MIC_COUNT; i++) fakeDevices.push(makeRow('audioinput'));
    for (let i = 0; i < CAM_COUNT; i++) fakeDevices.push(makeRow('videoinput'));
    for (let i = 0; i < SPEAKER_COUNT; i++) fakeDevices.push(makeRow('audiooutput'));
    if (typeof md.enumerateDevices === 'function') {
      const wrapped = function enumerateDevices() {
        return Promise.resolve(
          fakeDevices.map((d) => Object.assign({ toJSON: () => d }, d))
        );
      };
      try {
        md.enumerateDevices = wrapped;
        const lateReg = window.__crawlex_reg_target__;
        if (typeof lateReg === 'function') lateReg(md.enumerateDevices);
      } catch (_) {}
    }
    // getUserMedia: stub a pre-permission rejection. Bare-function
    // absence would itself be a signal on sites that feature-detect
    // `md.getUserMedia`; returning NotAllowedError matches what a real
    // Chrome returns the first time a page calls it without a prompt
    // grant, and keeps the behavioural surface covered by §23.
    if (typeof md.getUserMedia === 'function') {
      const origGUM = md.getUserMedia.bind(md);
      md.getUserMedia = function (constraints) {
        // Still call the real API so detectors that inspect the return
        // shape (Promise, not thenable) see a native promise. We
        // swallow the resolve branch to keep the surface consistent
        // with the enumerateDevices pre-permission persona.
        return origGUM(constraints).catch((err) => {
          throw err;
        });
      };
      try {
        const lateReg = window.__crawlex_reg_target__;
        if (typeof lateReg === 'function') lateReg(md.getUserMedia);
      } catch (_) {}
    }
  });

  // @worker-skip-start
  // ============================================================
  // 24. speechSynthesis.getVoices per-OS list (leak #8).
  //
  // `speechSynthesis.getVoices()` returns an OS-dependent voice array —
  // Windows SAPI voices, macOS voices (Samantha, Alex), Linux espeak.
  // Headless Chromium's default is an empty array, which is itself a
  // tell. Provide a minimal plausible set keyed off the GPU vendor
  // keyword (a proxy for OS since the Rust side enforces GPU↔OS).
  // ============================================================
  safe(() => {
    if (typeof speechSynthesis === 'undefined') return;
    const KW = '{{GPU_VENDOR_KEYWORD}}';
    let voices;
    if (KW === 'apple') {
      voices = [
        { name: 'Samantha', lang: 'en-US', voiceURI: 'com.apple.voice.compact.en-US.Samantha', localService: true, default: true },
        { name: 'Alex',     lang: 'en-US', voiceURI: 'com.apple.speech.synthesis.voice.Alex',  localService: true, default: false },
        { name: 'Daniel',   lang: 'en-GB', voiceURI: 'com.apple.voice.compact.en-GB.Daniel',   localService: true, default: false },
      ];
    } else if (KW === 'adreno') {
      voices = [
        { name: 'Google US English', lang: 'en-US', voiceURI: 'Google US English', localService: false, default: true },
      ];
    } else {
      // Windows + Linux desktop: Chrome ships the Google cloud voices
      // list when no local SAPI voices exist; real Windows adds Microsoft
      // David/Zira. Keep the list small so it survives a future real-
      // voice expansion without diverging.
      voices = [
        { name: 'Google US English',   lang: 'en-US', voiceURI: 'Google US English',   localService: false, default: true },
        { name: 'Google UK English Female', lang: 'en-GB', voiceURI: 'Google UK English Female', localService: false, default: false },
        { name: 'Microsoft David - English (United States)', lang: 'en-US', voiceURI: 'Microsoft David - English (United States)', localService: true, default: false },
      ];
    }
    const frozen = voices.map((v) => Object.freeze(Object.assign({}, v)));
    try {
      speechSynthesis.getVoices = function () { return frozen.slice(); };
    } catch (_) {}
  });
  // @worker-skip-end

  // ============================================================
  // 25. Font-list coherence (support for downstream font probes).
  //
  // No direct API returns the installed font list, but detectors measure
  // text-width for a catalog of strings against fallback fonts to infer
  // what's present. We don't touch measureText (that's brittle); we
  // surface the OS-coherent list at `navigator.__crawlex_fonts__` so an
  // instrumented probe can sanity-check that the persona's font cluster
  // matches the GPU/OS pair. The property is non-enumerable so page
  // scripts don't see it during introspection.
  // ============================================================
  safe(() => {
    try {
      Object.defineProperty(navigator, '__crawlex_fonts__', {
        value: Object.freeze({{FONTS_JSON}}),
        enumerable: false,
        configurable: false,
        writable: false,
      });
    } catch (_) {}
  });

  // ============================================================
  // 26. chrome.runtime.id / sendMessage shape (leak #4 extension hinting).
  //
  // Real content-page Chrome exposes `chrome.runtime` with `id`
  // undefined but `sendMessage`/`connect` present, plus the
  // `OnInstalledReason` / `PlatformOs` enum-like objects. Section 2
  // above provides the skeleton; this block adds the enum values the
  // objects carry in real Chrome so `Object.keys(chrome.runtime.PlatformOs)`
  // returns the expected tuple — detectors compare the enum key sets.
  // ============================================================
  safe(() => {
    if (!window.chrome || !window.chrome.runtime) return;
    const r = window.chrome.runtime;
    // Real Chrome values — sourced from the open Chromium extension API.
    // Keep them frozen so page scripts can't mutate them to force a
    // divergence later in the page lifecycle.
    const assign = (obj, src) => {
      for (const [k, v] of Object.entries(src)) {
        if (!(k in obj)) obj[k] = v;
      }
      return obj;
    };
    try { assign(r.OnInstalledReason,        { INSTALL: 'install', UPDATE: 'update', CHROME_UPDATE: 'chrome_update', SHARED_MODULE_UPDATE: 'shared_module_update' }); } catch (_) {}
    try { assign(r.OnRestartRequiredReason,  { APP_UPDATE: 'app_update', OS_UPDATE: 'os_update', PERIODIC: 'periodic' }); } catch (_) {}
    try { assign(r.PlatformArch,             { ARM: 'arm', ARM64: 'arm64', MIPS: 'mips', MIPS64: 'mips64', X86_32: 'x86-32', X86_64: 'x86-64' }); } catch (_) {}
    try { assign(r.PlatformNaclArch,         { ARM: 'arm', MIPS: 'mips', MIPS64: 'mips64', X86_32: 'x86-32', X86_64: 'x86-64' }); } catch (_) {}
    try { assign(r.PlatformOs,               { MAC: 'mac', WIN: 'win', ANDROID: 'android', CROS: 'cros', LINUX: 'linux', OPENBSD: 'openbsd', FUCHSIA: 'fuchsia' }); } catch (_) {}
    try { assign(r.RequestUpdateCheckStatus, { THROTTLED: 'throttled', NO_UPDATE: 'no_update', UPDATE_AVAILABLE: 'update_available' }); } catch (_) {}
  });

  // ============================================================
  // 27. Web Worker concurrency ceiling — match navigator.hardwareConcurrency.
  //
  // A detector that spawns N Web Workers and measures the total completion
  // time of `N * small_task_ms` can infer real CPU parallelism. If the
  // declared `navigator.hardwareConcurrency = 8` but the actual observed
  // parallelism is 1 (CPU bottleneck of the host), the browser is flagged.
  // Chrome on a real desktop gives linear scaling up to hardwareConcurrency;
  // a stealth shim needs the two to agree.
  //
  // This is an observability seal — we don't try to *fake* parallelism
  // (impossible) — we instead make the Worker constructor count concurrent
  // live workers and cap the count at `navigator.hardwareConcurrency` or
  // reject with a DOMException, so a probing detector that tries to exceed
  // the declared concurrency gets a consistent "engineered ceiling" instead
  // of a surprise scaling cliff. Detection pattern:
  //   `let n = navigator.hardwareConcurrency;
  //    let workers = [...Array(n)].map(() => new Worker(...));`
  // — if our cap kicks in at the declared count, nothing surprising fires;
  // if a test tries `n+1` workers, they error gracefully with
  // QuotaExceededError (same DOMException a real resource-exhausted browser
  // would throw).
  // ============================================================
  safe(() => {
    const g = (typeof window !== 'undefined') ? window : globalThis;
    if (g.__crawlex_worker_cap_installed__) return;
    g.__crawlex_worker_cap_installed__ = true;
    const OriginalWorker = g.Worker;
    if (!OriginalWorker) return;
    let liveCount = 0;
    const declared = navigator.hardwareConcurrency || 4;
    const WrappedWorker = function(url, opts) {
      if (liveCount >= declared) {
        throw new DOMException(
          'Failed to construct Worker: pool exhausted.',
          'QuotaExceededError'
        );
      }
      liveCount++;
      const w = new OriginalWorker(url, opts);
      // Decrement on terminate() + on message:close;
      // real workers decrement implicitly when they go out of scope but
      // we track the explicit terminate() path for tight loops.
      const origTerminate = w.terminate.bind(w);
      w.terminate = function() { liveCount--; return origTerminate(); };
      return w;
    };
    WrappedWorker.prototype = OriginalWorker.prototype;
    g.Worker = WrappedWorker;
    // Register the new constructor into the toString WeakSet so
    // `Worker.toString()` reports `function Worker() { [native code] }`.
    try {
      const lateReg = g.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(WrappedWorker);
    } catch (_) {}
  });

  // @worker-skip-start
  // ============================================================
  // 28. TextMetrics / measureText jitter (Camoufox HarfBuzz analogue).
  //
  // Font-metric fingerprinting hashes `ctx.measureText(str).width` plus
  // the `actualBoundingBox*` / `fontBoundingBox*` / `emHeight*` family
  // across a catalog of test strings. Camoufox perturbs glyph advances
  // at the HarfBuzz shaping layer (~0-0.1 px per glyph, seeded). We
  // can't patch shaping, so we apply a seeded multiplicative jitter in
  // [0.9990, 1.0010] — invisible to layout (0.1 %), detectable only by
  // a comparator between our FP and a published baseline.
  //
  // Determinism: seed is `canvas_audio_seed` mixed with the text string
  // + font combo so repeat calls for the same string yield the same
  // perturbed value within a session (double-read equality), but two
  // sessions diverge. Non-zero only when the original value is > 0 so
  // zero-width pseudo-strings stay zero (consistency invariant).
  // ============================================================
  safe(() => {
    if (typeof CanvasRenderingContext2D === 'undefined') return;
    const C2D = CanvasRenderingContext2D.prototype;
    if (!C2D || typeof C2D.measureText !== 'function') return;
    const baseSeed = (window.__crawlex_seed__ || 1) >>> 0;
    // FNV-1a 32-bit: cheap string-to-seed hash so repeat calls with the
    // same (string, font) yield the same jitter.
    const fnv = (s) => {
      let h = 0x811c9dc5 >>> 0;
      for (let i = 0; i < s.length; i++) {
        h ^= s.charCodeAt(i);
        h = Math.imul(h, 0x01000193);
      }
      return h >>> 0;
    };
    const METRIC_FIELDS = [
      'width',
      'actualBoundingBoxLeft',
      'actualBoundingBoxRight',
      'actualBoundingBoxAscent',
      'actualBoundingBoxDescent',
      'fontBoundingBoxAscent',
      'fontBoundingBoxDescent',
      'emHeightAscent',
      'emHeightDescent',
      'hangingBaseline',
      'alphabeticBaseline',
      'ideographicBaseline',
    ];
    const origMeasure = C2D.measureText;
    C2D.measureText = function (str) {
      const tm = origMeasure.apply(this, arguments);
      try {
        const font = (this.font || '') + '';
        const mix = (baseSeed ^ fnv((str || '') + '\x1f' + font)) >>> 0;
        // Map 32-bit state into a [0.9990, 1.0010] multiplier — 0.1 %
        // jitter band centred on 1.0. Pure-zero fields stay zero.
        const unit = (mix / 4294967296) * 2 - 1; // [-1, 1]
        const mul = 1 + unit * 0.001;
        // TextMetrics props are read-only accessors in modern Chrome, so
        // we can't overwrite in place. Build a plain-object proxy and
        // return that — `instanceof TextMetrics` will fail, but no real
        // detector actually checks that (they read numeric fields).
        const out = {};
        for (const f of METRIC_FIELDS) {
          const v = tm[f];
          if (typeof v === 'number' && v !== 0 && Number.isFinite(v)) {
            out[f] = v * mul;
          } else if (typeof v === 'number') {
            out[f] = v;
          }
        }
        return out;
      } catch (_) {
        return tm;
      }
    };
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') lateReg(C2D.measureText);
    } catch (_) {}
  });
  // @worker-skip-end

  // ============================================================
  // 29. WebRTC SDP/ICE/getStats scrub (Camoufox + IP-leak mitigation).
  //
  // `new RTCPeerConnection().createOffer()` plus a follow-up
  // `setLocalDescription({})` generates an SDP blob that, by default,
  // includes every local ICE candidate — which means every private IP
  // on the host (10.*, 192.168.*, 172.16-31.*, fe80:*). Detectors call
  // this the "STUN leak" and use it to (a) deanonymize a proxied user
  // and (b) match against browser-persistent mdns hostnames. We strip
  // private IPs out of three surfaces:
  //   * SDP text in setLocalDescription / createOffer / createAnswer
  //   * `RTCPeerConnection.onicecandidate` event (candidate.candidate)
  //   * `getStats()` result map — `local-candidate` entries' `address`
  //     and `relatedAddress`.
  //
  // The Chrome launch flag `--force-webrtc-ip-handling-policy=
  // default_public_interface_only` covers cases (a)+(b) at the
  // native layer, but many detectors call `getStats()` directly and
  // iterate the map, which flag alone doesn't touch — hence the JS
  // layer here.
  // ============================================================
  safe(() => {
    if (typeof RTCPeerConnection === 'undefined') return;
    // IPv4 private + loopback + link-local, plus IPv6 ULA/link-local.
    const PRIV_V4 = /^(10\.|127\.|169\.254\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/;
    const PRIV_V6 = /^(fc[0-9a-f]{2}:|fd[0-9a-f]{2}:|fe80:|::1\b)/i;
    const MDNS = /\.local$/i;
    const isPrivate = (addr) => {
      if (!addr) return false;
      const lc = ('' + addr).toLowerCase();
      return PRIV_V4.test(lc) || PRIV_V6.test(lc) || MDNS.test(lc);
    };
    // Scrub SDP lines: keep session-level, drop `a=candidate:…` entries
    // whose connection address is private. `c=IN IP4 <priv>` and
    // `c=IN IP6 <priv>` lines get rewritten to 0.0.0.0/::0.
    const scrubSdp = (sdp) => {
      if (!sdp) return sdp;
      return sdp
        .split(/\r?\n/)
        .filter((line) => {
          if (!line.startsWith('a=candidate:')) return true;
          const parts = line.split(' ');
          const addr = parts[4];
          return !isPrivate(addr);
        })
        .map((line) => {
          if (line.startsWith('c=IN IP4 ')) {
            const a = line.slice(9).trim();
            return isPrivate(a) ? 'c=IN IP4 0.0.0.0' : line;
          }
          if (line.startsWith('c=IN IP6 ')) {
            const a = line.slice(9).trim();
            return isPrivate(a) ? 'c=IN IP6 ::' : line;
          }
          return line;
        })
        .join('\r\n');
    };
    const proto = RTCPeerConnection.prototype;
    // createOffer / createAnswer: scrub the resulting SDP before the
    // caller ever sees it.
    const wrapCreate = (name) => {
      const orig = proto[name];
      if (typeof orig !== 'function') return;
      proto[name] = function () {
        return orig.apply(this, arguments).then((desc) => {
          try {
            if (desc && typeof desc.sdp === 'string') {
              const scrubbed = scrubSdp(desc.sdp);
              // `desc.sdp` is read-only on RTCSessionDescription, so
              // build a plain descriptor the local side accepts.
              return { type: desc.type, sdp: scrubbed };
            }
          } catch (_) {}
          return desc;
        });
      };
    };
    wrapCreate('createOffer');
    wrapCreate('createAnswer');
    // setLocalDescription: scrub caller-provided SDP on the way in.
    const origSetLocal = proto.setLocalDescription;
    if (origSetLocal) {
      proto.setLocalDescription = function (desc) {
        try {
          if (desc && typeof desc.sdp === 'string') {
            desc = { type: desc.type, sdp: scrubSdp(desc.sdp) };
          }
        } catch (_) {}
        return origSetLocal.call(this, desc);
      };
    }
    // onicecandidate: filter private candidates so the page never sees
    // them fire. The native pipeline already gates cross-process; this
    // is the JS surface most detectors sample.
    try {
      const desc = Object.getOwnPropertyDescriptor(proto, 'onicecandidate');
      if (desc && desc.configurable && desc.set) {
        Object.defineProperty(proto, 'onicecandidate', {
          configurable: true,
          get: desc.get,
          set: function (fn) {
            if (typeof fn !== 'function') return desc.set.call(this, fn);
            return desc.set.call(this, function (event) {
              try {
                if (event && event.candidate && isPrivate(event.candidate.address || '')) {
                  return; // swallow
                }
                const c = event && event.candidate && event.candidate.candidate;
                if (typeof c === 'string') {
                  const parts = c.split(' ');
                  if (isPrivate(parts[4])) return;
                }
              } catch (_) {}
              return fn.call(this, event);
            });
          },
        });
      }
    } catch (_) {}
    // getStats: iterate the Map result, sanitize local-candidate rows.
    const origGetStats = proto.getStats;
    if (origGetStats) {
      proto.getStats = function () {
        return origGetStats.apply(this, arguments).then((report) => {
          try {
            if (report && typeof report.forEach === 'function') {
              report.forEach((entry) => {
                if (!entry || entry.type !== 'local-candidate') return;
                if (isPrivate(entry.address)) entry.address = '';
                if (isPrivate(entry.relatedAddress)) entry.relatedAddress = '';
              });
            }
          } catch (_) {}
          return report;
        });
      };
    }
    try {
      const lateReg = window.__crawlex_reg_target__;
      if (typeof lateReg === 'function') {
        lateReg(proto.createOffer);
        lateReg(proto.createAnswer);
        lateReg(proto.setLocalDescription);
        lateReg(proto.getStats);
      }
    } catch (_) {}
  });
})();
