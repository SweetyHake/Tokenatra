import ctypes
import os
import sys

from platform_utils import terminate_process

# WSLg и части виртуалок: Wayland-бэкенд GTK и композитор WebKit могут
# оставить окно невидимым. Принудительный X11 + мягкий рендеринг лечат.
if sys.platform != 'win32':
    os.environ.setdefault('GDK_BACKEND', 'x11')
    os.environ.setdefault('WEBKIT_DISABLE_COMPOSITING_MODE', '1')
    os.environ.setdefault('WEBKIT_DISABLE_DMABUF_RENDERER', '1')

IMAGE_EXTENSIONS = {'.png', '.jpg', '.jpeg', '.webp', '.bmp', '.gif', '.tiff', '.tif'}
MEDIA_EXTENSIONS = {
    '.mp3', '.wav', '.m4a', '.flac', '.aac', '.wma', '.opus', '.ogg',
    '.mp4', '.mkv', '.mov', '.avi', '.webm', '.mpeg', '.mpg', '.m4v'
}

FILE_REG_ENTRIES = [
    ('Tokenatra_RemoveBg', 'Удалить фон', '--remove-bg'),
    ('Tokenatra_ToWebp', 'Конвертировать в WebP', '--to-webp'),
]

MEDIA_FILE_REG_ENTRIES = [
    ('Tokenatra_ToOgg', 'Конвертировать в OGG', '--to-ogg'),
]

DIR_REG_ENTRIES = [
    (r'Software\Classes\Directory\shell\Tokenatra_FolderToWebp', 'Конвертировать все изображения в WebP', '--folder-to-webp'),
    (r'Software\Classes\Directory\Background\shell\Tokenatra_FolderToWebp', 'Конвертировать все изображения в WebP', '--folder-to-webp'),
]

# Канонический путь для глаголов «все файлы-изображения»: SystemFileAssociations\image
# (там же Windows держит «Печать»). Глаголы под ключом расширения (.png\shell\...)
# Explorer игнорирует, когда у расширения есть OpenWithProgids/PersistentHandler.
SFA_IMAGE = r'Software\Classes\SystemFileAssociations\image\shell'

# AQS-фильтр AppliesTo: глаголы на *\shell показываются только для файлов-изображений
# (Windows 11 25H2 игнорирует глаголы под расширением и SystemFileAssociations,
# а *\shell без фильтра показывает их у всех файлов).
APPIES_TO_IMAGES = ' OR '.join(f'System.FileExtension:="{ext}"' for ext in sorted(IMAGE_EXTENSIONS))
APPLIES_TO_MEDIA = ' OR '.join(f'System.FileExtension:="{ext}"' for ext in sorted(MEDIA_EXTENSIONS))

# Ключи старых версий (регистрировались под расширениями / SFA / COM) — удаляются при регистрации/unregister
LEGACY_REG_ENTRIES = [
    r'Software\Classes\*\shell\Tokenatra_RemoveBg',
    r'Software\Classes\*\shell\Tokenatra_ToWebp',
    r'Software\Classes\CLSID\{B2ED14AF-9138-42A4-AA80-386EEAD2F219}',
    r'Software\Classes\CLSID\{D10C6CD7-4123-4D99-A508-F6FCB5565B47}',
]


def _context_menu_entry():
    from pathlib import Path
    if getattr(sys, 'frozen', False):
        return sys.executable, sys.executable
    base_dir = Path(__file__).parent
    cmd = f'"{sys.executable}" "{base_dir / "context_menu_helper.py"}"'
    return cmd, str(base_dir / 'icon.ico')


def _delete_verb(root, reg_path):
    import winreg
    for subkey in [reg_path + r'\command', reg_path]:
        try:
            winreg.DeleteKey(root, subkey)
        except FileNotFoundError:
            pass


