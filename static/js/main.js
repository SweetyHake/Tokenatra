function initTabs() {
    const panels = document.querySelectorAll('.panel');
    const tabs = document.querySelectorAll('.nav-btn[data-mode]');
    tabs.forEach(tab => tab.setAttribute('role', 'tab'));
    panels.forEach(panel => {
        const active = panel.classList.contains('active');
        panel.setAttribute('aria-hidden', String(!active));
        panel.inert = !active;
    });
    document.querySelectorAll('.nav-btn[data-mode]').forEach(tab => {
        tab.onclick = () => {
            const mode = tab.dataset.mode;
            document.querySelectorAll('.nav-btn[data-mode]').forEach(t => t.classList.remove('active'));
            tab.classList.add('active');
            document.querySelectorAll('.nav-btn[data-mode]').forEach(t => t.setAttribute('aria-selected', String(t === tab)));
            document.querySelectorAll('.panel').forEach(p => {
                const active = p.id === mode + 'Panel';
                p.classList.toggle('active', active);
                p.setAttribute('aria-hidden', String(!active));
                p.inert = !active;
            });
            const panel = $(mode + 'Panel');
            if (panel) panel.focus?.({ preventScroll: true });
        };
    });
}

function getActiveFileToolMode() {
    return document.querySelector('.file-mode-btn.active')?.dataset.fileMode || 'remover';
}

function initFileToolModes() {
    document.querySelectorAll('.file-mode-btn[data-file-mode]').forEach(btn => {
        btn.onclick = () => {
            const mode = btn.dataset.fileMode;
            document.querySelectorAll('.file-mode-btn[data-file-mode]').forEach(tab => {
                const active = tab === btn;
                tab.classList.toggle('active', active);
                tab.setAttribute('aria-selected', String(active));
            });
            document.querySelectorAll('.file-mode-panel').forEach(panel => {
                const active = panel.id === mode + 'ModePanel';
                panel.classList.toggle('active', active);
                panel.hidden = !active;
            });
        };
    });
}

function openModelSettings() {
    const settingsTab = document.querySelector('.nav-btn[data-mode="settings"]');
    if (settingsTab) settingsTab.click();
    const card = $('modelSettingsCard');
    if (!card) return;
    card.classList.remove('model-focus');
    requestAnimationFrame(() => {
        card.classList.add('model-focus');
        $('downloadModelBtn')?.focus();
        setTimeout(() => card.classList.remove('model-focus'), 1800);
    });
}

function updateDeviceIndicator(device, modelExists) {
    const deviceName = device || 'CPU';
    const isGPU = deviceName.includes('GPU') || deviceName.includes('DirectML') || deviceName.includes('CUDA') || deviceName.includes('ROCm');
    const statusClass = !modelExists ? 'model-missing' : (isGPU ? 'gpu' : 'cpu');
    const tooltip = !modelExists
        ? 'Модель для удаления фона не установлена'
        : (isGPU ? deviceName + ' (GPU)' : deviceName + ' (CPU)');
    const dot = $('deviceBadge');
    if (dot) {
        dot.className = 'device-dot ' + statusClass;
        dot.dataset.tooltip = tooltip;
    }
    const aboutBadge = document.querySelector('.about-device-badge');
    if (aboutBadge) aboutBadge.className = 'about-device-badge ' + statusClass;
    const aboutValue = $('aboutDeviceValue');
    if (aboutValue) aboutValue.textContent = !modelExists ? 'Модель не установлена' : deviceName;
    state.deviceName = deviceName;
    state.modelAvailable = !!modelExists;
}

function setModelAvailability(exists) {
    const available = !!exists;
    state.modelAvailable = available;
    const removeBgBtn = $('removeBgBtn');
    const removerTab = $('fileModeRemoverBtn');

    [removeBgBtn, removerTab].forEach((button) => {
        if (!button) return;
        button.classList.toggle('model-missing', !available);
        button.setAttribute('aria-disabled', String(!available));
        button.dataset.tooltip = available
            ? (button === removeBgBtn ? '' : 'Удаление фона')
            : 'Модель для удаления фона не установлена';
    });

    if (removeBgBtn && !removeBgBtn._modelGuarded) {
        removeBgBtn._modelGuarded = true;
        removeBgBtn._modelAction = removeBgBtn.onclick;
        removeBgBtn.onclick = function(event) {
            if (state.modelAvailable === false) {
                event.preventDefault();
                openModelSettings();
                return;
            }
            return removeBgBtn._modelAction?.call(removeBgBtn, event);
        };
    }
    if (removerTab && !removerTab._modelGuarded) {
        removerTab._modelGuarded = true;
        removerTab._modelAction = removerTab.onclick;
        removerTab.onclick = function(event) {
            if (state.modelAvailable === false) {
                event.preventDefault();
                openModelSettings();
                return;
            }
            return removerTab._modelAction?.call(removerTab, event);
        };
    }

    updateDeviceIndicator(state.deviceName || 'CPU', available);
}

