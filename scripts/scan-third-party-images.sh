#!/usr/bin/env bash
# Scans every image the Dockerfile pins from outside (each `FROM <image>@sha256:...`: the Rust, Debian, Caddy, cpa and searxng bases)
# with Trivy, using the flags of the `images` job in .github/workflows/ci.yml and .trivyignore.yaml. Exits 1 on a HIGH or CRITICAL
# finding that has a fix. Needs trivy (v0.70.0 in CI) and network; Trivy pulls each digest itself. `npm test` does not run it.
#   scripts/scan-third-party-images.sh          scan them all
#   scripts/scan-third-party-images.sh --list   print the images it would scan, scan nothing
# A base's own findings also show in the CI scan of the image built from it; this checks the pins alone, e.g. before moving one.
set -u
cd "$(dirname "$0")/.."
images=$(sed -n 's/^FROM \([^ ]*@sha256:[0-9a-f]\{64\}\)\( .*\)\{0,1\}$/\1/p' Dockerfile | sort -u)
[ -n "$images" ] || { echo "no pinned image found in the Dockerfile" >&2; exit 1; }
if [ "${1:-}" = "--list" ]; then echo "$images"; exit 0; fi
[ $# -eq 0 ] || { echo "usage: $0 [--list]" >&2; exit 2; }
command -v trivy >/dev/null || { echo "trivy is not installed" >&2; exit 2; }
failed=0
for image in $images; do
  echo "== $image"
  trivy image --scanners vuln --severity HIGH,CRITICAL --ignore-unfixed --ignorefile .trivyignore.yaml \
    --exit-code 1 --no-progress "$image" || failed=1
done
exit $failed
