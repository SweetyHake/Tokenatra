#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""tokenatra_mcp.py — MCP stdio-сервер для Tokenatra (в стиле Blender MCP).

Архитектура:
- Запускается MCP-клиентом (OpenCode, Claude Desktop, Cursor) как stdio-процесс.
- Редактор и ONNX-инференс живут в Rust-сервере `tokenatra.exe` на 127.0.0.1:7878.
- Если `tokenatra.exe` не запущен, прокси автоматически находит и поднимает
  `tokenatra.exe --server` в фоне при первом вызове!
- Принимает локальные пути (`image_path`, `output_path`) напрямую, чтобы агенту
  не приходилось кодировать мегабайты base64 в JS-песочнице Code Mode.
- При инициализации (`initialize`) отдаёт `instructions` (как Blender MCP),
  чтобы агент с первого шага знал инструменты, якоря и человеческий пайплайн.
- НОЛЬ сторонних зависимостей (чистый Python stdlib, собирается в TokenatraMCP.exe).
"""
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

SERVER_URL = os.environ.get('TOKENATRA_URL', 'http://127.0.0.1:7878').rstrip('/')
MARKER = {'X-Tokenatra-Agent': '1'}
SUPPORTED_VERSIONS = ('2025-06-18', '2025-03-26', '2024-11-05')
LATEST_VERSION = SUPPORTED_VERSIONS[0]
HTTP_TIMEOUT = 180

_INSTRUCTIONS = """\
Use tools from this server through `execute` under `tools["tokenatra"]`.
MCP for Tokenatra drives the Tokenatra token & portrait studio (Rust + ONNX Runtime).
If Tokenatra is not running yet, the first tool call starts `tokenatra.exe --server` automatically.

How a human creates a token in Tokenatra (follow the `example.png` style guide):
1. Open the image: call `tokenatra_create_session({ image_path: "C:/path/to/art.webp" })`.
   It returns `session_id` and `source_info` (`width`, `height`, `opaque_pct`, `already_transparent`, `fit1024_bbox`).
   The default ring `"steel"` and ring protection mask are already active, just like in the GUI.
2. Remove background ONLY if needed: check `already_transparent` (or `opaque_pct < 92`).
   - If the background is opaque (`already_transparent: false`), call `tokenatra_remove_background({ session_id })`.
   - NEVER call `tokenatra_remove_background` on an already transparent cutout/subject — re-matting eats fine edges and glow.
3. Frame the character (`tokenatra_set_transform`) using the `example.png` overlay (`tokenatra_preview({ session_id, example: true })`):
   In the GUI, the human turns on the "Пример" (`example.png`) overlay (at `2x` scale) and aligns the character to its three purple markers:
   - **Область головы (Макушка .. Подбородок)**: the character's head (without tall hats or horns) must fit inside the purple oval!
     - **Макушка (Crown of head)**: `y ~ -130` in 1x ring coords (`y ~ 436` in 2048 2x frame) — the top of the head pops out ABOVE the top rim of the ring (`y = 0`).
     - **Подбородок (Chin / Jawline)**: `y ~ 410` in 1x ring coords (`y ~ 711` in 2048 2x frame), with eyes around `y ~ 150-200` and face centered on `x ~ 512`.
   - **Талия / Пояс (Waist / Belt line)**: `y ~ 715` in 1x ring coords (`y ~ 1230` in 2048 2x frame), with the bottom of the torso/skirt reaching the lower inner rim of the ring (`y ~ 865`).
4. Mask the bottom / sides (`tokenatra_apply_preset` or `tokenatra_edit_mask`):
   - Call `tokenatra_apply_preset({ session_id, preset: "preset1" })` (or `"preset2"` for wide shields/wings/weapons).
     Because `canvas_mask` covers the full `3x` canvas (`-1024..2048`), `preset1` keeps the head/hat/weapons popping out above the ring (`y < 0`) while cleanly erasing everything outside the bottom/lower sides of the ring!
   - Or use `tokenatra_edit_mask` (`target: "canvas"` = Pink Mask fixed to ring; `target: "image"` = Blue Eraser attached to character).
5. Look at your work with the Example overlay (`tokenatra_preview({ session_id, example: true })`):
   Pass `example: true` to composite `example.png` ("Макушка", "Область головы", "Подбородок", "Талия / Пояс") directly over the preview at `2x` scale and return that image to you! Adjust `scale`, `offset_x`, `offset_y` until the head sits right inside the purple oval and the waist hits "Талия / Пояс".
6. Export (`tokenatra_export`):
   - Use `scale_mode: "auto"` (or `"2"` / `"1"`): if the head/hat/weapons pop out beyond the 1x ring (`y < 0`), `"auto"` exports a `2x` (or `3x`) canvas so the pop-out head/hat/shield is NEVER clipped at the border!
   - If you want a `subject.webp` (cutout without ring, like `Gambit/subject.webp`), enable drop shadow (`tokenatra_set_effects`) and export with `with_ring: false, scale_mode: "2", size: 1024` (which produces a `2048x2048` subject).