function initZoomControls() {
    const zoomIn = $('zoomInBtn');
    const zoomOut = $('zoomOutBtn');
    const zoomReset = $('zoomResetBtn');
    const zoomLabel = $('zoomLabel');

    function updateLabel() {
        if (zoomLabel && state.viewZoom !== undefined) zoomLabel.textContent = Math.round(state.viewZoom * 100) + '%';
    }

    if (zoomIn) zoomIn.onclick = () => {
        if (!TokenCanvas) return;
        state.viewZoom = Math.min(state.viewZoom * 1.25, CONFIG.MAX_ZOOM || 20);
        TokenCanvas.updateViewTransform();
        updateLabel();
    };
    if (zoomOut) zoomOut.onclick = () => {
        if (!TokenCanvas) return;
        state.viewZoom = Math.max(state.viewZoom * 0.8, CONFIG.MIN_ZOOM || 0.5);
        TokenCanvas.updateViewTransform();
        updateLabel();
    };
    if (zoomReset) zoomReset.onclick = () => {
        if (!TokenCanvas) return;
        TokenCanvas.resetView();
        updateLabel();
    };

    document.addEventListener('keydown', e => {
        if ((e.ctrlKey || e.metaKey) && (e.code === 'Equal' || e.code === 'NumpadAdd')) {
            e.preventDefault(); zoomIn?.click();
        }
        if ((e.ctrlKey || e.metaKey) && (e.code === 'Minus' || e.code === 'NumpadSubtract')) {
            e.preventDefault(); zoomOut?.click();
        }
        if ((e.ctrlKey || e.metaKey) && e.code === 'Digit0') {
            e.preventDefault(); zoomReset?.click();
        }
    });
}

function initGlobalShortcuts() {
    document.addEventListener('keydown', e => {
        const code = e.code;
        const hk = AppConfig.hotkeys;
        const tag = e.target.tagName.toLowerCase();
        const isInput = tag === 'input' || tag === 'textarea' || tag === 'select';

        if (code === 'F11') {
            // В браузере оставляем нативный полноэкранный режим
            if (window.pywebview?.api?.toggle_fullscreen) {
                e.preventDefault();
                window.pywebview.api.toggle_fullscreen();
            }
            return;
        }

        if (hotkeyMatches(e, hk.openFile) && !isInput) {
            e.preventDefault();
            const tokenPanel = $('tokenPanel');
            const fileToolsPanel = $('fileToolsPanel');
            if (tokenPanel?.classList.contains('active')) {
                $('tokenFileInput')?.click();
            } else if (fileToolsPanel?.classList.contains('active')) {
                const inputId = getActiveFileToolMode() === 'converter' ? 'convFileInput' : 'fileInput';
                $(inputId)?.click();
            } else {
                $('fileInput')?.click();
            }
            return;
        }

        if (hotkeyMatches(e, hk.saveAll) && !isInput) {
            e.preventDefault();
            const fileToolsPanel = $('fileToolsPanel');
            if (fileToolsPanel?.classList.contains('active')) {
                if (getActiveFileToolMode() === 'converter') Converter.downloadAll();
                else Remover.downloadAll();
            }
            return;
        }

        const tokenPanel = $('tokenPanel');
        if (!tokenPanel?.classList.contains('active')) return;

        if (isInput) return;

        // Эксклюзивная активация: при совпадении комбинаций (возможно при
        // ручной правке config.json) срабатывает только первый инструмент,
        // а не оба сразу
        if (hotkeyMatches(e, hk.toolMove)) {
            const btn = document.querySelector('.tool-btn[data-tool="move"]');
            if (btn && !btn.classList.contains('active')) btn.click();
        } else if (hotkeyMatches(e, hk.toolEraser)) {
            const btn = document.querySelector('.tool-btn[data-tool="eraser"]');
            if (btn) btn.click();
        } else if (hotkeyMatches(e, hk.toolMask)) {
            const btn = document.querySelector('.tool-btn[data-tool="mask"]');
            if (btn) btn.click();
        } else if (hotkeyMatches(e, hk.toolRemoveBg)) {
            const btn = $('removeBgBtn');
            if (btn) btn.click();
        }
    });
}

