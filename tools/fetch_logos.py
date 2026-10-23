"""Builds logos.js from fastfetch's ASCII logos (MIT, github.com/fastfetch-cli/fastfetch).

    python tools/fetch_logos.py

Pinned to one fastfetch commit (one tarball download), so a rebuild gives the same file. Every
logo fastfetch has goes in, as its own text: $1, $2, $3... switch the color from there on and
$$ is a literal $, the same as logo.txt. Next to each logo go its own colors (fastfetch's ANSI
colors turned into #rrggbb for termwall's dark background) for logo_colors = "own".

The 25 logos in FEATURED keep the ids termwall has always used (os-release IDs) and their
hand-picked colors for the distro palette; every other logo gets its fastfetch name as an id
(lowercase, a dash for anything not a letter or digit). All of fastfetch's names for a logo
become aliases, so an os-release ID or ID_LIKE that fastfetch knows finds its logo.
"""

from __future__ import annotations

import io
import json
import re
import sys
import tarfile
import unicodedata
import urllib.request
from pathlib import Path

COMMIT = "090a54751f637c811ee36fabe30967715c0cb2bf"
TARBALL = f"https://codeload.github.com/fastfetch-cli/fastfetch/tar.gz/{COMMIT}"

# termwall id -> fastfetch logo file, and the distro's own colors (accent, secondary)
FEATURED = {
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
# ids the logo setting already uses for something else
RESERVED = {"auto", "windows", "tux", "none", "custom", "image"}
# fastfetch's first name isn't the one people would type
PREFERRED = {"chrom": "chromeos"}
# names fastfetch doesn't have: Vista and XP had the same waving flag as 7
EXTRA_ALIASES = {"windows-vista": "windows-7", "windows-xp": "windows-7"}

# fastfetch's named colors on termwall's dark background (black would vanish: a dark grey)
ANSI = {
    "BLACK": "#5c5c5c", "RED": "#e05252", "GREEN": "#5fb85f", "YELLOW": "#e0b43c",
    "BLUE": "#4a86e8", "MAGENTA": "#c062c8", "CYAN": "#3fb8c8", "WHITE": "#d4d4d4",
    "LIGHT_BLACK": "#8a8a8a", "LIGHT_RED": "#ff7b72", "LIGHT_GREEN": "#8be08b",
    "LIGHT_YELLOW": "#ffe27a", "LIGHT_BLUE": "#79a8ff", "LIGHT_MAGENTA": "#e38ce8",
    "LIGHT_CYAN": "#7ee0ec", "LIGHT_WHITE": "#f4f4f4",
}
XTERM16 = [ANSI[k] for k in ("BLACK", "RED", "GREEN", "YELLOW", "BLUE", "MAGENTA", "CYAN", "WHITE")] + \
          [ANSI["LIGHT_" + k] for k in ("BLACK", "RED", "GREEN", "YELLOW", "BLUE", "MAGENTA", "CYAN", "WHITE")]


def xterm256(n: int) -> str:
    if n < 16:
        return XTERM16[n]
    if n < 232:
        n -= 16
        lv = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (lv[n // 36], lv[n // 6 % 6], lv[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


def color(token: str) -> str | None:
    """One entry of fastfetch's .colors -> #rrggbb, or None for the terminal's text color."""
    m = re.fullmatch(r'FF_COLOR_FG_RGB\s*"(\d+);(\d+);(\d+)"', token)
    if m:
        return "#%02x%02x%02x" % tuple(min(255, int(x)) for x in m.groups())
    m = re.fullmatch(r'FF_COLOR_FG_256\s*"(\d+)"', token)
    if m:
        return xterm256(int(m.group(1)) % 256)
    m = re.fullmatch(r"FF_COLOR_FG_([A-Z_]+)", token)
    return ANSI.get(m.group(1)) if m else None


def slug(name: str) -> str:
    plain = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    return re.sub(r"[^a-z0-9]+", "-", plain.lower()).strip("-")


def entries(inc: str):
    """fastfetch's src/logo/ascii/?.inc -> (names, logo file stem, colors) per logo."""
    for block in re.findall(r"\{\s*\.names\s*=.*?\n    \},", inc, re.S):
        names = re.findall(r'"([^"]*)"', re.search(r"\.names\s*=\s*\{([^}]*)\}", block).group(1))
        stem = re.search(r"\.lines\s*=\s*FASTFETCH_DATATEXT_LOGO_([A-Z0-9_]+)", block).group(1).lower()
        cm = re.search(r"\.colors\s*=\s*\{(.*?)\}", block, re.S)
        tokens = [t.strip() for t in cm.group(1).split(",") if t.strip()] if cm else []
        yield names, stem, [color(t) for t in tokens]


def main() -> int:
    out = Path(__file__).resolve().parent.parent / "logos.js"
    with urllib.request.urlopen(TARBALL, timeout=60) as r:
        tar = tarfile.open(fileobj=io.BytesIO(r.read()), mode="r:gz")
    files = {}
    for m in tar.getmembers():
        rel = m.name.split("/", 1)[-1]
        if m.isfile() and (rel.startswith("src/logo/ascii/") or rel == "LICENSE"):
            files[rel] = tar.extractfile(m).read().decode("utf-8")
    texts = {Path(p).stem: (p, t.rstrip("\n").replace("\r", "")) for p, t in files.items() if p.endswith(".txt")}

    logos, own, colors, names, aliases = {}, {}, {}, {}, {}
    by_stem = {}
    for p in sorted(p for p in files if p.endswith(".inc")):
        for ff_names, stem, cols in entries(files[p]):
            by_stem.setdefault(stem, (ff_names, cols))
            key = next((s for s in map(slug, ff_names) if s and s not in RESERVED and s not in logos), None)
            key = PREFERRED.get(key, key)
            if key is None or stem not in texts:
                continue
            logos[key] = texts[stem][1]
            own[key] = cols
            names[key] = ff_names
    # the featured ids: termwall's own names for them, and their hand-picked colors
    for key, (file, accent, secondary) in FEATURED.items():
        stem = file.split("/")[1]
        ff_names, cols = by_stem[stem]
        for old in [k for k in logos if k != key and logos[k] == texts[stem][1] and k not in FEATURED]:
            del logos[old], own[old], names[old]  # under its termwall id instead
        logos[key], own[key], names[key] = texts[stem][1], cols, ff_names
        colors[key] = [accent, secondary]
    for key, cols in own.items():
        if key not in colors:  # the palette's two colors: the logo's first two that aren't plain text
            picked = [c for c in cols if c and c not in (ANSI["WHITE"], ANSI["LIGHT_WHITE"], ANSI["BLACK"])]
            if picked:
                colors[key] = [picked[0], picked[1] if len(picked) > 1 else picked[0]]
    for key, ff_names in names.items():
        for s in map(slug, ff_names):
            if s and s not in logos and s not in RESERVED:
                aliases.setdefault(s, key)
    for name, key in EXTRA_ALIASES.items():
        aliases.setdefault(name, key)

    order = list(FEATURED) + sorted(k for k in logos if k not in FEATURED)
    logos = {k: logos[k] for k in order}
    header = "\n".join("// " + l if l else "//" for l in files["LICENSE"].strip().splitlines())
    dump = lambda v: json.dumps(v, ensure_ascii=False, separators=(",", ":"))
    js = (f"// Logos from fastfetch (https://github.com/fastfetch-cli/fastfetch, commit {COMMIT[:12]}),\n"
          f"// converted by tools/fetch_logos.py. Their license:\n//\n{header}\n\n"
          "// id -> the logo's text ($1, $2... switch the color, $$ = $)\n"
          f"window.DISTRO_LOGOS = {dump(logos)};\n"
          "// id -> its own colors for $1, $2... (null = the text color)\n"
          f"window.LOGO_OWN_COLORS = {dump({k: own[k] for k in order})};\n"
          "// id -> accent and secondary for theme \"logo\" / \"distro\"\n"
          f"window.DISTRO_COLORS = {dump({k: colors[k] for k in order if k in colors})};\n"
          "// id -> fastfetch's names for it (termwall logos searches them)\n"
          f"window.LOGO_NAMES = {dump({k: names[k] for k in order})};\n"
          "// another name (os-release ID, fastfetch name) -> id\n"
          f"window.LOGO_ALIASES = {dump(dict(sorted(aliases.items())))};\n"
          f"window.LOGO_FEATURED = {dump(list(FEATURED))};\n")
    out.write_text(js, encoding="utf-8", newline="\n")
    print(f"wrote {out}: {len(logos)} logos, {len(aliases)} aliases, {len(js.encode()) // 1024} kB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
