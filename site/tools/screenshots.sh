#!/bin/bash
# Render the page and the ad with headless Chrome.
#
#   screenshots.sh OUTDIR        review pictures of index.html (desktop, phone)
#   screenshots.sh --ads         the three ad pictures, written to ../ad/
#
# Chrome's profile goes to $TMPDIR (set it; never /tmp on a shared machine).
set -euo pipefail
site=$(cd "$(dirname "$0")/.." && pwd)
profile=${TMPDIR:?set TMPDIR}/chrome-profile
shoot() { # URL WIDTH HEIGHT FILE
    google-chrome --headless=new --no-sandbox --disable-gpu --hide-scrollbars \
        --user-data-dir="$profile" --force-device-scale-factor=2 \
        --window-size="$2,$3" --virtual-time-budget=3000 --screenshot="$4" "$1" >/dev/null 2>&1
    echo "$4"
}
if [[ ${1:-} == --ads ]]; then
    for size in 1200x630 1080x1080 1080x1350; do
        shoot "file://$site/ad.html" "${size%x*}" "${size#*x}" "$site/ad/thinfloe-$size.png"
    done
else
    out=$(realpath "${1:?usage: screenshots.sh OUTDIR | --ads}")
    shoot "file://$site/index.html?still" 1440 900 "$out/desktop.png"
    shoot "file://$site/index.html?still" 390 844 "$out/phone.png"
    shoot "file://$site/index.html?still" 1440 5200 "$out/desktop-full.png"
    shoot "file://$site/index.html?still" 390 7800 "$out/phone-full.png"
fi
