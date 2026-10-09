/**
 * upload.js — сохранение результата в хранилище Foundry.
 *
 * Цепочка: GM (FILES_UPLOAD) → data:<target>; игрок → свой пользовательский
 * каталог. Без прав на загрузку НИЧЕГО не пишем в документы (data:/blob:
 * URL в персистентных полях — мусор в БД мира) — скачиваем файл локально
 * и честно предупреждаем.
 */

const MODULE_ID = 'tokenatra-connector';

// Транслитерация имён файлов: кириллица (ru/ua/be/sr/mk/bg + архаика),
// греческий, латиница с диакритикой. Алфавиты без побуквенного эквивалента
// (CJK, арабица и т.п.) вычищаются санитайзером — в мире остаются только
// латинские имена.
const CYR_MAP = {
  'а': 'a', 'б': 'b', 'в': 'v', 'г': 'g', 'д': 'd', 'е': 'e', 'ё': 'yo',
  'ж': 'zh', 'з': 'z', 'и': 'i', 'й': 'y', 'к': 'k', 'л': 'l', 'м': 'm',
  'н': 'n', 'о': 'o', 'п': 'p', 'р': 'r', 'с': 's', 'т': 't', 'у': 'u',
  'ф': 'f', 'х': 'kh', 'ц': 'ts', 'ч': 'ch', 'ш': 'sh', 'щ': 'shch',
  'ъ': '', 'ы': 'y', 'ь': '', 'э': 'e', 'ю': 'yu', 'я': 'ya',
  // украинский / белорусский
  'ґ': 'g', 'є': 'ye', 'і': 'i', 'ї': 'yi', 'ў': 'u',
  // сербский / македонский
  'ђ': 'dj', 'ј': 'j', 'љ': 'lj', 'њ': 'nj', 'ћ': 'c', 'џ': 'dz',
  'ѕ': 'dz', 'ќ': 'kj', 'ѓ': 'gj', 'ѐ': 'e', 'ѝ': 'i',
  // архаика
  'ѣ': 'ye', 'ѳ': 'f', 'ѵ': 'y'
};

// Латинские буквы/лигатуры, не разлагающиеся через NFD
const LATIN_MAP = {
  'ß': 'ss', 'æ': 'ae', 'œ': 'oe', 'ø': 'o', 'ð': 'd',
  'đ': 'd', 'þ': 'th', 'ł': 'l', 'ı': 'i'
};

const GREEK_MAP = {
  'α': 'a', 'β': 'v', 'γ': 'g', 'δ': 'd', 'ε': 'e', 'ζ': 'z', 'η': 'i',
  'θ': 'th', 'ι': 'i', 'κ': 'k', 'λ': 'l', 'μ': 'm', 'ν': 'n', 'ξ': 'x',
  'ο': 'o', 'π': 'p', 'ρ': 'r', 'σ': 's', 'ς': 's', 'τ': 't', 'υ': 'y',
  'φ': 'f', 'χ': 'ch', 'ψ': 'ps', 'ω': 'o'
};

function mapCase(ch, t) {
  return (t && ch !== ch.toLowerCase()) ? (t.charAt(0).toUpperCase() + t.slice(1)) : t;
}

export function transliterate(str) {
  return String(str ?? '')
    // особые латинские буквы (до NFD: они не разлагаются)
    .replace(/[ßæœøðđþłı]/gi, (ch) => mapCase(ch, LATIN_MAP[ch.toLowerCase()] ?? ch))
    // кириллица (до NFD: й/ў и т.п. не должны разваливаться на базу + breve)
    .replace(/[\u0400-\u04FF]/g, (ch) => mapCase(ch, CYR_MAP[ch.toLowerCase()] ?? ''))
    // диакритика латиницы и греческие тоносы (ä -> a, ά -> α)
    .normalize('NFD')
    .replace(/[\u0300-\u036f]/g, '')
    // греческая база
    .replace(/[\u0370-\u03ff\u1f00-\u1fff]/g, (ch) => mapCase(ch, GREEK_MAP[ch.toLowerCase()] ?? ''));
}

/** Первое свободное имя: если занято — суффикс -2, -3, … (ничего не перезаписываем). */
async function uniqueFileName(source, target, fileName) {
  try {
    const listing = await FilePicker.browse(source, target);
    const taken = new Set((listing.files ?? []).map(f => f.split('/').pop().toLowerCase()));
    if (!taken.has(fileName.toLowerCase())) return fileName;
    const dot = fileName.lastIndexOf('.');
    const stem = dot > 0 ? fileName.slice(0, dot) : fileName;
    const ext = dot > 0 ? fileName.slice(dot) : '';
    for (let n = 2; n < 100; n++) {
      const cand = `${stem}-${n}${ext}`;
      if (!taken.has(cand.toLowerCase())) return cand;
    }
  } catch (err) { /* browse недоступен — сохраняем с исходным именем */ }
  return fileName;
}

export function sanitizeFileName(name, fallback = 'tokenatra') {
  const cleaned = String(name || '')
    .replace(/[\\/:*?"<>|]+/g, '_')
    .replace(/[^\w.\- ]+/g, '_')
    .replace(/\s+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 64);
  return cleaned || fallback;
}

/**
 * Загружает блоб в хранилище. Возвращает путь ("tokenatra/img.webp") или
 * null, если прав нет (файл при этом скачан локально).
 */
export async function upload(blob, fileName) {
  // Транслитерация + санитизация: в мире остаются только латинские имена
  const clean = sanitizeFileName(transliterate(fileName));

  if (game.user.can('FILES_UPLOAD')) {
    const target = game.settings.get(MODULE_ID, 'uploadTarget') || 'tokenatra/';
    try {
      // Папка назначения (Data/tokenatra) — создаём при отсутствии
      await FilePicker.createDirectory('data', target.replace(/\/+$/, ''));
    } catch (err) { /* уже существует или нет прав — пробуем как есть */ }
    try {
      const unique = await uniqueFileName('data', target, clean);
      const file = new File([blob], unique, { type: blob.type || 'image/webp' });
      const res = await FilePicker.upload('data', target, file);
      if (res?.path) return res.path;
    } catch (err) { /* падаем на следующую ступень */ }
  }
  try {
    const unique = await uniqueFileName('user', '', clean);
    const file = new File([blob], unique, { type: blob.type || 'image/webp' });
    const res = await FilePicker.upload('user', '', file);
    if (res?.path) return res.path;
  } catch (err) { /* фолбэк ниже */ }

  downloadBlob(blob, clean);
  console.warn(game.i18n.localize('TKC.WarnNoUploadRights'));
  return null;
}

function downloadBlob(blob, fileName) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = fileName;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
}
