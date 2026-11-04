# Анимации виджетов — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Шаги — чекбоксы.

**Goal:** анимации смены трека, очереди, прогресса, показа/скрытия и фоновых циклов во всех шести встроенных темах, в стиле каждой темы.

**Architecture:** общие помощники в `sdk.js`: `data-mode`, `replay`, `show`, `progress`, `list` (список по ключам с переиспользованием узлов, входом новых строк и FLIP). Темы описывают анимации в CSS и вызывают помощники. Анимации привязаны к событиям (новый `id`, смена трека), а не к каждому состоянию.

**Tech Stack:** vanilla JS, CSS animations, Web Animations API, Chromium/CEF (OBS).

**Spec:** `docs/superpowers/specs/2026-11-04-widget-animations.md`

## Global Constraints

- **Появление:** у каждой темы есть базовая анимация `appear` (без класса) на панелях: underground `.np` и `.qp`, glass/crt/block `#widget`, phantom/moonlight — существующий `enter` на `.now`/`.queue-panel`. Она играет при загрузке и при каждом показе (выход из `display:none` перезапускает CSS-анимации). Ей тоже `backwards`.
- Минимум — Chromium 104 (индивидуальные `translate`/`scale`/`rotate`, в т. ч. в WAAPI); указать в docs/themes.md.
- Файлы: `src-tauri/assets/widget/sdk.js`, `src-tauri/assets/widget/themes/*/{index.html,style.css,render.js}`, `src-tauri/tests/widget_sdk.mjs`, `src-tauri/examples/widget_demo.rs`, `docs/themes.md`. `tts.html` не трогать. Без зависимостей и внешних запросов. Данные — только `textContent`/узлы.
- **Свойства:** входы, выходы и FLIP анимируют `translate`/`scale`/`rotate` (индивидуальные), `opacity`, `filter`, `clip-path`, `background-position`, `box-shadow` — **не `transform`**, кроме элементов без собственного `transform` (`.now`/`.queue-panel` в moonlight/phantom, `#widget` всех тем). Единственное layout-свойство — `width` полосы прогресса (`transition: width 2.5s linear`), это разрешённое отступление.
- **Каскад `animation`:** один элемент — один итоговый список. Если у элемента есть цикл и вход/выход, правило входа/выхода пишет **полный список через запятую** (`animation: roll .6s cubic-bezier(...) backwards, spin 8s linear infinite`) и соответствующий список `animation-play-state` — на паузе ставится на паузу только цикл: `html:not([data-mode="Playing"]) .np.sa-new .disc{animation-play-state:running,paused}` и так же `html:not([data-mode="Playing"]) .now.sa-new .burst{animation-play-state:running,paused}` (shorthand `animation` сбрасывает play-state, поэтому правило паузы обязательно для каждого такого списка). Класс `sa-enter` — ровно одни `@keyframes` на элемент (вспышку и въезд объединять в один keyframes), т. к. `list` снимает класс по первому `animationend`. Правила `.sa-leaving` стоят **последними** в файле темы.
- **fill-mode:** входы с задержкой — `backwards`; `forwards`/`both` запрещены (конечное состояние = обычный стиль). Исключение — существующий `enter … both` в phantom/moonlight (его `to` равен покою), не трогать. Нулевой кадр каждого цикла (`0%`) совпадает с обычным стилем элемента.
- **Повтор анимации:** `replay` работает только если имя анимации меняется при добавлении класса; поэтому вход по `sa-new` использует отдельные `@keyframes` от анимации первой загрузки (например, `enter` и `enter-new`).
- Циклы идут только при `html[data-mode="Playing"]`, иначе `animation-play-state: paused`.
- **Reduced motion** в каждой теме: `@media (prefers-reduced-motion: reduce){*,*::before,*::after{animation:none!important;transition:none!important}}`. Помощники SDK при reduced motion не анимируют.
- **Выход при скрытии** задаётся анимацией на самом `#widget` (`#widget.sa-leaving`, ≤ 400 мс); анимации дочерних частей — той же длительности.
- Конечные кадры совпадают с эталоном пиксель в пиксель (см. «Проверка»).
- Вход 250–700 мс, задержки 40–120 мс, строки очереди — `calc(var(--i) * 60ms)`, `--i = min(index, 4)`. FLIP 300 мс. Циклы не мигают чаще 3 Гц.

## Review Focus

