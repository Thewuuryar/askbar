#!/usr/bin/env bash
# Isolation checks for a running Compose stack (`docker compose up -d`).
#
#   scripts/check-isolation.sh
#
# Proves the split between the server, the admin and Caddy holds in the real containers:
# the admin works with the server stopped, the server port needs the token (and
# fails closed without a hash file), only the admin can reach it, the server cannot touch
# the admin's files or socket, the admin allowlist does what its switch says, the server image
# carries no admin code, secret files are mode 600, a backup holds no secrets, and before a host
# name is set Caddy serves HTTPS on 443 only, with the install's own certificate.
#
# Meant for a throwaway stack: it stops and restarts the server, and it sets the admin's
# network settings (a made-up hostname, no certificate is requested), then puts them back to a fresh
# install's (no host name, allowlist off, the Cloudflare token cleared). Needs docker and the
# python:3.12-slim image. Set ASKBAR_ADMIN_PASSWORD if the admin already has a password;
# otherwise the first-run password is set to a random one.
set -u

PROJECT="${COMPOSE_PROJECT_NAME:-askbar}"
EDGE="${PROJECT}_edge"; SERVER_API_NET="${PROJECT}_server-api"
SERVER=askbar-server; ADMIN=askbar-admin; CADDY=askbar-caddy
PYIMG="${ASKBAR_CHECK_IMAGE:-python:3.12-slim@sha256:4840d514c583a4d3d8cad3940e68dee6f0f6f3635c12fcd0a3d74c21a5b93a54}"
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
export DOCKER_CLI_HINTS=false
# Pull the helper image up front so its progress output never lands in a check's captured output.
docker image inspect "$PYIMG" >/dev/null 2>&1 || docker pull -q "$PYIMG" >/dev/null || { echo "could not pull $PYIMG"; exit 2; }
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
req = urllib.request.Request('http://$2/api/server/v1/health')
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
# Before anything else: an admin whose published first-run password was already changed needs
# ASKBAR_ADMIN_PASSWORD. Without it every step would sign in wrongly, fail, and (five wrong tries) lock sign-in
# out for 15 minutes.
if [ -z "${ASKBAR_ADMIN_PASSWORD:-}" ]; then
  state=$(admin_script "s, body = call('GET', '/api/admin/v1/setup'); print(json.loads(body).get('password_change_required') if s == 200 else 'unknown')" 2>/dev/null | tail -1)
  if [ "$state" = "False" ]; then
    echo "The admin already has a password. Run again with it:" >&2
    echo "  ASKBAR_ADMIN_PASSWORD='your admin password' scripts/check-isolation.sh" >&2
    exit 2
  fi
fi
admin_login() { # python lines that sign in (or, on a fresh install, change the published password), used before every script
  cat <<EOF
s, _ = call("POST", "/api/admin/v1/session", {"username": "admin", "password": "$PW"})
if s != 204:
    s, _ = call("POST", "/api/admin/v1/session", {"username": "admin", "password": "admin"})
    if s == 204:
        call("PUT", "/api/admin/v1/password", {"current": "admin", "new": "$PW"})
    s, _ = call("POST", "/api/admin/v1/session", {"username": "admin", "password": "$PW"})
assert s == 204 or s == 200, "cannot sign in to the admin: set ASKBAR_ADMIN_PASSWORD to its password (repeated wrong tries lock sign-in for 15 minutes)"
EOF
}

echo "== 1. The admin works with the server stopped"
docker stop "$SERVER" >/dev/null
out=$(admin_script "$(admin_login)
s, body = call('GET', '/api/admin/v1/status')
print(s, 'offline' if '\"server_online\":false' in body.replace(' ', '') else 'online?')
s, _ = call('GET', '/admin/providers/')
print(s)" 2>&1)
if printf '%s' "$out" | head -1 | grep -q '^200 offline'; then pass "admin signs in and reports the server offline"; else fail "admin with the server stopped: $out"; fi
printf '%s' "$out" | tail -1 | grep -q '^200$' && pass "admin page loads" || fail "admin page loads"
docker start "$SERVER" >/dev/null
for i in $(seq 1 30); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$SERVER")" = healthy ] && break; sleep 2; done
[ "$(docker inspect -f '{{.State.Health.Status}}' "$SERVER")" = healthy ] && pass "server healthy again" || fail "server did not come back"