"""

_TOOLS = [
    {
        'name': 'tokenatra_manifest',
        'description': (
            'Get Tokenatra studio status and manifest: app version, GPU/CPU device, '
            'ONNX model status, limits, defaults, and available operations. '
            'Auto-starts the Tokenatra server if it is not running.'),
        'inputSchema': {'type': 'object', 'properties': {}},
    },
    {
        'name': 'tokenatra_recipe',
        'description': (
            'Standard human-like workflows & anchor math: '
            '(1) Creating a Token with Ring (1x steel ring + pop-out head + bottom protection/arc), '
            '(2) Creating a Subject cutout (square transparent bust with bottom arc), '
            '(3) Using Pink Canvas Mask vs Blue Image Eraser and built-in presets.'),
        'inputSchema': {'type': 'object', 'properties': {}},
    },
    {
        'name': 'tokenatra_list_assets',
        'description': (
            'List available token rings (steel, white-runic, ag-stormsteel, etc.) '
            'and mask presets (preset1, preset2, preset3, plus user presets).'),
        'inputSchema': {'type': 'object', 'properties': {}},
    },
    {
        'name': 'tokenatra_create_session',
        'description': (
            'Create an editing session from an image file path OR base64 string. '
            'You can pass a local file path (e.g. "W:/Images/art.webp") in `image_path` '
            'OR directly in `image_b64`! Returns session_id, source dimensions, '
            'opaque_pct, already_transparent flag, and fit1024_bbox.'),
        'inputSchema': {
            'type': 'object',
            'properties': {
                'image_path': {
                    'type': 'string',
                    'description': 'Local path to the source image (PNG/WebP/JPG/AVIF). Preferred over base64.',
                },
                'image_b64': {
                    'type': 'string',
                    'description': 'Either a local file path (e.g. "C:/.../img.webp") OR base64 / data:image/...;base64',
                },
                'ring': {
                    'type': 'string',
                    'description': 'Initial ring name (default "steel", pass "" or "none" for no ring)',
                },
            },
        },
    },
    {
        'name': 'tokenatra_remove_background',
        'description': (
            'Remove background with the ONNX model (RMBG-2.0 / BEN2 / BiRefNet), '
            'or toggle between original and cutout (`showing_original: true/false`). '
            'Check `already_transparent` from create_session/inspect first — do NOT '
            'call on already-transparent images.'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'edge_blur': {'type': 'number', 'description': '0..10, default 1'},
                'showing_original': {
                    'type': 'boolean',
                    'description': 'If background was already removed, toggle showing original vs cutout without re-running ONNX',
                },
            },
        },
    },
    {
        'name': 'tokenatra_edit_mask',
        'description': (
            'Edit visibility masks in 1024x1024 logical coords. '
            'Supports both GUI mask tools via `target`: '
            '"canvas" (Pink Mask, fixed in 1x token frame, default) or '
            '"image" (Blue Eraser, in source image coords, moves/scales with subject) or "both" (for reset). '
            'ops: erase, restore, apply_preset, invert, feather, grow, reset. '
            'shapes_json: [{"circle":{"cx":512,"cy":470,"r":395,"feather":8}}, '
            '{"rect":{"x0":0,"y0":470,"x1":1024,"y1":1024}}, '
            '{"stroke":{"points":[[200,800],[512,880],[824,800]],"width":60,"soft":true}}]. '
            'When `protect=true` (default), `erase` respects the ring inner circle protection mask so anything inside the ring is safe! Set `protect=false` to cut inside the ring.'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id', 'op'],
            'properties': {
                'session_id': {'type': 'string'},
                'target': {
                    'type': 'string',
                    'enum': ['canvas', 'image', 'both'],
                    'description': 'Mask layer: "canvas" (pink mask on token frame, default) or "image" (blue eraser on subject)',
                },
                'op': {
                    'type': 'string',
                    'enum': ['erase', 'restore', 'apply_preset', 'invert', 'feather', 'grow', 'reset'],
                },
                'shapes_json': {
                    'type': 'string',
                    'description': 'JSON array of shapes (circle / rect / stroke) in 0..1024 coords',
                },
                'preset': {'type': 'string', 'description': 'Preset name for apply_preset (e.g. preset1, preset2, preset3)'},
                'px': {'type': 'number', 'description': 'Radius in px for feather / grow (e.g. 8)'},
                'protect': {
                    'type': 'boolean',
                    'description': 'Respect ring protection mask during erase (default true; pass false for arc cuts inside the ring)',
                },
            },
        },
    },
    {
        'name': 'tokenatra_apply_preset',
        'description': (
            'Apply a named mask preset (e.g. "preset1" = top pop-out, '
            '"preset2" = wide pop-out, "preset3" = narrow column pop-out) to the Pink Canvas Mask.'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id', 'preset'],
            'properties': {
                'session_id': {'type': 'string'},
                'preset': {'type': 'string'},
            },
        },
    },
    {
        'name': 'tokenatra_set_transform',
        'description': (
            'Set subject transform in the 1024x1024 1x token frame: '
            'L(f) = 512 + (f - 512) * scale + offset. '
            '`scale = 1.0` fits the longest side of the source image into 1024 px (aspect ratio is preserved). '
            '`offset_x`, `offset_y` are in 1024 logical px from center (no 3x multiplier!). '
            '`rotation` is in degrees clockwise (-180..180).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'scale': {'type': 'number'},
                'rotation': {'type': 'number'},
                'offset_x': {'type': 'number'},
                'offset_y': {'type': 'number'},
            },
        },
    },
    {
        'name': 'tokenatra_set_ring',
        'description': 'Set token ring by name (e.g. "steel", "white-runic", "ag-stormsteel"), or remove it (name=null or "none").',
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'name': {'type': 'string', 'description': 'Ring name, or null / "none" to remove'},
            },
        },
    },
    {
        'name': 'tokenatra_set_effects',
        'description': (
            'Set drop shadow and color correction (just like the GUI accordions). '
            'shadow_json: {"enabled":true,"angle":135,"distance":7,"blur":3,"opacity":0.4}; '
            'color_json: {"enabled":true,"saturation":5,"lightness":-5,"contrast":0} (percent).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'shadow_json': {'type': 'string'},
                'color_json': {'type': 'string'},
            },
        },
    },
    {
        'name': 'tokenatra_history',
        'description': 'Undo or redo the last mask/transform/remove_bg operation in the session (up to 30 steps, just like Ctrl+Z / Ctrl+Y in the GUI).',
        'inputSchema': {
            'type': 'object',
            'required': ['session_id', 'action'],
            'properties': {
                'session_id': {'type': 'string'},
                'action': {'type': 'string', 'enum': ['undo', 'redo']},
            },
        },
    },
    {
        'name': 'tokenatra_portrait',
        'description': (
            'Use the Portrait Generator (square avatar crop, with or without background) '
            'and save it to disk. Mirrors the "Портрет" panel in the GUI.'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'variant': {'type': 'string', 'enum': ['original', 'cutout']},
                'scale': {'type': 'number'},
                'rotation': {'type': 'number'},
                'offset_x': {'type': 'number'},
                'offset_y': {'type': 'number'},
                'size': {'type': 'number', 'description': '512, 1024, 2048, or 4096 (default 1024)'},
                'format': {'type': 'string', 'enum': ['webp', 'png', 'jpg']},
                'quality': {'type': 'number', 'description': '10..100, default 95'},
                'save_to': {'type': 'string', 'description': 'Target file path (e.g. "W:/.../portrait.webp") or folder'},
            },
        },
    },
    {
        'name': 'tokenatra_inspect',
        'description': (
            'Inspect current state: `source_info` (width, height, opaque_pct, already_transparent, fit1024_bbox), '
            'composed 1x `bbox`, `coverage_pct`, 3x3 grid, `warnings`, and history status. '
            '`include_image=true` also returns a visual preview (with optional `example=true` overlay or `guides=true`).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'include_image': {'type': 'boolean'},
                'preview_size': {'type': 'number', 'description': '64..1024, default 512'},
                'example': {'type': 'boolean', 'description': 'Overlay example.png style template ("Макушка", "Область головы", "Челюсть", "Пояс")'},
                'example_opacity': {'type': 'number', 'description': '0.1..1.0, default 0.65'},
                'guides': {'type': 'boolean', 'description': 'Draw alignment guides'},
            },
        },
    },
    {
        'name': 'tokenatra_preview',
        'description': (
            'Look at the current token (like `look` in Blender MCP). '
            'Pass `example=true` to overlay the `example.png` style guide ("Макушка", "Область головы", "Челюсть", "Пояс") '
            'on a 2x canvas so you can visually verify that the head fits inside the purple oval and the waist aligns with "Пояс"!'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'size': {'type': 'number', 'description': '64..1024, default 512'},
                'scale_mode': {'type': 'string', 'enum': ['auto', '1', '2', '3']},
                'with_ring': {'type': 'boolean', 'description': 'default true'},
                'example': {'type': 'boolean', 'description': 'Overlay example.png style template ("Макушка", "Область головы", "Челюсть", "Пояс")'},
                'example_scale': {'type': 'number', 'description': '1, 2, or 3 (default 2, matching the GUI)'},
                'example_opacity': {'type': 'number', 'description': '0.1..1.0, default 0.65'},
                'guides': {'type': 'boolean', 'description': 'Draw alignment guides on the preview image'},
            },
        },
    },
    {
        'name': 'tokenatra_export',
        'description': (
            'Export the final token image (with or without ring). '
            '`save_to` accepts EITHER an existing folder path OR a full file path '
            '(e.g. "W:/Images/STokens/Brul/token.png"). Returns the saved path and attaches the image preview. '
            'Default `scale_mode` is "1" (full-canvas ring).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'format': {'type': 'string', 'enum': ['webp', 'png', 'jpg']},
                'quality': {'type': 'number', 'description': '10..100, default 95'},
                'size': {'type': 'number', 'description': '256..2048, default 1024'},
                'scale_mode': {'type': 'string', 'enum': ['auto', '1', '2', '3']},
                'with_ring': {'type': 'boolean', 'description': 'true = Save with Ring, false = Save without Ring (subject)'},
                'save_to': {
                    'type': 'string',
                    'description': 'Destination file path (e.g. "C:/.../token.png") or existing directory path',
                },
            },
        },
    },
    {
        'name': 'tokenatra_close_session',
        'description': 'Close a session and free its memory.',
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {'session_id': {'type': 'string'}},
        },
    },
]

_RESOURCES = [
    {'uri': 'tokenatra://manifest', 'name': 'Tokenatra manifest',
     'mimeType': 'application/json', 'description': 'Limits, defaults, and model status'},
    {'uri': 'tokenatra://assets', 'name': 'Tokenatra assets',
     'mimeType': 'application/json', 'description': 'Available token rings and mask presets'},
    {'uri': 'tokenatra://recipe', 'name': 'Tokenatra workflow recipe',
     'mimeType': 'text/plain', 'description': 'Human-parity workflow, anchors, and transform math'},
]


def log(msg, *args):
    print('[mcp]', msg % args if args else msg, file=sys.stderr, flush=True)


class AppDown(Exception):
    pass


class ToolError(Exception):
    """Ошибка тулзы: показывается модели как isError-результат."""


def _find_tokenatra_exe():
    """Ищет бинарник tokenatra.exe рядом с прокси, в репозитории или в Program Files."""
    here = Path(__file__).resolve().parent
    exe_dir = Path(sys.executable).resolve().parent
    candidates = [
        os.environ.get('TOKENATRA_EXE', ''),
        here / 'rust' / 'target' / 'release' / 'tokenatra.exe',
        here.parent / 'rust' / 'target' / 'release' / 'tokenatra.exe',
        exe_dir / 'tokenatra.exe',
        exe_dir.parent / 'tokenatra.exe',
        exe_dir.parent.parent / 'rust' / 'target' / 'release' / 'tokenatra.exe',
        Path(r'Z:\Tokenatra\rust\target\release\tokenatra.exe'),
        Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Tokenatra' / 'tokenatra.exe',
        Path(os.environ.get('ProgramFiles', r'C:\Program Files')) / 'Tokenatra' / 'tokenatra.exe',
        Path(os.environ.get('LOCALAPPDATA', '')) / 'Programs' / 'Tokenatra' / 'tokenatra.exe',
    ]
    for c in candidates:
        if not c:
            continue
        p = Path(c)
        if p.is_file():
            return p
    return None


def _ensure_server_running():
    """Автоматически запускает `tokenatra.exe --server`, если сервер на 7878 ещё не поднят."""
    exe = _find_tokenatra_exe()
    if not exe:
        return False
    log('Auto-starting Tokenatra server: %s --server', exe)
    creationflags = 0
    if sys.platform == 'win32':
        creationflags = (
            getattr(subprocess, 'CREATE_NO_WINDOW', 0x08000000)
            | getattr(subprocess, 'DETACHED_PROCESS', 0x00000008)
        )
    try:
        subprocess.Popen(
            [str(exe), '--server'],
            cwd=str(exe.parent),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=creationflags,
        )
    except Exception as e:
        log('Failed to spawn tokenatra.exe: %r', e)
        return False

    deadline = time.time() + 10.0
    ping_url = SERVER_URL + '/agent/v1/ping'
    while time.time() < deadline:
        try:
            req = urllib.request.Request(ping_url, headers=dict(MARKER), method='GET')
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                if resp.status == 200:
                    log('Tokenatra server is up at %s', SERVER_URL)
                    return True
        except Exception:
            time.sleep(0.15)
    return False


def http_call(method, path, body=None, raw=False):
    url = SERVER_URL + path
    data = json.dumps(body).encode('utf-8') if body is not None else None
    headers = dict(MARKER)
    if data is not None:
        headers['Content-Type'] = 'application/json'

    for attempt in range(2):
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as resp:
                payload = resp.read()
                ctype = resp.headers.get('Content-Type', '')
            if raw or not ctype.startswith('application/json'):
                return payload
            return json.loads(payload.decode('utf-8'))
        except urllib.error.HTTPError as e:
            err_body = ''
            try:
                err_body = e.read().decode('utf-8', 'replace')
                err = json.loads(err_body)
            except Exception:
                err = {'error': err_body or str(e), 'http': e.code}
            raise ToolError(json.dumps(err, ensure_ascii=False))
        except (urllib.error.URLError, OSError) as e:
            if attempt == 0 and _ensure_server_running():
                continue
            raise AppDown(
                f'Tokenatra is not running at {SERVER_URL} ({e.reason if hasattr(e, "reason") else e}) '
                'and could not be auto-started. Launch Tokenatra or `tokenatra.exe --server`.')


_RECIPE_TEXT = """\
TOKENATRA STUDIO STYLE GUIDE (based on `example.png` at 2x scale)

