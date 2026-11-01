# -*- coding: utf-8 -*-
"""agent_pipeline.py — headless-ядро редактора токенов для ИИ-агентов.

Чистый Python (Pillow/numpy) без Flask: композиция, маски, эффекты,
инспекция. Переиспользует инференс server.remove_background (ленивый
импорт — без циклических зависимостей).

Паритет с фронтендом (TokenCanvas.renderForSave, tokenEffects.js):
  рендер quality*3 -> кольцо снизу (quality, по центру) -> субъект
  (fit-1024 * scale, поворот, смещение) -> маска destination-in ->
  тень под субъектом -> кроп quality*usedScale по центру.

Расхождения (приняты по итогам ревью, задокументированы в docs/AGENT_API.md):
  - transform.scale — относительно вписанного в 1024 (fit), а не
    абсолютного размера пикселей как во фронтенде;
  - тень/цветокоррекция визуально эквивалентны, не бит-в-бит
    (canvas blur(R) ~= GaussianBlur(R/2); ImageEnhance vs ctx.filter);
  - база капится до 2048 px (UI работает до 6144) — агентный путь
    чуть грубее на экспорте x3.

Инвариант безопасности (аудит): операции ядра никогда не исполняют
код, не читают секреты/конфиг, не пишут на диск (экспорт отдаёт байты)
и не ходят в сеть. Источники — только base64/data_url.
"""
import base64
import io
import math
import re
import threading
import time
import uuid

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageEnhance, ImageFilter

# ===== Параметры (вынесены константами: тюнить в одном месте) =====

LOGICAL = 1024          # единое координатное пространство API
MAX_BASE_DIM = 2048     # кап стороны исходника сессии
MAX_SESSIONS = 4
SESSION_TTL_MIN = 30    # скользящий: сбрасывается любой операцией
MAX_OPS_PER_SESSION = 200
DEFAULT_PREVIEW = 384
MAX_PREVIEW = 768
EXPORT_SIZE_RANGE = (256, 2048)
EXPORT_QUALITY_RANGE = (10, 100)

DEFAULT_EFFECTS = {
    'shadow': {'enabled': False, 'angle': 135, 'distance': 7,
               'blur': 3, 'opacity': 0.4},
    'color': {'enabled': False, 'saturation': 0, 'lightness': 0},
}

_DATA_URL_RE = re.compile(r'^data:image/[a-z+]+;base64,', re.IGNORECASE)


