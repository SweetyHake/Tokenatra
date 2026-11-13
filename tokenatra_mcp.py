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
Call `tokenatra_recipe({})` for the complete built-in masking & QA guide.

How to create a token in Tokenatra (follow the `example.png` style guide and strict masking rules):
1. Pre-check source portrait & open session (`tokenatra_create_session`):
   - Eye Gaze Rule: a 3/4 head turn is fine, but irises/pupils MUST look directly into the camera lens (verify on 1:1 full-resolution eye crops, never on thumbnails).
   - Call `tokenatra_create_session({ image_path: "C:/path/to/art.webp" })`.
   - The default ring `"steel"` and ring protection mask (`r_in = 343` in 1x 1024x1024 space) are active.
2. Remove background ONLY if needed: check `already_transparent` (or `opaque_pct < 92`).
   - If `already_transparent: false`, call `tokenatra_remove_background({ session_id })`.
   - NEVER call `tokenatra_remove_background` on an already transparent cutout/subject — re-matting eats fine edges and glow.
   - When exporting a transparent portrait via `tokenatra_portrait`, always pass `variant: "cutout"`.
3. Frame the character strictly by head & waist anatomy (`tokenatra_set_transform`):
   - Scale and position (`scale`, `offset_x`, `offset_y`) are determined ONLY by head and waist anatomy, NEVER by weapon reach/size (never shrink a character to fit a raised sword or wide crossbow).
   - In 1x logical ring coords (`1024x1024`, center `(512, 512)`):
     - **Crown of head (Макушка, excluding tall hats/horns)**: `y ~ -76` (pops out above `y = 0`).
     - **Chin / Jawline (Подбородок)**: `y ~ 200` (head height `~ 276px`, `rx ~ 112px`, centered at `x ~ 512`).
     - **Waist / Belt line (Талия / Пояс)**: `y ~ 716`.
   - In 2x preview coords (`1024x1024` with `scale_mode: "2"`, where 1x ring is `[256..768]`):
     - **Head oval (Макушка .. Подбородок)**: `y = 218..356` (height `138px`, `rx = 56px`, center `x = 512`).
     - **Waist / Belt line**: `y ~ 615`.
4. Exact Ring & `preset1` Geometry (in 1x `1024x1024` canvas space, center `(512, 512)`):
   - **Inner ring radius & protection mask (`protect: true`)**: `r_in = 343` (bottom at `y = 855`; `r_in = 171.5` in 2x 1024 preview).
   - **Outer ring radius (`steel`)**: `r_out = 407` (bottom at `y = 919`; `r_out = 203.5` in 2x 1024 preview).
   - **`preset1`**: keeps `mask = 255` everywhere above `y = 564`; below `y = 564` erases everything outside `r = 342`.
