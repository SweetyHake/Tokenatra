# Tokenatra — AGENTS.md

## Commands

```bat
start.bat              # auto-installs deps + launches desktop app (pywebview, no console)
launch.vbs             # launches start.bat completely hidden (no window at all)
python app.py          # launches with console (for debugging)
python server.py       # standalone Flask on :7878 (no GUI window)
python server.py --remove-bg <file>   # CLI: remove background, saves <stem>_nobg.webp
python server.py --to-webp <file>     # CLI: convert to WebP, deletes original
```

## Build

```bat
build_installer.bat       # Step 1: PyInstaller -> Step 2: Inno Setup installer
build.bat                 # PyInstaller only (portable folder)
```

PyInstaller output: `dist/Tokenatra/Tokenatra.exe`
Installer output: `dist/installer/Tokenatra_Setup_v*.exe`

### GitHub Releases: ассеты по платформам

Релиз содержит: `Tokenatra_Setup_v*.exe` (Windows), `Tokenatra_<ver>_arm64.dmg` / `_x86_64.dmg` (macOS), `Tokenatra_<ver>_x86_64.AppImage` + `.tar.gz` (Linux). Все собирает CI (`build-release.yml`); имена ассетов значимы — updater ищет файл по суффиксу архитектуры.

Автообновление работает на всех трёх платформах:
- **Windows**: качает Setup-установщик (приоритет в `_find_exe_asset`) — тихая установка через bat.
- **macOS**: качает DMG своей архитектуры; `/apply_update` пишет `_update.sh`, который после закрытия приложения монтирует образ, подменяет .app (с бэкапом на время подмены) и перезапускает. Запуск из /Volumes или вне .app → отказ с подсказкой.
- **Linux**: качает AppImage; атомарный `os.replace` поверх работающего файла + перезапуск. Работает только из AppImage ($APPIMAGE).

Голый `dist/Tokenatra/Tokenatra.exe` — тонкий загрузчик без `_internal`; отдельно не публиковать.

### Публикация релиза (пошагово)

Версии по схеме `год.мажор.фикс` (2026 → `26.x.y`), тег `v26.x.y`.

> **CI:** пуш тега `v*` запускает `.github/workflows/build-release.yml` — он сам собирает все платформы и прикладывает ассеты к релизу: `Tokenatra_Setup_v*.exe` (Windows), `Tokenatra_*.dmg` (macOS), `Tokenatra_*_x86_64.AppImage` + `.tar.gz` (Linux). Ручные шаги 4-5 ниже нужны только если CI недоступен.

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

## Dependencies

Единый `requirements.txt` — источник правды для `start.bat`, `start.sh` и CI.
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

## Critical file: `server.py` gotchas

- **Duplicate route functions**: проверено — дубликатов нет, каждый `@app.route` объявлен ровно один раз.
- **Блок `if __name__ == '__main__'` обязан быть в КОНЦЕ `server.py`**: при запуске `python server.py` `app.run()` блокирует модуль, и маршруты ниже не регистрируются (получали 404 на `/config`, `/save_file`, `/shutdown` в dev-режиме).
- Every `@app.route` must be declared exactly once with the decorator.
- Models live in `BASE_DIR/models/` (`get_selected_model_path()`: config `selected_model` → первый `*.onnx`). При первом запуске `model.onnx` из `BASE_DIR` автоматически перемещается в `models/`.

## JS load order (strict, from `index.html` lines 713-725)

```
config.js → state.js → utils.js → i18n.js → urlManager.js → tokenEffects.js →
tokenHistory.js → tokenPresets.js → tokenCanvas.js → tokenEditor.js →
portraitGenerator.js → hotkeySettings.js → remover.js → main.js
```

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
| `/presets_list` | Reads from `presets/` subfolder (not BASE_DIR root). |
| `/preset_file/<filename>` | Serves from `presets/` subfolder. |
| `/process` POST | Background removal. Accepts `image` (file), `format` (webp/png/jpg), `quality` (int 10-100), `edge_blur` (float 0-10). |

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
