const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const IS_WINDOWS = /^Win/.test(navigator.platform || '');

function $(id) {
    return document.getElementById(id);
}

function escapeHtml(s) {
    return String(s ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function applyHotkeyHints() {
    // Подмену Ctrl→⌘ делаем только на macOS; на Windows/Linux остаётся Ctrl.
    var toPlatform = function(s) {
        if (!IS_MAC) return s;
        return s.replaceAll('Ctrl + ', '⌘').replaceAll('Ctrl+', '⌘');
    };
    var translate = function(src) {
        var res = typeof I18n !== 'undefined' ? I18n.t(src) : src;
        if (res === src) {
            var squeezed = src.replace(/ \+ /g, '+');
            var alt = I18n.t(squeezed);
            if (alt !== squeezed) res = alt;
        }
        return res;
    };
    document.querySelectorAll('[data-hotkey-hint]').forEach(function(el) {
        if (!el._hintBase) el._hintBase = {};
        ['data-tooltip', 'title'].forEach(function(attr) {
            var cur = el.getAttribute(attr);
            if (!cur) return;
            if (cur.indexOf('Ctrl') !== -1 || !el._hintBase[attr]) el._hintBase[attr] = cur;
            var out = toPlatform(translate(el._hintBase[attr]));
            if (el.getAttribute(attr) !== out) el.setAttribute(attr, out);
        });
        if (!el.firstElementChild && el.textContent) {
            var txt = el.textContent;
            if (txt.indexOf('Ctrl') !== -1 || !el._hintBase.text) el._hintBase.text = txt;
            var outT = toPlatform(translate(el._hintBase.text));
            if (txt !== outT) el.textContent = outT;
        }
    });
}

function debounce(func, wait) {
    var timeout;
    return function() {
        var args = arguments;
        var context = this;
        clearTimeout(timeout);
        timeout = setTimeout(function() {
            func.apply(context, args);
        }, wait);
    };
}

function toast(msg, isError) {
    var t = $('toast');
    if (!t) return;
    if (t._timer) { clearTimeout(t._timer); t._timer = null; }
    t.textContent = typeof I18n !== 'undefined' ? I18n.t(msg) : msg;
    t.classList.toggle('error', !!isError);
    t.classList.add('show');
    t._timer = setTimeout(function() { t.classList.remove('show'); t._timer = null; }, 2500);
}

function formatSize(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / 1048576).toFixed(1) + ' MB';
}

function getFileBaseName(filename) {
    return filename.replace(/\.[^.]+$/, '');
}

function clamp(value, min, max) {
    return Math.max(min, Math.min(max, value));
}

function setSliderFill(el) {
    if (!el) return;
    const val = parseFloat(el.value);
    const min = parseFloat(el.min);
    const max = parseFloat(el.max);
    const range = max - min;
    if (range === 0) return;
    el.style.setProperty('--p', ((val - min) / range * 100) + '%');
}

// Ручной ввод значения слайдера: применяет число из инпута к слайдеру
// (с клампом и округлением по step) через штатный oninput слайдера
function bindSliderValueInput(slider, input) {
    if (!slider || !input) return;
    input.addEventListener('change', () => {
        const min = parseFloat(slider.min), max = parseFloat(slider.max);
        const step = parseFloat(slider.step) || 1;
        let v = parseFloat(String(input.value).replace(',', '.'));
        if (isNaN(v)) { input.value = slider.value; return; }
        v = clamp(Math.round(v / step) * step, min, max);
        slider.value = v;
        slider.dispatchEvent(new Event('input'));
    });
    input.addEventListener('keydown', ev => { if (ev.key === 'Enter') { ev.preventDefault(); input.blur(); } });
    input.addEventListener('wheel', ev => ev.preventDefault(), { passive: false });
}

async function saveFileWithPicker(blob, suggestedName) {
    const fd = new FormData();
    fd.append('file', blob, suggestedName);
    fd.append('filename', suggestedName);

    try {
        const res = await fetch('/save_file', { method: 'POST', body: fd });
        const json = await res.json();
        if (json.cancelled) return;
        if (json.error) { toast('Ошибка: ' + json.error, true); return; }
        toast('Сохранено: ' + suggestedName);
    } catch(e) {
        toast('Ошибка сохранения', true);
    }
}

async function pickFolder() {
    try {
        const res = await fetch('/pick_folder');
        const json = await res.json();
        if (json.cancelled || !json.path) return null;
        return json.path;
    } catch(e) {
        toast('Ошибка выбора папки', true);
        return null;
    }
}

// Текущий CSS-zoom интерфейса: координаты в visual-пикселях (clientX,
// getBoundingClientRect) при записи в style.left/top/transform нужно
// делить на него — иначе при масштабе != 100% всплывающие элементы уезжают
function uiZoom() {
    return parseFloat(document.documentElement.style.zoom) || 1;
}

function initTooltips() {
    var el = null, hideTimer = null, removeTimer = null;
    function removeEl() {
        if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; }
        if (removeTimer) { clearTimeout(removeTimer); removeTimer = null; }
        if (!el) return;
        var e = el;
        el = null;
        e.classList.remove('show');
        removeTimer = setTimeout(function() { if (e && e.parentNode) e.parentNode.removeChild(e); }, 120);
    }
    document.addEventListener('mouseover', function(e) {
        var target = e.target.closest('[data-tooltip]');
        if (!target) { removeEl(); return; }
        if (el && el._target === target) {
            if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; }
            return;
        }
        removeEl();
        var text = target.getAttribute('data-tooltip');
        if (!text) return;
        el = document.createElement('div');
        el.className = 'tooltip-el';
        el.textContent = text;
        el._target = target;
        document.body.appendChild(el);
        requestAnimationFrame(function() {
            if (!el || el._target !== target) return;
            var zf = uiZoom();
            var rect = target.getBoundingClientRect();
            var top = rect.top - 6 - el.offsetHeight * zf;
            if (top < 4) top = rect.bottom + 6;
            var leftV = Math.max(4, Math.min(rect.left + rect.width / 2 - el.offsetWidth * zf / 2, window.innerWidth - el.offsetWidth * zf - 4));
            el.style.left = (leftV / zf) + 'px';
            el.style.top = (top / zf) + 'px';
            el.classList.add('show');
        });
    }, false);
    document.addEventListener('mouseout', function(e) {
        var target = e.target.closest('[data-tooltip]');
        if (!target) return;
        if (el && el._target === target) {
            var related = e.relatedTarget;
            if (related && related.nodeType === 1 && related.closest && related.closest('[data-tooltip]') === target) return;
        }
        hideTimer = setTimeout(removeEl, 20);
    }, false);
    document.addEventListener('scroll', removeEl, true);
}

async function saveToFolder(blob, filename, folderPath) {
    const fd = new FormData();
    fd.append('file', blob, filename);
    fd.append('filename', filename);
    fd.append('folder', folderPath);

    try {
        const res = await fetch('/save_to_folder', { method: 'POST', body: fd });
        const json = await res.json();
        if (json.error) { toast('Ошибка: ' + json.error, true); return false; }
        toast('Сохранено: ' + filename);
        return true;
    } catch(e) {
        toast('Ошибка сохранения', true);
        return false;
    }
}
