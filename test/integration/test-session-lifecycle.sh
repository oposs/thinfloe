#!/bin/bash
# Offline: the exit-status contract between thinfloe-menu and thinfloe-session,
# and the restart limit in session-verdict. No image, no compositor; the
# menu and foot are stubs, and a save stub records any call: since
# 2026-10-02 the menu saves before it exits, so the session must never save.
#
# Usage: test/integration/test-session-lifecycle.sh

set -uo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SESSION="$ROOT/overlay/usr/bin/thinfloe-session"
VERDICT="$ROOT/overlay/usr/libexec/thinfloe/session-verdict"

WORK=$(mktemp -d)
# Kill whatever a case left running (an idle helper, a swayidle stub, the
# sleep a helper runs for timeout 0) before the work directory goes.
cleanup() {
    local f p
    for f in "$WORK"/idle.* "$WORK"/*.pid "$WORK"/idle-kids "$WORK"/idle-helper; do
        [[ -e $f ]] || continue
        for p in $(cat "$f" 2>/dev/null) ${f##*/idle.}; do
            [[ $p =~ ^[0-9]+$ ]] && kill "$p" 2>/dev/null
        done
    done
    rm -rf "$WORK"
}
trap cleanup EXIT
fail=0
ok()   { echo "ok   - $1"; }
bad()  { echo "FAIL - $1"; fail=1; }

mkdir -p "$WORK/bin" "$WORK/home"
# foot records its arguments instead of opening a window.
cat >"$WORK/bin/foot" <<STUB
#!/bin/sh
echo "\$@" >"$WORK/foot.args"
STUB
# A save stub named like the real one, first on PATH, and also reachable
# through TF_SAVE_HOME: a call by name or through the variable is
# recorded. (thinfloe-session reads neither; the stub exists so the "never
# saves" assertions are able to fail if someone adds a call.)
cat >"$WORK/bin/thinfloe-save-home" <<STUB
#!/bin/sh
echo called >>"$WORK/saved"
STUB
# sudo records what it was asked to run and fails when the test says so.
# The boot screen helper is run, not recorded: it has a recorder of its own.
cat >"$WORK/bin/sudo" <<STUB
#!/bin/sh
if [ "\$1" = -n ] && [ "\$2" = "$WORK/bin/step" ]; then shift; exec "\$@"; fi
echo "\$@" >>"$WORK/sudo.args"
[ ! -e "$WORK/sudo.fail" ]
STUB
# The boot screen helper (TF_PLYMOUTH_STEP) records each call.
cat >"$WORK/bin/step" <<STUB
#!/bin/sh
echo "\$*" >>"$WORK/steps"
STUB
chmod 755 "$WORK/bin/foot" "$WORK/bin/thinfloe-save-home" "$WORK/bin/sudo" "$WORK/bin/step"
# The session also starts these helpers when they exist; on a dev host they
# might, so stub them rather than launch the real ones. swayidle leaves its
# arguments in idle.<its pid> (complete once the name appears) and stays up
# like the real one.
printf '#!/bin/sh\nexit 0\n' >"$WORK/bin/wlopm"
cat >"$WORK/bin/swayidle" <<STUB
#!/bin/sh
echo "\$*" >"$WORK/.idle.\$\$"
mv "$WORK/.idle.\$\$" "$WORK/idle.\$\$"
exec sleep 30
STUB
# For menu stubs: wait until a swayidle stub has started, at most 2 s.
cat >"$WORK/bin/wait-idle" <<STUB
#!/bin/sh
n=0
while [ \$n -lt 20 ]; do
    for f in "$WORK"/idle.*; do [ -e "\$f" ] && exit 0; done
    sleep 0.1
    n=\$((n + 1))
done
exit 1
STUB
chmod 755 "$WORK/bin/wlopm" "$WORK/bin/swayidle" "$WORK/bin/wait-idle"

