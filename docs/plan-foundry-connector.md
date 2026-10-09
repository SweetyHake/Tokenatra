# План внедрения: коннектор FoundryVTT 14 ↔ Tokenatra + режим трея

Статус: **v2** (обновлён по результатам рецензии трёх агентов; ключевые правки помечены ⟦R⟧).
Версии после внедрения: приложение Tokenatra `26.4.0`, модуль `tokenatra-connector 1.0.0`.

---

## 0. Контекст и архитектурные факты (из кода)

- **Desktop shell**: `app.py` — pywebview (edgechromium на Win, нативные декорации на *nix) + Flask в daemon-потоке на `127.0.0.1:7878` (`server.py`). `app.window_ref` прокинут в Flask: `/api/window/<action>` уже умеет minimize/maximize/restore/destroy/move.
- **Закрытие окна**: `window.events.closing += _nuclear_exit` — сейчас любой крестик убивает процесс. pywebview (проверено по 6.1): хендлер `closing`, вернувший `False`, отменяет закрытие; **но** `Event.set()` выполняет ВСЕ подписчики независимо от чужого `False`, а `window.destroy()` тоже проходит через `closing` (winforms `FormClosing`) — значит перехват обязан быть единым хендлером с флагом force-quit. ⟦R⟧
- **Безопасность**: `server.py:_guard_localhost_only`: (1) Host-чек (DNS-rebinding закрыт); (2) state-changing с `Sec-Fetch-Site: cross-site` → 403; (3) Origin-чек **уже игнорирует порт** — `http://localhost:30000` проходит Origin-проверку и сегодня. Реальный блокер для модуля Foundry — п.(2): `localhost:30000 → 127.0.0.1:7878` это **cross-site** (порт в same-site не входит), поэтому `POST /process` из модуля получает 403. Iframe-навигация (GET без Origin) проходит свободно: UI в iframe работает уже сегодня. `X-Frame-Options`/CSP не выставляются — нужно добавить `frame-ancestors` (кликджеккинг). ⟦R⟧
- **ONNX-сессия**: ленивая (`load_session()` при первом `/process`), глобальная `SESSION` + `SESSION_LOCK` (RLock). Разгрузка = `SESSION = None` под локом + `gc.collect()` — механика уже используется `_activate_model()`.
- **Фронтенд**: vanilla JS, строгий порядок загрузки (AGENTS.md), `$(id)`, `urlManager`, i18n `I18n.t('русская строка')`, экспорт токена `TokenCanvas.save()` → `renderForSave(withRing)` → **возвращает `{canvas}`** → `toBlob`; пакетное удаление фона — `Remover` (`/process`, блобы в `state.results`, карточки строятся в `btnWrap`). Флаг стирания — `TokenCanvas.isErasing` (не `state.isErasing`). ⟦R⟧
- **Конфиг**: `/config POST` — клиентский снапшот **заменяет** config.json c защитой перечисленных ключей (сейчас `selected_model`, `gpu_device_id`); новый embed-инстанс с тем же `beforeunload → AppConfig.flush()` затрёт незащищённые ключи → новые серверные ключи обязаны быть в защищённом списке и НЕ в `AppConfig._defaults()`. ⟦R⟧
- **Иконки для трея есть**: `icon.ico`, `logo.png` в ресурсах.
- **Foundry v13/v14**: ApplicationV2; `FilePicker.upload(source, target, file)` — позиционные, `file: File` (не Blob), ответ `{path}`; `actor.img` в v14 удалён — арт актёра живёт в `prototypeToken.texture.src`; хуки `getActorContextOptions`, `getSceneControlButtons`, `renderTokenHUD`; кейбиндинги `{editable: [{key:'KeyT', modifiers:[Alt]}]}`; диалоги — `DialogV2`. ⟦R⟧

---

## Часть A. Tokenatra: трей + производный режим

### A1. Новый модуль `tray_manager.py` (Windows-only, деградация без pystray)

Зависимость: `pystray; sys_platform == 'win32'` (на Windows бэкенд чистый ctypes, pywin32 не нужен). На macOS/Linux трей недоступен → `tray_available() == False`, ✕ = выход как раньше.

