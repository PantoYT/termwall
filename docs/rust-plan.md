# termwall in Rust: plan

termwall today is `termwall_api.py` (Python + psutil) plus `index.html`, shown by Wallpaper Engine
or Lively on Windows and by `linux/termwall-layer.py` on Wayland. winget rejected the Python
package as "scripted" (microsoft/winget-pkgs#446548); the moderator said a compiled port would
pass. This plan turns termwall into one compiled binary, `termwall`, that is both the stats server
and its own wallpaper host, so it needs neither Wallpaper Engine nor Lively.

Rules for the whole rewrite:

- Every phase ends in something that runs. The Python version keeps working (and keeps shipping)
  until the Rust one reaches parity; both read the same `termwall.toml`, `theme.json`,
  `share.json` and `logo.txt`, and write the same `token.js`.
- `index.html` stays the same page. It must keep working inside Wallpaper Engine and Lively, so the
  own host provides the Wallpaper Engine web API instead of the page learning a new one.
- Dependencies are taken when they do real work, and each one is listed below with its reason.
- Ported from `termwall_api.py` at commit `608f65b` (2.4.0).

## Phase 0: what exists already (checked 2026-11-13)

Each item says how it was checked. "Unverified" means exactly that.

### Wallpaper Engine's web API (what the page uses, and what the own host must provide)

Read at docs.wallpaperengine.io (web/audio/visualizer, web/audio/media, web/api/propertylistener):

- `wallpaperRegisterAudioListener(cb)`: register once; `cb` gets an array of exactly 128 floats,
  0-63 the left channel and 64-127 the right, each channel 64 frequency bands from bass (low
  index) to treble, usually 0.0-1.0 but sometimes above 1 (pages should clamp), "roughly 30
  times per second". Only delivered when `project.json` has `"supportsaudioprocessing": true`.
- Media integration: `wallpaperRegisterMediaStatusListener` (`enabled`),
  `wallpaperRegisterMediaPropertiesListener` (`title`, `artist`, `subTitle`, `albumTitle`,
  `albumArtist`, `genres`, `contentType`), `wallpaperRegisterMediaThumbnailListener` (`thumbnail`
  as a base64 PNG, `primaryColor`, `secondaryColor`, `tertiaryColor`, `textColor`,
  `highContrastColor`), `wallpaperRegisterMediaPlaybackListener` (`state`, compared with
  `window.wallpaperMediaIntegration.PLAYBACK_PLAYING / PAUSED / STOPPED`),
  `wallpaperRegisterMediaTimelineListener` (`position`, `duration`, seconds). Each fires when
  that part changes.
- `window.wallpaperPropertyListener` with `applyUserProperties(props)` (only changed properties,
  each as `{key: {value}}`, also on first load), `applyGeneralProperties` (e.g. `fps`),
  `setPaused(bool)`, and two directory callbacks termwall does not use.

termwall's page uses `wallpaperRegisterAudioListener`, `wallpaperRegisterMediaPropertiesListener`
and `wallpaperPropertyListener.applyUserProperties({screen: {value}})`. That is the surface the
own host provides first; the other media listeners are cheap to add with the same plumbing.

### Lively Wallpaper (open source, GPL-3.0: ideas only, no code copied into MIT termwall)

Read in rocksdanister/lively on GitHub (`src/Lively/Lively/Core/WinDesktopCore.cs`,
`src/Lively/Lively.Player.WebView2/Form1.cs`, `Lively.Common.Services/NAudioVisualizerService.cs`):

- Web wallpapers run in a separate player process (`Lively.Player.WebView2`, a WinForms window
  with WebView2) whose window Lively parents into the desktop.
- Windows 11 24H2 "raised desktop": Lively's source quotes Microsoft: Progman is now created with
  `WS_EX_NOREDIRECTIONBITMAP`, the `SHELLDLL_DefView` (icons) is a `WS_EX_LAYERED` child of
  Progman, and the wallpaper's `WorkerW` is a child of Progman z-ordered under the DefView. An app
  must put its own `WS_EX_LAYERED` child (alpha 255) under the DefView and above that WorkerW.
  Lively detects the new layout by `WS_EX_NOREDIRECTIONBITMAP` on Progman, sends `0x052C` to
  Progman as before, sets `WS_CHILD`, layered alpha 255, `SetParent(progman)`, then
  `SetWindowPos(hwnd, shellDLL_DefView, ...)` and checks that the WorkerW is the last child.
  Before 24H2 it parents into the separate top-level `WorkerW` found after the one holding the
  DefView. It hooks `EVENT_OBJECT_DESTROY` on the WorkerW and listens for `TaskbarCreated`
  (comparing Explorer's pid, since a DPI change also sends it) to re-attach after an Explorer
  restart.
- livery's `winwall.py` (the palette switcher that drives termwall) found the same thing from
  the other side: Wallpaper Engine's `WPEDesktop` windows live in Progman on 24H2. It also records
  a trap: a cross-process child window ties both processes' input queues together, and while
  Wallpaper Engine rebuilt its windows that froze livery's listener (2026-09-30). The own host is
  a cross-process child of Progman too, so its UI thread must never block (no synchronous calls
  into Explorer, no long work on the window thread).
- Audio: NAudio `WasapiLoopbackCapture` plus an FFT, 128 values handed to the page as
  `livelyAudioListener(...)`; now playing as `livelyCurrentTrack(json)`; properties as
  `livelyPropertyListener(name, value)`. No `wallpaperRegister*` call appears in the player's
  `Form1.cs`, which matches termwall's own status line ("not in Wallpaper Engine") under Lively:
  the visualizer only moves in Wallpaper Engine today.

### ColorWall

Unverified. Desktop Insights' automated analysis lists a Windows build 3.9.1 as a Rust app; the
Snap Store listing (developer keshavbhatt, version 3.1) describes a Qt/C++ wallpaper browser and
downloader. Closed source either way, so nothing to read or borrow. Not used for any decision.

### wry / tao on Linux, and the layer surface

- wry 0.57.2 (crates.io, 2026-11-10; its `Cargo.toml` on GitHub): Linux goes through
  `webkit2gtk = "=2.0.2"` (the webkit2gtk-4.1 API) and `gtk = "0.18"`, i.e. GTK 3. No GTK 4 /
  webkitgtk-6.0 backend. tao is GTK 3 as well.
- gtk3-rs is not archived (commits in 2026-09), but GTK 3 is the old line.
- The GTK 4 stack exists as crates: `gtk4` 0.11.5, `webkit6` 0.6.1 (WebKitGTK 6.0),
  `gtk4-layer-shell` 0.8.1 (pentamassiv/gtk4-layer-shell-gir, MIT). That is exactly the stack
  `linux/termwall-layer.py` already runs on Arch + Hyprland, so the Linux host uses these three
  directly instead of wry.
- Unverified: whether a Rust binary that links `gtk4-layer-shell` gets it loaded before
  `libwayland-client` without the `LD_PRELOAD` re-exec the Python script needs. The crate's docs
  and a test on Arch decide; the re-exec trick ports as-is if it is still needed.

### WebView2 on Windows

- `webview2-com` 0.39.1 is the binding wry itself uses on Windows; wry's `build_as_child` takes
  any parent window handle, so the Windows host can create its own Win32 windows (no tao) and let
  wry do the WebView2 environment and controller plumbing. If wry gets in the way of the desktop
  parenting (layered child, z-order), drop to `webview2-com` directly.
- Unverified until phase 2 runs: WebView2 rendering correctly inside a `WS_EX_LAYERED` child of
  Progman on 24H2. Lively does exactly this with WebView2 and ships it, which is the strongest
  evidence available without trying.

### skwd-paper / skwd-wall (liixini, MIT, Rust)

Read on GitHub: skwd-paper is a Wayland wallpaper daemon (layers background/bottom/top/overlay)
that renders images, video and Wallpaper Engine *scenes* with Vulkan (`paper-vk`). Its README
mentions no web wallpapers. skwd-wall is a selector and engine on top, with Matugen theming.
`crates/paper-audio` depends on `cpal` and `ffmpeg-the-third`; `spectrum.rs` keeps left/right
bands for 16, 32 and 64 bands (`BAND_COUNTS`) from a 2048-sample window with attack/release
smoothing. `pulse.rs` (capture) and `spectrum.rs` are the parts phase 3 may borrow, with
attribution in the file header and in the README, under MIT.

### Matugen, wallust

Palette generators (Material You from a wallpaper; a pywal successor). They overlap with
livery's job, not termwall's. If wanted later: a small adapter that turns their output into
termwall's six-color `theme.json`. Not part of this rewrite.

## The binary

```
termwall                  phase 1: the stats server (as termwall_api.py)
                          phase 2+: the server and the wallpaper host together
termwall serve            the server only (for people who keep Wallpaper Engine or Lively)
termwall host [...]       the host only (development; --monitor, --layer on Linux)
termwall <every command termwall_api.py has>   unchanged names, output and exit codes
```

Source layout (repository root, next to `index.html`):

```
Cargo.toml
src/main.rs        CLI: the same commands, words and messages as termwall_api.py
src/paths.rs       where the page, termwall.toml, theme.json, token.js, share.json live
src/style.rs       termwall.toml: settings spec, read/write with comments, [screen.N], check
src/theme.rs       theme.json, built-in palettes, palette resolution
src/logos.rs       logos.js catalog, logo.txt, logo images, ANSI preview
src/picture.rs     logo-from: picture -> logo text with $1..$9
src/png.rs         PNG writer, CRC32
src/qr.rs          QR encoder (byte mode, level M, versions 1-10)
src/sampler.rs     CPU, cores, RAM, swap, disks, network, processes, history
src/gpu.rs         nvidia-smi, Linux DRM (Intel fdinfo, AMD sysfs)
src/http.rs        the tiny HTTP/1.0 server, exclusive bind, token checks
src/server.rs      /stats /theme /health /logo-image, token.js
src/share.rs       termwall share: LAN server, key, QR, rate limit
src/integrate.rs   --link-we / --link-lively / --stop / --update / editor
src/selftest.rs    the selftest groups, shared by `termwall --selftest` and `cargo test`
src/host/          phase 2 and 3: windows.rs, linux.rs, audio, media
```

### Phase 1 dependencies

| crate | why |
|---|---|
| `sysinfo` | psutil's job: CPU per core, RAM, swap, disks, network counters, processes, boot time, on Windows and Linux |
| `serde_json` (`preserve_order`) | the JSON the page reads; also the dynamic value type for settings |
| `toml` (`preserve_order`) | reading `termwall.toml`, keeping unknown keys and `[screen.N]` in file order; writing stays the hand-made commented renderer |
| `image` (PNG, GIF, JPEG, WebP, BMP decoders only) | `logo-from` reads more than PNG without Pillow; pure Rust |
| `miniz_oxide` | zlib for the PNG writer (share icons) |
| `getrandom` | the per-start token and the share key |
| `sha2` | `termwall update` checks the installer's SHA-256 |
| `unicode-normalization` | `termwall logos WORD` folds accents like Python's NFKD |
| `socket2` + `windows-sys` | `SO_EXCLUSIVEADDRUSE` on Windows (a second server must not share the port); registry (Windows version, CPU, board, file associations); `ShellExecuteW` for `termwall config`; the paging file's use (`NtQuerySystemInformation`), which is what psutil reports as swap |

Not taken: an HTTP framework (the server answers five GET paths; a 150-line `std::net` server is
smaller than any framework and easier to audit) and an HTTPS client (`termwall update` and
`--version` call `curl`, which ships with Windows 10 1803+ and practically every Linux; a TLS
stack in Rust means `ring` or `aws-lc`, which need a C compiler to build).

### Toolchain

Local development works with the `x86_64-pc-windows-gnu` toolchain from rustup (self-contained,
no Visual Studio needed). CI and releases build
`x86_64-pc-windows-msvc` on `windows-latest` (the usual target for WebView2, and what users
expect), plus `x86_64-unknown-linux-gnu` for the AUR package. Phase 1 code has no C, so both
targets build the same sources.

### Paths (the Rust side of APP_DIR, CONFIG_DIR, DATA_DIR)

The app folder is the first that holds `index.html`: `$TERMWALL_HOME`; the binary's folder; up
to three of its parents (so `target/debug/termwall` in a checkout finds the repository root);
`/usr/share/termwall` and `/usr/local/share/termwall` on Linux. A `SYSTEM-INSTALL` file there
means a package: settings go to `$XDG_CONFIG_HOME/termwall`, `token.js` and `share.json` to
`$XDG_DATA_HOME/termwall`, exactly as the Python version does. `TERMWALL_STYLE` and
`TERMWALL_THEME` still override the two files.

### Deliberate differences from the Python version

- `logo-from` reads PNG, GIF (first frame), JPEG, WebP and BMP itself; the Pillow hint goes.
- `local_ip` takes the address of the interface with the default route (the docstring of
  `lan_addresses` already meant that); the Python version took the first up adapter in psutil's
  order, which can be a WSL or Hyper-V switch.
- Disk usage percent is `used / total`; psutil on Linux leaves out root-reserved blocks
  (`used / (used + available)`), so ext4 disks can read a few percent lower than before.
- Caches (theme, logo, settings) are keyed by path as well as mtime (the Python theme cache was
  keyed by mtime only).
- `$TERMWALL_EDITOR` / `$EDITOR` with quotes on Windows: the quotes are removed before running
  the program (Python's non-POSIX `shlex` kept them).
- `--stop` stops the other running copies of this binary (same executable path) and the Python
  server of the same folder, so an update from the Python version can replace it.
- `termwall update` on an installed copy recognises the installer by `unins000.exe` next to the
  binary instead of `python\python.exe`; a clone is told to `cargo build --release` as well.
- A TOML datetime, `inf` or `nan` in termwall.toml survives a rewrite (the Python version
  dropped such a line silently, because `_toml_value` could not write it back).
- `termwall share`'s firewall hint names termwall instead of Python, and only on Windows.
- Redirected output has LF line ends (Python printed CRLF on Windows); files keep CRLF.
- `$TERMWALL_PORT` (1024-65535) moves the stats server off 9002: for tests next to a running
  server, and for the own host, which can tell its page the port. Wallpaper Engine and Lively
  pages always ask 9002.

### Tests

The selftest assertions (126 in 2.4.0, 127 with the share fix) become `cargo test` tests, one
Rust test per group, every assertion kept with its message. Tests that changed environment
variables or module globals in Python call the same function with the value passed in (Rust
runs tests in parallel). `termwall --selftest`
stays as a command that runs the same checks inside the shipped binary, so a user without cargo
can still run it.

## Phase 1 status (2026-11-13): at parity, not released

Build and test (any stable Rust from rustup; on Windows the GNU or the MSVC toolchain):

```
cargo test                      # 23 tests: the 127 selftest assertions in 20 groups, their count,
                                # the stats server end to end, share font names
cargo build --release           # target\release\termwall.exe, 2.5 MB, no runtime to install
target\release\termwall.exe --selftest
```

How parity was checked, beyond the tests:

- The same `--style` / `--theme` sequence run through both versions writes a byte-identical
  termwall.toml (CRLF on Windows, every comment, `[screen.N]` tables, unknown keys at the end).
  `--style`, `--check`, `--themes`, `logos` print the same text; the only difference in
  `--help` is the logo-from line (more picture formats).
- `--once` from both: the same 28 keys with the same types. Three differences were found and
  fixed: swap (sysinfo reports commit charge; now the paging file's use, equal to psutil's to the
  byte), disk order (now C:, D:, E: like psutil) and the idle process (sysinfo names it
  `[System Process]`, now hidden like psutil's `System Idle Process`).
- The release binary run as the real server on `TERMWALL_PORT=19002` with `TERMWALL_HOME` on a
  copy of the page: token.js written there, `/health` without a token, 403 without one, `/stats`
  with 31 keys, history growing, per-process CPU, `termwall share` turned on and off live by
  editing share.json. The unchanged index.html, served by the share server, rendered the fetch
  layout with these stats in a browser, without console errors. Started on 9002 while the Python
  server runs, it says so and exits 1 without touching token.js.
- Linux: the same tests in a Debian trixie container: 22 passed, 128 assertions before the
  share fix (129 with it; the two
  extra are the package symlink checks). `cargo clippy` is clean for both targets.

Not done in phase 1, on purpose: the installer, CI, PKGBUILD and winget manifests still ship the
Python version (phase 4), and the README describes the Python version.

## Phase 2: Windows host

One WebView2 per monitor behind the desktop icons.

1. Desktop attach, 24H2 and earlier, following Lively's description above: detect the raised
   desktop (`WS_EX_NOREDIRECTIONBITMAP` on Progman), send `0x052C`, then either a layered child
   of Progman right under `SHELLDLL_DefView` or a child of the old top-level `WorkerW`.
2. Per-monitor DPI awareness v2 for the process; one window per monitor from
   `EnumDisplayMonitors`, placed in Progman's client coordinates (the virtual screen's origin can
   be negative); `WM_DISPLAYCHANGE` and `WM_DPICHANGED` rebuild the set.
3. Explorer restart: `RegisterWindowMessage("TaskbarCreated")` on a hidden top-level window,
   compared with Explorer's pid; `SetWinEventHook(EVENT_OBJECT_DESTROY)` on the WorkerW. Both
   re-run the attach.
4. Pause under a fullscreen app: `SHQueryUserNotificationState` plus a check of the foreground
   window's monitor rectangle; paused means `TrySuspend` on the WebView2 (and audio capture
   stopped), resumed on change.
5. Page: `file:///<app>/index.html?screen=N` so `token.js` keeps working as for Wallpaper Engine;
   `AddScriptToExecuteOnDocumentCreated` defines `wallpaperRegisterAudioListener`,
   `wallpaperRegisterMediaPropertiesListener` (and the other media listeners) and calls
   `wallpaperPropertyListener.applyUserProperties({screen: {value: "N"}})`.
6. Audio: WASAPI loopback on the default render device (`windows` crate), 2048-sample Hann
   window, FFT, 64 log-spaced bands per channel, about 30 updates a second, pushed with
   `PostWebMessageAsJson` (no script compiled per frame); stopped while paused or silent.
7. Now playing: `GlobalSystemMediaTransportControlsSessionManager` (WinRT through the `windows`
   crate), events turned into the media listener calls.
8. Check: the visualizer moves without Wallpaper Engine; Explorer killed and restarted brings the
   wallpaper back; a fullscreen game pauses it (WebView2 process CPU near zero); two monitors with
   different DPI line up; icons stay clickable on top.

### Phase 2 status (2026-11-15): the page, the music and now playing work; pausing is open

`termwall host [--monitor N] [--seconds N] [--window]` (`src/host/`). It uses the stats server
already running, or becomes it when none is (then `termwall host` alone is the whole of termwall).
With `TERMWALL_PORT` set it tells the page where the stats are.

Done and seen working on Windows 11 build 26340, next to a running Wallpaper Engine:

- The page in a WebView2 (through `wry`) behind the icons, one window per monitor, sized from the
  monitor's rectangle in physical pixels. A `--seconds` run asks each page for its status line
  and prints it: the page reported its layout, `screen 1` and fresh stats from the server.
- Where the window goes turned out different from the description in step 1. On this build a
  window of another process can't be moved into Progman with `SetParent` (access denied), and
  Wallpaper Engine's own windows are children of the `WorkerW` that is a child of Progman. The
  host creates its window as a child of that `WorkerW` from the start, on top of its siblings
  (so it covers Wallpaper Engine's wallpaper on that monitor while it runs). The older layout
  (top-level `WorkerW`) uses the same code path and is untested.
- Wallpaper Engine's web API for the page (`src/host/we_api.js`): the audio listener and the five
  media listeners can be registered; audio, the track's properties, the playback state and the
  status are fed. A listener registered after an event still hears the latest one.
- Now playing (`src/host/media.rs`): Windows' media session
  (`GlobalSystemMediaTransportControlsSessionManager`), asked once a second instead of wired to
  events, sent on every change and again every ten seconds. The page showed the title and artist
  of what a browser tab was playing. Timeline and thumbnail are not sent.
- Back after the wallpaper windows are lost: a hidden window of the host's own checks every two
  seconds that every wallpaper window still exists and still matches its monitor's rectangle, and
  builds them all again otherwise. Tested by destroying the windows from inside (a debug build
  with `TERMWALL_TEST_REBUILD`): both came back with audio and the track line. Then with
  Explorer killed and started again: the host waited ("no desktop"), one build failed half way
  while the new desktop was still settling, and the next one two seconds later put both windows
  back. Not tested with a monitor unplugged or a DPI change.
- Audio (`src/host/audio.rs`): WASAPI loopback, 2048-sample Hann window, own FFT, 64 bands per
  channel, about 30 frames a second while something plays, two a second of zeros while nothing
  does, no capture at all while no screen has `visualizer` on. With music playing the page's
  status read `audio` and its spectrum bars were drawn (94 % for the bass bars at one moment).
  What differs from step 6:
  - Bands are equal-width up to 20 kHz, not log-spaced: Wallpaper Engine doesn't document its
    layout, and the page's bar mapping and treble lift were tuned against data shaped like that.
    Unverified against Wallpaper Engine itself; a test tone through it would settle it.
  - An automatic gain: the loudest band sits near 0.9 whatever the system volume.
  - Which device: the default one while it plays, else the loudest one that does. With a mixer
    that spreads programs over virtual devices (SteelSeries Sonar here) the music was not on
    the default device, and Wallpaper Engine, set to one fixed device, heard silence.
  - Frames go to the page with `ExecuteScript`, not `PostWebMessageAsJson` (what `wry` offers
    directly); the cost of that is not measured yet.

A trap for testing: a program started from a folder that carries a Low integrity label runs at
Low integrity and may not create windows inside Explorer's (access denied), nor write to
`%TEMP%`. `icacls <folder>` shows such a label. Run test builds from a folder without one.

New dependencies, Windows only: `wry` (the WebView2 plumbing; `default-features = false`, so no
GTK or tao) and `windows` (COM for WASAPI and WinRT for the media session, the version `wry`
already brings in).

Pause (step 4, done differently): twice a second the host asks which window is at the middle of
every monitor; when that window covers the whole monitor, taskbar included (a fullscreen game or
video), the monitor's WebView2 is hidden, and shown again afterwards. With every page paused the
music isn't captured either. A first version paused under any window covering nine sample points:
wrong twice, since a large window that leaves the desktop visible paused it (a black desktop),
and under a maximized window a transparent taskbar showed black. Since 3.0.2 nothing is hidden:
the host calls the page's `wallpaperPropertyListener.setPaused` (Wallpaper Engine's own call),
the page stops its timers, its animation frames and its CSS animations and keeps its picture,
so a maximized window pauses it too and a transparent taskbar shows the frozen wallpaper.

Open: the real-world checks named above, whether `termwall` without arguments starts the host (`termwall host on` is the
autostart for now, `--audio-device NAME` the override), CPU and memory next to Wallpaper Engine, a manifest in the exe.
With the GNU toolchain `WebView2Loader.dll` has to sit next to the exe; MSVC links it in.

## termwall settings and termwall doctor (next release)

`termwall settings` is a keyboard-first TUI (ratatui 0.30 over crossterm 0.29, mouse optional)
over the same spec that drives `--style`: its rows are `style::spec()` sorted into groups (look,
logo, colors, sections, music, host), and a test fails when a setting has no group. Every change
goes through the writer `--style` uses, at once; the wallpaper polls `/theme` four times a
second, so it is the preview. Next to each value: where it comes from (default, termwall.toml,
livery's theme.json "style", `[screen.N]`), from the same layering `read_style` uses, so the two
can't disagree. A key livery overrides says so on its row. `s` switches the scope between all
screens and one `[screen.N]` table. Pickers: logos (search, preview in the logo's own colors),
themes (swatches), checklists (hide, effects), a line editor (prompt, colors, logo colors).
Host rows (Windows): autostart on/off, monitor, audio device; they rewrite the autostart entry.

`termwall doctor` gathers facts, then judges them in a function tests can feed: who holds the
port (process and path), the running version against this binary and the installed copy,
whether the wallpaper's requests are refused (another token.js), old logon entries that start
the Python server, the WebView2 runtime, the host autostart, termwall.toml problems and livery
overrides, and for the visualizer which output device plays against what Wallpaper Engine (its
config.json) or the host listens to. Plain text, exit 1 on a real problem.

Status (2026-11-21): both written, on main, not released. Tests cover the groups against the
spec, stepping values, typed lines, sources and write-through (main table and `[screen.N]`,
livery winning), the TUI driven by keys on ratatui's test backend, and doctor's judgement on
made-up facts (the Python server ahead of the installed one, a refused token, music on
another device than Wallpaper Engine hears). On this PC: doctor reports correctly; the TUI
draws in Windows Terminal; keys were tried only on the test backend.

## Layout custom (3.1)

`layout = "custom"`: rows of blocks from `blocks` in termwall.toml (a style key like any other,
so livery's per-pack look and `[screen.N]` can set it too). The page clones each block from a
`<template>` (the board's boxes, the fetch's clock, logo, info, visualizer); `put()` and `tick()`
already fill every element by name, so no data code changed. A row of only unboxed blocks takes
its natural height, the others share the rest; `name:N` gives a block N shares of its row.

Your own blocks, experimental: `[block.NAME]` with `text`, `file` or `command`, read by a thread
of the server (src/blocks.rs) and handed to the page through `/theme` on 127.0.0.1 as plain
text (`textContent`, never HTML). Commands need `experimental_commands = true` in termwall.toml;
theme.json's "style" goes through the spec, which has no such key, so a switcher can't turn
commands on or define blocks. Each command: `cmd /C` or `sh -c` in termwall.toml's folder,
10 s at most (the whole process tree is killed), 40 lines. `termwall share` doesn't send them.
The Python server (Linux, 2.4) doesn't know `custom` and shows `fetch`.

## Phase 3: Linux host

`gtk4` + `webkit6` + `gtk4-layer-shell`, the same as `linux/termwall-layer.py` with the same
options (`--monitor`, `--layer background|bottom`), monitors numbered in the compositor's order
for `[screen.N]`. Audio from PulseAudio / PipeWire's pulse server (the default sink's monitor),
borrowing skwd-paper's `pulse.rs` and `spectrum.rs` with attribution if they fit; the same
128-value array to the page via `WebView::evaluate_javascript` or a user-content message.
Now playing via MPRIS on D-Bus. `linux/install.sh` and the systemd user unit switch to the binary.
Check on Arch + Hyprland, where the Python layer was tested.

## Phase 4: shipping

- Installer: Inno Setup installs `termwall.exe`, `index.html`, `logos.js`, `fonts/`; no `.cmd`
  or `.py` as the program; autostart is the exe itself (no `.vbs`); `--stop` before replacing
  files; uninstall stops it and removes the PATH entry.
- AUR: a source PKGBUILD (`cargo build --release --locked`) and optionally `termwall-bin`.
- winget: manifests generated by `winget/make_manifest.py` from the release asset, as now.
- CI: `cargo test` on Windows and Linux, `cargo build --release` for both, then the existing
  silent-install check against the exe.
- livery: the host becomes an engine for a side monitor next to Wallpaper Engine (livery starts
  `termwall host --monitor N` instead of switching WE to the web wallpaper).

## Open questions

- The version number of the first Rust release (the crate says `3.0.0-alpha.1` until then).
- Whether `termwall` with no arguments should start the host by default from phase 2 on, or only
  after a setting (`host = true` in termwall.toml), so Wallpaper Engine users see no change.