5. Masking & Pop-Out Rules (`tokenatra_apply_preset` + `tokenatra_edit_mask` in `target: "canvas"`):
   - Always finalize `tokenatra_set_transform` BEFORE applying custom `target: "canvas"` mask edits.
   - Avoid `target: "image"` for fine coordinate edits on non-square portraits (aspect ratio scaling shifts X-coords); perform all ring-edge and weapon-loop edits in `target: "canvas"` (`0..1024`).
   - **Eliminate `preset1` side steps at `y = 564`**:
     - Erase wide cloaks/scarves/pauldrons outside the ring above `y = 564` (`y = 300..600`) using `op: "erase", target: "canvas", protect: true` so they follow `r = 343` without a horizontal step at `y = 564`.
     - Restore active bent elbows/bracers that dip slightly below `y = 564` (`y = 564..680`) using `op: "restore", target: "canvas"` along their bottom contour.
   - **Strict Pop-Out Rules**:
     - NEVER restore torso, waist, belts, pouches, skirts, or hanging charms outside `r = 343` at the bottom.
     - In the lower hemisphere (`y > 564`), ONLY the weapon itself (blade, guard, grip, pommel, crossbow prod/stirrup) and the active hand holding it may extend outside `r = 343`. Erase any cloak/tunic/pouch trapped between the weapon blade and `r = 343` (`op: "erase", protect: true`) so the metallic ring rim shows cleanly.
     - **Lowered empty arms MUST stay under the mask**: an empty hand/arm hanging down along the side (`3–5` or `7–9` o'clock) must be tucked inside `r = 343` (`op: "erase", protect: true`), never popped out over the ring.
     - **Clean closed weapon loops**: punch out opaque background/cloak trapped inside crossbow stirrups or sword D-guards (`op: "erase", target: "canvas", protect: false`).
6. Export (`tokenatra_export`):
   - For a `subject.webp` cutout (used with dynamic rings), enable drop shadow (`{"enabled":true,"angle":135,"distance":7,"blur":3,"opacity":0.4}`) and export with `with_ring: false, scale_mode: "2", size: 1024`.
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
TOKENATRA STUDIO BUILT-IN GUIDE & QA RULES

1. SOURCE PORTRAIT REQUIREMENTS (BEFORE SESSION):
   - **Eye Gaze Rule**: A 3/4 head turn is allowed, but irises and pupils MUST look directly into the camera lens (direct eye contact with the viewer). Always verify gaze on 1:1 full-resolution eye crops, never on downscaled thumbnails.
   - **Background Removal (`tokenatra_remove_background`)**:
     - Call ONLY if `already_transparent == false` (or `opaque_pct >= 92`).
     - NEVER call on an already transparent cutout/subject — re-running ONNX eats fine edges and glow.
     - When exporting a transparent portrait (`tokenatra_portrait`), always pass `variant: "cutout"`.

2. ANATOMICAL CALIBRATION OF SCALE & POSITION (`tokenatra_set_transform`):
   - `scale`, `offset_x`, `offset_y` are determined STRICTLY by head and waist anatomy, NEVER by weapon size/reach (never shrink a character just to fit a raised sword or wide crossbow).
   - **Reference Coordinates (`example.png` standard)**:
     - In `1x` logical ring space (`1024x1024`, center `(512, 512)`):
       - **Crown of head (Макушка, excluding tall hats/helmets/horns)**: `y ~ -76` (pops out above `y = 0`).
       - **Chin / Jawline (Подбородок)**: `y ~ 200` (head height `~ 276px`, horizontal radius `rx ~ 112px`, face centered at `x ~ 512`).
       - **Waist / Belt line (Талия / Пояс)**: `y ~ 716`.
     - In `2x` preview space (`1024x1024` with `scale_mode: "2"`, where the 1x ring occupies `[256..768]`):
       - **Head oval (Макушка .. Подбородок)**: `y = 218..356` (height `138px`, `rx = 56px`, center `x = 512`).
       - **Waist / Belt line**: `y ~ 615`.
   - **Transform Formula**:
     Non-square images preserve aspect ratio (`fit1024` fits longest side into 1024px, centered):
       L(fx) = 512 + (fx - 512) * scale + offset_x
       L(fy) = 512 + (fy - 512) * scale + offset_y

3. EXACT RING, PROTECTION MASK & `preset1` GEOMETRY (1x `1024x1024` SPACE, CENTER `(512, 512)`):
   - **Inner ring radius & protection mask (`protect: true`)**: `r_in = 343` (bottom point `y = 855`; `r_in = 171.5` in 2x 1024 preview). When `protect: true`, `erase` cannot erase any pixel inside `r = 343`.
   - **Outer ring radius (`steel`)**: `r_out = 407` (bottom point `y = 919`; `r_out = 203.5` in 2x 1024 preview).
   - **`preset1` (`tokenatra_apply_preset`)**: keeps `mask = 255` everywhere above `y = 564`; below `y = 564` erases everything outside `r = 342` (`y_bottom = 854`).
   - **QA Audit Rule**: Always composite verification sheets over the real ring asset (`with_ring: true` or the actual ring image), never over synthetic circles of arbitrary radius.

4. COORDINATE SPACES (`target: "canvas"` vs `target: "image"`):
   - **Avoid `target: "image"` for fine coordinate edits on non-square images**: `target: "image"` maps `0..1024` across `0..width` and `0..height` independently (`sx = width/1024, sy = height/1024`), so coordinates measured on a square aspect-preserved preview shift horizontally on 4:5 portraits.
   - **Always use `target: "canvas"` (`0..1024` = 1x ring frame)** for ring-edge masking, weapon pop-outs, and stirrup/guard loop cleanups.
   - **Order of operations (`transform` -> `mask`)**: `target: "canvas"` is fixed to the ring. Always finalize `(scale, offset_x, offset_y)` BEFORE applying custom `target: "canvas"` strokes. Use a `1024x1024` 1x grid (`20px` step) with `r = 342` and `r = 407` circles to read exact coordinates.

5. ELIMINATING `preset1` SIDE STEPS AT `y = 564`:
   Because `preset1` only erases outside `r = 342` below `y = 564`:
   1) **Wide cloaks / scarves / pauldrons above `y = 564`**: If a cloak or scarf extends outside `r = 343` in `y = 300..564`, `preset1` leaves it over the ring above `y = 564` and cuts it with a harsh horizontal step at `y = 564`. Fix: apply `op: "erase", target: "canvas", protect: true` over the cloak area in `y = 300..600` so it follows the smooth `r = 343` inner ring arc.
   2) **Active bent elbows / bracers crossing `y = 564`**: If an active elbow or forearm dips below `y = 564` (`y = 564..680`), `preset1` slices off its bottom edge horizontally at `y = 564`. Fix: smoothly `op: "restore", target: "canvas", protect: false` along the anatomical bottom contour of the forearm/bracer (overlapping `y = 564` from above without restoring any cloak below the bracer).

6. STRICT POP-OUT & QUALITY CONTROL RULES:
   - **No torso/waist/skirts/pouches below `r = 343`**: The lower torso, belt, sash, skirts (chainmail/scale/leather), pouches, and belt charms must be cleanly clipped at `r = 343` (`y = 855`). Never use wide `restore` shapes at the bottom that spill the character's belly or hips outside the ring.
   - **Clean weapon pop-outs in the lower hemisphere (`4–8` o'clock)**: When a blade, dagger, or crossbow crosses the lower ring (`r > 343, y > 564`), ONLY the weapon itself (blade, guard, grip, pommel, bow prod, stirrup) and the active hand holding it may exist outside `r = 343`. Erase all cloak, tunic, belt, pouch, or charm pixels trapped between the weapon and `r = 343` (or under the blade) using `op: "erase", target: "canvas", protect: true` so the metallic ring rim is cleanly visible between the body and the blade. Stop `restore` strokes right at the true steel tip of the blade.
   - **Lowered empty arms MUST stay under the mask (`inside r = 343`)**: If a free arm/hand (not holding a weapon or casting a spell) simply hangs down along the side (`3–5` or `7–9` o'clock), it MUST be tucked inside `r = 343` (`op: "erase", target: "canvas", protect: true`), never popped out over the ring.
   - **Closed weapon loops (crossbow stirrups, sword D-guards)**: Inspect closed loops on a high-contrast background and punch out any trapped background/cloak pixels (`op: "erase", target: "canvas", protect: false`) so the ring or transparent background shows cleanly through the loop.
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
