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
  ALBIN="${ASKBAR_ALERTER_BIN:-target/debug/askbar-alerter}"
  if [ ! -x "$ALBIN" ]; then cargo build -q -p askbar-alerter || { fail "build the alerter"; exit 1; }; fi
  EBIN="${ASKBAR_EGRESS_PROXY_BIN:-target/debug/askbar-egress-proxy}"
  if [ ! -x "$EBIN" ]; then cargo build -q -p askbar-egress-proxy || { fail "build the egress proxy"; exit 1; }; fi
  MBIN="${ASKBAR_MCP_GATEWAY_BIN:-target/debug/askbar-mcp-gateway}"
  if [ ! -x "$MBIN" ]; then cargo build -q -p askbar-mcp-gateway || { fail "build the MCP gateway's front"; exit 1; }; fi
  SBIN="${ASKBAR_SIGN_APPROVAL_BIN:-target/debug/examples/sign_approval}"
  if [ ! -x "$SBIN" ]; then cargo build -q -p askbar-service --features approval --example sign_approval || { fail "build the approval signer"; exit 1; }; fi
  MCP_SERVER_PORT=$(free_port); MCP_ADMIN_PORT=$(free_port); SEARX_PORT=$(free_port)
  API_PORT=$(free_port); SERVER_API_PORT=$(free_port); MOCK_PORT=$(free_port)
  AUDIT_PORT=$(free_port); GW_SERVER_PORT=$(free_port); GW_ADMIN_PORT=$(free_port)
  AL_AUDITOR_PORT=$(free_port); AL_ADMIN_PORT=$(free_port); NTFY_PORT=$(free_port)
  EG_ADMIN_PORT=$(free_port); EG_GW_PORT=$(free_port); EG_CPA_PORT=$(free_port); EG_AL_PORT=$(free_port)
  URL="http://127.0.0.1:$API_PORT"
  SOCK="$WORK/admin-run/admin.sock"
  mkdir -p "$WORK/data" "$WORK/admin-run" "$WORK/audit" "$WORK/alerter-data"
  # The tokens come from bootstrap, as in the stack (ADR 0049); here its volumes are plain directories.
  ASKBAR_BOOTSTRAP_ROOT="$WORK/credentials" "$BBIN" >/dev/null || { fail "bootstrap makes the tokens"; exit 1; }
  python3 -u tests/e2e/mock_llm.py "$MOCK_PORT" >/dev/null 2>&1 & PIDS+=($!)
  # The auditor and the gateway (ADR 0051, ADR 0054): every reply is recorded before a provider is called.
  ASKBAR_AUDIT_API_ADDR="127.0.0.1:$AUDIT_PORT" ASKBAR_AUDIT_DIR="$WORK/audit" ASKBAR_AUDITOR_TOKEN_DIR="$WORK/credentials/auditor-token" \
    "$AUBIN" >"$WORK/auditor.log" 2>&1 & PIDS+=($!)
  ASKBAR_GATEWAY_SERVER_ADDR="127.0.0.1:$GW_SERVER_PORT" ASKBAR_GATEWAY_ADMIN_ADDR="127.0.0.1:$GW_ADMIN_PORT" \
    ASKBAR_GATEWAY_TOKEN_DIR="$WORK/credentials/model-gateway-token" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" \
    "$GBIN" >"$WORK/gateway.log" 2>&1 & PIDS+=($!)
  # The alerter (ADR 0055) and a stand-in ntfy server it delivers to. The admin gives it its settings once they are entered.
  python3 -u tests/e2e/ntfy_sink.py "$NTFY_PORT" >/dev/null 2>&1 & PIDS+=($!)
  # No release check: smoke makes no request to the internet.
  ASKBAR_ALERTER_RELEASES_URL=off ASKBAR_ALERTER_AUDITOR_ADDR="127.0.0.1:$AL_AUDITOR_PORT" ASKBAR_ALERTER_ADMIN_ADDR="127.0.0.1:$AL_ADMIN_PORT" \
    ASKBAR_ALERTER_TOKEN_DIR="$WORK/credentials/alerter-token" ASKBAR_ALERTER_DATA_DIR="$WORK/alerter-data" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" \
    "$ALBIN" >"$WORK/alerter.log" 2>&1 & PIDS+=($!)
  # The egress proxy (ADR 0056): it denies everything until the admin pushes the allowlists.
  ASKBAR_EGRESS_PROXY_TOKEN_DIR="$WORK/credentials/egress-proxy-token" ASKBAR_EGRESS_PROXY_ADMIN_ADDR="127.0.0.1:$EG_ADMIN_PORT" \
    ASKBAR_EGRESS_PROXY_SOURCES="model-gateway=127.0.0.1:$EG_GW_PORT,cpa=127.0.0.1:$EG_CPA_PORT,alerter=127.0.0.1:$EG_AL_PORT" \
    ASKBAR_EGRESS_PROXY_RELAYS="" "$EBIN" >"$WORK/egress-proxy.log" 2>&1 & PIDS+=($!)
  # The MCP gateway's front (ADR 0057): it refuses every call until the admin pushes its config. `search` goes to a stand-in searxng.
  python3 -u tests/e2e/searxng_mock.py "$SEARX_PORT" >/dev/null 2>&1 & PIDS+=($!)
  ASKBAR_SEARXNG_ADDR="127.0.0.1:$SEARX_PORT" ASKBAR_MCP_GATEWAY_SERVER_ADDR="127.0.0.1:$MCP_SERVER_PORT" ASKBAR_MCP_GATEWAY_ADMIN_ADDR="127.0.0.1:$MCP_ADMIN_PORT" \
    ASKBAR_MCP_GATEWAY_TOKEN_DIR="$WORK/credentials/mcp-gateway-token" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" \
    "$MBIN" >"$WORK/mcp-gateway.log" 2>&1 & PIDS+=($!)
  # The admin service: its own database and secrets, a unix socket and no TCP port.
  ASKBAR_ADMIN_SOCKET="$SOCK" ASKBAR_ADMIN_DATA_DIR="$WORK/admin-data" ASKBAR_ADMIN_SECRETS_DIR="$WORK/admin-secrets" \
    ASKBAR_ADMIN_TOKEN_DIR="$WORK/credentials/admin-token" ASKBAR_SERVER_API_URL="http://127.0.0.1:$SERVER_API_PORT" ASKBAR_PUBLIC_URL="$URL" \
    ASKBAR_GATEWAY_URL="http://127.0.0.1:$GW_ADMIN_PORT" ASKBAR_ALERTER_URL="http://127.0.0.1:$AL_ADMIN_PORT" ASKBAR_EGRESS_PROXY_URL="http://127.0.0.1:$EG_ADMIN_PORT" ASKBAR_MCP_GATEWAY_URL="http://127.0.0.1:$MCP_ADMIN_PORT" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" "$ABIN" >"$WORK/admin.log" 2>&1 & PIDS+=($!)
  for _ in $(seq 100); do [ -S "$SOCK" ] && break; sleep 0.1; done
  # The server: the API, and a server listener that only the admin's token opens.
  ASKBAR_CLIENT_API_ADDR="127.0.0.1:$API_PORT" \
    ASKBAR_SERVER_API_ADDR="127.0.0.1:$SERVER_API_PORT" ASKBAR_SERVER_TOKEN_HASH_FILE="$WORK/credentials/server-token/accept/admin-server.sha256" \
    ASKBAR_DATA_DIR="$WORK/data" ASKBAR_SERVER_TOKEN_DIR="$WORK/credentials/server-token" \
    ASKBAR_GATEWAY_URL="http://127.0.0.1:$GW_SERVER_PORT" ASKBAR_AUDIT_API_URL="http://127.0.0.1:$AUDIT_PORT" \
    ASKBAR_MCP_GATEWAY_URL="http://127.0.0.1:$MCP_SERVER_PORT" ASKBAR_PUBLIC_URL="$URL" "$BIN" >"$WORK/server.log" 2>&1 & PIDS+=($!)
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
      "{\"name\":\"mock\",\"type\":\"openai\",\"base_url\":\"http://127.0.0.1:$MOCK_PORT/v1\",\"model\":\"mock-model\",\"api_key\":\"sk-smoke\"}" >"$WORK/provider.json"; then
    pass "add a provider that points at the mock LLM"
  else
    fail "add a provider that points at the mock LLM"
  fi
  # Nothing answers until a model is approved (ADR 0014): approve the mock's model and make it the default.
  PROVIDER_ID=$(json "$WORK/provider.json" 'd["id"]')
  if admin PUT /api/admin/v1/catalog \
      "{\"models\":[{\"provider_id\":\"$PROVIDER_ID\",\"model\":\"mock-model\"}],\"default\":\"$PROVIDER_ID/mock-model\"}" >/dev/null; then
    pass "approve the mock's model and make it the default"
  else
    fail "approve the mock's model and make it the default"
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

  # 6. The approved models (ADR 0014). The gateway has the catalog a moment after the admin saves it.
  MODEL=""
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    api "$URL/api/client/v1/models" >"$WORK/models.json" 2>/dev/null || true
    MODEL=$(json "$WORK/models.json" 'd["default"] or ""' 2>/dev/null || true)
    [ -n "$MODEL" ] && break
    sleep 1
  done
  if [ -n "$MODEL" ]; then pass "the server lists the approved model ($(json "$WORK/models.json" 'd["models"][0]["provider_name"]') / $(json "$WORK/models.json" 'd["models"][0]["model"]'))"; else fail "the server lists the approved model"; fi

  # 6b. Spaces (ADR 0009): the user has a personal space, listed first; the routes below are under it.
  api "$URL/api/client/v1/spaces" >"$WORK/spaces.json"
  SPACE=$(json "$WORK/spaces.json" 'next(s["id"] for s in d if s["kind"]=="personal")')
  if [ -n "$SPACE" ]; then pass "the user has a personal space"; else fail "the user has a personal space"; fi
  SP="$URL/api/client/v1/spaces/$SPACE"
  # A model nobody approved is refused, with its own error code, before anything starts.
  api -o "$WORK/unapproved.json" -w '%{http_code}' -X POST -H 'content-type: application/json' \
    -d '{"model":"nobody/not-approved","text":"hi"}' "$SP/chat" >"$WORK/unapproved.code" 2>/dev/null
  if [ "$(cat "$WORK/unapproved.code")" = "400" ] && [ "$(json "$WORK/unapproved.json" 'd["error"]')" = "model_not_approved" ]; then
    pass "a model nobody approved is refused (model_not_approved)"
  else
    fail "a model nobody approved is refused (model_not_approved)" "$(cat "$WORK/unapproved.code" 2>/dev/null) $(head -c 200 "$WORK/unapproved.json" 2>/dev/null)"
  fi
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
    -d "{\"model\":\"$MODEL\",\"text\":\"$QUESTION\"}" "$SP/chat" >"$WORK/chat.sse" 2>/dev/null
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

  # 7b. Three replies at once (plan step 7.2): they all finish, and the time to the first text is printed. A measurement here, with the
  # mock model's own time in it; the gate, which subtracts it, is `cargo test -p askbar-server --test load`.
  if LOADOUT=$(python3 -I tests/e2e/load_scenario.py "$URL" --token "$TOKEN" --rounds 4 2>&1); then
    pass "three replies at once all finish: $LOADOUT"
  else
    fail "three replies at once all finish" "$LOADOUT"
  fi

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
    -d "{\"model\":\"$MODEL\",\"text\":\"smoke 429once retry\"}" "$SP/chat" >"$WORK/retry.sse" 2>/dev/null
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

