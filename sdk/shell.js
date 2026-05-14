'use strict';

// crawlex shell (slice 23) — Node REPL with crawlex SDK preloaded.
//
// Mirrors the Rust shell (slice 22) helpers: `fetch`, `css`, `xpath`,
// `findByText`, `findByRegex`, `save`. Implementation is intentionally
// minimal — pure Node, no extra deps:
//   * fetch: node:http(s) GET
//   * selectors: regex-based subset (tag / tag#id / tag.class, //tag)
//   * adaptive save: JSON file under $XDG_DATA_HOME/crawlex
//
// Two layers (mirrors the Rust shell):
//   * `createShellApi` is the test surface — returns the helper object
//     and shared state without spinning up a REPL. Tests inject a stub
//     fetcher and assert on the state mutations.
//   * `startRepl` wires the API into `node:repl` with history.

const repl = require('node:repl');
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const http = require('node:http');
const https = require('node:https');
const { URL } = require('node:url');

function xdgDataHome() {
  return process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share');
}

function defaultHistoryPath() {
  return path.join(xdgDataHome(), 'crawlex', 'shell_history_node');
}

function defaultStorePath() {
  return path.join(xdgDataHome(), 'crawlex', 'adaptive_store.json');
}

function httpFetch(url) {
  const u = new URL(url);
  const mod = u.protocol === 'https:' ? https : http;
  return new Promise((resolve, reject) => {
    const req = mod.request(
      {
        method: 'GET',
        hostname: u.hostname,
        port: u.port || (u.protocol === 'https:' ? 443 : 80),
        path: u.pathname + u.search,
        headers: { 'user-agent': 'crawlex-shell' },
      },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          resolve({
            finalUrl: url,
            status: res.statusCode || 0,
            headers: res.headers,
            body: Buffer.concat(chunks).toString('utf8'),
          });
        });
      },
    );
    req.on('error', reject);
    req.end();
  });
}

function parseSimpleSelector(sel) {
  // Supports a minimal subset: tag, #id, .class, tag#id, tag.class.
  // Anything richer (descendant combinators, attribute selectors) is
  // out-of-scope for the JS shell — the Rust shell remains the heavy
  // engine.
  const m = /^([a-zA-Z*][a-zA-Z0-9-]*)?(?:#([a-zA-Z0-9_-]+))?(?:\.([a-zA-Z0-9_-]+))?$/.exec(
    sel.trim(),
  );
  if (!m || (!m[1] && !m[2] && !m[3])) return null;
  return { tag: m[1] || '*', id: m[2] || null, class: m[3] || null };
}

