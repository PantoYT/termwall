"""winget manifests for a termwall release (the three-file format winget-pkgs wants).

    python winget/make_manifest.py 1.1.0              # hashes the release asset from GitHub
    python winget/make_manifest.py 1.1.0 SHA256       # or give the hash
    winget validate winget/manifests/p/PantoYT/termwall/1.1.0

The output goes under winget/manifests/, laid out like microsoft/winget-pkgs, ready to be
copied into a fork of that repo for a pull request (or handed to wingetcreate).
"""

from __future__ import annotations

import datetime
import hashlib
import sys
import urllib.request
from pathlib import Path

ID = "PantoYT.termwall"
REPO = "https://github.com/PantoYT/termwall"
MANIFEST_VERSION = "1.12.0"


def asset_url(version: str) -> str:
    return f"{REPO}/releases/download/v{version}/termwall-setup-{version}.exe"


def sha256_of(url: str) -> str:
    h = hashlib.sha256()
    with urllib.request.urlopen(url, timeout=120) as r:
        while chunk := r.read(1 << 20):
            h.update(chunk)
    return h.hexdigest().upper()


def manifests(version: str, sha: str, date: str) -> dict[str, str]:
    head = f"PackageIdentifier: {ID}\nPackageVersion: {version}\n"
    tail = f"ManifestVersion: {MANIFEST_VERSION}\n"
    return {
        f"{ID}.yaml": head + "DefaultLocale: en-US\nManifestType: version\n" + tail,
        f"{ID}.installer.yaml": head + f"""InstallerType: inno
Scope: user
UpgradeBehavior: install
ReleaseDate: {date}
Installers:
- Architecture: x64
  InstallerUrl: {asset_url(version)}
  InstallerSha256: {sha}
ManifestType: installer
""" + tail,
        f"{ID}.locale.en-US.yaml": head + f"""PackageLocale: en-US
Publisher: PantoYT
PublisherUrl: https://github.com/PantoYT
PublisherSupportUrl: {REPO}/issues
PackageName: termwall
PackageUrl: {REPO}
License: MIT
LicenseUrl: {REPO}/blob/main/LICENSE
ShortDescription: A live, fastfetch-style system-stats wallpaper for Wallpaper Engine and Lively.
Description: |-
  termwall is a wallpaper that looks like a terminal running fastfetch, live: system info,
  per-core CPU bars, CPU and GPU history, memory, disks, network and top processes, refreshed
  every second. Three layouts (fetch, board, minimal) and a palette that can follow your
  wallpaper. It runs inside Wallpaper Engine or Lively Wallpaper; the installer brings its own
  Python and starts the local stats server at logon.
Moniker: termwall
Tags:
- wallpaper
- fastfetch
- system-monitor
- wallpaper-engine
- lively
- rice
ReleaseNotesUrl: {REPO}/releases/tag/v{version}
ManifestType: defaultLocale
""" + tail,
    }


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    version = argv[0].lstrip("v")
    sha = argv[1].upper() if len(argv) > 1 else sha256_of(asset_url(version))
    out = Path(__file__).parent / "manifests" / "p" / "PantoYT" / "termwall" / version
    out.mkdir(parents=True, exist_ok=True)
    kinds = {f"{ID}.yaml": "version", f"{ID}.installer.yaml": "installer",
             f"{ID}.locale.en-US.yaml": "defaultLocale"}
    for name, text in manifests(version, sha, datetime.date.today().isoformat()).items():
        schema = f"# yaml-language-server: $schema=https://aka.ms/winget-manifest.{kinds[name]}.{MANIFEST_VERSION}.schema.json\n\n"
        (out / name).write_text(schema + text, encoding="utf-8", newline="\n")
    print(f"wrote {out} (sha256 {sha})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
