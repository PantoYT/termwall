# termwall

A live system-stats wallpaper in the style of [fastfetch](https://github.com/fastfetch-cli/fastfetch):
system info, per-core CPU, CPU and GPU history, memory, disks, network and top processes,
refreshed every second. Three layouts, your distro's logo and colors on Linux, and a palette
that can follow your wallpaper.

![termwall on Windows and twelve Linux distros](docs/distros.gif)

| fetch | board | minimal |
|---|---|---|
| ![fetch layout](docs/fetch-arch.png) | ![board layout](docs/board.png) | ![minimal layout](docs/minimal.png) |

The pictures come from the demo mode (`index.html?demo`) with made-up stats.

## Requirements

- **Windows 10 or 11**, 64-bit.
- **Something that shows a web page as a wallpaper**: [Lively Wallpaper](https://github.com/rocksdanister/lively)
  (free, open source: `winget install rocksdanister.LivelyWallpaper`) or
  [Wallpaper Engine](https://store.steampowered.com/app/431960/) (paid, Steam). termwall is
  the page; one of these puts it on the desktop.
- Linux: the stats work, the wallpaper host is untested. See [Linux](#linux).

## Install

Download `termwall-setup-X.Y.Z.exe` from [Releases](https://github.com/PantoYT/termwall/releases)
and run it. No admin rights, nothing else to install.

- Installs to `%LOCALAPPDATA%\Programs\termwall` with its own Python.
- Starts the stats server now and at every logon.
- Adds termwall to Wallpaper Engine (a link in `projects\myprojects`) and to Lively's library,
  whichever you have. In Lively, restart it once to see termwall in the library.
- Uninstall: Windows' Apps list. It stops the server and removes the autostart and both
  links.

A winget package is in review (`winget install termwall` will work once it's accepted).

## Configuration

termwall reads two optional files from its folder (`%LOCALAPPDATA%\Programs\termwall` after
the installer, the repo folder otherwise). Both are picked up within a second, without
reloading the wallpaper.

**The look**: `termwall.json`, written by `--style`:

| setting | values |
|---|---|
| `layout` | `fetch` (the fastfetch screen), `board` (boxes, like btop), `minimal` (the clock and one line) |
| `bars` | `blocks`, `shade`, `dots`, `line` |
| `rotate` | minutes between layouts, `0` = off |

```
python termwall_api.py --style                # what is set now
python termwall_api.py --style layout board   # change one
python termwall_api.py --style rotate 20      # a different layout every 20 minutes
```

(With the installer, `python` is `%LOCALAPPDATA%\Programs\termwall\python\python.exe`.)

**The palette**: `theme.json`, six `#rrggbb` colors:

```json
{"bg": "#171717", "accent": "#b5f4e7", "secondary": "#7faca3",
 "text": "#939fa1", "dim": "#4b5252", "faint": "#242626"}
```

Those are also the defaults on Windows. A wallpaper switcher can rewrite this file on every
switch: [livery](https://github.com/PantoYT/livery) (also by me) changes the wallpaper and
recolors Discord, Spotify, browsers, RGB and termwall in one keypress. It may add a
`"style"` object (`{"layout": "minimal"}`), so each palette can have its own look.

**What wins**, highest first:

| source | sets | applies |
|---|---|---|
| URL `?layout=…&bars=…` | layout, bars | always (handy in a browser) |
| URL `?demo&bg=…&accent=…` (all six) | palette | demo mode only |
| `"style"` in `theme.json` | layout, bars, rotate | always |
| `termwall.json` | layout, bars, rotate | always |
| `theme.json` palette | colors | always |
| your distro's colors | colors | Linux, when there is no `theme.json` |
| built-in defaults | everything | otherwise |

`TERMWALL_THEME` and `TERMWALL_STYLE` (environment variables) point the server at other
files. The port, `127.0.0.1:9002`, is fixed.

## Troubleshooting

| problem | what to check |
|---|---|
| the wallpaper is empty, "api offline" at the bottom | the stats server isn't running: start it (`termwall-api.vbs`, or log off and on after the installer). `http://127.0.0.1:9002/stats` in a browser answers `403` when it runs (that's the token, below) |
| the server won't start: "port 9002 is taken" | another termwall is already running (a manual one next to the installed one?) |
| GPU shows `n/a` | only NVIDIA cards are read (through `nvidia-smi`); AMD and Intel GPUs show `n/a` |
| Wallpaper Engine shows an old version after an update | WE plays its own copy of termwall: use the installer, or a junction ([Manual setup](#manual-setup)) |
| colors or layout don't change | check the JSON in `theme.json` / `termwall.json` is valid: an invalid file is ignored |

## How it works

A browser can't read CPU or GPU usage, so termwall has two halves: the page (`index.html`)
and a small local server (`termwall_api.py`, "the server" in this README) that the page asks
once a second.

| file | what it is |
|---|---|
| `index.html` | the wallpaper: one page, no dependencies |
| `logos.js` | distro logos (from fastfetch, see [Credits](#credits)) |
| `termwall_api.py` | the server: psutil for the stats, `nvidia-smi` for an NVIDIA GPU |
| `termwall-api.vbs` | starts the server without a console window (manual setup) |
| `project.json` | Wallpaper Engine's project file |

The server samples in the background (every second; GPU every 2 s, processes every 3 s), so
a request never waits. Endpoints: `/stats` (everything), `/theme` (palette and look only,
polled four times a second so a palette switch lands fast), `/health` (request counters).

`/stats` returns: `user`, `host`, `os`, `os_family`, `distro`, `kernel`, `board`,
`cpu_model`, `cores`, `uptime`, `shell`, `cpu` and `cores_pct` (%), `cpu_freq`, `ram` and
`swap` (used/total/percent), `gpu` (name, util, temp, vram, power; `null` without NVIDIA),
`disks`, `net` (down/up bytes/s), `local_ip`, `procs` (top six by CPU), `procs_total`,
`history` (last 60 s of cpu, gpu, down, up), plus `theme`, `style` and `page`.

**Privacy and security.** The server listens on `127.0.0.1` only, so nothing on your network
can reach it. It shows your user name, computer name, local IP and process names, so it also
doesn't answer every program on your PC: every request must carry a random token that the
server writes next to the page (`token.js`) at each start. The wallpaper can read that file;
a web page in your browser can't read your disk, so it gets `403`. The `Host` header must be
`127.0.0.1` or `localhost` (no DNS rebinding). Nothing is sent anywhere.

## Manual setup

From the repo, with Python 3.12+:

```
pip install psutil
python termwall_api.py --selftest
```

1. Autostart: a shortcut to `termwall-api.vbs` in `shell:startup` (Win+R → `shell:startup`).
2. Wallpaper Engine → **Open wallpaper** → **Open from file** → `index.html`. WE then plays
   its **own copy** in `projects\myprojects\termwall`, which never sees your edits. Replace
   it with a junction once, in **cmd** (not PowerShell: `mklink` is a cmd command):

   ```
   rmdir /s /q "C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\projects\myprojects\termwall"
   mklink /J "C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\projects\myprojects\termwall" "C:\path\to\termwall"
   ```

   (Your Steam library may be elsewhere.) The server can also make the link:
   `python termwall_api.py --link-we`, and for Lively `--link-lively`.
3. The page reloads itself when `index.html` changes, so edits show up live.

Uninstall: delete the shortcut from `shell:startup`, then `python termwall_api.py --unlink-we`
(or `rmdir` the junction: that removes only the link).

| command | does |
|---|---|
| `python termwall_api.py` | run the server |
| `--once` | print one sample and exit |
| `--selftest` | run the self-test |
| `--style [KEY VALUE]` | show or change the look |
| `--link-we` / `--unlink-we` | the Wallpaper Engine link |
| `--link-lively` / `--unlink-lively` | the Lively library entry |
| `--stop` | stop this folder's server |
| `--version` | the version |

**Building the installer**: `installer\build.ps1` (needs Inno Setup 6). GitHub Actions builds
it on every `v*` tag, installs it silently, checks the server answers, uninstalls it and
publishes the release. `tools\fetch_logos.py` rebuilds `logos.js`.

**Demo mode**: `index.html?demo` (made-up stats, no server), with `&layout=board`,
`&bars=dots`, `&distro=arch` or `&distro=cycle` (every logo, five seconds each).

## Linux

The server works on Linux: it reads the distro from `/etc/os-release`, the CPU from
`/proc/cpuinfo`, the board from DMI, and skips pseudo filesystems (snaps, Docker overlays).
Tested on Ubuntu 24.04. The page shows your distro's logo (about 25 distros; derivatives get
their parent's through `ID_LIKE`, the rest a Tux) and, without a `theme.json`, its colors.

**Not tested: what puts the page on the desktop.** Wallpaper Engine and Lively are
Windows-only; a tool that uses a web page as a wallpaper should work (KDE Plasma has
wallpaper plugins for that), but none has been tried. No installer for Linux either.

## Credits

- The look imitates [fastfetch](https://github.com/fastfetch-cli/fastfetch), and the distro
  logos in `logos.js` are fastfetch's (MIT, Copyright (c) 2021-2023 Linus Dierheimer,
  2022-2026 Carter Li; the full license is at the top of `logos.js`). termwall is not
  affiliated with fastfetch.
- The Windows logo is fastfetch's `windows_11` shape.

## License

MIT, see [LICENSE](LICENSE).
