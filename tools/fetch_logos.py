"""Builds logos.js from fastfetch's ASCII logos (MIT, github.com/fastfetch-cli/fastfetch).

    python tools/fetch_logos.py

Pinned to one fastfetch commit, so a rebuild gives the same file. fastfetch's logos color
their parts with $1, $2, $3... and write a literal $ as $$; here $1 becomes the palette's
accent, $2 its secondary, anything after that its text color. The os-release IDs on the left
pick the logo; ID_LIKE covers the rest (an Arch derivative without its own logo gets Arch's).
"""

from __future__ import annotations

import json
import re
import sys
import urllib.request
from pathlib import Path

COMMIT = "090a54751f637c811ee36fabe30967715c0cb2bf"
RAW = f"https://raw.githubusercontent.com/fastfetch-cli/fastfetch/{COMMIT}"

# os-release ID -> fastfetch logo file, and the distro's own colors (accent, secondary)
LOGOS = {
    "arch": ("a/arch", "#1793d1", "#4fb3e6"),
    "ubuntu": ("u/ubuntu", "#e95420", "#f2a17c"),
    "debian": ("d/debian", "#d70a53", "#e9799f"),
    "fedora": ("f/fedora", "#51a2da", "#3c6eb4"),
    "linuxmint": ("l/linuxmint", "#87cf3e", "#d8e8c8"),
    "manjaro": ("m/manjaro", "#35bf5c", "#2a9a4a"),
    "nixos": ("n/nixos", "#7ebae4", "#5277c3"),
    "opensuse-tumbleweed": ("o/opensuse_tumbleweed", "#73ba25", "#35b9ab"),
    "opensuse-leap": ("o/opensuse_leap", "#73ba25", "#35b9ab"),
    "opensuse": ("o/opensuse_leap", "#73ba25", "#35b9ab"),
    "gentoo": ("g/gentoo", "#9a8fe0", "#dddaec"),
    "endeavouros": ("e/endeavouros", "#7f3fbf", "#f15f5f"),
    "pop": ("p/pop", "#48b9c7", "#faa41a"),
    "cachyos": ("c/cachyos", "#00ccff", "#00b894"),
    "void": ("v/void", "#478061", "#abc2ab"),
    "kali": ("k/kali", "#367bf0", "#a7c1f5"),
    "zorin": ("z/zorin", "#15a6f0", "#8fd3f7"),
    "elementary": ("e/elementary", "#64baff", "#c6e4ff"),
    "alpine": ("a/alpine", "#0d597f", "#3fa9d6"),
    "rocky": ("r/rocky", "#10b981", "#7ce3bd"),
    "almalinux": ("a/almalinux", "#ff4649", "#86da2f"),
    "centos": ("c/centos", "#efa724", "#9c4b9c"),
    "garuda": ("g/garuda", "#a050f0", "#e05ac8"),
    "artix": ("a/artix", "#10a0cc", "#7fd0e8"),
    "mx": ("m/mx", "#5b91c9", "#b8d3ee"),
}


def fetch(path: str) -> str:
    with urllib.request.urlopen(f"{RAW}/{path}", timeout=30) as r:
        return r.read().decode("utf-8")


def convert(text: str) -> list[list[list[str]]]:
    """fastfetch logo text -> rows of [class, text] runs (k = accent, t = secondary, g = text)."""
    cls = {"1": "k", "2": "t"}
    rows, current = [], "k"
    for line in text.rstrip("\n").split("\n"):
        runs: list[list[str]] = []
        buf = ""
        i = 0
        while i < len(line):
            if line[i] == "$" and i + 1 < len(line):
                nxt = line[i + 1]
                if nxt == "$":
                    buf += "$"
                    i += 2
                    continue
                if nxt.isdigit():
                    if buf:
                        runs.append([current, buf])
                        buf = ""
                    current = cls.get(nxt, "g")
                    i += 2
                    continue
            buf += line[i]
            i += 1
        if buf:
            runs.append([current, buf])
        rows.append(runs)
    return rows


def main() -> int:
    out = Path(__file__).resolve().parent.parent / "logos.js"
    license_text = fetch("LICENSE").strip()
    data, colors = {}, {}
    for distro, (file, accent, secondary) in LOGOS.items():
        data[distro] = convert(fetch(f"src/logo/ascii/{file}.txt"))
        colors[distro] = [accent, secondary]
        print(f"  {distro:22} {file}  {len(data[distro])} rows")
    header = "\n".join("// " + l if l else "//" for l in license_text.splitlines())
    js = (f"// Distro logos from fastfetch (https://github.com/fastfetch-cli/fastfetch, commit {COMMIT[:12]}),\n"
          f"// converted by tools/fetch_logos.py. Their license:\n//\n{header}\n\n"
          f"window.DISTRO_LOGOS = {json.dumps(data, ensure_ascii=False, separators=(',', ':'))};\n"
          f"window.DISTRO_COLORS = {json.dumps(colors, separators=(',', ':'))};\n")
    out.write_text(js, encoding="utf-8", newline="\n")
    print(f"wrote {out} ({len(js) // 1024} kB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
