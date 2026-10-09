/**
 * actions.js — применение результатов и быстрые действия по актёрам.
 */

import { processImage, health } from './api.js';
import { upload, sanitizeFileName } from './upload.js';

export function canEditActor(actor) {
  return !!actor && !!actor.isOwner;
}

/**
 * Обновить арт актёра: прототип токена (+ img там, где схема его ещё знает —
 * v13), и уже размещённые токены (GM). В v14 img у актёра удалён.
 */
export async function updateActorArt(actor, path) {
  if (!canEditActor(actor)) {
    console.warn(game.i18n.format('TKC.WarnNoActorPermission', { name: actor?.name ?? '?' }));
    return false;
  }
  const data = { 'prototypeToken.texture.src': path };
  if ('img' in (actor.schema?.fields ?? {})) data.img = path;
  await actor.update(data);

  if (game.user.isGM) {
    let tokens = [];
    try { tokens = actor.getActiveTokens?.() ?? []; } catch (err) { tokens = []; }
    if (!tokens.length && canvas?.tokens?.placeables) {
      tokens = canvas.tokens.placeables.filter(t => t.actor?.id === actor.id);
    }
    for (const token of tokens) {
      try { await token.document?.update?.({ texture: { src: path } }); }
      catch (err) { /* токен мог быть удалён параллельно */ }
    }
  }
  return true;
}

/** Создать актёра из готового изображения (без открытия листа). */
export async function createActorFromImage(path, fileName) {
  const types = game.system.documentTypes?.Actor ?? [];
  const type = types.includes('npc') ? 'npc' : types[0];
  if (!type) throw new Error(game.i18n.localize('TKC.ErrorApply'));
  const name = sanitizeFileName(fileName).replace(/\.[^.]+$/, '').replace(/[_-]+/g, ' ').trim()
    || game.i18n.localize('TKC.NewActorName');
  const actor = await Actor.create({
    name,
    type,
    prototypeToken: { texture: { src: path } }
  }, { renderSheet: false });
  // v13: у схемы ещё есть img — дублируем для листа актёра
  if (actor && 'img' in (actor.schema?.fields ?? {})) {
    await actor.update({ img: path });
  }
  return actor;
}

/** Blob арта актёра; только растровые изображения с того же origin (Data). */
export async function fetchActorImageBlob(actor) {
  const src = actor?.img || actor?.prototypeToken?.texture?.src;
  if (!src) throw new Error(game.i18n.localize('TKC.ErrorNoArtwork'));
  const clean = String(src).split('?')[0].toLowerCase();
  if (/\.(mp4|webm|mov|svg)$/.test(clean) || clean.includes('/icons/svg/')) {
    throw new Error(game.i18n.localize('TKC.ErrorNotRaster'));
  }
  let res;
  try {
    res = await fetch(src);
  } catch (err) {
    // Внешний арт (S3/CDN) без CORS-заголовков — fetch из окна Foundry падает
    throw new Error(game.i18n.localize('TKC.ErrorExternalArt'));
  }
  if (!res.ok) throw new Error(game.i18n.localize('TKC.ErrorArtworkFetch'));
  return res.blob();
}

export async function actorArtDataUrl(actor) {
  const blob = await fetchActorImageBlob(actor);
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(new Error(game.i18n.localize('TKC.ErrorArtworkFetch')));
    reader.readAsDataURL(blob);
  });
}

/** Обновить токены актёра, размещённые на сцене (GM). */
export async function refreshPlacedTokens(actor, update) {
  if (!game.user.isGM) return;
  let tokens = [];
  try { tokens = actor.getActiveTokens?.() ?? []; } catch (err) { tokens = []; }
  if (!tokens.length && canvas?.tokens?.placeables) {
    tokens = canvas.tokens.placeables.filter(t => t.actor?.id === actor.id);
  }
  for (const token of tokens) {
    try { await token.document?.update?.(update); }
    catch (err) { /* токен мог быть удалён параллельно */ }
  }
}

/**
 * Применить сохранение из студии к актёру.
 *  - token:   img + texture.src = пререндеренное кольцо, scale 1;
 *  - subject: ring.enabled=true, ring.subject.{texture,scale} + texture.scaleX/Y;
 *  - portrait: img = портрет.
 * Метрики (автоматические) считаются в студии по bbox непрозрачности.
 */
