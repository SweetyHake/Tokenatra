const TokenEditor = {
    pressedKeys: new Set(),
    moveInterval: null,
    moveTimeout: null,
    initialMoveDone: false,
    rotateInterval: null,
    rotateTimeout: null,
    initialRotateDone: false,

    init() {
        TokenCanvas.init();
        TokenPresets.loadProtectionMask();
        TokenPresets.loadPresets();
        TokenPresets.setupRingModal();
        TokenPresets.loadRings();

        this.setupDropzone();
        this.setupToolButtons();
        this.setupSliders();
        this.setupCheckboxes();
        this.setupSaveButtons();
        this.setupKeyboardControls();
        this.setupPortraitVisibility();
        this.setupAccordions();
        this.setupRightPanelTabs();
        this.updateToolHotkeys();
    },

    setupRightPanelTabs() {
        document.querySelectorAll('.rtab-btn[data-rtab]').forEach(btn => {
            btn.onclick = () => {
                document.querySelectorAll('.rtab-btn[data-rtab]').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                document.querySelectorAll('.rtab-content[data-rtab-content]').forEach(c => {
                    c.classList.remove('active');
                    c.style.display = 'none';
                });
                const content = document.querySelector('[data-rtab-content="' + btn.dataset.rtab + '"]');
                if (content) { content.classList.add('active'); content.style.display = 'flex'; }
            };
        });
    },

    updateToolHotkeys() {
        const hk = AppConfig.hotkeys;
        const map = {
            '[data-tool="move"] kbd':   codeToLabel(hk.toolMove),
            '#removeBgBtn kbd':         codeToLabel(hk.toolRemoveBg),
            '[data-tool="eraser"] kbd': codeToLabel(hk.toolEraser),
            '[data-tool="mask"] kbd':   codeToLabel(hk.toolMask),
        };
        Object.entries(map).forEach(([sel, label]) => {
            document.querySelectorAll(sel).forEach(el => { if (label) el.textContent = label; });
        });
        const openBtn = $('fileOpenBtn');
        if (openBtn) openBtn.dataset.tooltip = 'Открыть файл (' + codeToLabel(hk.openFile) + ')';
        const overlayKbd = document.querySelector('.overlay-kbd');
        if (overlayKbd) overlayKbd.textContent = codeToLabel(hk.openFile);
        const undoBtn = $('undoBtn');
        if (undoBtn) undoBtn.dataset.tooltip = 'Отменить (' + codeToLabel(hk.undo) + ')';
        const redoBtn = $('redoBtn');
        if (redoBtn) redoBtn.dataset.tooltip = 'Повторить (' + codeToLabel(hk.redo) + ')';
    },

    setupDropzone() {
        const tokenDropzone = $('tokenDropzone');
        const tokenFileInput = $('tokenFileInput');
        if (tokenDropzone && tokenFileInput) {
            tokenDropzone.onclick = e => {
                e.stopPropagation();
                this.pickImageViaServer();
            };
            tokenDropzone.ondragover = e => { e.preventDefault(); tokenDropzone.style.borderColor = 'var(--accent)'; };
            tokenDropzone.ondragleave = () => { tokenDropzone.style.borderColor = ''; };
            tokenDropzone.ondrop = e => {
                e.preventDefault();
                tokenDropzone.style.borderColor = '';
                const files = Array.from(e.dataTransfer.files).filter(f => f.type.startsWith('image/'));
                if (files.length > 0) TokenCanvas.loadImage(files[0]);
            };
        }
        if (tokenFileInput) {
            tokenFileInput.onchange = e => {
                if (e.target.files.length > 0) {
                    TokenCanvas.loadImage(e.target.files[0]);
                }
                tokenFileInput.value = '';
            };
        }
        this.setupNavButtons();
        const tokenFileNameInput = $('tokenFileName');
        if (tokenFileNameInput) {
            tokenFileNameInput.oninput = e => {
                state.tokenFileName = e.target.value || 'token';
                this.updateDropzoneLabel();
            };
        }
    },

    async pickImageViaServer() {
        try {
            const res = await fetch('/pick_image_to_open');
            const data = await res.json();
            if (data.cancelled) return;

            const fileRes = await fetch('/get_image_by_path?path=' + encodeURIComponent(data.path));
            if (!fileRes.ok) throw new Error('Не удалось загрузить файл');
            const blob = await fileRes.blob();
            const fileName = data.path.split(/[\\/]/).pop();
            const file = new File([blob], fileName, { type: data.mime });
            TokenCanvas.loadImage(file, data.path);
        } catch (err) {
            toast('Ошибка: ' + err.message, true);
        }
    },

    setupNavButtons() {
        const prevBtn = $('navPrev');
        const nextBtn = $('navNext');
        if (prevBtn) prevBtn.onclick = () => this.navigateTo(-1);
        if (nextBtn) nextBtn.onclick = () => this.navigateTo(1);
    },

    updateDropzoneLabel() {
        const label = $('dropzoneLabel');
        const dz = $('tokenDropzone');
        if (!label || !dz) return;
        if (state.userImage) {
            label.textContent = (state.tokenFileName || 'token') + '.webp';
            dz.classList.add('has-image');
        } else {
            label.textContent = 'Файл не выбран';
            dz.classList.remove('has-image');
        }
    },

    updateNavState() {
        this.updateDropzoneLabel();

        const el = $('navArrows');
        const infoEl = $('navInfo');
        if (!el || !infoEl) return;

        if (!state.currentFilePath) {
            el.style.display = 'none';
            return;
        }

        fetch('/list_images', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path: state.currentFilePath })
        })
        .then(r => r.json())
        .then(data => {
            if (data.error || !data.files || data.total <= 1) {
                el.style.display = 'none';
                state.imageFileList = [];
                state.imageFileIndex = -1;
                return;
            }
            state.imageFileList = data.files;
            state.imageFileIndex = data.currentIndex;
            el.style.display = 'flex';
            this.updateNavFileName();
        })
        .catch(() => {
            el.style.display = 'none';
        });
    },

    updateNavFileName() {
        const infoEl = $('navInfo');
        if (!infoEl) return;
        if (state.imageFileIndex >= 0 && state.imageFileIndex < state.imageFileList.length) {
            infoEl.textContent = (state.imageFileIndex + 1) + ' / ' + state.imageFileList.length;
        } else {
            infoEl.textContent = '';
        }
    },

    navigateTo(direction) {
        const newIndex = state.imageFileIndex + direction;
        if (newIndex < 0 || newIndex >= state.imageFileList.length) return;
        const newPath = state.imageFileList[newIndex];
        if (!newPath) return;
        state.imageFileIndex = newIndex;
        TokenCanvas.loadImageByPath(newPath);
    },

    setupToolButtons() {
        document.querySelectorAll('.tool-btn[data-tool]').forEach(btn => {
            btn.onclick = () => {
                const tool = btn.dataset.tool;
                document.querySelectorAll('.tool-btn[data-tool]').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.currentTool = tool;
                const tokenCanvas = $('tokenCanvas');
                if (tool === 'eraser' || tool === 'mask') {
                    state.currentEraserMode = tool === 'eraser' ? 'blue' : 'pink';
                    if (tokenCanvas) TokenCanvas.setToolCursorMode(true);
                    const eraserRow = $('eraserRow');
                    if (eraserRow) eraserRow.style.display = 'flex';
                    TokenCanvas.showEraserCursor();
                    $('removeBgBtn')?.classList.remove('active');
                } else {
                    if (tokenCanvas) TokenCanvas.setToolCursorMode(false);
                    const eraserRow = $('eraserRow');
                    if (eraserRow) eraserRow.style.display = 'none';
                    TokenCanvas.hideEraserCursor();
                }
            };
        });

        const removeBgBtn = $('removeBgBtn');
        if (removeBgBtn) removeBgBtn.onclick = () => this.handleRemoveBackground();

        const eraserSizeSlider = $('eraserSize');
        const eraserSizeInput = $('eraserSizeInput');
        function applyEraserSize(val) {
            val = clamp(parseInt(val) || 50, 1, 300);
            state.eraserSize = val;
            TokenCanvas.setEraserSize(val);
            if (eraserSizeSlider) { eraserSizeSlider.value = val; setSliderFill(eraserSizeSlider); }
            if (eraserSizeInput) eraserSizeInput.value = val;
            if (AppConfig.setEraserSize) AppConfig.setEraserSize(val);
        }
        if (eraserSizeSlider) {
            eraserSizeSlider.oninput = e => applyEraserSize(e.target.value);
            applyEraserSize(AppConfig.eraserSize);
        }
        if (eraserSizeInput) {
            eraserSizeInput.onchange = e => applyEraserSize(e.target.value);
            eraserSizeInput.oninput = e => applyEraserSize(e.target.value);
        }

        const resetEraserBtn = $('resetEraserBtn');
        if (resetEraserBtn) resetEraserBtn.onclick = () => {
            TokenCanvas.resetImageMask();
            TokenCanvas.resetMask();
        };
    },

    async handleRemoveBackground() {
        const btn = $('removeBgBtn');
        if (!btn) return;
        if (!state.userImageOriginal && !state.userImageWithoutBg) {
            toast('Сначала загрузите изображение', true); return;
        }
        if (state.backgroundRemoved && state.userImageWithoutBg) {
            state.showingOriginal = !state.showingOriginal;
            if (state.showingOriginal) {
                state.userImage = state.userImageOriginal;
                btn.classList.remove('active');
                toast('Показан оригинал');
            } else {
                state.userImage = state.userImageWithoutBg;
                btn.classList.add('active');
                toast('Показан без фона');
            }
            TokenCanvas._compositedImageDirty = true;
            TokenCanvas._ccDirty = true;
            TokenCanvas.render();
            if (typeof PortraitGenerator !== 'undefined' && PortraitGenerator.canvas) PortraitGenerator.render();
            return;
        }
        if (!state.userImageOriginal) return;

        toast('Удаление фона…');
        btn.disabled = true;

        try {
            const tempCanvas = document.createElement('canvas');
            tempCanvas.width = state.userImageOriginal.width;
            tempCanvas.height = state.userImageOriginal.height;
            const tCtx = tempCanvas.getContext('2d');
            tCtx.fillStyle = '#ffffff';
            tCtx.fillRect(0, 0, tempCanvas.width, tempCanvas.height);
            tCtx.drawImage(state.userImageOriginal, 0, 0);
            const blob = await new Promise(resolve => tempCanvas.toBlob(resolve, 'image/jpeg', 0.92));
            tempCanvas.getContext('2d').clearRect(0, 0, tempCanvas.width, tempCanvas.height);

            const fd = new FormData();
            fd.append('image', blob, 'image.jpg');
            fd.append('format', 'png');
            fd.append('edge_blur', AppConfig.edgeBlur !== undefined ? AppConfig.edgeBlur : 1);
            const res = await fetch('/process', { method: 'POST', body: fd });
            if (!res.ok) throw new Error('Ошибка обработки');
            const resultBlob = await res.blob();

            urlManager.revoke('userImage');
            state.userImageUrl = null;

            const url = urlManager.create(resultBlob, 'userImage');
            const newImage = new Image();

            newImage.onload = () => {
                state.userImageUrl = url;
                state.userImageWithoutBg = newImage;
                state.userImage = newImage;
                state.backgroundRemoved = true;
                state.showingOriginal = false;

                if (state.imageMaskCanvas) {
                    const ctx = state.imageMaskCanvas.getContext('2d');
                    ctx.clearRect(0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
                    ctx.fillStyle = 'white';
                    ctx.fillRect(0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
                    // Маска переписана напрямую — синхронизируем альфа-зеркало воркера
                    TokenCanvas._pushMaskToWorker(false);
                }

                TokenCanvas._compositedImageDirty = true;
                TokenCanvas._ccDirty = true;
                TokenCanvas._shadowDirty = true;
                TokenCanvas._zonesDirty = true;
                TokenCanvas._imageBrushCache = null;

                btn.disabled = false;
                btn.classList.add('active');
                this.updateRemoveBgButton();
                TokenHistory.save();
                TokenCanvas.render();
                if (typeof PortraitGenerator !== 'undefined' && PortraitGenerator.canvas) PortraitGenerator.render();
                toast('Фон удалён');
            };

            newImage.onerror = () => {
                urlManager.revoke('userImage');
                state.userImage = state.userImageOriginal;
                btn.disabled = false;
                btn.classList.remove('active');
                this.updateRemoveBgButton();
                toast('Не удалось загрузить результат', true);
            };

            newImage.src = url;
        } catch (err) {
            toast('Ошибка: ' + err.message, true);
            btn.disabled = false;
        }
    },

    updateRemoveBgButton() {
        const btn = $('removeBgBtn');
        if (!btn) return;
        if (state.backgroundRemoved && state.userImageWithoutBg) {
            btn.disabled = false;
            if (state.showingOriginal) { btn.classList.remove('active'); }
            else { btn.classList.add('active'); }
        } else {
            btn.disabled = false;
            btn.classList.remove('active');
        }
    },

    setupSliders() {
        const debouncedSave = debounce(() => TokenHistory.save(), CONFIG.DEBOUNCE_DELAY);
        const scaleSlider = $('scaleSlider');
        if (scaleSlider) {
            setSliderFill(scaleSlider);
            scaleSlider.oninput = e => {
                state.imageScale = parseInt(e.target.value) / 100;
                const input = $('scaleInput');
                if (input) input.value = e.target.value;
                setSliderFill(e.target);
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.scheduleEffects();
                TokenCanvas._forceFullRender();
                TokenCanvas.requestRender();
            };
            scaleSlider.onchange = debouncedSave;
        }
        const scaleInput = $('scaleInput');
        if (scaleInput) {
            scaleInput.onchange = e => {
                let val = clamp(parseInt(e.target.value) || CONFIG.DEFAULT_SCALE, CONFIG.MIN_SCALE, CONFIG.MAX_SCALE);
                state.imageScale = val / 100;
                const slider = $('scaleSlider');
                if (slider) { slider.value = val; setSliderFill(slider); }
                e.target.value = val;
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.scheduleEffects();
                TokenCanvas._forceFullRender();
                TokenCanvas.render();
                TokenHistory.save();
            };
        }
        const rotationSlider = $('rotationSlider');
        if (rotationSlider) {
            setSliderFill(rotationSlider);
            rotationSlider.oninput = e => {
                state.imageRotation = parseInt(e.target.value);
                const input = $('rotationInput');
                if (input) input.value = e.target.value;
                setSliderFill(e.target);
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.scheduleEffects();
                TokenCanvas._forceFullRender();
                TokenCanvas.requestRender();
            };
            rotationSlider.onchange = debouncedSave;
        }
        const rotationInput = $('rotationInput');
        if (rotationInput) {
            rotationInput.onchange = e => {
                let val = clamp(parseInt(e.target.value) || 0, -180, 180);
                state.imageRotation = val;
                const slider = $('rotationSlider');
                if (slider) { slider.value = val; setSliderFill(slider); }
                e.target.value = val;
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.scheduleEffects();
                TokenCanvas._forceFullRender();
                TokenCanvas.render();
                TokenHistory.save();
            };
        }
        const resetTransformBtn = $('resetTransformBtn');
        if (resetTransformBtn) {
            resetTransformBtn.onclick = () => {
                state.imageScale = 1;
                state.imageRotation = 0;
                TokenCanvas.updateScaleUI();
                TokenCanvas.updateRotationUI();
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas._forceFullRender();
                TokenCanvas.render();
                TokenHistory.save();
                toast('Трансформация сброшена');
            };
        }
    },

    setupAccordions() {
        function toggle(headerId, bodyId, arrowId, onOpen) {
            var header = $(headerId);
            var body = $(bodyId);
            var arrow = $(arrowId);
            if (!header || !body) return;
            var setOpen = function(isOpen) {
                body.classList.toggle('open', isOpen);
                body.inert = !isOpen;
                header.setAttribute('aria-expanded', String(isOpen));
                if (arrow) arrow.classList.toggle('open', isOpen);
                if (isOpen) {
                    if (onOpen) onOpen();
                    setTimeout(function() { body.scrollIntoView({ behavior: 'smooth', block: 'nearest' }); }, 50);
                }
            };
            body.inert = !body.classList.contains('open');
            header.onclick = function(e) {
                if (e.target.tagName === 'INPUT' || e.target.tagName === 'LABEL') return;
                if (e.target.closest('.accordion-reset-btn')) return;
                setOpen(!body.classList.contains('open'));
            };
            header.onkeydown = function(e) {
                if (e.key !== 'Enter' && e.key !== ' ') return;
                e.preventDefault();
                setOpen(!body.classList.contains('open'));
            };
        }
        toggle('shadowAccordion', 'dropShadowSettings', 'shadowArrow');
        toggle('ccAccordion', 'colorCorrectionSettings', 'ccArrow');
        toggle('portraitAccordion', 'portraitSettings', 'portraitArrow', function() {
            if (typeof PortraitGenerator !== 'undefined' && PortraitGenerator.canvas) {
                PortraitGenerator._applyDisplaySize();
                PortraitGenerator.render();
            }
        });
        toggle('exampleAccordion', 'exampleSettings', 'exampleArrow');
    },

    setupCheckboxes() {
        const dropShadowCheck = $('dropShadowCheck');
        if (dropShadowCheck) {
            dropShadowCheck.checked = state.dropShadowEnabled;
            dropShadowCheck.onchange = e => {
                state.dropShadowEnabled = e.target.checked;
                AppConfig.setDropShadowEnabled(state.dropShadowEnabled);
                if (!state.dropShadowEnabled) TokenCanvas._shadowCache = null;
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.render();
            };
        }
        const colorCorrectionCheck = $('colorCorrectionCheck');
        if (colorCorrectionCheck) {
            colorCorrectionCheck.checked = state.colorCorrectionEnabled;
            colorCorrectionCheck.onchange = e => {
                state.colorCorrectionEnabled = e.target.checked;
                AppConfig.setColorCorrectionEnabled(state.colorCorrectionEnabled);
                TokenCanvas.invalidateAllCaches();
                TokenCanvas.render();
            };
        }
        this._setupDropShadowSettings();
        this._setupColorCorrectionSettings();
        this._setupExampleOverlay();

        const showZonesCheck = $('showZonesCheck');
        if (showZonesCheck) {
            showZonesCheck.checked = !!state.showErasedZones;
            showZonesCheck.onchange = e => {
                state.showErasedZones = e.target.checked;
                state.showProtectionMask = e.target.checked;
                if (!e.target.checked) TokenCanvas._freeEffectCaches();
                else TokenCanvas.invalidateProtectionOverlay();
                TokenCanvas.render();
            };
        }

        const protectionEnabledCheck = $('protectionEnabledCheck');
        if (protectionEnabledCheck) {
            protectionEnabledCheck.checked = !!state.protectionEnabled;
            protectionEnabledCheck.onchange = e => {
                state.protectionEnabled = e.target.checked;
                AppConfig.setProtectionEnabled(state.protectionEnabled);
                TokenPresets.loadProtectionMask();
                TokenCanvas._imageBrushCache = null;
                TokenCanvas.invalidateProtectionOverlay();
                TokenCanvas.requestRender();
                toast(state.protectionEnabled ? 'Защита областей включена' : 'Защита областей выключена');
            };
        }

        const showBordersCheck = $('showBordersCheck');
        if (showBordersCheck) {
            showBordersCheck.checked = !!state.showScaleBorders;
            showBordersCheck.onchange = e => { state.showScaleBorders = e.target.checked; TokenCanvas.render(); };
        }

        const saveQualitySelect = $('saveQualitySelect');
        if (saveQualitySelect) {
            var savedQ = AppConfig.saveSettings.quality || 512;
            saveQualitySelect.value = savedQ;
            state.saveQuality = savedQ;
            saveQualitySelect.onchange = e => {
                state.saveQuality = parseInt(e.target.value);
                AppConfig.setSaveSetting('quality', state.saveQuality);
                toast('Размер: ' + state.saveQuality + 'px');
            };
        }

        const saveScaleSelect = $('saveScaleSelect');
        if (saveScaleSelect) {
            var savedS = AppConfig.saveSettings.scaleMode || 'auto';
            saveScaleSelect.value = savedS;
            state.saveScaleMode = savedS;
            saveScaleSelect.onchange = e => {
                state.saveScaleMode = e.target.value;
                AppConfig.setSaveSetting('scaleMode', state.saveScaleMode);
                const labels = { 'auto': 'Авто', '1': '×1', '2': '×2', '3': '×3' };
                toast('Масштаб сохранения: ' + (labels[state.saveScaleMode] || state.saveScaleMode));
            };
        }

        const quickSaveCheck = $('quickSaveCheck');
        const quickSaveFolderRow = $('quickSaveFolderRow');
        const quickSaveFolderBtn = $('quickSaveFolderBtn');
        const quickSaveFolderName = $('quickSaveFolderName');
        if (quickSaveCheck) {
            quickSaveCheck.checked = !!(state.quickSaveEnabled = AppConfig.quickSaveEnabled);
            quickSaveCheck.onchange = e => {
                state.quickSaveEnabled = e.target.checked;
                AppConfig.setQuickSaveEnabled(state.quickSaveEnabled);
                if (quickSaveFolderRow) quickSaveFolderRow.style.display = state.quickSaveEnabled ? 'flex' : 'none';
                if (state.quickSaveEnabled && !state.quickSaveFolder) this._pickSaveFolder(quickSaveFolderName);
            };
        }
        if (quickSaveFolderBtn) quickSaveFolderBtn.onclick = () => this._pickSaveFolder(quickSaveFolderName);
    },

    _setupDropShadowSettings() {
        const sliders = [
            { id: 'shadowAngle',    valId: 'shadowAngleVal',    key: 'angle',   factor: 1    },
            { id: 'shadowDistance', valId: 'shadowDistanceVal', key: 'distance',factor: 1    },
            { id: 'shadowBlur',     valId: 'shadowBlurVal',     key: 'blur',    factor: 1    },
            { id: 'shadowOpacity',  valId: 'shadowOpacityVal',  key: 'opacity', factor: 0.01 },
        ];
        const ds = AppConfig.dropShadow;
        const initVals = { shadowAngle: ds.angle, shadowDistance: ds.distance, shadowBlur: ds.blur, shadowOpacity: Math.round(ds.opacity * 100) };
        sliders.forEach(({ id, valId, key, factor }) => {
            const el = $(id); const valEl = $(valId);
            if (!el) return;
            el.value = initVals[id];
            if (valEl) valEl.textContent = el.value;
            setSliderFill(el);
            el.oninput = () => { 
                AppConfig.setDropShadow(key, parseFloat(el.value) * factor); 
                if (valEl) valEl.textContent = el.value; 
                setSliderFill(el);
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.scheduleEffects(false);
                TokenCanvas.requestRender(); 
            };
            el.addEventListener('wheel', ev => { ev.preventDefault(); const step = parseFloat(el.step)||1; el.value = clamp(parseFloat(el.value)+(ev.deltaY<0?step:-step),parseFloat(el.min),parseFloat(el.max)); el.dispatchEvent(new Event('input')); }, { passive: false });
        });
        const resetBtn = $('shadowResetBtn');
        if (resetBtn) {
            resetBtn.onclick = () => {
                AppConfig.resetDropShadow();
                const ds2 = AppConfig.dropShadow;
                $('shadowAngle').value = ds2.angle; $('shadowDistance').value = ds2.distance;
                $('shadowBlur').value = ds2.blur; $('shadowOpacity').value = Math.round(ds2.opacity * 100);
                sliders.forEach(({ id, valId }) => { const s = $(id); const valEl = $(valId); if (valEl) valEl.textContent = s.value; setSliderFill(s); });
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas.render(); 
                toast('Тень сброшена');
            };
        }
    },

    _setupColorCorrectionSettings() {
        const sliders = [
            { id: 'ccSaturation', valId: 'ccSaturationVal', key: 'saturation' },
            { id: 'ccLightness',  valId: 'ccLightnessVal',  key: 'lightness'  },
        ];
        const cc = AppConfig.colorCorrection;
        const initVals = { ccSaturation: cc.saturation, ccLightness: cc.lightness };
        sliders.forEach(({ id, valId, key }) => {
            const el = $(id); const valEl = $(valId);
            if (!el) return;
            el.value = initVals[id];
            if (valEl) valEl.textContent = el.value;
            setSliderFill(el);
            el.oninput = () => { 
                AppConfig.setColorCorrection(key, parseFloat(el.value)); 
                if (valEl) valEl.textContent = el.value; 
                setSliderFill(el);
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas._deferCC = true;
                TokenCanvas.scheduleEffects(false);
                TokenCanvas.requestRender(); 
            };
            el.addEventListener('wheel', ev => { ev.preventDefault(); const step = parseFloat(el.step)||1; el.value = clamp(parseFloat(el.value)+(ev.deltaY<0?step:-step),parseFloat(el.min),parseFloat(el.max)); el.dispatchEvent(new Event('input')); }, { passive: false });
        });
        const resetBtn = $('ccResetBtn');
        if (resetBtn) {
            resetBtn.onclick = () => {
                AppConfig.resetColorCorrection();
                const cc2 = AppConfig.colorCorrection;
                $('ccSaturation').value = cc2.saturation; $('ccLightness').value = cc2.lightness;
                sliders.forEach(({ id, valId }) => { const s = $(id); const valEl = $(valId); if (valEl) valEl.textContent = s.value; setSliderFill(s); });
                TokenCanvas.invalidateAllCaches();
                TokenCanvas.render(); 
                toast('Цветокоррекция сброшена');
            };
        }
    },

    setupSaveButtons() {
        const saveWithoutRingBtn = $('saveWithoutRing');
        if (saveWithoutRingBtn) saveWithoutRingBtn.onclick = () => TokenCanvas.save(false);
        const saveWithRingBtn = $('saveWithRing');
        if (saveWithRingBtn) saveWithRingBtn.onclick = () => TokenCanvas.save(true);
        this.updateRemoveBgButton();

        var previewTimer = null;
        var previewEl = null;
        function ensurePreviewEl() {
            if (previewEl && previewEl.parentNode) return previewEl;
            previewEl = document.createElement('div');
            previewEl.className = 'save-preview-tip';
            previewEl.innerHTML = '<img alt=""><div class="save-preview-tip-label"></div>';
            document.body.appendChild(previewEl);
            return previewEl;
        }
        function showPreview(btn, withRing) {
            if (!state.userImage) return;
            if (previewTimer) clearTimeout(previewTimer);
            previewTimer = setTimeout(function() {
                var result = TokenCanvas.renderForSave(withRing);
                if (!result || !result.canvas) return;
                var srcCanvas = result.canvas;
                var sw = srcCanvas.width, sh = srcCanvas.height;
                var previewCanvas = document.createElement('canvas');
                previewCanvas.width = 256;
                previewCanvas.height = 256;
                var pCtx = previewCanvas.getContext('2d');
                pCtx.imageSmoothingEnabled = true;
                pCtx.imageSmoothingQuality = 'high';
                pCtx.drawImage(srcCanvas, 0, 0, sw, sh, 0, 0, 256, 256);
                var el = ensurePreviewEl();
                var row = $('saveBtnRow');
                var anchor = row || btn;
                el.style.width = anchor.getBoundingClientRect().width + 'px';
                el.querySelector('img').src = previewCanvas.toDataURL('image/webp', 0.85);
                el.querySelector('.save-preview-tip-label').textContent = withRing ? 'С кольцом' : 'Без кольца';
                el.className = 'save-preview-tip show';
                var rect = anchor.getBoundingClientRect();
                var left = rect.left + rect.width / 2 - el.offsetWidth / 2;
                var top = rect.top - 6 - el.offsetHeight;
                el.style.left = Math.max(4, Math.min(left, window.innerWidth - el.offsetWidth - 4)) + 'px';
                el.style.top = (top < 4 ? rect.bottom + 6 : top) + 'px';
            }, 200);
        }
        function hidePreview() {
            if (previewTimer) { clearTimeout(previewTimer); previewTimer = null; }
            if (previewEl) previewEl.className = 'save-preview-tip';
        }
        if (saveWithRingBtn) {
            saveWithRingBtn.addEventListener('mouseenter', function() { showPreview(this, true); });
            saveWithRingBtn.addEventListener('mouseleave', hidePreview);
        }
        if (saveWithoutRingBtn) {
            saveWithoutRingBtn.addEventListener('mouseenter', function() { showPreview(this, false); });
            saveWithoutRingBtn.addEventListener('mouseleave', hidePreview);
        }

        const undoBtn = $('undoBtn');
        const redoBtn = $('redoBtn');
        const undoBar = $('undoBar');
        if (undoBtn) undoBtn.onclick = () => { TokenHistory.undo(); this._updateUndoButtons(); };
        if (redoBtn) redoBtn.onclick = () => { TokenHistory.redo(); this._updateUndoButtons(); };
        if (undoBar) undoBar.style.display = state.userImage ? 'flex' : 'none';

        const lastQuickSave = AppConfig.lastFolders.quickSave;
        if (lastQuickSave) {
            state.quickSaveFolder = lastQuickSave;
            const nameEl = $('quickSaveFolderName');
            if (nameEl) nameEl.textContent = lastQuickSave.split(/[\\/]/).pop() || lastQuickSave;
        }
    },

    _updateUndoButtons() {
        const undoBtn = $('undoBtn');
        const redoBtn = $('redoBtn');
        if (undoBtn) undoBtn.disabled = state.historyIndex <= 0;
        if (redoBtn) redoBtn.disabled = state.historyIndex >= state.history.length - 1;
    },

    setupPortraitVisibility() {
        if (!PortraitGenerator.canvas) PortraitGenerator.init();
    },

    setupKeyboardControls() {
        document.addEventListener('keydown', e => this.handleKeyDown(e));
        document.addEventListener('keyup', e => this.handleKeyUp(e));
    },

    _rotateCodes() {
        const hk = AppConfig.hotkeys;
        return {
            left: parseHotkey(hk.rotateLeft || 'KeyQ').code,
            right: parseHotkey(hk.rotateRight || 'KeyE').code
        };
    },

    handleKeyDown(e) {
        const isTokenMode = $('tokenPanel')?.classList.contains('active');
        if (!isTokenMode) return;
        const tag = e.target.tagName.toLowerCase();
        const isInput = tag === 'input' || tag === 'textarea' || tag === 'select';
        const code = e.code;
        const hk = AppConfig.hotkeys;
        if (isInput) return;
        if (hotkeyMatches(e, hk.undo)) { e.preventDefault(); TokenHistory.undo(); return; }
        if (hotkeyMatches(e, hk.redo)) { e.preventDefault(); TokenHistory.redo(); return; }
        const undoParsed = parseHotkey(hk.undo);
        if (e.ctrlKey && e.shiftKey && !e.altKey && undoParsed.ctrl && !undoParsed.alt && !undoParsed.shift && code === undoParsed.code) { e.preventDefault(); TokenHistory.redo(); return; }
        const rc = this._rotateCodes();
        if (!isInput && state.userImage && (hotkeyMatches(e, hk.rotateLeft) || hotkeyMatches(e, hk.rotateRight))) { e.preventDefault(); this.handleRotateKey(code); return; }
        if (!isInput && state.userImage && MOVE_KEYS.includes(code) && !e.ctrlKey) { e.preventDefault(); this.handleMoveKey(code); return; }
        if (!isInput && e.ctrlKey && code === 'ArrowLeft' && state.imageFileList.length > 1) { e.preventDefault(); this.navigateTo(-1); }
        if (!isInput && e.ctrlKey && code === 'ArrowRight' && state.imageFileList.length > 1) { e.preventDefault(); this.navigateTo(1); }
    },

    handleKeyUp(e) {
        const code = e.code;
        const rc = this._rotateCodes();
        if (code === rc.left || code === rc.right) {
            this.pressedKeys.delete(code);
            if (!this.pressedKeys.has(rc.left) && !this.pressedKeys.has(rc.right)) {
                this.clearRotateTimers(); this.initialRotateDone = false;
                if (state.userImage) TokenHistory.save();
            }
        }
        if (MOVE_KEYS.includes(code)) {
            this.pressedKeys.delete(code);
            const hasMovementKeys = MOVE_KEYS.some(k => this.pressedKeys.has(k));
            if (!hasMovementKeys) { this.clearMoveTimers(); this.initialMoveDone = false; if (state.userImage) TokenHistory.save(); }
        }
    },

    handleRotateKey(code) {
        if (!this.pressedKeys.has(code)) {
            this.pressedKeys.add(code);
            if (!this.initialRotateDone) {
                this.rotateByKey(code, CONFIG.ROTATE_STEP);
                this.initialRotateDone = true;
                const rc = this._rotateCodes();
                this.rotateTimeout = setTimeout(() => {
                    if (this.pressedKeys.has(rc.left) || this.pressedKeys.has(rc.right)) {
                        this.rotateInterval = setInterval(() => {
                            if (this.pressedKeys.has(rc.left)) this.rotateByKey(rc.left, CONFIG.ROTATE_STEP);
                            if (this.pressedKeys.has(rc.right)) this.rotateByKey(rc.right, CONFIG.ROTATE_STEP);
                        }, 50);
                    }
                }, 500);
            }
        }
    },

    handleMoveKey(code) {
        if (!this.pressedKeys.has(code)) {
            this.pressedKeys.add(code);
            if (!this.initialMoveDone) {
                this.moveByKeys(1);
                this.initialMoveDone = true;
                this.moveTimeout = setTimeout(() => {
                    if (this.pressedKeys.size > 0) this.moveInterval = setInterval(() => this.moveByKeys(CONFIG.MOVE_STEP), 50);
                }, 500);
            }
        }
    },

    rotateByKey(code, step) {
        if (!state.userImage) return;
        const rc = this._rotateCodes();
        const direction = code === rc.left ? -1 : 1;
        state.imageRotation += direction * step;
        if (state.imageRotation > 180) state.imageRotation -= 360;
        if (state.imageRotation < -180) state.imageRotation += 360;
        TokenCanvas.updateRotationUI(); TokenCanvas.scheduleEffects(); TokenCanvas.requestRender();
    },

    moveByKeys(step = CONFIG.MOVE_STEP) {
        if (!state.userImage) return;
        let moved = false;
        if (this.pressedKeys.has('KeyW') || this.pressedKeys.has('ArrowUp'))    { state.imageY -= step; moved = true; }
        if (this.pressedKeys.has('KeyS') || this.pressedKeys.has('ArrowDown'))  { state.imageY += step; moved = true; }
        if (this.pressedKeys.has('KeyA') || this.pressedKeys.has('ArrowLeft'))  { state.imageX -= step; moved = true; }
        if (this.pressedKeys.has('KeyD') || this.pressedKeys.has('ArrowRight')) { state.imageX += step; moved = true; }
        if (moved) { TokenCanvas.scheduleEffects(); TokenCanvas.requestRender(); }
    },

    clearRotateTimers() {
        if (this.rotateTimeout)  { clearTimeout(this.rotateTimeout);  this.rotateTimeout  = null; }
        if (this.rotateInterval) { clearInterval(this.rotateInterval); this.rotateInterval = null; }
    },

    clearMoveTimers() {
        if (this.moveTimeout)  { clearTimeout(this.moveTimeout);  this.moveTimeout  = null; }
        if (this.moveInterval) { clearInterval(this.moveInterval); this.moveInterval = null; }
    },

    _loadDefaultExample(fileNameEl) {
        fetch('/example')
            .then(r => {
                if (!r.ok) throw new Error('not found');
                return r.blob();
            })
            .then(blob => {
                urlManager.revoke('example-default');
                const url = urlManager.create(blob, 'example-default');
                const img = new Image();
                img.onload = () => {
                    state.exampleImage = img;
                    if (fileNameEl) fileNameEl.textContent = 'example.png';
                    TokenCanvas.render();
                };
                img.onerror = () => { urlManager.revoke('example-default'); state.exampleImage = null; };
                img.src = url;
            })
            .catch(() => {
                toast('Файл example.png не найден', true);
            });
    },

    _setupExampleOverlay() {
        const cfg = AppConfig.example;

        const check = $('exampleCheck');
        const opacitySlider = $('exampleOpacitySlider');
        const opacityVal = $('exampleOpacityVal');
        const scaleSelect = $('exampleScaleSelect');
        const fileInput = $('exampleFileInput');
        const fileBtn = $('exampleFileBtn');
        const fileName = $('exampleFileName');

        state.exampleEnabled = cfg.enabled;
        state.exampleOpacity = cfg.opacity / 100;
        state.exampleScaleMode = cfg.scaleMode;

        if (check) check.checked = cfg.enabled;
        if (opacitySlider) opacitySlider.value = cfg.opacity;
        if (opacityVal) opacityVal.textContent = cfg.opacity;
        if (scaleSelect) scaleSelect.value = cfg.scaleMode;

        if (cfg.enabled && !state.exampleImage) {
            this._loadDefaultExample(fileName);
        }

        const applyOpacity = val => {
            val = clamp(parseInt(val), 0, 100);
            state.exampleOpacity = val / 100;
            if (opacitySlider) opacitySlider.value = val;
            if (opacityVal) opacityVal.textContent = val;
            AppConfig.setExample('opacity', val);
            TokenCanvas.render();
        };

        if (check) {
            check.onchange = e => {
                state.exampleEnabled = e.target.checked;
                if (state.exampleEnabled && !state.exampleImage) {
                    this._loadDefaultExample(fileName);
                }
                AppConfig.setExample('enabled', state.exampleEnabled);
                TokenCanvas.render();
            };
        }

        if (opacitySlider) {
            opacitySlider.oninput = e => applyOpacity(e.target.value);
            opacitySlider.addEventListener('wheel', ev => {
                ev.preventDefault();
                opacitySlider.value = clamp(parseInt(opacitySlider.value) + (ev.deltaY < 0 ? 1 : -1), 0, 100);
                opacitySlider.dispatchEvent(new Event('input'));
            }, { passive: false });
        }

        if (scaleSelect) {
            scaleSelect.onchange = e => {
                state.exampleScaleMode = parseInt(e.target.value);
                AppConfig.setExample('scaleMode', state.exampleScaleMode);
                TokenCanvas.render();
            };
        }

        if (fileBtn && fileInput) {
            fileBtn.onclick = () => fileInput.click();
            fileInput.onchange = e => {
                const file = e.target.files[0];
                if (!file) return;
                urlManager.revoke('example-custom');
                const url = urlManager.create(file, 'example-custom');
                const img = new Image();
                img.onload = () => {
                    state._exampleCustomUrl = url;
                    state.exampleImage = img;
                    if (fileName) fileName.textContent = file.name;
                    TokenCanvas.render();
                };
                img.onerror = () => { urlManager.revoke('example-custom'); toast('Не удалось загрузить файл', true); };
                img.src = url;
                fileInput.value = '';
            };
        }

        const resetBtn = $('exampleResetBtn');
        if (resetBtn) {
            resetBtn.onclick = e => {
                e.stopPropagation();
                const def = AppConfig._defaults().example;
                urlManager.revoke('example-custom');
                state._exampleCustomUrl = null;
                state.exampleImage = null;
                state.exampleOpacity = def.opacity / 100;
                state.exampleScaleMode = def.scaleMode;
                if (fileName) fileName.textContent = 'example.png';
                if (opacitySlider) opacitySlider.value = def.opacity;
                if (opacityVal) opacityVal.textContent = def.opacity;
                if (scaleSelect) scaleSelect.value = def.scaleMode;
                AppConfig.setExample('opacity', def.opacity);
                AppConfig.setExample('scaleMode', def.scaleMode);
                this._loadDefaultExample(fileName);
                TokenCanvas.render();
                toast('Пример сброшен');
            };
        }
    },

    async _pickSaveFolder(nameEl) {
        const path = await pickFolder();
        if (!path) return;
        state.quickSaveFolder = path;
        AppConfig.setLastFolder('quickSave', path);
        if (nameEl) nameEl.textContent = path.split(/[\\/]/).pop() || path;
        toast('Папка выбрана: ' + path);

    }
};
