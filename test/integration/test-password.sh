#!/bin/bash
# Offline tests for thinfloe-password.
#
# The image ships no password for tfuser: adduser --disabled-password leaves
# `!` in the shadow field and nothing overwrites it. A person at the machine
# sets one with `sudo thinfloe-password`, and it has to survive a reboot, so the
# hash is stored in tfuser's home directory where thinfloe-save-home captures it
# and the initramfs restores it. thinfloe-password --apply, run before
# ssh.service, puts it back into the shadow file.
#
# Two properties are load-bearing and have each already cost this repo a bug
# in a sibling script:
#
#   - The home directory is resolved from `getent passwd tfuser`, never $HOME.
#     This script runs under sudo, where env_reset makes HOME /root, so a
#     $HOME-based path writes the hash into root's home: lost at reboot, and
#     with the wrong owner. Same defect as the one cfe7aa0 fixed in
#     thinfloe-save-home.
#   - The stored file is owned by tfuser, not root, like the rest of the
#     home. Until 2026-10 the netboot save in thinfloe-save-home ran tar as
#     tfuser, and a root-owned 600 file failed that save outright.
#
# Runs without root: chpasswd, getent, passwd and chown are stubbed and their
# calls recorded.
#
# Usage: test/integration/test-password.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$REPO/overlay/usr/bin/thinfloe-password"

if [[ ! -x "$SCRIPT" ]]; then
    echo "FAIL: $SCRIPT not found or not executable" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

STUBS="$WORK/stubs"
mkdir -p "$STUBS"

# chpasswd records its arguments and the line it was fed. Without -e it is
# being given a plaintext password to hash; with -e a ready-made hash.
cat >"$STUBS/chpasswd" <<'STUB'
#!/bin/bash
{ echo "chpasswd $*"; sed 's/^/stdin: /'; } >>"$TF_TEST_LOG"
exit 0
STUB

# getent serves both lookups the script makes: passwd for the home directory,
# shadow for the hash chpasswd just produced.
cat >"$STUBS/getent" <<'STUB'
#!/bin/bash
echo "getent $*" >>"$TF_TEST_LOG"
case "$1" in
    passwd) echo "tfuser:x:1001:1001:ThinUser:$TF_TEST_PASSWD_HOME:/bin/bash" ;;
    shadow) echo "tfuser:$TF_TEST_SHADOW_HASH:20000:0:99999:7:::" ;;
    *)      exit 2 ;;
esac
exit 0
STUB

cat >"$STUBS/passwd" <<'STUB'
#!/bin/bash
echo "passwd $*" >>"$TF_TEST_LOG"
exit 0
STUB

cat >"$STUBS/chown" <<'STUB'
#!/bin/bash
echo "chown $*" >>"$TF_TEST_LOG"
exit 0
STUB