function initPasteHandler() {
    document.addEventListener('paste', e => {
        const tag = e.target.tagName.toLowerCase();
        if (tag === 'input' || tag === 'textarea') return;

        e.preventDefault();
        const items = e.clipboardData?.items;
        if (!items) return;

        const files = [];
        for (let i = 0; i < items.length; i++) {
            const item = items[i];
            if (item.type.startsWith('image/')) {
                const blob = item.getAsFile();
                if (blob) files.push(new File([blob], `paste_${Date.now()}.png`, { type: item.type }));
            }
        }

        if (files.length === 0) return;

        if ($('tokenPanel')?.classList.contains('active')) {
            TokenCanvas.loadImage(files[0]);
            toast('Изображение вставлено');
        } else if ($('fileToolsPanel')?.classList.contains('active')) {
            if (getActiveFileToolMode() === 'converter') {
                Converter.handleFiles(files);
                toast('Вставлено в конвертер');
            } else {
                Remover.handleFiles(files);
                toast('Вставлено в вырезатель');
            }
        } else {
            Remover.handleFiles(files);
            toast('Вставлено из буфера');
        }
    });
}

function initSliderWheels() {
    // Портретные слайдеры не включаем: PortraitGenerator вешает свои wheel-обработчики
    const configs = [
        { slider: 'scaleSlider',            input: 'scaleInput',            valEl: null },
        { slider: 'rotationSlider',          input: 'rotationInput',         valEl: null },
        { slider: 'eraserSize',              input: 'eraserSizeInput',       valEl: null },
    ];

    configs.forEach(({ slider, input, valEl }) => {
        const sl = $(slider);
        if (!sl) return;
        sl.addEventListener('wheel', e => {
            e.preventDefault();
            const step = parseFloat(sl.step) || 1;
            sl.value = clamp(parseFloat(sl.value) + (e.deltaY < 0 ? step : -step), parseFloat(sl.min), parseFloat(sl.max));
            sl.dispatchEvent(new Event('input', { bubbles: true }));
        }, { passive: false });
    });
}

function initTheme() {
    // Single theme (Hero in a Box dark parchment) — tokens baked into :root,
    // legacy per-theme classes removed. Kept as no-op for compat.
}

function initDefaultSettings() {
    var languageSelect = $('languageSelect');
    if (languageSelect) {
        languageSelect.value = AppConfig.language || 'system';
        languageSelect.onchange = function() {
            AppConfig.setLanguage(this.value);
            I18n.setLanguage(this.value);
        };
    }

    var defQ = $('defQualitySelect');
    if (defQ) {
        defQ.value = AppConfig.saveSettings.quality || 512;
        defQ.onchange = function() {
            AppConfig.setSaveSetting('quality', parseInt(this.value));
            state.saveQuality = parseInt(this.value);
            const tokenQ = $('saveQualitySelect');
            if (tokenQ) tokenQ.value = this.value;
        };
    }
    var defFmt = $('defRemoverFormat');
    if (defFmt) {
        defFmt.value = AppConfig.remover.format || 'webp';
        defFmt.onchange = function() {
            AppConfig.setRemover('format', this.value);
        };
    }
    var tokenFmt = $('tokenFormatSelect');
    if (tokenFmt) {
        tokenFmt.value = (AppConfig.saveSettings && AppConfig.saveSettings.format) || 'webp';
        tokenFmt.onchange = function() {
            AppConfig.setSaveSetting('format', this.value);
            toast('Формат токена: ' + this.value.toUpperCase());
        };
    }
    var tokenComp = $('tokenCompressionSlider');
    var tokenCompValue = $('tokenCompressionValue');
    if (tokenComp) {
        tokenComp.value = (AppConfig.saveSettings && AppConfig.saveSettings.compression) || 95;
        if (tokenCompValue) tokenCompValue.textContent = tokenComp.value + '%';
        tokenComp.oninput = function() {
            if (tokenCompValue) tokenCompValue.textContent = this.value + '%';
        };
        tokenComp.onchange = function() {
            AppConfig.setSaveSetting('compression', parseInt(this.value));
        };
    }
    var histLimit = $('historyLimitSelect');
    if (histLimit) {
        histLimit.value = String(AppConfig.historyLimit || 30);
        histLimit.onchange = function() {
            AppConfig.setHistoryLimit(parseInt(this.value));
            CONFIG.MAX_HISTORY = parseInt(this.value);
        };
    }
    var canvasScaleSelect = $('canvasScaleSelect');
    if (canvasScaleSelect) {
        canvasScaleSelect.value = String(AppConfig.canvasScale || 2);
        canvasScaleSelect.onchange = function() {
            TokenCanvas.setCanvasScale(parseInt(this.value));
            this.value = String(state.canvasScale);
        };
    }
}

