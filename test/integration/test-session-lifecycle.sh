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
# shellcheck disable=SC2329 # run by the EXIT trap
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
# gone_within <pid> <seconds>: true once the process has ended, a zombie
# (state Z, dead but not yet reaped) counting as ended; false when it still
# runs after <seconds>.
gone_within() {
    local pid=$1 tries=$(( $2 * 20 )) stat
    while (( tries-- > 0 )); do
        stat=$(cat "/proc/$pid/stat" 2>/dev/null) || return 0
        [[ ${stat##*) } == Z* ]] && return 0
        sleep 0.05
    done
    return 1
}

mkdir -p "$WORK/bin" "$WORK/home" "$WORK/state"
# The session and the profile read the layout of the home from
# $TF_STATE_DIR/home-layout (the initramfs writes it). The cases below that
# write the 4.x files (~/.thinfloe-sessionrc, ~/.config/kanshi, the screen
# file) run on a "legacy" home; the others set "new".
set_layout() { printf '%s\n' "$1" >"$WORK/state/home-layout"; }
set_layout legacy
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
        TF_STATE_DIR="$WORK/state" XDG_RUNTIME_DIR="${SESSION_XDG-$WORK}" TF_SESSION_STATUS="$WORK/thinfloe-session.status" \
        TF_MENU="$WORK/bin/menu" TF_SAVE_HOME="$WORK/bin/thinfloe-save-home" \
        TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" TF_LABWC_SCALE=/bin/true \
        sh "$SESSION" >/dev/null 2>"$WORK/session.err"
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
# The session sends TERM on its way out and does not wait. A fixed 0.3 s
# then one kill -0 failed under load (2026-11-13, one run in make test):
# the helper was still dying, or a zombie not yet reaped, which kill -0
# also finds. A live helper that ignores TERM still fails after 5 s.
if [[ -n "$hp" ]] && gone_within "$hp" 5; then
    ok "session: the scale helper ends with the session"
else
    bad "session: scale helper $hp still running"
    [[ -n "$hp" ]] && kill "$hp" 2>/dev/null
fi

# A menu output of an earlier session would put this session's menu on a
# monitor nobody chose: labwc-scale applies it at its start. The session
# removes the file and its .applied before the helper starts, at the path
# TF_MENU_OUTPUT_FILE names and at the default one in XDG_RUNTIME_DIR.
cat >"$WORK/bin/output-helper" <<STUB
#!/bin/sh
f=\${TF_MENU_OUTPUT_FILE:-\$XDG_RUNTIME_DIR/thinfloe-menu-output}
if [ -e "\$f" ] || [ -e "\$f.applied" ]; then echo stale; else echo gone; fi >"$WORK/output-helper.out"
STUB
chmod 755 "$WORK/bin/output-helper"
# The menu outlives the helper's look: the session stops it when the menu ends.
printf '#!/bin/sh\nsleep 0.5\nexit 0\n' >"$WORK/bin/menu"
chmod 755 "$WORK/bin/menu"
for where in named default; do
    rm -f "$WORK/output-helper.out"
    if [[ $where == named ]]; then
        out_file="$WORK/stale-menu-output"
        echo HDMI-A-1 >"$out_file"
        echo HDMI-A-1 >"$out_file.applied"
        env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" XDG_RUNTIME_DIR="$WORK" \
            TF_MENU="$WORK/bin/menu" TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" \
            TF_MENU_OUTPUT_FILE="$out_file" TF_LABWC_SCALE="$WORK/bin/output-helper" \
            sh "$SESSION" >/dev/null 2>&1
    else
        out_file="$WORK/thinfloe-menu-output"
        echo HDMI-A-1 >"$out_file"
        echo HDMI-A-1 >"$out_file.applied"
        env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" XDG_RUNTIME_DIR="$WORK" \
            TF_MENU="$WORK/bin/menu" TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" \
            TF_LABWC_SCALE="$WORK/bin/output-helper" \
            sh "$SESSION" >/dev/null 2>&1
    fi
    if [[ ! -e "$out_file" && ! -e "$out_file.applied" \
          && "$(cat "$WORK/output-helper.out" 2>/dev/null)" == gone ]]; then
        ok "session: a stale menu output file ($where) and its .applied are gone before labwc-scale starts"
    else
        bad "session: stale menu output ($where): file $([[ -e $out_file ]] && echo kept || echo gone), .applied $([[ -e $out_file.applied ]] && echo kept || echo gone), helper saw '$(cat "$WORK/output-helper.out" 2>/dev/null)'"
    fi
    rm -f "$out_file" "$out_file.applied"
done

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
    local _ p left
    for _ in $(seq 20); do
        left=""
        for p in "$@"; do kill -0 "$p" 2>/dev/null && left="$left $p"; done
        [[ -z $left ]] && break
        sleep 0.1
    done
    echo "${left# }"
}
mkdir -p "${SCREEN%/*}"
# The menu stub's default: wait for swayidle, record its arguments and the
# environment the menu got.
SEEN_MENU="wait-idle; { cat '$WORK'/idle.*; echo \"pid=\$TF_IDLE_PID\"; echo \"tf=\${TF_IDLE_TIMEOUT-unset}\"; echo \"rc=\${TF_IDLE_TIMEOUT_RC:-}\"; } >'$WORK/seen'"
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
        *"timeout 900 "*"tf=900"*"rc=1") ok "idle: sessionrc wins ($name), the menu gets 900" ;;
        *) bad "idle: sessionrc $name: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
    esac
