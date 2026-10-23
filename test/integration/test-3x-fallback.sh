#!/bin/bash
# A home saved by 4.0 must still work when the stick boots 3.x again (tl.old).
#
# 4.0 renames a 3.x home's ~/.dbrrg-* files to ~/.thinfloe-* and leaves the
# old names as relative symlinks. This test builds such a home, archives it
# the way 4.0's thinfloe-save-home does, and runs the real 3.x scripts
# (test/fixtures/3.x, frozen from commit 53d72c1) against it.
#
# Runs without root: chown, getent, chpasswd and ssh-keygen are stubbed.
#
# Usage: test/integration/test-3x-fallback.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
LIB4="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh"
FIX="$REPO/test/fixtures/3.x"
SAVE4="$REPO/overlay/usr/bin/thinfloe-save-home"

for f in "$LIB4" "$SAVE4" "$FIX/dbrrg-lib.sh" "$FIX/dbrrg-ssh-hostkeys" \
         "$FIX/dbrrg-password" "$FIX/dbrrg-save-home"; do
    [[ -f "$f" ]] || { echo "FAIL: $f not found" >&2; exit 1; }
done

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# dracut's logging helpers are absent outside an initramfs.
info() { :; }
warn() { echo "warn: $*" >>"$WORK/warnings"; }
dinfo() { :; }
tf_log() { :; }
die()  { return 1; }
: >"$WORK/warnings"
_TF_LIB_LOADED=""
# shellcheck source=/dev/null
. "$LIB4"
tf_log() { :; }

OLD_NAMES=".dbrrg-environment .dbrrg-sessionrc .dbrrg-password .dbrrg-ssh-host-keys .config/dbrrg"
FAKE_HASH='$y$j9T$aaaaaaaaaaaaaaaaaaaaa$bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'

# --- build a 4.0 home out of a 3.x archive --------------------------------
SRC="$WORK/src3x"
mkdir -p "$SRC/.dbrrg-ssh-host-keys" "$SRC/.config/dbrrg/menu"
echo "XKB_DEFAULT_LAYOUT=ch" >"$SRC/.dbrrg-environment"
echo "user-rc"               >"$SRC/.dbrrg-sessionrc"
echo "$FAKE_HASH"            >"$SRC/.dbrrg-password"
echo "PRIVATE-ed25519"       >"$SRC/.dbrrg-ssh-host-keys/ssh_host_ed25519_key"
echo "PUBLIC-ed25519"        >"$SRC/.dbrrg-ssh-host-keys/ssh_host_ed25519_key.pub"
echo "[Desktop Entry]"       >"$SRC/.config/dbrrg/menu/x.desktop"
tar -C "$SRC" -czf "$WORK/3x.tgz" .

H4="$WORK/home4"
mkdir -p "$H4"
echo default-env >"$H4/.thinfloe-environment"
echo default-rc  >"$H4/.thinfloe-sessionrc"
tar -xzf "$WORK/3x.tgz" -C "$H4"
tf_migrate_home_names "$H4" "$(tar -tzf "$WORK/3x.tgz")" "$WORK/renamed"
tf_link_old_home_names "$H4"

# 4.0's save: the tar form of thinfloe-save-home, run from inside the home.
if grep -E '^[^#]*tar .*(-h|--dereference)' "$SAVE4" >/dev/null; then
    bad "thinfloe-save-home passes -h or --dereference to tar"
else
    ok "thinfloe-save-home does not dereference links"
fi
if grep -E '^[^#]*tar .*(-h|--dereference)' "$FIX/dbrrg-save-home" >/dev/null; then
    bad "3.x dbrrg-save-home passes -h or --dereference to tar"
else
    ok "3.x dbrrg-save-home does not dereference links"
fi
( cd "$H4" && tar --warning=no-file-changed -zcf - . ) >"$WORK/home4.tgz"

listing=$(tar -tvzf "$WORK/home4.tgz")
echo "$listing" | grep -E ' \./\.(dbrrg|config/dbrrg)' | sed 's/^/     /'
missing=""
for n in $OLD_NAMES; do
    echo "$listing" | grep -E "^l.* \./$n -> " >/dev/null || missing+=" $n"
done
if [[ -z "$missing" ]]; then
    ok "the archive holds the five old names as symlinks"
else
    bad "old names not stored as links:$missing"
fi

# --- 3.x restore keeps the links ------------------------------------------
H3="$WORK/home3"
mkdir -p "$H3"
echo default-env >"$H3/.dbrrg-environment"
(
    _DBRRG_LIB_LOADED=""
    # shellcheck source=/dev/null
    . "$FIX/dbrrg-lib.sh"
    dbrrg_log() { :; }
    dbrrg_restore_home_from_file "$WORK/home4.tgz" "$H3"
) 2>>"$WORK/warnings"
rc=$?
if [[ $rc -eq 0 && -L "$H3/.dbrrg-environment" \
   && "$(cat "$H3/.dbrrg-environment")" == "XKB_DEFAULT_LAYOUT=ch" ]]; then
    ok "3.x restore keeps the links: .dbrrg-environment is a link and reads ch"
