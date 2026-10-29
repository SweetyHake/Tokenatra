# Tokenatra (Rust edition)

Полный порт Python-приложения (`app.py` + `server.py` + `tray_manager.py` +
`updater.py` + `context_menu_helper.py`) на Rust. Фронтенд (`templates/`,
`static/`) и модуль Foundry (`foundry-module/`) используются без изменений:
они общаются с бэкендом по HTTP, контракт роутов воспроизведён 1:1.

## Сборка

```bat
cd rust
cargo build --release
```

Требования: Rust 1.92+, MSVC Build Tools + Windows SDK (для DirectML на
Windows нужен SDK 10.0.19041+; на старых SDK импорт-библиотека `DXCore.lib`
генерируется в `vendor/` из системной `dxcore.dll`, см. `build.rs`).

## Запуск

```
tokenatra.exe                  GUI-приложение (порт 7878)
tokenatra.exe --server         headless-сервер (как `python server.py`)
tokenatra.exe --tune-gpu       подбор адаптера DirectML (вызывается сам)
tokenatra.exe --remove-bg F    контекстное меню: удалить фон
tokenatra.exe --to-webp F      контекстное меню: конвертировать в WebP
tokenatra.exe --to-ogg F       контекстное меню: аудио/видео -> OGG
tokenatra.exe --folder-to-webp D  контекстное меню: папка -> WebP
```

Каталог ресурсов (templates/, static/, token_rings/, presets/, icon.ico)
ищется рядом с exe или в корне репозитория (dev-запуск); переменная
`TOKENATRA_RESOURCE_DIR` переопределяет. Пользовательские данные — там же,
где у Python-версии (LOCALAPPDATA/Tokenatra), общий `config.json` и модели.

## Стек

- HTTP: axum + tower (guard/CORS/CSP-мидлварь — порт `_guard_localhost_only`)
- ONNX: ort 2.0 (DirectML на Windows, CoreML на macOS, CPU на Linux),
  фолбэк-каскад EXTENDED→BASIC→CPU, f16/f32/f64-выходы, tune-gpu
- Изображения: image + imageproc (LANCZOS/bilinear, MinFilter3, гаусс),
  libwebp (lossy WebP как PIL), ravif (AVIF), AVIF-декод через ffmpeg
- GUI: wry/tao; кастомный тайтлбар Windows (DWM + WndProc-хук: NCCALCSIZE,
  NCHITTEST-ресайз, drag через WM_APP+1) — как в app.py
- Трей: tray-icon + muda (меню «Открыть/Модель/Выход»)
- Мост `window.pywebview` эмулируется init-скриптом поверх `/api/window/*`,
  поэтому фронтенд не меняется ни на байт

## Отличия от Python-версии

- AVIF-вход декодируется системным ffmpeg (если есть); AVIF-выход работает
  нативно (ravif). У Python AVIF-выход зависел от pillow-avif-plugin.
- CUDA/ROCm-провайдеры не слинкованы (pip-версия их тоже не поставляла).
- `webp` crate вместо Pillow для lossy-кодирования (тот же libwebp).

## Проверка (2026-10-29, Windows)

- Все 48 роутов: JSON/статусы/CORS/CSP совпадают с Flask-версией
- `/process`: маска IoU 1.0 против Python-сервера на одном входе,
  DirectML 0.27 с на инференс (RMBG/BEN2)
- GUI: окно рендерится, трей/hide_to_tray, minimize/maximize/restore
- CLI-глаголы `--to-webp`/`--remove-bg` проверены
- macOS/Linux: код и cfg-гейты на месте, сборка описана в CI
  (`.github/workflows/rust-build.yml`), на машине не запускалась