echo "== 2. The server port"
# No address is fixed in compose.yaml: the server listens on whatever server-api address Docker gave it,
# found by the alias server-api-host, which exists on that network alone. Read it after the restart above.
SERVER_API_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$SERVER_API_NET\").IPAddress}}" "$SERVER")
SERVER_API="$SERVER_API_IP:8082"
[ -n "$SERVER_API_IP" ] && pass "the server has an address on server-api ($SERVER_API_IP)" || fail "the server has no address on server-api"
[ "$(http_status "$SERVER_API_NET" server-api-host:8082)" = 401 ] && pass "the admin's address for it, server-api-host, reaches the server API" || fail "server-api-host:8082 does not reach the server API"
refused "compose.yaml pins no subnet or address (docs/guide.md, Docker networks)" grep -En '^[[:space:]]*(ipv[46]_address|subnet|ip_range):' "$(dirname "$0")/../compose.yaml"
HASH=$(docker exec "$ADMIN" cat /server-token/token.sha256 2>/dev/null | tr -d '[:space:]')
[ -n "$HASH" ] || fail "could not read the token hash from the admin's /server-token"
[ "$(http_status "$SERVER_API_NET" "$SERVER_API")" = 401 ] && pass "401 without a token" || fail "no token should give 401"
[ "$(http_status "$SERVER_API_NET" "$SERVER_API" "$HASH")" = 401 ] && pass "401 with the token's hash instead of the token" || fail "the hash must not work as the token"
[ "$(http_status "$SERVER_API_NET" "$SERVER_API" wrong)" = 401 ] && pass "401 with a wrong token" || fail "a wrong token should give 401"

# A second server on its own scratch volumes, with no hash file at all: nothing may be accepted.
docker rm -f askbar-check-nohash >/dev/null 2>&1
docker run -d --name askbar-check-nohash --network "$SERVER_API_NET" --network-alias askbar-check-nohash-host \
  -e ASKBAR_SERVER_API_ADDR=askbar-check-nohash-host:8082 -e ASKBAR_CLIENT_API_ADDR=127.0.0.1:8080 \
  -e ASKBAR_SERVER_TOKEN_HASH_FILE=/nowhere/token.sha256 "$(docker inspect -f '{{.Config.Image}}' "$SERVER")" >/dev/null
sleep 3
code=$(http_status "$SERVER_API_NET" askbar-check-nohash-host:8082 "$HASH")
case "$code" in 401|403|503) pass "fails closed with no hash file ($code)";; *) fail "no hash file: got $code, expected a refusal";; esac
docker rm -f askbar-check-nohash >/dev/null 2>&1

docker exec "$CADDY" sh -c "wget -q -T3 -O /dev/null http://$SERVER_API/api/server/v1/health" >/dev/null 2>&1 \
  && fail "Caddy reached the server port" || pass "Caddy cannot reach the server port"
EDGE_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$EDGE\").IPAddress}}" "$SERVER")
[ "$(http_status "$EDGE" "$EDGE_IP:8082")" = 0 ] && pass "nothing on the edge network answers on 8082" || fail "the server port answers on the edge network"
[ "$(http_status "$EDGE" "$SERVER_API")" = 0 ] && pass "a container on edge cannot reach the server API address" || fail "the server API address is reachable from edge"
admin_nets=$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$ADMIN" | tr ' ' '\n' | grep . | sort | tr '\n' ' ')
[ "$admin_nets" = "${PROJECT}_cpa-admin ${PROJECT}_server-api " ] && pass "the admin is on server-api and cpa-admin only (both internal: no route out, never edge)" || fail "the admin's networks are: $admin_nets"
for n in server-api cpa-admin; do
  [ "$(docker network inspect -f '{{.Internal}}' "${PROJECT}_$n" 2>/dev/null)" = true ] && pass "the $n network is internal" || fail "the $n network is not internal"
done

echo "== 3. What the server can see"
VOLS=$(docker inspect -f '{{range .Mounts}}{{.Name}} {{end}}' "$SERVER")
for v in admin-secrets admin-data admin-run caddy-run caddy-secrets control-token cpa-config; do
  case " $VOLS" in *" ${PROJECT}_$v "*) fail "the server mounts $v";; *) pass "the server does not mount $v";; esac
