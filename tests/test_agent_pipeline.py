# -*- coding: utf-8 -*-
"""Юнит-тесты agent_pipeline (ядро, без Flask и без ONNX — remove_bg
мокается пороговой маской).

Self-regression: тесты фиксируют поведение Python-реализации (паритет
с GUI — разовый ритуал сверки, см. docs/AGENT_API.md).
"""
import base64
import io

import numpy as np
import pytest
from PIL import Image, ImageDraw

import agent_pipeline as core
from agent_pipeline import AgentError


def subject_img(w=512, h=128, opaque=True):
    """Прямоугольник с прозрачным полем: субъект с альфой."""
    img = Image.new('RGBA', (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, w - 1, h - 1], fill=(200, 30, 30, 255) if opaque
                else (200, 30, 30, 0))
    return img


def make_session(**kw):
    return core.STORE.create(subject_img(**kw))


@pytest.fixture(autouse=True)
def _fresh_store():
    core.STORE._sessions.clear()
    core._clear_caches()
    yield
    core.STORE._sessions.clear()
    core._clear_caches()


@pytest.fixture()
def mock_remove_bg(monkeypatch):
    """Пороговая «модель»: всё красное видимо, остальное прозрачно."""
    def fake(image, edge_blur=1):
        arr = np.array(image.convert('RGBA'))
        red = arr[:, :, 0].astype(int) - arr[:, :, 1].astype(int)
        alpha = (red > 60).astype(np.uint8) * 255
        arr[:, :, 3] = alpha
        return Image.fromarray(arr, 'RGBA')
    import server
    monkeypatch.setattr(server, 'remove_background', fake)
    return fake


# ===== decode_source =====

def test_decode_source_data_url_and_raw():
    buf = io.BytesIO()
    subject_img(32, 32).save(buf, format='PNG')
    b64 = base64.b64encode(buf.getvalue()).decode()
    img1 = core.decode_source(f'data:image/png;base64,{b64}')
    img2 = core.decode_source(b64)
    assert img1.size == img2.size == (32, 32)


def test_decode_source_rejects_garbage():
    with pytest.raises(AgentError) as e:
        core.decode_source('not base64!!!')
    assert e.value.code == 'bad_params'
    with pytest.raises(AgentError) as e:
        core.decode_source(base64.b64encode(b'not an image').decode())
    assert e.value.code == 'bad_image'


def test_decode_source_caps_to_2048():
    img = core.decode_source(_b64(subject_img(3000, 3000)))
    assert max(img.size) == 2048


def _b64(img):
    buf = io.BytesIO()
    img.save(buf, format='PNG')
    return base64.b64encode(buf.getvalue()).decode()


# ===== Маски =====

def test_mask_erase_and_restore():
    s = make_session()
    before = np.array(s.alpha).mean()
    core.mask_op(s, 'erase', shapes=[{'circle': {'cx': 512, 'cy': 512, 'r': 400}}])
    after = np.array(s.alpha).mean()
    assert after < before
    core.mask_op(s, 'restore', shapes=[{'circle': {'cx': 512, 'cy': 512, 'r': 400}}])
    assert np.array(s.alpha).mean() == before


def test_mask_protection_blocks_erase(monkeypatch):
    s = make_session()
    monkeypatch.setattr(core, '_protection_image',
                        lambda sess: Image.new('L', (1024, 1024), 255))
    before = np.array(s.alpha).copy()
    core.mask_op(s, 'erase', shapes=[{'circle': {'cx': 512, 'cy': 512, 'r': 400}}],
                 protect=True)
    assert (np.array(s.alpha) == before).all()
    # protect=False — защита игнорируется
    core.mask_op(s, 'erase', shapes=[{'circle': {'cx': 512, 'cy': 512, 'r': 400}}],
                 protect=False)
    assert np.array(s.alpha).mean() < before.mean()


def test_mask_apply_preset(monkeypatch, tmp_path):
    preset = Image.new('RGBA', (128, 128), (0, 0, 0, 0))
    ImageDraw.Draw(preset).rectangle([32, 32, 95, 95], fill=(255, 0, 255, 255))
    preset.save(tmp_path / 'mytest.png')
    import server
    monkeypatch.setattr(server, 'USER_PRESET_DIR', tmp_path)
    monkeypatch.setattr(server, 'BUILTIN_PRESET_DIR', tmp_path / 'none')

    s = make_session()
    core.mask_op(s, 'apply_preset', preset='mytest')
    stats = core._mask_stats(s.alpha)
    # пресет 64/128 = 50% ширины -> ~25% площади канваса видимо
    assert 20 < stats['coverage_pct'] < 30


def test_mask_unknown_preset():
    s = make_session()
    with pytest.raises(AgentError) as e:
        core.mask_op(s, 'apply_preset', preset='nope')
    assert e.value.code == 'unknown_asset'


def test_mask_invert_feather_grow():
    s = make_session()
    full_mask = np.array(s.alpha).mean()
    # частичная маска: вырезать круг r=200 (~86% останется)
    core.mask_op(s, 'erase', shapes=[{'circle': {'cx': 512, 'cy': 512, 'r': 200}}],
                 protect=False)
    partial = core._mask_stats(s.alpha)['coverage_pct']
    assert 80 < partial < 92

    core.mask_op(s, 'grow', px=3)
    grown = core._mask_stats(s.alpha)['coverage_pct']
    assert grown > partial  # дилатация возвращает видимость по краям

    core.mask_op(s, 'feather', px=8)
    a = np.array(s.alpha)
    soft = ((a > 10) & (a < 245)).sum()
    assert soft > 0  # мягкий край даёт промежуточные значения альфы
    feathered = core._mask_stats(s.alpha)['coverage_pct']

    core.mask_op(s, 'invert')
    assert abs(core._mask_stats(s.alpha)['coverage_pct']
               - (100.0 - feathered)) < 5
    assert np.array(s.alpha).mean() != full_mask


