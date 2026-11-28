#!/bin/sh
# Runs inside the automount smoke VM as kernel-command-line.service
# (systemd.run=, see scripts/run-qemu-automount-smoke.sh) and writes ok/FAIL
# lines on what thinfloe-automount did to the serial console, where the host
# reads them. Lives on the stick's ESP, not in the image.
#
# Attached: the boot disk (virtio), a USB stick with serial SMOKEU (one vfat
# partition labelled SMOKEU, holding hello.txt) and a two-slot USB card
# reader: slot 0 holds a card labelled CARD1 (vfat, no partition table, with
# one.txt), slot 1 a card without a file system. The host swaps the card in
# slot 0 through the QEMU monitor: it takes the card out after
# SMOKE-AUTOMOUNT-SWAP-OUT and puts CARD2 (two.txt) in after
# SMOKE-AUTOMOUNT-SWAP-IN. A second stick (serial SMOKES, vfat labelled
# FLOEDROP) must not land on FloeDrop's tmpfs at $MEDIA/FloeDrop.

CONSOLE=/dev/console
HELPER=/usr/libexec/thinfloe/thinfloe-automount
LIST=/run/thinfloe/removable
EVENT=/run/thinfloe/removable.event
MEDIA=/media/tfuser

say() { echo "$*" >"$CONSOLE"; }
ok()  { say "SMOKE-AUTOMOUNT: ok   - $1"; }
bad() { say "SMOKE-AUTOMOUNT: FAIL - $1"; }
# check DESCRIPTION TEST: TEST is shell code, run with eval
check() {
    if eval "$2"; then ok "$1"; else bad "$1"; fi
}

# line NAME: the contract line whose name field is NAME
line() { awk -F '\t' -v n="$1" '$3 == n' "$LIST" 2>/dev/null; }
field() { printf '%s\n' "$1" | cut -f "$2"; }
# wait_for SECONDS COMMAND...: true once COMMAND is
wait_for() {
    t=$1; shift
    i=0
    while [ "$i" -lt "$t" ]; do
        "$@" && return 0
        i=$((i + 1))
        sleep 1
    done
    "$@"
}
has_line() { [ -n "$(line "$1")" ]; }
no_line() { [ -z "$(line "$1")" ]; }
# event_is NAME STATE: the last event names NAME and STATE
event_is() { [ "$(cut -f 2- "$EVENT" 2>/dev/null)" = "$(printf '%s\t%s' "$1" "$2")" ]; }
# event_is_batch NAMES1 NAMES2 STATE COUNT CARDS: the last event is a batch
# line naming NAMES1 or NAMES2 (the order of the disks), STATE, COUNT and
# how many of them are cards
event_is_batch() {
    got=$(cut -f 2- "$EVENT" 2>/dev/null)
    [ "$got" = "$(printf '%s\t%s\t%s\t%s' "$1" "$3" "$4" "$5")" ] ||
        [ "$got" = "$(printf '%s\t%s\t%s\t%s' "$2" "$3" "$4" "$5")" ]
}
show_state() {
    say "SMOKE-AUTOMOUNT-STATE-BEGIN"
    cat "$LIST" "$EVENT" >"$CONSOLE" 2>&1
    findmnt -rn -o SOURCE,TARGET >"$CONSOLE" 2>&1
    lsblk -o NAME,SIZE,TRAN,RM,FSTYPE,LABEL,MOUNTPOINTS >"$CONSOLE" 2>&1
    journalctl -b -o cat -t thinfloe-automount --no-pager 2>/dev/null | tail -20 >"$CONSOLE"
    say "SMOKE-AUTOMOUNT-STATE-END"
}

# --- the stick --------------------------------------------------------------
if ! wait_for 90 has_line SMOKEU || ! wait_for 30 has_line CARD1; then
    bad "SMOKEU and CARD1 were automounted within 120 s"
    show_state
fi
U=$(line SMOKEU)
U_DEV=$(field "$U" 1)
U_DISK=$(field "$U" 2)
check "the stick is listed as SMOKEU at $MEDIA/SMOKEU, kind stick ($U_DEV)" \
    '[ "$(field "$U" 4)" = "$MEDIA/SMOKEU" ] && [ "$(field "$U" 8)" = stick ]'
check "$MEDIA/SMOKEU/hello.txt is readable" \
    '[ "$(cat "$MEDIA/SMOKEU/hello.txt" 2>/dev/null)" = "hello from SMOKEU" ]'
check "hello.txt belongs to tfuser" \
    '[ "$(stat -c %U "$MEDIA/SMOKEU/hello.txt" 2>/dev/null)" = tfuser ]'
check "$MEDIA is owned by root, mode 755" \
    '[ "$(stat -c %U:%a "$MEDIA" 2>/dev/null)" = root:755 ]'
check "/run/media/tfuser leads to $MEDIA" \
    '[ "$(readlink -f /run/media/tfuser)" = "$MEDIA" ]'
check "tfuser can write to the stick" \
    'sudo -u tfuser sh -c "echo written >$MEDIA/SMOKEU/by-tfuser.txt"'

