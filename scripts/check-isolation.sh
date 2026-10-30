#!/usr/bin/env bash
# Isolation checks for a running Compose stack (`docker compose up -d`).
#
#   scripts/check-isolation.sh
#
# Proves the split between the server, the admin and Caddy holds in the real containers:
# the admin works with the server stopped, the management port needs the token (and
# fails closed without a hash file), only the admin can reach it, the server cannot touch
# the admin's files or socket, and the admin allowlist does what its switch says.
#
# Meant for a throwaway stack: it stops and restarts the server, and it sets the admin's
# network settings (a made-up hostname, no certificate is requested). Needs docker and the
# python:3.12-slim image. Set ASKBAR_ADMIN_PASSWORD if the admin already has a password;
# otherwise the first-run password is set to a random one.
set -u

PROJECT="${COMPOSE_PROJECT_NAME:-askbar}"
EDGE="${PROJECT}_edge"; MGMT="${PROJECT}_mgmt"
SERVER=askbar-server; ADMIN=askbar-admin; CADDY=askbar-caddy
SERVER_MGMT=172.30.0.2:8082
PYIMG="${ASKBAR_CHECK_IMAGE:-python:3.12-slim}"
PW="${ASKBAR_ADMIN_PASSWORD:-$(head -c 24 /dev/urandom | base64 | tr -d '/+=')}"
FAILS=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; FAILS=$((FAILS + 1)); }
check() { # description command...   (passes when the command succeeds)
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then pass "$d"; else fail "$d"; fi
}
refused() { # description command...  (passes when the command fails)
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then fail "$d"; else pass "$d"; fi
}

command -v docker >/dev/null || { echo "docker is required"; exit 2; }
for c in $SERVER $ADMIN $CADDY; do
  [ "$(docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null)" = true ] || { echo "$c is not running: docker compose up -d first"; exit 2; }
done

# Run a python snippet in a throwaway container: py <docker run args...> -- <code>
py() {
  local args=() code
  while [ "$1" != "--" ]; do args+=("$1"); shift; done; shift; code="$1"
  docker run --rm "${args[@]}" "$PYIMG" python -I -c "$code"
}

# HTTP status of a request to host:port from a throwaway container on a network; 0 = no connection.
http_status() { # network host:port [bearer]
  py --network "$1" -- "
import sys, urllib.request, urllib.error
req = urllib.request.Request('http://$2/mgmt/v1/health')
if '${3:-}': req.add_header('authorization', 'Bearer ${3:-}')
try: print(urllib.request.urlopen(req, timeout=4).status)
except urllib.error.HTTPError as e: print(e.code)
except Exception: print(0)
"
}

# Admin API through the admin's unix socket. admin METHOD PATH [JSON] -> prints 'status body'
ADMIN_PY='
import socket, json, http.client, sys
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
'
admin_script() { # python lines using call(); the result is printed by the lines themselves
  py --network "$EDGE" -v "${PROJECT}_admin-run:/run/admin" -- "$ADMIN_PY
$1"
}
admin_login() { # python lines that sign in (or run first-run setup), used before every script
  cat <<EOF
s, _ = call("POST", "/admin/login", {"password": "$PW"})
if s != 204:
    call("POST", "/admin/api/setup/password", {"password": "$PW"})
    s, _ = call("POST", "/admin/login", {"password": "$PW"})
assert s == 204 or s == 200, "cannot sign in to the admin"
EOF
}

echo "== 1. The admin works with the server stopped"
docker stop "$SERVER" >/dev/null
out=$(admin_script "$(admin_login)
s, body = call('GET', '/admin/api/status')
print(s, 'offline' if '\"server_online\":false' in body.replace(' ', '') else 'online?')
s, _ = call('GET', '/admin/')
print(s)" 2>&1)
if printf '%s' "$out" | head -1 | grep -q '^200 offline'; then pass "admin signs in and reports the server offline"; else fail "admin with the server stopped: $out"; fi
printf '%s' "$out" | tail -1 | grep -q '^200$' && pass "admin page loads" || fail "admin page loads"
docker start "$SERVER" >/dev/null
for i in $(seq 1 30); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$SERVER")" = healthy ] && break; sleep 2; done
[ "$(docker inspect -f '{{.State.Health.Status}}' "$SERVER")" = healthy ] && pass "server healthy again" || fail "server did not come back"

echo "== 2. The management port"
HASH=$(docker exec "$ADMIN" cat /mgmt/token.sha256 2>/dev/null | tr -d '[:space:]')
[ -n "$HASH" ] || fail "could not read the token hash from the admin's /mgmt"
[ "$(http_status "$MGMT" "$SERVER_MGMT")" = 401 ] && pass "401 without a token" || fail "no token should give 401"
[ "$(http_status "$MGMT" "$SERVER_MGMT" "$HASH")" = 401 ] && pass "401 with the token's hash instead of the token" || fail "the hash must not work as the token"
[ "$(http_status "$MGMT" "$SERVER_MGMT" wrong)" = 401 ] && pass "401 with a wrong token" || fail "a wrong token should give 401"

# A second server on its own scratch volumes, with no hash file at all: nothing may be accepted.
docker rm -f askbar-check-nohash >/dev/null 2>&1
docker run -d --name askbar-check-nohash --network "$MGMT" --ip 172.30.0.9 \
  -e ASKBAR_MGMT_ADDR=172.30.0.9:8082 -e ASKBAR_API_ADDR=127.0.0.1:8080 \
  -e ASKBAR_MGMT_TOKEN_HASH_FILE=/nowhere/token.sha256 "$(docker inspect -f '{{.Config.Image}}' "$SERVER")" >/dev/null
