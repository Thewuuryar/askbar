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
# Meant for a throwaway stack: it stops and restarts the server (and, in the failure drills at the end, every service in turn), and it sets the admin's
# network settings (a made-up hostname, no certificate is requested), then puts them back to a fresh
# install's (no host name, allowlist off, the Cloudflare token cleared). Needs docker and the
# python:3.12-slim image. Set ASKBAR_ADMIN_PASSWORD if the admin already has a password;
# otherwise the first-run password is set to a random one.
#
# One check is slow and is skipped unless ASKBAR_SLOW_CHECKS=1: a gateway that stays stopped for more than five minutes must raise
# component_silent (section 3d7), which takes about five minutes of waiting. Run it before changing the auditor's alert rules, the
# alerter or the gateway's link; CI runs it on a push to main and on a manual run, not on a pull request.
set -u

PROJECT="${COMPOSE_PROJECT_NAME:-askbar}"
EDGE="${PROJECT}_edge"; CLIENT_NET="${PROJECT}_client-api"; SERVER_API_NET="${PROJECT}_server-api"
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

# Shared helpers: the network pipeline the checks below use, and the 30 x 2s health wait.
nets_of() { docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$1" | tr ' ' '\n' | grep . | sort | tr '\n' ' '; }
wait_healthy() { local i; for i in $(seq 1 30); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$1")" = healthy ] && break; sleep 2; done; }
# Owners and modes: a token is its caller's and 0400, a hash its receiver's and 0444, directories 0500.
owned() { # container path want("uid mode") description
  local got; got=$(docker exec "$1" stat -c '%u %a' "$2" 2>/dev/null)
  [ "$got" = "$3" ] && pass "$4 is '$3'" || fail "$4 is '${got:-missing}', expected '$3'"
}
pfx() { local i; for i in $1; do printf '%s_%s ' "$PROJECT" "$i"; done; } # short names -> "askbar_a askbar_b "

# One service's structural properties, read from its profile on stdin: svc_profile CONTAINER NAME <<EOF ... EOF
# One "key value" line per property; a key left out is a check the service does not have. Messages use NAME.
# Only what runs is checked here (the networks it really joined, CapEff, owners); what compose.yaml declares is section 0.
#   health yes                       wait for healthy, then check it
#   uid N                            runs as uid N
#   caps yes                         CapEff is 0
#   nets a b c|PASS                  exactly these networks (short names, sorted); PASS is the pass message
#   owner CTR|PATH|UID MODE|TEXT     file owner and mode;  dirs CTR|UID MODE|TEXT|PATHS: the same for each directory
svc_profile() {
  local c="$1" n="$2" k v health="" uid="" caps="" nets="" orows=() a b d e x i got have
  while read -r k v; do
    case "$k" in
      health|uid|caps|nets) printf -v "$k" %s "$v";;
      owner) orows+=("$v");;
      dirs) IFS='|' read -r a b d e <<<"$v"; for i in $e; do orows+=("$a|$i|$b|$d $i"); done;;
    esac
  done
  if [ -n "$health" ]; then
    wait_healthy "$c"
    [ "$(docker inspect -f '{{.State.Health.Status}}' "$c")" = healthy ] && pass "$n is healthy" || fail "$n is not healthy"
  fi
  [ -z "$uid" ] || { [ "$(docker exec "$c" id -u)" = "$uid" ] && pass "$n runs as uid $uid" || fail "$n does not run as uid $uid"; }
  if [ -n "$caps" ]; then
    got=$(docker exec "$c" sh -c 'grep CapEff /proc/1/status' 2>/dev/null | awk '{print $2}')
    [ "$got" = 0000000000000000 ] && pass "$n's CapEff is 0" || fail "$n's CapEff is '${got:-unknown}', expected 0"
  fi
  if [ -n "$nets" ]; then
    IFS='|' read -r a b <<<"$nets"; have=$(nets_of "$c")
    [ "$have" = "$(pfx "$a")" ] && pass "$b" || fail "$n's networks are: $have"
  fi
  for x in ${orows[@]+"${orows[@]}"}; do IFS='|' read -r a b d e <<<"$x"; owned "$a" "$b" "$d" "$e"; done
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

