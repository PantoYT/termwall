termwall 2.0 runs on Linux.

## Linux (experimental)

- **The wallpaper itself**: `linux/termwall-layer.py` puts the page on the desktop as a
  wlr-layer-shell background surface, one WebKitGTK view per monitor. Works on Hyprland,
  Sway, river, niri and KDE Plasma; not on GNOME, which has no layer-shell.
- **One installer**: `bash linux/install.sh` installs the packages (pacman, dnf or apt), adds
  a `termwall` command, runs the stats server as a systemd user service and starts the
  wallpaper with your session (Caelestia's `hypr-user.lua`, Hyprland's `exec-once`, or an XDG
  autostart entry). `--remove` undoes it.
- **GPU without NVIDIA**: Intel (i915, xe) utilisation and clock from the kernel's per-client
  DRM statistics, AMD (amdgpu) utilisation, temperature, power and VRAM from sysfs. No root.
- **Tested** on Arch with Hyprland and Caelestia (Intel Arc), and on Fedora 44 and Ubuntu
  26.04 from a clean container image through `install.sh` to a screenshot on a headless Sway
  (`bash linux/test/run.sh`).

## Fixes

- The server no longer needs a minute to come back after a restart on Linux.
- A missing DMI file no longer stops the server from starting.
- Bind-mounted files are no longer listed as disks.

## Windows

Nothing changes in behaviour; the installer and `termwall update` work as in 1.5.
