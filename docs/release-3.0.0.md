termwall 3.0 is the same wallpaper as a compiled program. On Windows the installer no longer
brings a Python: it installs `termwall.exe` (Rust), and updates a 2.x install in place, settings
kept.

## One program instead of Python

- `termwall.exe` is the stats server and every command (`termwall config`, `check`, `share`,
  `logos`, `logo-from`, `update`, `--style` ...), with the same names, output and files as before:
  `termwall.toml`, `theme.json` (livery), `share.json`, `logo.txt`. `termwallw.exe` is the same
  program without a console window; that one starts at logon.
- The installer is a third of the size (4.7 MB, was 12.8) and removes the old Python on update.
- `termwall logo-from` reads PNG, GIF (first frame), JPEG, WebP and BMP by itself (no Pillow).
- `termwall --selftest` runs the same checks as the Python version did, inside the program.
- On Linux nothing changes yet: the package still runs `termwall_api.py`, kept in step with the
  Rust program (same settings, same page).

## New, experimental: `termwall host`

termwall without Wallpaper Engine or Lively: `termwall host` puts the page on the desktop itself,
behind the icons, one per monitor (`--monitor N` for one).

- The visualizer works there: the host listens to what plays on the PC. It takes the default
  playback device while that plays and otherwise the one that does, so mixers that spread
  programs over several devices (SteelSeries Sonar, VoiceMeeter) need no setting.
- The track line works too, from Windows' media session.
- A monitor covered by a fullscreen or maximized window pauses its page; with every page paused
  the music isn't captured.
- After an Explorer restart the wallpaper comes back by itself.
- When no stats server runs, the host is the server too.
- Not there yet: start at logon, a choice of audio device, Windows 10's desktop layout (untested).
  Tested on one Windows 11 machine with two monitors.

## Smaller things

- The spectrum in `fetch` is one and a half times as wide as the clock and stays that wide
  whatever the track title's length.
- `--stop` also stops the server when it was started as `termwallw.exe`.
