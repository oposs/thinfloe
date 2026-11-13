#!/bin/bash
# The slim step must stay the last RUN of the ubuntu Dockerfile: a dpkg run
# after it can fire the update-initramfs trigger, which rebuilds the initrd
# with default flags from the slimmed tree (spec 2026-11-12-image-size,
# "Why a separate step, and why last"). Static: needs no built image.
#
# Usage: test/integration/test-slim-last-run.sh [path/to/Dockerfile]

set -uo pipefail

DOCKERFILE="${1:-containers/ubuntu/Dockerfile}"

if [[ ! -f "$DOCKERFILE" ]]; then
    echo "FAIL: $DOCKERFILE not found" >&2
    exit 1
fi

# The last instruction line that starts with RUN (continuation lines are
# indented and do not count).
last=$(grep -nE '^[Rr][Uu][Nn][[:space:]]' "$DOCKERFILE" | tail -n 1)
if [[ $last == *'/thinfloe-slim/slim-rootfs.sh'* ]]; then
    echo "ok   - the last RUN of $DOCKERFILE runs /thinfloe-slim/slim-rootfs.sh"
    echo "PASSED"
    exit 0
fi
echo "FAIL - the last RUN of $DOCKERFILE runs /thinfloe-slim/slim-rootfs.sh (found: ${last:-no RUN line})"
echo "FAILED"
exit 1
