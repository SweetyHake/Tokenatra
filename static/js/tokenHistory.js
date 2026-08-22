var HISTORY_STORE_SIZE = CONFIG.HISTORY_STORE_SIZE || 1024;

// Общий скретч для разворачивания снапшотов при undo/redo — без аллокаций
var _histScratch = null;

var TokenHistory = {
    _storeSize: HISTORY_STORE_SIZE,

    // Снимок только альфа-канала: RGB масок всегда белый (255), хранить
    // 4 байта/px незачем. 1024² = 1 МБ вместо 4 МБ на маску — при 30
    // записях и двух масках это ~60 МБ вместо ~240 МБ
    _snapshotAlpha(canvas) {
        if (!canvas) return null;
        var maxDim = Math.max(canvas.width, canvas.height);
        var scale = maxDim > this._storeSize ? this._storeSize / maxDim : 1;
        var w = Math.round(canvas.width * scale);
        var h = Math.round(canvas.height * scale);
        var src = canvas;
        if (scale !== 1) {
            var down = document.createElement('canvas');
            down.width = w;
            down.height = h;
            var dCtx = down.getContext('2d');
            dCtx.imageSmoothingEnabled = true;
            dCtx.imageSmoothingQuality = 'high';
            dCtx.drawImage(canvas, 0, 0, w, h);
            src = down;
        }
        var data = src.getContext('2d').getImageData(0, 0, w, h).data;
        var alpha = new Uint8Array(w * h);
        for (var i = 0; i < alpha.length; i++) alpha[i] = data[i * 4 + 3];
        return { a: alpha, w: w, h: h };
    },

    save() {
        if (!state.maskCanvas) return;

        if (state.historyIndex < state.history.length - 1) {
            state.history.splice(state.historyIndex + 1);
        }

        state.history.push({
            mask: this._snapshotAlpha(state.maskCanvas),
            imageMask: this._snapshotAlpha(state.imageMaskCanvas),
            x: state.imageX,
            y: state.imageY,
            scale: state.imageScale,
            rotation: state.imageRotation
        });

        if (state.history.length > CONFIG.MAX_HISTORY) {
            state.history.shift();
            state.historyIndex = state.history.length - 1;
        } else {
            state.historyIndex++;
        }
        if (typeof TokenEditor !== 'undefined' && TokenEditor._updateUndoButtons) TokenEditor._updateUndoButtons();
    },

    _restoreMaskFromSnap(canvas, snap) {
        if (!canvas) return;
        var ctx = canvas.getContext('2d');
        ctx.globalCompositeOperation = 'source-over';
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        if (!snap || !snap.a) {
            ctx.fillStyle = 'white';
            ctx.fillRect(0, 0, canvas.width, canvas.height);
            return;
        }
        if (!_histScratch) _histScratch = document.createElement('canvas');
        if (_histScratch.width !== snap.w || _histScratch.height !== snap.h) {
            _histScratch.width = snap.w;
            _histScratch.height = snap.h;
        }
        var sCtx = _histScratch.getContext('2d');
        var imgData = sCtx.createImageData(snap.w, snap.h);
        var d = imgData.data;
        var n = snap.w * snap.h;
        for (var i = 0; i < n; i++) {
            var o = i * 4;
            d[o] = 255; d[o + 1] = 255; d[o + 2] = 255;
            d[o + 3] = snap.a[i];
        }
        sCtx.putImageData(imgData, 0, 0);
        ctx.imageSmoothingEnabled = true;
        ctx.imageSmoothingQuality = 'high';
        ctx.drawImage(_histScratch, 0, 0, snap.w, snap.h, 0, 0, canvas.width, canvas.height);
    },

    restore(entry) {
        if (!entry || !entry.mask) return;

        this._restoreMaskFromSnap(state.maskCanvas, entry.mask);
        this._restoreMaskFromSnap(state.imageMaskCanvas, entry.imageMask);

        state.imageX = entry.x;
        state.imageY = entry.y;
        state.imageScale = entry.scale;
        state.imageRotation = entry.rotation !== undefined ? entry.rotation : 0;

        TokenCanvas._compositedImageDirty = true;
        TokenCanvas._zonesDirty = true;
        TokenCanvas._shadowDirty = true;
        TokenCanvas._isErasing = false;
        TokenCanvas._workerQueues.pink = [];
        TokenCanvas._workerQueues.image = [];
        if (TokenCanvas._workerBatchTimer) {
            clearTimeout(TokenCanvas._workerBatchTimer);
            TokenCanvas._workerBatchTimer = null;
        }
        // _workerInFlight не сбрасываем: устаревшие ответы отбрасываются
        // по gen и декрементируют счётчик сами
        TokenCanvas._maskGen++;
        TokenCanvas.pendingErasePoints = [];
        TokenCanvas._strokeDirtyRect = null;
        TokenCanvas._strokeFullDirty = false;
        TokenCanvas._strokeChanged = false;
        state.effectsEnabled = true;
        if (TokenCanvas.eraseAnimationId) {
            cancelAnimationFrame(TokenCanvas.eraseAnimationId);
            TokenCanvas.eraseAnimationId = null;
        }
        TokenCanvas.isErasing = false;

        // Маски переписаны напрямую — обновляем альфа-зеркала воркера
        TokenCanvas._pushMaskToWorker(true);
        TokenCanvas._pushMaskToWorker(false);

        TokenCanvas.updateScaleUI();
        TokenCanvas.updateRotationUI();
        TokenCanvas.render();
    },

    undo() {
        if (state.historyIndex > 0) {
            state.historyIndex--;
            this.restore(state.history[state.historyIndex]);
            toast('Действие отменено');
        } else {
            toast('Нечего отменять', true);
        }
        if (typeof TokenEditor !== 'undefined' && TokenEditor._updateUndoButtons) TokenEditor._updateUndoButtons();
    },

    redo() {
        if (state.historyIndex < state.history.length - 1) {
            state.historyIndex++;
            this.restore(state.history[state.historyIndex]);
            toast('Повтор');
        } else {
            toast('Нечего повторять', true);
        }
        if (typeof TokenEditor !== 'undefined' && TokenEditor._updateUndoButtons) TokenEditor._updateUndoButtons();
    },

    init() {
        state.history = [];
        state.historyIndex = -1;
        this.save();
    }
};
