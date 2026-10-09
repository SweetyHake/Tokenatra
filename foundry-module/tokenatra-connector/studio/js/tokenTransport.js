/**
 * tokenTransport.js — сетевой слой hosted-студии.
 *
 * Активируется ТОЛЬКО когда студия открыта как статика модуля Foundry
 * (?hosted=1): интерфейс раздаёт сам Foundry (тот же origin), а серверные
 * вызовы подменяются:
 *   /config, /update_status  → локально (localStorage / «обновлений нет»)
 *   /presets_list, /rings_list, /mask, /ring, /preset_file, /ring_file,
 *   /icon, /logo, /example   → статика из assets студии (manifest.json)
 *   /process, /health, /device, /models_list, /version → релей через
 *       родителя (модуль Foundry) на машину, где запущен Tokenatra.
 *
 * В обычном приложении файл ничего не делает (нет ?hosted=1).
 */
(function() {
    'use strict';
    if (window.__STUDIO_BUILT__ !== true) return;   // только hosted-копия модуля
    window.__STUDIO_HOSTED__ = true;

    var NativeFetch = window.fetch.bind(window);
    // Абсолютная база студии — её внедряет родитель при доставке в srcdoc
    // (относительные пути в srcdoc резолвятся ненадёжно)
    var ABS = window.__STUDIO_BASE__ || '';
    var assetV = window.__STUDIO_VERSION__ ? ('?v=' + window.__STUDIO_VERSION__) : '';
    // ВАЖНО: location.origin у srcdoc-iframe — строка "null"! Origin родителя
    // берём из абсолютной базы, внедрённой родителем
    var PARENT_ORIGIN = (function() {
        try { return new URL(window.__STUDIO_BASE__).origin; }
        catch (err) { return window.location.origin; }
    })();
    // Подключение к ЛОКАЛЬНОЙ Tokenatra: тогда /config — настоящий
    // config.json (тема, защита, сохранения, папки). Иначе — localStorage.
    var conn = { base: '', known: false };
    var connWaiters = [];
    function waitForConn(timeout) {
        if (conn.known) return Promise.resolve();
        return new Promise(function(resolve) {
            connWaiters.push(resolve);
            setTimeout(resolve, timeout || 3000);
        });
    }
    var CONFIG_KEY = 'tokenatra-studio-config';
    var relaySeq = 0;
    var relayWaiters = {};

    function jsonResponse(obj, status) {
        return new Response(JSON.stringify(obj), {
            status: status || 200,
            headers: { 'Content-Type': 'application/json' }
        });
    }

    // ===== Релей через родительское окно (модуль Foundry) =====

    window.addEventListener('message', function(e) {
        var d = e.data;
        if (!d || d.type !== 'tokenatra:relay-result') {
            // Родитель сообщил режим подключения (local = есть доступ к его
            // локальной Tokenatra — тогда /config ходит в настоящий конфиг)
            if (d && d.type === 'tokenatra:conn') {
                conn.base = (d.mode === 'local' && d.base) ? d.base : '';
                conn.known = true;
                connWaiters.splice(0).forEach(function(r) { r(); });
                // Индикаторы UI (бейдж видеокарты) могут переопроситься,
                // когда связь с Tokenatra появилась после 'none'
                try { window.dispatchEvent(new CustomEvent('tokenatra:conn')); } catch (err) {}
            }
            return;
        }
        var w = relayWaiters[d.relayId];
        if (!w) return;
        delete relayWaiters[d.relayId];
        clearTimeout(w.timer);
        if (d.ok) w.resolve(d.result);
        else w.reject(new Error(d.error || 'relay failed'));
    });

    // Рукопожатие: страница анонсирует себя родителю, тот в ответ шлёт
    // tokenatra:conn. Без этого conn, отправленный родителем до создания
    // srcdoc-iframe, теряется безвозвратно — /config подвисает на полном
    // таймауте ожидания (3 с) и уходит в localStorage вместо config.json.
    try {
        window.parent.postMessage({ type: 'tokenatra:hello' }, PARENT_ORIGIN);
    } catch (err) { /* родитель недоступен — conn придёт таймаутом */ }

    function relay(action, payload, timeout) {
        return new Promise(function(resolve, reject) {
            var relayId = 'r' + (++relaySeq) + Math.random().toString(36).slice(2, 8);
            var timer = setTimeout(function() {
                delete relayWaiters[relayId];
                reject(new Error('relay timeout'));
            }, timeout || 20000);
            relayWaiters[relayId] = { resolve: resolve, reject: reject, timer: timer };
            try {
                window.parent.postMessage({
                    type: 'tokenatra:relay',
                    relayId: relayId,
                    action: action,
                    payload: payload || {}
                }, PARENT_ORIGIN);
            } catch (err) {
                clearTimeout(timer);
                delete relayWaiters[relayId];
                reject(err);
            }
        });
    }

    function fileToDataUrl(file) {
        return new Promise(function(resolve, reject) {
            var reader = new FileReader();
            reader.onload = function() { resolve(reader.result); };
            reader.onerror = function() { reject(new Error('read failed')); };
            reader.readAsDataURL(file);
        });
    }

    function blobResponse(blob) {
        return new Response(blob, { status: 200, headers: { 'Content-Type': blob.type || 'application/octet-stream' } });
    }

    function readInitBody(init) {
        if (!init || !init.body) return null;
        try { return init.body; } catch (e) { return null; }
    }

    // ===== Перехват fetch =====

    window.fetch = function(input, init) {
        var url = typeof input === 'string' ? input : (input && input.url) || String(input || '');
        var path = url.split('?')[0];
        var method = ((init && init.method) || (input && input.method) || 'GET').toUpperCase();

        // --- конфиг: при локальной Tokenatra — настоящий config.json,
        // иначе localStorage игрока ---
        if (path === '/config') {
            return waitForConn(3000).then(function() {
                if (!conn.base) {
                    if (method === 'GET') {
                        var stored = null;
                        try { stored = localStorage.getItem(CONFIG_KEY); } catch (err) {}
                        return jsonResponse(stored ? JSON.parse(stored) : {});
                    }
                    var b = readInitBody(init);
                    if (b) {
                        try {
                            localStorage.setItem(CONFIG_KEY, JSON.stringify(JSON.parse(b)));
                        } catch (err) { /* приватный режим — настройки не персистятся */ }
                    }
                    return jsonResponse({ ok: true });
                }
                var req = { method: method };
                if (method === 'POST') {
                    req.headers = { 'Content-Type': 'application/json' };
                    req.body = readInitBody(init) || '';
                }
                return NativeFetch(conn.base + '/config', req).then(function(r) {
                    if (method === 'GET') {
                        return r.json().then(function(d) { return jsonResponse(d, r.status); });
                    }
                    return jsonResponse({ ok: true });
                }).catch(function() {
                    // сервер мог закрыться посреди сессии — не роняем приложение
                    return jsonResponse({ ok: true });
                });
            });
        }

        // --- статика ассетов студии ---
        if (path === '/presets_list' || path === '/rings_list') {
            var manifestKey = path === '/rings_list' ? 'rings' : 'presets';
            return NativeFetch(ABS + 'manifest.json' + assetV)
                .then(function(r) { return r.json(); })
                .then(function(m) { return jsonResponse(m[manifestKey] || []); });
        }
        if (path.indexOf('/preset_file/') === 0) {
            return NativeFetch(ABS + 'assets/presets/' + decodeURIComponent(path.slice('/preset_file/'.length)) + assetV);
        }
        if (path.indexOf('/ring_file/') === 0) {
            return NativeFetch(ABS + 'assets/rings/' + decodeURIComponent(path.slice('/ring_file/'.length)) + assetV);
        }
        if (path === '/mask') return NativeFetch(ABS + 'assets/mask.png' + assetV);
        if (path === '/ring') return NativeFetch(ABS + 'assets/ring-default.png' + assetV);
        if (path === '/icon' || path === '/logo') return NativeFetch(ABS + 'assets/logo.png' + assetV);
        if (path === '/example') return NativeFetch(ABS + 'assets/example.png' + assetV);

        // --- AI и статус Tokenatra: релей на машину ГМ ---
        if (path === '/process') {
            var fd = readInitBody(init);
            if (!fd) return Promise.reject(new Error('no body'));
            var image = fd.get('image');
            return fileToDataUrl(image).then(function(dataUrl) {
                return relay('process', {
                    image: dataUrl,
                    format: fd.get('format') || 'webp',
                    quality: fd.get('quality') || 90,
                    edge_blur: fd.get('edge_blur') || 1
                }, 180000).then(function(result) {
                    return NativeFetch(result.dataUrl).then(function(r) { return r.blob(); });
                });
            }).then(blobResponse).catch(function(err) {
                // Сетевой отказ (ни локальной Tokenatra, ни ГМ) — вместо
                // браузерного «Failed to fetch» понятное сообщение
                var m = (err && err.message) || '';
                if (/failed to fetch|load failed|networkerror|relay timeout/i.test(m)) {
                    throw new Error(window.I18n && I18n.t ? I18n.t('Tokenatra не запущена') : 'Tokenatra не запущена');
                }
                throw err;
            });
        }
        if (path === '/health' || path === '/device') {
            return relay(path.slice(1), {}, 8000).then(jsonResponse)
                .catch(function() { return jsonResponse({ device: '—', model_exists: true, model_loaded: true }); });
        }
        if (path === '/models_list') {
            // Ошибка статуса не должна блокировать кнопку: доступность
            // проверит сам /process (реальная ошибка там информативнее)
            return relay('models_list', {}, 8000).then(jsonResponse)
                .catch(function() {
                    return jsonResponse({ models: [
                        { name: 'Tokenatra', selected: true, size: 0 }
                    ], selected: 'Tokenatra', models_dir: '' });
                });
        }
        if (path === '/version') {
            return relay('version', {}, 8000).then(jsonResponse)
                .catch(function() { return jsonResponse({ version: '—', name: 'Tokenatra' }); });
        }

        // --- обновления: в hosted-студии их нет ---
        if (path === '/update_status') return Promise.resolve(jsonResponse({ update_checked: true, update_available: false }));
        if (path === '/check_update' || path === '/start_update_download') return Promise.resolve(jsonResponse({ ok: true }));

        // --- сохранение без десктопа: браузер скачивает файл сам ---
        if (path === '/save_file') {
            var svFd = readInitBody(init);
            var svFile = svFd && svFd.get('file');
            if (!svFile) return Promise.resolve(jsonResponse({ error: 'no file' }, 400));
            var dlUrl = URL.createObjectURL(svFile);
            var a = document.createElement('a');
            a.href = dlUrl;
            a.download = svFd.get('filename') || svFile.name || 'token.webp';
            document.body.appendChild(a);
            a.click();
            a.remove();
            setTimeout(function() { URL.revokeObjectURL(dlUrl); }, 10000);
            return Promise.resolve(jsonResponse({ ok: true }));
        }
        // Нативный выбор папки в браузере недоступен — тихая отмена
        if (path === '/pick_folder') return Promise.resolve(jsonResponse({ path: null }));

        // --- экспорт в мир: blob -> dataURL (модуль заберёт fetch'ем data:) ---
        if (path === '/embed_export') {
            var exFd = readInitBody(init);
            var exImage = exFd && (exFd.get('image') || exFd.get('file'));
            if (!exImage) return Promise.resolve(jsonResponse({ error: 'no file' }, 400));
            return fileToDataUrl(exImage).then(function(dataUrl) {
                return jsonResponse({ id: 'hosted', url: dataUrl });
            });
        }

        return NativeFetch(input, init);
    };

    // Публичный хелпер: серверный путь -> файл ассета студии.
    // Нужен для <img src>, которые нельзя перехватить fetch-обёрткой.
    window.TokenTransport = {
        active: function() { return window.__STUDIO_BUILT__ === true; },
        assetUrl: function(p) {
            if (!this.active()) return p;
            var v = window.__STUDIO_VERSION__ ? ('?v=' + window.__STUDIO_VERSION__) : '';
            if (p.indexOf('/ring_file/') === 0) return ABS + 'assets/rings/' + decodeURIComponent(p.slice('/ring_file/'.length)) + v;
            if (p.indexOf('/preset_file/') === 0) return ABS + 'assets/presets/' + decodeURIComponent(p.slice('/preset_file/'.length)) + v;
            if (p === '/mask') return ABS + 'assets/mask.png' + v;
            if (p === '/ring') return ABS + 'assets/ring-default.png' + v;
            if (p === '/icon' || p === '/logo') return ABS + 'assets/logo.png' + v;
            if (p === '/example') return ABS + 'assets/example.png' + v;
            return p;
        }
    };

    // Статические атрибуты <img src="/icon"> / <img src="/logo"> (логотип
    // тайтлбара, «О программе», favicon): fetch-обёртка их не видит, а в
    // srcdoc абсолютный путь уходит на origin мира и даёт 404. Переписываем
    // на ассеты студии после разбора DOM (сам файл исполняется в <head>).
    function fixStaticAssetPaths() {
        ['img[src="/icon"]', 'img[src="/logo"]', 'link[href="/icon"]'].forEach(function(sel) {
            var els = document.querySelectorAll(sel);
            for (var i = 0; i < els.length; i++) {
                var el = els[i];
                var p = el.getAttribute('src') || el.getAttribute('href');
                var fixed = window.TokenTransport.assetUrl(p);
                if (el.tagName === 'LINK') el.setAttribute('href', fixed);
                else el.setAttribute('src', fixed);
            }
        });
    }
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', fixStaticAssetPaths);
    } else {
        fixStaticAssetPaths();
    }

    // sendBeacon('/config') при закрытии: при локальной Tokenatra — в неё,
    // иначе глотаем (настройки hosted-игрока живут в его localStorage)
    var nativeBeacon = navigator.sendBeacon ? navigator.sendBeacon.bind(navigator) : null;
    if (nativeBeacon) {
        navigator.sendBeacon = function(url, data) {
            if (String(url).split('?')[0] === '/config') {
                if (conn.base && nativeBeacon) return nativeBeacon.call(navigator, conn.base + '/config', data);
                return true;
            }
            return nativeBeacon(url, data);
        };
    }
})();
