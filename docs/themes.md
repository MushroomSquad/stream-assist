# Темы музыкального виджета

Встроенные темы: `underground` (по умолчанию), `glass`, `crt`, `block`, `phantom` (Phantom), `moonlight` (Moonlight).
Адрес для браузерного источника OBS: `http://127.0.0.1:47821/widget/music`
(порт зависит от настройки приложения). Фон страницы должен быть прозрачным.
Размер источника задаётся полями `width` и `height` темы.

## Файлы и импорт

ZIP содержит файлы непосредственно в корне, без внешней папки:

```text
theme.json
index.html
style.css
fonts/optional.woff2
```

Пример `theme.json` (базовые поля обязательны, `parts` необязательно):

```json
{"id":"my-theme","name":"Моя тема","author":"Автор","width":800,"height":420}
```

`id` — имя одной папки без `/`, `\`, `:`, `.` и `..` как отдельных
компонентов пути; рекомендуется латиница, цифры и дефис. Размеры — целые
неотрицательные числа в пикселях (`u32`). Импорт распаковывает архив в
`<каталог тем>/<id>`. Повторный импорт существующей папки отклоняется.
Абсолютные пути, переходы `..` и символические ссылки запрещены.
Пользовательская тема с id встроенной темы перекрывает её.

Ресурсы доступны по `/widget/theme/<id>/<путь>`. Используйте этот абсолютный
префикс для CSS, скриптов, изображений и шрифтов: основная страница открывается
по `/widget/music`. Подключайте шрифты из архива либо системные; внешние
шрифтовые сервисы не нужны. Phantom и Moonlight включают локальные OFL-шрифты и тексты лицензий.

## SDK и события

В конце `body`, перед кодом подписок, подключите:

```html
<script src="/widget/sdk.js"></script>
<script>
StreamAssist.on('state', state => {
  document.getElementById('widget').hidden = !state.visible;
  document.getElementById('title').textContent = state.now?.track.title || '';
});
</script>
```

`StreamAssist.on(name, callback)` регистрирует обработчик. SDK сам открывает
WebSocket, переподключается через 2 секунды после закрытия и перезагружает
страницу при `Reload`. Первое состояние приходит после соединения; подписки
нужно зарегистрировать сразу после загрузки SDK. Для каждого состояния события
вызываются в порядке:

| Событие | Аргумент |
| --- | --- |
| `state` | полный `WidgetState` |
| `nowPlaying` | `state.now`, объект или `null` |
| `queue` | `state.queue`, массив ожидающих треков |
| `progress` | `state.progress_ms`, миллисекунды |
| `hidden` | `!state.visible`, логическое значение |

Поля `WidgetState`:

| Поле | Значение |
| --- | --- |
| `visible` | Нужно ли показывать виджет; при `false` скрывайте весь интерфейс |
| `now` | Текущий `QueueItem` или `null` |
| `queue` | Массив `QueueItem` в порядке очереди |
| `progress_ms` | Позиция текущего трека в миллисекундах |
| `mode` | Строка состояния автомата воспроизведения |
| `warning` | Предупреждение или `null` |

`QueueItem`: `id` (число), `track`, `requester` (строка), `started` (логическое),
`redemption` (`null` или объект с `reward_id` и `redemption_id`).

`track`: `title`, `artist` (строки), `duration_ms` (миллисекунды или `null`),
`explicit` (логическое или `null`), `cover_url` (URL или `null`), `source`
(`{"Spotify":{"uri":"…"}}` либо `{"YouTube":{"video_id":"…"}}`).

Данные трека и пользователя вставляйте только через `textContent`, никогда
через `innerHTML`. Для очереди создавайте DOM-элементы и используйте
`replaceChildren`. При отсутствии `cover_url` или ошибке загрузки изображения
показывайте заглушку. Прогресс ограничивайте диапазоном 0–100%; при неизвестной
или нулевой длительности показывайте 0%. Соблюдайте `[hidden]{display:none!important}`,
чтобы стили сетки или flex не отменяли скрытие.

## Анимации (экспериментально)

Контракт экспериментальный на один выпуск. Минимальная версия — Chromium/CEF 104
(в том числе для индивидуальных `translate`, `scale`, `rotate` в Web Animations API).
Перед обработчиками состояния SDK задаёт `data-mode` на `<html>`:
`Idle`, `Starting`, `Playing`, `Paused` или `Suspended`. Фоновые циклы должны идти
только при `html[data-mode="Playing"]`; в остальных режимах ставьте их на паузу.

Все помощники доступны на `StreamAssist`:

| Сигнатура | Поведение |
| --- | --- |
| `replay(element: Element \| null, className: string): void` | Снимает класс, читает `offsetWidth` и возвращает класс для повторного входа; `null` игнорируется |
| `show(element: HTMLElement, visible: boolean): void` | Показывает сразу, отменяя ожидающее скрытие. При скрытии добавляет `sa-leaving`, ждёт `animationend` самого элемента (не дочернего) или максимум 400 мс, затем ставит `hidden`. Без CSS-анимации или при reduced motion скрывает сразу |
| `progress(element: HTMLElement, percent: number, trackId: unknown): void` | Ограничивает ширину диапазоном 0–100%, нечисловое значение заменяет на 0. Первый вызов, смена `trackId` и уменьшение процента устанавливают ширину без перехода; остальные обновления используют CSS `transition` |
| `list(container: HTMLElement, items: Array, keyOf: (item) => string \| number, build: (item, index) => HTMLElement): void` | Переиспользует строки по ключу, удаляет ушедшие, добавляет новые с `sa-enter` и анимирует сдвиг сохранённых через FLIP за 300 мс |

Для `list` возвращайте новый DOM-узел из `build`, задавайте устойчивый уникальный
ключ (обычно `item.id`). SDK записывает его в `data-key`, а `--i` — в
`Math.min(index, 4)`. Сохранённый узел остаётся на месте, если порядок не изменился;
его дочерние узлы заменяются только при изменении `textContent`, класс обновляется
из результата `build` с сохранением активного `sa-enter`. Поэтому не рассчитывайте
на обновление атрибутов дочерних узлов при неизменном тексте.
`sa-enter` снимается по собственному `animationend` строки или через 1500 мс.
Строки во время входа не получают FLIP; при reduced motion нет ни входа, ни FLIP.
Сдвиг вычисляется через `offsetTop`, чтобы `?scale=2` не удваивал расстояние.

Пример для разметки с `#widget`, `#now`, `#title`, `#progress`, `#queue`
(SDK уже подключён; панели изначально скрыты):

