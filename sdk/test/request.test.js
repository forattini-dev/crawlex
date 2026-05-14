'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const { Request } = require('../crawlex-sdk.js');

test('Request defaults to GET with no sessionId', () => {
  const r = new Request('https://example.com');
  assert.equal(r.url, 'https://example.com');
  assert.equal(r.method, 'GET');
  assert.equal(r.sessionId, undefined);
});

test('Request accepts sessionId via opts', () => {
  const r = new Request('https://example.com', { sessionId: 'sess-1' });
  assert.equal(r.sessionId, 'sess-1');
});

test('Request honours custom method', () => {
  const r = new Request('https://example.com', { method: 'POST' });
  assert.equal(r.method, 'POST');
});

test('Request rejects empty url', () => {
  assert.throws(() => new Request(''), /non-empty string/);
});

test('Request rejects empty sessionId', () => {
  assert.throws(() => new Request('https://x', { sessionId: '' }), /non-empty/);
});
