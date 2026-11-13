# -*- coding: utf-8 -*-
"""Сборка hosted-студии: копирует интерфейс Tokenatra в модуль Foundry.

python build_studio.py

Результат: foundry-module/tokenatra-connector/studio/
  index.html        — UI без Jinja (версия/ссылки захардкожены)
  js/, css/         — фронтенд приложения
  assets/           — mask, logo, example, кольца, пресеты, дефолтное кольцо
  manifest.json     — rings_list / presets_list для перехватчика fetch

Вызывать ПОСЛЕ изменений фронтенда: студия в модуле — статическая копия.
build_module.bat и CI делают это автоматически.
"""
import json
import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MODULE = ROOT / 'foundry-module' / 'tokenatra-connector'
STUDIO = MODULE / 'studio'

# Кольца, включаемые в hosted-студию модуля Foundry (первое — по умолчанию)
STUDIO_RING_ORDER = [
    'rune-steel.webp',
    'high-noon.webp',
    'eldritch.webp',
    'circus.webp',
    'cosmic.webp',
    'ag-obsidian-red.webp',
]
STUDIO_RING_INDEX = {name: idx for idx, name in enumerate(STUDIO_RING_ORDER)}

# Имена встроенных колец — синхронно с consts.rs:builtin_ring_name
BUILTIN_RING_NAMES = {
    'ag-obsidian-red.webp': 'Кровавый обсидиан',
    'ag-wood-runic.webp': 'Рунное дерево',
    'archer.webp': 'Стрелок',
    'armor_2x.webp': 'Доспех',
    'chronopunk.webp': 'Хронопанк',
    'circus.webp': 'Цирк',
    'cosmic.webp': 'Космос',
    'cristmas_2x.webp': 'Рождество',
    'druid.webp': 'Друид',
    'eldritch.webp': 'Бездна',
    'feywild.webp': 'Фейвайльд',
    'gold-white-rune.webp': 'Золотые руны',
    'high-noon.webp': 'Ковбой',
    'kraken.webp': 'Морская пучина',
    'lava.webp': 'Лава',
    'plague.webp': 'Чума',
    'rogue.webp': 'Теневое ремесло',
    'rune-gold.webp': 'Руны: золото',
    'rune-green.webp': 'Руны: зелёные',
    'rune-ice.webp': 'Руны: лёд',
    'rune-purple.webp': 'Руны: фиолетовые',
    'rune-red.webp': 'Руны: красные',
    'rune-steel.webp': 'Руны: сталь',
    'stained-glass.webp': 'Витраж',
    'tentacle.webp': 'Щупальца',
    'theathre.webp': 'Театр',
}
MIME = {'.webp': 'image/webp', '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg'}
THUMB_SIZE = 144  # сайдбар колец: ~10 КБ webp вместо 0.3–4 МБ оригинала


def _ring_like(f: Path) -> bool:
    """Фильтр файлов колец для hosted-студии модуля Foundry."""
    stem_cf = f.stem.casefold()
    return (f.is_file() and f.suffix.lower() in MIME
            and f.name.casefold() in STUDIO_RING_INDEX
            and not stem_cf.endswith('.mask')
            and not stem_cf.endswith('.top'))


def _find_companion(f: Path, suffix: str):
    for ext in MIME:
        cand = f.with_name(f'{f.stem}.{suffix}{ext}')
        if cand.is_file():
            return cand
    return None


def _read_meta(f: Path) -> dict:
    mp = f.with_name(f'{f.stem}.meta.json')
    if mp.is_file():
        try:
            return json.loads(mp.read_text(encoding='utf-8'))
        except Exception:
            pass
    return {}


