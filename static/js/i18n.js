const I18n = {
    _language: 'ru',
    _observer: null,
    _applying: false,
    _textSources: new WeakMap(),
    _attributeSources: new WeakMap(),
    _attributeWritten: new WeakMap(),

    translations: {
        'Доступно обновление': 'Update available',
        'Загрузка…': 'Loading…',
        'Создание токена': 'Token Maker',
        'Работа с файлами': 'File tools',
        'Настройки': 'Settings',
        'О программе': 'About',
        'Открыть настройки приложения': 'Open application settings',
        'Открыть информацию о приложении': 'Open application information',
        'Свернуть': 'Minimize',
        'Развернуть': 'Maximize',
        'Закрыть': 'Close',
        'Открыть «О программе»': 'Open “About”',
        'Скрыть': 'Hide',
        'Открыть': 'Open',
        'Файл не выбран': 'No file selected',
        'Предыдущий файл (Ctrl+Left)': 'Previous file (Ctrl+Left)',
        'Следующий файл (Ctrl+Right)': 'Next file (Ctrl+Right)',
        'Открыть файл (Ctrl+O)': 'Open file (Ctrl+O)',
        'Отменить (Ctrl+Z)': 'Undo (Ctrl+Z)',
        'Повторить (Ctrl+Y)': 'Redo (Ctrl+Y)',
        'Отменить действие': 'Undo action',
        'Повторить действие': 'Redo action',
        'Инструмент перемещения': 'Move tool',
        'Ластик (синий)': 'Eraser (blue)',
        'Маска (розовая)': 'Mask (pink)',
        'Удалить / восстановить фон': 'Remove / restore background',
        'Повернуть влево': 'Rotate left',
        'Повернуть вправо': 'Rotate right',
        'Открыть файл': 'Open file',
        'Сохранить все файлы': 'Save all files',
        'Вырезать фон (R)': 'Remove background (R)',
        'Нажмите для сравнения': 'Click to compare',
        'Копировать в буфер': 'Copy to clipboard',
        'Открыть в редакторе': 'Open in editor',
        'Скачать': 'Download',
        'Инструменты': 'Tools',
        'История': 'History',
        'Файлы': 'Files',
        'Пресеты масок': 'Mask presets',
        'Нет пресетов': 'No presets',
        'Добавить пресет': 'Add preset',
        'Удалить пресет': 'Delete preset',
        'Сохранить текущую маску как пресет': 'Save the current mask as a preset',
        'Параметры холста': 'Canvas parameters',
        'Область защиты': 'Protection area',
        'Отображать маску': 'Show mask',
        'Отображать границы': 'Show boundaries',
        'Действия': 'Actions',
        'Вырезать фон': 'Remove background',
        'Перемещение': 'Move',
        'Ластик': 'Eraser',
        'Маска': 'Mask',
        'Размер': 'Size',
        'Сброс': 'Reset',
        'Трансформация': 'Transform',
        'Масштаб': 'Scale',
        'Поворот': 'Rotation',
        'Сбросить трансформацию': 'Reset transform',
        'Уменьшить': 'Zoom out',
        'Увеличить': 'Zoom in',
        'Сбросить': 'Reset',
        'Кольцо': 'Ring',
        'Добавить кольцо': 'Add ring',
        'Тень': 'Shadow',
        'Сбросить настройки тени': 'Reset shadow settings',
        'Угол': 'Angle',
        'Смещение': 'Offset',
        'Размытие': 'Blur',
        'Прозрачность': 'Opacity',
        'Цветокоррекция': 'Color correction',
        'Сбросить цветокоррекцию': 'Reset color correction',
        'Насыщенность': 'Saturation',
        'Яркость': 'Brightness',
        'Превью': 'Preview',
        'Пример': 'Example',
        'Портрет': 'Portrait',
        'Показывать': 'Show',
        'Файл:': 'File:',
        'Файл': 'File',
        'Сбросить': 'Reset',
        'Папка не выбрана': 'No folder selected',
        'Папка models': 'models folder',
        'Папка': 'Folder',
        'Сохранить': 'Save',
        'Экспорт': 'Export',
        'Размер:': 'Size:',
        'Масштаб:': 'Scale:',
        'Масштаб подбирается автоматически по размеру изображения на холсте': 'Scale is selected automatically based on the image size on the canvas',
        'Авто': 'Auto',
        'Быстрое сохранение': 'Quick save',
        'Выбрать': 'Choose',
        'С кольцом': 'With ring',
        'Без кольца': 'Without ring',
        'Загрузите изображение': 'Load an image',
        'Перетащите файл сюда или вставьте из буфера': 'Drop a file here or paste it from the clipboard',
        'Работа с файлами': 'File Tools',
        'Удаление фона и конвертация в одном рабочем окне.': 'Background removal and conversion in one workspace.',
        'Инструмент': 'Tool',
        'Конвертер': 'Converter',
        'Удаление фона': 'Background removal',
        'Добавьте изображения': 'Add images',
        'Перетащите файлы сюда или нажмите для выбора': 'Drop files here or click to choose',
        'Формат файла:': 'File format:',
        'Качество:': 'Quality:',
        'Очистить': 'Clear',
        'Сохранить все': 'Save all',
        'Результаты появятся здесь': 'Results will appear here',
        'Добавьте изображение для удаления фона': 'Add an image to remove its background',
        'Добавьте файлы для конвертации': 'Add files to convert',
        'Изображения, аудио и видео • можно несколько файлов': 'Images, audio and video • multiple files supported',
        'OGG (аудио)': 'OGG (audio)',
        'Добавьте изображение, аудио или видео': 'Add an image, audio or video',
        'ПРИЛОЖЕНИЕ': 'APPLICATION',
        'ЯЗЫК': 'LANGUAGE',
        'Язык интерфейса': 'Interface language',
        'Язык:': 'Language:',
        'Как в системе': 'System default',
        'Русский': 'Russian',
        'Выберите язык интерфейса приложения.': 'Choose the application interface language.',
        'Настройки приложения': 'Application settings',
        'Параметры обработки, экспорта и горячих клавиш.': 'Processing, export and hotkey preferences.',
        'РАБОЧАЯ СРЕДА': 'WORKSPACE',
        'Основные параметры': 'General settings',
        'Изменения сохраняются автоматически': 'Changes are saved automatically',
        'МОДЕЛЬ': 'MODEL',
        'Модель удаления фона': 'Background removal model',
        'Проверка модели…': 'Checking model…',
        'Модель готова к работе': 'Model is ready',
        'Модель не установлена': 'Model is not installed',
        'Модель для удаления фона не установлена': 'Background removal model is not installed',
        'Нет моделей (.onnx)': 'No models (.onnx)',
        'текущая': 'current',
        'Загрузка модели с сервера…': 'Downloading model from server…',
        'Копирование модели…': 'Copying model…',
        'Модель:': 'Model:',
        'Выберите модель удаления фона': 'Choose a background removal model',
        'Загрузка модели с сервера': 'Download model from server',
        'Загрузить модель с диска': 'Load model from disk',
        'Загрузить модель удаления фона с сервера': 'Load a background removal model from the server',
        'Загрузить модель удаления фона с диска': 'Load a background removal model from disk',
        'ПРОИЗВОДИТЕЛЬНОСТЬ': 'PERFORMANCE',
        'Рабочий канвас': 'Working canvas',
        'Размер холста:': 'Canvas size:',
        'Размер рабочего канваса': 'Working canvas size',
        'Производительность': 'Performance',
        'Баланс': 'Balanced',
        'Качество': 'Quality',
        'История отмен:': 'Undo history:',
        'Количество шагов отмены': 'Number of undo steps',
        '10 шагов': '10 steps',
        '20 шагов': '20 steps',
        '30 шагов': '30 steps',
        '50 шагов': '50 steps',
        '100 шагов': '100 steps',
        'Больший размер даёт больше деталей при работе, но требует больше памяти. На экспортируемое изображение это не влияет — экспорт всегда выполняется в полном разрешении.': 'A larger canvas provides more detail but uses more memory. It does not affect exports: they always use the full output resolution.',
        'Больше шагов отмены — больше расход памяти. Применяется к новым записям истории.': 'More undo steps use more memory. This applies to new history entries.',
        'СОХРАНЕНИЕ': 'SAVING',
        'Сохранение по умолчанию': 'Default save settings',
        'Размер токена:': 'Token size:',
        'Размер сохраняемого изображения': 'Size of the saved image',
        'Формат пакетной обработки:': 'Batch processing format:',
        'Формат пакетной обработки': 'Batch processing format',
        'Формат токена:': 'Token format:',
        'Формат сохраняемого токена': 'Format of the saved token',
        'Качество токена (WebP/JPG):': 'Token quality (WebP/JPG):',
        'Качество токена для WebP/JPG': 'Token quality for WebP/JPG',
        'КЛАВИШИ': 'HOTKEYS',
        'Горячие клавиши': 'Hotkeys',
        'Настройте управление под свой рабочий процесс.': 'Customize controls for your workflow.',
        'Поиск по клавишам…': 'Search hotkeys…',
        'Сбросить по умолчанию': 'Reset to defaults',
        'Сбросить все горячие клавиши': 'Reset all hotkeys',
        'О ПРОГРАММЕ': 'ABOUT',
        'Локальный редактор токенов': 'Local token editor',
        'Версия ': 'Version ',
        'Проверить обновления': 'Check for updates',
        'Подготовьте изображение для игры': 'Prepare an image for your game',
        'Откройте портрет из файла, перетащите его в окно или вставьте из буфера. Затем удалите фон,': 'Open a portrait from a file, drop it into the window, or paste it from the clipboard. Then remove the background,',
        'настройте композицию и сохраните круглый токен для Foundry VTT. Исходные изображения не отправляются в интернет.': 'adjust the composition, and save a round token for Foundry VTT. Source images never leave your computer.',
        'Возможности': 'Features',
        'Удаление фона': 'Background removal',
        'Модель отделяет персонажа от фона. Результат можно уточнить ластиком и маской.': 'The model separates the subject from the background. Refine the result with the eraser and mask.',
        'Настройка композиции': 'Composition controls',
        'Масштабируйте, перемещайте и поворачивайте изображение на круглом холсте.': 'Scale, move, and rotate the image on a round canvas.',
        'Оформление токена': 'Token styling',
        'Добавьте кольцо, тень и цветокоррекцию или подготовьте портретный вариант.': 'Add a ring, shadow, and color correction, or prepare a portrait version.',
        'Пакетная обработка': 'Batch processing',
        'Обрабатывайте несколько изображений и сохраняйте их в WEBP, PNG, JPG или AVIF.': 'Process multiple images and save them as WEBP, PNG, JPG, or AVIF.',
        'Модель и устройство выбираются в настройках': 'The model and device are selected in Settings',
        'Изображение вставлено': 'Image pasted',
        'Вставлено в конвертер': 'Pasted into converter',
        'Вставлено в вырезатель': 'Pasted into background remover',
        'Вставлено из буфера': 'Pasted from clipboard',
        'Обработка…': 'Processing…',
        'Конвертация…': 'Converting…',
        'Не удалось прочитать изображение': 'Could not read the image',
        'Ошибка обработки': 'Processing error',
        'Не удалось загрузить результат': 'Could not load the result',
        'Маска сброшена': 'Mask reset',
        'Ластик сброшен': 'Eraser reset',
        'Сначала загрузите изображение': 'Load an image first',
        'Ошибка сохранения': 'Save error',
        'Ошибка выбора папки': 'Folder selection error',
        'Трансформация сброшена': 'Transform reset',
        'Защита областей включена': 'Area protection enabled',
        'Защита областей выключена': 'Area protection disabled',
        'Тень сброшена': 'Shadow reset',
        'Цветокоррекция сброшена': 'Color correction reset',
        'Пример сброшен': 'Example reset',
        'Файл example.png не найден': 'File example.png was not found',
        'Не удалось загрузить файл': 'Could not load the file',
        'Фон удалён': 'Background removed',
        'Действие отменено': 'Action undone',
        'Нечего отменять': 'Nothing to undo',
        'Повтор': 'Redo',
        'Нечего повторять': 'Nothing to redo',
        'Скопировано в буфер': 'Copied to clipboard',
        'Не удалось скопировать': 'Could not copy',
        'Результат не найден': 'Result not found',
        'Открыто в редакторе': 'Opened in editor',
        'Очищено': 'Cleared',
        'Не удалось удалить кольцо': 'Could not delete the ring',
        'Не удалось добавить кольцо': 'Could not add the ring',
        'Ошибка выбора модели': 'Model selection error',
        'операция не выполнена': 'operation failed',
        'Ошибка сервера': 'Server error',
        'Ошибка': 'Error',
        'Доступна версия ': 'Version available ',
        ' (текущая: ': ' (current: ',
        'Обновление ': 'Update ',
        ' скачано. Установить сейчас?': ' downloaded. Install now?',
        'Обновлений нет — у вас последняя версия': 'No updates available — you have the latest version',
        'Не удалось проверить обновления (нет соединения с GitHub)': 'Could not check for updates (no connection to GitHub)',
        'Не удалось проверить обновления': 'Could not check for updates',
        'Скачать': 'Download',
        'Установить': 'Install',
        'Позже': 'Later',
        'Сравнение': 'Compare',
        'Добавить кольцо': 'Add ring',
        'Изображение кольца': 'Ring image',
        'Название': 'Name',
        '(необязательно)': '(optional)',
        'Например, Серебряная окантовка': 'For example, Silver border',
        'Отмена': 'Cancel',
        'Добавить': 'Add',
        'Нет пресетов в папке presets/': 'No presets in the presets/ folder',
        'Нет колец': 'No rings',
        'Без кольца': 'No ring',
        'Удалить кольцо': 'Delete ring',
        'Сталь': 'Steel',
        'Красный обсидиан': 'Red Obsidian',
        'Штормовая сталь': 'Storm Steel',
        'Руническое дерево': 'Runic Wood',
        'Ковбой (High Noon)': 'Cowboy (High Noon)',
        'Чумной венок': 'Plague Wreath',
        'Аметистовые кристаллы': 'Amethyst Crystals',
        'Театр': 'Theatre',
        'Белые руны': 'White Runes',
        'Применено': 'Applied',
        'Пробел': 'Space',
        'Ошибка': 'Error',
        'Скачивание обновления…': 'Downloading update…',
        'Проверка обновлений…': 'Checking for updates…',
        'Установка обновления… Приложение закроется': 'Installing update… The application will close',
        'Закрыть': 'Close'
    },

    _systemLanguage() {
        return (navigator.language || '').toLowerCase().startsWith('ru') ? 'ru' : 'en';
    },

    get language() {
        return this._language;
    },

    t(value) {
        if (typeof value !== 'string') return value;
        if (this._language === 'ru') {
            const source = Object.keys(this.translations).find(key => this.translations[key] === value);
            return source || value;
        }
        if (this.translations[value]) return this.translations[value];

        const patterns = [
            [/^Сохранено: (.+)$/, 'Saved: $1'],
            [/^Сохранено файлов: (\d+)$/, 'Files saved: $1'],
            [/^Формат: (.+)$/, 'Format: $1'],
            [/^Файлов: (\d+)$/, 'Files: $1'],
            [/^Сохранить все \((\d+)\)$/, 'Save all ($1)'],
            [/^Масштаб канваса: (.+)$/, 'Canvas scale: $1'],
            [/^Масштаб сохранения: (.+)$/, 'Save scale: $1'],
            [/^Размер: (.+)$/, 'Size: $1'],
            [/^Папка выбрана: (.+)$/, 'Folder selected: $1'],
            [/^Версия (.+)$/, 'Version $1'],
            [/^Показать без фона \((.+)\)$/, 'Show without background ($1)'],
            [/^Показать оригинал \((.+)\)$/, 'Show original ($1)'],
            [/^Доступно обновление v(.+)$/, 'Update v$1 available'],
            [/^Кольцо добавлено$/, 'Ring added'],
            [/^Кольцо удалено$/, 'Ring deleted'],
            [/^Пресет «(.+)» применён$/, 'Preset “$1” applied'],
            [/^Ошибка: (.+)$/, 'Error: $1'],
            [/^Доступно обновление v(.+)$/, 'Update v$1 available'],
            [/^Модель «(.+)» загружена и выбрана$/, 'Model “$1” was loaded and selected'],
            [/^Модель «(.+)» — будет применена при следующей обработке$/, 'Model “$1” will be used for the next operation'],
            [/^Показан оригинал$/, 'Original shown'],
            [/^Показан без фона$/, 'Background-removed image shown'],
            [/^Клавиша назначена: (.+)$/, 'Key assigned: $1'],
            [/^(\d+)px × (\d+) = (\d+)px итоговый файл$/, '$1px × $2 = $3px final file']
        ];
        const resetMatch = value.match(/^«(.+)» сброшено до умолчания$/);
        if (resetMatch) return '“' + this.t(resetMatch[1]) + '” reset to default';
        const allResetMatch = value === 'Все горячие клавиши сброшены до умолчания';
        if (allResetMatch) return 'All hotkeys reset to default';
        for (const [pattern, replacement] of patterns) {
            if (pattern.test(value)) return value.replace(pattern, replacement);
        }
        return value;
    },

    _translateTextNode(node) {
        if (node.parentElement?.closest('script, style')) return;
        const raw = node.nodeValue;
        const trimmed = raw.trim();
        if (!trimmed && !this._textSources.has(node)) return;
        const source = this._textSources.get(node) || trimmed;
        this._textSources.set(node, source);
        const translated = this.t(source);
        const next = raw.replace(trimmed, translated);
        if (next !== raw) node.nodeValue = next;
    },

    apply() {
        if (this._applying) return;
        this._applying = true;
        document.documentElement.lang = this._language;
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
        const textNodes = [];
        while (walker.nextNode()) textNodes.push(walker.currentNode);
        textNodes.forEach(node => this._translateTextNode(node));
        document.querySelectorAll('[data-tooltip], [title], [placeholder], [aria-label]').forEach(el => {
            ['data-tooltip', 'title', 'placeholder', 'aria-label'].forEach(attr => {
                if (!el.hasAttribute(attr)) return;
                let sources = this._attributeSources.get(el);
                if (!sources) { sources = {}; this._attributeSources.set(el, sources); }
                let written = this._attributeWritten.get(el);
                if (!written) { written = {}; this._attributeWritten.set(el, written); }
                const current = el.getAttribute(attr);
                if (!(attr in sources)) {
                    sources[attr] = current;
                } else if (written[attr] !== undefined && current !== written[attr]) {
                    sources[attr] = current;
                }
                const translated = this.t(sources[attr]);
                el.setAttribute(attr, translated);
                written[attr] = translated;
            });
        });
        this._applying = false;
    },

    setLanguage(language) {
        this._language = language === 'system' ? this._systemLanguage() : (language === 'en' ? 'en' : 'ru');
        this.apply();
        document.dispatchEvent(new CustomEvent('languagechange', { detail: { language: this._language } }));
    },

    init() {
        const setting = AppConfig.language || 'system';
        this._language = setting === 'system' ? this._systemLanguage() : (setting === 'en' ? 'en' : 'ru');
        this.apply();
        this._observer = new MutationObserver(() => {
            if (!this._applying) this.apply();
        });
        this._observer.observe(document.body, { childList: true, subtree: true, characterData: true });
    }
};
