#!/bin/bash
# Every external command the 90thinfloe dracut hooks run must exist in the BUILT
# initrd.
#
# The initramfs holds only what module-setup.sh installs plus dracut's own
# base set. A hook that calls anything else fails with "command not found" at
# boot, usually behind `2>/dev/null || true`, while every offline test passes
# on the dev host, which has the command. That is how the hostname shipped as
# "thinfloe-", why netboot never recorded the boot MAC, and why every sync in the
# tl.new upgrade rotation failed.
#
# test-initramfs-home.sh runs the helpers with a PATH built from
# module-setup.sh, which proves the logic but trusts module-setup.sh's list.
# This test trusts nothing: it lists the initrd itself.
#
# Usage: test/integration/test-initramfs-commands.sh [initrd.img [ramroot.sqsh]]

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
INITRD="${1:-artifacts/rootfs/initrd.img}"
SQSH="${2:-artifacts/rootfs/ramroot.sqsh}"
MODDIR="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe"
EXTRACT="$REPO/test/integration/initramfs-commands.py"

for f in "$INITRD" "$SQSH"; do
    if [[ ! -f "$f" ]]; then
        echo "FAIL: $f not found - run 'make rootfs' first" >&2
        exit 1
    fi
done
for tool in lsinitramfs unmkinitramfs unsquashfs python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "FAIL: $tool not installed (initramfs-tools-core, squashfs-tools, python3)" >&2
        exit 1
    fi
done

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# --- the extractor finds commands where the hooks put them ---------------
#
# Without this a broken extractor returns nothing and the real check below
# passes vacuously.
cat >"$WORK/fixture.sh" <<'FIXTURE'
# comment_cmd should not count
plain_cmd -a "quoted (paren_word in a string)"
x=$(subst_cmd | piped_cmd)
echo "inside $(quoted_subst_cmd) quotes"
if cond_cmd; then then_cmd; else else_cmd; fi
for i in not_a_cmd also_not; do loop_cmd; done
case "$x" in
    pattern_word) case_body_cmd ;;
    *) other_body_cmd ;;
esac
a && and_cmd || or_cmd
local v=$(local_subst_cmd)
FIXTURE
got=$(python3 "$EXTRACT" "$WORK/fixture.sh" | sort | tr '\n' ' ')
want="a and_cmd case_body_cmd cond_cmd echo else_cmd local local_subst_cmd loop_cmd or_cmd other_body_cmd piped_cmd plain_cmd quoted_subst_cmd subst_cmd then_cmd "
if [[ "$got" == "$want" ]]; then
    ok "command extractor reads the fixture correctly"
else
    bad "command extractor: got '$got', want '$want'"
fi

# --- what the initrd holds -----------------------------------------------

if ! lsinitramfs "$INITRD" >"$WORK/initrd.list" 2>"$WORK/lsinitramfs.err" ||
   [[ ! -s "$WORK/initrd.list" ]]; then
    bad "cannot list $INITRD: $(head -3 "$WORK/lsinitramfs.err")"
    exit 1
fi
sed -nE 's#^(usr/)?s?bin/([^/]+)$#\2#p' "$WORK/initrd.list" | sort -u >"$WORK/bins"
echo "note - $(wc -l <"$WORK/bins") executables in $INITRD"

# The 90thinfloe hooks must be in there at all, or the rest proves nothing.
if grep -qE '(^|/)thinfloe-lib\.sh$' "$WORK/initrd.list"; then
    ok "the thinfloe module is in the initrd"
else
    bad "no thinfloe-lib.sh in $INITRD - the thinfloe dracut module was not included"
fi

# The drop-ins that order the pre-mount and mount hooks after the cmdline
# hook. Without them both raced dracut-cmdline.service; see
# 90thinfloe/thinfloe-after-cmdline.conf.
for unit in dracut-pre-mount dracut-mount; do
    if grep -qE "/$unit\.service\.d/90-thinfloe-after-cmdline\.conf$" "$WORK/initrd.list"; then
        ok "$unit.service is ordered after dracut-cmdline in the initrd"
    else
        bad "no $unit.service.d/90-thinfloe-after-cmdline.conf in $INITRD"
    fi
done
if grep -qx 'After=dracut-cmdline.service' "$MODDIR/thinfloe-after-cmdline.conf"; then
    ok "the drop-in orders After=dracut-cmdline.service"
else
    bad "thinfloe-after-cmdline.conf lacks After=dracut-cmdline.service"
fi

