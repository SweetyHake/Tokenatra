/**
 * foundryRings.js — перечисление и 2D-рендер динамических колец Foundry VTT
 * (CONFIG.Token.ring) для студии Tokenatra и интеграции с Multiple Rings.
 *
 * Реализует точную формулу шейдера TokenRingSamplerShader:
 *   - colorizeTokenBackground (overlay-тинт defaultBackgroundColor);
 *   - colorizeTokenRing (colorBand + -msk + defaultRingColor);
 *   - blend(ringColor, backgroundColor) в premultiplied-альфе;
 *   - поддержку config.scale (например 2× кольца) и config.overlays (-top
 *     слой, рисуемый поверх вырезки персонажа).
 */

export const MRINGS_MODULE_ID = 'multiple-rings';
const INTERNAL_MEGA_ID = 'mringsCombinedRings';

const _sheetMetaCache = new Map();   // cfgId -> Promise<SheetMeta|null>
const _thumbCache = new Map();       // cfgId -> { thumbUrl, scale } (webp dataURL 144×144 + scale)
const _thumbPending = new Map();     // cfgId -> Promise<{ thumbUrl, scale }|null>
const _fullRingCache = new Map();    // cfgId -> { ringBlob, topBlob, scale } (LRU ≤ 8)
const FULL_CACHE_LIMIT = 8;

function resolveUrl(relPath, baseHref) {
  if (!relPath) return null;
  const s = String(relPath).trim();
  if (!s) return null;
  if (/^(https?:|data:|blob:)/i.test(s)) return s;
  if (!baseHref && globalThis.foundry?.utils?.getRoute) {
    try {
      return new URL(foundry.utils.getRoute(s.replace(/^\/+/, '')), window.location.origin + '/').href;
    } catch { /* fallback below */ }
  }
  try {
    return new URL(s, baseHref || document.baseURI || window.location.href).href;
  } catch {
    return s;
  }
}

