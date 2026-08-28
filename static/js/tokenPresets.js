const TokenPresets = {
    _presetOverlayTimer: null,

    buildProtectionCanvasFromImg(img, internalSize) {
        // Защитная маска соответствует области кольца 1× (internalSize / 3),
        // например 2048×2048 на рабочем холсте 6144×6144.
        // Возвращает { canvas, alpha }: альфа-массив отдаётся воркеру и
        // оверлею напрямую — без повторного getImageData всего канваса.
        const srcW = img.naturalWidth || img.width;
        const srcH = img.naturalHeight || img.height;
        const maskSize = Math.round(internalSize / 3);
        const maskOffset = (internalSize - maskSize) / 2;

        const scaledMask = document.createElement('canvas');
        scaledMask.width = internalSize;
        scaledMask.height = internalSize;
        const sCtx = scaledMask.getContext('2d');
        sCtx.imageSmoothingEnabled = true;
        sCtx.imageSmoothingQuality = 'high';
        sCtx.drawImage(img, 0, 0, srcW, srcH, maskOffset, maskOffset, maskSize, maskSize);
        const srcData = sCtx.getImageData(0, 0, internalSize, internalSize);
        const sd = srcData.data;

        const protCanvas = document.createElement('canvas');
        protCanvas.width = internalSize;
        protCanvas.height = internalSize;
        const pCtx = protCanvas.getContext('2d');
        const protData = pCtx.createImageData(internalSize, internalSize);
        const pd = protData.data;
        const n = internalSize * internalSize;
        const alpha = new Uint8Array(n);

        for (let i = 0; i < n; i++) {
            const oi = i * 4;
            const a = sd[oi + 3];
            const isProtected = a > 0;
            alpha[i] = isProtected ? 255 : 0;
            pd[oi] = 255; pd[oi+1] = 255; pd[oi+2] = 255;
            pd[oi+3] = isProtected ? 255 : 0;
        }

        pCtx.putImageData(protData, 0, 0);
        return { canvas: protCanvas, alpha: alpha };
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

    loadProtectionMask(maskFile = state.activeRingMaskFile) {
        if (!state.protectionEnabled) {
            state.erasableCanvas = null;
            state.protectionAlpha = null;
            state.protectionSize = 0;
            state.protectionMask = null;
            TokenCanvas.invalidateProtectionOverlay();
            TokenCanvas.syncProtectionToWorker();
            return;
        }
        const loadId = (this._protectionLoadId || 0) + 1;
        this._protectionLoadId = loadId;
        urlManager.revoke('protection-mask');
        const source = maskFile
            ? `/ring_file/${encodeURIComponent(maskFile)}`
            : '/mask';
        return fetch(source).then(r => {
            if (!r.ok) throw new Error('Mask unavailable');
            return r.blob();
        }).then(blob => {
            const img = new Image();
            const maskUrl = urlManager.create(blob, 'protection-mask');
            img.onload = () => {
                if (loadId !== this._protectionLoadId) return;
                state._rawProtectionMaskImg = img;
                this._rebuildErasableCanvas();
            };
            // Ошибка декодирования: URL больше не нужен — иначе утечка
            img.onerror = () => urlManager.revoke('protection-mask');
            img.src = maskUrl;
        }).catch(() => {
            if (loadId !== this._protectionLoadId) return;
            if (maskFile) {
                state.activeRingMaskFile = null;
                return this.loadProtectionMask(null);
            }
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
            state.protectionAlpha = null;
            state.protectionSize = 0;
            state.protectionMask = null;
            TokenCanvas.invalidateProtectionOverlay();
            TokenCanvas.syncProtectionToWorker();
            return;
        }
        const built = this.buildProtectionCanvasFromImg(img, internalSize);
        state.protectionMask = built.canvas;
        state.erasableCanvas = built.canvas;
        state.protectionAlpha = built.alpha;
        state.protectionSize = internalSize;
        TokenCanvas.invalidateProtectionOverlay();
        TokenCanvas.syncProtectionToWorker();
    },

    loadPresets() {
        urlManager.revoke('presets');
        fetch('/presets_list')
            .then(r => r.json())
            .then(presets => {
                state.eraserPresets = [];
                // Список изменился — индексный кэш оверлея и выделение невалидны
                state.currentPreset = -1;
                this._overlayCache = null;
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
                        file: preset.file,
                        builtin: !!preset.builtin,
                        thumbUrl: null
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

    // Превью пресета — уменьшенная копия маски (белая форма на прозрачном).
    // dataURL кэшируется на объекте пресета: сетка перерисовывается часто,
    // а toDataURL на 1536²-канвасе дорогой
    _getPresetThumb(preset) {
        if (preset.thumbUrl) return preset.thumbUrl;
        const thumbSize = 96;
        const thumb = document.createElement('canvas');
        thumb.width = thumbSize;
        thumb.height = thumbSize;
        const tCtx = thumb.getContext('2d');
        tCtx.imageSmoothingEnabled = true;
        tCtx.imageSmoothingQuality = 'high';
        tCtx.drawImage(preset.canvas, 0, 0, thumbSize, thumbSize);
        preset.thumbUrl = thumb.toDataURL('image/png');
        return preset.thumbUrl;
    },

    // «+» в заголовке «Пресеты масок»: снимок текущей розовой маски (1024px —
    // лёгкий файл, форма маски не детализирована выше) уходит на сервер
    async saveCurrentPreset() {
        if (!state.userImage || !state.maskCanvas) {
            toast('Сначала загрузите изображение', true);
            return;
        }
        const btn = $('addPresetBtn');
        if (btn) btn.disabled = true;
        try {
            const snapSize = 1024;
            const snap = document.createElement('canvas');
            snap.width = snapSize;
            snap.height = snapSize;
            const sCtx = snap.getContext('2d');
            sCtx.imageSmoothingEnabled = true;
            sCtx.imageSmoothingQuality = 'high';
            sCtx.drawImage(state.maskCanvas, 0, 0, state.maskCanvas.width, state.maskCanvas.height, 0, 0, snapSize, snapSize);
            const blob = await new Promise(resolve => snap.toBlob(resolve, 'image/png'));
            if (!blob) throw new Error('Не удалось закодировать маску');

            const fd = new FormData();
            fd.append('image', blob, 'mask.png');
            const res = await fetch('/add_preset', { method: 'POST', body: fd });
            const data = await res.json();
            if (!res.ok || data.error) throw new Error(data.error || 'Не удалось сохранить пресет');

            await this.loadPresets();
            toast('Пресет «' + data.name + '» сохранён');
        } catch (error) {
            toast(error.message || 'Не удалось сохранить пресет', true);
        } finally {
            if (btn) btn.disabled = false;
        }
    },

    async deletePreset(preset) {
        if (!preset || preset.builtin) return;
        try {
            const res = await fetch('/delete_preset', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ file: preset.file })
            });
            const data = await res.json();
            if (!res.ok || data.error) {
                toast(data.error || 'Не удалось удалить пресет', true);
                return;
            }
            await this.loadPresets();
            toast('Пресет удалён');
        } catch (e) {
            toast('Не удалось удалить пресет', true);
        }
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

        // Розовым подсвечивается область, где маска ОСТАНЕТСЯ (совпадает
        // с миниатюрой: белая форма = маска), а не то, что будет вырезано
        for (let i = 0; i < sd.length; i += 4) {
            const hasMask = sd[i+3] >= 128;
            od[i]   = hasMask ? 255 : 0;
            od[i+1] = hasMask ? 80  : 0;
            od[i+2] = hasMask ? 160 : 0;
            od[i+3] = hasMask ? 200 : 0;
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
        // Без изображения токена превью маски не имеет смысла — раньше
        // оверлей рисовался поверх пустого канваса
        if (!state.userImage || !state.maskCanvas) return;
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
            empty.className = 'preset-empty';
            empty.textContent = I18n.t('Нет пресетов');
            container.appendChild(empty);
            return;
        }

        state.eraserPresets.forEach((preset, index) => {
            if (!preset || !preset.canvas) return;
            const item = document.createElement('div');
            item.className = 'preset-thumb' + (state.currentPreset === index ? ' active' : '');
            item.dataset.tooltip = preset.name;

            const img = document.createElement('img');
            img.src = this._getPresetThumb(preset);
            img.alt = preset.name;
            item.appendChild(img);

            // Крестик удаления — только у пресетов, созданных кнопкой «+»
            // (встроенные лежат в ресурсах приложения и защищены от записи)
            if (!preset.builtin) {
                const delBtn = document.createElement('button');
                delBtn.type = 'button';
                delBtn.className = 'ring-del-btn preset-del-btn';
                delBtn.dataset.tooltip = I18n.t('Удалить пресет');
                delBtn.setAttribute('aria-label', I18n.t('Удалить пресет') + ' ' + preset.name);
                delBtn.innerHTML = '<svg viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>';
                delBtn.onclick = event => {
                    event.stopPropagation();
                    this.deletePreset(preset);
                };
                item.appendChild(delBtn);
            }

            item.onclick = () => this.apply(index);
            item.addEventListener('mouseenter', () => this.showPresetOverlay(index));
            item.addEventListener('mouseleave', () => this.hidePresetOverlay());
            container.appendChild(item);
        });
    },

    apply(index) {
        const preset = state.eraserPresets[index];
        if (!preset || !preset.canvas || !state.maskCanvas) return;

        this.hidePresetOverlay();

        const maskCtx = state.maskCanvas.getContext('2d');
        // Полная перезапись маски в обход ластика: сбрасываем несброшенные
        // штрихи и инкрементируем ген, чтобы пачки из полёта не втаптывали
        // старые штрихи в свежую маску
        TokenCanvas._invalidatePendingStrokes();
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
        // Маска переписана напрямую — синхронизируем альфа-зеркало воркера
        TokenCanvas._pushMaskToWorker(true);
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
                // По умолчанию выбрано кольцо «Сталь» (steel.webp);
                // если его нет в наборе — первое в списке
                const defaultIdx = Math.max(0, rings.findIndex(r => r.file === 'steel.webp'));
                rings.forEach((ring, index) => {
                    const item = document.createElement('div');
                    item.className = 'ring-thumb';
                    if (index === defaultIdx) item.classList.add('active');
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
                            try {
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
                                    state.activeRingMaskFile = null;
                                    this.loadProtectionMask(null);
                                    TokenCanvas.render();
                                }
                                this.loadRings();
                                toast('Кольцо удалено');
                            } catch (e) {
                                toast('Не удалось удалить кольцо', true);
                            }
                        };
                        item.appendChild(delBtn);
                    }
                    item.onclick = () => {
                        document.querySelectorAll('.ring-thumb').forEach(i => i.classList.remove('active'));
                        item.classList.add('active');
                        this.loadSingleRing(ring.file, ring.mask_file);
                    };
                    container.appendChild(item);
                    if (index === defaultIdx) this.loadSingleRing(ring.file, ring.mask_file);
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

    loadSingleRing(filename, maskFile = null) {
        const loadId = (this._ringLoadId || 0) + 1;
        this._ringLoadId = loadId;
        this._protectionLoadId = (this._protectionLoadId || 0) + 1;
        urlManager.revoke('ring');
        urlManager.revoke('protection-mask');
        const loadImage = (blob, urlId) => new Promise((resolve, reject) => {
            const img = new Image();
            img.onload = () => resolve(img);
            img.onerror = reject;
            img.src = urlManager.create(blob, urlId);
        });
        const ringRequest = fetch(`/ring_file/${encodeURIComponent(filename)}`)
            .then(r => r.ok ? r.blob() : Promise.reject(new Error('Ring unavailable')));
        const maskRequest = maskFile
            ? fetch(`/ring_file/${encodeURIComponent(maskFile)}`)
                .then(r => r.ok ? r.blob() : Promise.reject(new Error('Mask unavailable')))
            : fetch('/mask').then(r => r.ok ? r.blob() : Promise.reject(new Error('Mask unavailable')));

        Promise.all([ringRequest, maskRequest])
            .then(([ringBlob, maskBlob]) => Promise.all([
                loadImage(ringBlob, 'ring'),
                loadImage(maskBlob, 'protection-mask')
            ]))
            .then(([ringImage, maskImage]) => {
                if (loadId !== this._ringLoadId) return;
                state.ringImages = { 2048: ringImage, 1024: ringImage, 512: ringImage };
                state.activeRingMaskFile = maskFile;
                state._rawProtectionMaskImg = maskImage;
                if (state.protectionEnabled) {
                    this._rebuildErasableCanvas();
                } else {
                    state.erasableCanvas = null;
                    state.protectionAlpha = null;
                    state.protectionSize = 0;
                    state.protectionMask = null;
                }
                TokenCanvas.render();
            })
            .catch(() => {
                if (loadId === this._ringLoadId) toast('Не удалось загрузить кольцо или маску', true);
            });
    },

    loadRingForSize(size) {
        return new Promise((resolve) => {
            fetch(`/ring?size=${size}`).then(r => r.blob()).then(blob => {
                const img = new Image();
                urlManager.revoke('ring-' + size);
                const url = urlManager.create(blob, 'ring-' + size);
                img.onload = () => { state.ringImages[size] = img; resolve(img); };
                img.onerror = () => { urlManager.revoke('ring-' + size); resolve(null); };
                img.src = url;
            }).catch(() => resolve(null));
        });
    },

};
