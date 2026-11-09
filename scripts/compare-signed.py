#!/usr/bin/env python3
"""Check that a signed Windows file is the unsigned file plus an Authenticode signature, and
nothing else changed.

Usage: compare-signed.py pe   <unsigned .exe> <signed .exe>
       compare-signed.py msi  <unsigned .msi> <signed .msi>
       compare-signed.py msix <unsigned .msix> <signed .msix>

pe: Launcher.exe, the NSIS uninstaller and setup.exe. Signing sets the PE header checksum and the
certificate table entry, pads the file with up to 7 zero bytes to an 8-byte boundary, and
appends the certificate table. Every other byte must match.

msi: signing adds the \\x05DigitalSignature stream (and \\x05MsiDigitalSignatureEx when
present). Every other stream must exist in both files with identical contents. Needs olefile.

msix: signing adds the AppxSignature.p7x entry and declares its content type in
[Content_Types].xml. Every other entry must exist in both files with identical contents, and
[Content_Types].xml may differ only by that one declaration.

Exits 0 and prints one line when the files match that way; otherwise exits 1 and says where
they differ. Used before a release's signed files replace the ones CI built (RELEASING.md).
"""
import struct
import sys


def pe_fields(data, name):
    """Return (checksum offset, certificate table entry offset, cert table offset, size)."""
    if data[:2] != b"MZ":
        sys.exit(f"{name}: not a PE file (no MZ header)")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        sys.exit(f"{name}: not a PE file (no PE signature)")
    opt = pe + 24
    magic = struct.unpack_from("<H", data, opt)[0]
    if magic not in (0x10B, 0x20B):
        sys.exit(f"{name}: unknown optional header magic {magic:#x}")
    checksum = opt + 64
    cert_entry = opt + (96 if magic == 0x10B else 112) + 4 * 8   # data directory 4
    cert_off, cert_size = struct.unpack_from("<II", data, cert_entry)
    return checksum, cert_entry, cert_off, cert_size


def blank(data, checksum, cert_entry):
    b = bytearray(data)
    b[checksum:checksum + 4] = b"\0" * 4
    b[cert_entry:cert_entry + 8] = b"\0" * 8
    return bytes(b)


def compare_pe(unsigned_path, signed_path):
    u = open(unsigned_path, "rb").read()
    s = open(signed_path, "rb").read()
    uc, ue, uoff, usize = pe_fields(u, unsigned_path)
    sc, se, soff, ssize = pe_fields(s, signed_path)
    if (uc, ue) != (sc, se):
        sys.exit("the two files' PE headers have different layouts")
    if uoff or usize:
        sys.exit(f"{unsigned_path} already carries a certificate table")
    if not soff or not ssize:
        sys.exit(f"{signed_path} has no certificate table: not signed")
    if soff + ssize != len(s):
        sys.exit(f"{signed_path}: the certificate table is not at the end of the file")
    body = blank(s[:soff], sc, se)
    base = blank(u, uc, ue)
    pad = len(body) - len(base)
    if not 0 <= pad < 8 or body[len(base):] != b"\0" * pad:
        sys.exit(f"{signed_path}: {pad} bytes between the original end and the certificate table, not 0 to 7 zero bytes")
    if body[:len(base)] != base:
        first = next(i for i in range(len(base)) if body[i] != base[i])
        sys.exit(f"the files differ at byte {first} (outside the checksum, the certificate table entry and the signature)")
    print(f"same apart from the signature: {len(u)} bytes of {unsigned_path} unchanged; "
          f"{signed_path} adds {pad} padding bytes and a {ssize}-byte certificate table")


SIGNATURE_STREAMS = {"\x05DigitalSignature", "\x05MsiDigitalSignatureEx"}


def compare_msi(unsigned_path, signed_path):
    import olefile
    def streams(path):
        with olefile.OleFileIO(path) as ole:
            return {"/".join(e): ole.openstream(e).read() for e in ole.listdir(streams=True, storages=False)}
    u, s = streams(unsigned_path), streams(signed_path)
    if SIGNATURE_STREAMS & set(u):
        sys.exit(f"{unsigned_path} already carries a signature stream")
    if "\x05DigitalSignature" not in s:
        sys.exit(f"{signed_path} has no \\x05DigitalSignature stream: not signed")
    extra = set(s) - set(u) - SIGNATURE_STREAMS
    missing = set(u) - set(s)
    changed = sorted(n for n in set(u) & set(s) if u[n] != s[n])
    problems = []
    if extra:
        problems.append("streams only in the signed file: " + ", ".join(sorted(map(repr, extra))))
    if missing:
        problems.append("streams missing from the signed file: " + ", ".join(sorted(map(repr, missing))))
    if changed:
        problems.append("streams that differ: " + ", ".join(map(repr, changed)))
    if problems:
        sys.exit("; ".join(problems))
    print(f"same apart from the signature: all {len(u)} streams of {unsigned_path} unchanged; "
          f"{signed_path} adds " + ", ".join(sorted(repr(n) for n in SIGNATURE_STREAMS & set(s))))


MSIX_SIGNATURE = "AppxSignature.p7x"
CONTENT_TYPES = "[Content_Types].xml"
SIGNATURE_TYPE = ("Override", "/AppxSignature.p7x", "application/vnd.ms-appx.signature")


def content_types(data):
    import xml.etree.ElementTree as ET
    root = ET.fromstring(data)
    out = []
    for e in root:
        tag = e.tag.split("}")[-1]
        out.append((tag, e.get("PartName") or e.get("Extension"), e.get("ContentType")))
    return out


def compare_msix(unsigned_path, signed_path):
    import zipfile
    with zipfile.ZipFile(unsigned_path) as uz, zipfile.ZipFile(signed_path) as sz:
        u = {n: uz.read(n) for n in uz.namelist()}
        s = {n: sz.read(n) for n in sz.namelist()}
    if MSIX_SIGNATURE in u:
        sys.exit(f"{unsigned_path} already carries {MSIX_SIGNATURE}")
    if MSIX_SIGNATURE not in s:
        sys.exit(f"{signed_path} has no {MSIX_SIGNATURE}: not signed")
    problems = []
    extra = set(s) - set(u) - {MSIX_SIGNATURE}
    missing = set(u) - set(s)
    changed = sorted(n for n in set(u) & set(s) if u[n] != s[n] and n != CONTENT_TYPES)
    if extra:
        problems.append("entries only in the signed file: " + ", ".join(sorted(extra)))
    if missing:
        problems.append("entries missing from the signed file: " + ", ".join(sorted(missing)))
    if changed:
        problems.append("entries that differ: " + ", ".join(changed))
    ut, st = content_types(u[CONTENT_TYPES]), content_types(s[CONTENT_TYPES])
    if st != ut + [SIGNATURE_TYPE]:
        problems.append(f"{CONTENT_TYPES} differs by more than the signature's content type")
    if problems:
        sys.exit("; ".join(problems))
    print(f"same apart from the signature: all {len(u)} entries of {unsigned_path} unchanged "
          f"apart from the {MSIX_SIGNATURE} declaration in {CONTENT_TYPES}; {signed_path} adds {MSIX_SIGNATURE} "
          f"({len(s[MSIX_SIGNATURE])} bytes)")


def main():
    modes = {"pe": compare_pe, "msi": compare_msi, "msix": compare_msix}
    if len(sys.argv) != 4 or sys.argv[1] not in modes:
        sys.exit(__doc__)
    modes[sys.argv[1]](sys.argv[2], sys.argv[3])


if __name__ == "__main__":
    main()
