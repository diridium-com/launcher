#!/bin/bash
# Sign Windows release files that CI built, inside a remotekey session (RELEASING.md).
#
# Usage: scripts/sign-windows.sh <CI's SHA-256 list> <new output folder> <file>...
#
#   <CI's SHA-256 list>  sha256sum-format lines printed by the CI run that built the files
#   <new output folder>  must not exist; signed copies and their SHA256SUMS go here
#   <file>...            .exe or .msi files downloaded from that run
#
# Run from the repo root, where yubikey-pkcs11.cfg and certchain.pem are linked (gitignored),
# with JSIGN_JAR pointing at a jsign jar that has a .sha256 file beside it.
# Test without the YubiKey: SIGN_TEST_P12=<PKCS#12 file> (alias "test"); its password is
# asked for in place of the PIN.
#
# Nothing is signed unless every file's SHA-256 matches CI's list. The originals are left as
# they are; jsign signs copies, all in one call, so the PIN is typed once. The PIN goes to
# jsign on its standard input (--storepass file:/dev/stdin): it is never exported or put on a
# command line. Afterwards every copy must verify, carry a DigiCert timestamp, be signed by
# Diridium Technologies, Inc. (test runs: by the test certificate), and be the original plus a
# signature and nothing else (scripts/compare-signed.py).
set -euo pipefail
[ "$#" -ge 3 ] || { sed -n '2,20p' "$0"; exit 1; }
EXPECTED=$1; OUT=$2; shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
TSA=http://timestamp.digicert.com
SIGNER="Diridium Technologies, Inc."

: "${JSIGN_JAR:?set JSIGN_JAR to the jsign jar}"
(cd "$(dirname "$JSIGN_JAR")" && sha256sum -c --quiet "$(basename "$JSIGN_JAR").sha256") \
  || { echo "jsign jar does not match its .sha256" >&2; exit 1; }
command -v osslsigncode > /dev/null || { echo "osslsigncode is not installed" >&2; exit 1; }

if [ -n "${SIGN_TEST_P12:-}" ]; then
  KEYSTORE=(--storetype PKCS12 --keystore "$SIGN_TEST_P12" --alias test)
  SIGNER=$(openssl pkcs12 -in "$SIGN_TEST_P12" -nokeys -passin env:SIGN_TEST_P12_PASS 2> /dev/null \
    | openssl x509 -noout -subject -nameopt multiline | sed -n 's/^ *commonName *= *//p')
  [ -n "$SIGNER" ] || { echo "set SIGN_TEST_P12_PASS so the test certificate's name can be read" >&2; exit 1; }
  CAFILE=$(mktemp)
  openssl pkcs12 -in "$SIGN_TEST_P12" -nokeys -passin env:SIGN_TEST_P12_PASS 2> /dev/null > "$CAFILE"
  cat /etc/ssl/certs/ca-certificates.crt >> "$CAFILE"
  PROMPT="Test keystore password: "
else
  for f in yubikey-pkcs11.cfg certchain.pem; do
    [ -e "$f" ] || { echo "$f not found here; run from the repo root with it linked in" >&2; exit 1; }
  done
  KEYSTORE=(--storetype PKCS11 --keystore yubikey-pkcs11.cfg
            --alias "Certificate for PIV Authentication" --certfile certchain.pem)
  CAFILE=/etc/ssl/certs/ca-certificates.crt
  PROMPT="YubiKey PIN: "
fi

echo "== checking every file against CI's list"
for f in "$@"; do
  name=$(basename "$f")
  want=$(awk -v n="$name" '{ sub(/^\*/, "", $2) } $2 == n { print $1 }' "$EXPECTED")
  [ "$(printf '%s\n' "$want" | grep -c .)" -eq 1 ] \
    || { echo "$name: not listed exactly once in $EXPECTED" >&2; exit 1; }
  have=$(sha256sum "$f" | cut -d' ' -f1)
  [ "$have" = "$want" ] || { echo "$name: SHA-256 $have, CI says $want" >&2; exit 1; }
  echo "ok  $have  $name"
done

mkdir "$OUT"
signed=()
for f in "$@"; do cp "$f" "$OUT/"; signed+=("$OUT/$(basename "$f")"); done

echo "== signing ${#signed[@]} files in one jsign call"
read -rs -p "$PROMPT" PIN
echo
printf '%s' "$PIN" | java -jar "$JSIGN_JAR" "${KEYSTORE[@]}" --storepass file:/dev/stdin \
  --alg SHA-256 --tsaurl "$TSA" --tsmode RFC3161 "${signed[@]}"
unset PIN

echo "== checking the signed copies"
fail=0
for f in "$@"; do
  s="$OUT/$(basename "$f")"
  case "$s" in *.msi) kind=msi ;; *) kind=pe ;; esac
  report="$s.verify.txt"
  if ! osslsigncode verify -in "$s" -CAfile "$CAFILE" -TSA-CAfile /etc/ssl/certs/ca-certificates.crt > "$report" 2>&1; then
    echo "FAIL $s: signature does not verify (see $report)"; fail=1; continue
  fi
  grep -q '^The signature is timestamped' "$report" || { echo "FAIL $s: no timestamp"; fail=1; }
  grep -q "Subject: .*CN=${SIGNER}" "$report" || { echo "FAIL $s: not signed by $SIGNER"; fail=1; }
  if [ -z "${SIGN_TEST_P12:-}" ]; then
    # Windows can't build the chain to SSL.com's root unless the signature carries the
    # intermediate as well as our certificate (jsign adds both from certchain.pem).
    certs=$(sed -n 's/^Number of certificates: *//p' "$report" | head -n 1)
    [ "${certs:-0}" -ge 2 ] || { echo "FAIL $s: the signature carries ${certs:-no} certificate(s); needs ours and the intermediate"; fail=1; }
  fi
  python3 -I -B "$HERE/compare-signed.py" "$kind" "$f" "$s" || fail=1
done
[ "$fail" -eq 0 ] || { echo "Some signed copies failed the checks; nothing to upload." >&2; exit 1; }
(cd "$OUT" && shopt -s nullglob && files=(*.exe *.msi) && sha256sum -- "${files[@]}" > SHA256SUMS && cat SHA256SUMS)
echo "== all ${#signed[@]} signed, verified and unchanged apart from the signature: $OUT"
