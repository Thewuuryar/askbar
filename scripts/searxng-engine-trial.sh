#!/usr/bin/env bash
# The engine trial (G14, plan step P5A.13): which hosts each enabled SearXNG engine needs, found by what the egress proxy denies.
# Phase 4's proxy has no log-only mode, and the admin pushes no `searxng` allowlist yet, so the `searxng` source is empty and denies
# everything. The script runs one query per engine from a throwaway container on mcp-searxng, waits for the admin's next read of
# the proxy's status (it reads every 12 seconds), takes the `searxng` entries of GET /api/admin/v1/egress/denied, and prints the
# union as a Rust array to paste into services/admin/src/constants.rs. The owner approves the list (G14).
#
# Limit: an engine that stops at its first denied host hides the hosts it would have gone on to (a redirect, an API call). Treat
# the output as the first layer; P5A.19 pushes the list, and a later run against the pushed list shows what is still missing.
#
# Usage (needs Docker; reads nothing from the engines but what they answer, and sends the query "askbar" to each):
#   docker compose up -d --build          # the stack, with the searxng and egress-proxy services running and healthy
#   ASKBAR_ADMIN_PASSWORD=<admin password> scripts/searxng-engine-trial.sh
#   ENGINES="duckduckgo brave yahoo mwmbl"   the engines to try (default: the four in docker/searxng/settings.yml)
#   COMPOSE_PROJECT_NAME                         the project name (default askbar)
set -u
PROJECT="${COMPOSE_PROJECT_NAME:-askbar}"
ENGINES="${ENGINES:-duckduckgo brave yahoo mwmbl}"
PYIMG="${ASKBAR_CHECK_IMAGE:-python:3.12-slim@sha256:4840d514c583a4d3d8cad3940e68dee6f0f6f3635c12fcd0a3d74c21a5b93a54}"
: "${ASKBAR_ADMIN_PASSWORD:?set ASKBAR_ADMIN_PASSWORD to the admin password}"
command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }
for c in askbar-searxng askbar-egress-proxy askbar-admin; do
  [ "$(docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null)" = true ] || { echo "$c is not running: docker compose up -d first" >&2; exit 2; }
done

query() { # engine: one search for that engine alone; prints the HTTP status
  docker run --rm --network "${PROJECT}_mcp-searxng" -e "ENG=$1" "$PYIMG" python -I -c '
import os, urllib.request, urllib.error, urllib.parse
d = urllib.parse.urlencode({"q": "askbar", "format": "json", "engines": os.environ["ENG"]}).encode()
try: print(urllib.request.urlopen(urllib.request.Request("http://searxng-host:8080/search", data=d), timeout=30).status)
except urllib.error.HTTPError as e: print(e.code)
except Exception as e: print("error", type(e).__name__)'
}

denied() { # the hosts the proxy denied the searxng source, one per line
  docker run --rm --network "${PROJECT}_edge" -v "${PROJECT}_admin-run:/run/admin" -e "PW=$ASKBAR_ADMIN_PASSWORD" "$PYIMG" python -I -c '
import os, socket, json, http.client
class U(http.client.HTTPConnection):
    def __init__(s): super().__init__("admin")
    def connect(s):
        s.sock = socket.socket(socket.AF_UNIX); s.sock.connect("/run/admin/admin.sock")
c = U(); cookie = ""
def call(m, p, b=None):
    global cookie
    c.request(m, p, json.dumps(b) if b is not None else None, {"x-askbar-admin": "1", "content-type": "application/json", "cookie": cookie})
    r = c.getresponse(); d = r.read(); sc = r.getheader("set-cookie")
    if sc: cookie = sc.split(";")[0]
    return r.status, d.decode()
s, _ = call("POST", "/api/admin/v1/session", {"username": "admin", "password": os.environ["PW"]})
assert s in (200, 204), "cannot sign in to the admin"
s, body = call("GET", "/api/admin/v1/egress/denied")
d = json.loads(body); assert d["online"], d["error"]
for e in d["denied"]:
    if e["source"] == "searxng": print(e["host"])'
}

all=""
for e in $ENGINES; do
  echo "== $e: query status $(query "$e")"
  sleep 14  # the admin reads the proxy's status every 12 seconds
  got=$(denied) || { echo "could not read the denied list" >&2; exit 1; }
  echo "$got" | sed 's/^/   denied: /'
  all=$(printf '%s\n%s\n' "$all" "$got")
done
hosts=$(printf '%s\n' "$all" | grep -v '^$' | sort -u)
[ -n "$hosts" ] || { echo "no host was denied: the engines did not reach the proxy, check the searxng logs" >&2; exit 1; }
echo; echo "Paste into services/admin/src/constants.rs (then the owner approves it, G14):"
printf 'pub const SEARXNG_EGRESS_HOSTS: [&str; %s] = [%s];\n' "$(printf '%s\n' "$hosts" | wc -l)" "$(printf '%s\n' "$hosts" | sed 's/.*/"&"/' | paste -sd, - | sed 's/,/, /g')"
