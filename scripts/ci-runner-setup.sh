#!/bin/bash
# Prepare a GitHub-hosted runner for ThinFloe's make targets.
#
# Usage: scripts/ci-runner-setup.sh artifacts|all
#
# Run after the checkout, by ci-local.yml and release-build-local.yml.
# `artifacts` does item 1 only, for a job that runs no build (make test-unit);
# `all` does items 1 to 7. Each item exists because the build assumes the
# development host; docs/release-ci.md has the measurements behind them.
# Variables for the later steps go to $GITHUB_ENV.
set -euo pipefail

if [[ $# != 1 || ($1 != artifacts && $1 != all) ]]; then
    echo "Usage: $0 artifacts|all" >&2
    exit 1
fi
MODE=$1
: "${GITHUB_ENV:?GITHUB_ENV is not set; this script runs on a GitHub runner}"
: "${RUNNER_TEMP:?RUNNER_TEMP is not set; this script runs on a GitHub runner}"

cd "$(dirname "$0")/.."

# 1. The committed artifacts symlink points at the development host's
# scratch disk and dangles here; every make call would print "mkdir: cannot
# create directory 'artifacts': File exists". skip-worktree keeps the
# replaced link out of `git status`, which would otherwise give the dev
# version .dirty.
echo "ci-runner-setup: artifacts directory"
if [[ -L artifacts ]]; then rm artifacts; fi
mkdir -p artifacts
git update-index --skip-worktree artifacts

if [[ $MODE == artifacts ]]; then
    exit 0
fi

# 2. run-qemu-write-smoke.sh and several integration tests default TMPDIR
# to the development host's scratch disk.
echo "ci-runner-setup: TMPDIR"
mkdir -p "$RUNNER_TEMP/thinfloe-tests"
echo "TMPDIR=$RUNNER_TEMP/thinfloe-tests" >>"$GITHUB_ENV"

# 3. The host tools the targets call outside containers. dnsmasq-base,
# because the full dnsmasq package starts a service on port 53.
echo "ci-runner-setup: host tools"
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    qemu-system-x86 qemu-utils ovmf mtools gdisk dnsmasq-base \
    squashfs-tools zstd dosfstools file shellcheck uidmap iproute2 \
    curl python3 openssh-client

# 4. The Containerfiles use short image names (ubuntu:26.04, ubuntu:24.04).
echo "ci-runner-setup: podman short names"
mkdir -p ~/.config/containers
printf '%s\n' 'unqualified-search-registries = ["docker.io"]' \
    'short-name-mode = "permissive"' >~/.config/containers/registries.conf

# 5. The netboot, write and time zone smoke tests run in user and network
# namespaces, which AppArmor blocks on the runner.
echo "ci-runner-setup: unprivileged user namespaces"
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0

# 6. Without a user manager podman has no systemd cgroup manager for the
# --cpu-quota of the container builds, and fails only later, in the build.
echo "ci-runner-setup: user manager"
sudo loginctl enable-linger "$(id -un)"
uid=$(id -u)
for _ in $(seq 30); do
    [[ -S /run/user/$uid/bus ]] && break
    sleep 1
done
if [[ ! -S /run/user/$uid/bus ]]; then
    echo "::error::no user bus at /run/user/$uid/bus 30 s after enable-linger; podman would lack the systemd cgroup manager"
    exit 1
fi
echo "XDG_RUNTIME_DIR=/run/user/$uid" >>"$GITHUB_ENV"
echo "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/bus" >>"$GITHUB_ENV"

# 7. runner is not in group kvm; without this QEMU falls back to TCG and
# the smoke tests time out.
echo "ci-runner-setup: /dev/kvm"
echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666"' | sudo tee /etc/udev/rules.d/99-kvm4all.rules >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --name-match=kvm
