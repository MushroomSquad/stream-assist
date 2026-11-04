# Анимации виджетов — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Шаги — чекбоксы.

**Goal:** анимации смены трека, очереди, прогресса, показа/скрытия и фоновых циклов во всех шести встроенных темах, в стиле каждой темы.

**Architecture:** общие помощники в `sdk.js` (режим в `data-mode`, «новые id», перезапуск анимации, показ/скрытие с выходом, FLIP, плавный прогресс). Темы описывают анимации в CSS и вызывают помощники из своего кода рендера. Анимации привязаны к событиям (новый `id`), а не к каждому состоянию.

**Tech Stack:** vanilla JS, CSS animations, Web Animations API (`element.animate`), Chromium/CEF (OBS).

**Spec:** `docs/superpowers/specs/2026-11-04-widget-animations.md`

## Global Constraints

- Файлы: только `src-tauri/assets/widget/sdk.js`, `src-tauri/assets/widget/themes/*/{index.html,style.css,render.js}`, `src-tauri/tests/widget_sdk.mjs`, `src-tauri/examples/widget_demo.rs`, `docs/themes.md`. `tts.html` не трогать.
- Без зависимостей и внешних запросов. Данные — только `textContent`.
- Анимации входа и FLIP используют индивидуальные свойства `translate`/`scale`/`rotate`, `opacity`, `filter`, `clip-path` — **не** `transform`: у многих элементов тем уже есть `transform` (скос, наклон букв phantom), его нельзя перетирать.
- Повторяющиеся (recreated) узлы — строки очереди, буквы phantom — получают анимацию входа только через класс `sa-enter`, который ставится лишь для свежих `id` (`StreamAssist.fresh`). Бесконечные циклы — только на постоянных узлах.
- Циклы работают только при `html[data-mode="Playing"]`, иначе `animation-play-state: paused`.
- `@media (prefers-reduced-motion: reduce)`: в каждой теме `*{animation:none!important;transition:none!important}`; помощники SDK при reduced motion не анимируют (show скрывает сразу, flip ничего не делает, crt печатает сразу).
- Конечное состояние после всех анимаций совпадает с текущей вёрсткой пиксель в пиксель (проверяется скриншотами с reduced motion).
- Вход 250–700 мс, задержки между элементами 40–120 мс, строки очереди — `min(index,4) × 60 мс`. Выход при скрытии ≤ 400 мс. FLIP 300 мс. Циклы не мигают чаще 3 Гц.

## Review Focus

1. Обновление состояния раз в 2,5 с без изменений не перезапускает ни одну анимацию (входы строк, название, буквы phantom, печать crt).
2. Быстрая смена трека во время анимации входа или печати crt: показывается новый трек, старая печать не дописывается поверх.
3. Скрытие → показ во время анимации выхода: виджет остаётся видимым, `hidden` не выставляется поздним таймером.
4. Смена трека сбрасывает прогресс в 0 без анимации «назад»; пауза останавливает полосу (transition доходит до последнего значения и стоит).
5. `?scale=2` (CSS zoom) и `?part=now|queue`: FLIP и раскладка частей не смещаются.

---

### Task 1: Помощники SDK, демо-цикл, документация

**Files:**
- Modify: `src-tauri/assets/widget/sdk.js`
- Modify: `src-tauri/tests/widget_sdk.mjs`
- Modify: `src-tauri/examples/widget_demo.rs`
- Modify: `docs/themes.md` (раздел «Анимации»)

