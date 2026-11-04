termwall 2.4 draws the music playing on the PC.

## The visualizer

- `visualizer = "bars"` puts a spectrum over the clock in `fetch` and under the logo in
  `logo`; `pulse` makes the logo beat with the bass (layout `logo`); `both` does the two.
  Off by default.
- The music comes from Wallpaper Engine (`wallpaperRegisterAudioListener`, 64 bands a
  channel), so it moves only there; in a browser nothing moves, and `?demo&visualizer=both`
  plays made-up music to look at.
- Bass on the left. Hearing is logarithmic, so the low bands get bars of their own and the
  high ones share. In `fetch` the bars move every display frame toward the latest levels
  (quick up, slower down) instead of jumping with WE's ~30 packets a second, and the treble
  is lifted against the bass, like analyzers weighted for pink noise.
- `sound_bars` sets how many bars `fetch` has (4-32, 16). `hide = ["sound"]` hides them.
- The bars take the logo's colors: `logo = "apple"` with `logo_colors = "own"` gets the
  rainbow, other logos the palette's accent to secondary.
- The status line says whether the music gets there: `no audio from WE`, `silence` (data
  comes but it's all zeros: WE listens to another output device than the one the music plays
  on; WE's settings pick it) or `audio`.

## Fixes

- After the stats server restarted (a new token), a page whose `token.js` came from the
  cache stayed on "connecting" for good: it now reads `token.js` again past the cache and
  carries on without a reload.
- `termwall --style` took a number from the command line only for `rotate`; `logo_width`
  (and now `sound_bars`) were refused.
