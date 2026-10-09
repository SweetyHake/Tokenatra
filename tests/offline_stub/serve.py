"""Мини-сервер для офлайн-теста студии: / -> parent.html,
/modules/tokenatra-connector/studio/* -> собранная студия модуля."""
import http.server
import os
import posixpath
import sys
import urllib.parse

ROOT = os.path.dirname(os.path.abspath(__file__))
STUDIO = os.path.normpath(os.path.join(ROOT, '..', '..', 'foundry-module', 'tokenatra-connector', 'studio'))
STUDIO_PREFIX = '/modules/tokenatra-connector/studio/'


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()

    def translate_path(self, path):
        path = path.split('?', 1)[0].split('#', 1)[0]
        path = urllib.parse.unquote(path)
        if path == '/':
            return os.path.join(ROOT, 'parent.html')
        if path.startswith(STUDIO_PREFIX):
            rel = path[len(STUDIO_PREFIX):]
            return os.path.normpath(os.path.join(STUDIO, rel))
        return os.path.join(ROOT, path.lstrip('/'))


if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8899
    http.server.ThreadingHTTPServer(('127.0.0.1', port), Handler).serve_forever()
