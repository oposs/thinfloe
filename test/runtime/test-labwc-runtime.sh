#!/bin/bash
# Runs labwc headless with two 1280x720 outputs and asserts that a fullscreen
# X11 client is given the union of both (2560x720), not a single output.
# Also asserts that Xwayland binds the keyboard-grab manager, and that
# labwc's environment-file parser lets a later duplicate assignment win
# (the assumption thinfloe-compose-labwc-config's merge depends on).
# Finally asserts that thinfloe-menu, a native Wayland client, maps at the full
# output size, rasterises on the CPU and keeps running.
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
expected_tiles=$(podman run --rm --log-driver=none "$IMAGE" sh -c \
    'n=0; for f in /etc/thinfloe/menu/*.desktop; do grep -q "^X-TF-Source-" "$f" || n=$((n+1)); done; echo $n')
icons_line=$(echo "$menu_out" | grep -o 'thinfloe-menu: [0-9]* of [0-9]* tile icons drawn' | tail -1)
icons_drawn=$(echo "$icons_line" | cut -d' ' -f2)
icons_total=$(echo "$icons_line" | cut -d' ' -f4)
if [[ -n "$icons_total" && "$icons_total" -gt 0 && "$icons_total" == "$expected_tiles" && "$icons_drawn" == "$icons_total" ]]; then
    echo "ok   - thinfloe-menu drew every shipped tile icon ($icons_drawn of $icons_total, add-ons hidden)"
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

exit $fail