```python
class TrayController:
    def __init__(self, on_open, on_quit): ...
    def show(self, tooltip='Tokenatra')      # НОВЫЙ pystray.Icon на каждый show
    def hide(self)                            # icon.stop(); инстанс после stop непригоден
    def set_tooltip(text, model_loaded: bool)
```
- **Жизненный цикл pystray ⟦R⟧**: остановленный `Icon` нельзя запустить повторно → `show()` каждый раз создаёт новый инстанс (`run_detached()`), старая ссылка держится только до `stop()`.
- Иконка: `logo.png` → Pillow resize 64×64 RGBA.
- Меню: «Открыть Tokenatra» (on_open: `window.show()`, `exit_tray_mode()`), «Выход» (on_quit: force-quit). Колбэки приходят из потока pystray — вызовы pywebview из чужого потока допустимы.

### A2. `app.py`: перехват закрытия с force-quit, тогглы трея ⟦R⟧

1. **Единый closing-хендлер** (заменяет `closing += _nuclear_exit`; `closed += _nuclear_exit` остаётся):
   ```python
   _force_quit = threading.Event()
   def _on_closing():
       if _force_quit.is_set() or not tray_manager.available() or not close_to_tray_enabled():
           _nuclear_exit()
           return True            # пропустить закрытие → выход
       enter_tray_mode()
       return False               # отменить закрытие, спрятать окно
   window.events.closing += _on_closing
   ```
   Модульный флаг, а не замыкание: `destroy()` (обновление, `applyAboutUpdate`, `/api/window/destroy`, `quit_app()`) обязан ставить `_force_quit` **до** вызова.
2. `WindowApi` (js_api) += `hide_to_tray()` (прячет окно и входит в трей; сам НЕ через closing — прямой путь), `quit_app()` (ставит `_force_quit`, `window.destroy()`).
3. Роут `/api/window/destroy` в server.py: перед `win.destroy()` выставить force-quit (через колбэк/атрибут, зарегистрированный app.py — `app.force_quit_ref = lambda: _force_quit.set()`); иначе путь автообновления при `close_to_tray=true` уедет в трей вместо выхода и bat/_update.sh не применится.
4. `enter_tray_mode()`: `TrayController.show()`, `server.set_tray_mode(True)`, асинхронно (из отдельного потока — evaluate_js из UI-потока closing-хендлера дедлочит WinForms ⟦R⟧) дернуть `window.evaluate_js('window.TokenTray&&TokenTray.enterIdle()')`. Основной механизм enterIdle — `visibilitychange` во фронтенде (скрытое окно WebView2 → `document.hidden`), evaluate_js — дубль из потока.
5. `exit_tray_mode()`: `window.show()` (не `restore()` — hidden ≠ minimized, Maximized-состояние WinForms сохраняет), `server.set_tray_mode(False)`, `TokenTray.exitIdle()`, `TrayController.hide()`.
6. Выход из трея: `TrayController.hide()` → `_force_quit.set()` → `_nuclear_exit()` → `window.destroy()`.

### A3. `server.py`: выгрузка модели в простое

```python
TRAY_MODE = False
_LAST_ACTIVITY = time.time()
_MODEL_LISTENERS = []            # app.py подписывается для tooltip трея

def set_tray_mode(on) -> bool    # лениво стартует монитор-поток при первом True
def mark_activity()              # вызывается из /process в начале и в конце
def unload_model() -> bool       # под SESSION_LOCK: SESSION=None, gc.collect(), listeners(False)
def add_model_state_listener(fn) # fn(loaded)
def start_tray_monitor()         # daemon-поток, проверка каждые 30 c — запускать ИЗ app.py, не на уровне модуля (server импортируют CLI-дети --tune-gpu) ⟦R⟧
```
- Монитор: `TRAY_MODE and SESSION is not None and idle > tray_idle_unload_min*60` → `unload_model()`. Порог `tray_idle_unload_min` из config.json (default **5**, `0` = не выгружать). Вне трея — никогда. Standalone `python server.py` — idle-unload не активен (нет трея), задокументировать.
- Гонки: инференс и unload под одним `SESSION_LOCK` (RLock) — безопасно; `mark_activity()` дополнительно сбрасывает таймер.
- `/health` (GET): `{app:'tokenatra', version, connectorApi: 1, model_exists, model_loaded, model_name, device, tray}` ⟦R⟧ — `connectorApi` для проверки совместимости модулем.
- Tooltip трея: «Модель в памяти» / «Модель выгружена — первый запрос ~10–30 с».

