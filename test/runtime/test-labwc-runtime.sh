#!/bin/bash
# Runs labwc headless with two 1280x720 outputs and asserts that a fullscreen
# X11 client is given the union of both (2560x720), not a single output.
# Also asserts that Xwayland binds the keyboard-grab manager, and that
# labwc's environment-file parser lets a later duplicate assignment win
# (the assumption thinfloe-compose-labwc-config's merge depends on).
# Finally asserts that thinfloe-menu, a native Wayland client, maps at the full
# output size, rasterises on the CPU and keeps running, and that a fullscreen
# window keeps the keyboard focus when the menu replaces its window behind it,
# and that labwc loads no keybindings: Alt-Tab and its kin reach the focused
# window, and that a keybinding marked allowWhenInhibited passes an X11
# keyboard grab while an unmarked one does not, and that the OSD of the
# volume and brightness keys shows above a fullscreen window on every
# output without taking its focus or its clicks, and that the volume and
# brightness keys work end to end: the Keyboard tab's switches decide whether
# labwc takes the keys, the OSD shows the level, the microphone mark and
# "not supported" without DDC/CI, and survives an output going away, and
# that a Deskflow capture strip takes the pointer lock and the keys from a
# fullscreen X11 window holding a keyboard grab and gives them back.
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
if grep -Eq 'wl_registry[@#][0-9]+\.bind\([0-9]+, "zwp_xwayland_keyboard_grab_manager_v1"' <<<"$grab_out"; then
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

if grep -qx 'MERGED XKB_DEFAULT_LAYOUT=ch' <<<"$kb_out"; then
    echo "ok   - a keyboard apply reaches labwc's environment"
else
    echo "FAIL - the merged environment lacks XKB_DEFAULT_LAYOUT=ch after the apply"
    echo "$kb_out"
    fail=1
fi
if grep -q 'failed to create xkb keymap' < <(after_apply <<<"$kb_control"); then
    if grep -q 'failed to create xkb keymap' < <(after_apply <<<"$kb_out"); then
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
elif grep -qx 'SESSION-RAN fr//' <<<"$kb_start"; then
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

if grep -q 'thinfloe-menu: configure 1280x720' <<<"$menu_out"; then
    echo "ok   - thinfloe-menu maps at the full output size (1280x720)"
else
    echo "FAIL - thinfloe-menu did not configure at 1280x720"
    echo "$menu_out"
    fail=1
fi
if grep -q 'thinfloe-menu: raster' <<<"$menu_out"; then
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
if grep -q 'menu-rc=124' <<<"$menu_out"; then
    echo "ok   - thinfloe-menu kept running until stopped"
else
    echo "FAIL - thinfloe-menu exited on its own: $(echo "$menu_out" | grep menu-rc)"
    fail=1
fi

# Geo must be known to fontconfig in the built image (title bar font).
geo_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none "$IMAGE" fc-list 2>&1)
if grep -q 'Geo' <<<"$geo_out"; then
    echo "ok   - fc-list lists Geo"
else
    echo "FAIL - fc-list does not list Geo"
    fail=1
fi

# A folded window must stay reachable: labwc has no keybindings unless a key
# switch is on (and then only the media keys), so the menu is the only way
# back. foot is folded with wlrctl; the menu (TF_MENU_DEBUG) must see
# it, --windows must list it as folded, --unfold must bring it back.
FOLD_CONTAINER_NAME="thinfloe-runtime-test-fold-$$"

# The session script travels in an environment variable and is written to a
# file in the container, so no quoting has to survive labwc's -S.
# The --windows lists go to files and are printed only after labwc has
# exited: the menu's debug lines (eprintln!, unbuffered, several writes per
# line) and labwc's log share the output stream, and a list line printed
# into the middle of one of them no longer starts with "foot".
FOLD_SCRIPT='foot sleep 60 &
sleep 2
TF_MENU_DEBUG=1 timeout 20 thinfloe-menu 2>&1 &
sleep 4
wlrctl toplevel minimize app_id:foot
sleep 2
thinfloe-menu --windows >/tmp/list1
thinfloe-menu --unfold foot; echo unfold-rc=$?
sleep 2
thinfloe-menu --windows >/tmp/list2
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
           { labwc -C /etc/thinfloe/labwc -S "sh /tmp/fold.sh"; rc=$?; } &&
           for l in list1 list2; do echo "BEGIN-$l"; cat "/tmp/$l"; echo "END-$l"; done;
           exit $rc' 2>&1)
fold_rc=$?

if [[ $fold_rc -eq 124 || $fold_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during folded-window test)"
    echo "$fold_out"
    podman rm -f "$FOLD_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi

list1=$(echo "$fold_out" | sed -n '/^BEGIN-list1$/,/^END-list1$/p')
list2=$(echo "$fold_out" | sed -n '/^BEGIN-list2$/,/^END-list2$/p')
if grep -q 'thinfloe-menu: folded: foot ' <<<"$fold_out"; then
    echo "ok   - thinfloe-menu saw foot fold"
else
    echo "FAIL - thinfloe-menu never printed 'folded: foot'"
    echo "$fold_out"
    fail=1
fi
if grep -Eq '^foot	.*	folded$' <<<"$list1"; then
    echo "ok   - --windows lists the folded foot as folded"
else
    echo "FAIL - --windows did not list foot as folded"
    echo "$fold_out"
    fail=1
fi
if grep -q 'unfold-rc=0' <<<"$fold_out"; then
    echo "ok   - --unfold foot exited 0"
else
    echo "FAIL - --unfold foot did not exit 0"
    echo "$fold_out"
    fail=1
fi
if grep -Eq '^foot	.*	open$' <<<"$list2"; then
    echo "ok   - --windows lists foot as open after --unfold"
else
    echo "FAIL - foot is not open after --unfold"
    echo "$fold_out"
    fail=1
fi
if grep -q 'thinfloe-menu: unfolded: foot ' <<<"$fold_out"; then
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
    "lines = [\"-battery 1x1\", \"network 2x2\", \"-connection 1x2\", \"-clock 1x1\"," \
    "    \"-temperatures 1x1\", \"-cpu 1x1\", \"-memory 1x1\"]" >/tmp/cover-cfg/config.toml
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

if grep -qx 'scale-before=1' <<<"$cover_out" && grep -qx 'scale-covered=1' <<<"$cover_out"; then
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
if grep -qx 'kanshi-first=1280x720@1280,0' <<<"$kanshi_out" \
    && grep -qx 'kanshi-second=1280x720@0,720' <<<"$kanshi_out"; then
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
    grep -qx 'rule-inserted' <<<"$out" && MOVE_RULE_OK=1
    MOVE_FIRST=$(echo "$out" | grep -E '^first: .*configure ' | tail -1 | grep -o '[0-9]*x[0-9]*$')
    MOVE_SECOND=$(echo "$out" | grep -E '^second: .*configure ' | tail -1 | grep -o '[0-9]*x[0-9]*$')
    MOVE_INVALID=0
    grep -q 'Invalid output' <<<"$out" && MOVE_INVALID=1
    MOVE_CRASH=0
    grep -Eqi 'segmentation|core dumped|panicked' <<<"$out" && MOVE_CRASH=1
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
    && grep -qx 'size HEADLESS-1 1000x600' <<<"$MOVE_RESULT" \
    && grep -qx 'size HEADLESS-2 1024x768' <<<"$MOVE_RESULT"; then
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
if grep -Eq 'wl_registry[@#][0-9]+\.global\([0-9]+, "zwp_keyboard_shortcuts_inhibit_manager_v1"' <<<"$inhibit_out"; then
    echo "ok   - labwc advertises the keyboard shortcuts inhibit manager"
else
    echo "FAIL - labwc does not advertise zwp_keyboard_shortcuts_inhibit_manager_v1"
    echo "$inhibit_out" | grep -E 'wl_registry[@#][0-9]+\.global' | head -60
    fail=1
fi

# ThinFloe has no hotkeys. labwc 0.9.3 loads its built-in keybindings
# (A-Tab, A-F4, A-Space, W-Return, W-a, ...) whenever rc.xml defines none;
# the shipped images had them although rc.xml has no keybind, and Alt-Tab
# switched windows on the desktop instead of reaching ThinLinc
# (containers/ubuntu/patches/0005-no-default-keybinds.patch). The rc.xml text
# guards in test/integration cannot see this, so this asks the compositor.
#
# Two foot windows; the second ("two") has the focus and copies every byte it
# gets to /tmp/keys. wtype types x, Alt-Tab, Alt-Space, Super-a, y through one
# virtual keyboard: wlrctl makes a new virtual keyboard per call and foot has
# not bound it when the key arrives, so nothing reached foot that way. The
# leading 1 s pause lets foot bind the new keyboard. With labwc's defaults,
# Alt-Tab moves the focus to "one" and only x arrives; with none, foot gets
# 78 1b 09 1b 20 61 79 (x, ESC Tab, ESC space, a, y) and keeps the focus.
# A rig that sends no keys at all fails the byte check, so this cannot pass
# vacuously.
KB_CONTAINER_NAME="thinfloe-runtime-test-keys-$$"
# shellcheck disable=SC2016 # expands inside the container
kb_script='foot --app-id=one sleep 20 &
one=$!
sleep 1.5
foot --app-id=two sh -c "stty raw -echo; exec dd bs=1 of=/tmp/keys 2>/dev/null" &
two=$!
sleep 1.5
wlrctl toplevel find app_id:two state:active && echo kb_before=active || echo kb_before=inactive
wtype -s 1000 x -M alt -P tab -p tab -m alt -s 300 -M alt -P space -p space -m alt -s 300 -M logo a -m logo -s 300 y -s 500
sleep 1
wlrctl toplevel find app_id:two state:active && echo kb_after=active || echo kb_after=inactive
echo "kb_bytes=$(od -An -tx1 /tmp/keys | tr -s " \n" " " | sed "s/^ //; s/ $//")"
kill "$one" "$two" 2>/dev/null
exit 0'
kb_out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$KB_CONTAINER_NAME" \
    -e WLR_BACKENDS=headless \
    -e WLR_HEADLESS_OUTPUTS=1 \
    -e WLR_RENDERER=pixman \
    -e XDG_RUNTIME_DIR=/tmp/xdg \
    -e KB_SCRIPT="$kb_script" \
    "$IMAGE" \
    sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
           printf "%s\n" "$KB_SCRIPT" >/tmp/kb.sh &&
           labwc -V -C /etc/thinfloe/labwc -S "sh /tmp/kb.sh"' 2>&1)
kb_rc=$?
if [[ $kb_rc -eq 124 || $kb_rc -eq 137 ]]; then
    echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during keybinding test)"
    podman rm -f "$KB_CONTAINER_NAME" >/dev/null 2>&1
    exit 1
