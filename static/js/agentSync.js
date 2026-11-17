// Двусторонняя Live GUI Sync между открытой студией Tokenatra и /agent/v1/* сессиями.
window.TokenAgentSync = {
    _sid: null,
    _rev: 0,
    _imgRev: 0,
    _cMaskRev: 0,
    _iMaskRev: 0,
    _lastRingKey: null,
    _applying: false,
    _inFlight: false,
    _pending: null,
    _pushTimer: null,
    _dirtyCanvasMask: false,
    _dirtyImageMask: false,

    isDisabled() {
        return !!(window.__STUDIO_BUILT__ || window.__STUDIO_HOSTED__ || (typeof state !== 'undefined' && state.embed));
    },

    isApplying() {
        return this._applying;
    },

    detachLocal() {
        if (this._pushTimer) {
            clearTimeout(this._pushTimer);
            this._pushTimer = null;
        }
        this._sid = null;
        this._rev = 0;
        this._imgRev = 0;
        this._cMaskRev = 0;
        this._iMaskRev = 0;
        this._lastRingKey = null;
        this._dirtyCanvasMask = false;
        this._dirtyImageMask = false;
    },

    onSessionClosed(sid) {
        if (this._sid === sid) {
            this.detachLocal();
        }
    },

    onSessionDetached() {
        this.detachLocal();
    },

    onServerPush(sid, rev, force) {
        if (this.isDisabled() || !sid) return;
        const r = Number(rev) || 0;
        if (!force && typeof state !== 'undefined' && state.userImage && this._sid && this._sid !== sid) {
            return;
        }
        if (!force && typeof state !== 'undefined' && state.userImage && !this._sid) {
            // Не перетирать локально открытую человеком картинку без явного push(force=true)
            return;
        }
        if (!force && this._sid === sid && r <= this._rev) {
            return;
        }
        if (this._inFlight) {
            if (!this._pending || this._pending.sid !== sid || r >= this._pending.rev) {
                this._pending = {
                    sid,
                    rev: r,
                    force: !!(force || (this._pending && this._pending.sid === sid && this._pending.force)),
                };
            }
            return;
        }
        this._applyFromServer(sid, !!force);
    },

    async _fetchAgent(path, opts) {
        const merged = Object.assign({}, opts || {});
        merged.headers = Object.assign({ 'X-Tokenatra-Agent': '1' }, merged.headers || {});
        return fetch(path, merged);
    },

    async _loadImageLayer(sid, layer, urlTag) {
        const resp = await this._fetchAgent(`/agent/v1/sessions/${encodeURIComponent(sid)}/layer/${layer}`);
        if (!resp.ok) throw new Error(`layer ${layer}: HTTP ${resp.status}`);
        const blob = await resp.blob();
        const url = urlManager.create(blob, urlTag);
        return await new Promise((resolve, reject) => {
            const img = new Image();
            img.onload = () => resolve({ img, url });
            img.onerror = () => {
                urlManager.revoke(urlTag);
                reject(new Error(`failed to decode ${layer}`));
            };
            img.src = url;
        });
    },

    async _applyMaskLayer(sid, layer, targetCanvas, isPink) {
        if (!targetCanvas) return;
        const resp = await this._fetchAgent(`/agent/v1/sessions/${encodeURIComponent(sid)}/layer/${layer}`);
        if (!resp.ok) throw new Error(`mask ${layer}: HTTP ${resp.status}`);
        const blob = await resp.blob();
        const url = URL.createObjectURL(blob);
        try {
            const img = await new Promise((resolve, reject) => {
                const im = new Image();
                im.onload = () => resolve(im);
                im.onerror = reject;
                im.src = url;
            });
            const w = targetCanvas.width;
            const h = targetCanvas.height;
            const tmp = document.createElement('canvas');
            tmp.width = w;
            tmp.height = h;
            const tCtx = tmp.getContext('2d');
            tCtx.imageSmoothingEnabled = true;
            tCtx.imageSmoothingQuality = 'high';
            tCtx.clearRect(0, 0, w, h);
            tCtx.drawImage(img, 0, 0, w, h);
            const srcData = tCtx.getImageData(0, 0, w, h);
            const sd = srcData.data;
            const ctx = targetCanvas.getContext('2d');
            const outData = ctx.createImageData(w, h);
            const od = outData.data;
            const n = w * h;
            for (let i = 0; i < n; i++) {
                const o = i * 4;
                od[o] = 255;
                od[o + 1] = 255;
                od[o + 2] = 255;
                od[o + 3] = sd[o];
            }
            ctx.putImageData(outData, 0, 0);
            if (typeof TokenCanvas !== 'undefined') {
                TokenCanvas._pushMaskToWorker(isPink);
                if (!isPink) TokenCanvas._invalidateComposite();
            }
        } finally {
            URL.revokeObjectURL(url);
        }
    },

    _syncEffectsUI(effects) {
        if (!effects) return;
        if (effects.shadow) {
            const sh = effects.shadow;
            state.dropShadowEnabled = !!sh.enabled;
            if (typeof AppConfig !== 'undefined') {
                AppConfig.setDropShadowEnabled(state.dropShadowEnabled);
                if (typeof sh.angle === 'number') AppConfig.setDropShadow('angle', sh.angle);
                if (typeof sh.distance === 'number') AppConfig.setDropShadow('distance', sh.distance);
                if (typeof sh.blur === 'number') AppConfig.setDropShadow('blur', sh.blur);
                if (typeof sh.opacity === 'number') AppConfig.setDropShadow('opacity', sh.opacity);
            }
            const chk = $('dropShadowCheck');
            if (chk) chk.checked = state.dropShadowEnabled;
            const map = [
                ['shadowAngle', 'shadowAngleVal', Math.round(sh.angle ?? 135)],
                ['shadowDistance', 'shadowDistanceVal', Math.round(sh.distance ?? 7)],
                ['shadowBlur', 'shadowBlurVal', Math.round(sh.blur ?? 3)],
                ['shadowOpacity', 'shadowOpacityVal', Math.round(((sh.opacity ?? 0.4) * 100))],
            ];
            map.forEach(([id, vid, val]) => {
                const el = $(id);
                const vel = $(vid);
                if (el) {
                    el.value = val;
                    if (typeof setSliderFill === 'function') setSliderFill(el);
                }
                if (vel) vel.value = val;
            });
            if (!state.dropShadowEnabled && typeof TokenCanvas !== 'undefined') {
                TokenCanvas._shadowCache = null;
            }
        }
        if (effects.color) {
            const cl = effects.color;
            state.colorCorrectionEnabled = !!cl.enabled;
            if (typeof AppConfig !== 'undefined') {
                AppConfig.setColorCorrectionEnabled(state.colorCorrectionEnabled);
                if (typeof cl.saturation === 'number') AppConfig.setColorCorrection('saturation', cl.saturation);
                if (typeof cl.lightness === 'number') AppConfig.setColorCorrection('lightness', cl.lightness);
                if (typeof cl.contrast === 'number') AppConfig.setColorCorrection('contrast', cl.contrast);
            }
            const chk = $('colorCorrectionCheck');
            if (chk) chk.checked = state.colorCorrectionEnabled;
            const map = [
                ['ccSaturation', 'ccSaturationVal', Math.round(cl.saturation ?? 0)],
                ['ccLightness', 'ccLightnessVal', Math.round(cl.lightness ?? 0)],
                ['ccContrast', 'ccContrastVal', Math.round(cl.contrast ?? 0)],
            ];
            map.forEach(([id, vid, val]) => {
                const el = $(id);
                const vel = $(vid);
                if (el) {
                    el.value = val;
                    if (typeof setSliderFill === 'function') setSliderFill(el);
                }
                if (vel) vel.value = val;
            });
        }
    },

    _syncRingUI(st) {
        const ringKey = st.ring_file || (st.ring === null ? '__none__' : '');
        if (ringKey === this._lastRingKey) return;
        this._lastRingKey = ringKey;
        if (!st.ring_file) {
            state.ringImages = {};
            state.ringTopImage = null;
            state.ringDrawScale = 1;
            state.activeTokenatraRingFile = null;
            state.activeTokenatraRingMask = null;
            state.activeTokenatraRingTop = null;
            state.activeTokenatraRingScale = 1;
            state.activeRingMaskFile = null;
            const cont = $('ringsContainer');
            if (cont) cont.querySelectorAll('.ring-thumb').forEach(i => i.classList.remove('active'));
            return;
        }
        if (typeof TokenPresets !== 'undefined' && TokenPresets.loadSingleRing) {
            TokenPresets.loadSingleRing(
                st.ring_file,
                st.mask_file || null,
                st.top_file || null,
                typeof st.ring_scale === 'number' ? st.ring_scale : 1
            );
            const cont = $('ringsContainer');
            if (cont) {
                cont.querySelectorAll('.ring-thumb').forEach(i => {
                    i.classList.toggle('active', i.dataset.ringFile === st.ring_file);
                });
            }
        }
    },

    async _applyFromServer(sid, force) {
        this._inFlight = true;
        this._applying = true;
        try {
            const resp = await this._fetchAgent(`/agent/v1/sessions/${encodeURIComponent(sid)}/sync_state`);
            if (!resp.ok) {
                if (resp.status === 404 && this._sid === sid) this.detachLocal();
                return;
            }
            const st = await resp.json();
            const isNewSession = this._sid !== sid;
            const needImg = isNewSession || st.img_rev !== this._imgRev || !state.userImageOriginal;
            const needCMask = isNewSession || st.canvas_mask_rev !== this._cMaskRev;
            const needIMask = isNewSession || st.image_mask_rev !== this._iMaskRev;

            if (needImg) {
                if (typeof TokenCanvas !== 'undefined') {
                    TokenCanvas._invalidatePendingStrokes();
                    TokenCanvas._clearCanvasCache();
                }
                urlManager.revoke('userImage');
                urlManager.revoke('agentSyncOrig');
                urlManager.revoke('agentSyncCutout');

                const origLoaded = await this._loadImageLayer(sid, 'original', 'agentSyncOrig');
                let cutoutLoaded = null;
                if (st.has_cutout) {
                    cutoutLoaded = await this._loadImageLayer(sid, 'cutout', 'agentSyncCutout');
                }
                state.userImageOriginal = origLoaded.img;
                state.userImageWithoutBg = cutoutLoaded ? cutoutLoaded.img : null;
                state.userImageUrl = (cutoutLoaded && !st.showing_original) ? cutoutLoaded.url : origLoaded.url;
                state.backgroundRemoved = !!st.bg_removed;
                state.showingOriginal = !!st.showing_original;
                state.userImage = (!state.showingOriginal && state.userImageWithoutBg)
                    ? state.userImageWithoutBg
                    : state.userImageOriginal;
                state.currentFilePath = null;
                state.tokenFileName = st.source_label || 'token';
                const fnInput = $('tokenFileName');
                if (fnInput) fnInput.value = state.tokenFileName;
                if (typeof minScalePercent === 'function') {
                    state.minScale = minScalePercent(Math.max(state.userImage.width, state.userImage.height)) / 100;
                }
                if (typeof TokenEditor !== 'undefined') {
                    TokenEditor.updateDropzoneLabel();
                    TokenEditor.updateRemoveBgButton();
                    const undoBar = $('undoBar');
                    if (undoBar) undoBar.style.display = 'flex';
                }
                const swr = $('saveWithoutRing');
                const swri = $('saveWithRing');
                if (swr) swr.disabled = false;
                if (swri) swri.disabled = false;
                if (window.syncZoomControlsState) syncZoomControlsState();
                if (typeof PortraitGenerator !== 'undefined' && PortraitGenerator.canvas) {
                    PortraitGenerator.onImageLoaded();
                    if (PortraitGenerator.setCutoutAvailable) {
                        PortraitGenerator.setCutoutAvailable(!!state.userImageWithoutBg);
                    }
                }
            } else if (state.showingOriginal !== !!st.showing_original) {
                state.showingOriginal = !!st.showing_original;
                state.userImage = (!state.showingOriginal && state.userImageWithoutBg)
                    ? state.userImageWithoutBg
                    : state.userImageOriginal;
                if (typeof TokenEditor !== 'undefined') TokenEditor.updateRemoveBgButton();
                if (typeof TokenCanvas !== 'undefined') {
                    TokenCanvas._compositedImageDirty = true;
                    TokenCanvas._ccDirty = true;
                }
            }

            if (typeof TokenCanvas !== 'undefined') {
                TokenCanvas.createMask();
                TokenCanvas.createImageMask();
            }

            if (needCMask && state.maskCanvas) {
                await this._applyMaskLayer(sid, 'canvas_mask', state.maskCanvas, true);
            }
            if (needIMask && state.imageMaskCanvas) {
                await this._applyMaskLayer(sid, 'image_mask', state.imageMaskCanvas, false);
            }

            if (state.userImage && st.transform) {
                const maxDim = Math.max(state.userImage.width, state.userImage.height) || 1;
                state.imageScale = (st.transform.scale || 1) * (1024 / 3) / maxDim;
                state.imageX = (st.transform.offset_x || 0) / 3;
                state.imageY = (st.transform.offset_y || 0) / 3;
                state.imageRotation = st.transform.rotation || 0;
            }

            this._syncRingUI(st);
            this._syncEffectsUI(st.effects);

            if (typeof TokenCanvas !== 'undefined') {
                TokenCanvas.updateScaleUI();
                TokenCanvas.updateRotationUI();
                TokenCanvas.invalidateEffectsCache();
                TokenCanvas._zonesDirty = true;
                TokenCanvas._dragLayer = null;
                if (isNewSession && force) {
                    TokenCanvas.resetView();
                }
                TokenCanvas.render();
            }

            if (isNewSession && typeof TokenHistory !== 'undefined') {
                TokenHistory.init();
            }

            this._sid = sid;
            this._rev = st.rev || 0;
            this._imgRev = st.img_rev || 0;
            this._cMaskRev = st.canvas_mask_rev || 0;
            this._iMaskRev = st.image_mask_rev || 0;
        } catch (err) {
            console.warn('[agentSync] apply error:', err);
        } finally {
            this._applying = false;
            this._inFlight = false;
            if (this._pending) {
                const next = this._pending;
                this._pending = null;
                this.onServerPush(next.sid, next.rev, next.force);
            }
        }
    },

    schedulePushToAgent(opts) {
        if (this.isDisabled() || this._applying || !this._sid || !state.userImage) return;
        if (opts && opts.canvasMask) this._dirtyCanvasMask = true;
        if (opts && opts.imageMask) this._dirtyImageMask = true;
        if (this._pushTimer) clearTimeout(this._pushTimer);
        this._pushTimer = setTimeout(() => {
            this._pushTimer = null;
            this._flushPushToAgent();
        }, 250);
    },

    async _flushPushToAgent() {
        const sid = this._sid;
        if (!sid || this._applying || !state.userImage) return;
        const sendCMask = this._dirtyCanvasMask;
        const sendIMask = this._dirtyImageMask;
        this._dirtyCanvasMask = false;
        this._dirtyImageMask = false;

        try {
            if (sendCMask && state.maskCanvas) {
                const blob = await new Promise(resolve => state.maskCanvas.toBlob(resolve, 'image/png'));
                if (blob) {
                    const r = await this._fetchAgent(
                        `/agent/v1/sessions/${encodeURIComponent(sid)}/gui_mask?target=canvas`,
                        { method: 'POST', headers: { 'Content-Type': 'image/png' }, body: blob }
                    );
                    if (r.ok) {
                        const d = await r.json();
                        this._rev = Math.max(this._rev, d.rev || 0);
                        this._cMaskRev = Math.max(this._cMaskRev, d.canvas_mask_rev || 0);
                    }
                }
            }
            if (sendIMask && state.imageMaskCanvas) {
                const blob = await new Promise(resolve => state.imageMaskCanvas.toBlob(resolve, 'image/png'));
                if (blob) {
                    const r = await this._fetchAgent(
                        `/agent/v1/sessions/${encodeURIComponent(sid)}/gui_mask?target=image`,
                        { method: 'POST', headers: { 'Content-Type': 'image/png' }, body: blob }
                    );
                    if (r.ok) {
                        const d = await r.json();
                        this._rev = Math.max(this._rev, d.rev || 0);
                        this._iMaskRev = Math.max(this._iMaskRev, d.image_mask_rev || 0);
                    }
                }
            }

            const maxDim = Math.max(state.userImage.width, state.userImage.height) || 1;
            const ds = (typeof AppConfig !== 'undefined' && AppConfig.dropShadow) ? AppConfig.dropShadow : {};
            const cc = (typeof AppConfig !== 'undefined' && AppConfig.colorCorrection) ? AppConfig.colorCorrection : {};
            const payload = {
                scale: state.imageScale * maxDim / (1024 / 3),
                rotation: state.imageRotation || 0,
                offset_x: (state.imageX || 0) * 3,
                offset_y: (state.imageY || 0) * 3,
                showing_original: !!state.showingOriginal,
                protection_enabled: !!state.protectionEnabled,
                ring: state.activeTokenatraRingFile || null,
                effects: {
                    shadow: {
                        enabled: !!state.dropShadowEnabled,
                        angle: ds.angle ?? 135,
                        distance: ds.distance ?? 7,
                        blur: ds.blur ?? 3,
                        opacity: ds.opacity ?? 0.4,
                    },
                    color: {
                        enabled: !!state.colorCorrectionEnabled,
                        saturation: cc.saturation ?? 0,
                        lightness: cc.lightness ?? 0,
                        contrast: cc.contrast ?? 0,
                    },
                },
            };
            const res = await this._fetchAgent(
                `/agent/v1/sessions/${encodeURIComponent(sid)}/gui_update`,
                {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(payload),
                }
            );
            if (res.ok) {
                const d = await res.json();
                this._rev = Math.max(this._rev, d.rev || 0);
            }
        } catch (err) {
            console.warn('[agentSync] push error:', err);
        }
    },
};
