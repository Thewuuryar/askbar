#!/bin/bash
# One vocabulary: client, server, admin (see docs/architecture.md). Reports the old words
# "mgmt", "management" and "device" in tracked files, except where scripts/names-allow.txt or a
# `names-ok` marker on the line allows them.
#   scripts/check-names.sh            list hits; exit 1 if there are any
#   ASKBAR_NAMES_ENFORCE=0 ...        list hits but exit 0
cd "$(dirname "$0")/.." || exit 2
ENFORCE="${ASKBAR_NAMES_ENFORCE:-1}"
allowed=$(sed -e 's/#.*//' -e 's/[[:space:]]*$//' scripts/names-allow.txt | grep -v '^$')
hits=$(git ls-files -z | xargs -0 grep -InHiE 'mgmt|management|device' 2>/dev/null \
  | grep -v 'names-ok' \
  | while IFS= read -r line; do
      file=${line%%:*}
      printf '%s\n' "$allowed" | grep -qxF "$file" || printf '%s\n' "$line"
    done)
if [ -n "$hits" ]; then
  printf '%s\n' "$hits"
  echo "$(printf '%s\n' "$hits" | wc -l) use(s) of an old word (use client, server or admin)"
  [ "$ENFORCE" = 1 ] && exit 1
  echo "(not enforced yet)"
  exit 0
fi
echo "names ok"