class AgentError(Exception):
    """Ошибка с машиночитаемым кодом (см. каталог в agent_api)."""

    def __init__(self, code, message, hint=None, valid=None, status=400,
                 payload=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.hint = hint
        self.valid = valid
        self.status = status
        self.payload = payload or {}


# ===== Кэши (процесс-глобальные, сидят под GIL — достаточно) =====

_RING_CACHE = {}        # name -> RGBA (натуральный размер)
_PROT_CACHE = {}        # путь -> 'L' 1024 (255 = защищено)
_CACHE_LOCK = threading.Lock()


def _clear_caches():
    with _CACHE_LOCK:
        _RING_CACHE.clear()
        _PROT_CACHE.clear()


# ===== Сессия =====

class AgentSession:
    def __init__(self, base):
        self.sid = uuid.uuid4().hex  # 128 бит
        self.lock = threading.RLock()
        self.created = time.time()
        self.last_used = time.time()
        self.ops = 0
        self.base = base                  # RGBA, сторона <= MAX_BASE_DIM
        self.alpha = Image.new('L', (LOGICAL, LOGICAL), 255)  # маска видимости
        self.bg_removed = False
        self.transform = {'scale': 1.0, 'rotation': 0.0,
                          'offset_x': 0.0, 'offset_y': 0.0}
        self.ring = None                  # имя кольца из каталога или None
        self.effects = {
            'shadow': dict(DEFAULT_EFFECTS['shadow']),
            'color': dict(DEFAULT_EFFECTS['color']),
        }
        self.recipe = [{'op': 'create', 'width': base.width,
                        'height': base.height}]
        self._cc_cache = None             # (ключ, скорректированный RGBA)
        self.last_inspect = None

    # --- служебное ---

    def touch(self):
        self.last_used = time.time()

    def log_op(self, op, **params):
        self.ops += 1
        entry = {'op': op}
        entry.update(params)
        self.recipe.append(entry)

    def state_summary(self):
        return {
            'session_id': self.sid,
            'image': {'width': self.base.width, 'height': self.base.height},
            'bg_removed': self.bg_removed,
            'transform': dict(self.transform),
            'ring': self.ring,
            'effects': {
                'shadow': dict(self.effects['shadow']),
                'color': dict(self.effects['color']),
            },
            'mask': _mask_stats(self.alpha),
            'ops_used': self.ops,
            'ops_limit': MAX_OPS_PER_SESSION,
            'age_sec': round(time.time() - self.created, 1),
            'recipe': list(self.recipe),
        }


class SessionStore:
    """Реестр сессий: кап + скользящий TTL (ленивая эвикция, без потока)."""

    def __init__(self):
        self._lock = threading.Lock()
        self._sessions = {}

    def _evict_expired(self):
        deadline = time.time() - SESSION_TTL_MIN * 60
        for sid in [s for s, v in self._sessions.items() if v.last_used < deadline]:
            self._sessions.pop(sid, None)

    def create(self, base):
        with self._lock:
            self._evict_expired()
            if len(self._sessions) >= MAX_SESSIONS:
                # LRU: выкидываем самую давно использованную
                oldest = min(self._sessions.values(), key=lambda s: s.last_used)
                self._sessions.pop(oldest.sid, None)
            s = AgentSession(base)
            self._sessions[s.sid] = s
            return s

    def get(self, sid):
        with self._lock:
            self._evict_expired()
            s = self._sessions.get(sid)
        if s is None:
            raise AgentError('session_not_found',
                             f'session {sid} does not exist or expired',
                             hint='create a new session: POST /agent/v1/sessions',
                             status=404)
        s.touch()
        return s

    def close(self, sid):
        with self._lock:
            return self._sessions.pop(sid, None) is not None

    def count(self):
        with self._lock:
            self._evict_expired()
            return len(self._sessions)


STORE = SessionStore()


# ===== Источник: только base64 / data_url (аудит: никаких path) =====

def decode_source(data):
    """Строка data:image/...;base64,XXXX или чистый base64 -> RGBA."""
    if not isinstance(data, str) or not data.strip():
        raise AgentError('bad_params', 'image must be a base64 or data URL string')
    raw = _DATA_URL_RE.sub('', data.strip())
    try:
        binary = base64.b64decode(raw, validate=False)
    except Exception:
        raise AgentError('bad_params', 'image is not valid base64')
    try:
        img = Image.open(io.BytesIO(binary))
        img.load()
    except Exception:
        raise AgentError('bad_image', 'image data could not be decoded',
                         valid=['png', 'webp', 'jpg', 'bmp'])
    rgba = img.convert('RGBA')
    # Кап стороны: память сессий важнее пиксельной точности (см. доку)
    m = max(rgba.width, rgba.height)
    if m > MAX_BASE_DIM:
        k = MAX_BASE_DIM / m
        rgba = rgba.resize((max(1, round(rgba.width * k)),
                            max(1, round(rgba.height * k))), Image.LANCZOS)
    return rgba


# ===== Каталоги (кольца/пресеты/маска защиты) =====

def _srv():
    import server  # лениво: без циклического импорта
    return server


def list_assets():
    srv = _srv()
    from pathlib import Path
    rings = []
    for d, builtin in ((srv.BUILTIN_RING_DIR, True), (srv.RING_DIR, False)):
        if d.exists():
            for f in sorted(d.iterdir()):
                if (f.is_file() and f.suffix.lower() in {'.webp', '.png', '.jpg', '.jpeg'}
                        and not f.stem.casefold().endswith('.mask')):
                    rings.append({'name': f.stem, 'file': f.name, 'builtin': builtin})
    presets = []
    for d, builtin in ((srv.BUILTIN_PRESET_DIR, True), (srv.USER_PRESET_DIR, False)):
        if d.exists():
            for f in sorted(d.iterdir()):
                if f.is_file() and f.suffix.lower() in {'.png', '.webp', '.jpg', '.jpeg'}:
                    presets.append({'name': f.stem, 'file': f.name, 'builtin': builtin})
    seen = set()
    rings = [r for r in rings
             if not (r['name'].casefold() in seen or seen.add(r['name'].casefold()))]
    return {'rings': rings, 'presets': presets,
            'logical_size': LOGICAL}


def _find_ring_file(name):
    srv = _srv()
    for d in (srv.RING_DIR, srv.BUILTIN_RING_DIR):
        for ext in ('.webp', '.png', '.jpg', '.jpeg'):
            p = d / (name + ext)
            if p.is_file():
                return p
    return None


def _ring_image(name):
    with _CACHE_LOCK:
        img = _RING_CACHE.get(name)
    if img is None:
        path = _find_ring_file(name)
        if path is None:
            raise AgentError('unknown_asset', f"ring '{name}' not found",
                             hint='GET /agent/v1/assets lists available rings')
        img = Image.open(path).convert('RGBA')
        img.load()
        with _CACHE_LOCK:
            _RING_CACHE[name] = img
    return img


def _protection_image(session):
    """255 = защищено от стирания. Приоритет: маска активного кольца,
    затем mask.png. Семантика фронтенда: альфа > 0 -> защищено
    (buildProtectionCanvasFromImg)."""
    srv = _srv()
    candidates = []
    if session.ring:
        stem = session.ring
        for d in (srv.RING_DIR, srv.BUILTIN_RING_DIR):
            for ext in ('.webp', '.png', '.jpg', '.jpeg'):
                p = d / f'{stem}.mask{ext}'
                if p.is_file():
                    candidates.append(p)
    candidates.append(srv.MASK_PATH)
    for path in candidates:
        if not path.exists():
            continue
        key = str(path)
        with _CACHE_LOCK:
            cached = _PROT_CACHE.get(key)
        if cached is not None:
            return cached
        prot = Image.open(path).convert('RGBA')
        prot.load()
        prot = prot.getchannel('A').point(lambda v: 255 if v > 0 else 0)
        prot = prot.resize((LOGICAL, LOGICAL), Image.LANCZOS)
        with _CACHE_LOCK:
            _PROT_CACHE[key] = prot
        return prot
    return None


def _find_preset_file(name):
    srv = _srv()
    for d in (srv.USER_PRESET_DIR, srv.BUILTIN_PRESET_DIR):
        for ext in ('.png', '.webp', '.jpg', '.jpeg'):
            p = d / (name + ext)
            if p.is_file():
                return p
    return None


# ===== Операции сессии =====

def remove_bg(session, edge_blur=1.0):
    srv = _srv()
    srv.mark_activity()  # tray idle-unload не должен выгрузить модель
    result = srv.remove_background(session.base, edge_blur)
    with session.lock:
        session.base = result
        session.bg_removed = True
        session.log_op('remove_bg', edge_blur=edge_blur)


_SHAPE_KEYS = ('circle', 'rect', 'stroke')


def _apply_shapes(mask_img, shapes, value):
    """Рисует фигуры на L-маске: value=0 (стереть) или 255 (вернуть)."""
    draw = ImageDraw.Draw(mask_img)
    for sh in shapes:
        if not isinstance(sh, dict):
            raise AgentError('bad_params', 'each shape must be an object')
        if 'circle' in sh:
            c = sh['circle']
            cx, cy, r = _num(c.get('cx')), _num(c.get('cy')), _num(c.get('r'))
            if r <= 0:
                raise AgentError('bad_params', 'circle.r must be > 0')
            draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=value)
        elif 'rect' in sh:
            rc = sh['rect']
            x0, y0 = _num(rc.get('x0')), _num(rc.get('y0'))
            x1, y1 = _num(rc.get('x1')), _num(rc.get('y1'))
            draw.rectangle([min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1)],
                           fill=value)
        elif 'stroke' in sh:
            st = sh['stroke']
            pts = st.get('points')
            w = _num(st.get('width', 20))
            if not isinstance(pts, list) or len(pts) < 2:
                raise AgentError('bad_params',
                                 'stroke.points must be a list of [x,y] pairs')
            polyline = [(float(p[0]), float(p[1])) for p in pts]
            draw.line(polyline, fill=value, width=max(1, round(w)), joint='curve')
        else:
            raise AgentError('bad_params',
                             f'shape must have one of {_SHAPE_KEYS}', valid=_SHAPE_KEYS)
    return mask_img


