/**
 * main.js — точка входа модуля tokenatra-connector.
 *
 * Запуск студии — по механизму Tokenizer 2: без глобального хоткея,
 * из контекстного меню актёров и кнопки в шапке листа актёра; редактор
 * открывается сразу «в режиме редактирования» для конкретного актёра
 * (синглтон по id приложения). Кнопка на панели токенов открывает пустую
 * студию (аналог standalone-режима).
 */

import { TokenatraApp, prefetchStudio } from './app.js';
import { quickRemoveBackground } from './actions.js';
import { probe } from './api.js';
import { initRelay } from './relay.js';
import { getServerUrl } from './api.js';

const MODULE_ID = 'tokenatra-connector';

function openStudio(actor = null) {
  return TokenatraApp.open(actor);
}

Hooks.once('init', () => {
  // World-scope: ГМ настраивает адрес и ключ один раз — у всех клиентов
  // они подхватываются автоматически (ключ хранится в настройках мира)
  game.settings.register(MODULE_ID, 'serverUrl', {
    name: 'TKC.Settings.ServerUrl',
    hint: 'TKC.Settings.ServerUrlHint',
    scope: 'world',
    config: true,
    type: String,
    default: 'http://127.0.0.1:7878'
  });
  game.settings.register(MODULE_ID, 'uploadTarget', {
    name: 'TKC.Settings.UploadTarget',
    hint: 'TKC.Settings.UploadTargetHint',
    scope: 'client',
    config: true,
    type: String,
    default: 'tokenatra/'
  });
  game.settings.register(MODULE_ID, 'format', {
    name: 'TKC.Settings.Format',
    scope: 'client',
    config: true,
    type: String,
    choices: { webp: 'WEBP', png: 'PNG', jpg: 'JPG' },
    default: 'webp'
  });
  game.settings.register(MODULE_ID, 'quality', {
    name: 'TKC.Settings.Quality',
    scope: 'client',
    config: true,
    type: Number,
    range: { min: 10, max: 100, step: 5 },
    default: 90
  });
  game.settings.register(MODULE_ID, 'autoApply', {
    name: 'TKC.Settings.AutoApply',
    hint: 'TKC.Settings.AutoApplyHint',
    scope: 'client',
    config: true,
    type: Boolean,
    default: false
  });
  game.settings.register(MODULE_ID, 'showSceneControl', {
    name: 'TKC.Settings.ShowSceneControl',
    scope: 'client',
    config: true,
    type: Boolean,
    default: true
  });
  game.settings.register(MODULE_ID, 'showSheetButton', {
    name: 'TKC.Settings.ShowSheetButton',
    hint: 'TKC.Settings.ShowSheetButtonHint',
    scope: 'client',
    config: true,
    type: Boolean,
    default: true
  });
});

Hooks.once('ready', () => {
  // Релей AI: клиент ГМа исполняет запросы игроков на своём localhost
  initRelay({
    process: async (p) => {
      const blob = await (await fetch(p.image)).blob();
      const fd = new FormData();
      const ext = p.format === 'png' ? 'png' : p.format === 'jpg' ? 'jpg' : 'webp';
      fd.append('image', new File([blob], `image.${ext}`, { type: blob.type || 'image/webp' }));
      fd.append('format', p.format || 'webp');
      fd.append('quality', String(p.quality ?? 90));
      fd.append('edge_blur', String(p.edge_blur ?? 1));
      const res = await fetch(getServerUrl() + '/process', { method: 'POST', body: fd });
      if (!res.ok) {
        let detail = '';
        try { detail = (await res.json().catch(() => ({}))).error || ''; } catch (err) { /* ниже */ }
        throw new Error(detail || game.i18n.localize('TKC.ErrorProcess'));
      }
      const out = await res.blob();
      return await new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve({ dataUrl: reader.result });
        reader.onerror = () => reject(new Error('read failed'));
        reader.readAsDataURL(out);
      });
    },
    health: async () => {
      const res = await fetch(getServerUrl() + '/health');
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return res.json();
    },
    // Бейдж видеокарты в студии (remover.js/main.js) ходит на /device —
    // без хендлера hosted-режим всегда показывал «—»
    device: async () => {
      const res = await fetch(getServerUrl() + '/device');
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return res.json();
    },
    models_list: async () => {
      const res = await fetch(getServerUrl() + '/models_list');
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return res.json();
    }
  });

  // Публичный API для макросов и других модулей
  game.modules.get(MODULE_ID).api = {
    open: (actor = null) => openStudio(actor),
    removeBackground: (actor) => quickRemoveBackground(actor),
    probe
  };

  // Прогрев студии в простое: страница собирается заранее (у ГМа — на его
  // localhost, у игрока — чанками через сокет с уже тёплого кеша ГМа) и
  // кешируется до перезапуска страницы. Первое открытие окна — мгновенное.
  const warm = () => prefetchStudio();
  if (typeof requestIdleCallback === 'function') requestIdleCallback(warm, { timeout: 15000 });
  else setTimeout(warm, 5000);
});

