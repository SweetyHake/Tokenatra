# -*- coding: utf-8 -*-
"""Политика агентной ветки гуарда (/agent/v1/*) — фаза 0.

Модель Blender MCP: без ключей и тумблеров. Доступ:
  local_tool    (нет Origin, нет Sec-Fetch-Site) + маркер -> разрешено
  browser_self  (Origin == scheme://host) + маркер      -> разрешено
  browser_foreign / connector-origin / lan / preflight  -> 403 всегда

X-Tokenatra-Agent: 1 — capability-маркер, не секрет: кросс-сайтовые
форма/no-cors его поставить не могут, а кастомный заголовок из чужого
браузерного Origin требует preflight, который здесь режется.

Роутов /agent/v1/* в этой фазе ещё нет, поэтому «гуард пропустил»
проверяем как 404 (маршрут не найден), «гуард зарезал» — как 403.
"""
MARKER = {'X-Tokenatra-Agent': '1'}
SELF_ORIGIN = 'http://localhost'  # тест-клиент шлёт Host без порта


def test_local_tool_with_marker_passes_guard(client):
    # Роуты появились в фазе 1: «гуард пропустил» видно по не-403 ответу
    # (здесь — 400 от валидации тела)
    r = client.post('/agent/v1/sessions', headers=MARKER)
    assert r.status_code == 400
    assert r.get_json()['code'] == 'bad_params'


def test_get_with_marker_passes_guard(client):
    assert client.get('/agent/v1/manifest', headers=MARKER).status_code == 200


def test_no_marker_blocked(client):
    assert client.post('/agent/v1/sessions').status_code == 403
    assert client.get('/agent/v1/manifest').status_code == 403


def test_marker_must_be_exact(client):
    r = client.post('/agent/v1/sessions',
                    headers={'X-Tokenatra-Agent': 'true'})
    assert r.status_code == 403


def test_foreign_origin_blocked_even_with_marker(client):
    r = client.post('/agent/v1/sessions',
                    headers={**MARKER, 'Origin': 'https://evil.com'})
    assert r.status_code == 403


def test_connector_origin_blocked(client):
    # Origin другого localhost-порта — для браузера same-site, но агентным
    # роутам не доверяем: маркер из чужой страницы = preflight = чужой Origin
    r = client.post('/agent/v1/sessions',
                    headers={**MARKER, 'Origin': 'http://localhost:30000'})
    assert r.status_code == 403


def test_same_origin_browser_with_marker_allowed(client):
    r = client.post('/agent/v1/sessions',
                    headers={**MARKER, 'Origin': SELF_ORIGIN})
    assert r.status_code == 400  # гуард пропустил, тело невалидно
    assert 'Access-Control-Allow-Origin' not in r.headers


def test_no_cors_subresource_blocked(client):
    # browser_odd: Origin нет, но Sec-Fetch-Site есть (img/form с чужого
    # сайта). Даже с маркером (спуфинг) — deny
    r = client.get('/agent/v1/sessions/x/preview',
                   headers={**MARKER, 'Sec-Fetch-Site': 'cross-site'})
    assert r.status_code == 403


def test_same_site_sec_fetch_allowed(client):
    r = client.get('/agent/v1/manifest',
                   headers={**MARKER, 'Sec-Fetch-Site': 'same-origin'})
    assert r.status_code == 200


def test_preflight_options_denied(client):
    r = client.options('/agent/v1/sessions', headers={
        **MARKER, 'Origin': 'https://evil.com',
        'Access-Control-Request-Method': 'POST',
    })
    assert r.status_code == 403


def test_foreign_host_denied_even_with_marker(client):
    # DNS rebinding: Host-проверка стоит раньше агентной ветки
    r = client.post('/agent/v1/sessions',
                    headers={**MARKER, 'Host': 'evil.com'})
    assert r.status_code == 403


def test_no_cors_headers_on_agent_routes(client):
    # Инвариант: агентные ответы не получают ACAO ни при каких Origin
    r = client.post('/agent/v1/sessions',
                    headers={**MARKER, 'Origin': 'https://evil.com'})
    assert 'Access-Control-Allow-Origin' not in r.headers
    r2 = client.get('/agent/v1/manifest', headers=MARKER)
    assert 'Access-Control-Allow-Origin' not in r2.headers


def test_agent_prefix_only(client):
    # Ветка режет только /agent/*: остальные роуты живут по старым правилам
    assert client.get('/health').status_code == 200
    assert client.post('/shutdown').status_code == 403
