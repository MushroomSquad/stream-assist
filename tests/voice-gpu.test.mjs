import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import vm from 'node:vm';
const read = p => readFileSync(`src/lib/${p}`, 'utf8');
test('GPU API forwards commands, clone device and progress payload', async () => {
  const calls = []; let listener;
  const context = { exports: {}, isTauri: () => true,
    invoke: async (...args) => { calls.push(args); },
    listen: async (name, cb) => { assert.equal(name, 'vl-gpu-install'); listener = cb; return () => {}; } };
  vm.runInNewContext(ts.transpile(read('voiceApi.ts').replace(/^import .*;$/gm, ''), { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 }), context);
  const api = context.exports;
  await api.vl_gpu_status(); await api.vl_gpu_install(); await api.vl_gpu_install_cancel(); await api.vl_gpu_uninstall();
  await api.vl_clone(1, 0, 5000, 'text', 'en', 'voice', 'fast', 42, 'gpu');
  await api.vl_clone(1, 0, 5000, 'text', 'en', 'voice', 'fast', 42);
  assert.deepEqual(calls.slice(0, 4).map(c => c[0]), ['vl_gpu_status', 'vl_gpu_install', 'vl_gpu_install_cancel', 'vl_gpu_uninstall']);
  assert.equal(calls[4][1].device, 'gpu'); assert.equal(calls[5][1].device, 'cpu');
  let progress; await api.onGpuInstall(p => { progress = p; });
  listener({ payload: { done: 5, total: 10 } }); assert.deepEqual(progress, { done: 5, total: 10 });
});
test('GPU status restores only usable device and tolerates blocked localStorage', async () => {
  const script = read('views/voice/Sources.svelte');
  const helper = script.match(/async function loadGpu\(\)[\s\S]*?\n}/)?.[0];
  assert.ok(helper, 'GPU status loader exists');
  const context = { gpu: null, device: 'cpu', gpuProgress: null,
    api: { vl_gpu_status: async () => ({ supported: true, pack: 'ok', installing: false }) },
    localStorage: { getItem: () => 'gpu' } };
  vm.createContext(context); vm.runInContext(ts.transpile(helper, { target: ts.ScriptTarget.ES2022 }), context);
  await context.loadGpu(); assert.equal(context.device, 'gpu');
  context.api.vl_gpu_status = async () => ({ supported: true, pack: 'broken', installing: false });
  await context.loadGpu(); assert.equal(context.device, 'cpu');
  context.localStorage.getItem = () => { throw Error('denied'); };
  await context.loadGpu(); assert.equal(context.device, 'cpu');
});
test('GPU controls gate unsupported and broken packs and use localized messages', () => {
  const ui = read('views/voice/Sources.svelte');
  for (const pattern of [/gpu\?\.supported/, /disabled={gpu.pack !== 'ok'}/, /gpu.pack === 'broken'/,
    /gpu.missingLibs/, /api.onGpuInstall/, /stopGpu\(\)/, /vlCloneGpuNote/, /vlCloneCpuNote/]) assert.match(ui, pattern);
  const ru = read('i18n/ru.ts');
  for (const key of ['GPU_FAILED', 'GPU_PACK_MISSING', 'GPU_PACK_INVALID', 'GPU_UNSUPPORTED', 'vlGpuMissingLibs']) assert.ok(ru.includes(key), key);
});
test('device selection persists the newly selected value even before bind updates', () => {
  const source = read('views/voice/Sources.svelte');
  const helper = source.match(/function saveDevice[^\n]*/)[0];
  let saved;
  const context = { device: 'cpu', localStorage: { setItem: (key, value) => { saved = [key, value]; } } };
  vm.createContext(context); vm.runInContext(ts.transpile(helper, { target: ts.ScriptTarget.ES2022 }), context);
  context.saveDevice('gpu'); assert.deepEqual(saved, ['vl.cloneDevice', 'gpu']);
  context.localStorage.setItem = () => { throw Error('denied'); };
  context.saveDevice('cpu'); assert.equal(context.device, 'cpu');
});
test('GPU package operations refresh status and clear busy after success or failure', async () => {
  const source = read('views/voice/Sources.svelte');
  const helpers = ['installGpu', 'uninstallGpu'].map(name => source.match(new RegExp('async function ' + name + '\\(\\)[\\s\\S]*?\\n}'))[0]).join('\n');
  let refresh = 0;
  const context = { gpuBusy: false, gpuProgress: null, loadGpu: async () => { refresh++; },
    api: { vl_gpu_install: async () => {}, vl_gpu_uninstall: async () => { throw Error('JOB_BUSY'); } } };
  vm.createContext(context); vm.runInContext(ts.transpile(helpers, { target: ts.ScriptTarget.ES2022 }), context);
  await context.installGpu(); assert.equal(context.gpuBusy, false); assert.equal(refresh, 1);
  await assert.rejects(context.uninstallGpu(), /JOB_BUSY/);
  assert.equal(context.gpuBusy, false); assert.equal(refresh, 2);
});
test('elapsed-time timer never requests expensive GPU status', () => {
  const ui = read('views/voice/Sources.svelte');
  const timer = ui.match(/setInterval\([\s\S]*?, 1000\)/)[0];
  assert.doesNotMatch(timer, /loadGpu|vl_gpu_status/);
  assert.equal((ui.match(/api.vl_gpu_status\(/g) ?? []).length, 1);
});