# 10c. Alerts (local, ADR 0055): the admin gives the alerter its delivery settings, a test alert reaches a stand-in ntfy server with the
# token, and the topic and token are in no answer of the admin and no log.
if [ $LOCAL = 1 ]; then
  if admin PUT /api/admin/v1/alerts "{\"ntfy\":{\"server\":\"http://127.0.0.1:$NTFY_PORT\",\"topic\":\"smoke-topic-77\",\"token\":\"smoke-token-77\"}}" >"$WORK/alerts.json" 2>/dev/null \
     && [ "$(json "$WORK/alerts.json" "d['ntfy']['topic'] and d['ntfy']['token']")" = "True" ]; then pass "the admin takes the alert delivery settings and the alerter accepts them"; else fail "the admin takes the alert delivery settings"; fi
  if admin POST /api/admin/v1/alerts/test >"$WORK/alerts-test.json" 2>/dev/null && [ "$(json "$WORK/alerts-test.json" "d['results'][0]['ok']")" = "True" ]; then
    pass "a test alert reports that it was delivered"
  else
    fail "a test alert reports that it was delivered" "$(cat "$WORK/alerts-test.json" 2>/dev/null)"
  fi
  curl -fsS "http://127.0.0.1:$NTFY_PORT/_seen" >"$WORK/ntfy-seen.json" 2>/dev/null || echo '[]' >"$WORK/ntfy-seen.json"
  if [ "$(json "$WORK/ntfy-seen.json" "d[0]['title'] == 'Askbar test alert' and d[0]['authorization'] == 'Bearer smoke-token-77' and d[0]['path'] == '/smoke-topic-77'")" = "True" ]; then
    pass "the stand-in ntfy server received the test alert, on the topic, with the token"
  else
    fail "the stand-in ntfy server received the test alert" "$(cat "$WORK/ntfy-seen.json")"
  fi
  LEAK=""
  for f in "$WORK/alerts.json" "$WORK/alerts-test.json" "$WORK/admin.log" "$WORK/alerter.log" "$WORK/auditor.log"; do
    grep -q -e 'smoke-topic-77' -e 'smoke-token-77' "$f" 2>/dev/null && LEAK="$LEAK $f"
  done
  admin GET /api/admin/v1/status 2>/dev/null | grep -q -e 'smoke-topic-77' -e 'smoke-token-77' && LEAK="$LEAK status"
  if [ -z "$LEAK" ]; then pass "the alert topic and token are in no admin answer and no log"; else fail "the alert topic and token are in no admin answer and no log" "found in:$LEAK"; fi