1. Обновление состояния раз в 2,5 с без изменений не перезапускает и не обрывает ни одну анимацию (входы строк, название, буквы phantom, печать crt, циклы).
2. Быстрая смена трека во время входа или печати crt: показывается новый трек, старая печать не дописывается.
3. Скрытие → показ во время выхода: виджет остаётся видимым; всплывший `animationend` дочернего элемента не скрывает виджет.
4. Смена трека сбрасывает прогресс в 0 без анимации «назад»; первая отрисовка ставит ширину без анимации; пауза останавливает полосу.
5. `?scale=2` и `?part=now|queue`: FLIP и раскладка частей не смещаются.

---

### Task 0: Эталон (выполняет оркестратор, уже сделано)

Скриншоты всех тем × `all|now|queue|queue&scale=2` сняты с `4594dc9` (темы как в `main`, демо с `MODE`/`PROGRESS`) для `MODE=Playing` и `MODE=Paused`, `PROGRESS=60000`; детерминированность подтверждена двумя прогонами, пустых кадров нет.

### Task 1: Помощники SDK и демо-цикл

**Files:** `src-tauri/assets/widget/sdk.js`, `src-tauri/tests/widget_sdk.mjs`, `src-tauri/examples/widget_demo.rs`

**Interfaces — Produces** (поля `window.StreamAssist` рядом с `on`; внутренний `reduced()` = `typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches`):
- Перед `emit('state', …)`: `document.documentElement.dataset.mode = String(state.mode)`.
- `replay(element: Element|null, className: string): void` — `classList.remove`, чтение `offsetWidth`, `classList.add`; `null` — ничего.
- `show(element: HTMLElement, visible: boolean): void` — `visible`: отменить ожидающее скрытие (токен), снять `sa-leaving`, `hidden = false`. `!visible`: если `hidden` или уже уходит — ничего; если `reduced()` — `hidden = true`; иначе добавить `sa-leaving`; если `getComputedStyle(element).animationName === 'none'` — скрыть сразу; иначе скрыть по `animationend` с `event.target === element` или через 400 мс, что раньше; скрытие выполняется только если токен не сменился; после скрытия снять `sa-leaving`.
- `progress(element: HTMLElement, percent: number, trackId: unknown): void` — `percent` → 0–100, не число → 0. Состояние в `WeakMap`. Первый вызов для элемента, смена `trackId` или уменьшение `percent`: `style.transition = 'none'`, `style.width = p + '%'`, чтение `offsetWidth`, `style.transition = ''`. Иначе только `style.width`.
- `list(container: HTMLElement, items: Array, keyOf: (item) => string|number, build: (item, index) => HTMLElement): void` — ключевой рендер:
  1. Запомнить `offsetTop` текущих детей по `dataset.key`.
  2. Для каждого `item` (по порядку): `node = build(item, index)`, `node.dataset.key = String(keyOf(item))`, `node.style.setProperty('--i', Math.min(index, 4))`. Если есть старый ребёнок с этим ключом — оставить старый элемент: если `old.textContent !== node.textContent`, `old.replaceChildren(...node.childNodes)`; `old.className = node.className` + `' sa-enter'`, если он был у старого; обновить `--i`. Иначе — новый узел с классом `sa-enter`, кроме случая `reduced()`.
  3. Привести детей контейнера к итоговому порядку без переустановки сохранённых узлов: удалить ушедшие (`remove()`), затем по порядку `if (container.children[i] !== node) container.insertBefore(node, container.children[i] || null)` (переустановка перезапустила бы их CSS-анимации).
  4. Для сохранённых узлов без `sa-enter` (у узлов в процессе входа FLIP не делается) с изменившимся `offsetTop` (`dy = было − стало ≠ 0`) и `typeof node.animate === 'function'` и не `reduced()`: `node.animate([{translate: '0 ' + dy + 'px'}, {translate: '0 0'}], {duration: 300, easing: 'cubic-bezier(.2,.8,.2,1)'})`.
  5. `sa-enter` снимается с узла по его `animationend` (`target === node`) или через 1500 мс (если у темы нет анимации `sa-enter`), что раньше.
  Первый вызов для контейнера: все строки новые (получают `sa-enter`).

