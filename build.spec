# -*- mode: python ; coding: utf-8 -*-
import sys
from pathlib import Path
from version import APP_NAME, MODEL_FILE
from PyInstaller.utils.hooks import collect_data_files

block_cipher = None
is_windows = sys.platform == 'win32'
is_macos = sys.platform == 'darwin'

imageio_ffmpeg_datas = []
try:
    imageio_ffmpeg_datas = collect_data_files('imageio_ffmpeg')
except ImportError:
    pass

a = Analysis(
    ['app.py', 'updater.py', 'context_menu_helper.py'],
    pathex=[],
    binaries=[],
    datas=[
        ('templates', 'templates'),
        ('static', 'static'),
        ('version.py', '.'),
        ('icon.ico', '.'),
        ('mask.png', '.'),
        ('logo.png', '.'),
        ('example.png', '.'),
        ('token_rings', 'token_rings'),
        ('presets', 'presets'),
        *imageio_ffmpeg_datas,
    ],
    hiddenimports=[
        'PIL', 'PIL._tkinter_finder',
        'server',
        'onnxruntime',
        'flask',
        'numpy',
        'webview',
        'psutil',
        'imageio_ffmpeg',
        'tkinter',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[
        'matplotlib', 'scipy', 'pandas', 'sympy',
        'tkinter.test',
        'torch', 'torchvision', 'torchaudio',
        'transformers', 'timm', 'tensorflow',
        'cv2', 'numba',
        'gradio', 'fastapi', 'uvicorn', 'starlette',
        'httpx', 'anyio', 'pydantic', 'tqdm',
        'einops', 'regex', 'tokenizers',
        'safetensors', 'huggingface_hub',
        'datasets', 'accelerate',
    ],
    win_no_prefer_redirects=False,
    win_private_assemblies=False,
    cipher=block_cipher,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name=APP_NAME,
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    version='version_info.txt' if is_windows else None,
    icon='icon.ico' if is_windows else None,
)

if is_macos:
    app = BUNDLE(
        exe,
        a.binaries,
        a.zipfiles,
        a.datas,
        name=f'{APP_NAME}.app',
        icon=None,
        bundle_identifier='com.sweetyhake.tokenatra',
        info_plist={
            'CFBundleName': APP_NAME,
            'CFBundleDisplayName': APP_NAME,
            'NSHighResolutionCapable': True,
        },
    )
else:
    coll = COLLECT(
        exe,
        a.binaries,
        a.zipfiles,
        a.datas,
        strip=False,
        upx=True,
        upx_exclude=[],
        name=APP_NAME,
    )
