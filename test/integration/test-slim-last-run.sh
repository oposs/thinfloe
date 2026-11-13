#!/bin/bash
# The slim step must stay the last RUN of the ubuntu Dockerfile, directly
# after the dracut RUN, and no apt-get, dpkg, dracut or update-initramfs may
# run after dracut built the initrd: a dpkg run can fire the
# update-initramfs trigger, which rebuilds the initrd with default flags
# (spec 2026-11-12-image-size, "Why a separate step, and why last").
# Static: needs no built image.
#
# Usage: test/integration/test-slim-last-run.sh [path/to/Dockerfile]

set -uo pipefail

DOCKERFILE="${1:-containers/ubuntu/Dockerfile}"

if [[ ! -f "$DOCKERFILE" ]]; then
    echo "FAIL: $DOCKERFILE not found" >&2
    exit 1
fi

fail=0

# The last instruction line that starts with RUN (continuation lines are
# indented and do not count).
last=$(grep -nE '^[Rr][Uu][Nn][[:space:]]' "$DOCKERFILE" | tail -n 1)
if [[ $last == *'/thinfloe-slim/slim-rootfs.sh'* ]]; then
    echo "ok   - the last RUN of $DOCKERFILE runs /thinfloe-slim/slim-rootfs.sh"
else
    echo "FAIL - the last RUN of $DOCKERFILE runs /thinfloe-slim/slim-rootfs.sh (found: ${last:-no RUN line})"
    fail=1
fi

# From the line that runs dracut on, no line (continuations included, and
# comments left out) runs apt-get or dpkg.
after=$(awk '/^[[:space:]]*dracut --force/ {on=1} on && !/^[[:space:]]*#/ && /(apt-get|dpkg)/ {print NR ": " $0}' "$DOCKERFILE")
if ! grep -qE '^[[:space:]]*dracut --force' "$DOCKERFILE"; then
    echo "FAIL - no apt-get or dpkg after the dracut RUN (no line runs dracut --force)"
    fail=1
elif [[ -z $after ]]; then
    echo "ok   - no apt-get or dpkg after the dracut RUN"
else
    echo "FAIL - no apt-get or dpkg after the dracut RUN (found: $(head -n 1 <<<"$after"))"
    fail=1
fi

# The dracut RUN ends at the first line from `dracut --force` on that has no
# continuation backslash. After it, no line (comments left out) calls dracut
# or update-initramfs, which would replace the initrd the slim step checks.
end=$(awk '/^[[:space:]]*dracut --force/ {on=1} on && !/\\$/ {print NR; exit}' "$DOCKERFILE")
if [[ -z $end ]]; then
    echo "FAIL - no dracut or update-initramfs after the dracut RUN (no line runs dracut --force)"
    echo "FAIL - no RUN between the dracut RUN and the slim RUN (no line runs dracut --force)"
    fail=1
else
    again=$(awk -v end="$end" 'NR > end && !/^[[:space:]]*#/ &&
        /(^|[^[:alnum:]_.-])(dracut|update-initramfs)([^[:alnum:]_.-]|$)/ {print NR ": " $0}' "$DOCKERFILE")
    if [[ -z $again ]]; then
        echo "ok   - no dracut or update-initramfs after the dracut RUN"
    else
        echo "FAIL - no dracut or update-initramfs after the dracut RUN (found: $(head -n 1 <<<"$again"))"
        fail=1
    fi
    # Any RUN after the dracut RUN other than the slim RUN itself.
    between=$(awk -v end="$end" 'NR > end && /^[Rr][Uu][Nn][[:space:]]/ &&
        !/\/thinfloe-slim\/slim-rootfs\.sh/ {print NR ": " $0}' "$DOCKERFILE")
    if [[ -z $between ]]; then
        echo "ok   - no RUN between the dracut RUN and the slim RUN"
    else
        echo "FAIL - no RUN between the dracut RUN and the slim RUN (found: $(head -n 1 <<<"$between"))"
        fail=1
    fi
fi

if [[ $fail -eq 0 ]]; then
    echo "PASSED"
else
    echo "FAILED"
    exit 1
fi
