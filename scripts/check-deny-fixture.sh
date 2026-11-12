#!/usr/bin/env bash
# `cargo deny` must fail on a known-vulnerable dependency (ADR 0017, D30). tests/fixtures/deny holds a crate pinned to a release with a
# RustSec advisory (regex 1.5.4, RUSTSEC-2022-0013). If `cargo deny check advisories` passes on it, deny has stopped catching vulnerable
# dependencies, and its pass on the real ones means nothing.
#
#   scripts/check-deny-fixture.sh prepare   makes .deny-fixture/ (a crate and its lock file) for CI to run cargo-deny-action on
#   scripts/check-deny-fixture.sh           prepares it, runs the local cargo-deny on it and checks that it fails for the right reason
#
# The fixture is made inside the repository, so cargo deny finds our deny.toml in a parent directory, as it does for the real manifests.
set -euo pipefail
cd "$(dirname "$0")/.."
ADVISORY=RUSTSEC-2022-0013
DIR=.deny-fixture

prepare() {
  rm -rf "$DIR"
  mkdir -p "$DIR/src"
  cp tests/fixtures/deny/vulnerable.Cargo.toml "$DIR/Cargo.toml"
  cp tests/fixtures/deny/vulnerable.Cargo.lock "$DIR/Cargo.lock"
  : > "$DIR/src/lib.rs"
}

case "${1:-}" in
  prepare) prepare; echo "prepared $DIR" ;;
  "")
    prepare
    set +e
    out=$(cargo deny --manifest-path "$DIR/Cargo.toml" check advisories 2>&1)
    status=$?
    set -e
    rm -rf "$DIR"
    if [ "$status" = 0 ]; then
      echo "FAIL  cargo deny passed on a crate with a known vulnerability: it is not catching them" >&2
      exit 1
    fi
    if ! printf '%s' "$out" | grep -q "$ADVISORY"; then
      echo "FAIL  cargo deny failed on the fixture, but not for $ADVISORY:" >&2
      printf '%s\n' "$out" | tail -20 >&2
      exit 1
    fi
    echo "ok    cargo deny fails on the known-vulnerable fixture ($ADVISORY)"
    ;;
  *) echo "usage: $0 [prepare]" >&2; exit 2 ;;
esac
