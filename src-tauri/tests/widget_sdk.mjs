import vm from 'node:vm';
import fs from 'node:fs';
import assert from 'node:assert/strict';
const sdk = fs.readFileSync(new URL('../assets/widget/sdk.js', import.meta.url), 'utf8');
for (const [query, obs, part, zoom, role] of [
  ['?part=queue', true, 'queue', '1', 'view'],
  ['?part=now&scale=2', true, 'now', '2', 'player'],
  ['', true, 'all', '1', 'player'],
  ['?part=invalid&scale=99', false, 'all', '4', 'view'],
  ['?part=queue&scale=0.1', true, 'queue', '0.5', 'view'],
  ['?scale=NaN', false, 'all', '1', 'view'],
  ['?scale=Infinity', false, 'all', '1', 'view'],
]) {
  const urls = [], scripts = [];
  const document = { documentElement: { dataset: {}, style: {} }, getElementById: () => null,
    createElement: () => ({ style: {} }), body: { append() {} }, head: { append: s => scripts.push(s) } };
  const window = obs ? { obsstudio: {} } : {};
  class WebSocket { constructor(url) { urls.push(url); } }
  vm.runInNewContext(sdk, { window, document, location: { search: query, host: '127.0.0.1:47821' }, URLSearchParams, WebSocket });
  assert.equal(document.documentElement.dataset.part, part);
  assert.equal(document.documentElement.style.zoom, zoom);
  assert.equal(urls[0], `ws://127.0.0.1:47821/ws?role=${role}`);
  assert.equal(scripts.length, role === 'player' ? 1 : 0);
}
console.log('PASS: SDK parts, OBS roles, YouTube isolation and scale (7 cases)');
