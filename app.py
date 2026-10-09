import ctypes
import ctypes.wintypes
import os
import sys

from platform_utils import terminate_process
import tray_manager

# WSLg и части виртуалок: Wayland-бэкенд GTK и композитор WebKit могут
# оставить окно невидимым. Принудительный X11 + мягкий рендеринг лечат.
if sys.platform != 'win32':
    os.environ.setdefault('GDK_BACKEND', 'x11')
    os.environ.setdefault('GTK_THEME', 'Adwaita-dark')
    # DMABUF-рендерер WebKitGTK на виртуальных видеокартах даёт дрожание
    # контента при анимациях — отключаем, композитинг остаётся включённым.
    os.environ.setdefault('WEBKIT_DISABLE_DMABUF_RENDERER', '1')
    # ВНИМАНИЕ: не отключать композитинг WebKit (WEBKIT_DISABLE_COMPOSITING_MODE)
    # — без него hover-анимации и тултипы перерисовывают весь кадр,
    # что выглядит как дрожание окна (особенно в VM и при масштабе 2x).

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
        # CREATE_NO_WINDOW: иначе netstat/taskkill вспыхивают окном терминала
        # при запуске windowed-приложения (освобождение порта 7878)
        no_window = 0x08000000 if sys.platform == 'win32' else 0
        r = subprocess.run(['netstat', '-ano'], capture_output=True, text=True,
                           creationflags=no_window)
        target = f'127.0.0.1:{port}'
        for line in r.stdout.splitlines():
            if 'LISTENING' in line and target in line:
                pid = line.strip().split()[-1]
                if pid and pid != '0':
                    import os as _os
                    if pid != str(_os.getpid()):
                        subprocess.run(['taskkill', '/F', '/PID', pid], capture_output=True,
                                       creationflags=no_window)
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


