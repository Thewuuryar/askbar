#!/usr/bin/env bash
# Runs the pinned CLIProxyAPI image (the digest in the Dockerfile's `cpa` target) on its own and checks the parts of its
# management API the admin depends on (plan step 3.5, ADR 0014, ADR 0042): both providers' login URLs and the address each
# sends the browser back to, how a wrong state, an unknown state and a missing secret are answered, and what the account
# list looks like. No account is needed and nothing leaves the machine except the image pull; a real login is a person's.
# Needs Docker. Run it after changing the pinned digest.
set -u
cd "$(dirname "$0")/.."
IMAGE=$(sed -n 's/^FROM \(eceasy\/cli-proxy-api:[^ ]*\) AS cpa$/\1/p' Dockerfile)
[ -n "$IMAGE" ] || { echo "no pinned image found in the Dockerfile" >&2; exit 2; }
PORT=${CPA_CHECK_PORT:-18317}; NAME=askbar-cpa-image-check; WORK=$(mktemp -d)
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$WORK"; }
trap cleanup EXIT
pass=0; failn=0
ok() { echo "ok    $1"; pass=$((pass+1)); }
bad() { echo "FAIL  $1 ${2:-}"; failn=$((failn+1)); }

mkdir -p "$WORK/cfg" "$WORK/auth"
# The file the admin writes (services/admin/src/cpa.rs `config_yaml`), with throwaway values.
printf 'config-version: 8\nserver:\n  host: ""\n  port: 8317\noauth:\n  auth-dir: "/root/.cli-proxy-api"\naccess:\n  api-keys:\n    - "check-api-key"\nmanagement:\n  allow-remote: true\n  secret-key: "check-control-secret"\n' > "$WORK/cfg/config.yaml"
docker run -d --name "$NAME" -p "127.0.0.1:$PORT:8317" -v "$WORK/cfg:/CLIProxyAPI/config-dir" -v "$WORK/auth:/root/.cli-proxy-api" \
  "$IMAGE" ./CLIProxyAPI -config /CLIProxyAPI/config-dir/config.yaml >/dev/null || { echo "could not start $IMAGE" >&2; exit 2; }
B="http://127.0.0.1:$PORT/v0/management"; H="Authorization: Bearer check-control-secret"
for _ in $(seq 1 30); do curl -fsS -o /dev/null -H "$H" "$B/auth-files" 2>/dev/null && break; sleep 1; done
py() { python3 -I -c "$@"; }
field() { py 'import json,sys; d=json.load(sys.stdin); print(d.get(sys.argv[1],""))' "$1"; }
redirect() { py 'import sys,urllib.parse as u; q=u.parse_qs(u.urlparse(sys.stdin.read().strip()).query); print(q.get("redirect_uri",[""])[0])'; }
echo "image: $IMAGE"

[ "$(curl -s -o /dev/null -w '%{http_code}' "$B/auth-files")" = 401 ] && ok "the management API refuses a call with no secret" || bad "the management API answered a call with no secret"
curl -s -H "$H" "$B/auth-files" > "$WORK/files.json"
[ "$(py 'import json,sys; print(type(json.load(open(sys.argv[1]))["files"]).__name__)' "$WORK/files.json")" = list ] && ok "the account list is {\"files\": [...]}" || bad "the account list is not {\"files\": [...]}" "$(cat "$WORK/files.json")"

for pair in "anthropic-auth-url http://localhost:54545/callback" "codex-auth-url http://localhost:1455/auth/callback"; do
  set -- $pair; endpoint=$1; want=$2
  curl -s -H "$H" "$B/$endpoint?is_webui=true" > "$WORK/login.json"
  state=$(field state < "$WORK/login.json"); url=$(field url < "$WORK/login.json")
  [ "$(field status < "$WORK/login.json")" = ok ] && [ -n "$state" ] && [ -n "$url" ] && ok "$endpoint gives a status, a state and an address" || bad "$endpoint gave no login" "$(cat "$WORK/login.json")"
  got=$(printf '%s' "$url" | redirect)
  [ "$got" = "$want" ] && ok "$endpoint sends the browser back to $want" || bad "$endpoint sends the browser back to '$got', not $want"
  case "$url" in *"state=$state"*) ok "$endpoint's address carries its state";; *) bad "$endpoint's address does not carry its state";; esac
  [ "$(curl -s -o /dev/null -w '%{http_code}' -H "$H" -H 'content-type: application/json' \
      -d "{\"provider\":\"x\",\"redirect_url\":\"http://localhost/cb?code=c&state=WRONG\",\"code\":\"c\",\"state\":\"WRONG\"}" "$B/oauth-callback")" = 404 ] \
    && ok "a callback with a state that is not a login's is refused (404)" || bad "a callback with a wrong state was not refused"
done
[ "$(curl -s -H "$H" "$B/get-auth-status?state=nope" | field status)" = error ] && ok "an unknown state reads as status error" || bad "an unknown state does not read as status error"

echo; echo "$pass passed, $failn failed"; [ "$failn" = 0 ]