fi
if grep -q 'load default key bindings' <<<"$kb_out"; then
    echo "FAIL - labwc loaded its default keybindings"
    echo "$kb_out" | grep 'load default key bindings'
    fail=1
else
    echo "ok   - labwc loads no default keybindings"
fi
KB_RESULT=$(echo "$kb_out" | grep -E '^kb_(before|after|bytes)=' | tr '\n' ' ')
if [[ "$KB_RESULT" == "kb_before=active kb_after=active kb_bytes=78 1b 09 1b 20 61 79 " ]]; then
    echo "ok   - Alt-Tab, Alt-Space and Super-a reach the focused window, which keeps the focus"
else
    echo "FAIL - keys did not all reach the focused window: $KB_RESULT (expected kb_before=active kb_after=active kb_bytes=78 1b 09 1b 20 61 79)"
    fail=1
fi


# Volume and brightness keys must reach the machine while ThinLinc holds an
# X11 keyboard grab or OxuLnk a shortcuts inhibitor, and nothing else may
# (containers/ubuntu/patches/0006-keybind-allow-when-inhibited.patch). A test
# rc.xml has two bindings: XF86AudioRaiseVolume with allowWhenInhibited="yes"
# appends "pass" to /tmp/hits, XF86AudioLowerVolume without it "plain". wtype
# starts first, so a virtual keyboard exists before Xwayland starts (Xwayland
# installs its grab hook only when the seat has a keyboard), and sends both
# keys after 4 s. The probe maps a window, takes the X11 grab with --grab, and
# prints the keys it gets (--keymap names wtype's key codes, which Xwayland's
# default keymap in this rig does not). Under the grab only the marked binding runs and the
# plain key reaches the probe; the control run without the grab runs both
# bindings, which proves the rig sends both keys.
# No client in this rig creates a zwp_keyboard_shortcuts_inhibitor_v1; the
# OxuLnk path rests on the same line of labwc and stays an owner check.
# shellcheck disable=SC2016 # expands inside the container
gb_rcxml='<?xml version="1.0"?>
<labwc_config>
  <keyboard>
    <keybind key="XF86AudioRaiseVolume" allowWhenInhibited="yes"><action name="Execute" command="sh -c '"'"'echo pass >>/tmp/hits'"'"'" /></keybind>
    <keybind key="XF86AudioLowerVolume"><action name="Execute" command="sh -c '"'"'echo plain >>/tmp/hits'"'"'" /></keybind>
  </keyboard>
</labwc_config>'
# shellcheck disable=SC2016 # expands inside the container
gb_script='touch /tmp/hits
wtype -s 4000 -k XF86AudioRaiseVolume -s 300 -k XF86AudioLowerVolume &
sleep 0.5
python3 /usr/local/bin/x11-probe.py $GB_GRAB --keymap XF86AudioRaiseVolume,XF86AudioLowerVolume --keys 7
echo "hits=$(tr "\n" " " </tmp/hits)"
exit 0'
gb_run() {
    local name="thinfloe-runtime-test-gb-$1-$$"
    local grab_arg=""
    [[ $1 == grab ]] && grab_arg="--grab"
    GB_OUT=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=1 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e GB_GRAB="$grab_arg" \
        -e GB_RCXML="$gb_rcxml" \
        -e GB_SCRIPT="$gb_script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg /tmp/gb && chmod 700 /tmp/xdg &&
               cp /etc/thinfloe/labwc/themerc-override /etc/thinfloe/labwc/environment /tmp/gb/ &&
               printf "%s\n" "$GB_RCXML" >/tmp/gb/rc.xml &&
               printf "%s\n" "$GB_SCRIPT" >/tmp/gb.sh &&
               WAYLAND_DEBUG=1 labwc -C /tmp/gb -S "sh /tmp/gb.sh"' 2>&1)
    local rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during grab bypass test)"
        echo "$GB_OUT" | grep -E '^(KEY |hits=)'
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
    GB_HITS=$(echo "$GB_OUT" | grep -E '^hits=' | tail -1)
    GB_KEYS=$(echo "$GB_OUT" | grep -E '^KEY ' | tr '\n' ' ')
}

gb_run grab
if ! echo "$GB_OUT" | grep -Eq 'zwp_xwayland_keyboard_grab_manager_v1[@#][0-9]+\.grab_keyboard\('; then
    echo "skip - the rig cannot drive an X11 keyboard grab; the grab bypass is an owner check on hardware"
else
    if [[ "$GB_HITS" == "hits=pass " ]]; then
        echo "ok   - under an X11 keyboard grab only the allowWhenInhibited binding runs"
    else
        echo "FAIL - under an X11 keyboard grab: $GB_HITS (expected hits=pass )"
        fail=1
    fi
    if [[ "$GB_KEYS" == *"KEY XF86AudioLowerVolume "* && "$GB_KEYS" != *"KEY XF86AudioRaiseVolume "* ]]; then
        echo "ok   - the grabbing client gets the plain key and not the allowWhenInhibited one"
    else
        echo "FAIL - keys at the grabbing client: '$GB_KEYS' (expected KEY XF86AudioLowerVolume only)"
        fail=1
    fi
fi

gb_run nograb
if [[ "$GB_HITS" == "hits=pass plain " ]]; then
    echo "ok   - without a grab both bindings run (control)"
else
    echo "FAIL - control run without a grab: $GB_HITS (expected hits=pass plain )"
    fail=1
fi

