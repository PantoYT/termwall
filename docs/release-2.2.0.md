termwall 2.2 has every logo fastfetch has, in their own colors if you want.

## All of fastfetch's logos

- 540 logos instead of 25: hundreds of distros, plus macOS (several generations), Apple,
  ChromeOS, Android, Windows 95, 7, 8/10, 11 and Server 2025, FreeBSD, OpenBSD, NetBSD,
  DragonFly, Haiku, Solaris, OS/2 Warp, Amiga and more. `logos.js` grows from 28 kB to
  396 kB and stays one local file.
- `termwall logos` lists them, `termwall logos mac` finds the ones matching every word,
  `termwall logos apple` shows one in the terminal in its own colors.
  `termwall --style logo apple` uses it.
- `termwall.toml` names the 25 featured logos and points to `termwall logos` instead of
  listing all 540.
- On Linux the logo is found by the os-release `ID`, `ID_LIKE` or any name fastfetch knows
  for it, so far more distros get their own logo instead of a parent's or the Tux.
- `windows-vista` and `windows-xp` are names for `windows-7`: fastfetch has no logo of
  their own, and they had the same waving flag.
- An unknown logo id is now refused by `termwall --style` and named by `termwall check`.

## The logo's own colors

- `logo_colors` now works for every logo, not just pictures, and is separate from the
  palette: the theme, livery and `colors` color the rest of the screen as before.
  - `own`: the logo's own colors, fastfetch's (the rainbow apple, ChromeOS's four colors)
    or a picture's;
  - `palette`: the palette's, as text logos always were;
  - a list of up to nine `#rrggbb` for `$1`, `$2`... of a text logo, `logo.txt` included;
  - `auto` (the default; `image` from older files means the same): a picture in its own
    colors, text logos in the palette, so nothing changes until you set it.
- `theme = "logo"` takes its two colors from any of the 540 logos.

## Smaller

- On macOS the automatic logo is macOS instead of Windows.
- Wide logos shrink to fit next to the info like tall ones already did.
- `docs/distros.gif` shows logos beyond Linux too; `tools/make_gifs.py --only distros`
  rebuilds it.
- `tools/fetch_logos.py` reads fastfetch from one tarball of the pinned commit.
