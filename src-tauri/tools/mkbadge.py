#!/usr/bin/env python3
"""Compose a launcher badge icon: white Phosphor glyph on a colored rounded
square, 256x256 PNG. Geometry matches the frontend's composeAndSave() in
app/pages/connections/[id].vue so bundled presets and user-composed icons look
identical.

Run from the REPO ROOT (it reads node_modules/@iconify-json/ph).  Requires
ImageMagick and Pillow is not needed.  Glyph names use Phosphor's fill weight,
e.g. "plug-fill" -- the bundled presets are all *-fill.

    python3 src-tauri/tools/mkbadge.py plug-fill '#64748B' src-tauri/resources/admin-icon.png
"""
import json, subprocess, sys, tempfile, os

glyph, color, out = sys.argv[1], sys.argv[2], sys.argv[3]
icons = json.load(open("node_modules/@iconify-json/ph/icons.json"))["icons"]
body = icons[glyph]["body"].replace("currentColor", "#ffffff")

svg = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">'
    f'<rect x="8" y="8" width="240" height="240" rx="58" fill="{color}"/>'
    f'<g transform="translate(48,48) scale(0.625)">{body}</g>'
    "</svg>"
)
with tempfile.NamedTemporaryFile("w", suffix=".svg", delete=False) as f:
    f.write(svg)
    tmp = f.name
subprocess.run(["magick", "-background", "none", tmp, "-resize", "256x256", out], check=True)
os.unlink(tmp)
print(f"wrote {out}: {glyph} on {color}")
