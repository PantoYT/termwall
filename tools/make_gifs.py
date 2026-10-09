"""Build the README GIFs from termwall's demo mode (made-up stats, nothing of yours).

    python tools/make_gifs.py --chrome PATH/TO/chrome-headless-shell [--only themes|settings]

Needs a headless Chrome (npx @puppeteer/browsers install chrome-headless-shell@stable) and
Pillow. Every frame is a screenshot of index.html?demo&..., so a GIF always shows the page as
it is now.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from termwall_api import THEMES, THEME_KEYS  # noqa: E402

SIZE = (960, 540)


def palette(name: str) -> str:
    return "&".join(f"{k}={THEMES[name][k][1:]}" for k in THEME_KEYS)


def shoot(chrome: str, query: str, out: Path, size=(1920, 1080)) -> None:
    url = (ROOT / "index.html").as_uri() + "?demo&" + query
    subprocess.run([chrome, "--headless", "--disable-gpu", "--hide-scrollbars", f"--window-size={size[0]},{size[1]}",
                    "--virtual-time-budget=4000", f"--screenshot={out}", url],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def gif(frames: list[Path], out: Path, ms: int) -> None:
    from PIL import Image
    imgs = [Image.open(f).convert("RGB").resize(SIZE, Image.LANCZOS) for f in frames]
    # one shared palette keeps colors from shifting between frames
    sheet = Image.new("RGB", (SIZE[0], SIZE[1] * len(imgs)))
    for i, im in enumerate(imgs):
        sheet.paste(im, (0, SIZE[1] * i))
    pal = sheet.quantize(colors=255, method=Image.Quantize.MEDIANCUT)
    q = [im.quantize(palette=pal, dither=Image.Dither.NONE) for im in imgs]
    q[0].save(out, save_all=True, append_images=q[1:], duration=ms, loop=0, optimize=True)
    print(f"{out.relative_to(ROOT)}: {len(q)} frames, {out.stat().st_size // 1024} KiB")


def themes(chrome: str, tmp: Path) -> None:
    frames = []
    for name in THEMES:
        f = tmp / f"theme-{name}.png"
        shoot(chrome, f"{palette(name)}&prompt=" + quote(f"termwall theme {name}"), f)
        frames.append(f)
    gif(frames, ROOT / "docs" / "themes.gif", 900)


SETTINGS = [  # (what the prompt line says, the URL settings)
    ("layout fetch", "layout=fetch"),
    ("layout board", "layout=board"),
    ("layout htop", "layout=htop"),
    ("layout minimal", "layout=minimal"),
    ("bars shade", "layout=fetch&bars=shade&" + palette("catppuccin")),
    ("bars dots", "layout=fetch&bars=dots&" + palette("catppuccin")),
    ("font iosevka", "layout=fetch&font=iosevka&" + palette("gruvbox")),
    ("font jetbrains", "layout=fetch&font=jetbrains&" + palette("gruvbox")),
    ("effects crt,glow", "layout=fetch&effects=crt,glow&" + palette("amber")),
    ("effects crt,glow,curve,noise", "layout=board&effects=crt,glow,curve,noise&" + palette("phosphor")),
    ("effects chroma", "layout=fetch&effects=chroma&" + palette("tokyo-night")),
    ("hide logo,info  clock 12h", "layout=fetch&hide=logo,info&clock=12h&" + palette("nord")),
    ("private on", "layout=fetch&private=on&" + palette("rose-pine")),
]


def settings(chrome: str, tmp: Path) -> None:
    frames = []
    for i, (label, q) in enumerate(SETTINGS):
        f = tmp / f"setting-{i:02}.png"
        shoot(chrome, f"{q}&prompt=" + quote(f"termwall style {label}"), f)
        frames.append(f)
    gif(frames, ROOT / "docs" / "settings.gif", 1400)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--chrome", default=os.environ.get("TERMWALL_CHROME"), help="headless Chrome (or $TERMWALL_CHROME)")
    ap.add_argument("--only", choices=("themes", "settings"))
    a = ap.parse_args()
    if not a.chrome:
        sys.exit("give --chrome PATH (a headless Chrome), or set TERMWALL_CHROME")
    with tempfile.TemporaryDirectory() as td:
        if a.only in (None, "themes"):
            themes(a.chrome, Path(td))
        if a.only in (None, "settings"):
            settings(a.chrome, Path(td))
