const CONFIG = {
    SCALE_SIZES: { 1: 1536, 2: 3072, 3: 6144 },
    BASE_SIZE: 2048,
    MAX_IMAGE_DIM: 4096,
    EFFECTS_DELAY: 750,
    MAX_HISTORY: 30,
    HISTORY_STORE_SIZE: 1024,
    DEFAULT_SCALE: 100,
    MIN_SCALE: 10,
    MAX_SCALE: 300,
    MIN_ZOOM: 0.5,
    MAX_ZOOM: 20,
    MOVE_STEP: 2,
    ROTATE_STEP: 1,
    PAN_AMOUNT: 30,
    DEBOUNCE_DELAY: 300,
    MIN_ERASER_SIZE: 1,
    MAX_ERASER_SIZE: 300
};

const DEFAULT_HOTKEYS = {
    toolMove:        'KeyV',
    toolEraser:      'KeyF',
    toolMask:        'KeyG',
    toolRemoveBg:    'KeyR',
    undo:            'Ctrl+KeyZ',
    redo:            'Ctrl+KeyY',
    rotateLeft:      'KeyQ',
    rotateRight:     'KeyE',
    openFile:        'Ctrl+KeyO',
    saveAll:         'Ctrl+KeyS'
};

const HOTKEYS_META = {
    toolMove:     { label: 'Инструмент перемещения',        ctrl: false },
    toolEraser:   { label: 'Ластик (синий)',                 ctrl: false },
    toolMask:     { label: 'Маска (розовая)',                ctrl: false },
    toolRemoveBg: { label: 'Удалить / восстановить фон',     ctrl: false },
    undo:         { label: 'Отменить действие',              ctrl: true  },
    redo:         { label: 'Повторить действие',             ctrl: true  },
    rotateLeft:   { label: 'Повернуть влево',                ctrl: false },
    rotateRight:  { label: 'Повернуть вправо',               ctrl: false },
    openFile:     { label: 'Открыть файл',                   ctrl: true  },
    saveAll:      { label: 'Сохранить все файлы',                ctrl: true  }
};