**Interfaces — Produces** (поля объекта `window.StreamAssist`, рядом с `on`):
- Перед `emit('state', …)` SDK ставит `document.documentElement.dataset.mode = String(state.mode)`.
- `reduced: boolean` — геттер, `matchMedia?.('(prefers-reduced-motion: reduce)').matches === true`.
- `fresh(key: string, ids: Iterable<number|string>): Set` — ids, которых не было в прошлом вызове с тем же `key`; первый вызов возвращает все; хранит последний набор целиком (удалённые id при возвращении снова считаются свежими).
- `replay(element: Element|null, className: string): void` — `classList.remove`, чтение `offsetWidth`, `classList.add`. `null` — ничего.
- `show(element: HTMLElement, visible: boolean): void` — `visible`: снять `sa-leaving`, `hidden = false`, отменить ожидающее скрытие. `!visible`: если уже `hidden` или идёт выход — ничего; если `reduced` — `hidden = true`; иначе добавить `sa-leaving` и скрыть по первому `animationend` на самом элементе или через 400 мс (что раньше); перед скрытием проверить, что скрытие не отменено (токен/счётчик).
- `flip(container: HTMLElement, render: () => void): void` — до `render()` запомнить `offsetTop` детей с `dataset.key`; после — для детей с тем же ключом и ненулевой разницей `dy` вызвать `child.animate([{translate: '0 ' + dy + 'px'}, {translate: '0 0'}], {duration: 300, easing: 'cubic-bezier(.2,.8,.2,1)'})`. При `reduced` или без `Element.prototype.animate` — просто `render()`. `offsetTop` (а не `getBoundingClientRect`) — в CSS-пикселях без учёта zoom и transform.
- `progress(element: HTMLElement, percent: number, trackId: unknown): void` — `percent` ограничить 0–100 (NaN → 0). Если `trackId` изменился с прошлого вызова для этого элемента или `percent` меньше прошлого: `style.transition = 'none'`, `style.width = percent + '%'`, чтение `offsetWidth`, `style.transition = ''`. Иначе просто `style.width`. Состояние хранить в `WeakMap`. Плавность задаёт CSS темы: `transition: width 2.5s linear`.

- [ ] **Step 1: Тесты в `widget_sdk.mjs`** (vm-контекст как в текущих кейсах, с фейковыми `matchMedia`, `setTimeout`, элементами с `classList`/`dataset`/`offsetTop`/`animate`):
  - `dataset.mode` равен `'Playing'` после сообщения `{State:{…mode:'Playing'}}` в `socket.onmessage`;
  - `fresh('q',[1,2])` → `{1,2}`; `fresh('q',[2,3])` → `{3}`; `fresh('q',[1])` → `{1}`; другой ключ независим;
  - `show(el,false)` → `el.classList` содержит `sa-leaving`, `hidden` ещё false; после таймера 400 мс `hidden === true`; `show(el,false)` затем `show(el,true)` затем таймер → `hidden === false`, `sa-leaving` снят; при `reduced` `show(el,false)` скрывает сразу;
  - `flip`: ребёнок с `key=a` смещается с `offsetTop` 60 на 0 → `animate` вызван с `translate: '0 60px'`; новый ключ — без вызова; при `reduced` `animate` не вызывается;
  - `progress`: 10 → 20 (тот же трек) — `transition` не трогается; 20 → 5 — сброс через `'none'`; новый трек — сброс; `NaN` → `'0%'`.
- [ ] **Step 2:** `node src-tauri/tests/widget_sdk.mjs` — FAIL (нет функций).
- [ ] **Step 3:** реализовать помощники в `sdk.js` (существующее поведение и первые 7 кейсов теста не менять).
- [ ] **Step 4:** `node src-tauri/tests/widget_sdk.mjs` — PASS.
- [ ] **Step 5:** `widget_demo.rs`: при `DEMO=cycle` тик 2,5 с; каждые 8 с текущий трек уходит, первый из очереди становится текущим, в конец добавляется новый элемент с новым `id` (циклический список названий), `progress_ms` = 0; каждый 4-й цикл режим `Paused` на один цикл; каждый 5-й цикл `visible=false` на 3 с. Без `DEMO` поведение прежнее. `cargo build --example widget_demo` — OK.
- [ ] **Step 6:** `docs/themes.md`: раздел «Анимации» — `data-mode`, все помощники с сигнатурами, правило «анимация входа только через класс для свежих id», индивидуальные свойства вместо `transform`, reduced motion, `DEMO=cycle`.
- [ ] **Step 7:** commit `feat(widget): animation helpers in SDK and demo cycle`.

### Task 2: underground и glass

**Files:** `themes/underground/{index.html,style.css}`, `themes/glass/{index.html,style.css}`

**Interfaces — Consumes:** Task 1 (`show`, `fresh`, `replay`, `flip`, `progress`, `data-mode`).

