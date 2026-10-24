import assert from 'node:assert/strict';
import { test } from 'node:test';
import { executeToolRequest } from './tool-recovery.ts';

const request = {
  request_id: 'request',
  tool_name: 'stopped_tool',
  arguments: { z: 1, nested: { b: [2, 3], a: null } },
};
const effect = {
  effect_id: 'effect',
  key: { request_id: 'request', invocation_key: '0' },
  tool_name: request.tool_name,
  arguments: { nested: { a: null, b: [2, 3] }, z: 1 },
  status: 'succeeded',
};
const noSubmit = () => {
  throw new Error('Recovery must not resubmit or approve an existing effect');
};

test('lost acknowledgement recovers any recorded status without a new POST', async () => {
  for (const status of [
    'proposed',
    'approved',
    'dispatched',
    'succeeded',
    'failed',
    'unknown',
    'cancelled',
  ]) {
    const old = { ...effect, status };
    assert.equal(await executeToolRequest(request, true, async () => [old], noSubmit), old);
  }
});

test('request recovery paginates to manual invocation zero', async () => {
  const cursors = [];
  const page = Array.from({ length: 100 }, (_, i) => ({
    ...effect,
    effect_id: `other-${i}`,
    key: { ...effect.key, invocation_key: `${i + 1}` },
  }));
  assert.equal(
    await executeToolRequest(
      request,
      true,
      async (before) => {
        cursors.push(before);
        return before ? [effect] : page;
      },
      noSubmit
    ),
    effect
  );
  assert.deepEqual(cursors, [undefined, 'other-99']);
});

test('lookup failure or a mismatched existing intent never submits', async () => {
  for (const old of [
    { ...effect, tool_name: 'different' },
    { ...effect, arguments: { z: 2 } },
    { ...effect, key: { ...effect.key, request_id: 'different' } },
  ]) {
    await assert.rejects(
      executeToolRequest(request, true, async () => [old], noSubmit),
      /already recorded/
    );
  }
  await assert.rejects(
    executeToolRequest(
      request,
      true,
      async () => {
        throw new Error('ledger unavailable');
      },
      noSubmit
    ),
    /ledger unavailable/
  );
});

test('new requests submit directly; an absent old invocation resubmits the same request', async () => {
  let submits = 0;
  const submit = async () => {
    submits += 1;
    return effect;
  };
  assert.equal(
    await executeToolRequest(
      request,
      false,
      async () => {
        throw new Error('new request does not query');
      },
      submit
    ),
    effect
  );
  assert.equal(await executeToolRequest(request, true, async () => [], submit), effect);
  assert.equal(submits, 2);
});
