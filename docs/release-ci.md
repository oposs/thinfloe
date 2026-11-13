# Release CI on GitHub-hosted runners

The Phase 0 spike of the GitHub release plan ran the whole build on a
GitHub-hosted runner: run 43188126430 on `oposs/thinfloe`, workflow
`spike-runner.yml` on branch `spike/runner` (never merged), 2026-11-12. Every
target passed; the job took 25 minutes from checkout to the last step. This
file records the numbers, the CI split that follows from them, and the setup
the project's own workflows need on the runner.

## Runner

- Image: `ubuntu-24.04`, version 20261101.360.1.
- Cores: 4 (`nproc`), matching `BUILD_JOBS=4`.
- Memory: 15 GiB, 2 GiB swap.
- Disk: `/` is 145 GiB with 59 GiB used at start. `/mnt` is the same file
  system.
- podman 4.9.3, rootless, `overlay` storage in
  `/home/runner/.local/share/containers/storage`.
- `/etc/subuid` and `/etc/subgid` give `runner` 165536:65536, so
  `podman unshare` works.

## Measurements

The build was cold: no cache, no registry. The make targets ran in this
order, each with `MEMCAP=`.

| Target | Wall time | Result |
|---|---|---|
| `make rootfs` (three container builds, rootfs export) | 8m14s | ok |
| `make test` (746 Python tests, 814 menu tests passed, 4 ignored, all integration guards) | 7m17s | ok |
| `make image` | 12s | ok |
| `make qemu-smoke` | 31s | PASSED |
| `make qemu-smoke-netboot` | 25s | PASSED |
| `make qemu-smoke-upgrade` | 31s | PASSED |
| `make qemu-smoke-write` | 1m13s | PASSED |
| `make qemu-smoke-grow` | 1m00s | PASSED |
| `make qemu-smoke-two-sticks` | 1m18s | PASSED |
| `make qemu-smoke-addons` | 28s | PASSED |
| `make qemu-smoke-timezone` | 31s | PASSED |
| `make qemu-smoke-timezone-netboot` | 29s | PASSED |
| `make test-runtime` | 2m41s | ok |

The nine smoke tests together take 6m26s. Runner setup (packages,
registries, linger) takes 20 seconds.

Disk, sampled every 15 seconds on `/`:

- Start: 59 GiB used, 86 GiB free.
- Peak: 73765 MiB (72 GiB) used at 09:22:30, during `qemu-smoke-write`. The
  build needs about 13 GiB at most; 73 GiB stay free.
- End: 71 GiB used. `artifacts/` holds 5.8 GiB, podman storage 4.2 GiB
  (52 images, all reclaimable).

No preinstalled toolchain has to be removed.

KVM: `/dev/kvm` exists as `crw-rw---- root kvm`, and `runner` is not in group
`kvm`. With the udev rule `KERNEL=="kvm", GROUP="kvm", MODE="0666"` it becomes
`crw-rw-rw-` and every smoke test boots with `accel=kvm`.

## Release downloads

Probed against the baseline release v4.2.0, which holds `version` and
`SHA256SUMS`. The probes run first in the spike job, right after the
diagnostics (commit dfb655b on `spike/runner`), so a job timeout during the
build cannot lose them.

| URL | `curl -fsSL` | `curl -sSfIL --max-time 60` | `curl -fsSL -r 0-17 --max-filesize 4096` |
|---|---|---|---|
| `releases/download/v4.2.0/version` | exit 0, prints `4.2.0`; 1 redirect (302) to `release-assets.githubusercontent.com`, then 200 | exit 0; 302, then 200 | exit 0; 206, `content-range: bytes 0-5/6`, `accept-ranges: bytes` |
| `releases/latest/download/version` | exit 0, prints `4.2.0`; 2 redirects (302 to `releases/download/v4.2.0/version`, 302 to `release-assets.githubusercontent.com`), then 200 | exit 0; 302, 302, then 200 | exit 0; 206, `content-range: bytes 0-5/6` |

The asset host is Azure Blob storage (`server: Windows-Azure-Blob/1.0`).
Its URL carries a signature that expires after about an hour, so a download
starts from the `github.com` URL each time and never stores the redirect
target.

The range request is proven only on a 6-byte file. It is checked again on
`thinfloe-usb.img.zst` of the 4.3.0 release (Task 15 of the plan).

## CI split

