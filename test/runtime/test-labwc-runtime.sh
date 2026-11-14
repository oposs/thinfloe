#!/bin/bash
# Runs labwc headless with two 1280x720 outputs and asserts that a fullscreen
# X11 client is given the union of both (2560x720), not a single output.
# Also asserts that Xwayland binds the keyboard-grab manager, and that
# labwc's environment-file parser lets a later duplicate assignment win
# (the assumption thinfloe-compose-labwc-config's merge depends on).
# Finally asserts that thinfloe-menu, a native Wayland client, maps at the full
# output size, rasterises on the CPU and keeps running, and that a fullscreen
# window keeps the keyboard focus when the menu replaces its window behind it.
#
# Not part of 'make test': it needs network (python3-xlib) and runs a
# compositor. Use 'make test-runtime'.

set -uo pipefail

IMAGE="${1:-localhost/thinfloe-runtime-test:latest}"
fail=0

# labwc -S only bounds the case where the probe runs and then exits or
# crashes. If labwc or Xwayland hangs during startup - before the session
# command ever executes - 'podman run' blocks forever and so does this
# script. Cap it so that failure mode is loud instead of silent. The whole
# rig normally completes in seconds; this bound is generous on purpose.
RUNTIME_TIMEOUT=180
CONTAINER_NAME="thinfloe-runtime-test-$$"

out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=2 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           labwc -C /etc/thinfloe/labwc -S "python3 /usr/local/bin/x11-probe.py"' 2>&1)
rc=$?

