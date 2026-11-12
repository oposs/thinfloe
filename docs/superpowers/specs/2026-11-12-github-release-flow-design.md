# ThinFloe Part 5: GitHub releases

Date: 2026-11-12. Status: decided, ready to plan.

This document replaces §"Part 5: GitHub releases" of
`docs/superpowers/specs/2026-10-21-thinfloe-design.md` and the version and
checksum contract that `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`
("Settled after the builder check", item 1) points at. Where they disagree,
this document wins. "What changed since the old design" lists each change.

Every decision is marked *owner* (decided by the owner) or *architect* (my
call, which the owner may still overturn). *Builder* marks what is left to
the person who plans and builds this.

## Intent

Today a release is made on one developer machine: raise `RELEASE`, run
`make`, run `scripts/publish.sh`, which rsyncs the files to
`https://tobi.oetiker.ch/dbrrg`. Nothing records which commit a release was
built from, the build and tests run only where the owner happens to run them,
and `CHANGES.md` has never been rolled: every entry since the rename still
sits under `[Unreleased]`, although 4.0.0, 4.0.1, 4.1.0 and 4.2.0 shipped it.

Part 5 moves releases onto the repo-infra standard at `github.com/oposs/thinfloe`:
`main` is protected, every change arrives through a pull request, and a
release is a pull request opened by **Create release PR** that was built and
tested on GitHub before anyone merges it. Machines in the field then find
and download new releases from GitHub. The repository is public, so anyone
can build, inspect and install the same image.

## Goals

- A release is built and smoke-tested on GitHub Actions from a known commit,
  tagged `vX.Y.Z`, and published as a GitHub release with fixed asset names.
- `CHANGES.md` is the version source; `RELEASE` and `/etc/thinfloe/version`
  follow it without a hand edit.
- A USB machine on 4.x finds the GitHub release through its daily check and
  the Upgrade tile, and refuses any download whose checksum does not match.
- Every machine in the field reaches the GitHub releases through one bridge
  release, including sticks still on 3.x.
- A self-built image says it is a development build and is always offered
  the current release.
- The public image holds nothing specific to the owner's site.

## Non-goals

- Netboot from GitHub (*owner*). The Upgrade tile stays hidden on netboot,
  and netboot machines keep updating as today.
- A 3.x maintenance line, pre-releases, release channels, or signed images.
  The checksum file guards against damaged downloads, not against a
  compromised GitHub account.
- Packages (`.deb`, `.rpm`), Homebrew, crates.io. ThinFloe ships a bootable
  image.
- Changing the Upgrade screen beyond its source address and checksum step.

## Anti-goals

- A machine in the field that can no longer upgrade: neither tobi.oetiker.ch
  nor GitHub offers it a path forward.
- A machine that writes a mix of two releases onto its stick because a new
  release appeared mid-download.
- A dev build that hides a release from its own stick, or one that can be
  published by mistake.
- A red check on `main` that nobody caused. The release guard refuses every
  dispatch from such a commit (`release-flow.md`, "The guard fails fast"), so
  the baseline must leave the publish workflow green from its first run.
- A per-commit version that forces a full container rebuild on every commit.
- Any Standing Constraint in `CLAUDE.md` broken along the way.

## Constraints

- repo-infra 2.13 (`/home/oetiker/.claude/plugins/cache/oposs-plugins/repo-infra/2.13.0/skills/repo-infra/`)
  is the standard. Read `SKILL.md`, `references/onboarding.md` and
  `references/release-flow.md` before planning; `references/examples/mtuspy/`
  is the model for the five callers, `assets/callers/` holds the parts copied
  word for word. Pieces are never edited; what they lack goes into a caller,
  `ci-local.yml` or `release-build-local.yml`.
