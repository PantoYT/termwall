"""termwall_api — live system stats for the termwall wallpaper.

Read-only JSON on 127.0.0.1 only (never the LAN). A background thread samples
once per second, so a request never blocks on psutil or nvidia-smi.

    python termwall_api.py              # serve on 127.0.0.1:9002
    python termwall_api.py --once       # print one snapshot and exit
    python termwall_api.py --selftest
    python termwall_api.py --config                 # open termwall.toml (every setting, commented)
    python termwall_api.py --style                  # the look: layout, bars, rotation
    python termwall_api.py --style layout board     # change one of them (the page follows live)
    python termwall_api.py --link-we                # show up in Wallpaper Engine (a junction in myprojects)
    python termwall_api.py --link-lively            # the same for Lively Wallpaper (its library)
    python termwall_api.py --unlink-we | --unlink-lively | --stop | --version

Dependencies: psutil (CPU/RAM/disks/net/processes). The GPU comes from
nvidia-smi if it exists, else on Linux from the kernel's DRM interfaces (Intel i915/xe,
AMD amdgpu); without either the "gpu" field is null.
"""

from __future__ import annotations

import datetime
import hmac
import secrets
import getpass
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import threading
import time
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import psutil

__version__ = "2.3.0"

HOST, PORT = "127.0.0.1", 9002
HISTORY = 60  # seconds of CPU/GPU/net history for sparklines
CREATE_NO_WINDOW = 0x08000000 if os.name == "nt" else 0
# Where termwall keeps what it writes. A clone or the Windows installer: its own folder, as
# always. A system package (read-only /usr/share/termwall, marked by a SYSTEM-INSTALL file):
# the settings in $XDG_CONFIG_HOME/termwall, and token.js and share.json in
# $XDG_DATA_HOME/termwall, which is also where the wallpaper loads the page from (links to the
# packaged files, made by prepare_page_dir, so token.js sits next to index.html).
APP_DIR = os.path.dirname(os.path.abspath(__file__))
SYSTEM_INSTALL = os.path.exists(os.path.join(APP_DIR, "SYSTEM-INSTALL"))


def _xdg_dir(var: str, fallback: str) -> str:
    return os.path.join(os.environ.get(var) or os.path.join(os.path.expanduser("~"), fallback), "termwall")


CONFIG_DIR = _xdg_dir("XDG_CONFIG_HOME", ".config") if SYSTEM_INSTALL else APP_DIR
DATA_DIR = _xdg_dir("XDG_DATA_HOME", os.path.join(".local", "share")) if SYSTEM_INSTALL else APP_DIR
PAGE_ASSETS = ("index.html", "logos.js", "fonts")
# Optional palette written by an external switcher (e.g. rice). The page swaps its
# CSS variables when this changes — no wallpaper reload.
THEME_FILE = os.environ.get("TERMWALL_THEME") or os.path.join(CONFIG_DIR, "theme.json")
THEME_KEYS = ("bg", "accent", "secondary", "text", "dim", "faint")


def valid_theme(t) -> bool:
    return (isinstance(t, dict) and set(THEME_KEYS) <= set(t)
            and all(isinstance(t[k], str) and len(t[k]) == 7 and t[k][0] == "#"
                    and all(c in "0123456789abcdefABCDEF" for c in t[k][1:]) for k in THEME_KEYS))


# The look, from termwall.toml next to the server (or $TERMWALL_STYLE): a commented file that
# lists every setting and its choices, meant to be edited by hand. A "style" object inside
# theme.json wins over it, so a switcher (livery) can give every palette its own look. Every key
# is optional; anything unknown or invalid is dropped, so an old or hand-edited file can't break
# the page. termwall.json (before 1.2) is carried over once.
STYLE_FILE = os.environ.get("TERMWALL_STYLE") or os.path.join(CONFIG_DIR, "termwall.toml")
LAYOUTS = ("fetch", "board", "minimal", "htop", "portrait", "phone", "logo")
SECTIONS = ("prompt", "logo", "info", "clock", "cpu", "cores", "gpu", "memory", "disks", "network",
            "procs", "swatches", "music", "status")
FONTS = ("cascadia", "jetbrains", "fira", "iosevka", "consolas", "system")
EFFECTS = ("crt", "glow", "flicker", "roll", "chroma", "curve", "noise")

# Built-in palettes for when nothing writes theme.json: the six roles termwall uses, taken
# from each project's published palette (dark variants).
THEMES = {
    "mint":         {"bg": "#171717", "accent": "#b5f4e7", "secondary": "#7faca3", "text": "#939fa1", "dim": "#4b5252", "faint": "#242626"},
    "catppuccin":   {"bg": "#1e1e2e", "accent": "#cba6f7", "secondary": "#89b4fa", "text": "#cdd6f4", "dim": "#6c7086", "faint": "#313244"},
    "gruvbox":      {"bg": "#282828", "accent": "#fabd2f", "secondary": "#8ec07c", "text": "#ebdbb2", "dim": "#928374", "faint": "#3c3836"},
    "nord":         {"bg": "#2e3440", "accent": "#88c0d0", "secondary": "#81a1c1", "text": "#d8dee9", "dim": "#4c566a", "faint": "#3b4252"},
    "dracula":      {"bg": "#282a36", "accent": "#bd93f9", "secondary": "#ff79c6", "text": "#f8f8f2", "dim": "#6272a4", "faint": "#44475a"},
    "tokyo-night":  {"bg": "#1a1b26", "accent": "#7aa2f7", "secondary": "#bb9af7", "text": "#c0caf5", "dim": "#565f89", "faint": "#24283b"},
    "rose-pine":    {"bg": "#191724", "accent": "#ebbcba", "secondary": "#c4a7e7", "text": "#e0def4", "dim": "#6e6a86", "faint": "#26233a"},
    "everforest":   {"bg": "#2d353b", "accent": "#a7c080", "secondary": "#83c092", "text": "#d3c6aa", "dim": "#859289", "faint": "#343f44"},
    "kanagawa":     {"bg": "#1f1f28", "accent": "#7e9cd8", "secondary": "#957fb8", "text": "#dcd7ba", "dim": "#727169", "faint": "#2a2a37"},
    "solarized":    {"bg": "#002b36", "accent": "#268bd2", "secondary": "#2aa198", "text": "#93a1a1", "dim": "#586e75", "faint": "#073642"},
    "one-dark":     {"bg": "#282c34", "accent": "#61afef", "secondary": "#c678dd", "text": "#abb2bf", "dim": "#5c6370", "faint": "#2c313a"},
    "monokai":      {"bg": "#272822", "accent": "#a6e22e", "secondary": "#f92672", "text": "#f8f8f2", "dim": "#75715e", "faint": "#3e3d32"},
    "amber":        {"bg": "#120d02", "accent": "#ffb000", "secondary": "#cc8c00", "text": "#e0a020", "dim": "#6b4a00", "faint": "#241a04"},
    "phosphor":     {"bg": "#050f07", "accent": "#33ff66", "secondary": "#1fbf4a", "text": "#2fe05a", "dim": "#145c27", "faint": "#0a1f0e"},
}


def _choice(*allowed):
    def ok(v):
        return v if v in allowed else None
    ok.help = " | ".join(allowed)
    return ok


def _int(lo, hi):
    def ok(v):
        return v if isinstance(v, int) and not isinstance(v, bool) and lo <= v <= hi else None
    ok.help = f"{lo}-{hi}"
    return ok


def _float(lo, hi):
    def ok(v):
        return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) and lo <= v <= hi else None
    ok.help = f"{lo}-{hi}"
    return ok


def _bool(v):
    return v if isinstance(v, bool) else None
_bool.help = "on | off"


def _subset(*allowed):
    def ok(v):
        return [x for x in dict.fromkeys(v) if x in allowed] if isinstance(v, list) else None
    ok.help = "a list from: " + ", ".join(allowed) + " (comma-separated; none = empty)"
    return ok


def _text(v):
    """The prompt's command: printable text, up to 60 characters."""
    return v if isinstance(v, str) and len(v) <= 60 and v.isprintable() else None
_text.help = "any text up to 60 characters"


def _logo(v):
    import re
    if not isinstance(v, str) or not re.fullmatch(r"[a-z0-9-]{1,40}", v):
        return None
    if v in ("auto", "windows", "tux", "none", "custom", "image"):
        return v
    cat = logo_catalog()
    return v if not cat or v in cat["logos"] or v in cat["aliases"] else None  # no logos.js: can't tell
_logo.help = ("auto | windows | tux | none | custom (logo.txt) | image (logo.gif/png/jpg/webp) | "
              "any logo id (termwall logos)")


def _logo_colors(v):
    import re
    if isinstance(v, str) and "#" in v:
        v = [x.strip() for x in v.split(",") if x.strip()]
    if isinstance(v, list):
        return v if 0 < len(v) <= 9 and all(isinstance(c, str) and re.fullmatch(r"#[0-9a-fA-F]{6}", c) for c in v) else None
    return v if v in ("auto", "own", "palette", "image") else None
_logo_colors.help = "auto | own | palette | up to nine #rrggbb for $1, $2... (comma-separated)"


def _palette(v):
    return {k: v[k] for k in THEME_KEYS} if valid_theme(v) else None
_palette.help = "six #rrggbb colors (bg, accent, secondary, text, dim, faint): edit termwall.toml"

STYLE_SPEC = {
    "layout": _choice(*LAYOUTS),                        # fetch / board / minimal / htop / portrait
    "bars": _choice("blocks", "shade", "dots", "line"),
    "rotate": _int(0, 1440),                            # minutes between layouts, 0 = off
    "theme": _choice("auto", "logo", "distro", *THEMES),                  # a built-in palette (auto = distro colors / mint)
    "colors": _palette,                                 # your own palette (wins over "theme")
    "clock": _choice("24h", "12h"),
    "seconds": _bool,
    "date": _choice("long", "short", "iso", "none"),
    "hide": _subset(*SECTIONS),
    "prompt": _text,
    "prompt_color": _choice("text", "accent", "secondary", "dim"),
    "logo_width": _int(12, 120),
    "logo_chars": _choice("blocks", "ascii"),
    "logo_colors": _logo_colors,
    "private": _bool,                                   # hide user, computer name and local IP
    "font": _choice(*FONTS),
    "scale": _float(0.8, 1.1),
    "effects": _subset(*EFFECTS),
    "logo": _logo,
}
STYLE_DEFAULTS = {"layout": "fetch", "bars": "blocks", "rotate": 0, "theme": "auto", "clock": "24h",
                  "seconds": True, "date": "long", "hide": [], "private": False, "font": "cascadia",
                  "scale": 1.0, "effects": [], "logo": "auto", "prompt_color": "text",
                  "logo_width": 40, "logo_chars": "blocks", "logo_colors": "auto"}


def clean_style(raw) -> dict:
    """Only known keys with valid values."""
    out: dict = {}
    if not isinstance(raw, dict):
        return out
    for k, check in STYLE_SPEC.items():
        if k in raw:
            v = check(raw[k])
            if v is not None:
                out[k] = v
    return out


def _json_file(path: str):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def screen_tables(own) -> dict:
    """[screen.N] tables of termwall.toml: {"2": {...}}, each cleaned like the main settings."""
    raw = own.get("screen") if isinstance(own, dict) else None
    if not isinstance(raw, dict):
        return {}
    return {str(n): clean_style(t) for n, t in raw.items() if str(n).isdigit() and isinstance(t, dict)}


def read_style(style_path: str | None = None, theme_path: str | None = None, screen: str | None = None) -> dict:
    """defaults < termwall.toml < theme.json's "style" (livery, per pack) < [screen.N] for the
    monitor asking (Wallpaper Engine's "termwall screen" property)."""
    style_path = style_path or STYLE_FILE
    theme_path = theme_path or THEME_FILE
    themed = _json_file(theme_path)
    own = load_config(style_path)[0]
    if own is None and style_path.endswith(".toml") and not os.path.exists(style_path):
        own = _json_file(os.path.join(os.path.dirname(style_path), "termwall.json"))  # not carried over yet
    return {**STYLE_DEFAULTS, **clean_style(own),
            **clean_style(themed.get("style") if isinstance(themed, dict) else None),
            **(screen_tables(own).get(str(screen)) or {} if screen else {})}


def screen_of(path_qs: str) -> str | None:
    from urllib.parse import parse_qs, urlsplit
    v = parse_qs(urlsplit(path_qs).query).get("screen", [""])[0]
    return v if v.isdigit() and len(v) <= 2 else None


def resolve_palette(style: dict, theme_path: str | None = None) -> dict | None:
    """Your own "colors" > a built-in "theme" you picked > theme.json (livery or another
    switcher) > None (the page then uses the distro's colors on Linux, its defaults on Windows).
    theme "auto" is what lets a switcher color termwall."""
    if style.get("colors"):
        return style["colors"]
    if style.get("theme", "auto") in ("logo", "distro"):
        return None  # the page colors itself from the logo (a picture's colors, or the distro's)
    if style.get("theme", "auto") != "auto":
        return THEMES.get(style["theme"])
    return read_theme(theme_path) if theme_path else read_theme()


LOGO_FILE = os.path.join(os.path.dirname(STYLE_FILE), "logo.txt")
_logo_cache: dict = {"mtime": None, "text": None}


def read_logo(path: str | None = None) -> str | None:
    """logo.txt for logo = "custom": up to 40 lines of 120 characters, tabs as spaces. Re-read
    only when it changes."""
    path = path or LOGO_FILE
    try:
        mtime = os.stat(path).st_mtime_ns
    except OSError:
        return None
    if path != _logo_cache.get("path") or mtime != _logo_cache["mtime"]:
        try:
            with open(path, encoding="utf-8-sig", errors="replace") as f:
                lines = f.read().expandtabs(4).splitlines()[:40]
            text = "\n".join(line.rstrip()[:120] for line in lines).strip("\n") or None
        except OSError:
            text = None
        _logo_cache.update(path=path, mtime=mtime, text=text)
    return _logo_cache["text"]


IMAGE_TYPES = {".gif": "image/gif", ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
               ".webp": "image/webp"}


def logo_image(folder: str | None = None) -> str | None:
    """logo.gif / .png / .jpg / .jpeg / .webp next to termwall.toml (the first found), up to 8 MB."""
    folder = folder or os.path.dirname(STYLE_FILE)
    for ext in IMAGE_TYPES:
        f = os.path.join(folder, "logo" + ext)
        try:
            if os.path.getsize(f) <= 8 * 1024 * 1024:
                return f
        except OSError:
            pass
    return None


def page_extras(st: dict) -> dict:
    """What the page needs besides stats, style and palette: the custom logo when it's used,
    or which logo image to load (its mtime, so an edited file is fetched again)."""
    if st.get("logo") == "custom":
        return {"logo_text": read_logo()}
    if st.get("logo") == "image":
        f = logo_image()
        return {"logo_image": os.stat(f).st_mtime_ns if f else None}
    return {}


def logo_image_bytes() -> tuple[bytes, str] | None:
    f = logo_image()
    if not f:
        return None
    try:
        with open(f, "rb") as fh:
            return fh.read(), IMAGE_TYPES[os.path.splitext(f)[1].lower()]
    except OSError:
        return None


def parse_value(key: str, value: str):
    """A command-line string -> the type the setting takes."""
    check = STYLE_SPEC[key]
    if check is _bool:
        v = value.lower()
        if v in ("on", "true", "yes", "1"):
            return True
        if v in ("off", "false", "no", "0"):
            return False
        return value
    if getattr(check, "help", "").startswith("a list"):
        return [] if value.lower() in ("none", "") else [x.strip() for x in value.split(",") if x.strip()]
    if key == "rotate":
        return int(value) if value.isdigit() else value
    if key == "scale":
        try:
            return float(value)
        except ValueError:
            return value
    return value