done
# The menu gets the value swayidle uses, as plain digits.
case "$(idle_run 'TF_IDLE_TIMEOUT=+0900' 'idle_timeout=600')" in
    *"timeout 900 "*"tf=900"*"rc=1") ok "idle: sessionrc +0900 is exported as 900" ;;
    *) bad "idle: sessionrc +0900: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
case "$(idle_run '' 'idle_timeout=600')" in
    *"tf=unset"*"rc=") ok "idle: without the sessionrc the menu gets no TF_IDLE_TIMEOUT" ;;
    *) bad "idle: no sessionrc value: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
case "$(idle_run 'TF_IDLE_TIMEOUT=600
DBRRG_IDLE_TIMEOUT=900' -)" in
    *"timeout 600 "*"rc=1") ok "idle: TF_IDLE_TIMEOUT wins over the old name" ;;
    *) bad "idle: new name did not win: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
# A sessionrc value that is no positive number disables blanking, as before.
# The menu then gets 0, so it shows "Never".
for v in 0 garbage -5; do
    idle_run "TF_IDLE_TIMEOUT=$v" 'idle_timeout=600' \
        "sleep 1; echo \"pid=\$TF_IDLE_PID tf=\${TF_IDLE_TIMEOUT-unset} rc=\${TF_IDLE_TIMEOUT_RC:-}\" >'$WORK/seen'" >/dev/null
    if ! ls "$WORK"/idle.* >/dev/null 2>&1 && grep -q '^pid=[0-9][0-9]* tf=0 rc=1$' "$WORK/seen" 2>/dev/null; then
        ok "idle: sessionrc TF_IDLE_TIMEOUT=$v starts no swayidle, the menu gets 0"
    else
        bad "idle: sessionrc TF_IDLE_TIMEOUT=$v: '$(cat "$WORK/seen" 2>/dev/null)' $(ls "$WORK"/idle.* 2>/dev/null)"
    fi
done
# Zero: no swayidle, the helper still runs (and its sleep must end with it).
idle_run '' 'idle_timeout=0' \
    "sleep 1; echo \"pid=\$TF_IDLE_PID\" >'$WORK/seen'; pgrep -P \"\$TF_IDLE_PID\" >'$WORK/idle-kids'" >/dev/null
mapfile -t zero_kids < <(cat "$WORK/idle-kids" 2>/dev/null)
if ! ls "$WORK"/idle.* >/dev/null 2>&1 && grep -q '^pid=[0-9][0-9]*$' "$WORK/seen" 2>/dev/null; then
    ok "idle: zero starts no swayidle, TF_IDLE_PID still set"
else
    bad "idle: zero: '$(cat "$WORK/seen" 2>/dev/null)' $(ls "$WORK"/idle.* 2>/dev/null)"
