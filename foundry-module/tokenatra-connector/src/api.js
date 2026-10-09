/**
 * api.js — клиент Tokenatra с умным выбором подключения.
 *
 * Приоритеты:
 *   1. Локальная Tokenatra (запущена на устройстве клиента) — прямой HTTP.
 *   2. Tokenatra хоста — релей через сокет мира: запрос уходит клиенту ГМа,
 *      тот исполняет его на своём localhost. Сеть до машины ГМа не нужна.
 *   3. Ничего не ответило — ошибка с инструкцией (уведомление + экран студии).
 *
 * Выбор кешируется на сессию страницы; режим 'none' каждый раз
 * перепроверяется (ГМ мог зайти в мир и включить Tokenatra).
 */

import { relay } from './relay.js';

const MODULE_ID = 'tokenatra-connector';

let resolvedBase = null;      // база прямого подключения (приоритет 1)
let connMode = null;          // 'local' | 'host' | 'none' | null
let lastNotified = null;

export function getServerUrl() {
  return String(game.settings.get(MODULE_ID, 'serverUrl') || 'http://127.0.0.1:7878').replace(/\/+$/, '');
}

export function resolvedBaseUrl() {
  return connMode === 'local' ? resolvedBase : null;
}

function isTokenatraHealth(data) {
  return !!data && String(data.app || '').toLowerCase() === 'tokenatra' && !!data.connectorApi;
}

function candidateBases() {
  const bases = [getServerUrl()];
  try {
    const u = new URL(getServerUrl());
    if (u.hostname === '127.0.0.1') u.hostname = 'localhost';
    else if (u.hostname === 'localhost') u.hostname = '127.0.0.1';
    const alt = u.origin;
    if (!bases.includes(alt)) bases.push(alt);
  } catch (err) { /* невалидный URL — работаем с тем, что есть */ }
  // Приоритет 1: Tokenatra на устройстве клиента (стандартный адрес)
  for (const b of ['http://127.0.0.1:7878', 'http://localhost:7878']) {
    if (!bases.includes(b)) bases.push(b);
  }
  return bases;
}

async function fetchWithTimeout(url, ms = 3000) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), ms);
  try {
    return await fetch(url, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

async function probeBase(base) {
  try {
    let res = await fetchWithTimeout(base + '/health');
    if (res.ok) {
      const data = await res.json().catch(() => null);
      if (isTokenatraHealth(data)) return { status: 'ok', health: data };
    }
    if (res.status === 401 || res.status === 403) return { status: 'auth' };
    // Отвечает, но /health нет — старая версия Tokenatra
    res = await fetchWithTimeout(base + '/version');
    if (res.ok) {
      const data = await res.json().catch(() => null);
      if (data && String(data.name || '').toLowerCase() === 'tokenatra') {
        return { status: 'old', version: data.version };
      }
    }
  } catch (err) { /* следующий кандидат */ }
  return { status: 'off' };
}

/** Диагностика: перебирает адреса, возвращает первый ответивший. */
export async function probe() {
  for (const base of candidateBases()) {
    const p = await probeBase(base);
    if (p.status !== 'off') return { ...p, base };
  }
  return { status: 'off' };
}

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(new Error('read failed'));
    reader.readAsDataURL(blob);
  });
}

function notifyConn(mode) {
  // Уведомляем ТОЛЬКО о полной недоступности; подключение видно по работе
  if (lastNotified === 'none' && mode === 'none') return;
  lastNotified = mode;
  if (mode === 'none') console.error(game.i18n.localize('TKC.ConnNone'));
}

/**
 * Умное подключение по приоритетам. Результат кешируется;
 * режим 'none' каждый раз перепроверяется (ГМ мог зайти в мир).
 * @returns {Promise<{mode: 'local'|'host', base?: string}>}
 */
