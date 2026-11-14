#!/usr/bin/env bash
# Copies the desktop installers of a release into one folder under one naming scheme:
#   Askbar_<version>_<os>_<architecture>.<extension>      for example Askbar_1.0.0-rc.1_windows_x64.exe
# <os> is linux, macos or windows (from the build's artifact folder, askbar-<runner>); <architecture> is x64 or arm64 (from the
# file Tauri wrote); <version> is the release's, which Tauri's own file names do not carry (they use 1.0.0 for 1.0.0-rc.1).
# Run by .github/workflows/images.yml. It fails if a file's architecture is unknown or two installers would get the same name.
#
# Usage: scripts/name-installers.sh <version> <artifacts-dir> <out-dir>
#   <artifacts-dir>  holds one folder for each build: askbar-ubuntu-22.04/, askbar-windows-latest/, askbar-macos-latest/
set -euo pipefail
VERSION=${1:?usage: scripts/name-installers.sh <version> <artifacts-dir> <out-dir>}
SRC=${2:?usage: scripts/name-installers.sh <version> <artifacts-dir> <out-dir>}
OUT=${3:?usage: scripts/name-installers.sh <version> <artifacts-dir> <out-dir>}
mkdir -p "$OUT"
found=0
while IFS= read -r -d '' f; do
  name=$(basename "$f")
  runner=${f#"$SRC"/}; runner=${runner%%/*}
  case "$runner" in
    askbar-ubuntu*) os=linux ;;
    askbar-macos*) os=macos ;;
    askbar-windows*) os=windows ;;
    *) echo "name-installers.sh: unknown build folder $runner for $f" >&2; exit 1 ;;
  esac
  # The architecture is a whole word in Tauri's name (separators are _ - . and the version in between).
  if [[ "$name" =~ (^|[_.-])(amd64|x86_64|x64)([_.-]|$) ]]; then arch=x64
  elif [[ "$name" =~ (^|[_.-])(aarch64|arm64)([_.-]|$) ]]; then arch=arm64
  else echo "name-installers.sh: cannot tell the architecture of $name" >&2; exit 1; fi
  ext=${name##*.}
  new="Askbar_${VERSION}_${os}_${arch}.${ext}"
  if [ -e "$OUT/$new" ]; then echo "name-installers.sh: two installers would be named $new (second: $f)" >&2; exit 1; fi
  cp "$f" "$OUT/$new"
  echo "$name -> $new"
  found=$((found + 1))
done < <(find "$SRC" -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*.dmg' -o -name '*.msi' -o -name '*.exe' \) -print0 | sort -z)
[ "$found" -gt 0 ] || { echo "name-installers.sh: no installer found under $SRC" >&2; exit 1; }
