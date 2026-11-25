#!/usr/bin/env bash
# Checks that the image the Dockerfile pins for SearXNG (the `searxng` stage) is the one scripts/third-party-notices.images.md
# names: the same tag and the same digest, and a source link at that version (ADR 0057, ADR 0017). It reads two files and
# needs no Docker and no network. Run it after changing the pinned image; `npm test` runs it.
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
exit $fail
