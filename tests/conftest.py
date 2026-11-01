# -*- coding: utf-8 -*-
"""Общие фикстуры тестов Tokenatra.

Импорт server безопасен (сессия ONNX не грузится на импорте), но любые
записи (POST /config и т.п.) обязаны идти в изолированный каталог —
патчим USER_DATA_DIR на tmp_path. config_file() читает глобал в момент
вызова, поэтому monkeypatch на модуль достаточен.
"""
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))


@pytest.fixture()
def server(tmp_path, monkeypatch):
    import server as server_module

    monkeypatch.setattr(server_module, 'USER_DATA_DIR', tmp_path)
    monkeypatch.setattr(server_module, 'MODELS_DIR', tmp_path / 'models')
    return server_module


@pytest.fixture()
def client(server):
    server.app.config['TESTING'] = True
    return server.app.test_client()


@pytest.fixture()
def write_config(server):
    """Записать изолированный config.json и вернуть его содержимое."""

    def _write(cfg):
        server.config_file().write_text(
            __import__('json').dumps(cfg, ensure_ascii=False), encoding='utf-8')
        return cfg

    return _write
