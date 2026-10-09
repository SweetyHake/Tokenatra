"""Системный трей Tokenatra (Windows, pystray).

Трей — единственная «ручка» к приложению, спрятанному через close_to_tray,
поэтому контроллер деградирует мягко: без pystray (или не на Windows)
available() == False и окно закрывается как раньше.

Жизненный цикл pystray: остановленный Icon непригоден для повторного запуска,
поэтому после hide() show() строит иконку заново (run_detached живёт в своём
потоке). Пока приложение работает, иконка не останавливается: показывается на
старте и живёт в трее постоянно (как у Discord), hide() — только реальный выход.
"""

import sys
import threading

_available = None


def available():
    """Установлен ли pystray и поддерживается ли трей на этой платформе."""
    global _available
    if _available is None:
        if sys.platform != 'win32':
            _available = False
        else:
            try:
                import pystray  # noqa: F401
                _available = True
            except Exception:
                _available = False
    return _available


class TrayController:
    """Иконка в трее: «Открыть Tokenatra» (двойной клик), «Загрузить/Выгрузить
    модель» и «Закрыть».

    Колбэки вызываются из потока pystray: on_open/on_toggle_model/on_quit.
    is_model_loaded — колбэк-читалка (метка пункта модели и её действие
    переоцениваются при каждом открытии меню).
    """

    def __init__(self, icon_path, on_open, on_quit,
                 is_model_loaded=None, on_toggle_model=None):
        self._icon_path = icon_path
        self._on_open = on_open
        self._on_quit = on_quit
        self._is_model_loaded = is_model_loaded
        self._on_toggle_model = on_toggle_model
        self._icon = None
        self._lock = threading.Lock()

    def _build_icon(self, tooltip):
        import pystray
        from PIL import Image
        img = Image.open(self._icon_path).convert('RGBA')
        img = img.resize((64, 64), Image.LANCZOS)
        menu = pystray.Menu(
            pystray.MenuItem('Открыть Tokenatra', self._open_clicked, default=True),
            pystray.MenuItem(self._model_label, self._toggle_model_clicked,
                             enabled=lambda item: bool(self._on_toggle_model)),
            pystray.MenuItem('Закрыть', self._quit_clicked),
        )
        return pystray.Icon('tokenatra', img, tooltip, menu)

    def _model_label(self, item):
        if self._is_model_loaded:
            try:
                if self._is_model_loaded():
                    return 'Выгрузить модель'
            except Exception:
                pass
        return 'Загрузить модель'

    def _toggle_model_clicked(self, icon=None, item=None):
        if self._on_toggle_model:
            try:
                self._on_toggle_model()
            except Exception:
                pass

    def _open_clicked(self, icon=None, item=None):
        if self._on_open:
            try:
                self._on_open()
            except Exception:
                pass

    def _quit_clicked(self, icon=None, item=None):
        if self._on_quit:
            try:
                self._on_quit()
            except Exception:
                pass

    def show(self, tooltip='Tokenatra'):
        """Показать иконку. False — трей недоступен или не создался.
        Идемпотентно: живая иконка не пересоздаётся, обновляется только тултип
        (пересоздание убило бы привязку меню)."""
        if not available():
            return False
        with self._lock:
            if self._icon is not None:
                try:
                    self._icon.title = tooltip
                except Exception:
                    pass
                return True
            try:
                self._icon = self._build_icon(tooltip)
                # run_detached: цикл сообщений в отдельном потоке
                self._icon.run_detached()
                return True
            except Exception:
                self._icon = None
                return False

    def _stop_locked(self):
        icon, self._icon = self._icon, None
        if icon is None:
            return
        try:
            icon.stop()
        except Exception:
            pass

    def hide(self):
        with self._lock:
            self._stop_locked()

    def set_tooltip(self, text):
        icon = self._icon
        if icon is None:
            return
        try:
            icon.title = text
        except Exception:
            pass
