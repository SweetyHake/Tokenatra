#!/usr/bin/env python3
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import warnings
import ctypes
from ctypes import wintypes
import argparse
import threading
import numpy as np
from pathlib import Path
from flask import Flask, request, render_template, jsonify, send_file, current_app
from PIL import Image, ImageFilter, ImageDraw
import onnxruntime as ort
from version import __version__, APP_NAME, GITHUB_REPO, MODEL_DOWNLOAD_URL
from platform_utils import user_data_dir
from updater import (
    get_status as updater_status,
    start_background_tasks,
    download_update,
    check_for_updates,
    reset_check_state,
)

warnings.filterwarnings('ignore')


def _resolve_resource_dir():
    """Find bundled read-only resources in source and PyInstaller builds."""
    configured = os.environ.get('TOKENATRA_RESOURCE_DIR')
    if configured:
        return Path(configured)

    candidates = []
    if getattr(sys, 'frozen', False):
        meipass = getattr(sys, '_MEIPASS', None)
        if meipass:
            candidates.append(Path(meipass))

        executable_dir = Path(sys.executable).resolve().parent
        if sys.platform == 'darwin' and executable_dir.name == 'MacOS':
            candidates.append(executable_dir.parent / 'Resources')
        candidates.append(executable_dir)
    else:
        candidates.append(Path(__file__).resolve().parent)

    for candidate in candidates:
        if (candidate / 'templates').is_dir() and (candidate / 'static').is_dir():
            return candidate
    return candidates[0]


RESOURCE_DIR = _resolve_resource_dir()
app = Flask(
    __name__,
    template_folder=str(RESOURCE_DIR / 'templates'),
    static_folder=str(RESOURCE_DIR / 'static'),
)
app.config['MAX_CONTENT_LENGTH'] = 50 * 1024 * 1024
# Статика не кэшируется: после обновления приложения WebView2 не должен
# подхватывать старые JS (12-часовой дефолт Flask давал микс старых/новых файлов)
app.config['SEND_FILE_MAX_AGE_DEFAULT'] = 0


@app.before_request
def _guard_localhost_only():
    """Защита от CSRF/DNS-rebinding: API доступен только с localhost.
    Внешние страницы (Origin != localhost) и чужие Host отклоняются."""
    try:
        host = (request.host or '').lower()
        # '[::1]:7878' -> '::1'
        host_name = host.rsplit(':', 1)[0]
        if host_name.startswith('['):
            host_name = host_name.lstrip('[')
        if host_name not in ('127.0.0.1', 'localhost', '::1'):
            return jsonify({'error': 'Forbidden'}), 403
        state_changing = request.method in ('POST', 'PUT', 'DELETE', 'PATCH')
        # Chromium присылает Sec-Fetch-Site всегда; кросс-сайтовые state-changing
        # запросы режем сразу (страховка от будущих дыр в Origin-проверке)
        sec_fetch_site = request.headers.get('Sec-Fetch-Site', '').lower()
        if state_changing and sec_fetch_site and sec_fetch_site not in ('same-origin', 'same-site', 'none'):
            return jsonify({'error': 'Forbidden'}), 403
        origin = request.headers.get('Origin', '')
        if origin:
            from urllib.parse import urlparse
            oh = urlparse(origin).netloc.lower()
            oh_name = oh.rsplit(':', 1)[0]
            if oh_name.startswith('['):
                oh_name = oh_name.lstrip('[')
            # Пустой netloc (Origin: null из sandboxed iframe / data:-документа)
            # НЕ пропускаем: это вектор CSRF на /save_to_folder и другие POST.
            # Легитимное приложение шлёт Origin http://127.0.0.1:7878 либо не шлёт вовсе.
            if oh_name not in ('127.0.0.1', 'localhost', '::1'):
                return jsonify({'error': 'Forbidden'}), 403
        elif state_changing:
            # State-changing запросы без Origin (не-браузерный клиент)
            return jsonify({'error': 'Forbidden'}), 403
    except Exception:
        return jsonify({'error': 'Forbidden'}), 403
    return None

if getattr(sys, 'frozen', False):
    # A portable build launched from the source tree keeps user data in the
    # project root; an installed build keeps it next to the executable.
    BASE_DIR = Path(os.environ.get('TOKENMAKER_DIR', Path(sys.executable).parent))
else:
    BASE_DIR = Path(os.environ.get('TOKENMAKER_DIR', Path(__file__).parent))


def _resolve_user_data_dir():
    """Каталог для пользовательских данных (models/, config.json).
    Установленные сборки лежат в Program Files и недоступны для записи
    обычному пользователю — переходим в LOCALAPPDATA (как в updater.py)."""
    candidates = [user_data_dir(), BASE_DIR]
    for cand in candidates:
        try:
            cand.mkdir(parents=True, exist_ok=True)
            probe = cand / '.tokenatra_write_probe'
            probe.write_text('x', encoding='utf-8')
            probe.unlink()
            return cand
        except OSError:
            continue
    fallback = Path(tempfile.gettempdir()) / "Tokenatra"
    fallback.mkdir(parents=True, exist_ok=True)
    return fallback


def config_file():
    return USER_DATA_DIR / 'config.json'


USER_DATA_DIR = _resolve_user_data_dir()
MODELS_DIR = USER_DATA_DIR / "models"
# Встроенные кольца живут рядом с приложением (read-only в Program Files),
# пользовательские — в LOCALAPPDATA (куда разрешена запись).
BUILTIN_RING_DIR = RESOURCE_DIR / "token_rings"
RING_DIR = USER_DATA_DIR / "token_rings"
MASK_PATH = RESOURCE_DIR / "mask.png"

# Встроенные кольца: имя файла (casefold) → отображаемое имя.
# Имя — исходный русский текст; в UI переводится через I18n.t().
BUILTIN_RINGS = {
    'steel.webp': 'Сталь',
    'ag-obsidian-red.webp': 'Красный обсидиан',
    'ag-stormsteel.webp': 'Штормовая сталь',
    'ag-wood-runic.png': 'Руническое дерево',
}
PRESET_DIR = RESOURCE_DIR
MODEL_DOWNLOAD_URL = os.environ.get('TOKENATRA_MODEL_URL', MODEL_DOWNLOAD_URL).strip()


def _migrate_legacy_model():
    """Если model.onnx лежит в папке приложения — перемещаем в models/ (разово)."""
    legacy = BASE_DIR / "model.onnx"
    if not legacy.exists():
        return
    try:
        MODELS_DIR.mkdir(exist_ok=True)
        dest = MODELS_DIR / "model.onnx"
        if not dest.exists():
            os.replace(str(legacy), str(dest))
            print(f" Модель перемещена: model.onnx -> {MODELS_DIR.name}/")
        else:
            legacy.unlink(missing_ok=True)
    except Exception as e:
        print(f" Не удалось переместить модель: {e}")


def get_selected_model_path():
    """Путь к выбранной модели: config selected_model -> первая .onnx в models/."""
    try:
        cfg_path = config_file()
        if cfg_path.exists():
            cfg = json.loads(cfg_path.read_text(encoding='utf-8'))
            sel = cfg.get('selected_model')
            if sel:
                p = MODELS_DIR / Path(str(sel)).name
                if p.exists():
                    return p
    except Exception:
        pass
    try:
        onnx_files = sorted(MODELS_DIR.glob('*.onnx'))
        if onnx_files:
            return onnx_files[0]
    except Exception:
        pass
    # Also read models from the application directory for portable builds.
    legacy = BASE_DIR / "models"
    if legacy.resolve() != MODELS_DIR.resolve():
        try:
            onnx_files = sorted(legacy.glob('*.onnx'))
            if onnx_files:
                return onnx_files[0]
        except Exception:
            pass
    return MODELS_DIR / "model.onnx"


def _activate_model(path):
    """Сохраняет модель выбранной и сбрасывает кеш ONNX-сессии."""
    global SESSION

    def _mutate(cfg):
        cfg['selected_model'] = path.name
        return cfg

    _update_config(_mutate)
    with SESSION_LOCK:
        SESSION = None


_migrate_legacy_model()
ONNX_PATH = get_selected_model_path()

ALLOWED_EXTENSIONS = {'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'avif'}
MEDIA_EXTENSIONS = {
    'mp3', 'wav', 'm4a', 'flac', 'aac', 'wma', 'opus', 'ogg',
    'mp4', 'mkv', 'mov', 'avi', 'webm', 'mpeg', 'mpg', 'm4v'
}
MAX_IMAGE_DIMENSION = 8192
# Рабочее разрешение для фоновой обработки: маска, композитинг и кодирование
# держат по несколько полноразмерных буферов — на 8K пик памяти ~6 ГБ.
# Токены и вырезатель работают с 4096 без потери качества.
MAX_PROCESS_DIM = 4096

SESSION = None
# RLock: load_session() легитимно вызывается и снаружи, и из-под уже
# удерживаемого SESSION_LOCK (инференс держит лок от получения сессии до run())
SESSION_LOCK = threading.RLock()
CONFIG_LOCK = threading.Lock()
DEVICE_NAME = "Определение…"
_PROVIDERS = None
_PROVIDER_OPTIONS = None


def _load_config():
    try:
        cfg_path = config_file()
        return json.loads(cfg_path.read_text(encoding='utf-8')) if cfg_path.exists() else {}
    except Exception:
        return {}


def _save_config(cfg):
    with CONFIG_LOCK:
        config_file().write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding='utf-8')


def _update_config(mutator):
    """Атомарный read-modify-write config.json: чтение, изменение и запись
    под ОДНИМ захватом CONFIG_LOCK. Иначе параллельные сохранения (настройки
    UI через POST /config, выбор модели, кеш GPU-адаптера) читают устаревший
    снимок и молча затирают чужие ключи. mutator(dict)->dict|None
    (None = только читать)."""
    with CONFIG_LOCK:
        cfg = _load_config()
        new_cfg = mutator(cfg)
        if new_cfg is not None:
            config_file().write_text(json.dumps(new_cfg, ensure_ascii=False, indent=2), encoding='utf-8')
            return new_cfg
        return cfg


def _read_device_cache():
    """Возвращает (providers, provider_options, device_name) из config.json,
    если кеш валиден для текущего onnxruntime (провайдеры реально доступны)."""
    try:
        cfg = _load_config()
        c = cfg.get('device_cache') or {}
        prov = c.get('providers')
        if not isinstance(prov, list) or not prov or 'CPUExecutionProvider' not in prov:
            return None
        avail = set(ort.get_available_providers())
        if not all(p in avail for p in prov):
            return None
        opts = c.get('provider_options')
        if opts is not None and not isinstance(opts, list):
            return None
        # Ручной выбор gpu_device_id в config.json должен совпадать с кешем,
        # иначе кеш недействителен (пользователь сменил адаптер)
        dev = cfg.get('gpu_device_id')
        if isinstance(dev, int):
            if not (opts and isinstance(opts[0], dict) and opts[0].get('device_id') == dev):
                return None
        name = c.get('device_name')
        if not isinstance(name, str) or not name:
            return None
        return prov, opts, name
    except Exception:
        return None


def _save_device_cache(providers, provider_options, device_name):
    try:
        def _mutate(cfg):
            cfg['device_cache'] = {
                'providers': providers,
                'provider_options': provider_options,
                'device_name': device_name,
            }
            return cfg
        _update_config(_mutate)
    except Exception:
        pass


def allowed_file(filename):
    return '.' in filename and filename.rsplit('.', 1)[1].lower() in ALLOWED_EXTENSIONS