def mask_op(session, op, shapes=None, preset=None, px=0, protect=True):
    with session.lock:
        alpha = session.alpha
        if op in ('erase', 'restore'):
            value = 0 if op == 'erase' else 255
            work = alpha.copy()
            _apply_shapes(work, shapes or [], value)
            if op == 'erase' and protect:
                prot = _protection_image(session)
                if prot is not None:
                    # Защищённые зоны не стираются (семантика воркера)
                    work = Image.composite(alpha, work, prot)
            session.alpha = work
        elif op == 'apply_preset':
            path = _find_preset_file(str(preset))
            if path is None:
                raise AgentError('unknown_asset', f"preset '{preset}' not found",
                                 hint='GET /agent/v1/assets lists available presets')
            img = Image.open(path).convert('RGBA')
            img.load()
            # Пресет растягивается на весь логический канвас (как фронтенд)
            session.alpha = img.getchannel('A').resize(
                (LOGICAL, LOGICAL), Image.LANCZOS)
        elif op == 'invert':
            session.alpha = ImageChops.invert(alpha)
        elif op == 'feather':
            session.alpha = alpha.filter(
                ImageFilter.GaussianBlur(max(0.1, px / 2.0)))
        elif op == 'grow':
            session.alpha = alpha.filter(
                ImageFilter.MaxFilter(2 * max(1, round(px)) + 1))
        else:
            raise AgentError('bad_params', f'unknown mask op {op!r}',
                             valid=['erase', 'restore', 'apply_preset',
                                    'invert', 'feather', 'grow'])
        session.log_op('mask.' + op, shapes=shapes, preset=preset, px=px,
                       protect=protect)
        return _mask_stats(session.alpha)