function initResizeHandles() {
    var leftPanel = document.querySelector('.context-panel-left');
    var leftHandle = $('leftPanelHandle');
    var savedLeft = AppConfig.panelWidths.left || 320;
    if (leftPanel) leftPanel.style.width = savedLeft + 'px';

    var rightPanel = document.querySelector('.context-panel-right');
    var rightHandle = $('rightPanelHandle');
    var savedRight = AppConfig.panelWidths.right || 320;
    if (rightPanel) rightPanel.style.width = savedRight + 'px';

    function makeResize(handle, panel, side) {
        if (!handle || !panel) return;
        var startX, startW;
        function onStart(e) {
            startX = e.clientX;
            startW = panel.offsetWidth;
            handle.classList.add('active');
            document.body.style.cursor = 'col-resize';
            document.body.style.userSelect = 'none';
            document.addEventListener('mousemove', onMove);
            document.addEventListener('mouseup', onEnd);
        }
        function onMove(e) {
            var dx = side === 'left' ? (e.clientX - startX) : (startX - e.clientX);
            var newW = Math.max(180, Math.min(480, startW + dx));
            panel.style.width = newW + 'px';
        }
        function onEnd() {
            handle.classList.remove('active');
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            document.removeEventListener('mousemove', onMove);
            document.removeEventListener('mouseup', onEnd);
            AppConfig.setPanelWidth(side, panel.offsetWidth);
            // Перепозиционировать канвас/оверлей после изменения размера рабочей области
            if (typeof TokenCanvas !== 'undefined' && TokenCanvas._fixCanvasDisplay) {
                TokenCanvas._fixCanvasDisplay();
                TokenCanvas.requestRender();
            }
        }
        handle.addEventListener('mousedown', onStart);
    }

    makeResize(leftHandle, leftPanel, 'left');
    makeResize(rightHandle, rightPanel, 'right');
}

function initWindowControls() {
    const flask = (action) => fetch('/api/window/' + action, { method: 'POST' }).catch(() => {});
    // Мост pywebview появляется асинхронно — читаем на каждый клик, не кэшируем
    const getApi = () => window.pywebview?.api;

    const doMinimize = () => {
        if (getApi()?.minimize) getApi().minimize();
        else flask('minimize');
    };
    // GTK/mutter игнорирует iconify у maximized-окна — сначала снимаем разворот
    $('wcMinimize')?.addEventListener('click', () => {
        if (document.documentElement.classList.contains('is-maximized')) {
            if (getApi()?.restore) getApi().restore();
            else flask('restore');
            setTimeout(doMinimize, 150);
        } else {
            doMinimize();
        }
    });
    $('wcClose')?.addEventListener('click', () => {
        if (getApi()?.destroy) getApi().destroy();
        else flask('close');
    });

    const maxBtn = $('wcMaximize');
    if (maxBtn) {
        const setIcon = () => {
            const isMax = document.documentElement.classList.contains('is-maximized');
            maxBtn.innerHTML = isMax
                ? '<svg viewBox="0 0 24 24"><rect x="7" y="7" width="14" height="14" rx="1"/><rect x="3" y="3" width="14" height="14" rx="1"/></svg>'
                : '<svg viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="14" rx="1"/></svg>';
        };
        maxBtn.addEventListener('click', () => {
            if (document.documentElement.classList.contains('is-maximized')) {
                document.documentElement.classList.remove('is-maximized');
                if (getApi()?.restore) getApi().restore();
                else flask('restore');
            } else {
                document.documentElement.classList.add('is-maximized');
                if (getApi()?.maximize) getApi().maximize();
                else flask('maximize');
            }
            setIcon();
        });
    }

    $('titleBar')?.addEventListener('dblclick', e => {
        if (e.target.closest('.tb-nav, .window-controls')) return;
        maxBtn?.click();
    });

    // Перетаскивание окна: Windows — через Win32 (flask('move')),
    // Linux/macOS — класс pywebview-drag-region на тайтлбаре.
    if (IS_WINDOWS) {
        $('titleBar')?.addEventListener('mousedown', e => {
            if (e.target.closest('.tb-nav, .window-controls')) return;
            flask('move');
        });
    }
}

