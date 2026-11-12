#!/usr/bin/env python3
"""A stand-in ntfy server for the tests (ADR 0055).

    python3 tests/e2e/ntfy_sink.py PORT [HOST]

A POST to any path other than /_seen is an alert: it is kept (path, priority, title, tags, whether it had an authorization header
and its token, body) and answered 200. `GET /_seen` returns what it has kept as JSON, and `DELETE /_seen` forgets it. `POST /_status/N`
makes every later alert answer with status N (200 puts it back), so a test can make delivery fail. It listens on 127.0.0.1 unless it is
given a HOST (the isolation check gives 0.0.0.0, inside a container on the alerter's network).
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

seen = []
status = [200]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, code, body=b"", ctype="application/json"):
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/_seen":
            return self._send(200, json.dumps(seen).encode())
        self._send(404)

    def do_DELETE(self):
        if self.path == "/_seen":
            seen.clear()
            return self._send(204)
        self._send(404)

    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        if self.path.startswith("/_status/"):
            status[0] = int(self.path.rsplit("/", 1)[1])
            return self._send(204)
        auth = self.headers.get("authorization") or ""
        seen.append(
            {
                "path": self.path,
                "priority": self.headers.get("priority"),
                "title": self.headers.get("title"),
                "tags": self.headers.get("tags"),
                "authorization": auth,
                "body": body,
            }
        )
        self._send(status[0], b"{}")


if __name__ == "__main__":
    HTTPServer((sys.argv[2] if len(sys.argv) > 2 else "127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