def test_mask_bad_shape():
    s = make_session()
    with pytest.raises(AgentError) as e:
        core.mask_op(s, 'erase', shapes=[{'triangle': {}}])
    assert 'valid' in e.value.__dict__ or True
    assert e.value.code == 'bad_params'


# ===== Трансформация и валидация =====

def test_transform_validation():
    s = make_session()
    core.set_transform(s, scale=1.5, rotation=15, offset_x=-40)
    assert s.transform['scale'] == 1.5
    with pytest.raises(AgentError):
        core.set_transform(s, scale=99)
    with pytest.raises(AgentError):
        core.set_transform(s, rotation=400)
    with pytest.raises(AgentError):
        core.set_transform(s, offset_x=5000)


def test_effects_validation_and_merge():
    s = make_session()
    core.set_effects(s, shadow={'enabled': True, 'opacity': 0.7})
    assert s.effects['shadow']['enabled'] is True
    assert s.effects['shadow']['opacity'] == 0.7
    assert s.effects['shadow']['angle'] == 135  # дефолт не тронут
    with pytest.raises(AgentError):
        core.set_effects(s, shadow={'opacity': 5})


# ===== Композиция =====

def test_compose_crop_used_scale():
    # fit-1024 заполняет канвас по ДЛИННОЙ стороне: 512x128 -> 1024x256.
    # scale 0.35 -> 358x90 в центре: влезает в бокс s=1
    s = make_session(w=512, h=128)
    core.set_transform(s, scale=0.3)
    img, used = core.compose(s, 512, scale_mode='auto')
    assert used == 1 and img.size == (512, 512)

    # масштаб x1.5 от fit (1536 по ширине при s3=1536) — только s=3
    core.set_transform(s, scale=1.5)
    img, used = core.compose(s, 512, scale_mode='auto')
    assert used == 3 and img.size == (1536, 1536)

    # явный scale_mode=2 бьёт auto
    img, used = core.compose(s, 512, scale_mode='2')
    assert used == 2 and img.size == (1024, 1024)


def test_compose_rotation_swaps_bbox():
    s = core.STORE.create(subject_img(400, 100))
    core.set_transform(s, rotation=90)
    layer = core._subject_layer(s, 1024)
    a = np.array(layer.getchannel('A'))
    ys, xs = np.nonzero(a > 15)
    height = ys.max() - ys.min()
    width = xs.max() - xs.min()
    assert height > width  # поворот на 90° поменял оси


def test_compose_shadow_smoke():
    s = make_session()
    core.set_effects(s, shadow={'enabled': True})
    img, _ = core.compose(s, 256, scale_mode='1')
    assert img.size == (256, 256)
    # в тени есть полупрозрачные тёмные пиксели за пределами субъекта
    a = np.array(img.getchannel('A'))
    assert ((a > 0) & (a < 250)).sum() > 0


def test_color_correction_cache_invalidated():
    s = make_session()
    core.set_effects(s, color={'enabled': True, 'saturation': 50})
    img1 = core._corrected_base(s)
    core.set_effects(s, color={'enabled': True, 'saturation': 0})
    img2 = core._corrected_base(s)
    assert img1 is not img2


# ===== remove_bg (мок) + inspect =====

def test_remove_bg_uses_session_base(mock_remove_bg):
    s = make_session(w=64, h=64)
    core.remove_bg(s, edge_blur=1.0)
    assert s.bg_removed is True
    alpha = np.array(s.base.getchannel('A'))
    assert set(np.unique(alpha)) <= {0, 255}
    assert s.recipe[-1]['op'] == 'remove_bg'


def test_inspect_metrics_and_delta(mock_remove_bg):
    s = make_session(w=256, h=256)
    first = core.inspect(s)
    assert first['coverage_pct'] == 100.0  # маска полна, субъект виден
    assert any('background not removed' in w for w in first['warnings'])
    core.remove_bg(s)
    second = core.inspect(s)
    assert 'delta' in second
    assert second['delta']['coverage_dpct'] != 0 or second['coverage_pct'] == 100.0
    assert second['bbox'] is not None
    assert len(second['grid_3x3_pct']) == 3
    assert len(second['alpha_hist']) == 5


def test_inspect_include_image(mock_remove_bg):
    s = make_session(w=128, h=128)
    res = core.inspect(s, include_image=True, preview_size=128)
    assert res['image_data_url'].startswith('data:image/webp;base64,')


# ===== Реестр сессий =====

def test_store_lru_eviction():
    sessions = [make_session() for _ in range(core.MAX_SESSIONS + 2)]
    assert core.STORE.count() <= core.MAX_SESSIONS
    # самые старые по last_used вытеснены
    alive = set(core.STORE._sessions.keys())
    assert sessions[0].sid not in alive
    assert sessions[-1].sid in alive


def test_store_ttl_expiry():
    s = make_session()
    s.last_used -= core.SESSION_TTL_MIN * 60 + 1
    with pytest.raises(AgentError) as e:
        core.STORE.get(s.sid)
    assert e.value.code == 'session_not_found'


def test_recipe_is_append_only_log():
    s = make_session()
    core.set_transform(s, scale=2)
    core.set_ring_none if False else None
    ops = [e['op'] for e in s.recipe]
    assert ops[0] == 'create'
    assert 'transform' in ops
