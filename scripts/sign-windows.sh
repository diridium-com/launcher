#!/bin/bash
# Sign Windows release files that CI built, inside a remotekey session (RELEASING.md).
#
# Usage: scripts/sign-windows.sh <CI's SHA-256 list> <new output folder> <file>...
#
#   <CI's SHA-256 list>  sha256sum-format lines printed by the CI run that built the files
#                        (scripts/ci-hashes.sh)
#   <new output folder>  must not exist; signed copies and their SHA256SUMS go here
#   <file>...            .exe, .msi or .msix files downloaded from that run
#
# Run from the repo root, where yubikey-pkcs11.cfg and certchain.pem are linked (gitignored),
# with JSIGN_JAR pointing at jsign 7.6 (its SHA-256 is checked against the value below).
# Test without the YubiKey: SIGN_TEST_P12=<PKCS#12 file> (alias "test") and SIGN_TEST_P12_PASS;
# the keystore password is asked for in place of the PIN.
#
# The files are copied into <output>.partial/ci-originals and nothing is signed unless every
# copy's SHA-256 matches CI's list. jsign signs second copies, all in one call, so the PIN is
# typed once. The PIN goes to jsign on its standard input (--storepass file:/dev/stdin): it is
# never exported or put on a command line. Afterwards every signed copy must verify, carry a
# DigiCert timestamp, be signed by Diridium Technologies, Inc. through SSL.com's intermediate
# (test runs: by the test certificate), and be the original plus a signature and nothing else
# (scripts/compare-signed.py). osslsigncode can't read MSIX, so for a .msix the signature jsign
# extracts is checked with openssl instead; Windows checks it is trusted when the draft is
# tested (test-windows-installers.yml). Only when every check passes is <output>.partial renamed
# to <output>, so a failed run leaves nothing where the upload step looks.
set -euo pipefail
usage() { awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "$0"; }
[ "$#" -ge 3 ] || { usage; exit 1; }
EXPECTED=$1; OUT=${2%/}; shift 2
PART="$OUT.partial"
HERE=$(cd "$(dirname "$0")" && pwd)
TSA=http://timestamp.digicert.com
SIGNER="Diridium Technologies, Inc."
INTERMEDIATE="SSL.com Code Signing Intermediate CA ECC R2"
# jsign 7.6 from Maven Central (its published SHA-1 843c90ab50bc0da16fc1aac9e2c6bba986dc4a36
# matched on download). A new jsign version is a deliberate change to this line, after the
# 14-day rule (scripts/dependency-ages.py checks its publish date).
JSIGN_VERSION=7.6
JSIGN_SHA256=6cf58aa080e7ff82656f320cff8d4a3bff30408df8766806dcba3740dd839fa5

: "${JSIGN_JAR:?set JSIGN_JAR to the jsign $JSIGN_VERSION jar}"
[ "$(sha256sum "$JSIGN_JAR" | cut -d' ' -f1)" = "$JSIGN_SHA256" ] \
  || { echo "$JSIGN_JAR is not jsign $JSIGN_VERSION (SHA-256 differs)" >&2; exit 1; }
for tool in java openssl osslsigncode python3; do
  command -v "$tool" > /dev/null || { echo "$tool is not installed" >&2; exit 1; }
done
for f in "$@"; do
  case "$f" in
    *.msi) python3 -I -c 'import olefile' 2> /dev/null \
             || { echo "python3 can't import olefile, which checking an .msi needs" >&2; exit 1; }
           break ;;
  esac
done
[ ! -e "$OUT" ] && [ ! -e "$PART" ] || { echo "$OUT or $PART already exists" >&2; exit 1; }

if [ -n "${SIGN_TEST_P12:-}" ]; then
  KEYSTORE=(--storetype PKCS12 --keystore "$SIGN_TEST_P12" --alias test)
  SIGNER=$(openssl pkcs12 -in "$SIGN_TEST_P12" -nokeys -passin env:SIGN_TEST_P12_PASS 2> /dev/null \
    | openssl x509 -noout -subject -nameopt multiline | sed -n 's/^ *commonName *= *//p') || SIGNER=
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