# The on-screen display of the volume and brightness keys must show above a
# fullscreen window, on every monitor, without taking the keyboard focus or
# the clicks of that window. 'thinfloe-menu --osd --probe' maps the OSD's
# layer surfaces (overlay layer, keyboard interactivity none, an empty input
# region) and draws a Volume 45 % panel and the muted-microphone mark: a
# 320x61 panel in TILE (0d1218) with a 1 px LINE (151b22) frame 22 px above
# the bottom centre, and a 30x30 mark in TILE with a WARN (c79a4a) frame 14
# px from the right and 34 px from the top. grim reads the pixels back; on a
# 1280x720 output the panel is x 480..799, y 637..697 and the mark x
# 1236..1265, y 34..63. The interior points miss the text, the icons and the
# bar: the panel's (640,668) lies between the name and the level, the
# mark's (1239,49) left of its 18 px icon (x 1242..1259). A red point outside the
# OSD (100,100) proves the window really is under it. The X11 run uses two
# outputs and the shipped config, so the probe window spans both (2560x720,
# at 0,0: window and global coordinates agree); a click on the mark of
# output 2 (2531,49) must reach the probe. The native run has a fullscreen
# foot on one output that copies every byte it gets to /tmp/keys; an x typed
# while the OSD is up must reach it. wlrctl's state:active alone cannot see
# a layer surface take the keyboard: with keyboard interactivity exclusive
# the toplevel stays active (tried).
# shellcheck disable=SC2016 # expands inside the container
osd_script='if [ "$OSD_MODE" = x11 ]; then
    python3 /usr/local/bin/x11-probe.py --background ff0000 --hold 12 --buttons &
    app=thinfloe-probe
else
    foot -o colors.background=ff0000 --fullscreen sh -c "stty raw -echo; exec timeout 12 dd bs=1 of=/tmp/keys 2>/dev/null" &
    app=foot
fi
sleep 3
wlrctl toplevel find app_id:$app state:active && echo focus_before=active || echo focus_before=inactive
thinfloe-menu --osd --probe &
osd=$!
sleep 2
wlrctl toplevel find app_id:$app state:active && echo focus_after=active || echo focus_after=inactive
grim -t ppm /tmp/shot.ppm
if [ "$OSD_MODE" = x11 ]; then
    wlrctl pointer move -5000 -5000; wlrctl pointer move 2531 49; wlrctl pointer click left
else
    wtype -s 1000 x -s 300
    echo "keys=$(od -An -tx1 /tmp/keys | tr -d " \n")"
