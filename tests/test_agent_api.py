# -*- coding: utf-8 -*-
"""HTTP-тесты Agent API (/agent/v1/*) через Flask test-client.

Заголовок X-Tokenatra-Agent — обязателен (см. test_agent_guard).
remove_bg мокается — ONNX не нужен.
"""
import base64
import io

import pytest
from PIL import Image, ImageDraw

MARKER = {'X-Tokenatra-Agent': '1'}


def _b64(img):
    buf = io.BytesIO()
    img.save(buf, format='PNG')
    return base64.b64encode(buf.getvalue()).decode()


def _red_square(w=96, h=96):
    img = Image.new('RGBA', (w, h), (10, 10, 10, 255))
    ImageDraw.Draw(img).rectangle([8, 8, w - 9, h - 9], fill=(200, 30, 30, 255))
    return img


@pytest.fixture()
def api(client, monkeypatch):
    import agent_pipeline as core
    core.STORE._sessions.clear()
    core._clear_caches()

    def fake_remove_bg(image, edge_blur=1):
        import numpy as np
        arr = np.array(image.convert('RGBA'))
        red = arr[:, :, 0].astype(int) - arr[:, :, 1].astype(int)
        arr[:, :, 3] = (red > 60).astype(np.uint8) * 255
        return Image.fromarray(arr, 'RGBA')
    import server
    monkeypatch.setattr(server, 'remove_background', fake_remove_bg)
    yield client
    core.STORE._sessions.clear()
    core._clear_caches()


def _create(api, img=None):
    r = api.post('/agent/v1/sessions',
                 json={'image': _b64(img or _red_square())},
                 headers=MARKER)
    assert r.status_code == 201, r.get_data(as_text=True)
    return r.get_json()['session_id']


# ===== Жизненный цикл =====

def test_full_pipeline_end_to_end(api):
    sid = _create(api)
    # удаление фона (мок)
    r = api.post(f'/agent/v1/sessions/{sid}/remove_bg', json={},
                 headers=MARKER)
    assert r.status_code == 200
    assert r.get_json()['bg_removed'] is True
    # маска
    r = api.post(f'/agent/v1/sessions/{sid}/mask',
                 json={'op': 'erase',
                       'shapes': [{'rect': {'x0': 0, 'y0': 0, 'x1': 512, 'y1': 1024}}]},
                 headers=MARKER)
    assert r.status_code == 200
    assert r.get_json()['mask']['coverage_pct'] < 100
    # эффекты + экспорт
    api.post(f'/agent/v1/sessions/{sid}/effects',
             json={'shadow': {'enabled': True}}, headers=MARKER)
    r = api.post(f'/agent/v1/sessions/{sid}/export',
                 json={'format': 'webp', 'quality': 90, 'size': 512,
                       'scale_mode': '1', 'with_ring': False},
                 headers=MARKER)
    assert r.status_code == 200
    assert r.mimetype == 'image/webp'
    assert r.headers['X-Tokenatra-Used-Scale'] == '1'
    img = Image.open(io.BytesIO(r.get_data()))
    assert img.size == (512, 512)


def test_session_get_and_close(api):
    sid = _create(api)
    r = api.get(f'/agent/v1/sessions/{sid}', headers=MARKER)
    assert r.status_code == 200
    body = r.get_json()
    assert body['recipe'][0]['op'] == 'create'
    assert api.delete(f'/agent/v1/sessions/{sid}', headers=MARKER).status_code == 200
    r = api.get(f'/agent/v1/sessions/{sid}', headers=MARKER)
    assert r.status_code == 404
    assert r.get_json()['code'] == 'session_not_found'


# ===== Формы ошибок =====

def test_create_without_image(api):
    r = api.post('/agent/v1/sessions', json={}, headers=MARKER)
    assert r.status_code == 400
    assert r.get_json()['code'] == 'bad_params'