Decision 17 of the design keeps the image build and the QEMU smoke tests out
of `ci.yml` because they "take long and need `/dev/kvm`" and per pull
request "would multiply the cost". The spike shows neither holds on a hosted
runner: KVM works after one udev rule, and the image plus all nine smoke
tests add under 7 minutes to a pull request run that already spends 8 minutes
on the rootfs for `make test`. Hosted runner minutes on a public repository
cost nothing.

The split changes as follows (decision 17, revised 2026-11-12):

- `ci.yml` (every pull request, every push to `main`, the release branch)
  runs two jobs through `ci-local.yml`:
  - `unit`: `make test-unit MEMCAP=`, for a result within minutes.
  - `image`: `make test MEMCAP=`, `make image`, the nine smoke targets, and
    `make test-runtime`, in one job so the rootfs is built once. About 25
    minutes.
- `release-build.yml` stays as decision 17 describes it: `make image` with
  the release version, the nine smoke tests on exactly the bytes it uploads,
  then the six assets. These are the bytes that ship, so the smoke tests run
  on them again even though the pull request already ran them on a dev
  build.

Reason: a pull request that breaks the image or a boot then fails its own
`ci-passed`, instead of surfacing only when someone starts a release.
`make test-runtime` passed on the runner, so it joins `ci.yml`, as decision 17
foresaw for that case.

No cache is needed at 25 minutes. If the run grows, the first candidate is
the container build inside `make rootfs`.

## Runner setup for the project's workflows

`ci-local.yml` and `release-build-local.yml` do this before the first make
target. Each item exists because the build assumes the development host.

1. Replace the `artifacts` symlink with a directory. The repository commits
   `artifacts` as a link to `/home/oetiker/scratch/dbrrg-artifacts`, which
   does not exist on the runner. `skip-worktree` keeps the replaced link out
   of `git status`, which would otherwise give the dev version `.dirty`:

   ```sh
   if [ -L artifacts ]; then rm artifacts; fi
   mkdir -p artifacts
   git update-index --skip-worktree artifacts
   ```

2. Set `TMPDIR`. `run-qemu-write-smoke.sh` and several integration tests
   default it to `/scratch/oetiker/claude-tmp`:

   ```sh
   mkdir -p "$RUNNER_TEMP/thinfloe-tests"
   echo "TMPDIR=$RUNNER_TEMP/thinfloe-tests" >>"$GITHUB_ENV"
   ```

3. Install the host tools the targets call outside containers.
   The list has `dnsmasq-base` because the full `dnsmasq` package starts a service on
   port 53.

   ```sh
   sudo apt-get update
   sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
     qemu-system-x86 qemu-utils ovmf mtools gdisk dnsmasq-base \
     squashfs-tools zstd dosfstools file shellcheck uidmap iproute2 \
     curl python3 openssh-client
   ```

4. Let podman resolve the short image names of the Containerfiles
   (`ubuntu:26.04`, `ubuntu:24.04`):

   ```sh
   mkdir -p ~/.config/containers
   printf '%s\n' 'unqualified-search-registries = ["docker.io"]' \
     'short-name-mode = "permissive"' >~/.config/containers/registries.conf
   ```

5. Allow unprivileged user namespaces. The runner starts with
   `kernel.apparmor_restrict_unprivileged_userns = 1`; the netboot, write and
   time zone smoke tests run in user and network namespaces:

   ```sh
   sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
   ```

6. Give `runner` a user manager. The runner service runs in
   `system.slice/hosted-compute-agent.service` without a user session; with
   linger, podman reports cgroup manager `systemd` with controllers
   `cpu memory pids`, which the `--cpu-quota` of the container builds needs.
   The spike did not run without this step, so whether podman would fall
   back to cgroupfs and fail is not measured.

   ```sh
   sudo loginctl enable-linger "$(id -un)"
   uid=$(id -u)
   for _ in $(seq 30); do [ -S "/run/user/$uid/bus" ] && break; sleep 1; done
   echo "XDG_RUNTIME_DIR=/run/user/$uid" >>"$GITHUB_ENV"
   echo "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/bus" >>"$GITHUB_ENV"
   ```

7. Open `/dev/kvm` to `runner` (only the jobs that boot QEMU):

   ```sh
   echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666"' | sudo tee /etc/udev/rules.d/99-kvm4all.rules
   sudo udevadm control --reload-rules
   sudo udevadm trigger --name-match=kvm
   ```

Every make target runs with `MEMCAP=`, as in the spike. No disk cleanup is
needed. `release-build-local.yml` passes `TF_VERSION` to every make call, not
only to `make image`: a call without it computes a dev version, exports the
rootfs again as a dev build, and the smoke tests and assets would carry that
one.