def _enable_windows_dark_dialogs():
    """Нативные диалоги и меню (включая контекстное меню трея) следуют теме
    Windows: uxtheme!SetPreferredAppMode(AllowDark) + FlushMenuThemes.
    На текущих сборках Windows функции НЕ экспортируются по имени — только
    по ординалам 135/136 (тот же приём, что у TrafficMonitor/Chromium)."""
    try:
        ux = ctypes.WinDLL('uxtheme')

        def _resolve(name, ordinal, *argtypes):
            try:
                return ctypes.WINFUNCTYPE(ctypes.c_long, *argtypes)((name, ux))
            except AttributeError:
                return ctypes.WINFUNCTYPE(ctypes.c_long, *argtypes)((ordinal, ux))

        set_pam = _resolve('SetPreferredAppMode', 135, ctypes.c_int)
        flush = _resolve('FlushMenuThemes', 136)
        set_pam(1)   # PAM_ALLOW = следовать системной теме
        flush()
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
    from server import (
        app, BASE_DIR, RESOURCE_DIR, USER_DATA_DIR,
        set_tray_mode, tray_settings,
        model_loaded as server_model_loaded,
        load_session as server_load_session,
        unload_model as server_unload_model,
        listen_host,
    )
    from updater import start_background_tasks

    global PID_FILE

    if sys.platform == 'win32':
        _set_taskbar_identity()
        _setup_webview2_gpu()
        _enable_windows_dark_dialogs()
        # Тайтлбар на Windows двигается нашим Win32-хуком (см. _on_loaded).
        # Встроенная обработка pywebview-drag-region в WebView2 конфликтует:
        # pywebviewMoveWindow получает дельты от mousedown и передаёт их в
        # window.move() как абсолютные координаты — окно дёргается и после
        # отпускания ЛКМ «догоняет» курсор отложенными вызовами (прилипание).
        # Подменяем селектор на несовпадающий: инъекция остаётся, но не
        # срабатывает. На Linux/macOS класс на тайтлбаре работает как раньше.
        webview.settings['DRAG_REGION_SELECTOR'] = '.pywebview-drag-region-disabled'

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

    # Остатки embed-экспорта прошлой сессии (TTL чистит только живой реестр)
    from server import purge_embed_export_dir
    purge_embed_export_dir()

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
        target=lambda: app.run(host=listen_host(), port=PORT, debug=False, threaded=True, use_reloader=False),
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
            # GTK/mutter: iconify у maximized-окна игнорируется — сначала unmaximize
            if sys.platform != 'win32':
                native = getattr(_window, 'native', None)
                if native is not None and hasattr(native, 'unmaximize'):
                    try:
                        if native.is_maximized():
                            native.unmaximize()
                        native.iconify()
                        return
                    except Exception:
                        pass
            _window.minimize()
        def maximize(self):
            _window.maximize()
        def restore(self):
            _window.restore()
        def destroy(self):
            _force_quit.set()
            _window.destroy()
        def toggle_fullscreen(self):
            _window.toggle_fullscreen()
        def hide_to_tray(self):
            """Свернуть окно в системный трей (кнопка ✕). False — трей недоступен
            или close_to_tray выключен (фронт тогда уходит в обычный выход)."""
            if not tray_manager.available() or not tray_settings()['close_to_tray']:
                return False
            try:
                _window.hide()
            except Exception:
                return False
            return _enter_tray_mode()
        def quit_app(self):
            _force_quit.set()
            _window.destroy()

    # Linux/macOS: имя программы до старта GTK → WM_CLASS, док и меню
    # показывают «Tokenatra» с логотипом, а не «app.py» с обобщённой иконкой
    if sys.platform != 'win32':
        try:
            from gi.repository import GLib
            GLib.set_prgname('Tokenatra')
            GLib.set_application_name('Tokenatra')
        except Exception:
            pass

    window = webview.create_window(
        title='Tokenatra',
        url=SPLASH_URL,
        width=1280,
        height=800,
        min_size=(800, 600),
        resizable=True,
        text_select=False,
        # Windows: своя frameless-реализация (Win32-хуки ниже) с кастомным
        # тайтлбаром; Linux/macOS — нативные декорации: системные кнопки,
        # нативные ресайз/перемещение/сворачивание (надёжнее кастома).
        frameless=False,
        # Windows: прячем до настройки тайтлбара/иконки; на Linux/macOS
        # событие loaded у GTK/Cocoa ненадёжно — создаём окно сразу видимым.
        hidden=(sys.platform == 'win32'),
        js_api=WindowApi(),
    )

    _window = window
    app.window_ref = window

    # ===== Системный трей и экономный режим =====

    _force_quit = threading.Event()
    app.force_quit_ref = _force_quit.set

    # Тумблер «Модель в памяти» в меню трея: загрузка идёт в фоне (~7–10 с),
    # защита от повторных кликов во время загрузки
    _model_busy = threading.Semaphore(1)

    def _toggle_model_from_tray():
        if not _model_busy.acquire(blocking=False):
            return

        def _work():
            try:
                if server_model_loaded():
                    server_unload_model()
                else:
                    server_load_session()
            except Exception:
                pass
            finally:
                _model_busy.release()

        threading.Thread(target=_work, daemon=True, name='tray-model-toggle').start()

    _tray = tray_manager.TrayController(
        icon_path=str(RESOURCE_DIR / 'logo.png'),
        on_open=lambda: _exit_tray_mode(),
        on_quit=lambda: _quit_from_tray(),
        is_model_loaded=server_model_loaded,
        on_toggle_model=_toggle_model_from_tray,
    )

    # Тултип иконки трея — всегда просто «Tokenatra», без статуса модели
    def _js_call(code):
        # evaluate_js нельзя звать из UI-потока closing-хендлера (дедлок
        # WinForms) — уводим в отдельный поток
        threading.Thread(target=_js_call_sync, args=(code,), daemon=True).start()

    def _js_call_sync(code):
        time.sleep(0.4)
        try:
            window.evaluate_js(code)
        except Exception:
            pass

    def _enter_tray_mode():
        """Войти в режим трея. False — иконка не создалась (окно возвращено)."""
        set_tray_mode(True)
        if not _tray.show():
            # Без иконки окно осталось бы недоступным — возвращаем его
            set_tray_mode(False)
            try:
                window.show()
            except Exception:
                pass
            return False
        _js_call('window.TokenTray && window.TokenTray.enterIdle && window.TokenTray.enterIdle(true)')
        return True

    def _exit_tray_mode():
        try:
            window.show()
        except Exception:
            pass
        set_tray_mode(False)
        # иконка из трея не убирается — живёт весь цикл приложения
        _js_call('window.TokenTray && window.TokenTray.exitIdle && window.TokenTray.exitIdle()')

    def _quit_from_tray():
        _force_quit.set()
        _tray.hide()
        _nuclear_exit()
        try:
            window.destroy()
        except Exception:
            pass

    def _on_closing():
        # ЕДИНСТВЕННЫЙ closing-хендлер: Event.set() выполняет всех подписчиков,
        # параллельный _nuclear_exit ломал бы сворачивание в трей.
        # destroy() (обновление/выход) тоже проходит через closing —
        # различаем пути флагом _force_quit.
        if _force_quit.is_set() or not tray_manager.available() or not tray_settings()['close_to_tray']:
            _final_exit()
            return True
        try:
            window.hide()
        except Exception:
            _final_exit()
            return True
        if not _enter_tray_mode():
            _final_exit()
            return True
        return False

    # Иконка в трее живёт весь цикл приложения (как у Discord): создаётся на
    # старте и не убирается при открытом окне; ✕ при включённом close_to_tray
    # прячет окно, иконка остаётся «ручкой» обратно.
    _tray.show()

    def _final_exit():
        """Реальный выход: убрать иконку трея + обычная очистка."""
        _tray.hide()
        _nuclear_exit()

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

                _drag_state = {}

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
                    if msg == 0x8001:  # WM_APP+1 — начать перетаскивание из Flask
                        # Без модального цикла SC_MOVE: WebView2 не отдаёт нам
                        # отпускание ЛКМ, и окно «прилипало» до следующего клика.
                        # Вместо этого сами берём SetCapture и тянем окно по
                        # WM_MOUSEMOVE; WM_LBUTTONUP при нашем capture приходит
                        # гарантированно и завершает перетаскивание.
                        try:
                            user32 = ctypes.windll.user32
                            zoomed = _IsZoomed(h)
                            swap = user32.GetSystemMetrics(23)  # SM_SWAPBUTTON
                            vk_primary = 0x02 if swap else 0x01
                            # Сообщение асинхронно после mousedown: если основная
                            # кнопка уже отпущена — клик, перетаскивание не начинаем.
                            if not zoomed and user32.GetAsyncKeyState(vk_primary) & 0x8000:
                                pt = ctypes.wintypes.POINT()
                                rect = ctypes.wintypes.RECT()
                                if user32.GetCursorPos(ctypes.byref(pt)) and _GetWindowRect(h, ctypes.byref(rect)):
                                    _drag_state['dx'] = pt.x - rect.left
                                    _drag_state['dy'] = pt.y - rect.top
                                    user32.ReleaseCapture()
                                    user32.SetCapture(h)
                        except Exception:
                            pass
                        return 0
                    if msg == 0x0200 and _drag_state:  # WM_MOUSEMOVE — тянем окно
                        try:
                            pt = ctypes.wintypes.POINT()
                            if ctypes.windll.user32.GetCursorPos(ctypes.byref(pt)):
                                ctypes.windll.user32.SetWindowPos(
                                    h, 0, pt.x - _drag_state['dx'], pt.y - _drag_state['dy'],
                                    0, 0, 0x0001 | 0x0004 | 0x0010)  # NOSIZE|NOZORDER|NOACTIVATE
                        except Exception:
                            pass
                        return 0
                    if msg == 0x0202:  # WM_LBUTTONUP
                        if _drag_state:
                            _drag_state.clear()
                            try:
                                ctypes.windll.user32.ReleaseCapture()
                            except Exception:
                                pass
                        return 0
                    if msg == 0x0208 and _drag_state:  # WM_CAPTURECHANGED — capture потерян
                        _drag_state.clear()
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

    # closing перехватывает только трей-хендлер (см. _on_closing); реальный
    # выход чистится через closed
    window.events.closing += _on_closing
    window.events.closed += _final_exit

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

    _final_exit()


if __name__ == '__main__':
    if any(a in sys.argv for a in ('--remove-bg', '--to-webp', '--to-ogg', '--folder-to-webp')):
        from context_menu_helper import main as helper_main
        helper_main()
    else:
        main()