// Linux/macOS frameless: растягивание за края через GTK begin_resize_drag
function initFramelessResize() {
    if (IS_WINDOWS) return;
    const band = 6;
    const cursors = { n: 'ns-resize', s: 'ns-resize', e: 'ew-resize', w: 'ew-resize',
        ne: 'nesw-resize', sw: 'nesw-resize', nw: 'nwse-resize', se: 'nwse-resize' };
    let edge = '';
    document.addEventListener('mousemove', e => {
        if (document.documentElement.classList.contains('is-maximized')) {
            if (edge) { edge = ''; document.body.style.cursor = ''; }
            return;
        }
        const n = e.clientY <= band, s = e.clientY >= innerHeight - band;
        const w = e.clientX <= band, ee = e.clientX >= innerWidth - band;
        edge = (n && w) ? 'nw' : (n && ee) ? 'ne' : (s && w) ? 'sw' : (s && ee) ? 'se'
            : n ? 'n' : s ? 's' : w ? 'w' : ee ? 'e' : '';
        document.body.style.cursor = edge ? cursors[edge] : '';
    });
    document.addEventListener('mousedown', e => {
        if (!edge || e.button !== 0) return;
        if (e.target.closest('.tb-nav, .window-controls, button, input, select')) return;
        e.preventDefault();
        fetch('/version?dbg=edge:' + edge + ':' + (!!getApi()));
        const api = getApi();
        if (api?.resizeEdge) api.resizeEdge(edge);
    });
}

// Страховка от залипшего drag pywebview: если mouseup произошёл вне страницы
// (нативный попап, край экрана) — слушатели mousemove остаются навсегда и окно
// «дрожит», следуя за мышью. Разжимаем по первому движению без кнопок или blur.
(function initDragGuard() {
    let lmb = false;
    const release = () => {
        if (!lmb) return;
        lmb = false;
        document.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
    };
    document.addEventListener('mousedown', e => { if (e.button === 0) lmb = true; }, true);
    document.addEventListener('mouseup', () => { lmb = false; }, true);
    document.addEventListener('mousemove', e => {
        if (lmb && e.buttons === 0) release();
    }, true);
    window.addEventListener('blur', release);
})();

function initEdgeResize() {
    // WebView2 consumes the border hit-test, so resize the native parent window
    // while the pointer is held on the outer client-area edge.
    // Windows-only: на Linux/macOS ресайз frameless-окна делает pywebview.
    if (!IS_WINDOWS) return;
    const band = 6;
    const bodyStyle = document.body.style;
    const getEdges = (x, y) => ({
        left: x < band,
        right: x >= window.innerWidth - band,
        top: y < band,
        bottom: y >= window.innerHeight - band,
    });
    const hasEdge = edges => edges.left || edges.right || edges.top || edges.bottom;
    const getCursor = edges => {
        if ((edges.top && edges.left) || (edges.bottom && edges.right)) return 'nwse-resize';
        if ((edges.top && edges.right) || (edges.bottom && edges.left)) return 'nesw-resize';
        if (edges.left || edges.right) return 'ew-resize';
        return 'ns-resize';
    };

    document.addEventListener('mousemove', event => {
        const edges = getEdges(event.clientX, event.clientY);
        bodyStyle.cursor = hasEdge(edges) ? getCursor(edges) : '';
    }, { passive: true });

    document.addEventListener('mousedown', event => {
        if (event.button !== 0) return;
        const edges = getEdges(event.clientX, event.clientY);
        if (!hasEdge(edges)) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        bodyStyle.cursor = getCursor(edges);
        fetch('/api/window/resize_start', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(edges)
        }).catch(() => {});
    }, true);

    window.addEventListener('mouseup', () => { bodyStyle.cursor = ''; });
}

