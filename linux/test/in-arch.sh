#!/usr/bin/env bash
# Runs inside archlinux:latest (see aur.sh).
set -euo pipefail

echo "== packages"
pacman -Syu --noconfirm --needed -q base-devel namcap sway grim curl \
    python python-psutil python-gobject gtk4 webkitgtk-6.0 gtk4-layer-shell >/dev/null

echo "== makepkg"
useradd -m builder
install -d -o builder /home/builder/pkg
cp /src/packaging/aur/PKGBUILD /src/packaging/aur/termwall.install "/out/termwall-$VER.tar.gz" /home/builder/pkg/
chown -R builder /home/builder/pkg
su builder -c "cd ~/pkg && makepkg --noconfirm --skipchecksums 2>&1 | grep -E 'selftest|==>|ERROR' "
pkg=$(ls /home/builder/pkg/termwall-*-any.pkg.tar.zst)

echo "== namcap"
namcap /home/builder/pkg/PKGBUILD "$pkg" || true

echo "== install"
pacman -U --noconfirm "$pkg" >/dev/null
pacman -Ql termwall | grep -v '/$'

echo "== as a user, on headless Sway"
cat > /tmp/wall.sh <<'WALL'
set -u
export XDG_RUNTIME_DIR=/tmp/xdg-builder
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
export LIBSEAT_BACKEND=noop
# Containers have no user namespaces for WebKit's bubblewrap sandbox; test only.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 LIBGL_ALWAYS_SOFTWARE=1
printf 'output HEADLESS-1 resolution 1600x900\n' > /tmp/sway.conf
dbus-run-session -- bash -c '
    sway -c /tmp/sway.conf -d >/tmp/sway.log 2>&1 &
    for _ in $(seq 40); do ls "$XDG_RUNTIME_DIR" | grep -q "^wayland-[0-9]$" && break; sleep 0.25; done
    export WAYLAND_DISPLAY=$(ls "$XDG_RUNTIME_DIR" | grep -m1 "^wayland-[0-9]$")
    termwall >/tmp/api.log 2>&1 &
    termwall-layer >/tmp/layer.log 2>&1 &
    for _ in $(seq 40); do curl -fs http://127.0.0.1:9002/health && break; sleep 0.25; done
    echo
    sleep 15
    grim /out/arch-aur.png && echo "screenshot: /out/arch-aur.png"
    echo "-- layer log"; tail -5 /tmp/layer.log
    echo "-- files the user got"
    ls -la ~/.config/termwall ~/.local/share/termwall
    termwall --version
'
WALL
su builder -c "bash /tmp/wall.sh"
echo "== /usr/share/termwall untouched"
find /usr/share/termwall -type f -newer "$pkg" -print | grep . && echo "WRITTEN TO" || echo "nothing written there"
