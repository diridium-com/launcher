#!/usr/bin/env python3
"""Write a copy of the built Launcher.exe marked for one Windows installer type.

Usage: patch-bundle-type.py <built exe> <output exe> <nsis|msi>

Tauri marks the main binary with the installer it ships in: it replaces the placeholder
__TAURI_BUNDLE_TYPE_VAR_UNK with ..._NSS or ..._MSI just before building each installer
(tauri-bundler 2.10.4, bundle.rs, patch_binary), and the app reads it at run time. A signed
exe can't be changed afterwards, so a release marks one copy per installer type here, signs
the copies, and builds the installers from them with `tauri bundle --no-binary-patching`
(RELEASING.md).

Refuses unless the placeholder occurs exactly once, and never overwrites an existing file.
Prints the output's SHA-256.
"""
import hashlib
import os
import sys

PLACEHOLDER = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
MARKS = {"nsis": b"__TAURI_BUNDLE_TYPE_VAR_NSS", "msi": b"__TAURI_BUNDLE_TYPE_VAR_MSI"}


def main():
    if len(sys.argv) != 4 or sys.argv[3] not in MARKS:
        sys.exit(__doc__)
    src, dst, kind = sys.argv[1:]
    data = open(src, "rb").read()
    count = data.count(PLACEHOLDER)
    if count != 1:
        sys.exit(f"{src}: expected the placeholder {PLACEHOLDER.decode()} once, found it {count} times")
    out = data.replace(PLACEHOLDER, MARKS[kind])
    # "xb" fails if the file exists, so an earlier output is never overwritten.
    try:
        with open(dst, "xb") as f:
            f.write(out)
    except FileExistsError:
        sys.exit(f"{dst} already exists; not overwriting it")
    print(f"{hashlib.sha256(out).hexdigest()}  {os.path.basename(dst)}")


if __name__ == "__main__":
    main()