function initModelSelector() {
    const sel = $('modelSelect');
    const dirLabel = $('modelsDirLabel');
    if (!sel) return;
    const status = $('modelStatus');
    const statusText = $('modelStatusText');
    const sizeLabel = $('modelSizeLabel');
    const serverBtn = $('downloadModelBtn');
    const diskBtn = $('loadModelDiskBtn');

    function setStatus(ready, size, modelsDir) {
        if (status) status.className = 'model-status ' + (ready ? 'ready' : 'missing');
        if (statusText) statusText.textContent = I18n.t(ready ? 'Модель готова к работе' : 'Модель не установлена');
        if (sizeLabel) sizeLabel.textContent = ready && size ? formatSize(size) : '';
    }

    function setBusy(button, busy, busyText) {
        if (!button) return;
        if (busy) {
            button.dataset.defaultHtml = button.innerHTML;
            button.disabled = true;
            button.textContent = busyText;
        } else {
            button.disabled = false;
            button.innerHTML = button.dataset.defaultHtml || button.innerHTML;
        }
    }

    fetch('/models_list').then(function(r) { return r.json(); }).then(function(d) {
        window._tokenatraModels = d;
        if (dirLabel && d.models_dir) dirLabel.textContent = d.models_dir;
        setStatus(!!d.selected, d.models?.find(m => m.selected)?.size, d.models_dir);
        setModelAvailability(!!d.selected);
        sel.innerHTML = '';
        if (!d.models || d.models.length === 0) {
            const opt = document.createElement('option');
            opt.value = '';
            opt.textContent = I18n.t('Нет моделей (.onnx)');
            sel.appendChild(opt);
            sel.disabled = true;
            return;
        }
        sel.disabled = false;
        d.models.forEach(function(m) {
            const opt = document.createElement('option');
            opt.value = m.name;
            opt.textContent = m.selected ? m.name + ' (' + I18n.t('текущая') + ')' : m.name;
            if (m.selected) opt.selected = true;
            sel.appendChild(opt);
        });
    }).catch(function() {});
    sel.onchange = function() {
        if (!sel.value) return;
        fetch('/select_model', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: sel.value })
        }).then(function(r) { return r.json(); }).then(function(d) {
            if (d && d.ok) {
                toast('Модель «' + d.name + '» — будет применена при следующей обработке');
            } else {
                toast('Ошибка: ' + (d.error || 'не удалось выбрать модель'), true);
            }
            initModelSelector();
        }).catch(function() {
            toast('Ошибка выбора модели', true);
        });
    };

    async function runModelAction(button, endpoint, busyText) {
        setBusy(button, true, busyText);
        if (status) status.className = 'model-status loading';
        if (statusText) statusText.textContent = I18n.t(endpoint.includes('download') ? 'Загрузка модели с сервера…' : 'Копирование модели…');
        try {
            const response = await fetch(endpoint, { method: 'POST' });
            const result = await response.json();
            if (result.cancelled) {
                initModelSelector();
                return;
            }
            if (!response.ok || !result.ok) throw new Error(result.error || 'операция не выполнена');
            toast('Модель «' + result.name + '» загружена и выбрана');
            initModelSelector();
        } catch (error) {
            toast('Ошибка: ' + error.message, true);
            initModelSelector();
        } finally {
            setBusy(button, false);
        }
    }

    if (serverBtn && !serverBtn.dataset.bound) {
        serverBtn.dataset.bound = '1';
        serverBtn.onclick = () => runModelAction(serverBtn, '/download_model_from_server', 'Загрузка…');
    }
    if (diskBtn && !diskBtn.dataset.bound) {
        diskBtn.dataset.bound = '1';
        diskBtn.onclick = () => runModelAction(diskBtn, '/load_model_from_disk', 'Выбор файла…');
    }
}

let aboutUpdating = false;
let aboutDownloading = false;

function openAboutTab() {
    document.querySelectorAll('.nav-btn[data-mode]').forEach(function(t) { t.classList.remove('active'); });
    const btn = document.querySelector('.nav-btn[data-mode="about"]');
    if (btn) {
        btn.classList.add('active');
        btn.setAttribute('aria-selected', 'true');
    }
    // Как в initTabs(): классы + aria-hidden + inert, иначе панель видима,
    // но неинтерактивна (кнопки «Проверить обновления»/«Скачать» мертвы)
    document.querySelectorAll('.panel').forEach(function(p) {
        const active = p.id === 'aboutPanel';
        p.classList.toggle('active', active);
        p.setAttribute('aria-hidden', String(!active));
        p.inert = !active;
    });
    const panel = $('aboutPanel');
    if (panel) panel.focus?.({ preventScroll: true });
}

function initUpdateNotify() {
    const banner = $('updateNotifyBanner');
    const badge = $('tbUpdateBadge');
    if (!banner) return;
    const open = $('updateNotifyOpen');
    const close = $('updateNotifyClose');
    banner.addEventListener('click', function(e) {
        if (e.target === close) return;
        openAboutTab();
    });
    if (open) open.addEventListener('click', function(e) { e.stopPropagation(); openAboutTab(); });
    if (close) close.addEventListener('click', function(e) { e.stopPropagation(); banner.hidden = true; });

    // Фоновая проверка обновлений уже запущена сервером при старте — опрашиваем её
    let tries = 0;
    function poll() {
        fetch('/update_status').then(function(r) { return r.json(); }).then(function(d) {
            if (d && d.update_available && !d.download_done && d.update_tag) {
                const v = $('updateNotifyVersion');
                if (v) v.textContent = 'v' + d.update_tag;
                banner.hidden = false;
                if (badge) {
                    badge.hidden = false;
                    badge.dataset.tooltip = 'Доступно обновление v' + d.update_tag;
                }
                toast('Доступно обновление v' + d.update_tag);
                return;
            }
            if (tries++ < 20) setTimeout(poll, 2000);
        }).catch(function() {
            if (tries++ < 20) setTimeout(poll, 2000);
        });
    }
    setTimeout(poll, 3000);
}

