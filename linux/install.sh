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
#   one marked line in ~/.bashrc if ~/.local/bin is not on PATH yet
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$HOME/.local/bin/termwall"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT="$UNIT_DIR/termwall-api.service"
LUA="$HOME/.config/caelestia/hypr-user.lua"
CONF="$HOME/.config/hypr/hyprland.conf"
BEGIN="-- termwall begin"
END="-- termwall end"
LAYER="python3 $DIR/linux/termwall-layer.py"

strip_block() {  # file, begin, end: remove a marked block if it is there
    [ -f "$1" ] && sed -i "\|^$2\$|,\|^$3\$|d" "$1" || true
}

if [ "${1:-}" = "--remove" ]; then
    systemctl --user disable --now termwall-api.service 2>/dev/null || true
    rm -f "$UNIT" "$BIN"
    systemctl --user daemon-reload
    strip_block "$LUA" "$BEGIN" "$END"
    strip_block "$CONF" "# termwall begin" "# termwall end"
    sed -i '/# termwall: ~\/.local\/bin on PATH/d' "$HOME/.bashrc" 2>/dev/null || true
    pkill -f "linux/termwall-layer.py" 2>/dev/null || true
    echo "termwall removed (the folder $DIR stays)"
    exit 0
fi

missing=()
for pkg in python-psutil python-gobject gtk4 webkitgtk-6.0 gtk4-layer-shell; do
    pacman -Q "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
done
if [ ${#missing[@]} -gt 0 ] && command -v pacman >/dev/null; then
    echo "installing: ${missing[*]}"
    sudo pacman -S --needed "${missing[@]}"
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
pkill -f "$DIR/termwall_api.py" 2>/dev/null || true   # a hand-started server holds the port
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
    echo "wallpaper: no Hyprland config found; start it yourself with: $LAYER"
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
