"""The HTTP handler setup the test fixtures share (ADR 0055): silent logging and a JSON reply.

The fixtures run as scripts (python3 tests/e2e/x.py, from any directory), so this module is found next to them.
"""
from http.server import BaseHTTPRequestHandler


class Stub(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, code, body=b""):
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
