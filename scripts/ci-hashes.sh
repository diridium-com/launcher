#!/bin/bash
# Print the SHA-256s a CI run's Windows jobs logged for the files they made, in sha256sum
# format, for scripts/sign-windows.sh to check the downloaded files against (RELEASING.md).
#
# Usage: scripts/ci-hashes.sh <run id> [owner/repo] > <list>
#
# Reads the logs of the run's Windows jobs with gh: the files to sign from the tag's build
# (Launcher_X.Y.Z_<arch>-nsis.exe, -msi.exe, -uninstall.exe), or the installers and packages from
# windows-installers.yml (-setup.exe, _en-US.msi, .msix).
# Windows' sha256sum may mark binary mode as " *name"; both forms are accepted. A name must end
# the hash's line or be followed by a space, so x.msix.sig is not taken for x.msix. Fails if a
# file is logged with two different hashes, or if nothing is found.
set -euo pipefail
RUN=${1:?usage: scripts/ci-hashes.sh <run id> [owner/repo]}
args=()
[ -n "${2:-}" ] && args=(-R "$2")
jobs=$(gh run view "$RUN" "${args[@]}" --json jobs -q '.jobs[] | select(.name | test("windows")) | .databaseId')
[ -n "$jobs" ] || { echo "run $RUN has no Windows jobs" >&2; exit 1; }
lines=$(for id in $jobs; do gh run view "$RUN" "${args[@]}" --job "$id" --log; done \
  | grep -oE '[0-9a-f]{64} [ *]Launcher_[0-9]+\.[0-9]+\.[0-9]+_(x64|arm64)(-nsis\.exe|-msi\.exe|-uninstall\.exe|-setup\.exe|_en-US\.msi|\.msix)([[:space:]]|$)' \
  | sed -E 's/^([0-9a-f]{64}) [ *]/\1  /; s/[[:space:]]+$//' | sort -u)
[ -n "$lines" ] || { echo "no file hashes found in run $RUN's Windows logs" >&2; exit 1; }
dups=$(printf '%s\n' "$lines" | awk '{ print $2 }' | sort | uniq -d)
[ -z "$dups" ] || { echo "logged with more than one hash: $dups" >&2; exit 1; }
printf '%s\n' "$lines"
