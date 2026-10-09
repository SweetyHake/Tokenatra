# Tokenatra — AGENTS.md

## Commands

```bat
pip install -r requirements.txt   # one-time setup
python app.py          # desktop app from source (console visible — handy for debugging)
python server.py       # standalone Flask on :7878 (no GUI window)
python server.py --remove-bg <file>   # CLI: remove background, saves <stem>_nobg.webp
python server.py --to-webp <file>     # CLI: convert to WebP, deletes original
build_module.bat       # zip Foundry-модуля -> dist/module/tokenatra-connector.zip
```

## Build

```bat
build_installer.bat       # Step 1: PyInstaller -> Step 2: Inno Setup installer
build.bat                 # PyInstaller only (portable folder)
```

PyInstaller output: `dist/Tokenatra/Tokenatra.exe`
Installer output: `dist/installer/Tokenatra_Setup_v*.exe`

### GitHub Releases: ассеты по платформам

Релиз содержит: `Tokenatra_v<ver>_windows_x86_64_setup.exe` (Windows), `Tokenatra_v<ver>_macos_arm64.dmg` / `_x86_64.dmg` (macOS), `Tokenatra_v<ver>_linux_x86_64.AppImage` + `.tar.gz` (Linux). Все собирает CI (`build-release.yml`); имена ассетов значимы — updater ищет файл по суффиксу платформы и архитектуры (`setup`, `_macos_arm64.`, `_linux_x86_64.`).

Автообновление работает на всех трёх платформах:
- **Windows**: качает Setup-установщик (приоритет в `_find_exe_asset`) — тихая установка через bat.
- **macOS**: качает DMG своей архитектуры; `/apply_update` пишет `_update.sh`, который после закрытия приложения монтирует образ, подменяет .app (с бэкапом на время подмены) и перезапускает. Запуск из /Volumes или вне .app → отказ с подсказкой.
- **Linux**: качает AppImage; атомарный `os.replace` поверх работающего файла + перезапуск. Работает только из AppImage ($APPIMAGE).

Голый `dist/Tokenatra/Tokenatra.exe` — тонкий загрузчик без `_internal`; отдельно не публиковать.

### Публикация релиза (пошагово)

Версии по схеме `год.мажор.фикс` (2026 → `26.x.y`), тег `v26.x.y`.

> **CI:** пуш тега `v*` запускает `.github/workflows/build-release.yml` — он сам собирает все платформы и прикладывает ассеты к релизу: `Tokenatra_v*.exe` (Windows), `Tokenatra_v*_macos_*.dmg`, `Tokenatra_v*_linux_*.AppImage/.tar.gz`. Ручные шаги 4-5 ниже нужны только если CI недоступен.

1. **Бамп версии**:
   - `version.py` → `__version__ = "26.x.y"` (имя установщика = `Tokenatra_Setup_v26.x.y.exe`; CI передаёт версию в Inno через `/DMyAppVersion=…`, в installer.iss остаётся только fallback)
2. **Коммит и пуш** в `main`:
   ```
   git add -A
   git commit -m "v26.x.y: <что исправлено>"
   git push
   ```
3. **Тег** (только после пуша кода, чтобы тег указывал на релизный коммит):
   ```
   git tag v26.x.y
   git push --tags
   ```
4. **Сборка установщика**: `build_installer.bat` (PyInstaller → ISCC), либо вручную `python -m PyInstaller build.spec` + `iscc installer.iss`. Результат: `dist/installer/Tokenatra_Setup_v26.x.y.exe`.
5. **Создать GitHub release и залить ассет** (токен из Windows Credential Manager через `git credential fill`):
   ```powershell
   $token = ("protocol=https`nhost=github.com`n" | git credential fill 2>$null | Select-String '^password=' | ForEach-Object { ($_ -split '=', 2)[1] })
   # release body — JSON, напр. { "tag_name": "v26.x.y", "name": "Tokenatra 26.x.y", "body": "## Исправления...", "draft": false, "prerelease": false }
   $resp = curl.exe -s -X POST -H "Authorization: Bearer $token" -H "Accept: application/vnd.github+json" -H "Content-Type: application/json" --data-binary "@release_body.json" "https://api.github.com/repos/SweetyHake/Tokenatra/releases"
   $id = $resp | python -c "import json,sys; print(json.load(sys.stdin).get('id'))"
   curl.exe -s -X POST -H "Authorization: Bearer $token" -H "Accept: application/vnd.github+json" -H "Content-Type: application/octet-stream" --data-binary "@Z:\Tokenatra\dist\installer\Tokenatra_Setup_v26.x.y.exe" "https://uploads.github.com/repos/SweetyHake/Tokenatra/releases/$id/assets?name=Tokenatra_Setup_v26.x.y.exe"
   ```
