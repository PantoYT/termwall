"""termwall_api — live system stats for the termwall wallpaper.

Read-only JSON on 127.0.0.1 only (never the LAN). A background thread samples
once per second, so a request never blocks on psutil or nvidia-smi.

    python termwall_api.py              # serve on 127.0.0.1:9002
    python termwall_api.py --once       # print one snapshot and exit
    python termwall_api.py --selftest
    python termwall_api.py --style                  # the look: layout, bars, rotation
    python termwall_api.py --style layout board     # change one of them (the page follows live)
    python termwall_api.py --link-we                # show up in Wallpaper Engine (a junction in myprojects)
    python termwall_api.py --link-lively            # the same for Lively Wallpaper (its library)
    python termwall_api.py --unlink-we | --unlink-lively | --stop | --version

Dependencies: psutil (CPU/RAM/disks/net/processes). The GPU comes from
nvidia-smi if it exists; without it the "gpu" field is null.
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

__version__ = "1.2.0"

HOST, PORT = "127.0.0.1", 9002
HISTORY = 60  # seconds of CPU/GPU/net history for sparklines
CREATE_NO_WINDOW = 0x08000000 if os.name == "nt" else 0
# Optional palette written by an external switcher (e.g. rice). The page swaps its
# CSS variables when this changes — no wallpaper reload.
THEME_FILE = os.environ.get("TERMWALL_THEME") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "theme.json")
THEME_KEYS = ("bg", "accent", "secondary", "text", "dim", "faint")


def valid_theme(t) -> bool:
    return (isinstance(t, dict) and set(THEME_KEYS) <= set(t)
            and all(isinstance(t[k], str) and len(t[k]) == 7 and t[k][0] == "#"
                    and all(c in "0123456789abcdefABCDEF" for c in t[k][1:]) for k in THEME_KEYS))


# The look, from termwall.json next to the server (or $TERMWALL_STYLE); a "style" object inside
# theme.json wins over it, so a switcher (livery) can give every palette its own look. Every key
# is optional; anything unknown or invalid is dropped, so an old or hand-edited file can't break
# the page.
STYLE_FILE = os.environ.get("TERMWALL_STYLE") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "termwall.json")
LAYOUTS = ("fetch", "board", "minimal", "htop", "portrait")
SECTIONS = ("prompt", "logo", "info", "clock", "cpu", "cores", "gpu", "memory", "disks", "network",
            "procs", "swatches", "music", "status")
FONTS = ("cascadia", "jetbrains", "fira", "iosevka", "consolas", "system")
EFFECTS = ("crt", "glow")

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
    return v if isinstance(v, str) and re.fullmatch(r"auto|windows|tux|none|[a-z0-9-]{1,30}", v) else None
_logo.help = "auto | windows | tux | none | a distro id (arch, ubuntu, nixos, ...)"


def _palette(v):
    return {k: v[k] for k in THEME_KEYS} if valid_theme(v) else None
_palette.help = "six #rrggbb colors (bg, accent, secondary, text, dim, faint): edit termwall.json"

STYLE_SPEC = {
    "layout": _choice(*LAYOUTS),                        # fetch / board / minimal / htop / portrait
    "bars": _choice("blocks", "shade", "dots", "line"),
    "rotate": _int(0, 1440),                            # minutes between layouts, 0 = off
    "theme": _choice("auto", *THEMES),                  # a built-in palette (auto = distro colors / mint)
    "colors": _palette,                                 # your own palette (wins over "theme")
    "clock": _choice("24h", "12h"),
    "seconds": _bool,
    "date": _choice("long", "short", "iso", "none"),
    "hide": _subset(*SECTIONS),
    "prompt": _text,                                    # the prompt's command, "fastfetch --live" by default
    "private": _bool,                                   # hide user, computer name and local IP
    "font": _choice(*FONTS),
    "scale": _float(0.8, 1.1),
    "effects": _subset(*EFFECTS),
    "logo": _logo,
}
STYLE_DEFAULTS = {"layout": "fetch", "bars": "blocks", "rotate": 0, "theme": "auto", "clock": "24h",
                  "seconds": True, "date": "long", "hide": [], "private": False, "font": "cascadia",
                  "scale": 1.0, "effects": [], "logo": "auto"}


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


def read_style(style_path: str | None = None, theme_path: str | None = None) -> dict:
    style_path = style_path or STYLE_FILE
    theme_path = theme_path or THEME_FILE
    themed = _json_file(theme_path)
    return {**STYLE_DEFAULTS, **clean_style(_json_file(style_path)),
            **clean_style(themed.get("style") if isinstance(themed, dict) else None)}


def resolve_palette(style: dict, theme_path: str | None = None) -> dict | None:
    """theme.json (livery or another switcher) > "colors" > a built-in "theme" > None (the page
    then uses the distro's colors on Linux, its defaults on Windows)."""
    t = read_theme(theme_path) if theme_path else read_theme()
    if t:
        return t
    if style.get("colors"):
        return style["colors"]
    return THEMES.get(style.get("theme", "auto"))


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


def write_style(cur: dict, path: str) -> None:
    with open(path, "a+", encoding="utf-8") as f:  # in place: no temp+rename (EFS-broken AppData)
        f.seek(0)
        f.truncate()
        f.write(json.dumps(cur, indent=2) + "\n")


def set_style(key: str, value: str, path: str | None = None) -> dict:
    """One setting from the command line; "default" removes it, key "reset" clears them all.
    Keys this version doesn't know (written by a newer termwall or by hand) are kept."""
    path = path or STYLE_FILE
    raw = _json_file(path)
    cur = dict(raw) if isinstance(raw, dict) else {}
    if key == "reset":
        cur = {}
    elif key not in STYLE_SPEC:
        raise ValueError(f"unknown setting {key!r} ({', '.join(STYLE_SPEC)})")
    elif value.lower() == "default":
        cur.pop(key, None)
    else:
        if key == "colors":
            raise ValueError("colors: put six #rrggbb colors into termwall.json by hand, or use --theme")
        v = STYLE_SPEC[key](parse_value(key, value))
        if v is None:
            raise ValueError(f"{key}: {STYLE_SPEC[key].help}")
        cur[key] = v
    write_style(cur, path)
    return cur


PAGE_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "index.html")
TOKEN_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "token.js")


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