1. THE `example.png` VISUAL TEMPLATE (HOW HUMANS ALIGN TOKENS IN TOKENATRA):
   - In Tokenatra, the working canvas is 3x the ring size, and the "Пример" (`example.png`)
     overlay is drawn at `2x` scale (`2048x2048` when `size=1024`), where the 1x ring occupies
     the central `1024x1024` box (`[512..1536]` in 2048 coords, or `[0..1024]` in 1x logical coords).
   - Call `tokenatra_preview({ session_id, example: true })` (or `tokenatra_inspect` with `include_image: true, example: true`)
     to see your character with the purple `example.png` overlay directly on top!
   - The three purple markers on `example.png` are:
     1) **Область головы (Макушка .. Подбородок)**:
        - The purple oval in the top center (`cx = 524, cy = 134, rx = 362, ry = 274` in 1x logical coords;
          or `y = 436..711, x = 866..1228` in the `2048x2048` 2x frame).
        - **Макушка (Crown of head / hair top, excluding tall hats/horns)**: must touch the top purple line
          (`y ~ -130` in 1x coords, which is ABOVE the upper ring border `y = 0`!).
        - **Подбородок (Chin / Jawline)**: must touch the middle purple line (`y ~ 410` in 1x coords;
          eyes sit around `y ~ 150-200`, face centered at `x ~ 512`).
        - The entire head (hair + face) fits inside the purple oval!
     2) **Талия / Пояс (Waist / Belt line)**:
        - Must align with the bottom purple line (`y ~ 715` in 1x coords, or `y ~ 1230` in 2048 2x coords).
        - The lower torso / hips continue down to the inner bottom rim of the ring (`y ~ 865`), where they are cut by a smooth arc.