# Run thinfloe-session with a menu stub that runs $1 as shell code.
run_session() {
    rm -f "$WORK/saved" "$WORK/foot.args" "$WORK/status" "$WORK/thinfloe-session.status" "$WORK/sudo.args" "$WORK/steps"
    printf '#!/bin/sh\n%s\n' "$1" >"$WORK/bin/menu"
    chmod 755 "$WORK/bin/menu"
    env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" \
        XDG_RUNTIME_DIR="$WORK" \
        TF_MENU="$WORK/bin/menu" TF_SAVE_HOME="$WORK/bin/thinfloe-save-home" \
        TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" TF_LABWC_SCALE=/bin/true \
        sh "$SESSION" >/dev/null 2>&1
}

# The session ends Plymouth, leaving its picture, before the menu starts.
run_session "echo menu >>'$WORK/steps'; exit 0"
if [[ "$(cat "$WORK/steps" 2>/dev/null)" == $'quit-retain\nmenu' ]]; then
    ok "session: Plymouth quits, picture retained, before the menu starts"
else
    bad "session: boot screen calls were '$(cat "$WORK/steps" 2>/dev/null)'"
fi

# The title bar scale helper starts before the menu, after a stale scale file
# of an earlier session is gone, and ends with the session.
cat >"$WORK/bin/scale-helper" <<STUB
#!/bin/sh
if [ -e "$WORK/stale-scale" ]; then s=stale; else s=gone; fi
echo "started \$s" >"$WORK/helper.out"
echo \$\$ >"$WORK/helper.pid"
exec sleep 30
STUB
chmod 755 "$WORK/bin/scale-helper"
rm -f "$WORK/helper.out" "$WORK/helper.pid"
echo 2 >"$WORK/stale-scale"
printf '#!/bin/sh\nsleep 0.5\nexit 0\n' >"$WORK/bin/menu"
chmod 755 "$WORK/bin/menu"
env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" XDG_RUNTIME_DIR="$WORK" \
    TF_MENU="$WORK/bin/menu" TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" \
    TF_SCALE_FILE="$WORK/stale-scale" TF_LABWC_SCALE="$WORK/bin/scale-helper" \
    sh "$SESSION" >/dev/null 2>&1
if [[ ! -e "$WORK/stale-scale" ]]; then
    ok "session: a stale scale file is removed"
else
    bad "session: stale scale file still there"
fi
if [[ "$(head -n1 "$WORK/helper.out" 2>/dev/null)" == "started gone" ]]; then
    ok "session: the scale helper started after the stale file was removed"
else
    bad "session: scale helper output '$(cat "$WORK/helper.out" 2>/dev/null)'"
fi
hp=$(cat "$WORK/helper.pid" 2>/dev/null)
sleep 0.3
if [[ -n "$hp" ]] && ! kill -0 "$hp" 2>/dev/null; then
    ok "session: the scale helper ends with the session"
else
    bad "session: scale helper $hp still running"
    [[ -n "$hp" ]] && kill "$hp" 2>/dev/null
fi

# No usable scale file path (TF_SCALE_FILE and XDG_RUNTIME_DIR unset or
# empty, as the menu reads them): the session removes nothing.
printf '#!/bin/sh\necho "$*" >>"%s/rm.args"\nexec /bin/rm "$@"\n' "$WORK" >"$WORK/bin/rm"
chmod 755 "$WORK/bin/rm"
printf '#!/bin/sh\nexit 0\n' >"$WORK/bin/menu"
chmod 755 "$WORK/bin/menu"
for empty in unset empty; do
    rm -f "$WORK/rm.args"
    if [[ $empty == unset ]]; then
        env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" TF_SESSION_STATUS="$WORK/status.file" \
            TF_MENU="$WORK/bin/menu" TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" \
            TF_LABWC_SCALE=/bin/true sh "$SESSION" >/dev/null 2>&1
    else
        env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" TF_SESSION_STATUS="$WORK/status.file" \
            XDG_RUNTIME_DIR= TF_SCALE_FILE= \
            TF_MENU="$WORK/bin/menu" TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" \
            TF_LABWC_SCALE=/bin/true sh "$SESSION" >/dev/null 2>&1
    fi
    if ! grep -q thinfloe-scale "$WORK/rm.args" 2>/dev/null; then
        ok "session: no scale file path ($empty), nothing removed"
    else
        bad "session: removed '$(cat "$WORK/rm.args")' with no scale file path ($empty)"
    fi
