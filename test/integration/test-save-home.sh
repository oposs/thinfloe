#!/bin/bash
# Offline tests for thinfloe-save-home.
#
# The archive must come from tfuser's home, whatever $HOME says. thinfloe-upgrade
# re-execs itself under sudo and then calls this script, and sudo's env_reset
# sets HOME to the target user's home: with no env_keep for HOME in
# /etc/sudoers, that is /root. A save triggered that way archived root's
# dotfiles over the machine's home.tar.gz, and because /root holds a .bashrc
# the result was a valid archive that the next boot's restore accepted,
# replacing the user's home and ~/.thinfloe-ssh-host-keys with no error anywhere.
#
# Runs without root: tar, curl, ping and sudo are stubbed and their calls
# recorded. Layout is redirected via TF_HOME_DIR / TF_STATE_DIR /
# TF_CMDLINE.
#
# Usage: test/integration/test-save-home.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$REPO/overlay/usr/bin/thinfloe-save-home"

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

# tar records the directory it was invoked in, which is the whole point of
# these tests, and creates its output file so the script's later rm succeeds.
cat >"$STUBS/tar" <<'STUB'
#!/bin/bash
{ echo "tar $*"; echo "PWD=$PWD"; } >>"$TF_TEST_TAR_LOG"
# Only the argument after a -f / -zcf style flag is an output file; --exclude=
# patterns also contain slashes and must not be created.
prev=""
for a in "$@"; do
    case "$prev" in
        -*f) [ "$a" = "-" ] || : >"$a" ;;
    esac
    prev="$a"
done
# A test that looks inside the archive asks for the real tar.
[ -n "${TF_TEST_REAL_TAR:-}" ] && exec "$TF_TEST_REAL_TAR" "$@"
exit "${TF_TEST_TAR_RC:-0}"
STUB

# curl also records whether the file it was told to upload exists at that
# moment, so a test can tell an upload of the archive from one of nothing.
# The form value goes through the real curl, whose -F parser splits it on ;
# and , unless the file name is quoted: it opens the file before it
# connects, so exit 26 is a value that named no readable file, and 7 is a
# readable one meeting the refused loopback port 1.
REAL_CURL=$(command -v curl) || { echo "FAIL - the test needs curl" >&2; exit 1; }
export TF_TEST_REAL_CURL="$REAL_CURL"
cat >"$STUBS/curl" <<'STUB'
#!/bin/bash
echo "curl $*" >>"$TF_TEST_CURL_LOG"
for a in "$@"; do
    case "$a" in
        data=@*)
            "$TF_TEST_REAL_CURL" -s -o /dev/null -F "$a" http://127.0.0.1:1/
            case $? in
                7) echo "upload-file-exists" >>"$TF_TEST_CURL_LOG" ;;
                26) exit 26 ;;
            esac
            ;;
    esac
done
# Stand in for an upload that hangs, so a test can signal the script while it
# waits. The sleep is bounded and in the script's process group, so a signal
# to the group ends it and nothing outlives the test.
if [ -n "${TF_TEST_CURL_BLOCK:-}" ]; then
    : >"$TF_TEST_CURL_BLOCK"
    /bin/sleep 30
fi
exit "${TF_TEST_CURL_RC:-0}"
STUB

# The real mountpoint(1) needs a real mount. The test ESP is a plain directory;
# it counts as mounted only when it holds a .test-mounted marker.
cat >"$STUBS/mountpoint" <<'STUB'
#!/bin/bash
[ "$1" = "-q" ] && shift
[ -e "$1/.test-mounted" ]
STUB

cat >"$STUBS/ping" <<'STUB'
#!/bin/bash
exit 0
STUB

# sudo runs only the commands the USB branch uses to write the ESP (tar, gzip,
# mv, rm) and the find that looks for large files, resolved through PATH so the stubbed tar and gzip are used;
# without that the atomicity test would be vacuous. Everything else, such as
# thinfloe-ssh-hostkeys --stage, mount, dd and sync (a real sync would flush the
# shared host), is logged and swallowed because an
# unprivileged test may not perform it.
cat >"$STUBS/sudo" <<'STUB'
#!/bin/bash
echo "sudo $*" >>"$TF_TEST_SUDO_LOG"
case "${1:-}" in
    tar|gzip|mv|rm|find) exec "$@" ;;
