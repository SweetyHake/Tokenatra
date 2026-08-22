// Протокол v2: воркер хранит альфа-зеркала масок (1 байт/пиксель) и защиту.
// Пачка несёт только координаты штрихов (Int32Array) — ноль чтений маски и
// ноль копий буферов на главном потоке в горячем пути стирания.

// Кисть кэшируется вместе с предрасчётом непрозрачных строк:
// радиальный градиент оставляет ~21% углов квадрата полностью
// прозрачными — их можно не обходить вовсе
function buildBrush(brushData, w, h) {
    var brush = brushData ? new Uint8ClampedArray(brushData) : null;
    var rows = null;
    if (brush && w > 0 && h > 0) {
        rows = new Array(h);
        for (var y = 0; y < h; y++) {
            var base = y * w;
            var s = -1, e = -1;
            for (var x = 0; x < w; x++) {
                if (brush[(base + x) * 4 + 3] > 0) {
                    if (s < 0) s = x;
                    e = x;
                }
            }
            rows[y] = s < 0 ? null : [s, e + 1];
        }
    }
    return { data: brush, w: w, h: h, rows: rows };
}

function normBytes(arr, Ctor) {
    if (!arr) return null;
    // Принимаем и TypedArray, и сырой ArrayBuffer: индексация ArrayBuffer
    // молча возвращает undefined (защита/маска бы тихо перестали работать)
    if (ArrayBuffer.isView(arr)) {
        return (arr instanceof Ctor) ? arr : new Ctor(arr.buffer, arr.byteOffset, arr.byteLength / Ctor.BYTES_PER_ELEMENT | 0);
    }
    return new Ctor(arr);
}

self._masks = { pink: null, image: null };