done
rm -f "$WORK/bin/rm"

run_session 'exit 0'
rc=$?
[[ $rc -eq 0 ]] && ok "logout: session exits 0" || bad "logout: session exited $rc"
[[ ! -e "$WORK/saved" ]] && ok "logout: the session does not save again" || bad "logout: the session saved after the menu"
[[ "$(cat "$WORK/thinfloe-session.status" 2>/dev/null)" == 0 ]] && ok "logout: status 0 recorded" || bad "logout: status file wrong"
[[ ! -e "$WORK/foot.args" ]] && ok "logout: no failure window" || bad "logout: failure window shown"

# 10 and 11: the menu has saved, as for 0; the session restarts or powers
# off the machine. There is no polkit, so it needs sudo.
for pair in 10:reboot 11:poweroff; do
    code=${pair%%:*} verb=${pair#*:}
    rm -f "$WORK/sudo.fail"
    run_session "exit $code"
    rc=$?
    [[ $rc -eq 0 ]] && ok "menu exit $code: session exits 0" || bad "menu exit $code: session exited $rc"
    [[ "$(cat "$WORK/sudo.args" 2>/dev/null)" == "systemctl $verb" ]] && ok "menu exit $code: sudo systemctl $verb" \
        || bad "menu exit $code: sudo got '$(cat "$WORK/sudo.args" 2>/dev/null)'"
    [[ ! -e "$WORK/saved" ]] && ok "menu exit $code: the session does not save" || bad "menu exit $code: the session saved"
    [[ ! -e "$WORK/foot.args" ]] && ok "menu exit $code: no window" || bad "menu exit $code: window shown"
    : >"$WORK/sudo.fail"
    run_session "exit $code"
    rc=$?
    [[ $rc -eq $code ]] && grep -q "$verb failed" "$WORK/foot.args" 2>/dev/null \
        && ok "menu exit $code: a failed systemctl $verb is shown" \
        || bad "menu exit $code: failed systemctl: rc=$rc window '$(cat "$WORK/foot.args" 2>/dev/null)'"
    rm -f "$WORK/sudo.fail"
done

for code in 1 101 127; do
    run_session "exit $code"
    rc=$?
    [[ $rc -eq $code ]] && ok "menu exit $code: session exits $code" || bad "menu exit $code: session exited $rc"
    [[ ! -e "$WORK/saved" ]] && ok "menu exit $code: home NOT saved" || bad "menu exit $code: home was saved"
    [[ "$(cat "$WORK/thinfloe-session.status" 2>/dev/null)" == "$code" ]] && ok "menu exit $code: status recorded" \
        || bad "menu exit $code: status file wrong"
    grep -q -- "-- sh -c" "$WORK/foot.args" 2>/dev/null && grep -q " $code " "$WORK/foot.args" \
        && ok "menu exit $code: failure window names the status" || bad "menu exit $code: no failure window"
done

# The failure window must claim only what it knows: the menu may have saved
# before it failed, and the third failure in a row ends at the console
# failure screen, not in a new session.
if grep -q -e 'NOT saved' -e 'start a new session' "$WORK/foot.args" 2>/dev/null; then
    bad "failure window claims the home was not saved or that a new session follows"
elif grep -q 'did not save your home directory' "$WORK/foot.args" 2>/dev/null; then
    ok "failure window says only that this session did not save"
else
    bad "failure window does not mention the save"
fi

# The stub does not catch an absolute /usr/bin/thinfloe-save-home, so check
# the code as well.
if grep -v '^[[:space:]]*#' "$SESSION" | grep -q 'thinfloe-save-home'; then
    bad "thinfloe-session still calls thinfloe-save-home; the menu saves before it exits"
else
    ok "thinfloe-session never calls thinfloe-save-home"
fi

# Killed by a signal: the shell reports 128+N. Still a failure, still no save.
run_session 'kill -9 $$'
rc=$?
[[ $rc -eq 137 && ! -e "$WORK/saved" ]] && ok "menu killed: no save, status 137" \
    || bad "menu killed: rc=$rc saved=$([[ -e $WORK/saved ]] && echo yes || echo no)"

# --- idle timeout ------------------------------------------------------
# The session runs swayidle in a helper loop and tells the menu its PID
# (TF_IDLE_PID) and whether ~/.thinfloe-sessionrc sets the timeout
# (TF_IDLE_TIMEOUT_RC). The timeout comes from TF_IDLE_TIMEOUT, then the
# pre-4.0 DBRRG_IDLE_TIMEOUT, then idle_timeout= in ~/.config/thinfloe/screen,
# then 300.
SCREEN="$WORK/home/.config/thinfloe/screen"
# The PIDs among $@ still running after up to 2 s.
still_alive() {
    local n p left
    for n in $(seq 20); do
        left=""
        for p in "$@"; do kill -0 "$p" 2>/dev/null && left="$left $p"; done
        [[ -z $left ]] && break
        sleep 0.1
    done
    echo $left
}
mkdir -p "${SCREEN%/*}"
# The menu stub's default: wait for swayidle, record its arguments and the
# environment the menu got.
SEEN_MENU="wait-idle; { cat '$WORK'/idle.*; echo \"pid=\$TF_IDLE_PID\"; echo \"rc=\${TF_IDLE_TIMEOUT_RC:-}\"; } >'$WORK/seen'"
idle_run() { # sessionrc-content screen-content|- [menu-code]
    rm -f "$WORK"/idle.* "$WORK/seen" "$WORK/idle-kids"
    printf '%s\n' "$1" >"$WORK/home/.thinfloe-sessionrc"
    if [[ $2 == - ]]; then rm -f "$SCREEN"; else printf '%s\n' "$2" >"$SCREEN"; fi
    run_session "${3:-$SEEN_MENU}"
    cat "$WORK/seen" 2>/dev/null
}
case "$(idle_run '' -)" in
    *"timeout 300 "*"pid="[0-9]*"rc=") ok "idle: default timeout is 300, PID given, no rc flag" ;;
    *) bad "idle: default: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