const AppConfig = {
    _data: null,
    _saveTimeout: null,

    _defaults() {
        return {
            theme: 'dark',
            language: 'system',
            canvasScale: 2,
            historyLimit: 30,
            eraserSize: 50,
            edgeBlur: 1,
            hotkeys: { ...DEFAULT_HOTKEYS },
            dropShadowEnabled: false,
            colorCorrectionEnabled: false,
            dropShadow: { angle: 135, distance: 7, blur: 3, opacity: 0.40 },
            colorCorrection: { saturation: 5, lightness: -5 },
            lastFolders: {
                quickSave: null,
                portrait: null,
                remover: null,
                converter: null
            },
            example: {
                enabled: false,
                opacity: 50,
                scaleMode: 2
            },
            remover: {
                format: 'webp',
                quality: 90
            },
            converter: {
                format: 'webp',
                quality: 90
            },
            saveSettings: {
                quality: 512,
                scaleMode: 'auto',
                format: 'webp',
                compression: 95
            },
            selected_model: null,
            protectionEnabled: true,
            quickSaveEnabled: false,
            portraitQuickSaveEnabled: false,
            panelWidths: {
                left: 320,
                right: 320
            }
        };
    },

    async load() {
        try {
            const res = await fetch('/config');
            const saved = await res.json();
            const def = this._defaults();
            const savedHotkeys = { ...(saved.hotkeys || {}) };
            delete savedHotkeys.toolAutoFrame;
            for (const [action, value] of Object.entries(savedHotkeys)) {
                if (typeof value !== 'string') continue;
                const p = parseHotkey(value);
                if (!p.ctrl && !p.alt && !p.shift && HOTKEYS_META[action]?.ctrl) {
                    savedHotkeys[action] = 'Ctrl+' + p.code;
                }
            }
            const savedRemover = { ...(saved.remover || {}) };
            delete savedRemover.tokenMode;
            const savedSS = { ...def.saveSettings, ...(saved.saveSettings || {}) };
            if (!['webp', 'png', 'jpg'].includes(savedSS.format)) savedSS.format = def.saveSettings.format;
            savedSS.compression = Math.min(100, Math.max(1, parseInt(savedSS.compression) || def.saveSettings.compression));
            this._data = {
                theme: saved.theme || def.theme,
                language: ['system', 'ru', 'en'].includes(saved.language) ? saved.language : def.language,
                canvasScale: [1, 2, 3].includes(saved.canvasScale) ? saved.canvasScale : def.canvasScale,
                historyLimit: [10, 20, 30, 50, 100].includes(saved.historyLimit) ? saved.historyLimit : def.historyLimit,
                eraserSize: Math.min(300, Math.max(1, parseInt(saved.eraserSize) || def.eraserSize)),
                edgeBlur: Math.min(10, Math.max(0, isFinite(parseFloat(saved.edgeBlur)) ? parseFloat(saved.edgeBlur) : def.edgeBlur)),
                hotkeys: { ...def.hotkeys, ...savedHotkeys },
                dropShadowEnabled: !!saved.dropShadowEnabled,
                colorCorrectionEnabled: !!saved.colorCorrectionEnabled,
                dropShadow: { ...def.dropShadow, ...(saved.dropShadow || {}) },
                colorCorrection: { ...def.colorCorrection, ...(saved.colorCorrection || {}) },
                lastFolders: { ...def.lastFolders, ...(saved.lastFolders || {}) },
                example: { ...def.example, ...(saved.example || {}) },
                converter: { ...def.converter, ...(saved.converter || {}) },
                remover: { ...def.remover, ...savedRemover },
                saveSettings: savedSS,
                selected_model: saved.selected_model || null,
                protectionEnabled: saved.protectionEnabled === undefined
                    ? def.protectionEnabled
                    : !!saved.protectionEnabled,
                quickSaveEnabled: !!saved.quickSaveEnabled,
                portraitQuickSaveEnabled: !!saved.portraitQuickSaveEnabled,
                panelWidths: { ...def.panelWidths, ...(saved.panelWidths || {}) }
            };
        } catch {
            this._data = this._defaults();
        }
        return this;
    },

    save() {
        if (this._saveTimeout) clearTimeout(this._saveTimeout);
        this._saveTimeout = setTimeout(() => {
            fetch('/config', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(this._data)
            }).catch(() => {});
        }, 500);
    },

    get hotkeys() { return this._data.hotkeys; },
    get dropShadow() { return this._data.dropShadow; },
    get colorCorrection() { return this._data.colorCorrection; },
    get lastFolders() { return this._data.lastFolders; },
    get theme() { return this._data.theme; },
    get language() { return this._data.language; },
    get canvasScale() { return this._data.canvasScale; },
    get historyLimit() { return this._data.historyLimit; },
    get eraserSize() { return this._data.eraserSize; },
    get edgeBlur() { return this._data.edgeBlur; },
    get dropShadowEnabled() { return this._data.dropShadowEnabled; },