fi
sleep 1
python3 -c "
import sys
data = open(\"/tmp/shot.ppm\", \"rb\").read()
fields, pos = [], 0
while len(fields) < 4:
    while data[pos:pos + 1].isspace():
        pos += 1
    start = pos
    while not data[pos:pos + 1].isspace():
        pos += 1
    fields.append(data[start:pos])
pos += 1
w = int(fields[1])
print(\"shot %sx%s\" % (fields[1].decode(), fields[2].decode()))
for arg in sys.argv[1:]:
    name, xy = arg.split(\"=\")
    x, y = map(int, xy.split(\",\"))
    o = pos + 3 * (y * w + x)
    print(\"px %s=%s\" % (name, data[o:o + 3].hex()))
" $OSD_POINTS
kill $osd
wait $osd
echo osd_rc=$?
wait
exit 0'
osd_run() {
    local mode="$1" outputs="$2" points="$3" name="thinfloe-runtime-test-osd-$1-$$" rc
    OSD_OUT=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS="$outputs" \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e OSD_MODE="$mode" \
        -e OSD_POINTS="$points" \
        -e OSD_SCRIPT="$osd_script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               printf "%s\n" "$OSD_SCRIPT" >/tmp/osd.sh &&
               labwc -C /etc/thinfloe/labwc -S "sh /tmp/osd.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during OSD test)"
        echo "$OSD_OUT" | grep -E '^(GEOMETRY|BUTTON|focus_|px |shot |osd_rc=)'
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
}
# osd_px NAME: the colour grim read at the named point.
osd_px() {
    echo "$OSD_OUT" | grep -E "^px $1=" | tail -1 | cut -d= -f2
}
# osd_expect WHAT NAME=RRGGBB...: one ok or FAIL line for all of them.
osd_expect() {
    local what="$1" bad="" p
    shift
    for p in "$@"; do
        [[ "$(osd_px "${p%%=*}")" == "${p#*=}" ]] || bad+=" ${p%%=*}=$(osd_px "${p%%=*}") (want ${p#*=})"
    done
    if [[ -z "$bad" ]]; then
        echo "ok   - $what"
    else
        echo "FAIL - $what:$bad"
        fail=1
    fi
}
osd_focus() {
    local what="$1" r
    r=$(echo "$OSD_OUT" | grep -E '^(focus_before|focus_after|osd_rc)=' | tr '\n' ' ')
    if [[ "$r" == "focus_before=active focus_after=active osd_rc=0 " ]]; then
        echo "ok   - $what keeps the focus under the OSD, which ends with 0 on SIGTERM"
    else
        echo "FAIL - $what under the OSD: $r (expected focus_before=active focus_after=active osd_rc=0)"
        echo "$OSD_OUT" | grep -E 'thinfloe-menu' | head -20
        fail=1
    fi
}

osd_run x11 2 "panel1=640,668 panel2=1920,668 frame1=480,668 mark1=1239,49 mark2=2519,49 markframe1=1265,49 control=100,100"
osd_geom=$(echo "$OSD_OUT" | grep -o '^GEOMETRY [0-9]*x[0-9]*' | tail -1 | cut -d' ' -f2)
if [[ "$osd_geom" == "2560x720" ]]; then
    echo "ok   - the X11 window under the OSD spans both outputs ($osd_geom)"
else
    echo "FAIL - the X11 window under the OSD is '$osd_geom', expected 2560x720"
    fail=1
fi
osd_expect "the OSD panel shows on both outputs above a spanning X11 window" \
    panel1=0d1218 panel2=0d1218 frame1=151b22
osd_expect "the OSD mark shows on both outputs above a spanning X11 window" \
    mark1=0d1218 mark2=0d1218 markframe1=c79a4a
osd_expect "the X11 window is under the OSD (control point)" control=ff0000
osd_focus "the spanning X11 window"
if grep -qx 'BUTTON 2531 49' <<<"$OSD_OUT"; then
    echo "ok   - a click on the OSD mark reaches the X11 window below"
else
    echo "FAIL - the click on the OSD mark did not reach the X11 window: '$(echo "$OSD_OUT" | grep '^BUTTON' | tr '\n' ' ')' (expected BUTTON 2531 49)"
    fail=1
fi

osd_run native 1 "panel1=640,668 frame1=480,668 mark1=1239,49 markframe1=1265,49 control=100,100"
osd_expect "the OSD panel and mark show above a native fullscreen window" \
    panel1=0d1218 frame1=151b22 mark1=0d1218 markframe1=c79a4a
osd_expect "the native fullscreen window is under the OSD (control point)" control=ff0000
osd_focus "the native fullscreen window"
osd_keys=$(echo "$OSD_OUT" | grep -E '^keys=' | tail -1)
if [[ "$osd_keys" == "keys=78" ]]; then
    echo "ok   - a key typed under the OSD reaches the native fullscreen window"
else
    echo "FAIL - a key typed under the OSD did not reach the native fullscreen window: '$osd_keys' (expected keys=78)"
    fail=1
fi

# The volume and brightness keys end to end: the Keyboard tab's switches
# (THINFLOE_VOLUME_KEYS, THINFLOE_BRIGHTNESS_KEYS in generated/environment),
# the image's composer, labwc's bindings, 'thinfloe-menu --osd-key' and the
# OSD process. A fake pactl (TF_PACTL) logs its arguments to /tmp/pactl.log
# and keeps two sinks (spk_a, spk_b at 45 %), two sources (mic_a, mic_b at
# 80 %, unmuted) and the monitor source spk_a.monitor in /tmp/pa as pactl 17
# prints them (tests/fixtures/pactl-*.json); a set changes the file and
# appends a change event, as pipewire-pulse does, and 'subscribe' follows
# /tmp/pa/events. PA_NOLOG=1 changes a device without a log line, as another
# program would. A fake ddcutil (TF_DDCUTIL) finds no monitor. wtype sends the keys
# through one virtual keyboard per call; the leading 1 s pause lets wev bind
# it (see the keybinding case above). wev prints a sym line for the press and
# for the release of a key that reaches it.
#
# The pixels, as the OSD case above, on 1280x720 outputs: the Volume panel is
# x 480..799, y 637..697 (61 high, 22 above the bottom). Its ICE (7cc4ff) bar
# is 6 high at y 680..685, from x 496 (16 in) over 45 % of the 288 px track:
# x 496..625. (547,682) is the brief's bar point, (522,682) a bar point that
# lies in the gap between the icon (x 496..517) and the name (from x 528) of a
# panel without a bar. The panel without a bar (Microphone off, Brightness not
# supported) is 46 high, y 652..697: its top edge moves down by 15 px. So
# (760,645) is TILE (0d1218) in the Volume panel and red above the shorter
# one; (760,652) is its top border (LINE, 151b22); (760,690) is inside both,
# below the row of icon and text, and x 760 is right of the name. Its icon is
# the box x 496..517, y 664..685 (16 in, 12 below its top, 22 square), drawn
# in WARN (c79a4a) for Microphone off; output 2 adds 1280 to x. The mark's
# right border (WARN, c79a4a) is at (1265,49) on output 1 and (2545,49) on
# output 2.
# shellcheck disable=SC2016 # expands inside the container
mk_pactl='#!/usr/bin/python3
import json, os, sys, time
D = "/tmp/pa/"
a = sys.argv[1:]
def dev(i, name, pct, mute, monitor=""):
    ch = {"value": round(pct * 65536 / 100), "value_percent": "%d%%" % pct, "db": "0.00 dB"}
    return {"index": i, "state": "SUSPENDED", "name": name, "description": name, "driver": "PipeWire",
            "mute": mute, "volume": {"front-left": dict(ch), "front-right": dict(ch)}, "balance": 0.0,
            "monitor_source": monitor, "ports": [], "active_port": None, "formats": ["pcm"],
            "properties": {"node.name": name, "device.class": "monitor" if monitor else "sound"}}
def load(kind):
    with open(D + kind + "s.json") as f:
        return json.load(f)
def save(kind, devs):
    with open(D + kind + "s.json.new", "w") as f:
        json.dump(devs, f)
    os.rename(D + kind + "s.json.new", D + kind + "s.json")
def event(what, kind, i):
    with open(D + "events", "a") as f:
        f.write("Event %s%s%s on %s #%d\n" % (chr(39), what, chr(39), kind, i))
def done(text=""):
    sys.stdout.write(text)
    sys.exit(0)
if a == ["tf-init"]:
    os.makedirs(D, exist_ok=True)
    save("sink", [dev(1, "spk_a", 45, False), dev(2, "spk_b", 45, False)])
    save("source", [dev(3, "spk_a.monitor", 100, False, "spk_a"), dev(4, "mic_a", 80, False), dev(5, "mic_b", 80, False)])
    for k, v in (("default-sink", "spk_a\n"), ("default-source", "mic_a\n"), ("events", "")):
        with open(D + k, "w") as f:
            f.write(v)
    done()
if a[:1] == ["tf-add-source"]:
    devs = load("source")
    devs.append(dev(int(a[2]), a[1], 100, False))
    save("source", devs)
    event("new", "source", int(a[2]))
    done()
if not os.environ.get("PA_NOLOG"):
    with open("/tmp/pactl.log", "a") as f:
        f.write(" ".join(a) + "\n")
if a == ["subscribe"]:
    # PA_SUBSCRIBE_FAIL=N: the first N runs find no sound server.
    n = 0
    if os.path.exists(D + "subscribe-fails"):
        n = int(open(D + "subscribe-fails").read())
    if n < int(os.environ.get("PA_SUBSCRIBE_FAIL") or 0):
        with open(D + "subscribe-fails", "w") as f:
            f.write(str(n + 1))
        sys.stderr.write("Connection failure: Connection refused\n")
        sys.exit(1)
    # Follow the events file as pactl follows the server. Not tail -f:
    # coreutils tail ends by itself once its reader is gone, pactl does not.
    with open(D + "events") as f:
        while True:
            line = f.readline()
            if line:
                sys.stdout.write(line)
                sys.stdout.flush()
            else:
                time.sleep(0.05)
if len(a) == 4 and a[:3] == ["-f", "json", "list"] and a[3] in ("sinks", "sources"):
    with open(D + a[3] + ".json") as f:
        done(f.read())
if a in (["get-default-sink"], ["get-default-source"]):
    with open(D + "default-" + a[0][12:]) as f:
        done(f.read())
if len(a) == 3 and a[0].split("-")[:1] == ["set"]:
    _, kind, what = a[0].split("-")
    devs = load(kind)
    for d in devs:
        if d["name"] == a[1]:
            if what == "volume":
                p = int(a[2].rstrip("%"))
                for ch in d["volume"].values():
                    ch["value"], ch["value_percent"] = round(p * 65536 / 100), "%d%%" % p
            else:
                d["mute"] = {"1": True, "0": False, "toggle": not d["mute"]}[a[2]]
            save(kind, devs)
            event("change", kind, d["index"])
            done()
    sys.stderr.write("Failure: No such entity\n")
    sys.exit(1)
sys.stderr.write("fake pactl: unknown call %s\n" % a)
sys.exit(1)'
# shellcheck disable=SC2016 # expands inside the container
mk_ddcutil='#!/bin/sh
echo "$*" >>/tmp/ddcutil.log
exit 1'
# px.py TAG [--within S] SPEC...: grim takes the screen and each SPEC prints
# "px TAG name=value". name=x,y gives the colour at a point, name=x,y@rrggbb
# the same with an expectation; name=x0,y0,x1,y1#rrggbb is "found" when some
# pixel of the box (x1, y1 exclusive) has the colour, else "none". With
# --within S the screen is taken again until every expectation holds, at
# most S seconds.
# shellcheck disable=SC2016 # expands inside the container
mk_px='import subprocess, sys, time
def shot():
    data = subprocess.run(["grim", "-t", "ppm", "-"], capture_output=True, check=True).stdout
    fields, pos = [], 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    return data, pos + 1, int(fields[1]), int(fields[2])
def at(img, x, y):
    data, pos, w, h = img
    if not (0 <= x < w and 0 <= y < h):
        return "outside"
    o = pos + 3 * (y * w + x)
    return data[o:o + 3].hex()
def value(img, spec):
    if "#" in spec:
        box, want = spec.split("#")
        x0, y0, x1, y1 = map(int, box.split(","))
        hit = any(at(img, x, y) == want for y in range(y0, y1) for x in range(x0, x1))
        return ("found" if hit else "none"), "found"
    xy, _, want = spec.partition("@")
    x, y = map(int, xy.split(","))
    return at(img, x, y), (want or None)
args = sys.argv[1:]
tag = args.pop(0)
within = 0.0
if args and args[0] == "--within":
    within = float(args[1])
    args = args[2:]
t0 = time.monotonic()
while True:
    img = shot()
    vals = [(a.split("=")[0],) + value(img, a.split("=", 1)[1]) for a in args]
    if all(w is None or v == w for _, v, w in vals) or time.monotonic() - t0 >= within:
        break
    time.sleep(0.1)
for name, v, _ in vals:
    print("px %s %s=%s" % (tag, name, v), flush=True)'
# shellcheck disable=SC2016 # expands inside the container
mk_script='# after T0 S: sleep until S seconds after the time T0 ($EPOCHREALTIME).
after() { sleep "$(awk -v a="$1" -v s="$2" -v n="$EPOCHREALTIME" "BEGIN { d = a + s - n; print (d > 0 ? d : 0) }")"; }
px() { python3 /tmp/px.py "$@"; }
wev_syms() { tail -n +"$1" /tmp/wev.log | grep -o "sym: [A-Za-z0-9_]*" | cut -d" " -f2 | tr "\n" " "; }
# pa ARGS: change a device as another program would (no log line).
pa() { PA_NOLOG=1 /tmp/fake-pactl "$@"; }
# ms_since T0: milliseconds since the time T0 ($EPOCHREALTIME).
ms_since() { awk -v a="$1" -v n="$EPOCHREALTIME" "BEGIN { printf \"%d\", (n - a) * 1000 }"; }
# wait_log S LINE...: wait at most S seconds until /tmp/pactl.log, from
# line $FROM on (default 1), has every LINE.
wait_log() {
    local deadline missing l
    deadline=$(awk -v n="$EPOCHREALTIME" -v s="$1" "BEGIN { printf \"%.3f\", n + s }")
    shift
    while :; do
        missing=0
        for l in "$@"; do
            grep -qxF -- "$l" <(tail -n +"${FROM:-1}" /tmp/pactl.log 2>/dev/null) || missing=1
        done
        [ "$missing" = 0 ] && return 0
        awk -v n="$EPOCHREALTIME" -v u="$deadline" "BEGIN { exit !(n > u) }" && return 1
        sleep 0.05
    done
}
# start_osd: the OSD, and socket=yes once it listens.
start_osd() {
    thinfloe-menu --osd 2>/tmp/osd.err &
    osd=$!
    for i in $(seq 50); do [ -S /tmp/xdg/thinfloe-osd.sock ] && break; sleep 0.1; done
    [ -S /tmp/xdg/thinfloe-osd.sock ] && echo socket=yes || echo socket=no
}
# stop_osd [SIGNAL]: SIGTERM (or SIGNAL), its status, its stderr, and
# whether its pactl subscribe is still there 1 s later.
stop_osd() {
    kill -"${1:-TERM}" "$osd"
    wait "$osd"
    echo "osd_rc=$?"
    local left f c i
    for i in $(seq 10); do
        left=no
        for f in /proc/[0-9]*/cmdline; do
            c=$(tr "\0" " " <"$f" 2>/dev/null)
            [[ "$c" == *"/tmp/fake-pactl subscribe"* ]] && left=yes
        done
        [ "$left" = no ] && break
        sleep 0.1
    done
    echo "subscribe_left=$left"
    sed "s/^/osderr: /" /tmp/osd.err
}
case $MK_MODE in
keys-off | keys-on | live)
    stdbuf -oL wev -f wl_keyboard:key >/tmp/wev.log 2>&1 &
    sleep 1.5
    wlrctl toplevel find app_id:wev state:active && echo wev_focus=active || echo wev_focus=inactive
    wtype -s 1000 -k XF86AudioRaiseVolume -s 300 x -s 500
    sleep 0.5
    echo "wev1=$(wev_syms 1)"
    if [ "$MK_MODE" = live ]; then
        n=$(($(wc -l </tmp/wev.log) + 1))
        printf "THINFLOE_VOLUME_KEYS=on\n" >/tmp/cfg/generated/environment
        thinfloe-compose-labwc-config --env-only /etc/thinfloe/labwc /tmp/cfg/generated/environment /nonexistent /tmp/xdg/thinfloe-labwc
        echo "binds=$(grep -c "keybind key=" /tmp/xdg/thinfloe-labwc/rc.xml)"
        labwc --reconfigure
        # labwc reads rc.xml again on the SIGHUP that --reconfigure sends; it
        # tells no client when it is done.
        sleep 1
        wtype -s 1000 -k XF86AudioRaiseVolume -s 300 y -s 500
        sleep 0.5
        echo "wev2=$(wev_syms "$n")"
    fi
    ;;
osd | bright)
    python3 /usr/local/bin/x11-probe.py --background ff0000 --hold 40 &
    sleep 3
    start_osd
    if [ "$MK_MODE" = osd ]; then
        wtype -k XF86AudioRaiseVolume
        t=$EPOCHREALTIME
        after "$t" 0.5
        px vol panel1=640,668 panel2=1920,668 bar1=547,682 bar2=1827,682 gap1=522,682 tall1=760,645
        after "$t" 2.5
        px volgone bar1=547,682 panel1=640,668
        # A volume key, then the microphone key 0.5 s later, while the
        # Volume panel still shows: the panel takes the size of the new view.
        wtype -k XF86AudioRaiseVolume -s 500 -k XF86AudioMicMute
        t=$EPOCHREALTIME
        after "$t" 0.5
        px mic in1=760,690 top1=760,652 above1=760,645 gap1=522,682 icon1=496,664,518,686#c79a4a in2=2040,690 top2=2040,652 above2=2040,645 gap2=1802,682 icon2=1776,664,1798,686#c79a4a
        px micmark --within 2 m1=1265,49@c79a4a m2=2545,49@c79a4a
        # The mark follows the default source, whoever sets it.
        pa set-source-mute mic_a 0
        px nomark --within 3 m1=1265,49@ff0000 m2=2545,49@ff0000
        pa set-source-mute mic_a 1
        px mark --within 2 m1=1265,49@c79a4a m2=2545,49@c79a4a
        # The mark follows the microphone, on every output, also when one goes.
        wlr-randr --output HEADLESS-2 --off
        sleep 1
        kill -0 "$osd" 2>/dev/null && echo osd_alive=yes || echo osd_alive=no
        px unplug m1=1265,49
        pa set-source-mute mic_a 0
        px markgone --within 2 m1=1265,49@ff0000
        # A microphone plugged in while the microphone key holds them muted
        # (the unmute above came from another program, not the key) is muted at once.
        t=$EPOCHREALTIME
        pa tf-add-source mic_c 6
        wait_log 1.5 "set-source-mute mic_c 1" && echo "hotplug_ms=$(ms_since "$t")" || echo hotplug_ms=none
    else
        # No sound server for the first two runs of pactl subscribe (at 0 s,
        # then 1 s later); the third, 2 s after that, reaches it. The start
        # puts every device at 100 % (the volume switch is off); spk_b then
        # drifts without an event, and a restart puts it back.
        wait_log 3 "set-sink-volume spk_b 100%" && pa set-sink-volume spk_b 30%
        from=$(($(grep -c "" /tmp/pactl.log) + 1))
        for i in $(seq 60); do
            [ "$(grep -cx subscribe /tmp/pactl.log)" -ge 3 ] && break
            sleep 0.1
        done
        FROM=$from wait_log 2 "set-sink-volume spk_b 100%" && echo resync=yes || echo resync=no
        # The first detection may run before the OSD knows the outputs; the
        # one 2 s after they came names both.
        for i in $(seq 80); do
            [ "$(grep -c "does not answer DDC/CI" /tmp/osd.err)" -ge 2 ] && break
            sleep 0.1
        done
        wtype -k XF86MonBrightnessUp
        px bright --within 1.2 warn1=640,652,800,698#c79a4a warn2=1920,652,2080,698#c79a4a
    fi
    stop_osd
    ;;
liveoff)
    start_osd
    # The keys move every speaker and microphone away from 100 % and
    # unmuted: up to 50 %, muted, and both microphones (at 80 %) muted.
    wtype -s 1000 -k XF86AudioRaiseVolume -s 300 -k XF86AudioMute -s 300 -k XF86AudioMicMute -s 300
    wait_log 3 "set-sink-volume spk_b 50%" "set-sink-mute spk_b 1" "set-source-mute mic_b 1" && echo keys=yes || echo keys=no
    # The Keyboard tab turns the switch off: the composer, labwc, then the
    # OSD is told.
    printf "THINFLOE_VOLUME_KEYS=off\n" >/tmp/cfg/generated/environment
    thinfloe-compose-labwc-config --env-only /etc/thinfloe/labwc /tmp/cfg/generated/environment /nonexistent /tmp/xdg/thinfloe-labwc
    labwc --reconfigure
    from=$(($(grep -c "" /tmp/pactl.log) + 1))
    echo "reload_from=$from"
    t=$EPOCHREALTIME
    thinfloe-menu --osd-key reload
    FROM=$from wait_log 1.5 \
        "set-sink-volume spk_a 100%" "set-sink-mute spk_a 0" "set-sink-volume spk_b 100%" "set-sink-mute spk_b 0" \
        "set-source-volume mic_a 100%" "set-source-mute mic_a 0" "set-source-volume mic_b 100%" "set-source-mute mic_b 0" &&
        echo "liveoff_ms=$(ms_since "$t")" || echo liveoff_ms=none
    # Another program mutes a microphone; a reload that does not change
    # the switch leaves it so.
    pa set-source-mute mic_a 1
    from=$(($(grep -c "" /tmp/pactl.log) + 1))
    thinfloe-menu --osd-key reload
    sleep 0.5
    echo "same_sets=$(tail -n +"$from" /tmp/pactl.log | grep -c "^set-")"
    # Killed outright, the OSD leaves no pactl subscribe behind either.
    stop_osd KILL
    ;;
esac
sed "s/^/pactl: /" /tmp/pactl.log 2>/dev/null
exit 0'
mk_run() {
    local mode="$1" outputs="$2" vol="$3" bri="$4" name="thinfloe-runtime-test-mk-$1-$$" rc
    MK_OUT=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS="$outputs" \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e TF_PACTL=/tmp/fake-pactl \
        -e TF_CONFIG_DIR=/tmp/cfg \
        -e PA_SUBSCRIBE_FAIL="${PA_FAIL:-0}" \
        -e TF_DDCUTIL=/tmp/fake-ddcutil \
        -e MK_MODE="$mode" \
        -e MK_VOL="$vol" \
        -e MK_BRI="$bri" \
        -e MK_PACTL="$mk_pactl" \
        -e MK_DDCUTIL="$mk_ddcutil" \
        -e MK_PX="$mk_px" \
        -e MK_SCRIPT="$mk_script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg /tmp/cfg/generated && chmod 700 /tmp/xdg &&
               printf "%s\n" "$MK_PACTL" >/tmp/fake-pactl && printf "%s\n" "$MK_DDCUTIL" >/tmp/fake-ddcutil &&
               chmod +x /tmp/fake-pactl /tmp/fake-ddcutil && /tmp/fake-pactl tf-init &&
               printf "%s\n" "$MK_PX" >/tmp/px.py && printf "%s\n" "$MK_SCRIPT" >/tmp/mk.sh &&
               printf "THINFLOE_VOLUME_KEYS=%s\nTHINFLOE_BRIGHTNESS_KEYS=%s\n" "$MK_VOL" "$MK_BRI" >/tmp/cfg/generated/environment &&
               thinfloe-compose-labwc-config /etc/thinfloe/labwc /tmp/cfg/generated/environment /nonexistent /tmp/xdg/thinfloe-labwc &&
               labwc -C /tmp/xdg/thinfloe-labwc -S "bash /tmp/mk.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during the media key test, $mode)"
        echo "$MK_OUT" | grep -E '^(wev|px|osd|socket|pactl|binds)'
        podman rm -f "$name" >/dev/null 2>&1
        exit 1
    fi
}
# mk_val KEY: the value of the last KEY=... line.
mk_val() {
    echo "$MK_OUT" | grep -E "^$1=" | tail -1 | cut -d= -f2-
}
# mk_px TAG NAME: the value px.py printed.
mk_px() {
    echo "$MK_OUT" | grep -E "^px $1 $2=" | tail -1 | cut -d= -f2
}
# mk_expect WHAT TAG NAME=VALUE...: one ok or FAIL line for all of them.
mk_expect() {
    local what="$1" tag="$2" bad="" p
    shift 2
    for p in "$@"; do
        [[ "$(mk_px "$tag" "${p%%=*}")" == "${p#*=}" ]] || bad+=" ${p%%=*}=$(mk_px "$tag" "${p%%=*}") (want ${p#*=})"
    done
    if [[ -z "$bad" ]]; then
        echo "ok   - $what"
    else
        echo "FAIL - $what:$bad"
        echo "$MK_OUT" | grep -E '^(px|osd|socket|osderr)' | head -40
        fail=1
    fi
}
mk_check() {
    if [[ "$1" == 0 ]]; then
        echo "ok   - $2"
    else
        echo "FAIL - $2: $3"
        fail=1
    fi
}

mk_run keys-off 1 off off
wev1=$(mk_val wev1)
[[ "$(mk_val wev_focus)" == active && " $wev1" == *" XF86AudioRaiseVolume "* ]]
mk_check $? "with the volume switch off, XF86AudioRaiseVolume reaches the focused window" \
    "wev focus '$(mk_val wev_focus)', syms '$wev1'"
! grep -q '^pactl: set-' <<<"$MK_OUT"
mk_check $? "with the volume switch off, the key does not change the volume" \
    "$(echo "$MK_OUT" | grep '^pactl: ' | tr '\n' ' ')"

mk_run keys-on 1 on off
wev1=$(mk_val wev1)
[[ "$(mk_val wev_focus)" == active && " $wev1" != *" XF86AudioRaiseVolume "* && " $wev1" == *" x "* ]]
mk_check $? "with the volume switch on, labwc takes XF86AudioRaiseVolume and the x after it reaches the window" \
    "wev focus '$(mk_val wev_focus)', syms '$wev1'"
grep -qx 'pactl: set-sink-volume spk_a 50%' <<<"$MK_OUT" && grep -qx 'pactl: set-sink-volume spk_b 50%' <<<"$MK_OUT"
mk_check $? "with the volume switch on and no OSD running, the key raises both speakers from 45 % to 50 %" \
    "$(echo "$MK_OUT" | grep '^pactl: ' | tr '\n' ' ')"

mk_run live 1 off off
wev1=$(mk_val wev1)
wev2=$(mk_val wev2)
[[ " $wev1" == *" XF86AudioRaiseVolume "* && " $wev2" != *" XF86AudioRaiseVolume "* && " $wev2" == *" y "* ]]
mk_check $? "the volume switch acts at once: after --env-only and labwc --reconfigure the key no longer reaches the window" \
    "before '$wev1', after '$wev2', bindings in rc.xml $(mk_val binds)"

mk_run osd 2 on off
[[ "$(mk_val socket)" == yes ]] && grep -qx 'pactl: set-sink-volume spk_a 50%' <<<"$MK_OUT" &&
    grep -qx 'pactl: set-sink-volume spk_b 50%' <<<"$MK_OUT"
mk_check $? "the volume key reaches the running OSD, which raises both speakers from 45 % to 50 %" \
    "socket $(mk_val socket); $(echo "$MK_OUT" | grep '^pactl: set' | tr '\n' ' ')"
mk_expect "0.5 s after the volume key the Volume panel shows on both outputs, its bar at 50 %" vol \
    panel1=0d1218 panel2=0d1218 bar1=7cc4ff bar2=7cc4ff gap1=7cc4ff tall1=0d1218
mk_expect "2.5 s after the volume key the panel is gone" volgone bar1=ff0000 panel1=ff0000
mk_expect "the microphone key 0.5 s after a volume key shows the Microphone off panel 15 px lower, without a bar, on both outputs" mic \
    in1=0d1218 top1=151b22 above1=ff0000 gap1=0d1218 in2=0d1218 top2=151b22 above2=ff0000 gap2=0d1218
mk_expect "the Microphone off panel draws its icon in WARN on both outputs" mic icon1=found icon2=found
grep -qx 'pactl: set-source-mute mic_a 1' <<<"$MK_OUT" && grep -qx 'pactl: set-source-mute mic_b 1' <<<"$MK_OUT"
mk_check $? "the microphone key mutes both microphones" \
    "$(grep '^pactl: set' <<<"$MK_OUT" | tr '\n' ' ')"
mk_expect "after the microphone key the mark shows on both outputs within 2 s" micmark m1=c79a4a m2=c79a4a
mk_expect "the mark goes when the default microphone is unmuted by another program" nomark m1=ff0000 m2=ff0000
mk_expect "the mark shows on both outputs within 2 s of the microphone being muted" mark m1=c79a4a m2=c79a4a
[[ "$(mk_val osd_alive)" == yes && "$(mk_px unplug m1)" == c79a4a ]]
mk_check $? "with HEADLESS-2 switched off the OSD keeps running and output 1 keeps the mark" \
    "OSD alive '$(mk_val osd_alive)', mark pixel '$(mk_px unplug m1)'"
mk_expect "the mark goes within 2 s of the microphone being on again" markgone m1=ff0000
hp=$(mk_val hotplug_ms)
[[ "$hp" =~ ^[0-9]+$ && $hp -le 1000 ]]
mk_check $? "a microphone plugged in while the microphone key holds them muted is muted within 1 s" \
    "after '$hp' ms; $(grep '^pactl: set-source' <<<"$MK_OUT" | tr '\n' ' ')"
! grep -q '^pactl: set-[a-z]*-[a-z]* spk_a\.monitor ' <<<"$MK_OUT"
mk_check $? "the monitor source is never set" "$(grep '^pactl: set-.*monitor' <<<"$MK_OUT" | tr '\n' ' ')"
[[ "$(mk_val osd_rc)" == 0 && "$(mk_val subscribe_left)" == no ]]
mk_check $? "the OSD ends with 0 on SIGTERM and leaves no pactl subscribe behind" \
    "osd_rc '$(mk_val osd_rc)', subscribe left '$(mk_val subscribe_left)'; $(echo "$MK_OUT" | grep '^osderr: ' | head -5 | tr '\n' ' ')"

mk_run liveoff 1 on off
lo=$(mk_val liveoff_ms)
[[ "$(mk_val keys)" == yes && "$lo" =~ ^[0-9]+$ && $lo -le 1000 ]]
mk_check $? "the volume switch turned off during a session puts every speaker and microphone at 100 % and unmuted within 1 s of the reload" \
    "keys '$(mk_val keys)', after '$lo' ms; $(tail -n +"$(mk_val reload_from)" <(grep '^pactl: ' <<<"$MK_OUT") | grep 'set-' | tr '\n' ' ')"
[[ "$(mk_val same_sets)" == 0 ]]
mk_check $? "a reload that leaves the switch as it is sets no device, so a microphone muted elsewhere stays muted" \
    "$(mk_val same_sets) set calls"
[[ "$(mk_val osd_rc)" == 137 && "$(mk_val subscribe_left)" == no ]]
mk_check $? "an OSD killed with SIGKILL leaves no pactl subscribe behind" \
    "osd_rc '$(mk_val osd_rc)', subscribe left '$(mk_val subscribe_left)'"

PA_FAIL=2 mk_run bright 2 off on
mk_expect "without DDC/CI the brightness key shows 'not supported' on both outputs" bright warn1=found warn2=found
n1=$(echo "$MK_OUT" | grep -c '^osderr: .*monitor HEADLESS-1 does not answer DDC/CI')
n2=$(echo "$MK_OUT" | grep -c '^osderr: .*monitor HEADLESS-2 does not answer DDC/CI')
[[ $n1 -eq 1 && $n2 -eq 1 ]]
mk_check $? "the OSD logs once per output that the monitor does not answer DDC/CI" \
    "HEADLESS-1 $n1 times, HEADLESS-2 $n2 times"
nsub=$(grep -cx 'pactl: subscribe' <<<"$MK_OUT")
nref=$(grep -c '^osderr: thinfloe-menu --osd: pactl subscribe: Connection failure: Connection refused$' <<<"$MK_OUT")
nr1=$(grep -cx 'osderr: thinfloe-menu --osd: pactl subscribe ended; retrying in 1 s' <<<"$MK_OUT")
nr2=$(grep -cx 'osderr: thinfloe-menu --osd: pactl subscribe ended; retrying in 2 s' <<<"$MK_OUT")
[[ $nsub -eq 3 && $nref -eq 1 && $nr1 -eq 1 && $nr2 -eq 1 ]]
mk_check $? "without a sound server pactl subscribe starts again after 1 s, then 2 s, and each message is logged once" \
    "subscribe started $nsub times (want 3); refused logged $nref, 1 s $nr1, 2 s $nr2 times; $(grep '^osderr: .*pactl' <<<"$MK_OUT" | tr '\n' ' ')"
[[ "$(mk_val resync)" == yes && "$(mk_val subscribe_left)" == no ]]
mk_check $? "once pactl subscribe runs again, a speaker that drifted is put back at the shared state" \
    "resync '$(mk_val resync)', subscribe left '$(mk_val subscribe_left)'"


# Deskflow edge capture over a fullscreen X11 window holding a keyboard grab,
# as ThinLinc does (the spec's Risk 1). 'thinfloe-menu --deskflow
# --probe-capture HEADLESS-1 right-of' puts a 1 px overlay strip on the right
# edge of HEADLESS-1, prints one line per capture event and starts the
# capture at once on 'enter': first the pointer lock, then exclusive keyboard
# interactivity, since labwc activates a lock on a layer surface only when
# the keyboard focus moves to it. Escape ends the capture. wtype starts first
# (Xwayland installs its grab hook only when the seat has a keyboard) and
# types x, Escape, y through one virtual keyboard, so they are its key codes
# 1, 2, 3 (X 9, 10, 11; --keymap names them for the X window); the probe
# names a key from the keymap it gets. x must reach the probe and not the X
# window, y the X window after Escape. TF_RUNTIME_MENU
# names a host-built thinfloe-menu to use instead of the image's (iteration
# only; the evidence is a run without it).
# shellcheck disable=SC2016 # expands inside the container
cp_script='wtype -s 9000 x -s 1500 -k Escape -s 1500 y &
sleep 0.5
python3 /usr/local/bin/x11-probe.py --grab --fullscreen --keymap x,Escape,y --keys 16 >/tmp/xkeys 2>&1 &
xprobe=$!
sleep 5
wlrctl toplevel find app_id:thinfloe-probe state:active && echo x_before=active || echo x_before=inactive
thinfloe-menu --deskflow --probe-capture HEADLESS-1 right-of >/tmp/cap.out 2>/tmp/cap.err &
cap=$!
sleep 1.5
wlrctl pointer move -5000 -5000
wlrctl pointer move 5000 360
sleep 6.5
wlrctl toplevel find app_id:thinfloe-probe state:active && echo x_after=active || echo x_after=inactive
kill $cap
wait $cap
echo cap_rc=$?
wait $xprobe
sed "s/^/cap: /" /tmp/cap.out
sed "s/^/caperr: /" /tmp/cap.err | head -40
sed "s/^/x11: /" /tmp/xkeys
wait
exit 0'
run_capture_probe_case() {
    local name="thinfloe-runtime-test-cap-$$" rc out mount=()
    [[ -n "${TF_RUNTIME_MENU:-}" ]] && mount=(-v "$TF_RUNTIME_MENU:/usr/bin/thinfloe-menu:ro")
    out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        "${mount[@]}" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=1 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e CP_SCRIPT="$cp_script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               printf "%s\n" "$CP_SCRIPT" >/tmp/cp.sh &&
               WAYLAND_DEBUG=server labwc -C /etc/thinfloe/labwc -S "sh /tmp/cp.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during the capture probe)"
        grep -E '^(cap|caperr|x11): |^x_|^cap_rc=' <<<"$out"
        podman rm -f "$name" >/dev/null 2>&1
        fail=1
        return
    fi
    local cap x11 diag
    cap=$(grep '^cap: ' <<<"$out" | cut -c6-)
    x11=$(grep '^x11: ' <<<"$out" | cut -c6- | tr '\n' ' ')
    diag="probe: $(tr '\n' '|' <<<"$cap") stderr: $(grep '^caperr: ' <<<"$out" | cut -c9- | tr '\n' '|') X window: '$x11' $(grep -E '^(x_before|x_after|cap_rc)=' <<<"$out" | tr '\n' ' ')"
    if ! grep -Eq 'zwp_xwayland_keyboard_grab_manager_v1[@#][0-9]+\.grab_keyboard\(' <<<"$out"; then
        echo "FAIL - the X window took no keyboard grab, so the capture case proves nothing: $diag"
        fail=1
        return
    fi
    # enter, then locked, then the x key, in that order.
    if awk '/^enter / && !e {e=NR} /^locked$/ && e && !l {l=NR} / down x$/ && l && !k {k=NR} END {exit !(e && l && k)}' <<<"$cap"; then
        echo "ok   - over a fullscreen X11 window with a keyboard grab the strip gets the pointer lock, then the keys"
    else
        echo "FAIL - no enter, locked, x key in that order at the capture strip: $diag"
        fail=1
    fi
    if [[ " $x11" == *" KEY x "* ]]; then
        echo "FAIL - the x typed while captured reached the X window: $diag"
        fail=1
    else
        echo "ok   - the x typed while captured did not reach the X window"
    fi
    if awk '/ down Escape$/ && !s {s=NR} /^unlocked$/ && s {u=1} END {exit !u}' <<<"$cap" &&
        [[ " $x11" == *" KEY y "* ]] && grep -qx 'x_before=active' <<<"$out" && grep -qx 'x_after=active' <<<"$out"; then
        echo "ok   - Escape ends the capture: the probe reports unlocked, y reaches the X window and it is active again"
    else
        echo "FAIL - after Escape: $diag"
        fail=1
    fi
    if grep -qx 'cap_rc=0' <<<"$out"; then
        echo "ok   - the capture probe ends with 0 on SIGTERM"
    else
        echo "FAIL - the capture probe did not end with 0 on SIGTERM: $diag"
        fail=1
    fi
}
run_capture_probe_case


# Deskflow interop: the stock Deskflow client (deskflow-core 1.26.0 of the
# image's universe) against 'thinfloe-menu --deskflow', in one labwc.
# deskflow-core has no flags for the connection: 'deskflow-core client
# --new-instance -s <dir>/Deskflow.conf' takes every setting from that INI
# file (client/remoteHost, core/port, core/computerName,
# security/tlsEnabled, log/level DEBUG2), logs to stdout, and
# --new-instance lets a
# second client run beside the first. Settings::tlsDir() is the directory of
# that file plus /tls: the client's certificate and key are
# <dir>/tls/deskflow.pem (deskflow-core does not make one, the GUI does), the
# servers it trusts <dir>/tls/trusted-servers, one 'v2:sha256:<hex>' per
# line. Without XDG_SESSION_TYPE=wayland the client drives X11, here
# labwc's Xwayland, whose root window is the 1280x720 output.
# The case: ThinFloe's fingerprint from the state file goes into the trust
# file; the client appears as pending; an attach in generated/deskflow and
# 'reload' on the command socket let its next try in (connected, with its
# screen size); a second 'laptop' with another certificate stays out
# (fp-changed) while the first stays connected; 'listen = false' and a
# reload end the daemon with 0 and nothing listens on 24800 any more.
# The client's console shows only its NOTE and IPC lines, whatever
# log/level says.
# shellcheck disable=SC2016 # expands inside the container
df_script='export TF_CONFIG_DIR=/tmp/tfcfg TF_DESKFLOW_STATE=/tmp/xdg/df.state TF_DESKFLOW_SOCKET=/tmp/xdg/df.sock
gen=$TF_CONFIG_DIR/generated/deskflow
mkdir -p $TF_CONFIG_DIR/generated
printf "[deskflow]\nlisten = true\nport = 24800\n" >$gen
upto() { n=$(($1 * 5)); shift; while [ $n -gt 0 ]; do "$@" && return 0; sleep 0.2; n=$((n - 1)); done; return 1; }
st() { cat $TF_DESKFLOW_STATE 2>/dev/null; }
line() { st | awk -F "\t" -v n="$1" -v f="$2" -v s="$3" "\$1 == n && \$2 == f && \$3 == s" | grep -q .; }
reload() { python3 -c "import socket, sys; socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM).sendto(b\"reload\n\", sys.argv[1])" $TF_DESKFLOW_SOCKET; }
client() {
    d=$1; mkdir -p $d/tls
    printf "[client]\nremoteHost=127.0.0.1\n[core]\nport=24800\ncomputerName=laptop\n[security]\ntlsEnabled=true\n[log]\nlevel=DEBUG2\n" >$d/Deskflow.conf
    openssl req -x509 -newkey rsa:2048 -nodes -keyout $d/key.pem -out $d/cert.pem -days 30 -subj /CN=laptop 2>/dev/null
    cat $d/cert.pem $d/key.pem >$d/tls/deskflow.pem
    echo "$fp" >$d/tls/trusted-servers
    echo "v2:sha256:$(openssl x509 -in $d/cert.pem -outform DER | sha256sum | cut -d" " -f1)" >$d/fp
    env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 QT_QPA_PLATFORM=offscreen \
        timeout 60 deskflow-core client --new-instance -s $d/Deskflow.conf >$d/log 2>&1 &
}
thinfloe-menu --deskflow >/tmp/d.out 2>/tmp/d.err &
dpid=$!
upto 15 sh -c "head -1 $TF_DESKFLOW_STATE 2>/dev/null | grep -q \"^self\"" && echo head=yes || echo head=no
fp=$(st | head -1 | cut -f2)
echo "fp=$fp"
client /tmp/c1
c1=$(cat /tmp/c1/fp)
upto 15 line laptop "$c1" pending && echo pending=yes || echo pending=no
printf "\n[[deskflow.device]]\nname = \"laptop\"\nfingerprint = \"%s\"\noutput = \"HEADLESS-1\"\nside = \"right-of\"\n" "$c1" >>$gen
reload
upto 15 line laptop "$c1" connected && echo connected=yes || echo connected=no
upto 5 grep -q "connected to server" /tmp/c1/log && echo c1_log=yes || echo c1_log=no
client /tmp/c2
c2=$(cat /tmp/c2/fp)
upto 15 line laptop "$c2" pending && echo pending2=yes || echo pending2=no
line laptop "$c1" connected && echo still=yes || echo still=no
st | sed "s/^/state: /"
printf "[deskflow]\nlisten = false\nport = 24800\n" >$gen
reload
upto 15 sh -c "! kill -0 $dpid 2>/dev/null"
wait $dpid
echo d_rc=$?
ss -Hltn "sport = :24800" | grep -q . && echo listening=yes || echo listening=no
sed "s/^/c1: /" /tmp/c1/log | tail -60
sed "s/^/c2: /" /tmp/c2/log | tail -30
sed "s/^/derr: /" /tmp/d.err | head -40
exit 0'
run_deskflow_pairing_case() {
    local name="thinfloe-runtime-test-df-$$" rc out mount=()
    [[ -n "${TF_RUNTIME_MENU:-}" ]] && mount=(-v "$TF_RUNTIME_MENU:/usr/bin/thinfloe-menu:ro")
    out=$(timeout --kill-after=10 "$RUNTIME_TIMEOUT" podman run --rm --log-driver=none --name "$name" \
        "${mount[@]}" \
        -e WLR_BACKENDS=headless \
        -e WLR_HEADLESS_OUTPUTS=1 \
        -e WLR_RENDERER=pixman \
        -e XDG_RUNTIME_DIR=/tmp/xdg \
        -e DF_SCRIPT="$df_script" \
        "$IMAGE" \
        sh -c 'mkdir -p /tmp/xdg && chmod 700 /tmp/xdg &&
               printf "%s\n" "$DF_SCRIPT" >/tmp/df.sh &&
               labwc -C /etc/thinfloe/labwc -S "sh /tmp/df.sh"' 2>&1)
    rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        echo "FAIL - podman run timed out after ${RUNTIME_TIMEOUT}s (labwc hang during the Deskflow case)"
        grep -E '^(state|c1|c2|derr): |^[a-z0-9_]+=' <<<"$out"
        podman rm -f "$name" >/dev/null 2>&1
        fail=1
        return
    fi
    local diag
    diag="$(grep -E '^(state|derr): |^[a-z0-9_]+=' <<<"$out" | tr '\n' '|') client 1: $(grep '^c1: ' <<<"$out" | cut -c5- | tail -15 | tr '\n' '|')"
    df_ok() { grep -qx "$1" <<<"$out"; }
    df_check() {
        if [[ $1 -eq 0 ]]; then
            echo "ok   - $2"
        else
            echo "FAIL - $2: $diag"
            fail=1
        fi
    }
    df_ok head=yes && grep -Eq '^fp=v2:sha256:[0-9a-f]{64}$' <<<"$out"
    df_check $? "the Deskflow daemon writes its fingerprint to the state file"
    # Deskflow 1.26 logs only its NOTE and IPC lines here, so the refusal
    # shows as a retry without "disconnected from server": a dropped
    # connection logs that line, a refusal (EUNK) does not.
    df_ok pending=yes && awk '/^c1: .*IPC: peer fingerprint/ && !p {p=NR; next} p && /IPC: disconnected from server/ && !c {d=1}
        p && /IPC: connecting to/ {c=1} END {exit !(p && c && !d)}' <<<"$out"
    df_check $? "a Deskflow client that trusts ThinFloe is refused with EUNK, tries again, and shows as pending with the fingerprint of its certificate"
    df_ok connected=yes && grep -Eq $'^state: laptop\tv2:sha256:[0-9a-f]{64}\tconnected\t[0-9]+\t1280\t720\t' <<<"$out"
    df_check $? "after the attach and a reload the client's next try connects, with its 1280x720 screen"
    df_ok c1_log=yes
    df_check $? "the Deskflow client logs that it connected to the server"
    df_ok pending2=yes && df_ok still=yes && grep -Eq $'^state: laptop\tv2:sha256:[0-9a-f]{64}\tpending\t[0-9]+\t0\t0\tfp-changed$' <<<"$out"
    df_check $? "a second 'laptop' with another certificate stays out as pending, fp-changed, and the first stays connected"
    df_ok d_rc=0 && df_ok listening=no
    df_check $? "'listen = false' and a reload end the daemon with 0 and nothing listens on 24800"
}
run_deskflow_pairing_case

exit $fail
