# PyInstaller runtime hook (Linux): заставляем pywebview использовать
# вшитые в бандл WebKit2-4.0 / Soup-2.4 вместо системных 4.1 / 3.0.
# Системные библиотеки несовместимы с нашей GLib и роняют приложение.
import gi

_original_require_version = gi.require_version


def _require_version(namespace, version):
    version = str(version)
    if namespace == 'WebKit2' and version == '4.1':
        raise ValueError('WebKit2 4.1 не поставляется с Tokenatra')
    if namespace == 'Soup' and version == '3.0':
        raise ValueError('Soup 3.0 не поставляется с Tokenatra')
    return _original_require_version(namespace, version)


gi.require_version = _require_version
