const PortraitGenerator = {
    canvas: null,
    ctx: null,
    SIZE: 2048,
    DISPLAY_SIZE: 480,

    imageX: 0,
    imageY: 0,
    imageScale: 1,
    imageRotation: 0,

    isDragging: false,
    dragStartX: 0,
    dragStartY: 0,
    dragStartImgX: 0,
    dragStartImgY: 0,

    _renderRafId: null,

    requestRender() {
        if (this._renderRafId) return;
        this._renderRafId = requestAnimationFrame(() => {
            this._renderRafId = null;
            this.render();
        });
    },

    quickSaveEnabled: false,
    saveFolder: null,

    init() {
        this.canvas = $('portraitCanvas');
        if (!this.canvas) return;
        this.canvas.addEventListener('contextmenu', function(e) { e.preventDefault(); });
        this.ctx = this.canvas.getContext('2d', { alpha: true });
        this.canvas.width = this.SIZE;
        this.canvas.height = this.SIZE;

        this.quickSaveEnabled = !!(AppConfig.portraitQuickSaveEnabled);

        const qsCheck = $('portraitQuickSaveCheck');
        if (qsCheck) qsCheck.checked = this.quickSaveEnabled;
        const qsRow = $('portraitQuickSaveFolderRow');
        if (qsRow) qsRow.style.display = this.quickSaveEnabled ? 'flex' : 'none';

        const lastFolder = AppConfig.lastFolders.portrait;
        if (lastFolder) {
            this.saveFolder = lastFolder;
            const nameEl = $('portraitFolderName');
            if (nameEl) nameEl.textContent = lastFolder.split(/[\\/]/).pop() || lastFolder;
        }

        this._applyDisplaySize();
        this._setupDisplaySizeObserver();
        this.setupEvents();
        this.setupControls();
        this.render();
    },

    _applyDisplaySize() {
        const wrap = this.canvas ? this.canvas.parentElement : null;
        if (!wrap) return;
        const availW = wrap.clientWidth || this.DISPLAY_SIZE;
        const side = Math.min(availW, this.DISPLAY_SIZE);
        this.canvas.style.width = side + 'px';
        this.canvas.style.height = side + 'px';
    },

    _setupDisplaySizeObserver() {
        if (this._ro) { this._ro.disconnect(); this._ro = null; }
        if (this._resizeHandler) {
            window.removeEventListener('resize', this._resizeHandler);
            this._resizeHandler = null;
        }
        const wrap = this.canvas ? this.canvas.parentElement : null;
        if (!wrap) return;
        if (typeof ResizeObserver !== 'undefined') {
            this._ro = new ResizeObserver(() => {
                this._applyDisplaySize();
                this.requestRender();
            });
            this._ro.observe(wrap);
        } else {
            this._resizeHandler = () => {
                this._applyDisplaySize();
                this.requestRender();
            };
            window.addEventListener('resize', this._resizeHandler);
        }
    },

    setupControls() {
        const folderBtn = $('portraitFolderBtn');
        if (folderBtn) folderBtn.onclick = () => this.pickFolder();

        const saveBtn = $('portraitSaveBtn');
        if (saveBtn) saveBtn.onclick = () => this.save();

        const qsCheck = $('portraitQuickSaveCheck');
        if (qsCheck) {
            qsCheck.checked = !!this.quickSaveEnabled;
            qsCheck.onchange = e => {
                this.quickSaveEnabled = e.target.checked;
                AppConfig.setPortraitQuickSaveEnabled(this.quickSaveEnabled);
                const qsRow = $('portraitQuickSaveFolderRow');
                if (qsRow) qsRow.style.display = this.quickSaveEnabled ? 'flex' : 'none';
                if (this.quickSaveEnabled && !this.saveFolder) this.pickFolder();
            };
        }

        const resetBtn = $('portraitResetBtn');
        if (resetBtn) resetBtn.onclick = e => { e.stopPropagation(); this.resetTransform(); };

        const scaleSlider = $('portraitScaleSlider');
        const scaleInput = $('portraitScaleInput');
        if (scaleSlider) {
            scaleSlider.oninput = e => {
                this.imageScale = parseInt(e.target.value) / 100;
                if (scaleInput) scaleInput.value = e.target.value;
                this.requestRender();
            };
            scaleSlider.addEventListener('wheel', ev => {
                ev.preventDefault();
                const step = 1;
                scaleSlider.value = clamp(parseInt(scaleSlider.value) + (ev.deltaY < 0 ? step : -step), 10, 500);
                scaleSlider.dispatchEvent(new Event('input'));
            }, { passive: false });
        }
        if (scaleInput) {
            scaleInput.onchange = e => {
                const v = clamp(parseInt(e.target.value) || 100, 10, 500);
                this.imageScale = v / 100;
                if (scaleSlider) scaleSlider.value = v;
                scaleInput.value = v;
                this.render();
            };
        }

        const rotSlider = $('portraitRotationSlider');
        const rotInput = $('portraitRotationInput');
        if (rotSlider) {
            rotSlider.oninput = e => {
                this.imageRotation = parseInt(e.target.value);
                if (rotInput) rotInput.value = e.target.value;
                this.requestRender();
            };
            rotSlider.addEventListener('wheel', ev => {
                ev.preventDefault();
                const step = 1;
                rotSlider.value = clamp(parseInt(rotSlider.value) + (ev.deltaY < 0 ? step : -step), -180, 180);
                rotSlider.dispatchEvent(new Event('input'));
            }, { passive: false });
        }
        if (rotInput) {
            rotInput.onchange = e => {
                const v = clamp(parseInt(e.target.value) || 0, -180, 180);
                this.imageRotation = v;
                if (rotSlider) rotSlider.value = v;
                rotInput.value = v;
                this.render();
            };
        }
    },

    setupEvents() {
        const c = this.canvas;

        c.addEventListener('mousedown', e => {
            if (e.button !== 0) return;
            e.preventDefault();
            this.isDragging = true;
            this.dragStartX = e.clientX;
            this.dragStartY = e.clientY;
            this.dragStartImgX = this.imageX;
            this.dragStartImgY = this.imageY;
            c.style.cursor = 'grabbing';
        });

        window.addEventListener('mousemove', e => {
            if (!this.isDragging) return;
            const displaySide = c.getBoundingClientRect().width || this.DISPLAY_SIZE;
            const ratio = this.SIZE / displaySide;
            this.imageX = this.dragStartImgX + (e.clientX - this.dragStartX) * ratio;
            this.imageY = this.dragStartImgY + (e.clientY - this.dragStartY) * ratio;
            this.requestRender();
        });

        window.addEventListener('mouseup', () => {
            if (!this.isDragging) return;
            this.isDragging = false;
            c.style.cursor = 'grab';
        });

        c.addEventListener('wheel', e => {
            e.preventDefault();
            const delta = e.deltaY > 0 ? -2 : 2;
            const scaleSlider = $('portraitScaleSlider');
            const scaleInput = $('portraitScaleInput');
            const newVal = clamp(Math.round(this.imageScale * 100) + delta, 10, 500);
            this.imageScale = newVal / 100;
            if (scaleSlider) scaleSlider.value = newVal;
            if (scaleInput) scaleInput.value = newVal;
            this.requestRender();
        }, { passive: false });

        c.addEventListener('contextmenu', e => e.preventDefault());
    },

    getImage() {
        return state.userImageWithoutBg || state.userImageOriginal || null;
    },

    render() {
        if (!this.ctx) return;
        const size = this.SIZE;

        this.ctx.clearRect(0, 0, size, size);

        const img = this.getImage();
        if (!img) {
            this.ctx.fillStyle = '#555';
            this.ctx.font = `${size * 0.022}px Inter, sans-serif`;
            this.ctx.textAlign = 'center';
            this.ctx.textBaseline = 'middle';
            this.ctx.fillText(I18n.t('Загрузите изображение'), size / 2, size / 2);
            return;
        }

        this.ctx.save();
        this.ctx.translate(size / 2 + this.imageX, size / 2 + this.imageY);
        this.ctx.rotate(this.imageRotation * Math.PI / 180);
        const w = img.width * this.imageScale;
        const h = img.height * this.imageScale;
        this.ctx.drawImage(img, -w / 2, -h / 2, w, h);
        this.ctx.restore();
    },

    renderForSave() {
        const quality = state.saveQuality || 512;
        const out = document.createElement('canvas');
        out.width = quality;
        out.height = quality;
        const ctx = out.getContext('2d', { alpha: true });

        ctx.clearRect(0, 0, quality, quality);

        const img = this.getImage();
        if (!img) return out;

        const ratio = quality / this.SIZE;

        ctx.save();
        ctx.translate(quality / 2 + this.imageX * ratio, quality / 2 + this.imageY * ratio);
        ctx.rotate(this.imageRotation * Math.PI / 180);
        ctx.drawImage(
            img,
            -img.width * this.imageScale * ratio / 2,
            -img.height * this.imageScale * ratio / 2,
            img.width * this.imageScale * ratio,
            img.height * this.imageScale * ratio
        );
        ctx.restore();

        return out;
    },

    resetTransform() {
        const img = this.getImage();
        if (img) {
            const maxDim = Math.max(img.width, img.height);
            // Клампим в диапазон слайдера 10-500%, иначе мелкие картинки
            // дают масштаб вне слайдера: UI и рендер расходятся
            this.imageScale = clamp(this.SIZE / maxDim, 0.1, 5);
        } else {
            this.imageScale = 1;
        }
        this.imageX = 0;
        this.imageY = 0;
        this.imageRotation = 0;
        const v = Math.round(this.imageScale * 100);
        const scaleSlider = $('portraitScaleSlider');
        const scaleInput = $('portraitScaleInput');
        const rotSlider = $('portraitRotationSlider');
        const rotInput = $('portraitRotationInput');
        if (scaleSlider) scaleSlider.value = v;
        if (scaleInput) scaleInput.value = v;
        if (rotSlider) rotSlider.value = 0;
        if (rotInput) rotInput.value = 0;
        this.render();
    },

    onImageLoaded() {
        this.resetTransform();
    },

    async pickFolder() {
        const path = await pickFolder();
        if (!path) return;
        this.saveFolder = path;
        AppConfig.setLastFolder('portrait', path);
        const nameEl = $('portraitFolderName');
        if (nameEl) nameEl.textContent = path.split(/[\\/]/).pop() || path;
        toast('Папка выбрана: ' + path);
    },

    async save() {
        const out = this.renderForSave();
        const fileName = ((state.tokenFileName || 'token').trim() || 'token') + '.webp';
        const blob = await new Promise(resolve => out.toBlob(resolve, 'image/webp', 0.95));

        if (this.quickSaveEnabled && this.saveFolder) {
            await saveToFolder(blob, fileName, this.saveFolder);
        } else {
            await saveFileWithPicker(blob, fileName);
        }
    }
};
