// Slice 32 — calibration probe. Runs inside the live external CDP page
// served by the local `__crawlex_calibrate` origin. Returns a JSON
// string so the host side can `serde_json::from_str` it without
// dealing with chromiumoxide's Value-shape discovery quirks.
//
// Best-effort: every individual capture is wrapped in a try/catch so a
// missing API on a stripped browser cannot abort the whole probe.
(async () => {
  const safe = async (fn, fb) => { try { return await fn(); } catch (e) { return fb; } };
  const nav = navigator || {};
  const scr = screen || {};
  const win = window || {};
  const intl = (Intl && Intl.DateTimeFormat) ? new Intl.DateTimeFormat().resolvedOptions() : {};

  const webgl = await safe(async () => {
    const c = document.createElement('canvas');
    const gl = c.getContext('webgl') || c.getContext('experimental-webgl');
    if (!gl) return null;
    const dbg = gl.getExtension('WEBGL_debug_renderer_info');
    return {
      vendor: String(gl.getParameter(gl.VENDOR) || ''),
      renderer: String(gl.getParameter(gl.RENDERER) || ''),
      unmasked_vendor: String(dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) || '' : ''),
      unmasked_renderer: String(dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) || '' : ''),
    };
  }, null);

  const canvas_hash = await safe(async () => {
    const c = document.createElement('canvas');
    c.width = 200; c.height = 50;
    const ctx = c.getContext('2d');
    ctx.textBaseline = 'top';
    ctx.font = '14px Arial';
    ctx.fillStyle = '#069';
    ctx.fillText('crawlex-calibrate', 2, 2);
    const data = c.toDataURL();
    let h = 0n;
    for (let i = 0; i < data.length; i++) h = (h * 31n + BigInt(data.charCodeAt(i))) & 0xffffffffffffffffn;
    return h.toString(16);
  }, '');

  const audio_hash = await safe(async () => {
    const Ctx = win.OfflineAudioContext || win.webkitOfflineAudioContext;
    if (!Ctx) return '';
    const ctx = new Ctx(1, 1024, 44100);
    const osc = ctx.createOscillator();
    osc.type = 'triangle';
    osc.frequency.value = 1000;
    osc.connect(ctx.destination);
    osc.start(0);
    const buf = await ctx.startRendering();
    const arr = buf.getChannelData(0);
    let s = 0;
    for (let i = 0; i < arr.length; i++) s += Math.abs(arr[i]);
    return s.toFixed(8);
  }, '');

  const storage_quota = await safe(async () => {
    if (nav.storage && nav.storage.estimate) {
      const e = await nav.storage.estimate();
      return Math.floor(Number(e.quota || 0));
    }
    return 0;
  }, 0);

  const media_devices = await safe(async () => {
    if (nav.mediaDevices && nav.mediaDevices.enumerateDevices) {
      const list = await nav.mediaDevices.enumerateDevices();
      return list.map(d => `${d.kind}:${d.label || ''}`);
    }
    return [];
  }, []);

  const webrtc = await safe(async () => {
    const out = { ipv4: [], ipv6: [] };
    if (!win.RTCPeerConnection) return out;
    const pc = new RTCPeerConnection({ iceServers: [] });
    pc.createDataChannel('crawlex');
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    const seen = new Set();
    await new Promise(r => {
      const t = setTimeout(r, 350);
      pc.onicecandidate = (ev) => {
        if (!ev.candidate) { clearTimeout(t); r(); return; }
        const m = ev.candidate.candidate.match(/(\d+\.\d+\.\d+\.\d+|[0-9a-fA-F:]+)/g) || [];
        for (const c of m) {
          if (seen.has(c)) continue; seen.add(c);
          if (c.includes(':')) out.ipv6.push(c); else if (c.match(/^\d+\.\d+\.\d+\.\d+$/)) out.ipv4.push(c);
        }
      };
    });
    try { pc.close(); } catch {}
    return out;
  }, { ipv4: [], ipv6: [] });

  const permissions = await safe(async () => {
    if (!nav.permissions || !nav.permissions.query) return [];
    const names = ['geolocation', 'notifications', 'camera', 'microphone', 'midi', 'clipboard-read'];
    const out = [];
    for (const name of names) {
      try {
        const r = await nav.permissions.query({ name });
        out.push({ name, state: String(r.state || '') });
      } catch (e) {
        out.push({ name, state: 'unsupported' });
      }
    }
    return out;
  }, []);

  const plugins = await safe(async () => {
    const list = nav.plugins || [];
    const out = [];
    for (let i = 0; i < list.length; i++) out.push(String(list[i].name || ''));
    return out;
  }, []);

  const performance_memory = await safe(async () => {
    const m = win.performance && win.performance.memory;
    if (!m) return null;
    return {
      js_heap_size_limit: Math.floor(Number(m.jsHeapSizeLimit || 0)),
      total_js_heap_size: Math.floor(Number(m.totalJSHeapSize || 0)),
      used_js_heap_size: Math.floor(Number(m.usedJSHeapSize || 0)),
    };
  }, null);

  const webgpu_adapter = await safe(async () => {
    if (!nav.gpu || !nav.gpu.requestAdapter) return null;
    const a = await nav.gpu.requestAdapter();
    if (!a) return null;
    const info = a.info || (a.requestAdapterInfo ? await a.requestAdapterInfo() : {});
    return `${info.vendor || ''}/${info.architecture || ''}/${info.device || ''}`;
  }, null);

  const ua_data = nav.userAgentData || {};
  const product = ua_data.brands && ua_data.brands.length
    ? (ua_data.brands.find(b => /chrome|chromium/i.test(b.brand)) || ua_data.brands[0]).brand
    : (nav.product || '');

  return JSON.stringify({
    browser_product: String(product || ''),
    browser_version: String((ua_data.brands && ua_data.brands[0] && ua_data.brands[0].version) || ''),
    platform: String(ua_data.platform || nav.platform || ''),
    user_agent: String(nav.userAgent || ''),
    locale: String(intl.locale || nav.language || ''),
    timezone: String(intl.timeZone || ''),
    screen: {
      width: Number(scr.width || 0) | 0,
      height: Number(scr.height || 0) | 0,
      avail_width: Number(scr.availWidth || 0) | 0,
      avail_height: Number(scr.availHeight || 0) | 0,
      color_depth: Number(scr.colorDepth || 0) | 0,
      pixel_ratio: Number(win.devicePixelRatio || 1),
    },
    window: {
      inner_width: Number(win.innerWidth || 0) | 0,
      inner_height: Number(win.innerHeight || 0) | 0,
      outer_width: Number(win.outerWidth || 0) | 0,
      outer_height: Number(win.outerHeight || 0) | 0,
    },
    webgl: webgl || { vendor: '', renderer: '', unmasked_vendor: '', unmasked_renderer: '' },
    canvas_hash,
    audio_hash,
    storage_quota,
    media_devices,
    webrtc,
    permissions,
    plugins,
    has_window_chrome: typeof win.chrome === 'object' && win.chrome !== null,
    performance_memory,
    webgpu_adapter,
    mismatch_count: 0,
    policy: 'report-only'
  });
})()