2. COORDINATE SYSTEM & TRANSFORM MATH:
   - Logical 1x ring frame is `1024x1024` (`(0,0)` = top-left of the 1x ring, `(512,512)` = ring center).
   - Coordinates outside `[0..1024]` (like `y = -130` for the crown of the head) are in the 2x/3x pop-out zone above/around the ring!
   - Non-square images preserve their aspect ratio automatically (`fit1024` fits the longest side into 1024 px, centered).
   - Transform formula (from source `fit1024` coordinate `f` to 1x logical coordinate `L`):
       L(fx) = 512 + (fx - 512) * scale + offset_x
       L(fy) = 512 + (fy - 512) * scale + offset_y
   - Solving `scale`, `offset_x`, `offset_y` from Crown `(fx_crown, fy_crown)` -> `y = -130` and Chin `(fx_chin, fy_chin)` -> `y = 410`:
       scale    = 540 / (fy_chin - fy_crown)     # 540 = 410 - (-130)
       offset_x = -(fx_face - 512) * scale
       offset_y = 410 - 512 - (fy_chin - 512) * scale

3. MASKING (PRESETS & PINK CANVAS MASK vs BLUE IMAGE ERASER):
   - The Pink Canvas Mask (`target: "canvas"`) spans the full `3x` canvas (`-1024..2048` in 1x coords), so pop-out heads, hats, weapons, and shields outside `0..1024` are preserved!
   - **Built-in Presets (`tokenatra_apply_preset`)**:
     - `"preset1"`: standard pop-out mask — keeps the entire inner ring circle + the upper pop-out zone for the head/hat/hair, and erases everything outside the bottom and lower sides of the ring.
     - `"preset2"`: wide pop-out mask — keeps upper-left and upper-right pop-outs (great for shields, wings, raised weapons) while cutting the bottom below the ring rim.
     - `"preset3"`: narrow vertical column pop-out.
   - **Manual Bottom Arc (for `subject.webp` cutouts without a ring, or full-body art)**:
     Call `tokenatra_edit_mask` (`target: "canvas"`, `protect: false`):
       1) `op: "erase"`,   `shapes_json: '[{"rect":{"x0":-512,"y0":270,"x1":1536,"y1":1536}}]'`
       2) `op: "restore"`, `shapes_json: '[{"circle":{"cx":512,"cy":270,"r":595}}]'`  (arc bottom at `y = 865`, matching the inner ring rim)
       3) `op: "feather"`, `px: 8`