- [ ] **Step 1: тесты** в `widget_sdk.mjs` (vm-контекст; `matchMedia`, `getComputedStyle`, `setTimeout` — глобалы контекста, не поля `window`; фейковые элементы с `classList`, `dataset`, `style.setProperty`, `offsetTop`, `textContent`, `replaceChildren`, `addEventListener`, `animate`):
  - `dataset.mode === 'Playing'` после `socket.onmessage({data: JSON.stringify({State:{visible:true,now:null,queue:[],progress_ms:0,mode:'Playing',warning:null}})})`;
  - `replay`: `remove` до `add`, между ними читается `offsetWidth`; `replay(null,'x')` не бросает;
  - `show`: скрытие ждёт таймер 400 мс; `animationend` с `target === el` скрывает сразу; `animationend` от ребёнка игнорируется; `show(false)`→`show(true)`→таймер — виден, `sa-leaving` снят; `animationName === 'none'` — скрыт сразу; reduced — скрыт сразу;
  - `progress`: первый вызов — сброс через `'none'`; 10→20 тот же трек — без сброса; 20→5 — сброс; новый трек — сброс; `NaN` → `'0%'`, 150 → `'100%'`;
  - фейковый контейнер вычисляет `offsetTop` ребёнка как `index × 30` (по текущему порядку детей), записывает вызовы `insertBefore`/`remove`; фейковый `style` записывает все присваивания `transition`;
  - `list`: первый вызов `[a,b]` — оба `sa-enter`; затем `animationend` с `target === b` снимает `sa-enter` у `b` (а `animationend` от ребёнка `b` — нет); таймер 1500 мс снимает его у `a`; второй вызов `[b,c]` — узел `b` тот же объект, без `sa-enter` (если не был), `c` новый с `sa-enter`, `a` удалён; при сдвиге (`build` даёт новый номер строки) узел `b` остаётся тем же объектом, а его `textContent` обновлён; `b` был на позиции 1 (`offsetTop` 30) и стал на 0 — `animate` вызван с `translate: '0 30px'`; узел в процессе входа (`sa-enter`) FLIP не получает; сохранённый узел, уже стоящий на месте, не переустанавливается (`insertBefore` для него не вызван); при reduced — нет `sa-enter` и нет `animate`; без `animate` у узла — не бросает; `--i` у пятой и шестой строки = 4.
- [ ] **Step 2:** `node src-tauri/tests/widget_sdk.mjs` — FAIL.
- [ ] **Step 3:** реализовать в `sdk.js` (существующие 7 кейсов не менять).
- [ ] **Step 4:** `node src-tauri/tests/widget_sdk.mjs` — PASS.
- [ ] **Step 5: `widget_demo.rs`** — (`MODE` и `PROGRESS` уже есть.) `DEMO=cycle`: тик 2,5 с; цикл = 3 тика; на границе цикла сразу (без ожидания тика) отправить состояние: текущий трек уходит, первый из очереди — текущий (`started=true`), `progress_ms = 0`; в очередь добавляется 0, 1 или 2 новых элемента (по номеру цикла, `id` растёт, названия из циклического списка), длина очереди 2–7; каждый 4-й цикл — `mode = "Paused"` на весь цикл; каждый 5-й цикл — `visible = false` на первый тик. Без `DEMO` — прежнее поведение (включая `PROGRESS`). `cargo build --example widget_demo` — OK, `cargo fmt`.
- [ ] **Step 6:** commit `feat(widget): animation helpers in SDK and demo cycle`.

### Общий шаблон рендера тем (Tasks 2–4)

`StreamAssist.show(el('widget'), state.visible)`; когда `now?.id != null` и отличается от прошлого (включая первую отрисовку) — `replay` класса `sa-new` на блоке трека: underground `.np`; glass `.info` и `.cov`; crt `.info`; block `.info` и `.cov`; phantom/moonlight `#now` (при `now → null` блок просто скрывается/очищается, без `sa-new`); прогресс — `progress(el('progress'), pct, now?.id)`; очередь — `list(el('queue'), <видимые элементы>, i => i.id, build)` (в phantom/moonlight видимые = `queue.slice(0,5)`); одиночная строка «Далее/NEXT» (glass, block) — `replay(el('queue'), 'sa-enter')` при смене `queue[0]?.id`; счётчик — `replay(count, 'sa-bump')` при изменении числа. CSS полосы: `transition: width 2.5s linear`.

### Task 2: underground и glass

**Files:** `themes/underground/{index.html,style.css}`, `themes/glass/{index.html,style.css}`

