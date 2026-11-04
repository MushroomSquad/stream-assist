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

function harness() {
  let reduced = false, animationName = 'leave', socket;
  const timers = new Map();
  let timerId = 0;
  class Element {
    constructor(text = '') {
      this.childNodes = []; this.parentNode = null; this.dataset = {}; this.className = '';
      this.hidden = false; this.events = {}; this.animations = []; this.operations = [];
      this.transitions = []; this.reads = [];
      this.style = { setProperty: (key, value) => { this.style[key] = String(value); } };
      Object.defineProperty(this.style, 'transition', { set: value => this.transitions.push(value) });
      this.classList = {
        contains: name => this.className.split(' ').includes(name),
        remove: name => { this.reads.push('remove'); this.className = this.className.split(' ').filter(c => c !== name).join(' '); },
        add: name => { this.reads.push('add'); if (!this.classList.contains(name)) this.className = (this.className + ' ' + name).trim(); },
      };
      this.textContent = text;
    }
    get children() { return this.childNodes.filter(n => n instanceof Element); }
    get textContent() { return this.childNodes.map(n => n.textContent).join(''); }
    set textContent(value) { this.childNodes = [{ textContent: value }]; }
    get offsetWidth() { this.reads.push('width'); return 100; }
    get offsetTop() { return this.parentNode ? this.parentNode.children.indexOf(this) * 30 : 0; }
    replaceChildren(...nodes) { this.childNodes = [...nodes]; }
    insertBefore(node, before) {
      this.operations.push(['insert', node]);
      if (node.parentNode) node.remove();
      this.childNodes.splice(before ? this.childNodes.indexOf(before) : this.childNodes.length, 0, node);
      node.parentNode = this;
    }
    remove() {
      if (this.parentNode) {
        this.parentNode.operations.push(['remove', this]);
        this.parentNode.childNodes.splice(this.parentNode.childNodes.indexOf(this), 1);
        this.parentNode = null;
      }
    }
    addEventListener(name, fn) { (this.events[name] ||= new Set()).add(fn); }
    removeEventListener(name, fn) { this.events[name]?.delete(fn); }
    end(target = this) { for (const fn of [...(this.events.animationend || [])]) fn({ target }); }
    animate(frames, options) { this.animations.push({ frames, options }); }
  }
  const document = { documentElement: { dataset: {}, style: {} } };
  const window = {};
  class WebSocket { constructor() { socket = this; } }
  vm.runInNewContext(sdk, { window, document, location: { search: '', host: 'localhost' }, URLSearchParams, WebSocket,
    matchMedia: () => ({ matches: reduced }), getComputedStyle: () => ({ animationName }),
    setTimeout: (fn, ms) => { timers.set(++timerId, { fn, ms }); return timerId; },
    clearTimeout: id => timers.delete(id),
  });
  return { api: window.StreamAssist, document, socket, Element,
    reduce: value => { reduced = value; }, animation: value => { animationName = value; },
    tick(ms) { for (const [id, timer] of [...timers]) if (timer.ms === ms) { timers.delete(id); timer.fn(); } },
  };
}
const h = harness(), { api, Element } = h;
h.socket.onmessage({ data: JSON.stringify({ State: { visible: true, now: null, queue: [], progress_ms: 0, mode: 'Playing', warning: null } }) });
assert.equal(h.document.documentElement.dataset.mode, 'Playing');
const replay = new Element();
api.replay(replay, 'x'); api.replay(null, 'x');
assert.deepEqual(replay.reads, ['remove', 'width', 'add']);
const shown = new Element();
api.show(shown, false); assert.equal(shown.hidden, false);
shown.end(new Element()); assert.equal(shown.hidden, false);
h.tick(400); assert.equal(shown.hidden, true);
assert.equal(shown.classList.contains('sa-leaving'), false);
api.show(shown, true); api.show(shown, false); shown.end(); assert.equal(shown.hidden, true);
api.show(shown, true); api.show(shown, false); api.show(shown, true); h.tick(400);
assert.equal(shown.hidden, false); assert.equal(shown.classList.contains('sa-leaving'), false);
h.animation('none'); api.show(shown, false); assert.equal(shown.hidden, true);
h.animation('leave'); api.show(shown, true); h.reduce(true); api.show(shown, false); assert.equal(shown.hidden, true); h.reduce(false);
const bar = new Element();
api.progress(bar, 10, 'a'); assert.deepEqual(bar.transitions, ['none', '']);
api.progress(bar, 20, 'a'); assert.equal(bar.transitions.length, 2);
api.progress(bar, 5, 'a'); assert.equal(bar.transitions.length, 4);
api.progress(bar, 10, 'b'); assert.equal(bar.transitions.length, 6);
api.progress(bar, NaN, 'b'); assert.equal(bar.style.width, '0%');
api.progress(bar, 150, 'b'); assert.equal(bar.style.width, '100%');
const container = new Element(); container.replaceChildren();
const build = (item, index) => { const node = new Element(`${index}: ${item}`); node.className = 'row'; return node; };
const list = items => api.list(container, items, x => x, build);
list(['a', 'b']);
const [a, b] = container.children;
assert.ok(a.classList.contains('sa-enter')); assert.ok(b.classList.contains('sa-enter'));
b.end(new Element()); assert.ok(b.classList.contains('sa-enter'));
b.end(); assert.equal(b.classList.contains('sa-enter'), false);
h.tick(1500); assert.equal(a.classList.contains('sa-enter'), false);
container.operations = []; list(['b', 'c']);
assert.equal(container.children[0], b); assert.equal(b.textContent, '0: b'); assert.equal(a.parentNode, null);
assert.equal(b.classList.contains('sa-enter'), false); assert.ok(container.children[1].classList.contains('sa-enter'));
assert.equal(b.animations[0].frames[0].translate, '0 30px'); assert.equal(b.animations[0].options.duration, 300);
container.operations = []; list(['b', 'c']);
assert.equal(container.operations.length, 0);
const c = container.children[1]; list(['c', 'b']); assert.equal(c.animations.length, 0);
b.animate = undefined; list(['b', 'c']);
h.reduce(true); list(['c', 'd', 'e', 'f', 'g', 'h']);
assert.equal(c.animations.length, 0);
assert.ok(container.children.slice(1).every(node => !node.classList.contains('sa-enter') && !node.animations.length));
assert.equal(container.children[4].style['--i'], '4'); assert.equal(container.children[5].style['--i'], '4');
console.log('PASS: SDK mode, replay, show, progress and keyed list');
