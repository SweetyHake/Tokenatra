#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "$ROOT_DIR"

python3 -m pip install -r requirements-cross-platform.txt pyinstaller
python3 -m PyInstaller build.spec --noconfirm --clean
printf 'Build created in dist/Tokenatra/\n'