def _delete_tree(root, reg_path):
    """Рекурсивное удаление ключа со всеми подключами (для CLSID из COM-эксперимента)."""
    import winreg
    try:
        k = winreg.OpenKey(root, reg_path, 0, winreg.KEY_READ | winreg.KEY_WRITE)
    except FileNotFoundError:
        return
    while True:
        try:
            sub = winreg.EnumKey(k, 0)
        except OSError:
            break
        k.Close()
        _delete_tree(root, reg_path + '\\' + sub)
        try:
            k = winreg.OpenKey(root, reg_path, 0, winreg.KEY_READ | winreg.KEY_WRITE)
        except FileNotFoundError:
            return
    k.Close()
    try:
        winreg.DeleteKey(root, reg_path)
    except FileNotFoundError:
        pass


def _set_verb(root, reg_path, label, icon_path, cmd, extra=None):
    import winreg
    key = winreg.CreateKey(root, reg_path)
    winreg.SetValueEx(key, '', 0, winreg.REG_SZ, label)
    winreg.SetValueEx(key, 'Icon', 0, winreg.REG_SZ, icon_path)
    for name, val in (extra or {}).items():
        winreg.SetValueEx(key, name, 0, winreg.REG_SZ, val)
    winreg.CloseKey(key)
    cmd_key = winreg.CreateKey(root, reg_path + r'\command')
    winreg.SetValueEx(cmd_key, '', 0, winreg.REG_SZ, cmd)
    winreg.CloseKey(cmd_key)


def _register_context_menu():
    import winreg
    try:
        cmd_prefix, icon_path = _context_menu_entry()
        # Чистка ключей старых версий (расширения / SFA / COM)
        for reg_path in LEGACY_REG_ENTRIES:
            if 'CLSID' in reg_path:
                _delete_tree(winreg.HKEY_CURRENT_USER, reg_path)
            else:
                _delete_verb(winreg.HKEY_CURRENT_USER, reg_path)
        for ext in IMAGE_EXTENSIONS:
            for name, _, _ in FILE_REG_ENTRIES:
                _delete_verb(winreg.HKEY_CURRENT_USER, rf'Software\Classes\{ext}\shell\{name}')
        for root in (winreg.HKEY_CURRENT_USER, winreg.HKEY_LOCAL_MACHINE):
            for name, _, _ in FILE_REG_ENTRIES + MEDIA_FILE_REG_ENTRIES:
                _delete_verb(root, r'Software\Classes\*\shell' + '\\' + name)
                _delete_verb(root, SFA_IMAGE + '\\' + name)

        # Глаголы на *\shell + AppliesTo работают в Windows 11 25H2,
        # даже если у расширения уже есть собственная ассоциация.
        for entries, applies_to in (
            (FILE_REG_ENTRIES, APPIES_TO_IMAGES),
            (MEDIA_FILE_REG_ENTRIES, APPLIES_TO_MEDIA),
        ):
            for name, label, flag in entries:
                reg_path = r'Software\Classes\*\shell' + '\\' + name
                _set_verb(winreg.HKEY_CURRENT_USER, reg_path, label, icon_path,
                          f'{cmd_prefix} {flag} "%1"',
                          extra={'AppliesTo': applies_to, 'MultiSelectModel': 'Player'})

        for reg_path, label, flag in DIR_REG_ENTRIES:
            arg = '"%V"' if 'Background' in reg_path else '"%1"'
            _set_verb(winreg.HKEY_CURRENT_USER, reg_path, label, icon_path, f'{cmd_prefix} {flag} {arg}')
    except Exception as e:
        print(f'Ошибка регистрации меню: {e}')


