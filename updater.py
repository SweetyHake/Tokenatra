import json
import logging
import os
import sys
import tempfile
import time
from pathlib import Path
from threading import Lock, Thread
from urllib.request import Request, urlopen

from version import __version__, APP_NAME, GITHUB_REPO

_logger = logging.getLogger(__name__)

if not logging.getLogger().handlers:
    logging.basicConfig(
        level=logging.WARNING,
        format="%(asctime)s %(name)s %(levelname)s: %(message)s",
    )

if getattr(sys, "frozen", False):
    BASE_DIR = Path(sys.executable).parent
else:
    BASE_DIR = Path(__file__).parent

_CHECK_INTERVAL_HOURS = 6


class _UpdateState:
    """Thread-safe container for update check / download state."""

    def __init__(self):
        self.lock = Lock()
        self.checked = False
        self.check_error = False
        self.available = None
        self.url = None
        self.tag = None
        self.download_progress = -1
        self.download_active = False
        self.download_done = False
        self.download_error = None
        self.download_path = None
        self.download_kind = None

    def snapshot(self):
        with self.lock:
            return {
                "update_checked": self.checked,
                "check_error": self.check_error,
                "update_available": self.available,
                "update_url": self.url,
                "update_tag": self.tag,
                "current_version": __version__,
                "download_progress": self.download_progress,
                "download_active": self.download_active,
                "download_done": self.download_done,
                "download_error": self.download_error,
                "download_path": self.download_path,
                "download_kind": self.download_kind,
            }

    def complete_check(self, available, url=None, tag=None):
        with self.lock:
            self.checked = True
            self.check_error = False
            self.available = available
            self.url = url
            self.tag = tag

    def set_check_error(self):
        with self.lock:
            self.check_error = True

    def reset_check(self):
        """Сброс перед ручной проверкой: чтобы UI ждал свежий результат,
        а не показывал устаревшее состояние от фонового/троттл-пропущенного чека."""
        with self.lock:
            self.checked = False
            self.check_error = False
            self.available = False
            self.url = None
            self.tag = None
            # Скачанное/повреждённое обновление больше не актуально
            self.download_active = False
            self.download_done = False
            self.download_error = None
            self.download_path = None
            self.download_kind = None

    def get_url(self):
        with self.lock:
            return self.url

    def set_download_progress(self, pct):
        with self.lock:
            self.download_progress = pct

    def set_download_kind(self, kind):
        with self.lock:
            self.download_kind = kind

    def set_download_active(self, active):
        with self.lock:
            self.download_active = active

    def complete_download(self, error=None, path=None):
        with self.lock:
            self.download_active = False
            self.download_done = error is None
            self.download_error = error
            self.download_path = path if error is None else None
            if error is None:
                self.download_progress = 100


_state = _UpdateState()


def _api_url(path):
    return f"https://api.github.com/repos/{GITHUB_REPO}/{path}"


def _releases_url():
    return f"https://github.com/{GITHUB_REPO}/releases"


def _parse_version(v):
    try:
        parts = str(v).split(".")
        return tuple(int(p) for p in parts[:3])
    except Exception:
        return (0, 0, 0)


def _check_throttle_file():
    # В LOCALAPPDATA: в Program Files (BASE_DIR) запись запрещена обычному пользователю
    path = _update_dir() / ".last_update_check"
    try:
        if path.exists():
            age = time.time() - path.stat().st_mtime
            return age < _CHECK_INTERVAL_HOURS * 3600
    except OSError:
        pass
    return False


def _touch_throttle_file():
    try:
        (_update_dir() / ".last_update_check").touch()
    except OSError:
        pass


def _update_dir():
    """Каталог для скачанных обновлений (LOCALAPPDATA — в Program Files запись запрещена)."""
    base = os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()
    d = Path(base) / "Tokenatra" / "update"
    try:
        d.mkdir(parents=True, exist_ok=True)
    except OSError:
        d = Path(tempfile.gettempdir()) / "Tokenatra_update"
        d.mkdir(parents=True, exist_ok=True)
    return d