fi
alive=$(still_alive "${zero_kids[@]}")
if [[ ${#zero_kids[@]} -gt 0 && -z $alive ]]; then
    ok "idle: the helper's sleep for zero ends with the session"
else
    bad "idle: zero: children '${zero_kids[*]}', alive '$alive'"
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
idle_pids=()
for f in "$WORK"/idle.*; do [[ -e $f ]] && idle_pids+=("${f##*.}"); done
alive=$(still_alive "$hp" "${idle_pids[@]}")
if [[ -n $hp && -z $alive ]]; then
    ok "idle: idle helper and swayidle end with the session"
else
    bad "idle: still running after the session: '$alive' (helper '$hp', swayidle '${idle_pids[*]}')"
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

# A new home: the timeout comes from generated/screen alone. The sessionrc
# is no longer read for it, and the menu is told nothing about it.
NEW_SCREEN="$WORK/home/.config/thinfloe/generated/screen"
NEW_RC="$WORK/home/.config/thinfloe/sessionrc"
mkdir -p "${NEW_SCREEN%/*}"
set_layout new
printf '%s\n' 'TF_IDLE_TIMEOUT=5' >"$NEW_RC"
printf '%s\n' 'idle_timeout=600' >"$NEW_SCREEN"
printf '%s\n' 'idle_timeout=77' >"$SCREEN"
run_session "$SEEN_MENU"
case "$(cat "$WORK/seen" 2>/dev/null)" in
    *"timeout 600 "*"tf=unset"*"rc=") ok "thinfloe-session ignores TF_IDLE_TIMEOUT in the new sessionrc" ;;
    *) bad "new home: sessionrc timeout: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
rm -f "$WORK"/idle.* "$NEW_SCREEN"
run_session "$SEEN_MENU"
case "$(cat "$WORK/seen" 2>/dev/null)" in
    *"timeout 300 "*) ok "new home: the old screen file is not read, the default is 300" ;;
    *) bad "new home: no generated/screen: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
# A legacy home keeps the sessionrc timeout.
set_layout legacy
rm -f "$WORK"/idle.*
printf '%s\n' 'TF_IDLE_TIMEOUT=900' >"$WORK/home/.thinfloe-sessionrc"
run_session "$SEEN_MENU"
case "$(cat "$WORK/seen" 2>/dev/null)" in
    *"timeout 900 "*"tf=900"*"rc=1") ok "thinfloe-session keeps the sessionrc timeout on a legacy home" ;;
    *) bad "legacy home: sessionrc timeout: '$(cat "$WORK/seen" 2>/dev/null)'" ;;
esac
rm -f "$WORK/home/.thinfloe-sessionrc" "$SCREEN" "$NEW_RC" "$WORK"/idle.*

# --- TERM and INT --------------------------------------------------------
# Run the session in the background, as labwc does; its PID is the shell's.
# Job control, because bash starts a plain background job with SIGINT
# ignored, and a shell cannot trap a signal it was started ignoring. labwc
# resets every signal for the session.
start_session() { # menu-code
    rm -f "$WORK"/idle.* "$WORK/seen" "$WORK/menu.ready" "$WORK/signalled" "$WORK/foot.args" \
        "$WORK/thinfloe-session.status" "$WORK/sudo.args" "$WORK/steps"
    printf '#!/bin/sh\n%s\n' "$1" >"$WORK/bin/menu"
    chmod 755 "$WORK/bin/menu"
    set -m
    env -i PATH="$WORK/bin:/usr/bin:/bin" HOME="$WORK/home" XDG_RUNTIME_DIR="$WORK" \
        TF_MENU="$WORK/bin/menu" TF_SESSION_LOG="$WORK/log" TF_PLYMOUTH_STEP="$WORK/bin/step" \
        TF_LABWC_SCALE=/bin/true sh "$SESSION" >/dev/null 2>&1 &
    session_pid=$!
    set +m
}
# Wait up to 3 s for file $1.
wait_file() {
    local _
    for _ in $(seq 30); do [[ -e $1 ]] && return 0; sleep 0.1; done
    return 1
}
# A signal while the menu runs waits for the menu: the helpers stay up
# with it, its exit status decides as usual, and the helpers end after it.
for sig in TERM INT; do
    start_session "wait-idle; echo \"\$TF_IDLE_PID\" >'$WORK/idle-helper'; : >'$WORK/menu.ready'