def test_unknown_ring_lists_valid(api):
    sid = _create(api)
    r = api.post(f'/agent/v1/sessions/{sid}/ring',
                 json={'name': 'no-such-ring'}, headers=MARKER)
    assert r.status_code == 400
    body = r.get_json()
    assert body['code'] == 'unknown_asset'
    assert 'hint' in body


def test_unknown_mask_op_lists_valid(api):
    sid = _create(api)
    r = api.post(f'/agent/v1/sessions/{sid}/mask',
                 json={'op': 'explode'}, headers=MARKER)
    assert r.status_code == 400
    assert 'erase' in r.get_json()['valid']


def test_export_bad_format(api):
    sid = _create(api)
    r = api.post(f'/agent/v1/sessions/{sid}/export',
                 json={'format': 'tiff'}, headers=MARKER)
    assert r.status_code == 400
    assert r.get_json()['valid'] == ['webp', 'png', 'jpg']


def test_body_too_large(api, monkeypatch):
    import agent_api
    monkeypatch.setattr(agent_api, 'AGENT_MAX_BODY', 16)
    r = api.post('/agent/v1/sessions',
                 json={'image': 'A' * 64}, headers=MARKER)
    assert r.status_code == 413
    assert r.get_json()['code'] == 'too_large'


def test_ops_limit_returns_recipe(api, monkeypatch):
    import agent_pipeline as core
    monkeypatch.setattr(core, 'MAX_OPS_PER_SESSION', 1)
    sid = _create(api)
    r1 = api.post(f'/agent/v1/sessions/{sid}/transform', json={'scale': 1.1},
                  headers=MARKER)
    assert r1.status_code == 200
    r = api.post(f'/agent/v1/sessions/{sid}/transform', json={'scale': 1.2},
                 headers=MARKER)
    assert r.status_code == 429
    body = r.get_json()
    assert body['code'] == 'ops_exhausted'
    assert body['recipe'][0]['op'] == 'create'


def test_ttl_expiry_404(api):
    import agent_pipeline as core
    sid = _create(api)
    core.STORE._sessions[sid].last_used -= core.SESSION_TTL_MIN * 60 + 1
    r = api.get(f'/agent/v1/sessions/{sid}', headers=MARKER)
    assert r.status_code == 404


# ===== Информационные и превью =====

def test_manifest(api):
    r = api.get('/agent/v1/manifest', headers=MARKER)
    assert r.status_code == 200
    body = r.get_json()
    assert body['logical_size'] == 1024
    assert body['required_header']['X-Tokenatra-Agent'] == '1'


def test_assets_lists_catalogs(api):
    r = api.get('/agent/v1/assets', headers=MARKER)
    assert r.status_code == 200
    assert 'rings' in r.get_json() and 'presets' in r.get_json()


def test_tools_schema_openai_dialect(api):
    r = api.get('/agent/v1/tools_schema', headers=MARKER)
    assert r.status_code == 200
    tools = r.get_json()
    names = {t['function']['name'] for t in tools}
    assert all(t['type'] == 'function' for t in tools)
    assert 'tokenatra_create_session' in names
    assert 'tokenatra_export' in names


def test_preview_webp(api):
    sid = _create(api)
    r = api.get(f'/agent/v1/sessions/{sid}/preview?size=128', headers=MARKER)
    assert r.status_code == 200
    assert r.mimetype == 'image/webp'
    img = Image.open(io.BytesIO(r.get_data()))
    assert img.size == (128, 128)


def test_inspect_json(api):
    sid = _create(api)
    r = api.get(f'/agent/v1/sessions/{sid}/inspect', headers=MARKER)
    assert r.status_code == 200
    body = r.get_json()
    for key in ('coverage_pct', 'bbox', 'grid_3x3_pct', 'edge_px',
                'alpha_hist', 'warnings'):
        assert key in body
    assert 'image_data_url' not in body  # только по явному запросу


def test_inspect_include_image(api):
    sid = _create(api)
    r = api.get(f'/agent/v1/sessions/{sid}/inspect?include_image=1&preview_size=64',
                headers=MARKER)
    assert r.get_json()['image_data_url'].startswith('data:image/webp;base64,')
