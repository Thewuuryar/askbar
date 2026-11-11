#!/usr/bin/env bash
# Smoke test: start the server and the admin on empty data directories, pair a client over the
# HTTP API, send a chat and read the streamed reply, then search history.
# Prints PASS or FAIL per step. Steps it cannot check itself ask a person for y or n.
#
#   scripts/smoke.sh                      start a throwaway server and a mock LLM
#   scripts/smoke.sh --url https://...    test a running stack instead
#
# With --url the script does not own the stack, so it cannot create a pair code
# or a provider: pass --pairing "askbar-pair:..." (from the admin page) or it
# asks for one. The stack must already have a provider.
#
# Needs bash, curl and python3. Exits 1 if any step FAILs.

set -u
cd "$(dirname "$0")/.."

URL=""
PAIRING="${ASKBAR_SMOKE_PAIRING:-}"
CLIENT_VERSION="0.6.0"
while [ $# -gt 0 ]; do
  case "$1" in
    --url) URL="${2:?--url needs a value}"; shift 2 ;;
    --pairing) PAIRING="${2:?--pairing needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

WORK="$(mktemp -d)"
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  chmod -R u+rwx "$WORK" 2>/dev/null; rm -rf "$WORK"
}
trap cleanup EXIT

PASSED=0; FAILED=0; SKIPPED=0
pass() { echo "PASS  $1"; PASSED=$((PASSED + 1)); }
fail() { echo "FAIL  $1${2:+: $2}"; FAILED=$((FAILED + 1)); }
skip() { echo "SKIP  $1${2:+: $2}"; SKIPPED=$((SKIPPED + 1)); }
# ask_person "question": y -> PASS, n -> FAIL. With no terminal to ask on, the step is skipped, not passed.
ask_person() {
  if [ ! -t 0 ]; then skip "$1" "needs a person (no terminal)"; return; fi
  local a
  read -r -p "ASK   $1 [y/n] " a
  case "$a" in [yY]*) pass "$1" ;; *) fail "$1" "answered no" ;; esac
}
# py CODE [ARGS...]: run python (isolated mode) and print its result.
py() { python3 -I -c "$@"; }
# json FILE EXPR: evaluate EXPR with the parsed document as `d`.
json() { py 'import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2" 2>/dev/null; }
free_port() { py 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
wait_for_url() { # url seconds
  local i
  for i in $(seq 1 $(($2 * 10))); do curl -fsS -o /dev/null "$1" 2>/dev/null && return 0; sleep 0.1; done
  return 1
}

LOCAL=0
if [ -z "$URL" ]; then
  LOCAL=1
  BIN="${ASKBAR_SERVER_BIN:-target/debug/askbar-server}"
  if [ ! -x "$BIN" ]; then cargo build -q -p askbar-server || { fail "build the server"; exit 1; }; fi
  ABIN="${ASKBAR_ADMIN_BIN:-target/debug/askbar-admin}"
  if [ ! -x "$ABIN" ]; then cargo build -q -p askbar-admin || { fail "build the admin"; exit 1; }; fi
  BBIN="${ASKBAR_BOOTSTRAP_BIN:-target/debug/askbar-bootstrap}"
  if [ ! -x "$BBIN" ]; then cargo build -q -p askbar-bootstrap || { fail "build bootstrap"; exit 1; }; fi
  AUBIN="${ASKBAR_AUDITOR_BIN:-target/debug/askbar-auditor}"
  if [ ! -x "$AUBIN" ]; then cargo build -q -p askbar-auditor || { fail "build the auditor"; exit 1; }; fi
  GBIN="${ASKBAR_GATEWAY_BIN:-target/debug/askbar-model-gateway}"
  if [ ! -x "$GBIN" ]; then cargo build -q -p askbar-model-gateway || { fail "build the gateway"; exit 1; }; fi
  API_PORT=$(free_port); SERVER_API_PORT=$(free_port); MOCK_PORT=$(free_port)
  AUDIT_PORT=$(free_port); GW_SERVER_PORT=$(free_port); GW_ADMIN_PORT=$(free_port)
  URL="http://127.0.0.1:$API_PORT"
  SOCK="$WORK/admin-run/admin.sock"
  mkdir -p "$WORK/data" "$WORK/admin-run" "$WORK/audit"
  # The tokens come from bootstrap, as in the stack (ADR 0049); here its volumes are plain directories.
  ASKBAR_BOOTSTRAP_ROOT="$WORK/credentials" "$BBIN" >/dev/null || { fail "bootstrap makes the tokens"; exit 1; }
  python3 -u tests/e2e/mock_llm.py "$MOCK_PORT" >/dev/null 2>&1 & PIDS+=($!)
  # The auditor and the gateway (ADR 0051, ADR 0054): every reply is recorded before a provider is called.
  ASKBAR_AUDIT_API_ADDR="127.0.0.1:$AUDIT_PORT" ASKBAR_AUDIT_DIR="$WORK/audit" ASKBAR_AUDITOR_TOKEN_DIR="$WORK/credentials/auditor-token" \
    "$AUBIN" >"$WORK/auditor.log" 2>&1 & PIDS+=($!)
  ASKBAR_GATEWAY_SERVER_ADDR="127.0.0.1:$GW_SERVER_PORT" ASKBAR_GATEWAY_ADMIN_ADDR="127.0.0.1:$GW_ADMIN_PORT" \
    ASKBAR_GATEWAY_TOKEN_DIR="$WORK/credentials/model-gateway-token" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" \
    "$GBIN" >"$WORK/gateway.log" 2>&1 & PIDS+=($!)
  # The admin service: its own database and secrets, a unix socket and no TCP port.
  ASKBAR_ADMIN_SOCKET="$SOCK" ASKBAR_ADMIN_DATA_DIR="$WORK/admin-data" ASKBAR_ADMIN_SECRETS_DIR="$WORK/admin-secrets" \
    ASKBAR_ADMIN_TOKEN_DIR="$WORK/credentials/admin-token" ASKBAR_SERVER_API_URL="http://127.0.0.1:$SERVER_API_PORT" ASKBAR_PUBLIC_URL="$URL" \
    ASKBAR_GATEWAY_URL="http://127.0.0.1:$GW_ADMIN_PORT" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" "$ABIN" >"$WORK/admin.log" 2>&1 & PIDS+=($!)
  for _ in $(seq 100); do [ -S "$SOCK" ] && break; sleep 0.1; done
  # The server: the API, and a server listener that only the admin's token opens.
  ASKBAR_CLIENT_API_ADDR="127.0.0.1:$API_PORT" \
    ASKBAR_SERVER_API_ADDR="127.0.0.1:$SERVER_API_PORT" ASKBAR_SERVER_TOKEN_HASH_FILE="$WORK/credentials/server-token/accept/admin-server.sha256" \
    ASKBAR_DATA_DIR="$WORK/data" ASKBAR_SERVER_TOKEN_DIR="$WORK/credentials/server-token" \
    ASKBAR_GATEWAY_URL="http://127.0.0.1:$GW_SERVER_PORT" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" \
    ASKBAR_PUBLIC_URL="$URL" "$BIN" >"$WORK/server.log" 2>&1 & PIDS+=($!)
fi
URL="${URL%/}"

echo "Target: $URL ($([ $LOCAL = 1 ] && echo 'throwaway server on an empty data directory' || echo 'running stack'))"

# 1. The server answers.
if wait_for_url "$URL/api/client/v1/health" 20; then pass "server is up"; else
  fail "server is up" "no answer from $URL/api/client/v1/health"; [ $LOCAL = 1 ] && tail -5 "$WORK/server.log"; exit 1
fi

# 2-3. Local only: first-run setup through the admin, a provider, a pair code.
if [ $LOCAL = 1 ]; then
  PW="smoke test password"
  # localhost, not a made-up host name: curl keeps the admin's `__Secure-` cookie (Secure) only from a
  # secure origin, and it counts http://localhost as one, as browsers do.
  admin() { # method path [json]
    curl -fsS --unix-socket "$SOCK" -X "$1" -H 'x-askbar-admin: 1' -H 'content-type: application/json' -b "$WORK/jar" -c "$WORK/jar" \
      ${3:+-d "$3"} "http://localhost$2"
  }
  # First run (ADR 0007): sign in with the published login, change it, then everything else opens.
  if admin POST /api/admin/v1/session '{"username":"admin","password":"admin"}' >/dev/null \
     && admin PUT /api/admin/v1/password "{\"current\":\"admin\",\"new\":\"$PW\"}" >/dev/null; then
    pass "first-run setup"
  else
    fail "first-run setup"
  fi
  # The audit passphrase is part of setup; without its key the server refuses to start replies.
  if admin PUT /api/admin/v1/setup/audit-key '{"passphrase":"smoke test audit passphrase","confirm":"smoke test audit passphrase"}' >/dev/null; then
    pass "audit key setup"
  else
    fail "audit key setup"
  fi
  if admin POST /api/admin/v1/providers \
      "{\"name\":\"mock\",\"type\":\"openai\",\"base_url\":\"http://127.0.0.1:$MOCK_PORT/v1\",\"model\":\"mock-model\",\"api_key\":\"sk-smoke\"}" >/dev/null; then
    pass "add a provider that points at the mock LLM"
  else
    fail "add a provider that points at the mock LLM"
  fi
  # A pairing key belongs to a user, made in the admin (ADR 0008).
  if admin POST /api/admin/v1/users '{"name":"smoke test user"}' >"$WORK/user.json" \
     && USER_ID=$(json "$WORK/user.json" 'd["id"]') \
     && admin POST "/api/admin/v1/users/$USER_ID/pairing-keys" '{"name":"smoke test client"}' >"$WORK/pair.json"; then
    PAIRING=$(json "$WORK/pair.json" 'd["pairing"]'); pass "create a user and a pairing key"
  else
    fail "create a user and a pairing key"
  fi
elif [ -z "$PAIRING" ]; then
  read -r -p "Paste a pairing key from the admin page: " PAIRING
fi

# 4. Pair a test client over the HTTP API.
TOKEN=""
if [ -n "$PAIRING" ]; then
  CODE=$(py 'import base64,json,sys
s=sys.argv[1]
assert s.startswith("askbar-pair:")
b=s[len("askbar-pair:"):]
d=json.loads(base64.urlsafe_b64decode(b+"="*(-len(b)%4)))
print(d["code"])' "$PAIRING" 2>/dev/null)
  if [ -n "$CODE" ] && curl -fsS -X POST -H "x-askbar-client-version: $CLIENT_VERSION" -H 'content-type: application/json' \
      -d "{\"code\":\"$CODE\"}" "$URL/api/client/v1/pairings" >"$WORK/token.json" 2>/dev/null; then
    TOKEN=$(json "$WORK/token.json" 'd["token"]')
  fi
fi
if [ -n "$TOKEN" ]; then pass "pair a test client"; else fail "pair a test client"; fi
api() { curl -sS -H "authorization: Bearer $TOKEN" -H "x-askbar-client-version: $CLIENT_VERSION" "$@"; }

if [ -n "$TOKEN" ]; then
  # 5. The token works; an unrelated one does not.
  [ "$(api -o /dev/null -w '%{http_code}' "$URL/api/client/v1/me")" = "204" ] && pass "the client token is accepted" || fail "the client token is accepted"
  [ "$(curl -sS -o /dev/null -w '%{http_code}' -H 'authorization: Bearer nope' -H "x-askbar-client-version: $CLIENT_VERSION" "$URL/api/client/v1/me")" = "401" ] \
    && pass "a wrong token is refused" || fail "a wrong token is refused"

  # 5b. Pair another app of the same user from this one (ADR 0008): its own code, used once.
  if api -X POST -H 'content-type: application/json' -d '{"name":"smoke second app"}' "$URL/api/client/v1/me/pairing-keys" >"$WORK/key2.json" 2>/dev/null \
     && CODE2=$(json "$WORK/key2.json" 'd["code"]') && [ -n "$CODE2" ] \
     && curl -fsS -X POST -H "x-askbar-client-version: $CLIENT_VERSION" -H 'content-type: application/json' -d "{\"code\":\"$CODE2\"}" "$URL/api/client/v1/pairings" >"$WORK/token2.json" 2>/dev/null \
     && TOKEN2=$(json "$WORK/token2.json" 'd["token"]') \
     && [ "$(curl -sS -o /dev/null -w '%{http_code}' -H "authorization: Bearer $TOKEN2" -H "x-askbar-client-version: $CLIENT_VERSION" "$URL/api/client/v1/me")" = "204" ]; then
    pass "a paired app pairs another app of its user"
  else
    fail "a paired app pairs another app of its user"
  fi

  # 6. Providers.
  api "$URL/api/client/v1/providers" >"$WORK/providers.json"
  PROVIDER=$(json "$WORK/providers.json" 'd[0]["id"]')
  if [ -n "$PROVIDER" ]; then pass "the server lists a provider ($(json "$WORK/providers.json" 'd[0]["name"]'))"; else fail "the server lists a provider"; fi

  # 6b. Spaces (ADR 0009): the user has a personal space, listed first; the routes below are under it.
  api "$URL/api/client/v1/spaces" >"$WORK/spaces.json"
  SPACE=$(json "$WORK/spaces.json" 'next(s["id"] for s in d if s["kind"]=="personal")')
  if [ -n "$SPACE" ]; then pass "the user has a personal space"; else fail "the user has a personal space"; fi
  SP="$URL/api/client/v1/spaces/$SPACE"
  if [ $LOCAL = 1 ]; then
    if [ -f "$WORK/data/control.db" ] && [ -f "$WORK/data/spaces/$SPACE.db" ] && [ ! -e "$WORK/data/askbar.db" ]; then
      pass "the server keeps control.db and one file per space"
    else
      fail "the server keeps control.db and one file per space" "$(ls -R "$WORK/data" 2>/dev/null | tr '\n' ' ')"
    fi
  fi

  # 7. Chat: read the streamed reply.
  QUESTION="smoke question zebra"
  api -N --max-time 60 -X POST -H 'content-type: application/json' \
    -d "{\"provider\":\"$PROVIDER\",\"text\":\"$QUESTION\"}" "$SP/chat" >"$WORK/chat.sse" 2>/dev/null
  CHAT=$(py 'import json,sys
events=[]; name=None
for line in open(sys.argv[1], encoding="utf-8"):
    line=line.rstrip("\n")
    if line.startswith("event:"): name=line[6:].strip()
    elif line.startswith("data:") and name: events.append((name, json.loads(line[5:].strip()))); name=None
names=[n for n,_ in events]
text="".join(d.get("text","") for n,d in events if n=="delta")
conv=next((d["conversation_id"] for n,d in events if n=="meta"), "")
ok = names[:1]==["meta"] and names.count("delta")>0 and names[-1:]==["done"] and len(text)>0
print("ok" if ok else "bad", conv, len(text))' "$WORK/chat.sse" 2>/dev/null)
  read -r CHAT_OK CONV_ID CHAT_LEN <<<"$CHAT"
  if [ "$CHAT_OK" = "ok" ]; then pass "chat streams meta, deltas and done ($CHAT_LEN characters)"; else fail "chat streams meta, deltas and done"; fi

  # 8. History holds the exchange.
  if [ -n "${CONV_ID:-}" ]; then
    api "$SP/conversations/$CONV_ID" >"$WORK/conv.json"
    if [ "$(json "$WORK/conv.json" 'len(d["messages"])')" = "2" ] \
       && [ "$(json "$WORK/conv.json" 'd["messages"][0]["content"]')" = "$QUESTION" ]; then
      pass "the conversation holds the question and the reply"
    else
      fail "the conversation holds the question and the reply"
    fi
  else
    fail "the conversation holds the question and the reply" "no conversation id"
  fi

  # 9. Search history.
  code=$(api -o "$WORK/search.json" -w '%{http_code}' --get --data-urlencode "q=zebra" "$SP/search")
  if [ "$code" = "200" ] && [ "$(json "$WORK/search.json" 'len(d)')" -ge 1 ] 2>/dev/null; then
    pass "search history finds the question"
  else
    fail "search history finds the question" "HTTP $code"
  fi

  # 10. Retries: the mock refuses this message with a 429 (Retry-After: 1) the first time; the
  # server waits and tries again, and the reply still completes.
  api -N --max-time 60 -X POST -H 'content-type: application/json' \
    -d "{\"provider\":\"$PROVIDER\",\"text\":\"smoke 429once retry\"}" "$SP/chat" >"$WORK/retry.sse" 2>/dev/null
  RETRY=$(py 'import sys
text=open(sys.argv[1], encoding="utf-8").read()
print("ok" if "event: delta" in text and text.rstrip().endswith("data: {\"event\":\"done\"}") and "event: error" not in text else "bad")' "$WORK/retry.sse" 2>/dev/null)
  if [ "$RETRY" = "ok" ]; then pass "a provider that answers 429 once still completes the reply"; else fail "a provider that answers 429 once still completes the reply" "$(tail -c 200 "$WORK/retry.sse")"; fi
fi

# 10b. The shapes the APIs promise: the old prefix says "update", a refusal is a problem, and (local) an admin API key works.
OLD=$(curl -sS -o "$WORK/old.json" -w '%{http_code}' "$URL/api/v1/health" 2>/dev/null || true)
if [ "$OLD" = "426" ] && [ "$(json "$WORK/old.json" "d['error']")" = "client_too_old" ]; then pass "the old /api/v1 prefix answers 426 client_too_old"; else fail "the old /api/v1 prefix answers 426" "got $OLD"; fi
NOPE=$(curl -sS -o "$WORK/nope.json" -w '%{http_code}' -H "x-askbar-client-version: $CLIENT_VERSION" "$URL/api/server/v1/health" 2>/dev/null || true)
if [ "$NOPE" = "404" ] && [ "$(json "$WORK/nope.json" "d['error']")" = "not_found" ]; then pass "the server API is not served on the client listener (404 problem)"; else fail "the server API is not served on the client listener" "got $NOPE"; fi
if [ $LOCAL = 1 ]; then
  if admin POST /api/admin/v1/keys '{"name":"smoke test"}' >"$WORK/key.json" 2>/dev/null; then
    KEY=$(json "$WORK/key.json" "d['key']")
    CODE_KEY=$(curl -sS -o /dev/null -w '%{http_code}' --unix-socket "$SOCK" -H "authorization: Bearer $KEY" "http://admin/api/admin/v1/providers" 2>/dev/null || true)
    CODE_SESSION=$(curl -sS -o /dev/null -w '%{http_code}' --unix-socket "$SOCK" -H "authorization: Bearer $KEY" "http://admin/api/admin/v1/keys" 2>/dev/null || true)
    if [ "$CODE_KEY" = "200" ] && [ "$CODE_SESSION" = "401" ]; then pass "an admin API key reads the API but cannot manage keys"; else fail "admin API key" "api=$CODE_KEY keys=$CODE_SESSION"; fi
  else
    fail "make an admin API key"
  fi
fi

# 11. What only a person can check.
ask_person "Does the desktop client, paired to this server, show the streamed reply?"

echo
echo "$PASSED passed, $FAILED failed, $SKIPPED skipped"
[ "$FAILED" -eq 0 ]