- [ ] **Step 1: underground** — диск: `spin` (`rotate` 0→360deg, 8 с linear infinite), paused вне `Playing`; `.np.sa-new .disc` — полный список: вкатывание (`translate: -60px 0`→0, `rotate: -180deg`→0, 600 мс, backwards) + `spin`; `.np.sa-new .box` — `clip-path: inset(0 100% 0 0)`→`inset(0)` (fill backwards: после анимации `clip-path` нет, `.tab` и тень не обрезаны), 450 мс, задержка 120 мс; `.a,.t,.by` — `translate: 12px 0`→0 + opacity, задержки 200/260/320 мс; `.r.sa-enter` — въезд справа (`translate: 40px 0`) + лаймовая вспышка `box-shadow: inset 0 0 0 999px #c6f27a55` → none, задержка по `--i`; `#count.sa-bump` — `scale` 1→1.35→1, 200 мс; `#widget.sa-leaving` — opacity 1→0, 350 мс, и `#widget.sa-leaving .np` / `.qp` — `translate` влево/вправо 350 мс.
- [ ] **Step 2: glass** — `#widget` загрузка: `appear` (opacity 0, `filter: blur(10px)`, `scale .97` → норма, 500 мс); смена трека — `sa-new` на `.info` и `.cov` (не на `#widget`): `.cov` — opacity + `scale` 1.06→1, `.t,.a,.by` — `translate: 0 6px`→0 + opacity, задержки 60/120/180 мс; блик: `.prog{position:relative;overflow:hidden}` `.prog::after` — полоса-градиент, `translate` −100%→300% (нулевой кадр за пределами полосы), 3,5 с infinite, только `Playing`; `.q.sa-enter` — подъём с затуханием 350 мс; `#widget.sa-leaving` — opacity→0 + `filter: blur(8px)`, 300 мс.
- [ ] **Step 3:** скрипт проверки для обеих тем — PASS; кадры середины анимации просмотрены.
- [ ] **Step 4:** commit `feat(widget): underground and glass animations`.

### Task 3: crt и block

**Files:** `themes/crt/{index.html,style.css}`, `themes/block/{index.html,style.css}`

- [ ] **Step 1: crt** —
  - `#time` в `index.html` один раз размечается как `<span id="mode"></span><span class="cur">_</span><span id="clock"></span>`; рендер обновляет только `textContent` у `#mode` (`'> ' + MODE`) и `#clock` (`' ' + время`), `.cur` не пересоздаётся (вид и пиксели как сейчас); `.cur` мигает (opacity 1↔0, 1 с `steps(1)`, нулевой кадр — видим) только в `Playing`.
  - Смена трека: `sa-new` на `.info` — «включение кинескопа» (`scale: 1 .02`→`1 1`, `filter: brightness(2)`→1, 450 мс). Название печатается без перекладки строк: полный текст в `#title`, где введённая часть — текст, а остальная — `<span class="ghost">` с `color: transparent; text-shadow: none`; каждые `min(28, 1200/длина)` мс один символ переходит из ghost в текст; новая смена трека отменяет таймер; при reduced motion — сразу полностью; тики без смены трека не трогают `#title`.
  - Развёртка в `Playing`: градиент остаётся на `#widget`; цикл `scan` анимирует `background-position` от `0 0` (покой) до `0 3px`, 0,4 с linear infinite, только `Playing` (в правилах `#widget` с `appear`/`sa-leaving` — полный список). Мерцание не делать. Проверить кадр середины цикла на шов у края; при шве — перенести полосы в отдельный дочерний слой того же размера, не меняя `background-size` у `#widget`.
  - Строки очереди `.sa-enter` — `clip-path: inset(0 100% 0 0)`→`inset(0)` `steps(12)`, 360 мс, задержка по `--i`.
  - `#widget.sa-leaving` — `scale: 1 .02` затем opacity 0, 350 мс.
- [ ] **Step 2: block** — `sa-new` на `.info` и `.cov`: `.t` — «шлепок» (`scale` 1.6→0.92→1, `rotate` −8deg→0, 380 мс, `cubic-bezier(.3,1.6,.5,1)`; базовый `transform: rotate(-1.5deg)` сохраняется); `.a`, `.by` — `translate: 0 -14px`→0 с задержкой 120/200 мс; `.cov` — `scale` 0→1 + `rotate` 25deg→0, 420 мс; `.q.sa-enter` — `translate: -110% 0`→0, `steps(4)`, 300 мс; `#widget.sa-leaving` — `translate: 0 20px` + opacity→0, 300 мс.
- [ ] **Step 3:** скрипт проверки для обеих — PASS; crt: состояние A, затем через 300 мс состояние B — после 2 с в `#title` только название B (проверить в DEMO=cycle глазами и тестом-скриптом в браузере по желанию).
- [ ] **Step 4:** commit `feat(widget): crt and block animations`.

### Task 4: phantom и moonlight

**Files:** `themes/phantom/{render.js,style.css}`, `themes/moonlight/{render.js,style.css}` (`render.js` — копии; изменения одинаковые, сохраняя существующие различия).