- Facts of the repo-infra release machinery this design depends on, read
  from its source (`oposs/repo-infra`, `ri-release-prepare.yml`,
  `lib/version.js`, `lib/publish.js`, `ri-publish-tag.yml`):
  - The next version is computed from the git tags `vX.Y.Z`; with no tag at
    all the base is `v0.0.0`, so a feature release would be `0.1.0`.
  - A released section in `CHANGES.md` is `## X.Y.Z - YYYY-MM-DD`; sections
    are `### New`, `### Changed`, `### Fixed`.
  - Every push of `CHANGES.md` to `main` runs the publish workflow. It
    verifies every `version_files` entry against the newest released section,
    then expects the tag and exactly one published release for it. A tag
    without a release fails it on every run.
  - `apply` and `check` refuse while the newest released section has no tag.
- Today `repo_infra check` reports 18 items missing: all callers, the pieces
  they call, `repo-infra.json`, `dependabot`, branch protection, required
  checks, the `no-changelog` label and `delete-merged-branches`.
  `actions-open-pr` and the default branch `main` are already fine; the
  Actions policy of the repository and of `oposs` allows all actions, so
  `oposs/repo-infra` reusable workflows run.
- The repository has no tags, no releases and no `.github/` directory.
- Asset sizes today: `vmlinuz` 17 MB, `initrd.img` 100 MB, `ramroot.sqsh`
  494 MB, `thinfloe-usb.img.zst` 602 MB (3.1 GB raw). GitHub refuses assets
  of 2 GiB or more.
- The build uses podman containers, builds labwc, iPXE and the Rust menu,
  and writes the USB image with `mtools`, without loop devices. The QEMU smoke
  tests need `/dev/kvm`. `BUILD_JOBS=4` caps parallelism.
- `thinfloe-upgrade` (`overlay/usr/bin/thinfloe-upgrade`) is the only place
  that knows the download address (`DEFAULT_URL`); the menu passes
  `--source` only for a custom address. Field machines on 4.x read
  `https://tobi.oetiker.ch/dbrrg`; 3.x sticks read the same server under the
  names `dbrrg-usb.img.zst`, `vmlinuz`, `initrd.img`, `ramroot.sqsh`.
- The repository is public: no step may need a stored secret, and no
  self-hosted runner may run code from fork pull requests.

## Decisions

### Process

1. *Owner.* The repo-infra standard applies in full: `main` is protected by
   the ruleset that requires `ci-passed` and `changelog-updated`, every change
   goes in through a pull request, releases are made with **Create release
   PR** (bugfix, feature or major) and published by merging its pull request.
2. *Owner.* The version is raised for every release, the minor number for a
   release with a feature. The release type chosen at dispatch now does this;
   the CLAUDE.md rule "Raise `RELEASE` before a release" is replaced by it.
3. *Architect.* `CHANGES.md` is the version source (repo-infra D5). `RELEASE`
   stays the file the build reads and becomes the one `version_files` entry
   in `.github/repo-infra.json` (pattern `^[0-9][^\n]*$`, as the `VERSION`
   entry in `onboarding.md`). Nobody edits it by hand any more; between
   releases it holds the newest released version.

### Baseline

4. *Architect.* The baseline is **v4.2.0**, not the `v3.0.0` at `4e9ca9c` that
   an older handoff note asks for. That note predates the 4.x releases: with
   `v3.0.0` as the newest tag the first feature release would be 3.1.0, below
   what the field runs. The baseline consists of:
   - `CHANGES.md`: every entry now under `[Unreleased]` moves under
     `## 4.2.0 - 2026-11-11`, with an empty `[Unreleased]` (`### New`,
     `### Changed`, `### Fixed`) above it. No entry was added after `db60ef6`,
     so the whole section is what 4.2.0 shipped. One section stands for
     4.0.0 to 4.2.0; the three earlier versions get no sections of their own,
     since nobody can now tell which entry shipped in which of them.
   - An annotated tag `v4.2.0` on `db60ef6` ("build: release 4.2.0").
   - A published GitHub release `v4.2.0`, so the publish workflow reads
     "v4.2.0 is released - nothing to do" instead of failing. It carries the
     4.2.0 files from tobi.oetiker.ch under the asset names below, plus a
     `SHA256SUMS` made from them, and no `release-build.json`. That makes
     `latest/download/` answer from the first day, which the builder's tests
     against the real GitHub need.

