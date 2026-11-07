# Tokenatra

Настольное приложение для подготовки токенов и портретов в Foundry VTT со встроенным удалением фона на базе ONNX Runtime. Собирается в один исполняемый файл на Rust (`axum` + `ort` + `wry`/`tao`) без внешних зависимостей от Python.

## Основные возможности

- Удаление фона локальными моделями RMBG-2.0, BEN2 и BiRefNet с аппаратным ускорением (DirectML и CUDA на Windows, CoreML на macOS, CPU/CUDA/ROCm на Linux).
- Редактор токенов: масштабирование, поворот, маски и ластик (по холсту и по слою), каталог колец, тени, цветокоррекция, сохранение пресетов.
- Генератор портретов и пакетный конвертер изображений (PNG, WebP, JPG, AVIF) и аудио (OGG через FFmpeg).
- Модуль-коннектор для Foundry VTT с возможностью открывать студию прямо внутри игрового мира.
- Локальный HTTP API `/agent/v1/*` для автоматизации сборки токенов.

## Установка и запуск

Готовые сборки публикуются на странице [Releases](https://github.com/SweetyHake/Tokenatra/releases):

- Windows: установщик `Tokenatra_v*_windows_x86_64_setup.exe`.
- macOS (Apple Silicon): образ `Tokenatra_v*_macos_arm64.dmg` или архив `Tokenatra_v*_macos_arm64.tar.gz`.
- Linux (x86_64): архив `Tokenatra_v*_linux_x86_64.tar.gz`.

Модели `.onnx` можно скачать прямо в разделе «Настройки» внутри приложения или положить вручную в каталог `models/` в папке пользовательских данных:
- Windows: `%LOCALAPPDATA%\Tokenatra\models`
- macOS: `~/Library/Application Support/Tokenatra/models`
- Linux: `~/.local/share/Tokenatra/models`

## Сборка из исходников

Потребуется установленный Rust ([rustup.rs](https://rustup.rs)).

На Linux (Ubuntu 24.04+ / Debian / производные) перед сборкой установите системные пакеты GTK3 и WebKit2GTK:

```bash
sudo apt-get update
sudo apt-get install -y \
  libgtk-3-dev \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev
```

Запуск приложения с графическим окном:

```bash
cd rust
cargo run --release
```

После запуска откроется окно приложения, а локальный HTTP-сервер будет доступен по адресу `http://127.0.0.1:7878`.

Запуск только HTTP-сервера без графического окна:

```bash
cd rust
cargo run --release -- --server
```

Сборка установщика для Windows (требуется Inno Setup 6):

```bat
build_installer.bat
```

Сборка архива модуля для Foundry VTT:

```bat
build_module.bat
```

## Agent API

Локальный интерфейс управления редактором работает по адресу `http://127.0.0.1:7878/agent/v1/*` и требует заголовок `X-Tokenatra-Agent: 1`.

Основные маршруты:

```text
GET    /agent/v1/manifest
GET    /agent/v1/assets
POST   /agent/v1/sessions                 {image_path | image, ring}
GET    /agent/v1/sessions/<id>
DELETE /agent/v1/sessions/<id>
POST   /agent/v1/sessions/<id>/remove_bg  {edge_blur, showing_original}
POST   /agent/v1/sessions/<id>/transform  {scale, rotation, offset_x, offset_y, fit_zone}
POST   /agent/v1/sessions/<id>/mask       {target, op, shapes|preset|px, protect}
POST   /agent/v1/sessions/<id>/ring       {name}
POST   /agent/v1/sessions/<id>/effects    {shadow, color}
POST   /agent/v1/sessions/<id>/export     {format, quality, size, scale_mode, with_ring}
GET    /agent/v1/sessions/<id>/preview
GET    /agent/v1/sessions/<id>/inspect
```

## English

Tokenatra is a desktop token editor and background remover for Foundry VTT built with Rust (`axum`, `ort` ONNX Runtime, and `wry`/`tao`).

### Installation

Prebuilt packages for Windows (`setup.exe`), macOS (`.dmg` and `.tar.gz`), and Linux (`.tar.gz`) are available on the [Releases](https://github.com/SweetyHake/Tokenatra/releases) page. Models can be downloaded from the in-app Settings tab or placed manually into the `models/` directory.

### Building from source

Install the Rust toolchain via [rustup.rs](https://rustup.rs). On Linux, install `libgtk-3-dev` and `libwebkit2gtk-4.1-dev` first.

```bash
cd rust
cargo run --release
```

To run in headless server mode on `http://127.0.0.1:7878`:

```bash
cd rust
cargo run --release -- --server
```
