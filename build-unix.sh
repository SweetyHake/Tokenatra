#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "$ROOT_DIR"

UNAME_S="$(uname -s)"

# Зависимости ставим один раз: если всё импортируется и PyInstaller на месте — пропускаем.
if ! python3 -c 'import numpy, PIL, flask, webview, psutil, imageio_ffmpeg, onnxruntime' >/dev/null 2>&1 \
   || ! python3 -m PyInstaller --version >/dev/null 2>&1; then
    python3 -m pip install -r requirements.txt pyinstaller
fi

# macOS: .icns из logo.png (нужен Pillow — поэтому после установки зависимостей).
if [ "$UNAME_S" = "Darwin" ]; then
    python3 - <<'PY' || printf 'Warning: failed to generate icon.icns from logo.png\n' >&2
from PIL import Image

img = Image.open('logo.png').convert('RGBA')
side = max(img.size)
canvas = Image.new('RGBA', (side, side), (0, 0, 0, 0))
canvas.paste(img, ((side - img.width) // 2, (side - img.height) // 2), img)
canvas.resize((1024, 1024), Image.LANCZOS).save('icon.icns', format='ICNS')
PY
fi

python3 -m PyInstaller build.spec --noconfirm --clean

if [ "$UNAME_S" = "Darwin" ]; then
    printf 'Build created in dist/Tokenatra.app/\n'
else
    printf 'Build created in dist/Tokenatra/\n'
fi

# DMG для macOS: .app + симлинк на Applications (drag-and-drop установка).
# hdiutil есть в macOS из коробки; шаг выполняется только на Darwin.
if [ "$UNAME_S" = "Darwin" ] && [ -d dist/Tokenatra.app ]; then
    APP_ARCH="$(uname -m)"
    APP_VERSION="$(python3 -c 'from version import __version__; print(__version__)')"
    DMG_PATH="dist/Tokenatra_v${APP_VERSION}_macos_${APP_ARCH}.dmg"
    DMG_STAGING="dist/dmg_staging"
    rm -f "$DMG_PATH"
    rm -rf "$DMG_STAGING"
    mkdir -p "$DMG_STAGING"
    cp -R dist/Tokenatra.app "$DMG_STAGING/"
    ln -s /Applications "$DMG_STAGING/Applications"
    if hdiutil create -volname "Tokenatra" -srcfolder "$DMG_STAGING" -ov -format UDZO "$DMG_PATH"; then
        printf 'DMG создан: %s\n' "$DMG_PATH"
    else
        printf 'Warning: hdiutil failed, DMG not created (app bundle is still valid)\n' >&2
    fi
    rm -rf "$DMG_STAGING"
fi

# AppImage для Linux: самодостаточный файл с иконкой и .desktop —
# привычный формат распространения вне репозиториев дистрибутивов.
if [ "$UNAME_S" = "Linux" ] && [ -d dist/Tokenatra ]; then
    APP_ARCH="$(uname -m)"
    APP_VERSION="$(python3 -c 'from version import __version__; print(__version__)')"
    APPIMAGE_PATH="dist/Tokenatra_v${APP_VERSION}_linux_${APP_ARCH}.AppImage"
    APPDIR="dist/Tokenatra.AppDir"
    CACHE_DIR="${HOME}/.cache/tokenatra-build"
    TOOL="$CACHE_DIR/appimagetool-${APP_ARCH}.AppImage"

    case "$APP_ARCH" in
        x86_64)  TOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" ;;
        aarch64) TOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-aarch64.AppImage" ;;
        *)       TOOL_URL="" ;;
    esac

    rm -rf "$APPDIR"
    mkdir -p "$APPDIR/usr/opt"
    cp -R dist/Tokenatra "$APPDIR/usr/opt/Tokenatra"

    cat > "$APPDIR/AppRun" << 'APPRUN_EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
# Typelib'ы, собранные PyInstaller-хуком gi (если есть), приоритетнее системных.
for d in "$HERE/usr/opt/Tokenatra/_internal/girepository-1.0" \
         "$HERE/usr/opt/Tokenatra/girepository-1.0"; do
    [ -d "$d" ] && GI_TYPELIB_PATH="$d${GI_TYPELIB_PATH:+:$GI_TYPELIB_PATH}"
done
export GI_TYPELIB_PATH
exec "$HERE/usr/opt/Tokenatra/Tokenatra" "$@"
APPRUN_EOF
    chmod +x "$APPDIR/AppRun"

    cat > "$APPDIR/tokenatra.desktop" << 'DESKTOP_EOF'
[Desktop Entry]
Type=Application
Name=Tokenatra
Comment=Удаление фона и редактор токенов для VTT
Exec=AppRun
Icon=tokenatra
Terminal=false
Categories=Graphics;RasterGraphics;
StartupWMClass=Tokenatra
DESKTOP_EOF

    ICON_PNG="dist/tokenatra-icon.png"
    if python3 - "$ICON_PNG" <<'PY'
import sys
from PIL import Image

img = Image.open('logo.png').convert('RGBA')
side = max(img.size)
canvas = Image.new('RGBA', (side, side), (0, 0, 0, 0))
canvas.paste(img, ((side - img.width) // 2, (side - img.height) // 2), img)
canvas.resize((512, 512), Image.LANCZOS).save(sys.argv[1])
PY
    then
        cp "$ICON_PNG" "$APPDIR/tokenatra.png"
    else
        cp logo.png "$APPDIR/tokenatra.png"
    fi
    ln -s tokenatra.png "$APPDIR/.DirIcon"

    if [ -z "$TOOL_URL" ]; then
        printf 'Warning: unsupported architecture %s, skipping AppImage\n' "$APP_ARCH" >&2
    else
        if [ ! -x "$TOOL" ]; then
            mkdir -p "$CACHE_DIR"
            rm -f "$TOOL"
            if command -v curl >/dev/null 2>&1; then
                curl -fL --retry 3 -o "$TOOL" "$TOOL_URL" || true
            else
                wget -qO "$TOOL" "$TOOL_URL" || true
            fi
            chmod +x "$TOOL" 2>/dev/null || true
        fi
        if [ -x "$TOOL" ]; then
            rm -f "$APPIMAGE_PATH"
            # --appimage-extract-and-run: работает даже без libfuse2 на хосте
            if "$TOOL" --appimage-extract-and-run "$APPDIR" "$APPIMAGE_PATH"; then
                printf 'AppImage создан: %s\n' "$APPIMAGE_PATH"
            else
                printf 'Warning: appimagetool failed, AppImage not created (dist/Tokenatra/ is still valid)\n' >&2
            fi
        else
            printf 'Warning: failed to download appimagetool, skipping AppImage\n' >&2
        fi
    fi
    rm -rf "$APPDIR"
fi