def _unregister_context_menu():
    import winreg
    try:
        for root in (winreg.HKEY_CURRENT_USER, winreg.HKEY_LOCAL_MACHINE):
            for reg_path in LEGACY_REG_ENTRIES:
                if 'CLSID' in reg_path:
                    _delete_tree(root, reg_path)
                else:
                    _delete_verb(root, reg_path)
            for name, _, _ in FILE_REG_ENTRIES + MEDIA_FILE_REG_ENTRIES:
                _delete_verb(root, r'Software\Classes\*\shell' + '\\' + name)
                _delete_verb(root, SFA_IMAGE + '\\' + name)
            for reg_path, _, _ in DIR_REG_ENTRIES:
                _delete_verb(root, reg_path)
            for ext in IMAGE_EXTENSIONS:
                for name, _, _ in FILE_REG_ENTRIES:
                    _delete_verb(root, rf'Software\Classes\{ext}\shell\{name}')
    except Exception as e:
        print(f'Ошибка удаления меню: {e}')


PID_FILE = None


def _kill_process_on_port(port):
    import subprocess
    if sys.platform != 'win32':
        found = False
        try:
            import psutil
            # На Linux без прав psutil отдаёт pid=None — тогда падаем на lsof
            for conn in psutil.net_connections(kind='tcp'):
                if conn.laddr and conn.laddr.port == port and conn.pid:
                    terminate_process(conn.pid)
                    found = True
        except Exception:
            pass
        if not found:
            try:
                r = subprocess.run(['lsof', '-t', '-i', f'tcp:{port}'], capture_output=True, text=True)
                for line in r.stdout.splitlines():
                    pid = line.strip()
                    if pid.isdigit():
                        terminate_process(int(pid))
            except Exception:
                pass
        return
    try:
        r = subprocess.run(['netstat', '-ano'], capture_output=True, text=True)
        target = f'127.0.0.1:{port}'
        for line in r.stdout.splitlines():
            if 'LISTENING' in line and target in line:
                pid = line.strip().split()[-1]
                if pid and pid != '0':
                    import os as _os
                    if pid != str(_os.getpid()):
                        subprocess.run(['taskkill', '/F', '/PID', pid], capture_output=True)
    except Exception:
        pass


kernel32 = ctypes.windll.kernel32 if sys.platform == 'win32' else None

# Keep WNDPROC callback objects alive (prevent GC)
_wndproc_refs = []


def _set_taskbar_identity():
    """AppUserModelID: панель задач группирует окно под своим значком и названием,
    а не под pythonw.exe/Python."""
    try:
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID('Tokenatra.App')
    except Exception:
        pass


