var TokenCanvas = {
    canvas: null,
    ctx: null,
    wrapper: null,
    _debouncedSave: null,

    eraserCursor: null,
    eraserBrush: null,
    _imageBrushCache: null,
    _imageBrushCacheSize: -1,

    _compositedImageCache: null,
    _compositedImageDirty: true,

    _ccDirty: true,
    _ccImageCache: null,
    _ccCacheKey: null,
    _shadowCache: null,
    _shadowDirty: true,
    _isErasing: false,
    _worker: null,
    _workerInFlight: 0,
    // Очереди раздельные: смена инструмента (eraser↔mask) при занятом воркере
    // не должна штамповать точки одного режима кистью другого
    _workerQueues: { pink: [], image: [] },
    _workerBatchTimer: null,
    _lastBrushSent: null,

    _strokeDirtyRect: null,
    _strokeFullDirty: false,
    _strokeChanged: false,

    _zonesCanvas: null,
    _zonesDirty: true,

    _protectionOverlayCache: null,
    _protectionOverlayDirty: true,

    _zoneInvImgMask: null,
    _zoneBlueTemp: null,
    _zoneInvScaled: null,
    _zoneInvMask: null,
    _zonePinkLayer: null,
    _zoneHelpersDirty: true,

    _cachedRect: null,
    _rectDirty: true,

    isErasing: false,
    pendingErasePoints: [],
    eraseAnimationId: null,

    get internalSize() {
        return CONFIG.SCALE_SIZES[state.canvasScale] || CONFIG.BASE_SIZE;
    },

    _renderRafId: null,
    _maskGen: 0,

    requestRender: function() {
        var self = this;
        if (this._renderRafId) return;
        this._renderRafId = requestAnimationFrame(function() {
            self._renderRafId = null;
            self.render();
        });
    },

    // Изменения трансформации (масштаб/поворот) во время штриха требуют
    // полного кадра — dirty-патчи его не покроют
    _forceFullRender: function() {
        if (this._isErasing) this._strokeFullDirty = true;
        this._strokeDirtyRect = null;
    },

    init: function() {
        this.canvas = $('tokenCanvas');
        this.canvas.addEventListener('contextmenu', function(e) { e.preventDefault(); });
        this.ctx = this.canvas.getContext('2d', { alpha: true });
        this.wrapper = $('canvasWrapper');
        this.area = this.canvas.closest('.canvas-area') || this.wrapper;
        this.eraserCursor = $('eraserCursor');

        this._applyCanvasSize();
        this._fixCanvasDisplay();

        this._tempCanvas = document.createElement('canvas');
        this._tempCanvas.width = this.internalSize;
        this._tempCanvas.height = this.internalSize;
        this._tempCtx = this._tempCanvas.getContext('2d');

        this._cachedRect = null;
        this._rectDirty = true;

        this._debouncedSave = debounce(function() { TokenHistory.save(); }, CONFIG.DEBOUNCE_DELAY);

        this._initWorker();

        this.createMask();
        this.createImageMask();
        this.createEraserBrush();
        this.setupEvents();

        window.addEventListener('resize', this._fixCanvasDisplay.bind(this));
    },

    _initWorker: function() {
        try {
            var workerUrl = '/static/js/eraserWorker.js';
            if (window.__APP_VERSION) workerUrl += '?v=' + window.__APP_VERSION;
            this._worker = new Worker(workerUrl);
            this._lastBrushSent = null;
            this._workerQueues.pink = [];
            this._workerQueues.image = [];
            this._workerInFlight = 0;
            this._worker.onmessage = this._onWorkerBrushDone.bind(this);
            this._worker.onerror = function() {
                // Сбрасываем очереди, чтобы штрихи не зависли и не применились позже
                TokenCanvas._worker = null;
                TokenCanvas._workerInFlight = 0;
                TokenCanvas._workerQueues.pink = [];
                TokenCanvas._workerQueues.image = [];
                if (TokenCanvas._workerBatchTimer) {
                    clearTimeout(TokenCanvas._workerBatchTimer);
                    TokenCanvas._workerBatchTimer = null;
                }
            };
            // Воркер хранит альфа-зеркала масок: при (пере)создании отправляем
            // текущее состояние, чтобы пачки не несли пиксели маски
            if (state.maskCanvas) this._pushMaskToWorker(true);
            if (state.imageMaskCanvas) this._pushMaskToWorker(false);
        } catch(e) {
            this._worker = null;
        }
    },

    // Полный снапшот альфы маски в воркер. Дорогой (полный getImageData),
    // но вызывается только при загрузке/undo/сбросе/пресете — не в горячем пути
    _pushMaskToWorker: function(pink) {
        if (!this._worker) return;
        var src = pink ? state.maskCanvas : state.imageMaskCanvas;
        var mode = pink ? 'pink' : 'image';
        if (!src) {
            this._worker.postMessage({ type: 'setMask', mode: mode, alphaData: null, width: 0, height: 0 });
            return;
        }
        var w = src.width;
        var h = src.height;
        var d = src.getContext('2d').getImageData(0, 0, w, h).data;
        var n = w * h;
        var alpha = new Uint8Array(n);
        for (var i = 0; i < n; i++) alpha[i] = d[i * 4 + 3];
        this._worker.postMessage({ type: 'setMask', mode: mode, alphaData: alpha, width: w, height: h }, [alpha.buffer]);
    },

    _onWorkerBrushDone: function(e) {
        this._workerInFlight--;

        var d = e.data;

        // Сначала кормим воркер следующей пачкой — он не должен простаивать,
        // пока главный поток применяет результат (putImageData/рендер)
        this._flushWorkerBatch(true);

        // Ответ устаревшего поколения (после undo/сброса/загрузки нового изображения) — отбрасываем
        if (d.gen !== this._maskGen) return;
        if (!d.maskData) return;

        var pink = d.mode === 'pink';
        var target = pink ? state.maskCanvas : state.imageMaskCanvas;
        if (!target) return;
        if (d.regionX + d.regionWidth > target.width ||
            d.regionY + d.regionHeight > target.height) return;

        // Штрих ничего не изменил (restore поверх белого / стирание поверх
        // прозрачного) — маска и канвас уже актуальны, рендер не нужен
        if (d.changed) {
            var ctx = target.getContext('2d');
            var imageData = new ImageData(
                new Uint8ClampedArray(d.maskData),
                d.regionWidth,
                d.regionHeight
            );
            ctx.putImageData(imageData, d.regionX, d.regionY);

            if (!pink) {
                // Композит пересчитывается только в изменённом регионе вместо полной
                // перерисовки изображения целиком — главная цена кадра при стирании
                this._patchCompositedRegion(d.regionX, d.regionY, d.regionWidth, d.regionHeight);
            }
            this._zonesDirty = true;
            this._shadowDirty = true;
            this._strokeChanged = true;

            if (d.dirtyRect) {
                this._strokeDirtyRect = this._unionDirtyRect(this._strokeDirtyRect, d.dirtyRect);
            }

            this.requestRender();
        }
    },

    _flushWorkerBatch: function(force) {
        if (this._workerBatchTimer) {
            clearTimeout(this._workerBatchTimer);
            this._workerBatchTimer = null;
        }
        if (!this._worker || (!state.imageMaskCanvas && !state.maskCanvas)) {
            // Воркер недоступен — прогоняем накопленные точки фолбэком
            var modes = ['pink', 'image'];
            for (var m = 0; m < modes.length; m++) {
                var pts = this._workerQueues[modes[m]];
                this._workerQueues[modes[m]] = [];
                for (var i = 0; i < pts.length; i++) {
                    var p = pts[i];
                    if (p.mode === 'pink') {
                        this.applyEraserBrushToPinkMask(p.cx, p.cy, p.restore);
                    } else {
                        this._fallbackBrushToImageMask(p.cx, p.cy, p.restore);
                    }
                }
            }
            return;
        }
        // Глубина пайплайна 2: воркер обрабатывает следующую пачку, пока
        // главный поток применяет результат предыдущей
        while (this._workerInFlight < 2) {
            // Маски разные, порядок между режимами не важен — берём любую непустую
            var mode = this._workerQueues.pink.length > 0 ? 'pink' :
                       (this._workerQueues.image.length > 0 ? 'image' : null);
            if (!mode) break;
            var queue = this._workerQueues[mode];
            var batch = queue.splice(0, Math.min(queue.length, 64));
            this._workerInFlight++;
            this._sendWorkerBatch(mode, batch);
            if (!this._worker) break; // onerror внутри send-пути маловероятен, но дешёвая страховка
        }
    },

    _ensureWorkerBrush: function(brushCanvas) {
        // В воркере ОДИН слот кисти (self._brush), поэтому кэш отправки тоже
        // один общий: при чередовании розовый/изображение кисть обязана
        // пересылаться, иначе штрихи уйдут кистью чужого режима.
        if (!this._worker || this._lastBrushSent === brushCanvas) return;
        // Свежий getImageData на каждую отправку: буфер передаётся воркеру
        // (detach), закэшированный ImageData повторно использовать нельзя
        var brushCtx = brushCanvas.getContext('2d');
        var buf = brushCtx.getImageData(0, 0, brushCanvas.width, brushCanvas.height).data.buffer;
        this._worker.postMessage({
            type: 'setBrush',
            brushData: buf,
            brushWidth: brushCanvas.width,
            brushHeight: brushCanvas.height
        }, [buf]);
        this._lastBrushSent = brushCanvas;
    },

    _strokeBrushRadius: function(pink) {
        if (pink) return this.eraserBrush.fullSize / 2 + 2;
        var ib = this._getImageBrush();
        if (!ib) return this.eraserBrush.fullSize / 2 + 2;
        return (ib.fullSize / 2 + 2) * (state.imageScale * (this.internalSize / 1024));
    },

    _unionDirtyRect: function(a, b) {
        if (!a) return { x: b.x, y: b.y, w: b.w, h: b.h };
        var x1 = Math.min(a.x, b.x);
        var y1 = Math.min(a.y, b.y);
        var x2 = Math.max(a.x + a.w, b.x + b.w);
        var y2 = Math.max(a.y + a.h, b.y + b.h);
        return { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
    },

    _markStrokeDirty: function(cx, cy, pink) {
        var r = this._strokeBrushRadius(pink);
        this._strokeDirtyRect = this._unionDirtyRect(this._strokeDirtyRect, { x: cx - r, y: cy - r, w: r * 2, h: r * 2 });
    },

    _sendWorkerBatch: function(mode, batch) {
        var pink = mode === 'pink';
        var maskCanvas = pink ? state.maskCanvas : state.imageMaskCanvas;
        if (!maskCanvas) { this._workerInFlight--; return; }

        var brush = pink ? this.eraserBrush : this._getImageBrush();
        if (!brush) { this._workerInFlight--; return; }
        this._ensureWorkerBrush(brush.canvas);

        var mw = maskCanvas.width;
        var mh = maskCanvas.height;
        var pad = 2;

        var internalSize = this.internalSize;
        var scale = internalSize / 1024;
        var effectiveScale = pink ? 1 : state.imageScale * scale;
        var rotation = pink ? 0 : state.imageRotation;

        // Лимит площади региона на пачку: длинный быстрый штрих иначе сливается
        // в один гигантский прямоугольник и ответ воркера/putImageData
        // становятся O(длина штриха²). Пачка делится на компактные кластеры —
        // стоимость каждого сообщения предсказуема
        var maxArea = Math.max(brush.fullSize * brush.fullSize * 6, 262144);

        // Бюджет кластера считается по объединению bbox (регион воркер
        // вычисляет сам — главный поток маску не читает)
        var minX = mw, minY = mh, maxX = 0, maxY = 0;
        var packed = [];
        var dirtyRect = null;
        var brushRadius = this._strokeBrushRadius(pink);
        var i = 0;
        for (; i < batch.length; i++) {
            var task = batch[i];
            var drawX, drawY;
            if (pink) {
                drawX = Math.round(task.cx - brush.fullSize / 2);
                drawY = Math.round(task.cy - brush.fullSize / 2);
            } else {
                var imgPos = this._canvasPosToImagePos(task.cx, task.cy);
                if (!imgPos) continue;
                drawX = Math.round(imgPos.x - brush.fullSize / 2);
                drawY = Math.round(imgPos.y - brush.fullSize / 2);
            }
            var rx1 = Math.max(0, drawX - pad);
            var ry1 = Math.max(0, drawY - pad);
            var rx2 = Math.min(mw, drawX + brush.fullSize + pad);
            var ry2 = Math.min(mh, drawY + brush.fullSize + pad);
            if (rx2 <= rx1 || ry2 <= ry1) continue;

            var nx1 = minX < rx1 ? minX : rx1;
            var ny1 = minY < ry1 ? minY : ry1;
            var nx2 = maxX > rx2 ? maxX : rx2;
            var ny2 = maxY > ry2 ? maxY : ry2;
            // Одиночный штамп всегда влезает в лимит, поэтому после break
            // пачка не пуста и воркер получит сообщение → хвост не зависнет
            if (packed.length > 0 && (nx2 - nx1) * (ny2 - ny1) > maxArea) break;
            minX = nx1; minY = ny1; maxX = nx2; maxY = ny2;
            packed.push(Math.round(task.cx), Math.round(task.cy), drawX, drawY, task.restore ? 1 : 0);

            // Грязный регион в координатах канваса для инкрементального рендера
            dirtyRect = this._unionDirtyRect(dirtyRect, { x: task.cx - brushRadius, y: task.cy - brushRadius, w: brushRadius * 2, h: brushRadius * 2 });
        }
        if (i < batch.length) {
            // Остаток возвращается в начало очереди — уйдёт следующим сообщением
            var q = this._workerQueues[mode];
            for (var j = batch.length - 1; j >= i; j--) q.unshift(batch[j]);
        }
        if (packed.length === 0) { this._workerInFlight--; return; }

        // Штрихи упакованы в Float64Array: [cx, cy, drawX, drawY, flags]×N —
        // компактное сообщение без мусора для GC, cx/cy без потери точности
        // (от них зависит сэмпл защиты)
        var strokeBuf = new Float64Array(packed).buffer;

        this._worker.postMessage({
            type: 'applyBrushBatch',
            id: Date.now(),
            gen: this._maskGen,
            mode: mode,
            strokeData: strokeBuf,
            strokeCount: packed.length / 5,
            effectiveScale: effectiveScale,
            imageRotation: rotation,
            dirtyRect: dirtyRect
        }, [strokeBuf]);
    },

    _fixCanvasDisplay: function() {
        var area = this.canvas.parentElement;
        if (!area) return;
        var w = area.clientWidth;
        var h = area.clientHeight;
        var side = Math.min(w, h);
        this.canvas.style.width = side + 'px';
        this.canvas.style.height = side + 'px';
        this.canvas.style.position = 'absolute';
        this.canvas.style.left = Math.round((w - side) / 2) + 'px';
        this.canvas.style.top = Math.round((h - side) / 2) + 'px';
        var po = $('presetOverlay');
        if (po && state.presetOverlayActive) {
            po.style.width = side + 'px';
            po.style.height = side + 'px';
            po.style.left = this.canvas.style.left;
            po.style.top = this.canvas.style.top;
        }
        this._rectDirty = true;
    },

    _applyCanvasSize: function() {
        var size = this.internalSize;
        this.canvas.width = size;
        this.canvas.height = size;
        if (this._tempCanvas) {
            this._tempCanvas.width = size;
            this._tempCanvas.height = size;
        }
        this._compositedImageDirty = true;
        this._zonesDirty = true;
        this._rectDirty = true;
        this._ccDirty = true;
        this._shadowDirty = true;
    },

    setCanvasScale: function(scale) {
        if (![1, 2, 3].includes(scale)) return;
        state.canvasScale = scale;
        AppConfig.setCanvasScale(scale);
        var sel = $('canvasScaleSelect');
        if (sel) sel.value = String(scale);
        this._applyCanvasSize();
        this._fixCanvasDisplay();
        this.createMask();
        this.createEraserBrush();
        TokenPresets.reloadProtectionMaskForScale();
        this._forceFullRender();
        this.render();
        toast(I18n.t('Масштаб канваса: ' + scale + '×'));
    },

    createEraserBrush: function() {
        this.updateEraserBrush(state.eraserSize);
    },

    updateEraserBrush: function(size) {
        var internalScale = this.internalSize / 1024;
        var scaledRadius = Math.ceil(size * internalScale);
        var brushSize = scaledRadius * 2 + 4;

        var brush = document.createElement('canvas');
        brush.width = brushSize;
        brush.height = brushSize;
        var ctx = brush.getContext('2d');

        var cx = brushSize / 2;
        var cy = brushSize / 2;

        var gradient = ctx.createRadialGradient(cx, cy, 0, cx, cy, scaledRadius);
        gradient.addColorStop(0, 'rgba(255,255,255,1)');
        gradient.addColorStop(0.7, 'rgba(255,255,255,1)');
        gradient.addColorStop(0.85, 'rgba(255,255,255,0.5)');
        gradient.addColorStop(1, 'rgba(255,255,255,0)');

        ctx.fillStyle = gradient;
        ctx.fillRect(0, 0, brushSize, brushSize);

        this.eraserBrush = { canvas: brush, size: scaledRadius, fullSize: brushSize };
        this._imageBrushCache = null;
        this._imageBrushCacheSize = -1;
        this._lastBrushSent = null;
    },

    _getImageBrush: function() {
        if (!state.userImage || !this.eraserBrush) return null;

        var internalScale = this.internalSize / 1024;
        var effectiveScale = state.imageScale * internalScale;

        if (this._imageBrushCache && this._imageBrushCacheSize === effectiveScale) {
            return this._imageBrushCache;
        }

        var brushRadiusInImagePx = this.eraserBrush.size / effectiveScale;
        var brushSize = Math.ceil(brushRadiusInImagePx * 2 + 4);

        var brush = document.createElement('canvas');
        brush.width = brushSize;
        brush.height = brushSize;
        var ctx = brush.getContext('2d');

        var cx = brushSize / 2;
        var cy = brushSize / 2;

        var gradient = ctx.createRadialGradient(cx, cy, 0, cx, cy, brushRadiusInImagePx);
        gradient.addColorStop(0, 'rgba(255,255,255,1)');
        gradient.addColorStop(0.7, 'rgba(255,255,255,1)');
        gradient.addColorStop(0.85, 'rgba(255,255,255,0.5)');
        gradient.addColorStop(1, 'rgba(255,255,255,0)');

        ctx.fillStyle = gradient;
        ctx.fillRect(0, 0, brushSize, brushSize);

        this._imageBrushCache = { canvas: brush, size: brushRadiusInImagePx, fullSize: brushSize };
        this._imageBrushCacheSize = effectiveScale;
        this._lastBrushSent = null;
        return this._imageBrushCache;
    },

    invalidateProtectionOverlay: function() {
        this._protectionOverlayDirty = true;
    },

    syncProtectionToWorker: function() {
        if (!this._worker) return;
        // Альфа защиты уже посчитана при построении erasableCanvas
        // (buildProtectionCanvasFromImg) — второй полный проход не нужен
        var alpha = state.protectionAlpha;
        if (!alpha) {
            this._worker.postMessage({ type: 'setProtection', protData: null, protSize: 0 });
            return;
        }
        // Без transfer: postMessage клонирует массив, state сохраняет владение
        this._worker.postMessage({ type: 'setProtection', protData: alpha, protSize: state.protectionSize });
    },

    invalidateEffectsCache: function() {
        this._shadowDirty = true;
        // Оверлей стёртых зон зависит от позиции/поворота/масштаба изображения
        this._zonesDirty = true;
    },

    invalidateAllCaches: function() {
        this._ccDirty = true;
        this._shadowDirty = true;
        this._compositedImageDirty = true;
    },

    // Цветокоррекция применяется один раз к копии изображения, а не на каждый
    // патч композита: ctx.filter на drawImage — самая дорогая часть стирания
    // при включённой КС. Инвалидация — через _ccDirty (слайдеры КС, тумблер,
    // смена изображения уже выставляют его)
    _getColorCorrectedImage: function() {
        var src = state.userImage;
        if (!src) return null;
        if (!state.colorCorrectionEnabled) {
            this._ccImageCache = null;
            this._ccCacheKey = null;
            return src;
        }
        if (this._ccImageCache && this._ccCacheKey === src && !this._ccDirty) {
            return this._ccImageCache;
        }
        var w = src.width;
        var h = src.height;
        if (!this._ccImageCache || this._ccImageCache.width !== w || this._ccImageCache.height !== h) {
            this._ccImageCache = document.createElement('canvas');
            this._ccImageCache.width = w;
            this._ccImageCache.height = h;
        }
        var ctx = this._ccImageCache.getContext('2d');
        ctx.clearRect(0, 0, w, h);
        ctx.filter = colorCorrectionFilter();
        ctx.drawImage(src, 0, 0);
        ctx.filter = 'none';
        this._ccCacheKey = src;
        this._ccDirty = false;
        return this._ccImageCache;
    },

    _freeEffectCaches: function() {
        var keys = ['_zoneInvMask', '_zoneBlueTemp', '_zoneInvScaled', '_zonePinkLayer',
                    '_zonesCanvas', '_zoneInvImgMask', '_shadowCache', '_protectionOverlayCache'];
        for (var i = 0; i < keys.length; i++) {
            var k = keys[i];
            if (this[k]) { this[k].width = 1; this[k].height = 1; this[k] = null; }
        }
        this._zonesDirty = true;
        this._zoneHelpersDirty = true;
        this._protectionOverlayDirty = true;
    },

    _patchCompositedRegion: function(x, y, w, h) {
        if (!state.userImage || !state.imageMaskCanvas || !this._compositedImageCache) {
            this._compositedImageDirty = true;
            return;
        }
        var iw = state.userImage.width;
        var ih = state.userImage.height;
        var rx = Math.max(0, Math.min(x, iw));
        var ry = Math.max(0, Math.min(y, ih));
        var rx2 = Math.min(iw, x + w);
        var ry2 = Math.min(ih, y + h);
        var rw = rx2 - rx;
        var rh = ry2 - ry;
        if (rw <= 0 || rh <= 0) return;

        var ctx = this._compositedImageCache.getContext('2d');
        ctx.save();
        ctx.beginPath();
        ctx.rect(rx, ry, rw, rh);
        ctx.clip();
        ctx.clearRect(rx, ry, rw, rh);
        // Цветокоррекция уже вшита в кэш — фильтр на патч не применяется
        ctx.drawImage(this._getColorCorrectedImage(), rx, ry, rw, rh, rx, ry, rw, rh);
        ctx.globalCompositeOperation = 'destination-in';
        ctx.drawImage(state.imageMaskCanvas, rx, ry, rw, rh, rx, ry, rw, rh);
        ctx.globalCompositeOperation = 'source-over';
        ctx.restore();
    },

    _invalidateComposite: function() {
        this._compositedImageDirty = true;
        this._zonesDirty = true;
        this._zoneHelpersDirty = true;
        this._shadowDirty = true;
    },

    _invalidateZones: function() {
        this._zonesDirty = true;
    },

    _buildCompositedImage: function() {
        if (!state.userImage) return;
        if (!this._compositedImageDirty && this._compositedImageCache) return;

        var iw = state.userImage.width;
        var ih = state.userImage.height;

        if (!this._compositedImageCache || this._compositedImageCache.width !== iw || this._compositedImageCache.height !== ih) {
            this._compositedImageCache = document.createElement('canvas');
            this._compositedImageCache.width = iw;
            this._compositedImageCache.height = ih;
        }

        var ctx = this._compositedImageCache.getContext('2d');
        ctx.clearRect(0, 0, iw, ih);

        // Источник уже с цветокоррекцией (кэш) — фильтр не нужен
        var ccSrc = this._getColorCorrectedImage();
        if (ccSrc) ctx.drawImage(ccSrc, 0, 0);

        if (state.imageMaskCanvas) {
            ctx.globalCompositeOperation = 'destination-in';
            ctx.drawImage(state.imageMaskCanvas, 0, 0);
            ctx.globalCompositeOperation = 'source-over';
        }

        this._compositedImageDirty = false;
    },

    updateEraserCursor: function(e) {
        if (!this.eraserCursor) return;
        var internalScale = this.internalSize / 1024;
        var displaySide = parseFloat(this.canvas.style.width);
        if (!displaySide || !isFinite(displaySide)) {
            if (this._cachedRect && this._cachedRect.width) {
                displaySide = this._cachedRect.width;
            } else {
                var r = this.canvas.getBoundingClientRect();
                displaySide = r.width;
            }
        }
        if (!displaySide) return;
        var pixelsPerInternalUnit = (displaySide / this.internalSize) * state.viewZoom;
        var displayDiameter = Math.round(state.eraserSize * internalScale * 2 * pixelsPerInternalUnit * 100) / 100;
        var color = state.currentEraserMode === 'blue'
            ? 'rgba(100, 180, 255, 0.85)'
            : 'rgba(255, 100, 180, 0.85)';
        var st = this.eraserCursor.style;
        // Позиция — через transform (композитор, без layout/paint); размер и
        // цвет меняются только при зуме/смене режима — не дёргаем стили
        // на каждом mousemove
        st.transform = 'translate3d(' + e.clientX + 'px,' + e.clientY + 'px,0) translate(-50%,-50%)';
        if (displayDiameter !== this._cursorLastSize) {
            st.width = displayDiameter + 'px';
            st.height = displayDiameter + 'px';
            this._cursorLastSize = displayDiameter;
        }
        if (color !== this._cursorLastColor) {
            st.borderColor = color;
            this._cursorLastColor = color;
        }
    },

    showEraserCursor: function() {
        if (this.eraserCursor && state.userImage) this.eraserCursor.classList.add('visible');
    },

    hideEraserCursor: function() {
        if (this.eraserCursor) this.eraserCursor.classList.remove('visible');
    },

    setToolCursorMode: function(active) {
        if (!this.canvas) return;
        if (active) {
            this.canvas.classList.add('eraser-mode');
            if (this.area) this.area.classList.add('eraser-active');
        } else {
            this.canvas.classList.remove('eraser-mode');
            if (this.area) this.area.classList.remove('eraser-active');
        }
    },

    createMask: function() {
        var size = this.internalSize;
        if (state.maskCanvas && state.maskCanvas.width === size) return;

        var newMask = document.createElement('canvas');
        newMask.width = size;
        newMask.height = size;
        var newCtx = newMask.getContext('2d');

        if (state.maskCanvas) {
            newCtx.drawImage(state.maskCanvas, 0, 0, size, size);
        } else {
            newCtx.fillStyle = 'white';
            newCtx.fillRect(0, 0, size, size);
        }

        state.maskCanvas = newMask;
        this._pushMaskToWorker(true);
    },

    createImageMask: function() {
        if (!state.userImage) return;
        if (state.imageMaskCanvas && state.imageMaskCanvas.width === state.userImage.width && state.imageMaskCanvas.height === state.userImage.height) return;

        var newMask = document.createElement('canvas');
        newMask.width = state.userImage.width;
        newMask.height = state.userImage.height;
        var ctx = newMask.getContext('2d');

        if (state.imageMaskCanvas) {
            ctx.drawImage(state.imageMaskCanvas, 0, 0, newMask.width, newMask.height);
        } else {
            ctx.fillStyle = 'white';
            ctx.fillRect(0, 0, newMask.width, newMask.height);
        }

        state.imageMaskCanvas = newMask;
        this._invalidateComposite();
        this._pushMaskToWorker(false);
    },

    render: function() {
        if (!this.ctx) return;
        var size = this.internalSize;
        var scale = size / 1024;

        if (!state.userImage) {
            var overlay = $('canvasOverlay');
            if (overlay) overlay.style.display = 'flex';
            this.canvas.style.display = 'none';
            var area = this.area;
            if (area) { area.classList.add('no-image'); area.style.cursor = ''; }
            return;
        }

        this.canvas.style.display = '';
        var areaEl = this.area;
        if (areaEl) areaEl.classList.remove('no-image');

        var erasing = this._isErasing;
        // Все кадры — в том числе инкрементальные патчи во время штриха —
        // рендерятся с одним качеством сэмплинга 'high'. Смешение medium/high
        // давало видимое расхождение пикселей патча с окружением, которое
        // исчезало на финальном кадре после отпускания кнопки («артефакты
        // ластика»). Стоимость ограничена размером грязного региона — полный
        // канвас во время штриха не перерисовывается.
        // Во время штриха канвас уже содержит актуальный кадр: перерисовываем
        // только грязный регион кисти. Полный кадр нужен лишь в начале штриха
        // и после него.
        var dirtyOnly = erasing && !this._strokeFullDirty;
        if (dirtyOnly && !this._strokeDirtyRect) {
            // Ничего не изменилось — канвас уже актуален, кадр не нужен
            var ov = $('canvasOverlay');
            if (ov) ov.style.display = 'none';
            return;
        }

        if (!dirtyOnly) this.ctx.clearRect(0, 0, size, size);

        if (state.userImage) {
            // Кольцо = область экспорта 1× (internalSize/3), совпадает с
            // границей 1× и кольцом в сохранённом файле. SCALE_SIZES[1]*(size/s3)
            // после перехода на {1536,3072,6144} даёт size/4 — кольцо не совпадало
            // с реальной областью токена 1× (size/3 в экспорте).
            var ringSize = Math.round(size / 3);
            var ringOffset = (size - ringSize) / 2;
            var ringImage = state.ringImages[2048] || state.ringImages[1024];
            if (!dirtyOnly) {
                if (ringImage) {
                    this.ctx.drawImage(ringImage, ringOffset, ringOffset, ringSize, ringSize);
                }
            }

            this._buildCompositedImage();

            var w = state.userImage.width * state.imageScale * scale;
            var h = state.userImage.height * state.imageScale * scale;
            var cx = size / 2 + state.imageX * scale;
            var cy = size / 2 + state.imageY * scale;

            if (dirtyOnly) {
                // Инкрементальный кадр: только область кисти. Прямоугольник —
                // целочисленный: дробный rect даёт субпиксельную выборку маски
                // и шов на границе патча, исчезающий на финальном кадре
                var dr = this._roundDirtyRect(this._strokeDirtyRect);
                this._strokeDirtyRect = null;
                // Кольцо не должно маскироваться: как в полном кадре, это базовая
                // подложка под изображением (destination-in патча стирало бы его так же,
                // как картинку — кольцо исчезало на время штриха и возвращалось на отпускании)
                var cctx = this.ctx;
                cctx.save();
                cctx.beginPath();
                cctx.rect(dr.x, dr.y, dr.w, dr.h);
                cctx.clip();
                cctx.clearRect(dr.x, dr.y, dr.w, dr.h);
                if (ringImage) {
                    cctx.drawImage(ringImage, ringOffset, ringOffset, ringSize, ringSize);
                }
                cctx.restore();

                // Изображение с маской собираем в offscreen-канвасе и кладём
                // поверх базового слоя кольца, чтобы destination-in маски
                // не стирал кольцо в грязном регионе. Патч пиксель-в-пиксель
                // совпадает с финальным кадром (тот же путь, то же качество)
                var tctx = this._tempCtx;
                tctx.imageSmoothingEnabled = true;
                tctx.imageSmoothingQuality = 'high';
                tctx.save();
                tctx.beginPath();
                tctx.rect(dr.x, dr.y, dr.w, dr.h);
                tctx.clip();
                tctx.clearRect(dr.x, dr.y, dr.w, dr.h);
                tctx.translate(cx, cy);
                tctx.rotate(state.imageRotation * Math.PI / 180);
                tctx.drawImage(this._compositedImageCache, -w / 2, -h / 2, w, h);
                tctx.restore();
                tctx.save();
                tctx.beginPath();
                tctx.rect(dr.x, dr.y, dr.w, dr.h);
                tctx.clip();
                tctx.globalCompositeOperation = 'destination-in';
                tctx.drawImage(state.maskCanvas, dr.x, dr.y, dr.w, dr.h, dr.x, dr.y, dr.w, dr.h);
                tctx.globalCompositeOperation = 'source-over';
                tctx.restore();

                cctx.save();
                cctx.beginPath();
                cctx.rect(dr.x, dr.y, dr.w, dr.h);
                cctx.clip();
                cctx.imageSmoothingEnabled = true;
                cctx.imageSmoothingQuality = 'high';
                cctx.drawImage(this._tempCanvas, dr.x, dr.y, dr.w, dr.h, dr.x, dr.y, dr.w, dr.h);
                cctx.restore();

                // Статичные оверлеи (зоны стирания, защита, границы, пример)
                // обязаны переживать патч: раньше он стирал их в зоне штриха,
                // и они «моргали» до финального кадра
                if (state.showErasedZones && this._zonesCanvas) {
                    cctx.save();
                    cctx.beginPath();
                    cctx.rect(dr.x, dr.y, dr.w, dr.h);
                    cctx.clip();
                    cctx.drawImage(this._zonesCanvas, 0, 0);
                    cctx.restore();
                }
                if (state.userImage && state.exampleEnabled && state.exampleImage) {
                    this._drawExampleOverlay(size, dr);
                }
                if (state.showProtectionMask && (state.erasableCanvas || state.protectionAlpha)) {
                    this._renderProtectionMaskOverlay(size, dr);
                }
                if (state.showScaleBorders) {
                    cctx.save();
                    cctx.beginPath();
                    cctx.rect(dr.x, dr.y, dr.w, dr.h);
                    cctx.clip();
                    this._drawScaleBorders(size);
                    cctx.restore();
                }
            } else {
                var tempCtx = this._tempCtx;
                tempCtx.clearRect(0, 0, size, size);
                tempCtx.imageSmoothingEnabled = true;
                tempCtx.imageSmoothingQuality = 'high';

                // Кольцо сюда НЕ рисуем: ниже цельноканвасный destination-in
                // вырезал бы его по розовой маске, тогда как кольцо — базовая
                // подложка на this.ctx (см. выше) и в экспорте renderForSave
                // маскируется только изображение, но не кольцо.

                tempCtx.save();
                tempCtx.translate(cx, cy);
                tempCtx.rotate(state.imageRotation * Math.PI / 180);
                tempCtx.drawImage(this._compositedImageCache, -w / 2, -h / 2, w, h);
                tempCtx.restore();

                tempCtx.globalCompositeOperation = 'destination-in';
                tempCtx.drawImage(state.maskCanvas, 0, 0, size, size, 0, 0, size, size);
                tempCtx.globalCompositeOperation = 'source-over';

                // Полный кадр: в конце штриха или обычный рендер
                this._strokeFullDirty = false;
                this._strokeDirtyRect = null;

                if (erasing) {
                    // Кадр в начале штриха: оверлей зон рисуем из кэша без
                    // пересборки (зоны меняются штрихом — пересборка на каждый
                    // кадр дорога, актуализируется на финальном кадре)
                    this.ctx.imageSmoothingEnabled = true;
                    this.ctx.imageSmoothingQuality = 'high';
                    this.ctx.drawImage(this._tempCanvas, 0, 0);
                    if (state.showErasedZones && this._zonesCanvas) {
                        this.ctx.drawImage(this._zonesCanvas, 0, 0);
                    }
                } else {
                    if (state.effectsEnabled && state.dropShadowEnabled) {
                        if (this._shadowDirty || !this._shadowCache) {
                            // Кэш тени — пониженное разрешение; экспорт не затронут
                            this._shadowCache = createDropShadow(this._tempCanvas, true);
                            this._shadowDirty = false;
                        }
                        if (this._shadowCache) {
                            this.ctx.drawImage(this._shadowCache, 0, 0, this._shadowCache.width, this._shadowCache.height, 0, 0, size, size);
                        }
                    }

                    this.ctx.drawImage(this._tempCanvas, 0, 0);

                    if (state.showErasedZones) {
                        this._renderErasedZonesCached(size, scale, w, h, cx, cy);
                    }
                }
            }
        }

        if (dirtyOnly) {
            // Статичные оверлеи (пример, рамки, защита) уже на канвасе
            var ov2 = $('canvasOverlay');
            if (ov2) ov2.style.display = 'none';
            return;
        }

        if (state.userImage && state.exampleEnabled && state.exampleImage) {
            this._drawExampleOverlay(size, null);
        }

        if (!erasing && state.presetOverlayActive && state.presetOverlayCanvas && !$('presetOverlay')) {
            // Резервный путь без DOM-оверлея: статичная подложка без анимации
            var po = state.presetOverlayCanvas;
            this.ctx.globalAlpha = 0.6;
            this.ctx.drawImage(po, 0, 0, po.width, po.height, 0, 0, size, size);
            this.ctx.globalAlpha = 1;
        }

        // Оверлей защиты статичен во время штриха (маска защиты не меняется
        // стиранием) — рисуем и во время штриха, иначе он «моргает»
        if (state.showProtectionMask && state.erasableCanvas) {
            this._renderProtectionMaskOverlay(size);
        }

        if (state.showScaleBorders) {
            this._drawScaleBorders(size);
        }

        var overlay = $('canvasOverlay');
        if (overlay) overlay.style.display = state.userImage ? 'none' : 'flex';
    },

    // Целочисленный прямоугольник грязного региона: floor по левому-верхнему,
    // ceil по правому-нижнему — дробные координаты дают субпиксельную выборку
    // маски и видимый шов патча, исчезающий на финальном кадре
    _roundDirtyRect: function(r) {
        if (!r) return null;
        var x = Math.floor(r.x);
        var y = Math.floor(r.y);
        var x2 = Math.ceil(r.x + r.w);
        var y2 = Math.ceil(r.y + r.h);
        return { x: x, y: y, w: x2 - x, h: y2 - y };
    },

    _activeExportScale: function(size, scale) {
        var scaleMode = state.saveScaleMode || 'auto';
        if (scaleMode !== 'auto') return parseInt(scaleMode) || 1;
        var imgMaxDim = state.userImage ? Math.max(state.userImage.width, state.userImage.height) : 0;
        var maxDisplayPx = CONFIG.SCALE_SIZES[1] * scale;
        if (imgMaxDim > 0) {
            var displayW = imgMaxDim * (state.imageScale || 1) * scale;
            if (displayW <= maxDisplayPx) return 1;
            if (displayW <= maxDisplayPx * 2) return 2;
            return 3;
        }
        return 1;
    },

    _drawScaleBorders: function(size) {
        var s3 = CONFIG.SCALE_SIZES[3];
        var scale = size / 1024;
        var activeScale = this._activeExportScale(size, scale);

        this.ctx.save();
        this.ctx.setLineDash([20 * (size / s3), 12 * (size / s3)]);
        this.ctx.lineWidth = 6 * (size / s3);

        // Области экспорта 1×/2×/3× во внутренних пикселях канваса:
        // центр m/3 канваса (в экспорте 1×=1/3, 2×=2/3, 3×=весь канвас).
        var b1 = Math.round(size / 3);
        var off1 = (size - b1) / 2;
        this.ctx.strokeStyle = activeScale === 1 ? 'rgba(255,200,0,1)' : 'rgba(255,200,0,0.25)';
        this.ctx.strokeRect(off1, off1, b1, b1);

        var b2 = Math.round(size * 2 / 3);
        var off2 = (size - b2) / 2;
        this.ctx.strokeStyle = activeScale === 2 ? 'rgba(100,200,255,1)' : 'rgba(100,200,255,0.25)';
        this.ctx.strokeRect(off2, off2, b2, b2);

        this.ctx.strokeStyle = activeScale === 3 ? 'rgba(100,255,140,1)' : 'rgba(100,255,140,0.25)';
        this.ctx.strokeRect(3, 3, size - 6, size - 6);
        this.ctx.restore();
    },

    _ensureHelperCanvases: function(size) {
        var helpers = ['_zoneInvMask', '_zoneBlueTemp', '_zoneInvScaled', '_zonePinkLayer'];
        helpers.forEach(function(key) {
            if (!TokenCanvas[key] || TokenCanvas[key].width !== size) {
                TokenCanvas[key] = document.createElement('canvas');
                TokenCanvas[key].width = size;
                TokenCanvas[key].height = size;
                TokenCanvas._zoneHelpersDirty = true;
            }
        });
    },

    _renderErasedZonesCached: function(size, scale, w, h, cx, cy) {
        if (!this._zonesCanvas || this._zonesCanvas.width !== size || this._zonesCanvas.height !== size) {
            this._zonesCanvas = document.createElement('canvas');
            this._zonesCanvas.width = size;
            this._zonesCanvas.height = size;
            this._zonesDirty = true;
        }

        if (this._zonesDirty) {
            this._ensureHelperCanvases(size);

            var zCtx = this._zonesCanvas.getContext('2d');
            zCtx.clearRect(0, 0, size, size);

            if (state.imageMaskCanvas) {
                // Хелперы переиспользуются между перестройками: обязательно
                // сбрасываем composite (после прошлого кадра тут остаётся
                // destination-out/in и fillRect работает как очистка) и
                // очищаем канвасы, иначе оверлей зон пропадает/замирает
                if (!this._zoneInvImgMask || this._zoneInvImgMask.width !== state.imageMaskCanvas.width) {
                    this._zoneInvImgMask = document.createElement('canvas');
                    this._zoneInvImgMask.width = state.imageMaskCanvas.width;
                    this._zoneInvImgMask.height = state.imageMaskCanvas.height;
                }
                var invImgCtx = this._zoneInvImgMask.getContext('2d');
                invImgCtx.globalCompositeOperation = 'source-over';
                invImgCtx.clearRect(0, 0, this._zoneInvImgMask.width, this._zoneInvImgMask.height);
                invImgCtx.fillStyle = 'white';
                invImgCtx.fillRect(0, 0, this._zoneInvImgMask.width, this._zoneInvImgMask.height);
                invImgCtx.globalCompositeOperation = 'destination-out';
                invImgCtx.drawImage(state.imageMaskCanvas, 0, 0);

                zCtx.save();
                zCtx.fillStyle = 'rgba(80, 160, 255, 0.5)';
                zCtx.translate(cx, cy);
                zCtx.rotate(state.imageRotation * Math.PI / 180);
                zCtx.globalCompositeOperation = 'source-over';

                var bCtx = this._zoneBlueTemp.getContext('2d');
                bCtx.globalCompositeOperation = 'source-over';
                bCtx.clearRect(0, 0, size, size);
                bCtx.fillStyle = 'rgba(80, 160, 255, 0.5)';
                bCtx.fillRect(0, 0, size, size);

                var isCtx = this._zoneInvScaled.getContext('2d');
                isCtx.globalCompositeOperation = 'source-over';
                isCtx.clearRect(0, 0, size, size);
                isCtx.save();
                isCtx.translate(cx, cy);
                isCtx.rotate(state.imageRotation * Math.PI / 180);
                isCtx.drawImage(this._zoneInvImgMask, -w / 2, -h / 2, w, h);
                isCtx.restore();

                bCtx.globalCompositeOperation = 'destination-in';
                bCtx.drawImage(this._zoneInvScaled, 0, 0);

                zCtx.restore();
                zCtx.drawImage(this._zoneBlueTemp, 0, 0);
            }

            var invCtx = this._zoneInvMask.getContext('2d');
            invCtx.globalCompositeOperation = 'source-over';
            invCtx.clearRect(0, 0, size, size);
            invCtx.fillStyle = 'white';
            invCtx.fillRect(0, 0, size, size);
            invCtx.globalCompositeOperation = 'destination-out';
            invCtx.drawImage(state.maskCanvas, 0, 0);

            var pCtx = this._zonePinkLayer.getContext('2d');
            pCtx.globalCompositeOperation = 'source-over';
            pCtx.clearRect(0, 0, size, size);
            pCtx.fillStyle = 'rgba(255, 80, 160, 0.5)';
            pCtx.fillRect(0, 0, size, size);
            pCtx.globalCompositeOperation = 'destination-in';
            pCtx.drawImage(this._zoneInvMask, 0, 0);

            zCtx.drawImage(this._zonePinkLayer, 0, 0);
            this._zonesDirty = false;
            this._zoneHelpersDirty = false;
        }

        this.ctx.drawImage(this._zonesCanvas, 0, 0);
    },

    renderForSave: function(withRing) {
        var scaleMode = state.saveScaleMode;
        var usedScale = scaleMode === 'auto' ? this.detectUsedScale() : (parseInt(scaleMode) || 1);

        var qualityBase = state.saveQuality;
        var scale3Size = qualityBase * 3;
        var internalSize = this.internalSize;
        var coordScale = scale3Size / internalSize;

        var fullRender = document.createElement('canvas');
        fullRender.width = scale3Size;
        fullRender.height = scale3Size;
        var fullCtx = fullRender.getContext('2d');
        fullCtx.imageSmoothingEnabled = true;
        fullCtx.imageSmoothingQuality = 'high';

        var ringSize = qualityBase;
        var ringOffset = (scale3Size - ringSize) / 2;
        var ringImage = state.ringImages[2048] || state.ringImages[1024];
        if (withRing && ringImage) {
            fullCtx.drawImage(ringImage, ringOffset, ringOffset, ringSize, ringSize);
        }

        if (state.userImage) {
            // При сохранении цветокоррекция должна быть актуальной, даже если
            // отложенный пересчёт (scheduleEffects) ещё не сработал
            if (state.colorCorrectionEnabled) this._compositedImageDirty = true;
            this._buildCompositedImage();

            var tempCanvas = document.createElement('canvas');
            tempCanvas.width = scale3Size;
            tempCanvas.height = scale3Size;
            var tempCtx = tempCanvas.getContext('2d');
            tempCtx.imageSmoothingEnabled = true;
            tempCtx.imageSmoothingQuality = 'high';

            var imgRenderScale = internalSize / 1024;
            var w = state.userImage.width * state.imageScale * imgRenderScale * coordScale;
            var h = state.userImage.height * state.imageScale * imgRenderScale * coordScale;
            var cx = scale3Size / 2 + state.imageX * imgRenderScale * coordScale;
            var cy = scale3Size / 2 + state.imageY * imgRenderScale * coordScale;

            tempCtx.save();
            tempCtx.translate(cx, cy);
            tempCtx.rotate(state.imageRotation * Math.PI / 180);
            tempCtx.drawImage(this._compositedImageCache, -w / 2, -h / 2, w, h);
            tempCtx.restore();

            var maskForSave = document.createElement('canvas');
            maskForSave.width = scale3Size;
            maskForSave.height = scale3Size;
            maskForSave.getContext('2d').drawImage(state.maskCanvas, 0, 0, internalSize, internalSize, 0, 0, scale3Size, scale3Size);

            tempCtx.globalCompositeOperation = 'destination-in';
            tempCtx.drawImage(maskForSave, 0, 0);
            tempCtx.globalCompositeOperation = 'source-over';

            // Тень в сохранённом файле всегда: effectsEnabled — только интерактивная оптимизация
            if (state.dropShadowEnabled) {
                var shadowCanvas = createDropShadow(tempCanvas);
                fullCtx.drawImage(shadowCanvas, 0, 0);
            }
            fullCtx.drawImage(tempCanvas, 0, 0);
        }

        var finalSize = qualityBase * usedScale;
        var cropOffset = (scale3Size - finalSize) / 2;

        var croppedCanvas = document.createElement('canvas');
        croppedCanvas.width = finalSize;
        croppedCanvas.height = finalSize;
        var croppedCtx = croppedCanvas.getContext('2d');
        croppedCtx.imageSmoothingEnabled = true;
        croppedCtx.imageSmoothingQuality = 'high';
        croppedCtx.drawImage(fullRender, cropOffset, cropOffset, finalSize, finalSize, 0, 0, finalSize, finalSize);

        return { canvas: croppedCanvas, finalSize: finalSize };
    },

    detectUsedScale: function() {
        if (!state.userImage) return 1;

        var internalSize = this.internalSize;
        var scale = internalSize / 1024;

        var SCAN = 512;
        var factor = internalSize / SCAN;

        var scanCanvas = document.createElement('canvas');
        scanCanvas.width = SCAN;
        scanCanvas.height = SCAN;
        var sCtx = scanCanvas.getContext('2d');

        this._buildCompositedImage();

        var w  = state.userImage.width  * state.imageScale * scale / factor;
        var h  = state.userImage.height * state.imageScale * scale / factor;
        var cx = SCAN / 2 + state.imageX * scale / factor;
        var cy = SCAN / 2 + state.imageY * scale / factor;
        var angle = state.imageRotation * Math.PI / 180;

        sCtx.save();
        sCtx.translate(cx, cy);
        sCtx.rotate(angle);
        sCtx.drawImage(this._compositedImageCache, -w / 2, -h / 2, w, h);
        sCtx.restore();

        sCtx.globalCompositeOperation = 'destination-in';
        sCtx.drawImage(state.maskCanvas, 0, 0, internalSize, internalSize, 0, 0, SCAN, SCAN);
        sCtx.globalCompositeOperation = 'source-over';

        var data = sCtx.getImageData(0, 0, SCAN, SCAN).data;

        scanCanvas.width = 1;
        scanCanvas.height = 1;

        var minX = SCAN, minY = SCAN, maxX = -1, maxY = -1;
        var threshold = 15;

        for (var y = 0; y < SCAN; y++) {
            for (var x = 0; x < SCAN; x++) {
                if (data[(y * SCAN + x) * 4 + 3] > threshold) {
                    if (x < minX) minX = x;
                    if (x > maxX) maxX = x;
                    if (y < minY) minY = y;
                    if (y > maxY) maxY = y;
                }
            }
        }

        if (maxX === -1) return 1;

        // Растягиваем границы на 1 пиксель скана наружу — компенсация даунскейла
        minX = Math.max(0, minX * factor - factor);
        minY = Math.max(0, minY * factor - factor);
        maxX = Math.min(internalSize, (maxX + 1) * factor + factor);
        maxY = Math.min(internalSize, (maxY + 1) * factor + factor);

        for (var s = 1; s <= 3; s++) {
            var borderSize   = Math.round(internalSize * s / 3);
            var borderOffset = (internalSize - borderSize) / 2;
            var borderEnd    = borderOffset + borderSize;

            if (
                minX >= borderOffset &&
                minY >= borderOffset &&
                maxX <= borderEnd   &&
                maxY <= borderEnd
            ) {
                return s;
            }
        }

        return 3;
    },

    _getVisibleBounds: function() {},

    updateViewTransform: function() {
        if (!this.canvas) return;
        this._rectDirty = true;

        var area = this.canvas.parentElement;
        var areaW = area ? area.clientWidth : 800;
        var areaH = area ? area.clientHeight : 600;

        var displaySide = parseFloat(this.canvas.style.width) || this.canvas.offsetWidth || areaW;

        var scaledSize = displaySide * state.viewZoom;

        var maxPanX = Math.max(0, (scaledSize - areaW) / 2 / state.viewZoom) + areaW * 0.25 / state.viewZoom;
        var maxPanY = Math.max(0, (scaledSize - areaH) / 2 / state.viewZoom) + areaH * 0.25 / state.viewZoom;

        state.viewPanX = clamp(state.viewPanX, -maxPanX, maxPanX);
        state.viewPanY = clamp(state.viewPanY, -maxPanY, maxPanY);

        this.canvas.style.transform = 'scale(' + state.viewZoom + ') translate(' + state.viewPanX + 'px, ' + state.viewPanY + 'px)';
        // DOM-оверлей пресета должен масштабироваться вместе с канвасом
        var po = $('presetOverlay');
        if (po) po.style.transform = this.canvas.style.transform;
        var indicator = $('zoomIndicator');
        if (indicator) {
            indicator.textContent = Math.round(state.viewZoom * 100) + '%';
            indicator.classList.toggle('show', state.viewZoom !== 1 || state.viewPanX !== 0 || state.viewPanY !== 0);
        }
    },

    resetView: function() {
        state.viewZoom = 1;
        state.viewPanX = 0;
        state.viewPanY = 0;
        this.updateViewTransform();
    },

    updateScaleUI: function() {
        var slider = $('scaleSlider');
        var input = $('scaleInput');
        var val = Math.round(state.imageScale * 100);
        if (slider) { slider.value = val; slider.style.setProperty('--p', ((val - parseFloat(slider.min)) / (parseFloat(slider.max) - parseFloat(slider.min)) * 100) + '%'); }
        if (input) input.value = val;
    },

    updateRotationUI: function() {
        var slider = $('rotationSlider');
        var input = $('rotationInput');
        var val = Math.round(state.imageRotation);
        if (slider) { slider.value = val; slider.style.setProperty('--p', ((val - parseFloat(slider.min)) / (parseFloat(slider.max) - parseFloat(slider.min)) * 100) + '%'); }
        if (input) input.value = val;
    },

    scheduleEffects: function(disableDuringUpdate = true) {
        var self = this;
        if (disableDuringUpdate) state.effectsEnabled = false;
        this._shadowDirty = true;
        this._zonesDirty = true;
        if (state.effectsTimeout) clearTimeout(state.effectsTimeout);
        state.effectsTimeout = setTimeout(function() {
            state.effectsEnabled = true;
            state.effectsTimeout = null;
            self._shadowDirty = true;
            if (self._deferCC) {
                self._deferCC = false;
                self._ccDirty = true;
                self._compositedImageDirty = true;
            }
            self.render();
        }, CONFIG.EFFECTS_DELAY);
    },

    getCanvasPos: function(e) {
        if (this._rectDirty || !this._cachedRect) {
            this._cachedRect = this.canvas.getBoundingClientRect();
            this._rectDirty = false;
        }
        var rect = this._cachedRect;
        var scaleX = this.internalSize / rect.width;
        var scaleY = this.internalSize / rect.height;
        var clientX = e.touches ? e.touches[0].clientX : e.clientX;
        var clientY = e.touches ? e.touches[0].clientY : e.clientY;
        return {
            x: (clientX - rect.left) * scaleX,
            y: (clientY - rect.top) * scaleY
        };
    },

    _canvasPosToImagePos: function(canvasX, canvasY) {
        if (!state.userImage) return null;
        var size = this.internalSize;
        var scale = size / 1024;
        var cx = size / 2 + state.imageX * scale;
        var cy = size / 2 + state.imageY * scale;
        var cos = Math.cos(-state.imageRotation * Math.PI / 180);
        var sin = Math.sin(-state.imageRotation * Math.PI / 180);
        var dx = canvasX - cx;
        var dy = canvasY - cy;
        var lx = cos * dx - sin * dy;
        var ly = sin * dx + cos * dy;
        var px = lx / (state.imageScale * scale) + state.userImage.width / 2;
        var py = ly / (state.imageScale * scale) + state.userImage.height / 2;
        return { x: px, y: py };
    },

    _queueWorkerTask: function(cx, cy, restore, mode) {
        var q = this._workerQueues[mode];
        if (!q) return false;
        restore = !!restore;

        // Точки не дропаем никогда — потеря штампа это дыра в штрихе.
        // Вместо этого сливаем близкие точки: интерполяция в addErasePoint
        // режет путь шагом 0.35·size, перекрытие штампов огромное, схлопывание
        // дрожащих (< половины шага) точек визуально ничего не меняет
        var step = this.eraserBrush ? this.eraserBrush.size * 0.4 : 4;
        var last = q.length ? q[q.length - 1] : null;
        if (last && last.restore === restore) {
            var dx = cx - last.cx;
            var dy = cy - last.cy;
            if (dx * dx + dy * dy < step * step * 0.25) {
                last.cx = cx;
                last.cy = cy;
                return true;
            }
        }
        if (q.length >= 1024 && last) {
            // Патологический случай (воркер молчит): растягиваем хвостовой
            // штамп до новой точки вместо бесконечного роста очереди
            last.cx = cx;
            last.cy = cy;
            return true;
        }
        q.push({ cx: cx, cy: cy, restore: restore, mode: mode });
        if (this._workerInFlight === 0 && !this._workerBatchTimer) {
            var self = this;
            this._workerBatchTimer = setTimeout(function() {
                self._flushWorkerBatch(true);
            }, 8);
        }
        return true;
    },

    applyEraserBrushToPinkMask: function(cx, cy, restore) {
        if (!this.eraserBrush || !state.maskCanvas) return;
        if (this._worker) {
            // Pink-маска уходит в тот же воркер, что и ластик:
            // без canvas-аллокаций на главном потоке
            this._queueWorkerTask(cx, cy, restore, 'pink');
            return;
        }

        // Фолбэк без воркера — синхронно на главном потоке
        var brush = this.eraserBrush;
        var maskCtx = state.maskCanvas.getContext('2d');
        var drawX = Math.round(cx - brush.fullSize / 2);
        var drawY = Math.round(cy - brush.fullSize / 2);

        var workCanvas = document.createElement('canvas');
        workCanvas.width = brush.fullSize;
        workCanvas.height = brush.fullSize;
        var workCtx = workCanvas.getContext('2d');
        workCtx.drawImage(brush.canvas, 0, 0);

        if (state.erasableCanvas) {
            workCtx.globalCompositeOperation = 'destination-out';
            workCtx.drawImage(
                state.erasableCanvas,
                drawX, drawY, brush.fullSize, brush.fullSize,
                0, 0, brush.fullSize, brush.fullSize
            );
            workCtx.globalCompositeOperation = 'source-over';
        }

        if (restore) {
            maskCtx.globalCompositeOperation = 'source-over';
        } else {
            maskCtx.globalCompositeOperation = 'destination-out';
        }
        maskCtx.drawImage(workCanvas, drawX, drawY);
        maskCtx.globalCompositeOperation = 'source-over';

        this._zonesDirty = true;
        this._shadowDirty = true;
        this._strokeChanged = true;
        this._markStrokeDirty(cx, cy, true);
        this.requestRender();
    },

    applyEraserBrushToImageMask: function(cx, cy, restore) {
        if (!state.userImage || !state.imageMaskCanvas) return;

        if (this._worker) {
            // Точки копятся в пачку: один postMessage + один getImageData/
            // putImageData + один рендер на десятки точек вместо одной на точку
            this._queueWorkerTask(cx, cy, restore, 'image');
            return;
        }

        this._fallbackBrushToImageMask(cx, cy, restore);
    },

    _fallbackBrushToImageMask: function(cx, cy, restore) {
        var imgPos = this._canvasPosToImagePos(cx, cy);
        if (!imgPos) return;

        var imgBrush = this._getImageBrush();
        if (!imgBrush) return;

        var maskCtx = state.imageMaskCanvas.getContext('2d');
        var drawX = Math.round(imgPos.x - imgBrush.fullSize / 2);
        var drawY = Math.round(imgPos.y - imgBrush.fullSize / 2);

        var workCanvas = document.createElement('canvas');
        workCanvas.width = imgBrush.fullSize;
        workCanvas.height = imgBrush.fullSize;
        var workCtx = workCanvas.getContext('2d');
        workCtx.drawImage(imgBrush.canvas, 0, 0);

        if (state.erasableCanvas) {
            var internalSize = this.internalSize;
            var scale = internalSize / 1024;
            var effectiveScale = state.imageScale * scale;

            var protSampler = document.createElement('canvas');
            protSampler.width = imgBrush.fullSize;
            protSampler.height = imgBrush.fullSize;
            var psCtx = protSampler.getContext('2d');
            psCtx.save();
            psCtx.translate(imgBrush.fullSize / 2, imgBrush.fullSize / 2);
            psCtx.rotate(-state.imageRotation * Math.PI / 180);
            psCtx.translate(-imgBrush.fullSize / 2, -imgBrush.fullSize / 2);
            var protHalf = imgBrush.fullSize / 2;
            psCtx.drawImage(
                state.erasableCanvas,
                cx - protHalf * effectiveScale,
                cy - protHalf * effectiveScale,
                imgBrush.fullSize * effectiveScale,
                imgBrush.fullSize * effectiveScale,
                0, 0, imgBrush.fullSize, imgBrush.fullSize
            );
            psCtx.restore();

            workCtx.globalCompositeOperation = 'destination-out';
            workCtx.drawImage(protSampler, 0, 0);
            workCtx.globalCompositeOperation = 'source-over';
        }

        if (restore) {
            maskCtx.globalCompositeOperation = 'source-over';
        } else {
            maskCtx.globalCompositeOperation = 'destination-out';
        }
        maskCtx.drawImage(workCanvas, drawX, drawY);
        maskCtx.globalCompositeOperation = 'source-over';

        this._invalidateComposite();
        this._strokeChanged = true;
        this._markStrokeDirty(cx, cy, false);
        this.requestRender();
    },

    processErasePoints: function() {
        var self = this;

        if (this.pendingErasePoints.length === 0) {
            this.eraseAnimationId = null;
            return;
        }

        var batchSize = Math.min(8, this.pendingErasePoints.length);

        if (state.currentEraserMode === 'pink') {
            for (var i = 0; i < batchSize; i++) {
                var point = this.pendingErasePoints.shift();
                this.applyEraserBrushToPinkMask(point.x, point.y, point.restore);
            }
        } else {
            for (var j = 0; j < batchSize; j++) {
                var pt = this.pendingErasePoints.shift();
                this.applyEraserBrushToImageMask(pt.x, pt.y, pt.restore);
            }
        }

        // Рендер здесь не вызывается намеренно: кадр нужен только когда
        // маска реально изменилась — об этом сообщает воркер (changed),
        // либо синхронный фолбэк сам вызывает requestRender.

        // rAF-цикл живёт, только пока есть точки: иначе удержание кнопки
        // мыши без движения гоняет render() на 60 FPS впустую
        if (this.pendingErasePoints.length > 0) {
            this.eraseAnimationId = requestAnimationFrame(function() { self.processErasePoints(); });
        } else {
            this.eraseAnimationId = null;
        }
    },

    addErasePoint: function(cx, cy, restore) {
        var point = { x: cx, y: cy, restore: !!restore };

        if (state.lastErasePos) {
            var dx = cx - state.lastErasePos.x;
            var dy = cy - state.lastErasePos.y;
            var dist = Math.sqrt(dx * dx + dy * dy);
            var step = this.eraserBrush.size * 0.4;
            if (dist > step) {
                var steps = Math.ceil(dist / step);
                for (var i = 1; i <= steps; i++) {
                    var t = i / steps;
                    this.pendingErasePoints.push({
                        x: state.lastErasePos.x + dx * t,
                        y: state.lastErasePos.y + dy * t,
                        restore: !!restore
                    });
                }
            } else {
                this.pendingErasePoints.push(point);
            }
        } else {
            this.pendingErasePoints.push(point);
        }

        state.lastErasePos = { x: cx, y: cy };

        if (!this.eraseAnimationId) {
            var self = this;
            this.eraseAnimationId = requestAnimationFrame(function() { self.processErasePoints(); });
        }
    },

    startErasing: function(pos, restore) {
        if (this._eraserBrushTimer) {
            clearTimeout(this._eraserBrushTimer);
            this._eraserBrushTimer = null;
            this.updateEraserBrush(state.eraserSize);
        }
        this.isErasing = true;
        this._isErasing = true;
        state.isRestoring = !!restore;
        state.lastErasePos = null;
        this.pendingErasePoints = [];
        this._strokeDirtyRect = null;
        this._strokeFullDirty = true;
        this._strokeChanged = false;

        // Эффекты (тень) держим выключенными на весь штрих — включаем
        // обратно в stopErasing(), без перезапуска таймера на каждый кадр
        state.effectsEnabled = false;
        if (state.effectsTimeout) {
            clearTimeout(state.effectsTimeout);
            state.effectsTimeout = null;
        }
        this._shadowDirty = true;
        this._zonesDirty = true;

        this.addErasePoint(pos.x, pos.y, state.isRestoring);
    },

    stopErasing: function() {
        this.isErasing = false;
        this._isErasing = false;
        state.lastErasePos = null;
        this._strokeDirtyRect = null;
        this._strokeFullDirty = false;

        // Эффекты снова включаются — тень пересчитается в финальном render()
        state.effectsEnabled = true;
        this._shadowDirty = true;
        this._eraseSaveTries = 0;

        // Накопленные точки улетают немедленно, не дожидаясь таймера
        this._flushWorkerBatch(true);

        var self = this;
        setTimeout(function() { self._trySaveHistoryAfterErase(); }, 50);
    },

    _trySaveHistoryAfterErase: function() {
        // Ждём завершения worker-пачек: снимок истории обязан включать
        // последние штрихи, иначе undo откатывает больше, чем стёрто
        if (this.pendingErasePoints.length === 0 && !this.eraseAnimationId &&
            !this._workerInFlight &&
            !this._workerQueues.pink.length && !this._workerQueues.image.length) {
            this._eraseSaveTries = 0;
            if (this._strokeChanged) TokenHistory.save();
            this.render();
            return;
        }
        // _workerInFlight уменьшается и по ответу воркера, и по onerror —
        // цикл гарантированно завершится; кап — только от тихого зависания.
        // Раньше здесь по таймауту принудительно сбрасывался pending и
        // сохранялась история без последней пачки — терялся хвост штриха
        if (this._eraseSaveTries < 100) {
            this._eraseSaveTries++;
            var self = this;
            setTimeout(function() { self._trySaveHistoryAfterErase(); }, this._eraseSaveTries > 20 ? 200 : 100);
        } else {
            this._eraseSaveTries = 0;
            if (this._strokeChanged) TokenHistory.save();
            this.render();
        }
    },

    setupEvents: function() {
        var self = this;
        var target = this.area;

        var invalidateRect = function() { self._rectDirty = true; };
        window.addEventListener('resize', invalidateRect);
        window.addEventListener('scroll', invalidateRect, true);

        // Pointer Events вместо mouse+touch: единый путь ввода, а
        // getCoalescedEvents отдаёт промежуточные точки высокочастотных
        // мышек/перов (125-240 Гц) — штрих глаже при меньшем числе штампов.
        // touch-action: none в CSS не даёт браузеру перехватить жест
        target.onpointerenter = function(e) {
            self._rectDirty = true;
            if (state.userImage && (state.currentTool === 'eraser' || state.currentTool === 'mask')) {
                self.showEraserCursor();
                self.updateEraserCursor(e);
            }
        };

        target.onpointerleave = function(e) {
            self.hideEraserCursor();
            if (state.isPanning) {
                state.isPanning = false;
                target.style.cursor = '';
            }
            if (state.isDragging && (state.dragStartPos.x !== state.imageX || state.dragStartPos.y !== state.imageY)) {
                TokenHistory.save();
            }
            if (self.isErasing) self.stopErasing();
            state.isDragging = false;
        };

        var beginPan = function(e) {
            state.isPanning = true;
            state.panStart = { x: e.clientX, y: e.clientY };
            state.panStartView = { x: state.viewPanX, y: state.viewPanY };
            target.style.cursor = 'grabbing';
            self.hideEraserCursor();
        };

        // Идентификатор указателя, захватившего активный жест (панорама/
        // перетаскивание/стирание). Второй палец или стилус игнорируются,
        // пока жест не завершён — иначе их события порождают ложные штрихи.
        self._activePointerId = null;

        target.onpointerdown = function(e) {
            self._rectDirty = true;
            if (!state.userImage) return;
            if (self._activePointerId !== null && e.pointerId !== self._activePointerId) return;
            if (e.button === 1 || e.button === 2) {
                e.preventDefault();
                self._activePointerId = e.pointerId;
                beginPan(e);
                return;
            }
            if (e.button === 0 && e.altKey) {
                e.preventDefault();
                self._activePointerId = e.pointerId;
                beginPan(e);
                return;
            }
            e.preventDefault();
            var pos = self.getCanvasPos(e);
            if (state.currentTool === 'move') {
                state.isDragging = true;
                self._activePointerId = e.pointerId;
                var sc = self.internalSize / 1024;
                state.dragStart = { x: pos.x / sc - state.imageX, y: pos.y / sc - state.imageY };
                state.dragStartPos = { x: state.imageX, y: state.imageY };
            } else if (state.currentTool === 'eraser' || state.currentTool === 'mask') {
                // Тач не имеет shift — восстановление только мышью/пером
                self._activePointerId = e.pointerId;
                self.startErasing(pos, e.shiftKey);
            }
        };

        target.onpointermove = function(e) {
            if ((state.currentTool === 'eraser' || state.currentTool === 'mask') && !state.isPanning) self.updateEraserCursor(e);
            if (self._activePointerId !== null && e.pointerId !== self._activePointerId) return;
            if (state.isPanning) {
                var pdx = (e.clientX - state.panStart.x) / state.viewZoom;
                var pdy = (e.clientY - state.panStart.y) / state.viewZoom;
                state.viewPanX = state.panStartView.x + pdx;
                state.viewPanY = state.panStartView.y + pdy;
                self.updateViewTransform();
                return;
            }
            e.preventDefault();
            if (state.currentTool === 'move' && state.isDragging) {
                var pos = self.getCanvasPos(e);
                var sc = self.internalSize / 1024;
                state.imageX = pos.x / sc - state.dragStart.x;
                state.imageY = pos.y / sc - state.dragStart.y;
                self.scheduleEffects();
                self.requestRender();
            } else if ((state.currentTool === 'eraser' || state.currentTool === 'mask') && self.isErasing) {
                // Коалесцированные точки: вся траектория между кадрами,
                // а не только позиция на момент rAF
                var evs = e.getCoalescedEvents ? e.getCoalescedEvents() : null;
                if (evs && evs.length > 1) {
                    for (var ci = 0; ci < evs.length; ci++) {
                        var cpos = self.getCanvasPos(evs[ci]);
                        self.addErasePoint(cpos.x, cpos.y, state.isRestoring);
                    }
                } else {
                    var pos2 = self.getCanvasPos(e);
                    self.addErasePoint(pos2.x, pos2.y, state.isRestoring);
                }
            }
        };

        target.onpointerup = function(e) {
            if (self._activePointerId !== null && e.pointerId !== self._activePointerId) return;
            self._activePointerId = null;
            if (state.isPanning) {
                state.isPanning = false;
                target.style.cursor = '';
                if (state.currentTool === 'eraser' || state.currentTool === 'mask') {
                    self.showEraserCursor();
                    self.updateEraserCursor(e);
                }
                return;
            }
            if (state.isDragging && (state.dragStartPos.x !== state.imageX || state.dragStartPos.y !== state.imageY)) {
                TokenHistory.save();
            }
            if (self.isErasing) self.stopErasing();
            state.isDragging = false;
        };

        // Жест прерван системой (жест ОС, входящий звонок стилуса и т.п.)
        target.onpointercancel = function(e) {
            if (self._activePointerId !== null && e.pointerId !== self._activePointerId) return;
            self._activePointerId = null;
            if (state.isPanning) {
                state.isPanning = false;
                target.style.cursor = '';
            }
            if (state.isDragging && (state.dragStartPos.x !== state.imageX || state.dragStartPos.y !== state.imageY)) {
                TokenHistory.save();
            }
            if (self.isErasing) self.stopErasing();
            state.isDragging = false;
            self.hideEraserCursor();
        };

        target.oncontextmenu = function(e) { e.preventDefault(); };

        target.onwheel = function(e) {
            e.preventDefault();
            if (!state.userImage) return;
            if (e.ctrlKey || e.metaKey) {
                var canvasRect = self.canvas.getBoundingClientRect();
                var mouseX = e.clientX - canvasRect.left - canvasRect.width / 2;
                var mouseY = e.clientY - canvasRect.top - canvasRect.height / 2;
                var zoomFactor = e.deltaY > 0 ? 0.95 : 1.05;
                var newZoom = clamp(state.viewZoom * zoomFactor, CONFIG.MIN_ZOOM, CONFIG.MAX_ZOOM);
                var zoomRatio = newZoom / state.viewZoom;
                state.viewPanX = state.viewPanX - mouseX * (zoomRatio - 1) / newZoom;
                state.viewPanY = state.viewPanY - mouseY * (zoomRatio - 1) / newZoom;
                state.viewZoom = newZoom;
                self.updateViewTransform();
                if (state.currentTool === 'eraser' || state.currentTool === 'mask') self.updateEraserCursor(e);
            } else if (e.altKey && state.userImage) {
                var delta = e.deltaY > 0 ? -1 : 1;
                var newScale = clamp(state.imageScale * 100 + delta, CONFIG.MIN_SCALE, CONFIG.MAX_SCALE);
                state.imageScale = newScale / 100;
                self._imageBrushCache = null;
                self.updateScaleUI();
                self.scheduleEffects();
                self._forceFullRender();
                self.requestRender();
                if (self._debouncedSave) self._debouncedSave();
            } else if (e.shiftKey) {
                var panAmount = CONFIG.PAN_AMOUNT / state.viewZoom;
                state.viewPanX -= e.deltaY > 0 ? panAmount : -panAmount;
                self.updateViewTransform();
            } else {
                var panAmount2 = CONFIG.PAN_AMOUNT / state.viewZoom;
                state.viewPanY -= e.deltaY > 0 ? panAmount2 : -panAmount2;
                self.updateViewTransform();
            }
        };

        this.canvas.oncontextmenu = function(e) { e.preventDefault(); };
    },

    cleanupUserImage: function() {
        urlManager.revoke('userImage');
        state.userImageUrl = null;
    },

    loadImage: function(file, filePath) {
        var self = this;

        var prevScale = state.userImage ? state.imageScale : null;
        var prevX = state.userImage ? state.imageX : null;
        var prevY = state.userImage ? state.imageY : null;
        var prevRotation = state.userImage ? state.imageRotation : null;

        urlManager.revoke('userImage');
        state.userImageUrl = null;

        // Прерываем идущий штрих ластика: точки и rAF-цикл не должны
        // «докапать» на маску нового изображения
        this.pendingErasePoints = [];
        this.isErasing = false;
        this._isErasing = false;
        if (this.eraseAnimationId) {
            cancelAnimationFrame(this.eraseAnimationId);
            this.eraseAnimationId = null;
        }

        this._clearCanvasCache();

        if (state.maskCanvas) {
            var mCtx = state.maskCanvas.getContext('2d');
            mCtx.clearRect(0, 0, state.maskCanvas.width, state.maskCanvas.height);
            state.maskCanvas = null;
        }
        if (state.imageMaskCanvas) {
            var imCtx = state.imageMaskCanvas.getContext('2d');
            imCtx.clearRect(0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
            state.imageMaskCanvas = null;
        }

        state.userImage = null;
        state.userImageOriginal = null;
        state.userImageWithoutBg = null;
        state.history = [];
        state.historyIndex = -1;

        state.currentFilePath = filePath || null;

        state.tokenFileName = getFileBaseName(file.name);
        var fileNameInput = $('tokenFileName');
        if (fileNameInput) fileNameInput.value = state.tokenFileName;

        var self = this;
        urlManager.revoke('userImage-decode');
        var decodeUrl = urlManager.create(file, 'userImage-decode');
        var img = new Image();
        img.onload = function() {
            urlManager.revoke('userImage-decode');
            var finalImg = self._capImage(img);

            state.userImage = finalImg;
            state.userImageOriginal = finalImg;
            state.userImageWithoutBg = null;
            state.backgroundRemoved = false;
            state.showingOriginal = false;
            TokenEditor.updateRemoveBgButton();

            if (prevScale !== null) {
                state.imageScale = prevScale;
                state.imageX = prevX;
                state.imageY = prevY;
                state.imageRotation = prevRotation;
            } else {
                var internalSize = self.internalSize;
                var scale = internalSize / 1024;
                var maxDisplayPx = CONFIG.SCALE_SIZES[1] * scale;
                var imgMaxDim = Math.max(finalImg.width, finalImg.height);
                state.imageScale = (maxDisplayPx / imgMaxDim) / scale;
                state.imageX = 0;
                state.imageY = 0;
                state.imageRotation = 0;
            }

            self.updateScaleUI();
            self.updateRotationUI();
            self.resetView();

            self.createMask();
            self.createImageMask();
            state.currentPreset = -1;
            TokenPresets.updateButtons();
            if (typeof TokenEditor !== 'undefined') {
                var undoBar = $('undoBar');
                if (undoBar) undoBar.style.display = 'flex';
                TokenEditor._updateUndoButtons();
            }
            TokenHistory.init();
            self.render();

            if (typeof PortraitGenerator !== 'undefined' && PortraitGenerator.canvas) {
                PortraitGenerator.onImageLoaded();
            }

            var saveWithoutRing = $('saveWithoutRing');
            var saveWithRing = $('saveWithRing');
            if (saveWithoutRing) saveWithoutRing.disabled = false;
            if (saveWithRing) saveWithRing.disabled = false;

            TokenEditor.updateNavState();
        };
        img.onerror = function() {
            urlManager.revoke('userImage-decode');
            toast('Не удалось прочитать изображение', true);
        };
        img.src = decodeUrl;
    },

    _capImage: function(img) {
        var maxDim = CONFIG.MAX_IMAGE_DIM || 4096;
        var w = img.naturalWidth || img.width;
        var h = img.naturalHeight || img.height;
        if (!w || !h || (w <= maxDim && h <= maxDim)) return img;
        var ratio = Math.min(maxDim / w, maxDim / h);
        var cw = Math.max(1, Math.round(w * ratio));
        var ch = Math.max(1, Math.round(h * ratio));
        var c = document.createElement('canvas');
        c.width = cw;
        c.height = ch;
        var ctx = c.getContext('2d');
        ctx.imageSmoothingEnabled = true;
        ctx.imageSmoothingQuality = 'high';
        ctx.drawImage(img, 0, 0, cw, ch);
        return c;
    },

    loadImageByPath: function(filePath) {
        var self = this;
        fetch('/get_image_by_path?path=' + encodeURIComponent(filePath))
            .then(function(r) {
                if (!r.ok) throw new Error('Не удалось загрузить файл');
                return r.blob();
            })
            .then(function(blob) {
                var fileName = filePath.split(/[\\/]/).pop();
                var file = new File([blob], fileName, { type: blob.type });
                file.path = filePath;
                self.loadImage(file, filePath);
            })
            .catch(function(err) {
                toast('Ошибка: ' + err.message, true);
            });
    },

    _clearCanvasCache: function() {
        this._compositedImageCache = null;
        this._shadowCache = null;
        this._zonesCanvas = null;
        this._protectionOverlayCache = null;
        this._protectionOverlayDirty = true;
        this._zoneHelpersDirty = true;
        this._compositedImageDirty = true;
        this._ccDirty = true;
        this._shadowDirty = true;
        this._zonesDirty = true;
        this._imageBrushCache = null;
        this._imageBrushCacheSize = -1;
        this._workerQueues.pink = [];
        this._workerQueues.image = [];
        if (this._workerBatchTimer) clearTimeout(this._workerBatchTimer);
        this._workerBatchTimer = null;
        this._ccImageCache = null;
        this._ccCacheKey = null;
        this._deferCC = false;
        this._strokeDirtyRect = null;
        this._strokeFullDirty = false;
        this._strokeChanged = false;
        this._maskGen++;
    },
    // Сброс несброшенных штрихов + инкремент генерации маски. Обязателен при
    // ЛЮБОЙ полной перезаписи маски в обход ластика (undo/redo, сброс, пресет,
    // заливка после удаления фона): иначе устаревшие пачки из полёта пройдут
    // ген-проверку и втаптывают до-перезаписные штрихи в свежую маску.
    _invalidatePendingStrokes: function() {
        this._workerQueues.pink = [];
        this._workerQueues.image = [];
        if (this._workerBatchTimer) clearTimeout(this._workerBatchTimer);
        this._workerBatchTimer = null;
        // _workerInFlight не трогаем: ответы в полёте декрементируют сами
        this._strokeDirtyRect = null;
        this._strokeFullDirty = false;
        this._strokeChanged = false;
        this._maskGen++;
    },
    resetMask: function() {
        if (!state.maskCanvas) return;
        this._invalidatePendingStrokes();
        var maskCtx = state.maskCanvas.getContext('2d');
        maskCtx.globalCompositeOperation = 'source-over';
        maskCtx.fillStyle = 'white';
        maskCtx.fillRect(0, 0, state.maskCanvas.width, state.maskCanvas.height);
        this._pushMaskToWorker(true);
        state.currentPreset = -1;
        this._zonesDirty = true;
        this._shadowDirty = true;
        TokenPresets.updateButtons();
        TokenHistory.save();
        this.render();
        toast('Маска сброшена');
    },

    _drawExampleOverlay: function(size, clipRect) {
        if (!state.userImage || !state.exampleEnabled || !state.exampleImage) return;
        var ex = state.exampleImage;
        // Пример — шаблон токена: масштаб m показывает область экспорта m×
        // (центр m/3 канваса) независимо от масштаба рабочего канваса.
        var exMode = [1, 2, 3].includes(state.exampleScaleMode) ? state.exampleScaleMode : 1;
        var exRatio = exMode / 3;
        var exW = size * exRatio;
        var exH = size * exRatio;
        this.ctx.save();
        if (clipRect) {
            this.ctx.beginPath();
            this.ctx.rect(clipRect.x, clipRect.y, clipRect.w, clipRect.h);
            this.ctx.clip();
        }
        this.ctx.globalAlpha = state.exampleOpacity;
        this.ctx.imageSmoothingEnabled = true;
        this.ctx.imageSmoothingQuality = 'high';
        this.ctx.drawImage(ex, (size - exW) / 2, (size - exH) / 2, exW, exH);
        this.ctx.restore();
    },

    _renderProtectionMaskOverlay: function(size, clipRect) {
        if (!state.erasableCanvas && !state.protectionAlpha) return;

        var internalSize = this.internalSize;
        // Оверлей — полупрозрачная красная подсветка: строится в пониженном
        // разрешении (до 1536) и растягивается. На 3072² это в 4 раза меньше
        // работы, на экране разницы нет
        var overlaySize = Math.min(internalSize, 1536);

        if (this._protectionOverlayDirty || !this._protectionOverlayCache || this._protectionOverlayCache.width !== overlaySize) {
            if (!this._protectionOverlayCache || this._protectionOverlayCache.width !== overlaySize) {
                this._protectionOverlayCache = document.createElement('canvas');
                this._protectionOverlayCache.width = overlaySize;
                this._protectionOverlayCache.height = overlaySize;
            }
            var oCtx = this._protectionOverlayCache.getContext('2d');
            var oData = oCtx.createImageData(overlaySize, overlaySize);
            var od = oData.data;

            var alpha = state.protectionAlpha;
            if (alpha && state.protectionSize === internalSize) {
                // Быстрый путь: сэмплируем готовый альфа-массив защиты
                var stride = internalSize / overlaySize;
                for (var y = 0; y < overlaySize; y++) {
                    var srow = Math.round(y * stride) * internalSize;
                    for (var x = 0; x < overlaySize; x++) {
                        var isBlocked = alpha[srow + Math.round(x * stride)] >= 128;
                        var oi = (y * overlaySize + x) * 4;
                        od[oi]     = isBlocked ? 255 : 0;
                        od[oi + 1] = isBlocked ? 80  : 0;
                        od[oi + 2] = isBlocked ? 80  : 0;
                        od[oi + 3] = isBlocked ? 120 : 0;
                    }
                }
            } else {
                // Резервный путь: чтение канваса защиты
                var eData = state.erasableCanvas.getContext('2d').getImageData(0, 0, internalSize, internalSize);
                var ed = eData.data;
                var estep = internalSize / overlaySize;
                for (var y2 = 0; y2 < overlaySize; y2++) {
                    var srow2 = Math.round(y2 * estep) * internalSize * 4;
                    for (var x2 = 0; x2 < overlaySize; x2++) {
                        var isBlocked2 = ed[srow2 + Math.round(x2 * estep) * 4 + 3] >= 128;
                        var oi2 = (y2 * overlaySize + x2) * 4;
                        od[oi2]     = isBlocked2 ? 255 : 0;
                        od[oi2 + 1] = isBlocked2 ? 80  : 0;
                        od[oi2 + 2] = isBlocked2 ? 80  : 0;
                        od[oi2 + 3] = isBlocked2 ? 120 : 0;
                    }
                }
            }

            oCtx.putImageData(oData, 0, 0);
            this._protectionOverlayDirty = false;
        }
        this.ctx.save();
        if (clipRect) {
            this.ctx.beginPath();
            this.ctx.rect(clipRect.x, clipRect.y, clipRect.w, clipRect.h);
            this.ctx.clip();
        }
        this.ctx.drawImage(this._protectionOverlayCache, 0, 0, overlaySize, overlaySize, 0, 0, size, size);
        this.ctx.restore();
    },
    
    resetImageMask: function() {
        if (!state.imageMaskCanvas) return;
        this._invalidatePendingStrokes();
        var ctx = state.imageMaskCanvas.getContext('2d');
        ctx.globalCompositeOperation = 'source-over';
        ctx.fillStyle = 'white';
        ctx.fillRect(0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
        this._pushMaskToWorker(false);
        this._invalidateComposite();
        TokenHistory.save();
        this.render();
        toast('Ластик сброшен');
    },

    save: function(withRing) {
        if (!state.userImage) {
            toast('Сначала загрузите изображение', true);
            return;
        }

        var fmt = (AppConfig.saveSettings && AppConfig.saveSettings.format) || 'webp';
        if (['webp', 'png', 'jpg'].indexOf(fmt) === -1) fmt = 'webp';
        var comp = parseInt(AppConfig.saveSettings.compression) || 95;
        var ext = fmt === 'jpg' ? 'jpg' : fmt;
        var fileName = (state.tokenFileName.trim() || 'token') + '.' + ext;
        var mime = fmt === 'png' ? 'image/png' : (fmt === 'jpg' ? 'image/jpeg' : 'image/webp');
        var quality = fmt === 'png' ? undefined : Math.min(1, Math.max(0.01, comp / 100));

        var result = this.renderForSave(withRing);
        var outCanvas = result.canvas;
        if (fmt === 'jpg') {
            var flat = document.createElement('canvas');
            flat.width = result.canvas.width;
            flat.height = result.canvas.height;
            var flatCtx = flat.getContext('2d');
            flatCtx.fillStyle = '#ffffff';
            flatCtx.fillRect(0, 0, flat.width, flat.height);
            flatCtx.drawImage(result.canvas, 0, 0);
            outCanvas = flat;
        }

        outCanvas.toBlob(async function(blob) {

            if (state.quickSaveEnabled && state.quickSaveFolder) {
                const ok = await saveToFolder(blob, fileName, state.quickSaveFolder);
                if (ok) toast('Сохранено: ' + fileName);
            } else {
                await saveFileWithPicker(blob, fileName);
            }
        }, mime, quality);
    },

    setEraserSize: function(size) {
        state.eraserSize = size;
        this._imageBrushCache = null;
        this._imageBrushCacheSize = -1;
        var self = this;
        if (this._eraserBrushTimer) clearTimeout(this._eraserBrushTimer);
        if (this.isErasing) {
            this.updateEraserBrush(size);
        } else {
            this._eraserBrushTimer = setTimeout(function() {
                self._eraserBrushTimer = null;
                self.updateEraserBrush(state.eraserSize);
            }, 80);
        }
    }
};
