#!/usr/bin/env python3
"""A stand-in for GitHub's latest-release API for the tests (ADR 0055, D11).

    python3 tests/e2e/releases_mock.py PORT [TAG]

`GET /repos/Thewuuryar/askbar/releases/latest` answers with the tag (default `v0.0.1`, older than any build, so no alert is sent).
The request's `User-Agent` and header names are kept: `GET /_seen` returns them as JSON. `POST /_tag/TAG` changes the tag.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

tag = [sys.argv[2] if len(sys.argv) > 2 else "v0.0.1"]
seen = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, code, body=b""):
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/repos/Thewuuryar/askbar/releases/latest":
            seen.append({"user_agent": self.headers.get("user-agent"), "headers": sorted(k.lower() for k in self.headers.keys())})
            return self._send(200, json.dumps({"tag_name": tag[0], "draft": False, "prerelease": False}).encode())
        if self.path == "/_seen":
            return self._send(200, json.dumps(seen).encode())
        self._send(404)

    def do_POST(self):
        if self.path.startswith("/_tag/"):
            tag[0] = self.path.rsplit("/", 1)[1]
            return self._send(204)
        self._send(404)


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