def _find_exe_asset(assets):
    """Лучший .exe-ассет для автообновления.
    Приоритет: установщик Inno Setup (Tokenatra_Setup_v*.exe) — он сам обновляет
    всю папку приложения. Голый Tokenatra.exe (без _internal) — только как
    запасной вариант: его нельзя копировать поверх exe приложения."""
    bare_asset = None
    for asset in assets:
        name = asset.get("name", "")
        if not name.lower().endswith(".exe"):
            continue
        if APP_NAME.lower() not in name.lower():
            continue
        if "setup" in name.lower() or "installer" in name.lower():
            return asset.get("browser_download_url")
        if bare_asset is None:
            bare_asset = asset.get("browser_download_url")
    return bare_asset


def check_for_updates(force=False):
    try:
        if not GITHUB_REPO:
            _state.complete_check(False)
            return

        if not force and _check_throttle_file():
            _logger.info("Skipping update check (throttled)")
            # Состояние НЕ трогаем: иначе первый клик «Проверить обновления» увидит
            # устаревшее «checked, обновлений нет» и не дождётся настоящей проверки
            return

        req = Request(_api_url("releases/latest"))
        req.add_header("Accept", "application/vnd.github.v3+json")
        req.add_header("User-Agent", f"{APP_NAME}/{__version__}")

        with urlopen(req, timeout=10) as r:
            data = json.loads(r.read().decode("utf-8"))

        latest_tag = data.get("tag_name", "").lstrip("v")
        assets = data.get("assets", [])

        if _parse_version(latest_tag) <= _parse_version(__version__):
            _touch_throttle_file()
            _state.complete_check(False)
            return

        url = _find_exe_asset(assets)
        if not url:
            url = data.get("html_url", _releases_url())

        _touch_throttle_file()
        _state.complete_check(True, url, latest_tag)
        _logger.info("Update available: %s", latest_tag)

    except Exception as exc:
        _logger.warning("Update check failed: %s", exc)
        _state.complete_check(False)
        _state.set_check_error()


def download_update():
    try:
        with _state.lock:
            if _state.download_active or _state.download_done:
                return
            _state.download_active = True

        url = _state.get_url()
        if not url:
            _state.set_download_active(False)
            raise ValueError("No download URL")

        clean = url.split("?")[0]
        if not clean.endswith(".exe"):
            _state.set_download_active(False)
            raise ValueError("Автообновление недоступно: нет exe-файла приложения. Скачайте установщик вручную с GitHub Releases")

        kind = "installer" if ("setup" in Path(clean).name.lower() or "installer" in Path(clean).name.lower()) else "bare"
        _state.set_download_kind(kind)

        upd_dir = _update_dir()
        dest = upd_dir / "Tokenatra_update.exe"
        # Удаляем только чужие/старые файлы обновлений, не трогая текущий dest
        for old in upd_dir.glob("Tokenatra_update*"):
            try:
                if old.resolve() != dest.resolve():
                    old.unlink()
            except OSError:
                pass

        req = Request(url)
        req.add_header("User-Agent", f"{APP_NAME}/{__version__}")

        with urlopen(req, timeout=300) as r:
            total = int(r.headers.get("Content-Length", 0))
            downloaded = 0
            chunk_size = 65536

            with open(dest, "wb") as f:
                while True:
                    chunk = r.read(chunk_size)
                    if not chunk:
                        break
                    f.write(chunk)
                    downloaded += len(chunk)
                    if total:
                        _state.set_download_progress(int(downloaded * 100 / total))

        if dest.stat().st_size < 1024 * 1024:
            dest.unlink(missing_ok=True)
            raise ValueError("Скачанный файл повреждён (слишком маленький)")

        _state.complete_download(path=str(dest))
        _logger.info("Update downloaded to %s", dest)

    except Exception as exc:
        _logger.error("Download failed: %s", exc)
        _state.complete_download(error=str(exc))


def start_background_tasks(force=False):
    Thread(target=lambda: check_for_updates(force=force), daemon=True).start()


def reset_check_state():
    _state.reset_check()


def get_status():
    return _state.snapshot()
