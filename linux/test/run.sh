#!/usr/bin/env bash
# termwall's Linux install check on several distros, in Docker. Screenshots land in
# linux/test/out/<distro>.png.
#
#   bash linux/test/run.sh                 # every image below
#   bash linux/test/run.sh fedora:44       # one image
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMAGES=("${@:-fedora:44 ubuntu:26.04}")
mkdir -p "$HERE/out"
export MSYS_NO_PATHCONV=1   # Git Bash on Windows would rewrite /src and /out

for image in ${IMAGES[@]}; do
    name="${image//[:\/]/-}"
    echo "######## $image"
    # SYS_NICE: Fedora's sway binary carries that file capability and won't exec without it
    cache="termwall-pkgcache-$name"   # Docker volumes: the image's dnf or apt package cache
    docker run --rm --cap-add SYS_NICE -e NAME="$name" -v "$ROOT:/src:ro" -v "$HERE/out:/out" \
        -v "$cache-dnf:/var/cache/libdnf5" -v "$cache-apt:/var/cache/apt/archives" "$image" \
        bash /src/linux/test/in-container.sh 2>&1 | tee "$HERE/out/$name.log" | grep -v "^\s*$"
done