case "$(idle_run '' 'idle_timeout=600')" in
    *"timeout 600 "*"rc=") ok "idle: the menu file sets the timeout" ;;
    *) bad "idle: menu file: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
case "$(idle_run '' '# written by thinfloe-menu
idle_timeout=60
idle_timeout=0600')" in
    *"timeout 600 "*) ok "idle: the last line counts, leading zeros are decimal" ;;
    *) bad "idle: last line: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
for g in 'idle_timeout=5m' 'idle_timeout=86401' 'idle_timeout= 60' 'idle_timeout=-5' 'idle_timeout=' \
         'idle_timeout=99999999999999999999' ' idle_timeout=600' 'idle_timeout=600
idle_timeout=x'; do
    case "$(idle_run '' "$g")" in
        *"timeout 300 "*) ok "idle: garbage in the file means 300 ('${g//$'\n'/\\n}')" ;;
        *) bad "idle: garbage '$g': '$(cat "$WORK/seen" 2>/dev/null)'" ;;
    esac
done
case "$(idle_run '' 'idle_timeout=86400')" in
    *"timeout 86400 "*) ok "idle: 86400 is the largest value" ;;
    *) bad "idle: 86400: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
for name in TF_IDLE_TIMEOUT DBRRG_IDLE_TIMEOUT; do
    case "$(idle_run "$name=900" 'idle_timeout=600')" in
        *"timeout 900 "*"rc=1") ok "idle: sessionrc wins ($name)" ;;
        *) bad "idle: sessionrc $name: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
    esac