fi

# 10d. The egress proxy (local, ADR 0056): the admin pushed the allowlists, so the proxy reports a config. The token is the admin's own.
if [ $LOCAL = 1 ]; then
  EG_TOKEN=$(cat "$WORK/credentials/admin-token/call/admin-egress-proxy.token" 2>/dev/null)
  for _ in $(seq 100); do
    curl -fsS -H "Authorization: Bearer $EG_TOKEN" "http://127.0.0.1:$EG_ADMIN_PORT/api/egress-proxy/v1/status" >"$WORK/egress-status.json" 2>/dev/null \
      && [ "$(json "$WORK/egress-status.json" "d['configured']")" = "True" ] && break
    sleep 0.2
  done
  if [ "$(json "$WORK/egress-status.json" "d['configured']")" = "True" ]; then pass "the admin pushed the allowlists and the egress proxy is configured"; else fail "the egress proxy is configured" "$(cat "$WORK/egress-status.json" 2>/dev/null)"; fi
fi

# 10e. The MCP gateway's front (local, ADR 0028): the admin pushed its config, so the front reports a version. The token is the admin's own.
if [ $LOCAL = 1 ]; then
  MG_TOKEN=$(cat "$WORK/credentials/admin-token/call/admin-mcp-gateway.token" 2>/dev/null)
  for _ in $(seq 100); do
    curl -fsS -H "Authorization: Bearer $MG_TOKEN" "http://127.0.0.1:$MCP_ADMIN_PORT/api/mcp-gateway/v1/status" >"$WORK/mcp-status.json" 2>/dev/null \
      && [ "$(json "$WORK/mcp-status.json" "d['config_version'] is not None")" = "True" ] && break
    sleep 0.2
  done
  if [ "$(json "$WORK/mcp-status.json" "d['config_version'] is not None")" = "True" ]; then pass "the admin pushed its config and the MCP gateway's front is configured"; else fail "the MCP gateway's front is configured" "$(cat "$WORK/mcp-status.json" 2>/dev/null)"; fi
