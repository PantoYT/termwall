termwall 3.0.1 fixes `termwall host` and adds two commands for scripts.

## `termwall host`

- A large window that left the desktop visible paused the page behind it and the desktop went
  black; under a maximized window a transparent taskbar showed black. Now only a window that
  covers the whole monitor, taskbar included (a fullscreen game or video), pauses it.
- Screen 1 is the main monitor (it was the leftmost), the others follow from the left, so
  `[screen.N]` in termwall.toml and `--monitor N` match what Windows calls the main display.

## For scripts

- `termwall palette #bg #accent #secondary #text #dim #faint` writes the palette other programs
  give termwall (theme.json); `termwall palette accent=#ff5555` changes some of it, `termwall
  palette off` removes it, `termwall palette` shows it.
- `termwall stats` prints one whole sample as JSON.
- docs/integrating.md describes every way in and out: theme.json, `--style`, the HTTP endpoints
  and their fields, the page inside another host.

## Smaller things

- The page said "run python termwall_api.py" when the server was off; it says "start termwall".
