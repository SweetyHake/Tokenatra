# -*- coding: utf-8 -*-
"""agent_api.py — HTTP-транспорт Agent API (blueprint /agent/v1/*).

Тонкий слой над agent_pipeline: парсинг/валидация, лимиты, машиночитаемые
ошибки. Вся логика — в ядре. Доступ контролирует гуард server.py
(ветка /agent/*: local_tool + X-Tokenatra-Agent, см. tests/test_agent_guard).

Каталог ошибок (code -> HTTP):
  bad_params        400   невалидные параметры (valid[] перечисляет допустимое)
  bad_image         400   не декодируется
  unknown_asset     400   нет такого кольца/пресета
  too_large         413   тело запроса больше AGENT_MAX_BODY
  ops_exhausted     429   лимит операций сессии (payload: recipe)
  session_not_found 404
  model_missing     503   .onnx не выбран/нет файла
  busy              503   семафор тяжёлых операций
"""
import json
import threading

from flask import Blueprint, jsonify, request, send_file

import agent_pipeline as core
from agent_pipeline import AgentError, STORE

AGENT_API_VERSION = 1
AGENT_MAX_BODY = 15 * 1024 * 1024          # аудитор: декод до семафора = OOM
AGENT_HEAVY_SEMAPHORE = threading.Semaphore(2)

EXPORT_FORMATS = ['webp', 'png', 'jpg']

agent_bp = Blueprint('agent', __name__, url_prefix='/agent/v1')


@agent_bp.before_request
def _agent_body_limits():
    if request.content_length is not None and request.content_length > AGENT_MAX_BODY:
        return _err(AgentError('too_large',
                               f'request body exceeds {AGENT_MAX_BODY} bytes',
                               status=413))
    return None


@agent_bp.errorhandler(AgentError)
def _agent_error(err):
    return _err(err)


def _err(err):
    body = {'error': err.message, 'code': err.code}
    if err.hint:
        body['hint'] = err.hint
    if err.valid:
        body['valid'] = err.valid
    if err.payload:
        body.update(err.payload)
    return jsonify(body), err.status


def _json_body():
    data = request.get_json(force=True, silent=True)
    if not isinstance(data, dict):
        raise AgentError('bad_params', 'request body must be a JSON object')
    return data


def _get_session(sid):
    s = STORE.get(sid)
    if s.ops >= core.MAX_OPS_PER_SESSION:
        raise AgentError('ops_exhausted',
                         f'session reached {core.MAX_OPS_PER_SESSION} operations',
                         hint='recipe is attached: replay it in a new session',
                         status=429,
                         payload={'recipe': list(s.recipe)})
    return s


def _op_log(sid, name):
    from flask import current_app
    current_app.logger.info('agent op %s session=%s from=%s',
                            name, sid, request.remote_addr)


class _Heavy:
    """Контекст-менеджер: не более 2 тяжёлых операций одновременно."""

    def __enter__(self):
        if not AGENT_HEAVY_SEMAPHORE.acquire(timeout=30):
            raise AgentError('busy', 'server is busy with other agent operations',
                             status=503)
        return self

    def __exit__(self, *exc):
        AGENT_HEAVY_SEMAPHORE.release()
        return False


# ===== Информационные =====

@agent_bp.get('/manifest')
def manifest():
    return jsonify({
        'name': 'tokenatra',
        'api_version': AGENT_API_VERSION,
        'logical_size': core.LOGICAL,
        'limits': {
            'max_sessions': core.MAX_SESSIONS,
            'session_ttl_min': core.SESSION_TTL_MIN,
            'max_ops_per_session': core.MAX_OPS_PER_SESSION,
            'max_body_bytes': AGENT_MAX_BODY,
            'max_base_dim': core.MAX_BASE_DIM,
            'export_size_range': list(core.EXPORT_SIZE_RANGE),
        },
        'defaults': core.DEFAULT_EFFECTS,
        'ops': ['create', 'remove_bg', 'mask', 'transform', 'ring',
                'effects', 'inspect', 'preview', 'export', 'close'],
        'mask_ops': ['erase', 'restore', 'apply_preset', 'invert',
                     'feather', 'grow'],
        'export_formats': EXPORT_FORMATS,
        'required_header': {'X-Tokenatra-Agent': '1'},
    })


@agent_bp.get('/assets')
def assets():
    return jsonify(core.list_assets())


@agent_bp.get('/tools_schema')
def tools_schema():
    """OpenAI function-calling схемы (static, без параметров диалекта —
    правка арх. ревью). Источник — те же определения, что отдаёт MCP:
    схемы не могут разъехаться."""
    import tokenatra_mcp
    return jsonify([{
        'type': 'function',
        'function': {
            'name': t['name'],
            'description': t['description'],
            'parameters': t['inputSchema'],
        },
    } for t in tokenatra_mcp._TOOLS])


# ===== Сессии =====

@agent_bp.post('/sessions')
def create_session():
    body = _json_body()
    if 'image' not in body:
        raise AgentError('bad_params', "body must contain 'image' (base64 or data URL)")
    with _Heavy():
        base = core.decode_source(body['image'])
        session = STORE.create(base)
    _op_log(session.sid, 'create')
    return jsonify(session.state_summary()), 201


@agent_bp.get('/sessions/<sid>')
def get_session(sid):
    return jsonify(STORE.get(sid).state_summary())


