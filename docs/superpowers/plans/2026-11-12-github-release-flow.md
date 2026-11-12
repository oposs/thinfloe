# ThinFloe GitHub releases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Releases of ThinFloe are built, smoke-tested and published on GitHub through the repo-infra standard, and machines in the field find and download them there.

**Architecture:** repo-infra callers in `.github/workflows/` call the `oposs/repo-infra` pieces at `@v2`; the image build and the QEMU smoke tests live in the repository's own `ci-local.yml` and `release-build-local.yml`, which run the existing Makefile targets. `thinfloe-upgrade` reads `releases/latest/download`, pins each write to one tag and checks one `SHA256SUMS`. A one-time bridge release is mirrored to tobi.oetiker.ch, after which `publish.sh` goes.

**Tech Stack:** GitHub Actions, repo-infra 2.13 (`@v2`), GNU make, podman, Python 3 (`thinfloe-upgrade`, `unittest`), Rust (`thinfloe-menu`), bash integration tests, QEMU/KVM.

**Spec:** `docs/superpowers/specs/2026-11-12-github-release-flow-design.md` (approved by the owner 2026-11-12). Decision numbers below (D1 to D18) and rollout step numbers (R1 to R12) refer to it.

## Global Constraints

- Machine rules for every implementer task on the development host: at most 4 cores (`BUILD_JOBS=4`); every build runs as `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make …`; `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests/<task>/` (create it; never `/tmp`); menu tests run under `systemd-run --user --scope -q -p MemoryMax=4G --`; containers with `podman`, never docker.
- **Owner confirms at the time:** any step that pushes to `oposs/thinfloe`, opens or merges a pull request there, changes a repository setting, creates a tag or a release, or uploads to tobi.oetiker.ch. The controller does these steps, after the owner's go-ahead in that moment. An implementer never does them; an implementer's task ends with a local commit on its branch.
- Asset names, exactly: `vmlinuz`, `initrd.img`, `ramroot.sqsh`, `thinfloe-usb.img.zst`, `version`, `SHA256SUMS`. No version in any name.
- Default source: `https://github.com/oposs/thinfloe/releases/latest/download`. Pinned source: `https://github.com/oposs/thinfloe/releases/download/v<X.Y.Z>`.
- Dev version: `X.Y.Z-dev+<short sha>` with `.dirty` appended for a changed tree; `X.Y.Z` is `RELEASE`. `RELEASE` itself stays one line `X.Y.Z` and is edited only by the release pull request.
- `CHANGES.md` released heading: `## X.Y.Z - YYYY-MM-DD`; sections `### New`, `### Changed`, `### Fixed`. Every user-visible change gets an entry under `[Unreleased]` written for users and administrators (repo-infra `writing-style` skill, "Changelog entries").
- No stored secrets in any workflow; `GITHUB_TOKEN` only.
- Workflow rules from repo-infra: `ci.yml` and `changelog.yml` carry no `paths` filter; `ci-passed` and `changelog-updated` carry no `name:`; every project-owned workflow triggers on `workflow_call` only, sets `timeout-minutes` on every job, checks out `ref: ${{ inputs.ref }}`, asks for `contents: read` only, and keeps conditions inside steps.
- Prose (docs, README, CHANGES, commit messages): no em dashes, "for example" never "e.g.".

## Review Focus

1. A new release appears on GitHub while a stick downloads: every file must come from the tag the write started with (Task 5, `test_write_uses_pinned_tag_for_every_file`).
2. A release without `SHA256SUMS`, or one whose `SHA256SUMS` lacks a line for a file: from the default source the stick must stay unchanged on upgrade and not be written on fresh install (Task 5, `test_required_checksum_missing_fails_before_write`).
3. GitHub answers with two redirects to another host: `check`, the size probe and the download must all follow them (Task 5, `test_follows_redirect_chain`).
4. A dev build of 4.3.0 work (`4.2.0-dev+abc`) must still be offered 4.2.0 and 4.3.0, and a release build with a mismatched `TF_VERSION` must fail (Task 4, `newer("4.2.0", "4.2.0-dev+abc.dirty")` and `test_release_build_refuses_wrong_version`).
5. A local `make image TF_VERSION=4.3.0` after a dev build must not ship the dev `/etc/thinfloe/version` (Task 4, `test-release-version.sh` case "switching to a release version re-exports the rootfs").