n=0; while [ ! -e '$WORK/signalled' ] && [ \$n -lt 30 ]; do sleep 0.1; n=\$((n + 1)); done
sleep 0.3
for f in '$WORK'/idle.*; do kill -0 \"\${f##*.}\" 2>/dev/null && echo swayidle-up; done >'$WORK/seen'
kill -0 \"\$TF_IDLE_PID\" 2>/dev/null && echo helper-up >>'$WORK/seen'
exit 10"
    wait_file "$WORK/menu.ready"
    kill -"$sig" "$session_pid"
    : >"$WORK/signalled"
    wait "$session_pid"
    rc=$?
    hp=$(cat "$WORK/idle-helper" 2>/dev/null)
    idle_pids=()
    for f in "$WORK"/idle.*; do [[ -e $f ]] && idle_pids+=("${f##*.}"); done
    alive=$(still_alive "$hp" "${idle_pids[@]}")
    if [[ "$(cat "$WORK/seen" 2>/dev/null)" == $'swayidle-up\nhelper-up' && $rc -eq 0 &&
          "$(cat "$WORK/thinfloe-session.status" 2>/dev/null)" == 10 &&
          "$(cat "$WORK/sudo.args" 2>/dev/null)" == "systemctl reboot" &&
          ! -e "$WORK/foot.args" && -n $hp && -z $alive ]]; then
        ok "$sig during the menu: blanking stays until the menu ends, its restart goes ahead, the helpers end"
    else
        bad "$sig during the menu: seen '$(cat "$WORK/seen" 2>/dev/null)' rc=$rc status '$(cat "$WORK/thinfloe-session.status" 2>/dev/null)' sudo '$(cat "$WORK/sudo.args" 2>/dev/null)' foot '$(cat "$WORK/foot.args" 2>/dev/null)' alive '$alive'"
    fi
done
# Once the menu has started, a signal leaves the helpers to the end of the
# session: a failure window after a failed menu still blanks the screen.
# (A trap that killed them would also catch a signal that came between
# marking the menu as started and its fork, and the menu would run without
# them.) The foot stub notes whether swayidle is up.
cat >"$WORK/bin/foot" <<STUB
#!/bin/sh
echo "\$@" >"$WORK/foot.args"
for f in '$WORK'/idle.*; do kill -0 "\${f##*.}" 2>/dev/null && echo swayidle-up; done >"$WORK/foot.seen"
STUB
for sig in TERM INT; do
    rm -f "$WORK/foot.seen"
    start_session "wait-idle; echo \"\$TF_IDLE_PID\" >'$WORK/idle-helper'; : >'$WORK/menu.ready'
n=0; while [ ! -e '$WORK/signalled' ] && [ \$n -lt 30 ]; do sleep 0.1; n=\$((n + 1)); done
exit 1"
    wait_file "$WORK/menu.ready"
    kill -"$sig" "$session_pid"
    : >"$WORK/signalled"
    wait "$session_pid"
    rc=$?
    hp=$(cat "$WORK/idle-helper" 2>/dev/null)
    alive=$(still_alive "$hp")
    if [[ $rc -eq 1 && "$(cat "$WORK/foot.seen" 2>/dev/null)" == swayidle-up && -n $hp && -z $alive ]]; then
        ok "$sig during a failing menu: the failure window still blanks, the helpers end with the session"
    else
        bad "$sig during a failing menu: rc=$rc foot saw '$(cat "$WORK/foot.seen" 2>/dev/null)' alive '$alive'"
    fi
