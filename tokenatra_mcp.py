#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""tokenatra_mcp.py — MCP stdio-сервер (тонкий прокси на Agent API).

Модель Blender MCP: запускается MCP-клиентом (Claude Desktop, OpenCode,
Cursor) как stdio-процесс; редактор живёт в самом Tokenatra на
127.0.0.1:7878 (приложение или `python server.py`). Прокси не содержит
логики — только перевод JSON-RPC <-> HTTP.

Требования по итогам ревью:
- НОЛЬ сторонних зависимостей (официальный MCP SDK тянет
  anyio/pydantic/httpx, заэксклюденные в build.spec).
- stdout — ТОЛЬКО JSON-RPC сообщения (newline-delimited, не LSP);
  весь логинг — в stderr. server.py печатает [RAM]-логи в stdout,
  поэтому in-process stdio-сервер был бы коррумпирован.
- UTF-8 на Windows (консоль по умолчанию cp1251/cp866).
- protocolVersion: отвечаем версией клиента, если поддерживаем,
  иначе своей последней.
- notifications (без id) никогда не получают ответа.

Адрес сервера: env TOKENATRA_URL (default http://127.0.0.1:7878).
"""
import base64
import json
import os
import sys
import urllib.error
import urllib.request

SERVER_URL = os.environ.get('TOKENATRA_URL', 'http://127.0.0.1:7878').rstrip('/')
MARKER = {'X-Tokenatra-Agent': '1'}
SUPPORTED_VERSIONS = ('2025-06-18', '2024-11-05')
LATEST_VERSION = SUPPORTED_VERSIONS[0]
HTTP_TIMEOUT = 180  # remove_bg на CPU может быть долгим

_TOOLS = [
    {
        'name': 'tokenatra_manifest',
        'description': (
            'Get Tokenatra agent API manifest: limits (max 4 sessions, '
            '30 min TTL, 200 ops), defaults, coordinate space (logical '
            '1024x1024). Call this first to understand the editor.'),
        'inputSchema': {'type': 'object', 'properties': {}},
    },
    {
        'name': 'tokenatra_list_assets',
        'description': (
            'List available token rings and mask presets (name + builtin). '
            'Use names with set_ring / apply_preset.'),
        'inputSchema': {'type': 'object', 'properties': {}},
    },
    {
        'name': 'tokenatra_create_session',
        'description': (
            'Create an editing session from an image. Read the image file '
            'YOURSELF (you have filesystem access) and pass its bytes as '
            'base64 (raw or data URL). PNG/WebP/JPG up to ~11 MB.'),
        'inputSchema': {
            'type': 'object',
            'required': ['image_b64'],
            'properties': {
                'image_b64': {'type': 'string',
                              'description': 'base64 or data:image/...;base64'},
            },
        },
    },
    {
        'name': 'tokenatra_remove_background',
        'description': (
            'Remove background with the ONNX model (BiRefNet/RMBG/BEN2). '
            'First call may take 3-10 s (model load).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'edge_blur': {'type': 'number',
                              'description': '0..10, default 1'},
            },
        },
    },
    {
        'name': 'tokenatra_edit_mask',
        'description': (
            'Edit the visibility mask (logical 1024x1024 coords). ops: '
            'erase/restore (shapes), apply_preset, invert, feather, grow. '
            'shapes_json: [{"circle":{"cx":512,"cy":512,"r":100}}, '
            '{"rect":{"x0":0,"y0":0,"x1":512,"y1":1024}}, '
            '{"stroke":{"points":[[0,0],[100,100]],"width":30}}]. '
            'erase respects the protection mask (protect=false to ignore).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id', 'op'],
            'properties': {
                'session_id': {'type': 'string'},
                'op': {'type': 'string', 'enum': ['erase', 'restore',
                                                  'apply_preset', 'invert',
                                                  'feather', 'grow']},
                'shapes_json': {'type': 'string',
                                'description': 'JSON array of shapes (see description)'},
                'preset': {'type': 'string', 'description': 'preset name for apply_preset'},
                'px': {'type': 'number', 'description': 'radius for feather/grow'},
                'protect': {'type': 'boolean', 'description': 'respect protection mask (default true)'},
            },
        },
    },
    {
        'name': 'tokenatra_apply_preset',
        'description': 'Apply a named mask preset (stretched to full canvas).',
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
            'Set subject transform. scale is relative to fit-in-canvas '
            '(1.0 = fitted into 1024 logical), rotation in degrees '
            '(clockwise, -180..180), offset in logical px from center.'),
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
        'description': 'Set token ring by name, or remove it (name=null).',
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'name': {'type': 'string', 'description': 'ring name or null to remove'},
            },
        },
    },
    {
        'name': 'tokenatra_set_effects',
        'description': (
            'Set effects. shadow_json: {"enabled":true,"angle":135,'
            '"distance":7,"blur":3,"opacity":0.4}; color_json: '
            '{"enabled":true,"saturation":0,"lightness":0} (percent).'),
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
        'name': 'tokenatra_inspect',
        'description': (
            'Inspect current state: coverage_pct, bbox, 3x3 grid, edge_px, '
            'alpha histogram, warnings, delta vs previous inspect. Numeric '
            'feedback for text models; include_image=true also returns a '
            'preview image (multimodal models).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'include_image': {'type': 'boolean'},
                'preview_size': {'type': 'number', 'description': '64..768, default 384'},
            },
        },
    },
    {
        'name': 'tokenatra_preview',
        'description': 'Full-canvas preview image (webp) of the current state.',
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'size': {'type': 'number', 'description': '64..768, default 384'},
            },
        },
    },
    {
        'name': 'tokenatra_export',
        'description': (
            'Export the final token image. Saves to save_to folder '
            '(must exist) or to a temp file; returns the path and a preview. '
            'scale_mode: auto|1|2|3 (final px = size * scale_mode).'),
        'inputSchema': {
            'type': 'object',
            'required': ['session_id'],
            'properties': {
                'session_id': {'type': 'string'},
                'format': {'type': 'string', 'enum': ['webp', 'png', 'jpg']},
                'quality': {'type': 'number', 'description': '10..100, default 95'},
                'size': {'type': 'number', 'description': '256..2048, default 1024'},
                'scale_mode': {'type': 'string', 'enum': ['auto', '1', '2', '3']},
                'with_ring': {'type': 'boolean'},
                'save_to': {'type': 'string', 'description': 'existing folder path'},
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
     'mimeType': 'application/json', 'description': 'Limits and defaults'},
    {'uri': 'tokenatra://assets', 'name': 'Tokenatra assets',
     'mimeType': 'application/json', 'description': 'Rings and presets'},
]


def log(msg, *args):
    print('[mcp]', msg % args if args else msg, file=sys.stderr, flush=True)


class AppDown(Exception):
    pass


def http_call(method, path, body=None, raw=False):
    url = SERVER_URL + path
    data = json.dumps(body).encode() if body is not None else None
    headers = dict(MARKER)
    if data is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as resp:
            payload = resp.read()
            ctype = resp.headers.get('Content-Type', '')
    except urllib.error.HTTPError as e:
        # Ошибка API: тело — машиночитаемый JSON, отдать модели как есть
        err_body = ''
        try:
            err_body = e.read().decode('utf-8', 'replace')
            err = json.loads(err_body)
        except Exception:
            err = {'error': err_body or str(e), 'http': e.code}
        raise ToolError(json.dumps(err, ensure_ascii=False))
    except (urllib.error.URLError, OSError) as e:
        raise AppDown(
            f'Tokenatra is not running at {SERVER_URL} ({e.reason if hasattr(e, "reason") else e}). '
            'Start the Tokenatra app or run `python server.py`, then retry.')
    if raw or not ctype.startswith('application/json'):
        return payload
    return json.loads(payload.decode('utf-8'))


class ToolError(Exception):
    """Ошибка тулзы: показывается модели как isError-результат."""


# ===== Диспетчер тулзов =====

def _shapes_from(args):
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
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise ToolError(f'{key} is not valid JSON: {e}')
    if not isinstance(parsed, dict):
        raise ToolError(f'{key} must be a JSON object')
    return parsed


def call_tool(name, args):
    sid = args.get('session_id')

    if name == 'tokenatra_manifest':
        return {'text': json.dumps(http_call('GET', '/agent/v1/manifest'),
                                   ensure_ascii=False, indent=1)}
    if name == 'tokenatra_list_assets':
        return {'text': json.dumps(http_call('GET', '/agent/v1/assets'),
                                   ensure_ascii=False, indent=1)}
    if name == 'tokenatra_create_session':
        res = http_call('POST', '/agent/v1/sessions',
                        {'image': args['image_b64']})
        return {'text': _session_line(res)}
    if name == 'tokenatra_remove_background':
        res = http_call('POST', f'/agent/v1/sessions/{sid}/remove_bg',
                        {'edge_blur': args.get('edge_blur', 1.0)})
        return {'text': _session_line(res)}
    if name == 'tokenatra_edit_mask':
        body = {'op': args['op'],
                'shapes': _shapes_from(args),
                'preset': args.get('preset'),
                'px': args.get('px', 0),
                'protect': args.get('protect', True)}
        body = {k: v for k, v in body.items() if v is not None}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/mask', body)
        return {'text': json.dumps(res, ensure_ascii=False)}
    if name == 'tokenatra_apply_preset':
        res = http_call('POST', f'/agent/v1/sessions/{sid}/mask',
                        {'op': 'apply_preset', 'preset': args['preset']})
        return {'text': json.dumps(res, ensure_ascii=False)}
    if name == 'tokenatra_set_transform':
        body = {k: args[k] for k in ('scale', 'rotation', 'offset_x', 'offset_y')
                if k in args}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/transform', body)
        return {'text': json.dumps(res, ensure_ascii=False)}
    if name == 'tokenatra_set_ring':
        body = {'name': args.get('name')}
        res = http_call('POST', f'/agent/v1/sessions/{sid}/ring', body)
        return {'text': json.dumps(res, ensure_ascii=False)}
    if name == 'tokenatra_set_effects':
        body = {}
        shadow = _block_from(args, 'shadow_json')
        color = _block_from(args, 'color_json')
        if shadow:
            body['shadow'] = shadow
        if color:
            body['color'] = color
        res = http_call('POST', f'/agent/v1/sessions/{sid}/effects', body)
        return {'text': json.dumps(res, ensure_ascii=False)}
    if name == 'tokenatra_inspect':
        qs = []
        if args.get('include_image'):
            qs.append('include_image=1')
        if 'preview_size' in args:
            qs.append(f"preview_size={int(args['preview_size'])}")
        suffix = ('?' + '&'.join(qs)) if qs else ''
        res = http_call('GET', f'/agent/v1/sessions/{sid}/inspect{suffix}')
        image = res.pop('image_data_url', None)
        out = {'text': json.dumps(res, ensure_ascii=False)}
        if image:
            out['image'] = _data_url_block(image)
        return out
    if name == 'tokenatra_preview':
        size = int(args.get('size', 384))
        payload = http_call('GET',
                            f'/agent/v1/sessions/{sid}/preview?size={size}',
                            raw=True)
        return {'image': {'data': base64.b64encode(payload).decode('ascii'),
                          'mimeType': 'image/webp'}}
    if name == 'tokenatra_export':
        body = {k: args[k] for k in ('format', 'quality', 'size', 'scale_mode',
                                     'with_ring') if k in args}
        payload = http_call('POST', f'/agent/v1/sessions/{sid}/export', body)
        fmt = body.get('format', 'webp')
        save_to = args.get('save_to')
        path = _save_export(payload, fmt, save_to)
        mime = 'image/png' if fmt == 'png' else (
            'image/jpeg' if fmt == 'jpg' else 'image/webp')
        return {'text': f'Saved: {path}',
                'image': {'data': base64.b64encode(payload).decode('ascii'),
                          'mimeType': mime}}
    if name == 'tokenatra_close_session':
        http_call('DELETE', f'/agent/v1/sessions/{sid}')
        return {'text': 'closed'}
    raise ToolError(f'unknown tool {name}')


def _session_line(res):
    return (f"session_id={res['session_id']} image="
            f"{res['image']['width']}x{res['image']['height']} "
            f"bg_removed={res['bg_removed']} ops={res['ops_used']}")


def _data_url_block(data_url):
    header, _, b64 = data_url.partition(',')
    mime = header[len('data:'):header.index(';')] if header.startswith('data:') else 'image/webp'
    return {'data': b64, 'mimeType': mime}


def _save_export(payload, fmt, save_to):
    import tempfile
    from pathlib import Path
    if save_to:
        folder = Path(save_to)
        if not folder.is_dir():
            raise ToolError(f'save_to folder does not exist: {save_to}')
    else:
        folder = Path(tempfile.gettempdir())
    path = folder / f"tokenatra-export-{_timestamp()}.{fmt}"
    path.write_bytes(payload)
    return str(path)


def _timestamp():
    import time
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
        return  # инициализация/отмена — молча
    if req_id is None and method in ('initialize', 'ping', 'tools/list',
                                     'tools/call', 'resources/list',
                                     'resources/read'):
        return  # битый запрос без id

    if method == 'initialize':
        version = msg.get('params', {}).get('protocolVersion', LATEST_VERSION)
        if version not in SUPPORTED_VERSIONS:
            version = LATEST_VERSION
        result(req_id, {
            'protocolVersion': version,
            'capabilities': {'tools': {}, 'resources': {}},
            'serverInfo': {'name': 'tokenatra', 'version': '1.0.0'},
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
        mapping = {'tokenatra://manifest': '/agent/v1/manifest',
                   'tokenatra://assets': '/agent/v1/assets'}
        if uri not in mapping:
            error(req_id, -32602, f'unknown resource {uri}')
            return
        data = http_call('GET', mapping[uri])
        result(req_id, {'contents': [{
            'uri': uri, 'mimeType': 'application/json',
            'text': json.dumps(data, ensure_ascii=False)}]})
    elif method:
        if not is_notification:
            error(req_id, -32601, f'method not found: {method}')


def main():
    # Windows: консоль по умолчанию не UTF-8; протокол MCP — UTF-8
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, 'reconfigure'):
            try:
                stream.reconfigure(encoding='utf-8', errors='replace')
            except Exception:
                pass
    log('tokenatra-mcp proxy -> %s', SERVER_URL)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            handle(json.loads(line))
        except json.JSONDecodeError as e:
            log('bad json line: %s', e)
        except Exception as e:  # прокси не должен умирать от одной тулзы
            log('handler error: %r', e)


if __name__ == '__main__':
    main()