def is_media_file(file):
    filename = file.filename or ''
    extension = Path(filename).suffix.lower().lstrip('.')
    return extension in MEDIA_EXTENSIONS or (file.mimetype or '').startswith(('audio/', 'video/'))


def find_ffmpeg():
    """Ищет FFmpeg рядом с приложением, в tools/ или в PATH."""
    candidates = (
        RESOURCE_DIR / 'ffmpeg.exe',
        RESOURCE_DIR / 'tools' / 'ffmpeg.exe',
        RESOURCE_DIR / 'ffmpeg',
        RESOURCE_DIR / 'tools' / 'ffmpeg',
        BASE_DIR / 'ffmpeg.exe',
        BASE_DIR / 'tools' / 'ffmpeg.exe',
        BASE_DIR / 'ffmpeg',
        BASE_DIR / 'tools' / 'ffmpeg',
    )
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    system_ffmpeg = shutil.which('ffmpeg')
    if system_ffmpeg:
        return system_ffmpeg
    try:
        import imageio_ffmpeg
        bundled_ffmpeg = imageio_ffmpeg.get_ffmpeg_exe()
        if bundled_ffmpeg and Path(bundled_ffmpeg).is_file():
            return bundled_ffmpeg
    except Exception:
        pass
    return None


def convert_media_to_ogg(file, quality):
    """Конвертирует первый аудиопоток аудио- или видеофайла в OGG Vorbis."""
    ffmpeg = find_ffmpeg()
    if not ffmpeg:
        raise RuntimeError('FFmpeg не найден. Установите FFmpeg или добавьте его в PATH')

    suffix = Path(file.filename or '').suffix.lower() or '.media'
    input_fd, input_path = tempfile.mkstemp(suffix=suffix)
    output_fd, output_path = tempfile.mkstemp(suffix='.ogg')
    os.close(input_fd)
    os.close(output_fd)
    try:
        file.save(input_path)
        vorbis_quality = round((quality - 10) * 10 / 90)
        command = [
            ffmpeg, '-y', '-v', 'error',
            '-i', input_path,
            '-map', '0:a:0', '-vn',
            '-c:a', 'libvorbis', '-q:a', str(vorbis_quality),
            output_path,
        ]
        creationflags = 0x08000000 if sys.platform == 'win32' else 0
        run_options = {'capture_output': True, 'text': True, 'timeout': 600}
        if sys.platform == 'win32':
            run_options['creationflags'] = creationflags
        result = subprocess.run(command, **run_options)
        if result.returncode != 0:
            details = result.stderr.strip().splitlines()
            detail = details[-1] if details else 'неизвестная ошибка FFmpeg'
            raise RuntimeError(f'FFmpeg: {detail}')
        with open(output_path, 'rb') as converted:
            return io.BytesIO(converted.read())
    finally:
        for path in (input_path, output_path):
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass


def open_image(stream):
    """Открывает изображение с проверкой размера ДО полной декомпрессии
    (защита от decompression bomb: decode 100+ МП позже займёт гигабайты)."""
    im = Image.open(stream)
    w, h = im.size
    if w * h > 100_000_000:
        raise ValueError('Изображение слишком большое (более 100 МП)')
    return im


def validate_image(image):
    if image.width > MAX_IMAGE_DIMENSION or image.height > MAX_IMAGE_DIMENSION:
        ratio = min(MAX_IMAGE_DIMENSION / image.width, MAX_IMAGE_DIMENSION / image.height)
        new_size = (int(image.width * ratio), int(image.height * ratio))
        image = image.resize(new_size, Image.LANCZOS)
    return image


def cap_process_size(image):
    """Уменьшает рабочее изображение до MAX_PROCESS_DIM (BILINEAR — быстро).
    Возвращает новое изображение; исходное освобождается по refcount."""
    import gc
    w, h = image.size
    if max(w, h) > MAX_PROCESS_DIM:
        ratio = MAX_PROCESS_DIM / max(w, h)
        image = image.resize((max(1, int(w * ratio)), max(1, int(h * ratio))), Image.BILINEAR)
        gc.collect()
    return image


def _enumerate_dxgi_adapters():
    """Перечисляет DXGI-адаптеры через ctypes (без лишних зависимостей).
    Возвращает [(device_id, description), ...]; device_id = индекс DXGI-адаптера,
    он же device_id для DmlExecutionProvider."""
    adapters = []
    try:
        import ctypes
        from ctypes import wintypes, POINTER, byref, cast, Structure, c_void_p, c_uint, c_ulong, c_long, c_wchar, c_size_t, c_int64, WINFUNCTYPE

        class GUID(Structure):
            _fields_ = [
                ("Data1", c_uint),
                ("Data2", wintypes.WORD),
                ("Data3", wintypes.WORD),
                ("Data4", ctypes.c_ubyte * 8),
            ]

        class DXGI_ADAPTER_DESC(Structure):
            _fields_ = [
                ("Description", c_wchar * 128),
                ("VendorId", c_uint),
                ("DeviceId", c_uint),
                ("SubSysId", c_uint),
                ("Revision", c_uint),
                ("DedicatedVideoMemory", c_size_t),
                ("DedicatedSystemMemory", c_size_t),
                ("SharedSystemMemory", c_size_t),
                ("AdapterLuid", c_int64),
            ]

        DXGI_ERROR_NOT_FOUND = 0x887A0027
        # IID_IDXGIFactory: 7B7166EC-21C7-44AE-B21A-C9AE321AE369
        # (CreateDXGIFactory1 на некоторых системах возвращает E_NOINTERFACE,
        # поэтому используем IDXGIFactory — EnumAdapters у него тот же слот vtable)
        IID_IDXGIFactory = GUID(0x7B7166EC, 0x21C7, 0x44AE,
                                (ctypes.c_ubyte * 8)(0xB2, 0x1A, 0xC9, 0xAE, 0x32, 0x1A, 0xE3, 0x69))

        dxgi = ctypes.WinDLL('dxgi.dll')
        CreateDXGIFactory = dxgi.CreateDXGIFactory
        CreateDXGIFactory.argtypes = [POINTER(GUID), POINTER(c_void_p)]
        CreateDXGIFactory.restype = c_long

        factory = c_void_p()
        if CreateDXGIFactory(byref(IID_IDXGIFactory), byref(factory)) != 0 or not factory:
            return []

        EnumAdaptersFn = WINFUNCTYPE(c_long, c_void_p, c_uint, POINTER(c_void_p))
        GetDescFn = WINFUNCTYPE(c_long, c_void_p, POINTER(DXGI_ADAPTER_DESC))
        ReleaseFn = WINFUNCTYPE(c_ulong, c_void_p)

        fvt = cast(factory, POINTER(POINTER(c_void_p))).contents
        EnumAdapters = EnumAdaptersFn(fvt[3])  # IDXGIFactory::EnumAdapters

        i = 0
        while i < 32:
            adapter = c_void_p()
            hr = EnumAdapters(factory, i, byref(adapter))
            if hr == DXGI_ERROR_NOT_FOUND:
                break
            if hr != 0 or not adapter:
                i += 1
                continue
            avt = cast(adapter, POINTER(POINTER(c_void_p))).contents
            desc = DXGI_ADAPTER_DESC()
            if GetDescFn(avt[4])(adapter, byref(desc)) == 0:  # IDXGIAdapter::GetDesc
                name = (desc.Description or '').strip()
                if name:
                    adapters.append((i, name))
            ReleaseFn(avt[2])(adapter)
            i += 1

        ReleaseFn(fvt[2])(factory)
    except Exception:
        return []
    return adapters


def _classify_adapter(name):
    """Классифицирует адаптер: 'discrete' | 'integrated' | 'virtual' | None."""
    low = name.lower()
    if any(k in low for k in ('microsoft', 'basic', 'remote', 'warp', 'parsec',
                              'virtual', 'rdp', 'indirect', 'software', 'teamviewer', 'anydesk')):
        return 'virtual'
    if any(k in low for k in ('nvidia', 'geforce', 'rtx', 'gtx', 'quadro', 'tesla')):
        return 'discrete'
    if 'arc' in low:
        return 'discrete'
    if 'intel' in low:
        return 'integrated'
    if 'radeon' in low or 'amd' in low:
        # Встроенные AMD заканчиваются на "Graphics" или содержат Vega/Ryzen/APU
        if low.endswith('graphics') or 'vega' in low or 'ryzen' in low or 'apu' in low:
            return 'integrated'
        return 'discrete'
    if 'graphics' in low or 'adreno' in low or 'mali' in low:
        return 'integrated'
    return None


def _pick_discrete_adapter(adapter_list):
    """Из списка [(idx, name)] выбирает дискретный GPU (минимальный индекс)."""
    best = None
    for idx, name in adapter_list:
        if _classify_adapter(name) == 'discrete':
            if best is None or idx < best[0]:
                best = (idx, name)
    return best if best else (None, None)


def _detect_discrete_dml_adapter():
    """Ищет дискретную видеокарту для DirectML (гибридные ноутбуки).
    Возвращает (device_id, description) или (None, None) при неоднозначности."""
    try:
        return _pick_discrete_adapter(_enumerate_dxgi_adapters())
    except Exception:
        return (None, None)


