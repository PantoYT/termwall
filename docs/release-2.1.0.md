termwall 2.1 can be installed as an Arch package.

## Arch package

- `packaging/aur/PKGBUILD` builds a `termwall` package: the files in `/usr/share/termwall`,
  `termwall` and `termwall-layer` commands, and a systemd user unit for the stats server.
  Until AUR registration reopens it is built from this repository:
  `cd packaging/aur && makepkg -si`.
- Checked in a clean Arch container through makepkg (the selftest runs in `check()`), namcap
  and pacman, then as a normal user on a headless Sway to a screenshot
  (`bash linux/test/aur.sh`).

## Settings outside the install folder

- When termwall is installed by a package (its folder is read-only), it keeps
  `termwall.toml`, `theme.json` and the logo in `~/.config/termwall`, and `token.js` and
  `share.json` in `~/.local/share/termwall`. The wallpaper loads the page from there, through
  links to the packaged files. A git clone and the Windows installer work as before.
- `termwall update` on a package says to update it with the package manager.

## Windows

Nothing changes.