function initAboutUpdate() {
    fetch('/version').then(r => r.json()).then(d => {
        const v = $('aboutVersion');
        if (v) v.textContent = 'Версия ' + (d.version || '?');
        const tv = $('titleVersion');
        if (tv) tv.textContent = d.version || '';
    }).catch(() => {});
    fetch('/device').then(r => r.json()).then(d => {
        const el = $('aboutDeviceValue');
        if (el) el.textContent = (d.device || 'CPU');
        if (typeof d.model_exists === 'boolean') updateDeviceIndicator(d.device, d.model_exists);
    }).catch(() => {});

    const btn = $('checkUpdatesBtn');
    if (btn) btn.onclick = () => checkForUpdatesManual();

    const badge = $('tbUpdateBadge');
    if (badge) badge.addEventListener('click', () => openAboutTab());
}

function aboutUpdateSetStatus(html) {
    const st = $('aboutUpdateStatus');
    if (st) st.innerHTML = html;
}

function aboutUpdateShowProgress(pct) {
    const box = $('aboutUpdateBox');
    if (!box) return;
    box.hidden = false;
    const prog = $('aboutUpdateProgress');
    const bar = $('aboutUpdateBarFill');
    const pctEl = $('aboutUpdatePct');
    const actions = $('aboutUpdateActions');
    if (prog) prog.hidden = false;
    if (bar) { bar.classList.remove('indeterminate'); bar.style.width = pct + '%'; }
    if (pctEl) pctEl.textContent = pct + '%';
    if (actions) actions.innerHTML = '';
    aboutUpdateSetStatus(I18n.t('Скачивание обновления…'));
}

function aboutUpdateShowIndeterminate() {
    const box = $('aboutUpdateBox');
    if (!box) return;
    box.hidden = false;
    const prog = $('aboutUpdateProgress');
    const bar = $('aboutUpdateBarFill');
    const pctEl = $('aboutUpdatePct');
    const actions = $('aboutUpdateActions');
    if (prog) prog.hidden = false;
    if (bar) { bar.classList.add('indeterminate'); bar.style.width = '40%'; }
    if (pctEl) pctEl.textContent = '…';
    if (actions) actions.innerHTML = '';
    aboutUpdateSetStatus(I18n.t('Скачивание обновления…'));
}

function hideAboutUpdate() {
    const box = $('aboutUpdateBox');
    if (box) box.hidden = true;
    const prog = $('aboutUpdateProgress');
    if (prog) prog.hidden = true;
    const bar = $('aboutUpdateBarFill');
    if (bar) { bar.classList.remove('indeterminate'); bar.style.width = '0%'; }
    const actions = $('aboutUpdateActions');
    if (actions) actions.innerHTML = '';
    const btn = $('checkUpdatesBtn');
    if (btn) btn.disabled = false;
    aboutUpdating = false;
    aboutDownloading = false;
}

function checkForUpdatesManual() {
    if (aboutUpdating) return;
    aboutUpdating = true;
    const box = $('aboutUpdateBox');
    const btn = $('checkUpdatesBtn');
    const actions = $('aboutUpdateActions');
    if (box) box.hidden = false;
    if (actions) actions.innerHTML = '';
    if (btn) btn.disabled = true;
    aboutUpdateSetStatus('Проверка обновлений…');
    fetch('/check_update', { method: 'POST' }).catch(() => {});
    pollAboutUpdate(40);
}

