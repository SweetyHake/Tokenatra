/**
 * tokenTray.js — экономный режим в системном трее.
 *
 * Гасит фоновые rAF/таймеры и освобождает кэши канваса, пока окно скрыто;
 * работа редактора (изображение, маски, воркер-зеркала) сохраняется —
 * инвариант TokenCanvas._pushMaskToWorker не нарушается.
 *
 * Основной триггер — visibilitychange (WebView2 помечает скрытое окно);
 * дубль — явный вызов enterIdle/exitIdle из app.py через evaluate_js.
 */
var TokenTray = (function() {
    var idle = false;
    var fullIdle = false;

    function cancelCanvasWork() {
        var tc = (typeof TokenCanvas !== 'undefined') ? TokenCanvas : null;
        if (!tc) return;
        // Незавершённое стирание не трогаем: воркер должен доработать пачку
        if (!tc.isErasing) {
            if (tc._renderRafId) {
                cancelAnimationFrame(tc._renderRafId);
                tc._renderRafId = null;
            }
            if (tc.eraseAnimationId) {
                cancelAnimationFrame(tc.eraseAnimationId);
                tc.eraseAnimationId = null;
            }
        }
        if (tc._workerBatchTimer) {
            clearTimeout(tc._workerBatchTimer);
            tc._workerBatchTimer = null;
        }
        if (tc._eraserBrushTimer) {
            clearTimeout(tc._eraserBrushTimer);
            tc._eraserBrushTimer = null;
        }
        // Освобождаем растровые кэши (пересоздадутся лениво при рендере)
        if (tc._freeEffectCaches) tc._freeEffectCaches();
        if (tc.invalidateAllCaches) tc.invalidateAllCaches();
        tc._compositedImageCache = null;
        tc._compositedImageDirty = true;
        tc._ccImageCache = null;
        tc._ccCacheKey = null;
        tc._imageBrushCache = null;
        tc._imageBrushCacheSize = -1;
    }

    function trimHistory() {
        if (typeof TokenHistory === 'undefined' || !TokenHistory.trimTo) return;
        // Честная экономия: снапшот ~1 МБ; в трее держим 10 (undo сокращается)
        TokenHistory.trimTo(10);
        if (typeof TokenEditor !== 'undefined' && TokenEditor._updateUndoButtons) {
            TokenEditor._updateUndoButtons();
        }
    }

    function enter(full) {
        if (!idle) {
            idle = true;
            if (state.effectsTimeout) {
                clearTimeout(state.effectsTimeout);
                state.effectsTimeout = null;
            }
            cancelCanvasWork();
        }
        // «Полный» экономный режим (трей) дополнительно режет историю отмен;
        // обычное сворачивание в панель задач — только rAF/кэши
        if (full && !fullIdle) {
            fullIdle = true;
            trimHistory();
        }
    }

    function exit() {
        if (!idle) return;
        idle = false;
        fullIdle = false;
        // scheduleEffects мог оставить effectsEnabled=false — без возврата
        // эффекты (тень/цветокоррекция) не отрисуются после восстановления
        state.effectsEnabled = true;
        // Отложенная цветокоррекция: render взял бы устаревший _ccImageCache,
        // догоняем форсом пересчёта эффектов
        if (TokenCanvas._deferCC) TokenCanvas.scheduleEffects(false);
        else if (typeof TokenCanvas !== 'undefined' && TokenCanvas.requestRender) {
            TokenCanvas.requestRender();
        }
    }

    document.addEventListener('visibilitychange', function() {
        if (document.hidden) enter(false);
        else exit();
    });

    return {
        // enterIdle(true) — из app.py при сворачивании в трей (полный режим);
        // visibilitychange зовёт enter(false) сам (лёгкий режим)
        enterIdle: function(full) { enter(full === true); },
        exitIdle: exit,
        isIdle: function() { return idle; }
    };
})();
