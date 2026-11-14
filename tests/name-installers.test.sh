#!/usr/bin/env bash
# scripts/name-installers.sh on a fake artifact tree laid out as Tauri writes it: each installer gets
# Askbar_<version>_<os>_<arch>.<ext>, the unpacked app folders are left out, and an unknown architecture or a name clash is refused.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
FAILS=0
expect() { local what=$1; shift; if "$@"; then echo "ok   $what"; else echo "FAIL $what"; FAILS=$((FAILS + 1)); fi; }
refuse() { local what=$1; shift; if "$@"; then echo "FAIL $what"; FAILS=$((FAILS + 1)); else echo "ok   $what"; fi; }

mk() { mkdir -p "$(dirname "$T/in/$1")"; echo x > "$T/in/$1"; }
mk askbar-ubuntu-22.04/bundle/deb/Askbar_1.0.0_amd64.deb
mk askbar-ubuntu-22.04/bundle/rpm/Askbar-1.0.0-1.x86_64.rpm
mk askbar-ubuntu-22.04/bundle/appimage/Askbar_1.0.0_amd64.AppImage
mk askbar-ubuntu-22.04/bundle/appimage/Askbar.AppDir/usr/bin/askbar
mk askbar-windows-latest/bundle/msi/Askbar_1.0.0_x64_en-US.msi
mk askbar-windows-latest/bundle/nsis/Askbar_1.0.0_x64-setup.exe
mk askbar-macos-latest/bundle/dmg/Askbar_1.0.0_aarch64.dmg
mk askbar-macos-latest/bundle/macos/Askbar.app/Contents/MacOS/askbar

expect "it names the installers" scripts/name-installers.sh 1.0.0-rc.1 "$T/in" "$T/out" > "$T/log"
for want in Askbar_1.0.0-rc.1_linux_x64.deb Askbar_1.0.0-rc.1_linux_x64.rpm Askbar_1.0.0-rc.1_linux_x64.AppImage \
            Askbar_1.0.0-rc.1_windows_x64.msi Askbar_1.0.0-rc.1_windows_x64.exe Askbar_1.0.0-rc.1_macos_arm64.dmg; do
  expect "$want exists" test -f "$T/out/$want"
done
expect "and nothing else is copied" test "$(ls "$T/out" | wc -l)" = 6

mk askbar-ubuntu-22.04/bundle/deb/Askbar_1.0.0_riscv.deb
refuse "an unknown architecture is refused" scripts/name-installers.sh 1.0.0 "$T/in" "$T/out2"
rm "$T/in/askbar-ubuntu-22.04/bundle/deb/Askbar_1.0.0_riscv.deb"
mk askbar-ubuntu-22.04/bundle/deb2/Askbar_1.0.0_x86_64.deb
refuse "two installers with one name are refused" scripts/name-installers.sh 1.0.0 "$T/in" "$T/out3"
refuse "an empty folder is refused" scripts/name-installers.sh 1.0.0 "$T/none" "$T/out4"
if [ "$FAILS" = 0 ]; then echo "all name-installers checks passed"; else echo "$FAILS check(s) FAILED"; exit 1; fi