Общий шаблон рендера (для всех тем в Tasks 2–4): `StreamAssist.show(el('widget'), state.visible)`; при смене `now?.id` — `replay(блок трека, 'sa-new')`; прогресс через `progress(el('progress'), pct, now?.id)`; очередь — `flip(container, () => container.replaceChildren(...rows))`, у каждой строки `dataset.key = item.id`, класс `sa-enter` если `id ∈ fresh('queue', ids)`, `style.setProperty('--i', min(index,4))`; счётчик — `replay(count,'sa-bump')` при изменении числа. CSS: `.prog i`/`.progress i{transition:width 2.5s linear}`, `#widget.sa-leaving{animation:<выход> …}`.

- [ ] **Step 1: underground** — диск: `@keyframes spin{to{rotate:360deg}}` 8 с linear infinite, `paused` вне `Playing`; `.np.sa-new .disc` — вкатывание (`translate` слева + `rotate` −180°→0, 600 мс); `.np.sa-new .box` — выезд из-под диска (`clip-path: inset(0 100% 0 0)` → `inset(0)`, 450 мс, задержка 120 мс), тексты `.a,.t,.by` — `translate` 12px→0 + opacity с задержками 200/260/320 мс; `.r.sa-enter` — въезд справа + лаймовая вспышка фона (через `box-shadow inset`/`background-color` keyframe, не меняя размеров), задержка `calc(var(--i)*60ms)`; `#count.sa-bump` — `scale` 1→1.35→1 200 мс; выход — `.np` уезжает влево и `.qp` вправо с opacity, 350 мс.
- [ ] **Step 2: glass** — вход `#widget`: opacity 0, `filter: blur(10px)`, `scale .97` → норма, 500 мс; `.sa-new` на `#widget`: `.cov` кроссфейд (opacity + `scale` 1.06→1), `.t,.a,.by` — подъём 6px с затуханием, задержки 60/120/180 мс; блик по прогрессу: `.prog` `position:relative;overflow:hidden`, `::after` с полупрозрачным градиентом, `translate` −100%→300% каждые 3,5 с, только в `Playing`; `.q.sa-enter` (строка «Далее» — один узел, класс через `replay` при смене id `queue[0]`) — подъём с затуханием; выход — обратный вход 300 мс.
- [ ] **Step 3: проверка** — запустить скрипт скриншотов (см. «Проверка» ниже) для `underground` и `glass`: финальные кадры с reduced motion совпадают с эталоном; кадры посреди анимации просмотрены.
- [ ] **Step 4:** commit `feat(widget): underground and glass animations`.

### Task 3: crt и block

**Files:** `themes/crt/{index.html,style.css}`, `themes/block/{index.html,style.css}`

- [ ] **Step 1: crt** — вход/`sa-new` на `#widget`: «включение кинескопа» (`scale: 1 .02` → `1 1` с `filter: brightness(2)` → 1, 450 мс); название печатается JS-ом: при смене `now.id` текст ставится по одному символу каждые 28 мс (не дольше 1,2 с — при длинном названии шаг = 1200/длина), предыдущая печать отменяется (таймер/токен); при `reduced` — сразу целиком; в тиках без смены трека текст не трогается; `#time::after{content:'_'}` мигает 1 с `steps(1)`; в `Playing` строки развёртки `#widget` бегут (`background-position` 0→0 3px, 0,4 с linear infinite) и мерцание `opacity` .97↔1 раз в 4 с; строки очереди `.sa-enter` — `clip-path: inset(0 100% 0 0)` → `inset(0)` с `steps(12)`, 360 мс, задержка по `--i`; выход — сжатие в линию (`scale: 1 .02`, затем opacity), 350 мс.
- [ ] **Step 2: block** — `sa-new` на `#widget`: `.t` «шлепок» (`scale` 1.6→0.92→1, `rotate` −8deg→0 через индивидуальное свойство, итоговый наклон −1,5° остаётся в `transform`), 380 мс, `cubic-bezier(.3,1.6,.5,1)`; `.a` и `.by` — падение сверху (`translate: 0 -14px` → 0) с задержкой 120/200 мс; `.cov` — выскакивание с поворотом (`scale` 0→1, `rotate` 25deg→0), 420 мс; `.q.sa-enter` — жёсткий въезд слева (`translate: -110% 0` → 0, `steps(4)` или резкий ease), 300 мс; выход — `translate` вниз + opacity, 300 мс.
- [ ] **Step 3: проверка** — скрипт скриншотов для `crt` и `block`; дополнительно: в crt при быстрой смене двух треков (демо с принудительным состоянием) отображается только второе название.
- [ ] **Step 4:** commit `feat(widget): crt and block animations`.

