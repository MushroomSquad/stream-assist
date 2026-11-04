const el = id => document.getElementById(id);
let previousTitle, previousId, previousCount;
el('now').addEventListener('animationend', event => {
  if ((event.animationName === 'detail-in' && event.target === el('requester')) || (event.animationName === 'sweep' && event.target === el('now'))) {
    el('now').classList.remove('sa-new');
  }
});
function ransom(target, title, animate = false) {
  target.replaceChildren();
  let seed = 2166136261;
  for (const char of title) seed = Math.imul(seed ^ char.codePointAt(0), 16777619) >>> 0;
  const chars = Array.from(title);
  const short = chars.length > 40 ? chars.slice(0, 39).concat('…') : chars;
  // letters are grouped per word so lines only break between words
  let word = null;
  short.forEach((char, index) => {
    if (/\s/.test(char)) { word = null; target.append(document.createTextNode(' ')); return; }
    if (!word) { word = document.createElement('span'); word.style.display = 'inline-block'; word.style.whiteSpace = 'nowrap'; word.style.marginRight = '0.35em'; target.append(word); }
    const span = document.createElement('span');
    const hash = Math.imul(seed ^ index, 2654435761) >>> 0;
    span.textContent = char;
    span.className = 'letter ' + (index % 2 ? 'inverse' : '');
    span.style.setProperty('--i', index);
    // Animated glyph layers alter Chromium's resting raster in Paused mode.
    if (animate && document.documentElement.dataset.mode !== 'Paused' && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
      span.classList.add('sa-enter');
    }
    span.style.fontFamily = ['Anton', 'Bangers', 'Barlow'][hash % 3];
    span.style.transform = `translateY(${hash % 5 - 2}px) rotate(${hash % 17 - 8}deg)`;
    span.style.fontSize = (85 + hash % 31) + '%';
    word.append(span);
  });
}
if (document.body.dataset.theme === 'phantom') ransom(el('heading'), 'ОЧЕРЕДЬ');
StreamAssist.on('state', state => {
  StreamAssist.show(el('widget'), state.visible);
  const now = state.now;
  el('now').hidden = !now;
  const title = now?.track.title || '';
  if (title !== previousTitle) {
    if (document.body.dataset.theme === 'phantom') ransom(el('title'), title, true);
    else el('title').textContent = title;
    previousTitle = title;
  }
  el('artist').textContent = now?.track.artist || '';
  el('requester').textContent = now ? 'ЗАКАЗАЛ: ' + now.requester : '';
  const cover = el('cover'), url = now?.track.cover_url;
  cover.hidden = !url; el('note').hidden = !!url;
  cover.onerror = () => { cover.hidden = true; el('note').hidden = false; };
  if (url && cover.getAttribute('src') !== url) cover.src = url;
  if (!url) cover.removeAttribute('src');
  const duration = now?.track.duration_ms;
  const progress = duration > 0 && Number.isFinite(state.progress_ms) ? Math.min(100, Math.max(0, state.progress_ms / duration * 100)) : 0;
  StreamAssist.progress(el('progress'), progress, now?.id);
  if (now?.id != null && now.id !== previousId) StreamAssist.replay(el('now'), 'sa-new');
  previousId = now?.id;
  el('queue-panel').hidden = !state.queue.length;
  el('count').textContent = String(state.queue.length).padStart(2, '0');
  if (state.queue.length !== previousCount) StreamAssist.replay(el('count'), 'sa-bump');
  previousCount = state.queue.length;
  StreamAssist.list(el('queue'), state.queue.slice(0, 5), item => item.id, (item, index) => {
    const row = document.createElement('div'); row.className = 'row';
    const number = document.createElement('b'); number.className = 'number'; number.textContent = String(index + 1).padStart(2, '0');
    const info = document.createElement('div'); info.className = 'info';
    const track = document.createElement('div'); track.className = 'track'; track.textContent = item.track.title;
    const user = document.createElement('small'); user.textContent = item.requester;
    info.append(track, user); row.append(number, info); return row;
  });
});
