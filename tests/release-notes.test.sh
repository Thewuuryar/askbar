#!/usr/bin/env bash
# scripts/release-notes.sh on the real notes of the version in package.json's release candidate: every relative link becomes
# an absolute link at the tag, every target exists, nothing relative is left, and a missing notes file, a missing link target
# or a bare #anchor is refused.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
T=$(mktemp -d); trap 'rm -rf "$T"; rm -f docs/releases/vtest.md' EXIT
FAILS=0
expect() { local what=$1; shift; if "$@"; then echo "ok   $what"; else echo "FAIL $what"; FAILS=$((FAILS + 1)); fi; }
refuse() { local what=$1; shift; if "$@"; then echo "FAIL $what"; FAILS=$((FAILS + 1)); else echo "ok   $what"; fi; }

for notes in docs/releases/v*.md; do
  tag=$(basename "$notes" .md)
  expect "$tag: the notes are rewritten" scripts/release-notes.sh "$tag" --out "$T/out.md"
  refuse "$tag: no relative link is left" grep -Eq '\]\((\.\./|\./|[a-z0-9_-]+/|#)' "$T/out.md"
  expect "$tag: it links to the changelog at the tag" grep -q "blob/$tag/docs/releases/changelog/CHANGELOG-$tag.md" "$T/out.md"
  expect "$tag: the changelog exists" test -f "docs/releases/changelog/CHANGELOG-$tag.md"
  scripts/release-notes.sh "$tag" --paths > "$T/paths"
  expect "$tag: it lists the paths" test -s "$T/paths"
  while read -r p; do expect "$tag: $p exists" test -e "$p"; done < "$T/paths"
done

refuse "a tag without notes is refused" scripts/release-notes.sh v0.0.0-none
printf '# x\n[a](../nothing.md)\n' > docs/releases/vtest.md
refuse "a link to a missing file is refused" scripts/release-notes.sh vtest
printf '# x\n[a](#here)\n' > docs/releases/vtest.md
refuse "a bare anchor is refused" scripts/release-notes.sh vtest
printf '# x\n[a](changelog/)\n[b](https://example.com/a)\n' > docs/releases/vtest.md
expect "a folder becomes a tree link" scripts/release-notes.sh vtest --out "$T/t.md"
expect "and the folder link is absolute" grep -q 'github.com/Thewuuryar/askbar/tree/vtest/docs/releases/changelog' "$T/t.md"
expect "an absolute link is left alone" grep -q '(https://example.com/a)' "$T/t.md"
if [ "$FAILS" = 0 ]; then echo "all release-notes checks passed"; else echo "$FAILS check(s) FAILED"; exit 1; fi
