#!/bin/bash
# Offline tests for thinfloe-ssh-hostkeys.
#
# Host keys persist inside the home directory, which is already saved to the
# EFI partition or the boot server. That removes the need for a separate EFI
# file or a hostkeys.pkg endpoint, but it means the keys arrive tluser-owned
# and must be forced to root:root 0600 before sshd will use them - the exact
# opposite of the home restore, which must PRESERVE uid 1000.
#
# Runs without root: chown is stubbed and its invocations are recorded, so
# the ownership fix can be asserted without actually being able to perform
# it. Directory layout is redirected via TF_SSH_DIR / TF_HOME_DIR.
#
# Usage: test/integration/test-ssh-hostkeys.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$REPO/overlay/usr/bin/thinfloe-ssh-hostkeys"

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

# chown cannot work as an unprivileged user. Record the calls instead so the
# ownership fix is still assertable.
cat >"$STUBS/chown" <<'STUB'
#!/bin/bash
echo "chown $*" >>"$TF_TEST_CHOWN_LOG"
exit 0
STUB
chmod +x "$STUBS/chown"

# ssh-keygen -A normally writes into /etc/ssh; honour TF_SSH_DIR instead.
# Real ssh-keygen -A only creates key types that are missing and leaves any
# existing key alone - skip existing files here too, so the stub can stand
# in for the top-up path without clobbering a key it should be preserving.
cat >"$STUBS/ssh-keygen" <<'STUB'
#!/bin/bash
echo "ssh-keygen $*" >>"$TF_TEST_KEYGEN_LOG"
for t in rsa ecdsa ed25519; do
    [ -e "$TF_SSH_DIR/ssh_host_${t}_key" ] && continue
    echo "PRIVATE-$t" >"$TF_SSH_DIR/ssh_host_${t}_key"
    echo "PUBLIC-$t"  >"$TF_SSH_DIR/ssh_host_${t}_key.pub"
done
exit 0
STUB
chmod +x "$STUBS/ssh-keygen"

export PATH="$STUBS:$PATH"

# fresh_case <name> -> sets TF_SSH_DIR, TF_HOME_DIR, logs
fresh_case() {
    CASE="$WORK/$1"
    export TF_SSH_DIR="$CASE/etc-ssh"
    export TF_HOME_DIR="$CASE/home"
    export TF_TEST_CHOWN_LOG="$CASE/chown.log"
    export TF_TEST_KEYGEN_LOG="$CASE/keygen.log"
    mkdir -p "$TF_SSH_DIR" "$TF_HOME_DIR"
    : >"$TF_TEST_CHOWN_LOG"
    : >"$TF_TEST_KEYGEN_LOG"
}

# --- keys present in the home keystore -> installed into /etc/ssh --------
#
# This keystore deliberately carries only ed25519 - a PARTIAL keystore, the
# same shape as a truncated home archive, a hand-edited keystore, or one
# written before a new key type existed. It doubles as the top-up test: the
# install path now always runs ssh-keygen -A afterwards (see
# thinfloe-ssh-hostkeys), so this case proves both that the missing types get
# generated and that the type which WAS present is left untouched.

fresh_case restore
mkdir -p "$TF_HOME_DIR/.thinfloe-ssh-host-keys"
echo "PRIVATE-ed25519" >"$TF_HOME_DIR/.thinfloe-ssh-host-keys/ssh_host_ed25519_key"
echo "PUBLIC-ed25519"  >"$TF_HOME_DIR/.thinfloe-ssh-host-keys/ssh_host_ed25519_key.pub"
chmod 644 "$TF_HOME_DIR/.thinfloe-ssh-host-keys/ssh_host_ed25519_key"

"$SCRIPT" >/dev/null 2>&1

if [[ -f "$TF_SSH_DIR/ssh_host_ed25519_key" ]]; then
    ok "keystore keys are installed into the ssh dir"
else
    bad "keystore keys were not installed"
fi

# The top-up call must not clobber the key that was already there - this is
# what proves ssh-keygen -A "only creates missing types" isn't just assumed.
if [[ "$(cat "$TF_SSH_DIR/ssh_host_ed25519_key" 2>/dev/null)" == "PRIVATE-ed25519" ]]; then
    ok "installed key keeps its original keystore content after top-up"
else
    bad "installed ed25519 key content changed - top-up must have overwritten it"
fi

if grep -q -- "-A" "$TF_TEST_KEYGEN_LOG"; then
    ok "ssh-keygen -A runs to top up a partial keystore"
else
    bad "ssh-keygen -A did not run to top up the partial keystore"
fi

if [[ -f "$TF_SSH_DIR/ssh_host_rsa_key" && -f "$TF_SSH_DIR/ssh_host_ecdsa_key" ]]; then
    ok "missing key types (rsa, ecdsa) are topped up in the ssh dir"
else
    bad "missing key types were not topped up - sshd would run with fewer key types"
fi

if [[ -f "$TF_HOME_DIR/.thinfloe-ssh-host-keys/ssh_host_rsa_key" ]]; then
    ok "topped-up key types are restaged into the keystore"
