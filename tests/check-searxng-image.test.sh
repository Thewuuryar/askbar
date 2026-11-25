#!/usr/bin/env bash
# scripts/check-searxng-image.sh --release: passes on this repo, fails on a copy whose searxng stage gains a RUN line.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
FAILS=0
expect() { local what=$1; shift; if "$@"; then echo "ok   $what"; else echo "FAIL $what"; FAILS=$((FAILS + 1)); fi; }
refuse() { local what=$1; shift; if "$@"; then echo "FAIL $what"; FAILS=$((FAILS + 1)); else echo "ok   $what"; fi; }
repo() { scripts/check-searxng-image.sh --release >/dev/null; }
expect "--release passes on the repo" repo
mkdir "$T/scripts"; cp -r Dockerfile compose.yaml docker "$T/"; cp scripts/check-searxng-image.sh scripts/third-party-notices.images.md "$T/scripts/"
check() { "$T/scripts/check-searxng-image.sh" "$@" >"$T/out" 2>&1; }
expect "the copy passes" check --release
sed -i '/^COPY --chmod=0755 docker\/searxng\/start.sh/a RUN sed -i s/a/b/ /usr/local/searxng/searx/webapp.py' "$T/Dockerfile"
refuse "a RUN line in the searxng stage fails --release" check --release
expect "the default mode ignores it" check
[ "$FAILS" = 0 ] && echo "all checks passed" || { echo "$FAILS check(s) FAILED"; exit 1; }
