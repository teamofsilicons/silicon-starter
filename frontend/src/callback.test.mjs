import test from 'node:test';
import assert from 'node:assert/strict';
import {callbackRecovery, cleanCallbackUrl, hasLoginCallback} from './callback.ts';

test('an uncertain login keeps its callback and retries identical credentials before redaction', async () => {
  const url = 'https://starter.example/auth/callback?slt=oac_original&state=nonce';
  const sent = [], redacted = [];
  const recovery = callbackRecovery(url, async body => {
    sent.push(body);
    if (sent.length === 1) throw new Error('IAM unavailable');
  }, value => redacted.push(value));
  await assert.rejects(recovery.retry(), /unavailable/);
  assert.deepEqual(redacted, []);
  await recovery.retry();
  assert.equal(sent[0], sent[1]);
  assert.deepEqual(JSON.parse(sent[0]), {slt:'oac_original', state:'nonce'});
  assert.deepEqual(redacted, ['/']);
  await assert.rejects(recovery.retry(), /no longer pending/);
});

test('a reload can reconstruct the same exact callback; cancellation redacts it without exchange', async () => {
  const entries = [{app_id:'other',slt:'oac_other'}, {app_id:'starter',slt:'oac_starter:+/='}];
  const url = `https://starter.example/?state=original#slts=${encodeURIComponent(JSON.stringify(entries))}`;
  const sent = [], redacted = [];
  for (let attempt = 0; attempt < 2; attempt++) {
    const recovery = callbackRecovery(url, async body => { sent.push(body); throw new Error('lost reply'); }, value => redacted.push(value));
    await assert.rejects(recovery.retry());
  }
  assert.equal(sent[0], sent[1]);
  const cancelled = callbackRecovery(url, async () => assert.fail('cancel must not exchange'), value => redacted.push(value));
  cancelled.cancel();
  assert.deepEqual(redacted, ['/']);
  await assert.rejects(cancelled.retry(), /no longer pending/);
});

test('a login attempt cannot race another exchange or be cancelled in flight', async () => {
  let finish;
  const recovery = callbackRecovery('https://starter.example/?slt=oac_a', () => new Promise(resolve => finish = resolve), () => {});
  const pending = recovery.retry();
  await assert.rejects(recovery.retry(), /already being checked/);
  assert.throws(() => recovery.cancel(), /Wait/);
  finish(); await pending;
});

test('unrelated or ambiguous callback credentials are never exchanged and remain cancellable', async () => {
  for (const entries of [[{app_id:'tos>starter',slt:'oac_legacy'}], [{app_id:'starter',slt:'a'},{app_id:'starter',slt:'b'}]]) {
    const redacted = [];
    const recovery = callbackRecovery(`https://starter.example/?state=n#slts=${encodeURIComponent(JSON.stringify(entries))}`, async () => assert.fail('invalid callback exchanged'), value => redacted.push(value));
    await assert.rejects(recovery.retry(), /No login token/);
    assert.deepEqual(redacted, []);
    recovery.cancel(); assert.deepEqual(redacted, ['/']);
  }
  assert.equal(hasLoginCallback(new URL('https://starter.example/?q=rust')), false);
  assert.equal(cleanCallbackUrl('https://starter.example/?slt=private&state=n&q=rust#normal'), '/?q=rust#normal');
});
