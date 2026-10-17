#!/usr/bin/env bash
# termwall on Linux, experimental: the `termwall` command, the stats server as a systemd user
# service, and the wallpaper started with the Hyprland session.
#
#   bash linux/install.sh            # install (safe to run again)
#   bash linux/install.sh --remove   # undo all of it
#
# What it touches, nothing else:
#   ~/.local/bin/termwall                         command (calls termwall_api.py in this folder)
#   ~/.config/systemd/user/termwall-api.service   stats server, starts at login
#   one marked block in ~/.config/caelestia/hypr-user.lua (Caelestia, Hyprland Lua config)
#     or in ~/.config/hypr/hyprland.conf (plain Hyprland, exec-once)
#     or ~/.config/autostart/termwall.desktop (KDE, GNOME and other XDG desktops)
#   one marked line in ~/.bashrc if ~/.local/bin is not on PATH yet
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$HOME/.local/bin/termwall"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT="$UNIT_DIR/termwall-api.service"
LUA="$HOME/.config/caelestia/hypr-user.lua"
CONF="$HOME/.config/hypr/hyprland.conf"
DESKTOP="$HOME/.config/autostart/termwall.desktop"
BEGIN="-- termwall begin"
END="-- termwall end"
LAYER="python3 $DIR/linux/termwall-layer.py"

strip_block() {  # file, begin, end: remove a marked block if it is there
    [ -f "$1" ] && sed -i "\|^$2\$|,\|^$3\$|d" "$1" || true
}

if [ "${1:-}" = "--remove" ]; then
    systemctl --user disable --now termwall-api.service 2>/dev/null || true
    rm -f "$UNIT" "$BIN" "$DESKTOP"
    systemctl --user daemon-reload
    strip_block "$LUA" "$BEGIN" "$END"
    strip_block "$CONF" "# termwall begin" "# termwall end"
    sed -i '/# termwall: ~\/.local\/bin on PATH/d' "$HOME/.bashrc" 2>/dev/null || true
    pkill -f "linux/termwall-layer.py" 2>/dev/null || true
    echo "termwall removed (the folder $DIR stays)"
    exit 0
fi

# Packages: psutil for the server; GTK4, WebKitGTK 6 and gtk4-layer-shell (with their
# GObject introspection data) for the wallpaper window.
missing=()
if command -v pacman >/dev/null; then
    for pkg in python-psutil python-gobject gtk4 webkitgtk-6.0 gtk4-layer-shell; do
        pacman -Q "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
    done
    [ ${#missing[@]} -eq 0 ] || sudo pacman -S --needed "${missing[@]}"
elif command -v dnf >/dev/null; then
    for pkg in python3-psutil python3-gobject gtk4 webkitgtk6.0 gtk4-layer-shell; do
        rpm -q "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
    done
    [ ${#missing[@]} -eq 0 ] || sudo dnf install -y "${missing[@]}"
elif command -v apt-get >/dev/null; then
    for pkg in python3-psutil python3-gi gir1.2-gtk-4.0 gir1.2-webkit-6.0 gir1.2-gtk4layershell-1.0; do
        dpkg -s "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
    done
    [ ${#missing[@]} -eq 0 ] || { sudo apt-get update && sudo apt-get install -y "${missing[@]}"; }
else
    echo "unknown package manager: install psutil, PyGObject, GTK4, WebKitGTK 6 and gtk4-layer-shell yourself"
fi

# 1. the command
mkdir -p "$(dirname "$BIN")"
printf '#!/bin/sh\nexec python3 "%s/termwall_api.py" "$@"\n' "$DIR" > "$BIN"
chmod +x "$BIN"
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) grep -q '# termwall: ~/.local/bin on PATH' "$HOME/.bashrc" 2>/dev/null ||
           echo 'export PATH="$HOME/.local/bin:$PATH"  # termwall: ~/.local/bin on PATH' >> "$HOME/.bashrc"
       echo "added ~/.local/bin to PATH in ~/.bashrc: open a new terminal, or run: source ~/.bashrc";;
esac

# 2. the stats server
mkdir -p "$UNIT_DIR"
cat > "$UNIT" <<UNIT
[Unit]
Description=termwall stats server (termwall_api.py)

[Service]
ExecStart=/usr/bin/python3 $DIR/termwall_api.py
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
UNIT
# A hand-started server holds the port; until it is gone the service would only restart
# against "already running".
if ! systemctl --user is-active -q termwall-api.service && pkill -f "$DIR/termwall_api.py" 2>/dev/null; then
    for _ in $(seq 20); do
        curl -fs http://127.0.0.1:9002/health >/dev/null || break
        sleep 0.5
    done
    pkill -9 -f "$DIR/termwall_api.py" 2>/dev/null || true
fi
systemctl --user daemon-reload
systemctl --user enable --now termwall-api.service

# 3. the wallpaper, with the session
if [ -d "$(dirname "$LUA")" ]; then
    strip_block "$LUA" "$BEGIN" "$END"
    cat >> "$LUA" <<LUA
$BEGIN
hl.on("hyprland.start", function()
    hl.exec_cmd("$LAYER")
end)
$END
LUA
    echo "wallpaper: added to $LUA (starts with the next Hyprland login)"
elif [ -f "$CONF" ]; then
    strip_block "$CONF" "# termwall begin" "# termwall end"
    printf '# termwall begin\nexec-once = %s\n# termwall end\n' "$LAYER" >> "$CONF"
    echo "wallpaper: added to $CONF (starts with the next Hyprland login)"
else
    # KDE, GNOME and most others start ~/.config/autostart entries with the session
    mkdir -p "$(dirname "$DESKTOP")"
    cat > "$DESKTOP" <<ENTRY
[Desktop Entry]
Type=Application
Name=termwall
Comment=termwall wallpaper (linux/termwall-layer.py)
Exec=$LAYER
X-GNOME-Autostart-enabled=true
NoDisplay=true
ENTRY
    echo "wallpaper: added $DESKTOP (starts with the next login)"
fi

for _ in $(seq 20); do   # the first start (psutil import, first sample) can take a few seconds
    curl -fs http://127.0.0.1:9002/health >/dev/null && break
    sleep 0.5
done
if curl -fs http://127.0.0.1:9002/health >/dev/null; then
    echo "server: running ($(curl -s http://127.0.0.1:9002/health))"
else
    echo "server: not answering yet; see: journalctl --user -u termwall-api -n 30"
fi
pgrep -f "linux/termwall-layer.py" >/dev/null || { nohup $LAYER >/dev/null 2>&1 & echo "wallpaper: started now"; }