else
    bad "3.x restore (rc $rc): $(ls -l "$H3/.dbrrg-environment" 2>&1), content '$(cat "$H3/.dbrrg-environment" 2>&1)'"
fi

# --- 3.x host keys follow the link ----------------------------------------
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
cat >"$STUBS/chown" <<'STUB'
#!/bin/bash
echo "chown $*" >>"$TF_TEST_LOG"
exit 0
STUB
cat >"$STUBS/ssh-keygen" <<'STUB'
#!/bin/bash
echo "ssh-keygen $*" >>"$TF_TEST_LOG"
exit 0
STUB
cat >"$STUBS/chpasswd" <<'STUB'
#!/bin/bash
{ echo "chpasswd $*"; sed 's/^/stdin: /'; } >>"$TF_TEST_LOG"
exit 0
STUB
cat >"$STUBS/getent" <<'STUB'
#!/bin/bash
case "$1" in
    passwd) echo "tluser:x:1000:1000:ThinUser:$TF_TEST_PASSWD_HOME:/bin/bash" ;;
    shadow) echo "tluser:$TF_TEST_SHADOW_HASH:20000:0:99999:7:::" ;;
    *)      exit 2 ;;
esac
STUB
chmod +x "$STUBS"/*
export TF_TEST_LOG="$WORK/calls.log"
: >"$TF_TEST_LOG"

SSHD="$WORK/etc-ssh"
mkdir -p "$SSHD"
env PATH="$STUBS:$PATH" DBRRG_HOME_DIR="$H3" DBRRG_SSH_DIR="$SSHD" \
    DBRRG_KEY_OWNER=tluser "$FIX/dbrrg-ssh-hostkeys" >"$WORK/hk.out" 2>&1
rc=$?
if [[ $rc -eq 0 && "$(cat "$SSHD/ssh_host_ed25519_key" 2>/dev/null)" == "PRIVATE-ed25519" ]]; then
    ok "3.x host keys follow the link: the key is installed from ~/.dbrrg-ssh-host-keys"
else
    bad "3.x host keys (rc $rc): $(cat "$WORK/hk.out")"
fi

rm -f "$SSHD"/ssh_host_*
echo "PRIVATE-new" >"$SSHD/ssh_host_ed25519_key"
echo "PUBLIC-new"  >"$SSHD/ssh_host_ed25519_key.pub"
env PATH="$STUBS:$PATH" DBRRG_HOME_DIR="$H3" DBRRG_SSH_DIR="$SSHD" \
    DBRRG_KEY_OWNER=tluser "$FIX/dbrrg-ssh-hostkeys" --stage >"$WORK/hk.out" 2>&1
if [[ -L "$H3/.dbrrg-ssh-host-keys" \
   && "$(cat "$H3/.thinfloe-ssh-host-keys/ssh_host_ed25519_key" 2>/dev/null)" == "PRIVATE-new" ]]; then
    ok "3.x --stage leaves .dbrrg-ssh-host-keys a link and writes through it"
else
    bad "3.x --stage: $(ls -ld "$H3/.dbrrg-ssh-host-keys"), key '$(cat "$H3/.thinfloe-ssh-host-keys/ssh_host_ed25519_key" 2>&1)', out: $(cat "$WORK/hk.out")"
fi

# --- 3.x password follows the link ----------------------------------------
: >"$TF_TEST_LOG"
env PATH="$STUBS:$PATH" TF_TEST_PASSWD_HOME="$H3" HOME=/root \
    "$FIX/dbrrg-password" --apply >"$WORK/pw.out" 2>&1
rc=$?
if [[ $rc -eq 0 ]] && grep -qx 'chpasswd -e' "$TF_TEST_LOG" \
   && grep -qxF "stdin: tluser:$FAKE_HASH" "$TF_TEST_LOG"; then
    ok "3.x password follows the link: chpasswd -e received the stored hash"
else
    bad "3.x password (rc $rc): $(cat "$WORK/pw.out") calls: $(cat "$TF_TEST_LOG")"
fi

# --- a password cleared on 3.x stays cleared on 4.0 ------------------------
# 3.x's --clear removes ~/.dbrrg-password, which is only the link; the hash in
# ~/.thinfloe-password survives and 3.x saves it. The next 4.0 restore must
# not hand the revoked password back to thinfloe-password.service.
cat >"$STUBS/passwd" <<'STUB'
#!/bin/bash
echo "passwd $*" >>"$TF_TEST_LOG"
exit 0
STUB
chmod +x "$STUBS/passwd"
env PATH="$STUBS:$PATH" TF_TEST_PASSWD_HOME="$H3" HOME=/root \
    "$FIX/dbrrg-password" --clear >"$WORK/pw.out" 2>&1
rc=$?
if [[ $rc -eq 0 && ! -L "$H3/.dbrrg-password" && -f "$H3/.thinfloe-password" ]]; then
    ok "3.x --clear removes only the link (fixture behaves as in the field)"
else
    bad "3.x --clear (rc $rc): $(ls -la "$H3" | tr '\n' ' ') out: $(cat "$WORK/pw.out")"
fi
( cd "$H3" && tar --warning=no-file-changed -zcf - . ) >"$WORK/home3.tgz"

NR="$WORK/newroot-after-clear"
mkdir -p "$NR/newroot/home/tluser" "$NR/efi" "$NR/state"
echo default-env >"$NR/newroot/home/tluser/.thinfloe-environment"
cp "$WORK/home3.tgz" "$NR/efi/home.tar.gz"
restore_home "$NR/newroot" "$NR/efi" "$NR/state" "tl/ramroot.sqsh"
H5="$NR/newroot/home/tluser"
if [[ ! -e "$H5/.thinfloe-password" && ! -L "$H5/.dbrrg-password" \
   && "$(cat "$H5/.thinfloe-environment")" == "XKB_DEFAULT_LAYOUT=ch" ]]; then
    ok "4.0 restore after a 3.x --clear: no ~/.thinfloe-password, the rest kept"
else
    bad "4.0 restore after 3.x --clear: pw=$(cat "$H5/.thinfloe-password" 2>&1) env=$(cat "$H5/.thinfloe-environment" 2>&1)"
fi

# --- a password set again on 3.x wins on 4.0 ------------------------------
# On a 3.x fallback the user runs `dbrrg-password --clear` and then `--set`:
# the clear removes the link, the set writes a real ~/.dbrrg-password beside
# the revoked hash in ~/.thinfloe-password. The next 4.0 restore must install
# the hash set on 3.x, not the revoked one.
NEW_HASH='$y$j9T$ccccccccccccccccccccc$ddddddddddddddddddddddddddddddddddddddddddd'
H6="$WORK/home6"
mkdir -p "$H6"
(
    _DBRRG_LIB_LOADED=""
    # shellcheck source=/dev/null
    . "$FIX/dbrrg-lib.sh"
    dbrrg_log() { :; }
    dbrrg_restore_home_from_file "$WORK/home4.tgz" "$H6"
) 2>>"$WORK/warnings"
env PATH="$STUBS:$PATH" TF_TEST_PASSWD_HOME="$H6" HOME=/root \
    "$FIX/dbrrg-password" --clear >"$WORK/pw.out" 2>&1
printf 'secret\nsecret\n' | env PATH="$STUBS:$PATH" TF_TEST_PASSWD_HOME="$H6" \
    TF_TEST_SHADOW_HASH="$NEW_HASH" HOME=/root \
    "$FIX/dbrrg-password" --set >>"$WORK/pw.out" 2>&1
rc=$?
if [[ $rc -eq 0 && -f "$H6/.dbrrg-password" && ! -L "$H6/.dbrrg-password" \
   && "$(cat "$H6/.dbrrg-password")" == "$NEW_HASH" \
   && "$(cat "$H6/.thinfloe-password")" == "$FAKE_HASH" ]]; then
    ok "3.x --clear then --set writes a real ~/.dbrrg-password (fixture behaves as in the field)"
else
    bad "3.x --clear/--set (rc $rc): $(ls -la "$H6" | tr '\n' ' ') out: $(cat "$WORK/pw.out")"
fi
( cd "$H6" && tar --warning=no-file-changed -zcf - . ) >"$WORK/home6.tgz"

NR="$WORK/newroot-after-set"
mkdir -p "$NR/newroot/home/tluser" "$NR/efi" "$NR/state"
echo default-env >"$NR/newroot/home/tluser/.thinfloe-environment"
cp "$WORK/home6.tgz" "$NR/efi/home.tar.gz"
restore_home "$NR/newroot" "$NR/efi" "$NR/state" "tl/ramroot.sqsh"
H7="$NR/newroot/home/tluser"
if [[ "$(cat "$H7/.thinfloe-password" 2>/dev/null)" == "$NEW_HASH" \
   && -L "$H7/.dbrrg-password" && "$(readlink "$H7/.dbrrg-password")" == ".thinfloe-password" \
   && ! -e "$NR/state/home-renamed" ]]; then
    ok "4.0 restore after a 3.x --clear and --set: ~/.thinfloe-password holds the 3.x hash"
else
    bad "4.0 restore after 3.x --clear/--set: pw=$(cat "$H7/.thinfloe-password" 2>&1) link=$(readlink "$H7/.dbrrg-password") renamed=$(cat "$NR/state/home-renamed" 2>&1)"
fi

exit "$fail"