```js
const el = id => document.getElementById(id);
let previousId;
StreamAssist.on('state', state => {
  const now = state.now;
  StreamAssist.show(el('widget'), state.visible);
  el('now').hidden = !now;
  el('title').textContent = now?.track.title || '';
  if (now?.id != null && now.id !== previousId) {
    StreamAssist.replay(el('now'), 'sa-new');
  }
  previousId = now?.id;
  const duration = now?.track.duration_ms;
  const percent = duration > 0 ? state.progress_ms / duration * 100 : 0;
  StreamAssist.progress(el('progress'), percent, now?.id);
  StreamAssist.list(el('queue'), state.queue.slice(0, 5), item => item.id,
    (item, index) => {
      const row = document.createElement('div');
      row.className = 'row';
      row.textContent = `${index + 1}. ${item.track.title}`;
      return row;
    });
});
```

Входы при обновлении данных запускайте только через `sa-new` (новый `now.id`,
включая первый трек) и `sa-enter` (новая строка). Обычный опрос раз в 2,5 с
не должен повторять вход, пересоздавать сохранённые строки или обрывать печать
названия. При `now → null` скрывайте/очищайте блок без `sa-new`. Для одиночной
строки «Далее» вызывайте `replay(queue, 'sa-enter')` только при смене первого id;
для счётчика — `replay(count, 'sa-bump')` только при изменении числа.
Если текст печатается таймером, отменяйте старый таймер при смене трека.

Правила CSS для сохранения исходной вёрстки:

- Базовый вход панели (`appear` без класса) играет при загрузке и повторном
  показе после `display:none`. Для повторного входа по `sa-new` задайте отдельные
  keyframes (например, `enter-new`): `replay` работает, когда добавление класса
  меняет имя анимации.
- Анимируйте индивидуальные `translate`, `scale`, `rotate`, а также `opacity`,
  `filter`, `clip-path`, `background-position`, `box-shadow`. `transform` допустим
  только на элементах без собственного `transform`, иначе потеряется наклон темы.
  Единственное исключение для layout — полоса прогресса:
  `#progress { transition: width 2.5s linear; }`. На паузе не увеличивайте процент.
- Входы длятся 250–700 мс, задержки между элементами — 40–120 мс;
  для строк используйте `animation-delay: calc(var(--i) * 60ms)`.
  У `.sa-enter` должна быть ровно одна анимация: вспышку и въезд объединяйте
  в одни keyframes, поскольку SDK снимает класс по первому `animationend`.
- Используйте `backwards`, особенно при задержке. Не используйте `forwards`/`both`:
  конечный кадр должен совпадать с обычным стилем пиксель в пиксель.
  Исключение — существующий `enter … both` в Phantom/Moonlight, его не меняйте.
  Нулевой кадр каждого цикла должен совпадать с обычным стилем.