echo "== copying the files and checking every copy against CI's list"
mkdir "$PART" "$PART/ci-originals"
signed=()
for f in "$@"; do
  name=$(basename "$f")
  copy="$PART/ci-originals/$name"
  cp "$f" "$copy"
  want=$(awk -v n="$name" '{ sub(/^\*/, "", $2) } $2 == n { print $1 }' "$EXPECTED")
  [ "$(printf '%s\n' "$want" | grep -c .)" -eq 1 ] \
    || { echo "$name: not listed exactly once in $EXPECTED" >&2; exit 1; }
  have=$(sha256sum "$copy" | cut -d' ' -f1)
  [ "$have" = "$want" ] || { echo "$name: SHA-256 $have, CI says $want" >&2; exit 1; }
  echo "ok  $have  $name"
  cp "$copy" "$PART/$name"
  signed+=("$PART/$name")
done

echo "== signing ${#signed[@]} files in one jsign call"
unset PIN
read -rs -p "$PROMPT" PIN
echo
if [ -z "$PIN" ]; then
  unset PIN; echo "Nothing typed; nothing signed." >&2; exit 1
fi
if [ -z "${SIGN_TEST_P12:-}" ] && { [ "${#PIN}" -lt 6 ] || [ "${#PIN}" -gt 8 ]; }; then
  # A YubiKey PIV PIN is 6 to 8 characters; anything else would only use up a retry.
  unset PIN; echo "That is not 6 to 8 characters, so it can't be the PIN; nothing signed." >&2; exit 1
fi
printf '%s' "$PIN" | java -jar "$JSIGN_JAR" "${KEYSTORE[@]}" --storepass file:/dev/stdin \
  --alg SHA-256 --tsaurl "$TSA" --tsmode RFC3161 "${signed[@]}"
unset PIN

# The first matching line of a section of an osslsigncode report, without its label.
report_line() {  # $1 report, $2 section heading (regex), $3 label (regex)
  awk -v sec="$2" -v lab="$3" '$0 ~ sec { s = 1; next } s && $0 ~ lab { sub("^[ \t]*" lab "[ \t]*", ""); print; exit }' "$1"
}