else
    bad "topped-up key types were not restaged - they would not persist"
fi

# sshd refuses to start on a private key that is not 0600.
perm=$(stat -c %a "$TF_SSH_DIR/ssh_host_ed25519_key")
if [[ "$perm" == "600" ]]; then
    ok "installed private key is chmod 600"
else
    bad "installed private key is $perm, expected 600 (it was 644 in the archive)"
fi

perm=$(stat -c %a "$TF_SSH_DIR/ssh_host_ed25519_key.pub")
if [[ "$perm" == "644" ]]; then
    ok "installed public key is chmod 644"
else
    bad "installed public key is $perm, expected 644"
fi

if grep -q "chown root:root" "$TF_TEST_CHOWN_LOG"; then
    ok "installed keys are chowned to root:root"
else
    bad "no chown to root:root - keys arrive tluser-owned and sshd will refuse them"
fi

# --- nothing anywhere -> generate, then stage into the keystore ----------

fresh_case generate
"$SCRIPT" >/dev/null 2>&1

if grep -q -- "-A" "$TF_TEST_KEYGEN_LOG"; then
    ok "ssh-keygen -A runs when no keys exist anywhere"
else
    bad "ssh-keygen -A did not run with no keys present"
fi

if [[ -f "$TF_HOME_DIR/.thinfloe-ssh-host-keys/ssh_host_ed25519_key" ]]; then
    ok "generated keys are staged into the home keystore"
else
    bad "generated keys were not staged into the keystore - they would not persist"
fi

if grep -q "chown -R tluser:tluser" "$TF_TEST_CHOWN_LOG"; then
    ok "staged keystore is chowned back to tluser"
else
    bad "staged keystore not chowned to tluser - save-home could not read it"
fi

# 3.x reads ~/.dbrrg-ssh-host-keys. The link is made when the keystore is
# written, so a home saved before the next boot's initramfs runs still lets a
# fallback boot of 3.x find the keys. It belongs to tluser, like the keystore.
OLD="$TF_HOME_DIR/.dbrrg-ssh-host-keys"
if [[ -L "$OLD" ]] && [[ "$(readlink "$OLD")" == ".thinfloe-ssh-host-keys" ]] &&
   grep -q "^chown -h tluser:tluser $OLD\$" "$TF_TEST_CHOWN_LOG"; then
    ok "staging links ~/.dbrrg-ssh-host-keys to the keystore, owned by tluser"
else
    bad "staging links ~/.dbrrg-ssh-host-keys to the keystore, owned by tluser (link '$(readlink "$OLD" 2>/dev/null)', chown: $(grep '^chown -h' "$TF_TEST_CHOWN_LOG" | head -1))"
fi

# --- keys already live, keystore empty -> do nothing ---------------------

fresh_case already
echo "EXISTING" >"$TF_SSH_DIR/ssh_host_ed25519_key"
"$SCRIPT" >/dev/null 2>&1

if [[ "$(cat "$TF_SSH_DIR/ssh_host_ed25519_key")" == "EXISTING" ]]; then
    ok "existing live keys are left untouched"
else
    bad "existing live keys were overwritten"
fi

if [[ ! -s "$TF_TEST_KEYGEN_LOG" ]]; then
    ok "no regeneration when live keys already exist"
else
    bad "ssh-keygen ran despite live keys existing"
fi

# --- --stage copies live keys into the keystore --------------------------

fresh_case stage
echo "LIVE" >"$TF_SSH_DIR/ssh_host_ed25519_key"
echo "LIVEPUB" >"$TF_SSH_DIR/ssh_host_ed25519_key.pub"
"$SCRIPT" --stage >/dev/null 2>&1

if [[ "$(cat "$TF_HOME_DIR/.thinfloe-ssh-host-keys/ssh_host_ed25519_key" 2>/dev/null)" == "LIVE" ]]; then
    ok "--stage copies the live keys into the keystore"
else
    bad "--stage did not copy the live keys into the keystore"
fi

if [[ -L "$TF_HOME_DIR/.dbrrg-ssh-host-keys" ]] &&
   [[ "$(readlink "$TF_HOME_DIR/.dbrrg-ssh-host-keys")" == ".thinfloe-ssh-host-keys" ]]; then
    ok "--stage links ~/.dbrrg-ssh-host-keys to the keystore"
else
    bad "--stage links ~/.dbrrg-ssh-host-keys to the keystore"
fi

# A link already in place (every boot after the first 4.0 one) is kept as a
# link, not turned into a directory or a link inside the keystore.
"$SCRIPT" --stage >/dev/null 2>&1
if [[ -L "$TF_HOME_DIR/.dbrrg-ssh-host-keys" ]] &&
   [[ ! -e "$TF_HOME_DIR/.thinfloe-ssh-host-keys/.thinfloe-ssh-host-keys" ]]; then
    ok "a second --stage leaves the link as it was"
else
    bad "a second --stage leaves the link as it was"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - SSH host key persistence"
    exit 1
fi

echo ""
echo "PASSED - SSH host key persistence"
exit 0
