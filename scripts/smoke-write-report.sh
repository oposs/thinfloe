#!/bin/sh
# Runs inside the smoke-test VM as kernel-command-line.service (systemd.run=,
# see scripts/run-qemu-write-smoke.sh) and writes what `thinfloe-upgrade
# drives` and `write` did to the serial console, where the host reads it.
# Lives on the stick's ESP, not in the image.
#
# Usage: smoke-write-report.sh SOURCE_URL
#
# The two target disks are USB mass storage with the serials SMOKEA and
# SMOKEB, each with a vfat file system of the same label, which
# thinfloe-automount mounts at boot. Disk B first gets a write from a release
# without the image, which must end before the first byte and mount B again,
# then a write that is cancelled after its first @progress line; disk A gets
# a complete write with --verify, after which the ThinFloe stick it is now
# must not be automounted. Last, the boot stick
# itself gets `write --mode upgrade` from SOURCE_URL/upgrade, which holds
# small stand-in release files: it proves the boot row says is_thinfloe and
# the upgrade gets past the refusals and through the switch.

SRC="${1:?usage: smoke-write-report.sh SOURCE_URL}"
CONSOLE=/dev/console
OUT=/run/smoke-write
TOOL=/usr/bin/thinfloe-upgrade
mkdir -p "$OUT"

say() { echo "$*" >"$CONSOLE"; }

LIST=/run/thinfloe/removable
HOLD=/run/thinfloe/automount/hold
# listed NAME: the mount point thinfloe-automount lists for NAME
listed() { awk -F '\t' -v n="$1" '$3 == n { print $4 }' "$LIST" 2>/dev/null; }
# listed_disk DISK: true if a line of DISK (sdb, not /dev/sdb) is listed
listed_disk() { awk -F '\t' -v d="$1" '$2 == d { f = 1 } END { exit !f }' "$LIST" 2>/dev/null; }
# wait_for SECONDS COMMAND...: true once COMMAND is
wait_for() {
    t=$1; shift
    w=0
    while [ "$w" -lt "$t" ]; do
        "$@" && return 0
        w=$((w + 1))
        sleep 1
    done
    "$@"
}
is_listed() { [ -n "$(listed "$1")" ]; }

# Both disks are mounted by the automounter before anything writes them.
wait_for 90 is_listed SMOKEA
wait_for 30 is_listed SMOKEB
say "SMOKE-WRITE-AUTOMOUNTED-A: $(listed SMOKEA)"
say "SMOKE-WRITE-AUTOMOUNTED-B: $(listed SMOKEB)"

# The network comes up after this unit starts.
i=0
while [ "$i" -lt 120 ]; do
    curl -fsI "$SRC/thinfloe-usb.img.zst" >/dev/null 2>&1 && break
    i=$((i + 1))
    sleep 1
done

# pick SERIAL: prints "<device> <id>" of the listed drive with that serial.
pick() {
    python3 -I -c '
import json, sys
for d in json.load(open(sys.argv[1])):
    if d["serial"] == sys.argv[2]:
        print(d["device"], d["id"])
        break
' "$OUT/drives.json" "$1"
}

