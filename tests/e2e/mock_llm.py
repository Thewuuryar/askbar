"""Tiny OpenAI-compatible streaming server for trying Askbar without an API key.

    python3 tests/e2e/mock_llm.py 8765

Then point a provider at http://127.0.0.1:8765/v1 (any key). Say "slow" in a
message to get a long, slow reply (handy for testing Stop; about a minute), "brisk" for the
same text in about 4 seconds (for tests that need the whole long reply), or "fail" to get
an HTTP 401 error. "429" answers HTTP 429 with Retry-After: 1 and "500" answers
HTTP 500. The same words in the model name ("m-429", "m-fail", "m-reset") work too. "429once" answers 429 (Retry-After: 1) the first time it sees that exact
message and succeeds after, and "reset" sends two deltas (or the MOCK-REPLY text, 6 characters at a time) then resets the connection (a failure
that comes after text has been delivered). With stream_options.include_usage a final usage chunk is sent
(cached_tokens is non-zero when the message contains "cached"). POST /v1/messages answers in Anthropic's streaming format with the same modes.
"hang" accepts the request and sends nothing until the client closes;
"drop" sends two content deltas and closes the connection without [DONE].
Quick search: the server's query-proposal call gets a JSON array of two queries, and the answer call (the prompt holds
<untrusted_web_content>) gets a reply that cites [1] and [2] and says how many numbered sources it was given.
Research, by the system prompt: the split gets a JSON array of two sub-questions, the researcher's summary a short text with
the marker SUMMARY_MARK, the writer a draft that cites [1] and [2] with DRAFT_MARK, and the judge approves unless the question holds
"MOCK-JUDGE refuse <reason>" (a refusal with that code) or "MOCK-JUDGE malformed" (the verdict with extra text). A line
"MOCK-TOKENS <n>" makes the call report n completion tokens (the research cap tests).
A last message with "MOCK-TRUNCATE" makes the call behave like Ollama over its context: only the newest message is kept
and the reported prompt_tokens is for it alone (the smaller count). A message with a line "MOCK-REPLY <text>" gets exactly <text> as the reply (the judge tests choose its answer this way).
"""
import json
import os
import re
import socket
import socketserver
import struct
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

QUERIES = '["smoke-w55-query-alpha", "smoke-w55-query-beta"]'

SUBS = '["smoke-p5b-sub-one", "smoke-p5b-sub-two"]'
SUMMARY_MARK = "smoke-p5b-summary-marker"
DRAFT_MARK = "smoke-p5b-draft-marker"

SLOW = "Streaming slowly so you can press Stop. " * 40


def text_of(content):
    """Message content is a string (OpenAI) or a list of blocks (Anthropic)."""
    if isinstance(content, list):
        return "".join(b.get("text", "") for b in content if isinstance(b, dict))
    return content or ""