6. **Проверка**: `https://api.github.com/repos/SweetyHake/Tokenatra/releases/latest` → тег и имя ассета. В приложении появится баннер «доступна новая версия» (ссылкой на GitHub).

Model files (`*.onnx`) are NOT bundled — user places them into the `models/` folder next to the exe and picks one in Settings (только `.onnx`).

### Inno Setup

To build the installer, install Inno Setup from https://jrsoftware.org/isdl.php
and run `build_installer.bat`. Or manually:

```bat
python -m PyInstaller build.spec
iscc installer.iss
```

## Bundled assets (included in .exe build)

- `templates/` — Jinja2 HTML templates
- `static/` — JS, CSS, workers
- `token_rings/` — ring overlay images
- `presets/` — preset mask images
- `version.py` — version & repo config
- `foundry-module/tokenatra-connector/` — исходники модуля Foundry VTT (в .exe НЕ бандлится; см. docs/FOUNDRY.md)

## Трей и экономный режим (Windows, pystray)

- Иконка трея живёт ВЕСЬ цикл приложения (как у Discord): `_tray.show()` зовётся
  на старте `main()` до `webview.start()`; `show()` идемпотентен (живую иконку
  не пересоздаёт, только обновляет тултип). Реальный выход — `_final_exit()`
  (`_tray.hide()` + `_nuclear_exit()`): его вызывают ветки `_on_closing`,
  `window.events.closed` и код после `webview.start()`; `_exit_tray_mode`
  иконку больше НЕ убирает.
- `close_to_tray` (config.json, default true) — ✕ прячет окно в трей
  вместо выхода (тумблер в Настройках → «Трей и память»). ЕДИНСТВЕННЫЙ
  `window.events.closing`-хендлер — `_on_closing`
  в `app.py` (pywebview `Event.set()` выполняет всех подписчиков, а
  `window.destroy()` тоже проходит через closing → нужен флаг `_force_quit`;
  его же выставляет `/api/window/destroy` — путь автообновления).
- `tray_idle_unload_min` (default 5, 0 = никогда) — в трее ONNX-сессия
  выгружается после простоя (`server.unload_model()` под `SESSION_LOCK`),
  первый `/process` платит прогрузку. Монитор-поток стартует лениво из
  `set_tray_mode(True)` (только desktop-режим, не standalone `server.py`).
- Эти два ключа — СЕРВЕРНЫЕ: их нет в `AppConfig._defaults()` (иначе embed-
  снапшот затрёт), `/config POST` защищает их как `selected_model`.
- Фронт: `tokenTray.js` (гасит rAF/кэши/таймеры по `visibilitychange`,
  история режется до 10). `tray_manager.py`: остановленный pystray `Icon`
  непригоден — после `hide()` `show()` строит иконку заново, живую не трогает.
- Меню трея: «Открыть Tokenatra», чекбокс «Модель в памяти» (тумблер
  load_session/unload_model в фоне, семафор `_model_busy` в app.py защищает
  от двойного клика) и «Выход». Нативные диалоги следуют теме Windows:
  `app.py:_enable_windows_dark_dialogs()` → `uxtheme!SetPreferredAppMode(AllowDark)`.

## Интеграция с FoundryVTT (модуль-коннектор)

- Каталог: `foundry-module/tokenatra-connector/`, установка по manifest URL
  (raw main), zip — ассет релиза приложения (`releases/latest/download/
  tokenatra-connector.zip`, CI job `release`). Версию module.json бампить
  ТОЛЬКО в релизном коммите (манифест не должен анонсировать несуществующий zip).
- Embed-режим: `/?embed=1&origin=<encoded>` → класс `html.embed` (ставится
  инлайн-скриптом в head до рендера), мост `tokenEmbed.js` активен только при
  валидном localhost-origin; протокол `connectorApi: 1` (`tokenatra:ping/pong/
  ready/load/export`). Экспорт: `POST /embed_export` (блоб, id =
  `secrets.token_urlsafe`, TTL 10 мин) → модуль забирает `GET /embed_export/
  <id>` с CORS.
- Гвард `_guard_localhost_only`: кросс-сайтово разрешены ТОЛЬКО роуты из
  `CONNECTOR_ROUTES` (`/health`, `/process`, `/version`, `/device`,
  `/models_list`, `/embed_export*`) и только Origin localhost-семейства
  (любой порт). Остальные роуты — строго same-origin. На HTML-ответах всегда
  `CSP frame-ancestors` ('none', для embed — localhost-семейство).