def read_gpu() -> dict | None:
    exe = shutil.which("nvidia-smi")
    if not exe:
        return None
    try:
        out = subprocess.run([exe, "--query-gpu=" + ",".join(GPU_FIELDS),
                              "--format=csv,noheader,nounits"],
                             capture_output=True, text=True, timeout=3,
                             creationflags=CREATE_NO_WINDOW).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    return parse_nvidia_smi(out.splitlines()[0]) if out.strip() else None


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
                    or part.mountpoint.startswith(("/snap", "/boot", "/var/lib/docker", "/run")):
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
            if path == "/stats":
                st = read_style()
                body = json.dumps({**sampler.get(), "theme": resolve_palette(st), "style": st,
                                   "page": page_version()}).encode()
            elif path == "/theme":  # tiny, polled often so palette switches land fast
                st = read_style()
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
    out of the first. Exclusive use instead (Windows), or no reuse at all elsewhere."""
    allow_reuse_address = False

    def server_bind(self):
        if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        super().server_bind()


def serve() -> None:
    sampler = Sampler()
    try:  # bind before anything else: a second copy must not rewrite token.js
        httpd = ExclusiveServer((HOST, PORT), make_handler(sampler, "pending"))
    except OSError:
        sys.exit(f"termwall's server is already running on {HOST}:{PORT} (check: http://{HOST}:{PORT}/health)")
    httpd.RequestHandlerClass = make_handler(sampler, write_token())
    sampler.sample()
    threading.Thread(target=sampler.run, daemon=True).start()
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
        sp, tp = os.path.join(td, "termwall.json"), os.path.join(td, "theme.json")
        check(read_style(sp, tp) == STYLE_DEFAULTS, "no files -> default look")
        set_style("layout", "board", sp)
        set_style("rotate", "15", sp)
        check(read_style(sp, tp)["layout"] == "board" and read_style(sp, tp)["rotate"] == 15, "termwall.json read")
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
        set_style("prompt", "neofetch", sp)
        set_style("theme", "nord", sp)
        st = read_style(sp, tp)
        check(st["hide"] == ["gpu", "procs"] and st["private"] is True and st["scale"] == 1.1
              and st["effects"] == ["crt"] and st["prompt"] == "neofetch", "settings parsed from the command line")
        check(resolve_palette(st, tp) == THEMES["nord"], "a built-in theme gives the palette")
        with open(sp, encoding="utf-8") as f:
            raw = json.load(f)
        raw["colors"] = dict(good, accent="#ff0000")
        raw["from_a_newer_termwall"] = 1
        write_style(raw, sp)
        check(resolve_palette(read_style(sp, tp), tp)["accent"] == "#ff0000", "own colors win over a theme")
        with open(tp, "w", encoding="utf-8") as f:
            json.dump(good, f)
        check(resolve_palette(read_style(sp, tp), tp) == good, "theme.json (livery) wins over everything")
        set_style("hide", "none", sp)
        with open(sp, encoding="utf-8") as f:
            raw = json.load(f)
        check(raw["hide"] == [] and raw.get("from_a_newer_termwall") == 1, "unknown keys survive a --style write")
        for bad_key, bad in (("scale", "9"), ("layout", "nope"), ("private", "maybe"), ("prompt", "x" * 61)):
            try:
                set_style(bad_key, bad, sp)
                check(False, f"bad {bad_key} refused")
            except ValueError:
                check(True, f"bad {bad_key} refused")
        set_style("theme", "default", sp)
        check("theme" not in json.load(open(sp, encoding="utf-8")), "default removes a setting")
        set_style("reset", "", sp)
        check(json.load(open(sp, encoding="utf-8")) == {}, "reset clears them all")
        try:
            set_style("layout", "nope", sp)
            check(False, "bad layout refused")
        except ValueError:
            check(True, "bad layout refused")
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
    print(f"selftest: {ok}/{ok + fail} OK")
    return 0 if fail == 0 else 1


USAGE = """termwall - a live system-stats wallpaper (github.com/PantoYT/termwall)

  termwall                       run the stats server here (the installer runs it at logon)
  termwall --style               every setting, its value and its choices
  termwall --style KEY VALUE     change one (KEY default: remove it, --style reset: all)
  termwall --theme NAME          a built-in palette (termwall --themes lists them)
  termwall --once                print one sample
  termwall --selftest            run the self-test
  termwall --link-we | --unlink-we | --link-lively | --unlink-lively
  termwall --stop                stop this folder's server
  termwall --version
"""

if __name__ == "__main__":
    if any(a in sys.argv for a in ("--help", "-h", "/?", "help")):
        print(USAGE)
        sys.exit(0)
    unknown = [a for a in sys.argv[1:2] if a.startswith("-") and a not in (
        "--selftest", "--version", "--link-we", "--unlink-we", "--link-lively", "--unlink-lively", "--stop",
        "--themes", "--theme", "--style", "--once")]
    if unknown:
        sys.exit(f"unknown option {unknown[0]}\n\n{USAGE}")
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    if "--version" in sys.argv:
        print(f"termwall {__version__}")
        sys.exit(0)
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
    if "--themes" in sys.argv:
        for name, t in THEMES.items():
            print(f"  {name:12} {t['bg']} {t['accent']} {t['secondary']}")
        print("  termwall --theme NAME   (auto = the distro's colors on Linux, mint on Windows)")
        sys.exit(0)
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
        print("  termwall --style KEY VALUE | KEY default | reset")
        sys.exit(0)
    if "--once" in sys.argv:
        s = Sampler()
        s.sample()
        time.sleep(1)
        s.sample()
        print(json.dumps(s.get(), indent=2, default=str)[:4000])
        sys.exit(0)
    serve()