done
refused "the server cannot list /admin-secrets" docker exec "$SERVER" ls /admin-secrets
refused "the server cannot reach an admin socket" docker exec "$SERVER" test -e /run/admin/admin.sock
refused "the server cannot write to /server-token (read-only)" docker exec "$SERVER" sh -c 'touch /server-token/x'
[ "$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/server-token"}}{{.RW}}{{end}}{{end}}' "$SERVER")" = false ] && pass "/server-token is mounted read-only in the server" || fail "/server-token is writable in the server"
[ "$(docker exec "$SERVER" id -u)" != "$(docker exec "$ADMIN" id -u)" ] && pass "the server and the admin run as different users" || fail "the server and the admin share a user"
# Even as the admin's own uid, the server container has nothing of the admin's to open.
refused "the admin's database is not in the server" docker exec -u "$(docker exec "$ADMIN" id -u)" "$SERVER" sh -c 'ls /admin-data/admin.db'
refused "the admin cannot read the server's master key" docker exec "$ADMIN" sh -c 'cat /secrets/master.key'

echo "== 3b. The audit store"
# audit.db is the server's alone: its own volume, mounted nowhere else.
case " $VOLS" in *" ${PROJECT}_audit-data "*) pass "the server mounts audit-data";; *) fail "the server does not mount audit-data";; esac
case " $(docker inspect -f '{{range .Mounts}}{{.Name}} {{end}}' "$ADMIN")" in
  *" ${PROJECT}_audit-data "*) fail "the admin mounts audit-data";; *) pass "the admin does not mount audit-data";;
esac
refused "the admin container has no /audit" docker exec "$ADMIN" test -e /audit
refused "no audit.db anywhere in the admin container" docker exec "$ADMIN" sh -c 'find / -xdev -name "audit.db*" 2>/dev/null | grep -q .'
# The server's data (ADR 0009): control.db and one file per space, mounted by the server only.
refused "the admin container has no server data (/data)" docker exec "$ADMIN" test -e /data
refused "no control.db or space file anywhere in the admin container" docker exec "$ADMIN" sh -c 'find / -xdev \( -name "control.db*" -o -path "*/spaces/*.db" \) 2>/dev/null | grep -q .'
check "audit.db exists in the server" docker exec "$SERVER" test -s /audit/audit.db
# The client API (port 8080, reachable from the edge network) has no audit and no server routes.
http_get() { # network host:port path -> status; 0 = no connection
  py --network "$1" -- "
import urllib.request, urllib.error
try: print(urllib.request.urlopen('http://$2$3', timeout=4).status)
except urllib.error.HTTPError as e: print(e.code)
except Exception: print(0)
"
}
for path in /audit /api/client/v1/audit /api/client/v1/audit/traces /api/client/v1/audit/stats /api/server/v1/health /api/server/v1/audit/traces /api/server/v1/audit/stats; do
  [ "$(http_get "$EDGE" "$EDGE_IP:8080" "$path")" = 404 ] && pass "the client API has no $path (404)" || fail "the client API answers $path"
done

echo "== 3c. The server holds no secret for the audit key"
# The server may seal (it has the public key) but must not be able to derive or open: no Argon2
# and no admin code in its dependency tree.
if command -v cargo >/dev/null 2>&1 && [ -f Cargo.toml ]; then
  TREE=$(cargo tree -p askbar-server --prefix none 2>/dev/null)
  if [ -z "$TREE" ]; then fail "cargo tree -p askbar-server printed nothing"; else
    printf '%s\n' "$TREE" | grep -qi '^argon2 ' && fail "askbar-server depends on argon2" || pass "askbar-server does not depend on argon2"
    printf '%s\n' "$TREE" | grep -q '^askbar-admin ' && fail "askbar-server depends on askbar-admin" || pass "askbar-server does not depend on any admin crate"
  fi
else
  fail "cargo (and the workspace) is needed to check the server's dependencies"
fi
# After opening sealed content, the admin's logs hold no passphrase. A made-up one is used for a
# failed open, which is the case that would be easiest to log by mistake.
LOGPASS="isolation-check-passphrase-$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
admin_script "$(admin_login)
call('POST', '/api/admin/v1/audit/payloads/no-such-span/open', {'passphrase': '$LOGPASS'})
call('POST', '/api/admin/v1/audit/rotate-key', {'current': '$LOGPASS', 'new': '$LOGPASS-new', 'confirm': '$LOGPASS-new'})" >/dev/null 2>&1
docker logs "$ADMIN" 2>&1 | grep -q "$LOGPASS" && fail "the admin's logs contain a passphrase" || pass "the admin's logs contain no passphrase after open and rotate attempts"
docker logs "$SERVER" 2>&1 | grep -q "$LOGPASS" && fail "the server's logs contain a passphrase" || pass "the server's logs contain no passphrase"

