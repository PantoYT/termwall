"""termwall_api — live system stats for the termwall wallpaper.

Read-only JSON on 127.0.0.1 only (never the LAN). A background thread samples
once per second, so a request never blocks on psutil or nvidia-smi.

    python termwall_api.py              # serve on 127.0.0.1:9002
    python termwall_api.py --once       # print one snapshot and exit
    python termwall_api.py --selftest

Dependencies: psutil (CPU/RAM/disks/net/processes). The GPU comes from
nvidia-smi if it exists; without it the "gpu" field is null.
"""

from __future__ import annotations

import datetime
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


PAGE_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "index.html")


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


def static_info() -> dict:
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
            if "cdrom" in part.opts or not part.fstype:
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

def make_handler(sampler: Sampler):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            path = self.path.split("?")[0]
            if path == "/stats":
                body = json.dumps({**sampler.get(), "theme": read_theme(), "page": page_version()}).encode()
            elif path == "/theme":  # tiny, polled often so palette switches land fast
                body = json.dumps({"theme": read_theme(), "page": page_version()}).encode()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            # The wallpaper runs from file:// (origin "null") — read-only data, so * is fine.
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
        httpd = ThreadingHTTPServer((HOST, PORT), make_handler(sampler))
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
    print(f"selftest: {ok}/{ok + fail} OK")
    return 0 if fail == 0 else 1


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    if "--once" in sys.argv:
        s = Sampler()
        s.sample()
        time.sleep(1)
        s.sample()
        print(json.dumps(s.get(), indent=2, default=str)[:4000])
        sys.exit(0)
    serve()