def make_ring_thumbs(ring_files) -> None:
    """Миниатюры колец для hosted-студии (assets/rings/thumbs/<stem>.webp):
    сайдбар игрока грузит их с сервера мира, полные файлы нужны только
    выбранному кольцу. Нет Pillow — manifest просто остаётся без thumb."""
    try:
        from PIL import Image
    except Exception as e:
        print(f'  WARN: миниатюры колец не сгенерированы (нет Pillow): {e}')
        return
    out = STUDIO / 'assets' / 'rings' / 'thumbs'
    out.mkdir(parents=True, exist_ok=True)
    for f in ring_files:
        try:
            img = Image.open(f).convert('RGBA')
            top = _find_companion(f, 'top')
            if top is not None:
                top_im = Image.open(top).convert('RGBA')
                if top_im.size != img.size:
                    top_im = top_im.resize(img.size, Image.LANCZOS)
                img = Image.alpha_composite(img, top_im)
            img.thumbnail((THUMB_SIZE, THUMB_SIZE), Image.LANCZOS)
            img.save(out / (f.stem + '.webp'), 'WEBP', quality=80)
        except Exception as e:
            print(f'  WARN: миниатюра {f.name}: {e}')


def build_index(version: str) -> None:
    html = (ROOT / 'templates' / 'index.html').read_text(encoding='utf-8')
    html = re.sub(r"\{\{ url_for\('static', filename='([^']+)', v=__version__\) \}\}",
                  r'./\1?v=' + version, html)
    html = html.replace('{{ github_repo }}', 'SweetyHake/Tokenatra')
    html = html.replace('{{ __version__ }}', version)
    html = html.replace('{{ embed_allowed|tojson }}', '[]')
    html = html.replace('href="/icon"', 'href="./assets/logo.png"')
    html = html.replace('src="/icon"', 'src="./assets/logo.png"')
    html = html.replace('src="/logo"', 'src="./assets/logo.png"')
    # Внешние шрифты Google в Foundry могут резаться CSP — системный fallback
    html = '\n'.join(line for line in html.split('\n')
                     if 'fonts.googleapis' not in line and 'fonts.gstatic' not in line)
    # tokenTransport.js — первым, до config.js (перехват fetch до старта приложения)
    i = html.find('<script src="./js/config.js?v=')
    assert i != -1, 'не найден тег config.js'
    html = html[:i] + '<script src="./js/tokenTransport.js?v=' + version + '"></script>\n    ' + html[i:]
    # srcdoc-iframe не имеет location.search — флаги запекаем в HTML,
    # embed-класс ставим безусловно; <base> держит относительные пути
    baked = ('<script>window.__STUDIO_VERSION__ = ' + json.dumps(version) + ';'
             'window.__STUDIO_BUILT__ = true;'
             "window.__EMBED__ = { embed: true, parentOrigin: '' };"
             "document.documentElement.classList.add('embed');</script>\n"
             '    <base href="/modules/tokenatra-connector/studio/">')
    i = html.find('// Embed-режим')
    assert i != -1, 'не найден embed-скрипт шаблона'
    start = html.rfind('<script>', 0, i)
    end = html.find('</script>', i) + len('</script>')
    assert start != -1 and end > start, 'границы embed-скрипта'
    html = html[:start] + baked + html[end:]
    assert '__STUDIO_BUILT__' in html and '<base href' in html
    assert '{{' not in html, 'не все Jinja-вставки заменены'
    (STUDIO / 'index.html').write_text(html, encoding='utf-8')