def get_providers():
    global DEVICE_NAME, _PROVIDER_OPTIONS
    try:
        cached = _read_device_cache()
        if cached:
            _PROVIDER_OPTIONS = cached[1]
            DEVICE_NAME = cached[2]
            print(f"  Устройство (кеш): {DEVICE_NAME}")
            return cached[0]
    except Exception:
        pass
    try:
        available = ort.get_available_providers()
    except AttributeError:
        ort_pkg = 'onnxruntime-directml' if sys.platform == 'win32' else 'onnxruntime'
        print(" WARNING: onnxruntime повреждён или установлен не полностью.")
        print(f" Выполните: pip install --force-reinstall {ort_pkg}")
        DEVICE_NAME = 'CPU (onnxruntime повреждён)'
        return ['CPUExecutionProvider']

    # macOS: CoreML определяется сразу, чтобы не запускать nvidia-smi/rocm-smi впустую
    if sys.platform == 'darwin' and 'CoreMLExecutionProvider' in available:
        DEVICE_NAME = 'CoreML'
        _save_device_cache(['CoreMLExecutionProvider', 'CPUExecutionProvider'], None, DEVICE_NAME)
        return ['CoreMLExecutionProvider', 'CPUExecutionProvider']

    VIRTUAL_ADAPTER_KEYWORDS = [
        'parsec', 'virtual', 'microsoft', 'basic', 'remote',
        'indirect', 'display only', 'rdp', 'teamviewer', 'anydesk',
        'warp', 'software renderer'
    ]

    def is_real_gpu(name):
        name_lower = name.lower()
        return not any(kw in name_lower for kw in VIRTUAL_ADAPTER_KEYWORDS)

    def run_cmd(cmd):
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=5)
            return result.stdout or ''
        except Exception:
            return ''

    def get_real_gpus():
        """Список физических видеокарт (виртуальные адаптеры отсеиваются)."""
        if sys.platform == 'win32':
            out = run_cmd(['powershell', '-Command',
                           'Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name'])
            names = [l.strip() for l in out.splitlines() if l.strip()]
        else:
            out = run_cmd(['nvidia-smi', '--query-gpu=name', '--format=csv,noheader'])
            names = [l.strip() for l in out.splitlines() if l.strip()]
            if not names:
                out = run_cmd(['rocm-smi', '--showproductname'])
                names = [l.strip() for l in out.splitlines() if l.strip()]
        return [n for n in names if is_real_gpu(n)]

    def get_cpu_name():
        cpu = ''
        try:
            import platform
            cpu = platform.processor()
        except Exception:
            cpu = ''
        if not cpu:
            if sys.platform == 'win32':
                cpu = run_cmd(['powershell', '-Command',
                               '(Get-CimInstance Win32_Processor | Select-Object -First 1).Name']).strip()
            elif sys.platform == 'darwin':
                cpu = run_cmd(['sysctl', '-n', 'machdep.cpu.brand_string']).strip()
            else:
                cpu = run_cmd(['grep', '-m1', 'model name', '/proc/cpuinfo'])
                cpu = cpu.split(':')[-1].strip() if ':' in cpu else ''
        return cpu

    real_gpus = get_real_gpus()
    gpu_name = real_gpus[0] if real_gpus else None
    nvidia_gpu = next((g for g in real_gpus if 'nvidia' in g.lower()), None)
    has_nvidia = nvidia_gpu is not None

    def device_options():
        """device_id для выбора дискретной карты на ноутбуках (гибридная графика).
        Задаётся в config.json ключом gpu_device_id (0, 1, ...)."""
        try:
            cfg_path = config_file()
            if cfg_path.exists():
                import json
                cfg = json.loads(cfg_path.read_text(encoding='utf-8'))
                dev = cfg.get('gpu_device_id')
                if isinstance(dev, int) and dev >= 0:
                    # Длина обязана совпадать с числом провайдеров (2),
                    # а не с len(available) — иначе ORT отбрасывает опции
                    return [{'device_id': dev}, {}]
        except Exception:
            pass
        return None

    if has_nvidia and 'CUDAExecutionProvider' in available:
        DEVICE_NAME = f'CUDA ({nvidia_gpu})'
        _PROVIDER_OPTIONS = device_options()
        _save_device_cache(['CUDAExecutionProvider', 'CPUExecutionProvider'], _PROVIDER_OPTIONS, DEVICE_NAME)
        return ['CUDAExecutionProvider', 'CPUExecutionProvider']

    if 'ROCMExecutionProvider' in available and gpu_name:
        DEVICE_NAME = f'ROCm ({gpu_name})'
        _save_device_cache(['ROCMExecutionProvider', 'CPUExecutionProvider'], None, DEVICE_NAME)
        return ['ROCMExecutionProvider', 'CPUExecutionProvider']

    if 'DmlExecutionProvider' in available and gpu_name:
        _PROVIDER_OPTIONS = device_options()
        name_to_show = gpu_name
        if not _PROVIDER_OPTIONS:
            # На гибридных ноутбуках адаптер 0 может быть встроенной картой —
            # ищем дискретную через DXGI и указываем её device_id напрямую.
            dml_id, dml_name = _detect_discrete_dml_adapter()
            if dml_id is not None:
                _PROVIDER_OPTIONS = [{'device_id': dml_id}, {}]
                name_to_show = dml_name
                print(f"  Дискретная видеокарта: {dml_name} (адаптер #{dml_id})")
        DEVICE_NAME = f'DirectML ({name_to_show})'
        _save_device_cache(['DmlExecutionProvider', 'CPUExecutionProvider'], _PROVIDER_OPTIONS, DEVICE_NAME)
        return ['DmlExecutionProvider', 'CPUExecutionProvider']

    # Нет физической видеокарты: DirectML уйдёт на WARP (софт) — это медленнее CPU
    cpu_name = get_cpu_name()
    DEVICE_NAME = cpu_name if cpu_name else 'CPU'
    _save_device_cache(['CPUExecutionProvider'], None, DEVICE_NAME)
    return ['CPUExecutionProvider']


# Detect device at import time (without loading the model).
# Выполняется сразу после определения get_providers — ДО любого вызова load_session()
# (включая блок __main__ ниже), иначе сессия создаётся с providers=None → CPU.
try:
    _PROVIDERS = get_providers()
except Exception:
    _PROVIDERS = ['CPUExecutionProvider']


