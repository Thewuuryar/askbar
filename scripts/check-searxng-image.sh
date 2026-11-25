#!/usr/bin/env bash
# Checks that the image the Dockerfile pins for SearXNG (the `searxng` stage) is the one scripts/third-party-notices.images.md
# names: the same tag and the same digest, and a source link at that version (ADR 0057, ADR 0017). It reads two files and
# needs no Docker and no network. Run it after changing the pinned image; `npm test` runs it.
# `--release` also asserts the conditions the AGPL-3.0 reading rests on (an unmodified upstream image plus configuration only;
# ADR 0057, no counsel: owner decision 2026-11-25) and prints one line for each. CONTRIBUTING.md runs it before a release, and
# images.yml before a tag is published.
set -u
cd "$(dirname "$0")/.."
FROM=$(sed -n 's/^FROM \(docker\.io\/searxng\/searxng:[^ ]*\) AS searxng$/\1/p' Dockerfile)
[ -n "$FROM" ] || { echo "FAIL  no pinned searxng image found in the Dockerfile" >&2; exit 1; }
TAG=${FROM#*:}; TAG=${TAG%@*}; DIGEST=${FROM#*@}
fail=0
bad() { echo "FAIL  $1"; fail=1; }
ok() { echo "ok    $1"; }
echo "image: $FROM"
case "$FROM" in *:latest@*) bad "the tag is latest";; esac
[[ "$DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]] && ok "the digest is a sha256 of 64 hex digits" || bad "the digest '$DIGEST' is not sha256:<64 hex>"
ROW=$(grep '^| `askbar-searxng`' scripts/third-party-notices.images.md)
[ -n "$ROW" ] || bad "no askbar-searxng row in scripts/third-party-notices.images.md"
case "$ROW" in *"$DIGEST"*) ok "the notices row names the Dockerfile's digest";; *) bad "the notices row does not name the digest $DIGEST";; esac
case "$ROW" in *":$TAG\`"*) ok "the notices row names the tag $TAG";; *) bad "the notices row does not name the tag $TAG";; esac
case "$ROW" in *"https://github.com/searxng/searxng/tree/${TAG##*-}"*) ok "the notices row links the source at ${TAG##*-}";; *) bad "the notices row has no source link at commit ${TAG##*-}";; esac
case "$ROW" in *AGPL-3.0*) ok "the notices row names AGPL-3.0";; *) bad "the notices row does not name AGPL-3.0";; esac
[ "$(grep -c 'docker.io/searxng/searxng' Dockerfile)" = 1 ] && ok "the Dockerfile has one upstream searxng line" || bad "the Dockerfile names the upstream searxng image more than once"
[ "${1:-}" = --release ] || exit $fail
# Release mode: the AGPL reading's conditions.
STAGE=$(awk '/^FROM / { p = / AS searxng$/ } p && !/^[[:space:]]*(#|$)/' Dockerfile)
BADLINE=$(echo "$STAGE" | grep -vE '^(FROM|ENV|USER|ENTRYPOINT|CMD|LABEL|EXPOSE|HEALTHCHECK|WORKDIR|STOPSIGNAL) |^COPY (--chmod=[0-7]+ )?docker/searxng/(settings\.yml|start\.sh) [^ ]+$')
echo "$STAGE" | grep -q '^FROM ' || bad "release: no searxng stage in the Dockerfile"
[ -z "$BADLINE" ] && ok "release: the searxng stage is FROM, the two COPYs of our files and metadata lines only (no RUN, ADD or COPY --from)" || bad "release: the searxng stage has a line that may change upstream code: $BADLINE"
echo "$STAGE" | grep -q '^COPY .*settings\.yml' && echo "$STAGE" | grep -q '^COPY .*start\.sh' && ok "release: the stage copies settings.yml and start.sh" || bad "release: the stage does not copy both settings.yml and start.sh"
[ "$(ls docker/searxng | sort | tr '\n' ' ')" = "settings.yml start.sh " ] && ok "release: docker/searxng holds exactly settings.yml and start.sh" || bad "release: docker/searxng holds something besides settings.yml and start.sh"
case "$ROW" in *askbar-searxng*searxng/searxng:*AGPL-3.0*) ok "release: the notices row names the image and the AGPL-3.0 licence";; *) bad "release: the notices row does not name the image and AGPL-3.0";; esac
case "$ROW" in *"](https://github.com/searxng/searxng/"*"${TAG##*-}"*) ok "release: the notices source link matches the pinned version ${TAG##*-}";; *) bad "release: the notices source link does not contain ${TAG##*-}";; esac
SVC=$(awk '/^  searxng:/ { p = 1; next } p && /^  [a-z]/ { exit } p && !/^[[:space:]]*#/' compose.yaml)
[ -n "$SVC" ] && ! echo "$SVC" | grep -qE '^[[:space:]]+(args|volumes):' && ok "release: compose's searxng service has no build.args and no volumes" || bad "release: compose's searxng service is missing, or has build.args or volumes"
exit $fail
