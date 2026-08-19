# Tokenatra

<p align="center">
  <img src="logo.png" alt="Tokenatra" width="180">
</p>

<p align="center">
  Локальное Windows-приложение для создания токенов персонажей,<br>
  удаления фона и пакетной конвертации файлов.
</p>

<p align="center">
  <a href="#русский">Русский</a> ·
  <a href="#english">English</a>
</p>

---

## Русский

Tokenatra готовит изображения персонажей для Foundry VTT и других виртуальных игровых столов. Приложение работает локально: исходные изображения не отправляются в интернет, а удаление фона выполняется ONNX-моделью на компьютере пользователя.

## Скриншоты

### Редактор токенов

![Редактор токенов](docs/screenshots/token-editor-example.png)

Загрузка изображения, круглый холст, пресеты масок, инструменты ручной обработки, кольца и параметры экспорта находятся в одном окне.

### Работа с файлами

![Работа с файлами](docs/screenshots/file-tools.png)

Раздел объединяет конвертер и пакетное удаление фона. Поддерживаются изображения, аудио и видео.

### Настройки

![Настройки приложения](docs/screenshots/settings.png)

Здесь выбираются ONNX-модель, рабочее разрешение холста, язык, форматы сохранения и горячие клавиши.

### О программе

![О программе](docs/screenshots/about.png)

На экране «О программе» отображаются версия, ссылка на GitHub, состояние модели и выбранное устройство обработки.

## Возможности

### Создание токенов

- Загрузка изображений через файл, перетаскивание или буфер обмена.
- Круглый рабочий холст с разрешением 1 536, 3 072 или 6 144 пикселя.
- Масштабирование, перемещение и поворот изображения.
- Удаление фона одной кнопкой и просмотр исходного изображения.
- Ластик и розовая маска для ручной обработки результата.
- Пресеты масок и опциональная защита областей от стирания.
- Отображение зон обработки и границ экспортируемой области.
- Кольца токенов из папки `token_rings/` и добавление собственных колец.
- Настраиваемая тень: угол, смещение, размытие и прозрачность.
- Цветокоррекция: насыщенность и яркость.
- Пример изображения для проверки масштаба и композиции.
- Портретный предпросмотр и сохранение портретного варианта.
- История действий с отменой и повтором.

### Удаление фона

- Пакетная обработка нескольких изображений.
- Форматы результата: WEBP, PNG, JPG и AVIF.
- Качество сжатия и смягчение краёв маски.
- Сравнение оригинала и результата.
- Копирование результата в буфер обмена.
- Открытие результата в редакторе токенов.
- Сохранение всей очереди в выбранную папку.

### Конвертер файлов

- Пакетная конвертация изображений, аудио и видео.
- Изображения: WEBP, PNG, JPG, AVIF, BMP, GIF и TIFF.
- Аудио и видео: конвертация в OGG через `ffmpeg`.
- Настройка качества для поддерживаемых форматов.
- Сохранение отдельных файлов или всей очереди.

### Дополнительные возможности

- Автоматический выбор CUDA, ROCm, DirectML или CPU.
- Ручной выбор GPU и ONNX-модели в настройках.
- Загрузка модели с сервера или с диска.
- Настраиваемые горячие клавиши.
- Контекстное меню Windows для удаления фона, конвертации изображений в WebP и аудио в OGG.
- Проверка новых версий через GitHub Releases.
- Быстрое сохранение в заранее выбранную папку.

## Установка

### Готовый установщик

