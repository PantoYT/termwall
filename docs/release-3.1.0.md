termwall 3.1.0: settings in the terminal, a doctor, and your own layout.

## termwall settings

Every setting in the terminal, grouped (look, logo, colors, sections, music, host). Arrows
change a value and it is written to termwall.toml at once, so the wallpaper is the preview. Next
to each value: where it comes from (default, termwall.toml, livery's theme.json, `[screen.N]`);
a row livery overrides says so. `s` switches between every screen and one `[screen.N]`. Pickers
for the logo (search, preview in its own colors), the theme (swatches), sections and effects;
an editor for your layout's blocks; host autostart, monitors and audio device. The mouse works
too.

## termwall doctor

Who holds port 9002 and from where, whether it is the installed version, whether the
wallpaper's requests are refused (another copy's token.js, the "connecting..." that stays), old
logon entries that start the Python server, the WebView2 runtime, the host's autostart,
termwall.toml problems and livery overrides, and which playback device the music is on against
what Wallpaper Engine or termwall host listens to. Exit code 1 when something is wrong.

## layout = "custom"

Your own grid: `blocks = [["logo", "info", "clock"], ["cpu:2", "memory"], ["procs"]]`, from the
board's boxes and the fetch's clock, logo, info and visualizer; `cpu:2` twice as wide. Per
monitor with `[screen.N]`.

Experimental, at your own risk: your own blocks as `[block.NAME]` tables with `text`, `file` or
`command` (its output as plain text, every N seconds; commands only with
`experimental_commands = true` in termwall.toml).

## Smaller things

- A silent first install (winget) no longer starts termwall at the end; it starts at the next
  logon. A silent update over an installed copy (`termwall update`) still starts the new server.
- A key termwall.toml doesn't know (from a newer version) is written above the `[screen.N]`
  tables, so a rewrite no longer moves it into the last table.
