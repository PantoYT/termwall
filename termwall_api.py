"""termwall_api — live system stats for the termwall wallpaper.

Read-only JSON on 127.0.0.1 only (never the LAN). A background thread samples
once per second, so a request never blocks on psutil or nvidia-smi.

    python termwall_api.py              # serve on 127.0.0.1:9002
    python termwall_api.py --once       # print one snapshot and exit
    python termwall_api.py --selftest
    python termwall_api.py --style                  # the look: layout, clock, bars, rotation
    python termwall_api.py --style layout board     # change one of them (the page follows live)

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


# The look, from termwall.json next to the API (or $TERMWALL_STYLE); a "style" object inside
# theme.json wins over it, so a switcher (livery) can give every palette its own look.
STYLE_FILE = os.environ.get("TERMWALL_STYLE") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "termwall.json")
STYLE_CHOICES = {
    "layout": ("fetch", "board", "minimal"),   # fastfetch screen / btop-like boxes / a big clock
    "clock": ("blocks", "segments", "text"),   # solid pixel digits / 7-segment / a thin font
    "bars": ("blocks", "shade", "dots", "line"),
}
STYLE_DEFAULTS = {"layout": "fetch", "clock": "blocks", "bars": "blocks", "rotate": 0}


def clean_style(raw) -> dict:
    """Only known keys with allowed values; "rotate" = minutes between layouts (0 = off)."""
    out: dict = {}
    if not isinstance(raw, dict):
        return out
    for k, allowed in STYLE_CHOICES.items():
        if raw.get(k) in allowed:
            out[k] = raw[k]
    r = raw.get("rotate")
    if isinstance(r, int) and not isinstance(r, bool) and 0 <= r <= 1440:
        out["rotate"] = r
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


def set_style(key: str, value: str, path: str | None = None) -> dict:
    path = path or STYLE_FILE
    cur = clean_style(_json_file(path))
    if key == "rotate":
        if not value.isdigit() or int(value) > 1440:
            raise ValueError("rotate is minutes, 0-1440 (0 = off)")
        cur["rotate"] = int(value)
    elif key in STYLE_CHOICES:
        if value not in STYLE_CHOICES[key]:
            raise ValueError(f"{key} is one of {', '.join(STYLE_CHOICES[key])}")
        cur[key] = value
    else:
        raise ValueError(f"unknown setting {key!r} (layout, clock, bars, rotate)")
    with open(path, "a+", encoding="utf-8") as f:  # in place: no temp+rename (EFS-broken AppData)
        f.seek(0)
        f.truncate()
        f.write(json.dumps(cur, indent=2) + "\n")
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

    def top_processes(self, n: int = 6) -> list:
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
            if not request_ok(self.path, self.headers.get("Host"), token):
                counts["forbidden"] += 1
                self.send_response(403)
                self.send_header("Access-Control-Allow-Origin", "*")  # lets the page see the 403 and reload
                self.end_headers()
                return
            if path == "/stats":
                body = json.dumps({**sampler.get(), "theme": read_theme(), "style": read_style(),
                                   "page": page_version()}).encode()
            elif path == "/theme":  # tiny, polled often so palette switches land fast
                body = json.dumps({"theme": read_theme(), "style": read_style(), "page": page_version()}).encode()
            elif path == "/health":
                body = json.dumps(counts).encode()
            else:
                self.send_response(404)
                self.end_headers()
                return
            if path != "/health":
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


def serve() -> None:
    sampler = Sampler()
    sampler.sample()
    threading.Thread(target=sampler.run, daemon=True).start()
    try:
        httpd = ThreadingHTTPServer((HOST, PORT), make_handler(sampler, write_token()))
    except OSError as e:
        sys.exit(f"port {PORT} is taken ({e}) — is another termwall_api already running?")
    print(f"termwall api on http://{HOST}:{PORT}/stats")
    httpd.serve_forever()


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
            json.dump({**good, "style": {"layout": "minimal", "clock": "comic-sans", "bars": "dots"}}, f)
        st = read_style(sp, tp)
        check(st["layout"] == "minimal" and st["bars"] == "dots" and st["clock"] == "blocks",
              "theme.json style wins, unknown values dropped")
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
    print(f"selftest: {ok}/{ok + fail} OK")
    return 0 if fail == 0 else 1


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    if "--style" in sys.argv:
        rest = sys.argv[sys.argv.index("--style") + 1:]
        try:
            st = set_style(rest[0], rest[1]) if len(rest) >= 2 else None
        except ValueError as e:
            sys.exit(f"error: {e}")
        for k, v in read_style().items():
            print(f"  {k:7} {v}" + (f"   ({' | '.join(STYLE_CHOICES[k])})" if k in STYLE_CHOICES else "   (minutes, 0 = off)"))
        sys.exit(0)
    if "--once" in sys.argv:
        s = Sampler()
        s.sample()
        time.sleep(1)
        s.sample()
        print(json.dumps(s.get(), indent=2, default=str)[:4000])
        sys.exit(0)
    serve()
