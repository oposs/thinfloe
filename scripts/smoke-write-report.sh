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
# line; disk A gets a complete write with --verify.

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
say "SMOKE-WRITE-DONE"
