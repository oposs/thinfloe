#!/bin/sh
# Runs inside the smoke-test VM as kernel-command-line.service (systemd.run=,
# see scripts/run-qemu-write-smoke.sh) and writes what `thinfloe-upgrade
# drives` and `write` did to the serial console, where the host reads it.
# Lives on the stick's ESP, not in the image.
#
# Usage: smoke-write-report.sh SOURCE_URL
#
# The two target disks are USB mass storage with the serials SMOKEA and
# SMOKEB. Disk B gets a write that is cancelled after its first @progress
# line; disk A gets a complete write with --verify. Last, the boot stick
# itself gets `write --mode upgrade` from SOURCE_URL/upgrade, which holds
# small stand-in release files: it proves the boot row says is_thinfloe and
# the upgrade gets past the refusals and through the switch.

SRC="${1:?usage: smoke-write-report.sh SOURCE_URL}"
CONSOLE=/dev/console
OUT=/run/smoke-write
TOOL=/usr/bin/thinfloe-upgrade
mkdir -p "$OUT"

say() { echo "$*" >"$CONSOLE"; }

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

if [ -z "$A_DEV" ] || [ -z "$B_DEV" ]; then
    say "SMOKE-WRITE-DRIVES-BEGIN"
    cat "$OUT/drives.json" "$OUT/drives.err" >"$CONSOLE" 2>&1
    say "SMOKE-WRITE-DRIVES-END"
    say "SMOKE-WRITE-DONE"
    exit 0
fi

# 1. Cancel a fresh write to B after its first @progress line.
"$TOOL" write --device "$B_DEV" --mode fresh --expect-id="$B_ID" \
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
# Non-zero bytes in the first MiB of B while it is being written: proves that
# the zero check after the cancel sees a wipe, not a stick never written.
say "SMOKE-WRITE-CANCEL-BEFORE-NONZERO: $(head -c 1048576 "$B_DEV" | tr -d '\0' | wc -c)"
kill -TERM "$pid" 2>/dev/null
wait "$pid"
say "SMOKE-WRITE-CANCEL-RC: $?"
say "SMOKE-WRITE-CANCEL-RESULT: $(grep '^@result' "$OUT/cancel.out" | tail -1)"
# Bytes that are not zero in the first MiB of B; the cancel wipes the stick.
say "SMOKE-WRITE-CANCEL-NONZERO: $(head -c 1048576 "$B_DEV" | tr -d '\0' | wc -c)"

# 2. Write A completely and read it back.
"$TOOL" write --device "$A_DEV" --mode fresh --expect-id="$A_ID" \
    --source "$SRC" --verify >"$OUT/write.out" 2>"$OUT/write.err"
say "SMOKE-WRITE-WRITE-RC: $?"
say "SMOKE-WRITE-WRITE-RESULT: $(grep '^@result' "$OUT/write.out" | tail -1)"
say "SMOKE-WRITE-WRITE-PROGRESS-LINES: $(grep -c '^@progress' "$OUT/write.out")"
say "SMOKE-WRITE-WRITE-STEPS: $(grep '^@step' "$OUT/write.out" | cut -d' ' -f2 | tr '\n' ' ')"
say "SMOKE-WRITE-WRITE-TAIL-BEGIN"
grep -v '^@progress' "$OUT/write.out" | tail -15 >"$CONSOLE"
tail -15 "$OUT/write.err" >"$CONSOLE"
say "SMOKE-WRITE-WRITE-TAIL-END"

# 3. Upgrade the boot stick from the stand-in release.
if [ -n "$BOOT_DEV" ]; then
    "$TOOL" write --device "$BOOT_DEV" --mode upgrade --expect-id="$BOOT_ID" \
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
