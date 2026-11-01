# -*- coding: utf-8 -*-
"""Тесты MCP stdio-прокси: реальный subprocess, реальный HTTP-сервер
(Flask test-сервер на свободном порту через werkzeug.run_simple).

DoD фазы 2 (арх. ревью): initialize-хендшейк, tools/list, tools/call,
notifications без ответа, stdout — только JSON-RPC, UTF-8.
"""
import base64
import io
import json
import socket
import subprocess
import sys
import threading
import time

import pytest
from PIL import Image, ImageDraw
from werkzeug.serving import make_server

MCP = 'tokenatra_mcp.py'


def _free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


@pytest.fixture()
def backend(tmp_path, monkeypatch):
    """Живой Flask на свободном порту; remove_bg замокан."""
    import server
    import agent_pipeline as core
    monkeypatch.setattr(server, 'USER_DATA_DIR', tmp_path)
    monkeypatch.setattr(server, 'MODELS_DIR', tmp_path / 'models')
    core.STORE._sessions.clear()

    def fake(image, edge_blur=1):
        import numpy as np
        arr = np.array(image.convert('RGBA'))
        red = arr[:, :, 0].astype(int) - arr[:, :, 1].astype(int)
        arr[:, :, 3] = (red > 60).astype(np.uint8) * 255
        return Image.fromarray(arr, 'RGBA')
    monkeypatch.setattr(server, 'remove_background', fake)

    srv = make_server('127.0.0.1', 0, server.app, threaded=True)
    port = srv.server_port
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    yield f'http://127.0.0.1:{port}'
    srv.shutdown()
    core.STORE._sessions.clear()


class McpClient:
    def __init__(self, url):
        self.proc = subprocess.Popen(
            [sys.executable, MCP],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding='utf-8',
            env={**__import__('os').environ, 'TOKENATRA_URL': url},
            cwd=__import__('os').getcwd(),
        )

    def send(self, obj):
        self.proc.stdin.write(json.dumps(obj) + '\n')
        self.proc.stdin.flush()

    def recv(self, timeout=15):
        # построчно с таймаутом через поток-обёртку не выйдет: читаем в треде
        import queue
        q = queue.Queue()

        def reader():
            q.put(self.proc.stdout.readline())
        t = threading.Thread(target=reader, daemon=True)
        t.start()
        try:
            return q.get(timeout=timeout)
        except queue.Empty:
            raise AssertionError('MCP did not respond in time')

    def call(self, method, params=None, req_id=None):
        msg = {'jsonrpc': '2.0', 'method': method}
        if params is not None:
            msg['params'] = params
        if req_id is not None:
            msg['id'] = req_id
        self.send(msg)
        return json.loads(self.recv())

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


@pytest.fixture()
def mcp(backend):
    client = McpClient(backend)
    yield client
    client.close()


def _b64():
    img = Image.new('RGBA', (96, 96), (10, 10, 10, 255))
    ImageDraw.Draw(img).rectangle([8, 8, 87, 87], fill=(200, 30, 30, 255))
    buf = io.BytesIO()
    img.save(buf, format='PNG')
    return 'data:image/png;base64,' + base64.b64encode(buf.getvalue()).decode()


# ===== Хендшейк и листинг =====

def test_initialize_handshake_and_version_negotiation(mcp):
    r = mcp.call('initialize', {'protocolVersion': '2024-11-05',
                                'capabilities': {},
                                'clientInfo': {'name': 'test'}}, req_id=1)
    assert r['id'] == 1
    assert r['result']['protocolVersion'] == '2024-11-05'
    assert r['result']['serverInfo']['name'] == 'tokenatra'
    # неизвестная версия -> наша последняя
    r2 = mcp.call('initialize', {'protocolVersion': '1999-01-01'}, req_id=2)
    assert r2['result']['protocolVersion'] == '2025-06-18'


def test_initialized_notification_gets_no_response(mcp):
    mcp.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
    time.sleep(0.3)
    # единственный способ проверить «нет ответа» — следующий запрос
    # получает ответ на СЕБЯ, а не на notification
    r = mcp.call('ping', req_id=42)
    assert r['id'] == 42 and r['result'] == {}


def test_ping(mcp):
    r = mcp.call('ping', req_id=7)
    assert r['result'] == {}


def test_tools_list_has_expected_shape(mcp):
    r = mcp.call('tools/list', req_id=3)
    tools = r['result']['tools']
    names = {t['name'] for t in tools}
    assert 'tokenatra_create_session' in names
    assert 'tokenatra_export' in names
    for t in tools:
        assert t['inputSchema']['type'] == 'object'


def test_unknown_method_error(mcp):
    r = mcp.call('no/such/method', req_id=9)
    assert r['error']['code'] == -32601


# ===== Полный цикл через MCP =====

def test_full_cycle_create_inspect_export(mcp, tmp_path):
    r = mcp.call('tools/call', {'name': 'tokenatra_create_session',
                                'arguments': {'image_b64': _b64()}}, req_id=10)
    assert not r['result'].get('isError'), r
    text = r['result']['content'][0]['text']
    sid = text.split('session_id=')[1].split()[0]

    r = mcp.call('tools/call', {'name': 'tokenatra_remove_background',
                                'arguments': {'session_id': sid}}, req_id=11)
    assert not r['result'].get('isError')

    r = mcp.call('tools/call', {'name': 'tokenatra_inspect',
                                'arguments': {'session_id': sid,
                                              'include_image': True,
                                              'preview_size': 128}}, req_id=12)
    types = [c['type'] for c in r['result']['content']]
    assert 'text' in types and 'image' in types
    insp = json.loads(r['result']['content'][0]['text'])
    assert 'coverage_pct' in insp
    img_block = r['result']['content'][1]
    assert img_block['mimeType'] == 'image/webp'
    assert base64.b64decode(img_block['data'])[:4] == b'RIFF'

    r = mcp.call('tools/call', {'name': 'tokenatra_export',
                                'arguments': {'session_id': sid, 'size': 256,
                                              'scale_mode': '1',
                                              'save_to': str(tmp_path)}},
                 req_id=13)
    text = r['result']['content'][0]['text']
    assert text.startswith('Saved:')
    from pathlib import Path
    saved = Path(text.split('Saved: ')[1])
    assert saved.exists() and saved.suffix == '.webp'


def test_tool_error_surface_as_iserror(mcp):
    r = mcp.call('tools/call', {'name': 'tokenatra_set_ring',
                                'arguments': {'session_id': 'deadbeef',
                                              'name': 'x'}}, req_id=20)
    assert r['result']['isError'] is True
    body = json.loads(r['result']['content'][0]['text'])
    assert body['code'] == 'session_not_found'


def test_app_down_is_actionable(backend):
    mcp = McpClient('http://127.0.0.1:1')  # ничего не слушает
    try:
        r = mcp.call('tools/call', {'name': 'tokenatra_manifest'}, req_id=1)
        assert r['result']['isError'] is True
        assert 'not running' in r['result']['content'][0]['text']
    finally:
        mcp.close()


# Контракт «stdout — только JSON-RPC, логи — в stderr» проверяется
# неявно каждым recv(): любая посторонняя строка сломала бы json.loads.
# Отдельного теста не требуется.


def test_resources(mcp):
    r = mcp.call('resources/list', req_id=2)
    uris = {res['uri'] for res in r['result']['resources']}
    assert 'tokenatra://manifest' in uris
    r2 = mcp.call('resources/read', {'uri': 'tokenatra://manifest'}, req_id=3)
    manifest = json.loads(r2['result']['contents'][0]['text'])
    assert manifest['logical_size'] == 1024
