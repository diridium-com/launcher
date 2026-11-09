#!/bin/bash
# Print the SHA-256s a CI run's Windows jobs logged for the files they made, in sha256sum
# format, for scripts/sign-windows.sh to check the downloaded files against (RELEASING.md).
#
# Usage: scripts/ci-hashes.sh <run id> <tag> [owner/repo] > <list>
#
# Refuses a run that isn't, for that tag, either the successful build of the tag push
# (build-launcher.yml) or a successful windows-installers.yml run dispatched with --ref <tag>:
# the run's workflow, event, ref, commit and result are compared with the tag. The files and
# the hashes both come from the run, so the hash check alone can't catch a wrong run.
#
# Reads the logs of the run's Windows jobs with gh: the files to sign from the tag's build
# (Launcher_X.Y.Z_<arch>-nsis.exe, -msi.exe, -uninstall.exe), or the installers and packages from
# windows-installers.yml (-setup.exe, _en-US.msi, .msix).
# Windows' sha256sum may mark binary mode as " *name"; both forms are accepted. A name must end
# the hash's line or be followed by a space, so x.msix.sig is not taken for x.msix. Fails if a
# file is logged with two different hashes, or if nothing is found.
set -euo pipefail
usage="usage: scripts/ci-hashes.sh <run id> <tag> [owner/repo]"
RUN=${1:?$usage}
TAG=${2:?$usage}
REPO=${3:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}
BUILD="Build on every push and every pull-request"
INSTALLERS="Windows installers from the signed exes"

[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "$TAG is not a vX.Y.Z tag" >&2; exit 1; }
# refs/tags/ only: commits/<name> would also resolve a branch of that name.
gh api "repos/$REPO/git/ref/tags/$TAG" --silent || { echo "$REPO has no tag $TAG" >&2; exit 1; }
tag_sha=$(gh api "repos/$REPO/commits/refs/tags/$TAG" --jq .sha) \
  || { echo "can't resolve tag $TAG in $REPO" >&2; exit 1; }
run=$(gh run view "$RUN" -R "$REPO" --json workflowName,event,headBranch,headSha,status,conclusion \
  -q '[.workflowName, .event, .headBranch, .headSha, .status, .conclusion] | join("\t")')
IFS=$'\t' read -r workflow event ref sha status conclusion <<< "$run"
case "$workflow|$event" in
  "$BUILD|push"|"$INSTALLERS|workflow_dispatch") ;;
  *) echo "run $RUN is '$workflow' ($event), not the tag's build or a windows-installers.yml run" >&2; exit 1 ;;
esac
[ "$ref" = "$TAG" ] || { echo "run $RUN ran on '$ref', not $TAG (dispatch windows-installers.yml with --ref $TAG)" >&2; exit 1; }
[ "$sha" = "$tag_sha" ] || { echo "run $RUN built $sha, but $TAG is $tag_sha" >&2; exit 1; }
[ "$status|$conclusion" = "completed|success" ] \
  || { echo "run $RUN is $status/$conclusion, not completed/success" >&2; exit 1; }
echo "run $RUN: '$workflow' for $TAG ($sha), succeeded" >&2

jobs=$(gh run view "$RUN" -R "$REPO" --json jobs -q '.jobs[] | select(.name | test("windows")) | .databaseId')
[ -n "$jobs" ] || { echo "run $RUN has no Windows jobs" >&2; exit 1; }
logs=$(for id in $jobs; do gh run view "$RUN" -R "$REPO" --job "$id" --log; done)
lines=$(grep -oE '[0-9a-f]{64} [ *]Launcher_[0-9]+\.[0-9]+\.[0-9]+_(x64|arm64)(-nsis\.exe|-msi\.exe|-uninstall\.exe|-setup\.exe|_en-US\.msi|\.msix)([[:space:]]|$)' <<< "$logs" \
  | sed -E 's/^([0-9a-f]{64}) [ *]/\1  /; s/[[:space:]]+$//' | sort -u) || lines=
[ -n "$lines" ] || { echo "no file hashes found in run $RUN's Windows logs" >&2; exit 1; }
dups=$(printf '%s\n' "$lines" | awk '{ print $2 }' | sort | uniq -d)
[ -z "$dups" ] || { echo "logged with more than one hash: $dups" >&2; exit 1; }
printf '%s\n' "$lines"
