'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { defineSpider, runSpider, Request } = require('../crawlex-sdk.js');

function startFixture() {
  // Tiny site:
  //   /         -> "<a href=/a>a</a> <a href=/b>b</a>"
  //   /a, /b    -> "leaf:<name>"
  //   /blocked  -> "secret"
  //   /robots.txt -> "User-agent: *\nDisallow: /blocked\n"
  const server = http.createServer((req, res) => {
    const url = req.url;
    if (url === '/') {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<a href="/a">a</a><a href="/b">b</a>');
    } else if (url === '/a' || url === '/b') {
      res.writeHead(200);
      res.end(`leaf:${url.slice(1)}`);
    } else if (url === '/blocked') {
      res.writeHead(200);
      res.end('secret');
    } else if (url === '/robots.txt') {
      res.writeHead(200);
      res.end('User-agent: *\nDisallow: /blocked\n');
    } else {
      res.writeHead(404);
      res.end();
    }
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({ server, base: `http://127.0.0.1:${port}` });
    });
  });
}

function close(server) {
  return new Promise((r) => server.close(() => r()));
}

test('defineSpider validates inputs', () => {
  assert.throws(() => defineSpider({}), /startUrls/);
  assert.throws(() => defineSpider({ startUrls: ['x'] }), /parse must be a function/);
  const spec = defineSpider({ startUrls: ['https://x'], parse: () => null });
  assert.equal(spec.userAgent, 'crawlex');
  assert.equal(spec.maxItems, null);
});

test('runSpider demuxes items vs Request yields against fixture server', async () => {
  const { server, base } = await startFixture();
  try {
    const spider = defineSpider({
      startUrls: [`${base}/`],
      parse: function* (resp) {
        const text = resp.body.toString('utf8');
        if (resp.finalUrl.endsWith('/')) {
          // Anchor crawl: emit children as Requests.
          for (const m of text.matchAll(/href="([^"]+)"/g)) {
            yield new Request(`${base}${m[1]}`);
          }
        } else {
          yield { url: resp.finalUrl, text };
        }
      },
    });

    const items = [];
    const handle = runSpider(spider);
    for await (const item of handle) items.push(item);

    assert.equal(items.length, 2);
    const urls = items.map((i) => i.url).sort();
    assert.deepEqual(urls, [`${base}/a`, `${base}/b`]);
    assert.equal(handle.isPaused(), false);
  } finally {
    await close(server);
  }
});

test('runSpider pauses on maxItems and resumes from checkpoint', async () => {
  const { server, base } = await startFixture();
  try {
    const spec = {
      startUrls: [`${base}/`],
      parse: function* (resp) {
        const text = resp.body.toString('utf8');
        if (resp.finalUrl.endsWith('/')) {
          for (const m of text.matchAll(/href="([^"]+)"/g)) {
            yield new Request(`${base}${m[1]}`);
          }
        } else {
          yield { url: resp.finalUrl };
        }
      },
    };

    const phase1 = defineSpider({ ...spec, maxItems: 1 });
    const h1 = runSpider(phase1);
    const items1 = [];
    for await (const i of h1) items1.push(i);
    assert.equal(items1.length, 1);
    assert.equal(h1.isPaused(), true);
    const cp = h1.checkpoint();
    assert.equal(cp.items_emitted, 1);
    assert.ok(cp.pending.length >= 1);

    const phase2 = defineSpider(spec);
    const h2 = runSpider(phase2, { resume: cp });
    const items2 = [];
    for await (const i of h2) items2.push(i);
    // Combined run hits both /a and /b exactly once.
    assert.equal(items1.length + items2.length, 2);
  } finally {
    await close(server);
  }
});

test('robotsTxtObey blocks disallowed paths', async () => {
  const { server, base } = await startFixture();
  try {
    // Pre-populate robots cache (no out-of-band fetch in slice 17).
    const robotsBody = 'User-agent: *\nDisallow: /blocked\n';
    const robotsCache = new Map([['127.0.0.1:' + new URL(base).port, robotsBody]]);

    const spider = defineSpider({
      startUrls: [`${base}/blocked`, `${base}/a`],
      robotsTxtObey: true,
      parse: function* (resp) {
        yield { url: resp.finalUrl, status: resp.status };
      },
    });

    const items = [];
    for await (const i of runSpider(spider, { robotsCache })) items.push(i);
    assert.equal(items.length, 1);
    assert.equal(items[0].url, `${base}/a`);
  } finally {
    await close(server);
  }
});

test('downloadDelayMs throttles per-host', async () => {
  const { server, base } = await startFixture();
  try {
    const spider = defineSpider({
      startUrls: [`${base}/a`, `${base}/b`],
      downloadDelayMs: 120,
      parse: function* (resp) {
        yield { url: resp.finalUrl, at: Date.now() };
      },
    });
    const start = Date.now();
    const items = [];
    for await (const i of runSpider(spider)) items.push(i);
    assert.equal(items.length, 2);
    // Two fetches against the same host with a 120ms gap => total ≥ ~120ms.
    assert.ok(items[1].at - items[0].at >= 110, `gap was ${items[1].at - items[0].at}ms`);
    const _elapsed = Date.now() - start; void _elapsed;
  } finally {
    await close(server);
  }
});

test('dedupes Request yields by method+url', async () => {
  const { server, base } = await startFixture();
  try {
    let parses = 0;
    const spider = defineSpider({
      startUrls: [`${base}/`],
      parse: function* (resp) {
        parses += 1;
        if (resp.finalUrl.endsWith('/')) {
          yield new Request(`${base}/a`);
          yield new Request(`${base}/a`);
          yield new Request(`${base}/a`);
        } else {
          yield { url: resp.finalUrl };
        }
      },
    });
    const items = [];
    for await (const i of runSpider(spider)) items.push(i);
    assert.equal(items.length, 1, 'duplicate URL should fetch+emit once');
    assert.equal(parses, 2, 'parse runs once per unique fetch');
  } finally {
    await close(server);
  }
});

test('AbortSignal pauses run mid-flight', async () => {
  const { server, base } = await startFixture();
  try {
    const ctrl = new AbortController();
    const spider = defineSpider({
      startUrls: [`${base}/a`, `${base}/b`],
      parse: function* (resp) {
        yield { url: resp.finalUrl };
        ctrl.abort(); // abort after first item
      },
    });
    const items = [];
    const h = runSpider(spider, { signal: ctrl.signal });
    for await (const i of h) items.push(i);
    assert.equal(items.length, 1);
    assert.equal(h.isPaused(), true);
    const cp = h.checkpoint();
    assert.ok(cp.pending.length >= 1, 'remaining work must land in checkpoint');
  } finally {
    await close(server);
  }
});