### A4. Фронтенд: `static/js/tokenTray.js` + кнопка ✕

Подключается в index.html **после main.js** (зависит от TokenCanvas/TokenHistory/TokenEditor).

`TokenTray.enterIdle()` (точные цели по коду ⟦R⟧):
- `visibilitychange` → `document.hidden` в pywebview — основной триггер (фолбэк не нужен, но evaluate_js-дубль из A2 остаётся);
- отмена rAF: `cancelAnimationFrame(TokenCanvas._renderRafId)` (если есть), `eraseAnimationId` — только если `!TokenCanvas.isErasing`;
- отмена таймеров: `state.effectsTimeout` (и `state.effectsEnabled = true` при выходе!), `TokenCanvas._workerBatchTimer`, `_eraserBrushTimer`;
- кэши: `TokenCanvas.invalidateAllCaches()` (ставит dirty) + явно `_compositedImageCache = null; _compositedImageDirty = true` + `_freeEffectCaches()` + `_ccImageCache/_ccCacheKey = null`, `_imageBrushCache = null`;
- история: новый метод `TokenHistory.trimTo(10)` — `splice(0, n)` спереди И `state.historyIndex -= n`, затем `TokenEditor._updateUndoButtons()`; в подсказке настроек честно писать «история отмен сокращается до 10 шагов»;
- НЕ трогаем: `state.userImage`, маски, воркер-зеркала (инвариант `_pushMaskToWorker` не нарушается).
`exitIdle()`: `state.effectsEnabled = true; TokenCanvas.requestRender()`.
Titlebar: ✕ при доступном трее и `close_to_tray` → `pywebview.api.hide_to_tray()` (tooltip «Свернуть в трей»), иначе прежний destroy.

### A5. Настройки UI + config + i18n ⟦R⟧

- Карточка «Трей и память» (только Windows; скрыта в embed): чекбокс «Сворачивать в трей при закрытии» (`close_to_tray`, default on), селект «Выгружать модель после простоя в трее» (никогда/1/3/5/10/30 мин → `tray_idle_unload_min`, default 5). ~~`start_in_tray`~~ — вырезано из v1 (нет автозапуска ОС, риск «приложение пропало»).
- **Хранение**: ключи НЕ добавляются в `AppConfig._defaults()/load()`; добавляются в защищённый кортеж `/config POST` (`server.py:2513`, рядом с `selected_model`). Карточка читает их отдельным `GET /config`, пишет — точечным POST тех же трёх ключей (серверный мерж защитит от embed-снапшотов без этих ключей).
- Честные цифры в подписи ⟦R⟧: «Выгрузка модели освобождает 0,5–2 ГБ (основная экономия); окно в трее продолжает держать ~200–400 МБ».
- i18n: ru→en пары всех новых строк.

### A6. Сборка ⟦R⟧

- `requirements.txt`: `pystray; sys_platform == 'win32'`.
- `build.spec`: внутри ветки `is_windows` в hiddenimports добавить `pystray` **и** `pystray._win32` (глобально — сломает linux/mac CI-джобы).

---

## Часть B. Embed-режим Tokenatra

### B1. Вход: `/?embed=1&origin=<encoded>`

- Инлайн-скрипт в `<head>` (до CSS): парсит query, ставит `document.documentElement.classList.add('embed')` и `window.__EMBED__ = {embed, parentOrigin}`. Без мигания хрома.
- Мост активен **только** при `embed=1` И `parentOrigin` с хостом localhost/127.0.0.1/::1 (любой порт) ⟦R⟧ — прочие родители мост не получают (никаких `targetOrigin:'*'`).
- `state.embed = true` → в `main.js init()` до аллокации канвасов: `state.canvasScale = 1` (embed-инстанс «производительность») ⟦R⟧; в `TokenCanvas.setCanvasScale` — параметр `{persist}`/пропуск записи при `state.embed`; селектор «Рабочий канвас» скрыт в embed-CSS.

