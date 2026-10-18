#!/usr/bin/env python3
"""termwall on Linux (Wayland): the page as a desktop-background layer surface.

Experimental. One borderless WebKitGTK view per monitor, placed with wlr-layer-shell (Hyprland,
Sway, river, niri, KDE; not GNOME), the same way a wallpaper daemon places its image.
termwall_api.py must be running for the stats, exactly as on Windows.

Needs (Arch): python-gobject gtk4 webkitgtk-6.0 gtk4-layer-shell

gtk4-layer-shell has to be loaded before libwayland-client, which Python cannot arrange by
itself, so the script restarts itself once with LD_PRELOAD set. Run it plainly:

    python linux/termwall-layer.py                     # every monitor
    python linux/termwall-layer.py --monitor eDP-1     # one monitor
    python linux/termwall-layer.py --layer bottom      # above another wallpaper (see below)

--layer background (default) is where wallpapers live. If a shell (Caelestia, for example)
draws its own wallpaper on that layer and covers termwall, use --layer bottom: still under
every window, but above the other wallpaper.

Monitors get ?screen=1, 2, ... in the order the compositor lists them, so termwall.toml's
[screen.N] sections apply per monitor as they do with Wallpaper Engine.
"""
import argparse
import os
import sys
from pathlib import Path

PRELOAD = "libgtk4-layer-shell.so"
PAGE = Path(__file__).resolve().parent.parent / "index.html"


def preload_path():
    # Arch ships libgtk4-layer-shell.so in the main package; Fedora and Debian only the
    # versioned .so.N there (the bare .so comes with -devel), so take either.
    for d in ("/usr/lib", "/usr/lib64", "/usr/local/lib", "/usr/lib/x86_64-linux-gnu",
              "/usr/lib/aarch64-linux-gnu"):
        found = sorted(Path(d).glob(PRELOAD + "*")) if Path(d).is_dir() else []
        if found:
            return str(found[0])
    return None


def main():
    ap = argparse.ArgumentParser(description="termwall as a Wayland background layer")
    ap.add_argument("--monitor", help="connector name (hyprctl monitors), default: all")
    ap.add_argument("--layer", choices=("background", "bottom"), default="background")
    ap.add_argument("--page", default=str(PAGE), help="index.html to show")
    args = ap.parse_args()

    if PRELOAD not in os.environ.get("LD_PRELOAD", ""):
        lib = preload_path()
        if not lib:
            sys.exit(f"{PRELOAD} not found: install gtk4-layer-shell")
        env = dict(os.environ, LD_PRELOAD=(lib + " " + os.environ.get("LD_PRELOAD", "")).strip())
        os.execve(sys.executable, [sys.executable, *sys.argv], env)

    import gi
    gi.require_version("Gtk", "4.0")
    gi.require_version("Gtk4LayerShell", "1.0")
    gi.require_version("WebKit", "6.0")
    from gi.repository import Gdk, Gtk, Gtk4LayerShell as Layer, WebKit

    if not os.environ.get("WAYLAND_DISPLAY") or Gdk.Display.get_default() is None:
        sys.exit("no Wayland session here (WAYLAND_DISPLAY unset or not reachable); "
                 "run this inside the desktop session")
    if not Layer.is_supported():
        sys.exit("this compositor has no wlr-layer-shell (GNOME does not); termwall can't sit on the desktop")
    page = Path(args.page).resolve()
    if not page.exists():
        sys.exit(f"no page at {page}")

    app = Gtk.Application(application_id="eu.panto.termwall.layer")
    windows = []

    def show(monitor, screen):
        win = Gtk.Window(application=app)
        Layer.init_for_window(win)
        Layer.set_layer(win, Layer.Layer.BACKGROUND if args.layer == "background" else Layer.Layer.BOTTOM)
        Layer.set_namespace(win, "termwall")
        Layer.set_monitor(win, monitor)
        for edge in (Layer.Edge.TOP, Layer.Edge.BOTTOM, Layer.Edge.LEFT, Layer.Edge.RIGHT):
            Layer.set_anchor(win, edge, True)
        Layer.set_exclusive_zone(win, -1)           # ignore bars' reserved space: full screen
        Layer.set_keyboard_mode(win, Layer.KeyboardMode.NONE)

        view = WebKit.WebView()
        s = view.get_settings()
        # The page is a file:// URL that reads token.js next to it and polls the local API.
        s.set_allow_file_access_from_file_urls(True)
        s.set_allow_universal_access_from_file_urls(True)
        view.load_uri(page.as_uri() + f"?screen={screen}")
        win.set_child(view)
        win.present()
        windows.append(win)
        print(f"termwall on {monitor.get_connector()} (screen {screen}, {args.layer} layer)")

    def activate(_app):
        monitors = Gdk.Display.get_default().get_monitors()
        found = 0
        for i in range(monitors.get_n_items()):
            m = monitors.get_item(i)
            if args.monitor and m.get_connector() != args.monitor:
                continue
            show(m, i + 1)
            found += 1
        if not found:
            names = [monitors.get_item(i).get_connector() for i in range(monitors.get_n_items())]
            print(f"no monitor {args.monitor!r}; there are: {', '.join(names)}")
            app.quit()

    app.connect("activate", activate)
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
