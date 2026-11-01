# -*- coding: utf-8 -*-
"""Characterization-тесты _guard_localhost_only (server.py).

Фиксируют ТЕКУЩЕЕ поведение гуарда до каких-либо изменений агентной
части (план Agent API, фаза 0). Правки гуарда обязаны держать эти
тесты зелёными; осознанные изменения поведения — отдельным коммитом
вместе с правкой соответствующих проверок.

Класс клиента определяется парой заголовков:
  browser_self    Origin == scheme://host (само приложение)
  browser_foreign Origin чужой (сайт в браузере пользователя)
  connector       Origin localhost-семейства, любой порт (Foundry)
  local_tool      без Origin и без Sec-Fetch-Site (curl/скрипты)

Фактическая поза (зафиксирована тестами): allow_browser=True (дефолт)
открывает коннекторные роуты, UI-ассеты и /config ВСЕМ клиентам (любой
Origin, без Origin, чужой localhost-порт). Строгие проверки (Origin ==
self, Sec-Fetch-Site, no-Origin-403) защищают только системные роуты
вне allow_browser-списка: /shutdown, /apply_update, /api/window/*,
/save_to_folder и т.п.
"""
import json

SELF_ORIGIN = 'http://localhost:7878'
EVIL_ORIGIN = 'https://evil.com'
CONNECTOR_ORIGIN = 'http://localhost:30000'


# ===== browser_self: полный доступ =====

def test_ui_get_same_origin(client):
    r = client.get('/', headers={'Origin': SELF_ORIGIN})
    assert r.status_code == 200


def test_config_post_same_origin_allowed(client):
    r = client.post('/config', json={'theme': 'dark'},
                    headers={'Origin': SELF_ORIGIN})
    assert r.status_code == 200
    assert r.get_json()['ok'] is True


def test_config_post_same_origin_but_cross_site_header_allowed__allow_browser(client):
    # Документация фактического поведения: allow_browser-ветка (293-295)
    # пропускает /config РАНЬШЕ страховки Sec-Fetch-Site (299-301) —
    # для /config и коннекторных роутов эта страховка сегодня мёртвый код
    r = client.post('/config', json={'theme': 'dark'},
                    headers={'Origin': SELF_ORIGIN,
                             'Sec-Fetch-Site': 'cross-site'})
    assert r.status_code == 200


# ===== local_tool (без Origin) =====
# Инверсия «no Origin => 403 на POST» действует ТОЛЬКО для системных
# роутов вне allow_browser-списка (shutdown, apply_update, api/window,
# save_to_folder). Коннекторные роуты и /config без Origin проходят.

def test_ui_get_no_origin_allowed(client):
    assert client.get('/').status_code == 200


def test_shutdown_post_no_origin_blocked(client):
    assert client.post('/shutdown').status_code == 403


def test_config_post_no_origin_allowed__allow_browser(client):
    # /config в allow_browser-списке — POST без Origin проходит гуард
    r = client.post('/config', json={'theme': 'dark'})
    assert r.status_code == 200


def test_connector_post_no_origin_passes_guard__allow_browser(client):
    # Гуард пропускает (allow_browser); отказ приходит от хендлера (400)
    r = client.post('/embed_export')
    assert r.status_code == 400


# ===== browser_foreign: allow_browser=True по умолчанию =====

def test_connector_route_foreign_origin_get_allowed_with_acao(client):
    # allow_browser (дефолт) открывает коннекторные роуты любому сайту
    # и отдаёт читаемый CORS — зафиксировано как осознанная поза
    r = client.get('/health', headers={'Origin': EVIL_ORIGIN})
    assert r.status_code == 200
    assert r.headers.get('Access-Control-Allow-Origin') == EVIL_ORIGIN


def test_ui_asset_foreign_origin_get_allowed(client):
    r = client.get('/presets_list', headers={'Origin': EVIL_ORIGIN})
    assert r.status_code == 200


def test_config_get_foreign_origin_strips_shared_secret(client, write_config):
    write_config({'shared_secret': 'top-secret', 'theme': 'dark'})
    r = client.get('/config', headers={'Origin': EVIL_ORIGIN})
    assert r.status_code == 200
    body = r.get_json()
    assert 'shared_secret' not in body
    assert body.get('theme') == 'dark'


def test_config_get_same_origin_keeps_shared_secret(client, write_config):
    write_config({'shared_secret': 'top-secret'})
    r = client.get('/config', headers={'Origin': SELF_ORIGIN})
    assert r.get_json().get('shared_secret') == 'top-secret'


# ===== connector: Origin localhost-семейства на коннекторных роутах =====

def test_connector_origin_post_passes_guard(client):
    # Гуард пропускает; отказ приходит уже от хендлера (нет файла) — 400
    r = client.post('/embed_export', headers={'Origin': CONNECTOR_ORIGIN})
    assert r.status_code == 400


def test_connector_origin_post_non_connector_route_allowed__allow_browser(client):
    # localhost:30000 -> localhost:7878 для браузера same-site, но не
    # same-origin. Строгая Origin-проверка (304-310) защищает только
    # системные роуты; /config в allow_browser-списке и проходит.
    r = client.post('/config', json={'theme': 'dark'},
                    headers={'Origin': CONNECTOR_ORIGIN})
    assert r.status_code == 200


def test_connector_preflight_options(client):
    r = client.options('/process', headers={
        'Origin': CONNECTOR_ORIGIN,
        'Access-Control-Request-Method': 'POST',
    })
    assert r.status_code == 200
    assert r.headers.get('Access-Control-Allow-Origin') == CONNECTOR_ORIGIN


# ===== DNS rebinding: чужой Host режется первым =====

def test_foreign_host_blocked(client):
    assert client.get('/', headers={'Host': 'evil.com'}).status_code == 403


def test_foreign_host_with_matching_origin_still_blocked(client):
    # rebinding: Origin совпадает с scheme://host, но Host не localhost
    r = client.post('/config', json={'theme': 'dark'},
                    headers={'Host': 'evil.com', 'Origin': 'http://evil.com'})
    assert r.status_code == 403


# ===== Сетевой режим (listen_host=0.0.0.0) =====

def test_network_mode_public_route_allowed_for_lan(client, write_config):
    write_config({'listen_host': '0.0.0.0'})
    r = client.get('/health', headers={'Host': 'evil.com:1234'})
    assert r.status_code == 200


def test_network_mode_system_route_blocked_for_lan(client, write_config):
    write_config({'listen_host': '0.0.0.0'})
    r = client.post('/shutdown', headers={'Host': 'evil.com:1234'})
    assert r.status_code == 403


# ===== Прочее стабильное поведение =====

def test_health_no_origin_get_allowed(client):
    assert client.get('/health').status_code == 200


def test_config_post_invalid_body_same_origin(client):
    r = client.post('/config', data='not-json',
                    headers={'Origin': SELF_ORIGIN})
    assert r.status_code == 400