echo "== 3d. CLIProxyAPI"
# It starts with the stack, on the edge network only, and publishes nothing.
CPA=$(docker compose -p "$PROJECT" ps -q cpa 2>/dev/null | head -1)
if [ -z "$CPA" ]; then
  echo "SKIP  cpa is not running"
else
  cpa_nets=$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$CPA" | tr ' ' '\n' | grep . | sort | tr '\n' ' ')
  [ "$cpa_nets" = "${PROJECT}_cpa-admin ${EDGE} " ] && pass "cpa is on edge and cpa-admin only" || fail "cpa's networks are: $cpa_nets"
  [ -z "$(docker inspect -f '{{range $p, $b := .NetworkSettings.Ports}}{{if $b}}{{$p}} {{end}}{{end}}' "$CPA")" ] && pass "cpa publishes no port" || fail "cpa publishes a port"
fi

echo "== 3d2. Container hardening (ADR 0032)"
# Every running service: read-only root, no capabilities but Caddy's NET_BIND_SERVICE, no privilege gain,
# process and memory limits, a private /tmp, and no host-gateway alias.
for svc in server admin caddy cpa mock-llm; do
  c=$(docker compose -p "$PROJECT" ps -q "$svc" 2>/dev/null | head -1)
  [ -n "$c" ] || { echo "SKIP  $svc is not running"; continue; }
  want_add=""; [ "$svc" = caddy ] && want_add="NET_BIND_SERVICE"
  got=$(docker inspect -f '{{.HostConfig.ReadonlyRootfs}}|{{join .HostConfig.CapDrop ","}}|{{join .HostConfig.CapAdd ","}}|{{join .HostConfig.SecurityOpt ","}}|{{if .HostConfig.PidsLimit}}{{.HostConfig.PidsLimit}}{{end}}|{{.HostConfig.Memory}}|{{range $k, $v := .HostConfig.Tmpfs}}{{$k}}{{end}}|{{join .HostConfig.ExtraHosts ","}}' "$c")
  IFS='|' read -r ro drop add sec pids mem tmp hosts <<EOF_H
$got
EOF_H
  bad=""
  [ "$ro" = true ] || bad="$bad read_only=$ro"
  [ "$drop" = ALL ] || bad="$bad cap_drop=$drop"
  [ "${add#CAP_}" = "$want_add" ] || bad="$bad cap_add=$add"
  case ",$sec," in *",no-new-privileges:true,"*|*",no-new-privileges,"*) ;; *) bad="$bad security_opt=$sec";; esac
  [ -n "$pids" ] && [ "$pids" -gt 0 ] || bad="$bad pids_limit=${pids:-none}"
  [ "${mem:-0}" -gt 0 ] || bad="$bad mem_limit=none"
  [ "$tmp" = /tmp ] || bad="$bad tmpfs=$tmp"
  case "$hosts" in *host-gateway*|*host.docker.internal*) bad="$bad extra_hosts=$hosts";; esac
  [ -z "$bad" ] && pass "$svc is hardened" || fail "$svc is not hardened:$bad"
done
# Each hardened service is writing only to its volumes: its root filesystem refuses a write.
for c in $SERVER $ADMIN $CADDY; do
  refused "$c cannot write its root filesystem" docker exec "$c" sh -c 'echo x > /hardening-probe'
done

echo "== 3e. No Docker socket"
# There is no controller (ADR 0005): no container of the project mounts the Docker socket, and none is left
# of the controller or its network.
for c in $(docker compose -p "$PROJECT" ps -aq 2>/dev/null); do
  name=$(docker inspect -f '{{.Name}}' "$c" | tr -d /)
  docker inspect -f '{{range .Mounts}}{{.Source}} {{.Destination}} {{end}}' "$c" | grep -q 'docker.sock' && fail "$name mounts the Docker socket" || pass "$name does not mount the Docker socket"
