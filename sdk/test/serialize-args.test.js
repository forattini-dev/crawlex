'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const { serializeArgs } = require('../crawlex-sdk.js');

test('camelCase → kebab-case for scalar fields', () => {
  assert.deepEqual(
    serializeArgs({ maxDepth: 3, screenshotMode: 'fullpage' }),
    ['--max-depth', '3', '--screenshot-mode', 'fullpage'],
  );
});

test('boolean true emits flag without value', () => {
  assert.deepEqual(
    serializeArgs({ screenshot: true, sameHostOnly: true }),
    ['--screenshot', '--same-host-only'],
  );
});

test('boolean false drops the flag', () => {
  assert.deepEqual(
    serializeArgs({ screenshot: false, crtsh: true }),
    ['--crtsh'],
  );
});

test('null and undefined are dropped', () => {
  assert.deepEqual(
    serializeArgs({ method: undefined, profile: null, maxDepth: 0 }),
    ['--max-depth', '0'],
  );
});

test('multi-value arrays repeat the flag in singular form', () => {
  assert.deepEqual(
    serializeArgs({
      seeds: ['https://a.test', 'https://b.test'],
      proxies: ['http://p1', 'http://p2'],
      hookScripts: ['hooks/foo.lua'],
      chromeFlags: ['--disable-blink-features=AutomationControlled'],
    }),
    [
      '--seed', 'https://a.test',
      '--seed', 'https://b.test',
      '--proxy', 'http://p1',
      '--proxy', 'http://p2',
      '--hook-script', 'hooks/foo.lua',
      '--chrome-flag', '--disable-blink-features=AutomationControlled',
    ],
  );
});

test('reserved SDK keys do not turn into flags', () => {
  assert.deepEqual(
    serializeArgs({
      bin: '/usr/local/bin/crawlex',
      signal: 'whatever',
      env: { FOO: 'bar' },
      config: { x: 1 },
      rawArgs: ['--something'],
      maxDepth: 2,
    }),
    ['--max-depth', '2'],
  );
});

test('unmapped arrays default to repeated flag', () => {
  assert.deepEqual(
    serializeArgs({ headers: ['x: 1', 'y: 2'] }),
    ['--headers', 'x: 1', '--headers', 'y: 2'],
  );
});

test('throws when a known multi-value field is not an array', () => {
  assert.throws(
    () => serializeArgs({ seeds: 'https://a.test' }),
    /must be an array/,
  );
});

test('numeric values stringify cleanly', () => {
  assert.deepEqual(
    serializeArgs({ maxConcurrentHttp: 8, renderRequestTimeoutMs: 60000 }),
    ['--max-concurrent-http', '8', '--render-request-timeout-ms', '60000'],
  );
});

test('persona codename routes to --persona', () => {
  assert.deepEqual(
    serializeArgs({ persona: 'tux' }),
    ['--persona', 'tux'],
  );
});
