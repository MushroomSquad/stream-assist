import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

const context = { exports: {} };
vm.runInNewContext(ts.transpile(readFileSync('src/lib/updateController.ts', 'utf8'), {
  module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022
}), context);
const { createUpdater } = context.exports;
function setup(overrides = {}, offerOverrides = {}) {
  let state;
  const calls = [];
  const offer = {
    version: '0.4.0', currentVersion: '0.3.0',
    download: async report => {
      calls.push('download');
      report({ event: 'Started', data: { contentLength: 100 } });
      report({ event: 'Progress', data: { chunkLength: 40 } });
      report({ event: 'Progress', data: { chunkLength: 60 } });
      report({ event: 'Finished' });
    },
    install: async () => { calls.push('install'); },
    close: async () => { calls.push('close'); },
    ...offerOverrides
  };
  const deps = {
    check: async () => { calls.push('check'); return offer; },
    supported: async () => true,
    prepare: async () => { calls.push('prepare'); },
    recover: async () => { calls.push('recover'); },
    restart: async () => { calls.push('restart'); },
    openRelease: async version => { calls.push(`open:${version}`); },
    ...overrides
  };
  const updater = createUpdater(deps, next => { state = next; });
  return { updater, offer, calls, state: () => state };
}

test('startup checks once and waits for explicit update consent', async () => {
  const s = setup();
  await s.updater.check(); await s.updater.check();
  assert.deepEqual(s.calls, ['check']);
  assert.equal(s.state().phase, 'available');
  s.updater.dismiss();
  assert.equal(s.state().offer, null);
  assert.deepEqual(s.calls, ['check', 'close']);
});
test('offline and no new version leave startup undisturbed', async () => {
  for (const check of [async () => { throw Error('offline'); }, async () => null]) {
    const s = setup({ check });
    await assert.doesNotReject(s.updater.check());
    assert.equal(s.state(), undefined);
  }
});
test('download completes before pausing rewards, installing and restarting', async () => {
  const s = setup(); await s.updater.check(); await s.updater.update();
  assert.deepEqual(s.calls, ['check', 'download', 'prepare', 'install', 'restart']);
  assert.equal(s.state().downloaded, 100);
  assert.equal(s.state().total, 100);
});
test('download failure allows retry without installing corrupt or partial bytes', async () => {
  let attempts = 0;
  const s = setup({}, { download: async () => { if (++attempts === 1) throw Error('bad signature'); } });
  await s.updater.check(); await s.updater.update();
  assert.equal(s.state().error, true);
  assert.equal(s.state().phase, 'available');
  assert.deepEqual(s.calls, ['check']);
  await s.updater.update();
  assert.equal(attempts, 2);
  assert.deepEqual(s.calls, ['check', 'prepare', 'install', 'restart']);
});
test('restart failure retries restart without reinstalling', async () => {
  let attempts = 0;
  const s = setup({ restart: async () => { if (++attempts === 1) throw Error('restart'); } });
  await s.updater.check(); await s.updater.update();
  assert.equal(s.state().phase, 'restart'); assert.equal(s.state().error, true);
  await s.updater.update();
  assert.equal(attempts, 2);
  assert.deepEqual(s.calls, ['check', 'download', 'prepare', 'install']);
});
test('failed installation restores rewards and retries using verified download', async () => {
  let attempts = 0;
  const s = setup({}, { install: async () => { if (++attempts === 1) throw Error('permission'); } });
  await s.updater.check(); await s.updater.update();
  assert.equal(s.state().error, true);
  assert.deepEqual(s.calls, ['check', 'download', 'prepare', 'recover']);
  await s.updater.update();
  assert.deepEqual(s.calls, ['check', 'download', 'prepare', 'recover', 'prepare', 'restart']);
});
test('deb/rpm open the specific release without downloading or installing', async () => {
  const s = setup({ supported: async () => false });
  await s.updater.check(); await s.updater.update();
  assert.deepEqual(s.calls, ['check', 'open:0.4.0', 'close']);
  assert.equal(s.state().offer, null);
});
test('while downloading duplicate clicks and dismiss are ignored', async () => {
  let finish;
  const s = setup({}, { download: () => new Promise(resolve => { finish = resolve; }) });
  await s.updater.check();
  const pending = s.updater.update();
  s.updater.dismiss(); await s.updater.update();
  assert.equal(s.state().phase, 'downloading');
  finish(); await pending;
  assert.deepEqual(s.calls, ['check', 'prepare', 'install', 'restart']);
});
test('late startup response is disposed without showing dialog', async () => {
  let resolve;
  let started;
  const entered = new Promise(done => { started = done; });
  const s = setup({ check: () => new Promise(done => { resolve = done; started(); }) });
  const pending = s.updater.check();
  await entered;
  s.updater.dispose(); resolve(s.offer); await pending;
  assert.equal(s.state(), undefined);
  assert.deepEqual(s.calls, ['close']);
});