done
cat >"$WORK/bin/foot" <<STUB
#!/bin/sh
echo "\$@" >"$WORK/foot.args"
STUB
# A signal before the menu starts (here while the boot screen helper runs)
# ends the session: no menu without screen blanking, no failure window.
cat >"$WORK/bin/step" <<STUB
#!/bin/sh
echo "\$*" >>"$WORK/steps"
: >"$WORK/step.running"
sleep 1
STUB
for sig in TERM INT; do
    rm -f "$WORK/step.running"
    start_session "echo menu >>'$WORK/steps'; exit 0"
    wait_file "$WORK/step.running"
    "$WORK/bin/wait-idle"
    idle_pids=()
    for f in "$WORK"/idle.*; do [[ -e $f ]] && idle_pids+=("${f##*.}"); done
    kill -"$sig" "$session_pid"
    wait "$session_pid"
    rc=$?
    alive=$(still_alive "${idle_pids[@]}")
    if [[ $rc -ne 0 && "$(cat "$WORK/steps" 2>/dev/null)" == quit-retain && ${#idle_pids[@]} -gt 0 && -z $alive &&
          ! -e "$WORK/thinfloe-session.status" && ! -e "$WORK/foot.args" ]]; then
        ok "$sig before the menu: the session ends, no menu, no failure window, swayidle ends"
    else
        bad "$sig before the menu: rc=$rc steps '$(cat "$WORK/steps" 2>/dev/null)' swayidle '${idle_pids[*]}' alive '$alive' status '$(cat "$WORK/thinfloe-session.status" 2>/dev/null)'"
    fi
done
cat >"$WORK/bin/step" <<STUB
#!/bin/sh
echo "\$*" >>"$WORK/steps"
STUB

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
        TF_STATE_DIR="$WORK/ps/state" XDG_RUNTIME_DIR="$WORK/ps/run" STUB_MENU_RC="$menu_rc" \
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

# --- the profile: migration, composer, merged environment -----------------
# A composer stub records its arguments and writes FOO=merged into the
# environment of the directory it is asked to build; a labwc stub that
# records the environment it got and the -C directory; a menu stub that
# records --migrate-home and moves the home to the new layout.
cat >"$WORK/ps/compose" <<'STUB'
#!/bin/sh
echo "$*" >"$PS_COMPOSE_ARGS"
[ "${STUB_COMPOSE_RC:-0}" -ne 0 ] && exit "$STUB_COMPOSE_RC"
mkdir -p "$4" && echo "FOO=merged" >"$4/environment"
STUB
cat >"$WORK/ps/menu" <<'STUB'
#!/bin/sh
echo "$*" >>"$PS_MIGRATE"
echo new >"$TF_STATE_DIR/home-layout"
STUB
cat >"$WORK/ps/bin/labwc" <<'STUB'
#!/bin/sh
echo "(labwc)" >>"$PS_STEPS"
echo "FOO=${FOO-unset} cfg=${TF_LABWC_CONFIG_DIR-unset} C=$2" >"$PS_LABWC_SEEN"
echo "$STUB_MENU_RC" >"$TF_SESSION_STATUS"
exit 0
STUB
chmod 755 "$WORK/ps/compose" "$WORK/ps/menu" "$WORK/ps/bin/labwc"
mkdir -p "$WORK/ps/state"
profile_cfg() { # layout [extra env...]
    local layout=$1
    shift
    rm -f "$WORK/ps/compose.args" "$WORK/ps/migrate" "$WORK/ps/labwc.seen"
    if [[ $layout == none ]]; then rm -f "$WORK/ps/state/home-layout"; else printf '%s\n' "$layout" >"$WORK/ps/state/home-layout"; fi
    run_profile 0 TF_COMPOSE_LABWC_CONFIG="$WORK/ps/compose" TF_MENU="$WORK/ps/menu" \
        PS_COMPOSE_ARGS="$WORK/ps/compose.args" PS_MIGRATE="$WORK/ps/migrate" \
        PS_LABWC_SEEN="$WORK/ps/labwc.seen" "$@"
}
PH="$WORK/ps/home"
PRUN="$WORK/ps/run"
profile_cfg legacy
if [[ "$(cat "$WORK/ps/migrate" 2>/dev/null)" == "--migrate-home" ]]; then
    ok "the profile runs the migration on a legacy home"
else
    bad "profile: migration calls '$(cat "$WORK/ps/migrate" 2>/dev/null)'"
fi
if [[ "$(cat "$WORK/ps/compose.args" 2>/dev/null)" == "/etc/thinfloe/labwc $PH/.config/thinfloe/generated/environment $PH/.config/thinfloe/environment $PRUN/thinfloe-labwc" ]]; then
    ok "the profile reads the layout again after the migration"
else
    bad "profile: compose args after migration '$(cat "$WORK/ps/compose.args" 2>/dev/null)'"
fi
profile_cfg new
[[ ! -e "$WORK/ps/migrate" ]] && ok "the profile does not run the migration on a new home" \
    || bad "profile: migration ran on a new home"
profile_cfg none
[[ ! -e "$WORK/ps/migrate" && -s "$WORK/ps/compose.args" ]] && ok "the profile treats a missing home-layout as new" \
    || bad "profile: missing home-layout: migrate=$(cat "$WORK/ps/migrate" 2>/dev/null)"
profile_cfg new
if [[ "$(cat "$WORK/ps/labwc.seen" 2>/dev/null)" == "FOO=merged cfg=$PRUN/thinfloe-labwc C=$PRUN/thinfloe-labwc" ]]; then
    ok "the profile sources the merged environment"
    ok "the profile exports TF_LABWC_CONFIG_DIR"
else
    bad "profile: labwc saw '$(cat "$WORK/ps/labwc.seen" 2>/dev/null)'"
fi
profile_cfg new STUB_COMPOSE_RC=1
if [[ "$(cat "$WORK/ps/labwc.seen" 2>/dev/null)" == "FOO=unset cfg=/etc/thinfloe/labwc C=/etc/thinfloe/labwc" ]]; then
    ok "the profile exports /etc/thinfloe/labwc after a failing composer"
else
    bad "profile: failing composer, labwc saw '$(cat "$WORK/ps/labwc.seen" 2>/dev/null)'"
fi
profile_cfg legacy TF_MENU=/nonexistent
if [[ "$(cat "$WORK/ps/compose.args" 2>/dev/null)" == "/etc/thinfloe/labwc $PH/.thinfloe-environment /nonexistent $PRUN/thinfloe-labwc" ]]; then
    ok "a legacy home composes with ~/.thinfloe-environment"
else
    bad "profile: legacy compose args '$(cat "$WORK/ps/compose.args" 2>/dev/null)'"
fi

# ---------------------------------------------------------------------------
# kanshi: the session undoes an unconfirmed layout, then starts kanshi on the
# menu's config. The fake records one argv line per start and its pid, and
# stays alive, so "not started" is an absent file and "once" is one line.
# ---------------------------------------------------------------------------
# kanshi_layout <legacy|new> points the paths at that layout's files and sets
# the home-layout state the session reads.
kanshi_layout() {
    set_layout "$1"
    if [[ $1 == legacy ]]; then
        KDIR="$WORK/home/.config/kanshi"
        KCONF="$KDIR/config"
        KPREV="$KDIR/config.prev"
        MARKER="$KDIR/thinfloe-layout-unconfirmed"
        SRC="$WORK/home/.thinfloe-sessionrc"
    else
        KDIR="$WORK/home/.config/thinfloe/generated"
        KCONF="$KDIR/kanshi"
        KPREV="$KDIR/kanshi.prev"
        MARKER="$KDIR/layout-unconfirmed"
        SRC="$WORK/home/.config/thinfloe/sessionrc"
    fi
}
cat >"$WORK/bin/kanshi" <<STUB
#!/bin/sh
echo "\$*" >>"$WORK/kanshi.argv"
cp "\$2" "$WORK/kanshi.seen" 2>/dev/null
echo \$\$ >>"$WORK/kanshi.pid"
trap 'kill \$s 2>/dev/null; exit 0' TERM
sleep 2147483647 &
s=\$!
wait \$s
STUB
# A pgrep that finds nothing: the already-running check must not need it.
printf '#!/bin/sh\nexit 1\n' >"$WORK/bin/pgrep"
chmod 755 "$WORK/bin/kanshi" "$WORK/bin/pgrep"
# The session looks for a kanshi of this user anywhere in /proc, so a stub
# still dying from the case before, or one of another run of this test on
# the same machine, makes it start none. kanshi_reset therefore waits until
# its stubs are gone, and the kanshi cases hold a per-user lock that other
# runs of this test take too.
kanshi_reset() {
    local p n=0
    for p in $(cat "$WORK/kanshi.pid" 2>/dev/null); do kill "$p" 2>/dev/null; done
    while pgrep -u "$(id -u)" -x kanshi >/dev/null && [[ $n -lt 40 ]]; do
        sleep 0.05
        n=$((n + 1))
    done
    if pgrep -u "$(id -u)" -x kanshi >/dev/null; then
        bad "a kanshi of this user still runs, the next kanshi case would not start one: $(pgrep -u "$(id -u)" -x kanshi | tr '\n' ' ')"
    fi
    rm -rf "$KDIR" "$MARKER" "$WORK/kanshi.argv" "$WORK/kanshi.pid" "$WORK/kanshi.seen" "$SRC"
}
exec {kanshi_lock}>"${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/thinfloe-test-kanshi.lock"
if ! flock -w 120 "$kanshi_lock"; then
    bad "kanshi cases: another run of this test held the kanshi lock for 120 s"
fi
if pgrep -u "$(id -u)" -x kanshi >/dev/null; then
    echo "skip - kanshi cases: a kanshi of this user is already running"
else
kanshi_cases() {
    kanshi_layout "$1"
    # Menu stub: give a started kanshi a moment to write its argv.
    KMENU='sleep 0.3; exit 0'

    mkdir -p "$KDIR"
    echo "profile a {}" >"$KCONF"
    run_session "$KMENU"
    if [[ "$(cat "$WORK/kanshi.argv" 2>/dev/null)" == "-c $KCONF" ]]; then
        ok "[$1 home] kanshi started when the config exists: once, with -c <config>"
    else
        bad "[$1 home] kanshi started: argv '$(cat "$WORK/kanshi.argv" 2>/dev/null)'"
    fi
    sleep 1.2
    kp=$(head -n1 "$WORK/kanshi.pid" 2>/dev/null)
    if [[ -n $kp ]] && ! kill -0 "$kp" 2>/dev/null; then
        ok "[$1 home] kanshi ends with the session (helper)"
    else
        bad "[$1 home] kanshi pid '$kp' still running after the session"
    fi
    kanshi_reset

    run_session "$KMENU"
    if [[ ! -e "$WORK/kanshi.argv" ]]; then
        ok "[$1 home] no config: no kanshi"
    else
        bad "[$1 home] no config: kanshi was started: $(cat "$WORK/kanshi.argv")"
    fi
    kanshi_reset

    # Already running: the sessionrc starts the fake by hand.
    mkdir -p "$KDIR"
    echo "profile a {}" >"$KCONF"
    mkdir -p "${SRC%/*}"
    printf 'kanshi -c "%s" &\n' "$KCONF" >"$SRC"
    run_session "sleep 0.5; exit 0"
    if [[ "$(wc -l <"$WORK/kanshi.argv" 2>/dev/null)" -eq 1 ]] &&
       [[ "$(wc -l <"$WORK/kanshi.pid" 2>/dev/null)" -eq 1 ]]; then
        ok "[$1 home] kanshi already running: session starts no second one (pgrep finds nothing)"
    else
        bad "[$1 home] kanshi already running: argv lines: $(cat "$WORK/kanshi.argv" 2>/dev/null)"
    fi
    kanshi_reset

    # An unconfirmed layout is undone before kanshi starts: the config
    # kanshi is given is already the old one.
    mkdir -p "$KDIR"
    echo new >"$KCONF"
    echo old >"$KPREV"
    echo prev >"$MARKER"
    run_session "$KMENU"
    if [[ "$(cat "$KCONF" 2>/dev/null)" == old && ! -e "$KPREV" && ! -e "$MARKER" &&
          "$(cat "$WORK/kanshi.seen" 2>/dev/null)" == old ]]; then
        ok "[$1 home] unconfirmed layout is undone: prev (before kanshi starts)"
    else
        bad "[$1 home] undo prev: config='$(cat "$KCONF" 2>/dev/null)' prev=$([[ -e $KPREV ]] && echo kept || echo gone) marker=$([[ -e $MARKER ]] && echo kept || echo gone) seen='$(cat "$WORK/kanshi.seen" 2>/dev/null)'"
    fi
    kanshi_reset

    mkdir -p "$KDIR"
    echo new >"$KCONF"
    echo none >"$MARKER"
    run_session "$KMENU"
    if [[ ! -e "$KCONF" && ! -e "$MARKER" && ! -e "$WORK/kanshi.argv" ]]; then
        ok "[$1 home] unconfirmed layout is undone: none (config gone, no kanshi)"
    else
        bad "[$1 home] undo none: config=$([[ -e $KCONF ]] && echo kept || echo gone) marker=$([[ -e $MARKER ]] && echo kept || echo gone) argv='$(cat "$WORK/kanshi.argv" 2>/dev/null)'"
    fi
    kanshi_reset

    # Unknown marker content: config stays, marker goes.
    mkdir -p "$KDIR"
    echo new >"$KCONF"
    echo old >"$KPREV"
    echo garbage >"$MARKER"
    run_session "$KMENU"
    if [[ "$(cat "$KCONF" 2>/dev/null)" == new && -e "$KPREV" && ! -e "$MARKER" ]]; then
        ok "[$1 home] unconfirmed layout: unknown marker content leaves the config alone"
    else
        bad "[$1 home] unknown marker: config='$(cat "$KCONF" 2>/dev/null)' marker=$([[ -e $MARKER ]] && echo kept || echo gone)"
    fi
    kanshi_reset

    # The undo comes before the sessionrc runs: a kanshi started there sees
    # the restored config.
    mkdir -p "$KDIR"
    echo new >"$KCONF"
    echo old >"$KPREV"
    echo prev >"$MARKER"
    mkdir -p "${SRC%/*}"
    printf 'kanshi -c "%s" &\n' "$KCONF" >"$SRC"
    run_session "$KMENU"
    if [[ "$(cat "$WORK/kanshi.seen" 2>/dev/null)" == old && "$(wc -l <"$WORK/kanshi.argv")" -eq 1 ]]; then
        ok "[$1 home] unconfirmed layout is undone before the sessionrc starts kanshi"
    else
        bad "[$1 home] undo before sessionrc: seen='$(cat "$WORK/kanshi.seen" 2>/dev/null)'"
    fi
    kanshi_reset

    # prev without config.prev: nothing to restore, so config goes, with a message.
    mkdir -p "$KDIR"
    echo new >"$KCONF"
    echo prev >"$MARKER"
    run_session "$KMENU"
    if [[ ! -e "$KCONF" && ! -e "$MARKER" && ! -e "$WORK/kanshi.argv" ]] &&
       grep -q "without ${KPREV##*/}" "$WORK/session.err"; then
        ok "[$1 home] unconfirmed layout: prev without config.prev removes the config and says so"
    else
        bad "[$1 home] prev without config.prev: config=$([[ -e $KCONF ]] && echo kept || echo gone) err='$(cat "$WORK/session.err")'"
    fi
    kanshi_reset

    # The marker sits in the home: the undo needs no runtime dir.
    mkdir -p "$KDIR"
    echo new >"$KCONF"
    echo old >"$KPREV"
    echo prev >"$MARKER"
    SESSION_XDG="" run_session "$KMENU"
    if [[ "$(cat "$KCONF" 2>/dev/null)" == old && ! -e "$KPREV" && ! -e "$MARKER" &&
          "$(cat "$WORK/kanshi.seen" 2>/dev/null)" == old ]]; then
        ok "[$1 home] unconfirmed layout is undone with XDG_RUNTIME_DIR empty (marker in the home)"
    else
        bad "[$1 home] undo without runtime dir: config='$(cat "$KCONF" 2>/dev/null)' prev=$([[ -e $KPREV ]] && echo kept || echo gone) marker=$([[ -e $MARKER ]] && echo kept || echo gone) seen='$(cat "$WORK/kanshi.seen" 2>/dev/null)' argv='$(cat "$WORK/kanshi.argv" 2>/dev/null)'"
    fi
    kanshi_reset
}
kanshi_cases legacy
kanshi_cases new
fi

exit $fail