### B2. CSS embed (`style.css`)

Скрыть: `.window-controls`, вкладку «О программе», «Интеграцию с ОС», тумблеры быстрого сохранения и нативные диалоги (`/save_file`/`/pick_folder` из-под Foundry всплывают под окном ⟦R⟧), drag-поведение тайтлбара. Компактные отступы. Кнопки сохранения токена в embed остаются (браузерный `<a download>`-скачивание — фолбэк не нужен, `/save_file` не используем: показываем «Отправить в Foundry»).

### B3. `static/js/tokenEmbed.js` — мост postMessage ⟦R⟧

Загружается перед `main.js`. Инициализация на DOMContentLoaded (позже `init()` main.js — родитель дождётся `tokenatra:ready`).

Входящие (проверка `event.origin === __EMBED__.parentOrigin`):
- `tokenatra:ping` → `{type:'tokenatra:pong', version, connectorApi: 1}`;
- `tokenatra:load {dataUrl, filename}` → dataUrl→blob→File → переключить таб «Создание токена» (классы active/aria-hidden/inert как в `Remover.openInEditor`, remover.js:500) → `TokenCanvas.loadImage(file)` + `state.tokenFileName`.

Исходящие (`parent.postMessage(msg, __EMBED__.parentOrigin)` — только валидный origin):
- `tokenatra:ready {version, connectorApi}` — после init;
- `tokenatra:export {url, filename, format, source: 'editor'|'remover'}`;
- `tokenatra:error {message}`.

### B4. Отправка в Foundry

1. Кнопки «⤴ В Foundry» (только embed) в экспорт-секции редактора (рядом с «С кольца/Без кольца», две: с кольцом/без) → `renderForSave(withRing).canvas.toBlob` (формат из saveSettings);
2. В карточках результатов вырезателя — кнопка «→ Foundry» (отправляет готовый блоб из карточки);
3. `POST /embed_export` (multipart, same-origin) → `{id, url}` → `tokenatra:export` родителю.

### B5. `server.py`: `/embed_export` + scoped-CORS + frame-ancestors ⟦R⟧

- `POST /embed_export`: file (≤40 МБ — с запасом под глобальный `MAX_CONTENT_LENGTH=50 МБ` ⟦R⟧) → реестр `{id: (path|bytes, mime, expires)}`, TTL 10 мин, ≤32 записи (LRU-вытеснение), чистка общим фоновым потоком. **id = `secrets.token_urlsafe(16)`** (не счётчик/uuid4-hex — не перебираем).
- `GET /embed_export/<id>`: требуем `Origin` localhost-семейства ИЛИ `Sec-Fetch-Site` same-site/none (защита от `<img src>`-эксфильтрации с чужих страниц; fetch модуля всегда с Origin).
- **Гвард** — scoped-ослабление только для роутов-allowlist:
  ```
  CONNECTOR_ROUTES = {'/health', '/process', '/version', '/device', '/models_list',
                      '/embed_export', '/embed_export/<id>'}
  ```
  Для них: localhost-семейство Origin (порт игнорируется — как сейчас) пропускает и `Sec-Fetch-Site: cross-site`, и отсутствие иных проверок. Остальные роуты (`/config`, `/shutdown`, `/apply_update`, `/save_to_folder`, `/delete_preset`, `/api/window/*`…) — гвард без изменений (никакого blanket-CORS: чужой dev-сервер на localhost не получает ничего нового).
