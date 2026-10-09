/**
 * tokenEmbed.js — мост Tokenatra ↔ родительское окно (модуль FoundryVTT).
 *
 * Активен ТОЛЬКО при ?embed=1 и origin=http(s)://localhost|127.0.0.1|::1:*
 * (передаёт модуль). Без валидного origin мост не поднимается — произвольная
 * страница не может встроить приложение и получить экспорт. Никаких
 * targetOrigin:'*'.
 *
 * Протокол (connectorApi 1):
 *   входящие:  tokenatra:ping → pong; tokenatra:load {dataUrl, filename};
 *              tokenatra:restore {originalUrl, filename, settings}
 *   исходящие: tokenatra:ready {version}; tokenatra:export {url, filename,
 *              format, source}; tokenatra:error {message}
 */
var TokenEmbed = (function() {
    var cfg = window.__EMBED__ || {};
    var active = false;
    var parentOrigin = null;

    function localhostOrigin(o) {
        var u = validHttpUrl(o);
        if (!u) return null;
        var h = (u.hostname || '').toLowerCase();
        // URL для IPv6 отдаёт hostname с квадратными скобками
        if (h === 'localhost' || h === '127.0.0.1' || h === '::1' || h === '[::1]') return u.origin;
        return null;
    }

    function validHttpUrl(o) {
        try {
            var u = new URL(o);
            if (u.protocol === 'http:' || u.protocol === 'https:') return u;
        } catch (e) { /* невалидный origin */ }
        return null;
    }

    // Родитель моста: localhost-семейство ИЛИ Origin из серверного allowlist
    // (window.__EMBED_ALLOWED__ — Foundry за своим доменом/прокси)
    function acceptParentOrigin(o) {
        var local = localhostOrigin(o);
        if (local) return local;
        var u = validHttpUrl(o);
        if (!u) return null;
        var allowed = window.__EMBED_ALLOWED__ || [];
        for (var i = 0; i < allowed.length; i++) {
            var au = validHttpUrl(allowed[i]);
            if (au && au.origin === u.origin) return u.origin;
        }
        return null;
    }

    if (cfg.embed) {
        if (window.__STUDIO_BUILT__ && window.parent && window.parent !== window) {
            // Hosted-студия (srcdoc): same-origin с миром — origin берём у родителя
            try { parentOrigin = window.parent.location.origin; } catch (err) { parentOrigin = null; }
        } else {
            parentOrigin = acceptParentOrigin(cfg.parentOrigin || '');
        }
        // Без parentOrigin мост неактивен: отвечать некому, '*' запрещён
        active = !!parentOrigin;
    }

    function send(msg) {
        if (!active) return;
        try {
            window.parent.postMessage(msg, parentOrigin);
        } catch (e) { /* родитель закрылся — молча */ }
    }

    function sanitizeName(name, fallback) {
        // Кириллицу сохраняем (русскоязычные имена файлов), режем только
        // разделители путей и спецсимволы
        var base = String(name || fallback || 'token').replace(/[\\/:*?"<>|]+/g, '_').trim();
        if (!base) base = fallback || 'token';
        return base.slice(0, 64);
    }

    // ===== Снапшот токена для флагов Foundry =====

    // Все применённые пункты вида токена на момент сохранения
    function buildTokenatraSnapshot() {
        try {
            var ringEl = document.querySelector('.ring-thumb.active');
            var presetName = null;
            if (state.currentPreset >= 0 && state.eraserPresets && state.eraserPresets[state.currentPreset]) {
                presetName = state.eraserPresets[state.currentPreset].name;
            }
            return {
                v: 1,
                savedAt: new Date().toISOString(),
                scale: state.imageScale,
                rotation: state.imageRotation,
                ring: ringEl ? (ringEl.dataset.ringName || null) : null,
                preset: presetName,
                shadow: {
                    enabled: !!state.dropShadowEnabled,
                    angle: AppConfig.dropShadow.angle,
                    distance: AppConfig.dropShadow.distance,
                    blur: AppConfig.dropShadow.blur,
                    opacity: AppConfig.dropShadow.opacity
                },
                colorCorrection: {
                    enabled: !!state.colorCorrectionEnabled,
                    saturation: AppConfig.colorCorrection.saturation,
                    lightness: AppConfig.colorCorrection.lightness
                }
            };
        } catch (err) { return null; }
    }

    // Оригинал (исходник до удаления фона) — PNG dataURL для файла во Foundry
    function getOriginalDataUrl() {
        try {
            var img = state.userImageOriginal;
            if (!img || !img.naturalWidth) return null;
            var c = document.createElement('canvas');
            c.width = img.naturalWidth;
            c.height = img.naturalHeight;
            c.getContext('2d').drawImage(img, 0, 0);
            return c.toDataURL('image/png');
        } catch (err) { return null; }
    }

    // djb2 + длина: дешёвый «отпечаток» dataURL для сверки оригиналов
    // между сохранениями (чтобы не дублировать одинаковый файл во Foundry)
    function hashString(s) {
        var h = 5381;
        for (var i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
        return 'h' + (h >>> 0).toString(36) + '-' + s.length.toString(36);
    }

    // Применение снапшота из флагов: слайдеры/чекбоксы через штатные
    // обработчики (dispatch input/change), кольцо и пресет — кликом
    function applySnapshotSettings(s) {
        if (!s) return;
        if (typeof s.scale === 'number') {
            var sc = Math.round(s.scale * 100);
            var scaleSlider = document.getElementById('scaleSlider');
            if (scaleSlider) { scaleSlider.value = sc; scaleSlider.dispatchEvent(new Event('input')); }
            var scaleInput = document.getElementById('scaleInput');
            if (scaleInput) scaleInput.value = sc;
        }
        if (typeof s.rotation === 'number') {
            var rotSlider = document.getElementById('rotationSlider');
            if (rotSlider) { rotSlider.value = s.rotation; rotSlider.dispatchEvent(new Event('input')); }
            var rotInput = document.getElementById('rotationInput');
            if (rotInput) rotInput.value = s.rotation;
        }
        if (s.ring) {
            var thumb = document.querySelector('.ring-thumb[data-ring-name="' + String(s.ring).replace(/"/g, '\\"') + '"]');
            if (thumb) thumb.click();
        }
        if (s.preset && typeof TokenPresets !== 'undefined' && state.eraserPresets) {
            var idx = -1;
            for (var i = 0; i < state.eraserPresets.length; i++) {
                if (state.eraserPresets[i] && state.eraserPresets[i].name === s.preset) { idx = i; break; }
            }
            if (idx >= 0 && typeof TokenPresets.apply === 'function') TokenPresets.apply(idx);
        }
        if (s.shadow) {
            var dsCheck = document.getElementById('dropShadowCheck');
            if (dsCheck) { dsCheck.checked = !!s.shadow.enabled; dsCheck.dispatchEvent(new Event('change')); }
            [['shadowAngle', 'angle'], ['shadowDistance', 'distance'], ['shadowBlur', 'blur'], ['shadowOpacity', 'opacity']].forEach(function(pair) {
                var v = s.shadow[pair[1]];
                if (typeof v !== 'number') return;
                var el = document.getElementById(pair[0]);
                if (el) {
                    el.value = pair[0] === 'shadowOpacity' ? Math.round(v * 100) : v;
                    el.dispatchEvent(new Event('input'));
                }
            });
        }
        if (s.colorCorrection) {
            var ccCheck = document.getElementById('colorCorrectionCheck');
            if (ccCheck) { ccCheck.checked = !!s.colorCorrection.enabled; ccCheck.dispatchEvent(new Event('change')); }
            [['ccSaturation', 'saturation'], ['ccLightness', 'lightness']].forEach(function(pair) {
                var v = s.colorCorrection[pair[1]];
                if (typeof v !== 'number') return;
                var el = document.getElementById(pair[0]);
                if (el) { el.value = v; el.dispatchEvent(new Event('input')); }
            });
        }
    }

    // Путь во Foundry, откуда загружено текущее изображение (арт актора,
    // выбор файла, прошлый оригинал). null — загрузка локальным файлом:
    // тогда оригинал при сохранении надо выгружать во Foundry
    var foundrySourcePath = null;

    // Восстановление из флагов актора: оригинал в редактор, поверх — настройки
    function restoreFromSnapshot(originalUrl, filename, settings, sourcePath) {
        loadIntoEditor(originalUrl, filename || 'original.png', sourcePath);
        if (!settings) return;
        var t0 = Date.now();
        var iv = setInterval(function() {
            // loadImage асинхронен: настройки применяем, когда изображение в редакторе
            if (!state.userImage && Date.now() - t0 < 10000) return;
            clearInterval(iv);
            try { applySnapshotSettings(settings); } catch (err) {
                send({ type: 'tokenatra:error', message: 'restore failed: ' + err });
            }
        }, 200);
    }

    function switchToTokenPanel() {
        // Как в initTabs(): классы + aria-hidden + inert — иначе панель
        // видима, но неинтерактивна
        document.querySelectorAll('.nav-btn[data-mode]').forEach(function(t) {
            var isToken = t.dataset.mode === 'token';
            t.classList.toggle('active', isToken);
            t.setAttribute('aria-selected', String(isToken));
        });
        document.querySelectorAll('.panel').forEach(function(p) {
            var activePanel = p.id === 'tokenPanel';
            p.classList.toggle('active', activePanel);
            p.setAttribute('aria-hidden', String(!activePanel));
            p.inert = !activePanel;
        });
        var panel = document.getElementById('tokenPanel');
        if (panel && panel.focus) panel.focus({ preventScroll: true });
    }

    function loadIntoEditor(dataUrl, filename, sourcePath) {
        fetch(dataUrl).then(function(r) { return r.blob(); }).then(function(blob) {
            var name = sanitizeName(filename, 'image');
            var file = new File([blob], name, { type: blob.type || 'image/png' });
            switchToTokenPanel();
            if (typeof TokenCanvas !== 'undefined' && TokenCanvas.loadImage) {
                TokenCanvas.loadImage(file);
            }
            // Изображение из Foundry: при сохранении оригинал не дублируется
            foundrySourcePath = sourcePath || null;
            var stem = name.replace(/\.[^.]+$/, '') || 'token';
            state.tokenFileName = stem;
            var input = document.getElementById('tokenFileName');
            if (input) input.value = stem;
        }).catch(function(err) {
            send({ type: 'tokenatra:error', message: 'Не удалось загрузить изображение: ' + err });
        });
    }

    function uploadBlob(blob, filename, format, source) {
        var fd = new FormData();
        try {
            fd.append('image', new File([blob], filename, { type: blob.type }), filename);
        } catch (e) {
            fd.append('image', blob, filename);
        }
        toast(I18n.t('Отправка…'));
        fetch('/embed_export', { method: 'POST', body: fd })
            .then(function(r) { return r.json().then(function(d) { return { ok: r.ok, d: d }; }); })
            .then(function(res) {
                if (res.ok && res.d && res.d.url) {
                    toast(I18n.t('Отправлено в Foundry'));
                    send({
                        type: 'tokenatra:export',
                        url: res.d.url,
                        filename: filename,
                        format: format,
                        source: source
                    });
                } else {
                    toast(I18n.t('Отправка не удалась'), true);
                    send({ type: 'tokenatra:error', message: (res.d && res.d.error) || 'export failed' });
                }
            })
            .catch(function(err) {
                toast(I18n.t('Отправка не удалась'), true);
                send({ type: 'tokenatra:error', message: String(err) });
            });
    }

    function exportEditor(withRing) {
        exportToFoundry(withRing ? 'token' : 'subject');
    }

    function exportToFoundry(kind) {
        if (!state.userImage) {
            toast(I18n.t('Сначала загрузите изображение'), true);
            return;
        }
        var withRing = kind === 'token';
        var result = TokenCanvas.renderForSave(withRing);
        if (!result || !result.canvas) return;
        var fmt = (AppConfig.saveSettings && AppConfig.saveSettings.format) || 'webp';
        if (['webp', 'png', 'jpg'].indexOf(fmt) === -1) fmt = 'webp';
        var mime = fmt === 'png' ? 'image/png' : (fmt === 'jpg' ? 'image/jpeg' : 'image/webp');
        var quality = fmt === 'png' ? undefined
            : Math.min(1, Math.max(0.01, (parseInt(AppConfig.saveSettings.compression) || 95) / 100));
        var base = (state.tokenFileName.trim() || 'token') + '-' + kind;
        // JPEG без альфы: прозрачный фон стал бы чёрным — композитим на белый
        // (тот же фолбэк, что и TokenCanvas.save)
        var outCanvas = result.canvas;
        if (fmt === 'jpg') {
            var flat = document.createElement('canvas');
            flat.width = result.canvas.width;
            flat.height = result.canvas.height;
            var fctx = flat.getContext('2d');
            fctx.fillStyle = '#ffffff';
            fctx.fillRect(0, 0, flat.width, flat.height);
            fctx.drawImage(result.canvas, 0, 0);
            outCanvas = flat;
        }
        // Масштаб для ring.subject/scaleX/scaleY — ТОЛЬКО значение из раздела
        // «Экспорт» (×1/×2/×3; renderForSave уже разрешил «авто» в finalSize)
        var exportScale = state.saveQuality
            ? Math.round((result.finalSize / state.saveQuality) * 1000) / 1000
            : 1;
        // Реальный масштаб для обеих кнопок (Token и Subject): модуль кладёт
        // его в prototypeToken scale и ring.subject.scale актора
        var metrics = { scale: exportScale, scaleX: exportScale, scaleY: exportScale };
        outCanvas.toBlob(function(blob) {
            if (blob) uploadAndSend(blob, base + '.' + fmt, fmt, {
                kind: kind,
                scale: metrics.scale,
                scaleX: metrics.scaleX,
                scaleY: metrics.scaleY
            });
        }, mime, quality);
    }

    function uploadAndSend(blob, filename, format, extra) {
        var fd = new FormData();
        try {
            fd.append('image', new File([blob], filename, { type: blob.type }), filename);
        } catch (e) {
            fd.append('image', blob, filename);
        }
        toast(I18n.t('Отправка…'));
        fetch('/embed_export', { method: 'POST', body: fd })
            .then(function(r) { return r.json().then(function(d) { return { ok: r.ok, d: d }; }); })
            .then(function(res) {
                if (res.ok && res.d && res.d.url) {
                    toast(I18n.t('Отправлено в Foundry'));
                    var saveMsg = Object.assign({
                        type: 'tokenatra:save',
                        url: res.d.url,
                        filename: filename,
                        format: format
                    }, extra || {});
                    // Токен/субъект: во флаги актора — снапшот настроек и оригинал.
                    // Оригинал уже во Foundry (загружен оттуда) — шлём только путь,
                    // модуль не будет создавать дубликат файла
                    var kind = extra && extra.kind;
                    if (kind === 'token' || kind === 'subject') {
                        saveMsg.tokenatra = buildTokenatraSnapshot();
                        if (foundrySourcePath) {
                            saveMsg.originalFoundryPath = foundrySourcePath;
                        } else {
                            saveMsg.original = getOriginalDataUrl();
                            saveMsg.originalHash = saveMsg.original ? hashString(saveMsg.original) : null;
                        }
                    }
                    send(saveMsg);
                } else {
                    toast(I18n.t('Отправка не удалась'), true);
                    send({ type: 'tokenatra:error', message: (res.d && res.d.error) || 'export failed' });
                }
            })
            .catch(function(err) {
                toast(I18n.t('Отправка не удалась'), true);
                send({ type: 'tokenatra:error', message: String(err) });
            });
    }

    function initEmbedOpenButton() {
        // «Открыть» и клик по строке файла — браузер файлов Foundry.
        // Capture на document: перехватываем раньше обработчиков приложения
        // (иначе клик по строке ушёл бы в /pick_image_to_open и падал)
        document.addEventListener('click', function(e) {
            if (e.button !== 0) return;
            if (!e.target.closest('#fileOpenBtn, #tokenDropzone')) return;
            e.preventDefault();
            e.stopImmediatePropagation();
            send({ type: 'tokenatra:pick-file' });
        }, true);
    }

    function initEmbedPortraitButton() {
        // Портрет: рендер → загрузка → модуль сохранит в папку и назначит
        // портретом актёра (перехват до нативного диалога из PortraitGenerator)
        var btn = document.getElementById('portraitSaveBtn');
        if (!btn || btn.dataset.embedBound) return;
        btn.dataset.embedBound = '1';
        btn.addEventListener('click', function(e) {
            e.preventDefault();
            e.stopImmediatePropagation();
            try {
                // Портрет рисуется из вырезанной/исходной версии — сам userImage
                // может отсутствовать, валиден именно источник портрета
                if (!PortraitGenerator.getImage()) {
                    toast(I18n.t('Сначала загрузите изображение'), true);
                    return;
                }
                var out = PortraitGenerator.renderForSave();
                var name = (state.tokenFileName.trim() || 'portrait') + '-portrait.webp';
                out.toBlob(function(blob) {
                    if (blob) uploadAndSend(blob, name, 'webp', { kind: 'portrait' });
                }, 'image/webp', 0.95);
            } catch (err) {
                send({ type: 'tokenatra:error', message: String(err) });
            }
        }, true);
    }

    function createRemoverButton(id) {
        var btn = document.createElement('button');
        btn.className = 'download-btn';
        btn.dataset.tooltip = I18n.t('Отправить в Foundry');
        btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M22 2L11 13"/><path d="M22 2l-7 20-4-9-9-4 20-7z"/></svg>';
        btn.onclick = function() {
            var data = state.results.get(id);
            if (!data || !data.blob) { toast(I18n.t('Результат не найден'), true); return; }
            uploadBlob(data.blob, data.name || ('result.' + (data.format || 'webp')), data.format || 'webp', 'remover');
        };
        return btn;
    }

    function rebindSaveButtons() {
        // Родные кнопки сохранения («С кольцом»/«Без кольца») переименовываем
        // в «Токен»/«Субъект» и перенаправляем на экспорт в мир — иконки и
        // стиль остаются от Tokenatra. Capture-фаза бьёт раньше onclick,
        // который TokenEditor назначает позже.
        document.addEventListener('click', function(e) {
            if (e.button !== 0) return;
            var btn = e.target.closest('#saveWithRing, #saveWithoutRing');
            if (!btn) return;
            e.preventDefault();
            e.stopImmediatePropagation();
            exportToFoundry(btn.id === 'saveWithRing' ? 'token' : 'subject');
        }, true);
        var rename = function(id, label) {
            var btn = document.getElementById(id);
            if (!btn) return;
            var svg = btn.querySelector('svg');
            btn.textContent = '';
            if (svg) btn.appendChild(svg);
            btn.appendChild(document.createTextNode(' ' + label));
        };
        rename('saveWithRing', I18n.t('Токен'));
        rename('saveWithoutRing', I18n.t('Субъект'));
    }

    function whenAppReady(cb) {
        if (window.__APP_READY__) { cb(); return; }
        var t = setInterval(function() {
            if (window.__APP_READY__) {
                clearInterval(t);
                cb();
            }
        }, 50);
    }

    function initTitlebarDrag() {
        // Тайтлбар Tokenatra = шапка окна Foundry: mousedown сообщает родителю,
        // тот глушит pointer-events у iframe и тянет окно сам
        var tb = document.querySelector('.title-bar');
        if (!tb || tb.dataset.embedDragBound) return;
        tb.dataset.embedDragBound = '1';
        tb.addEventListener('mousedown', function(e) {
            if (e.button !== 0) return;
            if (e.target.closest('.tb-nav, .window-controls, button, a, input')) return;
            e.preventDefault();
            // Координаты нужны в системе СТРАНИЦЫ (родителя): clientX внутри
            // iframe отсчитывается от угла iframe. frameElement доступен
            // (same-origin) и даёт смещение рамки в координатах страницы.
            var fr = window.frameElement ? window.frameElement.getBoundingClientRect() : { left: 0, top: 0 };
            send({ type: 'tokenatra:drag-start', x: e.clientX + fr.left, y: e.clientY + fr.top });
        });
    }

    function initEmbedWindowControls() {
        // В embed из кнопок окна остаётся только ✕: она закрывает ОКНО FOUNDRY
        // (перехват в capture-фазе до обработчиков main.js, иначе клик ушёл бы
        // в /api/window/close десктоп-приложения)
        var closeBtn = document.getElementById('wcClose');
        if (!closeBtn || closeBtn.dataset.embedBound) return;
        closeBtn.dataset.embedBound = '1';
        closeBtn.addEventListener('click', function(e) {
            e.preventDefault();
            e.stopImmediatePropagation();
            send({ type: 'tokenatra:window', action: 'close' });
        }, true);
    }

    function init() {
        if (!active) return;
        window.addEventListener('message', function(e) {
            if (e.origin !== parentOrigin) return;
            var d = e.data;
            if (!d || typeof d !== 'object') return;
            if (d.type === 'tokenatra:ping') {
                send({ type: 'tokenatra:pong', version: window.__APP_VERSION, connectorApi: 1 });
            } else if (d.type === 'tokenatra:load' && d.dataUrl) {
                whenAppReady(function() {
                    loadIntoEditor(d.dataUrl, d.filename, d.sourcePath);
                    wrapLoadImage();
                });
            } else if (d.type === 'tokenatra:restore' && d.originalUrl) {
                whenAppReady(function() {
                    restoreFromSnapshot(d.originalUrl, d.filename, d.settings, d.sourcePath);
                    wrapLoadImage();
                });
            }
        });
        rebindSaveButtons();
        initTitlebarDrag();
        initEmbedWindowControls();
        initEmbedOpenButton();
        initEmbedPortraitButton();
        whenAppReady(function() {
            wrapLoadImage();
            // Сообщаем готовность после полной инициализации приложения;
            // tokenatra:load, пришедший раньше, ждёт в whenAppReady сам
            send({ type: 'tokenatra:ready', version: window.__APP_VERSION, connectorApi: 1 });
        });
    }

    // Локальная загрузка (файл/дроп/паста) сбрасывает происхождение из
    // Foundry — тогда при сохранении оригинал выгружается как новый
    function wrapLoadImage() {
        if (typeof TokenCanvas === 'undefined' || !TokenCanvas.loadImage || TokenCanvas.__tkcWrapped) return;
        var orig = TokenCanvas.loadImage;
        TokenCanvas.loadImage = function(file, path) {
            foundrySourcePath = null;
            return orig.call(this, file, path);
        };
        TokenCanvas.__tkcWrapped = true;
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }

    return {
        isActive: function() { return active; },
        pickFile: function() { send({ type: 'tokenatra:pick-file' }); },
        createRemoverButton: createRemoverButton,
        exportEditor: exportEditor
    };
})();