- Hosted-студия (основной режим мультиплеера): игрокам отдаётся ЖИВОЙ UI
  из Tokenatra ГМа — клиент ГМа собирает self-contained страницу с localhost
  (relay.js: studio_meta/studio_chunk, инлайн CSS/JS + tokenTransport) и
  шлёт чанками через сокет мира; app.js грузит её в srcdoc. Фолбэк при
  офлайн-ГМе — статическая копия из модуля (`studio/`, `build_studio.py`,
  вызывается из build_module.bat и CI; после правок static/ пересобирать).
  Сетевой режим для модуля НЕ нужен.
- Скорость открытия окна: сборка кешируется до перезапуска страницы
  (app.js `fetchStudioHtml` с дедупом параллельных вызовов + `_studioCache`
  на клиенте ГМа) и прогревается на `ready` (`prefetchStudio` в
  requestIdleCallback). Рукопожатие `tokenatra:hello` (tokenTransport →
  родитель, ответ `#sendConn`) устраняет гонку `tokenatra:conn`: без него
  conn, отправленный до создания srcdoc-iframe, теряется и `/config`
  подвисает на 3-с таймауте, уходя в localStorage. Миниатюры колец:
  build_studio.py генерирует `thumbs/<stem>.webp` (144px) и кладёт `thumb`
  в manifest; `loadRings` использует их только в hosted
  (`__STUDIO_HOSTED__`), полное кольцо грузится лишь выбранное.
- Сетевой режим (опти-ин, БЕЗ КЛЮЧЕЙ, для прямых HTTP-сценариев): `listen_host` = '0.0.0.0' (тумблер
  «Разрешить подключения из сети», рестарт). Не-localhost запросы пускаются
  только на `PUBLIC_PROXY_ROUTES`/`PUBLIC_OPEN_*` (статика UI; /config
  НАМЕРЕННО исключён — утёк бы секретами). Опциональное ужесточение без UI:
  `shared_secret` в config.json (модуль шлёт `X-Tokenatra-Key`; cookie
  `tokenatra_key` ставится ответом `/?embed=1&key=…`). UI — карточка
  «Доступ для игроков» + `/network_info` (только localhost).
- Подробности и FAQ: docs/FOUNDRY.md.

## Dependencies

Единый `requirements.txt` — источник правды для `start.sh` и CI.
Маркеры окружения ставят `onnxruntime-directml` только в Windows; в macOS/Linux — обычный `onnxruntime`.

## Version & updates

- `version.py:GITHUB_REPO` — set to `"SweetyHake/Tokenatra"` before building
- On startup, `updater.py` checks GitHub Releases for newer version
- If newer version found, splash screen shows download button
- `model.onnx` is NOT auto-downloaded — the user places it into `models/` and selects it in Settings (see "External assets")

## Architecture