def set_transform(session, **params):
    with session.lock:
        t = dict(session.transform)
        for key in ('scale', 'rotation', 'offset_x', 'offset_y'):
            if key in params and params[key] is not None:
                t[key] = _num(params[key])
        if not (0.05 <= t['scale'] <= 8.0):
            raise AgentError('bad_params', 'scale must be in [0.05, 8.0]')
        if not (-180.0 <= t['rotation'] <= 180.0):
            raise AgentError('bad_params', 'rotation must be in [-180, 180] deg')
        if max(abs(t['offset_x']), abs(t['offset_y'])) > LOGICAL:
            raise AgentError('bad_params',
                             f'offset must be within +/-{LOGICAL} px')
        session.transform = t
        session.log_op('transform', **t)


def set_ring(session, name):
    with session.lock:
        if name is not None:
            _ring_image(name)  # валидация существования
        session.ring = name
        session.log_op('ring', name=name)


def set_effects(session, shadow=None, color=None):
    with session.lock:
        for block, key in ((shadow, 'shadow'), (color, 'color')):
            if not block:
                continue
            cur = dict(session.effects[key])
            for k, v in block.items():
                if k not in cur:
                    raise AgentError('bad_params', f'unknown {key} param {k!r}',
                                     valid=sorted(cur))
                cur[k] = bool(v) if k == 'enabled' else _num(v)
            if key == 'shadow':
                if not (0.0 <= cur['opacity'] <= 1.0):
                    raise AgentError('bad_params', 'shadow.opacity must be in [0, 1]')
                if not (-360 <= cur['angle'] <= 360):
                    raise AgentError('bad_params', 'shadow.angle must be in [-360, 360]')
            session.effects[key] = cur
        session.log_op('effects', shadow=shadow, color=color)


