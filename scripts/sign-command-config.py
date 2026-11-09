#!/usr/bin/env python3
"""Write a Tauri config overlay that sets the Windows signing command to scripts/sign-command.ps1,
for `tauri build|bundle --config <file>` in CI (RELEASING.md). The script path is absolute, since
makensis runs the command for the uninstaller from another folder. The mode and folder go to the
script through SIGN_COMMAND_MODE and SIGN_COMMAND_DIR in the environment.

Usage: python scripts/sign-command-config.py <output .json>
"""
import json
import os
import sys

if len(sys.argv) != 2:
    sys.exit(__doc__)
script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sign-command.ps1")
config = {"bundle": {"windows": {"signCommand": {
    "cmd": "powershell.exe",
    "args": ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script, "%1"],
}}}}
with open(sys.argv[1], "x", encoding="utf-8") as f:
    json.dump(config, f)
print(f"{sys.argv[1]}: signCommand runs {script}")
