#!/bin/bash
# Check that the Launcher.exe inside each Windows installer and MSIX package is byte-identical to
# the signed copy it was built from, before they are signed themselves (RELEASING.md).
#
# Usage: scripts/check-installers.sh <folder with the installers> <folder with the signed exes>
#
# Pairs Launcher_X.Y.Z_<arch>-setup.exe with Launcher_X.Y.Z_<arch>-nsis.exe, and
# Launcher_X.Y.Z_<arch>_en-US.msi with Launcher_X.Y.Z_<arch>-msi.exe, and Launcher_X.Y.Z_<arch>.msix
# with Launcher_X.Y.Z_<arch>-nsis.exe (subfolders are searched, as `gh run download` leaves one per
# artifact). Needs 7z, which names the files inside an MSI by their WiX ids; Tauri's template gives
# the main binary the id "Path". The MSIX is read with Python's zipfile, which checks each entry's
# CRC: p7zip 16.02 reports "Headers Error" on makeappx's packages and exits 2. Nothing is unpacked
# to disk. setup.exe's signed uninstall.exe is checked on the Windows runner instead: this 7z can't
# rebuild it from the installer.
set -euo pipefail
[ "$#" -eq 2 ] || { sed -n '2,14p' "$0"; exit 1; }
command -v 7z > /dev/null || { echo "7z is not installed" >&2; exit 1; }
# Write one entry of a zip file to stdout.
unzip_entry() { python3 -I -c 'import sys, zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))' "$1" "$2"; }
fail=0
found=0
while IFS= read -r -d '' inst; do
  name=$(basename "$inst")
  case "$name" in
    *-setup.exe) exe="${name%-setup.exe}-nsis.exe"; inside=Launcher.exe ;;
    *_en-US.msi) exe="${name%_en-US.msi}-msi.exe"; inside=Path ;;
    *.msix) exe="${name%.msix}-nsis.exe"; inside=Launcher.exe ;;
    *) continue ;;
  esac
  found=$((found + 1))
  signed=$(find "$2" -type f -name "$exe" -print -quit)
  if [ -z "$signed" ]; then echo "FAIL $name: no $exe in $2"; fail=1; continue; fi
  case "$name" in
    *.msix) extract=(unzip_entry "$inst" "$inside") ;;
    *) extract=(7z e -so "$inst" "$inside") ;;
  esac
  if "${extract[@]}" 2> /dev/null | cmp -s - "$signed"; then
    echo "ok   $name contains $exe"
  else
    echo "FAIL $name: its Launcher.exe is not $exe"; fail=1
  fi
done < <(find "$1" -type f \( -name '*-setup.exe' -o -name '*_en-US.msi' -o -name '*.msix' \) -print0 | sort -z)
[ "$found" -gt 0 ] || { echo "no installers found in $1" >&2; exit 1; }
[ "$fail" -eq 0 ] || exit 1
echo "all $found installers and packages contain their signed exe"