# ===== Композиция (паритет renderForSave) =====

def _corrected_base(session):
    eff = session.effects['color']
    key = (eff['enabled'], eff['saturation'], eff['lightness'])
    if session._cc_cache and session._cc_cache[0] == key:
        return session._cc_cache[1]
    img = session.base
    if eff['enabled']:
        sat = 1.0 + eff['saturation'] / 100.0
        light = 1.0 + eff['lightness'] / 100.0
        if sat != 1.0:
            img = ImageEnhance.Color(img).enhance(sat)
        if light != 1.0:
            img = ImageEnhance.Brightness(img).enhance(light)
    session._cc_cache = (key, img)
    return img


def _subject_layer(session, s3):
    """Субъект на холсте s3×s3: fit-1024 * scale, поворот, смещение."""
    base = _corrected_base(session)
    t = session.transform
    k_fit = min(LOGICAL / base.width, LOGICAL / base.height)
    k = k_fit * t['scale'] * (s3 / LOGICAL)
    w = max(1, round(base.width * k))
    h = max(1, round(base.height * k))
    layer = base.resize((w, h), Image.LANCZOS)
    if t['rotation'] % 360 != 0:
        # canvas rotate(+deg) — по часовой; PIL rotate(+deg) — против
        layer = layer.rotate(-t['rotation'], resample=Image.BICUBIC,
                             expand=True)
    out = Image.new('RGBA', (s3, s3), (0, 0, 0, 0))
    cx = s3 / 2 + t['offset_x'] * (s3 / LOGICAL)
    cy = s3 / 2 + t['offset_y'] * (s3 / LOGICAL)
    out.alpha_composite(layer, (round(cx - layer.width / 2),
                                round(cy - layer.height / 2)))
    return out


def _apply_mask(session, layer, s3):
    mask = session.alpha.resize((s3, s3), Image.LANCZOS)
    combined = ImageChops.multiply(layer.getchannel('A'), mask)
    layer.putalpha(combined)
    return layer


def _shadow_layer(layer, s3, sh):
    """Тень от ЗАМАСКИРОВАННОГО субъекта (как в renderForSave)."""
    scale = s3 / LOGICAL
    rad = math.radians(sh['angle'])
    dx = round(math.cos(rad) * sh['distance'] * scale)
    dy = round(-math.sin(rad) * sh['distance'] * scale)
    sil = Image.new('L', (s3, s3), 0)
    sil.paste(layer.getchannel('A'), (dx, dy))
    sil = sil.filter(ImageFilter.GaussianBlur(
        max(0.1, sh['blur'] * scale / 2.0)))
    sil = sil.point(lambda v: round(v * sh['opacity']))
    black = Image.new('RGBA', (s3, s3), (0, 0, 0, 255))
    black.putalpha(sil)
    return black


def _detect_used_scale(layer, s3, scan=512):
    """Порт detectUsedScale: bbox альфы на 512-скане, порог 15, дилатация
    1 px скана, минимальный s чтобы bbox влез в центральный бокс s/3."""
    small = layer.getchannel('A').resize((scan, scan), Image.BILINEAR)
    a = np.array(small)
    ys, xs = np.nonzero(a > 15)
    if len(xs) == 0:
        return 1
    factor = s3 / scan
    min_x = max(0.0, xs.min() * factor - factor)
    min_y = max(0.0, ys.min() * factor - factor)
    max_x = min(float(s3), (xs.max() + 1) * factor + factor)
    max_y = min(float(s3), (ys.max() + 1) * factor + factor)
    for s in (1, 2, 3):
        border = round(s3 * s / 3)
        offset = (s3 - border) / 2
        if (min_x >= offset and min_y >= offset
                and max_x <= offset + border and max_y <= offset + border):
            return s
    return 3