done
docker container inspect askbar-controller >/dev/null 2>&1 && fail "a controller container still exists" || pass "there is no controller container"
docker network inspect "${PROJECT}_control" >/dev/null 2>&1 && fail "the control network still exists" || pass "there is no control network"
out=$(admin_script "$(admin_login)
print(call('POST', '/api/admin/v1/services/controller/restart')[0], call('POST', '/api/admin/v1/services/server/stop')[0], call('POST', '/api/admin/v1/services/server/kill')[0])" 2>&1 | tail -1)
[ "$out" = "404 404 404" ] && pass "only the four services can be restarted, and nothing else can be done to them" || fail "service actions accepted a bad name or action: $out"

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
s, body = call('PUT', '/api/admin/v1/network', {'tls_enabled': True, 'hostname': 'askbar.isolation.invalid', 'admin_cidr': '10.0.0.0/8', 'acme_staging': True, 'allowlist_enabled': $1, 'cloudflare_token': 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN'})
assert s == 200, (s, body)
s, body = call('POST', '/api/admin/v1/network/apply')
print(s, 'ok' if '\"ok\":true' in body.replace(' ', '') else body)"
}
out=$(set_network True 2>&1); printf '%s' "$out" | tail -1 | grep -q '^200 ok' && pass "allowlist on: Caddy took the configuration" || fail "allowlist on: $out"
cfg=$(caddy_config)
printf '%s' "$cfg" | grep -q '"abort":true' && printf '%s' "$cfg" | grep -q '"/api/client/\*","/api/v1/\*"' && ! printf '%s' "$cfg" | grep -q '/api/server' && pass "the client API route names /api/client and /api/v1 only, never /api/server" || fail "the client API route is wrong"
printf '%s' "$cfg" | grep -q '"status_code":"\?404"\?' && pass "Caddy answers 404 for every other path" || fail "Caddy has no final 404"
# The admin site and the admin API share one route, so the address check guards both (ADR 0022, 0029).
printf '%s' "$cfg" | grep -q '"/admin","/admin/\*","/api/admin/\*"' && pass "the admin route covers /admin and /api/admin, behind the same address check" || fail "the admin route does not cover /admin and /api/admin together"
printf '%s' "$cfg" | grep -q 'admin.askbar.isolation.invalid' && fail "Caddy still has a second host name for the admin" || pass "Caddy has one host name"
out=$(set_network False 2>&1); printf '%s' "$out" | tail -1 | grep -q '^200 ok' && pass "allowlist off: Caddy took the configuration" || fail "allowlist off: $out"
cfg=$(caddy_config)
if printf '%s' "$cfg" | grep -q 'unix//run/admin/admin.sock' && ! printf '%s' "$cfg" | grep -q '"abort":true\|remote_ip'; then
  pass "allowlist off: the admin site has no address check, any client reaches the login"
else
  fail "allowlist off: the admin site still has an address rule"
fi
# Caddy's own admin API: unix socket only.
refused "Caddy's admin API refuses TCP" docker exec "$CADDY" sh -c 'wget -q -T3 -O /dev/null http://127.0.0.1:2019/config/'
check "uid 10001 reaches Caddy's admin API through the socket" py -u 10001:10001 -v "${PROJECT}_caddy-run:/r" -- "
import socket
s = socket.socket(socket.AF_UNIX); s.connect('/r/admin.sock')"

echo "== 5. Images, files and backups"
SERVER_IMG=$(docker inspect -f '{{.Config.Image}}' "$SERVER")
# The server binary carries nothing of the admin: no admin site, no Caddy or Cloudflare code.
for needle in "Askbar admin" "app.js" "style.css" "x-askbar-admin" "Caddyfile" "cloudflare" "admin_password_hash"; do
  docker run --rm --entrypoint grep "$SERVER_IMG" -aq -- "$needle" /usr/local/bin/askbar-server \
    && fail "the server binary contains \"$needle\"" || pass "the server binary has no \"$needle\""
done
docker run --rm --entrypoint sh "$SERVER_IMG" -c 'test -e /usr/local/bin/askbar-admin' \
  && fail "the server image holds the admin binary" || pass "the server image holds no admin binary"
case " $(docker inspect -f '{{range .Mounts}}{{.Name}} {{end}}' "$ADMIN")" in
  *" ${PROJECT}_askbar-secrets "*) fail "the admin mounts askbar-secrets";; *) pass "the admin does not mount askbar-secrets";;
esac
refused "the admin has no /secrets" docker exec "$ADMIN" test -e /secrets
modes_ok=1
for f in "$ADMIN:/admin-secrets/admin.key" "$ADMIN:/admin-secrets/server.token" "$SERVER:/secrets/master.key"; do
  m=$(docker exec "${f%%:*}" stat -c %a "${f#*:}" 2>/dev/null)
  [ "$m" = 600 ] || { fail "${f#*:} has mode ${m:-missing}, expected 600"; modes_ok=0; }
