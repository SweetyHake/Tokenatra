const TokenPresets = {
    _presetOverlayTimer: null,

    buildProtectionCanvasFromImg(img, internalSize) {
        // Маска — полноразмерный дизайн всего канваса (mask.png, напр. 6144×6144):
        // растягиваем на весь рабочий канвас. Раньше маска центрировалась как
        // кольцо (internalSize/3) и не совпадала с реальной областью изображения
        // при экспорте 2×/3×.
        const srcW = img.naturalWidth || img.width;
        const srcH = img.naturalHeight || img.height;

        const scaledMask = document.createElement('canvas');
        scaledMask.width = internalSize;
        scaledMask.height = internalSize;
        const sCtx = scaledMask.getContext('2d');
        sCtx.imageSmoothingEnabled = true;
        sCtx.imageSmoothingQuality = 'high';
        sCtx.drawImage(img, 0, 0, srcW, srcH, 0, 0, internalSize, internalSize);
        const srcData = sCtx.getImageData(0, 0, internalSize, internalSize);
        const sd = srcData.data;

        const protCanvas = document.createElement('canvas');
        protCanvas.width = internalSize;
        protCanvas.height = internalSize;
        const pCtx = protCanvas.getContext('2d');
        const protData = pCtx.createImageData(internalSize, internalSize);
        const pd = protData.data;
        const n = internalSize * internalSize;

        for (let i = 0; i < n; i++) {
            const oi = i * 4;
            const a = sd[oi + 3];
            const brightness = (sd[oi] + sd[oi+1] + sd[oi+2]) / 3;
            const isProtected = a > 16 && brightness < 220;
            pd[oi] = 255; pd[oi+1] = 255; pd[oi+2] = 255;
            pd[oi+3] = isProtected ? 255 : 0;
        }

        pCtx.putImageData(protData, 0, 0);
        return protCanvas;
    },

    processMaskImage(img, size) {
        if (!size) size = TokenCanvas.internalSize;
        const canvas = document.createElement('canvas');
        canvas.width = size;
        canvas.height = size;
        const ctx = canvas.getContext('2d');
        ctx.drawImage(img, 0, 0, size, size);
        const imgData = ctx.getImageData(0, 0, size, size);
        const result = ctx.createImageData(size, size);
        const d = imgData.data;
        const r = result.data;
        for (let i = 0; i < d.length; i += 4) {
            const a = d[i+3];
            const brightness = (d[i] + d[i+1] + d[i+2]) / 3;
            const isProtected = a > 16 && brightness < 220;
            r[i] = 255; r[i+1] = 255; r[i+2] = 255;
            r[i+3] = isProtected ? 0 : 255;
        }
        ctx.putImageData(result, 0, 0);
        return canvas;
    },

    loadProtectionMask() {
        if (!state.protectionEnabled) {
            state.erasableCanvas = null;
            state.protectionMask = null;
            TokenCanvas.invalidateProtectionOverlay();
            TokenCanvas.syncProtectionToWorker();
            return;
        }
        urlManager.revoke('protection-mask');
        fetch('/mask').then(r => r.blob()).then(blob => {
            const img = new Image();
            const maskUrl = urlManager.create(blob, 'protection-mask');
            img.onload = () => {
                state._rawProtectionMaskImg = img;
                this._rebuildErasableCanvas();
            };
            img.src = maskUrl;
        }).catch(() => {
            state._rawProtectionMaskImg = null;
            this._rebuildErasableCanvas();
        });
    },

    reloadProtectionMaskForScale() {
        this._rebuildErasableCanvas();
        TokenCanvas._imageBrushCache = null;
    },

    _rebuildErasableCanvas() {
        const img = state._rawProtectionMaskImg;
        const internalSize = TokenCanvas.internalSize;
        if (!img) {
            state.erasableCanvas = null;
            state.protectionMask = null;
            TokenCanvas.invalidateProtectionOverlay();
            TokenCanvas.syncProtectionToWorker();
            return;
        }
        const protCanvas = this.buildProtectionCanvasFromImg(img, internalSize);
        state.protectionMask = protCanvas;
        state.erasableCanvas = protCanvas;
        TokenCanvas.invalidateProtectionOverlay();
        TokenCanvas.syncProtectionToWorker();
    },

    loadPresets() {
        urlManager.revoke('presets');
        fetch('/presets_list')
            .then(r => r.json())
            .then(presets => {
                state.eraserPresets = [];
                const loads = presets.map((preset, index) => {
                    return fetch(`/preset_file/${encodeURIComponent(preset.file)}`)
                        .then(r => r.blob())
                        .then(blob => {
                            const img = new Image();
                            const url = urlManager.create(blob, 'presets');
                            return new Promise(resolve => {
                img.onload = () => {
                    state.eraserPresets[index] = {
                        canvas: this.processMaskImage(img, CONFIG.SCALE_SIZES[1]),
                        rawImg: img,
                        name: preset.name,
                        file: preset.file
                    };
                    resolve();
                };
                                img.onerror = () => resolve();
                                img.src = url;
                            });
                        })
                        .catch(() => {});
                });
                Promise.all(loads).then(() => this.updateButtons());
            })
            .catch(() => {});
    },

    _buildPresetOverlay(presetIndex) {
        const preset = state.eraserPresets[presetIndex];
        if (!preset || !preset.canvas) return null;
        if (this._overlayCache && this._overlayCache.index === presetIndex) {
            return this._overlayCache.canvas;
        }

        const overlaySize = 1024;
        const overlay = document.createElement('canvas');
        overlay.width = overlaySize;
        overlay.height = overlaySize;
        const ctx = overlay.getContext('2d');

        ctx.drawImage(preset.canvas, 0, 0, overlaySize, overlaySize);
        const srcData = ctx.getImageData(0, 0, overlaySize, overlaySize);
        const outData = ctx.createImageData(overlaySize, overlaySize);
        const sd = srcData.data;
        const od = outData.data;

        for (let i = 0; i < sd.length; i += 4) {
            const isErased = sd[i+3] < 128;
            od[i]   = isErased ? 255 : 0;
            od[i+1] = isErased ? 80  : 0;
            od[i+2] = isErased ? 160 : 0;
            od[i+3] = isErased ? 200 : 0;
        }

        ctx.putImageData(outData, 0, 0);

        if (this._overlayCache && this._overlayCache.canvas !== overlay) {
            this._overlayCache.canvas.width = 1;
            this._overlayCache.canvas.height = 1;
        }
        this._overlayCache = { index: presetIndex, canvas: overlay };
        return overlay;
    },

    showPresetOverlay(presetIndex) {
        const preset = state.eraserPresets[presetIndex];
        if (!preset || !preset.canvas) return;
        state.presetOverlayCanvas = this._buildPresetOverlay(presetIndex);
        state.presetOverlayActive = true;
        const el = $('presetOverlay');
        if (!el) {
            TokenCanvas.requestRender();
            return;
        }
        const canvas = state.presetOverlayCanvas;
        urlManager.revoke('presetOverlay');
        canvas.toBlob(blob => {
            // Применяем только если наведён именно этот пресет (асинхронный toBlob)
            if (!state.presetOverlayActive || state.presetOverlayCanvas !== canvas) return;
            const url = urlManager.create(blob, 'presetOverlay');
            el.style.backgroundImage = 'url(' + url + ')';
            el.style.width = TokenCanvas.canvas.style.width || '';
            el.style.height = TokenCanvas.canvas.style.height || '';
            el.style.left = TokenCanvas.canvas.style.left || '0';
            el.style.top = TokenCanvas.canvas.style.top || '0';
            el.classList.remove('pulsing');
            void el.offsetWidth;
            el.classList.add('pulsing');
        }, 'image/png');
    },

    hidePresetOverlay() {
        state.presetOverlayActive = false;
        state.presetOverlayCanvas = null;
        const el = $('presetOverlay');
        if (el) {
            el.classList.remove('pulsing');
            el.style.backgroundImage = '';
        }
        urlManager.revoke('presetOverlay');
        TokenCanvas.requestRender();
    },

    updateButtons() {
        const container = $('presetButtons');
        if (!container) return;
        container.innerHTML = '';

        if (state.eraserPresets.length === 0) {
            const empty = document.createElement('div');
            empty.style.cssText = 'font-size:11px;color:var(--text-muted);padding:4px 0;';
            empty.textContent = 'Нет пресетов в папке presets/';
            container.appendChild(empty);
            return;
        }

        state.eraserPresets.forEach((preset, index) => {
            if (!preset) return;
            const btn = document.createElement('button');
            btn.className = 'preset-btn' + (state.currentPreset === index ? ' active' : '');
            btn.textContent = preset.name;
            btn.onclick = () => this.apply(index);
            btn.addEventListener('mouseenter', () => this.showPresetOverlay(index));
            btn.addEventListener('mouseleave', () => this.hidePresetOverlay());
            container.appendChild(btn);
        });
    },

    apply(index) {
        const preset = state.eraserPresets[index];
        if (!preset || !preset.canvas || !state.maskCanvas) return;

        this.hidePresetOverlay();

        const maskCtx = state.maskCanvas.getContext('2d');
        maskCtx.globalCompositeOperation = 'source-over';
        maskCtx.fillStyle = 'white';
        maskCtx.fillRect(0, 0, state.maskCanvas.width, state.maskCanvas.height);
        maskCtx.globalCompositeOperation = 'destination-in';
        maskCtx.drawImage(
            preset.canvas, 0, 0, preset.canvas.width, preset.canvas.height,
            0, 0, state.maskCanvas.width, state.maskCanvas.height
        );
        maskCtx.globalCompositeOperation = 'source-over';

        state.currentPreset = index;
        this.updateButtons();
        TokenCanvas.invalidateAllCaches();
        TokenHistory.save();
        TokenCanvas.render();
        toast(`Пресет «${preset.name}» применён`);

        const pinkBtn = document.querySelector('.tool-btn[data-tool="mask"]');
        if (pinkBtn) pinkBtn.click();
    },

    loadRings() {
        return fetch('/rings_list')
            .then(r => r.json())
            .then(rings => {
                const container = $('ringSelectorList');
                if (!container) return;
                container.innerHTML = '';

                if (rings.length === 0) {
                    const empty = document.createElement('div');
                    empty.className = 'ring-empty';
                    empty.innerHTML = `<span style="font-size:9px;color:var(--text-muted);">Нет колец</span>`;
                    container.appendChild(empty);
                    return;
                }
                rings.forEach((ring, index) => {
                    const item = document.createElement('div');
                    item.className = 'ring-thumb';
                    if (index === 0) item.classList.add('active');
                    item.dataset.ringName = ring.name;
                    const img = document.createElement('img');
                    img.src = `/ring_file/${encodeURIComponent(ring.file)}`;
                    const localizedName = I18n.t(ring.name);
                    img.alt = localizedName;
                    img.dataset.tooltip = ring.name;
                    item.appendChild(img);
                    if (!ring.builtin) {
                        const delBtn = document.createElement('button');
                        delBtn.type = 'button';
                        delBtn.className = 'ring-del-btn';
                        delBtn.dataset.tooltip = 'Удалить кольцо';
                        delBtn.setAttribute('aria-label', I18n.t('Удалить кольцо') + ' ' + localizedName);
                        delBtn.innerHTML = '<svg viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>';
                        delBtn.onclick = async event => {
                            event.stopPropagation();
                            const response = await fetch('/delete_ring', {
                                method: 'POST',
                                headers: { 'Content-Type': 'application/json' },
                                body: JSON.stringify({ file: ring.file })
                            });
                            const data = await response.json();
                            if (!response.ok || data.error) {
                                toast(data.error || 'Не удалось удалить кольцо', true);
                                return;
                            }
                            if (item.classList.contains('active')) {
                                state.ringImages = {};
                                TokenCanvas.render();
                            }
                            this.loadRings();
                            toast('Кольцо удалено');
                        };
                        item.appendChild(delBtn);
                    }
                    item.onclick = () => {
                        document.querySelectorAll('.ring-thumb').forEach(i => i.classList.remove('active'));
                        item.classList.add('active');
                        this.loadSingleRing(ring.file);
                    };
                    container.appendChild(item);
                    if (index === 0) this.loadSingleRing(ring.file);
                });
            })
            .catch(() => {});
    },

    setupRingModal() {
        const modal = $('addRingModal');
        const form = $('addRingForm');
        const openBtn = $('addRingBtn');
        const closeBtn = $('closeAddRing');
        const cancelBtn = $('cancelAddRing');
        if (!modal || !form || !openBtn) return;

        const close = () => {
            modal.classList.remove('show');
            modal.setAttribute('aria-hidden', 'true');
        };
        const open = () => {
            modal.classList.add('show');
            modal.setAttribute('aria-hidden', 'false');
            $('ringFileInput')?.focus();
        };

        openBtn.onclick = open;
        if (closeBtn) closeBtn.onclick = close;
        if (cancelBtn) cancelBtn.onclick = close;
        modal.onclick = event => {
            if (event.target === modal) close();
        };

        form.onsubmit = async event => {
            event.preventDefault();
            const fileInput = $('ringFileInput');
            const file = fileInput?.files[0];
            if (!file) return;

            const submitBtn = form.querySelector('button[type="submit"]');
            if (submitBtn) submitBtn.disabled = true;
            try {
                const response = await fetch('/add_ring', { method: 'POST', body: new FormData(form) });
                const data = await response.json();
                if (!response.ok || data.error) throw new Error(data.error || 'Не удалось добавить кольцо');
                close();
                form.reset();
                await this.loadRings();
                toast('Кольцо добавлено');
            } catch (error) {
                toast(error.message || 'Не удалось добавить кольцо', true);
            } finally {
                if (submitBtn) submitBtn.disabled = false;
            }
        };

        document.addEventListener('keydown', event => {
            if (event.key === 'Escape' && modal.classList.contains('show')) close();
        });
    },

    loadSingleRing(filename) {
        urlManager.revoke('ring');
        fetch(`/ring_file/${encodeURIComponent(filename)}`)
            .then(r => r.blob())
            .then(blob => {
                const img = new Image();
                const url = urlManager.create(blob, 'ring');
                img.onload = () => {
                    state.ringImages = { 2048: img, 1024: img, 512: img };
                    TokenCanvas.render();
                };
                img.src = url;
            })
            .catch(() => {});
    },

    loadRingForSize(size) {
        return new Promise((resolve) => {
            fetch(`/ring?size=${size}`).then(r => r.blob()).then(blob => {
                const img = new Image();
                urlManager.revoke('ring-' + size);
                const url = urlManager.create(blob, 'ring-' + size);
                img.onload = () => { state.ringImages[size] = img; resolve(img); };
                img.onerror = () => resolve(null);
                img.src = url;
            }).catch(() => resolve(null));
        });
    },

};