def compose(session, size, scale_mode='auto', with_ring=True):
    """Финальная композиция. Возвращает (RGBA, used_scale)."""
    size = int(size)
    s3 = size * 3
    canvas = Image.new('RGBA', (s3, s3), (0, 0, 0, 0))

    if with_ring and session.ring:
        ring = _ring_image(session.ring).resize((size, size), Image.LANCZOS)
        off = (s3 - size) // 2
        canvas.alpha_composite(ring, (off, off))

    subject = _apply_mask(session, _subject_layer(session, s3), s3)

    if session.effects['shadow']['enabled']:
        canvas.alpha_composite(
            _shadow_layer(subject, s3, session.effects['shadow']), (0, 0))
    canvas.alpha_composite(subject, (0, 0))

    if scale_mode == 'auto':
        used = _detect_used_scale(subject, s3)
    else:
        used = int(scale_mode)
        if used not in (1, 2, 3):
            raise AgentError('bad_params', 'scale_mode must be auto, 1, 2 or 3',
                             valid=['auto', '1', '2', '3'])
    final = size * used
    crop = (s3 - final) // 2
    out = canvas.crop((crop, crop, crop + final, crop + final))
    return out, used


# ===== Inspect =====

def _mask_stats(mask):
    a = np.array(mask)
    total = a.size
    visible = int((a > 15).sum())
    ys, xs = np.nonzero(a > 15)
    return {
        'coverage_pct': round(visible * 100.0 / total, 2),
        'bbox': ([int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())]
                 if len(xs) else None),
    }


def inspect(session, include_image=False, preview_size=DEFAULT_PREVIEW):
    with session.lock:
        s3 = LOGICAL * 3
        subject = _apply_mask(session, _subject_layer(session, s3), s3)
        scan = subject.getchannel('A').resize((512, 512), Image.BILINEAR)
        a = np.array(scan)
        k = LOGICAL / 512.0

        ys, xs = np.nonzero(a > 15)
        coverage = round(float((a > 15).sum()) * 100.0 / a.size, 2)
        bbox = ([int(xs.min() * k), int(ys.min() * k),
                 int(xs.max() * k), int(ys.max() * k)]
                if len(xs) else None)

        grid = []
        for gy in range(3):
            row = []
            for gx in range(3):
                cell = a[gy * 170:(gy + 1) * 170, gx * 170:(gx + 1) * 170]
                row.append(round(float((cell > 15).sum()) * 100.0 / cell.size, 1))
            grid.append(row)

        edges = int(((a > 15) & (a < 240)).sum())
        hist = [int(((a >= i * 51) & (a < (i + 1) * 51)).sum()) for i in range(5)]

        warnings = []
        if bbox:
            if bbox[0] <= 2 or bbox[1] <= 2 or bbox[2] >= LOGICAL - 3 or bbox[3] >= LOGICAL - 3:
                warnings.append('subject touches canvas border')
        if coverage < 2.0:
            warnings.append('mask is almost empty: nothing to export')
        if not session.bg_removed:
            warnings.append('background not removed yet (remove_bg)')

        result = {
            'coverage_pct': coverage,
            'bbox': bbox,
            'grid_3x3_pct': grid,
            'edge_px': edges,
            'alpha_hist': hist,
            'warnings': warnings,
        }
        if session.last_inspect and isinstance(session.last_inspect, dict):
            prev = session.last_inspect
            result['delta'] = {
                'coverage_dpct': round(coverage - prev.get('coverage_pct', coverage), 2),
            }
        session.last_inspect = dict(result)

        if include_image:
            preview, _ = compose(session, preview_size, scale_mode='3',
                                 with_ring=True)
            buf = io.BytesIO()
            preview.save(buf, format='WEBP', quality=85)
            result['image_data_url'] = (
                'data:image/webp;base64,'
                + base64.b64encode(buf.getvalue()).decode('ascii'))
        return result


# ===== Валидация скаляров =====

def _num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        raise AgentError('bad_params', f'expected a number, got {v!r}')