export async function ensureConnection({ fresh = false } = {}) {
  // fresh: игнорировать кэш (открытие окна — момент истины о доступности)
  if (fresh) { connMode = null; resolvedBase = null; }
  if (connMode === 'local' && resolvedBase) return { mode: 'local', base: resolvedBase };
  if (connMode === 'host') return { mode: 'host' };

  // Приоритет 1: локальная Tokenatra на устройстве клиента
  for (const base of candidateBases()) {
    const p = await probeBase(base);
    if (p.status === 'ok') {
      resolvedBase = base;
      connMode = 'local';
      notifyConn('local');
      return { mode: 'local', base };
    }
    if (p.status === 'auth') break; // Tokenatra найдена, но доступ закрыт
  }

  // Приоритет 2а: Tokenatra хоста через клиент ГМа (сокет мира)
  try {
    const h = await relay('health', {}, 6000);
    connMode = 'host';
    notifyConn('host');
    return { mode: 'host' };
  } catch (err) { /* ГМ не в мире или его браузер не достал localhost */ }

  // Приоритет 2б: Tokenatra на машине хоста мира — тот же hostname, порт 7878
  // (LAN-игры: http://<ip-хоста>:7878; требует включённого сетевого режима)
  if (!location.hostname || !['127.0.0.1', 'localhost'].includes(location.hostname)) {
    const hostBase = `${location.protocol}//${location.hostname}:7878`;
    const p = await probeBase(hostBase);
    if (p.status === 'ok') {
      resolvedBase = hostBase;
      connMode = 'local'; // прямое HTTP-подключение, релей не нужен
      notifyConn('local', p.health);
      return { mode: 'local', base: hostBase };
    }
  }

  // Приоритет 3: ничего не ответило
  connMode = 'none';
  notifyConn('none');
  throw new Error(game.i18n.localize('TKC.ErrorNotRunning'));
}

/** Сброс подключения (для повторной попытки из UI). */
export function resetConnection() {
  connMode = null;
  resolvedBase = null;
  lastNotified = null;
}

/** Исполнение задачи на локальной Tokenatra (приоритет 1, без сокета). */
export async function executeLocally(action, payload = {}) {
  const base = resolvedBase;
  if (!base) throw new Error('no local connection');
  if (action === 'process') {
    const blob = await (await fetch(payload.image)).blob();
    const ext = payload.format === 'png' ? 'png' : payload.format === 'jpg' ? 'jpg' : 'webp';
    const fd = new FormData();
    fd.append('image', new File([blob], `image.${ext}`, { type: blob.type || 'image/webp' }));
    fd.append('format', payload.format || 'webp');
    fd.append('quality', String(payload.quality ?? 90));
    fd.append('edge_blur', String(payload.edge_blur ?? 1));
    const res = await fetch(base + '/process', { method: 'POST', body: fd });
    if (!res.ok) {
      let detail = '';
      try { detail = (await res.json().catch(() => ({}))).error || ''; } catch (err) { /* ниже */ }
      throw new Error(detail || `HTTP ${res.status}`);
    }
    return { dataUrl: await blobToDataUrl(await res.blob()) };
  }
  const res = await fetch(base + '/' + action);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

/** Статус активного подключения (локальное или хоста). */
export async function health() {
  const { mode } = await ensureConnection();
  if (mode === 'local') return executeLocally('health');
  return relay('health', {}, 10000);
}

/** Удаление фона: локально при приоритете 1, иначе релей. */
export async function processImage(blob, { format = 'webp', quality = 90, edgeBlur = 1 } = {}) {
  const image = await blobToDataUrl(blob);
  const { mode } = await ensureConnection();
  const task = { image, format, quality, edge_blur: edgeBlur };
  const r = mode === 'local'
    ? await executeLocally('process', task)
    : await relay('process', task, 180000);
  return await (await fetch(r.dataUrl)).blob();
}

/** Забрать файл экспорта: data:-URL (hosted-студия) или путь на сервере. */
export async function fetchExport(path) {
  const url = (path.startsWith('http') || path.startsWith('data:'))
    ? path
    : (await ensureConnection()).base + path;
  const res = await fetch(url);
  if (!res.ok) throw new Error(game.i18n.localize('TKC.ErrorExportFetch'));
  return res.blob();
}