# The initramfs's networkd must ask for the DHCP time zone (option 101), or a
# netbooted machine shows UTC until its first lease renewal. dracut copies
# its default network file to /run/systemd/network/zzzz-dracut-default.network;
# the drop-in extends it. make qemu-smoke-timezone-netboot proves it end to end.
TZ_DROPIN=etc/systemd/network/zzzz-dracut-default.network.d/50-timezone.conf
if grep -qx "$TZ_DROPIN" "$WORK/initrd.list" &&
   grep -qx 'UseTimezone=yes' "$MODDIR/50-timezone.conf"; then
    ok "the initrd's networkd asks for the DHCP time zone"
else
    bad "no UseTimezone=yes drop-in $TZ_DROPIN in $INITRD"
fi

# Netboot networking: DHCP from systemd-networkd, names from systemd-resolved.
for f in usr/lib/systemd/systemd-networkd usr/lib/systemd/systemd-networkd-wait-online \
         usr/lib/systemd/systemd-resolved; do
    if grep -qx "$f" "$WORK/initrd.list"; then
        ok "/$f is in the initrd"
    else
        bad "/$f is missing - netboot has no network (dracut --add systemd-networkd systemd-resolved)"
    fi
done

# The ThinFloe boot screen: the theme, its images, the script plugin, the
# label plugin that draws the host name and the font it draws it in. The
# default.plymouth link must name the theme, and plymouthd.conf must too,
# because plymouthd reads Theme= before it looks at the link.
for f in usr/share/plymouth/themes/thinfloe/thinfloe.plymouth \
         usr/share/plymouth/themes/thinfloe/thinfloe.script \
         usr/share/plymouth/themes/thinfloe/logo-320.png \
         usr/share/plymouth/themes/thinfloe/logo-427.png \
         usr/share/plymouth/themes/thinfloe/logo-480.png \
         usr/share/plymouth/themes/thinfloe/logo-640.png \
         usr/share/plymouth/themes/thinfloe/logo-853.png \
         usr/share/plymouth/themes/thinfloe/logo-960.png \
         usr/share/plymouth/themes/thinfloe/logo-1280.png \
         usr/share/plymouth/themes/thinfloe/glow.png \
         usr/share/plymouth/themes/thinfloe/bar-track.png \
         usr/share/plymouth/themes/thinfloe/bar-fill.png \
         usr/lib/x86_64-linux-gnu/plymouth/script.so \
         usr/lib/x86_64-linux-gnu/plymouth/label-pango.so \
         usr/share/fonts/truetype/thinfloe/Geo-Oblique.ttf; do
    if grep -qx "$f" "$WORK/initrd.list"; then
        ok "/$f is in the initrd"
    else
        bad "/$f is missing from the initrd - the boot screen would fall back to text"
    fi
done
# themes/default.plymouth -> /etc/alternatives/default.plymouth -> the theme
if lsinitramfs -l "$INITRD" 2>/dev/null |
        grep -E ' etc/alternatives/default\.plymouth -> .*/themes/thinfloe/thinfloe\.plymouth$' >/dev/null &&
   lsinitramfs -l "$INITRD" 2>/dev/null |
        grep -E ' usr/share/plymouth/themes/default\.plymouth -> /etc/alternatives/default\.plymouth$' >/dev/null; then
    ok "default.plymouth in the initrd points at the thinfloe theme"
else
    bad "default.plymouth in the initrd does not point at the thinfloe theme"
fi
mkdir -p "$WORK/ex"
unmkinitramfs "$INITRD" "$WORK/ex" >/dev/null 2>&1
pconf=$(find "$WORK/ex" -path '*etc/plymouth/plymouthd.conf' 2>/dev/null | head -1)
if [[ -n "$pconf" ]] && grep -qx 'Theme=thinfloe' "$pconf"; then
    ok "plymouthd.conf in the initrd says Theme=thinfloe"
else
    bad "plymouthd.conf in the initrd does not say Theme=thinfloe (${pconf:-not found})"
fi
# Plymouth's own progress assumes a 60 s boot when no boot-duration file was
# kept, which a RAM system never has: the bar ended at 7 to 15 %. The theme
# draws the bar from the elapsed time instead.
pscript=$(find "$WORK/ex" -path '*themes/thinfloe/thinfloe.script' 2>/dev/null | head -1)
if [[ -n "$pscript" ]] && grep -q '^ *set_progress(ease(duration));' "$pscript" &&
   ! grep -q '^ *set_progress(progress);' "$pscript"; then
    ok "the boot screen's bar follows the elapsed time, not Plymouth's estimate"
else
    bad "the boot screen's bar follows Plymouth's estimate, which stops near 15 % on a RAM system"
