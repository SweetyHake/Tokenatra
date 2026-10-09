/**
 * relay.js — релей AI-запросов через сокет Foundry.
 *
 * Игрок не имеет прямого доступа к Tokenatra (она на машине ГМа), но сокет
 * мира объединяет всех клиентов. Схема: клиент игрока emit'ит задачу,
 * клиент ГМа выполняет её на своём localhost Tokenatra и возвращает
 * результат. Транспорт — сам Foundry, поэтому работает на любом хостинге.
 *
 * На клиенте ГМа relay() выполняет обработчик локально (без сокета).
 */

import { getServerUrl } from './api.js';

const SOCKET = 'module.tokenatra-connector';

let handlers = null;

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(new Error('read failed'));
    reader.readAsDataURL(blob);
  });
}

async function gmGet(path) {
  const res = await fetch(getServerUrl() + path);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function gmProcess(p = {}) {
  const base = getServerUrl();
  const blob = await (await fetch(p.image)).blob();
  const ext = p.format === 'png' ? 'png' : p.format === 'jpg' ? 'jpg' : 'webp';
  const fd = new FormData();
  fd.append('image', new File([blob], `image.${ext}`, { type: blob.type || 'image/webp' }));
  fd.append('format', p.format || 'webp');
  fd.append('quality', String(p.quality ?? 90));
  fd.append('edge_blur', String(p.edge_blur ?? 1));
  const res = await fetch(base + '/process', { method: 'POST', body: fd });
  if (!res.ok) {
    let detail = '';
    try { detail = (await res.json().catch(() => ({}))).error || ''; } catch (err) { /* ниже */ }
    throw new Error(detail || `HTTP ${res.status}`);
  }
  return { dataUrl: await blobToDataUrl(await res.blob()) };
}

// ===== Живой UI: сборка самодостаточной студии из Tokenatra ГМа =====
// ГМ-клиент забирает с localhost:7878 отрендеренную страницу, инлайнит
// все CSS/JS и отдаёт игрокам чанками через сокет. Игроки получают
// ВСЕГДА АКТУАЛЬНЫЙ интерфейс без сети до машины ГМа.

let _studioCache = null;
let _studioVersion = '';

function escapeScript(code) {
  // </script> внутри инлайна ломает HTML; <\/script> валиден и в строках,
  // и в регэкспах
  return code.replace(/<\/script>/gi, '<\\/script>');
}

async function assembleStudio() {
  try {
    const h = await gmGet('/health');
    if (_studioCache && _studioVersion && h.version !== _studioVersion) {
      _studioCache = null; // приложение обновилось — пересобираем UI
    }
    _studioVersion = h.version || '';
  } catch (err) { /* health не критичен для сборки */ }
  if (_studioCache) return _studioCache;
  const base = getServerUrl();
  const fetchText = async (p) => {
    const res = await fetch(base + p);
    if (!res.ok) throw new Error(`${p}: HTTP ${res.status}`);
    return res.text();
  };

  let html = await fetchText('/?embed=1');
  const transport = await fetchText('/static/js/tokenTransport.js');

  // CSS инлайним
  html = await replaceAsync(html, /<link\s+rel="stylesheet"\s+href="([^"]+)"\s*>/g,
    async (m, href) => `<style>${escapeScript(await fetchText(href))}</style>`);
  // JS инлайним (токен-транспорт — первым, в <head>, до всех скриптов тела)
  const transportTag = '<script>' + escapeScript(transport) + '</script>';
  const studioBase = window.location.origin + '/modules/tokenatra-connector/studio/';
  const bootstrap = ('<base href="' + studioBase + '">'
    + '<script>window.__STUDIO_BASE__ = ' + JSON.stringify(studioBase) + ';</script>'
    + '<script>window.__STUDIO_BUILT__ = true;'
    + "window.__EMBED__ = { embed: true, parentOrigin: '' };"
    + "document.documentElement.classList.add('embed');</script>"
    + transportTag);
  html = html.replace(/<head([^>]*)>/i, `<head$1>${bootstrap}`);
  html = await replaceAsync(html, /<script\s+src="([^"]+)"\s*>\s*<\/script>/g,
    async (m, src) => `<script>${escapeScript(await fetchText(src))}</script>`);

  _studioCache = html;
  return html;
}

async function replaceAsync(str, regex, fn) {
  const jobs = [];
  str.replace(regex, (...args) => { jobs.push(fn(...args)); return ''; });
  const results = await Promise.all(jobs);
  let i = 0;
  return str.replace(regex, () => results[i++]);
}

const STUDIO_CHUNK = 96 * 1024;

/** Регистрация релея: только клиент ГМа исполняет задачи. */
export function initRelay(h) {
  handlers = {
    studio_meta: async () => {
      const html = await assembleStudio();
      const chunks = Math.max(1, Math.ceil(html.length / STUDIO_CHUNK));
      return { total: html.length, chunks, version: _studioVersion };
    },
    studio_chunk: async (p = {}) => {
      const html = await assembleStudio();
      const i = Math.max(0, p.i | 0);
      return { text: html.slice(i * STUDIO_CHUNK, (i + 1) * STUDIO_CHUNK) };
    },
    ...h
  };
  if (!game.socket) return;
  game.socket.on(SOCKET, async (data) => {
    if (!data?.relay || !game.user.isGM || !handlers) return;
    let result;
    try {
      result = { ok: true, result: await handlers[data.relay.action]?.(data.relay.payload ?? {}) };
    } catch (err) {
      result = { ok: false, error: err?.message || String(err) };
    }
    game.socket.emit(SOCKET, {
      relayResult: {
        relayId: data.relay.relayId,
        ok: result.ok,
        error: result.error,
        result: result.result
      }
    });
  });
}

/**
 * Выполнить действие на машине ГМа. ГМ — локально, игрок — через сокет.
 * @returns {Promise<object>} результат обработчика
 */
export function relay(action, payload = {}, timeout = 20000) {
  if (game.user.isGM) {
    if (!handlers?.[action]) return Promise.reject(new Error(`no relay handler: ${action}`));
    return Promise.resolve(handlers[action](payload));
  }
  return new Promise((resolve, reject) => {
    const relayId = foundry.utils.randomID();
    const timer = setTimeout(() => {
      game.socket.off(SOCKET, onMsg);
      reject(new Error(game.i18n.localize('TKC.ErrorRelay')));
    }, timeout);
    const onMsg = (data) => {
      if (data?.relayResult?.relayId !== relayId) return;
      clearTimeout(timer);
      game.socket.off(SOCKET, onMsg);
      if (data.relayResult.ok) resolve(data.relayResult.result);
      else reject(new Error(data.relayResult.error || 'relay failed'));
    };
    game.socket.on(SOCKET, onMsg);
    game.socket.emit(SOCKET, { relay: { relayId, action, payload } });
  });
}

export function blobToDataUrlExport(blob) {
  return blobToDataUrl(blob);
}