4. STEP-BY-STEP WORKFLOW:
   1) `tokenatra_create_session({ image_path: "..." })` -> check `already_transparent`.
   2) If `already_transparent == false`, call `tokenatra_remove_background({ session_id })`.
   3) Call `tokenatra_set_transform` and `tokenatra_preview({ session_id, example: true })`.
   4) Look at the returned image with the purple `example.png` overlay:
      - Is the head inside the purple oval ("Область головы", between "Макушка" and "Челюсть")?
      - Is the waist on the "Пояс" line?
      Adjust `scale`, `offset_x`, `offset_y` (1-2 iterations) until the head fits the oval and the waist sits on "Пояс".
   5) Apply mask (`tokenatra_apply_preset` with `"preset1"` or `"preset2"`, or custom `tokenatra_edit_mask`).
   6) Export (`tokenatra_export`):
      - For a **Token with Ring** where the head/hat/weapons pop out above the ring: use `with_ring: true, scale_mode: "auto"` (or `"2"`), `size: 1024`.
      - For a **Subject cutout** (`subject.webp` in `STokens`, used by Foundry dynamic ring or series tokens): enable drop shadow (`{"enabled":true,"angle":135,"distance":7,"blur":3,"opacity":0.4}`) and export with `with_ring: false, scale_mode: "2", size: 1024, format: "webp"`.