- **Desktop shell**: pywebview (edgechromium) → `app.py:138-148`
- **Web server**: Flask on `127.0.0.1:7878` → `server.py`
- **AI inference**: ONNX Runtime with a model from `models/` folder (BiRefNet **или** RMBG-2.0/IS-Net — обе поддерживаются), выбор модели в настройках (`/models_list`, `/select_model`). `get_providers()` in `server.py` detects physical GPUs via WMI (virtual/Parsec/RDP adapters are filtered), then picks CUDA (NVIDIA) → ROCm → DirectML → CPU. Runtime fallback to CPU if the GPU provider fails to load.
- **Frontend**: Vanilla JS, global `state` object mutated directly by all modules
- **Canvas**: internal size = `CONFIG.SCALE_SIZES[state.canvasScale]` (`{1:1536, 2:3072, 3:6144}`, **default 2 → 3072×3072 px**; варианты: 1536 «производительность», 3072 «баланс», 6144 «качество»), логические координаты в 1024 px пространстве (scale factor = 2×canvasScale). Масштаб меняется в Settings → Производительность → «Рабочий канвас» (`TokenCanvas.setCanvasScale()`, сохраняется в `config.json` как `canvasScale`). Экспорт НЕ зависит от рабочего масштаба: `renderForSave()` рендерит в `saveSettings.quality × 3` (независимый `coordScale`). Перф-инвариант: `getImageData`/`putImageData` по всему канвасу запрещены в горячих путях (тень — `source-in`, цветокоррекция — кэш `_getColorCorrectedImage()`, ластик — воркер).
- **Ластик (воркер, протокол v2)**: воркер хранит **альфа-зеркала масок** (1 байт/px, `setMask`) и защиту (`setProtection`, `state.protectionAlpha`). Пачка несёт только штрихи (`Float64Array [cx,cy,drawX,drawY,flags]`) — ноль чтений маски на главном потоке; воркер сам считает регион и возвращает RGBA-патч (RGB=255). Пайплайн глубиной 2 (`_workerInFlight`). **Инвариант: любое изменение маски в обход ластика (undo/redo, resetMask/resetImageMask, пресет `TokenPresets.apply`, заливка при удалении фона, `createMask/createImageMask`) обязано вызвать `TokenCanvas._pushMaskToWorker(pink)`** — иначе воркер применит штрихи к устаревшему зеркалу. Точки в очереди не дропаются никогда (схлопывание близких). Ввод — Pointer Events + `getCoalescedEvents` (touch-action:none на `.canvas-area`). История — только альфа-снапшоты (`{a,w,h}`, ~1 МБ на маску).
- **Тень при стирании**: на время штриха `effectsEnabled=false`, но кадр рисуется со **старым кэшем тени** (`_shadowCache`, и в полном кадре, и в инкрементальном патче) — перерасчёт только на отпускании (`stopErasing`), иначе токен «прыгает» в момент касания.
- **«Отображать скрытое» + ластик**: пачка режима `image` несёт `ghostOverride: state.showHidden` — воркер снимает защиту там, где розовое зеркало прозрачно (скрытое содержимое режется сквозь защиту кольца); над видимым токеном (розовая маска непрозрачна) защита действует. Фолбэк `_fallbackBrushToImageMask` делает то же через `destination-in` маски в protSampler.

## Critical file: `server.py` gotchas

- **Duplicate route functions**: проверено — дубликатов нет, каждый `@app.route` объявлен ровно один раз.
- **Блок `if __name__ == '__main__'` обязан быть в КОНЦЕ `server.py`**: при запуске `python server.py` `app.run()` блокирует модуль, и маршруты ниже не регистрируются (получали 404 на `/config`, `/save_file`, `/shutdown` в dev-режиме).
- Every `@app.route` must be declared exactly once with the decorator.
- Models live in `BASE_DIR/models/` (`get_selected_model_path()`: config `selected_model` → первый `*.onnx`). При первом запуске `model.onnx` из `BASE_DIR` автоматически перемещается в `models/`.

## JS load order (strict, from `index.html`)

```
config.js → state.js → utils.js → i18n.js → urlManager.js → tokenEffects.js →
tokenHistory.js → tokenPresets.js → tokenCanvas.js → tokenEditor.js →
portraitGenerator.js → hotkeySettings.js → remover.js → converter.js →
tokenEmbed.js → main.js → tokenTray.js
```

`tokenEmbed.js` — мост Foundry (до main.js: слушает сообщения с DOMContentLoaded);
`tokenTray.js` — экономный режим (после main.js: дергает TokenCanvas/TokenHistory).

Breaking this order causes runtime errors because objects reference each other.

## Frontend conventions

- ObjectURLs must use `urlManager.create()` / `urlManager.revoke()` — never `URL.createObjectURL()` directly
- `$(id)` = `document.getElementById(id)` from `utils.js`
- `debounce(fn, ms)` from `utils.js` for expensive history saves
- No frameworks, no inline CSS (except dynamic `.style.display`), CSS custom properties in `:root`
- All ObjectURLs in `state.userImage*` freed via `urlManager.revokeAll()` on new image load

## Server routes (non-obvious)

## Face detection cascade

Haar → heuristic

If OpenCV DNN model files (`opencv_face_detector_uint8.pb` + `.pbtxt`) exist in OpenCV's data dir, DNN is tried before Haar.

External models (`.gitignore`d):
- `model.onnx` — BiRefNet или RMBG-2.0/IS-Net (background removal)

### `model.onnx` требования