- [ ] **Step 1: render.js** — шаблон рендера; блок трека `el('now')` получает `sa-new`; в `ransom` при реальной смене названия каждая буква получает `--i = index` и класс `sa-enter` (не при reduced motion); заголовок «ОЧЕРЕДЬ» без анимации.
- [ ] **Step 2: phantom** — `.letter.sa-enter`: `scale` 0→1.25→1, `rotate` 40deg→0, opacity, 260 мс, задержка `calc(var(--i) * 35ms)`, backwards (inline `transform` наклона сохраняется); `.burst`: `breathe` (`scale` 1→1.04→1, 2,4 с infinite, только `Playing`); `.now.sa-new .burst` — полный список `burst-in` (`rotate: -90deg`→0, 500 мс) + `breathe` (у `burst-in` только `rotate`, чтобы не спорить за `scale`); `.now.sa-new .cover` — `translate: -40px 20px` + `rotate: -25deg` → 0, 450 мс; `#artist`, `#requester` — `translate: 30px 0`→0 + opacity, задержки 250/320 мс; `.now.sa-new` — `enter-new` (копия `enter`); `.row.sa-enter` — `translate: 60px 0`→0 + opacity, 280 мс, задержка по `--i`; `#count.sa-bump` — `scale`; `#widget.sa-leaving` — opacity→0, `.now`/`.queue-panel` — `translate` в стороны, 350 мс.
- [ ] **Step 3: moonlight** — `.now.sa-new` — `enter-new`; `.now::after` (поверх фона, под текстом: `.now::after{z-index:0}`, `.backword,.cover,.details{z-index:1}` (у `.backword` сохранить его позиционирование), тот же `inset` и `clip-path`, что у `::before`, `pointer-events:none`, в покое `opacity: 0`) — при `.sa-new` световая полоса: градиент `background-position` проходит слева направо, 700 мс; `.now.sa-new .cover` — `translate: -30px 0` + opacity, 450 мс (базовый `transform: skew` сохраняется); `.backword` — `translate` 0↔−18px, 9 с ease-in-out infinite alternate, только `Playing`, нулевой кадр = покой; `.progress i:after` — пульс `box-shadow` 1,6 с, только `Playing`, нулевой кадр = покой; `.row.sa-enter` — `translate: -40px 0`→0 + opacity, 320 мс, задержка по `--i`; `#count.sa-bump` — `scale` 1→1.3→1, 200 мс; `#widget.sa-leaving` — `translate: -12px 0` + opacity→0, 300 мс.
- [ ] **Step 4:** скрипт проверки для обеих — PASS.
- [ ] **Step 5:** commit `feat(widget): phantom and moonlight animations`.

### Task 5: Документация

**Files:** `docs/themes.md`

- [ ] **Step 1:** раздел «Анимации (экспериментально)»: `data-mode` и значения, `replay`, `show`, `progress`, `list` с сигнатурами и примером; правила: вход только через `sa-enter`/`sa-new`, индивидуальные свойства вместо `transform`, полный список `animation` при цикле + входе, `backwards`, отдельные keyframes для повтора, reduced motion с `*::before,*::after`, пример `matchMedia`; `DEMO=cycle`, `MODE`, `PROGRESS` для предпросмотра.
- [ ] **Step 2:** commit `docs(widget): animation helpers and rules`.

## Проверка (скрипт оркестратора, вне репозитория)

Для каждой темы и вида `all`, `now`, `queue`, `queue&scale=2`; демо `PROGRESS=60000`:
1. **Reduced:** `MODE=Playing`, `chromium --headless=new --force-prefers-reduced-motion --virtual-time-budget=4000 --screenshot`.
2. **Конечный кадр:** `MODE=Paused`, без reduced motion, `--virtual-time-budget=4000` (все входы закончились, циклы стоят на нулевом кадре); сравнивается с эталоном, снятым с `MODE=Paused` (crt выводит режим текстом).
3. Оба кадра побайтно равны эталону (`cmp`), и в кадре есть непрозрачные пиксели (`magick … -format %[fx:mean.a] info:` > 0).
4. Если Paused-кадр расходится с эталоном только из-за слоя на остановленном цикле (например, диск underground), допустимый запасной вариант — объявлять циклы только под `html[data-mode="Playing"]` для этого элемента; отметить в отчёте.
5. Кадры середины входа (`--virtual-time-budget=150`/`400`, без reduced) — просмотр глазами.
6. `DEMO=cycle` с `?part=queue&scale=2` — просмотр сдвига строк (FLIP) посреди анимации: строки не прыгают на удвоенное расстояние.
7. `DEMO=cycle` — ручной просмотр в Chromium и OBS (пользователь), в т. ч. нагрузка CPU в OBS.