get colorCorrectionEnabled() { return this._data.colorCorrectionEnabled; },
    get protectionEnabled() { return this._data.protectionEnabled; },
    get quickSaveEnabled() { return this._data.quickSaveEnabled; },
    get portraitQuickSaveEnabled() { return this._data.portraitQuickSaveEnabled; },

    setTheme(val) { this._data.theme = val; this.save(); },
    setLanguage(val) { this._data.language = ['system', 'ru', 'en'].includes(val) ? val : 'system'; this.save(); },
    setCanvasScale(val) { this._data.canvasScale = val; this.save(); },
    setHistoryLimit(val) { this._data.historyLimit = [10, 20, 30, 50, 100].includes(val) ? val : 30; this.save(); },
    setEraserSize(val) { this._data.eraserSize = Math.min(300, Math.max(1, parseInt(val) || 50)); this.save(); },
    setEdgeBlur(val) { this._data.edgeBlur = Math.min(10, Math.max(0, parseFloat(val) || 0)); this.save(); },
    setProtectionEnabled(val) { this._data.protectionEnabled = !!val; this.save(); },
    setDropShadowEnabled(val) { this._data.dropShadowEnabled = !!val; this.save(); },
    setColorCorrectionEnabled(val) { this._data.colorCorrectionEnabled = !!val; this.save(); },
    setQuickSaveEnabled(val) { this._data.quickSaveEnabled = !!val; this.save(); },
    setPortraitQuickSaveEnabled(val) { this._data.portraitQuickSaveEnabled = !!val; this.save(); },
    setHotkey(action, code) { this._data.hotkeys[action] = code; this.save(); },
    setDropShadow(key, val) { this._data.dropShadow[key] = val; this.save(); },
    setColorCorrection(key, val) { this._data.colorCorrection[key] = val; this.save(); },
    setLastFolder(key, path) { this._data.lastFolders[key] = path; this.save(); },

    resetHotkeys() { this._data.hotkeys = { ...DEFAULT_HOTKEYS }; this.save(); },

    setExample(key, val) { this._data.example[key] = val; this.save(); },

    get example() { return this._data.example; },
    get remover() { return this._data.remover; },
    get converter() { return this._data.converter; },
    get saveSettings() { return this._data.saveSettings; },
    get panelWidths() { return this._data.panelWidths; },
    setRemover(key, val) { this._data.remover[key] = val; this.save(); },
    setConverter(key, val) { this._data.converter[key] = val; this.save(); },
    setSaveSetting(key, val) { this._data.saveSettings[key] = val; this.save(); },
    setPanelWidth(side, val) { this._data.panelWidths[side] = val; this.save(); },
    resetDropShadow() { this._data.dropShadow = this._defaults().dropShadow; this.save(); },
    resetColorCorrection() { this._data.colorCorrection = this._defaults().colorCorrection; this.save(); }
};

const MOVE_KEYS = ['KeyW','KeyA','KeyS','KeyD','ArrowUp','ArrowDown','ArrowLeft','ArrowRight'];
const ERASER_SIZE_KEYS = ['BracketLeft','BracketRight'];

function parseHotkey(str) {
    const parts = (typeof str === 'string' ? str : '').split('+');
    const ctrl = parts.includes('Ctrl');
    const alt = parts.includes('Alt');
    const shift = parts.includes('Shift');
    const code = parts.filter(p => p !== 'Ctrl' && p !== 'Alt' && p !== 'Shift').join('+');
    return { ctrl, alt, shift, code };
}

function serializeHotkey(code, mods) {
    const parts = [];
    if (mods?.ctrl) parts.push('Ctrl');
    if (mods?.alt) parts.push('Alt');
    if (mods?.shift) parts.push('Shift');
    parts.push(code);
    return parts.join('+');
}

function hotkeyMatches(e, str) {
    const p = parseHotkey(str);
    if (!p.code) return false;
    return e.code === p.code
        && !!e.ctrlKey === p.ctrl
        && !!e.altKey === p.alt
        && !!e.shiftKey === p.shift;
}

function codeToLabel(code) {
    const p = parseHotkey(code);
    if (!p.code) return '—';
    const parts = [];
    if (p.ctrl) parts.push('Ctrl');
    if (p.alt) parts.push('Alt');
    if (p.shift) parts.push('Shift');
    parts.push(p.code
        .replace('ArrowUp','↑').replace('ArrowDown','↓')
        .replace('ArrowLeft','←').replace('ArrowRight','→')
        .replace('Key','')
        .replace('Digit','')
        .replace('Space', typeof I18n !== 'undefined' ? I18n.t('Пробел') : 'Пробел').replace('Escape','Esc')
        .replace('BracketLeft','[').replace('BracketRight',']')
        .replace('Backslash','\\').replace('Slash','/')
        .replace('Semicolon',';').replace('Quote',"'")
        .replace('Comma',',').replace('Period','.')
        .replace('Minus','-').replace('Equal','='));
    return parts.join(' + ');
}