echo "== checking the signed copies"
fail=0
for s in "${signed[@]}"; do
  orig="$PART/ci-originals/$(basename "$s")"
  case "$s" in *.msi) kind=msi ;; *.msix) kind=msix ;; *) kind=pe ;; esac
  report="$s.verify.txt"
  if [ "$kind" = msix ]; then
    java -jar "$JSIGN_JAR" extract --format DER "$s" > /dev/null
    sig="$s.sig"
    [ -s "$sig" ] || { echo "FAIL $s: no signature to extract"; fail=1; continue; }
    openssl pkcs7 -inform DER -in "$sig" -print_certs > "$sig.pem" 2> "$report" \
      || { echo "FAIL $s: the signature isn't readable PKCS#7 (see $report)"; fail=1; continue; }
    # One line per certificate the signature carries: subject CN, a tab, issuer CN.
    pairs=$(awk '/BEGIN CERTIFICATE/ { c = "" } { c = c $0 "\n" } /END CERTIFICATE/ { printf "%s", c | "openssl x509 -noout -subject -issuer -nameopt multiline"; close("openssl x509 -noout -subject -issuer -nameopt multiline") }' "$sig.pem" \
      | awk '/^subject=/ { w = "s" } /^issuer=/ { w = "i" } /^ *commonName *=/ { sub(/^ *commonName *= */, ""); if (w == "s") sub_cn = $0; else { print sub_cn "\t" $0 } }')
    printf '%s\n' "$pairs" >> "$report"
    # Here-strings, not pipes: under pipefail, grep -q quitting early can kill the writer.
    if [ -n "${SIGN_TEST_P12:-}" ]; then
      awk -F'\t' -v s="$SIGNER" '$1 == s { f = 1 } END { exit !f }' <<< "$pairs" \
        || { echo "FAIL $s: not signed by $SIGNER"; fail=1; }
    else
      awk -F'\t' -v s="$SIGNER" -v i="$INTERMEDIATE" '$1 == s && $2 == i { f = 1 } END { exit !f }' <<< "$pairs" \
        || { echo "FAIL $s: no certificate for $SIGNER issued by $INTERMEDIATE"; fail=1; }
      awk -F'\t' -v i="$INTERMEDIATE" '$1 == i { f = 1 } END { exit !f }' <<< "$pairs" \
        || { echo "FAIL $s: the signature doesn't carry $INTERMEDIATE"; fail=1; }
    fi
    oids=$(openssl asn1parse -inform DER -in "$sig" 2> /dev/null)
    # The timestamp token follows its content type; its signer's names come after that.
    awk '/id-smime-ct-TSTInfo/ { t = 1 } t && /:DigiCert, Inc\.$/ { f = 1; exit } END { exit !f }' <<< "$oids" \
      && grep -q '1.3.6.1.4.1.311.3.3.1' <<< "$oids" \
      || { echo "FAIL $s: no DigiCert RFC 3161 timestamp"; fail=1; }
    python3 -I -B "$HERE/compare-signed.py" msix "$orig" "$s" || fail=1
    continue
  fi
  if ! osslsigncode verify -in "$s" -CAfile "$CAFILE" -TSA-CAfile /etc/ssl/certs/ca-certificates.crt > "$report" 2>&1; then
    echo "FAIL $s: signature does not verify (see $report)"; fail=1; continue
  fi
  grep -q '^The signature is timestamped' "$report" || { echo "FAIL $s: no timestamp"; fail=1; }
  tsa=$(report_line "$report" '^Timestamp Verified by:' 'Issuer :')
  [[ "$tsa" == *"/O=DigiCert, Inc./"* ]] || { echo "FAIL $s: timestamp not from DigiCert ($tsa)"; fail=1; }
  subject=$(report_line "$report" "^Signer's certificate:" 'Subject:')
  # A field ends at the next "/" or the end of the line, so compare with a "/" appended.
  [[ "$subject/" == *"/CN=$SIGNER/"* ]] || { echo "FAIL $s: signed by $subject, not $SIGNER"; fail=1; }
  if [ -z "${SIGN_TEST_P12:-}" ]; then
    # Windows can't build the chain to SSL.com's root unless the signature carries the
    # intermediate as well as our certificate (jsign adds both from certchain.pem).
    issuer=$(report_line "$report" "^Signer's certificate:" 'Issuer :')
    [[ "$issuer/" == *"/CN=$INTERMEDIATE/"* ]] || { echo "FAIL $s: our certificate's issuer is $issuer"; fail=1; }
    awk -v i="/CN=$INTERMEDIATE/" '/^Number of certificates:/ { s = 1 } s && /Subject:/ && index($0 "/", i) { f = 1 } END { exit !f }' "$report" \
      || { echo "FAIL $s: the signature doesn't carry $INTERMEDIATE"; fail=1; }
  fi
  python3 -I -B "$HERE/compare-signed.py" "$kind" "$orig" "$s" || fail=1
done
if [ "$fail" -ne 0 ]; then
  echo "Some signed copies failed the checks; nothing to upload. Kept for inspection: $PART" >&2
  exit 1
fi
(cd "$PART" && shopt -s nullglob && files=(*.exe *.msi *.msix) && sha256sum -- "${files[@]}" > SHA256SUMS && cat SHA256SUMS)
mv "$PART" "$OUT"
echo "== all ${#signed[@]} signed, verified and unchanged apart from the signature: $OUT"
