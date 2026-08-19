var HISTORY_STORE_SIZE = CONFIG.HISTORY_STORE_SIZE || 1024;

var TokenHistory = {
    _storeSize: HISTORY_STORE_SIZE,

    _cloneDownscaled(canvas) {
        if (!canvas) return null;
        var maxDim = Math.max(canvas.width, canvas.height);
        var scale = maxDim > this._storeSize ? this._storeSize / maxDim : 1;
        var w = Math.round(canvas.width * scale);
        var h = Math.round(canvas.height * scale);
        var copy = document.createElement('canvas');
        copy.width = w;
        copy.height = h;
        copy.getContext('2d').drawImage(canvas, 0, 0, w, h);
        return copy;
    },

    save() {
        if (!state.maskCanvas) return;

        if (state.historyIndex < state.history.length - 1) {
            var removed = state.history.splice(state.historyIndex + 1);
            removed.forEach(function(entry) {
                if (entry.mask) { entry.mask.width = 1; entry.mask.height = 1; entry.mask = null; }
                if (entry.imageMask) { entry.imageMask.width = 1; entry.imageMask.height = 1; entry.imageMask = null; }
            });
        }

        state.history.push({
            mask: this._cloneDownscaled(state.maskCanvas),
            imageMask: this._cloneDownscaled(state.imageMaskCanvas),
            x: state.imageX,
            y: state.imageY,
            scale: state.imageScale,
            rotation: state.imageRotation
        });

        if (state.history.length > CONFIG.MAX_HISTORY) {
            var old = state.history.shift();
            if (old.mask) { old.mask.width = 1; old.mask.height = 1; old.mask = null; }
            if (old.imageMask) { old.imageMask.width = 1; old.imageMask.height = 1; old.imageMask = null; }
            state.historyIndex = state.history.length - 1;
        } else {
            state.historyIndex++;
        }
        if (typeof TokenEditor !== 'undefined' && TokenEditor._updateUndoButtons) TokenEditor._updateUndoButtons();
    },

    restore(entry) {
        if (!entry || !entry.mask) return;

        var maskCtx = state.maskCanvas.getContext('2d');
        maskCtx.clearRect(0, 0, state.maskCanvas.width, state.maskCanvas.height);
        maskCtx.drawImage(entry.mask, 0, 0, state.maskCanvas.width, state.maskCanvas.height);

        if (entry.imageMask && state.imageMaskCanvas) {
            var imgMaskCtx = state.imageMaskCanvas.getContext('2d');
            imgMaskCtx.clearRect(0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
            imgMaskCtx.drawImage(entry.imageMask, 0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
        } else if (!entry.imageMask && state.imageMaskCanvas) {
            var imgMaskCtx = state.imageMaskCanvas.getContext('2d');
            imgMaskCtx.fillStyle = 'white';
            imgMaskCtx.fillRect(0, 0, state.imageMaskCanvas.width, state.imageMaskCanvas.height);
        }

        state.imageX = entry.x;
        state.imageY = entry.y;
        state.imageScale = entry.scale;
        state.imageRotation = entry.rotation !== undefined ? entry.rotation : 0;

        TokenCanvas._compositedImageDirty = true;
        TokenCanvas._zonesDirty = true;
        TokenCanvas._shadowDirty = true;
        TokenCanvas._isErasing = false;
        TokenCanvas._workerBatch = null;
        TokenCanvas._workerBatchTimer = null;
        TokenCanvas._workerPending = false;
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
        state.history.forEach(function(entry) {
            if (entry.mask) { entry.mask.width = 1; entry.mask.height = 1; entry.mask = null; }
            if (entry.imageMask) { entry.imageMask.width = 1; entry.imageMask.height = 1; entry.imageMask = null; }
        });
        state.history = [];
        state.historyIndex = -1;
        this.save();
    }
};