fi
# Image.Scale interpolates: the 640 px logo drawn 1280 px wide on a 4K screen
# had edges blurred over 3 px. The logo comes in fixed widths and is drawn
# unscaled; DeviceScale=1 keeps Plymouth from doubling the whole screen.
if [[ -n "$pscript" ]] && ! grep -q 'logo\.image\.Scale' "$pscript" &&
   grep -q 'Image("logo-" + ' "$pscript"; then
    ok "the boot screen draws the logo at a rendered width, unscaled"
else
    bad "the boot screen scales the logo, which blurs it"
fi
if [[ -n "$pconf" ]] && grep -qx 'DeviceScale=1' "$pconf"; then
    ok "plymouthd.conf in the initrd says DeviceScale=1"
else
    bad "plymouthd.conf in the initrd does not say DeviceScale=1: a 4K screen is drawn at half size and enlarged"
fi

# --- names that are not files: builtins and shell functions ---------------
#
# dracut-lib.sh is taken from the image rather than the dev host: it is the
# version the hooks source at boot. The base module's number changes between
# dracut releases (99base, 80base), so it is looked up.
DRACUT_LIB=$(unsquashfs -l "$SQSH" 2>/dev/null |
    sed -nE 's#^squashfs-root/(usr/lib/dracut/modules\.d/[0-9]+base/dracut-lib\.sh)$#\1#p' | head -1)
if [[ -z "$DRACUT_LIB" ]] ||
   ! unsquashfs -cat "$SQSH" "$DRACUT_LIB" >"$WORK/dracut-lib.sh" 2>/dev/null ||
   [[ ! -s "$WORK/dracut-lib.sh" ]]; then
    bad "cannot read the base module's dracut-lib.sh from $SQSH"
    exit 1
fi
{
    compgen -b
    compgen -k
    # name() anywhere on a line: thinfloe-lib.sh defines a fallback dinfo with
    # `type dinfo || dinfo() { info "$@"; }`.
    grep -ohE '(^|[^A-Za-z0-9_])[A-Za-z_][A-Za-z0-9_]*[[:space:]]*\(\)' \
        "$WORK/dracut-lib.sh" "$MODDIR"/*.sh |
        sed -E 's/^[^A-Za-z_]*//; s/[[:space:]]*\(\)$//'
} | sort -u >"$WORK/known"

# --- the check -----------------------------------------------------------
#
# module-setup.sh runs on the build host, not in the initramfs, so it is not
# scanned.
hooks=()
for f in "$MODDIR"/*.sh; do
    [[ "${f##*/}" == module-setup.sh ]] || hooks+=("$f")
done
python3 "$EXTRACT" "${hooks[@]}" | sort -u >"$WORK/used"
used_n=$(wc -l <"$WORK/used")

# A sanity floor: these are certainly called by the hooks.
for must in curl tar mount; do
    grep -qx "$must" "$WORK/used" || bad "extractor missed '$must' in the hooks"
done

missing=$(comm -23 "$WORK/used" "$WORK/known" | comm -23 - "$WORK/bins")
if [[ -z "$missing" ]]; then
    ok "all $used_n commands the 90thinfloe hooks use exist in the initrd"
else
    for cmd in $missing; do
        where=$(grep -lwE "(^|[;&|({!]|\\\$\\(|then|do|else)[[:space:]]*$cmd\\b" "${hooks[@]}" \
            | sed 's#.*/##' | paste -sd' ')
        bad "'$cmd' is used by ${where:-a 90thinfloe hook} but is not in the initrd"
    done
fi

# --- no WiFi in the initrd ------------------------------------------------
#
# Nothing in the initrd uses WiFi: it has no wpa_supplicant, and netboot runs
# over a cable. `instmods iwlwifi` once pulled in 82 MB of Intel WiFi firmware,
# a sixth of the download. The running system loads the drivers and firmware
# from the squashfs after switch-root.
if grep -qE '(^|/)iwlwifi\.ko' "$WORK/initrd.list"; then
    bad "the initrd carries iwlwifi.ko; WiFi drivers belong in the squashfs only"
else
    ok "no iwlwifi driver in the initrd"
fi
if grep -q 'usr/lib/firmware/intel/iwlwifi/' "$WORK/initrd.list"; then
    bad "the initrd carries Intel WiFi firmware (82 MB that nothing in the initrd uses)"
else
    ok "no Intel WiFi firmware in the initrd"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - initramfs commands"
    exit 1
fi

echo ""
echo "PASSED - initramfs commands"
exit 0