done
# Caddy reads these two through group 10001 (no CAP_DAC_OVERRIDE, ADR 0032), so they are 0640, not 0600.
for f in self.key cf_token; do
  docker exec "$ADMIN" test -e "/caddy-secrets/$f" || continue
  m=$(docker exec "$ADMIN" stat -c '%a %g' "/caddy-secrets/$f")
  [ "$m" = "640 10001" ] || { fail "$f has mode and group '$m', expected '640 10001'"; modes_ok=0; }
done
[ "$modes_ok" = 1 ] && pass "token files and the master key are mode 600, Caddy's key and token 640"

MARK="sk-isolation-$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
out=$(admin_script "$(admin_login)
import tarfile, io, base64, json
s, body = call('POST', '/api/admin/v1/providers', {'name': 'iso-$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')', 'type': 'openai', 'base_url': 'http://mock-llm:8765/v1', 'model': 'm', 'api_key': '$MARK'})
assert s == 201, (s, body)
provider_id = json.loads(body)['id']
s, users = call('GET', '/api/admin/v1/users'); users = json.loads(users)
if not users:
    s, made = call('POST', '/api/admin/v1/users', {'name': 'isolation user'}); users = [json.loads(made)]
s, body = call('POST', '/api/admin/v1/users/' + users[0]['id'] + '/pairing-keys', {'name': 'iso client'})
assert s == 200, (s, body)
pairing = json.loads(body)['pairing']
b = pairing.split(':', 1)[1]
code = json.loads(base64.urlsafe_b64decode(b + '=' * (-len(b) % 4)))['code']
c.request('GET', '/api/admin/v1/backup', headers={'x-askbar-admin': '1', 'cookie': cookie})
r = c.getresponse(); raw = r.read()
assert r.status == 200, r.status
names = sorted(m.name for m in tarfile.open(fileobj=io.BytesIO(raw)).getmembers())
blob = b''.join(tarfile.open(fileobj=io.BytesIO(raw)).extractfile(n).read() for n in names)
print('names', ','.join(names))
for label, needle in (('provider key', '$MARK'), ('pair code', code), ('password hash', '\$argon2')):
    print(label, 'FOUND' if needle.encode() in blob else 'absent')
call('DELETE', '/api/admin/v1/providers/' + provider_id)" 2>&1)
printf '%s\n' "$out" | grep -q '^names admin.db,server/control.db,server/spaces/[0-9A-Z]\{26\}\.db' && pass "the backup holds admin.db, the server's control.db and its space files" || fail "backup contents: $out"
check "the server keeps control.db and a file per space under /data" docker exec "$SERVER" sh -c 'test -f /data/control.db && ls /data/spaces/*.db'
refused "the server makes no askbar.db (the layout before spaces)" docker exec "$SERVER" test -e /data/askbar.db
for what in "provider key" "pair code" "password hash"; do
  printf '%s\n' "$out" | grep -q "^$what absent" && pass "the backup holds no $what" || fail "the backup holds the $what ($out)"
done

echo "== 6. HTTPS before a host name is set"
# Back to a fresh install's settings: no host name, allowlist off. Caddy then serves every address on 443 with the
# install's own certificate, and the admin site is open to private addresses only.
out=$(admin_script "$(admin_login)
s, body = call('PUT', '/api/admin/v1/network', {'tls_enabled': False, 'hostname': '', 'admin_cidr': '', 'acme_staging': False, 'allowlist_enabled': False, 'clear_token': True})
assert s == 200, (s, body)
s, body = call('POST', '/api/admin/v1/network/apply')
print(s, 'ok' if '\"ok\":true' in body.replace(' ', '') else body)" 2>&1)
printf '%s' "$out" | tail -1 | grep -q '^200 ok' && pass "no host name: Caddy took the configuration" || fail "no host name: $out"
ports=$(docker inspect -f '{{range $p, $b := .NetworkSettings.Ports}}{{if $b}}{{$p}} {{end}}{{end}}' "$CADDY" | tr ' ' '\n' | grep . | sort | tr '\n' ' ')
[ "$ports" = "443/tcp 80/tcp " ] && pass "Caddy publishes 443 and 80 only" || fail "Caddy publishes: $ports"
cfg=$(caddy_config)
printf '%s' "$cfg" | grep -q '"abort":true' && printf '%s' "$cfg" | grep -q '192.168.0.0/16' && pass "the admin site is limited to private addresses" || fail "the admin site has no private-address rule"
# /admin/ goes to the first admin page (plan step 1.16), through Caddy.
code=$(curl -skL -o /dev/null -w '%{http_code} %{url_effective}' --max-time 5 https://localhost/admin/ 2>/dev/null || true)
[ "$code" = "200 https://localhost/admin/providers/" ] && pass "https://localhost/admin/ opens the admin's first page" || fail "https://localhost/admin/ answered '$code'"
code=$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 https://localhost/api/admin/v1/health 2>/dev/null || true)
[ "$code" = 200 ] && pass "the admin API answers at /api/admin/v1 over HTTPS" || fail "https://localhost/api/admin/v1/health answered '$code'"
code=$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 https://localhost/admin/api/v1/health 2>/dev/null || true)
[ "$code" = 404 ] && pass "nothing at the old /admin/api/v1 (404)" || fail "https://localhost/admin/api/v1/health answered '$code'"
# Signing in over HTTPS through Caddy sets the cookie ADR 0029 asks for; a change from another site is refused.
cookie=$(curl -sk -D - -o /dev/null --max-time 5 -X POST -H 'x-askbar-admin: 1' -H 'content-type: application/json' \
  -d "{\"password\":\"$PW\"}" https://localhost/api/admin/v1/session 2>/dev/null | tr -d '\r' | grep -i '^set-cookie:' | head -1)
case "$cookie" in
  *"__Secure-askbar_admin="*"; Secure"*"; HttpOnly"*"; SameSite=Strict"*"; Path=/api/admin;"*) pass "the session cookie is __Secure-, Secure, HttpOnly, SameSite=Strict, Path=/api/admin";;
  *) fail "the session cookie is: ${cookie:-none}";;
esac
pair=$(printf '%s' "$cookie" | sed 's/^[Ss]et-[Cc]ookie: *//; s/;.*//')
code=$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 -X DELETE -H 'x-askbar-admin: 1' -H 'sec-fetch-site: cross-site' -H "cookie: $pair" https://localhost/api/admin/v1/session 2>/dev/null || true)
[ "$code" = 403 ] && pass "a change marked cross-site is refused (403)" || fail "a cross-site change answered '$code'"
code=$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 https://localhost/api/client/v1/health 2>/dev/null || true)
[ "$code" = 200 ] && pass "the client API answers over HTTPS" || fail "https://localhost/api/client/v1/health answered '$code'"
curl -sk -D - -o /dev/null --max-time 5 https://localhost/api/client/v1/health 2>/dev/null | tr -d '\r' | grep -qi '^x-content-type-options: nosniff$' \
  && pass "the client API says nosniff" || fail "the client API has no X-Content-Type-Options: nosniff"
for path in /api/server/v1/health /health /; do
  code=$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 "https://localhost$path" 2>/dev/null || true)
  [ "$code" = 404 ] && pass "nothing at $path (404)" || fail "$path answered '$code'"
done
redirect=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' --max-time 5 http://localhost/admin/ 2>/dev/null || true)
case "$redirect" in 30[178]" https://localhost/admin/") pass "port 80 redirects to HTTPS";; *) fail "http://localhost/admin/ answered '$redirect'";; esac
# The certificate Caddy hands out, with no name (an address) and with a name it has no certificate for, is the one
# the admin made in caddy-secrets.
out=$(py --network "$EDGE" -u 10001:10001 -v "${PROJECT}_caddy-secrets:/s:ro" -- "
import base64, hashlib, socket, ssl
pem = open('/s/self.crt').read()
want = hashlib.sha256(base64.b64decode(''.join(pem.split('-----')[2].split()))).hexdigest()
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
for name in (None, 'some-other-name.example'):
    with socket.create_connection(('caddy', 443), timeout=5) as raw:
        with ctx.wrap_socket(raw, server_hostname=name) as t:
            got = hashlib.sha256(t.getpeercert(binary_form=True)).hexdigest()
    print(name, 'same' if got == want else 'DIFFERENT')
print('file', want)
" 2>&1)
printf '%s\n' "$out" | grep -q '^None same' && pass "a client connecting by address gets the install's certificate" || fail "by address: $out"
printf '%s\n' "$out" | grep -q '^some-other-name.example same' && pass "a client using any other name gets the install's certificate" || fail "by another name: $out"
# A pairing key made now names that certificate, so the app trusts exactly it.
file_fp=$(printf '%s\n' "$out" | sed -n 's/^file //p')
pin=$(admin_script "$(admin_login)
import base64
s, users = call('GET', '/api/admin/v1/users'); users = json.loads(users)
if not users:
    s, made = call('POST', '/api/admin/v1/users', {'name': 'isolation user'}); users = [json.loads(made)]
s, body = call('POST', '/api/admin/v1/users/' + users[0]['id'] + '/pairing-keys', {'name': 'isolation check'})
p = json.loads(body)['pairing'][len('askbar-pair:'):]
print(json.loads(base64.urlsafe_b64decode(p + '=' * (-len(p) % 4))).get('cert_sha256'))" 2>&1 | tail -1)
[ -n "$file_fp" ] && [ "$pin" = "$file_fp" ] && pass "a new pairing key pins the install's certificate" || fail "pairing pin '$pin', certificate '$file_fp'"

echo "== 7. Restarts without a controller (ADR 0005)"
# Each restart is asked from the admin site and comes back on its own; the server within a minute
# (ADR 0002). A service refuses a restart in its first 30 seconds, so a 429 is waited out once.
restart() { # service -> prints "status seconds"
  admin_script "$(admin_login)
import time
t = time.time()
s, body = call('POST', '/api/admin/v1/services/$1/restart')
if s == 429:
    time.sleep(31); s, body = call('POST', '/api/admin/v1/services/$1/restart')
print(s, round(time.time() - t, 1))" 2>&1 | tail -1
}
before=$(docker inspect -f '{{.State.StartedAt}}' "$SERVER")
out=$(restart server)
after=$(docker inspect -f '{{.State.StartedAt}}' "$SERVER")
case "$out" in
  "200 "*) secs=${out#200 }; [ "${secs%.*}" -lt 60 ] && [ "$before" != "$after" ] && pass "the server restarts itself and is back in ${secs}s" || fail "server restart: $out, started $before then $after";;
  *) fail "server restart answered: $out";;
esac
out=$(restart caddy); case "$out" in "200 "*) pass "Caddy reloads its configuration";; *) fail "Caddy restart answered: $out";; esac
# CLIProxyAPI runs under our supervisor (ADR 0037): it is the container's main process, and a restart from
# the site starts a new CLIProxyAPI process inside the same container.
cpa_pid() { docker top askbar-cpa -o pid,args 2>/dev/null | awk '/CLIProxyAPI -config/ && !/askbar-cpa-supervisor/ {print $1; exit}'; }
[ "$(docker inspect -f '{{index .Config.Entrypoint 0}}' askbar-cpa 2>/dev/null)" = askbar-cpa-supervisor ] && docker top askbar-cpa -o pid,args | grep -q 'askbar-cpa-supervisor ./CLIProxyAPI' \
  && pass "CLIProxyAPI runs under the supervisor" || fail "the cpa container's main process is not the supervisor"