export async function applyKindToActor(actor, kind, path, m = {}) {
  // Значение масштаба из раздела «Экспорт» студии (×1/×2/×3 — то же число,
  // что в тултипе предпросмотра «(Масштаб: N)») — пишется в prototypeToken
  // scale и ring.subject.scale, для обеих кнопок (Token и Subject)
  const num = (v) => (Number.isFinite(+v) && +v > 0 ? +v : 1);
  const exportScale = Math.min(3, Math.max(1, num(m.scale)));

  if (kind === 'portrait') {
    await actor.update({ img: path });
    return;
  }
  if (kind === 'subject') {
    await actor.update({
      'prototypeToken.texture.src': path,
      'prototypeToken.texture.scaleX': exportScale,
      'prototypeToken.texture.scaleY': exportScale,
      'prototypeToken.ring.enabled': true,
      'prototypeToken.ring.subject.texture': path,
      'prototypeToken.ring.subject.scale': exportScale
    });
    await refreshPlacedTokens(actor, {
      'texture.src': path,
      'texture.scaleX': exportScale,
      'texture.scaleY': exportScale,
      'ring.enabled': true,
      'ring.subject.texture': path,
      'ring.subject.scale': exportScale
    });
    return;
  }
  // token (с пререндеренным кольцом): масштаб текстуры и ring.subject.scale —
  // реальное значение из «Экспорта»; портрет актёра (img) не трогаем —
  // его назначает сохранение портрета
  await actor.update({
    'prototypeToken.texture.src': path,
    'prototypeToken.texture.scaleX': exportScale,
    'prototypeToken.texture.scaleY': exportScale,
    'prototypeToken.ring.subject.scale': exportScale
  });
  await refreshPlacedTokens(actor, {
    'texture.src': path,
    'texture.scaleX': exportScale,
    'texture.scaleY': exportScale,
    'ring.subject.scale': exportScale
  });
}

/** Диалог «что сделать с экспортом» + применение. */
export async function applyExport(path, fileName, { contextActor = null, autoApply = false, kind = 'token', metrics = null } = {}) {
  if (autoApply && canEditActor(contextActor)) {
    await updateActorArt(contextActor, path);
    console.info(game.i18n.format('TKC.DoneActorUpdated', { name: contextActor.name }));
    return;
  }

  const buttons = {};
  if (game.user.can('ACTOR_CREATE')) {
    buttons.create = {
      icon: '<i class="fas fa-user-plus"></i>',
      label: game.i18n.localize('TKC.BtnCreateActor'),
      callback: async () => {
        try {
          const actor = await createActorFromImage(path, fileName);
          if (actor && kind !== 'token') await applyKindToActor(actor, kind, path, metrics ?? {});
          console.info(game.i18n.localize('TKC.DoneActorCreated'));
        } catch (err) {
          console.error(game.i18n.localize('TKC.ErrorApply'));
        }
      }
    };
  }
  if (canEditActor(contextActor)) {
    buttons.update = {
      icon: '<i class="fas fa-user-pen"></i>',
      label: game.i18n.format('TKC.BtnUpdateActor', { name: contextActor.name }),
      callback: async () => {
        try {
          await applyKindToActor(contextActor, kind, path, metrics ?? {});
          console.info(game.i18n.format('TKC.DoneActorUpdated', { name: contextActor.name }));
        } catch (err) {
          console.error(game.i18n.localize('TKC.ErrorApply'));
        }
      }
    };
  }
  buttons.copy = {
    icon: '<i class="fas fa-copy"></i>',
    label: game.i18n.localize('TKC.BtnCopyPath'),
    callback: () => {
      game.clipboard.copyPlainText?.(path) ?? navigator.clipboard?.writeText(path);
      console.info(path);
    }
  };

  try {
    await Dialog.wait({
      title: game.i18n.localize('TKC.ExportDialogTitle'),
      content: `<div class="tkc-export-dialog"><img src="${path}" alt=""><p>${sanitizeFileName(fileName)}</p></div>`,
      buttons,
      default: buttons.update ? 'update' : (buttons.create ? 'create' : 'copy')
    });
  } catch (err) {
    // Диалог закрыт без выбора (Esc/крестик) — это не ошибка
  }
}

/** Быстрое действие: вырезать фон у арта актёра без открытия студии. */
export async function quickRemoveBackground(actor) {
  if (!actor) return;
  const h = await health();
  if (!h) {
    const p = await import('./api.js').then(m => m.probe());
    console.error(game.i18n.localize(p.status === 'old' ? 'TKC.WarnOldApp' : 'TKC.ErrorNotRunning'));
    return;
  }
  if (h.model_exists === false) {
    console.error(game.i18n.localize('TKC.ErrorNoModel'));
    return;
  }
  if (h.model_loaded === false) {
    // В трее модель выгружается по простою — первый запрос платит прогрузку
    console.info(game.i18n.localize('TKC.InfoModelLoading'));
  }

  let blob;
  try {
    blob = await fetchActorImageBlob(actor);
  } catch (err) {
    console.error(err.message);
    return;
  }

  const format = game.settings.get('tokenatra-connector', 'format') || 'webp';
  const quality = game.settings.get('tokenatra-connector', 'quality') || 90;
  let result;
  try {
    console.info(game.i18n.format('TKC.InfoProcessing', { name: actor.name }));
    result = await processImage(blob, { format, quality });
  } catch (err) {
    console.error(err.statusCode === 413
      ? game.i18n.localize('TKC.ErrorTooLarge')
      : err.message);
    return;
  }

  const ext = format === 'png' ? 'png' : format === 'jpg' ? 'jpg' : 'webp';
  const path = await upload(result, `${sanitizeFileName(actor.name || 'actor')}_nobg.${ext}`);
  if (!path) return;

  try {
    await updateActorArt(actor, path);
    console.info(game.i18n.format('TKC.DoneActorUpdated', { name: actor.name }));
  } catch (err) {
    console.error(game.i18n.localize('TKC.ErrorApply'));
  }
}
