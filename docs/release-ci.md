# Release CI on GitHub-hosted runners

The Phase 0 spike of the GitHub release plan ran the whole build on a
GitHub-hosted runner: run 43188126430 on `oposs/thinfloe`, workflow
`spike-runner.yml` on branch `spike/runner` (never merged), 2026-11-12. Every
target passed; the job took 25 minutes from checkout to the last step. This
file records the numbers, the CI split that follows from them, and the setup
the project's own workflows need on the runner.

## Runner

- Image: `ubuntu-24.04`, version 20261101.360.1.
- Cores: 4 (`nproc`), matching `BUILD_JOBS=4` and the Makefile's default
  `TEST_THREADS=4` (threads of the Rust tests). A larger runner raises both,
  e.g. `make test-unit BUILD_JOBS=8 TEST_THREADS=8`.
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

The split changes as follows (decision 17, revised 2026-11-12 and 2026-11-22):

- `ci.yml` (every pull request, every push to `main`, the release branch)
  runs three jobs through `ci-local.yml`:
  - `unit`: `make test-unit MEMCAP=` (the unit tests run only here), for a
    result within minutes.
  - `image`: `make test-integration MEMCAP=`, `make image`,
    `scripts/ci-check-prebuilt.sh record`, upload of the `prebuilt` artifact
    (`artifacts/rootfs/{vmlinuz,initrd.img,ramroot.sqsh}`,
    `artifacts/images/thinfloe-usb.img.zst{,.sha256}` and
    `artifacts/prebuilt.sha256`), and `make test-runtime`.
  - `smoke` (`needs: image`, `fail-fast: false`): three parallel matrix jobs
    that download `prebuilt` and run `make -k PREBUILT=1 MEMCAP=` on those
    files without building the container or exporting the rootfs:
    - `upgrade`: `qemu-smoke-upgrade`.
    - `sticks`: `qemu-smoke-two-sticks`, `qemu-smoke-write`, `qemu-smoke-grow`,
      `qemu-smoke-automount`.
    - `boot`: `qemu-smoke`, `qemu-smoke-netboot`, `qemu-smoke-addons`,
      `qemu-smoke-timezone`, `qemu-smoke-timezone-netboot`.
    Each smoke job runs `scripts/ci-check-prebuilt.sh verify` afterwards to
    fail if any downloaded file changed or a `thinfloe-ubuntu` container
    image exists, and uploads `ci-logs-<group>` on failure.
- `release-build.yml` stays as decision 17 describes it: `make image` with
  the release version, the ten smoke tests on exactly the bytes it uploads,
  then the six assets. These are the bytes that ship, so the smoke tests run
  on them again even though the pull request already ran them on a dev
  build.

Reason: a pull request that breaks the image or a boot then fails its own
`ci-passed`, instead of surfacing only when someone starts a release.
`make test-runtime` passed on the runner, so it joins `ci.yml`, as decision 17
foresaw for that case. `make test-runtime` stays inside `image` because it
bases its test container on the `thinfloe-ubuntu` podman image; moving it to a
separate job would require exporting and loading a multi-gigabyte container
image.

### Parallel smoke measurements (2026-11-22)

Measured on `oposs/thinfloe` (`local` wall time from the first `local / *` job
`startedAt` to the last `completedAt`), comparing two `main` runs after PR #40
(sequential smoke tests inside `image`) against two PR #45 runs (parallel
`smoke` jobs):

| Run | Ref | `local` wall time | `image` | Smokes (total or max job) | `unit` |
|---|---|---|---|---|---|
| 45013960608 | `main` (`712689f`) | 30m32s | 30m32s (`test-integration` 11m52s, `test-runtime` 6m55s) | 9m31s (sequential) | 5m08s |
| 45016706044 | `main` (`8c40f25`) | 30m09s | 30m09s (`test-integration` 11m47s, `test-runtime` 6m26s) | 9m32s (sequential) | 4m50s |
| 45024962472 (attempt 1) | PR #45 (`6844df0`) | 28m38s | 23m57s (`test-integration` 14m28s, `test-runtime` 6m28s) | 4m39s (`upgrade`, `sticks` 4m19s, `boot` 2m53s) | 4m54s |
| 45024962472 (attempt 2) | PR #45 (`6844df0`) | 26m21s | 21m57s (`test-integration` 12m45s, `test-runtime` 6m36s) | 4m22s (`upgrade`, `sticks` 4m12s, `boot` 3m02s) | 4m02s |

The failure path was verified on throwaway draft PR #46 (run 45027809938,
`exit 1` at the top of `scripts/run-qemu-grow-smoke.sh`): `local / smoke
(sticks)` failed, `boot` and `upgrade` succeeded, and `ci-passed` failed.

The parallel split cuts the smoke phase from 9m32s to 4m22s-4m39s (about 5
minutes). Because `smoke` (`needs: image`) starts only after `make
test-runtime` finishes rather than overlapping it, and `make test-integration`
varied by 2m40s across runners (11m47s to 14m28s), mean `local` wall time
dropped from 30m20s to 27m30s (kept by owner decision).

Because `local` stays above 20 minutes, the next candidate to cut CI time is
caching podman container layers inside `make rootfs`: either an Actions cache
of podman storage (within the 10 GB repository cache limit) or a registry
cache, which needs `packages: write` in a `repo-infra` caller workflow and
therefore goes through `repo-infra` first.

