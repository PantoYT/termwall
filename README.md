# termwall

A live, fastfetch-style terminal wallpaper for Wallpaper Engine — like panto-os, but as
a CLI. Windows logo, system info, per-core CPU bars, CPU/GPU history sparklines, RAM,
disks, network and top processes, refreshed every second. Designed for 1920×1080 and
scaled to whatever screen it lands on.

Default palette: `#171717` background, `#b5f4e7` mint, `#7faca3` teal, `#939fa1` grey.
Font: Cascadia Mono (falls back to Consolas).

## Live palette

Drop a `theme.json` next to the API (or point `TERMWALL_THEME` at one) and the
wallpaper recolors within a second — only CSS variables change, Wallpaper Engine
doesn't reload anything:

```json
{"bg": "#171717", "accent": "#b5f4e7", "secondary": "#7faca3",
 "text": "#939fa1", "dim": "#4b5252", "faint": "#242626"}
```

All six keys, `#rrggbb` only; anything invalid is ignored and the defaults stay.
`theme.json` is machine state, so it's gitignored. A switcher (mine is a private
"rice" script bound to macro keys) just rewrites this file.

## Parts

| File | What it is |
|---|---|
| `index.html` | the wallpaper (single file, no dependencies) |
| `termwall_api.py` | read-only stats server on **127.0.0.1:9002** (`/stats`), psutil + nvidia-smi |
| `termwall-api.vbs` | starts the API in the background without a console window (autostart) |
| `project.json` | Wallpaper Engine project file (type `web`) |

The browser can't read CPU/GPU usage, so the wallpaper polls the local API once per
second. The API binds to localhost only — it's never reachable from the LAN.

## Setup

```
pip install psutil
python termwall_api.py --selftest
```

1. Autostart: put a **shortcut** to `termwall-api.vbs` in `shell:startup` (the launcher finds `termwall_api.py` next to itself and uses `pythonw` from PATH).
2. Wallpaper Engine → **Open wallpaper** → **Open from file** → pick `index.html`, then
   choose the monitor.

Inside Wallpaper Engine the currently playing track also appears under the clock
(Wallpaper Engine's media integration); in a normal browser that line stays empty.

## Other systems

- **Wallpaper Engine is Windows-only**, but `index.html` is a plain page — any tool
  that can use a web page as a wallpaper works (e.g. Lively Wallpaper on Windows, web
  wallpaper plugins on Linux desktops).
- **The API is cross-platform** (psutil runs on Linux and macOS; `nvidia-smi` is
  optional). CPU and motherboard names are read from the Windows registry, so elsewhere
  they fall back to what `platform` reports. Only tested on Windows so far.
- The prompt adapts: PowerShell style on Windows, `user@host:~$` elsewhere.

## API

```
python termwall_api.py            # serve
python termwall_api.py --once     # print one snapshot
python termwall_api.py --selftest # 22 assertions
```

A background thread samples once per second (GPU every 2 s, processes every 3 s), so a
request never waits on psutil or nvidia-smi. Without `nvidia-smi` the `gpu` field is
`null` and the wallpaper shows `n/a`.
