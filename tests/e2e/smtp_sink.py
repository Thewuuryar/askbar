#!/usr/bin/env python3
"""A stand-in mail server for the tests (ADR 0055), on raw sockets.

    python3 tests/e2e/smtp_sink.py SMTP_PORT HTTP_PORT [HOST]

SMTP_PORT speaks just enough SMTP to take a message: no TLS, no login (STARTTLS and AUTH are not offered, so a client that insists
on either fails). Each message is kept: the MAIL FROM and RCPT TO lines and the data. HTTP_PORT shows what it has: `GET /_seen` returns
the messages as JSON, `DELETE /_seen` forgets them, and `POST /_reject/N` makes every later recipient answer with code N (0 takes
them again). It listens on 127.0.0.1 unless it is given a HOST (the isolation check gives 0.0.0.0, inside a container on the
alerter's network).
"""
import json
import socket
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

seen = []
reject = [0]
lock = threading.Lock()


def serve_smtp(conn):
    f = conn.makefile("rwb", buffering=0)

    def say(line):
        f.write((line + "\r\n").encode())

    try:
        say("220 sink ready")
        sender = rcpt = ""
        while True:
            raw = f.readline()
            if not raw:
                return
            line = raw.decode("utf-8", "replace").strip()
            up = line.upper()
            if up.startswith(("EHLO", "HELO")):
                say("250 sink")
            elif up.startswith("MAIL FROM"):
                sender = line
                say("250 ok")
            elif up.startswith("RCPT TO"):
                if reject[0]:
                    say(f"{reject[0]} no")
                else:
                    rcpt = line
                    say("250 ok")
            elif up == "DATA":
                say("354 go")
                data = []
                while True:
                    row = f.readline()
                    if not row:
                        return
                    if row == b".\r\n":
                        break
                    data.append(row.decode("utf-8", "replace"))
                with lock:
                    seen.append({"from": sender, "to": rcpt, "data": "".join(data)})
                say("250 queued")
            elif up == "QUIT":
                say("221 bye")
                return
            elif up.startswith(("STARTTLS", "AUTH")):
                say("502 not offered")
            else:
                say("250 ok")
    finally:
        conn.close()


class Http(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, code, body=b""):
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/_seen":
            with lock:
                return self._send(200, json.dumps(seen).encode())
        self._send(404)

    def do_DELETE(self):
        if self.path == "/_seen":
            with lock:
                seen.clear()
            return self._send(204)
        self._send(404)

    def do_POST(self):
        if self.path.startswith("/_reject/"):
            reject[0] = int(self.path.rsplit("/", 1)[1])
            return self._send(204)
        self._send(404)


def main():
    smtp_port, http_port = int(sys.argv[1]), int(sys.argv[2])
    host = sys.argv[3] if len(sys.argv) > 3 else "127.0.0.1"
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((host, smtp_port))
    s.listen(16)
    threading.Thread(target=HTTPServer((host, http_port), Http).serve_forever, daemon=True).start()
    while True:
        conn, _ = s.accept()
        threading.Thread(target=serve_smtp, args=(conn,), daemon=True).start()


if __name__ == "__main__":
    main()
