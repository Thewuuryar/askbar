"""Tiny OpenAI-compatible streaming server for trying Askbar without an API key.

    python3 tests/mock_llm.py 8765

Then point a provider at http://127.0.0.1:8765/v1 (any key). Say "slow" in a
message to get a long, slow reply (handy for testing Stop), or "fail" to get
an HTTP 401 error.
"""
import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPLY = """Here is a quick answer with **bold**, `inline code` and a list:

- First point
- Second point with a [link](https://example.com)

```python
def greet(name):
    return f"Hello, {name}!"
```

| Option | Meaning |
|--------|---------|
| `-v`   | verbose |

Anything else?"""

SLOW = "Streaming slowly so you can press Stop. " * 40


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        last = ""
        for m in body.get("messages", []):
            if m.get("role") == "user":
                last = m.get("content", "")
        system = next((m.get("content", "") for m in body.get("messages", []) if m.get("role") == "system"), "")
        print(f"POST {self.path} model={body.get('model')} msgs={len(body.get('messages', []))} "
              f"auth={'yes' if self.headers.get('authorization') else 'no'} last={last[:40]!r} system={system[:60]!r}", flush=True)

        if "fail" in last.lower():
            payload = json.dumps({"error": {"message": "Incorrect API key provided (mock)"}}).encode()
            self.send_response(401)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        slow = "slow" in last.lower()
        text = SLOW if slow else REPLY
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        try:
            step = 6 if not slow else 4
            for i in range(0, len(text), step):
                chunk = {"choices": [{"delta": {"content": text[i:i + step]}}]}
                self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                self.wfile.flush()
                time.sleep(0.15 if slow else 0.012)
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            print("client disconnected", flush=True)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    print(f"mock LLM on http://127.0.0.1:{port}/v1", flush=True)
    ThreadingHTTPServer((os.environ.get("MOCK_LLM_HOST", "127.0.0.1"), port), Handler).serve_forever()