@agent_bp.delete('/sessions/<sid>')
def close_session(sid):
    closed = STORE.close(sid)
    if not closed:
        raise AgentError('session_not_found',
                         f'session {sid} does not exist or expired', status=404)
    _op_log(sid, 'close')
    return jsonify({'closed': True})


@agent_bp.post('/sessions/<sid>/remove_bg')
def op_remove_bg(sid):
    session = _get_session(sid)
    body = _json_body()
    edge_blur = body.get('edge_blur', 1.0)
    _op_log(sid, 'remove_bg')
    try:
        with _Heavy():
            core.remove_bg(session, float(edge_blur))
    except FileNotFoundError:
        raise AgentError('model_missing',
                         'ONNX model is not available; select it in Settings',
                         status=503)
    return jsonify(session.state_summary())


@agent_bp.post('/sessions/<sid>/mask')
def op_mask(sid):
    session = _get_session(sid)
    body = _json_body()
    op = body.get('op')
    _op_log(sid, 'mask.' + str(op))
    stats = core.mask_op(
        session, op,
        shapes=body.get('shapes'),
        preset=body.get('preset'),
        px=body.get('px', 0),
        protect=body.get('protect', True),
    )
    return jsonify({'ok': True, 'mask': stats, 'ops_used': session.ops})


@agent_bp.post('/sessions/<sid>/transform')
def op_transform(sid):
    session = _get_session(sid)
    body = _json_body()
    _op_log(sid, 'transform')
    core.set_transform(session, **{
        k: body.get(k) for k in ('scale', 'rotation', 'offset_x', 'offset_y')})
    return jsonify({'ok': True, 'transform': session.transform,
                    'ops_used': session.ops})


@agent_bp.post('/sessions/<sid>/ring')
def op_ring(sid):
    session = _get_session(sid)
    body = _json_body()
    name = body.get('name')
    if name is not None and not isinstance(name, str):
        raise AgentError('bad_params', "ring 'name' must be a string or null")
    _op_log(sid, 'ring')
    core.set_ring(session, name)
    return jsonify({'ok': True, 'ring': session.ring, 'ops_used': session.ops})


@agent_bp.post('/sessions/<sid>/effects')
def op_effects(sid):
    session = _get_session(sid)
    body = _json_body()
    shadow = body.get('shadow')
    color = body.get('color')
    for block in (shadow, color):
        if block is not None and not isinstance(block, dict):
            raise AgentError('bad_params', 'effects blocks must be objects')
    _op_log(sid, 'effects')
    core.set_effects(session, shadow=shadow, color=color)
    return jsonify({'ok': True, 'effects': session.effects,
                    'ops_used': session.ops})


@agent_bp.get('/sessions/<sid>/preview')
def op_preview(sid):
    session = _get_session(sid)
    try:
        size = int(request.args.get('size', core.DEFAULT_PREVIEW))
    except ValueError:
        raise AgentError('bad_params', 'size must be an integer')
    size = max(64, min(core.MAX_PREVIEW, size))
    _op_log(sid, 'preview')
    with _Heavy():
        img, _ = core.compose(session, size, scale_mode='3', with_ring=True)
        # scale_mode 3 возвращает size*3 (весь канвас): приводим к запрошенному
        if img.size != (size, size):
            from PIL import Image
            img = img.resize((size, size), Image.LANCZOS)
    import io
    buf = io.BytesIO()
    img.save(buf, format='WEBP', quality=85)
    buf.seek(0)
    return send_file(buf, mimetype='image/webp')


@agent_bp.get('/sessions/<sid>/inspect')
def op_inspect(sid):
    session = _get_session(sid)
    include_image = request.args.get('include_image', '0') in ('1', 'true')
    try:
        preview_size = int(request.args.get('preview_size', core.DEFAULT_PREVIEW))
    except ValueError:
        preview_size = core.DEFAULT_PREVIEW
    preview_size = max(64, min(core.MAX_PREVIEW, preview_size))
    _op_log(sid, 'inspect')
    with _Heavy():
        return jsonify(core.inspect(session, include_image=include_image,
                                    preview_size=preview_size))


@agent_bp.post('/sessions/<sid>/export')
def op_export(sid):
    session = _get_session(sid)
    body = _json_body()
    fmt = body.get('format', 'webp')
    if fmt not in EXPORT_FORMATS:
        raise AgentError('bad_params', f'unknown format {fmt!r}',
                         valid=EXPORT_FORMATS)
    quality = int(body.get('quality', 95))
    quality = max(core.EXPORT_QUALITY_RANGE[0],
                  min(core.EXPORT_QUALITY_RANGE[1], quality))
    size = int(body.get('size', 1024))
    size = max(core.EXPORT_SIZE_RANGE[0], min(core.EXPORT_SIZE_RANGE[1], size))
    scale_mode = str(body.get('scale_mode', 'auto'))
    with_ring = bool(body.get('with_ring', True))
    _op_log(sid, 'export')
    with _Heavy():
        img, used = core.compose(session, size, scale_mode=scale_mode,
                                 with_ring=with_ring)
        import server
        try:
            buf, mime = server.save_image(img, fmt, quality)
        except ValueError as e:  # avif и пр. недоступны
            raise AgentError('bad_params', str(e), valid=EXPORT_FORMATS)
    response = send_file(buf, mimetype=mime)
    response.headers['X-Tokenatra-Used-Scale'] = str(used)
    response.headers['X-Tokenatra-Session'] = session.sid
    return response
