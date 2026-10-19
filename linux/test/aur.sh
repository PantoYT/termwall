#!/usr/bin/env bash
# The AUR package, checked in a clean Arch container: makepkg (with the selftest in check()),
# namcap, pacman -U, then the installed termwall on a headless Sway as a normal user, to a
# screenshot in linux/test/out/arch-aur.png.
#
# The release tarball the PKGBUILD names is replaced by one made from this working copy (git
# archive of the tracked files, uncommitted changes included), so the package can be checked
# before the tag exists.
#
#   bash linux/test/aur.sh
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
export MSYS_NO_PATHCONV=1   # Git Bash on Windows would rewrite /src and /out
mkdir -p "$HERE/out"

ver=$(sed -n 's/^pkgver=//p' "$ROOT/packaging/aur/PKGBUILD")
tree=$(git -C "$ROOT" stash create)
git -C "$ROOT" archive --prefix="termwall-$ver/" -o "$HERE/out/termwall-$ver.tar.gz" "${tree:-HEAD}"
echo "tarball: termwall-$ver.tar.gz from ${tree:-HEAD}"

docker run --rm --cap-add SYS_NICE -e VER="$ver" -v "$ROOT:/src:ro" -v "$HERE/out:/out" \
    -v termwall-pkgcache-arch:/var/cache/pacman/pkg archlinux:latest \
    bash /src/linux/test/in-arch.sh 2>&1 | tee "$HERE/out/arch-aur.log" | grep -v "^\s*$"