done
case "$(idle_run 'TF_IDLE_TIMEOUT=600
DBRRG_IDLE_TIMEOUT=900' -)" in
    *"timeout 600 "*"rc=1") ok "idle: TF_IDLE_TIMEOUT wins over the old name" ;;
    *) bad "idle: new name did not win: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
# A sessionrc value that is no positive number disables blanking, as before.
for v in 0 garbage; do
    idle_run "TF_IDLE_TIMEOUT=$v" 'idle_timeout=600' \
        "sleep 1; echo \"pid=\$TF_IDLE_PID rc=\${TF_IDLE_TIMEOUT_RC:-}\" >'$WORK/seen'" >/dev/null
    if ! ls "$WORK"/idle.* >/dev/null 2>&1 && grep -q '^pid=[0-9][0-9]* rc=1$' "$WORK/seen" 2>/dev/null; then
        ok "idle: sessionrc TF_IDLE_TIMEOUT=$v starts no swayidle"
    else
        bad "idle: sessionrc TF_IDLE_TIMEOUT=$v: '$(cat "$WORK/seen" 2>/dev/null)' $(ls "$WORK"/idle.* 2>/dev/null)"
    fi
done
# Zero: no swayidle, the helper still runs (and its sleep must end with it).
idle_run '' 'idle_timeout=0' \
    "sleep 1; echo \"pid=\$TF_IDLE_PID\" >'$WORK/seen'; pgrep -P \"\$TF_IDLE_PID\" >'$WORK/idle-kids'" >/dev/null
zero_kids=$(cat "$WORK/idle-kids" 2>/dev/null)
if ! ls "$WORK"/idle.* >/dev/null 2>&1 && grep -q '^pid=[0-9][0-9]*$' "$WORK/seen" 2>/dev/null; then
    ok "idle: zero starts no swayidle, TF_IDLE_PID still set"
else
    bad "idle: zero: '$(cat "$WORK/seen" 2>/dev/null)' $(ls "$WORK"/idle.* 2>/dev/null)"
fi
alive=$(still_alive $zero_kids)
if [[ -n $zero_kids && -z $alive ]]; then
    ok "idle: the helper's sleep for zero ends with the session"
else
    bad "idle: zero: children '$zero_kids', alive '$alive'"
fi
# The menu changes the file twice and signals each time: one swayidle is
# left, with the last value.
idle_run '' - "wait-idle
printf 'idle_timeout=120\n' >'$SCREEN'; kill -USR1 \$TF_IDLE_PID
printf 'idle_timeout=1800\n' >'$SCREEN'; kill -USR1 \$TF_IDLE_PID
sleep 1
for f in '$WORK'/idle.*; do kill -0 \"\${f##*.}\" 2>/dev/null && cat \"\$f\"; done >'$WORK/seen'
echo \"\$TF_IDLE_PID\" >'$WORK/idle-helper'" >/dev/null
if [[ $(wc -l <"$WORK/seen" 2>/dev/null) -eq 1 ]] && grep -q 'timeout 1800 ' "$WORK/seen"; then
    ok "idle: two signals leave one swayidle, with the new timeout"
else
    bad "idle: after two signals, live swayidle: '$(cat "$WORK/seen" 2>/dev/null)'"
fi
# Everything the helper started ends with the session.
hp=$(cat "$WORK/idle-helper" 2>/dev/null)
idle_pids=""
for f in "$WORK"/idle.*; do [[ -e $f ]] && idle_pids="$idle_pids ${f##*.}"; done
alive=$(still_alive $hp $idle_pids)
if [[ -n $hp && -z $alive ]]; then
    ok "idle: idle helper and swayidle end with the session"
else
    bad "idle: still running after the session: '$alive' (helper '$hp', swayidle '$idle_pids')"
