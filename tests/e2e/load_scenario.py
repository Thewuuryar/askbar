#!/usr/bin/env python3
"""Three simultaneous replies, and how long each takes to start (ADR 0002: added latency before the first token; plan step 7.2).

    python3 tests/e2e/load_scenario.py URL --token TOKEN        [--rounds N] [--insecure]
    python3 tests/e2e/load_scenario.py URL --pairing askbar-pair:… [--rounds N] [--insecure]
    python3 tests/e2e/load_scenario.py mock URL                  [--rounds N]

The first two chat through a running stack's client API (URL is the stack's address, such as https://localhost for Caddy), 3 at a
time, for N rounds, each in a new conversation, and read every reply to its end. The time measured is from sending the message
to the first piece of text. The third asks the mock model (URL ends in /v1) the same way, 3 at a time, so its own time can be
subtracted. Prints one line of figures and exits 0; it exits 1 if a reply did not finish. It is a measurement, not a gate: the gate
is `cargo test -p askbar-server --test load`. Standard library only.
"""
import base64
import http.client
import json
import ssl
import statistics
import sys
import threading
import time
import urllib.parse

CONCURRENT = 3
CLIENT_VERSION = "0.6.0"


def connect(url, insecure):
    u = urllib.parse.urlparse(url)
    if u.scheme == "https":
        ctx = ssl.create_default_context()
        if insecure:
            ctx.check_hostname = False
            ctx.verify_mode = ssl.CERT_NONE
        return http.client.HTTPSConnection(u.hostname, u.port or 443, timeout=60, context=ctx), u.path.rstrip("/")
    return http.client.HTTPConnection(u.hostname, u.port or 80, timeout=60), u.path.rstrip("/")


def request(url, insecure, method, path, token=None, body=None):
    conn, base = connect(url, insecure)
    headers = {"x-askbar-client-version": CLIENT_VERSION, "content-type": "application/json"}
    if token:
        headers["authorization"] = "Bearer " + token
    conn.request(method, base + path, body=json.dumps(body) if body is not None else None, headers=headers)
    resp = conn.getresponse()
    data = resp.read()
    conn.close()
    return resp.status, (json.loads(data) if data and resp.getheader("content-type", "").startswith(("application/json", "application/problem")) else None)


def sse_events(resp):
    """(event name, parsed data, seconds when it arrived) for each server-sent event."""
    name = None
    while True:
        line = resp.readline()
        if not line:
            return
        line = line.decode("utf-8", "replace").rstrip("\r\n")
        if line.startswith("event:"):
            name = line[6:].strip()
        elif line.startswith("data:"):
            raw = line[5:].strip()
            yield name, (json.loads(raw) if raw.startswith("{") else raw), time.perf_counter()
            name = None


def chat_once(url, insecure, token, space, model, text):
    """Seconds to the first text of one reply, or None if it did not finish."""
    conn, base = connect(url, insecure)
    headers = {"x-askbar-client-version": CLIENT_VERSION, "content-type": "application/json", "authorization": "Bearer " + token}
    t0 = time.perf_counter()
    conn.request("POST", f"{base}/api/client/v1/spaces/{space}/chat", body=json.dumps({"model": model, "text": text}), headers=headers)
    resp = conn.getresponse()
    first, done = None, False
    if resp.status == 200:
        for name, _, at in sse_events(resp):
            if name == "delta" and first is None:
                first = at - t0
            if name == "done":
                done = True
    conn.close()
    return first if done else None


def mock_once(url, insecure):
    """Seconds to the first piece of text from the mock model itself."""
    conn, base = connect(url, insecure)
    body = {"model": "m", "stream": True, "messages": [{"role": "user", "content": "hello"}]}
    t0 = time.perf_counter()
    conn.request("POST", base + "/chat/completions", body=json.dumps(body), headers={"content-type": "application/json", "authorization": "Bearer x"})
    resp = conn.getresponse()
    first, done = None, False
    while resp.status == 200:
        line = resp.readline()
        if not line:
            break
        line = line.decode("utf-8", "replace").strip()
        if line == "data: [DONE]":
            done = True
            break
        if line.startswith("data:") and first is None and '"content"' in line:
            first = time.perf_counter() - t0
    conn.close()
    return first if done else None


def rounds(n, one):
    """`n` rounds of CONCURRENT replies at once; every time to first text, in seconds. Exits 1 if a reply did not finish."""
    times, failed = [], 0
    for r in range(n):
        out = [None] * CONCURRENT
        def run(i):
            try:
                out[i] = one(r, i)
            except Exception as e:  # a reset or a refusal is a reply that did not finish
                print("reply failed:", type(e).__name__, file=sys.stderr)
        threads = [threading.Thread(target=run, args=(i,)) for i in range(CONCURRENT)]
        [t.start() for t in threads]
        [t.join() for t in threads]
        times += [x for x in out if x is not None]
        failed += sum(x is None for x in out)
    return times, failed


def figures(times):
    ms = sorted(t * 1000 for t in times)
    p95 = ms[min(len(ms) - 1, int(0.95 * len(ms) + 0.999999) - 1)]
    return f"{len(ms)} replies, 3 at a time: p50 {statistics.median(ms):.1f} ms, p95 {p95:.1f} ms, max {ms[-1]:.1f} ms"


def main(argv):
    args = argv[1:]
    insecure = "--insecure" in args
    n = int(args[args.index("--rounds") + 1]) if "--rounds" in args else 10
    if args and args[0] == "mock":
        times, failed = rounds(n, lambda r, i: mock_once(args[1], insecure))
        print("the mock model alone, time to first text:", figures(times) if times else "no reply finished")
        return 1 if failed or not times else 0
    url = args[0]
    token = args[args.index("--token") + 1] if "--token" in args else None
    if not token:
        pairing = args[args.index("--pairing") + 1]
        b = pairing[len("askbar-pair:"):]
        code = json.loads(base64.urlsafe_b64decode(b + "=" * (-len(b) % 4)))["code"]
        status, body = request(url, insecure, "POST", "/api/client/v1/pairings", body={"code": code})
        if status != 200:
            print("could not pair:", status, file=sys.stderr)
            return 1
        token = body["token"]
    status, models = request(url, insecure, "GET", "/api/client/v1/models", token)
    status2, spaces = request(url, insecure, "GET", "/api/client/v1/spaces", token)
    if status != 200 or status2 != 200 or not models.get("default") or not spaces:
        print("no approved model or no space to chat in", file=sys.stderr)
        return 1
    model, space = models["default"], spaces[0]["id"]
    times, failed = rounds(n, lambda r, i: chat_once(url, insecure, token, space, model, f"load {r}.{i}"))
    print(f"through {url}, time to first text (the mock model's own time is in it):", figures(times) if times else "no reply finished")
    return 1 if failed or not times else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
