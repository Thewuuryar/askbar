#!/usr/bin/env bash
# compose.staging.yaml merged over compose.yaml and over a release compose file: every container name has the askbar-staging- prefix,
# Caddy publishes only 8443 and 8080, and the server keeps the same mounts as in compose.yaml except that its data directory is
# ./data-staging (the override repeats the list, so this catches a mount added to compose.yaml and not here). Skipped, and said so,
# without `docker compose` (it needs no daemon). `!override` needs Docker Compose 2.24 or later.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
if ! docker compose version > /dev/null 2>&1; then echo "skip staging override: docker compose is not installed"; exit 0; fi
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
FAILS=0
expect() { local what=$1; shift; if "$@"; then echo "ok   $what"; else echo "FAIL $what"; FAILS=$((FAILS + 1)); fi; }

docker compose -f compose.yaml --profile dev config --format json > "$T/base.json" 2> "$T/err" || { echo "FAIL compose.yaml config: $(cat "$T/err")"; exit 1; }
docker compose -f compose.yaml -f compose.staging.yaml --profile dev config --format json > "$T/staging.json" 2> "$T/err" \
  || { echo "FAIL the staging merge: $(cat "$T/err") (Docker Compose 2.24 or later is needed for !override)"; exit 1; }

python3 - "$T/base.json" "$T/staging.json" <<'PY'
import json, sys
base, st = (json.load(open(p))["services"] for p in sys.argv[1:3])
problems = []
named = {n: s["container_name"] for n, s in st.items() if "container_name" in s}
base_named = {n for n, s in base.items() if "container_name" in s}
if set(named) != base_named: problems.append(f"container_name differs: {sorted(set(named) ^ base_named)}")
for n, c in named.items():
    if c != f"askbar-staging-{n}": problems.append(f"{n} is named {c}")
if len(named) != 8: problems.append(f"{len(named)} fixed container names, not 8")
ports = sorted((p["published"], p["target"]) for p in st["caddy"].get("ports", []))
if ports != [("8080", 80), ("8443", 443)]: problems.append(f"caddy ports are {ports}")
for n, s in st.items():
    if n != "caddy" and s.get("ports"): problems.append(f"{n} publishes a port")
def mounts(svc):
    return {(v["target"], bool(v.get("read_only")), v["type"]) for v in svc.get("volumes", [])}
b, s = mounts(base["server"]), mounts(st["server"])
if b != s: problems.append(f"the server's mounts differ from compose.yaml: {sorted(b ^ s)}")
src = {v["target"]: v["source"] for v in st["server"]["volumes"]}
if not src["/data"].endswith("data-staging"): problems.append(f"the server's data is {src['/data']}")
for n in base:
    if n not in ("caddy", "server") and {k: v for k, v in base[n].items() if k != "container_name"} != {k: v for k, v in st[n].items() if k != "container_name"}:
        problems.append(f"{n} changed beyond its container name")
print("\n".join(problems)); sys.exit(1 if problems else 0)
PY
expect "the staging override changes only names, Caddy's ports and the server's data directory" test $? -eq 0

# With the release file (from a fake metadata file), as the guide runs it.
python3 - "$T/meta.json" <<'PY'
import json, sys
json.dump({n: {"containerimage.digest": "sha256:" + format(i + 1, "x") * 64} for i, n in enumerate("bootstrap server admin auditor model-gateway alerter caddy cpa".split())}, open(sys.argv[1], "w"))
PY
scripts/release-compose.sh "$T/meta.json" "$T/compose.release.yaml" > /dev/null 2>&1
expect "the release file and the staging override merge" docker compose -p askbar-staging --project-directory . -f "$T/compose.release.yaml" -f compose.staging.yaml --profile dev config -q
if [ "$FAILS" = 0 ]; then echo "all staging override checks passed"; else echo "$FAILS check(s) FAILED"; exit 1; fi
