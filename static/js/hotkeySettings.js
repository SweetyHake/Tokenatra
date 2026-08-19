const HOTKEY_CATEGORIES = {
    'Инструменты': ['toolMove', 'toolEraser', 'toolMask', 'toolRemoveBg'],
    'История': ['undo', 'redo'],
    'Трансформация': ['rotateLeft', 'rotateRight'],
    'Файлы': ['openFile', 'saveAll']
};

const HotkeySettings = {
    _listeningAction: null,
    _listeningEl: null,

    init() {
        this._renderTable();
        this._setupResetAll();
        this._setupSearch();
        document.addEventListener('keydown', e => this._onKey(e), true);
        document.addEventListener('languagechange', () => this._renderTable($('hotkeysSearch')?.value));
    },

    _renderTable(filterText) {
        const container = $('hotkeysEditorTable');
        if (!container) return;
        container.innerHTML = '';

        const search = (filterText || '').toLowerCase().trim();

        Object.entries(HOTKEY_CATEGORIES).forEach(([category, actions]) => {
            const items = actions
                .map(action => {
                    const meta = HOTKEYS_META[action];
                    if (!meta) return null;
                    const translatedLabel = I18n.t(meta.label);
                    const label = translatedLabel.toLowerCase();
                    const key = codeToLabel(AppConfig.hotkeys[action]).toLowerCase();
                    if (search && !label.includes(search) && !key.includes(search) && !I18n.t(category).toLowerCase().includes(search)) return null;
                    return { action, meta, label: translatedLabel, keyLabel: codeToLabel(AppConfig.hotkeys[action]) };
                })
                .filter(Boolean);

            if (search && items.length === 0) return;

            const sectionTitle = document.createElement('div');
            sectionTitle.className = 'hotkey-section-title';
            sectionTitle.textContent = I18n.t(category);
            container.appendChild(sectionTitle);

            items.forEach(({ action, meta, label, keyLabel }) => {
                const row = document.createElement('div');
                row.className = 'hotkey-editor-row';

                const labelEl = document.createElement('span');
                labelEl.className = 'hotkey-editor-label';
                labelEl.appendChild(document.createTextNode(label));

                const btn = document.createElement('button');
                btn.className = 'hotkey-bind-btn';
                btn.dataset.action = action;
                btn.textContent = keyLabel;
                btn.onclick = () => this._startListening(action, btn);

                const controls = document.createElement('div');
                controls.className = 'hotkey-editor-controls';
                controls.appendChild(btn);

                const resetBtn = document.createElement('button');
                resetBtn.type = 'button';
                resetBtn.className = 'hotkey-reset-btn';
                resetBtn.dataset.tooltip = I18n.t(`Сбросить «${meta.label}» до умолчания`);
                resetBtn.setAttribute('aria-label', I18n.t(`Сбросить «${meta.label}» до умолчания`));
                resetBtn.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/></svg>';
                resetBtn.onclick = () => this._resetAction(action);
                controls.appendChild(resetBtn);

                row.appendChild(labelEl);
                row.appendChild(controls);
                container.appendChild(row);
            });
        });
    },

    _setupSearch() {
        const input = $('hotkeysSearch');
        if (!input) return;
        input.oninput = () => this._renderTable(input.value);
    },

    _startListening(action, btn) {
        if (this._listeningEl) {
            this._listeningEl.classList.remove('listening');
            this._listeningEl.textContent = codeToLabel(AppConfig.hotkeys[this._listeningAction]);
        }
        this._listeningAction = action;
        this._listeningEl = btn;
        btn.classList.add('listening');
        btn.textContent = '…';
    },

    _stopListening() {
        if (this._listeningEl) {
            this._listeningEl.classList.remove('listening');
            this._listeningEl.textContent = codeToLabel(AppConfig.hotkeys[this._listeningAction]);
            this._listeningEl = null;
        }
        this._listeningAction = null;
    },

    _resetAction(action) {
        this._stopListening();
        AppConfig.setHotkey(action, DEFAULT_HOTKEYS[action]);
        this._renderTable($('hotkeysSearch')?.value);
        TokenEditor.updateToolHotkeys();
        toast(`«${HOTKEYS_META[action].label}» сброшено до умолчания`);
    },

    _onKey(e) {
        if (!this._listeningAction) return;
        if (e.code === 'Escape' && !e.ctrlKey && !e.altKey && !e.shiftKey) { this._stopListening(); return; }
        if (['Control','Shift','Alt','Meta'].includes(e.key)) return;
        e.preventDefault(); e.stopPropagation();
        const hk = serializeHotkey(e.code, { ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey });
        AppConfig.setHotkey(this._listeningAction, hk);
        this._listeningEl.textContent = codeToLabel(hk);
        this._stopListening();
        TokenEditor.updateToolHotkeys();
        toast('Клавиша назначена: ' + codeToLabel(hk));
    },

    _setupResetAll() {
        const btn = $('hotkeysResetAllBtn');
        if (!btn) return;
        btn.onclick = () => {
            this._stopListening();
            AppConfig.resetHotkeys();
            this._renderTable($('hotkeysSearch')?.value);
            TokenEditor.updateToolHotkeys();
            toast('Все горячие клавиши сброшены до умолчания');
        };
    }
};
