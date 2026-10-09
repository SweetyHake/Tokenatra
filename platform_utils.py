"""Platform-neutral helpers shared by the desktop and server layers."""

import os
import signal
import sys
import tempfile
from pathlib import Path


def user_data_dir() -> Path:
    """Return a writable per-user application data directory."""
    if sys.platform == "win32":
        root = os.environ.get("LOCALAPPDATA") or os.environ.get("APPDATA")
        base = Path(root) if root else Path.home() / "AppData" / "Local"
    elif sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    target = base / "Tokenatra"
    try:
        target.mkdir(parents=True, exist_ok=True)
        return target
    except OSError:
        fallback = Path(tempfile.gettempdir()) / "Tokenatra"
        fallback.mkdir(parents=True, exist_ok=True)
        return fallback


def terminate_process(pid: int) -> bool:
    """Terminate a process without relying on a platform shell."""
    if pid <= 0 or pid == os.getpid():
        return False
    try:
        if sys.platform == "win32":
            import subprocess
            # CREATE_NO_WINDOW: windowed-exe без этого флага вспыхивает окном
            # терминала (kill прошлого экземпляра по app.pid при каждом старте)
            subprocess.run(
                ["taskkill", "/F", "/PID", str(pid)],
                capture_output=True,
                check=False,
                creationflags=0x08000000,  # CREATE_NO_WINDOW
            )
        else:
            os.kill(pid, signal.SIGTERM)
        return True
    except (OSError, ValueError):
        return False
