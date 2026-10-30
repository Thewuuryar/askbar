#!/usr/bin/env bash
# Smoke test: start the server and the admin on empty data directories, pair a device over the
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
CLIENT_VERSION="0.3.0"
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
  rm -rf "$WORK"
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
  API_PORT=$(free_port); OLD_ADMIN_PORT=$(free_port); MGMT_PORT=$(free_port); MOCK_PORT=$(free_port)
  URL="http://127.0.0.1:$API_PORT"
  SOCK="$WORK/admin-run/admin.sock"
  mkdir -p "$WORK/data" "$WORK/secrets" "$WORK/admin-run"
  python3 -u tests/mock_llm.py "$MOCK_PORT" >/dev/null 2>&1 & PIDS+=($!)
  # The admin service: its own database and secrets, a unix socket and no TCP port.
  ASKBAR_ADMIN_SOCKET="$SOCK" ASKBAR_ADMIN_DATA_DIR="$WORK/admin-data" ASKBAR_ADMIN_SECRETS_DIR="$WORK/admin-secrets" \
    ASKBAR_MGMT_DIR="$WORK/mgmt" ASKBAR_SERVER_MGMT_URL="http://127.0.0.1:$MGMT_PORT" ASKBAR_PUBLIC_URL="$URL" \
    "$ABIN" >"$WORK/admin.log" 2>&1 & PIDS+=($!)
  for _ in $(seq 100); do [ -S "$SOCK" ] && break; sleep 0.1; done
  # The server: the API, and a management listener that only the admin's token opens.
  ASKBAR_API_ADDR="127.0.0.1:$API_PORT" ASKBAR_ADMIN_ADDR="127.0.0.1:$OLD_ADMIN_PORT" \
    ASKBAR_MGMT_ADDR="127.0.0.1:$MGMT_PORT" ASKBAR_MGMT_TOKEN_HASH_FILE="$WORK/mgmt/token.sha256" \
    ASKBAR_DATA_DIR="$WORK/data" ASKBAR_SECRETS_DIR="$WORK/secrets" \
    ASKBAR_PUBLIC_URL="$URL" "$BIN" >"$WORK/server.log" 2>&1 & PIDS+=($!)
fi
URL="${URL%/}"

echo "Target: $URL ($([ $LOCAL = 1 ] && echo 'throwaway server on an empty data directory' || echo 'running stack'))"

# 1. The server answers.
if wait_for_url "$URL/api/v1/health" 20; then pass "server is up"; else
  fail "server is up" "no answer from $URL/api/v1/health"; [ $LOCAL = 1 ] && tail -5 "$WORK/server.log"; exit 1
fi

# 2-3. Local only: first-run setup through the admin, a provider, a pair code.
if [ $LOCAL = 1 ]; then
  PW="smoke test password"
  admin() { # method path [json]
    curl -fsS --unix-socket "$SOCK" -X "$1" -H 'x-askbar-admin: 1' -H 'content-type: application/json' -b "$WORK/jar" -c "$WORK/jar" \
      ${3:+-d "$3"} "http://admin$2"
  }
  if admin POST /admin/api/setup/password "{\"password\":\"$PW\"}" >/dev/null; then
    pass "first-run setup"
  else
    fail "first-run setup"
  fi
  if admin POST /admin/api/providers \
      "{\"name\":\"mock\",\"type\":\"openai\",\"base_url\":\"http://127.0.0.1:$MOCK_PORT/v1\",\"model\":\"mock-model\",\"api_key\":\"sk-smoke\"}" >/dev/null; then
    pass "add a provider that points at the mock LLM"
  else
    fail "add a provider that points at the mock LLM"
  fi
  if admin POST /admin/api/pair '{"name":"smoke test device"}' >"$WORK/pair.json"; then
    PAIRING=$(json "$WORK/pair.json" 'd["pairing"]'); pass "create a pair code"
  else
    fail "create a pair code"
  fi
elif [ -z "$PAIRING" ]; then
  read -r -p "Paste a pairing string from the admin page: " PAIRING
fi

# 4. Pair a test device over the HTTP API.
TOKEN=""
if [ -n "$PAIRING" ]; then
  CODE=$(py 'import base64,json,sys
s=sys.argv[1]
assert s.startswith("askbar-pair:")
b=s[len("askbar-pair:"):]
d=json.loads(base64.urlsafe_b64decode(b+"="*(-len(b)%4)))
print(d["code"])' "$PAIRING" 2>/dev/null)
  if [ -n "$CODE" ] && curl -fsS -X POST -H "x-askbar-client-version: $CLIENT_VERSION" -H 'content-type: application/json' \
      -d "{\"code\":\"$CODE\"}" "$URL/api/v1/pair" >"$WORK/token.json" 2>/dev/null; then
    TOKEN=$(json "$WORK/token.json" 'd["token"]')
  fi
fi
if [ -n "$TOKEN" ]; then pass "pair a test device"; else fail "pair a test device"; fi
api() { curl -sS -H "authorization: Bearer $TOKEN" -H "x-askbar-client-version: $CLIENT_VERSION" "$@"; }

if [ -n "$TOKEN" ]; then
  # 5. The token works; an unrelated one does not.
  [ "$(api -o /dev/null -w '%{http_code}' "$URL/api/v1/whoami")" = "204" ] && pass "the device token is accepted" || fail "the device token is accepted"
  [ "$(curl -sS -o /dev/null -w '%{http_code}' -H 'authorization: Bearer nope' -H "x-askbar-client-version: $CLIENT_VERSION" "$URL/api/v1/whoami")" = "401" ] \
    && pass "a wrong token is refused" || fail "a wrong token is refused"

  # 6. Providers.
  api "$URL/api/v1/providers" >"$WORK/providers.json"
  PROVIDER=$(json "$WORK/providers.json" 'd[0]["id"]')
  if [ -n "$PROVIDER" ]; then pass "the server lists a provider ($(json "$WORK/providers.json" 'd[0]["name"]'))"; else fail "the server lists a provider"; fi

  # 7. Chat: read the streamed reply.
  QUESTION="smoke question zebra"
  api -N --max-time 60 -X POST -H 'content-type: application/json' \
    -d "{\"provider\":\"$PROVIDER\",\"text\":\"$QUESTION\"}" "$URL/api/v1/chat" >"$WORK/chat.sse" 2>/dev/null
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
    api "$URL/api/v1/conversations/$CONV_ID" >"$WORK/conv.json"
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
  code=$(api -o "$WORK/search.json" -w '%{http_code}' --get --data-urlencode "q=zebra" "$URL/api/v1/search")
  if [ "$code" = "200" ] && [ "$(json "$WORK/search.json" 'len(d)')" -ge 1 ] 2>/dev/null; then
    pass "search history finds the question"
  else
    fail "search history finds the question" "HTTP $code"
  fi
fi

# 10. What only a person can check.
ask_person "Does the desktop client, paired to this server, show the streamed reply?"

echo
echo "$PASSED passed, $FAILED failed, $SKIPPED skipped"
[ "$FAILED" -eq 0 ]