# What each setting does, for the comments in termwall.toml (the choices come from STYLE_SPEC).
STYLE_DOCS = {
    "layout": "How the screen is laid out.\n"
              "fetch = the fastfetch screen, board = boxes like btop, htop = meters and a big process\n"
              "table, minimal = the clock and one line, portrait = for a monitor turned on its side,\n"
              "phone = big gauges for a phone (termwall share uses it when the phone is upright),\n"
              "logo = the logo as big as the screen, with who you are under it (a second monitor)",
    "bars": "How meters are drawn: blocks ████, shade ▓▓░░, dots ■■··, line ━━──",
    "rotate": "Minutes between layouts (fetch, board, minimal, htop, again); 0 = off",
    "theme": "A built-in palette. auto = follow livery (or another switcher) if it runs, else the\n"
             "logo's colors, else mint. logo = always the logo's colors: a picture's own (logo =\n"
             "\"image\") or the shown distro's (logo = \"arch\" on Windows gives Arch's); distro is\n"
             "the same. Any theme but auto wins over livery; \"colors\" below wins over this.\n"
             "termwall themes shows them",
    "colors": "Your own palette: six #rrggbb colors. Wins over \"theme\" and over livery.",
    "clock": "24h or 12h (am/pm)",
    "seconds": "Seconds under the clock",
    "date": "long = monday 5 october, short = mon 5 oct, iso = 2026-10-05, none = no date",
    "hide": "Sections to hide, e.g. [\"gpu\", \"procs\"]; [] shows everything",
    "prompt": "The command shown in the prompt line, up to 60 characters",
    "prompt_color": "The color of that command: one of the palette's colors",
    "private": "true hides your user name, computer name and local IP (for screenshots and streams)",
    "scale": "Text size, 0.8 (smaller, fits more) to 1.1 (bigger)",
    "effects": "Any mix, e.g. [\"crt\", \"glow\"]; [] = none.\n"
               "crt = scanlines and a dark vignette, glow = soft light around text, flicker = a faint\n"
               "unsteady brightness, roll = a slow band rolling down the screen, chroma = red/cyan\n"
               "color fringes, curve = rounded tube corners, noise = film grain",
    "logo": "The logo. auto = your OS; or any of fastfetch's logos whatever the OS (macOS, ChromeOS,\n"
            "Windows 95 to 11, the BSDs, hundreds of distros: termwall logos lists them, termwall\n"
            "logos mac finds the macOS ones, termwall logos macos shows one); custom = your own,\n"
            "from logo.txt next to this file (up to 40 lines; $1 accent, $2 secondary, $3 text,\n"
            "$4 dim switch the color from there on; $$ is a $; termwall logo-from PICTURE makes one,\n"
            "its colors going to logo_colors); image = a picture or an animated\n"
            "GIF turned into text: logo.gif, .png, .jpg or .webp next to this file",
    "logo_width": "logo = image: how many characters wide",
    "logo_chars": "logo = image: blocks = half blocks, two pixels per character (best for pixel art);\n"
                  "ascii = characters from light to dense",
    "logo_colors": "The logo's colors, apart from the rest of the screen (theme, livery and \"colors\"\n"
                   "stay as they are). own = the logo's own: fastfetch's (a rainbow apple for\n"
                   "\"apple\"), a picture's; palette = the palette's (a picture's light and dark\n"
                   "turned into it); a list = your own colors for $1, $2... of a text logo,\n"
                   "e.g. [\"#e05252\", \"#5fb85f\"]; auto = a picture in its own, text logos in the palette",
}
STYLE_DOCS["font"] = ("The font. cascadia comes with Windows 11; jetbrains, fira and iosevka come with termwall;\n"
            "consolas and system (the browser's monospace) are always there")
CONFIG_DEFAULTS = {**STYLE_DEFAULTS, "prompt": "fastfetch --live"}


def _toml_value(v) -> str:
    """A TOML value for the types settings take. JSON strings are valid TOML basic strings."""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return repr(v)
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return "[" + ", ".join(_toml_value(x) for x in v) + "]"
    if isinstance(v, dict):
        return "{ " + ", ".join(f"{k} = {_toml_value(x)}" for k, x in v.items()) + " }"
    raise TypeError(type(v))


_catalog_cache: dict = {}


def logo_catalog(path: str | None = None) -> dict:
    """The logos termwall has (fastfetch's, from logos.js next to the page): {"logos": id -> text,
    "own": id -> colors, "names": id -> fastfetch's names, "aliases": name -> id, "featured": [ids]}.
    Empty when logos.js can't be read."""
    path = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), "logos.js")
    try:
        mtime = os.stat(path).st_mtime_ns
        if _catalog_cache.get("key") != (path, mtime):
            with open(path, encoding="utf-8") as f:
                text = f.read()
            dec, out = json.JSONDecoder(), {}
            for name, var in (("logos", "DISTRO_LOGOS"), ("own", "LOGO_OWN_COLORS"), ("names", "LOGO_NAMES"),
                              ("aliases", "LOGO_ALIASES"), ("featured", "LOGO_FEATURED")):
                i = text.find(f"window.{var} = ")
                out[name] = dec.raw_decode(text, i + len(f"window.{var} = "))[0] if i >= 0 else {}
            _catalog_cache.update(key=(path, mtime), data=out)
        return _catalog_cache["data"]
    except (OSError, ValueError):
        return {}


def distro_ids(path: str | None = None) -> list[str]:
    """The ids of the logos termwall has."""
    return list(logo_catalog(path).get("logos", {}))


def logo_slug(name: str) -> str:
    import re
    import unicodedata
    plain = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    return re.sub(r"[^a-z0-9]+", "-", plain.lower()).strip("-")


def find_logos(words: list[str], path: str | None = None) -> list[str]:
    """termwall logos WORD...: the ids whose id or fastfetch names contain every word."""
    cat = logo_catalog(path)
    words = [logo_slug(w) for w in words if logo_slug(w)]
    hay = {k: " ".join([k, *map(logo_slug, cat["names"].get(k, []))]) for k in cat.get("logos", {})}
    return [k for k, h in hay.items() if all(w in h for w in words)]


def logo_preview(key: str, path: str | None = None) -> str:
    """One logo in this terminal, in its own colors (24-bit ANSI)."""
    cat = logo_catalog(path)
    return ansi_logo(cat["logos"][key], cat["own"].get(key) or [])


def ansi_logo(text: str, own: list) -> str:
    """Logo text ($1..$9, $$) -> 24-bit ANSI with these colors for $1, $2... (None = the terminal's)."""
    def sgr(n):
        c = own[n - 1] if 0 < n <= len(own) else None
        return f"\x1b[38;2;{int(c[1:3], 16)};{int(c[3:5], 16)};{int(c[5:7], 16)}m" if c else "\x1b[39m"
    out, cur = [], 1
    for line in text.split("\n"):
        s, i = sgr(cur), 0
        while i < len(line):
            if line[i] == "$" and i + 1 < len(line) and line[i + 1] in "123456789":
                cur = int(line[i + 1])
                s += sgr(cur)
                i += 2
                continue
            if line.startswith("$$", i):
                i += 1
            s += line[i]
            i += 1
        out.append(s + "\x1b[0m")
    return "\n".join(out)


# --- termwall logo-from PICTURE: a picture -> logo.txt ($1..$9) and its colors ---

