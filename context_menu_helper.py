#!/usr/bin/env python3
import sys
import os
import shutil
import subprocess
import tempfile
import urllib.request
from pathlib import Path

PORT = 7878
BASE_URL = f'http://localhost:{PORT}'

IMAGE_EXTENSIONS = {'.png', '.jpg', '.jpeg', '.webp', '.bmp', '.gif', '.tiff', '.tif'}
MEDIA_EXTENSIONS = {
    '.mp3', '.wav', '.m4a', '.flac', '.aac', '.wma', '.opus', '.ogg',
    '.mp4', '.mkv', '.mov', '.avi', '.webm', '.mpeg', '.mpg', '.m4v'
}


def server_running():
    try:
        urllib.request.urlopen(f'{BASE_URL}/device', timeout=2)
        return True
    except Exception:
        return False


def remove_bg(file_path: str):
    file_path = Path(file_path)
    if not file_path.exists():
        sys.exit(1)

    if not server_running():
        _notify('Tokenatra is not running. Open the app first.')
        sys.exit(1)

    with open(file_path, 'rb') as f:
        file_data = f.read()

    boundary = 'TokenatraBoundary'
    body = (
        f'--{boundary}\r\n'
        f'Content-Disposition: form-data; name="image"; filename="{file_path.name}"\r\n'
        f'Content-Type: application/octet-stream\r\n\r\n'
    ).encode() + file_data + (
        f'\r\n--{boundary}\r\n'
        f'Content-Disposition: form-data; name="format"\r\n\r\nwebp'
        f'\r\n--{boundary}--\r\n'
    ).encode()

    req = urllib.request.Request(
        f'{BASE_URL}/process',
        data=body,
        headers={
            'Content-Type': f'multipart/form-data; boundary={boundary}',
            'Content-Length': str(len(body)),
            'Origin': BASE_URL,
        },
        method='POST'
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            if resp.status != 200:
                err = resp.read().decode('utf-8', errors='replace')[:200]
                _notify(f'Ошибка сервера: {err}')
                sys.exit(1)
            result_data = resp.read()
        out_path = file_path.with_suffix('.webp')
        out_path.write_bytes(result_data)
    except Exception:
        sys.exit(1)


def to_webp(file_path: str):
    from PIL import Image
    import io

    file_path = Path(file_path)
    if not file_path.exists():
        sys.exit(1)

    if file_path.suffix.lower() == '.webp':
        sys.exit(0)

    try:
        with Image.open(file_path) as image:
            out_path = file_path.with_suffix('.webp')
            buf = io.BytesIO()
            image.save(buf, format='WEBP', quality=90)
            out_path.write_bytes(buf.getvalue())
        if out_path != file_path:
            file_path.unlink()
    except Exception:
        sys.exit(1)


def _find_ffmpeg():
    base_dir = Path(sys.executable).parent if getattr(sys, 'frozen', False) else Path(__file__).parent
    for candidate in (
        base_dir / 'ffmpeg.exe',
        base_dir / 'tools' / 'ffmpeg.exe',
        base_dir / 'ffmpeg',
        base_dir / 'tools' / 'ffmpeg',
    ):
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


def to_ogg(file_path: str):
    file_path = Path(file_path)
    if not file_path.exists() or file_path.suffix.lower() not in MEDIA_EXTENSIONS:
        sys.exit(1)
    if file_path.suffix.lower() == '.ogg':
        sys.exit(0)

    ffmpeg = _find_ffmpeg()
    if not ffmpeg:
        _notify('FFmpeg не найден. Переустановите Tokenatra или добавьте ffmpeg.exe в папку tools.')
        sys.exit(1)

    output_path = file_path.with_suffix('.ogg')
    temp_fd, temp_path = tempfile.mkstemp(dir=file_path.parent, suffix='.ogg')
    os.close(temp_fd)
    try:
        result = subprocess.run(
            [
                ffmpeg, '-y', '-v', 'error', '-i', str(file_path),
                '-map', '0:a:0', '-vn', '-c:a', 'libvorbis', '-q:a', '8', temp_path,
            ],
            capture_output=True,
            text=True,
            timeout=600,
            creationflags=0x08000000 if sys.platform == 'win32' else 0,
        )
        if result.returncode != 0:
            _notify('Не удалось конвертировать файл в OGG.')
            sys.exit(1)
        os.replace(temp_path, output_path)
        if output_path != file_path:
            file_path.unlink()
    except Exception:
        sys.exit(1)
    finally:
        try:
            os.unlink(temp_path)
        except FileNotFoundError:
            pass


def folder_to_webp(folder_path: str):
    from PIL import Image
    import io

    folder = Path(folder_path)
    if not folder.exists() or not folder.is_dir():
        sys.exit(1)

    files = [
        f for f in folder.iterdir()
        if f.is_file() and f.suffix.lower() in IMAGE_EXTENSIONS and f.suffix.lower() != '.webp'
    ]

    if not files:
        _notify('Нет изображений для конвертации.')
        sys.exit(0)

    converted = 0
    failed = 0

    for f in files:
        try:
            image = Image.open(f)
            out_path = f.with_suffix('.webp')
            buf = io.BytesIO()
            image.save(buf, format='WEBP', quality=90)
            out_path.write_bytes(buf.getvalue())
            image.close()
            f.unlink()
            converted += 1
        except Exception:
            failed += 1

    if failed:
        _notify(f'Конвертировано: {converted}, ошибок: {failed}')
    else:
        _notify(f'Готово. Конвертировано файлов: {converted}.')


def _notify(message: str):
    try:
        if sys.platform == 'darwin':
            escaped = message.replace('\\', '\\\\').replace('"', '\\"')
            subprocess.Popen(
                ['osascript', '-e', f'display notification "{escaped}" with title "Tokenatra"']
            )
            return
        if sys.platform.startswith('linux'):
            try:
                subprocess.Popen(['notify-send', '-a', 'Tokenatra', message])
            except FileNotFoundError:
                pass
            return
        subprocess.Popen(
            [
                'powershell', '-WindowStyle', 'Hidden', '-Command',
                f'[System.Reflection.Assembly]::LoadWithPartialName("System.Windows.Forms") | Out-Null;'
                f'$n = New-Object System.Windows.Forms.NotifyIcon;'
                f'$n.Icon = [System.Drawing.SystemIcons]::Information;'
                f'$n.Visible = $true;'
                f'$n.ShowBalloonTip(4000, "Tokenatra", "{message}", [System.Windows.Forms.ToolTipIcon]::Info);'
                f'Start-Sleep -Milliseconds 4500;'
                f'$n.Dispose()'
            ],
            creationflags=0x08000000
        )
    except Exception:
        pass


def main():
    if len(sys.argv) < 3:
        sys.exit(1)

    flag = sys.argv[1]
    file_arg = sys.argv[2]

    if flag == '--remove-bg':
        remove_bg(file_arg)
    elif flag == '--to-webp':
        to_webp(file_arg)
    elif flag == '--to-ogg':
        to_ogg(file_arg)
    elif flag == '--folder-to-webp':
        folder_to_webp(file_arg)
    else:
        sys.exit(1)


if __name__ == '__main__':
    main()