- `after_request`: для CONNECTOR_ROUTES + валидного Origin — `Access-Control-Allow-Origin: <origin>`, `Vary: Origin`, `Allow-Methods/Headers`, `Max-Age: 600`. OPTIONS уже проходит (не state-changing) — отдельный preflight-обработчик не нужен ⟦R⟧.
- **`Content-Security-Policy: frame-ancestors`** на HTML-ответах: без `embed=1` → `frame-ancestors 'none'`; с `embed=1` → `frame-ancestors http://localhost:* http://127.0.0.1:*` ⟦R⟧ (кликджеккинг закрыт; ограничение «Foundry должен открываться по localhost/127.0.0.1» — в FAQ).
- LAN/remote-Foundry — вне v1: документированное ограничение («одна машина»; `listen_host`/`allowed_origins` отложены).

---

## Часть C. Модуль Foundry `tokenatra-connector`

### C1. Структура

```
foundry-module/tokenatra-connector/
├── module.json
├── README.md
├── src/
│   ├── main.js        # init: settings/keybindings; ready: кнопки, контексты; экспорт api
│   ├── app.js         # TokenatraApp (ApplicationV2, синглтон)
│   ├── api.js         # health (пробует 127.0.0.1 и localhost), process(), fetchExport()
│   ├── actions.js     # quick-действия + применение результата
│   └── upload.js      # data → user; без прав — «скачать файл» (никаких data:/blob: в БД) ⟦R⟧
├── templates/tokenatra-app.hbs   # резолвится как modules/tokenatra-connector/templates/... ⟦R⟧
├── styles/tokenatra.css (+ tokenatra/_variables.css, _base.css; префикс tkc-)
└── lang/en.json, lang/ru.json
```

### C2. `module.json` ⟦R⟧

- `compatibility: {minimum: "13", verified: "14", maximum: "14"}`; esmodules/styles/languages/readme.
- `manifest`: `https://raw.githubusercontent.com/SweetyHake/Tokenatra/main/foundry-module/tokenatra-connector/module.json` (движущийся ref — иначе Foundry не увидит обновлений).
- `download`: `https://github.com/SweetyHake/Tokenatra/releases/latest/download/tokenatra-connector.zip` (стабильный URL последнего релиза; zip — ассет релиза приложения, отдельного тега модуля НЕТ — тег `v1.0.0` запустил бы app-CI и сломал бы `releases/latest`).
- Дисциплина: версия в module.json бампается ТОЛЬКО в релизном коммите вместе с тегом `v26.x.y` (манифест на main не должен анонсировать несуществующий zip).
- CI: в `.github/workflows/build-release.yml` (job релиза) добавить Compress-Archive/zip модуля и upload-ассет `tokenatra-connector.zip`.

### C3. Настройки (`game.settings`, client-scope)

| Ключ | Тип | Default |
|---|---|---|
| `serverUrl` | String | `http://127.0.0.1:7878` (api.js при недоступности пробует hostname-замену на `localhost`) ⟦R⟧ |
| `uploadTarget` | String | `tokenatra/` |
| `format` / `quality` | choices / range | `webp` / 90 |
| `autoApply` | Boolean | false (экспорт сразу обновляет арт актёра-источника и его токены) |
| `showSceneControl` | Boolean | true |

### C4. `TokenatraApp` (ApplicationV2, синглтон) ⟦R⟧

- `DEFAULT_OPTIONS`: `window {title, icon: 'fa-solid fa-wand-magic-sparkles', resizable}`; `position {width: 1000, height: 760}`; один part `app`; шаблон по полному пути `modules/tokenatra-connector/templates/tokenatra-app.hbs`.
- Шаблон: `<iframe class="tkc-frame">` (flex:1) + статус-бар. **Статус-бар обновляется императивно (DOM), без `render()`** — ре-рендер ApplicationV2 пересоздаёт DOM части и перезагружает iframe (потеря сессии). Повторное открытие — `render(true)` + `bringToFront()`.
- Открытие: `api.health()` (таймаут 3 c; пробует `127.0.0.1`↔`localhost`): нет → help-экран («не запущена» / `404|нет connectorApi` → «обновите Tokenatra до 26.4+» / `model_exists:false` → «установите модель»); есть → iframe `src={url}/?embed=1&origin={origin Foundry}`.
- **Поллинг `/health` каждые 8 с при открытом окне** ⟦R⟧: точка краснеет при недоступности, iframe затемняется; при восстановлении — `iframe.contentWindow.location.reload()` + ре-синк (`loadFromActor(contextActor)`).
- `message`-листенер: подписка в `_onFirstRender`, снятие в `_onClose`; фильтры `event.source === this.frame?.contentWindow` и `event.origin === origin(serverUrl)`; при `!this.frame` игнор.
- **Очередь исходящих до `tokenatra:ready`** ⟦R⟧: `loadFromActor` буферизует `tokenatra:load`, flush по ready (и после каждого reload iframe).
- Обработка `tokenatra:export {url, filename}`: `api.fetchExport(url)` → `upload.upload(File)` → `actions.apply(...)`.
- Кнопка хедера: «Открыть во вкладке» (`window.open(serverUrl)`).