### Release assets

5. *Architect.* Each release carries exactly these assets, listed in
   `release_assets`:
   - `vmlinuz`, `initrd.img`, `ramroot.sqsh`: the A/B upgrade.
   - `thinfloe-usb.img.zst`: the fresh install.
   - `version`: one line `X.Y.Z`, the file `thinfloe-upgrade check` reads.
   - `SHA256SUMS`: `sha256sum` format over the five files above.

   Names carry no version, so `https://github.com/oposs/thinfloe/releases/latest/download/<name>`
   always names the newest release. The tag (`vX.Y.Z`) and the title
   (`thinfloe vX.Y.Z`, set by repo-infra) carry the number. The release
   build checks that `version`, `RELEASE` and the image's
   `/etc/thinfloe/version` all equal the `version` input, and that every
   asset is below 2 GiB.

### Versions of local builds

6. *Architect.* A build outside the release build writes
   `X.Y.Z-dev+<short sha>` into `/etc/thinfloe/version`, where `X.Y.Z` is
   `RELEASE` and the sha is `HEAD`, with `.dirty` appended when the tree has
   changes. It sorts below `X.Y.Z`, so a self-built stick is offered the
   current release, and scripts tell a dev build by the `-dev`. The release
   build passes the version explicitly (for example `make image
   TF_VERSION=4.3.0`) and the build refuses a value that differs from
   `RELEASE`.

   What this touches:
   - The menu's comparison (`src/thinfloe-menu/src/release.rs`, `newer`)
     already handles it: it drops `+...`, ranks a pre-release below its
     release and has the test `newer("4.0.0", "4.0.0-dev+abc")`. A test for
     `.dirty` and for `4.3.0` against `4.2.0-dev+abc` is added; no logic
     changes.
   - `thinfloe-upgrade` accepts it (`VERSION_RE` allows `+`, `-`, `.` and 32
     characters; `4.2.0-dev+85034f2.dirty` is 23), and `thinfloe-addon`'s
     User-Agent accepts it.
   - The Dockerfile check "RELEASE must be one line X.Y.Z" and
     `test/integration/test-release-version.sh` change: `RELEASE` stays strict
     `X.Y.Z`, the written version may be `X.Y.Z` or the dev form.
   - `scripts/publish.sh` already refuses anything but `X.Y.Z`; it keeps that
     until it is retired.
   - *Builder:* where the version is written. Today it is a Dockerfile layer
     with many layers after it, and the `.ubuntu-container` stamp depends on
     `RELEASE`. A sha that changes with every commit must not rebuild the
     container or the rootfs when no image input changed: write it at the
     latest step that still lands in `ramroot.sqsh`, or accept that the sha
     names the commit the rootfs was last built from, and say which.

### Download and checksum

7. *Architect.* `DEFAULT_URL` in `thinfloe-upgrade` becomes
   `https://github.com/oposs/thinfloe/releases/latest/download`. That address
   skips drafts and pre-releases and does not count against the API limit of
   60 requests per hour per address.
8. *Architect.* A write from the default source is pinned to one release:
   it reads `version` from `latest/download` once, at its start, then takes
   every other file from `https://github.com/oposs/thinfloe/releases/download/v<version>/`.
   A release published during the download can then not mix two releases on
   one stick. A custom source is used as given.
9. *Architect.* The checksums come from `SHA256SUMS` (`expected_download_sha256`
   is the one function that changes). From the default source a missing
   `SHA256SUMS`, a missing line or a mismatch fails the job before the stick
   is changed (upgrade) or clears the start of the stick (fresh install), as a
   mismatch does today. From a custom source the tool falls back to
   `<name>.sha256` and then to "checksum not checked" in the log, so old
   server directories stay usable for tests.