def build_manifest() -> dict:
    rings_dir = ROOT / 'token_rings'
    presets_dir = ROOT / 'presets'
    rings = []
    for f in sorted(rings_dir.iterdir()):
        if not _ring_like(f):
            continue
        meta = _read_meta(f)
        mask_p = _find_companion(f, 'mask')
        mask_file = mask_p.name if mask_p else None
        top_file = meta.get('top_file') if meta.get('top_file') and (rings_dir / meta['top_file']).is_file() else None
        if not top_file:
            top_p = _find_companion(f, 'top')
            top_file = top_p.name if top_p else None
        scale = float(meta.get('scale', 1.0)) if isinstance(meta.get('scale'), (int, float)) else 1.0
        entry = {
            'name': BUILTIN_RING_NAMES.get(f.name.casefold(), meta.get('name') or f.stem),
            'name_en': meta.get('name_en'),
            'file': f.name,
            'mime': MIME[f.suffix.lower()],
            'builtin': True,
            'mask_file': mask_file,
            'top_file': top_file,
            'scale': scale,
            'added_at': 0,
        }
        thumb = STUDIO / 'assets' / 'rings' / 'thumbs' / (f.stem + '.webp')
        if thumb.is_file():
            entry['thumb'] = 'thumbs/' + thumb.name
        rings.append(entry)
    rings.sort(key=lambda r: STUDIO_RING_INDEX.get(r['file'].casefold(), 999))
    presets = [{'name': f.stem, 'file': f.name, 'builtin': True}
               for f in sorted(presets_dir.iterdir())
               if f.is_file() and f.suffix.lower() in MIME]
    manifest = {'rings': rings, 'presets': presets}
    (STUDIO / 'manifest.json').write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding='utf-8')
    return manifest


def main() -> None:
    module_version = json.loads((MODULE / 'module.json').read_text(encoding='utf-8'))['version']

    if STUDIO.exists():
        shutil.rmtree(STUDIO)
    (STUDIO / 'js').mkdir(parents=True)
    (STUDIO / 'css').mkdir(parents=True)
    (STUDIO / 'assets' / 'rings').mkdir(parents=True)
    (STUDIO / 'assets' / 'presets').mkdir(parents=True)

    build_index(module_version)

    # Фронтенд приложения целиком (tokenTransport.js активен только с ?hosted=1)
    for js in (ROOT / 'static' / 'js').glob('*.js'):
        shutil.copy2(js, STUDIO / 'js' / js.name)
    shutil.copy2(ROOT / 'static' / 'css' / 'style.css', STUDIO / 'css' / 'style.css')

    # Ассеты
    shutil.copy2(ROOT / 'mask.png', STUDIO / 'assets' / 'mask.png')
    shutil.copy2(ROOT / 'logo.png', STUDIO / 'assets' / 'logo.png')
    example = ROOT / 'example.png'
    if example.is_file():
        shutil.copy2(example, STUDIO / 'assets' / 'example.png')
    ring_files = [f for f in (ROOT / 'token_rings').iterdir() if _ring_like(f)]
    for f in ring_files:
        shutil.copy2(f, STUDIO / 'assets' / 'rings' / f.name)
        for suffix in ('mask', 'top'):
            comp = _find_companion(f, suffix)
            if comp is not None:
                shutil.copy2(comp, STUDIO / 'assets' / 'rings' / comp.name)
        mp = f.with_name(f'{f.stem}.meta.json')
        if mp.is_file():
            shutil.copy2(mp, STUDIO / 'assets' / 'rings' / mp.name)
    make_ring_thumbs(ring_files)
    for f in (ROOT / 'presets').iterdir():
        if f.is_file() and f.suffix.lower() in MIME:
            shutil.copy2(f, STUDIO / 'assets' / 'presets' / f.name)

    # Дефолтное кольцо (сервер рисует его через PIL при /ring)
    try:
        from PIL import Image, ImageDraw
        img = Image.new('RGBA', (1024, 1024), (0, 0, 0, 0))
        draw = ImageDraw.Draw(img)
        draw.ellipse([20, 20, 1004, 1004], outline=(100, 100, 100, 255), width=40)
        img.save(STUDIO / 'assets' / 'ring-default.png')
    except Exception as e:
        print(f'  WARN: дефолтное кольцо не сгенерировано: {e}')

    manifest = build_manifest()
    total = sum(1 for _ in STUDIO.rglob('*') if _.is_file())
    print(f'studio: {total} файлов | колец: {len(manifest["rings"])} | '
          f'пресетов: {len(manifest["presets"])} | версия: {module_version}')


if __name__ == '__main__':
    sys.exit(main())