### Cargo cache (2026-11-27)

A layer cache saves little for a change to the menu: the binary enters the
final stage before dracut and the slim step, so they, the export and every
test run again. The cargo builds were the larger part. The three cargo
`RUN` lines of the `menu-build` stage (owlyd, thinfloe-share,
thinfloe-menu) keep their target directories in a cache mount
(`--mount=type=cache,id=thinfloe-cargo-target,target=/cargo-target`, one
directory per crate, `sharing=locked`). Podman keeps it in
`$TMPDIR/buildah-cache-<uid>/`, `/var/tmp` without `TMPDIR`; `make
BUILD_CACHE=<dir>` runs the container build with `TMPDIR=<dir>`. The mount is
no part of a layer, and the stage's `cmp` checks of the installed binaries
stay.

Cargo takes a local crate as built when no source file is newer than the
binary, and every checkout on a host shares the mount. A worktree whose files
were older than a binary another worktree had built got that binary: the
image held the other worktree's menu, and the gates passed on it
(2026-11-27). The thinfloe-share and thinfloe-menu builds therefore set
`TF_SOURCE_KEY`, a SHA-256 over the copied sources (the menu's also over the
home templates it embeds). The crates read it with `option_env!` and print it
with `--source-key`; cargo records it as an env dependency and rebuilds the
crate when it changes. In the menu the key sits in the library (`lib.rs`),
which holds the code: in `main.rs` it rebuilt only `main.rs` and linked the
stale library. `test-source-key.sh` computes the key over the checkout
(`test/integration/lib/source-key.sh`) and looks for it in the image's
binaries.

In CI the `image` job sets `BUILD_CACHE=/home/runner/build-cache`, restores
the newest Actions cache `cargo-target-v1-*` before `make test-integration`
and saves one (key `cargo-target-v1-<sha>`) only on a push to `main`, so
pull requests read the cache of `main` and do not fill the 10 GB limit. The
cache is about 1.4 GB. The release build (`release-build-local.yml`) restores
nothing and builds cold.

Measured on the development host (4 cores): cold, owlyd 59 s, thinfloe-share
72 s, thinfloe-menu 156 s. With the cache, after a change to the menu's
`main.rs`: thinfloe-share 3 s, thinfloe-menu 61 s, owlyd a few seconds.

The same change removed a second export: `make image` built the container
stamp and exported the rootfs again (about 2 minutes) after `make
test-integration`, because the cargo test of `test-network-netplan.sh`
created `src/thinfloe-menu/target/` on the runner (no `CARGO_TARGET_DIR`)
and so changed the mtime of a directory that was a prerequisite of
`.ubuntu-container`. The stamp's input hash now covers the directory names
instead.

## Runner setup for the project's workflows

`scripts/ci-runner-setup.sh` prepares the runner after the checkout:
`ci-local.yml` and `release-build-local.yml` run it with `all` before the
first make target, and the `unit` job runs it with `artifacts` (item 1
only). Each item exists because the build assumes the development host; the
commands are in the script.

1. Make the `artifacts` directory. Up to 4.6.0 the repository committed
   `artifacts` as a link to `/home/oetiker/scratch/dbrrg-artifacts`, which
   does not exist on the runner, and every make call printed
   `mkdir: cannot create directory 'artifacts': File exists`. For a checkout
   of such a ref the script replaces the link with a directory, and
   `git update-index --skip-worktree artifacts` keeps the change out of
   `git status`, which would otherwise give the dev version `.dirty`.
2. Set `TMPDIR` to `$RUNNER_TEMP/thinfloe-tests`. The test targets put
   their work dirs under `TMPDIR`; without one the `Makefile` uses `.tmp/`
   in the checkout.
3. Install the host tools the targets call outside containers. The list has
   `dnsmasq-base` because the full `dnsmasq` package starts a service on
   port 53.
4. Let podman resolve the short image names of the Containerfiles
   (`ubuntu:26.04`, `ubuntu:24.04`) through `docker.io`.
5. Allow unprivileged user namespaces. The runner starts with
   `kernel.apparmor_restrict_unprivileged_userns = 1`; the netboot, write and
   time zone smoke tests run in user and network namespaces.
6. Give `runner` a user manager (`loginctl enable-linger`). The runner
   service runs in `system.slice/hosted-compute-agent.service` without a
   user session; with linger, podman reports cgroup manager `systemd` with
   controllers `cpu memory pids`, which the `--cpu-quota` of the container
   builds needs. The spike did not run without this step, so whether podman
   would fall back to cgroupfs and fail is not measured. The script fails
   when the user bus is still missing 30 seconds after it.
7. Open `/dev/kvm` to `runner` with the udev rule
   `KERNEL=="kvm", GROUP="kvm", MODE="0666"`.

Every make target runs with `MEMCAP=`, as in the spike. No disk cleanup is
needed. `release-build-local.yml` passes `TF_VERSION` to every make call, not
only to `make image`: a call without it computes a dev version, exports the
rootfs again as a dev build, and the smoke tests and assets would carry that
one. It also records the checksums of the four built files after
`make image` and fails before the uploads when `artifacts/release/SHA256SUMS`
names other ones. On a failure, each CI smoke job uploads `artifacts/images/*.log*` (the smoke
tests' serial logs and the write test's HTTP log) as `ci-logs-<group>`, and
`release-build-local.yml` uploads them as `release-build-logs`.