fi
# No swayidle or no wlopm: no helper, no TF_IDLE_PID.
mv "$WORK/bin/wlopm" "$WORK/wlopm.off"
idle_run '' - "sleep 0.3; echo \"pid=\${TF_IDLE_PID:-}\" >'$WORK/seen'" >/dev/null
if [[ "$(cat "$WORK/seen" 2>/dev/null)" == "pid=" ]] && ! ls "$WORK"/idle.* >/dev/null 2>&1; then
    ok "idle: without wlopm no helper and no TF_IDLE_PID"
else
    bad "idle: without wlopm: '$(cat "$WORK/seen" 2>/dev/null)'"
fi
mv "$WORK/wlopm.off" "$WORK/bin/wlopm"
rm -f "$WORK/home/.thinfloe-sessionrc" "$SCREEN"

# --- session-verdict ---------------------------------------------------
S="$WORK/vstatus"
C="$WORK/vcount"
rm -f "$S" "$C"
"$VERDICT" "$S" "$C" >/dev/null; rc=$?
[[ $rc -eq 0 ]] && ok "verdict: no status file restarts" || bad "verdict: no status file gave $rc"
echo 0 >"$S"; echo 2 >"$C"
"$VERDICT" "$S" "$C" >/dev/null; rc=$?
[[ $rc -eq 0 && ! -e "$C" ]] && ok "verdict: clean logout resets the count" || bad "verdict: clean logout rc=$rc"
for code in 10 11; do
    echo "$code" >"$S"; echo 2 >"$C"
    "$VERDICT" "$S" "$C" >/dev/null; rc=$?
    [[ $rc -eq 0 && ! -e "$C" ]] && ok "verdict: $code is a clean end" || bad "verdict: $code rc=$rc"
done
echo 101 >"$S"; rm -f "$C"
"$VERDICT" "$S" "$C" >/dev/null; r1=$?
"$VERDICT" "$S" "$C" >/dev/null; r2=$?
out=$("$VERDICT" "$S" "$C"); r3=$?
[[ $r1 -eq 0 && $r2 -eq 0 && $r3 -eq 1 && "$out" == 101 ]] \
    && ok "verdict: third consecutive failure stops the restarts and names 101" \
    || bad "verdict: got $r1 $r2 $r3 '$out'"
[[ ! -e "$C" ]] && ok "verdict: count cleared after stopping" || bad "verdict: count left behind"
echo garbage >"$C"
"$VERDICT" "$S" "$C" >/dev/null; rc=$?
[[ $rc -eq 0 && "$(cat "$C")" == 1 ]] && ok "verdict: a corrupt count starts again at 1" || bad "verdict: corrupt count rc=$rc"

# --- 10-thinfloe-session.sh stops restarting after repeated failures ----
PROFILE="$ROOT/overlay/etc/profile.d/10-thinfloe-session.sh"
mkdir -p "$WORK/ps/bin" "$WORK/ps/home" "$WORK/ps/run"
printf '#!/bin/sh\necho /dev/tty1\n' >"$WORK/ps/bin/tty"
# labwc runs the session, which records the menu's status, and exits 0 as
# the real one always does; STUB_LABWC_RC makes it a labwc that could not
# start. Each run is logged with the boot screen calls.
cat >"$WORK/ps/bin/labwc" <<'STUB'
#!/bin/sh
echo "(labwc)" >>"$PS_STEPS"
[ "${STUB_LABWC_RC:-0}" -ne 0 ] && exit "$STUB_LABWC_RC"
echo "$STUB_MENU_RC" >"$TF_SESSION_STATUS"
exit 0
STUB
# sudo -n runs the boot screen helper; the helper records each call, and a
# quit made after the failure screen began is marked as late.
cat >"$WORK/ps/bin/sudo" <<'STUB'
#!/bin/sh
[ "$1" = -n ] && shift
exec "$@"
STUB
cat >"$WORK/ps/step" <<'STUB'
#!/bin/sh
if [ "$1" = quit ] && grep -q '=====' "$PS_OUT" 2>/dev/null; then
    echo "quit (late)" >>"$PS_STEPS"