function pollAboutUpdate(retries) {
    fetch('/update_status').then(r => r.json()).then(d => {
        const actions = $('aboutUpdateActions');
        if (d.download_error) {
            // Строка из исключений updater — экранируем перед вставкой в HTML
            aboutUpdateSetStatus('<span style="color:#ef4444">Ошибка скачивания: ' + escapeHtml(d.download_error) + '</span>');
            if (actions) actions.innerHTML = '<button class="about-link" onclick="hideAboutUpdate()">Закрыть</button>';
            aboutUpdating = false;
            aboutDownloading = false;
            if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = false;
            return;
        }
        if (d.download_done) {
            aboutUpdateShowProgress(100);
            aboutUpdateSetStatus('Обновление <b>' + escapeHtml(d.update_tag || '') + '</b> скачано. Установить сейчас?');
            if (actions) actions.innerHTML =
                '<button class="accent-btn accent-btn-compact" onclick="applyAboutUpdate()">Установить</button>' +
                '<button class="about-link" onclick="hideAboutUpdate()">Позже</button>';
            aboutUpdating = false;
            aboutDownloading = false;
            if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = false;
            return;
        }
        if (d.download_active) {
            if (d.download_progress >= 0) {
                aboutUpdateShowProgress(d.download_progress);
            } else {
                aboutUpdateShowIndeterminate();
            }
            setTimeout(() => pollAboutUpdate(retries), 1000);
            return;
        }
        if (d.update_checked && d.check_error) {
            aboutUpdateSetStatus('<span style="color:#ef4444">Не удалось проверить обновления (нет соединения с GitHub)</span>');
            if (actions) actions.innerHTML = '<button class="about-link" onclick="hideAboutUpdate()">Закрыть</button>';
            aboutUpdating = false;
            aboutDownloading = false;
            if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = false;
            return;
        }
        if (d.update_available && d.update_url) {
            aboutUpdateSetStatus('Доступна версия <b>' + escapeHtml(d.update_tag || '') + '</b> (текущая: ' + escapeHtml(d.current_version || '') + ')');
            if (actions) actions.innerHTML =
                '<button class="accent-btn accent-btn-compact" onclick="startAboutDownload()">Скачать</button>' +
                '<button class="about-link" onclick="hideAboutUpdate()">Закрыть</button>';
            aboutUpdating = false;
            if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = false;
            // Если загрузка уже стартовала, но флаг ещё не доехал — продолжаем опрашивать
            if (aboutDownloading) setTimeout(() => pollAboutUpdate(retries), 1000);
            return;
        }
        if (d.update_checked) {
            hideAboutUpdate();
            toast('Обновлений нет — у вас последняя версия');
            return;
        }
        if (retries > 0) setTimeout(() => pollAboutUpdate(retries - 1), 500);
        else {
            aboutUpdateSetStatus('Не удалось проверить обновления');
            if (actions) actions.innerHTML = '<button class="about-link" onclick="hideAboutUpdate()">Закрыть</button>';
            aboutUpdating = false;
            if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = false;
        }
    }).catch(() => {
        if (retries > 0) setTimeout(() => pollAboutUpdate(retries - 1), 500);
        else {
            aboutUpdateSetStatus('Не удалось проверить обновления');
            const actions = $('aboutUpdateActions');
            if (actions) actions.innerHTML = '<button class="about-link" onclick="hideAboutUpdate()">Закрыть</button>';
            aboutUpdating = false;
            if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = false;
        }
    });
}

function startAboutDownload() {
    if (aboutDownloading) {
        pollAboutUpdate(600);
        return;
    }
    aboutDownloading = true;
    aboutUpdating = true;
    if ($('checkUpdatesBtn')) $('checkUpdatesBtn').disabled = true;
    const actions = $('aboutUpdateActions');
    if (actions) actions.innerHTML = '';
    aboutUpdateShowProgress(0);
    fetch('/start_update_download', { method: 'POST' }).catch(() => {});
    pollAboutUpdate(600);
}

function applyAboutUpdate() {
    aboutUpdateSetStatus('Установка обновления…');
    const actions = $('aboutUpdateActions');
    if (actions) actions.innerHTML = '';
    fetch('/apply_update', { method: 'POST' })
        .then(r => r.json())
        .then(d => {
            if (!d || !d.ok) {
                aboutUpdateSetStatus((d && d.error) ? d.error : 'Не удалось установить обновление');
                return;
            }
            aboutUpdateSetStatus('Установка обновления… Приложение закроется');
            fetch('/api/window/destroy', { method: 'POST' }).catch(() => {});
        })
        .catch(() => aboutUpdateSetStatus('Не удалось связаться с сервером'));
}

async function init() {
    await AppConfig.load();
    I18n.init();
    applyHotkeyHints();
    document.addEventListener('languagechange', applyHotkeyHints);
    if (!IS_WINDOWS) {
        // Функции, доступные только в Windows (контекстное меню ОС)
        document.querySelectorAll('[data-windows-only]').forEach(el => { el.style.display = 'none'; });
    }
    state.canvasScale = AppConfig.canvasScale || 2;
    state.dropShadowEnabled = !!AppConfig.dropShadowEnabled;
    state.colorCorrectionEnabled = !!AppConfig.colorCorrectionEnabled;
    state.protectionEnabled = !!AppConfig.protectionEnabled;
    state.quickSaveEnabled = !!AppConfig.quickSaveEnabled;
    state.saveQuality = parseInt(AppConfig.saveSettings.quality) || 512;
    state.saveScaleMode = AppConfig.saveSettings.scaleMode || 'auto';
    CONFIG.MAX_HISTORY = AppConfig.historyLimit || 30;
    initTheme();
    initDefaultSettings();
    initResizeHandles();
initTabs();
    initFileToolModes();
    initWindowControls();
    initEdgeResize();
    initFramelessResize();
    initZoomControls();
    initGlobalShortcuts();
    initPasteHandler();
    initSliderWheels();
    initTooltips();
    initUpdateNotify();
    initAboutUpdate();
    Remover.init();
    Converter.init();
    TokenEditor.init();
    initModelSelector();
    HotkeySettings.init();
    window.addEventListener('beforeunload', () => { AppConfig.flush(); urlManager.revokeAll(); });
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
} else {
    init();
}
