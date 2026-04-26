'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const { defineHooks, HOOK_EVENTS } = require('../crawlex-sdk.js');

function ctx(extra = {}) {
  return {
    url: 'https://example.test/p',
    depth: 0,
    html_present: false,
    captured_urls: [],
    retry_count: 0,
    allow_retry: true,
    user_data: {},
    ...extra,
  };
}

test('subscribed reflects only the handlers that were registered', () => {
  const spec = defineHooks({
    onAfterFirstByte: () => 'continue',
    onDiscovery: () => 'continue',
  });
  assert.deepEqual(
    [...spec.subscribed].sort(),
    ['after_first_byte', 'on_discovery'],
  );
});

test('dispatch returns null for non-invoke envelopes', async () => {
  const spec = defineHooks({ onAfterFirstByte: () => 'skip' });
  const result = await spec.dispatch({ kind: 'hello', v: 1 });
  assert.equal(result, null);
});

test('handler returning bare decision string is normalized', async () => {
  const spec = defineHooks({ onAfterFirstByte: () => 'skip' });
  const r = await spec.dispatch({
    kind: 'hook_invoke',
    id: 7,
    event: 'after_first_byte',
    ctx: ctx(),
  });
  assert.deepEqual(r, { kind: 'hook_result', id: 7, decision: 'skip' });
});

test('handler returning an object with patch translates camelCase → snake_case', async () => {
  const spec = defineHooks({
    onDiscovery: () => ({
      decision: 'continue',
      patch: {
        capturedUrls: ['https://a.test', 'https://b.test'],
        userData: { tag: 1 },
        robotsAllowed: false,
        allowRetry: true,
      },
    }),
  });
  const r = await spec.dispatch({
    kind: 'hook_invoke',
    id: 9,
    event: 'on_discovery',
    ctx: ctx(),
  });
  assert.deepEqual(r, {
    kind: 'hook_result',
    id: 9,
    decision: 'continue',
    patch: {
      captured_urls: ['https://a.test', 'https://b.test'],
      user_data: { tag: 1 },
      robots_allowed: false,
      allow_retry: true,
    },
  });
});

test('handler returning null/undefined defaults to continue', async () => {
  const spec = defineHooks({ onAfterFirstByte: () => undefined });
  const r = await spec.dispatch({
    kind: 'hook_invoke',
    id: 1,
    event: 'after_first_byte',
    ctx: ctx(),
  });
  assert.deepEqual(r, { kind: 'hook_result', id: 1, decision: 'continue' });
});

test('throwing handler becomes abort with hook_error in user_data', async () => {
  const spec = defineHooks({
    onAfterFirstByte: () => {
      throw new Error('boom');
    },
  });
  const r = await spec.dispatch({
    kind: 'hook_invoke',
    id: 4,
    event: 'after_first_byte',
    ctx: ctx({ user_data: { existing: 1 } }),
  });
  assert.equal(r.decision, 'abort');
  assert.equal(r.id, 4);
  assert.deepEqual(r.patch.user_data, { existing: 1, hook_error: 'boom' });
});

test('event handler key snake_case form is also accepted', async () => {
  const spec = defineHooks({
    after_first_byte: () => 'retry',
  });
  assert.deepEqual(spec.subscribed, ['after_first_byte']);
  const r = await spec.dispatch({
    kind: 'hook_invoke',
    id: 2,
    event: 'after_first_byte',
    ctx: ctx(),
  });
  assert.equal(r.decision, 'retry');
});

test('unknown event handler key throws at definition time', () => {
  assert.throws(
    () => defineHooks({ onBananaSplit: () => 'continue' }),
    /unknown handler/,
  );
});

test('unknown decision value throws when handler runs', async () => {
  const spec = defineHooks({ onAfterFirstByte: () => 'maybe' });
  await assert.rejects(
    spec.dispatch({
      kind: 'hook_invoke',
      id: 1,
      event: 'after_first_byte',
      ctx: ctx(),
    }),
    /unknown decision/,
  );
});

test('dispatch on event with no handler short-circuits to continue', async () => {
  const spec = defineHooks({ onAfterFirstByte: () => 'skip' });
  const r = await spec.dispatch({
    kind: 'hook_invoke',
    id: 5,
    event: 'on_discovery',
    ctx: ctx(),
  });
  assert.deepEqual(r, { kind: 'hook_result', id: 5, decision: 'continue' });
});

test('HOOK_EVENTS exposes all 12 stable wire names', () => {
  assert.equal(HOOK_EVENTS.length, 12);
  assert.ok(HOOK_EVENTS.includes('before_each_request'));
  assert.ok(HOOK_EVENTS.includes('on_robots_decision'));
});