else
    echo "$*" >>"$PS_STEPS"
fi
if [ -n "${STUB_STEP_DEAD:-}" ]; then
    echo "plymouth noise on stdout"
    echo "plymouth noise on stderr" >&2
    exit 1
fi
exit 0
STUB
chmod 755 "$WORK/ps/bin/tty" "$WORK/ps/bin/labwc" "$WORK/ps/bin/sudo" "$WORK/ps/step"
run_profile() { # menu-status [extra env assignments...]
    local menu_rc=$1
    shift
    rm -f "$WORK/ps/steps"
    env -i PATH="$WORK/ps/bin:/usr/bin:/bin" HOME="$WORK/ps/home" \
        XDG_RUNTIME_DIR="$WORK/ps/run" STUB_MENU_RC="$menu_rc" \
        TF_SESSION_VERDICT="$VERDICT" TF_SESSION_FAILURES="$WORK/ps/failures" \
        TF_PLYMOUTH_STEP="$WORK/ps/step" TF_LABWC_SCALE=/bin/true TF_WAIT_KMS=/bin/true \
        PS_STEPS="$WORK/ps/steps" PS_OUT="$WORK/ps/out" "$@" \
        sh "$PROFILE" >"$WORK/ps/out" 2>&1
    ps_rc=$?
}
steps() { tr '\n' ',' <"$WORK/ps/steps" 2>/dev/null; }
run_profile 101
r1=$ps_rc; o1=$(cat "$WORK/ps/out")
run_profile 101
r2=$ps_rc
run_profile 101
if [[ $r1 -eq 0 && $r2 -eq 0 && "$o1" != *"graphical session exited"* ]] &&
   grep -q 'graphical session exited with status 101' "$WORK/ps/out"; then
    ok "profile: two failed menus restart, the third shows the failure screen"
else
    bad "profile: r1=$r1 r2=$r2 out=$(cat "$WORK/ps/out")"
fi
run_profile 0
if [[ $ps_rc -eq 0 && ! -e "$WORK/ps/failures" ]] && ! grep -q 'graphical session exited' "$WORK/ps/out"; then
    ok "profile: a clean logout ends the session and clears the count"
else
    bad "profile: clean logout rc=$ps_rc out=$(cat "$WORK/ps/out")"
fi

# The boot picture stays until the session draws: the profile sends a step,
# lets labwc take the display, and ends Plymouth once labwc returns.
if [[ "$(steps)" == "status Starting the session,deactivate,(labwc),quit," ]]; then
    ok "profile: step, deactivate, labwc, quit"
else
    bad "profile: boot screen calls were '$(steps)'"
fi

# labwc could not start, twice: Plymouth gets the display back while the
# profile waits for a graphics driver, and is ended before the failure screen.
run_profile 0 STUB_LABWC_RC=1
want="status Starting the session,deactivate,(labwc),reactivate,status Waiting for the graphics driver,deactivate,(labwc),quit,"
if [[ "$(steps)" == "$want" ]] && grep -q 'graphical session exited with status 1' "$WORK/ps/out"; then
    ok "profile: a failed labwc gives the display back, then quits Plymouth before the failure screen"
else
    bad "profile: failed labwc: calls '$(steps)' out=$(cat "$WORK/ps/out")"
fi

# A later session finds Plymouth gone: the helper fails and talks; the
# session neither shows it nor stops.
run_profile 0 STUB_STEP_DEAD=1
if [[ $ps_rc -eq 0 ]] && ! grep -q 'plymouth noise' "$WORK/ps/out" &&
   [[ "$(steps)" == "status Starting the session,deactivate,(labwc),quit," ]]; then
    ok "profile: a dead boot screen changes nothing"
else
    bad "profile: dead boot screen: rc=$ps_rc calls '$(steps)' out=$(cat "$WORK/ps/out")"
fi

exit $fail
