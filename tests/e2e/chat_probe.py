#!/usr/bin/env python3
"""One chat, and whatever answers it, through a running stack: what the failure drills in scripts/check-isolation.sh ask (plan step 7.3, D32).

    chat_probe.py pair URL PAIRING [--insecure]      redeems a pairing key, prints the client's token
    chat_probe.py chat URL TOKEN [--insecure]        asks one question in the first space, prints how it ended
    chat_probe.py conversations URL TOKEN [--insecure]   prints how many conversations the first space has

`chat` prints one line: `ok` when a reply finished, or `refused <http status or sse> <error code>` when the stack said no, or
`failed <what>` when it did not answer at all. It exits 0 only for `ok`. A refusal comes either as an HTTP error before the stream
starts or as an `error` event in it; both are told apart by the code, never by the sentence. Standard library only.
"""
import base64
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from load_scenario import CLIENT_VERSION, connect, request, sse_events  # noqa: E402


def pair(url, insecure, pairing):
    b = pairing[len("askbar-pair:"):]
    code = json.loads(base64.urlsafe_b64decode(b + "=" * (-len(b) % 4)))["code"]
    status, body = request(url, insecure, "POST", "/api/client/v1/pairings", body={"code": code})
    if status != 200:
        print(f"failed pairing {status}")
        return 1
    print(body["token"])
    return 0


def first_space(url, insecure, token):
    status, models = request(url, insecure, "GET", "/api/client/v1/models", token)
    status2, spaces = request(url, insecure, "GET", "/api/client/v1/spaces", token)
    if status != 200 or status2 != 200 or not models.get("default") or not spaces:
        return None, None
    return models["default"], spaces[0]["id"]


def chat(url, insecure, token):
    model, space = first_space(url, insecure, token)
    if not space:
        print("failed no approved model or no space")
        return 1
    conn, base = connect(url, insecure)
    headers = {"x-askbar-client-version": CLIENT_VERSION, "content-type": "application/json", "authorization": "Bearer " + token}
    conn.request("POST", f"{base}/api/client/v1/spaces/{space}/chat", body=json.dumps({"model": model, "text": "drill"}), headers=headers)
    resp = conn.getresponse()
    if resp.status != 200:
        raw = resp.read()
        try:
            code = json.loads(raw).get("error", "")
        except ValueError:
            code = ""
        print(f"refused {resp.status} {code}".rstrip())
        return 1
    got_text = False
    for name, data, _ in sse_events(resp):
        if name == "delta":
            got_text = True
        elif name == "done":
            print("ok" if got_text else "failed done with no text")
            return 0 if got_text else 1
        elif name == "error":
            code = data.get("code", "") if isinstance(data, dict) else ""
            print(f"refused sse {code}".rstrip())
            return 1
    print("failed the stream ended with no done")
    return 1


def conversations(url, insecure, token):
    model, space = first_space(url, insecure, token)
    if not space:
        print("failed no space")
        return 1
    status, rows = request(url, insecure, "GET", f"/api/client/v1/spaces/{space}/conversations", token)
    if status != 200 or not isinstance(rows, list):
        print(f"failed {status}")
        return 1
    print(len(rows))
    return 0


def main(argv):
    args = [a for a in argv[1:] if a != "--insecure"]
    insecure = "--insecure" in argv
    try:
        if len(args) == 3 and args[0] == "pair":
            return pair(args[1], insecure, args[2])
        if len(args) == 3 and args[0] == "chat":
            return chat(args[1], insecure, args[2])
        if len(args) == 3 and args[0] == "conversations":
            return conversations(args[1], insecure, args[2])
    except (OSError, ValueError) as e:  # a refused connection, a reset, a timeout, an answer that is not JSON
        print(f"failed {type(e).__name__}")
        return 1
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