self.onmessage = function(e) {
    var data = e.data;

    // Защита загружается в воркер один раз (при загрузке/изменении маски),
    // а не сэмплируется postMessage-ом на каждую точку штриха с главного потока.
    if (data.type === 'setProtection') {
        self._protData = normBytes(data.protData, Uint8Array);
        self._protSize = data.protSize || 0;
        return;
    }

    // Альфа-зеркало маски: полный снапшот при загрузке/undo/сбросе/пресете
    if (data.type === 'setMask') {
        var snap = data.alphaData
            ? { a: normBytes(data.alphaData, Uint8ClampedArray), w: data.width, h: data.height }
            : null;
        self._masks[data.mode] = snap;
        return;
    }

    // Кисть кэшируется в воркере один раз на штрих (setBrush)
    if (data.type === 'setBrush') {
        var cached = buildBrush(data.brushData, data.brushWidth || 0, data.brushHeight || 0);
        self._brush = cached.data;
        self._brushW = cached.w;
        self._brushH = cached.h;
        self._brushRows = cached.rows;
        return;
    }

    if (data.type !== 'applyBrushBatch') return;

    var respond = function(changed, maskBuf, rx, ry, rw, rh) {
        self.postMessage({
            type: 'brushDone',
            id: data.id,
            gen: data.gen,
            mode: data.mode,
            changed: changed,
            dirtyRect: data.dirtyRect || null,
            maskData: maskBuf,
            regionX: rx, regionY: ry, regionWidth: rw, regionHeight: rh
        }, maskBuf ? [maskBuf] : []);
    };

    var m = self._masks[data.mode];
    if (!m || !m.a) { respond(false, null, 0, 0, 0, 0); return; }

    var brush = self._brush, brushW = self._brushW, brushH = self._brushH, brushRows = self._brushRows;
    if (!brush || !brushRows) { respond(false, null, 0, 0, 0, 0); return; }

    // Штрихи упакованы в Float64Array: [cx, cy, drawX, drawY, flags]×N.
    // cx/cy — float без потери точности (координата сэмпла защиты),
    // drawX/drawY/flags — целые
    var strokes = new Float64Array(data.strokeData);
    var strokeCount = data.strokeCount | 0;

    var prot = self._protData;
    var protSize = self._protSize;
    var effScale = data.effectiveScale || 1;
    var ang = (data.imageRotation || 0) * Math.PI / 180;
    var cosR = Math.cos(ang);
    var sinR = Math.sin(ang);
    var half = brushW / 2;

    // Общий регион пачки = объединение bbox всех штрихов (+пад 2px),
    // считается здесь: главный поток регион не знает и не читает маску
    var pad = 2;
    var mw = m.w, mh = m.h;
    var minX = mw, minY = mh, maxX = 0, maxY = 0;
    for (var si = 0; si < strokeCount; si++) {
        var drawX = strokes[si * 5 + 2];
        var drawY = strokes[si * 5 + 3];
        var rx1 = Math.max(0, drawX - pad);
        var ry1 = Math.max(0, drawY - pad);
        var rx2 = Math.min(mw, drawX + brushW + pad);
        var ry2 = Math.min(mh, drawY + brushH + pad);
        if (rx2 <= rx1 || ry2 <= ry1) continue;
        if (rx1 < minX) minX = rx1;
        if (ry1 < minY) minY = ry1;
        if (rx2 > maxX) maxX = rx2;
        if (ry2 > maxY) maxY = ry2;
    }
    if (maxX <= minX || maxY <= minY) { respond(false, null, 0, 0, 0, 0); return; }

    var regionW = maxX - minX;
    var regionH = maxY - minY;

    // Применение кистей прямо к зеркалу (альфа, 1 байт/px)
    var mask = m.a;
    var changed = false;

    for (var s = 0; s < strokeCount; s++) {
        var stCx = strokes[s * 5];
        var stCy = strokes[s * 5 + 1];
        var drawX2 = strokes[s * 5 + 2];
        var drawY2 = strokes[s * 5 + 3];
        var restore = strokes[s * 5 + 4] === 1;

        // Кисть обрезается по границам региона один раз на штрих —
        // вместо сравнений на каждый пиксель
        var by0 = drawY2 < minY ? minY - drawY2 : 0;
        var by1 = drawY2 + brushH > minY + regionH ? minY + regionH - drawY2 : brushH;
        var bx0 = drawX2 < minX ? minX - drawX2 : 0;
        var bx1 = drawX2 + brushW > minX + regionW ? minX + regionW - drawX2 : brushW;

        for (var by = by0; by < by1; by++) {
            var span = brushRows[by];
            if (!span) continue;
            var xs = span[0] > bx0 ? span[0] : bx0;
            var xe = span[1] < bx1 ? span[1] : bx1;
            if (xs >= xe) continue;

            var my = drawY2 + by;
            // Зеркало — полная маска, индекс АБСОЛЮТНЫЙ (не регион-относительный):
            // именно здесь v2 ломался против v1 при ненулевом origin региона
            var maskIdx = my * mw + drawX2 + xs;
            var brushIdx = by * brushW + xs;
            var dyb = by - half;

            // Защита живёт в координатах канваса: пиксель кисти (bx, by)
            // соответствует точке st + effectiveScale·Rot(+rot)·(bx-half).
            // Отображение линейно по bx — старт и шаг на строку вместо
            // полной математики на каждый пиксель
            var pxc = 0, pyc = 0, stepX = 0, stepY = 0;
            if (prot) {
                var dxb = xs - half;
                pxc = stCx + effScale * (cosR * dxb - sinR * dyb);
                pyc = stCy + effScale * (sinR * dxb + cosR * dyb);
                stepX = effScale * cosR;
                stepY = effScale * sinR;
            }

            for (var bx = xs; bx < xe; bx++) {
                var brushAlpha = brush[brushIdx * 4 + 3];
                if (brushAlpha !== 0) {
                    var skip = false;
                    if (prot) {
                        var pxi = Math.round(pxc);
                        var pyi = Math.round(pyc);
                        if (pxi >= 0 && pyi >= 0 && pxi < protSize && pyi < protSize &&
                            prot[pyi * protSize + pxi] > 128) skip = true;
                    }
                    if (!skip) {
                        var cur = mask[maskIdx];
                        // Uint8ClampedArray сам клампит 0..255
                        mask[maskIdx] = restore ? cur + brushAlpha : cur - brushAlpha;
                        // Читаем записанное значение: присвоение округляет,
                        // сравнение с float давало ложные changed
                        if (mask[maskIdx] !== cur) changed = true;
                    }
                }
                maskIdx++;
                brushIdx++;
                pxc += stepX;
                pyc += stepY;
            }
        }
    }

    if (!changed) { respond(false, null, minX, minY, regionW, regionH); return; }

    // Ответ: RGBA региона (RGB=255 — маски всегда белые, меняется только альфа)
    var n = regionW * regionH;
    var out = new Uint8ClampedArray(n * 4);
    var src = minY * mw + minX;
    var dst = 0;
    for (var y = 0; y < regionH; y++) {
        for (var x = 0; x < regionW; x++) {
            out[dst] = 255; out[dst + 1] = 255; out[dst + 2] = 255;
            out[dst + 3] = mask[src];
            dst += 4;
            src++;
        }
        src += mw - regionW;
    }

    respond(true, out.buffer, minX, minY, regionW, regionH);
};
