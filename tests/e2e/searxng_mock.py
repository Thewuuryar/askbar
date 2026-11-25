#!/usr/bin/env python3
"""A stand-in searxng for the smoke test (P5A.23).

    python3 tests/e2e/searxng_mock.py PORT

POST /search with a form body (q, format=json) answers two results. A GET answers 405: the front must never put the query in a URL.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, code, body=b""):
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        form = parse_qs(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        if self.path != "/search" or form.get("format") != ["json"] or not form.get("q"):
            return self._send(400)
        results = [{"title": f"Result {i}", "url": f"http://page.example/{i}", "content": f"Snippet {i}"} for i in (1, 2)]
        self._send(200, json.dumps({"query": form["q"][0], "results": results}).encode())

    def do_GET(self):
        self._send(405)


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