def _setup_webview2_gpu():
    """WebView2 (Edge) на гибридных ноутбуках может выбрать встроенную видеокарту
    или уйти в программный рендеринг (swiftshader) — большой канвас тогда тормозит.
    Принудительно включаем аппаратное ускорение через официальные аргументы WebView2
    и пишем GpuPreference=2 (дискретная карта) в реестре по ПОЛНОМУ пути процесса:
    и для приложения, и для актуального msedgewebview2.exe (Windows сопоставляет
    UserGpuPreferences по полному пути, голое имя файла игнорируется).

    Если лаги остались: Параметры Windows → Система → Дисплей → Графика →
    добавить msedgewebview2.exe (C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\
    Application\\*\\msedgewebview2.exe) → «Высокая производительность»."""
    try:
        args = os.environ.get('WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS', '')
        for sw in ('--use-angle=d3d11', '--enable-gpu-rasterization'):
            if sw not in args:
                args = (args + ' ' + sw).strip()
        os.environ['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = args
    except Exception:
        pass
    try:
        import winreg
        key_path = r'Software\Microsoft\DirectX\UserGpuPreferences'

        def set_pref(exe_path):
            key = winreg.CreateKey(winreg.HKEY_CURRENT_USER, key_path)
            winreg.SetValueEx(key, exe_path, 0, winreg.REG_SZ, 'GpuPreference=2;')
            winreg.CloseKey(key)

        # 1) Сам процесс приложения — полный путь
        set_pref(sys.executable)

        # 2) Актуальный(ые) msedgewebview2.exe. Версия берётся из реестра EdgeUpdate
        #    (привязка по пути не переживает автообновления WebView2, поэтому
        #    переписываем запись на каждом запуске).
        versions = []
        clients_keys = (
            r'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-27E8-4D08-8A21-E6FBEFF3DFB0}',
            r'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-27E8-4D08-8A21-E6FBEFF3DFB0}',
        )
        for hive in (winreg.HKEY_LOCAL_MACHINE, winreg.HKEY_CURRENT_USER):
            for sub in clients_keys:
                try:
                    with winreg.OpenKey(hive, sub) as k:
                        ver, _ = winreg.QueryValueEx(k, 'pv')
                    if ver and ver not in versions:
                        versions.append(ver)
                except OSError:
                    continue
        webview_root = r'C:\Program Files (x86)\Microsoft\EdgeWebView\Application'
        found = set()
        for v in versions:
            exe = os.path.join(webview_root, v, 'msedgewebview2.exe')
            if os.path.exists(exe):
                found.add(exe)
        # Резерв: любые установленные версии в каталоге рантайма
        if os.path.isdir(webview_root):
            for name in os.listdir(webview_root):
                exe = os.path.join(webview_root, name, 'msedgewebview2.exe')
                if os.path.isfile(exe):
                    found.add(exe)
        for exe in sorted(found):
            set_pref(exe)
    except Exception:
        pass


def _nuclear_exit():
    if PID_FILE and PID_FILE.exists():
        try:
            PID_FILE.unlink()
        except Exception:
            pass
    if sys.platform == 'win32':
        _unregister_context_menu()


def main():
    import os as _os
    import signal
    import subprocess
    import threading
    import time
    import webview
    from server import app, BASE_DIR, RESOURCE_DIR, USER_DATA_DIR
    from updater import start_background_tasks

    global PID_FILE

    if sys.platform == 'win32':
        _set_taskbar_identity()
        _setup_webview2_gpu()

    if '--tune-gpu' in sys.argv:
        from server import cli_tune_gpu
        cli_tune_gpu()
        return

    PORT = 7878
    URL = f'http://localhost:{PORT}'
    SPLASH_URL = f'http://localhost:{PORT}/splash'

    if kernel32 is not None:
        kernel32.SetConsoleCtrlHandler(
            ctypes.CFUNCTYPE(ctypes.c_bool, ctypes.c_uint)(lambda event: (kernel32.TerminateProcess(kernel32.GetCurrentProcess(), 0), True)[1]),
            1
        )

    PID_FILE = USER_DATA_DIR / 'app.pid'

    if PID_FILE.exists():
        try:
            old_pid = int(PID_FILE.read_text())
            if old_pid != _os.getpid():
                terminate_process(old_pid)
                time.sleep(0.3)
        except Exception:
            pass

    _kill_process_on_port(PORT)

    try:
        PID_FILE.write_text(str(_os.getpid()))
    except Exception:
        pass

    if sys.platform == 'win32':
        _register_context_menu()
    # The on-disk throttle has no cached result to restore in a new process.
    # Always perform one real check for each application session.
    start_background_tasks(force=True, delay=15)

    flask_thread = threading.Thread(
        target=lambda: app.run(host='127.0.0.1', port=PORT, debug=False, threaded=True, use_reloader=False),
        daemon=True
    )
    flask_thread.start()

    deadline = time.time() + 15
    while time.time() < deadline:
        try:
            import urllib.request
            urllib.request.urlopen(SPLASH_URL, timeout=1)
            break
        except Exception:
            time.sleep(0.1)

    class WindowApi:
        def minimize(self):
            _window.minimize()
        def maximize(self):
            _window.maximize()
        def restore(self):
            _window.restore()
        def destroy(self):
            _window.destroy()
        def toggle_fullscreen(self):
            _window.toggle_fullscreen()

    window = webview.create_window(
        title='Tokenatra',
        url=SPLASH_URL,
        width=1280,
        height=800,
        min_size=(800, 600),
        resizable=True,
        text_select=False,
        frameless=False,
        # Windows: прячем до настройки тайтлбара/иконки; на Linux/macOS
        # событие loaded у GTK/Cocoa ненадёжно — создаём окно сразу видимым.
        hidden=(sys.platform == 'win32'),
        js_api=WindowApi(),
    )

    _window = window
    app.window_ref = window

    def _set_icon():
        try:
            icon_path = str(RESOURCE_DIR / 'icon.ico')
            native = window.native
            if not native or not hasattr(native, 'Handle'):
                return
            hwnd_int = native.Handle.ToInt64() if hasattr(native.Handle, 'ToInt64') else native.Handle.ToInt32()
            hwnd = ctypes.c_void_p(hwnd_int)
            ctypes.windll.user32.LoadImageW.restype = ctypes.c_void_p
            ctypes.windll.user32.SendMessageW.restype = ctypes.c_void_p
            # Маленькая иконка (16px) — панель задач, большая (32px) — Alt+Tab
            hicon_small = ctypes.windll.user32.LoadImageW(None, ctypes.c_wchar_p(icon_path), 1, 16, 16, 0x00000010)
            hicon_big = ctypes.windll.user32.LoadImageW(None, ctypes.c_wchar_p(icon_path), 1, 32, 32, 0x00000010)
            if not hicon_small:
                hicon_small = hicon_big
            if hicon_small:
                ctypes.windll.user32.SendMessageW(hwnd, 0x0080, 0, hicon_small)   # ICON_SMALL
            if hicon_big:
                ctypes.windll.user32.SendMessageW(hwnd, 0x0080, 1, hicon_big)     # ICON_BIG
                ctypes.windll.user32.SetClassLongPtrW(hwnd, -14, hicon_big)       # GCLP_HICON
            if hicon_small:
                ctypes.windll.user32.SetClassLongPtrW(hwnd, -34, hicon_small)     # GCLP_HICONSM
        except Exception:
            pass

    def _on_loaded():
        _set_icon()
        if sys.platform != 'win32':
            try:
                window.show()
            except Exception:
                pass
            return
        try:
            native = window.native
            if native and hasattr(native, 'Handle'):
                hwnd_int = native.Handle.ToInt64() if hasattr(native.Handle, 'ToInt64') else native.Handle.ToInt32()
                hwnd = ctypes.c_void_p(hwnd_int)

                # Dark title bar
                try:
                    ctypes.windll.dwmapi.DwmSetWindowAttribute(
                        hwnd, 20, ctypes.byref(ctypes.c_int(1)), 4
                    )
                except Exception:
                    pass

                # Hide DWM NC rendering so our content shows through
                try:
                    ctypes.windll.dwmapi.DwmSetWindowAttribute(
                        hwnd, 2, ctypes.byref(ctypes.c_int(1)), 4
                    )
                except Exception:
                    pass

                # Install WndProc to handle NCCALCSIZE only
                NC_TOP = 0
                NC_EDGE = 0
                WNDPROC = ctypes.WINFUNCTYPE(
                    ctypes.c_long, ctypes.c_void_p, ctypes.c_uint, ctypes.c_void_p, ctypes.c_void_p
                )
                _CallWindowProc = ctypes.windll.user32.CallWindowProcW
                _CallWindowProc.restype = ctypes.c_long
                _CallWindowProc.argtypes = [
                    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint, ctypes.c_void_p, ctypes.c_void_p
                ]
                _DefWindowProc = ctypes.windll.user32.DefWindowProcW
                _DefWindowProc.restype = ctypes.c_long
                _DefWindowProc.argtypes = [
                    ctypes.c_void_p, ctypes.c_uint, ctypes.c_void_p, ctypes.c_void_p
                ]
                _GetWindowRect = ctypes.windll.user32.GetWindowRect
                _GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
                _IsZoomed = ctypes.windll.user32.IsZoomed
                _IsZoomed.argtypes = [ctypes.c_void_p]

                try:
                    _SetWindowLong = ctypes.windll.user32.SetWindowLongPtrW
                    _SetWindowLong.restype = ctypes.c_void_p
                    _SetWindowLong.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p]
                except AttributeError:
                    _SetWindowLong = ctypes.windll.user32.SetWindowLongW
                    _SetWindowLong.restype = ctypes.c_long

                @WNDPROC
                def _hook(h, msg, wp, lp):
                    if msg == 0x0084 and not _IsZoomed(h):  # WM_NCHITTEST: native resize edges
                        try:
                            x = ctypes.c_short(lp & 0xFFFF).value
                            y = ctypes.c_short((lp >> 16) & 0xFFFF).value
                            rect = ctypes.wintypes.RECT()
                            if _GetWindowRect(h, ctypes.byref(rect)):
                                edge = 6
                                top = y < rect.top + edge
                                bottom = y >= rect.bottom - edge
                                left = x < rect.left + edge
                                right = x >= rect.right - edge
                                if top and left:
                                    return 13  # HTTOPLEFT
                                if top and right:
                                    return 14  # HTTOPRIGHT
                                if bottom and left:
                                    return 16  # HTBOTTOMLEFT
                                if bottom and right:
                                    return 17  # HTBOTTOMRIGHT
                                if left:
                                    return 10  # HTLEFT
                                if right:
                                    return 11  # HTRIGHT
                                if top:
                                    return 12  # HTTOP
                                if bottom:
                                    return 15  # HTBOTTOM
                        except Exception:
                            pass
                    if msg == 0x0083:  # WM_NCCALCSIZE
                        try:
                            r = ctypes.cast(lp, ctypes.POINTER(ctypes.wintypes.RECT))
                            r[0].top += NC_TOP
                            r[0].left += NC_EDGE
                            r[0].right -= NC_EDGE
                            r[0].bottom -= NC_EDGE
                            return 0
                        except Exception:
                            pass
                    if msg == 0x8001:  # WM_APP+1 — drag from Flask
                        try:
                            ctypes.windll.user32.ReleaseCapture()
                            ctypes.windll.user32.SendMessageW(h, 0x00A1, 2, 0)
                        except Exception:
                            pass
                        return 0
                    return _CallWindowProc(orig, h, msg, wp, lp)

                @WNDPROC
                def _dummy(h, msg, wp, lp):
                    return _DefWindowProc(h, msg, wp, lp)
                orig = _SetWindowLong(hwnd, -4, ctypes.cast(_dummy, ctypes.c_void_p))
                _SetWindowLong(hwnd, -4, ctypes.cast(_hook, ctypes.c_void_p))
                _wndproc_refs.append((_hook, _dummy))

                ctypes.windll.user32.SetWindowPos(hwnd, 0, 0, 0, 0, 0,
                    0x0020 | 0x0002 | 0x0004 | 0x0001)
        except Exception:
            import traceback
            traceback.print_exc()
        try:
            window.show()
        except Exception:
            pass
    window.events.loaded += _on_loaded

    window.events.closing += _nuclear_exit
    window.events.closed += _nuclear_exit

    # Let pywebview select Cocoa/WebKit2GTK on Unix; EdgeChromium is Windows-only.
    gui = 'edgechromium' if sys.platform == 'win32' else None
    start_kwargs = {'gui': gui}
    if sys.platform != 'win32':
        try:
            import inspect
            _icon = RESOURCE_DIR / 'logo.png'
            if _icon.exists() and 'icon' in inspect.signature(webview.start).parameters:
                start_kwargs['icon'] = str(_icon)
        except Exception:
            pass
    webview.start(**start_kwargs, debug=False)

    _nuclear_exit()


if __name__ == '__main__':
    if any(a in sys.argv for a in ('--remove-bg', '--to-webp', '--to-ogg', '--folder-to-webp')):
        from context_menu_helper import main as helper_main
        helper_main()
    else:
        main()