def read_png(data: bytes) -> tuple[int, int, list]:
    """A PNG -> (width, height, rows of (r, g, b, a)). Every color type and bit depth, not interlaced."""
    import struct
    import zlib
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    pos, idat, plte, trns, hdr = 8, b"", b"", b"", None
    while pos + 8 <= len(data):
        n, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + n]
        pos += 12 + n
        if kind == b"IHDR":
            hdr = struct.unpack(">IIBBBBB", body)
        elif kind == b"PLTE":
            plte = body
        elif kind == b"tRNS":
            trns = body
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    if not hdr:
        raise ValueError("a PNG without a header")
    w, h, depth, ctype, _, _, interlace = hdr
    if interlace:
        raise ValueError("an interlaced PNG: save it again without interlacing")
    chans = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    bpp = max(1, chans * depth // 8)
    stride = (w * chans * depth + 7) // 8
    raw, rows, prev = zlib.decompress(idat), [], bytearray(stride)
    for y in range(h):
        f, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b, c = prev[i], prev[i - bpp] if i >= bpp else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        prev = line
        if depth == 16:
            vals = list(line[0::2])
        elif depth == 8:
            vals = list(line)
        else:  # 1, 2, 4 bits: unpack, then scale gray to 0-255 (palette indices stay)
            per, mask = 8 // depth, (1 << depth) - 1
            vals = [(byte >> (8 - depth * (k + 1))) & mask for byte in line for k in range(per)][:w * chans]
            if ctype == 0:
                vals = [v * 255 // mask for v in vals]
        px = []
        for x in range(w):
            v = vals[x * chans:(x + 1) * chans]
            if ctype == 3:
                r, g, b = plte[v[0] * 3:v[0] * 3 + 3]
                px.append((r, g, b, trns[v[0]] if v[0] < len(trns) else 255))
            elif ctype == 0:
                px.append((v[0], v[0], v[0], 255))
            elif ctype == 4:
                px.append((v[0], v[0], v[0], v[1]))
            elif ctype == 2:
                px.append((v[0], v[1], v[2], 255))
            else:
                px.append(tuple(v))
        rows.append(px)
    return w, h, rows


def read_picture(path: str) -> tuple[int, int, list]:
    """PNG with the standard library; GIF (first frame), JPEG, WebP, BMP... with Pillow if it's there."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return read_png(data)
    try:
        from PIL import Image
    except ImportError:
        raise ValueError("termwall reads PNG by itself; for other formats save it as PNG, "
                         "or install Pillow (pip install pillow)") from None
    import io
    im = Image.open(io.BytesIO(data)).convert("RGBA")
    raw, w = im.tobytes(), im.width
    return w, im.height, [[tuple(raw[(y * w + x) * 4:(y * w + x) * 4 + 4]) for x in range(w)] for y in range(im.height)]


def _shrink(w: int, h: int, rows: list, cols: int, out_rows: int) -> list:
    """Area-average down to cols x out_rows (alpha-weighted); pixel art picks the nearest pixel."""
    sharp = w <= cols * 2
    out = []
    for oy in range(out_rows):
        y0, y1 = oy * h // out_rows, max(oy * h // out_rows + 1, (oy + 1) * h // out_rows)
        line = []
        for ox in range(cols):
            x0, x1 = ox * w // cols, max(ox * w // cols + 1, (ox + 1) * w // cols)
            if sharp:
                line.append(rows[min(h - 1, (y0 + y1) // 2)][min(w - 1, (x0 + x1) // 2)])
                continue
            r = g = b = a = 0
            n = 0
            for y in range(y0, y1):
                for p in rows[y][x0:x1]:
                    r += p[0] * p[3]; g += p[1] * p[3]; b += p[2] * p[3]; a += p[3]; n += 1
            line.append((r // a, g // a, b // a, a // n) if a else (0, 0, 0, 0))
        out.append(line)
    return out


def _drop_background(rows: list, tol: int = 40) -> tuple[list, tuple | None]:
    """A picture with no transparency: the color around its border, flooded in from the edges,
    becomes transparent (a logo on a white square loses the square, not its own white).
    Returns the rows and that color (None: nothing dropped)."""
    h, w = len(rows), len(rows[0])
    if any(p[3] < 255 for line in rows for p in line):
        return rows, None
    border = [rows[y][x] for y in (0, h - 1) for x in range(w)] + [rows[y][x] for x in (0, w - 1) for y in range(h)]
    bg = max(set(border), key=border.count)
    if border.count(bg) < len(border) * 0.6:
        return rows, None  # no single background color: keep the whole picture
    near = lambda p: sum(abs(p[i] - bg[i]) for i in range(3)) <= tol
    rows = [list(line) for line in rows]
    todo = [(x, y) for x in range(w) for y in (0, h - 1)] + [(x, y) for x in (0, w - 1) for y in range(h)]
    while todo:
        x, y = todo.pop()
        if 0 <= x < w and 0 <= y < h and rows[y][x][3] and near(rows[y][x]):
            rows[y][x] = (0, 0, 0, 0)
            todo += [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
    return rows, bg[:3]


def _seg_d2(c, a, b) -> float:
    """Squared distance from color c to the line between colors a and b (a == b: to a)."""
    ab = [b[i] - a[i] for i in range(3)]
    ac = [c[i] - a[i] for i in range(3)]
    n = sum(x * x for x in ab)
    t = max(0.0, min(1.0, sum(ab[i] * ac[i] for i in range(3)) / n)) if n else 0.0
    return sum((ac[i] - t * ab[i]) ** 2 for i in range(3))


def _quantize(pixels: list, k: int, bg: tuple | None = None) -> list:
    """Up to k colors for these pixels (k-means from the most common color outwards), the most
    used first; close ones are merged. bg: a dropped background, so its blend into the edges goes too."""
    hist: dict = {}
    for p in pixels:
        q = (p[0] >> 3 << 3 | 4, p[1] >> 3 << 3 | 4, p[2] >> 3 << 3 | 4)
        hist[q] = hist.get(q, 0) + 1
    if not hist:
        return []
    d2 = lambda a, b: (a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 + (a[2] - b[2]) ** 2
    cents = [max(hist, key=hist.get)]
    while len(cents) < min(k, len(hist)):
        cents.append(max(hist, key=lambda c: hist[c] ** 0.5 * min(d2(c, x) for x in cents)))
    total = sum(hist.values())
    for step in range(14):
        sums = [[0, 0, 0, 0] for _ in cents]
        for c, n in hist.items():
            s = sums[min(range(len(cents)), key=lambda i: d2(c, cents[i]))]
            s[0] += c[0] * n; s[1] += c[1] * n; s[2] += c[2] * n; s[3] += n
        cents = [(s[0] // s[3], s[1] // s[3], s[2] // s[3], s[3]) for s in sums if s[3]]
        if step == 7:  # a color (under a fifth of the picture) that mixes two bigger ones is an edge blend: dropped,
            cents.sort(key=lambda c: -c[3])  # its pixels go to those (a small white eye stays)
            keep = []
            for c in cents:
                ends = keep + ([bg] if bg else [])
                inner_bg = bg is not None and _seg_d2(c, bg, bg) < 28 ** 2  # the background's color inside: a detail
                if c[3] >= total * 0.2 or inner_bg or not any(_seg_d2(c, a, b) < 28 ** 2 for a in ends for b in ends):
                    keep.append(c)
            cents = keep
        counts = [c[3] for c in cents]
        cents = [c[:3] for c in cents]
    merged: list = []
    for c, n in sorted(zip(cents, counts), key=lambda x: -x[1]):
        if all(d2(c, m) > 24 ** 2 for m in merged):
            merged.append(c)
    return merged


def picture_logo(path: str, width: int = 40, chars: str = "blocks", colors: int = 9,
                 keep_bg: bool = False) -> tuple[str, list[str]]:
    """A picture -> (logo text with $1..$N, ["#rrggbb" for $1..$N]), at most 120 x 40 characters.
    blocks: half blocks, two pixels a character, one color each (logo.txt has no background color,
    so where the two pixels differ the rarer one wins: lines and highlights survive a fill);
    ascii: characters from light to dense."""
    w, h, rows = read_picture(path)
    width = max(12, min(120, width))
    per = 2 if chars == "blocks" else 1
    out_rows = max(1, round(width * h / w * 0.5))
    if out_rows > 40:  # logo.txt: up to 40 lines
        width, out_rows = max(4, round(width * 40 / out_rows)), 40
    if not keep_bg:  # on a 4x bigger copy, so the background's blend into the edges goes with it
        mid_w, mid_h = min(w, width * 4), min(h, out_rows * per * 4)
        rows, bg = _drop_background(_shrink(w, h, rows, mid_w, mid_h))
        w, h = mid_w, mid_h
    px = _shrink(w, h, rows, width, out_rows * per)
    ph = len(px)  # specks of up to 3 pixels apart from the rest (sparkles, noise) go
    on = lambda x, y: 0 <= x < width and 0 <= y < ph and px[y][x][3] >= 128
    seen: set = set()
    for y0 in range(ph):
        for x0 in range(width):
            if (x0, y0) in seen or not on(x0, y0):
                continue
            part, todo = [], [(x0, y0)]
            seen.add((x0, y0))
            while todo:
                x, y = todo.pop()
                part.append((x, y))
                for n in ((x + dx, y + dy) for dx in (-1, 0, 1) for dy in (-1, 0, 1)):
                    if n not in seen and on(*n):
                        seen.add(n)
                        todo.append(n)
            if len(part) <= 3:
                for x, y in part:
                    px[y][x] = (0, 0, 0, 0)
    solid = [p for line in px for p in line if p[3] >= 128]
    pal = _quantize(solid, max(1, min(9, colors)), None if keep_bg else bg)
    if not pal:
        raise ValueError("the picture is all transparent")
    nearest: dict = {}
    def idx(p):
        if p is None or p[3] < 128:
            return None
        key = p[:3]
        if key not in nearest:
            nearest[key] = min(range(len(pal)), key=lambda i: sum((p[j] - pal[i][j]) ** 2 for j in range(3)))
        return nearest[key]
    sums = [[0, 0, 0, 0] for _ in pal]  # each color: the average of its own pixels, not of the bins
    for p in solid:
        s = sums[idx(p)]
        s[0] += p[0]; s[1] += p[1]; s[2] += p[2]; s[3] += 1
    final = [(s[0] // s[3], s[1] // s[3], s[2] // s[3]) if s[3] else c for s, c in zip(sums, pal)]
    use = [0] * len(pal)
    cells = []
    for y in range(out_rows):
        line = []
        for x in range(width):
            if chars == "blocks":
                a, b = idx(px[y * 2][x]), idx(px[y * 2 + 1][x])
                line.append((a, b))
                for i in (a, b):
                    if i is not None:
                        use[i] += 1
            else:
                i = idx(px[y][x])
                line.append((i, px[y][x]))
                if i is not None:
                    use[i] += 1
        cells.append(line)
    ramp = " .:-=+*#%@"
    text_rows = []
    cur = None
    for line in cells:
        s = ""
        for cell in line:
            if chars == "blocks":
                a, b = cell
                if a is None and b is None:
                    s += " "
                    continue
                if a is not None and b is not None and a != b:
                    a, b = (a, None) if use[a] <= use[b] else (None, b)
                i, ch = (a, "█") if a is not None and a == b else (a, "▀") if a is not None else (b, "▄")
            else:
                i, p = cell
                if i is None:
                    s += " "
                    continue
                lum = (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) / 255
                ch = ramp[min(len(ramp) - 1, int(lum * len(ramp)))]
                ch = "." if ch == " " else ch
            if i != cur:
                s += f"${i + 1}"
                cur = i
            s += ch
        text_rows.append(s.rstrip())
    while text_rows and not text_rows[-1].strip():
        text_rows.pop()
    while text_rows and not text_rows[0].strip():
        text_rows.pop(0)
    plain = lambda r: __import__("re").sub(r"\$\d", "", r)
    margin = min((len(plain(r)) - len(plain(r).lstrip()) for r in text_rows if plain(r).strip()), default=0)
    if margin:  # a common left margin out; the color switches before it move along
        trimmed = []
        for r in text_rows:
            out, cut, i = "", margin, 0
            while i < len(r):
                if r[i] == "$" and i + 1 < len(r) and r[i + 1].isdigit():
                    out += r[i:i + 2]
                    i += 2
                elif cut and r[i] == " ":
                    cut -= 1
                    i += 1
                else:
                    out += r[i:]
                    break
            trimmed.append(out)
        text_rows = trimmed
    return "\n".join(text_rows), ["#%02x%02x%02x" % c for c in final]


def _wrapped(prefix: str, words: list[str], width: int = 92) -> list[str]:
    lines, cur = [], prefix
    for w in words:
        if len(cur) + len(w) + 2 > width and cur.strip() != prefix.strip():
            lines.append(cur.rstrip(", ") + ",")
            cur = "#   "
        cur += w + ", "
    lines.append(cur.rstrip(", "))
    return lines


def render_config(cur: dict) -> str:
    """termwall.toml with every setting, its current value, what it does and what it accepts.
    Keys this version doesn't know (from a newer termwall) are kept at the end."""
    out = ["# termwall settings. Edit a value and save: the wallpaper follows within a second.",
           "# A value termwall doesn't accept is ignored (the default is used); termwall --check",
           "# tells you which line is wrong. Delete this file to get every default back.",
           "#",
           "# With theme = \"auto\", a switcher like livery (theme.json) gives the colors; its per-pack",
           "# look wins over this file. This file is rewritten by termwall --style, so keep notes",
           "# elsewhere.",
           "#",
           "# Several monitors (Wallpaper Engine): give termwall on each monitor a number with its",
           "# \"termwall screen\" property in WE, then a [screen.N] table at the end of this file",
           "# changes any setting for that monitor only, e.g.",
           "#   [screen.2]",
           "#   layout = \"logo\"",
           ""]
    for k, check in STYLE_SPEC.items():
        for line in STYLE_DOCS[k].split("\n"):
            out.append(f"# {line}")
        if k == "colors":
            if isinstance(cur.get("colors"), dict):
                out.append(f"colors = {_toml_value(cur['colors'])}")
            else:
                example = {key: THEMES["mint"][key] for key in THEME_KEYS}
                out.append(f"# colors = {_toml_value(example)}")
        else:
            if k in ("hide", "effects"):
                out.append(f"# choices: {', '.join(SECTIONS if k == 'hide' else EFFECTS)}")
            elif k == "theme":
                out.append("# choices: auto, logo, distro, " + ", ".join(list(THEMES)[:5]) + ",")
                out.append("#          " + ", ".join(list(THEMES)[5:]) + "   default: \"auto\"")
            elif k == "logo":
                out.append('# choices: auto | windows | tux | none | custom | image | a logo id:   default: "auto"')
                cat = logo_catalog()
                out += _wrapped("#   ", list(cat.get("featured", [])) + [
                    f"... {len(cat.get('logos', {}))} in all: termwall logos"])
            elif k != "prompt":
                choices = "true | false" if check is _bool else check.help
                out.append(f"# choices: {choices}   default: {_toml_value(CONFIG_DEFAULTS[k])}")
            out.append(f"{k} = {_toml_value(cur.get(k, CONFIG_DEFAULTS[k]))}")
        out.append("")
    extra = {k: v for k, v in cur.items() if k not in STYLE_SPEC and k != "screen"}
    tables = cur.get("screen") if isinstance(cur.get("screen"), dict) else {}
    for n, t in sorted(tables.items(), key=lambda kv: str(kv[0])):
        if isinstance(t, dict):
            out.append(f"[screen.{n}]")
            for k, v in t.items():
                try:
                    out.append(f"{k} = {_toml_value(v)}")
                except TypeError:
                    pass
            out.append("")
    if extra:
        out.append("# from a newer termwall (kept as they were)")
        for k, v in extra.items():
            try:
                out.append(f"{k} = {_toml_value(v)}")
            except TypeError:
                pass
    return "\n".join(out).rstrip("\n") + "\n"


def write_style(cur: dict, path: str) -> None:
    text = render_config(cur) if path.endswith(".toml") else json.dumps(cur, indent=2) + "\n"
    with open(path, "a+", encoding="utf-8") as f:  # in place: no temp+rename (EFS-broken AppData)
        f.seek(0)
        f.truncate()
        f.write(text)
    _cfg_cache.pop(path, None)  # two writes in one timestamp tick must not read the old one back


_cfg_cache: dict = {}  # path -> (mtime, parsed dict, error or None)


def load_config(path: str) -> tuple[dict | None, str | None]:
    """The settings file as a dict, and the parse error if any. A TOML file is re-read only
    when it changes, and a broken edit keeps the last good settings (the wallpaper doesn't
    jump to defaults while you're typing)."""
    if not path.endswith(".toml"):
        raw = _json_file(path)
        return (raw if isinstance(raw, dict) else None), None
    try:
        st = os.stat(path)
        mtime = (st.st_mtime_ns, st.st_size)  # the size too: a coarse clock can repeat a timestamp
    except OSError:
        return None, None
    hit = _cfg_cache.get(path)
    if hit and hit[0] == mtime:
        return hit[1], hit[2]
    try:
        import tomllib
    except ImportError:  # Python < 3.11
        return None, "termwall.toml needs Python 3.11 or newer"
    last_good = hit[1] if hit else None
    try:
        with open(path, "rb") as f:
            data, err = tomllib.load(f), None
    except (OSError, ValueError) as e:  # tomllib.TOMLDecodeError is a ValueError
        data, err = last_good, f"{os.path.basename(path)}: {e}"
    _cfg_cache[path] = (mtime, data, err)
    return data, err


def ensure_config(path: str | None = None) -> str:
    """termwall.toml exists after this. An older termwall.json next to it is carried over
    once and renamed to termwall.json.old."""
    path = path or STYLE_FILE
    if not path.endswith(".toml") or os.path.exists(path):
        return path
    legacy = os.path.join(os.path.dirname(path), "termwall.json")
    raw = _json_file(legacy)
    write_style(raw if isinstance(raw, dict) else {}, path)
    if os.path.exists(legacy):
        try:
            os.replace(legacy, legacy + ".old")
        except OSError:
            pass
    return path


def refresh_config(path: str | None = None) -> bool:
    """Rewrite termwall.toml with this version's comments (new choices, new settings) when its
    values parse; the values stay. True if it was rewritten. A broken file is left alone."""
    path = path or STYLE_FILE
    if not path.endswith(".toml"):
        return False
    raw, err = load_config(path)
    if err or raw is None:
        return False
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return False
    fresh = render_config(raw)
    if fresh == text:
        return False
    write_style(raw, path)
    return True


def check_config(path: str | None = None) -> list[str]:
    """Problems in the settings file, one line each (empty = fine)."""
    path = path or STYLE_FILE
    raw, err = load_config(path)
    if err:
        return [err]
    problems = []
    items = list((raw or {}).items())
    screens = (raw or {}).get("screen")
    if isinstance(screens, dict):
        items = [(k, v) for k, v in items if k != "screen"]
        for n, t in screens.items():
            if not str(n).isdigit() or not isinstance(t, dict):
                problems.append(f"screen.{n}: has to be [screen.NUMBER] with settings under it")
                continue
            items += [(f"screen.{n}.{k}", v) for k, v in t.items()]
    for k, v in items:
        k = k.split(".")[-1] if k.startswith("screen.") else k
        if k not in STYLE_SPEC:
            problems.append(f"{k}: unknown setting (ignored)")
        elif STYLE_SPEC[k](v) is None:
            problems.append(f"{k} = {_toml_value(v) if not isinstance(v, (dict,)) else 'table'}: "
                            f"not accepted, the default is used ({STYLE_SPEC[k].help})")
        elif isinstance(v, list) and len(STYLE_SPEC[k](v)) != len(v):
            problems.append(f"{k}: some entries ignored ({STYLE_SPEC[k].help})")
    return problems


def set_style(key: str, value: str, path: str | None = None) -> dict:
    """One setting from the command line; "default" removes it, key "reset" clears them all.
    Keys this version doesn't know (written by a newer termwall or by hand) are kept."""
    path = ensure_config(path or STYLE_FILE)
    raw, err = load_config(path)
    if err:
        raise ValueError(f"{err}\nfix that line by hand (or delete the file to start from the defaults)")
    cur = dict(raw) if isinstance(raw, dict) else {}
    if key == "reset":
        cur = {}
    elif key not in STYLE_SPEC:
        raise ValueError(f"unknown setting {key!r} ({', '.join(STYLE_SPEC)})")
    elif value.lower() == "default":
        cur.pop(key, None)
    else:
        if key == "colors":
            raise ValueError("colors: put six #rrggbb colors into termwall.toml by hand, or use --theme")
        v = STYLE_SPEC[key](parse_value(key, value))
        if v is None:
            raise ValueError(f"{key}: {STYLE_SPEC[key].help}")
        cur[key] = v
    write_style(cur, path)
    return cur


PAGE_FILE = os.path.join(APP_DIR, "index.html")
TOKEN_FILE = os.path.join(DATA_DIR, "token.js")


def prepare_page_dir(app_dir: str = APP_DIR, data_dir: str = DATA_DIR) -> list[str]:
    """A system package: links in data_dir to the packaged page, so the wallpaper loads it from
    there with token.js beside it. Links that already point at the right file stay; returns
    the ones made. Nothing to do when the page lives where it's written (clone, installer)."""
    if os.path.abspath(app_dir) == os.path.abspath(data_dir):
        return []
    os.makedirs(data_dir, exist_ok=True)
    made = []
    for name in PAGE_ASSETS:
        target, link = os.path.join(app_dir, name), os.path.join(data_dir, name)
        if os.path.islink(link) and os.readlink(link) == target:
            continue
        if os.path.islink(link) or os.path.isfile(link):
            os.remove(link)
        elif os.path.isdir(link):
            shutil.rmtree(link)
        os.symlink(target, link)
        made.append(link)
    return made


def write_token(path: str = TOKEN_FILE) -> str:
    """A random token per API start, written next to the page as token.js. The wallpaper (a
    local file) can load it; a web page in a browser can't read your disk, so it can't call
    the API even though the API answers on 127.0.0.1. In-place write, no temp+rename."""
    token = secrets.token_urlsafe(24)
    with open(path, "a+", encoding="utf-8") as f:
        f.seek(0)
        f.truncate()
        f.write(f'window.TERMWALL_TOKEN = "{token}";\n')
    return token


def request_ok(path_qs: str, host: str | None, token: str) -> bool:
    """Token in ?t= and a loopback Host header (blocks DNS rebinding)."""
    from urllib.parse import parse_qs, urlsplit
    if (host or "").split(":")[0] not in ("127.0.0.1", "localhost"):
        return False
    sent = parse_qs(urlsplit(path_qs).query).get("t", [""])[0]
    return hmac.compare_digest(sent.encode(), token.encode())


def page_version() -> int | None:
    """mtime of index.html next to the API. The page reloads itself when this changes,
    so edits show up without re-adding the wallpaper in Wallpaper Engine."""
    try:
        return os.stat(PAGE_FILE).st_mtime_ns
    except OSError:
        return None


_theme_cache: dict = {"mtime": None, "theme": None}


def read_theme(path: str = THEME_FILE) -> dict | None:
    """Palette from theme.json, re-read only when the file changes. Invalid → None (page keeps its defaults)."""
    try:
        mtime = os.stat(path).st_mtime_ns
    except OSError:
        return None
    if mtime != _theme_cache["mtime"]:
        try:
            with open(path, encoding="utf-8") as f:
                t = json.load(f)
            t = {k: t[k] for k in THEME_KEYS} if valid_theme(t) else None
        except (OSError, ValueError):
            t = None
        _theme_cache.update(mtime=mtime, theme=t)
    return _theme_cache["theme"]


# ─── static info (read once) ────────────────────────────────────────────────

def _reg(path: str, name: str) -> str:
    if os.name != "nt":
        return ""
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, path) as k:
            return str(winreg.QueryValueEx(k, name)[0]).strip()
    except OSError:
        return ""


def parse_os_release(text: str) -> dict:
    """/etc/os-release → {"NAME": ..., "PRETTY_NAME": ..., "ID": ...}."""
    out = {}
    for line in text.splitlines():
        k, sep, v = line.partition("=")
        if sep and k.strip() and not k.startswith("#"):
            out[k.strip()] = v.strip().strip('"').strip("'")
    return out


def parse_cpuinfo(text: str) -> str:
    for line in text.splitlines():
        k, _, v = line.partition(":")
        if k.strip() in ("model name", "Hardware", "Processor") and v.strip():
            return v.strip()
    return ""


def _read(path: str) -> str:
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return ""


def static_info_linux() -> dict:
    rel = parse_os_release(_read("/etc/os-release"))
    vendor = _read("/sys/devices/virtual/dmi/id/board_vendor").strip()
    board = _read("/sys/devices/virtual/dmi/id/board_name").strip() or _read("/sys/devices/virtual/dmi/id/product_name").strip()
    return {
        "user": getpass.getuser(),
        "host": socket.gethostname(),
        "os": f"{rel.get('PRETTY_NAME') or platform.system()} {platform.machine()}".strip(),
        "os_family": "linux",
        "distro": rel.get("ID", ""),
        "distro_like": rel.get("ID_LIKE", ""),
        "kernel": f"Linux {platform.release()}",
        "board": f"{vendor} {board}".strip(),
        "cpu_model": " ".join(parse_cpuinfo(_read("/proc/cpuinfo")).split()) or platform.processor(),
        "cores": psutil.cpu_count(logical=True),
        "physical_cores": psutil.cpu_count(logical=False),
        "ram_total": psutil.virtual_memory().total,
        "boot": psutil.boot_time(),
        "shell": os.path.basename(os.environ.get("SHELL", "")) or "sh",
    }


def static_info() -> dict:
    if sys.platform.startswith("linux"):
        return static_info_linux()
    cv = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"
    product = _reg(cv, "ProductName") or platform.system()
    build = _reg(cv, "CurrentBuild")
    ubr = _reg(cv, "UBR")
    # Windows 11 still reports "Windows 10" in ProductName; build >= 22000 is 11
    if product.startswith("Windows 10") and build.isdigit() and int(build) >= 22000:
        product = product.replace("Windows 10", "Windows 11", 1)
    display = _reg(cv, "DisplayVersion")
    cpu = _reg(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString") or platform.processor()
    board = _reg(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardProduct")
    return {
        "user": getpass.getuser(),
        "host": socket.gethostname(),
        "os": f"{product} {display}".strip() + f" {platform.machine()}".rstrip(),
        "os_family": "windows" if os.name == "nt" else platform.system().lower(),
        "distro": "",
        "distro_like": "",
        "kernel": f"WIN32_NT {platform.version()}" + (f".{ubr}" if ubr else ""),
        "board": board,
        "cpu_model": " ".join(cpu.split()),
        "cores": psutil.cpu_count(logical=True),
        "physical_cores": psutil.cpu_count(logical=False),
        "ram_total": psutil.virtual_memory().total,
        "boot": psutil.boot_time(),
        "shell": "PowerShell" if os.name == "nt" else os.environ.get("SHELL", ""),
    }


def local_ip() -> str:
    for name, addrs in psutil.net_if_addrs().items():
        stats = psutil.net_if_stats().get(name)
        if not stats or not stats.isup:
            continue
        for a in addrs:
            if a.family == socket.AF_INET and not a.address.startswith(("127.", "169.254.", "100.")):
                return f"{a.address} ({name})"
    return "-"


# ─── GPU ─────────────────────────────────────────────────────────────────────

GPU_FIELDS = ["name", "utilization.gpu", "temperature.gpu", "memory.used",
              "memory.total", "power.draw", "clocks.gr"]


def parse_nvidia_smi(line: str) -> dict | None:
    parts = [p.strip() for p in line.split(",")]
    if len(parts) != len(GPU_FIELDS):
        return None

    def num(v):
        try:
            return float(v)
        except ValueError:
            return None  # "[N/A]" on some cards/drivers

    return {"name": parts[0], "util": num(parts[1]), "temp": num(parts[2]),
            "vram_used": num(parts[3]), "vram_total": num(parts[4]),
            "power": num(parts[5]), "clock": num(parts[6])}


# Linux without nvidia-smi: what the kernel exposes, no root needed. AMD reports utilisation,
# temperature, power and VRAM in sysfs. Intel does not report utilisation anywhere a user can
# read except per DRM client: each open /dev/dri file's fdinfo carries the nanoseconds every
# engine spent on that client (what nvtop and gputop read). Summing the deltas over the
# user's processes covers the compositor, browsers and games, which is what loads the GPU.

DRM_DRIVERS = ("i915", "xe", "amdgpu")
_drm = {"card": None, "name": None, "busy": None, "t": None}


def _sysfs(path: str) -> str | None:
    try:
        with open(path, encoding="utf-8") as f:
            return f.read().strip()
    except OSError:
        return None


def _num(path: str, scale: float = 1.0) -> float | None:
    v = _sysfs(path)
    try:
        return float(v) / scale if v is not None else None
    except ValueError:
        return None


def drm_card() -> tuple[str, str] | None:
    """The first DRM card with a driver read here: (sysfs card path, driver name)."""
    import glob
    for card in sorted(glob.glob("/sys/class/drm/card[0-9]")):
        try:
            driver = os.path.basename(os.readlink(card + "/device/driver"))
        except OSError:
            continue
        if driver in DRM_DRIVERS:
            return card, driver
    return None


def gpu_marketing_name(lspci_line: str, driver: str) -> str:
    """'00:02.0 VGA ...: Intel Corporation Arrow Lake-P [Arc Pro 130T/140T] (rev 03)' ->
    'Intel Arc Pro 130T/140T'. Without a bracket, the device part after the vendor."""
    import re
    vendor = "AMD" if driver == "amdgpu" else "Intel"
    text = lspci_line.split(": ", 1)[-1]
    text = re.sub(r"\s*\(rev [0-9a-f]+\)$", "", text)
    names = re.findall(r"\[([^\]]+)\]", text)
    if names and names[-1] not in ("AMD/ATI", "AMD"):
        name = names[-1]
    else:
        name = re.sub(r"^(Intel Corporation|Advanced Micro Devices, Inc\. \[AMD/ATI\])\s*", "", text)
    return name if name.startswith(vendor) else f"{vendor} {name}"


def drm_name(card: str, driver: str) -> str:
    slot = os.path.basename(os.path.realpath(card + "/device"))
    exe = shutil.which("lspci")
    if exe:
        try:
            out = subprocess.run([exe, "-s", slot], capture_output=True, text=True, timeout=3).stdout
            if out.strip():
                return gpu_marketing_name(out.strip().splitlines()[0], driver)
        except (OSError, subprocess.TimeoutExpired):
            pass
    return "AMD GPU" if driver == "amdgpu" else "Intel GPU"


def parse_fdinfo(text: str) -> tuple[str | None, str | None, dict[str, tuple[int, int]]]:
    """One DRM fdinfo -> (pdev, client id, {engine: (busy ns, capacity)})."""
    pdev = client = None
    busy, cap = {}, {}
    for line in text.splitlines():
        key, _, val = line.partition(":")
        val = val.strip()
        if key == "drm-pdev":
            pdev = val
        elif key == "drm-client-id":
            client = val
        elif key.startswith("drm-engine-capacity-"):
            cap[key[len("drm-engine-capacity-"):]] = int(val or 1)
        elif key.startswith("drm-engine-") and val.endswith(" ns"):
            busy[key[len("drm-engine-"):]] = int(val[:-3])
    return pdev, client, {e: (ns, cap.get(e, 1)) for e, ns in busy.items()}


def drm_busy(pdev: str) -> dict[tuple[str, str], tuple[int, int]]:
    """{(client id, engine): (busy ns, capacity)} over every DRM client this user can read."""
    out = {}
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        fd_dir = f"/proc/{pid}/fd"
        try:
            fds = os.listdir(fd_dir)
        except OSError:
            continue
        for fd in fds:
            try:
                if not os.readlink(f"{fd_dir}/{fd}").startswith("/dev/dri/"):
                    continue
            except OSError:
                continue
            text = _sysfs(f"/proc/{pid}/fdinfo/{fd}")
            if not text:
                continue
            dev, client, engines = parse_fdinfo(text)
            if dev != pdev or client is None:
                continue
            for engine, v in engines.items():
                out[(client, engine)] = v   # several fds of one client report the same numbers
    return out


def busy_percent(prev: dict, cur: dict, dt: float) -> float:
    """Utilisation of the busiest engine class between two drm_busy() samples."""
    per_engine: dict[str, float] = {}
    for (client, engine), (ns, cap) in cur.items():
        before = prev.get((client, engine))
        if before is None or ns < before[0]:
            continue                                    # a new client, or a restarted one
        per_engine[engine] = per_engine.get(engine, 0.0) + (ns - before[0]) / max(cap, 1)
    if not per_engine or dt <= 0:
        return 0.0
    return round(min(100.0, max(per_engine.values()) / (dt * 1e9) * 100), 1)


def read_gpu_drm() -> dict | None:
    if _drm["card"] is None:
        _drm["card"] = drm_card() or False
        if _drm["card"]:
            _drm["name"] = drm_name(*_drm["card"])
    if not _drm["card"]:
        return None
    card, driver = _drm["card"]
    dev = card + "/device"
    g = {"name": _drm["name"], "util": None, "temp": None, "vram_used": None, "vram_total": None,
         "power": None, "clock": None}
    if driver == "amdgpu":
        import glob
        g["util"] = _num(dev + "/gpu_busy_percent")
        hw = (glob.glob(dev + "/hwmon/hwmon*") or [None])[0]
        if hw:
            g["temp"] = _num(hw + "/temp1_input", 1000)
            g["power"] = _num(hw + "/power1_average", 1e6) or _num(hw + "/power1_input", 1e6)
        g["vram_used"] = _num(dev + "/mem_info_vram_used", 1024 ** 2)
        g["vram_total"] = _num(dev + "/mem_info_vram_total", 1024 ** 2)
        for line in (_sysfs(dev + "/pp_dpm_sclk") or "").splitlines():
            if line.endswith("*"):
                g["clock"] = _num_text(line)
        return g
    # Intel: utilisation from fdinfo, clock from sysfs (i915 on the card, xe per GT)
    pdev = os.path.basename(os.path.realpath(dev))
    now, cur = time.time(), drm_busy(pdev)
    if _drm["busy"] is not None:
        g["util"] = busy_percent(_drm["busy"], cur, now - _drm["t"])
    _drm["busy"], _drm["t"] = cur, now
    # the actual clock is 0 while the GPU sleeps (RC6); the requested one then
    for f in (card + "/gt_act_freq_mhz", dev + "/tile0/gt0/freq0/act_freq",
              card + "/gt_cur_freq_mhz", dev + "/tile0/gt0/freq0/cur_freq"):
        if (v := _num(f)):
            g["clock"] = v
            break
    return g


def _num_text(line: str) -> float | None:
    """'1: 2100Mhz *' -> 2100.0"""
    import re
    m = re.search(r"(\d+)\s*[Mm]hz", line)
    return float(m.group(1)) if m else None


def read_gpu() -> dict | None:
    # nvidia-smi first; on Linux the kernel's view when it is missing or finds no NVIDIA card
    # (nvidia-utils is often installed on machines without one)
    g = read_gpu_nvidia()
    if g is None and sys.platform.startswith("linux"):
        return read_gpu_drm()
    return g


_nvidia = {"failed": 0}


def read_gpu_nvidia() -> dict | None:
    exe = shutil.which("nvidia-smi")
    if not exe or _nvidia["failed"] >= 3:      # three failures in a row: no NVIDIA card here
        return None
    try:
        r = subprocess.run([exe, "--query-gpu=" + ",".join(GPU_FIELDS),
                            "--format=csv,noheader,nounits"],
                           capture_output=True, text=True, timeout=3,
                           creationflags=CREATE_NO_WINDOW)
    except (OSError, subprocess.TimeoutExpired):
        return None
    g = parse_nvidia_smi(r.stdout.splitlines()[0]) if r.returncode == 0 and r.stdout.strip() else None
    _nvidia["failed"] = 0 if g else _nvidia["failed"] + 1
    return g


# ─── install helpers (the Windows installer runs these) ─────────────────────

HERE_DIR = os.path.dirname(os.path.abspath(__file__))


def steam_libraries(vdf_text: str) -> list[str]:
    """Library folders from Steam's libraryfolders.vdf (every "path" entry)."""
    import re
    return [m.replace("\\\\", "\\") for m in re.findall(r'"path"\s+"([^"]+)"', vdf_text)]


def we_projects() -> str | None:
    """Wallpaper Engine's projects/myprojects, found through Steam (registry → libraries)."""
    if os.name != "nt":
        return None
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam") as k:
            steam = winreg.QueryValueEx(k, "SteamPath")[0]
    except OSError:
        return None
    libs = [steam]
    try:
        with open(os.path.join(steam, "steamapps", "libraryfolders.vdf"), encoding="utf-8", errors="replace") as f:
            libs += steam_libraries(f.read())
    except OSError:
        pass
    for lib in libs:
        we = os.path.join(lib, "steamapps", "common", "wallpaper_engine")
        if os.path.isfile(os.path.join(we, "wallpaper64.exe")):
            return os.path.join(we, "projects", "myprojects")
    return None


def link_we() -> str:
    """A junction projects/myprojects/termwall → this folder, so Wallpaper Engine lists
    termwall and plays this very copy (no stale copy of its own). An existing termwall there
    that isn't ours is left alone."""
    projects = we_projects()
    if projects is None:
        return "Wallpaper Engine not found — open index.html from this folder in your wallpaper app"
    link = os.path.join(projects, "termwall")
    if os.path.lexists(link):
        try:
            if os.path.samefile(link, HERE_DIR):
                return f"already linked: {link}"
        except OSError:
            pass
        return f"left alone: {link} exists and isn't this install (your own termwall?)"
    os.makedirs(projects, exist_ok=True)
    r = subprocess.run(["cmd", "/c", "mklink", "/J", link, HERE_DIR], capture_output=True, text=True,
                       creationflags=CREATE_NO_WINDOW)
    return f"linked {link} -> {HERE_DIR}" if r.returncode == 0 else f"mklink failed: {r.stdout}{r.stderr}".strip()


def unlink_we() -> str:
    projects = we_projects()
    link = os.path.join(projects, "termwall") if projects else None
    if not link or not os.path.lexists(link):
        return "nothing linked"
    try:
        ours = os.path.samefile(link, HERE_DIR)
    except OSError:
        ours = False
    if not ours:
        return f"left alone: {link} isn't this install"
    os.rmdir(link)  # removes the junction itself, never what it points to
    return f"unlinked {link}"


LIVELY_PKG = "12030rocksdanister.LivelyWallpaper"  # the Microsoft Store build's package name


def lively_library() -> str | None:
    """Lively Wallpaper's wallpapers folder on disk (the Store build's writes are redirected
    into its package folder), or None when Lively hasn't run yet."""
    local = os.environ.get("LOCALAPPDATA", "")
    virtual = os.path.join(local, "Lively Wallpaper")
    real = None
    packages = os.path.join(local, "Packages")
    if os.path.isdir(packages):
        for d in sorted(os.listdir(packages)):
            cand = os.path.join(packages, d, "LocalCache", "Local", "Lively Wallpaper")
            if d.startswith(LIVELY_PKG + "_") and os.path.isfile(os.path.join(cand, "Settings.json")):
                real = cand
                break
    if real is None and os.path.isfile(os.path.join(virtual, "Settings.json")):
        real = virtual
    if real is None:
        return None
    try:
        with open(os.path.join(real, "Settings.json"), encoding="utf-8-sig") as f:
            wd = json.load(f).get("WallpaperDir") or os.path.join(virtual, "Library")
    except (OSError, ValueError):
        wd = os.path.join(virtual, "Library")
    rel = os.path.relpath(wd, virtual) if os.path.normcase(wd).startswith(os.path.normcase(virtual)) else None
    return os.path.join(real if rel is not None else wd, rel or "", "wallpapers")


def lively_entry() -> str | None:
    lib = lively_library()
    return os.path.join(lib, "termwall") if lib else None


def link_lively() -> str:
    """A Lively library entry (LivelyInfo.json, a web wallpaper) that points at this folder's
    index.html — termwall shows up in Lively after Lively's next start."""
    entry = lively_entry()
    if entry is None:
        return "Lively not found (or never started) — nothing to do"
    info = {"AppVersion": "1.0.0.0", "Title": "termwall", "Desc": "live system stats, fastfetch style",
            "Author": "PantoYT", "License": "MIT", "Contact": "https://github.com/PantoYT/termwall",
            "Type": 1, "FileName": os.path.join(HERE_DIR, "index.html"), "Arguments": None, "IsAbsolutePath": True}
    f = os.path.join(entry, "LivelyInfo.json")
    if os.path.exists(f):
        try:
            with open(f, encoding="utf-8-sig") as fh:
                if os.path.normcase(json.load(fh).get("FileName", "")) != os.path.normcase(info["FileName"]):
                    return f"left alone: {entry} is another termwall"
        except (OSError, ValueError):
            return f"left alone: {entry}"
    os.makedirs(entry, exist_ok=True)
    with open(f, "a+", encoding="utf-8") as fh:  # in place: no temp+rename
        fh.seek(0)
        fh.truncate()
        fh.write(json.dumps(info, indent=2) + "\n")
    return f"added to Lively's library: {entry} (restart Lively to see it)"


def unlink_lively() -> str:
    entry = lively_entry()
    f = os.path.join(entry, "LivelyInfo.json") if entry else None
    if not f or not os.path.exists(f):
        return "nothing in Lively's library"
    try:
        with open(f, encoding="utf-8-sig") as fh:
            ours = os.path.normcase(json.load(fh).get("FileName", "")) == os.path.normcase(os.path.join(HERE_DIR, "index.html"))
    except (OSError, ValueError):
        ours = False
    if not ours:
        return f"left alone: {entry} isn't this install"
    shutil.rmtree(entry, ignore_errors=True)
    return f"removed {entry}"


def stop_running() -> int:
    """Stop the API processes running THIS folder's termwall_api.py (installer: before an
    update replaces files, and on uninstall). Returns how many were stopped."""
    me = os.path.normcase(os.path.abspath(__file__))
    n = 0
    for p in psutil.process_iter(["pid", "cmdline"]):
        try:
            cmd = p.info["cmdline"] or []
            if p.info["pid"] != os.getpid() and any(os.path.normcase(os.path.abspath(c)) == me
                                                     for c in cmd[1:] if c.endswith(".py")):
                p.kill()
                n += 1
        except (psutil.Error, OSError):
            continue
    return n


# ─── sampler ─────────────────────────────────────────────────────────────────

PSEUDO_FS = ("squashfs", "overlay", "tmpfs", "devtmpfs", "efivarfs", "fuse.snapfuse", "nsfs")


class Sampler:
    def __init__(self):
        self.static = static_info()
        self.lock = threading.Lock()
        self.snapshot: dict = {}
        self.cpu_hist: deque = deque(maxlen=HISTORY)
        self.gpu_hist: deque = deque(maxlen=HISTORY)
        self.down_hist: deque = deque(maxlen=HISTORY)
        self.up_hist: deque = deque(maxlen=HISTORY)
        self._net = psutil.net_io_counters()
        self._net_t = time.monotonic()
        self._procs: list = []
        self._gpu = None
        self._tick = 0
        psutil.cpu_percent(percpu=True)  # prime: the first call always returns 0
        for p in psutil.process_iter():
            try:
                p.cpu_percent()
            except psutil.Error:
                pass

    def top_processes(self, n: int = 12) -> list:
        ncpu = self.static["cores"] or 1
        by_name: dict = {}
        for p in psutil.process_iter(["name", "memory_info"]):
            try:
                name = p.info["name"] or ""
                if name in ("", "System Idle Process", "Idle"):
                    continue
                cpu = p.cpu_percent() / ncpu
                mem = p.info["memory_info"].rss if p.info["memory_info"] else 0
            except psutil.Error:
                continue
            e = by_name.setdefault(name, {"name": name, "cpu": 0.0, "mem": 0, "count": 0})
            e["cpu"] += cpu
            e["mem"] += mem
            e["count"] += 1
        rows = sorted(by_name.values(), key=lambda e: (e["cpu"], e["mem"]), reverse=True)[:n]
        for r in rows:
            r["cpu"] = round(r["cpu"], 1)
        return rows

    def sample(self) -> None:
        cores = psutil.cpu_percent(percpu=True)
        cpu = round(sum(cores) / len(cores), 1) if cores else 0.0
        freq = psutil.cpu_freq()
        vm, sw = psutil.virtual_memory(), psutil.swap_memory()

        now = time.monotonic()
        net = psutil.net_io_counters()
        dt = max(now - self._net_t, 1e-3)
        down = (net.bytes_recv - self._net.bytes_recv) / dt
        up = (net.bytes_sent - self._net.bytes_sent) / dt
        self._net, self._net_t = net, now

        if self._tick % 2 == 0:
            self._gpu = read_gpu()
        if self._tick % 3 == 0:
            self._procs = self.top_processes()
        self._tick += 1

        disks = []
        for part in psutil.disk_partitions(all=False):
            if "cdrom" in part.opts or not part.fstype or part.fstype in PSEUDO_FS \
                    or part.mountpoint.startswith(("/snap", "/boot", "/var/lib/docker", "/run")) \
                    or os.path.isfile(part.mountpoint):   # a bind-mounted file (containers' /etc/hosts)
                continue
            try:
                u = psutil.disk_usage(part.mountpoint)
            except OSError:
                continue
            disks.append({"mount": part.mountpoint.rstrip("\\"), "fs": part.fstype,
                          "used": u.used, "total": u.total, "percent": u.percent})

        self.cpu_hist.append(cpu)
        self.gpu_hist.append((self._gpu or {}).get("util") or 0)
        self.down_hist.append(down)
        self.up_hist.append(up)

        snap = {
            **self.static,
            "time": time.time(),
            "uptime": int(time.time() - self.static["boot"]),
            "local_ip": local_ip(),
            "cpu": cpu, "cores_pct": cores,
            "cpu_freq": round(freq.current) if freq else None,
            "ram": {"used": vm.total - vm.available, "total": vm.total, "percent": vm.percent},
            "swap": {"used": sw.used, "total": sw.total, "percent": sw.percent},
            "gpu": self._gpu,
            "disks": disks,
            "net": {"down": down, "up": up},
            "procs": self._procs,
            "procs_total": len(psutil.pids()),
            "history": {"cpu": list(self.cpu_hist), "gpu": list(self.gpu_hist),
                        "down": list(self.down_hist), "up": list(self.up_hist)},
        }
        with self.lock:
            self.snapshot = snap

    def run(self) -> None:
        while True:
            t0 = time.monotonic()
            try:
                self.sample()
            except Exception as e:  # the sampler must survive a bad reading
                print("sample error:", e, file=sys.stderr)
            time.sleep(max(0.0, 1.0 - (time.monotonic() - t0)))

    def get(self) -> dict:
        with self.lock:
            return self.snapshot


# ─── HTTP ────────────────────────────────────────────────────────────────────

def make_handler(sampler: Sampler, token: str):
    counts = {"ok": 0, "forbidden": 0}  # /health — lets you check the wallpaper is getting through

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            path = self.path.split("?")[0]
            host_ok = (self.headers.get("Host") or "").split(":")[0] in ("127.0.0.1", "localhost")
            if path == "/health" and host_ok:  # "is it running?" - counters only, so no token needed
                body = json.dumps({"termwall": __version__, **counts}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if not request_ok(self.path, self.headers.get("Host"), token):
                counts["forbidden"] += 1
                self.send_response(403)
                self.send_header("Access-Control-Allow-Origin", "*")  # lets the page see the 403 and reload
                self.end_headers()
                return
            if path == "/logo-image":  # logo = image: the picture itself, decoded by the page
                img = logo_image_bytes()
                self.send_response(200 if img else 404)
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Content-Type", img[1] if img else "text/plain")
                self.send_header("Content-Length", str(len(img[0]) if img else 0))
                self.end_headers()
                if img:
                    self.wfile.write(img[0])
                return
            if path == "/stats":
                st = read_style(screen=screen_of(self.path))
                body = json.dumps({**sampler.get(), "theme": resolve_palette(st), "style": st,
                                   "page": page_version(), **page_extras(st)}).encode()
            elif path == "/theme":  # tiny, polled often so palette switches land fast
                st = read_style(screen=screen_of(self.path))
                body = json.dumps({"theme": resolve_palette(st), "style": st, "page": page_version()}).encode()
            else:
                self.send_response(404)
                self.end_headers()
                return
            counts["ok"] += 1
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            # The wallpaper runs from file:// (origin "null"). * is fine: without the token
            # nothing but a 403 is readable.
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    return Handler


class ExclusiveServer(ThreadingHTTPServer):
    """http.server sets SO_REUSEADDR, and on Windows that lets a second server bind the same
    port next to a running one: two servers, and the second one's token.js locks the wallpaper
    out of the first. Exclusive use instead on Windows. Elsewhere SO_REUSEADDR stays: there it
    never lets two listeners share a port, and without it a restarted server can't bind for
    about a minute while the old connections sit in TIME_WAIT."""
    allow_reuse_address = os.name != "nt"

    def server_bind(self):
        if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        super().server_bind()


# ─── share: the page on a phone (or any screen in the house) ─────────────────────────────
#
# termwall share turns on a second, read-only server on the LAN (0.0.0.0:9012). It serves the
# page itself, so a phone needs nothing but its browser: scan the QR code, add the page to the
# home screen. The link carries a random key (share.json next to the server; termwall share
# new-key replaces it). User name, computer name and local IP never leave the PC on this
# server, and the Host header has to be an IP address (a web page can't point a name at it).

SHARE_FILE = os.path.join(DATA_DIR, "share.json")
SHARE_PORT = 9012
SHARE_STATIC = {"logos.js": "text/javascript; charset=utf-8"}


def read_share(path: str | None = None) -> dict:
    raw = _json_file(path or SHARE_FILE)
    raw = raw if isinstance(raw, dict) else {}
    port = raw.get("port")
    return {"enabled": raw.get("enabled") is True,
            "key": raw["key"] if isinstance(raw.get("key"), str) and len(raw["key"]) >= 12 else None,
            "port": port if isinstance(port, int) and 1024 <= port <= 65535 else SHARE_PORT}


def write_share(cfg: dict, path: str | None = None) -> None:
    with open(path or SHARE_FILE, "a+", encoding="utf-8") as f:  # in place (EFS-broken AppData)
        f.seek(0)
        f.truncate()
        f.write(json.dumps(cfg, indent=2) + "\n")


def lan_addresses() -> list[tuple[str, str]]:
    """(address, what it is) a phone could use: the main network address (the one with the
    default route), then Tailscale's 100.x if it runs. Virtual adapters (WSL, VirtualBox,
    Hyper-V) and disconnected ones are left out: a phone can't reach them."""
    import ipaddress
    out = []
    main = local_ip().split(" ")[0]
    if main and not main.startswith(("127.", "169.254.")):
        out.append((main, "your network"))
    up = {n for n, st in psutil.net_if_stats().items() if st.isup}
    for name, addrs in psutil.net_if_addrs().items():
        for a in addrs:
            if (a.family == socket.AF_INET and name in up and a.address != main
                    and ipaddress.ip_address(a.address) in ipaddress.ip_network("100.64.0.0/10")):
                out.append((a.address, "Tailscale: away from home too, with Tailscale on the phone"))
    return out


def share_url(ip: str, cfg: dict) -> str:
    return f"http://{ip}:{cfg['port']}/?k={cfg['key']}"


def share_page(key: str) -> bytes:
    """index.html for a phone: the key instead of token.js, the API on this server, home-screen
    tags (manifest, fullscreen, theme color)."""
    with open(PAGE_FILE, encoding="utf-8") as f:
        html = f.read()
    head = (f'<link rel="manifest" href="manifest.webmanifest?k={key}">'
            '<meta name="mobile-web-app-capable" content="yes">'
            '<meta name="apple-mobile-web-app-capable" content="yes">'
            '<meta name="apple-mobile-web-app-status-bar-style" content="black">'
            '<link rel="apple-touch-icon" href="icon-192.png">'
            '<meta name="referrer" content="no-referrer">')
    html = html.replace("</head>", head + "</head>", 1)
    html = html.replace('<script src="token.js"></script>',
                        f'<script>window.TERMWALL_TOKEN = {json.dumps(key)}; window.TERMWALL_API = "stats";'
                        ' window.TERMWALL_SHARE = true;</script>', 1)
    return html.encode("utf-8")


def share_manifest(key: str, theme: dict | None) -> bytes:
    bg = (theme or {}).get("bg", "#171717")
    return json.dumps({
        "name": "termwall", "short_name": "termwall", "start_url": f"/?k={key}", "scope": "/",
        "display": "fullscreen", "orientation": "any", "background_color": bg, "theme_color": bg,
        "icons": [{"src": "icon-192.png", "sizes": "192x192", "type": "image/png"},
                  {"src": "icon-512.png", "sizes": "512x512", "type": "image/png"}],
    }).encode()


def png(width: int, height: int, rows) -> bytes:
    """A truecolor PNG from rows of (r, g, b) tuples. stdlib only."""
    import struct
    import zlib

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + bytes(c for px in row for c in px) for row in rows)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


_icon_cache: dict = {}


def share_icon(size: int, theme: dict | None) -> bytes:
    """The home-screen icon: a prompt ">_" in the accent color on the background."""
    theme = theme or THEMES["mint"]
    key = (size, theme["bg"], theme["accent"])
    if key in _icon_cache:
        return _icon_cache[key]
    hexrgb = lambda h: tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))
    bg, fg = hexrgb(theme["bg"]), hexrgb(theme["accent"])
    s = size / 100  # drawn on a 100x100 grid

    def seg(px, py, ax, ay, bx, by):  # distance from a point to a segment
        dx, dy = bx - ax, by - ay
        t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
        return ((px - ax - t * dx) ** 2 + (py - ay - t * dy) ** 2) ** 0.5
    rows = []
    for y in range(size):
        row = []
        for x in range(size):
            u, v = x / s, y / s
            on = (min(seg(u, v, 24, 30, 46, 50), seg(u, v, 46, 50, 24, 70)) < 6.5
                  or (52 <= u <= 78 and 63 <= v <= 72))
            row.append(fg if on else bg)
        rows.append(row)
    _icon_cache[key] = png(size, size, rows)
    return _icon_cache[key]


# --- a QR code encoder (byte mode, error correction M, versions 1-10): enough for a link ---

_QR_M = {  # version: (EC codewords per block, [data codewords of each block])
    1: (10, [16]), 2: (16, [28]), 3: (26, [44]), 4: (18, [32, 32]), 5: (24, [43, 43]),
    6: (16, [27] * 4), 7: (18, [31] * 4), 8: (22, [38, 38, 39, 39]), 9: (22, [36, 36, 36, 37, 37]),
    10: (26, [43, 43, 43, 43, 44]),
}
_QR_ALIGN = {1: [], 2: [6, 18], 3: [6, 22], 4: [6, 26], 5: [6, 30], 6: [6, 34], 7: [6, 22, 38],
             8: [6, 24, 42], 9: [6, 26, 46], 10: [6, 28, 50]}


def _gf_mul(x: int, y: int) -> int:
    z = 0
    for i in reversed(range(8)):
        z = (z << 1) ^ ((z >> 7) * 0x11D)
        z ^= ((y >> i) & 1) * x
    return z


def _rs_ec(data: list[int], degree: int) -> list[int]:
    div = [0] * (degree - 1) + [1]
    root = 1
    for _ in range(degree):
        for j in range(degree):
            div[j] = _gf_mul(div[j], root)
            if j + 1 < degree:
                div[j] ^= div[j + 1]
        root = _gf_mul(root, 2)
    rem = [0] * degree
    for b in data:
        factor = b ^ rem.pop(0)
        rem.append(0)
        for i, c in enumerate(div):
            rem[i] ^= _gf_mul(c, factor)
    return rem


def qr_matrix(text: str) -> list[list[bool]]:
    """The modules of a QR code for text (True = dark), without the quiet zone."""
    data = text.encode("utf-8")
    for ver in range(1, 11):
        ec_len, blocks = _QR_M[ver]
        cap = sum(blocks)
        cc_bits = 8 if ver < 10 else 16
        if 4 + cc_bits + 8 * len(data) <= cap * 8:
            break
    else:
        raise ValueError("too long for a QR code here")
    bits = [0, 1, 0, 0] + [(len(data) >> i) & 1 for i in reversed(range(cc_bits))]
    for b in data:
        bits += [(b >> i) & 1 for i in reversed(range(8))]
    bits += [0] * min(4, cap * 8 - len(bits))
    bits += [0] * (-len(bits) % 8)
    words = [int("".join(map(str, bits[i:i + 8])), 2) for i in range(0, len(bits), 8)]
    pad = 0
    while len(words) < cap:
        words.append((0xEC, 0x11)[pad % 2])
        pad += 1
    split, at = [], 0
    for n in blocks:
        split.append(words[at:at + n])
        at += n
    ecs = [_rs_ec(b, ec_len) for b in split]
    final = [b[i] for i in range(max(blocks)) for b in split if i < len(b)]
    final += [e[i] for i in range(ec_len) for e in ecs]

    size = ver * 4 + 17
    mod = [[False] * size for _ in range(size)]
    fn = [[False] * size for _ in range(size)]

    def setf(x, y, dark):
        mod[y][x] = dark
        fn[y][x] = True
    for i in range(size):
        setf(6, i, i % 2 == 0)
        setf(i, 6, i % 2 == 0)
    for cx, cy in ((3, 3), (size - 4, 3), (3, size - 4)):
        for dy in range(-4, 5):
            for dx in range(-4, 5):
                x, y = cx + dx, cy + dy
                if 0 <= x < size and 0 <= y < size:
                    setf(x, y, max(abs(dx), abs(dy)) not in (2, 4))
    al = _QR_ALIGN[ver]
    for i, ax in enumerate(al):
        for j, ay in enumerate(al):
            if (i, j) in ((0, 0), (0, len(al) - 1), (len(al) - 1, 0)):
                continue
            for dy in range(-2, 3):
                for dx in range(-2, 3):
                    setf(ax + dx, ay + dy, max(abs(dx), abs(dy)) != 1)

    def draw_format(mask):
        d = mask  # error correction M = 00
        rem = d
        for _ in range(10):
            rem = (rem << 1) ^ ((rem >> 9) * 0x537)
        fb = (d << 10 | rem) ^ 0x5412
        bit = lambda i: (fb >> i) & 1 == 1
        for i in range(6):
            setf(8, i, bit(i))
        setf(8, 7, bit(6))
        setf(8, 8, bit(7))
        setf(7, 8, bit(8))
        for i in range(9, 15):
            setf(14 - i, 8, bit(i))
        for i in range(8):
            setf(size - 1 - i, 8, bit(i))
        for i in range(8, 15):
            setf(8, size - 15 + i, bit(i))
        setf(8, size - 8, True)
    draw_format(0)
    if ver >= 7:
        rem = ver
        for _ in range(12):
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25)
        vb = ver << 12 | rem
        for i in range(18):
            b = (vb >> i) & 1 == 1
            a, c = size - 11 + i % 3, i // 3
            setf(a, c, b)
            setf(c, a, b)

    i, right = 0, size - 1
    nbits = len(final) * 8
    while right >= 1:
        if right == 6:
            right = 5
        for vert in range(size):
            for j in range(2):
                x = right - j
                y = size - 1 - vert if ((right + 1) & 2) == 0 else vert
                if not fn[y][x] and i < nbits:
                    mod[y][x] = (final[i >> 3] >> (7 - (i & 7))) & 1 == 1
                    i += 1
        right -= 2

    masks = [lambda x, y: (x + y) % 2 == 0, lambda x, y: y % 2 == 0, lambda x, y: x % 3 == 0,
             lambda x, y: (x + y) % 3 == 0, lambda x, y: (x // 3 + y // 2) % 2 == 0,
             lambda x, y: x * y % 2 + x * y % 3 == 0, lambda x, y: (x * y % 2 + x * y % 3) % 2 == 0,
             lambda x, y: ((x + y) % 2 + x * y % 3) % 2 == 0]

    def apply(m):
        for y in range(size):
            for x in range(size):
                if not fn[y][x] and masks[m](x, y):
                    mod[y][x] = not mod[y][x]

    def penalty():
        p = 0
        lines = [row for row in mod] + [[mod[y][x] for y in range(size)] for x in range(size)]
        for line in lines:
            run, prev = 0, None
            for v in line:
                run = run + 1 if v == prev else 1
                prev = v
                if run == 5:
                    p += 3
                elif run > 5:
                    p += 1
            s = "".join("1" if v else "0" for v in line)
            p += 40 * (s.count("10111010000") + s.count("00001011101"))
        for y in range(size - 1):
            for x in range(size - 1):
                if mod[y][x] == mod[y][x + 1] == mod[y + 1][x] == mod[y + 1][x + 1]:
                    p += 3
        dark = sum(v for row in mod for v in row)
        p += 10 * (abs(dark * 20 - size * size * 10) // (size * size))
        return p
    best = None
    for m in range(8):
        apply(m)
        draw_format(m)
        score = penalty()
        if best is None or score < best[0]:
            best = (score, m)
        apply(m)  # undo (the mask is its own inverse)
    apply(best[1])
    draw_format(best[1])
    return mod


def qr_text(matrix: list[list[bool]]) -> str:
    """The QR code for a terminal: two rows per line with half blocks, dark on light, with a
    quiet zone. Black and white are forced so a dark terminal theme can't invert it."""
    q = 2
    size = len(matrix)
    get = lambda x, y: 0 <= x < size and 0 <= y < size and matrix[y][x]
    lines = []
    for y in range(-q, size + q, 2):
        row = "".join({(False, False): " ", (True, False): "▀", (False, True): "▄", (True, True): "█"}
                      [(get(x, y), get(x, y + 1))] for x in range(-q, size + q))
        lines.append("\x1b[30;107m" + row + "\x1b[0m")
    return "\n".join(lines)


def make_share_handler(sampler: Sampler, key: str):
    fails: dict = {}  # ip -> [count, first failure time]: a wrong key 20 times in 10 min = blocked

    class ShareHandler(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, code: int, body: bytes = b"", ctype: str = "text/plain; charset=utf-8",
                 cache: str = "no-store") -> None:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", cache)
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            import ipaddress
            from urllib.parse import parse_qs, urlsplit
            host = (self.headers.get("Host") or "").rsplit(":", 1)[0].strip("[]")
            try:
                ipaddress.ip_address(host)  # an IP, never a name: no DNS rebinding
            except ValueError:
                return self.send(403, b"open termwall by the address termwall share shows\n")
            url = urlsplit(self.path)
            path = url.path.lstrip("/")
            if path == "ping":
                return self.send(200, b"termwall")
            if path in SHARE_STATIC or (path.startswith("fonts/") and path.endswith(".woff2")
                                        and "/" not in path[6:] and ".." not in path):
                try:
                    with open(os.path.join(os.path.dirname(PAGE_FILE), *path.split("/")), "rb") as f:
                        body = f.read()
                except OSError:
                    return self.send(404)
                return self.send(200, body, SHARE_STATIC.get(path, "font/woff2"), "public, max-age=3600")
            ip = self.client_address[0]
            f = fails.get(ip)
            if f and f[0] >= 20 and time.time() - f[1] < 600:
                return self.send(429, b"too many wrong keys, try again in 10 minutes\n")
            q = parse_qs(url.query)
            sent = (q.get("k") or q.get("t") or [""])[0]
            if not hmac.compare_digest(sent.encode(), key.encode()):
                if not f or time.time() - f[1] >= 600:
                    fails[ip] = [0, time.time()]
                fails[ip][0] += 1
                return self.send(403, b"this link's key is wrong or old: termwall share shows the current one\n")
            st = {**read_style(), "private": True}
            theme = resolve_palette(st)
            if path == "":
                return self.send(200, share_page(key), "text/html; charset=utf-8")
            if path == "manifest.webmanifest":
                return self.send(200, share_manifest(key, theme), "application/manifest+json")
            if path in ("icon-192.png", "icon-512.png"):
                return self.send(200, share_icon(int(path[5:8]), theme), "image/png", "public, max-age=3600")
            if path == "logo-image":
                img = logo_image_bytes()
                return self.send(200, img[0], img[1]) if img else self.send(404)
            if path in ("stats", "theme"):
                body = {"theme": theme, "style": st, "page": page_version()}
                if path == "stats":  # nothing that names you or your network
                    body = {**sampler.get(), "user": "user", "host": "computer", "local_ip": "hidden", **body,
                            **page_extras(st)}
                return self.send(200, json.dumps(body).encode(), "application/json")
            return self.send(404)

    return ShareHandler


def run_share(sampler: Sampler) -> None:
    """Watch share.json; start or stop the LAN server when it changes (termwall share on/off
    works without restarting termwall)."""
    httpd, running = None, None
    while True:
        cfg = read_share()
        want = (cfg["port"], cfg["key"]) if cfg["enabled"] and cfg["key"] else None
        if want != running:
            if httpd:
                httpd.shutdown()
                httpd.server_close()
                httpd = None
            if want:
                try:
                    httpd = ExclusiveServer(("0.0.0.0", want[0]), make_share_handler(sampler, want[1]))
                    threading.Thread(target=httpd.serve_forever, daemon=True).start()
                    print(f"share: on, port {want[0]}")
                except OSError as e:
                    print(f"share: port {want[0]} can't be used ({e})")
            running = want
        time.sleep(2)


def cmd_share(args: list[str]) -> int:
    cfg = read_share()
    sub = args[0] if args else "on"
    if sub == "off":
        write_share({**cfg, "enabled": False})
        print("share: off (the LAN server stops within a few seconds)")
        return 0
    if sub not in ("on", "new-key", "show"):
        print("termwall share [on | off | new-key | show]")
        return 2
    if sub == "new-key" or not cfg["key"]:
        cfg["key"] = secrets.token_urlsafe(12)
        if sub == "new-key":
            print("new key: links and home-screen icons made before stop working")
    if sub != "show":
        cfg["enabled"] = True
    write_share(cfg)
    if not cfg["enabled"]:
        print("share is off: termwall share on")
        return 1
    import urllib.request
    up = False
    for _ in range(10):  # the running server picks share.json up within 2 s
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{cfg['port']}/ping", timeout=1) as r:
                up = r.read() == b"termwall"
                break
        except OSError:
            time.sleep(0.5)
    ips = lan_addresses()
    if not ips:
        print("no network address found")
        return 1
    url = share_url(ips[0][0], cfg)
    print(qr_text(qr_matrix(url)))
    print()
    for ip, what in ips:
        print(f"  {share_url(ip, cfg)}   ({what})")
    print("\nScan it with the phone (same Wi-Fi), then the browser menu: Add to Home screen.")
    print("Only stats and the look go out: no user name, computer name or IP.")
    if not up:
        print("\nnote: termwall's server isn't answering on that port. Is it running (termwall)?")
    print("If the phone can't open it: Windows Firewall has to allow Python on private networks"
          " (it asks once; or allow it in Windows Security > Firewall > Allow an app).")
    return 0


# ─── update: only when asked, from GitHub's latest release ───────────────────────────────

RELEASES_API = "https://api.github.com/repos/PantoYT/termwall/releases/latest"
RELEASES_PAGE = "https://github.com/PantoYT/termwall/releases/latest"


def version_tuple(v: str) -> tuple:
    import re
    return tuple(int(x) for x in re.findall(r"\d+", v)[:3]) or (0,)


def pick_installer(release: dict) -> dict | None:
    """The release's Windows installer: name, url, size and sha256 (when GitHub gives one)."""
    for a in release.get("assets") or []:
        name = a.get("name", "")
        if name.startswith("termwall-setup-") and name.endswith(".exe"):
            digest = a.get("digest") or ""
            return {"name": name, "url": a.get("browser_download_url"), "size": a.get("size"),
                    "sha256": digest[7:] if digest.startswith("sha256:") else None}
    return None


def latest_release(timeout: float = 10) -> dict:
    import urllib.request
    req = urllib.request.Request(RELEASES_API, headers={"Accept": "application/vnd.github+json",
                                                        "User-Agent": f"termwall/{__version__}"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def newer_release(timeout: float = 10) -> dict | None:
    """The latest release if it's newer than this copy, else None."""
    rel = latest_release(timeout)
    return rel if version_tuple(rel.get("tag_name", "")) > version_tuple(__version__) else None


def install_kind() -> str:
    """installer (its own Python next to the script), git (a clone) or copy (anything else)."""
    here = os.path.dirname(os.path.abspath(__file__))
    if os.path.isdir(os.path.join(here, ".git")):
        return "git"
    if os.path.exists(os.path.join(here, "python", "python.exe")):
        return "installer"
    return "copy"


def cmd_update(args: list[str]) -> int:
    """termwall update [--check]: the latest release, downloaded, checked and installed."""
    try:
        rel = newer_release()
    except OSError as e:
        print(f"can't reach GitHub: {e}")
        return 1
    if not rel:
        print(f"termwall {__version__} is the latest")
        return 0
    tag = rel.get("tag_name", "?").lstrip("v")
    print(f"termwall {tag} is out (this is {__version__}): {rel.get('html_url') or RELEASES_PAGE}")
    if "--check" in args:
        return 0
    kind = install_kind()
    if kind == "git":
        print("this is a git clone: git pull, then restart the server (termwall stop, then termwall)")
        return 0
    if SYSTEM_INSTALL:
        print("installed by your package manager: update it there (AUR: termwall), then"
              " systemctl --user restart termwall-api")
        return 0
    asset = pick_installer(rel)
    if os.name != "nt" or kind != "installer" or not asset:
        print(f"download it from {RELEASES_PAGE}")
        return 0
    import hashlib
    import tempfile
    import urllib.request
    dest = os.path.join(tempfile.gettempdir(), asset["name"])
    print(f"downloading {asset['name']} ({(asset['size'] or 0) / 1e6:.1f} MB)")
    h = hashlib.sha256()
    req = urllib.request.Request(asset["url"], headers={"User-Agent": f"termwall/{__version__}"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r, open(dest, "wb") as f:
            while chunk := r.read(1 << 16):
                f.write(chunk)
                h.update(chunk)
    except OSError as e:
        print(f"download failed: {e}")
        return 1
    if asset["size"] and os.path.getsize(dest) != asset["size"]:
        print("download incomplete; nothing installed")
        return 1
    if asset["sha256"] and h.hexdigest() != asset["sha256"]:
        os.remove(dest)
        print("the download doesn't match the release's checksum; nothing installed")
        return 1
    # The installer replaces the Python this command runs on, so it runs on its own after we exit;
    # it stops the server, keeps termwall.toml and starts the new server when it's done.
    subprocess.Popen([dest, "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"],
                     creationflags=0x00000008 | 0x00000200, close_fds=True)  # DETACHED | NEW_PROCESS_GROUP
    print(f"installing {tag} in the background; the wallpaper comes back in about half a minute."
          " Your settings stay.")
    return 0


def serve() -> None:
    sampler = Sampler()
    try:  # bind before anything else: a second copy must not rewrite token.js
        httpd = ExclusiveServer((HOST, PORT), make_handler(sampler, "pending"))
    except OSError:
        sys.exit(f"termwall's server is already running on {HOST}:{PORT} (check: http://{HOST}:{PORT}/health)")
    if SYSTEM_INSTALL:
        os.makedirs(CONFIG_DIR, exist_ok=True)
        prepare_page_dir()
    httpd.RequestHandlerClass = make_handler(sampler, write_token())
    try:
        ensure_config()
        refresh_config()  # an update brings new choices into the comments
    except OSError as e:
        print(f"note: could not write {STYLE_FILE}: {e}")
    sampler.sample()
    threading.Thread(target=sampler.run, daemon=True).start()
    threading.Thread(target=run_share, args=(sampler,), daemon=True).start()  # termwall share
    print(f"termwall's server on http://{HOST}:{PORT} - Ctrl+C stops it")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("stopped")


def selftest() -> int:
    ok = fail = 0

    def check(c, m):
        nonlocal ok, fail
        if c:
            ok += 1
        else:
            fail += 1
            print("  FAIL:", m)

    g = parse_nvidia_smi("NVIDIA GeForce RTX 3060, 41, 47, 2059, 12288, 29.78, 1860")
    check(g and g["name"] == "NVIDIA GeForce RTX 3060" and g["util"] == 41.0, "nvidia-smi line")
    check(g and g["vram_total"] == 12288.0, "VRAM")
    g2 = parse_nvidia_smi("GPU, 5, 40, 100, 200, [N/A], 900")
    check(g2 and g2["power"] is None and g2["util"] == 5.0, "[N/A] -> None")
    check(parse_nvidia_smi("too,few") is None, "wrong field count")
    check(gpu_marketing_name("00:02.0 VGA compatible controller: Intel Corporation Arrow Lake-P "
                             "[Arc Pro 130T/140T] (rev 03)", "i915") == "Intel Arc Pro 130T/140T",
          "Intel GPU name from lspci")
    check(gpu_marketing_name("03:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] "
                             "Navi 23 [Radeon RX 6600] (rev c7)", "amdgpu") == "AMD Radeon RX 6600",
          "AMD GPU name from lspci")
    fi = "drm-driver:\ti915\ndrm-client-id:\t16\ndrm-pdev:\t0000:00:02.0\n" \
         "drm-engine-render:\t1000 ns\ndrm-engine-video:\t0 ns\ndrm-engine-capacity-video:\t2\n"
    dev, cl, eng = parse_fdinfo(fi)
    check(dev == "0000:00:02.0" and cl == "16" and eng == {"render": (1000, 1), "video": (0, 2)},
          "DRM fdinfo")
    prev = {("16", "render"): (0, 1), ("16", "video"): (0, 2)}
    cur = {("16", "render"): (250_000_000, 1), ("16", "video"): (1_000_000_000, 2),
           ("17", "render"): (9_000_000_000, 1)}
    check(busy_percent(prev, cur, 1.0) == 50.0, "GPU busy: busiest engine, capacity, new clients skipped")
    check(_read("/nonexistent/termwall") == "" and _sysfs("/nonexistent/termwall") is None,
          "missing files: _read gives \"\" (static info), _sysfs gives None (GPU)")

    s = Sampler()
    s.sample()
    time.sleep(1.1)
    s.sample()
    snap = s.get()
    check(0 <= snap["cpu"] <= 100, "cpu 0-100")
    check(len(snap["cores_pct"]) == snap["cores"], "per-core percentages")
    check(snap["ram"]["total"] > 0 and 0 <= snap["ram"]["percent"] <= 100, "RAM")
    check(snap["net"]["down"] >= 0 and snap["net"]["up"] >= 0, "network rates non-negative")
    check(len(snap["history"]["cpu"]) == 2, "history grows per sample")
    check(snap["disks"] and all("mount" in d for d in snap["disks"]), "disks")
    check(isinstance(snap["procs"], list), "process list")
    check(snap["user"] and snap["host"], "static info")
    check(isinstance(snap["cpu_model"], str) and snap["cpu_model"], "CPU model is a name, not overwritten by usage")
    check(isinstance(snap["cpu"], float), "cpu is the usage percentage")
    check(json.loads(json.dumps(snap)) == snap, "JSON-serializable")

    a = ExclusiveServer(("127.0.0.1", 0), make_handler(s, "x"))
    try:
        ExclusiveServer(("127.0.0.1", a.server_address[1]), make_handler(s, "x")).server_close()
        check(False, "a second server can't bind a port in use")
    except OSError:
        check(True, "a second server can't bind a port in use")
    a.server_close()
    tok = "abc123"
    check(request_ok("/stats?t=abc123", "127.0.0.1:9002", tok), "right token + loopback host")
    check(not request_ok("/stats", "127.0.0.1:9002", tok), "no token -> refused")
    check(not request_ok("/stats?t=nope", "127.0.0.1:9002", tok), "wrong token -> refused")
    check(not request_ok("/stats?t=abc123", "evil.example:9002", tok), "foreign Host (DNS rebinding) -> refused")
    check(request_ok("/theme?t=abc123", "localhost:9002", tok), "localhost Host accepted")

    import tempfile
    good = {"bg": "#171717", "accent": "#b5f4e7", "secondary": "#7faca3",
            "text": "#939fa1", "dim": "#4b5252", "faint": "#242626"}
    check(valid_theme(good), "valid theme accepted")
    check(not valid_theme({**good, "bg": "#12"}), "short hex rejected")
    check(not valid_theme({**good, "bg": "url(x)"}), "non-color rejected (goes into CSS)")
    check(not valid_theme({k: v for k, v in good.items() if k != "dim"}), "missing key rejected")
    with tempfile.TemporaryDirectory() as td:
        p = os.path.join(td, "theme.json")
        check(read_theme(p) is None, "no file -> None")
        with open(p, "w", encoding="utf-8") as f:
            json.dump({**good, "extra": 1}, f)
        check(read_theme(p) == good, "theme read, extra keys dropped")
        with open(p, "w", encoding="utf-8") as f:
            f.write("{broken")
        os.utime(p, ns=(1, 1))  # force a new mtime even on coarse-timestamp filesystems
        check(read_theme(p) is None, "broken JSON -> None")
    with tempfile.TemporaryDirectory() as td:
        sp, tp = os.path.join(td, "termwall.toml"), os.path.join(td, "theme.json")
        check(read_style(sp, tp) == STYLE_DEFAULTS, "no files -> default look")
        set_style("layout", "board", sp)
        set_style("rotate", "15", sp)
        check(read_style(sp, tp)["layout"] == "board" and read_style(sp, tp)["rotate"] == 15, "termwall.toml read")
        text = open(sp, encoding="utf-8").read()
        check(all(f"\n{k} = " in text for k in STYLE_SPEC if k != "colors") and "# colors = {" in text,
              "termwall.toml lists every setting")
        check("# choices: fetch | board | minimal | htop | portrait" in text, "its choices are in the comments")
        import tomllib
        check(clean_style(tomllib.loads(text)) == {**read_style(sp, tp)}, "the written file parses back the same")
        with open(tp, "w", encoding="utf-8") as f:
            json.dump({**good, "style": {"layout": "minimal", "clock": "segments", "bars": "comic-sans"}}, f)
        st = read_style(sp, tp)
        check(st["layout"] == "minimal" and st["bars"] == "blocks" and st["clock"] == "24h",
              "theme.json style wins, unknown values dropped")
        os.remove(tp)
        set_style("hide", "gpu,procs,nonsense", sp)
        set_style("private", "on", sp)
        set_style("scale", "1.1", sp)
        set_style("effects", "crt", sp)
        set_style("prompt", 'say "hi" \\ bye', sp)
        set_style("theme", "nord", sp)
        st = read_style(sp, tp)
        check(st["hide"] == ["gpu", "procs"] and st["private"] is True and st["scale"] == 1.1
              and st["effects"] == ["crt"] and st["prompt"] == 'say "hi" \\ bye', "settings parsed from the command line")
        check(resolve_palette(st, tp) == THEMES["nord"], "a built-in theme gives the palette")

        def edit(old, new):  # by hand, like a person in Notepad
            t = open(sp, encoding="utf-8").read()
            assert old in t, old
            with open(sp, "w", encoding="utf-8") as f:
                f.write(t.replace(old, new, 1))
            os.utime(sp, ns=(time.time_ns(), time.time_ns() + 1))
        edit("# colors = {", "colors = {")
        edit('accent = "#b5f4e7"', 'accent = "#ff0000"')
        check(resolve_palette(read_style(sp, tp), tp)["accent"] == "#ff0000", "own colors (edited in) win over a theme")
        edit('layout = "board"', 'layout = "htop"\nfrom_a_newer_termwall = 1')
        check(read_style(sp, tp)["layout"] == "htop", "a hand edit is picked up")
        check(check_config(sp) == ["from_a_newer_termwall: unknown setting (ignored)"], "--check names an unknown key")
        with open(tp, "w", encoding="utf-8") as f:
            json.dump(good, f)
        check(resolve_palette(read_style(sp, tp), tp)["accent"] == "#ff0000", "own colors win over theme.json (livery)")
        edit("colors = {", "# colors = {")
        check(resolve_palette(read_style(sp, tp), tp) == THEMES["nord"], "a picked theme wins over theme.json (livery)")
        edit('theme = "nord"', 'theme = "auto"')
        check(resolve_palette(read_style(sp, tp), tp) == good, "theme auto follows theme.json (livery)")
        edit("# colors = {", "colors = {")
        os.remove(tp)
        set_style("hide", "none", sp)
        raw = tomllib.loads(open(sp, encoding="utf-8").read())
        check(raw["hide"] == [] and raw.get("from_a_newer_termwall") == 1 and raw["colors"]["accent"] == "#ff0000",
              "unknown keys and own colors survive a --style write")
        edit('scale = 1.1', 'scale = 7')
        check(read_style(sp, tp)["scale"] == 1.0 and any(x.startswith("scale = 7") for x in check_config(sp)),
              "an invalid value falls back to the default and --check names it")
        edit('scale = 7', 'scale = 1.1')
        read_style(sp, tp)
        edit('font = "cascadia"', 'font = "jetbrains')  # a missing quote: not TOML any more
        check(read_style(sp, tp)["scale"] == 1.1, "a broken file keeps the last good settings")
        check(len(check_config(sp)) == 1 and "termwall.toml" in check_config(sp)[0], "--check shows the parse error")
        try:
            set_style("layout", "board", sp)
            check(False, "--style refuses to overwrite a broken file")
        except ValueError:
            check(True, "--style refuses to overwrite a broken file")
        edit('font = "jetbrains', 'font = "jetbrains"')
        for bad_key, bad in (("scale", "9"), ("layout", "nope"), ("private", "maybe"), ("prompt", "x" * 61)):
            try:
                set_style(bad_key, bad, sp)
                check(False, f"bad {bad_key} refused")
            except ValueError:
                check(True, f"bad {bad_key} refused")
        set_style("theme", "default", sp)
        check(read_style(sp, tp)["theme"] == "auto", "default puts a setting back")
        set_style("reset", "", sp)
        check(read_style(sp, tp) == {**STYLE_DEFAULTS, "prompt": "fastfetch --live"}, "reset puts them all back")
        edit('# choices: crt, glow, flicker', '# choices: crt, glow')
        edit('layout = "fetch"', 'layout = "board"')
        check(refresh_config(sp) and "# choices: crt, glow, flicker" in open(sp, encoding="utf-8").read()
              and read_style(sp, tp)["layout"] == "board", "old comments are refreshed, values kept")
        check(not refresh_config(sp), "a current file is left alone")
    with tempfile.TemporaryDirectory() as td:
        sp, tp = os.path.join(td, "termwall.toml"), os.path.join(td, "theme.json")
        set_style("layout", "htop", sp)
        with open(tp, "w", encoding="utf-8") as f:
            json.dump({**good, "style": {"layout": "fetch", "bars": "blocks"}}, f)
        check(theme_overrides(sp, tp) == {"layout": "fetch"}, "check names what theme.json overrides")
        os.environ.pop("TERMWALL_EDITOR", None)
        os.environ["TERMWALL_EDITOR"] = "myedit --wait"
        check(editor_for(sp) == ["myedit", "--wait", sp], "TERMWALL_EDITOR picks the editor")
        os.environ.pop("TERMWALL_EDITOR")
    with tempfile.TemporaryDirectory() as td:
        sp, legacy = os.path.join(td, "termwall.toml"), os.path.join(td, "termwall.json")
        with open(legacy, "w", encoding="utf-8") as f:
            json.dump({"layout": "portrait", "effects": ["crt"]}, f)
        check(read_style(sp, os.path.join(td, "theme.json"))["layout"] == "portrait", "termwall.json still read before it's carried over")
        ensure_config(sp)
        check(os.path.exists(legacy + ".old") and not os.path.exists(legacy), "termwall.json carried over and renamed")
        check(read_style(sp, os.path.join(td, "theme.json"))["effects"] == ["crt"], "termwall.toml has what termwall.json had")
    rel = parse_os_release('NAME="Ubuntu"\nPRETTY_NAME="Ubuntu 24.04.1 LTS"\nID=ubuntu\n# comment\n')
    check(rel["PRETTY_NAME"] == "Ubuntu 24.04.1 LTS" and rel["ID"] == "ubuntu", "os-release parsed")
    check(parse_cpuinfo("processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5-4590 CPU @ 3.30GHz\n")
          == "Intel(R) Core(TM) i5-4590 CPU @ 3.30GHz", "cpuinfo model name")
    check(snap.get("os_family") in ("windows", "linux", "darwin"), "os_family for the logo")
    vdf = '"libraryfolders"\n{\n "0"\n {\n  "path"\t\t"C:\\\\Program Files (x86)\\\\Steam"\n }\n "1"\n {\n  "path"\t\t"D:\\\\SteamLibrary"\n }\n}'
    with tempfile.TemporaryDirectory() as td:
        saved = os.environ.get("LOCALAPPDATA")
        os.environ["LOCALAPPDATA"] = td
        check(lively_library() is None, "no Lively -> nothing to link")
        os.makedirs(os.path.join(td, "Lively Wallpaper"))
        with open(os.path.join(td, "Lively Wallpaper", "Settings.json"), "w", encoding="utf-8") as f:
            json.dump({"WallpaperDir": os.path.join(td, "Lively Wallpaper", "Library")}, f)
        check(lively_library() == os.path.join(td, "Lively Wallpaper", "Library", "wallpapers"), "Lively's library")
        check("added" in link_lively() and "already" not in link_lively() and "removed" in unlink_lively(),
              "Lively entry added (twice is fine) and removed")
        if saved is None:
            os.environ.pop("LOCALAPPDATA")
        else:
            os.environ["LOCALAPPDATA"] = saved
    check(steam_libraries(vdf) == ["C:\\Program Files (x86)\\Steam", "D:\\SteamLibrary"], "Steam libraries from libraryfolders.vdf")
    with tempfile.TemporaryDirectory() as td:
        lp = os.path.join(td, "logo.txt")
        check(read_logo(lp) is None, "no logo.txt -> no custom logo")
        with open(lp, "w", encoding="utf-8") as f:
            f.write("\ufeff$1 /\\_/\\\n( o.o )\t$2x\n" + "\n".join("y" * 200 for _ in range(60)))
        t = read_logo(lp)
        check(t.startswith("$1 /\\_/\\\n( o.o ) $2x") and len(t.split("\n")) == 40
              and max(len(x) for x in t.split("\n")) == 120, "logo.txt: BOM gone, tabs expanded, 40 x 120 at most")
        check(page_extras({"logo": "custom"}) .keys() == {"logo_text"} and page_extras({"logo": "arch"}) == {},
              "the custom logo goes out only when it's used")
    with tempfile.TemporaryDirectory() as td:
        check(logo_image(td) is None, "no logo image -> none")
        with open(os.path.join(td, "logo.png"), "wb") as f:
            f.write(png(2, 2, [[(255, 0, 0), (0, 0, 0)], [(0, 0, 0), (0, 0, 255)]]))
        with open(os.path.join(td, "logo.gif"), "wb") as f:
            f.write(b"GIF89a")
        check(os.path.basename(logo_image(td)) == "logo.gif", "logo image: gif first, then png")
    # logo-from: a red and a blue square, a white dot inside the red one, on a white background
    W, R, B = (255, 255, 255), (230, 40, 30), (20, 90, 230)
    pic = [[R if 8 <= x < 28 and 8 <= y < 28 and not (14 <= x < 18 and 14 <= y < 18)
            else B if 36 <= x < 56 and 8 <= y < 28 else W for x in range(64)] for y in range(36)]
    with tempfile.TemporaryDirectory() as td:
        pp = os.path.join(td, "pic.png")
        with open(pp, "wb") as f:
            f.write(png(64, 36, pic))
        w, h, rows = read_picture(pp)
        check((w, h) == (64, 36) and rows[10][10] == R + (255,) and rows[0][0] == W + (255,), "a PNG read by the stdlib")
        text, cols = picture_logo(pp, width=32)
        lines = text.split("\n")
        check(sorted(cols) == sorted(["#e6281e", "#145ae6", "#ffffff"]),
              "logo-from: the two squares and the dot inside, the background and its blends gone")
        check(cols[0] in ("#e6281e", "#145ae6") and "$1" in text and "$2" in text and "$3" in text,
              "logo-from: the most used color is $1")
        check(len(lines) == 5 and not lines[0].startswith(" ") and all("█" in x or "▀" in x or "▄" in x for x in lines),
              "logo-from: the white border trimmed, half blocks")
        a_text, _ = picture_logo(pp, width=32, chars="ascii")
        check(set(__import__("re").sub(r"\$\d", "", a_text)) <= set(" .:-=+*#%@\n"), "logo-from --ascii: the ramp")
        k_text, k_cols = picture_logo(pp, width=32, keep_bg=True)
        check("#ffffff" in k_cols and len(k_text.split("\n")) == 9, "logo-from --keep-bg keeps the background")
        tall = os.path.join(td, "tall.png")
        with open(tall, "wb") as f:
            f.write(png(10, 400, [[R] * 10 for _ in range(400)]))
        t_text, _ = picture_logo(tall, width=120, keep_bg=True)
        check(len(t_text.split("\n")) <= 40, "logo-from: 40 lines at most")
        check(picture_logo(pp, width=32, colors=1)[1] in (["#e6281e"], ["#145ae6"]) or
              len(picture_logo(pp, width=32, colors=1)[1]) == 1, "logo-from --colors 1: one color")
        saved = (globals()["LOGO_FILE"], globals()["STYLE_FILE"])
        globals()["LOGO_FILE"], globals()["STYLE_FILE"] = os.path.join(td, "logo.txt"), os.path.join(td, "termwall.toml")
        try:
            import contextlib
            import io
            with contextlib.redirect_stdout(io.StringIO()):
                rc1 = cmd_logo_from([pp, "--width", "32"])
                rc2 = cmd_logo_from([pp])
            st = read_style(os.path.join(td, "termwall.toml"))
            check(rc1 == 0 and rc2 == 1 and read_logo(os.path.join(td, "logo.txt")) == text
                  and st.get("logo") == "custom" and st.get("logo_colors") == cols,
                  "logo-from writes logo.txt, sets logo custom and its colors, won't overwrite without --force")
        finally:
            globals()["LOGO_FILE"], globals()["STYLE_FILE"] = saved
    check(clean_style({"logo": "image", "logo_width": 48, "logo_chars": "ascii", "logo_colors": "palette"})
          == {"logo": "image", "logo_width": 48, "logo_chars": "ascii", "logo_colors": "palette"}
          and clean_style({"logo_width": 500, "logo_chars": "x"}) == {}, "logo image settings validated")
    with tempfile.TemporaryDirectory() as td:
        sp, tp = os.path.join(td, "termwall.toml"), os.path.join(td, "theme.json")
        set_style("layout", "fetch", sp)
        with open(sp, "a", encoding="utf-8") as f:
            f.write('\n[screen.2]\nlayout = "logo"\neffects = ["crt"]\n\n[screen.3]\nlayout = "nope"\n')
        check(read_style(sp, tp)["layout"] == "fetch" and read_style(sp, tp, "2")["layout"] == "logo"
              and read_style(sp, tp, "2")["effects"] == ["crt"] and read_style(sp, tp, "3")["layout"] == "fetch",
              "[screen.N] overrides only that monitor, bad values dropped")
        with open(tp, "w", encoding="utf-8") as f:
            json.dump({**good, "style": {"layout": "board"}}, f)
        check(read_style(sp, tp)["layout"] == "board" and read_style(sp, tp, "2")["layout"] == "logo",
              "[screen.N] wins over livery's per-pack look")
        set_style("bars", "dots", sp)
        text = open(sp, encoding="utf-8").read()
        check('[screen.2]\nlayout = "logo"' in text and read_style(sp, tp, "2")["layout"] == "logo",
              "[screen.N] tables survive a --style write")
        check(any("screen.3.layout" in x or "layout" in x for x in check_config(sp)), "check names a bad screen value")
    check(screen_of("/stats?t=x&screen=2") == "2" and screen_of("/stats?screen=../x") is None, "?screen= read safely")
    check(version_tuple("v1.10.0") > version_tuple("1.9.3") and version_tuple("1.5.0") == (1, 5, 0),
          "versions compare as numbers")
    a = pick_installer({"assets": [{"name": "notes.txt"}, {"name": "termwall-setup-9.9.9.exe", "size": 5,
                        "browser_download_url": "https://x/t.exe", "digest": "sha256:" + "ab" * 32}]})
    check(a and a["name"] == "termwall-setup-9.9.9.exe" and a["sha256"] == "ab" * 32, "update: the installer asset and its sha256")
    check(pick_installer({"assets": []}) is None, "update: no installer in a release -> none")
    ids = distro_ids()
    check("arch" in ids and "nixos" in ids and len(ids) >= 500, "the logo list comes from logos.js")
    cfg = render_config({})
    check(all(f"#   {x}" in cfg or f", {x}" in cfg for x in ("arch", "mx")) and "macos2" not in cfg
          and "termwall logos" in cfg, "termwall.toml names the featured logos, not all of them")
    cat = logo_catalog()
    check(len(cat["own"]["macos"]) == 6 and all(c and c.startswith("#") for c in cat["own"]["macos"]),
          "macOS keeps its six colors")
    check(all(k in cat["logos"] for k in ("chromeos", "windows-7", "windows-95", "windows-11", "freebsd", "haiku")),
          "logos beyond Linux: ChromeOS, Windows, BSD, Haiku")
    check(cat["aliases"].get("sunos") == "solaris" and "windows" not in cat["logos"],
          "fastfetch's other names are aliases; termwall's own windows logo keeps its id")
    check(_logo("macos") == "macos" and _logo("sunos") == "sunos" and _logo("no-such-os") is None,
          "logo: a known id or alias, an unknown one refused")
    check(find_logos(["mac"])[:1] == ["macos"] or "macos" in find_logos(["mac"]), "termwall logos mac finds macOS")
    check(find_logos(["windows", "95"]) == ["windows-95"], "every word has to match")
    check("\x1b[38;2;" in logo_preview("apple") and "$1" not in logo_preview("apple"),
          "a preview in the logo's own colors, without the $N")
    check(_logo_colors("own") == "own" and _logo_colors("image") == "image"
          and _logo_colors("#e05252, #5fb85f") == ["#e05252", "#5fb85f"]
          and _logo_colors(["#e05252"]) == ["#e05252"] and _logo_colors(["red"]) is None
          and _logo_colors(["#000000"] * 10) is None, "logo_colors: a mode or up to nine colors")
    check(resolve_palette({"theme": "logo"}, os.path.join(tempfile.gettempdir(), "no-such-theme.json")) is None,
          "theme logo: the page colors itself")
    check(resolve_palette({"theme": "distro"}, os.path.join(tempfile.gettempdir(), "no-such-theme.json")) is None,
          "theme distro: the page colors itself")
    check(clean_style({"prompt_color": "accent", "logo": "custom"}) == {"prompt_color": "accent", "logo": "custom"}
          and clean_style({"prompt_color": "pink"}) == {}, "prompt_color and logo custom validated")
    # share: the QR code's structure, and the LAN server's rules
    m = qr_matrix("http://192.168.1.20:9012/?k=" + "A" * 16)
    check(len(m) == 33 and all(len(r) == 33 for r in m), "a share link fits a version 4 QR code")
    check([m[0][x] for x in range(7)] == [True] * 7 and [m[3][x] for x in range(7)] == [True, False, True, True, True, False, True],
          "QR finder pattern")
    check([m[6][x] for x in range(8, 25)] == [x % 2 == 0 for x in range(8, 25)], "QR timing pattern")
    check(m[4 * 4 + 9][8] is True, "QR dark module")
    check(len(qr_matrix("x" * 200)) == 57, "a long text takes version 10")
    try:
        qr_matrix("x" * 400)
        check(False, "too long for a QR code is refused")
    except ValueError:
        check(True, "too long for a QR code is refused")
    check(share_icon(192, None)[:8] == b"\x89PNG\r\n\x1a\n", "the home-screen icon is a PNG")

    class FakeSampler:
        def get(self):
            return {"user": "panto", "host": "KOMPU", "local_ip": "192.168.1.20 (Ethernet)", "cpu": 12.0, "time": 1}
    import urllib.error
    import urllib.request
    key = "k" * 16
    srv = ExclusiveServer(("127.0.0.1", 0), make_share_handler(FakeSampler(), key))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    port = srv.server_address[1]

    def get(path, host=None):
        req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", headers={"Host": host or f"127.0.0.1:{port}"})
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            return e.code, b""
    code, body = get(f"/stats?t={key}")
    st = json.loads(body) if code == 200 else {}
    check(code == 200 and st.get("cpu") == 12.0 and st.get("style", {}).get("private") is True, "share serves stats, private on")
    check("panto" not in body.decode() and "KOMPU" not in body.decode() and "192.168.1.20" not in body.decode(),
          "share never sends the user, computer name or IP")
    check(get("/stats?t=wrong")[0] == 403, "share: a wrong key gets 403")
    check(get(f"/stats?t={key}", host=f"evil.example:{port}")[0] == 403, "share: a host name (DNS rebinding) gets 403")
    code, body = get(f"/?k={key}")
    check(code == 200 and key.encode() in body and b'<script src="token.js">' not in body and b"manifest.webmanifest" in body,
          "share serves the page with its key and a manifest")
    check(get("/logos.js")[0] == 200 and get("/termwall_api.py")[0] in (403, 404) and get("/fonts/../share.json")[0] in (403, 404),
          "share: static files only from the list, no source or settings")
    for _ in range(25):
        get("/stats?t=nope")
    check(get(f"/stats?t={key}")[0] == 429, "share: 20 wrong keys block that address for a while")
    srv.shutdown()
    srv.server_close()
    with tempfile.TemporaryDirectory() as td:
        sf = os.path.join(td, "share.json")
        check(read_share(sf) == {"enabled": False, "key": None, "port": SHARE_PORT}, "share is off without share.json")
        write_share({"enabled": True, "key": "short", "port": 80}, sf)
        check(read_share(sf) == {"enabled": True, "key": None, "port": SHARE_PORT}, "share.json: a short key and a bad port are refused")
    check(prepare_page_dir(APP_DIR, APP_DIR) == [], "a clone or the installer keeps the page where it is")
    if os.name != "nt":  # symlinks need extra rights on Windows, and packages are Linux only
        with tempfile.TemporaryDirectory() as td:
            app, data = os.path.join(td, "share"), os.path.join(td, "data")
            os.makedirs(os.path.join(app, "fonts"))
            for name in ("index.html", "logos.js"):
                open(os.path.join(app, name), "w").close()
            os.makedirs(os.path.join(data, "fonts"))  # a stale real folder is replaced by a link
            check(len(prepare_page_dir(app, data)) == 3 and os.path.realpath(os.path.join(data, "index.html"))
                  == os.path.realpath(os.path.join(app, "index.html")), "system package: page linked into the data dir")
            check(prepare_page_dir(app, data) == [], "system package: correct links are left alone")
    print(f"selftest: {ok}/{ok + fail} OK")
    return 0 if fail == 0 else 1


USAGE = """termwall - a live system-stats wallpaper (github.com/PantoYT/termwall)

  termwall                       run the stats server here (the installer runs it at logon)
  termwall config                open termwall.toml in an editor: every setting, its choices in
                                 comments ($EDITOR, else the .toml app, else VS Code, else Notepad)
  termwall check                 which lines of termwall.toml are wrong, and what livery overrides
  termwall reset                 every setting back to its default
  termwall update [--check]      install the latest release (asks GitHub only when you run it)
  termwall share [off|new-key]   the wallpaper on a phone: a QR code to scan, a read-only server
                                 on your network (no user name, computer name or IP go out)
  (the other options work without the dashes too: termwall themes, termwall version)
  termwall --style               every setting, its value and its choices
  termwall --style KEY VALUE     change one (KEY default: remove it, --style reset: all)
  termwall --theme NAME          a built-in palette (termwall --themes lists them)
  termwall logos [WORD...]       the logos (fastfetch's), or those matching: termwall logos mac
  termwall logos ID              show one in its own colors; termwall --style logo ID uses it
  termwall logo-from PICTURE     a picture (PNG; others with Pillow) -> logo.txt with $1..$9, used
                                 as logo = "custom" with its colors in logo_colors. Options:
                                 --width N (40), --ascii, --colors N (1-9), --keep-bg,
                                 --out FILE (write only that file), --force (overwrite logo.txt)
  termwall --once                print one sample
  termwall --selftest            run the self-test
  termwall --link-we | --unlink-we | --link-lively | --unlink-lively
  termwall --stop                stop this folder's server
  termwall --version
"""

def editor_for(path: str) -> list[str] | None:
    """How to open a text file for editing: $TERMWALL_EDITOR / $EDITOR, else the app Windows
    associates with the extension, else VS Code, else Notepad (Linux: xdg-open). None = the
    association (os.startfile)."""
    for var in ("TERMWALL_EDITOR", "EDITOR"):
        if os.environ.get(var):
            import shlex
            return [*shlex.split(os.environ[var], posix=os.name != "nt"), path]
    if os.name != "nt":
        return ["xdg-open", path]
    import winreg
    ext = os.path.splitext(path)[1]
    try:  # the user's own "open with" choice, or a registered type with an open command
        winreg.CloseKey(winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                                       rf"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}\UserChoice"))
        return None
    except OSError:
        pass
    try:
        prog = winreg.QueryValue(winreg.HKEY_CLASSES_ROOT, ext)
        if prog:
            winreg.CloseKey(winreg.OpenKey(winreg.HKEY_CLASSES_ROOT, rf"{prog}\shell\open\command"))
            return None
    except OSError:
        pass
    code = shutil.which("code")
    return [code, path] if code else ["notepad.exe", path]


def open_config() -> str:
    """Create termwall.toml if needed and open it in an editor."""
    path = ensure_config()
    refresh_config(path)
    cmd = editor_for(path)
    try:
        if cmd is None:
            os.startfile(path)
        elif os.environ.get("EDITOR") and os.name != "nt":
            subprocess.call(cmd)  # a terminal editor takes this terminal
        else:
            subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=0x08000000 if os.name == "nt" and cmd[0].lower().endswith(".cmd") else 0)
    except OSError as e:
        return f"{path}\n(could not open an editor: {e}; set EDITOR, or open the file yourself)"
    return path


def theme_overrides(style_path: str | None = None, theme_path: str | None = None) -> dict:
    """Settings a switcher's theme.json "style" sets differently from termwall.toml: those win."""
    themed = _json_file(theme_path or THEME_FILE)
    sw = clean_style(themed.get("style") if isinstance(themed, dict) else None)
    own = {**STYLE_DEFAULTS, **clean_style(load_config(style_path or STYLE_FILE)[0])}
    return {k: v for k, v in sw.items() if own.get(k) != v}


def cmd_logos(args: list[str]) -> int:
    """termwall logos [WORD... | ID]: list or find logos; an exact id shows it."""
    cat = logo_catalog()
    if not cat.get("logos"):
        print("logos.js is missing next to termwall_api.py")
        return 1
    if len(args) == 1 and (args[0] in cat["logos"] or args[0] in cat["aliases"]):
        key = cat["aliases"].get(args[0], args[0])
        print(logo_preview(key))
        print(f"\n{key}  ({', '.join(cat['names'].get(key, []))})   use it: termwall --style logo {key}")
        print("its own colors: termwall --style logo_colors own")
        return 0
    found = find_logos(args) if args else list(cat["logos"])
    if not found:
        print(f"no logo matches {' '.join(args)!r}; termwall logos lists them all")
        return 1
    width = max(len(k) for k in found) + 2
    for k in found:
        print(f"  {k:{width}}{', '.join(cat['names'].get(k, []))[:70]}")
    print(f"{len(found)} logo(s). termwall logos ID shows one; termwall --style logo ID uses it")
    return 0


def cmd_logo_from(args: list[str]) -> int:
    """termwall logo-from PICTURE [--width N] [--ascii] [--colors N] [--keep-bg] [--out FILE] [--force]"""
    opts = {"--width": "40", "--colors": "9", "--out": None}
    flags, files, i = set(), [], 0
    while i < len(args):
        if args[i] in opts and i + 1 < len(args):
            opts[args[i]] = args[i + 1]
            i += 2
        elif args[i] in ("--ascii", "--keep-bg", "--force"):
            flags.add(args[i])
            i += 1
        elif args[i].startswith("--"):
            print(f"unknown option {args[i]}\n\n{USAGE}")
            return 2
        else:
            files.append(args[i])
            i += 1
    if len(files) != 1 or not opts["--width"].isdigit() or not opts["--colors"].isdigit():
        print("termwall logo-from PICTURE [--width N] [--ascii] [--colors N] [--keep-bg] [--out FILE] [--force]")
        return 2
    try:
        text, cols = picture_logo(files[0], int(opts["--width"]), "ascii" if "--ascii" in flags else "blocks",
                                  int(opts["--colors"]), "--keep-bg" in flags)
    except (OSError, ValueError, KeyError) as e:
        print(f"error: {files[0]}: {e}")
        return 1
    out = opts["--out"] or LOGO_FILE
    if os.path.exists(out) and not opts["--out"] and "--force" not in flags:
        print(ansi_logo(text, cols))
        print(f"\n{out} already exists: --force replaces it, --out FILE writes somewhere else")
        return 1
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text + "\n")
    print(ansi_logo(text, cols))
    print(f"\nwrote {out}: {len(text.splitlines())} lines, {len(cols)} colors")
    colors_toml = "logo_colors = [" + ", ".join(f'"{c}"' for c in cols) + "]"
    if opts["--out"]:
        print(f"its colors: {colors_toml}")
        return 0
    try:
        set_style("logo", "custom")
        set_style("logo_colors", ",".join(cols))
    except ValueError as e:
        print(f"could not change termwall.toml ({e}); set by hand: logo = \"custom\", {colors_toml}")
        return 1
    print(f'termwall.toml: logo = "custom", {colors_toml}')
    return 0


# termwall config / check / reset ... without the dashes
WORDS = {"config": "--config", "check": "--check", "style": "--style", "theme": "--theme", "themes": "--themes",
         "logos": "--logos", "logo-from": "--logo-from", "version": "--version", "stop": "--stop", "once": "--once", "selftest": "--selftest",
         "share": "--share", "update": "--update"}


if __name__ == "__main__":
    for stream in (sys.stdout, sys.stderr):  # the bar glyphs in --style / termwall.toml
        try:
            stream.reconfigure(encoding="utf-8")
        except (AttributeError, ValueError):
            pass
    if len(sys.argv) > 1 and sys.argv[1] in WORDS:
        sys.argv[1] = WORDS[sys.argv[1]]
    elif len(sys.argv) > 1 and sys.argv[1] == "reset":
        sys.argv[1:2] = ["--style", "reset"]
    if any(a in sys.argv for a in ("--help", "-h", "/?", "help")):
        print(USAGE)
        sys.exit(0)
    unknown = [a for a in sys.argv[1:2] if a not in (
        "--selftest", "--version", "--link-we", "--unlink-we", "--link-lively", "--unlink-lively", "--stop",
        "--themes", "--theme", "--logos", "--logo-from", "--style", "--once", "--config", "--check", "--share", "--update")]
    if unknown:
        sys.exit(f"unknown option {unknown[0]}\n\n{USAGE}")
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    if "--version" in sys.argv:
        print(f"termwall {__version__}")
        try:  # asked for, so a quick look is fine; offline is not an error
            rel = newer_release(timeout=3)
            if rel:
                print(f"{rel.get('tag_name', '').lstrip('v')} is out: termwall update")
        except (OSError, ValueError):
            pass
        sys.exit(0)
    if "--update" in sys.argv:
        sys.exit(cmd_update(sys.argv[sys.argv.index("--update") + 1:]))
    if "--link-we" in sys.argv:
        print(link_we())
        sys.exit(0)
    if "--link-lively" in sys.argv:
        print(link_lively())
        sys.exit(0)
    if "--unlink-lively" in sys.argv:
        print(unlink_lively())
        sys.exit(0)
    if "--unlink-we" in sys.argv:
        print(unlink_we())
        sys.exit(0)
    if "--stop" in sys.argv:
        print(f"stopped {stop_running()}")
        sys.exit(0)
    if "--share" in sys.argv:
        sys.exit(cmd_share(sys.argv[sys.argv.index("--share") + 1:]))
    if "--config" in sys.argv:
        print(f"opening {open_config()}")
        print("save it and the wallpaper follows within a second; termwall check if something doesn't change")
        sys.exit(0)
    if "--check" in sys.argv:
        problems = check_config(ensure_config())
        for line in problems:
            print(f"  {line}")
        print(f"{STYLE_FILE}: " + ("fine" if not problems else f"{len(problems)} problem(s)"))
        own = read_style()
        if read_theme() and own.get("theme", "auto") == "auto" and not own.get("colors"):
            print("note: theme \"auto\": the colors come from theme.json (livery); pick a theme here to override")
        over = theme_overrides()
        if over:
            print("note: theme.json also sets " + ", ".join(f"{k} = {_toml_value(v)}" for k, v in over.items())
                  + " for the wallpaper shown now, and that wins over this file"
                  + " (livery: that pack's own termwall look; livery termwall look PACK none drops it)")
        sys.exit(1 if problems else 0)
    if "--themes" in sys.argv:
        for name, t in THEMES.items():
            print(f"  {name:12} {t['bg']} {t['accent']} {t['secondary']}")
        print("  termwall --theme NAME   (auto = the distro's colors on Linux, mint on Windows)")
        sys.exit(0)
    if "--logo-from" in sys.argv:
        sys.exit(cmd_logo_from(sys.argv[sys.argv.index("--logo-from") + 1:]))
    if "--logos" in sys.argv:
        sys.exit(cmd_logos(sys.argv[sys.argv.index("--logos") + 1:]))
    if "--theme" in sys.argv:
        rest = sys.argv[sys.argv.index("--theme") + 1:]
        try:
            set_style("theme", rest[0] if rest else "")
        except (ValueError, IndexError) as e:
            sys.exit(f"error: {e}")
        print(f"theme: {rest[0]}" + ("  (theme.json exists: its palette wins)" if read_theme() else ""))
        sys.exit(0)
    if "--style" in sys.argv:
        rest = sys.argv[sys.argv.index("--style") + 1:]
        try:
            if rest and rest[0] == "reset":
                set_style("reset", "")
            elif len(rest) >= 2:
                set_style(rest[0], " ".join(rest[1:]))
            elif rest:
                raise ValueError(f"{rest[0]}: give a value ({STYLE_SPEC[rest[0]].help})" if rest[0] in STYLE_SPEC
                                 else f"unknown setting {rest[0]!r}")
        except ValueError as e:
            sys.exit(f"error: {e}")
        st = read_style()
        for k, check in STYLE_SPEC.items():
            v = st.get(k, "-")
            v = ",".join(v) if isinstance(v, list) else ("on" if v is True else "off" if v is False else v)
            if k == "colors":
                v = "set" if st.get("colors") else "-"
            print(f"  {k:8} {str(v) or '-':22} {check.help}")
        print("  termwall --style KEY VALUE | KEY default | reset    or edit it: termwall --config")
        sys.exit(0)
    if "--once" in sys.argv:
        s = Sampler()
        s.sample()
        time.sleep(1)
        s.sample()
        print(json.dumps(s.get(), indent=2, default=str)[:4000])
        sys.exit(0)
    serve()
