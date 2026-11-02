const el = id => document.getElementById(id);
let previousTitle;
function ransom(target, title) {
  target.replaceChildren();
  let seed = 2166136261;
  for (const char of title) seed = Math.imul(seed ^ char.codePointAt(0), 16777619) >>> 0;
  const chars = Array.from(title);
  const short = chars.length > 40 ? chars.slice(0, 39).concat('…') : chars;
  short.forEach((char, index) => {
    if (/\s/.test(char)) { target.append(document.createTextNode(' ')); return; }
    const span = document.createElement('span');
    const hash = Math.imul(seed ^ index, 2654435761) >>> 0;
    span.textContent = char;
    span.className = 'letter ' + (index % 2 ? 'inverse' : '');
    span.style.fontFamily = ['Anton', 'Bangers', 'Barlow'][hash % 3];
    span.style.transform = `translateY(${hash % 5 - 2}px) rotate(${hash % 17 - 8}deg)`;
    span.style.fontSize = (85 + hash % 31) + '%';
    target.append(span);
  });
}
if (document.body.dataset.theme === 'phantom') ransom(el('heading'), 'ОЧЕРЕДЬ');
StreamAssist.on('state', state => {
  el('widget').hidden = !state.visible;
  const now = state.now;
  el('now').hidden = !now;
  const title = now?.track.title || '';
  if (title !== previousTitle) {
    if (document.body.dataset.theme === 'phantom') ransom(el('title'), title);
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
  el('progress').style.width = progress + '%';
  el('queue-panel').hidden = !state.queue.length;
  el('count').textContent = String(state.queue.length).padStart(2, '0');
  el('queue').replaceChildren(...state.queue.slice(0, 5).map((item, index) => {
    const row = document.createElement('div'); row.className = 'row';
    const number = document.createElement('b'); number.className = 'number'; number.textContent = String(index + 1).padStart(2, '0');
    const info = document.createElement('div'); info.className = 'info';
    const track = document.createElement('div'); track.className = 'track'; track.textContent = item.track.title;
    const user = document.createElement('small'); user.textContent = item.requester;
    info.append(track, user); row.append(number, info); return row;
  }));
});