function parseAttrs(s) {
  const out = {};
  const re = /\b([a-zA-Z][a-zA-Z0-9-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/g;
  let m;
  while ((m = re.exec(s))) {
    out[m[1]] = m[2] !== undefined ? m[2] : m[3] !== undefined ? m[3] : m[4];
  }
  return out;
}

const VOID_TAGS = new Set([
  'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input',
  'link', 'meta', 'param', 'source', 'track', 'wbr',
]);

// Walk every element in the document, including nested ones, yielding
// `{ tag, attrs, inner, outer, index }`. The regex iterator approach
// (matchAll on `<x>...</x>`) only catches top-level matches because the
// engine advances past each match. Here we re-scan from every `<tag>`
// open and resolve the matching `</tag>` with a depth counter so nested
// elements with the same tag name (e.g. `<div>` inside `<div>`) close
// correctly. O(n*k) for k opens — fine for shell-sized pages.
function* elementIter(html) {
  const openRe = /<([a-zA-Z][a-zA-Z0-9-]*)\b([^>]*)>/g;
  let m;
  while ((m = openRe.exec(html))) {
    const tag = m[1];
    const attrs = m[2];
    if (VOID_TAGS.has(tag.toLowerCase())) continue;
    if (/\/\s*$/.test(attrs)) continue; // self-closing like <br/>
    const startInner = m.index + m[0].length;
    const close = findMatchingClose(html, tag, startInner);
    if (close < 0) continue;
    yield {
      0: html.slice(m.index, close + tag.length + 3),
      1: tag,
      2: attrs,
      3: html.slice(startInner, close),
      index: m.index,
    };
  }
}

function findMatchingClose(html, tag, from) {
  const openRe = new RegExp(`<${tag}\\b[^>]*>`, 'gi');
  const closeRe = new RegExp(`</${tag}\\s*>`, 'gi');
  let depth = 1;
  let pos = from;
  while (depth > 0) {
    openRe.lastIndex = pos;
    closeRe.lastIndex = pos;
    const o = openRe.exec(html);
    const c = closeRe.exec(html);
    if (!c) return -1;
    if (o && !/\/\s*$/.test(o[0].slice(1, -1)) && o.index < c.index) {
      depth += 1;
      pos = o.index + o[0].length;
    } else {
      depth -= 1;
      if (depth === 0) return c.index;
      pos = c.index + c[0].length;
    }
  }
  return -1;
}

function textOf(html) {
  return html
    .replace(/<script[\s\S]*?<\/script>/gi, ' ')
    .replace(/<style[\s\S]*?<\/style>/gi, ' ')
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function cssQuery(html, selector) {
  const parsed = parseSimpleSelector(selector);
  if (!parsed) {
    throw new Error(
      `unsupported selector '${selector}'. JS shell supports tag, #id, .class, tag#id, tag.class.`,
    );
  }
  const out = [];
  for (const m of elementIter(html)) {
    const tag = m[1];
    if (parsed.tag !== '*' && tag.toLowerCase() !== parsed.tag.toLowerCase()) continue;
    const attrs = parseAttrs(m[2]);
    if (parsed.id && attrs.id !== parsed.id) continue;
    if (parsed.class) {
      const classes = (attrs.class || '').split(/\s+/);
      if (!classes.includes(parsed.class)) continue;
    }
    out.push({ tag, attrs, inner: m[3], outer: m[0], text: textOf(m[3]) });
  }
  return out;
}

function xpathQuery(html, expr) {
  // Supports `//tag` only. Anything richer goes to the Rust shell.
  const m = /^\/\/([a-zA-Z][a-zA-Z0-9-]*)$/.exec(expr.trim());
  if (!m) {
    throw new Error(`unsupported xpath '${expr}'. JS shell supports //tag only.`);
  }
  return cssQuery(html, m[1]);
}

function findByText(html, needle) {
  if (typeof needle !== 'string' || needle.length === 0) {
    throw new TypeError('findByText: needle must be a non-empty string');
  }
  const out = [];
  for (const m of elementIter(html)) {
    const text = textOf(m[3]);
    if (text.includes(needle)) {
      out.push({ tag: m[1], attrs: parseAttrs(m[2]), inner: m[3], outer: m[0], text });
    }
  }
  return out;
}

function findByRegex(html, pattern) {
  const re = pattern instanceof RegExp ? pattern : new RegExp(pattern);
  const out = [];
  for (const m of elementIter(html)) {
    const text = textOf(m[3]);
    if (re.test(text)) {
      out.push({ tag: m[1], attrs: parseAttrs(m[2]), inner: m[3], outer: m[0], text });
    }
  }
  return out;
}

class Page {
  constructor(state, resp) {
    this._state = state;
    this.url = resp.finalUrl;
    this.status = resp.status;
    this.headers = resp.headers;
    this.body = resp.body;
  }
  _record(result, query) {
    this._state.lastSelection = result[0] || null;
    this._state.lastSelectionQuery = query;
    this._state.lastSelectionUrl = this.url;
    return result;
  }
  css(selector) {
    return this._record(cssQuery(this.body, selector), `css:${selector}`);
  }
  xpath(expr) {
    return this._record(xpathQuery(this.body, expr), `xpath:${expr}`);
  }
  findByText(needle) {
    return this._record(findByText(this.body, needle), `text:${needle}`);
  }
  findByRegex(pattern) {
    const label = pattern instanceof RegExp ? pattern.source : String(pattern);
    return this._record(findByRegex(this.body, pattern), `regex:${label}`);
  }
  save(identifier) {
    if (typeof identifier !== 'string' || !identifier) {
      throw new TypeError('save: identifier must be a non-empty string');
    }
    if (!this._state.lastSelection) {
      throw new Error('save: no element selected. Run css/xpath/findByText first.');
    }
    const host = new URL(this.url).host;
    const storePath = this._state.storePath;
    let store = {};
    try {
      store = JSON.parse(fs.readFileSync(storePath, 'utf8'));
    } catch (_) {
      store = {};
    }
    const entry = {
      query: this._state.lastSelectionQuery,
      tag: this._state.lastSelection.tag,
      attrs: this._state.lastSelection.attrs,
      saved_at: new Date().toISOString(),
    };
    store[host] = store[host] || {};
    store[host][identifier] = entry;
    fs.mkdirSync(path.dirname(storePath), { recursive: true });
    fs.writeFileSync(storePath, JSON.stringify(store, null, 2));
    return entry;
  }
}

function createShellApi(opts = {}) {
  const fetcher = opts.fetcher || httpFetch;
  const state = {
    last: null,
    lastSelection: null,
    lastSelectionQuery: null,
    lastSelectionUrl: null,
    storePath: opts.storePath || defaultStorePath(),
  };
  const crawlex = {
    async fetch(url) {
      if (typeof url !== 'string' || !url) {
        throw new TypeError('crawlex.fetch: url must be a non-empty string');
      }
      const resp = await fetcher(url);
      const page = new Page(state, resp);
      state.last = page;
      return page;
    },
    css(sel) { return requireLast(state).css(sel); },
    xpath(e) { return requireLast(state).xpath(e); },
    findByText(t) { return requireLast(state).findByText(t); },
    findByRegex(p) { return requireLast(state).findByRegex(p); },
    save(id) { return requireLast(state).save(id); },
    get last() { return state.last; },
  };
  return { crawlex, state };
}

function requireLast(state) {
  if (!state.last) {
    throw new Error('no page fetched yet — call crawlex.fetch(url) first.');
  }
  return state.last;
}

function startRepl(opts = {}) {
  const stdin = opts.stdin || process.stdin;
  const stdout = opts.stdout || process.stdout;
  const historyPath = opts.historyPath || defaultHistoryPath();
  const { crawlex } = createShellApi({ storePath: opts.storePath });
  stdout.write(`crawlex shell — node ${process.version}\n`);
  stdout.write(
    `crawlex globals: fetch(url), css(sel), xpath(expr), findByText(s), findByRegex(re), save(id), last\n`,
  );
  stdout.write(`type .exit to quit.\n`);
  const server = repl.start({
    prompt: 'crawlex> ',
    input: stdin,
    output: stdout,
    useColors: stdout.isTTY === true,
  });
  server.context.crawlex = crawlex;
  Object.defineProperty(server.context, 'last', {
    get: () => crawlex.last,
    configurable: true,
  });
  try {
    fs.mkdirSync(path.dirname(historyPath), { recursive: true });
    server.setupHistory(historyPath, () => {});
  } catch (_) {
    // History is best-effort — non-fatal if the data dir is unwritable.
  }
  return server;
}

module.exports = {
  createShellApi,
  startRepl,
  Page,
  cssQuery,
  xpathQuery,
  findByText,
  findByRegex,
  defaultHistoryPath,
  defaultStorePath,
};