- Вход: 1024×1024×3 RGB, ImageNet-нормализация (mean 0.485/0.456/0.406, std 0.229/0.224/0.225) — всё это делает `server.py` сам
- **Исключение — BEN2** (имя файла содержит `ben`, рекомендуется `PramaLLC/BEN2`): официальный ONNX ждёт plain ToTensor 0..1 БЕЗ нормализации (см. `onnx_run.py` авторов) — `remove_background` нормализует по имени модели; плюс для BEN2 `refine_mask` идёт в gentle-режиме (без `^1.2`, без эрозии `MinFilter(3)`, отсечка 0.05 вместо 0.15) — жёсткий рефайн «съедает» тонкие детали по краям (клинки, рваные края)
- Выход: одноканальная маска 1024×1024. Если выход уже в [0,1] (RMBG-2.0 `alphas` содержит sigmoid в графе) — sigmoid повторно НЕ применяется (`server.py` detect: `mask.min() >= 0 and mask.max() <= 1`); для сырых logits (BiRefNet) — применяется
- Вход/выход могут быть объявлены как `dynamic` (напр. `[1,3,'height','width']`), но фактически модель может требовать ровно 1024×1024 (фикс. split в декодере)
- Источник: HuggingFace, напр. `briaai/RMBG-2.0` (`onnx/model_fp16.onnx`, ~514 МБ) или `DanielLavric/BiRefNet-ONNX`
- `server.py` грузит с `ORT_ENABLE_EXTENDED`, при падении (`InsertedPrecisionFreeCast` — известный баг ORT на LayerNorm-fusion) откатывается на `ORT_ENABLE_BASIC`

| Route | Note |
|-------|------|
| `/save_file` POST | Opens native Windows file dialog via tkinter. Expects `file` + `filename` in form. |
| `/pick_folder` GET | Native folder picker via tkinter. |
| `/save_to_folder` POST | Write file to a given folder path. |
| `/config` GET/POST | Reads/writes `config.json` in BASE_DIR (`.gitignore`d). |
| `/presets_list` | Встроенные пресеты из `RESOURCE_DIR/presets` (`builtin: true`) + пользовательские из `USER_DATA_DIR/presets` (новые сверху, `builtin: false`). |
| `/preset_file/<filename>` | Сначала ищет в пользовательском каталоге пресетов, затем во встроенном. |
| `/add_preset` POST | Сохраняет текущую маску как PNG-пресет в `USER_DATA_DIR/presets` (multipart `image`), имя авто «Пресет N». |
| `/delete_preset` POST | Удаляет ТОЛЬКО пользовательский пресет (встроенные в ресурсах недоступны для записи). |
| `/process` POST | Background removal. Accepts `image` (file), `format` (webp/png/jpg), `quality` (int 10-100), `edge_blur` (float 0-10). Изображения с альфой предварительно композитятся на белый фон (иначе тёмная кайма по краям). |

## New features (v2 integration)

## Hotkeys

Stored in `config.json`, managed by `AppConfig` (JS) with a rebindable UI in `hotkeySettings.js`. Defaults in `config.js:DEFAULT_HOTKEYS`. Hardcoded secondary bindings in `config.js:MOVE_KEYS`, `ROTATE_KEYS`, `ERASER_SIZE_KEYS`.

## New modules (beyond old CLAUDE.md)

- `portraitGenerator.js` — separate canvas for portrait-oriented token creation
- `hotkeySettings.js` — UI for rebinding keyboard shortcuts
- `eraserWorker.js` — Web Worker for performant eraser brush application on mask canvas
- `context_menu_helper.py` — Windows context menu handler (registered/unregistered by `app.py`)

## Common pitfalls

| Symptom | Likely cause |
|---------|-------------|
| /process returns 500 | Модель missing в `models/` (баннер + выбор в настройках) |
| Rings don't load | Syntax error in `tokenPresets.js` prevents parsing |
| Image doesn't render | `state.userImage` or `state.maskCanvas` is null |
| ObjectURL leak | `urlManager` bypassed |
| Route 404 | Duplicate `def` without `@app.route` shadowing the real one |
| Flask won't start | Port 7878 already in use |

## External assets

- `model.onnx` — BiRefNet или RMBG-2.0/IS-Net, кладётся вручную в `models/` (не в репозитории, в `.gitignore`)
- `mask.png` — защита областей для ластика (поставляется с приложением). Включается тумблером «Защита» в редакторе (`state.protectionEnabled`, config `protectionEnabled`), по умолчанию ВКЛЮЧЕНА — при включённой тёмные области маски защищены от стирания (и ластик, и розовая маска пропускают их). Маска — полноразмерный дизайн всего канваса (у пользователя `mask.png` = 6144×6144): `buildProtectionCanvasFromImg()` растягивает её на весь рабочий канвас (случай 1:1 при `canvasScale` 3). ВАЖНО: при выключенной защите маска не грузится вовсе — иначе тёмный `mask.png` блокирует стирание (баг «ластик не работает»).
- `token_rings/` — folder with ring PNG/WebP files (optional)
- `presets/` — folder with preset mask images (optional)
- Images loaded from clipboard/files are never stored on disk