### C5. `upload.js` ⟦R⟧

1. Есть `FILES_UPLOAD` (GM): `const f = new File([blob], fileName, {type}); const res = await FilePicker.upload('data', uploadTarget, f); path = res.path;` (позиционные аргументы!).
2. Игрок: `FilePicker.upload('user', …)` — личный каталог игрока.
3. Оба упали/нет прав: **не писать ничего в акторы** (никаких data:/blob: в персистентных полях) — notification «нет прав на загрузку: файл скачан» + принудительное скачивание blob.
- `fileName` санитизировать (только `[A-Za-z0-9._-]`, ≤64 символа, без пути).

### C6. `actions.js` ⟦R⟧

- `apply(path, {contextActor})`:
  - `autoApply` + contextActor: сразу `updateActorArt(actor, path)`;
  - иначе `DialogV2` (не кастомный hbs): «Создать актёра» (тип — базовый из `game.system.documentTypes.Actor`, `{renderSheet: false}`; опция скрыта без `ACTOR_CREATE`), «Обновить актёра {name}» (при contextActor и `actor.isOwner`), «Скопировать путь».
- `updateActorArt(actor, path)`: апдейт только `prototypeToken.texture.src` (+ `img` условно, если `'img' in actor.schema.fields` — v13); GM: также `actor.getActiveTokens()?.forEach(t => t.update({texture: {src: path}}))` — размещённые токены.
- Quick-action «Tokenatra: вырезать фон» (контекст актёров + кнопка TokenHUD): до вызова проверить `health.model_loaded` — если false, notification «первый запуск после простоя: загрузка модели ~10–30 с» (прогресса в /process нет ⟦R⟧); `fetch(actor.img)` (только изображения — гард по расширению/mime, видео/SVG → отказ) → `POST /process` (format/quality/edge_blur=1) → upload → `updateActorArt`; ошибки ответа (500 «модель не установлена», 413) — notification; внешний арт (S3/CDN без CORS) → понятная ошибка + ограничение в FAQ.
- «Tokenatra: студия токена» — открыть app + `loadFromActor(actor)`.
- Кнопка scene controls: `controls.tokens.tools` += `{name, icon, title, button: true, onClick}` (не toggle ⟦R⟧).
- Кейбинд: `game.keybindings.register('tokenatra-connector','openStudio',{editable:[{key:'KeyT',modifiers:[foundry.CONST?.KEYBINDING_ALT ?? 'Alt'}]})` — в `init`.
- Публичный API: `game.modules.get('tokenatra-connector').api = {open: (actor)=>…, removeBackground: (actor)=>…}`.

### C7. Стиль и i18n

- Токены `--tkc-*`, тёмная/светлая темы, хром окна по foundry-ui-style (title по центру, gap:0, скроллбары 8px, BEM `.tkc-*`), fieldset, focus-visible.
- Все строки через `localize('TKC.*')`; ru+en.

---

## Часть D. Документация, версии, релиз

