#!/bin/bash
# Offline tests for overlay/usr/bin/thinfloe-terminal, which starts foot with
# Smalti at 14 px, or 28 px when a screen is HiDPI, and for the shipped
# /etc/xdg/foot/foot.ini. Stubs for wlr-randr and foot stand in for the real
# ones.
#
# Usage: test/integration/test-terminal.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
WRAPPER="$REPO/overlay/usr/bin/thinfloe-terminal"
INI="$REPO/overlay/etc/xdg/foot/foot.ini"

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

: "${TMPDIR:=/scratch/oetiker/claude-tmp/thinfloe-tests}"
mkdir -p "$TMPDIR"
work=$(mktemp -d "$TMPDIR/test-terminal.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"

cat >"$work/bin/foot" <<STUB
#!/bin/sh
printf '%s\n' "\$@" >"$work/foot.args"
STUB
chmod 755 "$work/bin/foot"

# $1: the text wlr-randr prints, or "fail" for a wlr-randr that cannot connect.
run() {
    if [[ $1 == fail ]]; then
        printf '#!/bin/sh\necho "failed to connect to display" >&2\nexit 1\n' >"$work/bin/wlr-randr"
    else
        printf '#!/bin/sh\ncat <<"EOF"\n%s\nEOF\n' "$1" >"$work/bin/wlr-randr"
    fi
    chmod 755 "$work/bin/wlr-randr"
    rm -f "$work/foot.args"
    PATH="$work/bin:$PATH" "$WRAPPER" -e true
}

output() {  # name, mode, enabled
    cat <<EOF
$1 "Some Monitor"
  Enabled: $3
  Modes:
    1024x768 px, 60.004000 Hz
    $2 px, 60.000000 Hz (preferred, current)
  Position: 0,0
  Transform: normal
  Scale: 1.000000
EOF
}

expect() {  # description, pixel size
    local pad=$(( $2 * 12 / 14 ))
    if grep -qx -- "--override=main.font=Smalti 7x14:pixelsize=$2" "$work/foot.args" 2>/dev/null &&
       grep -qx -- "--override=main.pad=${pad}x${pad}" "$work/foot.args" &&
       grep -qx -- "-e" "$work/foot.args"; then
        ok "$1: pixelsize $2, pad $pad"
    else
        bad "$1: want pixelsize $2, foot got: $(tr '\n' ' ' <"$work/foot.args" 2>/dev/null)"
    fi
}

run "$(output HDMI-A-1 1920x1080 yes)";                         expect "1080p" 14
run "$(output DP-1 3840x2160 yes)";                             expect "4K" 28
run "$(output DP-1 2160x3840 yes)";                             expect "4K rotated" 28
run "$(output DP-1 2560x1440 yes)";                             expect "1440p" 14
run "$(output HDMI-A-1 1920x1080 yes; output DP-1 3840x2160 yes)"; expect "1080p and 4K" 28
run fail;                                                       expect "no wlr-randr answer" 14

if [[ -x $WRAPPER ]]; then ok "thinfloe-terminal is executable"; else bad "thinfloe-terminal is not executable"; fi

grep -q '^Exec=thinfloe-terminal$' "$REPO/overlay/etc/thinfloe/menu/30-terminal.desktop" \
    && ok "the Terminal tile starts thinfloe-terminal" \
    || bad "the Terminal tile does not start thinfloe-terminal"

for kv in 'font=Smalti 7x14:pixelsize=14' 'dpi-aware=no' 'pad=12x12' \
          'indicator-position=relative' 'initial-window-size-chars=100x30' 'background=0d1218'; do
    grep -qxF "$kv" "$INI" && ok "foot.ini: $kv" || bad "foot.ini lacks $kv"
done

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - terminal"
    exit 1
fi
echo ""
echo "PASSED - terminal"
exit 0
