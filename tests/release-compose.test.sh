#!/usr/bin/env bash
# scripts/release-compose.sh against a fake bake metadata file: no `build:` left, every built service by digest, the other
# lines (container names, the dev profile's mock model) kept, a service without a digest refused, and the result parses with
# `docker compose config` (skipped, and said so, when docker compose is not installed; it needs no daemon).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
FAILS=0
# expect "what it shows" command args...: ok when the command succeeds.
expect() { local what=$1; shift; if "$@"; then echo "ok   $what"; else echo "FAIL $what"; FAILS=$((FAILS + 1)); fi; }
# refuse "what it shows" command args...: ok when the command fails.
refuse() { local what=$1; shift; if "$@"; then echo "FAIL $what"; FAILS=$((FAILS + 1)); else echo "ok   $what"; fi; }
count_is() { [ "$(grep -c "$2" "$3")" = "$1" ]; }

SERVICES=(bootstrap server admin auditor model-gateway alerter caddy cpa)
python3 - "$T/meta.json" "${SERVICES[@]}" <<'PY'
import json, sys
out, names = sys.argv[1], sys.argv[2:]
json.dump({n: {"containerimage.digest": "sha256:" + format(i + 1, "x") * 64, "image.name": "x"} for i, n in enumerate(names)}, open(out, "w"))
PY

run() { scripts/release-compose.sh "$@" > "$T/log" 2>&1; }
expect "it writes the file" run "$T/meta.json" "$T/out.yaml"
[ -f "$T/out.yaml" ] || { echo "$FAILS check(s) FAILED (no output to check)"; exit 1; }

refuse "no build block is left" grep -q '^    build:' "$T/out.yaml"
for s in "${SERVICES[@]}"; do
  expect "$s is set to an image by digest" grep -Eq "^    image: ghcr\.io/thewuuryar/askbar-$s@sha256:[0-9a-f]{64}$" "$T/out.yaml"
done
expect "exactly eight Askbar images are set by digest" count_is 8 '^    image: ghcr.io/thewuuryar/askbar-.*@sha256:' "$T/out.yaml"
refuse "no image line has a variable left" grep -q '^    image: .*ASKBAR_IMAGE_' "$T/out.yaml"
expect "every container_name is kept" count_is "$(grep -c '^    container_name:' compose.yaml)" '^    container_name:' "$T/out.yaml"
expect "the dev profile's mock model image is untouched" grep -q 'image: python:3.12-slim@sha256:' "$T/out.yaml"
expect "it lists the data directory bind mount it kept" grep -q 'DATA_DIR' "$T/log"
expect "it lists the mock model bind mount it kept" grep -q 'mock_llm.py' "$T/log"

# A service without a digest is refused, and nothing is written.
python3 - "$T/meta.json" "$T/short.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1])); m.pop("auditor"); json.dump(m, open(sys.argv[2], "w"))
PY
refuse "a service without a digest is refused" run "$T/short.json" "$T/short.yaml"
expect "and the refusal names it" grep -q 'no digest for auditor' "$T/log"
refuse "and nothing is written" test -e "$T/short.yaml"

# A malformed digest counts as missing.
python3 - "$T/meta.json" "$T/odd.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1])); m["server"]["containerimage.digest"] = "sha256:abc"; json.dump(m, open(sys.argv[2], "w"))
PY
refuse "a malformed digest is refused" run "$T/odd.json" "$T/odd.yaml"

if docker compose version > /dev/null 2>&1; then
  expect "docker compose config parses the result" docker compose -f "$T/out.yaml" --project-directory . config -q
  expect "and with the dev profile" docker compose -f "$T/out.yaml" --project-directory . --profile dev config -q
else
  echo "skip docker compose config: docker compose is not installed"
fi
if [ "$FAILS" = 0 ]; then echo "all release-compose checks passed"; else echo "$FAILS check(s) FAILED"; exit 1; fi