- `version.py` → `26.4.0`; module.json — `1.0.0`.
- CI `build-release.yml`: собрать и приложить `tokenatra-connector.zip` к релизу ⟦R⟧.
- `build_module.bat` — локальная сборка zip (корень архива = папка `tokenatra-connector/`).
- `AGENTS.md`: новый каталог, эндпоинты (`/health`, `/embed_export*`), CONNECTOR_ROUTES/гвард, ключи конфига (`close_to_tray`, `tray_idle_unload_min`), порядок загрузки JS (+`tokenTray.js`, `tokenEmbed.js`), правила релиза модуля.
- `README.md` + `docs/FOUNDRY.md`: установка (manifest URL), сценарии, **FAQ** ⟦R⟧: localhost vs 127.0.0.1; Foundry на другой машине не поддерживается; диалоги/права игроков; трей только Windows; модуль предлагает обновление раньше ассета; **честные цифры памяти** (модель 0,5–2 ГБ выгружается, окно держит ~200–400 МБ; история undo сокращается до 10 в трее).

---

## Часть E. Порядок реализации и тесты

1. `server.py`: tray-state/unload/monitor + `/health` + `/embed_export` + scoped-CORS гвард + frame-ancestors + `/api/window/destroy` force-quit.
2. `tray_manager.py` + `app.py` (closing-хендлер, force-quit, WindowApi, enter/exit tray).
3. Фронтенд: `tokenTray.js`, `tokenEmbed.js`, embed-CSS, карточка настроек, main.js (`state.embed`, canvasScale), tokenCanvas (`setCanvasScale persist`), remover-кнопка, index.html, i18n.
4. Foundry-модуль целиком + CI/скрипты + доки.

Проверки:
- `python -m py_compile app.py server.py tray_manager.py`; `node --check` всех JS;
- standalone `python server.py`: `curl /health`, `/?embed=1`; CORS-матрица curl'ом: `Origin: http://localhost:30000` → `/health`/`/process` 200+ACAO, `/config` POST 403; `Origin: http://evil.com` → 403 везде; GET `/embed_export/<id>` без Origin с `Sec-Fetch-Site: cross-site` → 403; `POST /embed_export` roundtrip;
- регрессия: ответы `/` и `/static/*` НЕ содержат X-Frame-Options, содержат CSP frame-ancestors (без embed — 'none') ⟦R⟧;
- desktop-прогон: ✕→трей→RAM до/после idle-unload; вырезание фона из трея (первый запрос — прогрузка); «Выход» из трея реально завершает процесс; сценарий обновления при `close_to_tray=true` (скачать → Установить → приложение закрылось, bat применился) ⟦R⟧; restore окна сохраняет работу редактора;
- Foundry v13 и v14 отдельно ⟦R⟧: студия, загрузка арта, экспорт→актёр (ветка `img`/`prototypeToken`), quick-action, TokenHUD, игрок без `FILES_UPLOAD`, Foundry по `localhost` и по `127.0.0.1`.

---

## Риски и компромиссы

| Риск | Решение |
|---|---|
| pystray нет (портативная сборка) | `tray_manager.available()` до перехвата ✕; без него — выход как раньше |
| Выгрузка модели → медленный первый запрос | tooltip + notification модуля; порог настраиваемый (0 = никогда) |
| CORS scoped к CONNECTOR_ROUTES | Остальные роуты строго same-origin; Host-чек (rebinding) и whitelist папок `/save_to_folder` не тронуты |
| Эксфильтрация через `/embed_export` | `secrets.token_urlsafe(16)`, TTL 10 мин, лимит записей, Sec-Fetch/Origin-чек на GET |
| Кликджеккинг полного UI | CSP `frame-ancestors 'none'` (обычно) / localhost-семейство (embed) |
| Подмена postMessage | Мост активен только `embed=1`+localhost-origin; модуль фильтрует `event.source`+`event.origin` |
| destroy() отменяется closing-хендлером | Единый хендлер + `_force_quit` (включая `/api/window/destroy` для автообновления) |
| Игрок без прав загрузки | data→user→отказ без записи в БД + скачивание файла |
| Разные машины / LAN Foundry | Не поддерживается в v1, задокументировано |
| Рассинхрон версий моста | `connectorApi` в `/health`/`pong`/`ready`; модуль различает refused/старую версию/нет модели |
| Скрытый WebView держит память | Честные цифры в доках; основная экономия — выгрузка модели |