- Один элемент получает один полный список `animation`. Если есть вход и цикл,
  перечисляйте оба и явно задавайте паузу только циклу (shorthand сбрасывает
  `animation-play-state`). Например:

  ```css
  .disc { animation: spin 8s linear infinite; }
  .np.sa-new .disc {
    animation: roll 600ms ease-out backwards, spin 8s linear infinite;
  }
  html:not([data-mode="Playing"]) .disc { animation-play-state: paused; }
  html:not([data-mode="Playing"]) .np.sa-new .disc {
    animation-play-state: running, paused;
  }
  ```

- Циклы не должны мигать чаще 3 Гц. Выход задавайте на самом `#widget.sa-leaving`
  длительностью не более 400 мс; выходы дочерних панелей — той же длительности.
  Правила `.sa-leaving` размещайте последними в файле темы.

В каждой теме отключайте анимации и переходы при reduced motion, включая
псевдоэлементы:

```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { animation: none !important; transition: none !important; }
}
```

Для JS-эффектов (например, печати названия) проверяйте настройку самостоятельно
и сразу показывайте полный текст:

```js
const reduced = typeof matchMedia === 'function' &&
  matchMedia('(prefers-reduced-motion: reduce)').matches;
```

`replay` сам эту проверку не делает: CSS выше отключает его анимацию.
`show` и `list` учитывают настройку в SDK.

Для предпросмотра из `src-tauri` запустите:

```sh
THEME=glass DEMO=cycle cargo run --example widget_demo
```

Откройте напечатанный адрес в браузере или OBS. `DEMO=cycle` обновляет состояние
каждые 2,5 с и меняет трек каждые три тика (7,5 с), начиная сразу; очередь
пополняется и сдвигается, каждый четвёртый цикл приостановлен, каждый пятый
скрывает виджет на первый тик. Проверьте также `?part=now`, `?part=queue`
и `?part=queue&scale=2`.

Без `DEMO=cycle` переменная `MODE` задаёт режим (по умолчанию `Playing`),
а `PROGRESS` фиксирует позицию в миллисекундах для снимков, например:

```sh
THEME=glass MODE=Paused PROGRESS=60000 cargo run --example widget_demo
```

Для сравнения конечного кадра дождитесь завершения входов; проверьте `Paused`
без reduced motion и `Playing` с reduced motion. Вёрстка, текст, цвета и размеры
должны совпадать с исходной темой во всех частях и масштабах.

## YouTube и OBS

Предусмотрите `<div id="sa-youtube"></div>` вне скрываемого интерфейса трека.
SDK заменяет этот элемент YouTube-плеером в OBS. Его можно разместить за
пределами кадра (например, `position:fixed;left:-300px;top:0;width:200px;height:200px`),
но не скрывать через `display:none`. Если элемента нет, SDK создаёт его сам.
Не создавайте собственный плеер и не отправляйте события подтверждения старта:
это ответственность SDK. В OBS (`window.obsstudio` существует) SDK использует
роль `player` для `now`/`all`, а для `queue` и в обычном браузере — `view`. Повторный OBS-плеер вытесняет
предыдущий, который переходит в режим просмотра.

Сами темы не требуют внешних запросов для оформления. Обложка может загружаться
по полученному URL; для воспроизведения YouTube SDK в OBS подключает внешний
YouTube IFrame API.

## Отдельные части и масштаб

`/widget/music?part=now` показывает текущий трек, `?part=queue` — очередь.
Без `part` (или с неизвестным значением) сохраняется общий вид `all`.
SDK до подписок темы задаёт `document.documentElement.dataset.part`:
`now`, `queue` или `all`. CSS темы должен скрывать другую часть и размещать
выбранную в точке (0, 0), без отступов от углов. Вид `all` сохраняет прежнюю
композицию. SDK подключайте перед кодом рендера темы; исходный интерфейс
держите скрытым до первого состояния.

Необязательное поле `parts` в `theme.json` задаёт размеры частей при масштабе 1:

```json
"parts": {"now": {"width": 560, "height": 120}, "queue": {"width": 300, "height": 260}}
```

Без `parts` API возвращает для обеих частей исходные `width` и `height`.
Размеры очереди рассчитывайте как минимум для пяти строк. Более длинный
контент должен помещаться в выбранную область или обрезаться самой темой.

Параметр `scale` — число от 0.5 до 4 (по умолчанию 1; конечные значения вне
диапазона ограничиваются). SDK применяет CSS `zoom` на `<html>`: Chromium/CEF
рендерит шрифты в нужном размере. Размер браузерного источника OBS равен
размерам части × scale с округлением вверх. Например, `?part=now&scale=2`
для 560×120 требует источник 1120×240. Не растягивайте источник в OBS для
увеличения текста. В OBS только `now` и `all` запрашивают роль `player`;
`queue` всегда подключается как `view` и не создаёт YouTube-плеер.