"""


def _shapes_from(args):
    if 'shapes' in args and isinstance(args['shapes'], list):
        return args['shapes']
    raw = args.get('shapes_json')
    if not raw:
        return None
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise ToolError(f'shapes_json is not valid JSON: {e}')
    if not isinstance(parsed, list):
        raise ToolError('shapes_json must be a JSON array of shapes')
    return parsed


def _block_from(args, key):
    raw = args.get(key)
    if not raw:
        return None
    if isinstance(raw, dict):
        return raw
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise ToolError(f'{key} is not valid JSON: {e}')
    if not isinstance(parsed, dict):
        raise ToolError(f'{key} must be a JSON object')
    return parsed


def _looks_like_file_path(s):
    if not isinstance(s, str):
        return False
    s = s.strip()
    if not s or s.startswith('data:image/') or len(s) > 1024 or '\n' in s:
        return False
    # Windows drive letter, UNC path, or relative path with image extension
    if (len(s) >= 3 and s[1] == ':' and s[2] in ('/', '\\')) or s.startswith(('\\\\', '/', './', '../')):
        return True
    lower = s.lower()
    if lower.endswith(('.png', '.webp', '.jpg', '.jpeg', '.avif', '.bmp', '.gif')):
        return True
    return Path(s).is_file()


def call_tool(name, args):
    sid = args.get('session_id')

    if name == 'tokenatra_manifest':
        return {'text': json.dumps(http_call('GET', '/agent/v1/manifest'),
                                   ensure_ascii=False, indent=2)}
    if name == 'tokenatra_recipe':
        return {'text': _RECIPE_TEXT}
    if name == 'tokenatra_list_assets':
        return {'text': json.dumps(http_call('GET', '/agent/v1/assets'),
                                   ensure_ascii=False, indent=2)}
    if name == 'tokenatra_create_session':
        body = {}
        img_path = args.get('image_path') or args.get('path')
        img_b64 = args.get('image_b64') or args.get('image')
        if img_path:
            p = Path(img_path)
            if not p.is_file():
                raise ToolError(f'image_path does not exist: {img_path}')
            body['image_path'] = str(p.resolve())
        elif img_b64 and _looks_like_file_path(img_b64):
            p = Path(img_b64.strip())
            if not p.is_file():
                raise ToolError(f'file does not exist: {img_b64}')
            body['image_path'] = str(p.resolve())
        elif img_b64:
            body['image'] = img_b64
        else:
            raise ToolError('provide image_path (local file path) or image_b64')

        if 'ring' in args:
            r = args.get('ring')
            body['ring'] = None if r in (None, '', 'none', 'null') else r

        res = http_call('POST', '/agent/v1/sessions', body)
        return {'text': json.dumps(res, ensure_ascii=False, indent=2)}

    if name == 'tokenatra_remove_background':
        body = {'edge_blur': args.get('edge_blur', 1.0)}
        if 'showing_original' in args:
            body['showing_original'] = bool(args['showing_original'])
        res = http_call('POST', f'/agent/v1/sessions/{sid}/remove_bg', body)
        return {'text': json.dumps(res, ensure_ascii=False, indent=2)}

    if name == 'tokenatra_edit_mask':
        body = {
            'op': args['op'],
            'target': args.get('target', 'canvas'),
            'shapes': _shapes_from(args),
            'preset': args.get('preset'),
            'px': args.get('px', 0),
            'protect': args.get('protect', True),
        }
        body = {k: v for k, v in body.items() if v is not None}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/mask', body)
        return {'text': json.dumps(res, ensure_ascii=False)}

    if name == 'tokenatra_apply_preset':
        res = http_call('POST', f'/agent/v1/sessions/{sid}/mask',
                        {'op': 'apply_preset', 'preset': args['preset']})
        return {'text': json.dumps(res, ensure_ascii=False)}

    if name == 'tokenatra_set_transform':
        body = {k: args[k] for k in ('scale', 'rotation', 'offset_x', 'offset_y', 'fit_zone')
                if k in args and args[k] is not None}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/transform', body)
        return {'text': json.dumps(res, ensure_ascii=False)}

    if name == 'tokenatra_set_ring':
        r = args.get('name')
        if isinstance(r, str) and r.strip().lower() in ('', 'none', 'null'):
            r = None
        body = {'name': r}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/ring', body)
        return {'text': json.dumps(res, ensure_ascii=False)}

    if name == 'tokenatra_set_effects':
        body = {}
        shadow = _block_from(args, 'shadow_json') or _block_from(args, 'shadow')
        color = _block_from(args, 'color_json') or _block_from(args, 'color')
        if shadow:
            body['shadow'] = shadow
        if color:
            body['color'] = color
        res = http_call('POST', f'/agent/v1/sessions/{sid}/effects', body)
        return {'text': json.dumps(res, ensure_ascii=False)}

    if name == 'tokenatra_history':
        body = {'action': args.get('action', 'undo')}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/history', body)
        return {'text': json.dumps(res, ensure_ascii=False)}

    if name == 'tokenatra_portrait':
        body = {k: args[k] for k in ('variant', 'scale', 'rotation', 'offset_x', 'offset_y',
                                     'size', 'format', 'quality')
                if k in args and args[k] is not None}
        payload = http_call('POST', f'/agent/v1/sessions/{sid}/portrait', body, raw=True)
        fmt = body.get('format', 'webp')
        save_to = args.get('save_to') or args.get('output_path')
        path = _save_export(payload, fmt, save_to, prefix='tokenatra-portrait')
        mime = _mime_for(fmt)
        return {
            'text': f'Saved portrait: {path}',
            'image': {'data': base64.b64encode(payload).decode('ascii'), 'mimeType': mime},
        }

    if name == 'tokenatra_inspect':
        qs = []
        if args.get('include_image'):
            qs.append('include_image=1')
        if 'preview_size' in args and args['preview_size'] is not None:
            qs.append(f"preview_size={int(args['preview_size'])}")
        if args.get('example'):
            qs.append('example=1')
        if 'example_opacity' in args and args['example_opacity'] is not None:
            qs.append(f"example_opacity={float(args['example_opacity'])}")
        if args.get('guides'):
            qs.append('guides=1')
        if 'scale_mode' in args and args['scale_mode'] is not None:
            qs.append(f"scale_mode={urllib.parse.quote(str(args['scale_mode']))}")
        suffix = ('?' + '&'.join(qs)) if qs else ''
        res = http_call('GET', f'/agent/v1/sessions/{sid}/inspect{suffix}')
        image = res.pop('image_data_url', None)
        out = {'text': json.dumps(res, ensure_ascii=False, indent=2)}
        if image:
            out['image'] = _data_url_block(image)
        return out

    if name == 'tokenatra_preview':
        qs = [f"size={int(args.get('size', 512))}"]
        if 'scale_mode' in args and args['scale_mode'] is not None:
            qs.append(f"scale_mode={urllib.parse.quote(str(args['scale_mode']))}")
        elif args.get('example'):
            qs.append('scale_mode=2')
        else:
            qs.append('scale_mode=1')
        if 'with_ring' in args and args['with_ring'] is not None:
            qs.append(f"with_ring={'1' if args['with_ring'] else '0'}")
        if args.get('example'):
            qs.append('example=1')
        if 'example_scale' in args and args['example_scale'] is not None:
            qs.append(f"example_scale={int(args['example_scale'])}")
        if 'example_opacity' in args and args['example_opacity'] is not None:
            qs.append(f"example_opacity={float(args['example_opacity'])}")
        if args.get('guides'):
            qs.append('guides=1')
        payload = http_call('GET', f"/agent/v1/sessions/{sid}/preview?{'&'.join(qs)}", raw=True)
        return {'image': {'data': base64.b64encode(payload).decode('ascii'),
                          'mimeType': 'image/webp'}}

    if name == 'tokenatra_export':
        body = {k: args[k] for k in ('format', 'quality', 'size', 'scale_mode', 'with_ring')
                if k in args and args[k] is not None}
        if 'scale_mode' not in body:
            body['scale_mode'] = '1'
        save_to = args.get('output_path') or args.get('save_to')
        # Infer format from file extension if save_to is a file path and format was not explicitly set
        if save_to and 'format' not in body:
            ext = Path(save_to).suffix.lower().lstrip('.')
            if ext in ('png', 'webp', 'jpg', 'jpeg'):
                body['format'] = 'jpg' if ext == 'jpeg' else ext
        fmt = body.get('format', 'png')
        body['format'] = fmt
        payload = http_call('POST', f'/agent/v1/sessions/{sid}/export', body, raw=True)
        path = _save_export(payload, fmt, save_to, prefix='tokenatra-export')
        mime = _mime_for(fmt)
        return {
            'text': f'Saved: {path}',
            'image': {'data': base64.b64encode(payload).decode('ascii'), 'mimeType': mime},
        }

    if name == 'tokenatra_close_session':
        http_call('DELETE', f'/agent/v1/sessions/{sid}')
        return {'text': 'closed'}

    raise ToolError(f'unknown tool {name}')


def _mime_for(fmt):
    if fmt == 'png':
        return 'image/png'
    if fmt in ('jpg', 'jpeg'):
        return 'image/jpeg'
    return 'image/webp'


def _data_url_block(data_url):
    header, _, b64 = data_url.partition(',')
    mime = header[len('data:'):header.index(';')] if header.startswith('data:') and ';' in header else 'image/webp'
    return {'data': b64, 'mimeType': mime}


def _save_export(payload, fmt, save_to, prefix='tokenatra-export'):
    if save_to:
        target = Path(save_to)
        # If save_to has an image extension (e.g. "W:/.../token.png"), save directly to that file!
        if target.suffix.lower() in ('.png', '.webp', '.jpg', '.jpeg') and not target.is_dir():
            parent = target.parent
            if not parent.exists():
                parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(payload)
            return str(target)
        # Otherwise treat save_to as a directory
        if not target.exists():
            target.mkdir(parents=True, exist_ok=True)
        if not target.is_dir():
            raise ToolError(f'save_to is neither a valid image file path nor a directory: {save_to}')
        folder = target
    else:
        folder = Path(tempfile.gettempdir())
    path = folder / f"{prefix}-{_timestamp()}.{fmt}"
    path.write_bytes(payload)
    return str(path)


def _timestamp():
    return time.strftime('%Y%m%d-%H%M%S')


# ===== JSON-RPC =====

def send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + '\n')
    sys.stdout.flush()


def result(req_id, value):
    send({'jsonrpc': '2.0', 'id': req_id, 'result': value})


def error(req_id, code, message):
    send({'jsonrpc': '2.0', 'id': req_id,
          'error': {'code': code, 'message': message}})


def handle(msg):
    if not isinstance(msg, dict):
        return
    method = msg.get('method', '')
    req_id = msg.get('id')
    is_notification = 'id' not in msg

    if method.startswith('notifications/'):
        return
    if req_id is None and method in ('initialize', 'ping', 'tools/list',
                                     'tools/call', 'resources/list',
                                     'resources/read'):
        return

    if method == 'initialize':
        version = msg.get('params', {}).get('protocolVersion', LATEST_VERSION)
        if version not in SUPPORTED_VERSIONS:
            version = LATEST_VERSION
        result(req_id, {
            'protocolVersion': version,
            'capabilities': {'tools': {}, 'resources': {}},
            'serverInfo': {'name': 'tokenatra', 'version': '2.0.0'},
            'instructions': _INSTRUCTIONS,
        })
    elif method == 'ping':
        result(req_id, {})
    elif method == 'tools/list':
        result(req_id, {'tools': _TOOLS})
    elif method == 'tools/call':
        params = msg.get('params', {})
        try:
            out = call_tool(params.get('name', ''), params.get('arguments') or {})
            content = []
            if 'text' in out:
                content.append({'type': 'text', 'text': out['text']})
            if 'image' in out:
                content.append({'type': 'image', **out['image']})
            result(req_id, {'content': content})
        except ToolError as e:
            result(req_id, {'content': [{'type': 'text', 'text': str(e)}],
                            'isError': True})
        except AppDown as e:
            result(req_id, {'content': [{'type': 'text', 'text': str(e)}],
                            'isError': True})
        except KeyError as e:
            result(req_id, {'content': [{'type': 'text',
                                         'text': f'missing parameter {e}'}],
                            'isError': True})
    elif method == 'resources/list':
        result(req_id, {'resources': _RESOURCES})
    elif method == 'resources/read':
        uri = msg.get('params', {}).get('uri', '')
        if uri == 'tokenatra://recipe':
            result(req_id, {'contents': [{
                'uri': uri, 'mimeType': 'text/plain',
                'text': _RECIPE_TEXT}]})
            return
        mapping = {'tokenatra://manifest': '/agent/v1/manifest',
                   'tokenatra://assets': '/agent/v1/assets'}
        if uri not in mapping:
            error(req_id, -32602, f'unknown resource {uri}')
            return
        data = http_call('GET', mapping[uri])
        result(req_id, {'contents': [{
            'uri': uri, 'mimeType': 'application/json',
            'text': json.dumps(data, ensure_ascii=False, indent=2)}]})
    elif method:
        if not is_notification:
            error(req_id, -32601, f'method not found: {method}')


def main():
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, 'reconfigure'):
            try:
                stream.reconfigure(encoding='utf-8', errors='replace')
            except Exception:
                pass
    log('tokenatra-mcp proxy v2.0 -> %s', SERVER_URL)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            handle(json.loads(line))
        except json.JSONDecodeError as e:
            log('bad json line: %s', e)
        except Exception as e:
            log('handler error: %r', e)


if __name__ == '__main__':
    main()