10. *Architect.* The custom-URL option in `thinfloe-upgrade` and on the
    Upgrade screen stays, for testing.
11. *Owner.* The Upgrade tile stays hidden on netboot; netboot machines keep
    updating as today.

### Bridge from tobi.oetiker.ch

12. *Architect.* The bridge is the **first release made by the GitHub flow**
    (a feature release, so 4.3.0). Its `thinfloe-upgrade` reads GitHub. After
    it is public on GitHub, the owner copies its six assets unchanged to
    tobi.oetiker.ch with `scripts/publish.sh`, adapted to take a downloaded
    release directory and to check it against `SHA256SUMS`. The files on
    tobi.oetiker.ch are then byte for byte the GitHub release.
    - A 4.x stick sees 4.3.0 on tobi.oetiker.ch, upgrades, and reads GitHub
      from then on.
    - The bridge also stays reachable as `dbrrg-usb.img.zst` (plus its
      `.sha256`), as `publish.sh` uploads today, and the per-file
      `thinfloe-usb.img.zst.sha256` stays, because 4.0 to 4.2 read that file.
      3.x sticks then land on the bridge directly.
    - tobi.oetiker.ch keeps the bridge files and is never updated again. It
      remains the way forward for any stick that still reads it.
13. *Architect.* After the bridge is on tobi.oetiker.ch, `scripts/publish.sh`
    is removed, with its parts of `test-release-version.sh`, its lines in
    `CLAUDE.md` and the README. Every later release exists only on GitHub.

### Site-specific content

14. *Owner.* No site-specific content in the public image.
15. *Architect.* What goes, before the first GitHub release:
    - `overlay/home/tfuser/.thinlinc/tlclient.conf` is removed, with its
      directory. Besides `SERVER_NAME=thinlinc-aarweg-01.oetiker.ch` and
      `HOST_ALIASES=...=192.168.0.11` it carries the site's SSH port
      (`SSH_ARBITRARY=46022`) and a stale client version and update stamp;
      ThinLinc writes its own defaults on first start. Machines that upgrade
      keep their `~/.thinlinc/` through the restored home. A new stick for the
      owner's site gets the settings through "copy the home" on the Upgrade
      screen.
    - What stays: the OxuLnk tile's add-on source
      `https://gitea.oetiker.ch/api/packages/oposs/debian` is the owner's
      public package server, which any user may install from;
      `tobi.oetiker.ch` in `thinfloe-upgrade` disappears with decision 7; the
      `LABEL network` entry in `configs/syslinux.cfg` with
      `http://192.168.1.100/boot/ramroot.sqsh` is a placeholder example.
    - *Builder:* search again (host names, addresses, domains, user names,
      keys) over `overlay/`, `configs/`, `containers/`, `brand/` and the
      default home, and confirm `gitea.oetiker.ch` answers the add-on source
      without login from outside the owner's network.

### CI and release build

16. *Architect.* `ci.yml` calls the repository's own jobs through
    `ci-local.yml`; the release build calls `release-build-local.yml`. No
    repo-infra CI piece fits a podman image build. Whether `ri-ci-rust` can
    test the menu crate under `src/thinfloe-menu/` (there is no root
    `Cargo.toml`) is the builder's call; if it can, `dependabot` gets
    `{"cargo": ["/src/thinfloe-menu"]}`, otherwise cargo stays in
    `ci-local.yml` and dependabot watches GitHub Actions only.
