#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "$ROOT_DIR"

python3 -m pip install -r requirements.txt

if ! python3 -c 'import webview' 2>/dev/null; then
    echo 'Ошибка: pywebview не импортируется — на Linux ему нужен системный WebKit2GTK.' >&2
    echo '  Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-0 gir1.2-webkit2-4.1 python3-gi' >&2
    echo '  Fedora: sudo dnf install webkit2gtk4.1 python3-gobject' >&2
    echo '  Arch: sudo pacman -S webkit2gtk-4.1 python-gobject' >&2
    exit 1
fi

exec python3 app.py