# timeout(1) exits 124 when it kills the command with the default TERM. But
# 'podman run' does not always die from TERM alone (observed: it can take
# longer than a 10s --kill-after grace period to notice), in which case
# timeout escalates to KILL and the exit status is 128+9=137 instead. Treat
# both as "timed out". Report that distinctly from a wrong-but-present
# geometry - a hang and a bad geometry mean very different things to whoever
# reads the failure - and clean up the container explicitly, since a killed
# 'podman run' may not get to run its own --rm cleanup.
if [[ $rc -eq 124 || $rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc/Xwayland hang during startup)"
    echo "--- compositor output so far ---"
    echo "$out"
    podman rm -f "$CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

geom=$(echo "$out" | grep -o 'GEOMETRY [0-9]*x[0-9]*' | tail -1 | cut -d' ' -f2)

if [[ "$geom" == "2560x720" ]]; then
    echo "ok   - fullscreen X11 window spans both outputs ($geom)"
else
    echo "FAIL - fullscreen X11 window is $geom, expected 2560x720"
    echo "--- compositor output ---"
    echo "$out"
    fail=1
fi

# Rootless Xwayland forwards XGrabKeyboard via
# zwp_xwayland_keyboard_grab_manager_v1. This only asserts that Xwayland
# *binds* that global (wl_registry.bind(..., "zwp_xwayland_keyboard_grab_
# manager_v1", ...)), not that a grab_keyboard request is ever sent. That
# weaker check is deliberate, not an oversight:
#
# Xwayland only installs its grab-forwarding hook
# (setup_keyboard_grab_handler(), which overrides ActivateGrab/
# DeactivateGrab on the master keyboard device - the path that eventually
# calls zwp_xwayland_keyboard_grab_manager_v1_grab_keyboard()) once
# xwl_seat->keyboard already exists (xwayland/xwayland-input.c,
# init_keyboard_grab(), ~L3091-3106). xwl_seat->keyboard is only created in
# the seat capabilities handler (~L1847-1859) in response to the compositor
# advertising WL_SEAT_CAPABILITY_KEYBOARD. This headless rig
# (WLR_BACKENDS=headless) provides zero input devices, so wl_seat stays at
# capabilities(0) for the entire run - confirmed in the WAYLAND_DEBUG output
# both before and after this patch. wlroots' headless backend has no public
# API to synthesize one either: wlr/backend/headless.h only exposes
# wlr_headless_add_output(), no wlr_headless_add_input_device() equivalent.
#
# So Xwayland never believes it has a keyboard to grab, and grab_keyboard
# can never fire here, regardless of whether our implementation is correct.
# Do not "fix" this back to grepping for grab_keyboard - it would just make
# the assertion unfalsifiable in this rig. Real XGrabKeyboard forwarding is
# unverified until tested on real hardware with a real keyboard attached.
GRAB_CONTAINER_NAME="thinfloe-runtime-test-grab-$$"

grab_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$GRAB_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=2 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    -e WAYLAND_DEBUG=1 \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           labwc -C /etc/thinfloe/labwc -S "python3 /usr/local/bin/x11-probe.py --grab"' 2>&1)
grab_rc=$?

if [[ $grab_rc -eq 124 || $grab_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc/Xwayland hang during startup)"
    echo "--- compositor output so far ---"
    echo "$grab_out"
    podman rm -f "$GRAB_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

# The interface/id separator in WAYLAND_DEBUG output changed between
# libwayland versions: 1.22 and earlier print "wl_registry@N", 1.24 (shipped
# in this image) prints "wl_registry#N". Accept either rather than pinning
# to whichever the base image currently ships.
if echo "$grab_out" | grep -Eq 'wl_registry[@#][0-9]+\.bind\([0-9]+, "zwp_xwayland_keyboard_grab_manager_v1"'; then
    echo "ok   - Xwayland bound the xwayland keyboard grab manager"
else
    echo "FAIL - labwc is not advertising the manager to Xwayland"
    echo "--- compositor output ---"
    echo "$grab_out"
    fail=1
fi


# The whole ~/.thinfloe-environment override fix (thinfloe-compose-labwc-config)
# rests on one assumption about labwc's own parser: that a LATER duplicate
# assignment in the environment file wins. If labwc kept the FIRST value
# instead, the merge would silently do nothing - the same class of bug as
# the "export" prefix issue documented in
# overlay/etc/thinfloe/labwc/environment. This is tested here, against the
# real labwc binary, rather than assumed.
#
# Build a config dir containing the shipped rc.xml (labwc needs one to
# start, but its contents are not what's under test here) and an
# environment file with LABWC_FULLSCREEN_SPAN_OUTPUTS=1 followed by
# LABWC_FULLSCREEN_SPAN_OUTPUTS=0. If the later '0' wins, a fullscreen
# probe window on this two-output rig gets a single output (1280x720)
# instead of the spanned union (2560x720) that the first assertion above
# proved '1' produces.
DUP_CONTAINER_NAME="thinfloe-runtime-test-dup-$$"

dup_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$DUP_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=2 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           mkdir -p /tmp/dup-config &&
           cp /etc/thinfloe/labwc/rc.xml /tmp/dup-config/rc.xml &&
           printf "LABWC_FULLSCREEN_SPAN_OUTPUTS=1\nLABWC_FULLSCREEN_SPAN_OUTPUTS=0\n" >/tmp/dup-config/environment &&
           labwc -C /tmp/dup-config -S "python3 /usr/local/bin/x11-probe.py"' 2>&1)
dup_rc=$?

if [[ $dup_rc -eq 124 || $dup_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc/Xwayland hang during startup)"
    echo "--- compositor output so far ---"
    echo "$dup_out"
    podman rm -f "$DUP_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

dup_geom=$(echo "$dup_out" | grep -o 'GEOMETRY [0-9]*x[0-9]*' | tail -1 | cut -d' ' -f2)

if [[ "$dup_geom" == "1280x720" ]]; then
    echo "ok   - labwc's environment parser lets a later duplicate assignment win ($dup_geom)"
else
    echo "FAIL - a later duplicate assignment did not win (got $dup_geom, expected 1280x720)"
    echo "--- compositor output ---"
    echo "$dup_out"
    fail=1
fi

# A keyboard apply (the Keyboard tab): the menu rewrites generated/environment,
# runs 'thinfloe-compose-labwc-config --env-only' and 'labwc --reconfigure'.
# The session starts with layout us, the script applies the given layout and
# variant with the options of the tab, then prints the layout line of the
# merged file. This rig has no keyboard device (see the Xwayland note above),
# so labwc may build no keymap at all and "no keymap error" would pass whatever
# the menu writes. A control run applies a keymap XKB cannot build
# (fr/de_nodeadkeys) through the same sequence: only when labwc prints
# 'failed to create xkb keymap' after the apply can the rig see keymap errors
# and the keymap assertion counts; otherwise it is reported as a skip. (A bad
# pair in the STARTUP environment would not do: labwc then dies before the
# session script runs.) The control turns the composer's keyboard check off
# (TF_KB_CHECK, the 4th argument), which would repair the pair.
kb_run() {
    local layout=$1 variant=$2 name=$3 check=${4:-} out rc
    out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        ${check:+-e TF_KB_CHECK="$check"} \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=1 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e KB_LAYOUT="$layout" \
        -e KB_VARIANT="$variant" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               mkdir -p /tmp/kb/gen /tmp/kb/out && : >/tmp/kb/user &&
               echo XKB_DEFAULT_LAYOUT=us >/tmp/kb/gen/environment &&
               thinfloe-compose-labwc-config /etc/thinfloe/labwc /tmp/kb/gen/environment /tmp/kb/user /tmp/kb/out &&
               cat >/tmp/kb/session.sh <<"EOS"
sleep 2
echo APPLY-START
{ echo "XKB_DEFAULT_LAYOUT=$KB_LAYOUT"
  echo "XKB_DEFAULT_VARIANT=$KB_VARIANT"
  echo XKB_DEFAULT_OPTIONS=compose:ralt,ctrl:nocaps; } >/tmp/kb/gen/environment
thinfloe-compose-labwc-config --env-only /etc/thinfloe/labwc /tmp/kb/gen/environment /tmp/kb/user /tmp/kb/out
labwc --reconfigure
sleep 1
echo "MERGED $(grep "^XKB_DEFAULT_LAYOUT=" /tmp/kb/out/environment | tail -1)"
EOS
               labwc -C /tmp/kb/out -S "sh /tmp/kb/session.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during keyboard apply test)" >&2
        echo "$out" >&2
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
    printf '%s\n' "$out"
    return "$rc"
}

# Output after the apply only: the lines before it belong to the start.
after_apply() { sed -n '/^APPLY-START$/,$p'; }

kb_control=$(kb_run fr de_nodeadkeys "thinfloe-runtime-test-kbc-$$" /nonexistent)
kb_control_rc=$?
kb_out=$(kb_run ch de_nodeadkeys "thinfloe-runtime-test-kb-$$")
kb_rc=$?
for rc_name in kb_control_rc kb_rc; do
    if [[ ${!rc_name} -ne 0 ]]; then
        echo "info - ${rc_name#kb_} of the keyboard apply run: ${!rc_name} (labwc ended with a non-zero status)"
    fi
done

if echo "$kb_out" | grep -qx 'MERGED XKB_DEFAULT_LAYOUT=ch'; then
    echo "ok   - a keyboard apply reaches labwc's environment"
else
    echo "FAIL - the merged environment lacks XKB_DEFAULT_LAYOUT=ch after the apply"
    echo "$kb_out"
    fail=1
fi
if echo "$kb_control" | after_apply | grep -q 'failed to create xkb keymap'; then
    if echo "$kb_out" | after_apply | grep -q 'failed to create xkb keymap'; then
        echo "FAIL - labwc could not build the new keymap"
        echo "$kb_out"
        fail=1
    else
        echo "ok   - labwc built the new keymap"
    fi
else
    echo "skip - labwc builds no keymap without a keyboard device (headless rig)"
fi

# labwc 0.9.3 segfaults at start on a pair XKB cannot build (fr with
# de_nodeadkeys), before the session runs. The composer asks
# 'thinfloe-menu --check-keyboard' about the values labwc will read and
# appends a repair, so the session starts, with layout fr and no variant.
KB_START_NAME="thinfloe-runtime-test-kbs-$$"
kb_start=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$KB_START_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=1 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           mkdir -p /tmp/kb/gen /tmp/kb/out && : >/tmp/kb/user &&
           printf "XKB_DEFAULT_LAYOUT=fr\nXKB_DEFAULT_VARIANT=de_nodeadkeys\n" >/tmp/kb/gen/environment &&
           thinfloe-compose-labwc-config /etc/thinfloe/labwc /tmp/kb/gen/environment /tmp/kb/user /tmp/kb/out &&
           labwc -C /tmp/kb/out -S "sh -c \"echo SESSION-RAN \$XKB_DEFAULT_LAYOUT/\$XKB_DEFAULT_VARIANT/\""' 2>&1)
kb_start_rc=$?
if [[ $kb_start_rc -eq 124 || $kb_start_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang with a repaired keyboard)"
    podman rm -f "$KB_START_NAME" >/dev/null 2>&1
    fail=1
elif echo "$kb_start" | grep -qx 'SESSION-RAN fr//'; then
    echo "ok   - a keyboard pair XKB cannot build is repaired before labwc starts"
else
    echo "FAIL - labwc with fr/de_nodeadkeys at start: rc=$kb_start_rc, no session with layout fr and no variant"
    echo "$kb_start"
    fail=1
fi

# thinfloe-menu is a native Wayland client, so x11-probe cannot see it; it
# prints its own configure size under TF_MENU_DEBUG. One output, so the
# maximized grid must get all of it. timeout ends the menu after 10s: exit
# 124 means it was still running, i.e. it neither crashed nor exited on its
# own, and an exit 0 here would be a logout nobody asked for.
MENU_CONTAINER_NAME="thinfloe-runtime-test-menu-$$"

menu_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$MENU_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=1 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           labwc -C /etc/thinfloe/labwc -S "sh -c \"TF_MENU_DEBUG=1 timeout 10 thinfloe-menu; echo menu-rc=\$?\""' 2>&1)
menu_rc=$?

if [[ $menu_rc -eq 124 || $menu_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during menu startup)"
    echo "$menu_out"
    podman rm -f "$MENU_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

if echo "$menu_out" | grep -q 'thinfloe-menu: configure 1280x720'; then
    echo "ok   - thinfloe-menu maps at the full output size (1280x720)"
else
    echo "FAIL - thinfloe-menu did not configure at 1280x720"
    echo "$menu_out"
    fail=1
fi
if echo "$menu_out" | grep -q 'thinfloe-menu: raster'; then
    echo "ok   - thinfloe-menu rasterised a frame on the CPU"
else
    echo "FAIL - thinfloe-menu never rasterised a frame"
    echo "$menu_out"
    fail=1
fi
# Every tile in this container is a shipped one (root has no
# ~/.config/thinfloe/menu), so every icon must draw. A tile drawing its letter
# also logs "thinfloe-menu: icon <name> for <file>: <why>". The count is of
# the tiles the grid draws: no add-on is installed here (there is no status
# file), so every shipped add-on tile, one with X-TF-Source-* keys, is hidden.
# The upgrade tile (X-TF-Action=upgrade) is hidden too, as no ESP is mounted in the container.
expected_tiles=$(podman run --rm --log-driver=none "$IMAGE" sh -c \
    'n=0; for f in /etc/thinfloe/menu/*.desktop; do grep -q "^X-TF-Source-" "$f" || grep -q "^X-TF-Action=upgrade\$" "$f" || n=$((n+1)); done; echo $n')
icons_line=$(echo "$menu_out" | grep -o 'thinfloe-menu: [0-9]* of [0-9]* tile icons drawn' | tail -1)
icons_drawn=$(echo "$icons_line" | cut -d' ' -f2)
icons_total=$(echo "$icons_line" | cut -d' ' -f4)
if [[ -n "$icons_total" && "$icons_total" -gt 0 && "$icons_total" == "$expected_tiles" && "$icons_drawn" == "$icons_total" ]]; then
    echo "ok   - thinfloe-menu drew every shipped tile icon ($icons_drawn of $icons_total, add-ons and Upgrade hidden)"
else
    echo "FAIL - expected $expected_tiles drawn tiles, each with its icon: ${icons_line:-no icon count printed}"
    echo "$menu_out" | grep 'thinfloe-menu: icon '
    fail=1
fi
if echo "$menu_out" | grep -q 'menu-rc=124'; then
    echo "ok   - thinfloe-menu kept running until stopped"
else
    echo "FAIL - thinfloe-menu exited on its own: $(echo "$menu_out" | grep menu-rc)"
    fail=1
fi

# Geo must be known to fontconfig in the built image (title bar font).
geo_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none "$IMAGE" fc-list 2>&1)
if echo "$geo_out" | grep -q 'Geo'; then
    echo "ok   - fc-list lists Geo"
else
    echo "FAIL - fc-list does not list Geo"
    fail=1
fi

# A folded window must stay reachable: with zero keybindings the menu is the
# only way back. foot is folded with wlrctl; the menu (TF_MENU_DEBUG) must see
# it, --windows must list it as folded, --unfold must bring it back.
FOLD_CONTAINER_NAME="thinfloe-runtime-test-fold-$$"

# The session script travels in an environment variable and is written to a
# file in the container, so no quoting has to survive labwc's -S.
FOLD_SCRIPT='foot sleep 60 &
sleep 2
TF_MENU_DEBUG=1 timeout 20 thinfloe-menu 2>&1 &
sleep 4
wlrctl toplevel minimize app_id:foot
sleep 2
echo LIST1; thinfloe-menu --windows
thinfloe-menu --unfold foot; echo unfold-rc=$?
sleep 2
echo LIST2; thinfloe-menu --windows
sleep 1
wait'

fold_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$FOLD_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=1 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    -e FOLD_SCRIPT="$FOLD_SCRIPT" \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           printf "%s\n" "$FOLD_SCRIPT" >/tmp/fold.sh &&
           labwc -C /etc/thinfloe/labwc -S "sh /tmp/fold.sh"' 2>&1)
fold_rc=$?

if [[ $fold_rc -eq 124 || $fold_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during folded-window test)"
    echo "$fold_out"
    podman rm -f "$FOLD_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

list1=$(echo "$fold_out" | sed -n '/^LIST1$/,/^unfold-rc=/p')
list2=$(echo "$fold_out" | sed -n '/^LIST2$/,$p')
if echo "$fold_out" | grep -q 'thinfloe-menu: folded: foot '; then
    echo "ok   - thinfloe-menu saw foot fold"
else
    echo "FAIL - thinfloe-menu never printed 'folded: foot'"
    echo "$fold_out"
    fail=1
fi
if echo "$list1" | grep -Eq '^foot	.*	folded$'; then
    echo "ok   - --windows lists the folded foot as folded"
else
    echo "FAIL - --windows did not list foot as folded"
    echo "$fold_out"
    fail=1
fi
if echo "$fold_out" | grep -q 'unfold-rc=0'; then
    echo "ok   - --unfold foot exited 0"
else
    echo "FAIL - --unfold foot did not exit 0"
    echo "$fold_out"
    fail=1
fi
if echo "$list2" | grep -Eq '^foot	.*	open$'; then
    echo "ok   - --windows lists foot as open after --unfold"
else
    echo "FAIL - foot is not open after --unfold"
    echo "$fold_out"
    fail=1
fi
if echo "$fold_out" | grep -q 'thinfloe-menu: unfolded: foot '; then
    echo "ok   - thinfloe-menu saw foot unfold"
else
    echo "FAIL - thinfloe-menu never printed 'unfolded: foot'"
    fail=1
fi

# A fullscreen window that covers the menu whole leaves the menu visible on no
# output, so winit reports no monitor. The menu took that as a move and wrote
# the guess for the largest mode: 2 next to a 4K monitor, and the title bars
# grew while OxuLnk ran. Two outputs, the menu on the 1280x720 one (scale 1),
# the other 3840x2160; a fullscreen foot covers the menu. The scale must stay.
# The same run proves the signal the menu takes "covered" from, in both
# directions: it looks for no monitor at the start of a frame and once a second
# while covered. A layout (the [status] table of a config.toml) that shows
# only the network graph, which is drawn again every second, gives it a frame
# a second to notice the cover in; podman's default network gives the
# container the route the graph needs.
COVER_CONTAINER_NAME="thinfloe-runtime-test-cover-$$"

COVER_SCRIPT='wlr-randr --output HEADLESS-1 --custom-mode 3840x2160 --pos 1280,0
wlr-randr --output HEADLESS-2 --pos 0,0
mkdir -p /tmp/cover-cfg
printf "%s\n" "version = 1" "" "[status]" \
    "lines = [\"-battery 1x1\", \"network 2x2\", \"-network-info 1x2\", \"-temperatures 1x1\"," \
    "    \"-cpu 1x1\", \"-memory 1x1\", \"-system 1x1\", \"-clock 1x1\"]" >/tmp/cover-cfg/config.toml
TF_CONFIG_DIR=/tmp/cover-cfg TF_MENU_DEBUG=1 \
    timeout 12 thinfloe-menu >/dev/null 2>/tmp/cover-menu.log &
sleep 3
echo "scale-before=$(cat "$XDG_RUNTIME_DIR/thinfloe-scale")"
foot --fullscreen sleep 5 &
sleep 3
echo "scale-covered=$(cat "$XDG_RUNTIME_DIR/thinfloe-scale")"
wait
grep -xE "thinfloe-menu: (un)?covered" /tmp/cover-menu.log'

cover_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$COVER_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=2 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    -e COVER_SCRIPT="$COVER_SCRIPT" \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           printf "%s\n" "$COVER_SCRIPT" >/tmp/cover.sh &&
           labwc -C /etc/thinfloe/labwc -S "sh /tmp/cover.sh"' 2>&1)
cover_rc=$?

if [[ $cover_rc -eq 124 || $cover_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during covered-menu test)"
    echo "$cover_out"
    podman rm -f "$COVER_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

if echo "$cover_out" | grep -qx 'scale-before=1' && echo "$cover_out" | grep -qx 'scale-covered=1'; then
    echo "ok   - a fullscreen window over the menu leaves the scale as it was"
else
    echo "FAIL - the scale changed while a fullscreen window covered the menu"
    echo "$cover_out" | grep '^scale-'
    fail=1
fi

cover_seen=$(echo "$cover_out" | grep -xE 'thinfloe-menu: (un)?covered' | tr '\n' ' ')
if [[ "$cover_seen" == "thinfloe-menu: covered thinfloe-menu: uncovered " ]]; then
    echo "ok   - the menu sees a fullscreen window cover it, and its end uncover it"
else
    echo "FAIL - covered/uncovered lines were '$cover_seen'"
    echo "       (the network graph's 1 s frames need a default route in the container)"
    echo "$cover_out"
    fail=1
fi

# Part D gate: the building blocks of the monitor arrangement. kanshi applies
# a profile written by the menu, and re-applies it after an edit. The image's
# kanshi 1.9 has no --version and ships no kanshictl, so a reload is SIGHUP.
# The headless outputs have one mode, 1280x720 without a refresh rate, so the
# fixture names the mode without one and the test checks placement only.
KANSHI_CONTAINER_NAME="thinfloe-runtime-test-kanshi-$$"
FIXTURE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

KANSHI_SCRIPT='echo "kanshi-package=$(dpkg-query -W kanshi)"
command -v kanshictl >/dev/null && echo "kanshictl=present" || echo "kanshictl=absent"
cp /fixture/kanshi-two.conf /tmp/kanshi.conf
kanshi -c /tmp/kanshi.conf >/tmp/kanshi.log 2>&1 &
KP=$!
pos() { wlr-randr --json | python3 -c "
import json,sys
for o in json.load(sys.stdin):
    if o[\"name\"]==\"HEADLESS-2\":
        m=[m for m in o[\"modes\"] if m.get(\"current\")][0]
        print(\"%dx%d@%d,%d\" % (m[\"width\"],m[\"height\"],o[\"position\"][\"x\"],o[\"position\"][\"y\"]))
"; }
wait_pos() { for i in 1 2 3 4 5 6 7 8 9 10; do sleep 0.5; [ "$(pos)" = "$1" ] && return; done; }
wait_pos 1280x720@1280,0
echo "kanshi-first=$(pos)"
sed -i "s/position 1280,0/position 0,720/" /tmp/kanshi.conf
kill -HUP $KP
wait_pos 1280x720@0,720
echo "kanshi-second=$(pos)"
echo "--- kanshi.log ---"; cat /tmp/kanshi.log'

kanshi_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$KANSHI_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=2 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    -e KANSHI_SCRIPT="$KANSHI_SCRIPT" \
    -v "$FIXTURE_DIR/kanshi-two.conf:/fixture/kanshi-two.conf:ro,z" \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           printf "%s\n" "$KANSHI_SCRIPT" >/tmp/kanshi.sh &&
           labwc -C /etc/thinfloe/labwc -S "sh /tmp/kanshi.sh"' 2>&1)
kanshi_rc=$?

if [[ $kanshi_rc -eq 124 || $kanshi_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during kanshi test)"
    echo "$kanshi_out"
    podman rm -f "$KANSHI_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

echo "info - $(echo "$kanshi_out" | grep -E '^kanshi-package=') $(echo "$kanshi_out" | grep -E '^kanshictl=')"
# kanshi-first equals labwc's own placement of HEADLESS-2, so only
# kanshi-second (a position labwc never picks) proves that kanshi acted.
if echo "$kanshi_out" | grep -qx 'kanshi-first=1280x720@1280,0' \
    && echo "$kanshi_out" | grep -qx 'kanshi-second=1280x720@0,720'; then
    echo "ok   - kanshi applies a profile and re-applies it after the file changes (SIGHUP)"
else
    echo "FAIL - kanshi did not apply the profile or did not re-apply it on SIGHUP"
    echo "$kanshi_out"
    fail=1
fi

# MoveToOutput as a window rule: the menu maps on the named output. The system
# rc.xml is copied and edited inside the container before labwc starts (labwc
# reads it once). Layout: HEADLESS-1 1000x600 at 0,0 and HEADLESS-2 1024x768
# at 1000,0. With no rule the menu lands on HEADLESS-2 (the output under the
# cursor), so the rule targets HEADLESS-1 and the case can fail. The menu is
# configured on the cursor output first and moved at map, so the LAST configure
# line is the one that counts. A second menu process started later stands in
# for a window created after a change.
run_move_case() {
    local rule_output="$1" name="thinfloe-runtime-test-move-$$-$1" script out rc
    script='wlr-randr --output HEADLESS-1 --custom-mode 1000x600 --output HEADLESS-2 --custom-mode 1024x768 --pos 1000,0
(TF_MENU_DEBUG=1 timeout 9 thinfloe-menu 2>&1 | sed "s/^/first: /") &
sleep 3
(TF_MENU_DEBUG=1 timeout 6 thinfloe-menu 2>&1 | sed "s/^/second: /") &
sleep 1
wlr-randr --json | python3 -c "
import json,sys
for o in json.load(sys.stdin):
    m=[m for m in o[\"modes\"] if m.get(\"current\")][0]
    print(\"size %s %dx%d\" % (o[\"name\"], m[\"width\"], m[\"height\"]))
"
wait'
    out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=2 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e MOVE_OUTPUT="$rule_output" \
        -e MOVE_SCRIPT="$script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               printf "%s\n" "$MOVE_SCRIPT" >/tmp/move.sh &&
               cp -r /etc/thinfloe/labwc /tmp/lw &&
               if [ "$MOVE_OUTPUT" != none ]; then
                   sed -i "s|<action name=\"ToggleAlwaysOnBottom\" />|<action name=\"MoveToOutput\" output=\"$MOVE_OUTPUT\"/>\n      &|" /tmp/lw/rc.xml &&
                   grep -qF "<action name=\"MoveToOutput\" output=\"$MOVE_OUTPUT\"/>" /tmp/lw/rc.xml &&
                   echo "rule-inserted"
               fi &&
               labwc --debug -C /tmp/lw -S "bash /tmp/move.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during MoveToOutput test)"
        echo "$out" | grep -E 'configure|size |rule-|Invalid output|Handling action' 
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
    # Keep the lines the assertions use; the labwc debug log is long.
    MOVE_RESULT=$(echo "$out" | grep -E '^(first|second): .*configure |^size |^rule-inserted|Invalid output|MoveToOutput|segmentation|core dumped|panicked')
    MOVE_RULE_OK=0
    [[ "$rule_output" == none ]] && MOVE_RULE_OK=1
    echo "$out" | grep -qx 'rule-inserted' && MOVE_RULE_OK=1
    MOVE_FIRST=$(echo "$out" | grep -E '^first: .*configure ' | tail -1 | grep -o '[0-9]*x[0-9]*$')
    MOVE_SECOND=$(echo "$out" | grep -E '^second: .*configure ' | tail -1 | grep -o '[0-9]*x[0-9]*$')
    MOVE_INVALID=0
    echo "$out" | grep -q 'Invalid output' && MOVE_INVALID=1
    MOVE_CRASH=0
    echo "$out" | grep -Eqi 'segmentation|core dumped|panicked' && MOVE_CRASH=1
}

# Control: no rule. The menu must NOT end on HEADLESS-1, or the next case
# proves nothing.
run_move_case none
if [[ "$MOVE_FIRST" == 1024x768 && "$MOVE_SECOND" == 1024x768 ]]; then
    echo "ok   - without a rule the menu lands on the cursor output (1024x768), not on HEADLESS-1"
else
    echo "FAIL - control run: the menu is on $MOVE_FIRST / $MOVE_SECOND, expected 1024x768 (the MoveToOutput case would prove nothing)"
    echo "$MOVE_RESULT"
    fail=1
fi

run_move_case HEADLESS-1
if [[ $MOVE_RULE_OK -eq 1 && "$MOVE_FIRST" == 1000x600 && "$MOVE_SECOND" == 1000x600 ]] \
    && echo "$MOVE_RESULT" | grep -qx 'size HEADLESS-1 1000x600' \
    && echo "$MOVE_RESULT" | grep -qx 'size HEADLESS-2 1024x768'; then
    echo "ok   - MoveToOutput moves the menu to HEADLESS-1 (1000x600), also a second one started later; HEADLESS-2 keeps 1024x768"
else
    echo "FAIL - menu did not end up on HEADLESS-1 through MoveToOutput (rule inserted $MOVE_RULE_OK, first $MOVE_FIRST, second $MOVE_SECOND)"
    echo "$MOVE_RESULT"
    fail=1
fi

run_move_case HEADLESS-9
if [[ $MOVE_RULE_OK -eq 1 && $MOVE_INVALID -eq 1 && $MOVE_CRASH -eq 0 ]] \
    && [[ "$MOVE_FIRST" == 1024x768 && "$MOVE_SECOND" == 1024x768 ]]; then
    echo "ok   - MoveToOutput to a missing output is refused (labwc: Invalid output) and leaves the menu mapped ($MOVE_FIRST, $MOVE_SECOND)"
else
    echo "FAIL - MoveToOutput to a missing output: rule inserted $MOVE_RULE_OK, 'Invalid output' logged $MOVE_INVALID, crash $MOVE_CRASH, menu $MOVE_FIRST / $MOVE_SECOND"
    echo "$MOVE_RESULT"
    fail=1
fi

# The menu moves to its monitor by replacing its window (TF_MENU_DEBUG_MOVE
# stands in for *Show the menu here*). The session's own parts: the composed
# config dir and labwc-scale, which turns the menu's output file into a
# MoveToOutput rule, reconfigures labwc and confirms in <file>.applied. Same
# layout as the MoveToOutput gate above: labwc places a new window on
# HEADLESS-2 (1024x768, under the cursor), so the move targets HEADLESS-1
# (1000x600) and the control run without a move must stay on 1024x768. Every
# menu line is stamped with the seconds since the menu started.
run_menu_move_case() {
    local mode="$1" name="thinfloe-runtime-test-menumove-$$-$1" script out rc
    script='wlr-randr --output HEADLESS-1 --custom-mode 1000x600 --output HEADLESS-2 --custom-mode 1024x768 --pos 1000,0
export TF_MENU_DEBUG=1
if [ "$MM_MODE" = unanswered ]; then
    export TF_MENU_OUTPUT_FILE=/tmp/xdg/test-menu-output
else
    /usr/libexec/thinfloe/labwc-scale >/tmp/scale.log 2>&1 &
    scale_pid=$!
fi
[ "$MM_MODE" != control ] && export TF_MENU_DEBUG_MOVE=HEADLESS-1
t0=$EPOCHREALTIME
timeout 10 thinfloe-menu 2>&1 | while IFS= read -r l; do
    printf "menu %s: %s\n" "$(awk -v a="$t0" -v b="$EPOCHREALTIME" "BEGIN { printf \"%.2f\", b - a }")" "$l"
done
[ -n "${scale_pid:-}" ] && kill "$scale_pid" 2>/dev/null
f=${TF_MENU_OUTPUT_FILE:-/tmp/xdg/thinfloe-menu-output}
echo "output-file: $(cat "$f" 2>/dev/null || echo missing)"
echo "applied: $(cat "$f.applied" 2>/dev/null || echo missing)"
sed "s/^/scale: /" /tmp/scale.log 2>/dev/null
true'
    out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=2 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e MM_MODE="$mode" \
        -e MM_SCRIPT="$script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               printf "%s\n" "$MM_SCRIPT" >/tmp/mm.sh &&
               thinfloe-compose-labwc-config /etc/thinfloe/labwc /nonexistent /nonexistent /tmp/xdg/thinfloe-labwc &&
               labwc -C /tmp/xdg/thinfloe-labwc -S "bash /tmp/mm.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during menu move test)"
        echo "$out" | grep -E '^(menu|scale|output-file|applied)'
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
    MM_RESULT=$(echo "$out" | grep -E '^menu .*(configure |window|moved)|^(scale|output-file|applied):|panicked')
    MM_CONFIGURES=$(echo "$out" | grep -E '^menu .*: thinfloe-menu: configure ' | grep -o '[0-9]*x[0-9]*$' | tr '\n' ' ')
    MM_FIRST=$(echo "$out" | grep -E '^menu .*: thinfloe-menu: configure ' | head -1 | grep -o '[0-9]*x[0-9]*$')
    MM_LAST=$(echo "$out" | grep -E '^menu .*: thinfloe-menu: configure ' | tail -1 | grep -o '[0-9]*x[0-9]*$')
    # One line per window replacement (TF_MENU_DEBUG): a replace loop that
    # ends on the right output must still fail.
    MM_REPLACES=$(echo "$out" | grep -cE '^menu .*: thinfloe-menu: window replaced$')
    MM_OUTPUT_FILE=$(echo "$out" | sed -n 's/^output-file: //p')
    MM_APPLIED=$(echo "$out" | sed -n 's/^applied: //p')
    # The seconds of the warning, and whether a configure came after it,
    # which only a new window gets: nothing else resizes the menu here.
    MM_WARN_AT=$(echo "$out" | grep -F 'The menu could not be moved to HEADLESS-1.' | head -1 | sed -n 's/^menu \([0-9.]*\): .*/\1/p')
    MM_CONF_AFTER_WARN=$(echo "$out" | awk '/The menu could not be moved to HEADLESS-1\./ { w = 1 } w && /thinfloe-menu: configure / { n++ } END { print n + 0 }')
}

# Control: no move asked. The menu stays where labwc puts it, so the moves
# case below can fail.
run_menu_move_case control
if [[ "$MM_FIRST" == 1024x768 && "$MM_LAST" == 1024x768 && "$MM_CONFIGURES" != *1000x600* \
      && -z "$MM_WARN_AT" && -z "$MM_APPLIED" && "$MM_REPLACES" -eq 0 ]]; then
    echo "ok   - without a chosen monitor the menu stays on the cursor output (1024x768)"
else
    echo "FAIL - control run: configures '$MM_CONFIGURES', applied '$MM_APPLIED', warning at '$MM_WARN_AT', $MM_REPLACES window replacements"
    echo "$MM_RESULT"
    fail=1
fi

run_menu_move_case moves
if [[ "$MM_FIRST" == 1024x768 && "$MM_LAST" == 1000x600 && "$MM_APPLIED" == HEADLESS-1 \
      && "$MM_OUTPUT_FILE" == HEADLESS-1 && -z "$MM_WARN_AT" && "$MM_REPLACES" -eq 1 ]]; then
    echo "ok   - the menu replaces its window once and moves to HEADLESS-1 once labwc-scale confirms it (configures $MM_CONFIGURES)"
else
    echo "FAIL - the menu did not move to HEADLESS-1 with one window replacement: configures '$MM_CONFIGURES', output file '$MM_OUTPUT_FILE', applied '$MM_APPLIED', warning at '$MM_WARN_AT', $MM_REPLACES window replacements"
    echo "$MM_RESULT"
    fail=1
fi

run_menu_move_case unanswered
if [[ -n "$MM_WARN_AT" ]] && awk -v t="$MM_WARN_AT" 'BEGIN { exit !(t >= 3 && t < 8) }' \
      && [[ "$MM_CONF_AFTER_WARN" -ge 1 && "$MM_LAST" == 1024x768 \
      && "$MM_OUTPUT_FILE" == HEADLESS-1 && "$MM_APPLIED" == missing && "$MM_REPLACES" -eq 1 ]]; then
    echo "ok   - unanswered, the menu replaces its window once after 3 s and logs that it could not be moved (at ${MM_WARN_AT}s, $MM_CONF_AFTER_WARN configure after)"
else
    echo "FAIL - unanswered move: warning at '$MM_WARN_AT', configures after it $MM_CONF_AFTER_WARN, configures '$MM_CONFIGURES', output file '$MM_OUTPUT_FILE', applied '$MM_APPLIED', $MM_REPLACES window replacements"
    echo "$MM_RESULT"
    fail=1
fi

# A fullscreen window on top keeps the keyboard when the menu opens a window
# behind it. The menu replaces its window whenever its monitor changes, e.g.
# when a monitor wakes from power save; labwc gives every new window the
# focus, so the keys went to the hidden menu instead of the ThinLinc session
# (containers/ubuntu/patches/0003-fullscreen-keeps-focus.patch). The menu
# starts, foot opens on top of it, and 3 s later the unanswered
# TF_MENU_DEBUG_MOVE replaces the menu window. In the windowed control run
# the new menu window still takes the focus, as before.
run_focus_case() {
    local mode="$1" name="thinfloe-runtime-test-focus-$$-$1" script out rc
    # shellcheck disable=SC2016 # expands inside the container
    script='export TF_MENU_DEBUG=1 TF_MENU_DEBUG_MOVE=HEADLESS-1 TF_MENU_OUTPUT_FILE=/tmp/xdg/test-menu-output
timeout 9 thinfloe-menu >/tmp/menu.log 2>&1 &
sleep 1
if [ "$FC_MODE" = fullscreen ]; then foot -F sleep 20 & else foot sleep 20 & fi
sleep 1.5
wlrctl toplevel find app_id:foot state:active && echo before=active || echo before=inactive
sleep 4
wlrctl toplevel find app_id:foot state:active && echo after=active || echo after=inactive
echo "replaced=$(grep -c "window replaced" /tmp/menu.log)"
wait'
    # shellcheck disable=SC2016 # expands inside the container
    out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=1 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e FC_MODE="$mode" \
        -e FC_SCRIPT="$script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               printf "%s\n" "$FC_SCRIPT" >/tmp/fc.sh &&
               labwc -C /etc/thinfloe/labwc -S "sh /tmp/fc.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during focus test)"
        echo "$out" | grep -E '^(before|after|replaced)='
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
    FC_RESULT=$(echo "$out" | grep -E '^(before|after|replaced)=' | tr '\n' ' ')
}

run_focus_case fullscreen
if [[ "$FC_RESULT" == "before=active after=active replaced=1 " ]]; then
    echo "ok   - a fullscreen window keeps the keyboard when the menu replaces its window behind it"
else
    echo "FAIL - fullscreen foot lost the focus to the menu window: $FC_RESULT"
    fail=1
fi

run_focus_case windowed
if [[ "$FC_RESULT" == "before=active after=inactive replaced=1 " ]]; then
    echo "ok   - beside a window that is not fullscreen, the new menu window takes the focus"
else
    echo "FAIL - windowed control run: $FC_RESULT (expected foot to lose the focus to the new menu window)"
    fail=1
fi

# Native Wayland remote desktop clients (OxuLnk) ask labwc to switch its
# keybindings off for their window with zwp_keyboard_shortcuts_inhibit_v1
# (containers/ubuntu/patches/0004-keyboard-shortcuts-inhibit.patch). As with
# the Xwayland grab manager above, this rig has no client that creates an
# inhibitor and no keyboard, so it only asserts that labwc advertises the
# global to an ordinary client.
INHIBIT_CONTAINER_NAME="thinfloe-runtime-test-inhibit-$$"
inhibit_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$INHIBIT_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=1 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           labwc -C /etc/thinfloe/labwc -S "sh -c \"WAYLAND_DEBUG=1 timeout 3 foot true\""' 2>&1)
inhibit_rc=$?
if [[ $inhibit_rc -eq 124 || $inhibit_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during inhibit test)"
    podman rm -f "$INHIBIT_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi
if echo "$inhibit_out" | grep -Eq 'wl_registry[@#][0-9]+\.global\([0-9]+, "zwp_keyboard_shortcuts_inhibit_manager_v1"'; then
    echo "ok   - labwc advertises the keyboard shortcuts inhibit manager"
else
    echo "FAIL - labwc does not advertise zwp_keyboard_shortcuts_inhibit_manager_v1"
    echo "$inhibit_out" | grep -E 'wl_registry[@#][0-9]+\.global' | head -60
    fail=1
fi

exit $fail