def load_session():
    """Возвращает активную сессию, создавая её при необходимости.
    ВАЖНО: для run() сессию нужно получать ПОД SESSION_LOCK и использовать
    до выхода из него — ссылка, переживший лок «быстрый путь» вернувшая
    сессия, может оказаться старой после /select_model, а две живые
    DML-сессии ломают инференс (ExecuteKernel 80070057)."""
    global SESSION, DEVICE_NAME, _PROVIDER_OPTIONS
    if SESSION is None:
        # Создание сессии и прогрев — под блокировкой: параллельные DML-сессии
        # ломают инференс (ExecuteKernel 80070057)
        with SESSION_LOCK:
            if SESSION is not None:
                return SESSION
            model_path = get_selected_model_path()
            if not model_path.exists():
                raise FileNotFoundError(f"Модель не найдена: {model_path}")
            if not hasattr(ort, 'InferenceSession'):
                raise RuntimeError(
                    "onnxruntime повреждён или установлен не полностью. "
                    "Выполните: pip install --force-reinstall onnxruntime-directml"
                )
            providers = _PROVIDERS
            print(f"Модель: {model_path.name} | Загрузка на {DEVICE_NAME}…")

            opts = ort.SessionOptions()
            opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_EXTENDED
            opts.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
            opts.inter_op_num_threads = 1
            opts.intra_op_num_threads = max(1, os.cpu_count() // 2)
            opts.enable_mem_pattern = True
            opts.enable_mem_reuse = True
            opts.add_session_config_entry("session.disable_prepacking", "0")

            try:
                SESSION = ort.InferenceSession(
                    str(model_path),
                    sess_options=opts,
                    providers=providers,
                    provider_options=_PROVIDER_OPTIONS
                )
            except Exception as e:
                # Авто-выбранный device_id мог не подойти (DML) — пробуем дефолтный адаптер,
                # и только потом откатываемся на CPU.
                if _PROVIDER_OPTIONS and providers != ['CPUExecutionProvider']:
                    try:
                        print(f"  Адаптер #{_PROVIDER_OPTIONS[0].get('device_id')} не запустился ({e})")
                        print("  Пробуем дефолтный адаптер…")
                        SESSION = ort.InferenceSession(
                            str(model_path),
                            sess_options=opts,
                            providers=providers,
                            provider_options=None
                        )
                        _PROVIDER_OPTIONS = None
                        print("  Дефолтный адаптер работает.")
                    except Exception:
                        SESSION = None

                if SESSION is None:
                    opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
                    print(f"  Оптимизация EXTENDED не загрузилась ({e})")
                    print("  Откат на BASIC…")
                    try:
                        SESSION = ort.InferenceSession(
                            str(model_path),
                            sess_options=opts,
                            providers=providers,
                            provider_options=_PROVIDER_OPTIONS
                        )
                    except Exception:
                        if providers != ['CPUExecutionProvider']:
                            print(f"  GPU-провайдер не запустился ({e})")
                            print("  Откат на CPU…")
                            DEVICE_NAME = 'CPU (fallback)'
                        SESSION = ort.InferenceSession(
                            str(model_path),
                            sess_options=opts,
                            providers=['CPUExecutionProvider']
                        )

            print(f" Прогрев модели…")
            warmup_time = None
            try:
                dummy = np.zeros((1, 3, 1024, 1024), dtype=np.float32)
                input_name = SESSION.get_inputs()[0].name
                t0 = time.time()
                SESSION.run(None, {input_name: dummy})
                warmup_time = time.time() - t0
                del dummy
                import gc
                gc.collect()
                print(f" Прогрев завершён ({warmup_time:.2f} с).")
            except Exception as e:
                print(f" Прогрев не удался: {e}")

            print(f" Готово\n")
            _spawn_tune_child()
    return SESSION


def _spawn_tune_child():
    """Запускает подбор адаптера DirectML в ОТДЕЛЬНОМ процессе (--tune-gpu).
    Параллельные DML-сессии в одном процессе ломают основной инференс
    (баг onnxruntime-directml: ExecuteKernel 80070057), поэтому изолируем пробу.
    Запуск отложен на 5 с и с пониженным приоритетом, чтобы не конкурировать
    с приложением за GPU/CPU на слабых ноутбуках."""
    try:
        if 'DmlExecutionProvider' not in _PROVIDERS or _PROVIDER_OPTIONS:
            return
        cfg_path = config_file()
        cfg = json.loads(cfg_path.read_text(encoding='utf-8')) if cfg_path.exists() else {}
        if isinstance(cfg.get('gpu_device_id'), int):
            return

        def _spawn():
            if getattr(sys, 'frozen', False):
                args = [sys.executable, '--tune-gpu']
            else:
                args = [sys.executable, str(BASE_DIR / 'server.py'), '--tune-gpu']
            # 0x08000000 = CREATE_NO_WINDOW, 0x00004000 = BELOW_NORMAL_PRIORITY_CLASS
            options = {'close_fds': True}
            if sys.platform == 'win32':
                options['creationflags'] = 0x08000000 | 0x00004000
            subprocess.Popen(args, **options)
            print(" Запущен фоновый подбор адаптера DirectML (отдельный процесс)…")

        t = threading.Timer(5.0, _spawn)
        t.daemon = True
        t.start()
    except Exception as e:
        print(f" Не удалось запустить подбор адаптера: {e}")


def cli_tune_gpu():
    """CLI-подбор самого быстрого адаптера DirectML (гибридные ноутбуки).
    Вызывается в отдельном процессе с флагом --tune-gpu.
    Результат пишется в config.json (gpu_device_id) и применится после перезапуска."""
    import time as _t
    try:
        if 'DmlExecutionProvider' not in ort.get_available_providers():
            return

        def bench(device_id):
            opts = ort.SessionOptions()
            opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
            opts.intra_op_num_threads = 2
            model_path = str(get_selected_model_path())
            if device_id is None:
                sess = ort.InferenceSession(
                    model_path, sess_options=opts,
                    providers=['DmlExecutionProvider', 'CPUExecutionProvider']
                )
            else:
                sess = ort.InferenceSession(
                    model_path, sess_options=opts,
                    providers=['DmlExecutionProvider', 'CPUExecutionProvider'],
                    provider_options=[{'device_id': device_id}, {}]
                )
            input_name = sess.get_inputs()[0].name
            x = np.zeros((1, 3, 1024, 1024), dtype=np.float32)
            t0 = _t.time()
            sess.run(None, {input_name: x})
            dt = _t.time() - t0
            del sess, x
            return dt

        default_t = bench(None)
        print(f"  [tune] дефолтный адаптер: {default_t:.2f} с")
        best_id, best_time = None, None
        # Тестируем столько адаптеров, сколько реально есть (но не больше 4),
        # чтобы дискретная карта на гибридных ноутбуках не оказалась за пределами списка.
        try:
            adapter_count = len(_enumerate_dxgi_adapters())
        except Exception:
            adapter_count = 2
        limit = max(2, min(adapter_count, 4))
        for did in range(limit):
            try:
                dt = bench(did)
                print(f"  [tune] адаптер #{did}: {dt:.2f} с")
                if best_time is None or dt < best_time:
                    best_id, best_time = did, dt
            except Exception as e:
                print(f"  [tune] адаптер #{did}: не доступен ({e})")
                break
        if best_id is None or best_time is None:
            return
        if best_time >= default_t * 0.85:
            print(f"  [tune] текущий адаптер оптимален ({default_t:.2f} с) — оставляем")
            return
        def _mutate(cfg):
            cfg['gpu_device_id'] = best_id
            return cfg
        _update_config(_mutate)
        print(f"  [tune] выбран адаптер #{best_id} ({best_time:.2f} с), сохранено в config.json")
    except Exception as e:
        print(f"  [tune] ошибка: {e}")


def refine_mask(mask_pil, edge_blur=1, threshold_low=10, threshold_high=245, work_size=1024, target_size=None):
    """Маска рефайнится на уменьшенной копии (work_size), чтобы не жечь RAM на больших картинках.
    Результат возвращается в размере target_size (или в исходном размере маски)."""
    orig_size = target_size or mask_pil.size
    w, h = orig_size
    if work_size and max(w, h) > work_size:
        ratio = work_size / max(w, h)
        work = mask_pil.resize((max(1, int(w * ratio)), max(1, int(h * ratio))), Image.BILINEAR)
    else:
        work = mask_pil

    mask_np = np.array(work).astype(np.float32) / 255.0

    low = threshold_low / 255.0
    high = threshold_high / 255.0
    mask_np = np.clip((mask_np - low) / (high - low + 1e-8), 0.0, 1.0)
    mask_np = mask_np ** 1.2

    mask_pil = Image.fromarray((mask_np * 255).astype(np.uint8), mode='L')

    mask_pil = mask_pil.filter(ImageFilter.MinFilter(3))

    mask_np = np.array(mask_pil).astype(np.float32) / 255.0
    mask_np = np.where(mask_np < 0.15, 0.0, mask_np)
    mask_np = np.where(mask_np > 0.92, 1.0, mask_np)
    mask_pil = Image.fromarray((mask_np * 255).astype(np.uint8), mode='L')

    if edge_blur > 0:
        mask_pil = mask_pil.filter(ImageFilter.GaussianBlur(edge_blur * 0.35))

    if mask_pil.size != orig_size:
        mask_pil = mask_pil.resize(orig_size, Image.LANCZOS)

    return mask_pil


def remove_background(image, edge_blur=1):
    import psutil, os as _os, gc
    proc = psutil.Process(_os.getpid())

    def mem():
        return proc.memory_info().rss / 1024 / 1024

    session = load_session()
    image = cap_process_size(image)
    print(f"  [RAM] старт обработки: {mem():.0f} МБ | размер входа: {image.size}")

    if image.mode != 'RGB':
        image = image.convert('RGB')

    img_resized = image.resize((1024, 1024), Image.LANCZOS)
    print(f"  [RAM] после resize: {mem():.0f} МБ")

    arr = np.array(img_resized, dtype=np.float32) / 255.0
    del img_resized
    gc.collect()
    print(f"  [RAM] после numpy arr: {mem():.0f} МБ")

    arr -= np.array([0.485, 0.456, 0.406], dtype=np.float32)
    arr /= np.array([0.229, 0.224, 0.225], dtype=np.float32)
    arr = arr.transpose(2, 0, 1)

    tensor = arr[np.newaxis]

    print(f"  [RAM] перед инференсом: {mem():.0f} МБ")
    t_infer = time.time()
    # Сессия и run() обязаны жить под ОДНИМ захватом лока: быстрый путь
    # load_session() вне лока мог вернуть ссылку на старую сессию, а
    # /select_model тем временем создать новую — две живые DML-сессии
    # дают ExecuteKernel 80070057. RLock допускает вложенный вход.
    with SESSION_LOCK:
        session = load_session()
        input_name = session.get_inputs()[0].name
        output = session.run(None, {input_name: tensor})
    print(f"  Инференс: {time.time() - t_infer:.2f} с")
    print(f"  [RAM] после инференса: {mem():.0f} МБ")

    del tensor, arr
    gc.collect()
    print(f"  [RAM] после del tensor: {mem():.0f} МБ")

    mask = output[0]
    del output
    gc.collect()
    print(f"  [RAM] после del output: {mem():.0f} МБ")

    while mask.ndim > 2:
        mask = mask.squeeze(0)
    if mask.ndim == 3:
        mask = mask[0]

    if mask.min() >= 0.0 and mask.max() <= 1.0:
        mask = np.clip(mask, 0.0, 1.0)
    else:
        mask = 1.0 / (1.0 + np.exp(-mask))
    mn, mx = mask.min(), mask.max()
    if mx - mn < 1e-4:
        # Модель не нашла объект: почти однородная маска. Растяжение делением
        # на (mx-mn) превратило бы числовой шум в спекл «маску» — отдаём пустую.
        mask = np.zeros(mask.shape, dtype=np.uint8)
    else:
        mask = ((mask - mn) / (mx - mn + 1e-8) * 255).astype(np.uint8)

    mask_pil = Image.fromarray(mask, mode='L')
    del mask
    gc.collect()

    mask_pil = refine_mask(mask_pil, edge_blur, target_size=image.size)

    # White-penalty считается на уменьшенной копии (1024), поправка затем масштабируется.
    small = image.resize((1024, 1024), Image.BILINEAR)
    small_arr = np.array(small, dtype=np.float32)
    del small
    wp = (small_arr[:, :, 0] * 0.299 + small_arr[:, :, 1] * 0.587 + small_arr[:, :, 2] * 0.114)
    alpha_small = np.array(mask_pil.resize((1024, 1024), Image.BILINEAR)).astype(np.float32)
    suppress = (wp > 220) & (alpha_small < 180)
    corr = np.zeros_like(wp)
    corr[suppress] = np.minimum((wp[suppress] - 220) * 3.5, 255.0)
    corr_img = Image.fromarray(corr.astype(np.uint8)).resize(image.size, Image.BILINEAR)
    del small_arr, wp, alpha_small, corr
    corr_u8 = np.array(corr_img)
    del corr_img
    gc.collect()

    # Композитинг: одна RGBA-копия исходника + uint8-маска; арифметика — in-place.
    result = image.convert('RGBA')
    rgba_np = np.array(result)
    del result
    alpha_np = np.array(mask_pil)
    del mask_pil
    del image
    gc.collect()

    alpha16 = alpha_np.astype(np.int16)
    del alpha_np
    alpha16 -= corr_u8
    del corr_u8
    np.clip(alpha16, 0, 255, out=alpha16)
    rgba_np[:, :, 3] = alpha16.astype(np.uint8)
    del alpha16
    gc.collect()

    result = Image.fromarray(rgba_np, mode='RGBA')
    del rgba_np
    gc.collect()

    print(f"  [RAM] финал: {mem():.0f} МБ")
    return result


def save_image(image, format_type, quality=90):
    buffer = io.BytesIO()
    if format_type == 'webp':
        image.save(buffer, format='WEBP', quality=quality, lossless=False)
        mime = 'image/webp'
    elif format_type == 'avif':
        if 'avif' not in Image.SAVE or not callable(Image.SAVE['avif']):
            # pillow-avif-plugin не установлен (нет в зависимостях) —
            # отдаём понятную ошибку вместо generic 500
            raise ValueError(
                "Формат AVIF недоступен: установите pillow-avif-plugin"
            )
        image.save(buffer, format='AVIF', quality=quality)
        mime = 'image/avif'
    elif format_type == 'png':
        image.save(buffer, format='PNG', optimize=True, compress_level=6)
        mime = 'image/png'
    elif format_type == 'jpg':
        if image.mode == 'RGBA':
            bg = Image.new('RGB', image.size, (255, 255, 255))
            bg.paste(image, mask=image.split()[3])
            image = bg
        image.save(buffer, format='JPEG', quality=quality)
        mime = 'image/jpeg'
    elif format_type == 'bmp':
        if image.mode not in ('RGB', 'RGBA'):
            image = image.convert('RGBA')
        image.save(buffer, format='BMP')
        mime = 'image/bmp'
    elif format_type == 'gif':
        if image.mode == 'RGBA':
            bg = Image.new('RGB', image.size, (255, 255, 255))
            bg.paste(image, mask=image.split()[3])
            image = bg
        elif image.mode not in ('P', 'L'):
            image = image.convert('RGB')
        image.save(buffer, format='GIF')
        mime = 'image/gif'
    elif format_type == 'tiff':
        if image.mode == 'RGBA':
            image = image.convert('RGBA')
        elif image.mode not in ('RGB', 'L'):
            image = image.convert('RGB')
        image.save(buffer, format='TIFF')
        mime = 'image/tiff'
    else:
        image.save(buffer, format='PNG', optimize=True)
        mime = 'image/png'
    buffer.seek(0)
    return buffer, mime


def create_default_ring(size, color=(100, 100, 100), width=40):
    scale = size / 1024
    w = int(width * scale)
    img = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.ellipse([w//2, w//2, size-w//2, size-w//2], outline=(*color, 255), width=w)
    return img


@app.route('/')
def index():
    theme = 'indigo'
    config_path = config_file()
    if config_path.exists():
        try:
            import json
            cfg = json.loads(config_path.read_text(encoding='utf-8'))
            theme = cfg.get('theme', 'indigo')
        except Exception:
            pass
    return render_template('index.html', github_repo=GITHUB_REPO, theme=theme, __version__=__version__)

@app.route('/version')
def version_info():
    return jsonify({'version': __version__, 'name': APP_NAME, 'platform': sys.platform})


@app.route('/model_status')
def model_status():
    p = get_selected_model_path()
    exists = p.exists()
    return jsonify({
        'exists': exists,
        'size': p.stat().st_size if exists else 0,
        'path': str(p),
        'models_dir': str(MODELS_DIR)
    })


@app.route('/models_list')
def models_list():
    try:
        MODELS_DIR.mkdir(exist_ok=True)
    except Exception:
        pass
    selected = get_selected_model_path()
    models = []
    seen = set()
    dirs = [MODELS_DIR]
    # Модель «рядом с exe» (сценарий из документации) и бандл-каталог:
    # в frozen-сборках BASE_DIR (каталог exe) != RESOURCE_DIR (_internal),
    # показываем обе, иначе пользовательская модель невидима в настройках
    for extra in (BASE_DIR / "models", RESOURCE_DIR / "models"):
        if extra.resolve() not in {d.resolve() for d in dirs} and extra.resolve() != MODELS_DIR.resolve():
            dirs.append(extra)
    for d in dirs:
        if not d.is_dir():
            continue
        for f in sorted(d.glob('*.onnx')):
            if not f.is_file():
                continue
            if f.name in seen:
                continue
            seen.add(f.name)
            try:
                size = f.stat().st_size
            except OSError:
                size = 0
            models.append({
                'name': f.name,
                'size': size,
                'selected': f.resolve() == selected.resolve(),
            })
    return jsonify({
        'models': models,
        'models_dir': str(MODELS_DIR),
        'selected': selected.name if selected.exists() else None,
    })


@app.route('/select_model', methods=['POST'])
def select_model():
    data = request.get_json(force=True, silent=True) or {}
    filename = str(data.get('name', ''))
    p = MODELS_DIR / Path(filename).name
    if p.suffix.lower() != '.onnx' or not p.exists() or not p.is_file():
        # Модель, положенная вручную рядом с exe (BASE_DIR) или в бандл
        for legacy in (BASE_DIR / "models" / Path(filename).name,
                       RESOURCE_DIR / "models" / Path(filename).name):
            if legacy.suffix.lower() == '.onnx' and legacy.is_file():
                p = legacy
                break
        else:
            return jsonify({'error': 'Модель не найдена'}), 404

    try:
        _activate_model(p)
    except Exception as e:
        return jsonify({'error': f'Не удалось сохранить настройку: {e}'}), 500

    return jsonify({'ok': True, 'name': p.name})


def _dropbox_direct_url(url):
    """Превращает обычную Dropbox-ссылку в ссылку на скачивание файла."""
    if 'dropbox.com' not in url.lower():
        return url
    from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
    parts = urlsplit(url)
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    query['dl'] = '1'
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


def _download_model(url):
    """Скачивает модель во временный файл, сохраняя имя из URL."""
    import gc
    from urllib.parse import unquote, urlsplit
    from urllib.request import Request, urlopen

    MODELS_DIR.mkdir(exist_ok=True)
    filename = Path(unquote(urlsplit(url).path)).name
    if not filename or Path(filename).suffix.lower() != '.onnx':
        filename = 'model.onnx'
    target = MODELS_DIR / filename
    if target.exists():
        raise FileExistsError(
            f"Модель «{target.name}» уже загружена — повторная загрузка не требуется"
        )
    # Уникальный temp: два параллельных запроса не должны писать в один
    # файл, иначе перемешанные чанки публикуют битую модель через os.replace
    temp_path = MODELS_DIR / f"{filename}.{os.getpid()}.{threading.get_ident()}.download"
    request_obj = Request(_dropbox_direct_url(url), headers={'User-Agent': 'Tokenatra model downloader'})
    try:
        with urlopen(request_obj, timeout=600) as response, temp_path.open('wb') as output:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                output.write(chunk)
        if temp_path.stat().st_size < 1024:
            raise ValueError('Сервер вернул слишком маленький файл')
        with SESSION_LOCK:
            global SESSION
            SESSION = None
            gc.collect()
            os.replace(str(temp_path), str(target))
        _activate_model(target)
        return target
    except Exception:
        temp_path.unlink(missing_ok=True)
        raise


@app.route('/download_model_from_server', methods=['POST'])
def download_model_from_server():
    if not MODEL_DOWNLOAD_URL:
        return jsonify({'error': 'Ссылка на модель ещё не настроена'}), 503
    try:
        model = _download_model(MODEL_DOWNLOAD_URL)
        return jsonify({'ok': True, 'name': model.name, 'size': model.stat().st_size})
    except Exception as e:
        app.logger.error('model download error: %s', e, exc_info=True)
        return jsonify({'error': f'Не удалось загрузить модель: {e}'}), 502


def _pw_dialog_open(file_types=(), allow_multiple=False):
    """Нативный диалог открытия через pywebview (работает из любого потока)."""
    win = getattr(app, 'window_ref', None)
    if win is None:
        return None
    try:
        import webview
        result = win.create_file_dialog(
            webview.OPEN_DIALOG, directory='', allow_multiple=allow_multiple, file_types=file_types)
        if result:
            return result[0] if isinstance(result, (list, tuple)) else result
    except Exception:
        pass
    return None


def _pw_dialog_save(save_filename):
    """Нативный диалог сохранения через pywebview (работает из любого потока)."""
    win = getattr(app, 'window_ref', None)
    if win is None:
        return None
    try:
        import webview
        result = win.create_file_dialog(webview.SAVE_DIALOG, directory='', save_filename=save_filename)
        if result:
            return result if isinstance(result, str) else result[0]
    except Exception:
        pass
    return None


def _pw_dialog_folder():
    """Нативный диалог выбора папки через pywebview (работает из любого потока)."""
    win = getattr(app, 'window_ref', None)
    if win is None:
        return None
    try:
        import webview
        result = win.create_file_dialog(webview.FOLDER_DIALOG, directory='')
        if result:
            return result[0] if isinstance(result, (list, tuple)) else result
    except Exception:
        pass
    return None


# Папки, выбранные пользователем через нативный диалог в текущей сессии.
# /save_to_folder принимает запись только в них (плюс папки из config.json),
# чтобы CSRF/компрометация фронта не дала писать в произвольные каталоги.
_picked_folders = set()


def _norm_folder(p):
    return os.path.normcase(os.path.abspath(str(p)))


@app.route('/load_model_from_disk', methods=['POST'])
def load_model_from_disk():
    win = getattr(app, 'window_ref', None)
    if win is not None:
        path = _pw_dialog_open(('Модель ONNX (*.onnx)',))
        if not path:
            return jsonify({'cancelled': True})
    else:
        # Браузерный/автономный режим без GUI-окна: tkinter доступен только на Windows
        import tkinter as tk
        from tkinter import filedialog

        root = tk.Tk()
        root.withdraw()
        root.attributes('-topmost', True)
        path = filedialog.askopenfilename(
            title='Выберите модель ONNX',
            filetypes=[('Модель ONNX', '*.onnx')]
        )
        root.destroy()

        if not path:
            return jsonify({'cancelled': True})

    source = Path(path)
    if source.suffix.lower() != '.onnx' or not source.is_file():
        return jsonify({'error': 'Можно выбрать только файл .onnx'}), 400

    MODELS_DIR.mkdir(exist_ok=True)
    target = MODELS_DIR / source.name
    temp_path = MODELS_DIR / (source.name + '.download')
    try:
        if source.resolve() != target.resolve():
            shutil.copy2(str(source), str(temp_path))
            import gc
            with SESSION_LOCK:
                global SESSION
                SESSION = None
                gc.collect()
                os.replace(str(temp_path), str(target))
        _activate_model(target)
        return jsonify({'ok': True, 'name': target.name, 'size': target.stat().st_size})
    except Exception as e:
        temp_path.unlink(missing_ok=True)
        return jsonify({'error': f'Не удалось загрузить модель: {e}'}), 500

@app.route('/icon')
def app_icon():
    return send_file(RESOURCE_DIR / 'icon.ico', mimetype='image/x-icon')


@app.route('/logo')
def app_logo():
    logo = RESOURCE_DIR / 'logo.png'
    if logo.exists():
        return send_file(logo, mimetype='image/png')
    return send_file(RESOURCE_DIR / 'icon.ico', mimetype='image/x-icon')


@app.route('/splash')
def splash():
    return render_template('splash.html')


@app.route('/api/window/<action>', methods=['POST'])
def window_action(action):
    win = getattr(current_app, 'window_ref', None)
    if not win:
        return jsonify({'ok': False, 'error': 'no window'})
    try:
        if action == 'minimize':
            win.minimize()
        elif action == 'maximize':
            win.maximize()
        elif action == 'restore':
            win.restore()
        elif action in ('close', 'destroy'):
            win.destroy()
        elif action == 'move':
            if sys.platform != 'win32':
                return jsonify({'ok': False, 'error': 'native move is unavailable on this platform'}), 501
            import ctypes
            native = win.native
            if native:
                hwnd = native.Handle.ToInt64() if hasattr(native.Handle, 'ToInt64') else int(native.Handle)
                ctypes.windll.user32.PostMessageW(ctypes.c_void_p(hwnd), 0x8001, 0, 0)
            return jsonify({'ok': True})
        else:
            return jsonify({'ok': False, 'error': 'unknown action'})
        return jsonify({'ok': True})
    except Exception as e:
        return jsonify({'ok': False, 'error': str(e)})


_resize_state = {}


def _apply_frameless_resize(native, base, dx, dy):
    west, north = base['west'], base['north']
    east, south = base['east'], base['south']
    nx = base['x'] + (dx if west else 0)
    ny = base['y'] + (dy if north else 0)
    nw = base['w'] + (dx if east else -dx if west else 0)
    nh = base['h'] + (dy if south else -dy if north else 0)
    native.move(max(0, int(nx)), max(0, int(ny)))
    native.resize(max(400, int(nw)), max(300, int(nh)))


@app.route('/api/window/resize_begin', methods=['POST'])
def window_resize_begin():
    """Linux/macOS frameless: старт интерактивного ресайза за край."""
    if sys.platform == 'win32':
        return jsonify({'ok': False}), 501
    win = getattr(current_app, 'window_ref', None)
    native = getattr(win, 'native', None)
    if native is None:
        return jsonify({'ok': False, 'error': 'no window'})
    edge = str((request.get_json(silent=True) or {}).get('edge', '')).lower()
    if not edge or len(edge) > 2 or any(c not in 'nsew' for c in edge):
        return jsonify({'ok': False, 'error': 'bad edge'}), 400
    # GTK/X вызовы только в главном потоке: базовую геометрию захватываем там же
    _resize_state.clear()
    _resize_state.update({
        'native': native, 'edge': edge,
        'west': 'w' in edge, 'north': 'n' in edge,
        'east': 'e' in edge, 'south': 's' in edge,
        'base': None,
    })
    from gi.repository import GLib
    GLib.idle_add(_capture_resize_base, native)
    return jsonify({'ok': True})


def _capture_resize_base(native):
    st = _resize_state
    if 'native' not in st or st.get('base'):
        return False
    try:
        x, y = native.get_position()
        w, h = native.get_size()
        st.update({'x': x, 'y': y, 'w': w, 'h': h, 'base': True})
    except Exception:
        st.clear()
    return False


@app.route('/api/window/resize_move', methods=['POST'])
def window_resize_move():
    st = _resize_state
    if not st or 'native' not in st or not st.get('base'):
        return jsonify({'ok': True})
    data = request.get_json(silent=True) or {}
    try:
        dx, dy = int(data.get('dx', 0)), int(data.get('dy', 0))
    except (TypeError, ValueError):
        return jsonify({'ok': False}), 400
    from gi.repository import GLib
    native = st['native']
    GLib.idle_add(_apply_frameless_resize, native, dict(st), dx, dy)
    return jsonify({'ok': True})


@app.route('/api/window/resize_end', methods=['POST'])
def window_resize_end():
    _resize_state.clear()
    return jsonify({'ok': True})


@app.route('/api/window/resize_start', methods=['POST'])
def window_resize_start():
    """Resize the parent window when WebView2 owns the client-area hit-test."""
    if sys.platform != 'win32':
        return jsonify({'ok': False, 'error': 'native resize is unavailable on this platform'}), 501
    win = getattr(current_app, 'window_ref', None)
    if not win:
        return jsonify({'ok': False, 'error': 'no window'})

    data = request.get_json(silent=True) or {}
    edges = {key: bool(data.get(key)) for key in ('left', 'right', 'top', 'bottom')}
    if not any(edges.values()):
        return jsonify({'ok': False, 'error': 'no edges'})

    hwnd_int = None
    try:
        native = win.native
        if native and hasattr(native, 'Handle'):
            hwnd_int = native.Handle.ToInt64() if hasattr(native.Handle, 'ToInt64') else int(native.Handle)
    except Exception:
        pass
    if hwnd_int is None:
        return jsonify({'ok': False, 'error': 'no hwnd'})

    user32 = ctypes.windll.user32
    hwnd = ctypes.c_void_p(hwnd_int)
    user32.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    user32.SetWindowPos.argtypes = [
        ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int,
        ctypes.c_int, ctypes.c_int, ctypes.c_uint
    ]
    user32.GetCursorPos.argtypes = [ctypes.c_void_p]
    user32.GetAsyncKeyState.argtypes = [ctypes.c_int]

    start_rect = wintypes.RECT()
    start_cursor = wintypes.POINT()
    user32.GetWindowRect(hwnd, ctypes.byref(start_rect))
    user32.GetCursorPos(ctypes.byref(start_cursor))

    min_width, min_height = 800, 600
    swp_nozorder = 0x0004
    swp_noactivate = 0x0010
    vk_lbutton = 0x01

    try:
        while user32.GetAsyncKeyState(vk_lbutton) & 0x8000:
            cursor = wintypes.POINT()
            user32.GetCursorPos(ctypes.byref(cursor))
            dx = cursor.x - start_cursor.x
            dy = cursor.y - start_cursor.y

            left = start_rect.left + (dx if edges['left'] else 0)
            right = start_rect.right + (dx if edges['right'] else 0)
            top = start_rect.top + (dy if edges['top'] else 0)
            bottom = start_rect.bottom + (dy if edges['bottom'] else 0)

            if right - left < min_width:
                if edges['left']:
                    left = right - min_width
                else:
                    right = left + min_width
            if bottom - top < min_height:
                if edges['top']:
                    top = bottom - min_height
                else:
                    bottom = top + min_height

            user32.SetWindowPos(
                hwnd, None, left, top, right - left, bottom - top,
                swp_nozorder | swp_noactivate
            )
            ctypes.windll.kernel32.Sleep(8)
    except Exception:
        pass
    return jsonify({'ok': True})


@app.route('/presets_list')
def presets_list():
    extensions = {'.png', '.webp', '.jpg', '.jpeg'}
    preset_dir = RESOURCE_DIR / 'presets'
    if not preset_dir.exists():
        try:
            # Read-only установка (Program Files) — просто отдаём пустой список
            preset_dir.mkdir(exist_ok=True)
        except OSError:
            return jsonify([])
    presets = []
    for f in sorted(preset_dir.iterdir()):
        if f.is_file() and f.suffix.lower() in extensions:
            presets.append({'name': f.stem, 'file': f.name})
    return jsonify(presets)


@app.route('/preset_file/<filename>')
def preset_file(filename):
    safe = Path(filename).name
    preset_dir = RESOURCE_DIR / 'presets'
    path = preset_dir / safe
    if not path.exists() or not path.is_file():
        return jsonify({'error': 'Not found'}), 404
    ext = path.suffix.lower()
    mime_map = {'.png': 'image/png', '.webp': 'image/webp', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.avif': 'image/avif'}
    mime = mime_map.get(ext, 'image/octet-stream')
    response = send_file(str(path), mimetype=mime)
    response.headers['Cache-Control'] = 'public, max-age=86400'
    return response


@app.route('/device')
def device():
    return jsonify({
        'device': DEVICE_NAME,
        'model_exists': get_selected_model_path().exists(),
    })


@app.route('/update_status')
def update_status():
    return jsonify(updater_status())


@app.route('/start_update_download', methods=['POST'])
def start_update_download():
    threading.Thread(target=download_update, daemon=True).start()
    return jsonify({'ok': True})


@app.route('/check_update', methods=['POST'])
def check_update():
    # Сброс состояния: UI должен дождаться СВЕЖЕГО результата проверки,
    # а не показать устаревшее «обновлений нет» от фонового чека
    reset_check_state()
    threading.Thread(target=lambda: check_for_updates(force=True), daemon=True).start()
    return jsonify({'ok': True})


def _macos_app_bundle():
    """Каталог .app, из которого запущено приложение (None вне frozen-.app)."""
    if not getattr(sys, 'frozen', False):
        return None
    exe_dir = Path(sys.executable).resolve().parent
    if exe_dir.name != 'MacOS':
        return None
    bundle = exe_dir.parent.parent
    if bundle.suffix == '.app' and bundle.is_dir():
        return bundle
    return None


def _linux_appimage_path():
    """Путь к запущенному AppImage ($APPIMAGE), None вне AppImage."""
    p = os.environ.get('APPIMAGE')
    if p:
        path = Path(p)
        if path.is_file():
            return path
    return None


def _apply_update_macos(src):
    kind = updater_status().get('download_kind', '')
    if kind != 'dmg':
        return jsonify({'error': 'Ожидался DMG-файл обновления'}), 400
    target = _macos_app_bundle()
    if not target:
        return jsonify({'error': 'Обновление доступно только для установленного приложения (.app)'}), 400
    if str(target).startswith('/Volumes/'):
        return jsonify({'error': 'Приложение запущено из образа. Скопируйте Tokenatra в «Программы» и запустите оттуда'}), 400
    try:
        writable = os.access(str(target.parent), os.W_OK)
    except OSError:
        writable = True
    if not writable:
        return jsonify({'error': 'Нет прав на замену приложения. Перенесите Tokenatra в «Программы»'}), 400

    # Сценарий выполняется ПОСЛЕ закрытия приложения: ждём PID, монтируем DMG,
    # меняем .app (со старым на время подмены бэкапом), перезапускаем, чистим.
    upd_dir = user_data_dir() / 'update'
    upd_dir.mkdir(parents=True, exist_ok=True)
    script = upd_dir / '_update.sh'
    script.write_text('\n'.join([
        '#!/bin/sh',
        'tries=0',
        'while kill -0 "$PID" 2>/dev/null; do',
        '  tries=$((tries+1))',
        '  [ "$tries" -ge 120 ] && break',
        '  sleep 0.5',
        'done',
        'sleep 1',
        'MNT="$(mktemp -d)"',
        'if ! hdiutil attach -nobrowse -quiet -mountpoint "$MNT" "$SRC"; then',
        '  rmdir "$MNT" 2>/dev/null',
        '  rm -f "$0"',
        '  exit 1',
        'fi',
        'APP_SRC="$(find "$MNT" -maxdepth 2 -name "*.app" -type d 2>/dev/null | head -n 1)"',
        'if [ -z "$APP_SRC" ]; then',
        '  hdiutil detach "$MNT" -quiet 2>/dev/null',
        '  rmdir "$MNT" 2>/dev/null',
        '  rm -f "$0"',
        '  exit 1',
        'fi',
        'mv "$TARGET" "$TARGET.old"',
        'if cp -R "$APP_SRC" "$TARGET"; then',
        '  rm -rf "$TARGET.old"',
        '  hdiutil detach "$MNT" -quiet 2>/dev/null',
        '  rm -f "$SRC"',
        '  open "$TARGET"',
        'else',
        '  mv "$TARGET.old" "$TARGET"',
        '  hdiutil detach "$MNT" -quiet 2>/dev/null',
        'fi',
        'rmdir "$MNT" 2>/dev/null',
        'rm -f "$0"',
    ]) + '\n', encoding='utf-8')
    os.chmod(script, 0o755)
    subprocess.Popen(
        ['/bin/sh', str(script)],
        env={**os.environ, 'SRC': src, 'TARGET': str(target), 'PID': str(os.getpid())},
        start_new_session=True,
        close_fds=True,
    )
    return jsonify({'ok': True})


def _apply_update_linux(src):
    kind = updater_status().get('download_kind', '')
    if kind != 'appimage':
        return jsonify({'error': 'Ожидался AppImage-файл обновления'}), 400
    appimage = _linux_appimage_path()
    if not appimage:
        return jsonify({'error': 'Обновление доступно только для AppImage-версии. Скачайте новый файл вручную.'}), 400
    tmp = appimage.parent / (appimage.name + '.new')
    shutil.copyfile(src, tmp)
    os.chmod(tmp, 0o755)
    os.replace(str(tmp), str(appimage))  # атомарно: запущенный процесс держит старый inode
    try:
        os.unlink(src)
    except OSError:
        pass
    subprocess.Popen([str(appimage)], start_new_session=True, close_fds=True)
    return jsonify({'ok': True})


@app.route('/apply_update', methods=['POST'])
def apply_update():
    if sys.platform == 'darwin':
        src = updater_status().get('download_path', '')
        if not src or not os.path.exists(src):
            return jsonify({'error': 'No update file'}), 400
        return _apply_update_macos(src)
    if sys.platform.startswith('linux'):
        src = updater_status().get('download_path', '')
        if not src or not os.path.exists(src):
            return jsonify({'error': 'No update file'}), 400
        return _apply_update_linux(src)

    try:
        s = updater_status()
        src = s.get('download_path', '')
        kind = s.get('download_kind', 'bare')
        if not src or not os.path.exists(src):
            return jsonify({'error': 'No update file'}), 400

        dst = sys.executable if getattr(sys, 'frozen', False) else str(BASE_DIR / 'Tokenatra.exe')
        exe_name = os.path.basename(dst)
        dst_dir = os.path.dirname(dst)

        # bat пишем в LOCALAPPDATA: в Program Files запись запрещена.
        # Пути передаются через переменные окружения (%SRC% и т.д.) — так они
        # не зависят от кодовой страницы bat-файла и не ломаются на %/& в путях.
        upd_dir = Path(os.environ.get('LOCALAPPDATA', tempfile.gettempdir())) / 'Tokenatra' / 'update'
        upd_dir.mkdir(parents=True, exist_ok=True)
        bat = upd_dir / '_update.bat'

        if kind == 'installer':
            # Установщик: дождаться выхода приложения (и снятия блокировки файлов),
            # тихая установка (/SILENT), перезапуск. UAC-запрос покажет сам установщик.
            lines = [
                '@echo off',
                'setlocal',
                'set /a tries=0',
                ':waitloop',
                'tasklist /FI "IMAGENAME eq %EXE%" 2>nul | find /I "%EXE%" >nul',
                'if errorlevel 1 goto settle',
                'set /a tries+=1',
                'if %tries% geq 30 (',
                '    taskkill /F /IM "%EXE%" >nul 2>&1',
                '    goto settle',
                ')',
                'ping 127.0.0.1 -n 2 > nul',
                'goto waitloop',
                ':settle',
                'ping 127.0.0.1 -n 3 > nul',
                ':run',
                'start "" /wait "%SRC%" /SILENT /SP- /SUPPRESSMSGBOXES /NORESTART',
                'if exist "%SRC%" del "%SRC%" >nul 2>&1',
                'if exist "%DST%" start "" /D "%DSTDIR%" "%DST%" >nul 2>&1',
                'del "%~f0" >nul 2>&1',
                'exit /b 0',
            ]
        else:
            # Голый exe приложения: копируем поверх (работает только если _internal на месте)
            lines = [
                '@echo off',
                'setlocal',
                'set /a tries=0',
                ':waitloop',
                'tasklist /FI "IMAGENAME eq %EXE%" 2>nul | find /I "%EXE%" >nul',
                'if errorlevel 1 goto copy',
                'set /a tries+=1',
                'if %tries% geq 30 (',
                '    taskkill /F /IM "%EXE%" >nul 2>&1',
                '    goto copy',
                ')',
                'ping 127.0.0.1 -n 2 > nul',
                'goto waitloop',
                ':copy',
                'set /a ctries=0',
                ':copyloop',
                'copy /Y "%SRC%" "%DST%.new" >nul 2>&1',
                'if exist "%DST%.new" goto moveit',
                'set /a ctries+=1',
                'if %ctries% geq 10 goto fail',
                'ping 127.0.0.1 -n 2 > nul',
                'goto copyloop',
                ':moveit',
                'set /a mtries=0',
                ':moveloop',
                'move /Y "%DST%.new" "%DST%" >nul 2>&1',
                'if not exist "%DST%.new" goto done',
                'set /a mtries+=1',
                'if %mtries% geq 10 goto fail',
                'ping 127.0.0.1 -n 2 > nul',
                'goto moveloop',
                ':done',
                'if exist "%SRC%" del "%SRC%" >nul 2>&1',
                'start "" /D "%DSTDIR%" "%DST%" >nul 2>&1',
                'del "%~f0" >nul 2>&1',
                'exit /b 0',
                ':fail',
                'if exist "%DST%.new" del "%DST%.new" >nul 2>&1',
                'if exist "%SRC%" del "%SRC%" >nul 2>&1',
                'del "%~f0" >nul 2>&1',
                'exit /b 1',
            ]

        bat.write_text('\r\n'.join(lines) + '\r\n', encoding='ascii')
        bat_env = {
            'SRC': src,
            'DST': dst,
            'DSTDIR': dst_dir,
            'EXE': exe_name,
        }
        subprocess.Popen(
            ['cmd', '/c', str(bat)],
            env={**os.environ, **bat_env},
            close_fds=True,
            creationflags=0x08000000
        )
        return jsonify({'ok': True})
    except Exception as e:
        return jsonify({'error': str(e)}), 500


@app.route('/ring')
def ring():
    size = request.args.get('size', '1024')
    if size not in ('512', '1024', '2048'):
        size = '1024'
    ring_files = {
        '512': BUILTIN_RING_DIR / 'token512.webp',
        '1024': BUILTIN_RING_DIR / 'token1024.webp',
        '2048': BUILTIN_RING_DIR / 'token2048.webp'
    }
    ring_path = ring_files.get(size)
    if ring_path and ring_path.exists():
        response = send_file(str(ring_path), mimetype='image/webp')
        response.headers['Cache-Control'] = 'public, max-age=86400'
        return response
    size_int = int(size)
    img = create_default_ring(size_int)
    buffer = io.BytesIO()
    img.save(buffer, format='PNG')
    buffer.seek(0)
    response = send_file(buffer, mimetype='image/png')
    response.headers['Cache-Control'] = 'public, max-age=86400'
    return response


@app.route('/mask')
def mask():
    if MASK_PATH.exists():
        with Image.open(MASK_PATH) as img:
            if img.mode != 'RGBA':
                img = img.convert('RGBA')
            buffer = io.BytesIO()
            img.save(buffer, format='PNG')
            buffer.seek(0)
            response = send_file(buffer, mimetype='image/png')
            response.headers['Cache-Control'] = 'public, max-age=86400'
            return response
    img = Image.new('RGBA', (1024, 1024), (0, 0, 0, 0))
    buffer = io.BytesIO()
    img.save(buffer, format='PNG')
    buffer.seek(0)
    response = send_file(buffer, mimetype='image/png')
    response.headers['Cache-Control'] = 'public, max-age=86400'
    return response


@app.route('/preset')
def preset():
    name = request.args.get('name', 'preset1')
    if not name.replace('_', '').replace('-', '').isalnum():
        return jsonify({'error': 'Invalid preset name'}), 400
    preset_dir = RESOURCE_DIR / 'presets'
    for ext in ['.png', '.webp', '.jpg']:
        preset_path = preset_dir / f"{name}{ext}"
        if preset_path.exists():
            mime = 'image/png' if ext == '.png' else 'image/webp' if ext == '.webp' else 'image/jpeg'
            response = send_file(str(preset_path), mimetype=mime)
            response.headers['Cache-Control'] = 'public, max-age=86400'
            return response
    return jsonify({'error': 'Preset not found'}), 404


@app.route('/example')
def example():
    example_path = PRESET_DIR / 'example.png'
    if example_path.exists():
        response = send_file(str(example_path), mimetype='image/png')
        response.headers['Cache-Control'] = 'public, max-age=86400'
        return response
    for ext in ['.webp', '.jpg']:
        alt_path = PRESET_DIR / f'example{ext}'
        if alt_path.exists():
            mime = 'image/webp' if ext == '.webp' else 'image/jpeg'
            response = send_file(str(alt_path), mimetype=mime)
            response.headers['Cache-Control'] = 'public, max-age=86400'
            return response
    return jsonify({'error': 'Example not found'}), 404


@app.route('/process', methods=['POST'])
def process():
    import gc

    if 'image' not in request.files:
        return jsonify({'error': 'Нет изображения'}), 400
    file = request.files['image']
    if file.filename == '':
        return jsonify({'error': 'Файл не выбран'}), 400
    if not allowed_file(file.filename):
        return jsonify({'error': 'Недопустимый формат файла'}), 400

    format_type = request.form.get('format', 'webp')
    if format_type not in ('webp', 'png', 'jpg', 'avif'):
        format_type = 'webp'
    try:
        quality = min(100, max(10, int(request.form.get('quality', 90))))
    except (ValueError, TypeError):
        quality = 90
    try:
        edge_blur = min(10, max(0, float(request.form.get('edge_blur', 1))))
    except (ValueError, TypeError):
        edge_blur = 1.0

    try:
        image = open_image(file.stream)
        image.load()
        image = validate_image(image)
        # Даунскейл до MAX_PROCESS_DIM сразу после декода — освобождает оригинал
        # до модели/композитинга (иначе 8K держит ~200-270 МБ лишних).
        image = cap_process_size(image)
        result = remove_background(image, edge_blur)
        del image
        gc.collect()

        buffer, mime = save_image(result, format_type, quality)
        del result
        gc.collect()

        return send_file(buffer, mimetype=mime, as_attachment=False,
                        download_name=f'result.{format_type}')
    except FileNotFoundError:
        return jsonify({'error': 'Модель не найдена. Загрузите файл .onnx в разделе «Настройки»'}), 500
    except Exception as e:
        gc.collect()
        app.logger.error('process error: %s', e, exc_info=True)
        return jsonify({'error': 'Ошибка обработки изображения'}), 500


@app.route('/convert', methods=['POST'])
def convert_file():
    if 'image' not in request.files:
        return jsonify({'error': 'Файл не выбран'}), 400
    file = request.files['image']

    format_type = request.form.get('format', 'webp')
    if format_type not in ('webp', 'png', 'jpg', 'avif', 'bmp', 'gif', 'tiff', 'ogg'):
        format_type = 'webp'
    try:
        quality = min(100, max(10, int(request.form.get('quality', 90))))
    except (ValueError, TypeError):
        quality = 90

    media = is_media_file(file)
    if media and format_type != 'ogg':
        return jsonify({'error': 'Аудио и видео можно конвертировать только в OGG'}), 400
    if format_type == 'ogg' and not media:
        return jsonify({'error': 'Формат OGG доступен только для аудио и видео'}), 400

    try:
        if format_type == 'ogg':
            buffer = convert_media_to_ogg(file, quality)
            return send_file(buffer, mimetype='audio/ogg', as_attachment=False,
                            download_name='converted.ogg')

        image = open_image(file.stream)
        image.load()

        buffer, mime = save_image(image, format_type, quality)
        del image

        return send_file(buffer, mimetype=mime, as_attachment=False,
                        download_name=f'converted.{format_type}')
    except RuntimeError as e:
        app.logger.error('media convert error: %s', e)
        return jsonify({'error': str(e)}), 500
    except Exception as e:
        app.logger.error('convert error: %s', e, exc_info=True)
        return jsonify({'error': 'Ошибка конвертации файла'}), 500


def _migrate_legacy_rings():
    """Пользовательские кольца из старого каталога (BASE_DIR/token_rings)
    переносим в RING_DIR — в установленной сборке BASE_DIR в Program Files
    недоступен для записи."""
    if BUILTIN_RING_DIR == RING_DIR:
        return
    if not BUILTIN_RING_DIR.exists():
        return
    RING_DIR.mkdir(parents=True, exist_ok=True)
    for f in BUILTIN_RING_DIR.iterdir():
        if (not f.is_file() or f.suffix.lower() not in {'.webp', '.png', '.jpg', '.jpeg'}
                or f.stem.casefold().endswith('.mask')):
            continue
        if f.name.casefold() in BUILTIN_RINGS:
            continue
        try:
            dest = RING_DIR / f.name
            if dest.exists():
                continue
            os.replace(str(f), str(dest))
            for extension in {f.suffix.lower(), '.png', '.webp', '.jpg', '.jpeg'}:
                legacy_mask = f.with_name(f'{f.stem}.mask{extension}')
                migrated_mask = dest.with_name(legacy_mask.name)
                if legacy_mask.is_file() and not migrated_mask.exists():
                    os.replace(str(legacy_mask), str(migrated_mask))
        except OSError:
            continue


@app.route('/rings_list')
def rings_list():
    extensions = {'.webp', '.png', '.jpg', '.jpeg'}
    _migrate_legacy_rings()
    ring_files = []
    seen_names = set()

    def collect(source_dir, builtin):
        if not source_dir.exists():
            return
        for f in source_dir.iterdir():
            if f.is_file() and f.suffix.lower() in extensions and not f.stem.casefold().endswith('.mask'):
                key = f.name.casefold()
                if key in seen_names:
                    continue
                seen_names.add(key)
                try:
                    added_at = f.stat().st_ctime_ns
                except OSError:
                    added_at = 0
                ring_files.append((builtin, added_at, key, f))

    collect(BUILTIN_RING_DIR, True)
    collect(RING_DIR, False)

    rings = []
    mime_map = {'.webp': 'image/webp', '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg'}
    for is_builtin, added_at, _, f in sorted(ring_files, key=lambda item: (not item[0], item[1], item[2])):
        mask_path = next((
            f.with_name(f'{f.stem}.mask{extension}')
            for extension in (f.suffix.lower(), '.png', '.webp', '.jpg', '.jpeg')
            if f.with_name(f'{f.stem}.mask{extension}').is_file()
        ), None)
        rings.append({
            'name': BUILTIN_RINGS.get(f.name.casefold(), f.stem),
            'file': f.name,
            'mime': mime_map.get(f.suffix.lower(), 'image/png'),
            'builtin': f.name.casefold() in BUILTIN_RINGS,
            'mask_file': mask_path.name if mask_path else None,
            'added_at': added_at,
        })
    return jsonify(rings)


@app.route('/delete_ring', methods=['POST'])
def delete_ring():
    """Удалить кольцо, добавленное пользователем (встроенные защищены)."""
    filename = (request.get_json(silent=True) or {}).get('file') or ''
    safe = Path(filename).name
    if not safe or '/' in filename or '\\' in filename:
        return jsonify({'error': 'Некорректное имя файла'}), 400
    if safe.casefold() in BUILTIN_RINGS:
        return jsonify({'error': 'Встроенное кольцо нельзя удалить'}), 403

    path = RING_DIR / safe
    if not path.exists() or not path.is_file():
        return jsonify({'error': 'Кольцо не найдено'}), 404

    try:
        path.unlink()
        for extension in {path.suffix.lower(), '.png', '.webp', '.jpg', '.jpeg'}:
            mask_path = path.with_name(f'{path.stem}.mask{extension}')
            if mask_path.exists() and mask_path.is_file():
                mask_path.unlink()
    except Exception:
        app.logger.error('ring delete error', exc_info=True)
        return jsonify({'error': 'Не удалось удалить кольцо'}), 500
    return jsonify({'ok': True})


@app.route('/add_ring', methods=['POST'])
def add_ring():
    """Сохранить пользовательское изображение кольца в папку приложения."""
    ring = request.files.get('ring')
    mask = request.files.get('mask')
    if not ring or not ring.filename:
        return jsonify({'error': 'Выберите изображение кольца'}), 400

    extension = Path(ring.filename).suffix.lower()
    if extension not in {'.webp', '.png', '.jpg', '.jpeg'}:
        return jsonify({'error': 'Поддерживаются только WebP, PNG и JPG'}), 400

    try:
        ring.stream.seek(0)
        with Image.open(ring.stream) as image:
            image.verify()
        ring.stream.seek(0)
    except Exception:
        return jsonify({'error': 'Файл не является корректным изображением'}), 400

    mask_extension = None
    if mask and mask.filename:
        mask_extension = Path(mask.filename).suffix.lower()
        if mask_extension not in {'.webp', '.png', '.jpg', '.jpeg'}:
            return jsonify({'error': 'Маска должна быть в формате WebP, PNG или JPG'}), 400
        try:
            mask.stream.seek(0)
            with Image.open(mask.stream) as image:
                image.verify()
            mask.stream.seek(0)
        except Exception:
            return jsonify({'error': 'Файл маски не является корректным изображением'}), 400

    name = (request.form.get('name') or '').strip()
    stem = Path(name or ring.filename).stem
    safe_stem = ''.join(char for char in stem if char.isalnum() or char in ' _-').strip(' ._-')
    if not safe_stem:
        safe_stem = 'Кольцо'

    RING_DIR.mkdir(exist_ok=True)
    target = RING_DIR / f'{safe_stem}{extension}'
    suffix = 2
    while target.exists():
        target = RING_DIR / f'{safe_stem}_{suffix}{extension}'
        suffix += 1

    try:
        ring.save(str(target))
        if mask and mask.filename:
            mask.save(str(target.with_name(f'{target.stem}.mask{mask_extension}')))
    except Exception:
        try:
            if target.exists():
                target.unlink()
        except OSError:
            pass
        app.logger.error('ring save error', exc_info=True)
        return jsonify({'error': 'Не удалось сохранить кольцо'}), 500

    return jsonify({'ok': True, 'name': target.stem, 'file': target.name})


@app.route('/ring_file/<filename>')
def ring_file(filename):
    safe = Path(filename).name
    path = RING_DIR / safe
    if not path.exists() or not path.is_file():
        path = BUILTIN_RING_DIR / safe
    if not path.exists() or not path.is_file():
        return jsonify({'error': 'Not found'}), 404
    ext = path.suffix.lower()
    mime_map = {'.webp': 'image/webp', '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg'}
    mime = mime_map.get(ext, 'image/octet-stream')
    response = send_file(str(path), mimetype=mime)
    response.headers['Cache-Control'] = 'public, max-age=86400'
    return response


def cli_remove_bg(input_path: str):
    input_path = Path(input_path)
    if not input_path.exists():
        print(f"Файл не найден: {input_path}", file=sys.stderr)
        sys.exit(1)
    print(f"Удаление фона: {input_path.name}")
    load_session()
    with Image.open(input_path) as image:
        image = validate_image(image)
        result = remove_background(image)
    out_path = input_path.with_name(input_path.stem + '_nobg.webp')
    buf, _ = save_image(result, 'webp', 90)
    out_path.write_bytes(buf.read())
    print(f"Сохранено: {out_path}")


def cli_to_webp(input_path: str):
    input_path = Path(input_path)
    if not input_path.exists():
        print(f"Файл не найден: {input_path}", file=sys.stderr)
        sys.exit(1)
    if input_path.suffix.lower() == '.webp':
        # Перекодирование lossy поверх оригинала = потеря данных
        print(f"Уже WebP, пропущено: {input_path.name}")
        return
    print(f"Конвертация в WebP: {input_path.name}")
    with Image.open(input_path) as image:
        out_path = input_path.with_suffix('.webp')
        buf, _ = save_image(image, 'webp', 90)
        out_path.write_bytes(buf.read())
    # Конвертация заменяет исходник (см. AGENTS.md: «deletes original»)
    try:
        input_path.unlink()
    except OSError as ex:
        print(f"Не удалось удалить исходник: {ex}", file=sys.stderr)
    print(f"Сохранено: {out_path}")

@app.route('/save_file', methods=['POST'])
def save_file():
    suggested = request.form.get('filename', 'file.webp')
    ext = suggested.rsplit('.', 1)[-1].lower() if '.' in suggested else 'webp'
    mime_map = {'webp': 'WebP Image', 'png': 'PNG Image', 'jpg': 'JPEG Image', 'avif': 'AVIF Image'}
    label = mime_map.get(ext, 'File')

    if 'file' not in request.files:
        return jsonify({'error': 'No file'}), 400

    data = request.files['file'].read()

    win = getattr(app, 'window_ref', None)
    if win is not None:
        path = _pw_dialog_save(suggested)
        if not path:
            return jsonify({'cancelled': True})
    else:
        # Браузерный/автономный режим без GUI-окна: tkinter доступен только на Windows
        import tkinter as tk
        from tkinter import filedialog

        root = tk.Tk()
        root.withdraw()
        root.attributes('-topmost', True)
        path = filedialog.asksaveasfilename(
            initialfile=suggested,
            defaultextension='.' + ext,
            filetypes=[(label, '*.' + ext), ('All files', '*.*')]
        )
        root.destroy()

        if not path:
            return jsonify({'cancelled': True})

    try:
        Path(path).write_bytes(data)
        return jsonify({'saved': True, 'path': path})
    except Exception as ex:
        return jsonify({'error': str(ex)}), 500

@app.route('/pick_folder', methods=['GET'])
def pick_folder():
    win = getattr(app, 'window_ref', None)
    if win is not None:
        path = _pw_dialog_folder()
        if not path:
            return jsonify({'cancelled': True})
    else:
        # Браузерный/автономный режим без GUI-окна: tkinter доступен только на Windows
        import tkinter as tk
        from tkinter import filedialog

        root = tk.Tk()
        root.withdraw()
        root.attributes('-topmost', True)
        path = filedialog.askdirectory()
        root.destroy()

        if not path:
            return jsonify({'cancelled': True})
    _picked_folders.add(_norm_folder(path))

    return jsonify({'path': path})

@app.route('/save_to_folder', methods=['POST'])
def save_to_folder():
    folder = request.form.get('folder', '')
    filename = request.form.get('filename', 'file.webp')

    if not folder:
        return jsonify({'error': 'No folder'}), 400
    if 'file' not in request.files:
        return jsonify({'error': 'No file'}), 400

    folder_path = Path(folder)
    if not folder_path.exists() or not folder_path.is_dir():
        return jsonify({'error': 'Invalid folder'}), 400

    # Разрешены только папки, выбранные через диалог в этой сессии, либо
    # сохранённые в config.json (quick-save/портрет/remover/converter после
    # перезапуска приложения). Произвольные пути — 403.
    allowed = set(_picked_folders)
    try:
        cfg_last_folders = (_load_config() or {}).get('lastFolders') or {}
        for f in cfg_last_folders.values():
            if f:
                allowed.add(_norm_folder(f))
    except Exception:
        pass
    if _norm_folder(folder_path) not in allowed:
        return jsonify({'error': 'Folder was not picked via dialog'}), 403

    data = request.files['file'].read()
    out = folder_path / Path(filename).name

    try:
        out.write_bytes(data)
        return jsonify({'saved': True, 'path': str(out)})
    except Exception as ex:
        return jsonify({'error': str(ex)}), 500

@app.route('/pick_image_to_open', methods=['GET'])
def pick_image_to_open():
    win = getattr(app, 'window_ref', None)
    if win is not None:
        path = _pw_dialog_open(('Images (*.webp;*.png;*.jpg;*.jpeg)', 'All files (*.*)'))
        if not path:
            return jsonify({'cancelled': True})
    else:
        # Браузерный/автономный режим без GUI-окна: tkinter доступен только на Windows
        import tkinter as tk
        from tkinter import filedialog

        root = tk.Tk()
        root.withdraw()
        root.attributes('-topmost', True)
        path = filedialog.askopenfilename(
            title='Выберите изображение',
            filetypes=[('Images', '*.webp *.png *.jpg *.jpeg'), ('All files', '*.*')]
        )
        root.destroy()

        if not path:
            return jsonify({'cancelled': True})

    path_obj = Path(path)
    ext = path_obj.suffix.lower()
    mime_map = {'.png': 'image/png', '.webp': 'image/webp', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.avif': 'image/avif'}
    mime = mime_map.get(ext, 'image/octet-stream')

    return jsonify({
        'path': str(path_obj.resolve()),
        'mime': mime
    })


@app.route('/list_images', methods=['POST'])
def list_images():
    data = request.get_json(force=True, silent=True) or {}
    path_str = data.get('path', '')
    if not path_str:
        return jsonify({'error': 'No path provided'}), 400
    path = Path(path_str)
    parent = path.parent
    if not parent.exists():
        return jsonify({'error': 'Directory not found'}), 404

    extensions = {'.webp', '.png', '.jpg', '.jpeg'}
    files = []
    for f in sorted(parent.iterdir()):
        if f.is_file() and f.suffix.lower() in extensions:
            files.append(str(f.resolve()))

    current_name = path.resolve().name
    current_index = -1
    for i, fp in enumerate(files):
        if Path(fp).name == current_name:
            current_index = i
            break

    return jsonify({
        'folder': str(parent.resolve()),
        'files': files,
        'currentIndex': current_index,
        'total': len(files)
    })


@app.route('/get_image_by_path')
def get_image_by_path():
    path_str = request.args.get('path', '')
    if not path_str:
        return jsonify({'error': 'No path'}), 400
    path = Path(path_str)
    if not path.exists() or not path.is_file():
        return jsonify({'error': 'File not found'}), 404
    ext = path.suffix.lower()
    mime_map = {'.png': 'image/png', '.webp': 'image/webp', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.avif': 'image/avif'}
    mime = mime_map.get(ext, 'image/octet-stream')
    return send_file(str(path), mimetype=mime)


@app.route('/config', methods=['GET'])
def get_config():
    config_path = config_file()
    if not config_path.exists():
        return jsonify({})
    try:
        import json
        return jsonify(json.loads(config_path.read_text(encoding='utf-8')))
    except Exception:
        return jsonify({})

@app.route('/config', methods=['POST'])
def save_config():
    try:
        data = request.get_json(force=True, silent=True)
        if not isinstance(data, dict):
            return jsonify({'error': 'Invalid body'}), 400
        # Серверные ключи (выбор модели, GPU) не должны затираться клиентским снапшотом.
        # Слияние и запись — атомарно под CONFIG_LOCK, иначе параллельный
        # /select_model или tune-gpu потеряет свои ключи.
        def _mutate(existing):
            merged = dict(data)
            for key in ('selected_model', 'gpu_device_id'):
                if (key not in data or data[key] is None) and key in existing:
                    merged[key] = existing[key]
            return merged
        _update_config(_mutate)
        return jsonify({'ok': True})
    except Exception as ex:
        return jsonify({'error': str(ex)}), 500


@app.route('/shutdown', methods=['POST'])
def shutdown():
    """Остановить Flask-сервер"""
    try:
        func = request.environ.get('werkzeug.server.shutdown')
        if func:
            func()
        return 'ok'
    except Exception:
        return 'error', 500


if __name__ == '__main__' and not getattr(sys, 'frozen', False):
    # Блок обязан быть в КОНЦЕ файла: иначе маршруты ниже не зарегистрируются
    # при запуске `python server.py` (app.run() блокирует выполнение модуля).
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--remove-bg', metavar='FILE')
    parser.add_argument('--to-webp', metavar='FILE')
    parser.add_argument('--tune-gpu', action='store_true')
    args, _ = parser.parse_known_args()

    if args.remove_bg:
        cli_remove_bg(args.remove_bg)
    elif args.to_webp:
        cli_to_webp(args.to_webp)
    elif args.tune_gpu:
        cli_tune_gpu()
    else:
        print("\n" + "=" * 50)
        print("Background Remover & Tokenatra")
        print("=" * 50)
        try:
            load_session()
            print(f"Device: {DEVICE_NAME}")
        except Exception as e:
            print(f"Model: {e}")
        print("http://localhost:7878")
        print("=" * 50 + "\n")
        app.run(host='127.0.0.1', port=7878, debug=False, threaded=True)