esac
exit 0
STUB

# The script uses only -t (test): report the archive as valid unless the test
# asked for a corrupt one. The real tar runs gzip for -z; that call goes
# through.
REAL_GZIP=$(command -v gzip) || { echo "FAIL - the test needs gzip" >&2; exit 1; }
export TF_TEST_REAL_GZIP="$REAL_GZIP"
cat >"$STUBS/gzip" <<'STUB'
#!/bin/bash
[ "${1:-}" = "-t" ] || exec "$TF_TEST_REAL_GZIP" "$@"
[ -n "${TF_TEST_GZIP_FAIL:-}" ] && exit 1
exit 0
STUB

# The unreachable-server loop sleeps 1s per attempt; do not wait it out.
cat >"$STUBS/sleep" <<'STUB'
#!/bin/bash
exit 0
STUB

chmod +x "$STUBS"/*

run_save_home() {
    # $1 = value for HOME, $2 = value for TF_HOME_DIR
    TF_TEST_TAR_LOG="$WORK/tar.log" \
    TF_TEST_CURL_LOG="$WORK/curl.log" \
    TF_TEST_SUDO_LOG="$WORK/sudo.log" \
    HOME="$1" \
    TF_HOME_DIR="$2" \
    TF_STATE_DIR="$WORK/state" \
    TF_CMDLINE="$WORK/cmdline" \
    TF_EFI_MOUNT="${TF_EFI_MOUNT_OVERRIDE:-/run/thinfloe/storage/efi}" \
    TF_EXCLUDE_DEFAULT="${TF_EXCLUDE_DEFAULT_OVERRIDE:-/etc/thinfloe/save-home-exclude}" \
    TF_TEST_GZIP_FAIL="${TF_TEST_GZIP_FAIL:-}" \
    TF_TEST_TAR_RC="${TF_TEST_TAR_RC:-0}" \
    TF_TEST_CURL_RC="${TF_TEST_CURL_RC:-0}" \
    TMPDIR="${TMPDIR_OVERRIDE:-$WORK/tmp}" \
    PATH="$STUBS:$PATH" \
        "$SCRIPT" </dev/null >"$WORK/out" 2>"$WORK/err"
    echo $?
}

setup() {
    rm -rf "$WORK/home" "$WORK/root" "$WORK/state" "$WORK/tmp" \
           "$WORK/tar.log" "$WORK/curl.log" "$WORK/sudo.log"
    mkdir -p "$WORK/home/tfuser" "$WORK/root" "$WORK/state" "$WORK/tmp"
    echo "the user's wifi password" >"$WORK/home/tfuser/.thinfloe-sessionrc"
    echo "root's shell config"      >"$WORK/root/.bashrc"
    # Netboot: a boot server in the cmdline selects the upload path, which
    # needs no block device and so runs unprivileged.
    echo "ro ramroot=http://boot.example/thinfloe/ramroot.sqsh quiet" \
        >"$WORK/cmdline"
}

# ---------------------------------------------------------------- test 1
setup
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
archived=$(sed -n 's/^PWD=//p' "$WORK/tar.log" 2>/dev/null | head -1)
if [[ "$archived" == "$WORK/home/tfuser" ]]; then
    ok "archives tfuser's home when HOME points at /root"
else
    bad "archives tfuser's home when HOME points at /root (archived '$archived', exit $rc)"
fi

# ---------------------------------------------------------------- test 2
setup
rc=$(run_save_home "$WORK/root" "$WORK/does-not-exist")
if [[ "$rc" != "0" ]] && [[ ! -s "$WORK/tar.log" ]] &&
   grep -q "refusing to save" "$WORK/err"; then
    ok "refuses to save when the resolved home is missing"
else
    bad "refuses to save when the resolved home is missing (exit $rc, tar log $(wc -c <"$WORK/tar.log" 2>/dev/null || echo 0) bytes)"
fi

# ---------------------------------------------------------------- test 3
# A USB boot records no boot-home-base: restore_home() writes it only on the
# netboot branch (thinfloe-lib.sh:323-357). Reading it under `set -e` with no
# `|| true` ended the script on its second line, so a USB machine saved
# nothing at logout and lost every change made that session.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
if grep -q "thinfloe-ssh-hostkeys" "$WORK/sudo.log" 2>/dev/null; then
    ok "keeps going on a USB boot that recorded no boot-home-base"
else
    bad "keeps going on a USB boot that recorded no boot-home-base (exit $rc, stderr: $(head -1 "$WORK/err" 2>/dev/null))"
fi

# ---------------------------------------------------------------- test 7
# Every refusal needs its own exit code. The menu switches on these to tell
# the user what happened; with everything at 0 it can only say "saved".
setup
echo failed >"$WORK/state/home-restore"
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "2" ]] && [[ ! -s "$WORK/tar.log" ]]; then
    ok "exit 2 when this boot's home restore failed"
else
    bad "restore-failed refusal exited $rc (wanted 2), tar log $(wc -c <"$WORK/tar.log") bytes"
fi

# ---------------------------------------------------------------- test 8
setup
cat >"$STUBS/ping" <<'STUB'
#!/bin/bash
exit 1
STUB
chmod +x "$STUBS/ping"
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "3" ]] && [[ ! -s "$WORK/curl.log" ]]; then
    ok "exit 3 when the boot server is unreachable"
else
    bad "unreachable-server refusal exited $rc (wanted 3)"
fi
# restore the passing ping for the tests below
cat >"$STUBS/ping" <<'STUB'
#!/bin/bash
exit 0
STUB
chmod +x "$STUBS/ping"

# ---------------------------------------------------------------- test 9
# No boot server in the cmdline and no ESP mount: this machine has nowhere to
# put a home. It used to print "home saved" and exit 0 on this path.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/no-such-mount" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "4" ]] && ! grep -q "home saved" "$WORK/out"; then
    ok "exit 4 with no 'home saved' when there is nowhere to store the home"
else
    bad "no-target branch exited $rc (wanted 4), stdout: $(cat "$WORK/out")"
fi

# ---------------------------------------------------------------- test 10
# An attempted save that breaks must not collide with the missing-home
# refusal, which is exit 1. The menu would otherwise tell the user their home
# directory does not exist when the upload merely failed.
setup
rc=$(TF_TEST_CURL_RC=7 run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "5" ]]; then
    ok "exit 5 when the upload was attempted and failed"
else
    bad "failed upload exited $rc (wanted 5)"
fi

# ---------------------------------------------------------------- test 11
setup
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && grep -q "home saved" "$WORK/out"; then
    ok "exit 0 and 'home saved' on a successful netboot save"
else
    bad "successful save exited $rc, stdout: $(cat "$WORK/out")"
fi
# A stub cannot emulate an HTTP status, so the honest check is that curl is
# asked to fail on one (-f); without it a 413/500 reply exits 0.
if grep -qE '^curl (.* )?-f( |$)|^curl -[a-zA-Z]*f' "$WORK/curl.log"; then
    ok "netboot upload passes -f so an HTTP error fails the save"
else
    bad "curl was called without -f: $(cat "$WORK/curl.log")"
fi

# --------------------------------------------------------------- test 12
# The archive is unpacked synchronously by the initramfs on every boot, so an
# unfiltered home makes every startup slower. A 233MB Claude binary did this.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
printf './.cache\n./.local/share/claude/versions\n' >"$WORK/exclude-default"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     TF_EXCLUDE_DEFAULT_OVERRIDE="$WORK/exclude-default" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if grep -q 'exclude' "$WORK/tar.log" 2>/dev/null; then
    ok "passes exclude patterns to tar"
else
    bad "no --exclude reached tar (exit $rc, tar log: $(cat "$WORK/tar.log" 2>/dev/null))"
fi

# --------------------------------------------------------------- test 13
# A user file replaces the shipped defaults.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
printf './my-own-junk\n' >"$WORK/home/tfuser/.save-home-exclude"
printf './.cache\n' >"$WORK/exclude-default"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     TF_EXCLUDE_DEFAULT_OVERRIDE="$WORK/exclude-default" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if grep -q 'my-own-junk' "$WORK/tar.log" 2>/dev/null; then
    ok "~/.save-home-exclude takes precedence over the shipped defaults"
else
    bad "the user's exclude file was ignored (tar log: $(cat "$WORK/tar.log"))"
fi

# --------------------------------------------------------------- test 14
# A pattern containing a space must stay one pattern. Word-splitting a line
# into tar arguments breaks every path with a space in it.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
printf './My Documents/big\n' >"$WORK/home/tfuser/.save-home-exclude"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if grep -q 'exclude=./My Documents/big' "$WORK/tar.log" 2>/dev/null; then
    ok "an exclude pattern containing a space survives as one pattern"
else
    bad "a pattern with a space was split (tar log: $(cat "$WORK/tar.log"))"
fi

# --------------------------------------------------------------- test 15
# A missing or empty exclude file must not produce a bare --exclude=, which
# matches nothing or everything depending on the tar version.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
: >"$WORK/home/tfuser/.save-home-exclude"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     TF_EXCLUDE_DEFAULT_OVERRIDE="$WORK/nonexistent" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && ! grep -qE 'exclude=($|[[:space:]])' "$WORK/tar.log"; then
    ok "an empty exclude file produces no bare --exclude="
else
    bad "empty exclude file produced '$(grep -o 'exclude=[^ ]*' "$WORK/tar.log" | head -3)' (exit $rc)"
fi

# --------------------------------------------------------------- test 16
# Atomicity. The previous archive must survive a failed write - it is the only
# copy of the user's home and the machine's SSH identity.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
echo "THE GOOD OLD ARCHIVE" >"$WORK/esp/home.tar.gz"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     TF_TEST_GZIP_FAIL=1 \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "5" ]] &&
   grep -q "THE GOOD OLD ARCHIVE" "$WORK/esp/home.tar.gz" &&
   [[ ! -e "$WORK/esp/home.tar.gz.new" ]]; then
    ok "a corrupt new archive is discarded and the old one survives"
else
    bad "atomicity broken: exit $rc, old archive $(head -c40 "$WORK/esp/home.tar.gz" 2>/dev/null), .new $([[ -e "$WORK/esp/home.tar.gz.new" ]] && echo present || echo absent)"
fi

# --------------------------------------------------------------- test 17
# It must write to the ESP the initramfs already mounted, never mount
# by-partlabel again: every ThinFloe stick carries that label, so with two sticks
# plugged in the home can land on the wrong one.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && [[ -f "$WORK/esp/home.tar.gz" ]] &&
   ! grep -q 'mount' "$WORK/sudo.log" 2>/dev/null; then
    ok "writes to the existing ESP mount without mounting anything"
else
    bad "did not use the existing mount (exit $rc, sudo log: $(cat "$WORK/sudo.log" 2>/dev/null))"
fi

# --------------------------------------------------------------- test 18
# And the whole script must no longer name that symlink at all.
if ! grep -q 'by-partlabel' "$SCRIPT"; then
    ok "thinfloe-save-home no longer mentions /dev/disk/by-partlabel"
else
    bad "thinfloe-save-home still references by-partlabel: $(grep -n by-partlabel "$SCRIPT")"
fi

# --------------------------------------------------------------- test 19
# GNU tar exits 1 for "file changed as we read it". A live home triggers that,
# so status 1 with a complete archive must still be a successful save.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" TF_TEST_TAR_RC=1 \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && [[ -f "$WORK/esp/home.tar.gz" ]] &&
   [[ ! -e "$WORK/esp/home.tar.gz.new" ]]; then
    ok "tar exit 1 (file changed) still saves"
else
    bad "tar exit 1 broke the save (exit $rc)"
fi

# --------------------------------------------------------------- test 20
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
echo "THE GOOD OLD ARCHIVE" >"$WORK/esp/home.tar.gz"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" TF_TEST_TAR_RC=2 \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "5" ]] && grep -q "THE GOOD OLD ARCHIVE" "$WORK/esp/home.tar.gz"; then
    ok "tar exit 2 is a failed save and keeps the old archive"
else
    bad "tar exit 2 gave exit $rc"
fi

# --------------------------------------------------------------- test 21
# The ESP directory exists but nothing is mounted on it (the initramfs creates
# it on every boot). The archive must not land in RAM and be called saved.
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
rm -rf "$WORK/esp"; mkdir -p "$WORK/esp"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "4" ]] && ! grep -q "home saved" "$WORK/out" &&
   [[ -z "$(ls -A "$WORK/esp")" ]]; then
    ok "an existing but unmounted ESP directory exits 4 and writes nothing"
else
    bad "unmounted ESP gave exit $rc, esp: $(ls -A "$WORK/esp")"
fi

# --------------------------------------------------------------- test 22
# Both branches archive as root. The netboot branch ran tar as tfuser, so one
# file in the home that tfuser cannot read (a root-owned 600 file) failed
# every netboot save with exit 5 while USB saved the same home.
setup
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && grep -q '^sudo tar ' "$WORK/sudo.log" 2>/dev/null; then
    ok "netboot save runs tar through sudo, like the USB save"
else
    bad "netboot tar did not go through sudo (exit $rc, sudo log: $(cat "$WORK/sudo.log" 2>/dev/null))"
fi
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && grep -q '^sudo tar ' "$WORK/sudo.log" 2>/dev/null; then
    ok "USB save runs tar through sudo"
else
    bad "USB tar did not go through sudo (exit $rc)"
fi

# --------------------------------------------------------------- test 23
# A netboot archive that tar could not write is exit 5 and is never uploaded.
setup
rc=$(TF_TEST_TAR_RC=2 run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "5" ]] && [[ ! -s "$WORK/curl.log" ]]; then
    ok "netboot tar exit 2 is exit 5 and uploads nothing"
else
    bad "netboot tar exit 2 gave exit $rc, curl log: $(cat "$WORK/curl.log" 2>/dev/null)"
fi

# --------------------------------------------------------------- test 24
# The netboot archive is staged in a file from mktemp under $TMPDIR, never at
# a predictable /tmp/<pid>.tar.gz that another process could create first,
# and that file is gone again on every way out of the script.
fixed_tmp=$(grep -nE '^[^#]*(/tmp/|\$\$)' "$SCRIPT")
if [[ -z "$fixed_tmp" ]]; then
    ok "thinfloe-save-home names no fixed path under /tmp and no \$\$ file name"
else
    bad "thinfloe-save-home still uses a predictable temp path: $fixed_tmp"
fi
setup
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
upload=$(grep -o 'data=@[^ ]*' "$WORK/curl.log" 2>/dev/null | head -1)
upload=${upload#data=@}
upload=${upload#\"}
upload=${upload%%;*}
if [[ "$rc" == "0" ]] && [[ "$upload" == "$WORK/tmp/"* ]] &&
   grep -q upload-file-exists "$WORK/curl.log"; then
    ok "netboot upload sends a file mktemp made under \$TMPDIR"
else
    bad "netboot upload sent '$upload' (exit $rc, curl log: $(cat "$WORK/curl.log" 2>/dev/null))"
fi
# The server sees a fixed name, not the mktemp one.
if grep -qF ';filename=home.tar.gz ' "$WORK/curl.log"; then
    ok "netboot upload names the file home.tar.gz"
else
    bad "netboot upload does not name the file home.tar.gz: $(cat "$WORK/curl.log")"
fi
if [[ -z "$(ls -A "$WORK/tmp")" ]]; then
    ok "the temporary archive is removed after a successful upload"
else
    bad "left behind after a successful upload: $(ls -A "$WORK/tmp")"
fi
setup
rc=$(TF_TEST_CURL_RC=22 run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "5" ]] && [[ -z "$(ls -A "$WORK/tmp")" ]]; then
    ok "the temporary archive is removed when the upload fails"
else
    bad "failed upload: exit $rc, left behind: $(ls -A "$WORK/tmp")"
fi
setup
rc=$(TF_TEST_TAR_RC=2 run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "5" ]] && [[ -z "$(ls -A "$WORK/tmp")" ]]; then
    ok "the temporary archive is removed when tar fails"
else
    bad "failed tar: exit $rc, left behind: $(ls -A "$WORK/tmp")"
fi

# A $TMPDIR holding ; or , still uploads the archive. curl splits an
# unquoted -F value on both, so the path it opened ended at the first one,
# curl exited 26 and nothing was saved. The " and \ check the escaping the
# quoted form needs.
setup
odd_tmp="$WORK/tmp/a;b,c\"d\\e"
mkdir -p "$odd_tmp"
rc=$(TMPDIR_OVERRIDE="$odd_tmp" run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == "0" ]] && grep -q upload-file-exists "$WORK/curl.log" &&
   [[ -z "$(ls -A "$odd_tmp")" ]]; then
    ok "netboot upload works from a \$TMPDIR holding ; , \" and \\"
else
    bad "TMPDIR '$odd_tmp': exit $rc, left behind '$(ls -A "$odd_tmp")', curl log: $(cat "$WORK/curl.log" 2>/dev/null)"
fi

# --------------------------------------------------------------- test 25
# Logout can send SIGHUP while the upload is still running. A POSIX sh killed
# by a signal skips its EXIT trap, so without `trap 'exit 5' HUP INT TERM` the
# archive - password hash and SSH host private keys - stays behind in $TMPDIR
# and the menu sees a signal status instead of 5.
#
# The script runs in its own process group (setsid) and the whole group gets
# the signal, as a terminal hangup delivers it. Every wait has a deadline.
setup
ready="$WORK/curl-ready"
rm -f "$ready"
TF_TEST_TAR_LOG="$WORK/tar.log" \
TF_TEST_CURL_LOG="$WORK/curl.log" \
TF_TEST_SUDO_LOG="$WORK/sudo.log" \
TF_TEST_CURL_BLOCK="$ready" \
HOME="$WORK/root" \
TF_HOME_DIR="$WORK/home/tfuser" \
TF_STATE_DIR="$WORK/state" \
TF_CMDLINE="$WORK/cmdline" \
TMPDIR="$WORK/tmp" \
PATH="$STUBS:$PATH" \
    setsid "$SCRIPT" </dev/null >"$WORK/out" 2>"$WORK/err" &
pid=$!
for _ in $(seq 100); do
    [[ -e "$ready" ]] && break
    sleep 0.1
done
staged=$(ls -A "$WORK/tmp")
kill -HUP -- "-$pid" 2>/dev/null
for _ in $(seq 100); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then
    kill -KILL -- "-$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    bad "SIGHUP during the upload: the script did not end within 10s"
else
    wait "$pid"
    rc=$?
    if [[ -e "$ready" ]] && [[ -n "$staged" ]] && [[ "$rc" == 5 ]] &&
       [[ -z "$(ls -A "$WORK/tmp")" ]]; then
        ok "SIGHUP during the upload exits 5 and removes the archive"
    else
        bad "SIGHUP during the upload: exit $rc, staged '$staged', left behind '$(ls -A "$WORK/tmp")', curl reached: $([[ -e "$ready" ]] && echo yes || echo no)"
    fi
fi

# ---------------------------------------------------------------- large files
# The archive carries configuration. A file over 10 MB is a download or a
# binary left in the home: it is left out and named in the output, which the
# menu shows in its log. The shipped list also leaves out ~/Downloads.
REAL_TAR=$(command -v tar)
big() { mkdir -p "$(dirname "$1")"; truncate -s "${2:-10485761}" "$1"; }
setup
echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
rm -f "$WORK/esp/home.tar.gz"
H="$WORK/home/tfuser"
big "$H/stray.iso"
big "$H/.config/exact" 10485760
big "$H/a*b"
echo small >"$H/axb"
big "$H/nl
name"
big "$H/.cache/blob"
mkdir -p "$H/Downloads" && echo small >"$H/Downloads/note.txt"
rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
     TF_EXCLUDE_DEFAULT_OVERRIDE="$REPO/overlay/etc/thinfloe/save-home-exclude" \
     TF_TEST_REAL_TAR="$REAL_TAR" \
     run_save_home "$WORK/root" "$H")
members=$("$REAL_TAR" -tzf "$WORK/esp/home.tar.gz" 2>/dev/null)
if [[ "$rc" == 0 ]] &&
   grep -qx './.thinfloe-sessionrc' <<<"$members" &&
   grep -qx './.config/exact' <<<"$members" &&
   grep -qx './axb' <<<"$members" &&
   ! grep -q 'stray.iso\|a\*b\|^./nl\|blob\|Downloads/note' <<<"$members"; then
    ok "files over 10 MB and ~/Downloads stay out of the archive"
else
    bad "large-file exclusion: exit $rc, members: $(tr '\n' ' ' <<<"$members") err: $(head -3 "$WORK/err")"
fi
if grep -q 'not saved.*: ./stray.iso$' "$WORK/out" &&
   grep -q 'not saved.*: ./a\*b$' "$WORK/out" &&
   ! grep -q 'exact\|blob' "$WORK/out"; then
    ok "each large file left out is named, excluded ones are not"
else
    bad "large-file report: $(cat "$WORK/out")"
fi

# The netboot path builds the same tar arguments.
setup
big "$WORK/home/tfuser/stray.iso"
rc=$(run_save_home "$WORK/root" "$WORK/home/tfuser")
if [[ "$rc" == 0 ]] && grep -q -- '--exclude=./stray.iso' "$WORK/tar.log"; then
    ok "a netboot save leaves large files out too"
else
    bad "netboot large-file exclusion: exit $rc, tar log: $(cat "$WORK/tar.log")"
fi

# ---------------------------------------------------------------- old names
# 4.0 keeps no links under the 3.x names: the initramfs migrates a 3.x home
# once and marks it with ~/.thinfloe-home-version, which every save carries.
names_setup() {
    setup
    echo "ro ramroot=tl/ramroot.sqsh quiet" >"$WORK/cmdline"
    mkdir -p "$WORK/esp" && : >"$WORK/esp/.test-mounted"
    rm -f "$WORK/esp/home.tar.gz"
    H="$WORK/home/tfuser"
    echo 1 >"$H/.thinfloe-home-version"
    echo env >"$H/.thinfloe-environment"
    echo hash >"$H/.thinfloe-password"
    mkdir -p "$H/.config/thinfloe/menu"
}
names_run() {
    rc=$(TF_EFI_MOUNT_OVERRIDE="$WORK/esp" \
         TF_EXCLUDE_DEFAULT_OVERRIDE="$REPO/overlay/etc/thinfloe/save-home-exclude" \
         TF_TEST_REAL_TAR="$REAL_TAR" \
         run_save_home "$WORK/root" "$H")
    listing=$("$REAL_TAR" -tzf "$WORK/esp/home.tar.gz" 2>/dev/null)
}

names_setup
names_run
if [[ "$rc" == 0 ]] && grep -qxF ./.thinfloe-home-version <<<"$listing" \
   && ! grep -q 'dbrrg' <<<"$listing" \
   && [[ ! -e "$H/.dbrrg-environment" && ! -L "$H/.dbrrg-environment" ]]; then
    ok "a save makes no old-name links and carries the home marker"
else
    bad "old names: exit $rc, listing: $listing err: $(cat "$WORK/err")"
fi

# A user pattern naming the old names applies like any other.
names_setup
echo written-by-3x >"$H/.dbrrg-environment"
printf '%s\n' './.dbrrg-*' >"$H/.save-home-exclude"
names_run
if [[ "$rc" == 0 ]] && ! grep -q 'dbrrg' <<<"$listing" \
   && grep -qxF ./.thinfloe-environment <<<"$listing" && ! grep -q 'ignored' "$WORK/out"; then
    ok "an exclude pattern over the old names applies as written"
else
    bad "old-name pattern: exit $rc, out: $(cat "$WORK/out") listing: $listing"
fi

exit $fail