function parseHexRgb01(hex, fallback = [1, 1, 1]) {
  if (hex === null || hex === undefined || hex === '') return fallback;
  if (typeof hex === 'number' && Number.isFinite(hex)) {
    const n = hex >>> 0;
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
  }
  let s = String(hex).trim();
  if (s.startsWith('0x') || s.startsWith('0X')) s = '#' + s.slice(2);
  if (!s.startsWith('#')) return fallback;
  s = s.slice(1);
  if (s.length === 3) s = s[0] + s[0] + s[1] + s[1] + s[2] + s[2];
  if (s.length < 6) return fallback;
  const v = parseInt(s.slice(0, 6), 16);
  if (!Number.isFinite(v)) return fallback;
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

function localizeRingLabel(raw, fallbackId) {
  const label = String(raw || fallbackId || '').trim();
  if (!label) return fallbackId;
  try {
    if (game?.i18n?.has?.(label)) return game.i18n.localize(label);
    const loc = game?.i18n?.localize?.(label);
    if (loc && loc !== label) return loc;
  } catch { /* noop */ }
  return label;
}

/** Список валидных ID колец, зарегистрированных в CONFIG.Token.ring. */
export function getValidFoundryRingIds() {
  const ringCfg = globalThis.CONFIG?.Token?.ring;
  if (!ringCfg) return [];
  let rawIds = [];
  if (Array.isArray(ringCfg.configIDs)) {
    rawIds = ringCfg.configIDs;
  } else if (typeof ringCfg.getConfig === 'function' && ringCfg._config) {
    rawIds = Object.keys(ringCfg._config);
  }
  const out = [];
  for (const id of rawIds) {
    if (!id || id === INTERNAL_MEGA_ID) continue;
    const cfg = ringCfg.getConfig?.(id);
    if (!cfg?.spritesheet) continue;
    out.push(id);
  }
  return out;
}

/** ID кольца мира по умолчанию (core.dynamicTokenRing). */
export function getWorldFoundryRingId() {
  const valid = getValidFoundryRingIds();
  let worldId = '';
  try {
    worldId = String(game.settings.get('core', 'dynamicTokenRing') || '').trim();
  } catch { /* noop */ }
  if (worldId && worldId !== INTERNAL_MEGA_ID && valid.includes(worldId)) return worldId;
  if (valid.includes('coreSteel')) return 'coreSteel';
  return valid[0] || 'coreSteel';
}

/** Явно назначенное кольцо из флага Multiple Rings (или null, если не задано). */
export function getActorExplicitRingId(actor) {
  if (!actor) return null;
  const valid = getValidFoundryRingIds();
  const tokFlag = actor.token?.getFlag?.(MRINGS_MODULE_ID, 'ringAppearance')
    ?? actor.token?._source?.flags?.[MRINGS_MODULE_ID]?.ringAppearance
    ?? actor.token?.flags?.[MRINGS_MODULE_ID]?.ringAppearance;
  const protoFlag = actor.prototypeToken?._source?.flags?.[MRINGS_MODULE_ID]?.ringAppearance
    ?? actor.prototypeToken?.flags?.[MRINGS_MODULE_ID]?.ringAppearance;
  const chosen = String(tokFlag || protoFlag || '').trim();
  return (chosen && valid.includes(chosen)) ? chosen : null;
}

/**
 * Текущее динамическое кольцо актёра/токена (с учётом флага Multiple Rings
 * и дефолта мира).
 */
export function getActorFoundryRingId(actor) {
  return getActorExplicitRingId(actor) || getWorldFoundryRingId();
}

/** Метаданные всех доступных колец Foundry (с уже готовыми кэшированными миниатюрами). */
export function listFoundryRings() {
  const ringCfg = globalThis.CONFIG?.Token?.ring;
  const ids = getValidFoundryRingIds();
  const worldRingId = getWorldFoundryRingId();
  const rings = ids.map(id => {
    const cfg = ringCfg?.getConfig?.(id);
    const cached = _thumbCache.get(id);
    return {
      id,
      label: localizeRingLabel(cfg?.label, id),
      isWorldDefault: id === worldRingId,
      thumbUrl: cached?.thumbUrl || null,
      scale: cached?.scale || 1
    };
  });
  return {
    rings,
    worldRingId,
    multipleRingsActive: !!game?.modules?.get?.(MRINGS_MODULE_ID)?.active
  };
}

/** Загрузить и распарсить JSON спрайт-листа кольца (результат кешируется). */
async function loadSheetMeta(cfgId) {
  if (_sheetMetaCache.has(cfgId)) return _sheetMetaCache.get(cfgId);
  const p = (async () => {
    const cfg = globalThis.CONFIG?.Token?.ring?.getConfig?.(cfgId);
    if (!cfg?.spritesheet) return null;
    const sheetUrl = resolveUrl(cfg.spritesheet, null);
    if (!sheetUrl) return null;
    const res = await fetch(sheetUrl);
    if (!res.ok) throw new Error(`HTTP ${res.status}: ${sheetUrl}`);
    const json = await res.json();
    const frames = json?.frames || {};
    const imgRel = json?.meta?.image;
    if (!imgRel) return null;
    const imgUrl = resolveUrl(imgRel, sheetUrl);
    const sheetCfg = json?.config || {};
    const overlays = sheetCfg.overlays || {};

    const variants = [];
    for (const [name, entry] of Object.entries(frames)) {
      if (!entry?.frame) continue;
      if (name.endsWith('-bkg') || name.endsWith('-msk') || name.endsWith('-top')) continue;
      const fr = entry.frame;
      if (!fr.w || !fr.h) continue;
      const bkgEntry = frames[`${name}-bkg`] || null;
      const mskEntry = frames[`${name}-msk`] || null;
      const inlineTop = frames[`${name}-top`] || null;
      const extOverlay = overlays[name] || null;
      let topSpec = null;
      if (extOverlay?.frame && extOverlay?.image) {
        topSpec = {
          imgUrl: resolveUrl(extOverlay.image, sheetUrl),
          frame: extOverlay.frame
        };
      } else if (inlineTop?.frame) {
        topSpec = {
          imgUrl,
          frame: inlineTop.frame
        };
      }
      variants.push({
        name,
        gridTarget: Number(entry.gridTarget) || (fr.w <= 512 ? 1 : fr.w <= 1024 ? 2 : 4),
        w: fr.w,
        h: fr.h,
        ringFrame: fr,
        bkgFrame: bkgEntry?.frame || null,
        mskFrame: mskEntry?.frame || null,
        topSpec,
        colorBand: entry.colorBand ?? sheetCfg.defaultColorBand ?? { startRadius: 0.59, endRadius: 0.7225 },
        ringColor: parseHexRgb01(entry.ringColor ?? sheetCfg.defaultRingColor ?? cfg.ringColor, [1, 1, 1]),
        bgColor: parseHexRgb01(entry.backgroundColor ?? sheetCfg.defaultBackgroundColor ?? cfg.backgroundColor, [1, 1, 1])
      });
    }
    if (!variants.length) return null;
    variants.sort((a, b) => a.w - b.w);
    const rawScale = Number(sheetCfg.scale);
    const scale = Number.isFinite(rawScale) && rawScale > 0 ? rawScale : 1;
    return { cfgId, sheetUrl, imgUrl, scale, variants };
  })().catch(err => {
    console.warn(`Tokenatra | failed to parse ring sheet "${cfgId}":`, err);
    _sheetMetaCache.delete(cfgId);
    return null;
  });
  _sheetMetaCache.set(cfgId, p);
  return p;
}

/** Вырезать прямоугольник кадра из Blob листа и сразу нарисовать в целевой размер. */
async function extractFrameImageData(sheetBlob, frame, targetSize) {
  if (!sheetBlob || !frame) return null;
  const bmp = await createImageBitmap(sheetBlob, frame.x, frame.y, frame.w, frame.h);
  try {
    const c = document.createElement('canvas');
    c.width = targetSize;
    c.height = targetSize;
    const ctx = c.getContext('2d', { willReadFrequently: true });
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(bmp, 0, 0, targetSize, targetSize);
    return ctx.getImageData(0, 0, targetSize, targetSize);
  } finally {
    bmp.close();
  }
}

async function extractFrameCanvas(sheetBlob, frame, targetSize) {
  if (!sheetBlob || !frame) return null;
  const bmp = await createImageBitmap(sheetBlob, frame.x, frame.y, frame.w, frame.h);
  try {
    const c = document.createElement('canvas');
    c.width = targetSize;
    c.height = targetSize;
    const ctx = c.getContext('2d');
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(bmp, 0, 0, targetSize, targetSize);
    return c;
  } finally {
    bmp.close();
  }
}

/**
 * Попиксельный композит bkg + colorized ring по формуле TokenRingSamplerShader.
 * Возвращает готовый HTMLCanvasElement размера targetSize×targetSize.
 */
function compositeRingPixels(targetSize, variant, ringImgData, bkgImgData, mskImgData) {
  const outCanvas = document.createElement('canvas');
  outCanvas.width = targetSize;
  outCanvas.height = targetSize;
  const outCtx = outCanvas.getContext('2d');
  const outImg = outCtx.createImageData(targetSize, targetSize);
  const od = outImg.data;

  const rd = ringImgData.data;
  const bd = bkgImgData ? bkgImgData.data : null;
  const md = mskImgData ? mskImgData.data : null;

  const [rcR, rcG, rcB] = variant.ringColor;
  const [bcR, bcG, bcB] = variant.bgColor;
  const tintBg = (Math.abs(bcR - 1) > 1e-3) || (Math.abs(bcG - 1) > 1e-3) || (Math.abs(bcB - 1) > 1e-3);

  const bandStart = Number(variant.colorBand?.startRadius) || 0;
  const bandEnd = Number(variant.colorBand?.endRadius) || 0;
  const hasBand = bandEnd > bandStart;

  const invSize2 = 2 / targetSize;

  for (let y = 0; y < targetSize; y++) {
    const dy = (y + 0.5) * invSize2 - 1;
    const dy2 = dy * dy;
    const rowOff = y * targetSize;
    for (let x = 0; x < targetSize; x++) {
      const i = (rowOff + x) * 4;

      // 1) colorizeTokenBackground (premultiplied)
      let bgPmR = 0, bgPmG = 0, bgPmB = 0, bgA = 0;
      if (bd) {
        const ba = bd[i + 3] / 255;
        if (ba > 0) {
          let br = bd[i] / 255;
          let bg = bd[i + 1] / 255;
          let bb = bd[i + 2] / 255;
          if (tintBg) {
            br = br < 0.5 ? (2 * br * bcR) : (1 - 2 * (1 - br) * (1 - bcR));
            bg = bg < 0.5 ? (2 * bg * bcG) : (1 - 2 * (1 - bg) * (1 - bcG));
            bb = bb < 0.5 ? (2 * bb * bcB) : (1 - 2 * (1 - bb) * (1 - bcB));
          }
          bgA = ba;
          bgPmR = br * ba;
          bgPmG = bg * ba;
          bgPmB = bb * ba;
        }
      }

      // 2) colorizeTokenRing (premultiplied)
      const ra = rd[i + 3] / 255;
      const ma = md ? (md[i + 3] / 255) : 0;
      let ringPmR = 0, ringPmG = 0, ringPmB = 0, ringA = 0;

      if (ra > 0 || ma > 0) {
        const tr = rd[i] / 255;
        const tg = rd[i + 1] / 255;
        const tb = rd[i + 2] / 255;
        const redCh = ma > 0 ? (md[i] / 255) : tr;
        const gradR = rcR * redCh;
        const gradG = rcG * redCh;
        const gradB = rcB * redCh;

        let mixF = 0;
        if (hasBand && ra > 0) {
          const dx = (x + 0.5) * invSize2 - 1;
          const dist = Math.sqrt(dx * dx + dy2);
          if (dist >= bandStart && dist < bandEnd) mixF = 1;
        }

        const cR = mixF ? gradR : tr;
        const cG = mixF ? gradG : tg;
        const cB = mixF ? gradB : tb;
        ringPmR = cR * ra;
        ringPmG = cG * ra;
        ringPmB = cB * ra;
        ringA = ra;

        if (ma > 0) {
          const invMa = 1 - ma;
          ringPmR = gradR * ma + ringPmR * invMa;
          ringPmG = gradG * ma + ringPmG * invMa;
          ringPmB = gradB * ma + ringPmB * invMa;
          ringA = ma + ringA * invMa;
        }
      }

      // 3) blend(ringColor, backgroundColor) -> straight RGBA
      const outA = ringA + bgA * (1 - ringA);
      if (outA > 1e-5) {
        const invRingA = 1 - ringA;
        const outPmR = ringPmR + bgPmR * invRingA;
        const outPmG = ringPmG + bgPmG * invRingA;
        const outPmB = ringPmB + bgPmB * invRingA;
        od[i]     = Math.min(255, Math.max(0, Math.round((outPmR / outA) * 255)));
        od[i + 1] = Math.min(255, Math.max(0, Math.round((outPmG / outA) * 255)));
        od[i + 2] = Math.min(255, Math.max(0, Math.round((outPmB / outA) * 255)));
        od[i + 3] = Math.min(255, Math.max(0, Math.round(outA * 255)));
      }
    }
  }

  outCtx.putImageData(outImg, 0, 0);
  return outCanvas;
}

function pickThumbVariant(variants) {
  let best = variants[0];
  let bestDiff = Math.abs((best.gridTarget || 1) - 1);
  for (const v of variants) {
    const diff = Math.abs((v.gridTarget || 1) - 1);
    if (diff < bestDiff || (diff === bestDiff && v.w < best.w)) {
      best = v;
      bestDiff = diff;
    }
  }
  return best;
}

function pickFullVariant(variants) {
  // Предпочитаем самый крупный кадр ≤ 2048px (например large-huge 2048 на 2×
  // листах и gargantuan 2048 на 1× листах), чтобы не декодировать 4096².
  const capped = variants.filter(v => v.w <= 2048 && v.h <= 2048);
  if (capped.length) return capped[capped.length - 1];
  return variants[0];
}

/** Миниатюра кольца 144×144 (dataURL WebP), с кэшированием и дедупликацией. */
export async function getFoundryRingThumb(cfgId) {
  if (_thumbCache.has(cfgId)) return _thumbCache.get(cfgId);
  if (_thumbPending.has(cfgId)) return _thumbPending.get(cfgId);

  const job = (async () => {
    const meta = await loadSheetMeta(cfgId);
    if (!meta) return null;
    const v = pickThumbVariant(meta.variants);
    const thumbSize = 144;

    const sheetRes = await fetch(meta.imgUrl);
    if (!sheetRes.ok) throw new Error(`HTTP ${sheetRes.status}`);
    const sheetBlob = await sheetRes.blob();

    const [ringData, bkgData, mskData] = await Promise.all([
      extractFrameImageData(sheetBlob, v.ringFrame, thumbSize),
      v.bkgFrame ? extractFrameImageData(sheetBlob, v.bkgFrame, thumbSize) : Promise.resolve(null),
      v.mskFrame ? extractFrameImageData(sheetBlob, v.mskFrame, thumbSize) : Promise.resolve(null)
    ]);

    const canvas = compositeRingPixels(thumbSize, v, ringData, bkgData, mskData);

    // В миниатюру сразу дорисовываем -top слой, чтобы превью в сетке отражало полный вид кольца
    if (v.topSpec) {
      try {
        const topBlob = v.topSpec.imgUrl === meta.imgUrl
          ? sheetBlob
          : await fetch(v.topSpec.imgUrl).then(r => r.ok ? r.blob() : null);
        if (topBlob) {
          const topCanvas = await extractFrameCanvas(topBlob, v.topSpec.frame, thumbSize);
          if (topCanvas) {
            canvas.getContext('2d').drawImage(topCanvas, 0, 0);
            topCanvas.width = 1;
            topCanvas.height = 1;
          }
        }
      } catch { /* миниатюра валидна и без top */ }
    }

    const dataUrl = canvas.toDataURL('image/webp', 0.84);
    canvas.width = 1;
    canvas.height = 1;
    const result = { thumbUrl: dataUrl, scale: meta.scale || 1 };
    _thumbCache.set(cfgId, result);
    return result;
  })().catch(err => {
    console.warn(`Tokenatra | ring thumb failed "${cfgId}":`, err);
    return null;
  }).finally(() => {
    _thumbPending.delete(cfgId);
  });

  _thumbPending.set(cfgId, job);
  return job;
}

function canvasToBlob(canvas, type = 'image/webp', quality = 0.95) {
  return new Promise(resolve => canvas.toBlob(resolve, type, quality));
}

/**
 * Полноразмерный рендер кольца для холста студии:
 * возвращает { id, scale, ringBlob, topBlob } (Blob передаются через postMessage без base64-оверхеда).
 */
export async function renderFoundryRingFull(cfgId) {
  if (_fullRingCache.has(cfgId)) {
    const hit = _fullRingCache.get(cfgId);
    _fullRingCache.delete(cfgId);
    _fullRingCache.set(cfgId, hit);
    return { id: cfgId, ...hit };
  }

  const meta = await loadSheetMeta(cfgId);
  if (!meta) throw new Error(`Ring config "${cfgId}" unavailable`);
  const v = pickFullVariant(meta.variants);
  const outSize = Math.min(2048, Math.max(512, v.w));

  const sheetRes = await fetch(meta.imgUrl);
  if (!sheetRes.ok) throw new Error(`HTTP ${sheetRes.status}`);
  const sheetBlob = await sheetRes.blob();

  const [ringData, bkgData, mskData] = await Promise.all([
    extractFrameImageData(sheetBlob, v.ringFrame, outSize),
    v.bkgFrame ? extractFrameImageData(sheetBlob, v.bkgFrame, outSize) : Promise.resolve(null),
    v.mskFrame ? extractFrameImageData(sheetBlob, v.mskFrame, outSize) : Promise.resolve(null)
  ]);

  const ringCanvas = compositeRingPixels(outSize, v, ringData, bkgData, mskData);
  const ringBlob = await canvasToBlob(ringCanvas, 'image/webp', 0.96);
  ringCanvas.width = 1;
  ringCanvas.height = 1;

  let topBlob = null;
  if (v.topSpec) {
    try {
      const srcTopBlob = v.topSpec.imgUrl === meta.imgUrl
        ? sheetBlob
        : await fetch(v.topSpec.imgUrl).then(r => r.ok ? r.blob() : null);
      if (srcTopBlob) {
        const topCanvas = await extractFrameCanvas(srcTopBlob, v.topSpec.frame, outSize);
        if (topCanvas) {
          topBlob = await canvasToBlob(topCanvas, 'image/webp', 0.96);
          topCanvas.width = 1;
          topCanvas.height = 1;
        }
      }
    } catch (err) {
      console.warn(`Tokenatra | ring top overlay failed "${cfgId}":`, err);
    }
  }

  const entry = { ringBlob, topBlob, scale: meta.scale };
  if (_fullRingCache.size >= FULL_CACHE_LIMIT) {
    const oldest = _fullRingCache.keys().next().value;
    if (oldest !== undefined) _fullRingCache.delete(oldest);
  }
  _fullRingCache.set(cfgId, entry);
  return { id: cfgId, ...entry };
}