17. *Architect.* The proposed split, to be confirmed by Phase 0:
    - `ci.yml`, on every pull request, every push to `main` and on the
      release branch: `make test-unit` (Python tests and `cargo test`), and
      the container and rootfs build followed by `make test`. The Standing
      Constraints are guarded in `make test`; a pull request that breaks one
      must not turn `ci-passed` green.
    - `release-build.yml`, on the release branch only: `make image` with the
      release version, then the QEMU smoke tests on exactly the bytes it
      uploads (`qemu-smoke`, `-netboot`, `-upgrade`, `-write`, `-grow`,
      `-two-sticks`, `-addons`, `-timezone`, `-timezone-netboot`), then the
      six assets as `release-asset-*` artifacts. A release whose smoke test
      fails never gets a pull request.

    The image and the smoke tests take long and need `/dev/kvm`; per pull
    request they would multiply the cost. repo-infra D28 guarantees they run
    on the release commit before the merge, which is where a broken image
    must be stopped. `make test-runtime` needs network and a compositor; it
    joins `ci.yml` if Phase 0 shows it runs on the runner.
18. *Architect.* No step uses a stored secret. The release flow needs only
    `GITHUB_TOKEN`.

## What changed since the old design

- **Bridge.** The old design sent only 4.0.0 to the field through
  tobi.oetiker.ch, with every later release from GitHub. Since then 4.0.0,
  4.0.1, 4.1.0 and 4.2.0 all went out through `publish.sh`, so field machines
  on 4.x still read tobi.oetiker.ch. The bridge is now the first GitHub
  release, mirrored to tobi.oetiker.ch (decision 12).
- **Asset names.** The old design put the version into asset file names. They
  now carry none, because `latest/download/<name>` needs a fixed name
  (decision 5).
- **Baseline tag.** `v3.0.0` at `4e9ca9c` is replaced by `v4.2.0` at
  `db60ef6` with a GitHub release and a rolled `CHANGES.md` (decision 4).
- **Version split.** The old design split the Makefile's `VERSION` into an
  image tag and the release version. Commit `8677396` already introduced
  `RELEASE`; `VERSION := 3.0.0` stays as the container tag, which keeps
  checkouts apart. `RELEASE` becomes a `version_files` entry (decision 3).
- **Checksums.** Today only the image has a checksum file and the firmware
  files are logged "checksum not checked". The settings design skipped the
  check until Part 5. Now one `SHA256SUMS` covers all and is required from the
  default source (decision 9).
- **Pinned download.** New: a write takes all files from one tag
  (decision 8).
