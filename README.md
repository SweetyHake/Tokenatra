# Tokenatra

Редактор токенов для Foundry VTT и фоновым удалением (AI ONNX): загрузи арта → вырезка фона → кольцо → экспорт.

Нативная **Rust-сборка**: axum (HTTP) + ort (ONNX Runtime: DirectML/CUDA/CoreML/ROCm) + wry/tao (GUI).
Один бинарник, без Python-окружения.

---

## Возможности

- **AI-удаление фона** (RMBG-2.0 / BEN2 / BiRefNet, ONNX) с GPU-ускорением
- **Редактор токенов**: масштаб/поворот, ластик, кольца, тени, цветокоррекция, пресеты
- **Горячие клавиши**, портретный генератор, конвертер (PNG/WebP/JPG/AVIF)
- **Foundry VTT коннектор**: хостинг студии в мире, GUI-препроцессинг у игроков
- **Экспорт**: PNG / WebP / JPG, встроенный апдейтер
- **Агентский API** `/agent/v1/*` (JSON) + MCP-прокси — для ИИ-агентов (Claude, OpenCode, DeepSeek)

## Установка

### Windows (установщик)
Скачай `Tokenatra_v*_windows_x86_64_setup.exe` со страницы
[Releases](https://github.com/SweetyHake/Tokenatra/releases) и установи.
Модель `.onnx` кладётся вручную в `models/` рядом с приложением (или выбери в Настройках).

### Из исходников (Rust)
Требуется Rust toolchain (https://rustup.rs). На Linux — WebKit2GTK и GTK3.

```bash
cargo run --release
```

Интерфейс откроется в окне приложения; сервер слушает `http://127.0.0.1:7878`.

Headless-режим (только сервер, без окна):

```bash
cargo run --release -- --server
```

## Agent API (для ИИ-агентов)

Все операции — JSON поверх HTTP на `http://127.0.0.1:7878/agent/v1/*`
с заголовком `X-Tokenatra-Agent: 1` (только localhost).

```
GET  /agent/v1/manifest                 # возможности и лимиты
POST /agent/v1/sessions                 {image: base64}     -> session_id
POST /agent/v1/sessions/<id>/remove_bg  {edge_blur}
POST /agent/v1/sessions/<id>/transform  {scale, offset_x, offset_y}
POST /agent/v1/sessions/<id>/mask       {op: erase|restore|feather|grow|...}
POST /agent/v1/sessions/<id>/ring       {name}
POST /agent/v1/sessions/<id>/export     {format, size, scale_mode, with_ring}
GET  /agent/v1/sessions/<id>/preview
DELETE /agent/v1/sessions/<id>
```

Полное руководство с якорями композиции и рецептами:
[`W:\Images\STokens\TOKEN_STYLE.md`](TOKEN_STYLE.md) (локально) и `docs/`.

MCP-прокси: `tokenatra_mcp.py` (stdio, только stdlib) — подключается
к Claude Desktop / OpenCode: `python tokenatra_mcp.py`.

## Сборка установщика (Windows)

```bat
build_installer.bat
```

(cargo build + Inno Setup → `dist\installer\Tokenatra_v*_windows_x86_64_setup.exe`)

## Foundry VTT модуль

`foundry-module/` — коннектор Tokenatra ↔ Foundry (manifest URL или zip из
`build_module.bat`).

---

## English

**Tokenatra** — a token editor for Foundry VTT with AI background removal.
Native **Rust edition**: axum (HTTP) + ort (ONNX Runtime: DirectML/CUDA/CoreML/ROCm) + wry/tao GUI.
Single binary, no Python environment required.

### Install

Download `Tokenatra_v*_windows_x86_64_setup.exe` from
[Releases](https://github.com/SweetyHake/Tokenatra/releases).
Put your `.onnx` model into the `models/` folder next to the app.

### Build from source

Requires the Rust toolchain (https://rustup.rs). On Linux, WebKit2GTK + GTK3 are needed.

```bash
cargo run --release
```

The UI opens in the app window; the server listens on `http://127.0.0.1:7878`.
Headless server mode: `cargo run --release -- --server`.

### Agent API (for AI agents)

JSON over HTTP at `http://127.0.0.1:7878/agent/v1/*` with header
`X-Tokenatra-Agent: 1` (localhost only). See the endpoints above.
MCP stdio proxy: `python tokenatra_mcp.py`.

### Windows installer

```bat
build_installer.bat
```