### Task 4: phantom и moonlight

**Files:** `themes/phantom/{render.js,style.css,index.html}`, `themes/moonlight/{render.js,style.css,index.html}`. `render.js` у тем — копии; менять обе одинаково (кроме существующих различий: группировка букв по словам в phantom).

- [ ] **Step 1: render.js (обе темы)** — `show`, `progress`, `flip` для `#queue` со строками `dataset.key`, `sa-enter` по `fresh`, `replay(el('now'),'sa-new')` при смене `now.id`, `replay(count,'sa-bump')`; в `ransom` каждой букве `style.setProperty('--i', index)` и класс `sa-enter` только когда название реально сменилось (существующая проверка `previousTitle`); заголовок «ОЧЕРЕДЬ» — без анимации букв.
- [ ] **Step 2: phantom** — `.letter.sa-enter` — появление по одной: `scale` 0→1.25→1, `rotate` 40deg→0 (индивидуальное свойство, inline `transform` наклон сохраняется), opacity, 260 мс, задержка `calc(var(--i)*35ms)` (не больше 40 букв); `.burst` — в `Playing` «дыхание» `scale` 1↔1.04 за 2,4 с; `.now.sa-new .burst` — поворот `rotate` −90deg→0 + `scale` .6→1, 500 мс; `.now.sa-new .cover` — влёт `translate: -40px 20px` + `rotate: -25deg` → 0, 450 мс; `#artist,#requester` — въезд справа с задержкой 250/320 мс; `.row.sa-enter` — врезка справа `translate: 60px 0` → 0 с opacity, 280 мс, задержка по `--i`; `#count.sa-bump` — `scale`; выход — `.now` и `.queue-panel` уезжают (`translate`) и гаснут, 350 мс.
- [ ] **Step 3: moonlight** — существующий `@keyframes enter` (у `.now`/`.queue-panel` собственного `transform` нет, поэтому `skewX` в нём допустим) оставить и повторять его для `.now.sa-new`; `.now.sa-new::after` (новый псевдоэлемент поверх фона, `pointer-events:none`, тот же `clip-path`, что у `::before`) — световая полоса: градиент проходит слева направо 700 мс; `.now.sa-new .cover` — выезд со скосом слева (`translate: -30px 0`, opacity), 450 мс; `.backword` — в `Playing` дрейф `translate` 0↔−18px за 9 с ease-in-out alternate; `.progress i:after` — пульс свечения (`box-shadow`/opacity) 1,6 с в `Playing`; `.row.sa-enter` — выезд слева `translate: -40px 0` + opacity, 320 мс, задержка по `--i`; выход — `translate: -12px 0` + opacity 300 мс.
- [ ] **Step 4: проверка** — скрипт скриншотов для `phantom` и `moonlight`.
- [ ] **Step 5:** commit `feat(widget): phantom and moonlight animations`.

## Проверка (скрипт скриншотов)

Скрипт держит исполнитель-проверяющий вне репозитория. Для каждой темы и части `all|now|queue`:
1. `THEME=<t> PORT=<p> cargo run --example widget_demo` (без `DEMO`, статичное состояние).
2. `chromium --headless=new --force-prefers-reduced-motion --hide-scrollbars --window-size=<width,height из theme.json/parts> --virtual-time-budget=4000 --screenshot=<file> http://127.0.0.1:<p>/widget/music?part=<part>`.
3. Эталон снимается с `main` до изменений; после изменений — побайтовое/пиксельное сравнение (ImageMagick `compare -metric AE`, допуск 0). Обложек нет (`cover_url: null`), поэтому кадры детерминированы.
4. Без `--force-prefers-reduced-motion` и с `--virtual-time-budget=150` — кадр посреди входа, просматривается глазами.
5. `DEMO=cycle` — ручной просмотр в Chromium и OBS (пользователь).