SEEN_ONCE = set()  # messages "429once" has already refused
RST = set()  # connections to abort with a reset instead of closing politely


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def send_json(self, status, obj, headers=None):
        payload = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(payload)

    def sse(self, event, obj):
        self.wfile.write(f"event: {event}\ndata: {json.dumps(obj)}\n\n".encode())
        self.wfile.flush()

    def anthropic_start(self, body, cached=False):
        self.sse("message_start", {"type": "message_start", "message": {
            "id": "msg_mock", "type": "message", "role": "assistant", "model": body.get("model"),
            "content": [], "usage": {"input_tokens": 100, "output_tokens": 1,
                                     "cache_read_input_tokens": 64 if cached else 0,
                                     "cache_creation_input_tokens": 0}}})
        self.sse("content_block_start", {"type": "content_block_start", "index": 0,
                                         "content_block": {"type": "text", "text": ""}})

    def delta(self, anthropic, piece):
        if anthropic:
            self.sse("content_block_delta", {"type": "content_block_delta", "index": 0,
                                             "delta": {"type": "text_delta", "text": piece}})
        else:
            self.wfile.write(f"data: {json.dumps({'choices': [{'delta': {'content': piece}}]})}\n\n".encode())
            self.wfile.flush()

    def do_GET(self):
        # The model list, for the admin's model dropdown (OpenAI shape; any key).
        if self.path.split("?")[0].rstrip("/").endswith("/models"):
            return self.send_json(200, {"object": "list", "data": [{"id": "mock-model"}, {"id": "mock-model-2"}]})
        self.send_json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        anthropic = self.path.split("?")[0].rstrip("/").endswith("/messages")
        last = ""
        for m in body.get("messages", []):
            if m.get("role") == "user":
                last = text_of(m.get("content"))
        system = text_of(body.get("system")) if anthropic else next(
            (text_of(m.get("content")) for m in body.get("messages", []) if m.get("role") == "system"), "")
        print(f"POST {self.path} model={body.get('model')} msgs={len(body.get('messages', []))} "
              f"auth={'yes' if self.headers.get('authorization') else 'no'} last={last[:40]!r} system={system[:60]!r} "
              f"stream_options={json.dumps(body.get('stream_options'))}", flush=True)

        # The failure modes can also be chosen by the model name ("m-429", "m-fail", "m-reset"),
        # so two providers pointed at the same mock can behave differently for the same message.
        probe = last + " " + str(body.get("model") or "")
        if "fail" in probe.lower():
            payload = json.dumps({"error": {"message": "Incorrect API key provided (mock)"}}).encode()
            self.send_response(401)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        if "429" in probe and "429once" not in probe:
            self.send_json(429, {"error": {"message": "Rate limited (mock)"}}, {"retry-after": "1"})
            return
        if "429once" in probe and probe not in SEEN_ONCE:
            SEEN_ONCE.add(probe)
            self.send_json(429, {"error": {"message": "Rate limited once (mock)"}}, {"retry-after": "1"})
            return
        if "reset" in probe.lower():
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            try:
                if anthropic:
                    self.anthropic_start(body)
                chosen = re.search(r"^MOCK-REPLY (.*)$", last, re.M)
                pieces = [chosen.group(1)[i:i + 6] for i in range(0, len(chosen.group(1)), 6)] if chosen else ("Reset ", "midway")
                for piece in pieces:
                    self.delta(anthropic, piece)
                time.sleep(0.05)
                # Abort the connection (RST) instead of ending the stream: with SO_LINGER 0 a plain
                # close() resets, but the server's shutdown_request would send a polite FIN first.
                self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                RST.add(self.connection)
            except OSError:
                pass
            self.close_connection = True
            return
        if "500" in probe:
            self.send_json(500, {"error": {"message": "Internal error (mock)"}})
            return

        if "hang" in probe.lower():
            # Send nothing; wait for the client to close its side.
            self.connection.settimeout(None)
            try:
                while self.connection.recv(4096):
                    pass
            except OSError:
                pass
            print("client disconnected", flush=True)
            return
        if "drop" in probe.lower():
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            try:
                if anthropic:
                    self.anthropic_start(body)
                for piece in ("Dropped ", "mid-stream"):
                    self.delta(anthropic, piece)
            except ConnectionError:
                pass
            self.close_connection = True
            return

        brisk = "brisk" in last.lower()
        slow = brisk or "slow" in last.lower()
        text = SLOW if slow else REPLY
        if (chosen := re.search(r"^MOCK-REPLY (.*)$", last, re.M)):
            text = chosen.group(1)
        elif "You propose web search queries" in system:
            text = QUERIES
        elif "You split one question" in system:
            text = SUBS
        elif "You summarise web pages" in system:
            text = f"{SUMMARY_MARK}: the page says so."
        elif "You write cited research reports" in system:
            text = f"{DRAFT_MARK}: the sources agree [1] and [2]."
        elif "You review a draft report" in system:
            j = re.search(r"MOCK-JUDGE (approve|malformed|refuse (\w+))", last)
            text = ('{"verdict":"approve"}' if not j or j.group(1) == "approve" else
                    '{"verdict":"approve"} and a word' if j.group(1) == "malformed" else
                    json.dumps({"verdict": "refuse", "reason": j.group(2)}))
        elif "<untrusted_web_content>" in last:
            text = f"From the sources [1] and [2]: sources seen {len(re.findall(r'^\[\d+\] ', last, re.M))}."
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        try:
            if anthropic:
                self.anthropic_start(body, cached="cached" in last.lower())
            step = 6 if not slow else 4
            for i in range(0, len(text), step):
                self.delta(anthropic, text[i:i + step])
                time.sleep(0.01 if brisk else 0.15 if slow else 0.012)
            if anthropic:
                self.sse("content_block_stop", {"type": "content_block_stop", "index": 0})
                self.sse("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn"},
                                           "usage": {"output_tokens": 50}})
                self.sse("message_stop", {"type": "message_stop"})
            else:
                # The last text chunk carries the stop reason, as OpenAI-style streams do.
                self.wfile.write(f"data: {json.dumps({'choices': [{'delta': {'content': ''}, 'finish_reason': 'stop'}]})}\n\n".encode())
                if (body.get("stream_options") or {}).get("include_usage"):
                    big = re.search(r"^MOCK-TOKENS (\d+)$", last, re.M)
                    out = int(big.group(1)) if big else 50
                    kept = -(-len(last) // 4) if "MOCK-TRUNCATE" in last else 100
                    usage = {"prompt_tokens": kept, "completion_tokens": out, "total_tokens": kept + out,
                             "prompt_tokens_details": {"cached_tokens": 64 if "cached" in last.lower() else 0}}
                    self.wfile.write(f"data: {json.dumps({'choices': [], 'usage': usage})}\n\n".encode())
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
        except ConnectionError:  # broken pipe, reset, or (on Windows) aborted
            print("client disconnected", flush=True)


class Server(ThreadingHTTPServer):
    def shutdown_request(self, request):
        if request in RST:
            RST.discard(request)
            request.close()  # no FIN first: the peer sees a reset
        else:
            super().shutdown_request(request)

    def server_bind(self):
        # HTTPServer.server_bind also looks up the host's FQDN, which can take
        # many seconds on some machines, and nothing here needs it.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    # Bind before announcing: callers wait for this line and then connect straight away.
    server = Server((os.environ.get("MOCK_LLM_HOST", "127.0.0.1"), port), Handler)
    print(f"mock LLM on http://127.0.0.1:{port}/v1", flush=True)
    server.serve_forever()