sleep 3
code=$(http_status "$MGMT" 172.30.0.9:8082 "$HASH")
case "$code" in 401|403|503) pass "fails closed with no hash file ($code)";; *) fail "no hash file: got $code, expected a refusal";; esac
docker rm -f askbar-check-nohash >/dev/null 2>&1

docker exec "$CADDY" sh -c "wget -q -T3 -O /dev/null http://$SERVER_MGMT/mgmt/v1/health" >/dev/null 2>&1 \
  && fail "Caddy reached the management port" || pass "Caddy cannot reach the management port"
EDGE_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$EDGE\").IPAddress}}" "$SERVER")
[ "$(http_status "$EDGE" "$EDGE_IP:8082")" = 0 ] && pass "nothing on the edge network answers on 8082" || fail "the management port answers on the edge network"
[ "$(http_status "$EDGE" "$SERVER_MGMT")" = 0 ] && pass "a container on edge cannot reach the mgmt address" || fail "the mgmt address is reachable from edge"
[ "$(docker inspect -f '{{len .NetworkSettings.Networks}}' "$ADMIN")" = 1 ] && pass "the admin is on one network only (mgmt)" || fail "the admin joined more than one network"

echo "== 3. What the server can see"
VOLS=$(docker inspect -f '{{range .Mounts}}{{.Name}} {{end}}' "$SERVER")
for v in admin-secrets admin-data admin-run caddy-run caddy-secrets; do
  case " $VOLS" in *" ${PROJECT}_$v "*) fail "the server mounts $v";; *) pass "the server does not mount $v";; esac
done
refused "the server cannot list /admin-secrets" docker exec "$SERVER" ls /admin-secrets
refused "the server cannot reach an admin socket" docker exec "$SERVER" test -e /run/admin/admin.sock
refused "the server cannot write to /mgmt (read-only)" docker exec "$SERVER" sh -c 'touch /mgmt/x'
[ "$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/mgmt"}}{{.RW}}{{end}}{{end}}' "$SERVER")" = false ] && pass "/mgmt is mounted read-only in the server" || fail "/mgmt is writable in the server"
[ "$(docker exec "$SERVER" id -u)" != "$(docker exec "$ADMIN" id -u)" ] && pass "the server and the admin run as different users" || fail "the server and the admin share a user"
# Even as the admin's own uid, the server container has nothing of the admin's to open.
refused "the admin's database is not in the server" docker exec -u "$(docker exec "$ADMIN" id -u)" "$SERVER" sh -c 'ls /admin-data/admin.db'
refused "the admin cannot read the server's master key" docker exec "$ADMIN" sh -c 'cat /secrets/master.key'

echo "== 4. The allowlist"
caddy_config() { # prints the admin site's route from Caddy's loaded config
  py -u 10001:10001 -v "${PROJECT}_caddy-run:/r" -- "
import socket
s = socket.socket(socket.AF_UNIX); s.connect('/r/admin.sock')
s.sendall(b'GET /config/apps/http/servers/ HTTP/1.0\r\nHost: x\r\n\r\n')
data = b''
while True:
    chunk = s.recv(65536)
    if not chunk: break
    data += chunk
print(data.decode().split('\r\n\r\n', 1)[1])
"
}
set_network() { # allowlist_enabled(true|false)
  admin_script "$(admin_login)
s, body = call('PUT', '/admin/api/network', {'tls_enabled': True, 'hostname': 'askbar.isolation.invalid', 'admin_cidr': '10.0.0.0/8', 'acme_staging': True, 'allowlist_enabled': $1, 'cloudflare_token': 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN'})
assert s == 200, (s, body)
s, body = call('POST', '/admin/api/network/apply')
print(s, 'ok' if '\"ok\":true' in body.replace(' ', '') else body)"
}
out=$(set_network True 2>&1); printf '%s' "$out" | tail -1 | grep -q '^200 ok' && pass "allowlist on: Caddy took the configuration" || fail "allowlist on: $out"
cfg=$(caddy_config)
printf '%s' "$cfg" | grep -q '"abort":true' && printf '%s' "$cfg" | grep -q '"not":\[{"remote_ip":{"ranges":\["10.0.0.0/8"\]' \
  && pass "allowlist on: addresses outside 10.0.0.0/8 are aborted on the admin site" || fail "allowlist on: no abort rule for outside addresses"
out=$(set_network False 2>&1); printf '%s' "$out" | tail -1 | grep -q '^200 ok' && pass "allowlist off: Caddy took the configuration" || fail "allowlist off: $out"
cfg=$(caddy_config)
if printf '%s' "$cfg" | grep -q 'unix//run/admin/admin.sock' && ! printf '%s' "$cfg" | grep -q '"abort":true\|remote_ip'; then
  pass "allowlist off: the admin site has no address check, any device reaches the login"
else
  fail "allowlist off: the admin site still has an address rule"
fi
# Caddy's own admin API: unix socket only.
refused "Caddy's admin API refuses TCP" docker exec "$CADDY" sh -c 'wget -q -T3 -O /dev/null http://127.0.0.1:2019/config/'
check "uid 10001 reaches Caddy's admin API through the socket" py -u 10001:10001 -v "${PROJECT}_caddy-run:/r" -- "
import socket
s = socket.socket(socket.AF_UNIX); s.connect('/r/admin.sock')"

echo
if [ "$FAILS" -eq 0 ]; then echo "All isolation checks passed."; else echo "$FAILS isolation check(s) FAILED."; exit 1; fi