---

### Task 1: Baseline v4.2.0 (R1, D4). Owner confirms at the time; controller only

**Files:** none in the repository.

- [ ] **Step 1: Make the two files** in `/scratch/oetiker/claude-tmp/thinfloe-tests/baseline/`: `version` containing `4.2.0\n`; `SHA256SUMS` from `sha256sum version`.
- [ ] **Step 2 (owner confirms):** `git tag -a v4.2.0 -m "ThinFloe 4.2.0" db60ef6 && git push origin v4.2.0`.
- [ ] **Step 3 (owner confirms):** `gh release create v4.2.0 --repo oposs/thinfloe --title "thinfloe v4.2.0" --notes "Baseline for the GitHub release flow. Images of 4.2.0 are not published here." version SHA256SUMS`.
- [ ] **Step 4: Verify:** `curl -fsSL https://github.com/oposs/thinfloe/releases/latest/download/version` prints `4.2.0`; `gh release view v4.2.0 --repo oposs/thinfloe --json isDraft,assets` shows `false` and two assets.

### Task 2: Phase 0 spike on a GitHub-hosted runner (R2)

**Files:**
- Create (spike branch only, never merged): `.github/workflows/spike-runner.yml`
- Create (on the implementer's branch, merged later with Task 9): `docs/release-ci.md`

- [ ] **Step 1: Write `spike-runner.yml`**, triggered on `push` to branch `spike/runner`, one job on `ubuntu-latest`, `timeout-minutes: 340`. Steps: print `df -h /`, `nproc`, `free -g`, `podman --version`, `ls -l /dev/kvm`; add the udev rule `KERNEL=="kvm", GROUP="kvm", MODE="0666"` and reload udev; run with `time` each of `make rootfs`, `make test`, `make image`, `make qemu-smoke`, `make qemu-smoke-netboot`, `make qemu-smoke-upgrade`, `make qemu-smoke-write`, `make qemu-smoke-grow`, `make qemu-smoke-two-sticks`, `make qemu-smoke-addons`, `make qemu-smoke-timezone`, `make qemu-smoke-timezone-netboot`, `make test-runtime` (each `continue-on-error: true`, `MEMCAP=`), printing `df -h /` after each; then the curl probes on both `https://github.com/oposs/thinfloe/releases/download/v4.2.0/version` and `https://github.com/oposs/thinfloe/releases/latest/download/version`: `curl -fsSL`, `curl -sSfIL --max-time 60`, `curl -fsSL -r 0-17 --max-filesize 4096`, printing exit codes and headers.
- [ ] **Step 2 (owner confirms):** controller pushes branch `spike/runner` to `oposs/thinfloe` (or to a fork the owner names) and waits for the run.
- [ ] **Step 3: Write `docs/release-ci.md`**: a table of disk peak, wall time per target, KVM state and curl results; the CI split of D17 confirmed or changed, with the reason; what `ci-local.yml` must do to free disk if needed.
- [ ] **Step 4: Stop point.** If the build, KVM or the 6-hour limit fails on the hosted runner, the controller puts the runner choice (larger hosted runner or self-hosted, never running fork pull requests) to the owner and the plan waits.
- [ ] **Step 5 (owner confirms):** controller deletes the remote branch `spike/runner`. Commit `docs/release-ci.md` on the implementer's branch.

### Task 3: Site-specific content out (R3, D15)

**Files:**
- Delete: `overlay/home/tfuser/.thinlinc/tlclient.conf` (and the empty `.thinlinc/`)
- Create: `test/integration/test-public-content.sh`
- Modify: `Makefile` (add the script to `test`), `CHANGES.md`

- [ ] **Step 1: Write `test-public-content.sh`** (offline, style of `test-release-version.sh` with `ok`/`bad`): fails when `overlay/home/tfuser/.thinlinc` exists; fails when `grep -rIl -e aarweg -e 'thinlinc-[a-z]*-[0-9]*\.oetiker\.ch' -e '192\.168\.0\.11' -e 46022 overlay configs containers brand` finds anything; passes `configs/syslinux.cfg` with `192.168.1.100` and `gitea.oetiker.ch` in `20-oxulnk.desktop`.
- [ ] **Step 2: Run** `test/integration/test-public-content.sh`. Expected: FAIL naming `tlclient.conf`.
- [ ] **Step 3: Search** once more by hand (host names, addresses, domains, user names, keys) over `overlay/ configs/ containers/ brand/`; report each further hit to the controller instead of deciding it.
- [ ] **Step 4: Delete the file**, add the script to the `test` target, add a `### Changed` entry: a new stick no longer comes with ThinLinc settings for the owner's server; ThinLinc starts with its own defaults, and upgraded sticks keep theirs.
- [ ] **Step 5: Run** the script (PASS) and `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test`. Expected: all green.
- [ ] **Step 6: Commit** `release: the public image carries no site-specific ThinLinc settings`.

### Task 4: Dev version (R4, D6)

**Files:**
- Modify: `Makefile`, `containers/ubuntu/Dockerfile:287-293`, `scripts/export-rootfs.sh:38-40`, `test/integration/test-release-version.sh`, `test/integration/test-container-inputs.sh`, `src/thinfloe-menu/src/release.rs` (tests only), `CLAUDE.md`, `CHANGES.md`

**Interfaces:**
- Produces: `make … TF_VERSION=X.Y.Z` builds a release; without it the version is `$(shell cat RELEASE)-dev+<sha>[.dirty]`. `artifacts/rootfs/version` holds the written version.

Where the version is written (the builder's call of D6, decided here): `export-rootfs.sh` writes `/etc/thinfloe/version` from the env `TF_VERSION` inside its throwaway `--rm` container, right before `mksquashfs`, and copies it to `artifacts/rootfs/version`. The Dockerfile layer and the `RELEASE` prerequisite of `.ubuntu-container` go, so no commit rebuilds the container. The export reruns when the stamp `artifacts/rootfs/.version-kind` changes; it holds `$(RELEASE)` for a release build and `$(RELEASE)-dev` for a dev build (the sha alone does not re-export, so a dev version names the commit the rootfs was last exported from).

- [ ] **Step 1: Write the failing tests** in `test-release-version.sh`, replacing the Dockerfile cases: `export-rootfs.sh` accepts `4.3.0` and `4.2.0-dev+85034f2` and `4.2.0-dev+85034f2.dirty`, and refuses `4.3`, an empty value and two lines (run its version check in isolation, as the old test did for the Dockerfile); `make -n image TF_VERSION=9.9.9` with `RELEASE` at `4.2.0` fails with `TF_VERSION 9.9.9 is neither RELEASE (4.2.0) nor a dev version` (case name `test_release_build_refuses_wrong_version`); the Makefile writes `.version-kind` only when its content changes; case "switching to a release version re-exports the rootfs": `.version-kind` from `4.2.0-dev` to `4.2.0` makes `$(SQUASHFS)` out of date (`make -q`). In `release.rs` `version_order`: `assert!(newer("4.2.0", "4.2.0-dev+abc.dirty"))`, `assert!(newer("4.3.0", "4.2.0-dev+abc"))`, `assert!(!newer("4.2.0-dev+abc", "4.2.0"))`.
- [ ] **Step 2: Run** `test/integration/test-release-version.sh` and `cd src/thinfloe-menu && systemd-run --user --scope -q -p MemoryMax=4G -- cargo test --locked -j 4 release::`. Expected: the shell cases FAIL; the Rust cases already PASS (no logic change in `newer`).
- [ ] **Step 3: Implement** in the Makefile `TF_VERSION ?=` the dev form (git `rev-parse --short HEAD`, `.dirty` from `git diff --quiet HEAD`), the `$(error …)` check, the `.version-kind` stamp as a prerequisite of `$(KERNEL) $(INITRD) $(SQUASHFS)`, and `-e TF_VERSION=$(TF_VERSION)` on the export run; in `export-rootfs.sh` the check (`^[0-9]+\.[0-9]+\.[0-9]+(-dev\+[0-9a-f]+(\.dirty)?)?$`, one line) and the write; remove the Dockerfile layer and `RELEASE` from `.ubuntu-container`; update `test-container-inputs.sh` (RELEASE no longer rebuilds the container; a changed `.version-kind` re-exports). `scripts/publish.sh` keeps refusing anything but `X.Y.Z`.
- [ ] **Step 4: Run** `test/integration/test-release-version.sh`, `test/integration/test-container-inputs.sh`, then `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test`. Expected: PASS; `cat artifacts/rootfs/version` shows `4.2.0-dev+<sha>`.
- [ ] **Step 5: Docs.** `CLAUDE.md`: replace "Raise `RELEASE` before a release" with: the release pull request sets `RELEASE`; local builds are `X.Y.Z-dev+<sha>`; `make … TF_VERSION=X.Y.Z` reproduces a release. `CHANGES.md` `### Changed`: the menu header of a self-built image shows a version such as `4.2.0-dev+85034f2`, and such an image is offered the current release.
- [ ] **Step 6: Commit** `build: local builds carry a dev version below the release`.

### Task 5: Download from GitHub, pinned, with SHA256SUMS (R4, D7 to D10)

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`DEFAULT_URL` line 89, `expected_download_sha256` 1194-1216, `_ab_upgrade` 1661-1669, `_fresh_install_open` 2155-2175, `_batch_run` 2730ff, interactive main 2810ff, usage text 12-36)
- Test: `test/unit/test_upgrade_batch.py`, `test/unit/test_upgrade_image.py`
- Modify: `CHANGES.md`

**Interfaces:**
- Produces in `thinfloe-upgrade`:
  - `DEFAULT_URL = "https://github.com/oposs/thinfloe/releases/latest/download"`
  - `RELEASE_DOWNLOAD = "https://github.com/oposs/thinfloe/releases/download"`
  - `CHECKSUM_ASSET = "SHA256SUMS"`
  - `pin_source(source: str) -> str`: for `DEFAULT_URL`, reads the version once with `_fetch_version` and returns `f"{RELEASE_DOWNLOAD}/v{version}"`; raises `WriteFailed("the newest release could not be read: <error>")` when the version cannot be read; any other source is returned unchanged.
  - `checksum_required(base_url: str) -> bool`: true when `base_url` starts with `RELEASE_DOWNLOAD + "/v"`.
  - `parse_sha256sums(text: str, name: str) -> Optional[str]`: the lower-case hash of the line naming `name` (two-space or ` *` form), `None` when absent, malformed or listed twice.
  - `expected_download_sha256(base_url: str, name: str) -> Optional[str]`: `SHA256SUMS` first; only when `checksum_required` is false, falls back to `<name>.sha256`.
- `check` keeps reading `<source>/version` and `thinfloe-usb.img.zst` from the unpinned source (the status file still says `source=` the default).

- [ ] **Step 1: Write the failing tests** in `test_upgrade_batch.py`: `TestExpectedDownloadSha` gains `test_sha256sums_line`, `test_sha256sums_missing_name_is_none`, `test_sha256sums_wins_over_single_file`, `test_pinned_source_does_not_fall_back` (base `file://…/download/v4.3.0` with `RELEASE_DOWNLOAD` patched to `file://…/download`, only `<name>.sha256` present: `None`); `TestPinSource`: `test_default_is_pinned_to_the_version_read` (`_fetch_version` patched to `("4.3.0", "")`: result ends `/download/v4.3.0`), `test_custom_source_unchanged`, `test_unreadable_version_fails`; `TestRedirects` with a `http.server` in a thread answering `/latest/download/version` with 302 to `/hop` and `/hop` with 302 to `/download/v4.3.0/version`, and the same chain for the image: `test_follows_redirect_chain` asserts `_fetch_version` returns `4.3.0` and `image_size` returns the zstd frame size of a small test image; `TestFreshInstall` gains `test_required_checksum_missing_fails_before_write` (pinned base, `expected_download_sha256` returns `None`: no `stream_image` call, result failed, stick untouched) and `test_write_uses_pinned_tag_for_every_file` (every URL handed to curl, `stream_image` and `image_size` starts with the pinned base). In `test_upgrade_image.py`, the A/B upgrade gets the same two cases (`test_required_checksum_missing_fails_before_write`, `test_write_uses_pinned_tag_for_every_file`: `download_firmware` and each checksum lookup use the pinned base; nothing under `tl.new` is written).
- [ ] **Step 2: Run** `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests/task5 python3 -m unittest test.unit.test_upgrade_batch test.unit.test_upgrade_image -v`. Expected: the new cases FAIL.
- [ ] **Step 3: Implement** the interface above. Both write paths call `pin_source` once at their start (batch `write` and the interactive path after `select_source`), fetch the expected checksums before the first byte reaches the stick, fail with `the release has no checksum for <name>` when one is missing and `checksum_required`, and otherwise keep today's "checksum not checked" log line. A mismatch keeps today's messages. The usage text names the new default.
- [ ] **Step 4: Run** the same unittest command, then `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test-unit`. Expected: PASS.
- [ ] **Step 5: CHANGES.md** `### Changed`: the daily check, the Upgrade tile and `thinfloe-upgrade` take releases from `https://github.com/oposs/thinfloe/releases`; every file of an upgrade or a fresh install comes from the release the check found, and a release without a matching checksum is refused before the stick changes.
- [ ] **Step 6: Commit** `upgrade: read releases from GitHub, pinned to one tag, checked against SHA256SUMS`.

### Task 6: publish.sh mirrors a downloaded release (D12)

**Files:**
- Modify: `scripts/publish.sh`, `test/integration/test-release-version.sh` (publish cases)

**Interfaces:**
- Produces: `scripts/publish.sh <release-dir>`, where the directory holds the six assets of one GitHub release.

- [ ] **Step 1: Write the failing tests** (the stubbed `rsync` of the existing cases stays): refuses with no argument; refuses when one of the six files is missing; refuses when `sha256sum -c SHA256SUMS` fails; refuses a `version` that is not one line `X.Y.Z` (dev form included); on success the rsync arguments list the six files plus `thinfloe-usb.img.zst.sha256`, `dbrrg-usb.img.zst` and `dbrrg-usb.img.zst.sha256`, with `-H`, and the target is still `oetiker@web-volki-01-adm:public_html/dbrrg/`.
- [ ] **Step 2: Run** `test/integration/test-release-version.sh`. Expected: new cases FAIL.
- [ ] **Step 3: Implement**: the checks above, then the per-file `thinfloe-usb.img.zst.sha256` and the hard-linked `dbrrg-usb.img.zst` plus its `.sha256` in a work directory under the release directory, then rsync. The build-tree checks (artifacts, file times) go.
- [ ] **Step 4: Run** the test script. Expected: PASS.
- [ ] **Step 5: Commit** `publish: mirror a downloaded GitHub release to tobi.oetiker.ch` (no CHANGES entry: nothing a user sees).

### Task 7: Release assets script (D5)

**Files:**
- Create: `scripts/release-assets.sh`, `test/integration/test-release-assets.sh`
- Modify: `Makefile` (target `release-assets`, script in `test`)

**Interfaces:**
- Produces: `scripts/release-assets.sh <artifact-dir> <out-dir> <version>` and `make release-assets TF_VERSION=X.Y.Z`, writing the six assets into `artifacts/release/`.

- [ ] **Step 1: Write the failing test** with fake artifacts (small files): the out dir holds exactly the six names; `SHA256SUMS` lists the other five and `sha256sum -c` passes inside the out dir; refuses when `artifacts/rootfs/version` differs from `<version>`, when `<version>` is a dev form, and when any file is 2 GiB or more (use a sparse file from `truncate -s 2G`).
- [ ] **Step 2: Run** it. Expected: FAIL (script missing).
- [ ] **Step 3: Implement** with hard links (`ln -f`) into the out dir; the size limit is `2147483648` bytes.
- [ ] **Step 4: Run** it, then `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test`. Expected: PASS.
- [ ] **Step 5: Commit** `build: collect the release assets and their SHA256SUMS`.

### Task 8: `no-changelog` label (R5). Owner confirms at the time; controller only

- [ ] **Step 1 (owner confirms):** `python3 /home/oetiker/.claude/plugins/cache/oposs-plugins/repo-infra/2.13.0/skills/repo-infra/scripts/repo_infra apply --item no-changelog-label`.
- [ ] **Step 2: Verify:** `repo_infra check` lists `no-changelog-label ok`.

Before Task 9, the controller lands Tasks 2 to 7 on `main` through pull requests (each push and merge: owner confirms at the time).

### Task 9: Onboarding files and the CHANGES roll (R6, D1 to D3, D16 to D18)

**Files:**
- Create: `.github/workflows/ci.yml`, `ci-local.yml`, `changelog.yml`, `release-pr.yml`, `release-build.yml`, `release-build-local.yml`, `release-publish.yml`, `.github/repo-infra.json`
- Create via `repo_infra apply --item dependabot`: `.github/dependabot.yml`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `make test-unit`, `make test`, `make image TF_VERSION=…`, the `qemu-smoke*` targets, `make release-assets TF_VERSION=…` (Task 7); the split recorded in `docs/release-ci.md` (Task 2).

- [ ] **Step 1: Copy** `changelog.yml` and `release-pr.yml` word for word from `…/repo-infra/2.13.0/skills/repo-infra/assets/callers/`; model `ci.yml`, `release-build.yml` and `release-publish.yml` on `references/examples/mtuspy/`.
- [ ] **Step 2: `ci.yml`**: jobs `local` (calls `./.github/workflows/ci-local.yml` with `ref`), `release-pr-current`, `release-verdict`, `ci-passed` (from `assets/callers/ci-passed.yml`, `needs: [local, release-pr-current, release-verdict]`); every job but `ci-passed` and `release-verdict` carries the `if:` of `assets/callers/release-pr-skip.yml`.
- [ ] **Step 3: `ci-local.yml`**: per `docs/release-ci.md`, by default a job `unit` (`make test-unit MEMCAP=`) and a job `image-guards` (`make test MEMCAP=`, after any disk cleanup Task 2 found necessary). If Task 2 found that cargo can run through `ri-ci-rust` on `src/thinfloe-menu`, call that piece from `ci.yml` instead of running cargo here, and add `"dependabot": {"cargo": ["/src/thinfloe-menu"]}`.
- [ ] **Step 4: `release-build.yml`** calls `./.github/workflows/release-build-local.yml` with `version` and `ref`. **`release-build-local.yml`**: one job (or the split Task 2 recorded) that enables KVM (the udev rule of Task 2), runs `make image TF_VERSION=${{ inputs.version }}`, the smoke targets of D17 that passed in Task 2, `make release-assets TF_VERSION=${{ inputs.version }}`, and uploads each of the six files as artifact `release-asset-<name>`.
- [ ] **Step 5: `release-publish.yml`**: `publish` (`ri-publish-tag`) and `finalize` (`needs: [publish]`), as in mtuspy without `gitea`.
- [ ] **Step 6: `.github/repo-infra.json`**: `"moving_major_tag": false`; `version_files` with the one entry `{"path": "RELEASE", "pattern": "^[0-9][^\\n]*$", "replacement": "$VERSION", "verify": "^$VERSION$"}`; `release_files: []`; `release_assets` the six names; `exceptions`: `linux-packages` and `homebrew` ("ThinFloe ships a bootable image, not a program installed through a package manager"), `man-page` ("the command line tools run inside the image; the README documents the ones used over SSH").
- [ ] **Step 7: Install dependabot:** `repo_infra apply --item dependabot`, then cherry-pick its commit onto the task branch.
- [ ] **Step 8: Roll CHANGES.md:** every entry that 4.2.0 shipped moves under `## 4.2.0 - 2026-11-11` (all entries present at `db60ef6`; check with `git diff db60ef6 -- CHANGES.md`); entries added by Tasks 3 to 7 stay under `## [Unreleased]`, which keeps `### New`, `### Changed`, `### Fixed`.
- [ ] **Step 9: Run** `python3 …/repo_infra check`. Expected: every item `ok` except `branch-protection`, `required-checks` and `delete-merged-branches`. Also run `actionlint` if installed.
- [ ] **Step 10: Commit** `ci: onboard the repo-infra release flow`.

### Task 10: README and CLAUDE.md to the project shape (part of R6)

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `docs/release-ci.md`
- Create: `docs/screenshot-menu.png`

- [ ] **Step 1: README**: the two badges of `project-shape.md` "Badges" below the title for `oposs/thinfloe`; an install section: download `thinfloe-usb.img.zst` from `https://github.com/oposs/thinfloe/releases/latest`, write it with `zstd -dc thinfloe-usb.img.zst | sudo dd of=/dev/sdX bs=4M conv=fsync status=progress`, one sentence on finding the device; the screenshot near the top; remove every mention of tobi.oetiker.ch as the release source and of `publish.sh` as the release path.
- [ ] **Step 2: Screenshot**: boot the image with `make qemu-test` (or the smoke QEMU line with a display) and save the menu at 1920x1080 as `docs/screenshot-menu.png`.
- [ ] **Step 3: CLAUDE.md**: the release paragraph (lines 24-38) describes Create release PR, the six assets and `docs/release-ci.md`; `publish.sh` is named only as the one-time bridge mirror.
- [ ] **Step 4: Review** the changed prose against the `repo-infra:writing-style` skill.
- [ ] **Step 5: Commit** `docs: README and CLAUDE.md for releases on GitHub`.

### Task 11: Land the onboarding pull request (R6). Owner confirms at the time; controller only

- [ ] **Step 1 (owner confirms):** push the branch with Tasks 9 and 10 and open the pull request with `gh pr create --label no-changelog` (label at creation, never added later).
- [ ] **Step 2:** wait for `ci-passed` on the pull request; fix failures through the implementer of Task 9.
- [ ] **Step 3 (owner confirms):** merge.
- [ ] **Step 4: Verify** on `main`: the `Publish release (automatic)` run says `v4.2.0 is released - nothing to do`; `ci-passed` is green on the merge commit.

### Task 12: Required checks (R7). Owner confirms at the time; controller only

- [ ] **Step 1 (owner confirms):** `repo_infra apply --item required-checks`.
- [ ] **Step 2: Verify:** `repo_infra check` shows `branch-protection ok` and `required-checks ok`; a direct push to `main` is refused.

### Task 13: Delete merged branches (R8). Owner confirms at the time; controller only

- [ ] **Step 1 (owner confirms):** `repo_infra apply --item delete-merged-branches`.
- [ ] **Step 2: Verify:** `repo_infra check` exits 0.

### Task 14: Image size reduction (R9). Placeholder

Scope, design and plan follow the separate size audit (611 MB today: `ramroot.sqsh` 494, `initrd.img` 100, `vmlinuz` 17). Task 15 does not start before that work is merged on `main`.

### Task 15: Bridge release 4.3.0 (R10). Owner confirms at the time; controller only

- [ ] **Step 1 (owner confirms):** dispatch **Create release PR** with `release_type: feature` (`gh workflow run release-pr.yml --repo oposs/thinfloe -f release_type=feature`).
- [ ] **Step 2: Check the draft:** six assets plus `release-build.json`; `version` reads `4.3.0`; `sha256sum -c SHA256SUMS` passes on the downloaded assets.
- [ ] **Step 3 (owner confirms):** merge the release pull request; the publish run ends green and `releases/latest/download/version` reads `4.3.0`.
- [ ] **Step 4:** on the downloaded release, rerun the Task 2 curl probes against `thinfloe-usb.img.zst` (HEAD and `-r 0-17` on a large asset).
- [ ] **Step 5 (owner confirms):** on a stick with 4.2.0, set the Upgrade screen's custom source to `https://github.com/oposs/thinfloe/releases/download/v4.3.0` and upgrade; the stick boots 4.3.0, and its header shows `4.3.0`.

### Task 16: Mirror the bridge to tobi.oetiker.ch (R11). Owner confirms at the time; controller only

- [ ] **Step 1:** `gh release download v4.3.0 --repo oposs/thinfloe --dir /scratch/oetiker/claude-tmp/thinfloe-tests/bridge`.
- [ ] **Step 2 (owner confirms):** `scripts/publish.sh /scratch/oetiker/claude-tmp/thinfloe-tests/bridge`.
- [ ] **Step 3: Verify:** `curl -fsSL https://tobi.oetiker.ch/dbrrg/version` prints `4.3.0`; `make qemu-smoke-upgrade` against a 4.2.0 stick image and a local copy of the bridge directory shows 4.3.0 offered and installed, and a following `thinfloe-upgrade check` reports `source=https://github.com/oposs/thinfloe/releases/latest/download`.

### Task 17: Retire publish.sh (R12, D13)

**Files:**
- Delete: `scripts/publish.sh`
- Modify: `test/integration/test-release-version.sh` (drop the publish cases), `CLAUDE.md`, `README.md`

- [ ] **Step 1:** delete the script and its test cases; remove `publish.sh` and tobi.oetiker.ch from `CLAUDE.md` and `README.md`, keeping one sentence in `docs/release-ci.md` that tobi.oetiker.ch/dbrrg holds the 4.3.0 bridge for old sticks and is never updated.
- [ ] **Step 2: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test`. Expected: PASS.
- [ ] **Step 3: Commit** `build: retire publish.sh; releases exist only on GitHub`.
- [ ] **Step 4 (owner confirms):** controller opens the pull request with the `no-changelog` label at creation and merges it once `ci-passed` is green.