- **Site-specific content.** The old design kept `tlclient.conf` with neutral
  defaults and named the home `tluser`; the file is removed, and the home is
  `tfuser` (decision 15). The OxuLnk question ("shipped only if the OEP
  package server is publicly reachable") is settled: it is public.

## Left to the builder

### Phase 0 spike: measure before building

Run the real build on a GitHub-hosted runner (`ubuntu-latest`) from a spike
workflow on a branch that is not merged, or in a fork. Pushing a branch to
`oposs/thinfloe` is outward-facing: ask the owner first. Measure and write
down:

- free disk at start, peak use during `make image` and the smoke tests, and
  whether removing preinstalled toolchains is needed;
- that podman is installed and builds the containers rootless as the Makefile
  expects;
- that `/dev/kvm` exists and is usable by the runner user (a udev rule may be
  needed), and that each smoke test passes within its timeout;
- wall time of the container build, `make test`, `make image` and each smoke
  test, cold and with whatever cache works (`actions/cache`, a registry);
- against the real `v4.2.0` release: that `curl -fsSL` follows the redirects
  of `latest/download/version`, that the range request of `image_size`
  (`-r 0-17`) and the `HEAD` request (`curl -sSfIL`) used for sizes and
  progress work on the redirected asset URL, and what to do where they do not.

From the numbers, confirm or change the split of decision 17 and say why.
If a hosted runner cannot build or smoke-test the image (disk, KVM or the
six-hour job limit), stop and put it to the owner as a decision: a larger
GitHub-hosted runner (paid per minute) or a self-hosted runner (which on a
public repository must never run fork pull requests).

### Also the builder's call

- How `ci-local.yml` and `release-build-local.yml` split into jobs, and how
  the six assets reach the upload (one job, or artifacts between jobs).
- Where the dev version is written (decision 6).
- The exceptions in `.github/repo-infra.json`. Rules that cannot apply to a
  bootable image get an exception with that reason: `linux-packages`,
  `homebrew`, and `man-page` (the command line tools are internal to the
  image; the README documents the ones used over SSH). `screenshot`, `badges`,
  `install-text`, `readme-current` and `writing-style` are done: a screenshot
  of the menu from QEMU, the two badges, an install section that downloads
  `thinfloe-usb.img.zst` from the latest release and writes it to a stick.
- Tests for decisions 8 and 9 with a local HTTP server that redirects the way
  GitHub does, extending `test/unit/test_upgrade_image.py` and
  `test_upgrade_batch.py`.

## Rollout

Each step lands before the next starts. Steps marked *owner* change the live
repository or a server; the builder prepares them, the owner confirms each
one at the time it is done.

1. **Phase 0 spike** (above). Its result may stop the rollout at an owner
   decision.
2. **Site-specific content out** (decision 15). An ordinary pull request with
   a `CHANGES.md` entry; `main` is not protected yet.
3. **Dev version** (decision 6) and **download from GitHub** (decisions 7 to
   10). One pull request or two, each with `CHANGES.md` entries. They change
   what the image does, but nothing ships until step 9.
4. *Owner:* **baseline v4.2.0** (decision 4). Push the annotated tag `v4.2.0`
   on `db60ef6` and create the published release `v4.2.0` with the six files.
   This must exist before the `CHANGES.md` roll of step 6 reaches `main`, or
   the first publish run fails and blocks every later dispatch.
5. *Owner:* **`no-changelog-label`** (`repo_infra apply --item no-changelog-label`).
6. **Onboarding pull request**, opened with the `no-changelog` label at
   creation: the five callers, `ci-local.yml`, `release-build-local.yml`,
   `.github/repo-infra.json`, `dependabot` (`repo_infra apply --item
   dependabot`), and the `CHANGES.md` roll to `## 4.2.0 - 2026-11-11` (keeping
   under `[Unreleased]` any entry steps 2 and 3 added). `repo_infra check`
   exits 0 apart from the administration items. After the merge, the publish
   run on `main` must read "v4.2.0 is released - nothing to do" and `ci-passed`
   must be green on the merge commit.
7. *Owner:* **`required-checks`** (`repo_infra apply --item required-checks`),
   only now that `ci.yml` and `changelog.yml` are on `main`. From here every
   change needs a pull request with `ci-passed` and `changelog-updated`.
8. *Owner:* **`delete-merged-branches`** (`repo_infra apply --item
   delete-merged-branches`). `repo_infra check` now exits 0.
9. *Owner:* **First release, the bridge.** Dispatch **Create release PR** with
   `feature`. Check the draft: six assets, `version` reads `4.3.0`,
   `SHA256SUMS` matches. Merge. Then, on a stick with 4.2.0, point the Upgrade
   screen's custom source at the release's download address and upgrade, to
   prove the GitHub path end to end.
10. *Owner:* **Bridge to tobi.oetiker.ch.** Download the 4.3.0 assets, run the
    adapted `scripts/publish.sh`. Check with a 4.x stick (or
    `make qemu-smoke-upgrade` against a local copy) that it is offered 4.3.0,
    upgrades, and its next check reads GitHub.
11. **Retire `publish.sh`** (decision 13), through a pull request with a
    `CHANGES.md` entry if anything user-visible changes, otherwise with the
    `no-changelog` label at creation.

## Questions for the owner

- **How do the netboot machines get a new image today?** If the boot server
  takes its files from tobi.oetiker.ch/dbrrg, freezing that server at the
  bridge (decision 12) also freezes netboot. Then either the boot server
  fetches from the GitHub release (a script to build) or the owner copies
  each release there by hand.
- **Runner**, only if Phase 0 fails: larger hosted runner or self-hosted.
