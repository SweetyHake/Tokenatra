# -*- coding: utf-8 -*-
"""Тесты security-фиксов фазы 0 (по итогам аудита Agent API).

1. /save_to_folder: allowlist расширений — имя файла приходит из формы
   и не должно превращать эндпоинт в «запиши .bat в разрешённую папку».
   Цепочка из аудита: чужой сайт переписывает lastFolders.quickSave в
   Startup-папку через POST /config, дальше quick-save (или агентный
   экспорт) молча кладёт туда token.bat.
2. POST /config с чужого Origin не может подменить lastFolders
   (и не теряет существующее значение).
"""
import io
import json

from pathlib import Path

SELF_ORIGIN = 'http://localhost'  # тест-клиент Werkzeug шлёт Host без порта
EVIL_ORIGIN = 'https://evil.com'


def _post_file(client, folder, filename, content=b'x'):
    data = {
        'folder': str(folder),
        'filename': filename,
        'file': (io.BytesIO(content), filename),
    }
    # save_to_folder — системный роут: гуард требует same-origin Origin
    return client.post('/save_to_folder', data=data,
                       headers={'Origin': SELF_ORIGIN})


def test_save_to_folder_accepts_webp(client, tmp_path, write_config):
    write_config({'lastFolders': {'quickSave': str(tmp_path)}})
    r = _post_file(client, tmp_path, 'token.webp', b'webp-bytes')
    assert r.status_code == 200
    assert (tmp_path / 'token.webp').read_bytes() == b'webp-bytes'


def test_save_to_folder_rejects_bat(client, tmp_path, write_config):
    write_config({'lastFolders': {'quickSave': str(tmp_path)}})
    r = _post_file(client, tmp_path, 'token.bat', b'@echo off')
    assert r.status_code == 400
    assert not (tmp_path / 'token.bat').exists()


def test_save_to_folder_rejects_double_extension(client, tmp_path, write_config):
    write_config({'lastFolders': {'quickSave': str(tmp_path)}})
    # «token.bat.webp» — валидное имя картинки; а вот «token.webp.bat»
    # и трюки с нулевым байтом должны отрезаться по суффиксу
    r = _post_file(client, tmp_path, 'token.webp.bat')
    assert r.status_code == 400
    r = _post_file(client, tmp_path, 'noext')
    assert r.status_code == 400


def test_config_post_foreign_origin_cannot_swap_lastfolders(client, write_config):
    write_config({'lastFolders': {'quickSave': 'C:/legit/quick'}})
    r = client.post('/config', json={'lastFolders': {
        'quickSave': 'C:/Users/x/AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup',
    }}, headers={'Origin': EVIL_ORIGIN})
    assert r.status_code == 200  # POST /config чужому Origin разрешён (allow_browser)
    cfg = json.loads(client.get('/config').get_data())
    assert cfg['lastFolders']['quickSave'] == 'C:/legit/quick'


def test_config_post_same_origin_updates_lastfolders(client, write_config):
    write_config({'lastFolders': {'quickSave': 'C:/old'}})
    r = client.post('/config', json={'lastFolders': {'quickSave': 'C:/new'}},
                    headers={'Origin': SELF_ORIGIN})
    assert r.status_code == 200
    cfg = json.loads(client.get('/config').get_data())
    assert cfg['lastFolders']['quickSave'] == 'C:/new'