chmod +x "$STUBS"/*

STORE_NAME=".config/thinfloe/password"
OLD_STORE_NAME=".thinfloe-password"

# run_password <home-for-HOME-env> <stdin-text> [args...]
run_password() {
    local home_env="$1" input="$2"; shift 2
    printf '%s' "$input" | \
    TF_TEST_LOG="$WORK/calls.log" \
    TF_STATE_DIR="$WORK/state" \
    TF_TEST_PASSWD_HOME="$WORK/home/tfuser" \
    TF_TEST_SHADOW_HASH="$FAKE_HASH" \
    HOME="$home_env" \
    PATH="$STUBS:$PATH" \
        "$SCRIPT" "$@" >"$WORK/out" 2>"$WORK/err"
    echo $?
}

FAKE_HASH='$y$j9T$aaaaaaaaaaaaaaaaaaaaa$bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
STORE="$WORK/home/tfuser/$STORE_NAME"

setup() {
    rm -rf "$WORK/home" "$WORK/root" "$WORK/calls.log" \
           "$WORK/out" "$WORK/err" "$WORK/state" "$WORK/elsewhere"
    mkdir -p "$WORK/home/tfuser" "$WORK/root" "$WORK/state"
    echo "root's shell config" >"$WORK/root/.bashrc"
}

# ---------------------------------------------------------------- test 1
# The hash that lands in the file is the one the system produced for the
# plaintext, read back out of the shadow file, so the stored value always
# matches whatever crypt method the image is configured for.
setup
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if [[ "$rc" == "0" ]] && [[ "$(cat "$STORE" 2>/dev/null)" == "$FAKE_HASH" ]]; then
    ok "--set stores the hash the system produced"
else
    bad "--set stores the hash the system produced (exit $rc, stored '$(cat "$STORE" 2>/dev/null)')"
fi

# ---------------------------------------------------------------- test 2
# HOME is /root under sudo. A $HOME-based path would write the hash there.
setup
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if [[ -f "$STORE" ]] && [[ ! -e "$WORK/root/$STORE_NAME" ]]; then
    ok "--set resolves the home from getent, not from HOME"
else
    bad "--set resolves the home from getent, not from HOME (exit $rc, in root's home: $([[ -e "$WORK/root/$STORE_NAME" ]] && echo yes || echo no))"
fi

# ---------------------------------------------------------------- test 3
# The stored file belongs to tfuser, like the rest of the home (see the
# header).
setup
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
mode=$(stat -c '%a' "$STORE" 2>/dev/null)
if [[ "$mode" == "600" ]] && grep -q "^chown -h tfuser:tfuser $(dirname "$STORE")/.password\." "$WORK/calls.log"; then
    ok "the hash is stored in ~/.config/thinfloe/password (mode 600, owned by tfuser)"
else
    bad "--set stores mode 600 owned by tfuser (mode '$mode', chown: $(grep '^chown' "$WORK/calls.log" 2>/dev/null | head -1))"
fi

# ---------------------------------------------------------------- test 4
setup
rc=$(run_password "$WORK/root" $'swordfish\nhaddock\n')
if [[ "$rc" != "0" ]] && [[ ! -e "$STORE" ]] &&
   ! grep -q '^chpasswd' "$WORK/calls.log" 2>/dev/null; then
    ok "--set refuses a mismatch and changes nothing"
else
    bad "--set refuses a mismatch and changes nothing (exit $rc, stored $([[ -e "$STORE" ]] && echo yes || echo no))"
fi

# ---------------------------------------------------------------- test 5
# An empty password plus PermitEmptyPasswords would be a silent open door, and
# an empty stored file is indistinguishable from a truncated one.
setup
rc=$(run_password "$WORK/root" $'\n\n')
if [[ "$rc" != "0" ]] && [[ ! -e "$STORE" ]] &&
   ! grep -q '^chpasswd' "$WORK/calls.log" 2>/dev/null; then
    ok "--set refuses an empty password"
else
    bad "--set refuses an empty password (exit $rc, stored $([[ -e "$STORE" ]] && echo yes || echo no))"
fi

# ---------------------------------------------------------------- test 6
# `passwd -l` alone is not enough and this test exists because the first
# implementation got it wrong: -l prefixes the EXISTING hash with `!` and
# leaves it recoverable, so clearing the password left the old one sitting in
# the shadow file. Observed against the real image: after --clear the field
# read `!$y$j9T$ULDsq...`. The delete must come first.
setup
mkdir -p "$(dirname "$STORE")"; echo "$FAKE_HASH" >"$STORE"
rc=$(run_password "$WORK/root" '' --clear)
order=$(grep '^passwd ' "$WORK/calls.log" | tr '\n' ' ')
if [[ "$rc" == "0" ]] && [[ ! -e "$STORE" ]] &&
   [[ "$order" == "passwd -d tfuser passwd -l tfuser " ]]; then
    ok "--clear removes the stored hash and deletes before locking"
else
    bad "--clear removes the stored hash and deletes before locking (exit $rc, stored $([[ -e "$STORE" ]] && echo yes || echo no), passwd calls: '$order')"
fi

# ---------------------------------------------------------------- test 7
# The common case on every machine nobody has set a password on. It must not
# fail the unit, which is ordered before ssh.service.
setup
rc=$(run_password "$WORK/root" '' --apply)
if [[ "$rc" == "0" ]] && ! grep -q '^chpasswd' "$WORK/calls.log" 2>/dev/null; then
    ok "--apply is a no-op when no hash is stored"
else
    bad "--apply is a no-op when no hash is stored (exit $rc, chpasswd: $(grep '^chpasswd' "$WORK/calls.log" 2>/dev/null | head -1))"
fi

# ---------------------------------------------------------------- test 8
setup
mkdir -p "$(dirname "$STORE")"; echo "$FAKE_HASH" >"$STORE"
rc=$(run_password "$WORK/root" '' --apply)
if [[ "$rc" == "0" ]] && grep -q '^chpasswd -e$' "$WORK/calls.log" &&
   grep -qF "stdin: tfuser:$FAKE_HASH" "$WORK/calls.log"; then
    ok "--apply feeds the stored hash to chpasswd -e"
else
    bad "--apply feeds the stored hash to chpasswd -e (exit $rc, calls: $(grep -c . "$WORK/calls.log" 2>/dev/null))"
fi

# ---------------------------------------------------------------- test 9
# A truncated or empty store must not become an empty password.
setup
mkdir -p "$(dirname "$STORE")"; : >"$STORE"
rc=$(run_password "$WORK/root" '' --apply)
if [[ "$rc" != "0" ]] && ! grep -q '^chpasswd' "$WORK/calls.log" 2>/dev/null; then
    ok "--apply refuses an empty stored hash"
else
    bad "--apply refuses an empty stored hash (exit $rc, chpasswd: $(grep '^chpasswd' "$WORK/calls.log" 2>/dev/null | head -1))"
fi

# ---------------------------------------------------------------- test 10
# The plaintext must never reach the stored file or the terminal.
setup
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if ! grep -qF 'swordfish' "$STORE" 2>/dev/null &&
   ! grep -qF 'swordfish' "$WORK/out" "$WORK/err" 2>/dev/null; then
    ok "--set never writes the plaintext to the store or the terminal"
else
    bad "--set never writes the plaintext to the store or the terminal (exit $rc)"
fi

# ---------------------------------------------------------------- test 11
# 4.0 keeps no link under the 3.x name ~/.dbrrg-password: a 3.x home is
# migrated once by the initramfs and nothing reads the old name afterwards.
setup
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
OLD="$WORK/home/tfuser/.dbrrg-password"
if [[ "$rc" == "0" ]] && [[ ! -e "$OLD" && ! -L "$OLD" ]] &&
   ! grep -q '^chown -h.*dbrrg' "$WORK/calls.log"; then
    ok "--set makes no ~/.dbrrg-password"
else
    bad "--set makes no ~/.dbrrg-password (exit $rc, link '$(readlink "$OLD" 2>/dev/null)')"
fi

# ---------------------------------------------------------------- test 12
# --clear also removes a ~/.dbrrg-password that a save on a 3.x fallback
# boot left in the home. 4.0 never reads it, but the next 3.x boot would
# install it, and so would a migration on a boot whose archive lost the
# home-version marker: a cleared password must not come back.
setup
mkdir -p "$(dirname "$STORE")"; echo "$FAKE_HASH" >"$STORE"
echo "3x-hash" >"$WORK/home/tfuser/.dbrrg-password"
rc=$(run_password "$WORK/root" '' --clear)
if [[ "$rc" == "0" ]] && [[ ! -e "$STORE" ]] &&
   [[ ! -e "$WORK/home/tfuser/.dbrrg-password" ]]; then
    ok "--clear removes the new store and a 3.x ~/.dbrrg-password"
else
    bad "--clear removes ~/.thinfloe-password and a 3.x ~/.dbrrg-password (exit $rc)"
fi

# ---------------------------------------------------------------- test 13
# A legacy home keeps the hash in ~/.thinfloe-password; --apply reads that
# and not the new store.
setup
echo legacy >"$WORK/state/home-layout"
echo "$FAKE_HASH" >"$WORK/home/tfuser/$OLD_STORE_NAME"
mkdir -p "$WORK/home/tfuser/.config/thinfloe"
echo 'other-hash' >"$WORK/home/tfuser/.config/thinfloe/password"
rc=$(run_password "$WORK/root" '' --apply)
if [[ "$rc" == "0" ]] && grep -qF "stdin: tfuser:$FAKE_HASH" "$WORK/calls.log" &&
   ! grep -q 'other-hash' "$WORK/calls.log"; then
    ok "--apply reads ~/.thinfloe-password on a legacy home"
else
    bad "--apply reads ~/.thinfloe-password on a legacy home (exit $rc)"
fi

# A legacy --set writes the old name and creates no ~/.config/thinfloe.
setup
echo legacy >"$WORK/state/home-layout"
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if [[ "$rc" == "0" ]] && [[ -f "$WORK/home/tfuser/$OLD_STORE_NAME" ]] &&
   [[ ! -e "$WORK/home/tfuser/.config" ]]; then
    ok "--set on a legacy home writes ~/.thinfloe-password"
else
    bad "--set on a legacy home writes ~/.thinfloe-password (exit $rc)"
fi

# ---------------------------------------------------------------- test 14
# --clear removes the old and the new store, whatever the layout says.
for layout in new legacy; do
    setup
    echo "$layout" >"$WORK/state/home-layout"
    mkdir -p "$WORK/home/tfuser/.config/thinfloe"
    echo "$FAKE_HASH" >"$STORE"
    echo "$FAKE_HASH" >"$WORK/home/tfuser/$OLD_STORE_NAME"
    rc=$(run_password "$WORK/root" '' --clear)
    if [[ "$rc" == "0" && ! -e "$STORE" && ! -e "$WORK/home/tfuser/$OLD_STORE_NAME" ]]; then
        ok "--clear removes the old and the new store ($layout)"
    else
        bad "--clear removes the old and the new store ($layout, exit $rc)"
    fi
done

# ---------------------------------------------------------------- test 15
# The layout file is tfuser's to flip, so it may only choose between two
# paths in tfuser's home. A symlinked ~/.config/thinfloe (or ~/.config, or
# the store) must not be followed by root: nothing lands in the target and
# the password is not stored.
setup
mkdir -p "$WORK/home/tfuser/.config" "$WORK/elsewhere"
ln -s "$WORK/elsewhere" "$WORK/home/tfuser/.config/thinfloe"
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if [[ "$rc" != "0" ]] && [[ -z "$(ls -A "$WORK/elsewhere")" ]] &&
   grep -q 'link' "$WORK/err"; then
    ok "a symlinked ~/.config/thinfloe is refused by --set"
else
    bad "a symlinked ~/.config/thinfloe is refused by --set (exit $rc, target: $(ls -A "$WORK/elsewhere"))"
fi

setup
mkdir -p "$WORK/elsewhere"
ln -s "$WORK/elsewhere" "$WORK/home/tfuser/.config"
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if [[ "$rc" != "0" ]] && [[ -z "$(ls -A "$WORK/elsewhere")" ]]; then
    ok "a symlinked ~/.config is refused by --set"
else
    bad "a symlinked ~/.config is refused by --set (exit $rc, target: $(ls -A "$WORK/elsewhere"))"
fi

setup
mkdir -p "$WORK/home/tfuser/.config/thinfloe"
echo "precious" >"$WORK/precious"
ln -s "$WORK/precious" "$STORE"
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
if [[ "$(cat "$WORK/precious")" == "precious" ]] && [[ ! -L "$STORE" || "$rc" != "0" ]]; then
    ok "a symlinked store is not written through by --set"
else
    bad "a symlinked store is not written through by --set (exit $rc, target: $(cat "$WORK/precious"))"
fi

setup
mkdir -p "$WORK/home/tfuser/.config/thinfloe"
echo "$FAKE_HASH" >"$WORK/precious"
ln -s "$WORK/precious" "$STORE"
rc=$(run_password "$WORK/root" '' --apply)
if [[ "$rc" != "0" ]] && ! grep -q '^chpasswd' "$WORK/calls.log" 2>/dev/null; then
    ok "--apply refuses a symlinked store"
else
    bad "--apply refuses a symlinked store (exit $rc)"
fi

# ---------------------------------------------------------------- test 16
# A failing mv must not leave a temp file holding the hash in the home, where
# the next save would archive it.
setup
mkdir -p "$WORK/home/tfuser/.config/thinfloe"
cat >"$STUBS/mv" <<'STUB'
#!/bin/bash
exit 1
STUB
chmod +x "$STUBS/mv"
rc=$(run_password "$WORK/root" $'swordfish\nswordfish\n')
rm -f "$STUBS/mv"
if [[ "$rc" != "0" ]] && [[ -z "$(ls -A "$WORK/home/tfuser/.config/thinfloe")" ]]; then
    ok "a failing rename leaves no temporary file with the hash"
else
    bad "a failing rename leaves no temporary file (exit $rc, left: $(ls -A "$WORK/home/tfuser/.config/thinfloe"))"
fi

# ---------------------------------------------------------------- test 17
# A legacy home uses neither ~/.config nor ~/.config/thinfloe: a link there
# must not stop --apply (a unit before ssh.service) or --set.
setup
echo legacy >"$WORK/state/home-layout"
mkdir -p "$WORK/elsewhere"
ln -s "$WORK/elsewhere" "$WORK/home/tfuser/.config"
echo "$FAKE_HASH" >"$WORK/home/tfuser/$OLD_STORE_NAME"
rc=$(run_password "$WORK/root" '' --apply)
if [[ "$rc" == "0" ]] && grep -qF "stdin: tfuser:$FAKE_HASH" "$WORK/calls.log"; then
    ok "legacy --apply works with a symlinked ~/.config"
else
    bad "legacy --apply works with a symlinked ~/.config (exit $rc: $(cat "$WORK/err"))"
fi

if [[ "$fail" == "0" ]]; then
    echo
    echo "PASSED - thinfloe-password"
fi
exit $fail
