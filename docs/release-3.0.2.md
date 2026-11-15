termwall 3.0.2: `termwall host` grows up a little.

## `termwall host`

- `termwall host on` starts the host now and at every logon, `termwall host off` stops it and
  removes the autostart. `--monitor N` and `--audio-device NAME` given to `host on` are kept.
- `--audio-device NAME` fixes the visualizer to the playback device whose name contains NAME,
  for mixers where following the sound picks the wrong one.
- Under a maximized or fullscreen window the page now freezes instead of being hidden: the
  picture stays (a transparent taskbar shows the wallpaper, not black), and the page stops
  asking the server, drawing and animating until it is seen again. A maximized window pauses
  it again, which 3.0.1 had to give up.

## The page

- It implements Wallpaper Engine's `setPaused`, so the same freeze applies there.
- It waits for its screen's layout before showing anything, instead of flashing the default
  layout first.