# HTTP status from a throwaway container on NET: http NET URL [METHOD] [TOKEN] [JSON]; 0 = no connection.
# The token and body reach the Python through its environment, never its source.
http() {
  py --network "$1" -e "HTTP_URL=$2" -e "HTTP_METHOD=${3:-GET}" -e "HTTP_TOKEN=${4:-}" -e "HTTP_JSON=${5:-}" -- '
import os, urllib.request, urllib.error
h = {}
if os.environ["HTTP_TOKEN"]: h["authorization"] = "Bearer " + os.environ["HTTP_TOKEN"]
body = os.environ["HTTP_JSON"].encode() or None
if body: h["content-type"] = "application/json"
req = urllib.request.Request(os.environ["HTTP_URL"], method=os.environ["HTTP_METHOD"], data=body, headers=h)
try: print(urllib.request.urlopen(req, timeout=4).status)
except urllib.error.HTTPError as e: print(e.code)
except Exception: print(0)
'
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
# A provider the mock model answers for, an approved model, a person and a pairing key for them. Prints "TAG id pairing".
# The person and the client are named "<name> user" and "<name> client". A refused step stops the block (assert).
admin_setup_provider() { # TAG name api_key
  admin_script "$(admin_login)
import json, uuid
s, body = call('POST', '/api/admin/v1/providers', {'name': '$2-' + uuid.uuid4().hex[:6], 'type': 'openai', 'base_url': 'http://mock-llm:8765/v1', 'model': 'm', 'api_key': '$3'})
assert s == 201, (s, body)
pid = json.loads(body)['id']
s, cat = call('GET', '/api/admin/v1/catalog'); cat = json.loads(cat)
cat['models'].append({'provider_id': pid, 'model': 'm'}); cat['default'] = pid + '/m'
s, body = call('PUT', '/api/admin/v1/catalog', cat); assert s == 200, (s, body)
s, users = call('GET', '/api/admin/v1/users'); users = json.loads(users)
if not users:
    s, made = call('POST', '/api/admin/v1/users', {'name': '$2 user'}); users = [json.loads(made)]
s, body = call('POST', '/api/admin/v1/users/' + users[0]['id'] + '/pairing-keys', {'name': '$2 client'})
assert s == 200, (s, body)
print('$1', pid, json.loads(body)['pairing'])" 
}
# Puts the catalog back without the given provider, then removes the provider.
admin_cleanup_provider() { # provider_id
  admin_script "$(admin_login)
import json
s, cat = call('GET', '/api/admin/v1/catalog'); cat = json.loads(cat)
cat['models'] = [m for m in cat['models'] if m['provider_id'] != '$1']
if (cat.get('default') or '').startswith('$1/'):
    cat['default'] = (cat['models'][0]['provider_id'] + '/' + cat['models'][0]['model']) if cat['models'] else None
call('PUT', '/api/admin/v1/catalog', cat)
call('DELETE', '/api/admin/v1/providers/$1')"
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

echo "== 0. What compose.yaml declares: networks, mounts, ports and hardening (ADR 0032)"
# One read of `docker compose config`, checked against a table here. It is what was declared, not what runs: the sections
# below keep a live spot check for each service (the networks it joined, CapEff, the Docker socket, a write to its root).
DECLARED_PY=$(cat <<'PY'
import json, sys
c = json.load(sys.stdin)
S, N = c["services"], c["networks"]
def out(ok, msg): print(("ok|" if ok else "bad|") + msg)
def sp(s): return s.split()
# service: networks | named volumes (name:ro = read-only) | published ports
SPEC = {
  "bootstrap": ("", "server-token admin-token auditor-token model-gateway-token alerter-token egress-proxy-token", ""),
  "server": ("audit-server client-api gateway-server server-api", "server-token:ro", ""),
  "admin": ("alerter-admin audit-admin cpa-admin gateway-admin server-api", "cpa-config admin-data admin-secrets backup admin-token:ro admin-run caddy-run caddy-secrets", ""),
  "auditor": ("audit-admin audit-alerter audit-caddy audit-model-gateway audit-server", "auditor-data auditor-token:ro", ""),
  "model-gateway": ("audit-model-gateway cpa-gateway gateway-admin gateway-server outbound", "model-gateway-token:ro", ""),
  "alerter": ("alerter-admin alerter-outbound audit-alerter", "alerter-data alerter-token:ro", ""),
  "egress-proxy": ("egress-admin egress-alerter egress-cpa egress-model-gateway egress-outbound", "egress-proxy-token:ro", ""),
  "mock-llm": ("edge outbound", "", ""),
  "caddy": ("audit-caddy client-api edge", "caddy-data caddy-config caddy-run admin-run caddy-secrets:ro", "443/tcp 80/tcp"),
  "cpa": ("cpa-admin cpa-gateway outbound", "cpa-config cpa-auth", ""),
}
OPEN = {"edge", "outbound", "alerter-outbound", "egress-outbound"}  # the networks with a route out
ADD = {"bootstrap": "CHOWN", "caddy": "NET_BIND_SERVICE"}
SEPARATE = [("cpa", "caddy"), ("alerter", "server"), ("alerter", "model-gateway"), ("alerter", "caddy"), ("alerter", "cpa"), ("egress-proxy", "server"), ("egress-proxy", "auditor"), ("egress-proxy", "caddy")]
out(set(S) == set(SPEC), "the stack declares exactly the services bootstrap, server, admin, auditor, model-gateway, alerter, egress-proxy, mock-llm, caddy and cpa")
nets = {s: set(S[s].get("networks") or {}) for s in S}
for s, (n, v, p) in SPEC.items():
    d = S.get(s)
    if d is None: continue
    out(nets[s] == set(sp(n)), f"{s} is declared on {n or 'no network'} only")
    if s == "bootstrap": out(d.get("network_mode") == "none", "bootstrap has network_mode none")
    out(not any((e or {}).get(k) for e in (d.get("networks") or {}).values() for k in ("ipv4_address", "ipv6_address")), f"{s} pins no address")
    want = {x.split(":")[0]: x.endswith(":ro") for x in sp(v)}
    have = {m["source"]: bool(m.get("read_only")) for m in d.get("volumes") or [] if m.get("type") == "volume"}
    out(have == want, f"{s} mounts the volumes {v or 'none'} only, read-only where marked")
    out(not any("docker.sock" in str(m.get("source", "")) for m in d.get("volumes") or []), f"{s} does not mount the Docker socket")
    ports = {f'{q["published"]}/{q.get("protocol", "tcp")}' for q in d.get("ports") or []}
    out(ports == set(sp(p)), f"{s} publishes {p or 'no port'}")
    bad = []
    if d.get("read_only") is not True: bad.append("read_only")
    if d.get("cap_drop") != ["ALL"]: bad.append("cap_drop")
    if [x.removeprefix("CAP_") for x in d.get("cap_add") or []] != sp(ADD.get(s, "")): bad.append("cap_add")
    if not any(o in ("no-new-privileges", "no-new-privileges:true") for o in d.get("security_opt") or []): bad.append("security_opt")
    if int(d.get("pids_limit") or 0) <= 0: bad.append("pids_limit")
    if int(d.get("mem_limit") or 0) <= 0: bad.append("mem_limit")
    if d.get("tmpfs") != ["/tmp"]: bad.append("tmpfs")
    if "host-gateway" in str(d.get("extra_hosts")) or "host.docker.internal" in str(d.get("extra_hosts")): bad.append("extra_hosts")
    if d.get("privileged"): bad.append("privileged")
    out(not bad, f"{s} is hardened" if not bad else f"{s} is not hardened: {' '.join(bad)}")
for n in sorted(set().union(*nets.values()) | set(N)):
    out(n in N, f"network {n} is declared")
    if n in N:
        out(bool(N[n].get("internal")) == (n not in OPEN), f"network {n} is {'not ' if n in OPEN else ''}internal")
        out(not (N[n].get("ipam") or {}).get("config"), f"network {n} pins no subnet or range")
for a, b in SEPARATE:
    out(not nets[a] & nets[b], f"{a} shares no network with {b}")
PY
)
CFG=$(docker compose -p "$PROJECT" --profile dev config --format json 2>/dev/null)
res=$(printf '%s' "$CFG" | py -i -- "$DECLARED_PY" 2>&1) || res="bad|docker compose config could not be read or checked: $res"
[ -n "$res" ] || res="bad|the declared-config check printed nothing"
while IFS='|' read -r r m; do [ "$r" = ok ] && pass "$m" || fail "${m:-$r}"; done <<<"$res"

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
wait_healthy "$SERVER"
[ "$(docker inspect -f '{{.State.Health.Status}}' "$SERVER")" = healthy ] && pass "server healthy again" || fail "server did not come back"

echo "== 2. The server port"
# No address is fixed in compose.yaml: the server listens on whatever server-api address Docker gave it,
# found by the alias server-api-host, which exists on that network alone. Read it after the restart above.
SERVER_API_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$SERVER_API_NET\").IPAddress}}" "$SERVER")
SERVER_API="$SERVER_API_IP:8082"
[ -n "$SERVER_API_IP" ] && pass "the server has an address on server-api ($SERVER_API_IP)" || fail "the server has no address on server-api"
[ "$(http "$SERVER_API_NET" "http://server-api-host:8082/api/server/v1/health")" = 401 ] && pass "the admin's address for it, server-api-host, reaches the server API" || fail "server-api-host:8082 does not reach the server API"
# The hash is the server's (the admin mounts no server-token any more): bootstrap made it (ADR 0049).
HASH=$(docker exec "$SERVER" cat /server-token/accept/admin-server.sha256 2>/dev/null | tr -d '[:space:]')
[ -n "$HASH" ] || fail "could not read the token hash from the server's /server-token/accept"
[ "$(http "$SERVER_API_NET" "http://$SERVER_API/api/server/v1/health")" = 401 ] && pass "401 without a token" || fail "no token should give 401"
[ "$(http "$SERVER_API_NET" "http://$SERVER_API/api/server/v1/health" GET "$HASH")" = 401 ] && pass "401 with the token's hash instead of the token" || fail "the hash must not work as the token"
[ "$(http "$SERVER_API_NET" "http://$SERVER_API/api/server/v1/health" GET wrong)" = 401 ] && pass "401 with a wrong token" || fail "a wrong token should give 401"

# A second server on its own scratch volumes, with no hash file at all: nothing may be accepted.
docker rm -f askbar-check-nohash >/dev/null 2>&1
docker run -d --name askbar-check-nohash --network "$SERVER_API_NET" --network-alias askbar-check-nohash-host \
  -e ASKBAR_SERVER_API_ADDR=askbar-check-nohash-host:8082 -e ASKBAR_CLIENT_API_ADDR=127.0.0.1:8080 \
  -e ASKBAR_SERVER_TOKEN_HASH_FILE=/nowhere/admin-server.sha256 "$(docker inspect -f '{{.Config.Image}}' "$SERVER")" >/dev/null
sleep 3
# Since bootstrap (ADR 0049) the server does not even start without its hash file; a server that did start would
# have to refuse every call.
if [ "$(docker inspect -f '{{.State.Running}}' askbar-check-nohash)" = false ] && [ "$(docker inspect -f '{{.State.ExitCode}}' askbar-check-nohash)" != 0 ]; then
  docker logs askbar-check-nohash 2>&1 | grep -q 'docker compose up' && pass "refuses to start with no hash file, and says what to run" || fail "no hash file: the server stopped without saying what to run"
else
  code=$(http "$SERVER_API_NET" "http://askbar-check-nohash-host:8082/api/server/v1/health" GET "$HASH")
  case "$code" in 401|403|503) pass "fails closed with no hash file ($code)";; *) fail "no hash file: got $code, expected a refusal";; esac
fi
docker rm -f askbar-check-nohash >/dev/null 2>&1

docker exec "$CADDY" sh -c "wget -q -T3 -O /dev/null http://$SERVER_API/api/server/v1/health" >/dev/null 2>&1 \
  && fail "Caddy reached the server port" || pass "Caddy cannot reach the server port"
CLIENT_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$CLIENT_NET\").IPAddress}}" "$SERVER")
[ "$(http "$CLIENT_NET" "http://$CLIENT_IP:8082/api/server/v1/health")" = 0 ] && pass "nothing on the client-api network answers on 8082" || fail "the server port answers on the client-api network"
[ "$(http "$EDGE" "http://$SERVER_API/api/server/v1/health")" = 0 ] && pass "a container on edge cannot reach the server API address" || fail "the server API address is reachable from edge"
admin_nets=$(nets_of "$ADMIN")
[ "$admin_nets" = "${PROJECT}_alerter-admin ${PROJECT}_audit-admin ${PROJECT}_cpa-admin ${PROJECT}_gateway-admin ${PROJECT}_server-api " ] && pass "the admin is on server-api, cpa-admin, audit-admin, gateway-admin and alerter-admin only (all internal: no route out, never edge)" || fail "the admin's networks are: $admin_nets"

echo "== 3. What the server can see"
refused "the server cannot list /admin-secrets" docker exec "$SERVER" ls /admin-secrets
refused "the server cannot reach an admin socket" docker exec "$SERVER" test -e /run/admin/admin.sock
refused "the server cannot write to /server-token (read-only)" docker exec "$SERVER" sh -c 'touch /server-token/x'
[ "$(docker exec "$SERVER" id -u)" != "$(docker exec "$ADMIN" id -u)" ] && pass "the server and the admin run as different users" || fail "the server and the admin share a user"
# Even as the admin's own uid, the server container has nothing of the admin's to open.
refused "the admin's database is not in the server" docker exec -u "$(docker exec "$ADMIN" id -u)" "$SERVER" sh -c 'ls /admin-data/admin.db'
refused "the admin cannot read a master key in the server" docker exec "$ADMIN" sh -c 'cat /secrets/master.key'
# The server holds no provider key and no audit store (ADR 0054, ADR 0051). The three volumes of earlier versions that could have held
# them are no longer declared (1.0), so a fresh stack has none and nothing can mount one; an install that has them keeps them on disk (the
# guide says how to remove them).
for v in askbar-secrets audit-data control-token; do
  docker volume ls -q | grep -qx "${PROJECT}_$v" && fail "the stack has a $v volume (retired at 1.0)" || pass "the stack has no $v volume (retired at 1.0)"
done
refused "the server has no /secrets" docker exec "$SERVER" test -e /secrets
refused "the server has no master.key anywhere" docker exec "$SERVER" sh -c 'find / -xdev -name "master.key*" 2>/dev/null | grep -q .'
refused "the server has no audit.db anywhere" docker exec "$SERVER" sh -c 'find / -xdev -name "audit.db*" 2>/dev/null | grep -q .'

echo "== 3b. The audit store"
# audit.db is the auditor's alone (section 3d4); the server's old audit-data volume is gone from the stack (section 3).
refused "the admin container has no /audit" docker exec "$ADMIN" test -e /audit
refused "no audit.db anywhere in the admin container" docker exec "$ADMIN" sh -c 'find / -xdev -name "audit.db*" 2>/dev/null | grep -q .'
# The server's data (ADR 0009): control.db and one file per space, mounted by the server only.
refused "the admin container has no server data (/data)" docker exec "$ADMIN" test -e /data
refused "no control.db or space file anywhere in the admin container" docker exec "$ADMIN" sh -c 'find / -xdev \( -name "control.db*" -o -path "*/spaces/*.db" \) 2>/dev/null | grep -q .'
# The client API (port 8080, reachable from the edge network) has no audit and no server routes.
for path in /audit /api/client/v1/audit /api/client/v1/audit/traces /api/client/v1/audit/stats /api/server/v1/health /api/server/v1/audit/traces /api/server/v1/audit/stats; do
  [ "$(http "$CLIENT_NET" "http://$CLIENT_IP:8080$path")" = 404 ] && pass "the client API has no $path (404)" || fail "the client API answers $path"
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
# After a try at unsealing content, the admin's logs hold no passphrase. A made-up one is used for a
# failed unseal, which is the case that would be easiest to log by mistake.
LOGPASS="isolation-check-passphrase-$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
admin_script "$(admin_login)
call('POST', '/api/admin/v1/audit/session', {'passphrase': '$LOGPASS'})
call('POST', '/api/admin/v1/audit/rotate-key', {'current': '$LOGPASS', 'new': '$LOGPASS-new', 'confirm': '$LOGPASS-new'})" >/dev/null 2>&1
docker logs "$ADMIN" 2>&1 | grep -q "$LOGPASS" && fail "the admin's logs contain a passphrase" || pass "the admin's logs contain no passphrase after unseal and rotate attempts"
docker logs "$SERVER" 2>&1 | grep -q "$LOGPASS" && fail "the server's logs contain a passphrase" || pass "the server's logs contain no passphrase"

echo "== 3d. CLIProxyAPI"
# It starts with the stack, on its own internal networks plus the route out, and publishes nothing.
CPA=$(docker compose -p "$PROJECT" ps -q cpa 2>/dev/null | head -1)
if [ -z "$CPA" ]; then
  echo "SKIP  cpa is not running"
else
  svc_profile "$CPA" cpa <<EOF
nets cpa-admin cpa-gateway outbound|cpa is on cpa-admin, cpa-gateway and outbound only (never edge)
EOF
fi

echo "== 3d2. Container hardening (ADR 0032)"
# The declared hardening is section 0 (CapEff 0 is in svc_profile). Live: each hardened service's root filesystem refuses a write.
for c in $SERVER $ADMIN $CADDY askbar-auditor askbar-model-gateway askbar-alerter; do
  refused "$c cannot write its root filesystem" docker exec "$c" sh -c 'echo x > /hardening-probe'
done

echo "== 3d3. Bootstrap and the credential volumes (ADR 0049)"
BOOT=$(docker compose -p "$PROJECT" ps -aq bootstrap 2>/dev/null | head -1)
if [ -z "$BOOT" ]; then fail "there is no bootstrap container"; else
  [ "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$BOOT")" = "exited 0" ] && pass "bootstrap ran once and exited 0" || fail "bootstrap is $(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$BOOT")"
  [ "$(docker inspect -f '{{.Config.User}}' "$BOOT")" = 0 ] && pass "bootstrap runs as root, with CHOWN only (checked above)" || fail "bootstrap does not run as root"
fi
refused "the admin has no server-token" docker exec "$ADMIN" test -e /server-token
refused "the server has no admin-token" docker exec "$SERVER" test -e /admin-token
refused "the admin keeps no server.token in admin-secrets" docker exec "$ADMIN" test -e /admin-secrets/server.token
owned "$ADMIN" /admin-token/call/admin-server.token "10001 400" "the admin's token file (owner, mode)"
owned "$SERVER" /server-token/accept/admin-server.sha256 "10002 444" "the server's hash file (owner, mode)"
for d in /admin-token /admin-token/call /admin-token/accept; do owned "$ADMIN" "$d" "10001 500" "the admin's $d"; done
for d in /server-token /server-token/call /server-token/accept; do owned "$SERVER" "$d" "10002 500" "the server's $d"; done
admin_token=$(docker exec "$ADMIN" cat /admin-token/call/admin-server.token 2>/dev/null)
[ "$(printf '%s' "$admin_token" | sha256sum | cut -d' ' -f1)" = "$HASH" ] && pass "the server's hash is the hash of the admin's token" || fail "the server's hash does not match the admin's token"
case "$admin_token" in mgt_*) pass "the admin's token has the admin-server prefix";; *) fail "the admin's token has the wrong prefix";; esac

# A second run changes nothing, and a rotation makes a new token and a matching hash. Both run in the real
# container, where root has no capability but CHOWN: only there is it clear that bootstrap can still change a
# file it has already given away. After a rotation the admin and the server read the new files on restart.
before=$(docker exec "$ADMIN" sh -c 'cat /admin-token/call/admin-server.token' 2>/dev/null)
out=$(docker compose -p "$PROJECT" run --rm --no-deps bootstrap 2>/dev/null)
[ "$out" = "$(printf 'kept admin-server\nkept admin-auditor-append\nkept admin-auditor-read\nkept server-model-gateway\nkept admin-model-gateway\nkept model-gateway-auditor\nkept server-auditor\nkept model-gateway-auditor-read\nkept auditor-alerter\nkept admin-alerter\nkept alerter-auditor\nkept admin-auditor-restore\nkept admin-egress-proxy')" ] && [ "$(docker exec "$ADMIN" cat /admin-token/call/admin-server.token 2>/dev/null)" = "$before" ] \
  && pass "a second bootstrap run keeps the token" || fail "a second bootstrap run: $out"
out=$(docker compose -p "$PROJECT" run --rm --no-deps bootstrap rotate admin-server 2>/dev/null)
printf '%s' "$out" | grep -q '^rotated admin-server$' && printf '%s' "$out" | grep -q '^Restart from the admin: admin, server$' \
  && pass "rotate admin-server says what to restart" || fail "rotate output: $out"
after=$(docker exec "$ADMIN" cat /admin-token/call/admin-server.token 2>/dev/null)
[ -n "$after" ] && [ "$after" != "$before" ] && pass "rotation made a new token" || fail "rotation did not change the token"
[ "$(printf '%s' "$after" | sha256sum | cut -d' ' -f1)" = "$(docker exec "$SERVER" cat /server-token/accept/admin-server.sha256 2>/dev/null | tr -d '[:space:]')" ] \
  && pass "the hash matches the new token" || fail "the hash does not match the new token"
owned "$ADMIN" /admin-token/call/admin-server.token "10001 400" "the admin's token file after a rotation (owner, mode)"
owned "$SERVER" /server-token/accept/admin-server.sha256 "10002 444" "the server's hash file after a rotation (owner, mode)"
docker restart "$SERVER" "$ADMIN" >/dev/null
for c in "$SERVER" "$ADMIN"; do
  wait_healthy "$c"
  [ "$(docker inspect -f '{{.State.Health.Status}}' "$c")" = healthy ] && pass "$c is healthy after the restart" || fail "$c is not healthy after the rotation"
done
HASH=$(docker exec "$SERVER" cat /server-token/accept/admin-server.sha256 2>/dev/null | tr -d '[:space:]')

echo "== 3d4. The auditor (ADR 0051)"
AUDITOR=askbar-auditor
if [ "$(docker inspect -f '{{.State.Running}}' "$AUDITOR" 2>/dev/null)" != true ]; then fail "the auditor is not running"; else
  svc_profile "$AUDITOR" "the auditor" <<EOF
health yes
uid 10003
caps yes
nets audit-admin audit-alerter audit-caddy audit-model-gateway audit-server|the auditor is on the audit-* networks only (audit-admin, audit-alerter, audit-caddy, audit-model-gateway, audit-server)
owner $AUDITOR|/auditor-token/accept/admin-auditor-append.sha256|10003 444|the auditor's append hash file (owner, mode)
owner $AUDITOR|/auditor-token/accept/admin-auditor-read.sha256|10003 444|the auditor's read hash file (owner, mode)
owner $AUDITOR|/auditor-token/accept/model-gateway-auditor-read.sha256|10003 444|the auditor's gateway read hash file (owner, mode)
owner $AUDITOR|/auditor-token/accept/admin-auditor-restore.sha256|10003 444|the auditor's restore hash file (owner, mode)
dirs $AUDITOR|10003 500|the auditor's|/auditor-token /auditor-token/accept
EOF
  docker exec "$AUDITOR" bash -c 'timeout 4 bash -c "echo > /dev/tcp/1.1.1.1/443"' >/dev/null 2>&1 \
    && fail "the auditor reached the internet" || pass "the auditor has no route out"
  docker exec "$AUDITOR" bash -c 'timeout 4 getent hosts example.com' >/dev/null 2>&1 \
    && fail "the auditor resolved an outside name" || pass "the auditor cannot resolve an outside name"
  # The API, from a throwaway container on audit-admin with the admin's own tokens: only the right token reaches each route.
  AUD_APPEND=$(docker exec "$ADMIN" cat /admin-token/call/admin-auditor-append.token 2>/dev/null)
  AUD_READ=$(docker exec "$ADMIN" cat /admin-token/call/admin-auditor-read.token 2>/dev/null)
  AUD_RESTORE=$(docker exec "$ADMIN" cat /admin-token/call/admin-auditor-restore.token 2>/dev/null)
  [ -n "$AUD_APPEND" ] && [ -n "$AUD_READ" ] && [ -n "$AUD_RESTORE" ] || fail "the admin holds no auditor tokens"
  out=$(py --network "${PROJECT}_audit-admin" -e "AA=$AUD_APPEND" -e "AR=$AUD_READ" -e "AS=$AUD_RESTORE" -- "
import os, json, urllib.request, urllib.error
def call(method, path, token=None, body=None):
    req = urllib.request.Request('http://auditor:8083/api/audit/v1' + path, method=method, data=None if body is None else json.dumps(body).encode())
    if token: req.add_header('authorization', 'Bearer ' + token)
    if body is not None: req.add_header('content-type', 'application/json')
    try: return urllib.request.urlopen(req, timeout=5).status
    except urllib.error.HTTPError as e: return e.code
    except Exception: return 0
aa, ar, asr = os.environ['AA'], os.environ['AR'], os.environ['AS']
act = {'id': 'isolation1', 'at': 1, 'category': 'isolation', 'action': 'check', 'outcome': 'done'}
print('health-no-token', call('GET', '/health'))
print('status-no-token', call('GET', '/status'))
print('status-wrong', call('GET', '/status', 'wrong'))
print('read-reads', call('GET', '/traces', ar))
print('append-reads', call('GET', '/traces', aa))
print('append-appends', call('POST', '/admin-actions', aa, act))
print('read-appends', call('POST', '/admin-actions', ar, act))
print('append-spans', call('POST', '/spans', aa, {}))
print('read-retention-low', call('PUT', '/retention', ar, {'days': 3}))
print('deletes', call('DELETE', '/admin-actions', ar))
print('restore-reads', call('GET', '/traces', asr))
print('restore-appends', call('POST', '/admin-actions', asr, act))
print('read-restores', call('POST', '/restore', ar, {}))
print('append-restores', call('POST', '/restore', aa, {}))
# The restore link reaches its own route: a used store says 409 (not empty), an empty one 400 (not a backup). Never 401 or 403.
print('restore-restores', 'ok' if call('POST', '/restore', asr, {}) in (400, 409) else 'refused')
" 2>&1)
  want="health-no-token 200
status-no-token 401
status-wrong 401
read-reads 200
append-reads 403
append-appends 201
read-appends 403
append-spans 403
read-retention-low 400
deletes 403
restore-reads 403
restore-appends 403
read-restores 403
append-restores 403
restore-restores ok"
  [ "$out" = "$want" ] && pass "the auditor's tokens each reach only their own routes (no token or a wrong one: 401, the other link's route: 403, retention below 7 days: 400, a delete is refused: no token may change or delete a record; the restore token reaches only POST /restore and no other token reaches it)" || fail "the auditor's API answered: $out"
  # Nothing but the admin's network reaches it: a container on the edge network cannot.
  AUD_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"${PROJECT}_audit-admin\").IPAddress}}" "$AUDITOR")
  [ "$(py --network "$EDGE" -- "
import urllib.request
try: print(urllib.request.urlopen('http://$AUD_IP:8083/api/audit/v1/health', timeout=4).status)
except Exception: print(0)")" = 0 ] && pass "a container on edge cannot reach the auditor" || fail "the auditor is reachable from edge"
  # Caddy's network reaches the ingest listener and nothing else of the auditor: the token API is not bound there.
  out=$(py --network "${PROJECT}_audit-caddy" -- "
import socket
def can(host, port):
    try: socket.create_connection((host, port), timeout=4).close(); return 'open'
    except Exception: return 'closed'
print('ingest', can('auditor-ingest-host', 8086))
print('api-by-alias', can('auditor-ingest-host', 8083))
print('api-by-name', can('auditor', 8083))
" 2>&1)
  [ "$out" = "ingest open
api-by-alias closed
api-by-name closed" ] && pass "on audit-caddy the auditor's ingest listener answers and its token API does not exist" || fail "on audit-caddy the auditor answered: $out"
  # The ingest listener alone cannot be asked for anything: it answers nothing, whatever is sent to it.
  out=$(py --network "${PROJECT}_audit-caddy" -- "
import socket
s = socket.create_connection(('auditor-ingest-host', 8086), timeout=4)
s.sendall(b'GET /api/audit/v1/traces HTTP/1.0\r\n\r\n'); s.settimeout(2)
try: got = s.recv(100)
except Exception: got = b''
print(len(got))
" 2>&1)
  [ "$out" = 0 ] && pass "the ingest listener answers nothing, so Caddy cannot read through it" || fail "the ingest listener answered $out bytes"
  # A request through Caddy becomes an access line, with its path and none of its query.
  MARK="isolation-access-$(head -c 4 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  py --network "$EDGE" -- "
import ssl, urllib.request, urllib.error
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
try: urllib.request.urlopen('https://caddy/$MARK?secret=QUERY-$MARK', timeout=8, context=ctx)
except urllib.error.HTTPError: pass
" >/dev/null 2>&1
  got=""
  for i in $(seq 1 15); do
    got=$(py --network "${PROJECT}_audit-admin" -e "AR=$AUD_READ" -- "
import os, json, urllib.request
req = urllib.request.Request('http://auditor:8083/api/audit/v1/access-logs?limit=50')
req.add_header('authorization', 'Bearer ' + os.environ['AR'])
rows = json.load(urllib.request.urlopen(req, timeout=5))
print(json.dumps(rows))
" 2>&1)
    case "$got" in *"/$MARK"*) break;; esac; sleep 2
  done
  case "$got" in *"/$MARK"*) pass "a request through Caddy is an access line in the auditor";; *) fail "no access line for a request through Caddy: $got";; esac
  case "$got" in *"QUERY-$MARK"*|*"secret="*) fail "an access line kept a query string";; *) pass "the access line keeps the path and not the query string";; esac
fi

echo "== 3d5. model-gateway (ADR 0054)"
GW=askbar-model-gateway
GW_NET="${PROJECT}_gateway-admin"; GW_SRV_NET="${PROJECT}_gateway-server"; GW_OUT="${PROJECT}_outbound"; GW_AUD="${PROJECT}_audit-model-gateway"
if [ "$(docker inspect -f '{{.State.Running}}' "$GW" 2>/dev/null)" != true ]; then fail "model-gateway is not running"; else
  svc_profile "$GW" model-gateway <<EOF
health yes
uid 10004
caps yes
nets audit-model-gateway cpa-gateway gateway-admin gateway-server outbound|model-gateway is on audit-model-gateway, cpa-gateway, gateway-admin, gateway-server and outbound only
owner $GW|/model-gateway-token/accept/server-model-gateway.sha256|10004 444|the gateway's server hash file (owner, mode)
owner $GW|/model-gateway-token/accept/admin-model-gateway.sha256|10004 444|the gateway's admin hash file (owner, mode)
owner $GW|/model-gateway-token/call/model-gateway-auditor.token|10004 400|the gateway's auditor token file (owner, mode)
owner $GW|/model-gateway-token/call/model-gateway-auditor-read.token|10004 400|the gateway's auditor read token file (owner, mode)
dirs $GW|10004 500|the gateway's|/model-gateway-token /model-gateway-token/call /model-gateway-token/accept
owner $AUDITOR|/auditor-token/accept/model-gateway-auditor.sha256|10003 444|the auditor's gateway hash file (owner, mode)
owner $SERVER|/server-token/call/server-model-gateway.token|10002 400|the server's gateway token file (owner, mode)
owner $ADMIN|/admin-token/call/admin-model-gateway.token|10001 400|the admin's gateway token file (owner, mode)
EOF
  # The gateway reaches the auditor and nothing of ours but that.
  docker exec "$GW" bash -c 'exec 3<>/dev/tcp/auditor/8083' >/dev/null 2>&1 && pass "model-gateway reaches the auditor" || fail "model-gateway cannot reach the auditor"
  # (The admin, the server and cpa share a network with it, so their names do resolve there.)
  docker exec "$GW" bash -c "getent hosts caddy" >/dev/null 2>&1 && fail "model-gateway resolves caddy" || pass "model-gateway cannot resolve caddy"
  # Each listener is on its own network's address, and only its own link's token opens it.
  GW_ADMIN_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$GW_NET\").IPAddress}}" "$GW")
  GW_SRV_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$GW_SRV_NET\").IPAddress}}" "$GW")
  GW_OUT_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$GW_OUT\").IPAddress}}" "$GW")
  GW_AUD_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$GW_AUD\").IPAddress}}" "$GW")
  GW_ADM_TOK=$(docker exec "$ADMIN" cat /admin-token/call/admin-model-gateway.token 2>/dev/null)
  GW_SRV_TOK=$(docker exec "$SERVER" cat /server-token/call/server-model-gateway.token 2>/dev/null)
  [ -n "$GW_ADM_TOK" ] && [ -n "$GW_SRV_TOK" ] || fail "the admin or the server holds no gateway token"
  [ "$(http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/health" GET)" = 200 ] && pass "the admin's listener answers on gateway-admin" || fail "the admin's listener does not answer on gateway-admin"
  [ "$(http "$GW_SRV_NET" "http://$GW_SRV_IP:8084/api/model-gateway/v1/health" GET)" = 200 ] && pass "the server's listener answers on gateway-server" || fail "the server's listener does not answer on gateway-server"
  [ "$(http "$GW_OUT" "http://$GW_OUT_IP:8084/api/model-gateway/v1/health" GET)" = 0 ] && pass "nothing answers on outbound" || fail "the gateway answers on outbound"
  [ "$(http "$GW_AUD" "http://$GW_AUD_IP:8084/api/model-gateway/v1/health" GET)" = 0 ] && pass "nothing answers on audit-model-gateway" || fail "the gateway answers on audit-model-gateway"
  [ "$(http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/status" GET)" = 401 ] && pass "no token: 401 on the admin's listener" || fail "the admin's listener answers without a token"
  [ "$(http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/status" GET "$GW_SRV_TOK")" = 401 ] && pass "the server's token does not open the admin's listener" || fail "the server's token opened the admin's listener"
  [ "$(http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/status" GET "$GW_ADM_TOK")" = 200 ] && pass "the admin's token opens GET /status" || fail "the admin's token does not open GET /status"
  [ "$(http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/chat" POST "$GW_ADM_TOK" '{}')" = 404 ] && pass "the admin's listener has no chat route" || fail "the admin's listener serves /chat"
  [ "$(http "$GW_SRV_NET" "http://$GW_SRV_IP:8084/api/model-gateway/v1/chat" POST)" = 401 ] && pass "no token: 401 on the server's listener" || fail "the server's listener answers without a token"
  [ "$(http "$GW_SRV_NET" "http://$GW_SRV_IP:8084/api/model-gateway/v1/chat" POST "$GW_ADM_TOK" '{}')" = 401 ] && pass "the admin's token does not open the server's listener" || fail "the admin's token opened the server's listener"
  [ "$(http "$GW_SRV_NET" "http://$GW_SRV_IP:8084/api/model-gateway/v1/config" PUT "$GW_SRV_TOK" '{}')" = 404 ] && pass "the server's listener has no config route" || fail "the server's listener serves /config"
  [ "$(http "$EDGE" "http://$GW_SRV_IP:8084/api/model-gateway/v1/health" GET)" = 0 ] && pass "a container on edge cannot reach the gateway" || fail "the gateway is reachable from edge"
  # A marker key entered in the admin reaches the gateway, survives its restart (the admin pushes it again), and
  # appears nowhere it must not: not in a response, a log, the gateway's files, the admin's database in the clear,
  # or the auditor's store.
  MARK="sk-isolation-marker-$(head -c 12 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9')"
  # The gateway runs nothing without the audit key (it could not record what a provider says), so a fresh install
  # needs one first; an install that has one keeps it (this call is then refused, which is fine).
  admin_script "$(admin_login)
call('PUT', '/api/admin/v1/setup/audit-key', {'passphrase': 'isolation check passphrase, long enough', 'confirm': 'isolation check passphrase, long enough'})" >/dev/null 2>&1
  pid=$(admin_script "$(admin_login)
s, body = call('POST', '/api/admin/v1/providers', {'name': 'Isolation marker', 'type': 'openai', 'base_url': 'http://mock-llm:8765/v1', 'model': 'm', 'api_key': '$MARK'})
print(json.loads(body).get('id', '') if s == 201 else 'failed')" 2>&1 | tail -1)
  case "$pid" in failed|"") fail "could not add the marker provider: $pid";; *) pass "a provider with a marker key was added through the admin";; esac
  gw_version() { http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/status" GET "$GW_ADM_TOK" >/dev/null; py --network "$GW_NET" -- "
import urllib.request, json
req = urllib.request.Request('http://$GW_ADMIN_IP:8084/api/model-gateway/v1/status', headers={'authorization': 'Bearer $GW_ADM_TOK'})
print(json.load(urllib.request.urlopen(req, timeout=4))['config_version'])"; }
  have=None
  for i in $(seq 1 20); do have=$(gw_version 2>/dev/null | tail -1); [ -n "$have" ] && [ "$have" != None ] && break; sleep 2; done
  [ -n "$have" ] && [ "$have" != None ] && pass "the admin pushed its configuration, the marker key included, to the gateway" || fail "the gateway has no configuration: '$have'"
  # Restart it: the key was only in memory, so the admin has to push it again.
  rs=$(admin_script "$(admin_login)
print(call('POST', '/api/admin/v1/services/model-gateway/restart')[0])" 2>&1 | tail -1)
  [ "$rs" = 200 ] && pass "the admin restarted the gateway and waited for it" || fail "restarting the gateway through the admin answered '$rs'"
  have=None
  for i in $(seq 1 30); do have=$(gw_version 2>/dev/null | tail -1); [ -n "$have" ] && [ "$have" != None ] && break; sleep 2; done
  [ -n "$have" ] && [ "$have" != None ] && pass "after the restart the gateway has its configuration again, with no command" || fail "the restarted gateway has no configuration: '$have'"
  # Where the marker must not be.
  for c in $GW $SERVER $ADMIN $AUDITOR $CADDY; do
    docker logs "$c" 2>&1 | grep -q -- "$MARK" && fail "$c's log holds the marker key" || pass "$c's log holds no marker key"
  done
  docker exec "$GW" sh -c "grep -rIl -- '$MARK' /tmp /model-gateway-token /etc /home /var 2>/dev/null | head -1" | grep -q . \
    && fail "the gateway's filesystem holds the marker key" || pass "the gateway's filesystem holds no marker key"
  docker exec "$AUDITOR" sh -c "grep -rl -- '$MARK' /audit 2>/dev/null | head -1" | grep -q . && fail "the auditor's store holds the marker key" || pass "the auditor's store holds no marker key"
  docker exec "$ADMIN" sh -c "grep -rl -- '$MARK' /admin-data 2>/dev/null | head -1" | grep -q . && fail "the admin's database holds the marker key in the clear" || pass "the admin's database holds the marker key encrypted only"
  resp=$(admin_script "$(admin_login)
for p in ('/api/admin/v1/providers', '/api/admin/v1/services', '/api/admin/v1/status'):
    print(call('GET', p)[1])" 2>&1)
  printf '%s' "$resp" | grep -q -- "$MARK" && fail "an admin response returned the marker key" || pass "no admin response returns the marker key"
  [ "$(http "$GW_NET" "http://$GW_ADMIN_IP:8084/api/model-gateway/v1/config" GET "$GW_ADM_TOK")" != 200 ] && pass "the gateway has no route that returns its configuration" || fail "GET /config answers"
  admin_script "$(admin_login)
print(call('DELETE', '/api/admin/v1/providers/$pid')[0])" >/dev/null 2>&1
fi

echo "== 3d6. The server has no route out and no key (ADR 0054)"
if [ "$(docker inspect -f '{{.State.Running}}' "$SERVER" 2>/dev/null)" != true ]; then fail "the server is not running"; else
  svc_profile "$SERVER" "the server" <<EOF
nets audit-server client-api gateway-server server-api|the server is on audit-server, client-api, gateway-server and server-api only
EOF
  docker exec "$SERVER" bash -c 'timeout 4 bash -c "echo > /dev/tcp/1.1.1.1/443"' >/dev/null 2>&1 && fail "the server reached the internet" || pass "the server has no route out"
  docker exec "$SERVER" bash -c 'timeout 4 getent hosts example.com' >/dev/null 2>&1 && fail "the server resolved an outside name" || pass "the server cannot resolve an outside name"
  for h in cpa mock-llm; do
    docker exec "$SERVER" bash -c "getent hosts $h" >/dev/null 2>&1 && fail "the server resolves $h" || pass "the server cannot resolve $h"
  done
  docker exec "$SERVER" bash -c 'exec 3<>/dev/tcp/gateway-server-host/8084' >/dev/null 2>&1 && pass "the server reaches the gateway" || fail "the server cannot reach the gateway"
  docker exec "$SERVER" bash -c 'exec 3<>/dev/tcp/auditor/8083' >/dev/null 2>&1 && pass "the server reaches the auditor" || fail "the server cannot reach the auditor"
  # A key given to the gateway reaches the server nowhere: not its files, not its database.
  if [ -n "${MARK:-}" ]; then
    docker exec "$SERVER" sh -c "grep -rIl -- '$MARK' /data /tmp /etc /home /var 2>/dev/null | head -1" | grep -q . \
      && fail "the server's filesystem holds the marker key" || pass "the server's filesystem holds no marker key"
    docker exec "$SERVER" sh -c "grep -rl -- '$MARK' /data 2>/dev/null | head -1" | grep -q . && fail "the server's database holds the marker key" || pass "the server's database holds no marker key"
  fi
fi

echo "== 3d7. The alerter (ADR 0055)"
ALERTER=askbar-alerter
AL_ADMIN="${PROJECT}_alerter-admin"; AL_AUD="${PROJECT}_audit-alerter"; AL_OUT="${PROJECT}_alerter-outbound"
if [ "$(docker inspect -f '{{.State.Running}}' "$ALERTER" 2>/dev/null)" != true ]; then fail "the alerter is not running"; else
  svc_profile "$ALERTER" "the alerter" <<EOF
health yes
uid 10005
caps yes
nets alerter-admin alerter-outbound audit-alerter|the alerter is on alerter-admin, alerter-outbound and audit-alerter only
owner $ALERTER|/alerter-token/accept/auditor-alerter.sha256|10005 444|the alerter's auditor hash file (owner, mode)
owner $ALERTER|/alerter-token/accept/admin-alerter.sha256|10005 444|the alerter's admin hash file (owner, mode)
owner $ALERTER|/alerter-token/call/alerter-auditor.token|10005 400|the alerter's auditor token file (owner, mode)
dirs $ALERTER|10005 500|the alerter's|/alerter-token /alerter-token/call /alerter-token/accept
owner $AUDITOR|/auditor-token/accept/alerter-auditor.sha256|10003 444|the auditor's alerter hash file (owner, mode)
owner $AUDITOR|/auditor-token/call/auditor-alerter.token|10003 400|the auditor's alerter token file (owner, mode)
owner $ADMIN|/admin-token/call/admin-alerter.token|10001 400|the admin's alerter token file (owner, mode)
EOF
  # The server and the auditor still have no route out (3d4, 3d6); the gateway's is `outbound`, which the alerter is not on.
  # It reaches the auditor on audit-alerter, and nothing else of ours.
  docker exec "$ALERTER" bash -c 'exec 3<>/dev/tcp/auditor/8083' >/dev/null 2>&1 && pass "the alerter reaches the auditor" || fail "the alerter cannot reach the auditor"
  for h in caddy server askbar-model-gateway cpa; do
    docker exec "$ALERTER" bash -c "getent hosts $h" >/dev/null 2>&1 && fail "the alerter resolves $h" || pass "the alerter cannot resolve $h"
  done
  AL_ADM_TOK=$(docker exec "$ADMIN" cat /admin-token/call/admin-alerter.token 2>/dev/null)
  AL_AUD_TOK=$(docker exec "$AUDITOR" cat /auditor-token/call/auditor-alerter.token 2>/dev/null)
  AL_OWN_TOK=$(docker exec "$ALERTER" cat /alerter-token/call/alerter-auditor.token 2>/dev/null)
  case "$AL_ADM_TOK" in admalr_*) pass "the admin's alerter token has its prefix";; *) fail "the admin's alerter token has the wrong prefix";; esac
  case "$AL_AUD_TOK" in audalr_*) pass "the auditor's alerter token has its prefix";; *) fail "the auditor's alerter token has the wrong prefix";; esac
  case "$AL_OWN_TOK" in alraud_*) pass "the alerter's auditor token has its prefix";; *) fail "the alerter's auditor token has the wrong prefix";; esac
  # Each listener is on its own network's address and nowhere else, and only its own link's token opens it.
  AL_ADMIN_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$AL_ADMIN\").IPAddress}}" "$ALERTER")
  AL_AUD_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$AL_AUD\").IPAddress}}" "$ALERTER")
  AL_OUT_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$AL_OUT\").IPAddress}}" "$ALERTER")
  [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1/health" GET)" = 200 ] && pass "the admin's listener answers on alerter-admin" || fail "the admin's listener does not answer on alerter-admin"
  [ "$(http "$AL_AUD" "http://$AL_AUD_IP:8085/api/alerter/v1/health" GET)" = 200 ] && pass "the auditor's listener answers on audit-alerter" || fail "the auditor's listener does not answer on audit-alerter"
  [ "$(http "$AL_OUT" "http://$AL_OUT_IP:8085/api/alerter/v1/health" GET)" = 0 ] && pass "nothing answers on alerter-outbound" || fail "the alerter answers on alerter-outbound"
  [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1/status" GET)" = 401 ] && pass "no token: 401 on the admin's listener" || fail "the admin's listener answers without a token"
  [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1/status" GET "wrong-token")" = 401 ] && pass "a wrong token: 401 on the admin's listener" || fail "a wrong token opened the admin's listener"
  [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1/status" GET "$AL_AUD_TOK")" = 401 ] && pass "the auditor's token does not open the admin's listener" || fail "the auditor's token opened the admin's listener"
  [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1/status" GET "$AL_ADM_TOK")" = 200 ] && pass "the admin's token opens GET /status" || fail "the admin's token does not open GET /status"
  [ "$(http "$AL_AUD" "http://$AL_AUD_IP:8085/api/alerter/v1/events" POST "" '{}')" = 401 ] && pass "no token: 401 on the auditor's listener" || fail "the auditor's listener answers without a token"
  [ "$(http "$AL_AUD" "http://$AL_AUD_IP:8085/api/alerter/v1/events" POST "$AL_ADM_TOK" '{}')" = 401 ] && pass "the admin's token does not open the auditor's listener" || fail "the admin's token opened the auditor's listener"
  [ "$(http "$AL_AUD" "http://$AL_AUD_IP:8085/api/alerter/v1/status" GET "$AL_AUD_TOK")" = 404 ] && pass "the auditor's listener has no status route" || fail "the auditor's listener serves /status"
  [ "$(http "$AL_AUD" "http://$AL_AUD_IP:8085/api/alerter/v1/config" PUT "$AL_AUD_TOK" '{}')" = 404 ] && pass "the auditor's listener has no config route" || fail "the auditor's listener serves /config"
  # A backup's export and a restore's import are the admin's link alone (ADR 0013): no token and the auditor's token are refused on the
  # admin's listener, and the auditor's listener has neither route.
  for route in /exports /imports; do
    [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1$route" POST)" = 401 ] && pass "no token: 401 on POST $route" || fail "POST $route answers without a token"
    [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1$route" POST "$AL_AUD_TOK")" = 401 ] && pass "the auditor's token does not open POST $route" || fail "the auditor's token opened POST $route"
    [ "$(http "$AL_AUD" "http://$AL_AUD_IP:8085/api/alerter/v1$route" POST "$AL_AUD_TOK")" = 404 ] && pass "the auditor's listener has no $route route" || fail "the auditor's listener serves $route"
  done
  [ "$(http "$EDGE" "http://$AL_AUD_IP:8085/api/alerter/v1/health" GET)" = 0 ] && pass "a container on edge cannot reach the alerter" || fail "the alerter is reachable from edge"
  # The admin gives it its settings with no command (an alerter with no channel is given an empty set, so that it answers): after a
  # few seconds the status shows a configuration, and a test alert says that nothing is set up.
  have=null
  for i in $(seq 1 20); do
    have=$(py --network "$AL_ADMIN" -- "
import urllib.request, json
req = urllib.request.Request('http://$AL_ADMIN_IP:8085/api/alerter/v1/status', headers={'authorization': 'Bearer $AL_ADM_TOK'})
print(json.load(urllib.request.urlopen(req, timeout=4))['config_version'])" 2>/dev/null | tail -1)
    [ -n "$have" ] && [ "$have" != None ] && break; sleep 2
  done
  [ -n "$have" ] && [ "$have" != None ] && pass "the admin gave the alerter its settings, with no command" || fail "the alerter has no configuration: '$have'"
  [ "$(http "$AL_ADMIN" "http://$AL_ADMIN_IP:8085/api/alerter/v1/test" POST "$AL_ADM_TOK")" = 200 ] && pass "configured, POST /test answers" || fail "a configured alerter did not answer POST /test"
  # Its own link to the auditor works end to end: its token is accepted there, and the auditor notes its request.
  ar=$(py --network "$AL_AUD" -- "
import urllib.request, json
req = urllib.request.Request('http://auditor-alerter-host:8083/api/audit/v1/status', headers={'authorization': 'Bearer $AL_OWN_TOK'})
print('alerter' in json.load(urllib.request.urlopen(req, timeout=4)).get('last_requests', {}))" 2>&1 | tail -1)
  [ "$ar" = True ] && pass "the alerter's token opens the auditor on audit-alerter, which has noted a request from it" || fail "the alerter's link to the auditor: '$ar'"
  ar=$(py --network "$AL_AUD" -- "
import urllib.request, urllib.error
req = urllib.request.Request('http://auditor-alerter-host:8083/api/audit/v1/traces', headers={'authorization': 'Bearer $AL_OWN_TOK'})
try: print(urllib.request.urlopen(req, timeout=4).status)
except urllib.error.HTTPError as e: print(e.code)" 2>&1 | tail -1)
  [ "$ar" = 403 ] && pass "the alerter's token cannot read the auditor's records" || fail "the alerter's token read the auditor's records: '$ar'"
  # A stand-in ntfy server on alerter-outbound (for this check only), a marker topic and token entered in the admin, a test alert that
  # reaches the sink with them, a restart that needs no command to bring the settings back, and the marker nowhere it must not be.
  docker rm -f askbar-ntfy-sink >/dev/null 2>&1
  SINK_PY="$(cd "$(dirname "$0")/.." && pwd)/tests/e2e/ntfy_sink.py"
  if docker run -d --rm --name askbar-ntfy-sink --network "$AL_OUT" -v "$SINK_PY:/sink.py:ro" "$PYIMG" python -I /sink.py 8080 0.0.0.0 >/dev/null 2>&1; then
    sleep 2
    ALMARK="isolation-marker-$(head -c 12 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9')"
    ALTOK="isolation-token-$(head -c 12 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9')"
    sink_seen() { py --network "$AL_OUT" -- "import urllib.request; print(urllib.request.urlopen('http://askbar-ntfy-sink:8080/_seen', timeout=4).read().decode())" 2>/dev/null; }
    out=$(admin_script "$(admin_login)
s, body = call('PUT', '/api/admin/v1/alerts', {'ntfy': {'server': 'http://askbar-ntfy-sink:8080', 'topic': '$ALMARK', 'token': '$ALTOK'}})
print(s)" 2>&1 | tail -1)
    [ "$out" = 200 ] && pass "alert delivery settings with a marker topic and token were entered in the admin" || fail "entering the alert settings answered '$out'"
    ok=""
    for i in $(seq 1 10); do
      ok=$(admin_script "$(admin_login)
s, body = call('POST', '/api/admin/v1/alerts/test')
print(json.loads(body)['results'][0]['ok'] if s == 200 and json.loads(body)['results'] else 'no')" 2>&1 | tail -1)
      [ "$ok" = True ] && break; sleep 2
    done
    [ "$ok" = True ] && pass "a test alert is delivered through the alerter, over alerter-outbound" || fail "the test alert: '$ok'"
    seen=$(sink_seen)
    case "$seen" in *"/$ALMARK"*"Bearer $ALTOK"*) pass "the sink received the test alert on the topic, with the token";; *) fail "the sink did not get the test alert as expected: $seen";; esac
    # A restart: the settings were only in memory, so the admin has to push them again, with no command.
    rs=""
    for i in $(seq 1 12); do
      rs=$(admin_script "$(admin_login)
print(call('POST', '/api/admin/v1/services/alerter/restart')[0])" 2>&1 | tail -1)
      [ "$rs" = 200 ] && break; sleep 5
    done
    [ "$rs" = 200 ] && pass "the admin restarted the alerter and waited for it" || fail "restarting the alerter through the admin answered '$rs'"
    ok=""
    for i in $(seq 1 15); do
      ok=$(admin_script "$(admin_login)
s, body = call('POST', '/api/admin/v1/alerts/test')
print(json.loads(body)['results'][0]['ok'] if s == 200 and json.loads(body)['results'] else 'no')" 2>&1 | tail -1)
      [ "$ok" = True ] && break; sleep 2
    done
    [ "$ok" = True ] && pass "after the restart the alerter delivers again: the admin gave it its settings back" || fail "after the restart: '$ok'"
    # Where the marker must not be.
    for c in $ALERTER $SERVER $ADMIN $AUDITOR $GW $CADDY; do
      docker logs "$c" 2>&1 | grep -q -e "$ALMARK" -e "$ALTOK" && fail "$c's log holds the marker topic or token" || pass "$c's log holds no marker topic or token"
    done
    docker exec "$ALERTER" sh -c "grep -rIl -e '$ALMARK' -e '$ALTOK' /alerter-data /alerter-token /tmp /etc /home /var 2>/dev/null | head -1" | grep -q . \
      && fail "the alerter's filesystem holds the marker" || pass "the alerter's filesystem holds no marker (the settings are in memory only)"
    docker exec "$AUDITOR" sh -c "grep -rl -e '$ALMARK' -e '$ALTOK' /audit 2>/dev/null | head -1" | grep -q . && fail "the auditor's store holds the marker" || pass "the auditor's store holds no marker"
    docker exec "$ADMIN" sh -c "grep -rl -e '$ALMARK' -e '$ALTOK' /admin-data 2>/dev/null | head -1" | grep -q . && fail "the admin's database holds the marker in the clear" || pass "the admin's database holds the marker encrypted only"
    resp=$(admin_script "$(admin_login)
for p in ('/api/admin/v1/alerts', '/api/admin/v1/services', '/api/admin/v1/status', '/api/admin/v1/settings'):
    print(call('GET', p)[1])" 2>&1)
    printf '%s' "$resp" | grep -q -e "$ALMARK" -e "$ALTOK" && fail "an admin response returned the marker" || pass "no admin response returns the marker topic or token"
    # The auditor's rules (ADR 0051, ADR 0055), end to end: an unseal reaches the sink and holds nothing of the passphrase, and a
    # gateway that is stopped for more than five minutes is a `component_silent` that names its link. (The passphrase is the one the
    # check set up for the audit key above; an install that already had a key refuses it and this says so.)
    UNSEAL_PASS='isolation check passphrase, long enough'
    out=$(admin_script "$(admin_login)
s, body = call('POST', '/api/admin/v1/audit/session', {'passphrase': '$UNSEAL_PASS'})
print(s)
call('DELETE', '/api/admin/v1/audit/session')" 2>&1 | tail -1)
    case "$out" in 200|201) pass "sealed content was opened through the admin (to see its alert)";; *) fail "the unseal for the alert check answered '$out'";; esac
    got=""
    for i in $(seq 1 15); do
      seen=$(sink_seen); case "$seen" in *"Audit content opened"*) got=1; break;; esac; sleep 2
    done
    [ -n "$got" ] && pass "an unseal reached the sink as an alert" || fail "no unseal alert reached the sink"
    case "$seen" in *"isolation check passphrase"*) fail "the unseal alert holds the passphrase";; *) pass "the unseal alert holds no passphrase";; esac
    # This one waits out the auditor's five minutes of silence, about half of the whole run, so it is slow: it runs when
    # ASKBAR_SLOW_CHECKS=1 (ci.yml sets it on a push to main and on a manual run, not on a pull request). The rule itself is
    # tested with a fake clock (services/auditor/src/rules.rs) and against a real auditor (services/auditor/tests/alerts.rs).
    if [ "${ASKBAR_SLOW_CHECKS:-}" = 1 ]; then
      docker stop "$GW" >/dev/null 2>&1
      got=""
      for i in $(seq 1 60); do
        seen=$(sink_seen); case "$seen" in *"Nothing has come from model-gateway-auditor"*) got=1; break;; esac; sleep 10
      done
      [ -n "$got" ] && pass "a gateway stopped for more than five minutes raised component_silent, naming its link" || fail "no component_silent reached the sink within ten minutes of stopping the gateway"
      docker start "$GW" >/dev/null 2>&1
      wait_healthy "$GW"
      [ "$(docker inspect -f '{{.State.Health.Status}}' "$GW")" = healthy ] && pass "the gateway is healthy again" || fail "the gateway did not come back"
    else
      echo "SKIP  a stopped gateway raising component_silent (about 5 minutes: set ASKBAR_SLOW_CHECKS=1 to run it)"
    fi
    # Put it back: no channel, and the sink gone.
    admin_script "$(admin_login)
print(call('PUT', '/api/admin/v1/alerts', {'ntfy': None})[0])" >/dev/null 2>&1
    docker rm -f askbar-ntfy-sink >/dev/null 2>&1
  else
    fail "could not start the stand-in ntfy server on alerter-outbound"
  fi
fi

echo "== 3d8. The egress proxy (ADR 0056)"
EGP=askbar-egress-proxy
EGP_OUT="${PROJECT}_egress-outbound"
if [ "$(docker inspect -f '{{.State.Running}}' "$EGP" 2>/dev/null)" != true ]; then fail "the egress proxy is not running"; else
  svc_profile "$EGP" "the egress proxy" <<EOF
health yes
uid 10006
caps yes
nets egress-admin egress-alerter egress-cpa egress-model-gateway egress-outbound|the egress proxy is on egress-admin, egress-alerter, egress-cpa, egress-model-gateway and egress-outbound only
owner $EGP|/egress-proxy-token/accept/admin-egress-proxy.sha256|10006 444|the egress proxy's admin hash file (owner, mode)
dirs $EGP|10006 500|the egress proxy's|/egress-proxy-token /egress-proxy-token/call /egress-proxy-token/accept
owner $ADMIN|/admin-token/call/admin-egress-proxy.token|10001 400|the admin's egress proxy token file (owner, mode)
EOF
  # Its listeners are on the internal networks' aliases, never a wildcard: on egress-outbound, the one route out, nothing answers.
  EGP_OUT_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$EGP_OUT\").IPAddress}}" "$EGP")
  for p in 8086 3128 2525; do
    [ "$(http "$EGP_OUT" "http://$EGP_OUT_IP:$p/" GET)" = 0 ] && pass "nothing answers on egress-outbound, port $p" || fail "the egress proxy answers on egress-outbound, port $p"
  done
  # It resolves itself and no name of ours, and reaches none of the services that hold data.
  docker exec "$EGP" bash -c 'getent hosts egress-admin-host' >/dev/null 2>&1 && pass "the egress proxy resolves its own alias" || fail "the egress proxy cannot resolve its own alias"
  for h in caddy server admin auditor askbar-model-gateway askbar-alerter cpa mock-llm; do
    docker exec "$EGP" bash -c "getent hosts $h" >/dev/null 2>&1 && fail "the egress proxy resolves $h" || pass "the egress proxy cannot resolve $h"
  done
  for t in auditor/8083 server/8080 admin/8084; do
    docker exec "$EGP" bash -c "exec 3<>/dev/tcp/$t" >/dev/null 2>&1 && fail "the egress proxy reaches ${t%/*}" || pass "the egress proxy cannot reach ${t%/*}"
  done
fi

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
[ "$out" = "404 404 404" ] && pass "only the six services can be restarted, and nothing else can be done to them" || fail "service actions accepted a bad name or action: $out"

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
refused "the admin has no /secrets" docker exec "$ADMIN" test -e /secrets
modes_ok=1
m=$(docker exec "$ADMIN" stat -c %a /admin-secrets/admin.key 2>/dev/null)
[ "$m" = 600 ] || { fail "/admin-secrets/admin.key has mode ${m:-missing}, expected 600"; modes_ok=0; }
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
# A backup needs a backup key (ADR 0013); make one if the install has none (a 409 means it has).
call('PUT', '/api/admin/v1/setup/backup-key', {'passphrase': 'isolation backup passphrase, long enough', 'confirm': 'isolation backup passphrase, long enough'})
c.request('GET', '/api/admin/v1/backup', headers={'x-askbar-admin': '1', 'cookie': cookie})
r = c.getresponse(); raw = r.read()
assert r.status == 200, (r.status, raw[:200])
names = [m.name for m in tarfile.open(fileobj=io.BytesIO(raw)).getmembers()]
print('names', ','.join(names))
# The whole bundle is ciphertext: no database header, no provider key, no pair code, no password hash, no private key.
for label, needle in (('plaintext database', 'SQLite format 3'), ('provider key', '$MARK'), ('pair code', code), ('password hash', '\$argon2'), ('private key', 'AGE-SECRET-KEY')):
    print(label, 'FOUND' if needle.encode() in raw else 'absent')
call('DELETE', '/api/admin/v1/providers/' + provider_id)" 2>&1)
printf '%s\n' "$out" | grep -q '^names manifest.json,backup-key.age,admin.age,server.age,alerter.age,cpa.age,audit.age$' && pass "the backup is a bundle of a manifest, the protected key and the admin's, the server's, the alerter's, cpa's and the auditor's encrypted exports" || fail "backup contents: $out"
# Only the admin mounts the backup volume, read-write (ADR 0013, D27): section 0.
check "the admin can write to /backup" docker exec "$ADMIN" sh -c 'touch /backup/.isolation-check && rm /backup/.isolation-check'
check "the server keeps control.db and a file per space under /data" docker exec "$SERVER" sh -c 'test -f /data/control.db && ls /data/spaces/*.db'
refused "the server makes no askbar.db (the layout before spaces)" docker exec "$SERVER" test -e /data/askbar.db
# The contract changed (plan step 6.2): the backup used to be a plain tar with the secrets taken out; it is now encrypted and
# holds them, so the check is that nothing is readable, not that nothing secret is there.
for what in "plaintext database" "provider key" "pair code" "password hash" "private key"; do
  printf '%s\n' "$out" | grep -q "^$what absent" && pass "the backup holds no $what in the clear" || fail "the backup holds the $what in the clear ($out)"
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
# /admin/ goes to the admin's landing page, Home (LANDING_PAGE in services/admin/src/constants.rs), through Caddy.
code=$(curl -skL -o /dev/null -w '%{http_code} %{url_effective}' --max-time 5 https://localhost/admin/ 2>/dev/null || true)
[ "$code" = "200 https://localhost/admin/home/" ] && pass "https://localhost/admin/ opens the admin's landing page (Home)" || fail "https://localhost/admin/ answered '$code'"
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
# Three replies at once, through Caddy (plan step 7.2, D31). PRINTED, NOT A GATE: the gate is
# `cargo test -p askbar-server --test load`, which subtracts the mock model's own time. Here the same scenario runs through the
# real stack (Caddy, the server, model-gateway, the auditor) to the mock model, and the mock's own time is measured inside its
# container and printed beside it. Nothing in this block can fail the run: a problem prints a NOTE line and the block moves on.
note() { printf 'NOTE  %s\n' "$1"; }
LOAD_MOCK="${PROJECT}-mock-llm-1"
if ! command -v python3 >/dev/null 2>&1; then
  note "latency through Caddy: python3 is not on this machine"
elif [ "$(docker inspect -f '{{.State.Running}}' "$LOAD_MOCK" 2>/dev/null)" != true ]; then
  note "latency through Caddy: $LOAD_MOCK is not running (it is in the dev profile)"
else
  load_set=$(admin_setup_provider LOAD load sk-load-check 2>&1 | grep '^LOAD ' | tail -1)
  if [ -z "$load_set" ]; then
    note "latency through Caddy: could not set up a provider, an approved model and a client"
  else
    load_pid=$(printf '%s' "$load_set" | cut -d' ' -f2); load_pairing=$(printf '%s' "$load_set" | cut -d' ' -f3)
    sleep 2   # the gateway has the catalog a moment after the admin saves it
    out=$(python3 -I tests/e2e/load_scenario.py https://localhost --pairing "$load_pairing" --insecure --rounds 10 2>&1) \
      && note "$out" || note "latency through Caddy did not run: $out"
    out=$(docker exec -i "$LOAD_MOCK" python3 - mock http://127.0.0.1:8765/v1 --rounds 10 < tests/e2e/load_scenario.py 2>&1) \
      && note "$out" || note "the mock model's own latency did not run: $out"
    # Put the catalog back as it was and take the provider away again.
    admin_cleanup_provider "$load_pid" >/dev/null 2>&1 || note "latency through Caddy: the provider it made could not be removed"
  fi
fi

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

echo "== 8. Failure drills (plan step 7.3, D32)"
# What the stack does when a part of it is gone, run for real against the throwaway stack: a full restart, the auditor down, the admin
# down, a tiny auditor disk and the alerter down. Each drill leaves every service running again. They need python3 on this machine (a
# chat goes in through Caddy, as in the latency block above) and the mock LLM, which the dev profile starts here if it is not running.
drill_compose() { docker compose -p "$PROJECT" --profile dev "$@"; }
drill_probe() { python3 tests/e2e/chat_probe.py "$@" --insecure 2>&1 | tail -1; }
# Healthy by its own health check; Caddy and CLIProxyAPI have none, so for them running is what there is to ask.
drill_healthy() { [ "$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{if .State.Running}}healthy{{end}}{{end}}' "$1" 2>/dev/null)" = healthy ]; }
drill_wait_healthy() { # container seconds
  local i; for i in $(seq 1 "$2"); do drill_healthy "$1" && return 0; sleep 1; done; return 1
}
drill_chat_ok() { # token seconds -> 0 when a reply finishes within the time; DRILL_LAST holds the last answer
  local i; DRILL_LAST=""
  for i in $(seq 1 $(( $2 / 3 + 1 ))); do
    DRILL_LAST=$(drill_probe chat https://localhost "$1"); [ "$DRILL_LAST" = ok ] && return 0; sleep 3
  done
  return 1
}
DRILL_SERVICES="$SERVER $ADMIN $CADDY askbar-cpa askbar-auditor askbar-model-gateway askbar-alerter"
DRILL_AUDITOR=askbar-auditor; DRILL_GW=askbar-model-gateway; DRILL_ALERTER=askbar-alerter
drill_token=""; drill_pid=""
if ! command -v python3 >/dev/null 2>&1; then
  fail "the failure drills need python3 on this machine"
else
  # A provider the mock model answers for, an approved model, a person and a client, as the latency block makes.
  drill_compose up -d --no-build mock-llm >/dev/null 2>&1
  drill_set=$(admin_setup_provider DRILL drill sk-drill-check 2>&1 | grep '^DRILL ' | tail -1)
  if [ -n "$drill_set" ]; then
    drill_pid=$(printf '%s' "$drill_set" | cut -d' ' -f2)
    drill_token=$(drill_probe pair https://localhost "$(printf '%s' "$drill_set" | cut -d' ' -f3)")
  fi
  if [ -z "$drill_token" ] || [ "${#drill_token}" -lt 20 ]; then
    fail "the failure drills could not be set up (a provider, an approved model, a client): '${drill_token:-no token}'"
    drill_token=""
  elif drill_chat_ok "$drill_token" 90; then
    pass "a chat through Caddy, the server, model-gateway and the auditor to the mock model works before the drills"
  else
    fail "a chat did not work before the drills: $DRILL_LAST"
    drill_token=""
  fi
fi

if [ -n "$drill_token" ]; then
  echo "-- the stack comes back from a full restart"
  convs_before=$(drill_probe conversations https://localhost "$drill_token")
  drill_compose stop >/dev/null 2>&1
  if drill_compose up -d --no-build --wait --wait-timeout 300 >/dev/null 2>&1; then
    pass "every service stopped and started again, and the stack reports itself up"
  else
    fail "the stack did not come back up from a full stop"
  fi
  for c in $DRILL_SERVICES; do
    drill_wait_healthy "$c" 90 || fail "$c is not healthy after the restart"
  done
  out=$(admin_script "$(admin_login)
s, body = call('GET', '/api/admin/v1/providers')
print('PROVIDER', '$drill_pid' in body)" 2>&1 | grep '^PROVIDER ' | tail -1)
  [ "$out" = "PROVIDER True" ] && pass "after the restart the admin signs in with the same password and still has its provider" || fail "after the restart the admin: '$out'"
  # The server answers 503 not_configured until the admin has pushed its settings again, so the count is asked for until it
  # is a number (up to 90 s), as the reply check below is.
  convs_after=""
  for _ in $(seq 1 30); do
    convs_after=$(drill_probe conversations https://localhost "$drill_token")
    case "$convs_after" in [0-9]*) break;; esac
    sleep 3
  done
  case "$convs_before:$convs_after" in
    [0-9]*:[0-9]*) [ "$convs_after" -ge "$convs_before" ] && [ "$convs_after" -ge 1 ] && pass "the conversations the server held are still there ($convs_after)" || fail "conversations before the restart: $convs_before, after: $convs_after";;
    *) fail "could not count conversations around the restart: '$convs_before' then '$convs_after'";;
  esac
  drill_chat_ok "$drill_token" 150 && pass "replies work again after the restart (the admin gave the services their settings again)" || fail "no reply after the restart: $DRILL_LAST"

  echo "-- the auditor down: a new reply is refused, then recovery"
  docker stop "$DRILL_AUDITOR" >/dev/null
  out=$(drill_probe chat https://localhost "$drill_token")
  case "$out" in
    # The gateway's 503 audit_backlog reaches the client as an error event with the code provider_error
    # once the stream is open (wire contract: the server's own words); before it, as the HTTP 503.
    "refused 503 audit_backlog"|"refused sse provider_error") pass "with the auditor down a reply is refused, never made unrecorded ($out)";;
    ok) fail "a reply was made while the auditor was down: nothing may be unrecorded";;
    *) fail "with the auditor down the answer was '$out', not a refusal (audit_backlog or provider_error)";;
  esac
  docker start "$DRILL_AUDITOR" >/dev/null
  drill_wait_healthy "$DRILL_AUDITOR" 60 && pass "the auditor is healthy again" || fail "the auditor did not come back"
  drill_chat_ok "$drill_token" 150 && pass "replies work again once the auditor is back" || fail "no reply after the auditor came back: $DRILL_LAST"

  echo "-- the admin down: chat still works"
  docker stop "$ADMIN" >/dev/null
  drill_chat_ok "$drill_token" 30 && pass "with the admin stopped a reply still finishes (the server and the gateway hold what they need)" || fail "no reply with the admin stopped: $DRILL_LAST"
  docker start "$ADMIN" >/dev/null
  drill_wait_healthy "$ADMIN" 90 && pass "the admin is healthy again" || fail "the admin did not come back"
  out=$(admin_script "$(admin_login)
print('SIGNED IN')" 2>&1 | grep '^SIGNED IN' | tail -1)
  [ "$out" = "SIGNED IN" ] && pass "the admin signs in again after it was down" || fail "the admin after it was down: '$out'"
  drill_chat_ok "$drill_token" 60 && pass "replies still work with the admin back" || fail "no reply once the admin was back: $DRILL_LAST"

  echo "-- a tiny auditor disk: a new span is refused"
  # A second auditor, from the same image and with the real hardening, on a 2 MB memory disk: the real one is not touched. It takes the
  # real hash files (read-only) and the gateway's own token, so what is asked of it is what the gateway asks of the real one.
  drill_gw_tok=$(docker exec "$DRILL_GW" cat /model-gateway-token/call/model-gateway-auditor.token 2>/dev/null)
  docker rm -f askbar-drill-auditor >/dev/null 2>&1
  if [ -z "$drill_gw_tok" ]; then
    fail "the tiny-disk drill could not read the gateway's token"
  elif docker run -d --name askbar-drill-auditor --network "${PROJECT}_audit-model-gateway" --read-only --cap-drop ALL \
      --security-opt no-new-privileges --tmpfs /audit:rw,size=2m,uid=10003,gid=10003,mode=0700 --tmpfs /tmp:rw,size=8m \
      -v "${PROJECT}_auditor-token:/auditor-token:ro" -e ASKBAR_AUDIT_API_ADDR=0.0.0.0:8083 askbar-auditor:latest >/dev/null 2>&1 \
      && drill_wait_healthy askbar-drill-auditor 60; then
    out=$(py --network "${PROJECT}_audit-model-gateway" -- "
import json, time, urllib.request, urllib.error
def call(method, path, body=None):
    req = urllib.request.Request('http://askbar-drill-auditor:8083/api/audit/v1' + path, method=method, data=None if body is None else json.dumps(body).encode())
    req.add_header('authorization', 'Bearer $drill_gw_tok')
    if body is not None: req.add_header('content-type', 'application/json')
    try:
        r = urllib.request.urlopen(req, timeout=5); return r.status, r.read().decode()
    except urllib.error.HTTPError as e: return e.code, e.read().decode()
    except Exception as e: return 0, type(e).__name__
now = int(time.time())
print('hello', call('POST', '/hello', {'boot_id': 'drill'})[0])
s, body = call('POST', '/spans', {'id': 'drill-span', 'trace_id': 'drill-trace', 'kind': 'turn', 'name': 'drill', 'started_at': now, 'started_ms': now * 1000})
try: code = json.loads(body).get('error')
except Exception: code = body[:40]
print('span', s, code)
print('status', call('GET', '/status')[0])" 2>&1)
    printf '%s' "$out" | grep -q '^span 503 audit_backlog$' && pass "an auditor on a 2 MB disk refuses a new span with audit_backlog" || fail "a new span on a tiny disk: $out"
    printf '%s' "$out" | grep -q '^hello 200$' && printf '%s' "$out" | grep -q '^status 200$' && pass "and still answers its hello and its status, so the services can tell it is alive" || fail "the tiny auditor's hello and status: $out"
  else
    fail "the second auditor on a tiny disk did not start: $(docker logs askbar-drill-auditor 2>&1 | tail -2 | tr '\n' ' ')"
  fi
  docker rm -f askbar-drill-auditor >/dev/null 2>&1
  drill_healthy "$DRILL_AUDITOR" && pass "the real auditor was not touched by it" || fail "the real auditor is not healthy"

  echo "-- the alerter down: the admin shows its banner"
  docker stop "$DRILL_ALERTER" >/dev/null
  out=$(admin_script "$(admin_login)
import time
end = time.time() + 180
state = None
while time.time() < end:
    s, body = call('GET', '/api/admin/v1/status')
    state = json.loads(body).get('alerts') if s == 200 else 'status ' + str(s)
    if state == 'unreachable': break
    time.sleep(5)
print('BANNER', state)" 2>&1 | grep '^BANNER ' | tail -1)
  [ "$out" = "BANNER unreachable" ] && pass "with the alerter stopped the admin's status says alerts are unreachable (the banner on every page)" || fail "the admin's alert banner with the alerter down: '$out'"
  docker start "$DRILL_ALERTER" >/dev/null
  drill_wait_healthy "$DRILL_ALERTER" 90 && pass "the alerter is healthy again" || fail "the alerter did not come back"
  out=$(admin_script "$(admin_login)
import time
end = time.time() + 150
state = None
while time.time() < end:
    s, body = call('GET', '/api/admin/v1/status')
    state = json.loads(body).get('alerts') if s == 200 else 'status ' + str(s)
    if state != 'unreachable': break
    time.sleep(5)
print('BANNER', state)" 2>&1 | grep '^BANNER ' | tail -1)
  case "$out" in "BANNER ok"|"BANNER not_set_up") pass "the banner goes away once the alerter answers ($out)";; *) fail "the banner after the alerter came back: '$out'";; esac

  echo "-- the stack is whole again"
  for c in $DRILL_SERVICES; do drill_healthy "$c" || fail "$c is not healthy at the end of the drills"; done
  [ "$FAILS" -eq 0 ] && pass "every service is healthy at the end of the drills" || true
  # The provider the drills made goes, and the catalog is put back as it was.
  admin_cleanup_provider "$drill_pid" >/dev/null 2>&1 || note "the failure drills' provider could not be removed"
fi

echo
if [ "$FAILS" -eq 0 ]; then echo "All isolation checks passed."; else echo "$FAILS isolation check(s) FAILED."; exit 1; fi
