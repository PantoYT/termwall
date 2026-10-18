#!/usr/bin/env bash
# Runs inside a distro container (see run.sh): installs termwall's packages the way
# install.sh does, checks that Python sees GTK4, WebKitGTK 6 and gtk4-layer-shell, then puts
# the wallpaper on a headless Sway (a real wlr-layer-shell compositor) and saves a screenshot
# to /out/$NAME.png. Sway stands in for Hyprland and KDE: all three place it the same way.
set -euo pipefail
NAME="${NAME:-distro}"
cp -r /src /tmp/tw
cd /tmp/tw
# A fresh clone has none of these; the working copy on the host can (livery's theme.json, the
# host's termwall.toml), and they would color and configure the test like that machine.
rm -f theme.json termwall.toml termwall.json.old token.js share.json share-qr.png logo.txt logo.*

# Keep downloaded packages in the cache volumes run.sh mounts: a rerun installs everything
# again (that is what is tested) without downloading it again.
if command -v dnf >/dev/null; then
    echo "keepcache=True" >> /etc/dnf/dnf.conf
elif command -v apt-get >/dev/null; then
    rm -f /etc/apt/apt.conf.d/docker-clean
fi

echo "== packages"
bash linux/install.sh --deps
if command -v dnf >/dev/null; then
    dnf install -y -q sway grim dbus-daemon curl procps-ng >/dev/null
elif command -v apt-get >/dev/null; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q sway grim dbus curl procps >/dev/null
fi

echo "== bindings"
python3 - <<'PY'
import gi
for ns, v in (("Gtk", "4.0"), ("WebKit", "6.0"), ("Gtk4LayerShell", "1.0")):
    gi.require_version(ns, v)
from gi.repository import Gtk, WebKit, Gtk4LayerShell
import psutil
print("gi ok: Gtk", Gtk.get_major_version(), Gtk.get_minor_version(),
      "| WebKit", WebKit.get_major_version(), WebKit.get_minor_version(),
      "| psutil", psutil.__version__)
PY

echo "== wallpaper on headless Sway"
export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
export LIBSEAT_BACKEND=noop     # no seatd or logind seat in a container
printf 'output HEADLESS-1 resolution 1600x900\n' > /tmp/sway.conf
# Containers have no user namespaces for WebKit's bubblewrap sandbox; test only.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 LIBGL_ALWAYS_SOFTWARE=1
dbus-run-session -- bash -c '
    sway -c /tmp/sway.conf -d >/tmp/sway.log 2>&1 &
    for _ in $(seq 40); do ls "$XDG_RUNTIME_DIR" | grep -q "^wayland-[0-9]$" && break; sleep 0.25; done
    export WAYLAND_DISPLAY=$(ls "$XDG_RUNTIME_DIR" | grep -m1 "^wayland-[0-9]$")
    python3 termwall_api.py >/tmp/api.log 2>&1 &
    for _ in $(seq 40); do curl -fs http://127.0.0.1:9002/health && break; sleep 0.25; done
    echo
    python3 linux/termwall-layer.py >/tmp/layer.log 2>&1 &
    sleep 15
    grim /out/'"$NAME"'.png && echo "screenshot: /out/'"$NAME"'.png"
    echo "-- layer log"; tail -5 /tmp/layer.log
    cp /tmp/sway.log /tmp/layer.log /tmp/api.log /out/ 2>/dev/null; for f in sway layer api; do mv /out/$f.log /out/'"$NAME"'-$f.log 2>/dev/null; done
'