"$TOOL" drives --source "$SRC" >"$OUT/drives.json" 2>"$OUT/drives.err"
say "SMOKE-WRITE-DRIVES-RC: $?"
read -r A_DEV A_ID <<EOF_A
$(pick SMOKEA)
EOF_A
read -r B_DEV B_ID <<EOF_B
$(pick SMOKEB)
EOF_B
read -r BOOT_DEV BOOT_ID BOOT_TF <<EOF_BOOT
$(python3 -I -c '
import json, sys
for d in json.load(open(sys.argv[1])):
    if d["is_boot"]:
        print(d["device"], d["id"], str(d["is_thinfloe"]).lower())
        break
' "$OUT/drives.json")
EOF_BOOT
say "SMOKE-WRITE-BOOT-ROW: $(python3 -I -c '
import json, sys
print(json.dumps([d for d in json.load(open(sys.argv[1])) if d["is_boot"]]))
' "$OUT/drives.json" 2>&1)"
say "SMOKE-WRITE-BOOT-DRIVE: ${BOOT_DEV:-missing}"
say "SMOKE-WRITE-BOOT-THINFLOE: ${BOOT_TF:-missing}"
say "SMOKE-WRITE-DRIVE-A: ${A_DEV:-missing}"
say "SMOKE-WRITE-DRIVE-B: ${B_DEV:-missing}"
say "SMOKE-WRITE-DRIVE-FLAGS: $(python3 -I -c '
import json, sys
for d in json.load(open(sys.argv[1])):
    if d["serial"] in ("SMOKEA", "SMOKEB"):
        print(d["serial"], "mounted=%s" % str(d["mounted"]).lower(),
              "automounted=%s" % str(d.get("automounted")).lower(), end=" ")
' "$OUT/drives.json" 2>&1)"

if [ -z "$A_DEV" ] || [ -z "$B_DEV" ]; then
    say "SMOKE-WRITE-DRIVES-BEGIN"
    cat "$OUT/drives.json" "$OUT/drives.err" >"$CONSOLE" 2>&1
    say "SMOKE-WRITE-DRIVES-END"
    say "SMOKE-WRITE-DONE"
    exit 0
fi

B_DISK=${B_DEV##*/}
A_DISK=${A_DEV##*/}

# 0. A write to B from a release without the image fails before its first
# byte: the writer released B for it and mounts it again.
"$TOOL" write --device "$B_DEV" --mode fresh --expect-id "$B_ID" \
    --source "$SRC/missing" >"$OUT/nowrite.out" 2>"$OUT/nowrite.err"
say "SMOKE-WRITE-NOWRITE-RC: $?"
say "SMOKE-WRITE-NOWRITE-RESULT: $(grep '^@result' "$OUT/nowrite.out" | tail -1)"
wait_for 30 is_listed SMOKEB
say "SMOKE-WRITE-NOWRITE-REMOUNTED: $(listed SMOKEB)"
[ -e "$HOLD/$B_DISK" ] && say "SMOKE-WRITE-NOWRITE-HOLD: left" || say "SMOKE-WRITE-NOWRITE-HOLD: gone"
say "SMOKE-WRITE-NOWRITE-STATE-BEGIN"
{
    echo "id chosen: $B_ID"
    "$TOOL" drives --source "$SRC" 2>/dev/null | python3 -I -c '
import json, sys
for d in json.load(sys.stdin):
    if d["serial"] == "SMOKEB":
        print("id now:", d["id"])
'
    tail -5 "$OUT/nowrite.err"
    systemctl is-active "thinfloe-automount@$B_DISK.service"
    journalctl -b -o cat -t thinfloe-automount --no-pager | tail -15
    cat "$LIST"
} >"$CONSOLE" 2>&1
say "SMOKE-WRITE-NOWRITE-STATE-END"

# 1. Cancel a fresh write to B after its first @progress line.
"$TOOL" write --device "$B_DEV" --mode fresh --expect-id "$B_ID" \
    --source "$SRC" >"$OUT/cancel.out" 2>"$OUT/cancel.err" &
pid=$!
# Wait for a @progress line past 4 MiB: the tool emits it after os.write()
# returned, and a read of the block device goes through the same page cache,
# so the first MiB is then readable (no O_DIRECT or fdatasync needed).
progress_done() { grep '^@progress' "$OUT/cancel.out" | tail -1 | cut -d' ' -f2; }
i=0
while [ "$i" -lt 300 ] && kill -0 "$pid" 2>/dev/null; do
    d=$(progress_done)
    [ -n "$d" ] && [ "$d" -ge 4194304 ] 2>/dev/null && break
    i=$((i + 1))
    sleep 1
done
d=$(progress_done)
[ -n "$d" ] && [ "$d" -ge 4194304 ] 2>/dev/null && say "SMOKE-WRITE-CANCEL-SAW-PROGRESS: yes"
# While it writes, B is released (no line) and held (the automounter keeps off).
listed_disk "$B_DISK" && say "SMOKE-WRITE-CANCEL-RELEASED: no" || say "SMOKE-WRITE-CANCEL-RELEASED: yes"
[ -e "$HOLD/$B_DISK" ] && say "SMOKE-WRITE-CANCEL-HELD: yes" || say "SMOKE-WRITE-CANCEL-HELD: no"
# Non-zero bytes in the first MiB of B while it is being written: proves that
# the zero check after the cancel sees a wipe, not a stick never written.
say "SMOKE-WRITE-CANCEL-BEFORE-NONZERO: $(head -c 1048576 "$B_DEV" | tr -d '\0' | wc -c)"
kill -TERM "$pid" 2>/dev/null
wait "$pid"
say "SMOKE-WRITE-CANCEL-RC: $?"
say "SMOKE-WRITE-CANCEL-RESULT: $(grep '^@result' "$OUT/cancel.out" | tail -1)"
# Bytes that are not zero in the first MiB of B; the cancel wipes the stick.
say "SMOKE-WRITE-CANCEL-NONZERO: $(head -c 1048576 "$B_DEV" | tr -d '\0' | wc -c)"
[ -e "$HOLD/$B_DISK" ] && say "SMOKE-WRITE-CANCEL-HOLD: left" || say "SMOKE-WRITE-CANCEL-HOLD: gone"

# 2. Write A completely and read it back.
"$TOOL" write --device "$A_DEV" --mode fresh --expect-id "$A_ID" \
    --source "$SRC" --verify >"$OUT/write.out" 2>"$OUT/write.err"
say "SMOKE-WRITE-WRITE-RC: $?"
say "SMOKE-WRITE-WRITE-RESULT: $(grep '^@result' "$OUT/write.out" | tail -1)"
say "SMOKE-WRITE-WRITE-PROGRESS-LINES: $(grep -c '^@progress' "$OUT/write.out")"
say "SMOKE-WRITE-WRITE-STEPS: $(grep '^@step' "$OUT/write.out" | cut -d' ' -f2 | tr '\n' ' ')"
say "SMOKE-WRITE-WRITE-TAIL-BEGIN"
grep -v '^@progress' "$OUT/write.out" | tail -15 >"$CONSOLE"
tail -15 "$OUT/write.err" >"$CONSOLE"
say "SMOKE-WRITE-WRITE-TAIL-END"
# The stick is a ThinFloe stick now (partition EFI-SYSTEM): never automounted.
udevadm settle --timeout=30
sleep 5
listed_disk "$A_DISK" && say "SMOKE-WRITE-WRITTEN-AUTOMOUNTED: yes" || say "SMOKE-WRITE-WRITTEN-AUTOMOUNTED: no"

# 3. Upgrade the boot stick from the stand-in release.
if [ -n "$BOOT_DEV" ]; then
    "$TOOL" write --device "$BOOT_DEV" --mode upgrade --expect-id "$BOOT_ID" \
        --source "$SRC/upgrade" >"$OUT/upgrade.out" 2>"$OUT/upgrade.err"
    say "SMOKE-WRITE-UPGRADE-RC: $?"
    say "SMOKE-WRITE-UPGRADE-RESULT: $(grep '^@result' "$OUT/upgrade.out" | tail -1)"
    say "SMOKE-WRITE-UPGRADE-STEPS: $(grep '^@step' "$OUT/upgrade.out" | cut -d' ' -f2 | tr '\n' ' ')"
    say "SMOKE-WRITE-UPGRADE-TAIL-BEGIN"
    grep -v '^@progress' "$OUT/upgrade.out" | tail -15 >"$CONSOLE"
    tail -15 "$OUT/upgrade.err" >"$CONSOLE"
    say "SMOKE-WRITE-UPGRADE-TAIL-END"
fi
say "SMOKE-WRITE-DONE"
