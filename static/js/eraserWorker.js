self.onmessage = function(e) {
    var data = e.data;

    // Защита загружается в воркер один раз (при загрузке/изменении маски),
    // а не сэмплируется postMessage-ом на каждую точку штриха с главного потока.
    if (data.type === 'setProtection') {
        self._protData = data.protData || null;
        self._protSize = data.protSize || 0;
        return;
    }

    // Кисть кэшируется в воркере один раз на штрих (setBrush), а не
    // передаётся с каждой пачкой — экономит копию буфера на каждый батч.
    if (data.type === 'setBrush') {
        self._brush = data.brushData ? new Uint8ClampedArray(data.brushData) : null;
        self._brushW = data.brushWidth || 0;
        self._brushH = data.brushHeight || 0;
        return;
    }

    if (data.type !== 'applyBrushBatch') return;

    var mask = new Uint8ClampedArray(data.maskData);
    var brush;
    var brushW;
    var brushH;
    if (data.brushData) {
        brush = new Uint8ClampedArray(data.brushData);
        brushW = data.brushWidth;
        brushH = data.brushHeight;
    } else {
        brush = self._brush;
        brushW = self._brushW;
        brushH = self._brushH;
    }
    if (!brush) return;

    var regionX = data.regionX;
    var regionY = data.regionY;
    var regionW = data.regionWidth;
    var regionH = data.regionHeight;

    var prot = self._protData;
    var protSize = self._protSize;
    var effScale = data.effectiveScale || 1;
    var ang = (data.imageRotation || 0) * Math.PI / 180;
    var cosR = Math.cos(ang);
    var sinR = Math.sin(ang);
    var half = brushW / 2;
    var changed = false;

    for (var s = 0; s < data.strokes.length; s++) {
        var st = data.strokes[s];
        var drawX = st.drawX;
        var drawY = st.drawY;

        for (var by = 0; by < brushH; by++) {
            var my = drawY + by;
            if (my < regionY || my >= regionY + regionH) continue;
            var dyb = by - half;
            for (var bx = 0; bx < brushW; bx++) {
                var mx = drawX + bx;
                if (mx < regionX || mx >= regionX + regionW) continue;

                var brushAlpha = brush[(by * brushW + bx) * 4 + 3] / 255;
                if (brushAlpha <= 0) continue;

                // Защита живёт в координатах канваса: пиксель кисти (bx, by)
                // соответствует точке st + effectiveScale·Rot(+rot)·(bx-half)
                if (prot) {
                    var dxb = bx - half;
                    var pxc = st.cx + effScale * (cosR * dxb - sinR * dyb);
                    var pyc = st.cy + effScale * (sinR * dxb + cosR * dyb);
                    var pxi = Math.round(pxc);
                    var pyi = Math.round(pyc);
                    if (pxi >= 0 && pyi >= 0 && pxi < protSize && pyi < protSize &&
                        prot[pyi * protSize + pxi] > 128) continue;
                }

                var maskIdx = ((my - regionY) * regionW + (mx - regionX)) * 4 + 3;
                var cur = mask[maskIdx];
                var nv;
                if (st.restore) {
                    nv = Math.min(255, cur + brushAlpha * 255);
                } else {
                    nv = Math.max(0, cur - brushAlpha * 255);
                }
                mask[maskIdx] = nv;
                if (nv !== cur) changed = true;
            }
        }
    }

    self.postMessage({
        type: 'brushDone',
        id: data.id,
        gen: data.gen,
        mode: data.mode || 'image',
        changed: changed,
        dirtyRect: data.dirtyRect || null,
        maskData: mask.buffer,
        regionX: regionX,
        regionY: regionY,
        regionWidth: regionW,
        regionHeight: regionH
    }, [mask.buffer]);
};
