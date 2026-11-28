"""Shared fixtures: an in-process mock provider and a temp ``$LEXI_HOME``."""

import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

# Saved keys go in files under the test's $LEXI_HOME, never the real keychain
# (set before the first lexi call, for every test).
os.environ["LEXI_KEY_STORE"] = "file"


class Mock:
    """A tiny provider: ``routes[(method, path)](body) -> (status, content_type, bytes)``."""

    def __init__(self):
        self.routes = {}
        self.requests = []  # (method, path, parsed JSON body or None)
        mock = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def _serve(self):
                n = int(self.headers.get("content-length") or 0)
                raw = self.rfile.read(n) if n else b""
                try:
                    body = json.loads(raw) if raw else None
                except ValueError:
                    body = None  # multipart upload
                mock.requests.append((self.command, self.path, body))
                route = mock.routes.get((self.command, self.path.split("?")[0]))
                if route is None:
                    status, ctype, data = 404, "application/json", b'{"error":"no route"}'
                else:
                    status, ctype, data = route(body)
                self.send_response(status)
                self.send_header("content-type", ctype)
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            do_GET = do_POST = _serve

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = "http://127.0.0.1:%d" % self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def bodies(self, method="POST"):
        return [b for m, _, b in self.requests if m == method]


def ok(obj):
    return lambda _body: (200, "application/json", json.dumps(obj).encode())


@pytest.fixture
def mock(tmp_path, monkeypatch):
    m = Mock()
    home = tmp_path / "home"
    (home / "providers").mkdir(parents=True)
    (home / "providers" / "providers-config.yaml").write_text(
        "providers:\n"
        f"  p:\n    url: {m.url}\n    api: responses\n    api_key: k\n    default_model: m\n"
        f"  img:\n    url: {m.url}\n    api: image-generations\n    api_key: k\n"
        "    default_model: gpt-image-1\n"
    )
    monkeypatch.setenv("LEXI_HOME", str(home))
    m.home = home
    yield m
    m.server.shutdown()


@pytest.fixture
def home(tmp_path, monkeypatch):
    """An empty ``$LEXI_HOME`` (the first lexi call seeds the defaults)."""
    h = tmp_path / "lexi-home"
    h.mkdir()
    monkeypatch.setenv("LEXI_HOME", str(h))
    return h
