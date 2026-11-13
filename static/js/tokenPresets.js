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

    // Превью пресета: тёмный фон = то, что останется, оранжевая форма
    // (фирменный --accent #ff9800) = маска — то, что будет вырезано.
    // dataURL кэшируется на объекте пресета: сетка перерисовывается часто,
    // а toDataURL на 1536²-канвасе дорогой
    _getPresetThumb(preset) {
        if (preset.thumbUrl) return preset.thumbUrl;
        const thumbSize = 96;
        const thumb = document.createElement('canvas');
        thumb.width = thumbSize;
        thumb.height = thumbSize;
        const tCtx = thumb.getContext('2d');
        tCtx.fillStyle = '#17151a';
        tCtx.fillRect(0, 0, thumbSize, thumbSize);
        // форма выреза = прозрачные места preset.canvas: заполняем их оранжевым
        const tmp = document.createElement('canvas');
        tmp.width = thumbSize;
        tmp.height = thumbSize;
        const xc = tmp.getContext('2d');
        xc.fillStyle = '#ff9800';
        xc.fillRect(0, 0, thumbSize, thumbSize);
        xc.globalCompositeOperation = 'destination-out';
        xc.imageSmoothingEnabled = true;
        xc.imageSmoothingQuality = 'high';
        xc.drawImage(preset.canvas, 0, 0, thumbSize, thumbSize);
        tCtx.drawImage(tmp, 0, 0);
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
            // Файл пресета хранится в семантике, обратной маске: непрозрачные
            // тёмные пиксели = вырез, прозрачные = остаётся (как у встроенных).
            // maskCanvas наоборот (белое = остаётся), поэтому инвертируем:
            // заливаем цветом выреза и вырезаем область сохранённой маски
            sCtx.fillStyle = '#9833ff';
            sCtx.fillRect(0, 0, snapSize, snapSize);
            sCtx.globalCompositeOperation = 'destination-out';
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

        // Оранжевым (--accent #ff9800) подсвечивается маска — область, которая
        // будет ВЫРЕЗАНА (в preset.canvas она прозрачна), тёмным остаётся то,
        // что сохранится — так же, как в миниатюрах пресетов
        for (let i = 0; i < sd.length; i += 4) {
            const removed = sd[i+3] < 128;
            od[i]   = removed ? 255 : 0;
            od[i+1] = removed ? 152 : 0;
            od[i+2] = removed ? 0   : 0;
            od[i+3] = removed ? 200 : 0;
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

    _tokenatraRings: [],

    setupRingSourceSwitch() {
        const sw = $('ringSourceSwitch');
        if (!sw) return;
        sw.querySelectorAll('.chip-btn[data-ring-source]').forEach(btn => {
            btn.onclick = () => {
                const src = btn.dataset.ringSource;
                if (src === 'foundry' || src === 'tokenatra') {
                    this.setRingSource(src);
                }
            };
        });
        this._syncRingSourceUi();
    },

    _syncRingSourceUi() {
        const src = (state.embed && state.ringSource === 'foundry') ? 'foundry' : 'tokenatra';
        if (state.embed) {
            document.documentElement.dataset.ringSource = src;
        }
        const sw = $('ringSourceSwitch');
        if (sw) {
            sw.querySelectorAll('.chip-btn[data-ring-source]').forEach(btn => {
                btn.classList.toggle('active', btn.dataset.ringSource === src);
            });
        }
        const saveRow = $('saveBtnRow');
        if (saveRow) {
            saveRow.classList.toggle('foundry-ring-mode', src === 'foundry');
        }
    },

    setRingSource(source, { skipLoad = false } = {}) {
        const next = (state.embed && source === 'foundry') ? 'foundry' : 'tokenatra';
        state.ringSource = next;
        this._syncRingSourceUi();
        if (next === 'foundry') {
            this.renderFoundryRings({ skipLoad });
        } else {
            this.renderTokenatraRings({ skipLoad });
        }
    },

    receiveFoundryRings(msg) {
        const rings = Array.isArray(msg?.rings) ? msg.rings : [];
        state.foundryRings = rings;
        state.foundryWorldRingId = msg?.worldRingId || null;
        if (!rings.length) {
            if (state.ringSource === 'foundry') {
                this.setRingSource('tokenatra');
            }
            return;
        }
        const preferred = msg?.activeRingId;
        if (preferred && rings.some(r => r.id === preferred)) {
            state.activeFoundryRingId = preferred;
        } else if (!state.activeFoundryRingId || !rings.some(r => r.id === state.activeFoundryRingId)) {
            state.activeFoundryRingId = state.foundryWorldRingId || rings[0].id;
        }
        if (state.ringSource === 'foundry') {
            this.renderFoundryRings();
        }
    },

    updateFoundryRingThumb(id, thumbUrl, scale) {
        if (!id || !thumbUrl || !Array.isArray(state.foundryRings)) return;
        const entry = state.foundryRings.find(r => r.id === id);
        if (entry) {
            entry.thumbUrl = thumbUrl;
            if (typeof scale === 'number' && scale > 0) entry.scale = scale;
        }
        if (state.ringSource !== 'foundry') return;
        const container = $('ringSelectorList');
        if (!container) return;
        const el = container.querySelector('.ring-thumb[data-foundry-ring-id="' + String(id).replace(/"/g, '\\"') + '"]');
        if (!el) return;
        el.classList.remove('ring-thumb-loading');
        let stage = el.querySelector('.ring-thumb-stage');
        if (!stage) {
            const clip = document.createElement('div');
            clip.className = 'ring-thumb-clip';
            stage = document.createElement('div');
            stage.className = 'ring-thumb-stage';
            clip.appendChild(stage);
            el.appendChild(clip);
        }
        const s = (typeof scale === 'number' && scale > 0) ? scale : (entry?.scale || 1);
        stage.style.transform = Math.abs(s - 1) > 1e-3 ? `scale(${s})` : '';
        let img = stage.querySelector('img');
        if (!img) {
            img = document.createElement('img');
            img.alt = entry ? entry.label : id;
            img.dataset.tooltip = entry ? entry.label : id;
            stage.appendChild(img);
        }
        img.src = thumbUrl;
    },

    selectFoundryRing(id) {
        if (!id || !Array.isArray(state.foundryRings)) return false;
        if (!state.foundryRings.some(r => r.id === id)) return false;
        state.activeFoundryRingId = id;
        if (state.ringSource !== 'foundry') {
            this.setRingSource('foundry', { skipLoad: true });
        } else {
            const container = $('ringSelectorList');
            if (container) {
                container.querySelectorAll('.ring-thumb').forEach(el => {
                    el.classList.toggle('active', el.dataset.foundryRingId === id);
                });
            }
        }
        this.loadFoundryRing(id);
        return true;
    },

    renderFoundryRings({ skipLoad = false } = {}) {
        const container = $('ringSelectorList');
        if (!container) return;
        container.innerHTML = '';

        const rings = Array.isArray(state.foundryRings) ? state.foundryRings : [];
        if (!rings.length) {
            const empty = document.createElement('div');
            empty.className = 'ring-empty';
            empty.innerHTML = `<span style="font-size:9px;color:var(--text-muted);">${I18n.t('Загрузка колец Foundry…')}</span>`;
            container.appendChild(empty);
            return;
        }

        let activeId = state.activeFoundryRingId;
        if (!activeId || !rings.some(r => r.id === activeId)) {
            activeId = state.foundryWorldRingId && rings.some(r => r.id === state.foundryWorldRingId)
                ? state.foundryWorldRingId
                : rings[0].id;
            state.activeFoundryRingId = activeId;
        }

        rings.forEach(ring => {
            const item = document.createElement('div');
            item.className = 'ring-thumb' + (ring.id === activeId ? ' active' : '');
            if (!ring.thumbUrl) item.classList.add('ring-thumb-loading');
            item.dataset.foundryRingId = ring.id;
            item.dataset.ringName = ring.label;
            item.dataset.tooltip = ring.label;
            const clip = document.createElement('div');
            clip.className = 'ring-thumb-clip';
            const stage = document.createElement('div');
            stage.className = 'ring-thumb-stage';
            const rScale = (typeof ring.scale === 'number' && ring.scale > 0) ? ring.scale : 1;
            if (Math.abs(rScale - 1) > 1e-3) {
                stage.style.transform = `scale(${rScale})`;
            }
            if (ring.thumbUrl) {
                const img = document.createElement('img');
                img.src = ring.thumbUrl;
                img.alt = ring.label;
                img.dataset.tooltip = ring.label;
                stage.appendChild(img);
            }
            clip.appendChild(stage);
            item.appendChild(clip);
            item.onclick = () => {
                container.querySelectorAll('.ring-thumb').forEach(i => i.classList.remove('active'));
                item.classList.add('active');
                state.activeFoundryRingId = ring.id;
                this.loadFoundryRing(ring.id);
            };
            container.appendChild(item);
        });

        if (!skipLoad && activeId) {
            this.loadFoundryRing(activeId);
        }
    },

    loadFoundryRing(ringId) {
        if (!ringId || typeof TokenEmbed === 'undefined' || !TokenEmbed.requestFoundryRing) return;
        const loadId = (this._ringLoadId || 0) + 1;
        this._ringLoadId = loadId;
        this._protectionLoadId = (this._protectionLoadId || 0) + 1;
        urlManager.revoke('ring');
        urlManager.revoke('ring-top');
        urlManager.revoke('protection-mask');

        const loadImage = (blob, urlId) => new Promise((resolve, reject) => {
            const img = new Image();
            img.onload = () => resolve(img);
            img.onerror = reject;
            img.src = urlManager.create(blob, urlId);
        });

        const ringPromise = TokenEmbed.requestFoundryRing(ringId);
        const maskPromise = fetch('/mask').then(r => r.ok ? r.blob() : Promise.reject(new Error('Mask unavailable')));

        Promise.all([ringPromise, maskPromise])
            .then(([ringRes, maskBlob]) => Promise.all([
                loadImage(ringRes.ringBlob, 'ring'),
                ringRes.topBlob ? loadImage(ringRes.topBlob, 'ring-top') : Promise.resolve(null),
                loadImage(maskBlob, 'protection-mask'),
                Promise.resolve(ringRes.scale || 1)
            ]))
            .then(([ringImage, topImage, maskImage, ringScale]) => {
                if (loadId !== this._ringLoadId) return;
                state.activeFoundryRingId = ringId;
                state.ringImages = { 2048: ringImage, 1024: ringImage, 512: ringImage };
                state.ringTopImage = topImage || null;
                state.ringDrawScale = (typeof ringScale === 'number' && ringScale > 0) ? ringScale : 1;
                state.activeRingMaskFile = null;
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

    loadRings() {
        return fetch('/rings_list')
            .then(r => r.json())
            .then(rings => {
                this._tokenatraRings = Array.isArray(rings) ? rings : [];
                this._syncRingSourceUi();
                if (state.embed && state.ringSource === 'foundry') {
                    this.renderFoundryRings();
                    return;
                }
                this.renderTokenatraRings();
            })
            .catch(() => {});
    },

    renderTokenatraRings({ skipLoad = false } = {}) {
        const rings = Array.isArray(this._tokenatraRings) ? this._tokenatraRings : [];
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
        // По умолчанию выбрано сохранённое кольцо или «Руны: сталь» (rune-steel.webp)
        let defaultIdx = -1;
        if (state.activeTokenatraRingFile) {
            defaultIdx = rings.findIndex(r => r.file === state.activeTokenatraRingFile);
        }
        if (defaultIdx < 0) {
            defaultIdx = Math.max(0, rings.findIndex(r => (r.file || '').toLowerCase() === 'rune-steel.webp'));
        }
        rings.forEach((ring, index) => {
            const item = document.createElement('div');
            item.className = 'ring-thumb';
            if (index === defaultIdx) item.classList.add('active');
            item.dataset.ringName = ring.name;
            item.dataset.ringFile = ring.file;
            const img = document.createElement('img');
            // Hosted-студия: миниатюра из manifest (~10 КБ вместо
            // 0.3–4 МБ полного кольца с сервера мира); в десктопе
            // полный файл локален — грузим его
            const hasBakedThumb = !!(window.__STUDIO_HOSTED__ && ring.thumb);
            const thumbSrc = hasBakedThumb
                ? TokenTransport.assetUrl('/ring_file/' + encodeURIComponent(ring.thumb))
                : null;
            img.src = thumbSrc || (window.TokenTransport
                ? TokenTransport.assetUrl(`/ring_file/${encodeURIComponent(ring.file)}`)
                : `/ring_file/${encodeURIComponent(ring.file)}`);
            const baseName = (I18n.language === 'en' && ring.name_en) ? ring.name_en : ring.name;
            const localizedName = I18n.t(baseName);
            const ringScale = (typeof ring.scale === 'number' && ring.scale > 0) ? ring.scale : 1;
            const tipParts = [localizedName];
            if (ring.top_file) tipParts.push(I18n.t('2 слоя'));
            if (ringScale > 1.25) tipParts.push(`×${Math.round(ringScale * 10) / 10}`);
            const fullTip = tipParts.join(' · ');
            const clip = document.createElement('div');
            clip.className = 'ring-thumb-clip';
            const stage = document.createElement('div');
            stage.className = 'ring-thumb-stage';
            if (Math.abs(ringScale - 1) > 1e-3) {
                stage.style.transform = `scale(${ringScale})`;
            }
            img.alt = localizedName;
            img.dataset.tooltip = fullTip;
            stage.appendChild(img);
            if (ring.top_file && !hasBakedThumb) {
                const topImg = document.createElement('img');
                topImg.className = 'ring-thumb-top';
                topImg.src = window.TokenTransport
                    ? TokenTransport.assetUrl(`/ring_file/${encodeURIComponent(ring.top_file)}`)
                    : `/ring_file/${encodeURIComponent(ring.top_file)}`;
                topImg.alt = '';
                topImg.setAttribute('aria-hidden', 'true');
                stage.appendChild(topImg);
            }
            clip.appendChild(stage);
            item.appendChild(clip);
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
                            state.ringTopImage = null;
                            state.ringDrawScale = 1;
                            state.activeTokenatraRingFile = null;
                            state.activeTokenatraRingMask = null;
                            state.activeTokenatraRingTop = null;
                            state.activeTokenatraRingScale = 1;
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
                container.querySelectorAll('.ring-thumb').forEach(i => i.classList.remove('active'));
                item.classList.add('active');
                this.loadSingleRing(ring.file, ring.mask_file, ring.top_file || null, ringScale);
            };
            container.appendChild(item);
            if (!skipLoad && index === defaultIdx) {
                this.loadSingleRing(ring.file, ring.mask_file, ring.top_file || null, ringScale);
            }
        });
    },

    setupRingModal() {
        const modal = $('addRingModal');
        const form = $('addRingForm');
        const foundryForm = $('importFoundryForm');
        const openBtn = $('addRingBtn');
        const closeBtn = $('closeAddRing');
        const cancelBtn = $('cancelAddRing');
        const cancelFoundryBtn = $('cancelFoundryImport');
        const modeSwitch = $('addRingModeSwitch');
        if (!modal || !form || !openBtn) return;

        let foundrySource = null; // 'disk' | 'files' | null
        let foundryEntries = [];  // [{ key, jsonPath?, jsonFile?, name, scale, hasTop, size, checked }]
        let foundryUploadedAssets = []; // File[] (images for 'files' mode)

        const setTab = tab => {
            if (modeSwitch) {
                modeSwitch.querySelectorAll('.chip-btn[data-ring-tab]').forEach(btn => {
                    btn.classList.toggle('active', btn.dataset.ringTab === tab);
                });
            }
            form.hidden = (tab !== 'manual');
            if (foundryForm) foundryForm.hidden = (tab !== 'foundry');
        };

        if (modeSwitch) {
            modeSwitch.querySelectorAll('.chip-btn[data-ring-tab]').forEach(btn => {
                btn.onclick = () => setTab(btn.dataset.ringTab);
            });
        }

        const setFoundryStatus = msg => {
            const st = $('foundryImportStatus');
            if (!st) return;
            st.textContent = msg || '';
            st.hidden = !msg;
        };

        const syncFoundrySubmit = () => {
            const btn = $('foundryImportSubmitBtn');
            const countEl = $('foundryFoundCount');
            const checkedCount = foundryEntries.filter(e => e.checked).length;
            if (btn) btn.disabled = checkedCount === 0;
            if (countEl) {
                countEl.textContent = `${I18n.t('Найдено:')} ${foundryEntries.length} · ${I18n.t('Выбрано:')} ${checkedCount}`;
            }
        };

        const renderFoundryList = () => {
            const wrap = $('foundryFoundWrap');
            const list = $('foundryFoundList');
            if (!wrap || !list) return;
            list.innerHTML = '';
            if (!foundryEntries.length) {
                wrap.hidden = true;
                syncFoundrySubmit();
                return;
            }
            wrap.hidden = false;
            foundryEntries.forEach(entry => {
                const row = document.createElement('div');
                row.className = 'foundry-found-item';

                const cb = document.createElement('input');
                cb.type = 'checkbox';
                cb.checked = !!entry.checked;
                cb.onchange = () => {
                    entry.checked = cb.checked;
                    syncFoundrySubmit();
                };
                row.appendChild(cb);

                const nameInput = document.createElement('input');
                nameInput.type = 'text';
                nameInput.className = 'ring-text-input foundry-found-name';
                nameInput.value = entry.name;
                nameInput.maxLength = 80;
                nameInput.oninput = () => { entry.name = nameInput.value; };
                row.appendChild(nameInput);

                const badges = document.createElement('div');
                badges.className = 'foundry-found-badges';
                if (entry.hasTop) {
                    const b = document.createElement('span');
                    b.className = 'foundry-badge accent';
                    b.textContent = I18n.t('2-й слой');
                    badges.appendChild(b);
                }
                if (entry.scale && Math.abs(entry.scale - 1) > 0.01) {
                    const b = document.createElement('span');
                    b.className = 'foundry-badge';
                    b.textContent = `×${Math.round(entry.scale * 100) / 100}`;
                    badges.appendChild(b);
                }
                if (entry.size) {
                    const b = document.createElement('span');
                    b.className = 'foundry-badge';
                    b.textContent = `${entry.size}px`;
                    badges.appendChild(b);
                }
                row.appendChild(badges);
                list.appendChild(row);
            });
            syncFoundrySubmit();
        };

        const resetFoundryState = () => {
            foundrySource = null;
            foundryEntries = [];
            foundryUploadedAssets = [];
            if (foundryForm) foundryForm.reset();
            setFoundryStatus('');
            renderFoundryList();
        };

        const close = () => {
            modal.classList.remove('show');
            modal.setAttribute('aria-hidden', 'true');
        };
        const open = () => {
            setTab('manual');
            modal.classList.add('show');
            modal.setAttribute('aria-hidden', 'false');
            $('ringFileInput')?.focus();
        };

        openBtn.onclick = open;
        if (closeBtn) closeBtn.onclick = close;
        if (cancelBtn) cancelBtn.onclick = close;
        if (cancelFoundryBtn) cancelFoundryBtn.onclick = close;
        modal.onclick = event => {
            if (event.target === modal) close();
        };

        const pickJsonBtn = $('pickFoundryJsonBtn');
        if (pickJsonBtn) {
            pickJsonBtn.onclick = async () => {
                setFoundryStatus('');
                try {
                    const res = await fetch('/pick_foundry_rings');
                    const data = await res.json();
                    if (data.cancelled) return;
                    const items = Array.isArray(data.items) ? data.items : [];
                    foundrySource = 'disk';
                    foundryUploadedAssets = [];
                    foundryEntries = items.map(it => ({
                        key: it.json_path,
                        jsonPath: it.json_path,
                        name: (I18n.language === 'en' && it.suggested_name_en) ? it.suggested_name_en : it.suggested_name,
                        scale: it.scale || 1,
                        hasTop: !!it.has_top,
                        size: it.size || 0,
                        checked: true,
                    }));
                    renderFoundryList();
                    if (Array.isArray(data.errors) && data.errors.length) {
                        setFoundryStatus(data.errors.join(' · '));
                    } else if (!foundryEntries.length) {
                        setFoundryStatus(I18n.t('Подходящие кольца Foundry не найдены'));
                    }
                } catch (e) {
                    setFoundryStatus(e.message || I18n.t('Не удалось открыть файлы'));
                }
            };
        }

        const pickFolderBtn = $('pickFoundryFolderBtn');
        if (pickFolderBtn) {
            pickFolderBtn.onclick = async () => {
                setFoundryStatus('');
                try {
                    const res = await fetch('/pick_foundry_folder');
                    const data = await res.json();
                    if (data.cancelled) return;
                    const items = Array.isArray(data.items) ? data.items : [];
                    foundrySource = 'disk';
                    foundryUploadedAssets = [];
                    foundryEntries = items.map(it => ({
                        key: it.json_path,
                        jsonPath: it.json_path,
                        name: (I18n.language === 'en' && it.suggested_name_en) ? it.suggested_name_en : it.suggested_name,
                        scale: it.scale || 1,
                        hasTop: !!it.has_top,
                        size: it.size || 0,
                        checked: true,
                    }));
                    renderFoundryList();
                    if (!foundryEntries.length) {
                        setFoundryStatus(I18n.t('В выбранной папке не найдены спрайт-листы колец Foundry'));
                    }
                } catch (e) {
                    setFoundryStatus(e.message || I18n.t('Не удалось просканировать папку'));
                }
            };
        }

        const foundryFilesInput = $('foundryFilesInput');
        if (foundryFilesInput) {
            foundryFilesInput.onchange = async () => {
                setFoundryStatus('');
                const files = Array.from(foundryFilesInput.files || []);
                if (!files.length) return;
                const jsonFiles = files.filter(f => f.name.toLowerCase().endsWith('.json'));
                const assetFiles = files.filter(f => !f.name.toLowerCase().endsWith('.json'));
                if (!jsonFiles.length) {
                    setFoundryStatus(I18n.t('Выберите хотя бы один .json файл кольца вместе с изображениями'));
                    foundryEntries = [];
                    renderFoundryList();
                    return;
                }
                const entries = [];
                const errs = [];
                for (const jf of jsonFiles) {
                    try {
                        const txt = await jf.text();
                        const parsed = JSON.parse(txt);
                        const framesRaw = parsed?.frames ?? parsed?.config?.frames;
                        const frameList = Array.isArray(framesRaw)
                            ? framesRaw
                            : (framesRaw && typeof framesRaw === 'object' ? Object.entries(framesRaw).map(([k, v]) => ({ filename: k, ...v })) : []);
                        const baseFrames = frameList.filter(f => {
                            const n = String(f?.filename || '');
                            return n && !n.endsWith('-bkg') && !n.endsWith('-msk') && !n.endsWith('-top') && f?.frame?.w > 0;
                        });
                        if (!parsed?.meta?.image || !baseFrames.length) {
                            errs.push(`${jf.name}: ${I18n.t('не является спрайт-листом кольца')}`);
                            continue;
                        }
                        baseFrames.sort((a, b) => (a.frame.w || 0) - (b.frame.w || 0));
                        const best = baseFrames.filter(f => f.frame.w <= 2048).pop() || baseFrames[0];
                        const bname = String(best.filename || '');
                        const hasTop = !!(parsed?.config?.overlays?.[bname] || frameList.some(f => f.filename === `${bname}-top`));
                        const rawScale = Number(parsed?.config?.scale);
                        const scale = (Number.isFinite(rawScale) && rawScale >= 0.1 && rawScale <= 10) ? rawScale : 1;
                        const stem = jf.name.replace(/\.json$/i, '');
                        const pretty = stem.replace(/[_-]2x$/i, '').replace(/[_-]+/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
                        entries.push({
                            key: jf.name,
                            jsonFile: jf,
                            name: pretty,
                            scale: Math.round(scale * 10000) / 10000,
                            hasTop,
                            size: Math.min(2048, best.frame.w || 0),
                            checked: true,
                        });
                    } catch {
                        errs.push(`${jf.name}: ${I18n.t('ошибка чтения JSON')}`);
                    }
                }
                foundrySource = 'files';
                foundryUploadedAssets = assetFiles;
                foundryEntries = entries;
                renderFoundryList();
                if (errs.length) setFoundryStatus(errs.join(' · '));
            };
        }

        const selAllBtn = $('foundrySelectAllBtn');
        if (selAllBtn) selAllBtn.onclick = () => {
            foundryEntries.forEach(e => { e.checked = true; });
            renderFoundryList();
        };
        const selNoneBtn = $('foundrySelectNoneBtn');
        if (selNoneBtn) selNoneBtn.onclick = () => {
            foundryEntries.forEach(e => { e.checked = false; });
            renderFoundryList();
        };

        if (foundryForm) {
            foundryForm.onsubmit = async event => {
                event.preventDefault();
                const selected = foundryEntries.filter(e => e.checked);
                if (!selected.length) return;
                const submitBtn = $('foundryImportSubmitBtn');
                if (submitBtn) submitBtn.disabled = true;
                setFoundryStatus('');
                try {
                    let response;
                    if (foundrySource === 'disk') {
                        response = await fetch('/import_foundry_from_paths', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify({
                                items: selected.map(e => ({ json_path: e.jsonPath, name: e.name }))
                            })
                        });
                    } else {
                        const fd = new FormData();
                        const namesMap = {};
                        selected.forEach(e => {
                            if (e.jsonFile) {
                                fd.append('files', e.jsonFile, e.jsonFile.name);
                                namesMap[e.jsonFile.name] = e.name;
                            }
                        });
                        foundryUploadedAssets.forEach(af => fd.append('files', af, af.name));
                        fd.append('names', JSON.stringify(namesMap));
                        response = await fetch('/import_foundry_upload', { method: 'POST', body: fd });
                    }
                    const data = await response.json();
                    if (!response.ok || data.error) {
                        throw new Error(data.error || I18n.t('Не удалось импортировать кольца'));
                    }
                    const imported = Array.isArray(data.imported) ? data.imported : [];
                    if (imported.length && imported[0].file) {
                        state.activeTokenatraRingFile = imported[0].file;
                    }
                    close();
                    resetFoundryState();
                    await this.loadRings();
                    toast(`${I18n.t('Импортировано колец:')} ${imported.length}`);
                } catch (err) {
                    setFoundryStatus(err.message || I18n.t('Не удалось импортировать кольца'));
                } finally {
                    syncFoundrySubmit();
                }
            };
        }

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
                if (data.file) state.activeTokenatraRingFile = data.file;
                close();
                form.reset();
                const scInput = $('ringScaleInput');
                if (scInput) scInput.value = '1';
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

    loadSingleRing(filename, maskFile = null, topFile = null, ringScale = 1) {
        const loadId = (this._ringLoadId || 0) + 1;
        this._ringLoadId = loadId;
        this._protectionLoadId = (this._protectionLoadId || 0) + 1;
        urlManager.revoke('ring');
        urlManager.revoke('ring-top');
        urlManager.revoke('protection-mask');
        state.activeTokenatraRingFile = filename;
        state.activeTokenatraRingMask = maskFile || null;
        state.activeTokenatraRingTop = topFile || null;
        const resolvedScale = (typeof ringScale === 'number' && ringScale > 0) ? ringScale : 1;
        state.activeTokenatraRingScale = resolvedScale;
        const loadImage = (blob, urlId) => new Promise((resolve, reject) => {
            const img = new Image();
            img.onload = () => resolve(img);
            img.onerror = reject;
            img.src = urlManager.create(blob, urlId);
        });
        const ringRequest = fetch(`/ring_file/${encodeURIComponent(filename)}`)
            .then(r => r.ok ? r.blob() : Promise.reject(new Error('Ring unavailable')));
        const topRequest = topFile
            ? fetch(`/ring_file/${encodeURIComponent(topFile)}`)
                .then(r => r.ok ? r.blob() : null)
                .catch(() => null)
            : Promise.resolve(null);
        const maskRequest = maskFile
            ? fetch(`/ring_file/${encodeURIComponent(maskFile)}`)
                .then(r => r.ok ? r.blob() : Promise.reject(new Error('Mask unavailable')))
            : fetch('/mask').then(r => r.ok ? r.blob() : Promise.reject(new Error('Mask unavailable')));

        Promise.all([ringRequest, topRequest, maskRequest])
            .then(([ringBlob, topBlob, maskBlob]) => Promise.all([
                loadImage(ringBlob, 'ring'),
                topBlob ? loadImage(topBlob, 'ring-top') : Promise.resolve(null),
                loadImage(maskBlob, 'protection-mask')
            ]))
            .then(([ringImage, topImage, maskImage]) => {
                if (loadId !== this._ringLoadId) return;
                state.ringImages = { 2048: ringImage, 1024: ringImage, 512: ringImage };
                state.ringTopImage = topImage || null;
                state.ringDrawScale = resolvedScale;
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