fi

# 10e2. Quick search end to end (local, W5.5), before 10f swaps the approval key: the model-gateway signs each call with the key the admin
# pushed, and the front checks it with the public half. The roles are pinned to the mock's model; the stand-in searxng gives three results.
# Fetch cannot succeed natively (the front takes only dotted DNS names on ports 80 and 443, and a fixture host does not resolve): the two plain
# sources are listed unread and the reply says so (tests/quick_search.rs covers the injection cases, tests/fetch.rs the fetch itself).
if [ $LOCAL = 1 ] && [ -n "${TOKEN:-}" ]; then
  MCP_TOK=$(cat "$WORK/credentials/server-token/call/server-mcp-gateway.token" 2>/dev/null)
  qs_tools() { curl -fsS -H "Authorization: Bearer $MCP_TOK" "http://127.0.0.1:$MCP_SERVER_PORT/api/mcp-gateway/v1/tools?user=$1" 2>/dev/null; }
  qs_chat() { # token text out: a chat with web_search on, as a client sends it
    curl -sS -N --max-time 60 -X POST -H "authorization: Bearer $1" -H "x-askbar-client-version: $CLIENT_VERSION" -H 'content-type: application/json' \
      -d "{\"model\":\"$MODEL\",\"text\":\"$2\",\"web_search\":true}" "$SP/chat" >"$3" 2>/dev/null
  }
  qs_trace() { curl -fsS -H "Authorization: Bearer $(cat "$WORK/credentials/admin-token/call/admin-auditor-read.token")" "http://127.0.0.1:$AUDIT_PORT/api/audit/v1/traces/$1" 2>/dev/null; }
  QS_Q="smoke w55 question about stripes"
  for r in search.query search.writer; do admin PUT "/api/admin/v1/roles/$r" "{\"provider_id\":\"$PROVIDER_ID\",\"model\":\"mock-model\"}" >/dev/null 2>&1; done
  admin PUT "/api/admin/v1/mcp/permissions/$USER_ID" '{"allowed":["search","fetch"],"denied":[]}' >/dev/null 2>&1
  for _ in $(seq 100); do qs_tools "$USER_ID" | grep -q '"search"' && break; sleep 0.2; done
  qs_chat "$TOKEN" "$QS_Q" "$WORK/qs.sse"
  read -r QS_OK QS_CONV QS_GEN < <(py 'import json,sys
ev=[]; n=None
for l in open(sys.argv[1], encoding="utf-8"):
    l=l.rstrip("\n")
    if l.startswith("event:"): n=l[6:].strip()
    elif l.startswith("data:") and n: ev.append((n,json.loads(l[5:].strip()))); n=None
names=[x for x,_ in ev if x!="provider"]
s=next((d for x,d in ev if x=="search"),{})
text="".join(d.get("text","") for x,d in ev if x=="delta")
meta=next(d for x,d in ev if x=="meta")
ok=(names[:2]==["meta","search"] and set(names[2:-1])=={"delta"} and names[-1]=="done"
    and s["queries"]==["smoke-w55-query-alpha","smoke-w55-query-beta"] and [x["url"] for x in s["sources"]]==["https://pages.example/a","https://pages.example/b","https://pages.example/c?id=1"]
    and not any(x["fetched"] for x in s["sources"]) and "[1]" in text and "sources seen 3" in text
    and text.count("could not be read")==2 and "Source [3]" not in text)
print("ok" if ok else "bad", meta["conversation_id"], meta["generation_id"])' "$WORK/qs.sse" 2>/dev/null)
  if [ "${QS_OK:-}" = "ok" ]; then pass "quick search: events meta, search (2 queries, 3 sources, none read), deltas, done; the reply cites [1] and says which sources could not be read"; else fail "quick search reply and events" "$(head -c 600 "$WORK/qs.sse")"; fi
  if [ "$(curl -fsS "http://127.0.0.1:$SEARX_PORT/_hits")" = "2" ]; then pass "the stand-in searxng saw exactly 2 searches"; else fail "the stand-in searxng saw exactly 2 searches" "$(curl -sS "http://127.0.0.1:$SEARX_PORT/_hits")"; fi
  api "$SP/conversations/${QS_CONV:-x}" >"$WORK/qs-conv.json"
  if [ "$(json "$WORK/qs-conv.json" 'len(d["messages"])')" = "2" ] && [ "$(json "$WORK/qs-conv.json" 'd["messages"][0]["content"]')" = "$QS_Q" ] \
     && [ "$(json "$WORK/qs-conv.json" '"untrusted_web_content" in str(d) or "pages.example" in str(d)')" = "False" ]; then
    pass "quick search: history holds the question and the answer, no sources text"
  else fail "quick search: history holds the question and the answer" "$(head -c 300 "$WORK/qs-conv.json")"; fi
  sleep 1; qs_trace "${QS_GEN:-x}" >"$WORK/qs-trace.json"
  QS_SPANS=$(json "$WORK/qs-trace.json" "sorted((s['kind'],s['name'],s['status'],(s.get('attrs') or {}).get('role')) for s in d if s['kind'] in ('tool','gate','turn'))")
  if [ "$(json "$WORK/qs-trace.json" "[(s['kind'],s['name'],s['status']) for s in d].count(('tool','search','ok'))")" = "2" ] \
     && [ "$(json "$WORK/qs-trace.json" "sum(1 for s in d if s['kind']=='gate' and s['name']=='quick_search_approval')")" -ge 4 ] 2>/dev/null \
     && [ "$(json "$WORK/qs-trace.json" "sorted((s.get('attrs') or {}).get('role') for s in d if s['kind']=='turn')")" = "['search.query', 'search.writer']" ]; then
    pass "the trace holds 2 search tool spans, the gateway's approval spans and a turn for each of search.query and search.writer"
  else fail "the quick search trace" "$QS_SPANS"; fi
  # A second user without the `search` permission: a plain reply with the fixed note, a refusal span, no search made.
  if admin POST /api/admin/v1/users '{"name":"smoke no-search user"}' >"$WORK/user2.json" \
     && admin POST "/api/admin/v1/users/$(json "$WORK/user2.json" 'd["id"]')/pairing-keys" '{"name":"smoke no-search client"}' >"$WORK/pair2.json"; then
    CODE3=$(py 'import base64,json,sys
b=sys.argv[1][len("askbar-pair:"):]
print(json.loads(base64.urlsafe_b64decode(b+"="*(-len(b)%4)))["code"])' "$(json "$WORK/pair2.json" 'd["pairing"]')")
    TOKEN3=$(curl -fsS -X POST -H "x-askbar-client-version: $CLIENT_VERSION" -H 'content-type: application/json' -d "{\"code\":\"$CODE3\"}" "$URL/api/client/v1/pairings" | py 'import json,sys; print(json.load(sys.stdin)["token"])')
    curl -fsS -H "authorization: Bearer $TOKEN3" -H "x-askbar-client-version: $CLIENT_VERSION" "$URL/api/client/v1/spaces" >"$WORK/spaces3.json"
    SP3="$URL/api/client/v1/spaces/$(json "$WORK/spaces3.json" 'next(s["id"] for s in d if s["kind"]=="personal")')"
    SP_SAVE="$SP"; SP="$SP3"
    qs_chat "$TOKEN3" "smoke w55 no permission question" "$WORK/qs2.sse"
    SP="$SP_SAVE"
    GEN3=$(py 'import json,sys
for l in open(sys.argv[1], encoding="utf-8"):
    if l.startswith("data:") and "generation_id" in l: print(json.loads(l[5:])["generation_id"])' "$WORK/qs2.sse")
    sleep 1; qs_trace "$GEN3" >"$WORK/qs2-trace.json"
    if grep -q 'event: search' "$WORK/qs2.sse" || ! grep -q 'Web search is not turned on for you' "$WORK/qs2.sse" || ! grep -q 'event: done' "$WORK/qs2.sse"; then
      fail "a user without the search permission gets a plain reply with the note" "$(head -c 300 "$WORK/qs2.sse")"
    elif [ "$(json "$WORK/qs2-trace.json" "[(s['kind'],s['name']) for s in d].count(('gate','quick_search_refused'))")" = "1" ] \
         && [ "$(curl -fsS "http://127.0.0.1:$SEARX_PORT/_hits")" = "2" ]; then
      pass "a user without the search permission gets a plain reply with the note, a refusal span and no search"
    else fail "a user without the search permission: refusal span, no search" "$(head -c 300 "$WORK/qs2-trace.json")"; fi
  else fail "make a user without the search permission"; fi
  # No query text in any span's attributes or any service log.
  LEAK=""
  grep -q -e 'smoke-w55-query' -e "$QS_Q" "$WORK/qs-trace.json" "$WORK/qs2-trace.json" 2>/dev/null && LEAK="$LEAK spans"
  for f in server gateway mcp-gateway auditor; do grep -q -e 'smoke-w55-query' -e 'smoke w55' -e 'sources seen' "$WORK/$f.log" 2>/dev/null && LEAK="$LEAK $f.log"; done
  if [ -z "$LEAK" ]; then pass "no query, question or answer text is in any span attribute or in the server, gateway, front or auditor log"; else fail "quick search text in spans or logs" "found in:$LEAK"; fi
fi

# 10e3. Research end to end (local, P5B.7), on the quick search setup above. The native stack cannot fetch a page (see 10e2), so no researcher
# gets text to summarise and the flow ends at "No sources could be gathered" before the writer and the judge: the report, the judge's three
# answers and the writer-stage cap are covered by services/server/tests/research.rs (P5.13); the real proxy path only in the Docker stack.
# Here: the split, query and search stages, the failed fetches, the plain note, the audit spans, the cap, research off, no permission.
if [ $LOCAL = 1 ] && [ -n "${MCP_TOK:-}" ]; then
  rs_chat() { # token text out: a chat with research on, as a client sends it
    curl -sS -N --max-time 90 -X POST -H "authorization: Bearer $1" -H "x-askbar-client-version: $CLIENT_VERSION" -H 'content-type: application/json' \
      -d "{\"model\":\"$MODEL\",\"text\":\"$2\",\"research\":true}" "$SP/chat" >"$3" 2>/dev/null
  }
  rs_read() { # sse file: "stages|event names|generation id|text" of the reply (the text last: it may hold a |)
    py 'import json,sys
ev=[]; n=None
for l in open(sys.argv[1], encoding="utf-8"):
    l=l.rstrip("\n")
    if l.startswith("event:"): n=l[6:].strip()
    elif l.startswith("data:") and n: ev.append((n,json.loads(l[5:].strip()))); n=None
names=[x for x,_ in ev if x!="provider"]
st=",".join(d["stage"] for x,d in ev if x=="progress")
text="".join(d.get("text","") for x,d in ev if x=="delta")
gen=next((d["generation_id"] for x,d in ev if x=="meta"),"")
print(st+"|"+",".join(dict.fromkeys(names))+"|"+gen+"|"+text.replace("\n"," "))' "$1" 2>/dev/null
  }
  RS_HITS() { curl -fsS "http://127.0.0.1:$SEARX_PORT/_hits"; }
  for r in research.researcher research.writer judge.review; do admin PUT "/api/admin/v1/roles/$r" "{\"provider_id\":\"$PROVIDER_ID\",\"model\":\"mock-model\"}" >/dev/null 2>&1; done
  RS_Q="smoke p5b question about stripes"
  H0=$(RS_HITS); rs_chat "$TOKEN" "$RS_Q" "$WORK/rs.sse"
  IFS='|' read -r RS_STAGES RS_NAMES RS_GEN RS_TEXT < <(rs_read "$WORK/rs.sse")
  if [ "${RS_NAMES:-}" = "meta,progress,delta,done" ] && echo "$RS_STAGES" | grep -q '^splitting' && ! echo "$RS_STAGES" | grep -q -e writing -e reviewing \
     && echo "$RS_TEXT" | grep -q 'No sources could be gathered'; then
    pass "research: progress (splitting first), then the plain note that no sources could be gathered, no report"
  else fail "research reply: stages, events, note" "$RS_STAGES | $RS_NAMES | $RS_TEXT"; fi
  if [ "$(( $(RS_HITS) - H0 ))" = "4" ]; then pass "research: two researchers made 4 searches through the front (2 queries each)"; else fail "research: 4 searches" "$(( $(RS_HITS) - H0 ))"; fi
  sleep 1; qs_trace "${RS_GEN:-x}" >"$WORK/rs-trace.json"
  RS_SPANS=$(json "$WORK/rs-trace.json" "sorted((s['kind'],s['name'],(s.get('attrs') or {}).get('role')) for s in d if s['kind']!='generation')")
  if [ "$(json "$WORK/rs-trace.json" "[(s['kind'],s['name'],s['status']) for s in d].count(('tool','search','ok'))")" = "4" ] \
     && [ "$(json "$WORK/rs-trace.json" "sum(1 for s in d if s['kind']=='tool' and s['name']=='fetch')")" -ge 2 ] 2>/dev/null \
     && [ "$(json "$WORK/rs-trace.json" "sorted((s.get('attrs') or {}).get('role') for s in d if s['kind']=='turn')")" = "['research.researcher', 'research.researcher', 'research.writer']" ] \
     && [ "$(json "$WORK/rs-trace.json" "sum(1 for s in d if s['kind']=='gate' and s['name']=='research_approval')")" -ge 4 ] 2>/dev/null; then
    pass "research trace: 4 search and 2 or more failed fetch tool spans, turns for the split (research.writer) and both researchers, the gateway's research_approval spans"
  else fail "the research trace" "$RS_SPANS"; fi
  # The cap at its minimum, and the mock reporting 30000 tokens for the split call: the next model call of the task is refused (402),
  # no researcher searches and the reply is the plain note (the cap note needs a draft in hand: tests/research.rs).
  admin PUT /api/admin/v1/limits '{"token_cap":20000}' >/dev/null 2>&1; sleep 2
  H0=$(RS_HITS); rs_chat "$TOKEN" "smoke p5b cap question\nMOCK-TOKENS 30000" "$WORK/rs-cap.sse"
  IFS='|' read -r _ RS_NAMES RS_CGEN RS_TEXT < <(rs_read "$WORK/rs-cap.sse")
  sleep 1; qs_trace "${RS_CGEN:-x}" >"$WORK/rs-cap-trace.json"
  if [ "${RS_NAMES:-}" = "meta,progress,delta,done" ] && echo "$RS_TEXT" | grep -q 'No sources could be gathered' && [ "$(RS_HITS)" = "$H0" ] \
     && [ "$(json "$WORK/rs-cap-trace.json" "sorted((s.get('attrs') or {}).get('role') for s in d if s['kind']=='turn')")" = "['research.writer']" ]; then
    pass "research cap: at the minimum token cap the second model call is refused, no search is made and the reply is the plain note"
  else fail "research cap path" "$RS_NAMES | $RS_TEXT | $(json "$WORK/rs-cap-trace.json" "[(s['kind'],s['name'],s['status']) for s in d]")"; fi
  admin PUT /api/admin/v1/limits '{"token_cap":null}' >/dev/null 2>&1
  # Research off for everyone: a plain reply with the note, and no research call to the gateway.
  rs_enable() { admin GET /api/admin/v1/settings | py 'import json,sys; d=json.load(sys.stdin); d["research_enabled"]=sys.argv[1]=="true"; print(json.dumps(d))' "$1" >"$WORK/settings-rs.json" \
    && admin PUT /api/admin/v1/settings "$(cat "$WORK/settings-rs.json")" >/dev/null 2>&1; }
  rs_enable false; sleep 2
  H0=$(RS_HITS); rs_chat "$TOKEN" "smoke p5b off question" "$WORK/rs-off.sse"
  IFS='|' read -r _ RS_NAMES RS_OGEN RS_TEXT < <(rs_read "$WORK/rs-off.sse")
  sleep 1; qs_trace "${RS_OGEN:-x}" >"$WORK/rs-off-trace.json"
  if ! echo "$RS_NAMES" | grep -q progress && echo "$RS_TEXT" | grep -q 'Research is turned off by your administrator' && [ "$(RS_HITS)" = "$H0" ] \
     && [ "$(json "$WORK/rs-off-trace.json" "[s for s in d if (s.get('attrs') or {}).get('role') in ('research.writer','research.researcher','judge.review')]")" = "[]" ]; then
    pass "research off: a plain reply with the note, no progress, no search and no research model call"
  else fail "research off path" "$RS_NAMES | $RS_TEXT"; fi
  rs_enable true
  # A user without the permissions: the plain note.
  if [ -n "${TOKEN3:-}" ]; then
    SP_SAVE="$SP"; SP="$SP3"; rs_chat "$TOKEN3" "smoke p5b no permission question" "$WORK/rs-np.sse"; SP="$SP_SAVE"
    IFS='|' read -r _ RS_NAMES _ RS_TEXT < <(rs_read "$WORK/rs-np.sse")
    if ! echo "$RS_NAMES" | grep -q progress && echo "$RS_TEXT" | grep -q 'Research is not turned on for you'; then pass "a user without the permissions gets a plain reply with the note"
    else fail "research without permission" "$RS_NAMES | $RS_TEXT"; fi
  fi
  LEAK=""
  grep -q -e 'smoke-p5b' -e 'smoke p5b' "$WORK"/rs-*trace.json 2>/dev/null && LEAK="$LEAK spans"
  for f in server gateway mcp-gateway auditor; do grep -q -e 'smoke-p5b' -e 'smoke p5b' "$WORK/$f.log" 2>/dev/null && LEAK="$LEAK $f.log"; done
  if [ -z "$LEAK" ]; then pass "no question, sub-question or summary text is in any research span attribute or in the server, gateway, front or auditor log"; else fail "research text in spans or logs" "found in:$LEAK"; fi
fi

# 10f. MCP search (local, P5A.23): a call with no signature is refused, a signed one returns untrusted results, its tool span is in the
# audit store, and no query or result is in the front's log. The smoke has no private approval key (only model-gateway holds it, in
# memory), so it makes its own and writes the PUBLIC half over the throwaway admin.db's, which the admin then pushes to the front.
# Fetch is not here: the front takes only dotted DNS names on ports 80 and 443, which a native run cannot resolve (tests/fetch.rs covers it).
if [ $LOCAL = 1 ]; then
  { read -r APPROVAL_KEY; read -r APPROVAL_PUB; } < <("$SBIN" generate); export APPROVAL_KEY
  py 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1],timeout=10); c.execute("UPDATE approval_keys SET public_key=?, version=version+1 WHERE id=1",(bytes.fromhex(sys.argv[2]),)); c.commit()' "$WORK/admin-data/admin.db" "$APPROVAL_PUB"
  admin PUT "/api/admin/v1/mcp/permissions/$USER_ID" '{"allowed":["search"],"denied":[]}' >/dev/null 2>&1
  mcp_call() { # task_id one_time_id blank_signature: POST a signed search call to the front's server listener
    local act='{"tool":"search","arguments":{"query":"smoke-query-zebra-91"},"destination":"searxng"}'
    local ap; ap=$("$SBIN" sign "$USER_ID" "$1" "$2" $(( $(date +%s) + 60 )) <<<"$act")
    py 'import json,sys
a=json.loads(sys.argv[1]); ap=json.loads(sys.argv[2])
if sys.argv[3]=="1": ap["signature"]=""
del a["tool"]; a["approval"]=ap; print(json.dumps(a))' "$act" "$ap" "$3" \
      | curl -sS -X POST -H "Authorization: Bearer $(cat "$WORK/credentials/server-token/call/server-mcp-gateway.token")" -H 'content-type: application/json' \
        -d @- "http://127.0.0.1:$MCP_SERVER_PORT/api/mcp-gateway/v1/tools/search/call"
  }
  MG_TASK="smoke-task-$$"
  # The admin pushes the new key and permission within a push or two; the first signed call that is not refused ends the wait.
  for i in $(seq 100); do
    mcp_call "$MG_TASK" "one-time-signed-$i" 0 >"$WORK/mcp-search.json" 2>/dev/null
    [ "$(json "$WORK/mcp-search.json" "d.get('untrusted')")" = "True" ] && break
    sleep 0.2
  done
  mcp_call "$MG_TASK" one-time-unsigned 1 >"$WORK/mcp-unsigned.json" 2>/dev/null
  if [ "$(json "$WORK/mcp-unsigned.json" "d['error']")" = "approval_unsigned" ]; then pass "an MCP tool call with no signature is refused (approval_unsigned)"; else fail "an unsigned MCP call is refused" "$(head -c 200 "$WORK/mcp-unsigned.json")"; fi
  if [ "$(json "$WORK/mcp-search.json" "d['untrusted'] and len(d['result']['results'])")" = "3" ]; then pass "a signed MCP search returns three results, marked untrusted"; else fail "a signed MCP search returns results" "$(head -c 200 "$WORK/mcp-search.json")"; fi
  AUD_TOKEN=$(cat "$WORK/credentials/admin-token/call/admin-auditor-read.token" 2>/dev/null)
  curl -fsS -H "Authorization: Bearer $AUD_TOKEN" "http://127.0.0.1:$AUDIT_PORT/api/audit/v1/traces/$MG_TASK" >"$WORK/mcp-trace.json" 2>/dev/null
  if [ "$(json "$WORK/mcp-trace.json" "[(s['kind'],s['name'],s['status']) for s in d].count(('tool','search','ok'))")" = "1" ]; then pass "the audit store holds the search's tool span"; else fail "the audit store holds the search's tool span" "$(head -c 300 "$WORK/mcp-trace.json")"; fi
  if grep -q -e 'smoke-query-zebra-91' -e 'pages.example' -e 'Snippet' "$WORK/mcp-gateway.log" 2>/dev/null; then fail "no query or result in the front's log"; else pass "no query or result is in the front's log"; fi
fi

# 11. What only a person can check.
ask_person "Does the desktop client, paired to this server, show the streamed reply?"

echo
echo "$PASSED passed, $FAILED failed, $SKIPPED skipped"
[ "$FAILED" -eq 0 ]
