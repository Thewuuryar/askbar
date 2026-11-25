#!/usr/bin/env bash
# The engine trial (G14, plan step P5A.13): which hosts each enabled SearXNG engine needs, found by what the egress proxy denies.
# Phase 4's proxy has no log-only mode, and P5A.19 makes the admin push SEARXNG_EGRESS_HOSTS as the `searxng` allowlist. Hosts on a
# pushed list are allowed, never denied, so the trial only measures with an EMPTY pushed list. The script runs one query per engine
# from a throwaway container on mcp-searxng, waits for the admin's next read of the proxy's status (it reads every 12 seconds),
# takes the `searxng` entries of GET /api/admin/v1/egress/denied, and prints the union as a Rust array to paste into
# services/admin/src/constants.rs. The owner approves the list (G14). Per engine it also prints the result count and the
# `unresponsive_engines` reasons from SearXNG, so a failure that is not the proxy (CAPTCHA, HTTP error, timeout) shows next to the denials.
#
# Limit: an engine that stops at its first denied host hides the hosts it would have gone on to (a redirect, an API call). Treat
# the output as the first layer; a later run against the pushed list shows what is still missing.
#
# Usage (needs Docker; reads nothing from the engines but what they answer, and sends the query "askbar" to each):
#   1. In services/admin/src/constants.rs set SEARXNG_EGRESS_HOSTS to `pub const SEARXNG_EGRESS_HOSTS: [&str; 0] = [];` for the
#      trial (note the current list first; the output below replaces it). An empty array compiles with egress_link.rs and passes
#      the constants test, which only checks each entry.
#   2. docker compose up -d --build admin  # the stack running, searxng and egress-proxy healthy, the admin pushing the empty list
#   3. ASKBAR_ADMIN_PASSWORD=<admin password> scripts/searxng-engine-trial.sh
#   4. Paste the printed array into constants.rs (or restore the old one), then rebuild the admin again.
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

query() { # engine: one search for that engine alone; prints "<http status> <result count> <unresponsive engine:reason, ...>"
  docker run --rm --network "${PROJECT}_mcp-searxng" -e "ENG=$1" "$PYIMG" python -I -c '
import os, json, urllib.request, urllib.error, urllib.parse
d = urllib.parse.urlencode({"q": "askbar", "format": "json", "engines": os.environ["ENG"]}).encode()
try:
    r = urllib.request.urlopen(urllib.request.Request("http://searxng-host:8080/search", data=d), timeout=30)
    j = json.loads(r.read())
    u = ", ".join("%s:%s" % (x[0], x[1]) for x in j.get("unresponsive_engines", [])) or "-"
    print(r.status, len(j.get("results", [])), u)
except urllib.error.HTTPError as e: print(e.code, 0, "-")
except Exception as e: print("error", 0, type(e).__name__)'
}

denied() { # the hosts the proxy denied the searxng source, one per line; with ALL=1 every source: "source host port reason"
  docker run --rm --network "${PROJECT}_edge" -v "${PROJECT}_admin-run:/run/admin" -e "PW=$ASKBAR_ADMIN_PASSWORD" -e "ALL=${ALL:-}" "$PYIMG" python -I -c '
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
    if os.environ["ALL"]: print(e["source"], e["host"], e.get("port"), e["reason"])
    elif e["source"] == "searxng": print(e["host"])'
}

all=""; seen=0
for e in $ENGINES; do
  read -r st n un <<<"$(query "$e")"
  sleep 14  # the admin reads the proxy's status every 12 seconds
  got=$(denied) || { echo "could not read the denied list" >&2; exit 1; }
  echo "== $e: query status $st, $n results, unresponsive: $un"
  echo "$got" | sed 's/^/   denied: /'
  [ -n "$got" ] || [ "${n:-0}" -gt 0 ] && seen=1
  all=$(printf '%s\n%s\n' "$all" "$got")
done
hosts=$(printf '%s\n' "$all" | grep -v '^$' | sort -u)
if [ -z "$hosts" ]; then
  echo "no host was denied for searxng. Either the pushed allowlist is not empty (then hosts are allowed, never denied), or nothing reached the proxy:" >&2
  echo "  - results > 0 with an empty allowlist: the engine did NOT go through the proxy (a searxng settings problem)" >&2
  echo "  - 0 results, unresponsive 'HTTP error' or 'access denied', nothing denied: check the proxy: docker compose logs egress-proxy" >&2
  echo "Denied by the proxy for all sources (source host port reason):" >&2
  ALL=1 denied | sed 's/^/   /' >&2
  [ "$seen" = 1 ] && exit 0 || exit 1
fi
echo; echo "Paste into services/admin/src/constants.rs (then the owner approves it, G14):"
printf 'pub const SEARXNG_EGRESS_HOSTS: [&str; %s] = [%s];\n' "$(printf '%s\n' "$hosts" | wc -l)" "$(printf '%s\n' "$hosts" | sed 's/.*/"&"/' | paste -sd, - | sed 's/,/, /g')"