# The boot disk: not listed, nothing of it mounted under the mount root.
boot_mounts=$(findmnt -rn -o SOURCE,TARGET | awk -v m="$MEDIA/" 'index($2, m) == 1 && $1 ~ "^/dev/vd"')
check "nothing of the boot disk is listed or mounted under $MEDIA" \
    '[ -z "$boot_mounts" ] && ! cut -f 2 "$LIST" | grep -q "^vd"'

# --- the card reader --------------------------------------------------------
C=$(line CARD1)
C_DISK=$(field "$C" 2)
check "the card in the reader is listed as CARD1, kind card ($(field "$C" 1))" \
    '[ "$(field "$C" 4)" = "$MEDIA/CARD1" ] && [ "$(field "$C" 8)" = card ]'
check "$MEDIA/CARD1/one.txt is readable" \
    '[ "$(cat "$MEDIA/CARD1/one.txt" 2>/dev/null)" = "card one" ]'
check "the reader's slot without a file system is not listed" \
    '[ -z "$(cut -f 3 "$LIST" | grep -vxE "SMOKEU|CARD1|FLOEDROP1")" ]'

# --- FloeDrop's folder ------------------------------------------------------
SHARE=$MEDIA/FloeDrop
if ! wait_for 30 has_line FLOEDROP1; then
    bad "the stick labelled FLOEDROP was automounted as FLOEDROP1 within 30 s"
    show_state
fi
check "$SHARE is a tmpfs owned by tfuser" \
    '[ "$(findmnt -rn -o FSTYPE "$SHARE")" = tmpfs ] && [ "$(stat -c %U "$SHARE" 2>/dev/null)" = tfuser ]'
check "$SHARE is not in $LIST" \
    '! cut -f 4 "$LIST" | grep -qxF "$SHARE"'
S=$(line FLOEDROP1)
check "the stick labelled FLOEDROP is at $MEDIA/FLOEDROP1, kind stick" \
    '[ "$(field "$S" 4)" = "$MEDIA/FLOEDROP1" ] && [ "$(field "$S" 8)" = stick ] && [ "$(cat "$MEDIA/FLOEDROP1/share.txt" 2>/dev/null)" = "not the folder" ]'
check "only the tmpfs is mounted at $SHARE" \
    '[ "$(findmnt -rn -o SOURCE "$SHARE" | wc -l)" -eq 1 ] && [ ! -e "$SHARE/share.txt" ]'

say "SMOKE-AUTOMOUNT-SWAP-OUT"
check "the card taken out of the reader loses its line" 'wait_for 60 no_line CARD1'
check "its mount point is gone" '[ ! -e "$MEDIA/CARD1" ]'
check "the event file says CARD1 went unclean ($(cut -f 2- "$EVENT" 2>/dev/null | tr '\t' ' '))" \
    'event_is CARD1 unclean'

say "SMOKE-AUTOMOUNT-SWAP-IN"
if wait_for 60 has_line CARD2; then
    ok "the next card in the same slot is automounted as CARD2"
else
    bad "the next card in the same slot is automounted as CARD2"
    show_state
fi
C2=$(line CARD2)
check "CARD2 is on the reader's disk $C_DISK, kind card" \
    '[ "$(field "$C2" 2)" = "$C_DISK" ] && [ "$(field "$C2" 8)" = card ]'
check "$MEDIA/CARD2/two.txt is readable" \
    '[ "$(cat "$MEDIA/CARD2/two.txt" 2>/dev/null)" = "card two" ]'

# --- eject ------------------------------------------------------------------
"$HELPER" eject "$C_DISK"
rc=$?
check "eject of the card exits 0 (exit $rc)" '[ "$rc" -eq 0 ]'
check "the ejected card loses its line and mount point" \
    'no_line CARD2 && [ ! -e "$MEDIA/CARD2" ]'
check "the event file says CARD2 went clean" 'event_is CARD2 clean'
sleep 3
check "the card reader stays powered ($C_DISK still there)" \
    '[ -n "$C_DISK" ] && [ -e "/sys/class/block/$C_DISK" ]'

# eject all with two sticks mounted (SMOKEU and FLOEDROP1): one event for
# the batch, both names, the count and no card, in the order of their disks.
check "two sticks are mounted before eject all" 'has_line SMOKEU && has_line FLOEDROP1'
"$HELPER" eject all
rc=$?
check "eject all exits 0 and leaves no line (exit $rc)" '[ "$rc" -eq 0 ] && [ ! -s "$LIST" ]'
check "the ejected stick's mount is gone" \
    '! findmnt -rn "$MEDIA/SMOKEU" >/dev/null && [ ! -e "$MEDIA/SMOKEU" ]'
check "the event file says both sticks went clean, count 2, no card ($(cut -f 2- "$EVENT" 2>/dev/null | tr '\t' ' '))" \
    'event_is_batch "SMOKEU, FLOEDROP1" "FLOEDROP1, SMOKEU" clean 2 0'
check "the stick is powered off ($U_DISK gone)" \
    '[ -n "$U_DISK" ] && wait_for 10 [ ! -e "/sys/class/block/$U_DISK" ]'
check "eject all leaves FloeDrop's tmpfs mounted at $SHARE" \
    '[ "$(findmnt -rn -o FSTYPE "$SHARE")" = tmpfs ]'

show_state
say "SMOKE-AUTOMOUNT-DONE"
