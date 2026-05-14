'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const {
  createShellApi,
  cssQuery,
  xpathQuery,
  findByText,
  findByRegex,
  Page,
} = require('../shell.js');

const FIXTURE = `<!doctype html><html><body>
  <h1 id="title">Hello World</h1>
  <div class="card"><p>Alpha card body</p></div>
  <div class="card highlight"><p>Beta card body</p></div>
  <ul><li>one</li><li>two</li><li>three</li></ul>
  <a href="/next" class="more">Read more</a>
</body></html>`;

function stubFetcher(url, body = FIXTURE, status = 200) {
  return () => Promise.resolve({ finalUrl: url, status, headers: {}, body });
}

function tmpStore() {
  return path.join(os.tmpdir(), `crawlex-shell-${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2)}.json`);
}

test('createShellApi.fetch records last and returns Page', async () => {
  const { crawlex, state } = createShellApi({ fetcher: stubFetcher('https://ex.test/') });
  const page = await crawlex.fetch('https://ex.test/');
  assert.ok(page instanceof Page);
  assert.equal(page.status, 200);
  assert.equal(state.last, page);
  assert.equal(crawlex.last, page);
});

test('css selects by tag, #id, .class', async () => {
  const { crawlex } = createShellApi({ fetcher: stubFetcher('https://ex.test/') });
  const page = await crawlex.fetch('https://ex.test/');
  assert.equal(page.css('h1').length, 1);
  assert.equal(page.css('#title')[0].text, 'Hello World');
  assert.equal(page.css('.card').length, 2);
  assert.equal(page.css('div.highlight').length, 1);
});

test('crawlex.css delegates to last page', async () => {
  const { crawlex } = createShellApi({ fetcher: stubFetcher('https://ex.test/') });
  await crawlex.fetch('https://ex.test/');
  assert.equal(crawlex.css('li').length, 3);
});

test('css throws on unsupported selector', () => {
  assert.throws(() => cssQuery(FIXTURE, 'div > p'), /unsupported selector/);
});

test('xpath supports //tag, rejects everything else', () => {
  assert.equal(xpathQuery(FIXTURE, '//li').length, 3);
  assert.throws(() => xpathQuery(FIXTURE, '//div[@class]'), /unsupported xpath/);
});

test('findByText finds matching elements', () => {
  const r = findByText(FIXTURE, 'Hello World');
  assert.ok(r.length >= 1);
  assert.ok(r.some((e) => e.tag === 'h1'));
});

test('findByRegex accepts string or RegExp', () => {
  assert.ok(findByRegex(FIXTURE, /Alpha/).length >= 1);
  assert.ok(findByRegex(FIXTURE, 'Beta').length >= 1);
});

test('css/xpath/findByText update lastSelection', async () => {
  const { crawlex, state } = createShellApi({ fetcher: stubFetcher('https://ex.test/') });
  await crawlex.fetch('https://ex.test/');
  crawlex.css('#title');
  assert.equal(state.lastSelection.tag, 'h1');
  assert.equal(state.lastSelectionQuery, 'css:#title');
  crawlex.findByText('Beta');
  assert.match(state.lastSelectionQuery, /^text:Beta/);
});

test('save writes adaptive store entry keyed by host + identifier', async () => {
  const storePath = tmpStore();
  try {
    const { crawlex } = createShellApi({ fetcher: stubFetcher('https://shop.test/'), storePath });
    await crawlex.fetch('https://shop.test/');
    crawlex.css('#title');
    const entry = crawlex.save('product_title');
    assert.equal(entry.tag, 'h1');
    const onDisk = JSON.parse(fs.readFileSync(storePath, 'utf8'));
    assert.equal(onDisk['shop.test'].product_title.tag, 'h1');
    assert.equal(onDisk['shop.test'].product_title.query, 'css:#title');
  } finally {
    try { fs.unlinkSync(storePath); } catch (_) {}
  }
});

test('save without selection throws', async () => {
  const { crawlex } = createShellApi({ fetcher: stubFetcher('https://ex.test/'), storePath: tmpStore() });
  await crawlex.fetch('https://ex.test/');
  assert.throws(() => crawlex.save('foo'), /no element selected/);
});

test('save rejects empty identifier', async () => {
  const { crawlex } = createShellApi({ fetcher: stubFetcher('https://ex.test/'), storePath: tmpStore() });
  await crawlex.fetch('https://ex.test/');
  crawlex.css('h1');
  assert.throws(() => crawlex.save(''), /non-empty/);
});

test('helpers require a prior fetch', () => {
  const { crawlex } = createShellApi({ fetcher: stubFetcher('https://ex.test/') });
  assert.throws(() => crawlex.css('h1'), /no page fetched/);
  assert.throws(() => crawlex.save('x'), /no page fetched/);
});

test('fetch rejects empty url', async () => {
  const { crawlex } = createShellApi({ fetcher: stubFetcher('https://ex.test/') });
  await assert.rejects(() => crawlex.fetch(''), /non-empty string/);
});

test('save preserves existing entries (round-trip)', async () => {
  const storePath = tmpStore();
  try {
    const { crawlex: c1 } = createShellApi({ fetcher: stubFetcher('https://a.test/'), storePath });
    await c1.fetch('https://a.test/');
    c1.css('h1');
    c1.save('title');

    const { crawlex: c2 } = createShellApi({ fetcher: stubFetcher('https://b.test/'), storePath });
    await c2.fetch('https://b.test/');
    c2.css('.card');
    c2.save('card');

    const onDisk = JSON.parse(fs.readFileSync(storePath, 'utf8'));
    assert.ok(onDisk['a.test'].title);
    assert.ok(onDisk['b.test'].card);
  } finally {
    try { fs.unlinkSync(storePath); } catch (_) {}
  }
});