// ===== Кнопка в панели инструментов токенов =====
Hooks.on('getSceneControlButtons', (controls) => {
  if (!game.settings.get(MODULE_ID, 'showSceneControl')) return;
  // v13 передаёт массив, v14 — Record по id контролов
  const tokenControls = Array.isArray(controls)
    ? controls.find(c => c.id === 'token')
    : controls.token;
  if (!tokenControls) return;
  tokenControls.tools.push({
    name: 'tokenatra',
    title: game.i18n.localize('TKC.SceneControl'),
    icon: 'tkc-icon',
    button: true, // не toggle: активность кнопки не связана с состоянием мира
    onClick: () => openStudio()
  });
});

// ===== Кнопка в шапке листа актёра (v13 + v14) — как в Tokenizer 2 =====
Hooks.on('getActorSheetHeaderButtons', (app, buttons) => {
  if (!game.settings.get(MODULE_ID, 'showSheetButton')) return;
  const actor = app?.document ?? app?.actor;
  if (!(actor instanceof foundry.documents.BaseActor)) return;
  buttons.unshift({
    label: 'Tokenatra',
    icon: 'tkc-icon',
    class: MODULE_ID,
    onclick: () => openStudio(actor)
  });
});

Hooks.on('getHeaderControlsApplicationV2', (app, controls) => {
  if (!game.settings.get(MODULE_ID, 'showSheetButton')) return;
  const actor = app?.document;
  if (!(actor instanceof foundry.documents.BaseActor)) return;
  // action резолвится через options.actions листа (приём Tokenizer 2)
  app.options.actions ??= {};
  app.options.actions['tokenatra-open'] = () => openStudio(actor);
  controls.push({
    label: 'Tokenatra',
    icon: 'tkc-icon',
    action: 'tokenatra-open'
  });
});

// ===== Контекстное меню актёров (сайдбар) =====
Hooks.on('getActorContextOptions', (html, contextOptions) => {
  contextOptions.push({
    name: 'TKC.ContextStudio',
    icon: '<i class="tkc-icon"></i>',
    condition: (li) => !!actorFromLi(li),
    callback: (li) => {
      const actor = actorFromLi(li);
      if (actor) openStudio(actor);
    }
  });
  contextOptions.push({
    name: 'TKC.ContextRemoveBg',
    icon: '<i class="fas fa-eraser"></i>',
    condition: (li) => {
      const actor = actorFromLi(li);
      return !!actor && !!actor.isOwner;
    },
    callback: (li) => {
      const actor = actorFromLi(li);
      if (actor) quickRemoveBackground(actor);
    }
  });
});

/** v13: li — jQuery; v14 — HTMLElement. */
function domElement(li) {
  if (li instanceof HTMLElement) return li;
  return li?.[0] ?? null;
}

function actorFromLi(li) {
  const el = domElement(li);
  try {
    const uuid = el?.dataset?.uuid ?? li?.data?.('uuid');
    if (uuid) {
      const doc = fromUuidSync(uuid);
      if (doc) return doc;
    }
  } catch (err) { /* ниже — по documentId */ }
  const id = el?.dataset?.documentId ?? li?.data?.('documentId');
  return id ? game.actors.get(id) : null;
}