1. Откройте раздел [Releases](https://github.com/SweetyHake/Tokenatra/releases).
2. Скачайте `Tokenatra_Setup_v*.exe`.
3. Запустите установщик и следуйте инструкциям.
4. Поместите ONNX-модель в папку `models/` рядом с приложением.
5. Запустите Tokenatra и выберите модель в разделе «Настройки» → «Модель удаления фона».

Модель не входит в установщик из-за большого размера. Поддерживаются модели удаления фона BiRefNet, RMBG-2.0 и IS-Net в формате `.onnx`. При необходимости модель можно загрузить прямо из окна настроек.

### Запуск из исходников

Требуется Windows и Python 3. Скрипт `start.bat` автоматически проверяет зависимости, устанавливает их при необходимости, пересобирает приложение PyInstaller и запускает desktop-оболочку без консольного окна:

```bat
start.bat
```

Для запуска с консолью и диагностики:

```bat
python app.py
```

Для запуска только локального Flask-сервера:

```bat
python server.py
```

После запуска интерфейс доступен по адресу `http://127.0.0.1:7878`.

Зависимости устанавливаются из `start.bat`:

```bat
python -m pip install onnxruntime-directml numpy Pillow flask pywebview psutil imageio-ffmpeg
```

## ONNX-модель

1. Создайте папку `models/` рядом с `Tokenatra.exe` или в корне проекта.
2. Поместите в неё одну или несколько моделей с расширением `.onnx`.
3. Откройте «Настройки» → «Модель удаления фона».
4. Выберите модель из списка или загрузите её с диска.

Встроенный загрузчик также умеет скачать модель с сервера. По умолчанию используется RMBG-2.0; URL можно переопределить переменной окружения `TOKENATRA_MODEL_URL`.

Примеры совместимых моделей:

- [RMBG-2.0](https://huggingface.co/briaai/RMBG-2.0)
- [BiRefNet ONNX](https://huggingface.co/DanielLavric/BiRefNet-ONNX)

Приложение само подготавливает входное изображение, выполняет нормализацию и приводит результат модели к маске прозрачности. При сбое GPU-провайдера обработка автоматически переключается на CPU.

## Команды обработки

Удалить фон из одного файла:

```bat
python server.py --remove-bg path\to\image.png
```

Результат сохранится рядом с исходным файлом под именем `*_nobg.webp`.

Конвертировать изображение в WebP:

```bat
python server.py --to-webp path\to\image.png
```

Для интеграции с контекстным меню desktop-оболочка также поддерживает команды `--to-ogg` и `--folder-to-webp`.

## Горячие клавиши по умолчанию

| Действие | Клавиша |
|---|---|
| Перемещение изображения | `V` |
| Ластик | `F` |
| Маска | `G` |
| Удалить или показать фон | `R` |
| Отменить действие | `Ctrl+Z` |
| Повторить действие | `Ctrl+Y` |
| Повернуть влево | `Q` |
| Повернуть вправо | `E` |
| Открыть файл | `Ctrl+O` |
| Сохранить все результаты | `Ctrl+S` |

Горячие клавиши можно изменить в разделе «Настройки» → «Горячие клавиши».

## Сборка

Переносимая сборка PyInstaller:

```bat
build.bat
```

Результат появится в `dist/Tokenatra/`. Это папка целиком, а не отдельный файл `Tokenatra.exe`: exe требует расположенную рядом папку `_internal`.

Сборка установщика Inno Setup:

```bat
build_installer.bat
```

Результат появится в `dist/installer/Tokenatra_Setup_v*.exe`. Для этой команды требуется [Inno Setup 6](https://jrsoftware.org/isdl.php).

## Структура проекта

```text
Tokenatra/
├── app.py                 # Desktop-оболочка pywebview
├── server.py              # Flask-сервер и обработка изображений
├── updater.py             # Проверка обновлений GitHub Releases
├── context_menu_helper.py # Команды контекстного меню Windows
├── templates/             # HTML-шаблоны интерфейса
├── static/                # JavaScript, CSS и Web Worker
├── models/                # Локальные ONNX-модели, не входят в Git
├── token_rings/            # Кольца токенов
├── presets/                # Пресеты масок
├── docs/screenshots/       # Скриншоты для документации
├── mask.png               # Маска защиты областей
├── start.bat              # Установка зависимостей и запуск
├── build.bat              # Переносимая сборка
└── build_installer.bat    # Сборка установщика
```

## Технологии

- Python и Flask.
- pywebview с оболочкой EdgeChromium.
- Vanilla JavaScript и CSS без frontend-фреймворков.
- Pillow для обработки изображений.
- ONNX Runtime для локального удаления фона.
- `imageio-ffmpeg` для конвертации аудио и видео.
- Web Worker для производительной работы ластика.

---

## English

Tokenatra is a local Windows application for preparing character images for Foundry VTT and other virtual tabletop platforms. It creates circular tokens, removes backgrounds, and batch-converts files. Source images stay on the local machine; background removal runs with an ONNX model on the user’s computer.

## Screenshots

![Token editor](docs/screenshots/token-editor-example.png)
![File tools](docs/screenshots/file-tools.png)
![Settings](docs/screenshots/settings.png)
![About](docs/screenshots/about.png)

## Features

- Token editor with file, drag-and-drop, and clipboard input.
- Circular working canvas at 1,536, 3,072, or 6,144 pixels.
- Image scaling, moving, rotation, background removal, eraser, and mask tools.
- Mask presets, protected areas, token rings, drop shadow, and color correction.
- Portrait preview and export, example-image overlay, undo and redo history.
- Batch background removal to WEBP, PNG, JPG, or AVIF.
- Batch image conversion to WEBP, PNG, JPG, AVIF, BMP, GIF, or TIFF.
- Audio and video conversion to OGG through `ffmpeg`.
- Model and GPU selection, model download from Settings, and CPU fallback.
- Rebindable shortcuts, Windows Explorer context-menu integration, and update checks.

## Installation

1. Download `Tokenatra_Setup_v*.exe` from the [Releases](https://github.com/SweetyHake/Tokenatra/releases) page.
2. Install and launch Tokenatra.
3. Put one or more `.onnx` background-removal models in the `models/` folder next to the application.
4. Select a model in Settings, or use the built-in model download button.

The installer does not include the model because of its size. BiRefNet, RMBG-2.0, and IS-Net ONNX models are supported.

## Run from source

Windows and Python 3 are required:

```bat
start.bat
```

The script installs missing dependencies, rebuilds the PyInstaller application, and launches it without a console window. For debugging, use `python app.py`. To run the Flask server directly, use `python server.py`; the local interface is available at `http://127.0.0.1:7878`.

## CLI

```bat
python server.py --remove-bg path\to\image.png
python server.py --to-webp path\to\image.png
```

The first command writes `*_nobg.webp` next to the source file. The second converts an image to WebP.

## Build

```bat
build.bat
build_installer.bat
```

The portable build is written to `dist/Tokenatra/`. The installer is written to `dist/installer/` and requires [Inno Setup 6](https://jrsoftware.org/isdl.php).

## Technology

- Python, Flask, Pillow, and pywebview.
- Vanilla JavaScript and CSS.
- ONNX Runtime for local background removal.
- `imageio-ffmpeg` for media conversion.
- A Web Worker for responsive eraser operations.

---

<p align="center">
  <sub>Tokenatra 26.0.0 · <a href="https://github.com/SweetyHake/Tokenatra">GitHub</a></sub>
</p>