cpa_before=$(docker inspect -f '{{.State.StartedAt}}' askbar-cpa 2>/dev/null); pid_before=$(cpa_pid)
out=$(restart cpa)
case "$out" in
  "200 "*) [ "$cpa_before" = "$(docker inspect -f '{{.State.StartedAt}}' askbar-cpa)" ] && [ -n "$pid_before" ] && [ "$pid_before" != "$(cpa_pid)" ] \
    && pass "CLIProxyAPI restarts through its supervisor, in the same container (${out#200 }s)" || fail "cpa restart: container or process not as expected";;
  *) fail "CLIProxyAPI restart answered: $out";;
esac
admin_before=$(docker inspect -f '{{.State.StartedAt}}' "$ADMIN")
out=$(restart admin)
back=no
for i in $(seq 60); do
  [ "$(docker inspect -f '{{.State.StartedAt}}' "$ADMIN")" != "$admin_before" ] && docker exec "$ADMIN" askbar-admin healthcheck >/dev/null 2>&1 && { back=yes; break; }
  sleep 1
done
case "$out" in "202 "*) [ "$back" = yes ] && pass "the admin restarts itself and answers again" || fail "the admin did not come back";; *) fail "admin restart answered: $out";; esac

echo
if [ "$FAILS" -eq 0 ]; then echo "All isolation checks passed."; else echo "$FAILS isolation check(s) FAILED."; exit 1; fi
