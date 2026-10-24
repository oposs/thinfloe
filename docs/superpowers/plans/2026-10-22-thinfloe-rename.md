# ThinFloe Part 1: Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every name `dbrrg` becomes `thinfloe` / `tf_` / `TF_` / `X-TF-`, and a stick in the field that boots ThinFloe keeps its keyboard, display, password, SSH host keys, network config and user tiles, and can still fall back to 3.x.

**Architecture:** One scripted, mechanical rename commit (file moves plus a fixed substitution table), followed by small hand-written commits for the parts the table cannot do: the home-file migration inside the initramfs restore, the old names that must still be accepted, a frozen copy of the 3.x scripts to prove the fallback, and the build-level proof (image, `make test`, QEMU smokes with a 3.x home on the stick).

**Tech Stack:** POSIX sh / bash (dracut module, overlay scripts, tests), Rust (menu crate), Python (`thinfloe-upgrade` and its unit test), GNU make, podman, QEMU + mtools.

**Spec:** `docs/superpowers/specs/2026-10-21-thinfloe-design.md`, section "Part 1: Rename" (and "Anti-goals"). Read it before starting.

## Global Constraints

- Naming rule (spec): `thinfloe` where a name stands alone (programs, services, directories, crate, host name prefix, dotfiles); `tf_` / `TF_` for shell functions and environment variables; `X-TF-` for desktop-file keys. Product name in prose and on screen: `ThinFloe`.
- Host name: `thinfloe-` plus the same six hex digits as before (`thinfloe-3f9a2c`).
- labwc package version suffix `+thinfloe1` (`0.9.3-1+thinfloe1`).
- Unchanged this round (decided, or needed by sticks in the field): everything on the ESP (`/config/machine-id`, `tl/`, `tl.old/`, `tl.new/`, `home.tar.gz`, `syslinux.cfg` and its labels), the partition label `EFI-SYSTEM`, the boot server paths (`home.pkg?mac=…`), the upgrade server `https://tobi.oetiker.ch/dbrrg` (`DEFAULT_URL`) and its rsync target `public_html/dbrrg/`, `VERSION := 3.0.0` in the `Makefile` (Part 5 changes it).
- Shipped tile **file names** stay (`10-thinlinc.desktop` … `50-upgrade-image.desktop` … `81-poweroff.desktop`): a user reword matches a shipped tile by file name, so renaming `50-upgrade-image.desktop` would silently orphan every user's reword of it. Its `Exec` changes to `thinfloe-upgrade`.
- `https://github.com/oposs/dbrrg` URLs stay until the owner renames the repository (Task 8).
- `docs/superpowers/` and `docs/reports/` are history: not edited, except this plan.
- The migration code stays until removed on purpose; it carries no release-count expiry.
- The initramfs has only the tools `90thinfloe/module-setup.sh` installs plus dracut's base set (`cat rm mv sleep` are in the test sandbox). No `readlink`, `cut`, `basename`, `head`, `chown`.
- All Standing Constraints in `CLAUDE.md` hold (no shipped password, `--no-hostonly`, zero keybindings, masked `ssh.socket`, network config installed by the initramfs, `/etc/hostname` written by the initramfs, …).
- Builds and tests use at most 4 cores (`BUILD_JOBS` in the `Makefile`); cargo tests run under `MEMCAP` (already in `make test-unit`). Containers: podman.
- `CHANGES.md` entries follow `~/.claude/CLAUDE.md` "On writing Changelog Entries": reader-observable effect, at most three sentences, under `## [Unreleased]` → `### New` or `### Changed` / `### Fixed`.
- Do not push, publish, or rename the GitHub repository without the owner's explicit go.

## Review Focus

1. **A home archive whose members carry no `./` prefix** (hand-made, or written by another tar invocation): the migration must still see the old names in it. Test in Task 2.
2. **The second ThinFloe boot**: the archive now holds `.thinfloe-*` files plus `.dbrrg-*` symlinks. Nothing may be renamed again, and the "came from 3.x" marker must not be written. Test in Task 2.
3. **An archive holding both an old and a new name as real files** (a 3.x fallback boot ran `dbrrg-password --clear` and then `--set`, which replaced the link with a file): the spec rule says the new name wins; the old file is replaced by the link. Test in Task 2, and the behaviour is stated in `CLAUDE.md` (Task 7).
4. **A user reword of `50-upgrade-image.desktop`** in `~/.config/thinfloe/menu/` still rewords the Upgrade tile. Test in Task 3.
5. **A boot whose restore was absent or failed** still gets the `.dbrrg-*` links to the default files, so a fallback to 3.x after that boot finds `.dbrrg-environment`; no marker is written. Test in Task 2.

---

### Task 1: Mechanical rename

**Files:** every tracked file outside `docs/superpowers/`, `docs/reports/` and `CHANGES.md` that contains `dbrrg` in its path or content (about 80 files; `git ls-files` lists them). Notable moves:

| Old path | New path |
|---|---|
| `src/dbrrg-menu/` | `src/thinfloe-menu/` |
| `overlay/etc/dbrrg/` | `overlay/etc/thinfloe/` |
| `overlay/usr/libexec/dbrrg/` | `overlay/usr/libexec/thinfloe/` |
| `overlay/usr/lib/dracut/modules.d/90dbrrg/` | `…/90thinfloe/` (and inside: `parse-thinfloe.sh`, `thinfloe-lib.sh`, `thinfloe-cleanup.sh`, `thinfloe-after-cmdline.conf`) |
| `overlay/usr/bin/dbrrg-{session,save-home,password,ssh-hostkeys,compose-labwc-config}` | `overlay/usr/bin/thinfloe-…` |
| `overlay/usr/bin/upgrade-image` | `overlay/usr/bin/thinfloe-upgrade` |
| `overlay/etc/systemd/system/dbrrg-{password,ssh-hostkeys,wait-kms}.service` | `thinfloe-….service` |
| `overlay/etc/profile.d/10-dbrrg-session.sh` | `10-thinfloe-session.sh` |
| `overlay/etc/ssh/sshd_config.d/01-dbrrg.conf`, `overlay/etc/sysctl.d/99-dbrrg-memory.conf` | `01-thinfloe.conf`, `99-thinfloe-memory.conf` |
| `overlay/home/tluser/.dbrrg-environment`, `.dbrrg-sessionrc` | `.thinfloe-environment`, `.thinfloe-sessionrc` |

**Interfaces:**
- Produces, for every later task: `tf_log`, `tf_restore_home_from_file`, `tf_restore_home_from_url`, `tf_write_hostname`, `tf_finalize_upgrade` and the other `dbrrg_*` functions as `tf_*` in `/lib/thinfloe-lib.sh`; `TF_BASE=/run/thinfloe`, `TF_STATE`, `TF_STORAGE`, `TF_LAYERS`; `root=thinfloe`; `/tmp/thinfloe-ramroot`; crate `thinfloe-menu` (lib `thinfloe_menu`, binary `thinfloe-menu`, app_id `thinfloe-menu`); menu env `TF_MENU_USER_DIR`, `TF_MENU_SHIPPED_DIR`, `TF_STATE_DIR`, `TF_SAVE_HOME`; menu reads `/etc/thinfloe/menu` and `~/.config/thinfloe/menu`; icons under `/usr/share/thinfloe/icons`; `thinfloe-session` reads `TF_IDLE_TIMEOUT`.

- [ ] **Step 1: Record the baseline.** On the unchanged tree run `make test-unit` and each offline integration script (those in the `test:` target that pass without a built rootfs: `test-labwc-config-merge.sh`, `test-initramfs-home.sh`, `test-boot-smoke-check.sh`, `test-ssh-hostkeys.sh`, `test-save-home.sh`, `test-password.sh`, `test-field-report.sh`, `test-session-lifecycle.sh`, `test-container-stamp.sh`, `test-format-bytes.sh`, `test-make-clean.sh`). Write down which pass and the `ok` count of each. Any failure here is reported to the controller before going on; it is not "pre-existing and fine".

- [ ] **Step 2: Move the files** with `git mv`, every path containing `dbrrg` (table above), plus `upgrade-image` → `thinfloe-upgrade`. Not moved: `test/unit/test_upgrade_image.py`, `overlay/etc/dbrrg/menu/50-upgrade-image.desktop` keeps its base name inside the new `overlay/etc/thinfloe/menu/`.

- [ ] **Step 3: Substitute content**, in this order, over the moved file set (excluding `docs/superpowers/`, `docs/reports/`, `CHANGES.md`, `Cargo.lock`, and binary files):

  1. protect: `oposs/dbrrg` → `@@GH@@`, `tobi.oetiker.ch/dbrrg` → `@@SRV@@`, `public_html/dbrrg` → `@@PUB@@`, `50-upgrade-image.desktop` → `@@T50@@`
  2. `X-DBRRG-` → `X-TF-`
  3. `DBRRG_` → `TF_` (this also turns `_DBRRG_LIB_LOADED` into `_TF_LIB_LOADED`)
  4. `dbrrg_menu` → `thinfloe_menu` (the Rust lib name), then `dbrrg_` → `tf_`
  5. `upgrade-image` → `thinfloe-upgrade`
  6. `dbrrg` → `thinfloe`
  7. restore the four protected strings.

  Then `git grep -n -i dbrrg -- ':!docs/superpowers' ':!docs/reports' ':!CHANGES.md'` must list only `oposs/dbrrg`, `tobi.oetiker.ch/dbrrg` and `public_html/dbrrg`.

- [ ] **Step 4: Hand pass over `git diff`.** Where `dbrrg` was the product's name in prose or on screen, write `ThinFloe`: the `SAY` banner and `MENU LABEL` in `configs/syslinux.cfg`, unit `Description=` lines, `thinfloe-upgrade`'s printed text (`Install ThinFloe (overwrites entire drive)`, the `[ThinFloe]` drive tag), `make help`'s title, the `foot` window titles in `thinfloe-session`, `Author: ThinFloe maintainers` in the two labwc patches, the comment headers. Identifiers, paths and log prefixes (`thinfloe:`) stay lower case; the smoke scripts grep for those prefixes. `CLAUDE.md` and `README.md` get the same treatment; their intro paragraph is rewritten in Task 7.

- [ ] **Step 5: Update the lock file root entry.** `cd src/thinfloe-menu && cargo update --workspace --offline`. `git diff Cargo.lock` must show only the root package name changing.

- [ ] **Step 6: Run the baseline set again.** Same scripts as Step 1, same pass set, same `ok` counts. Expected: identical. A changed count means a test was renamed away or lost; find out which.

- [ ] **Step 7: CHANGES.md.** Under `### Changed`: the host name prefix (`dbrrg-` → `thinfloe-`; DNS entries, monitoring or ThinLinc server settings that know a machine by name need updating), and the renamed commands an admin types (`sudo thinfloe-password`, `thinfloe-upgrade`, `thinfloe-menu --check`), with their old names. Also: boot and session log lines start with `thinfloe:` instead of `dbrrg:`.

- [ ] **Step 8: Commit.** `git add -A` (check `git status` first: nothing untracked from the build) and commit `refactor: rename dbrrg to thinfloe` with a body listing the substitution table and the four protected strings.

---

### Task 2: Migrate the home files in the initramfs restore

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (new functions after `tf_restore_home_from_url`; calls in `tf_restore_home_from_file` and `restore_home`)
- Test: `test/integration/test-initramfs-home.sh`

**Interfaces:**
- Consumes: `tf_restore_home_from_file <archive> <target-home>`, `tf_restore_home_from_url <url> <target-home> [timeout] [tmp-file]`, `restore_home <newroot> <efi-mount> <state-dir> <ramroot>`, `tf_log` (Task 1).
- Produces:
  - `TF_HOME_RENAMES`: newline-separated `old:new:link-target` triples, paths relative to the home:
    `.dbrrg-environment:.thinfloe-environment:.thinfloe-environment`, `.dbrrg-sessionrc:.thinfloe-sessionrc:.thinfloe-sessionrc`, `.dbrrg-password:.thinfloe-password:.thinfloe-password`, `.dbrrg-ssh-host-keys:.thinfloe-ssh-host-keys:.thinfloe-ssh-host-keys`, `.config/dbrrg:.config/thinfloe:thinfloe`
  - `tf_migrate_home_names <home> <archive> [<renamed-file>]` → always 0. For each triple: if the archive lists `new` → nothing; else if it lists `old` and `<home>/old` exists and is not a symlink → `rm -rf <home>/new` (an image default), `mv <home>/old <home>/new`, log `thinfloe: renamed ~/old to ~/new`, append `old` to `<renamed-file>` when given. Archive membership: list the archive once (`tar -tzf`), match each name exactly as `./name`, `name`, `./name/` or `name/` (`grep -qxF`, no regex).
  - `tf_link_old_home_names <home>` → always 0. For each triple whose `<home>/new` exists: `rm -rf <home>/old` and `ln -s <link-target> <home>/old` (relative, so it resolves the same in the initramfs and after the pivot).
  - `tf_restore_home_from_file` gains an optional third argument `<renamed-file>` and calls `tf_migrate_home_names` after a successful extraction; `tf_restore_home_from_url` gains an optional fifth argument and passes it through.
  - `restore_home` passes `<state-dir>/home-renamed` and calls `tf_link_old_home_names "$_rh_home"` after the outcome is recorded, on every path (ok, absent, failed).
  - `/run/thinfloe/state/home-renamed`: one old name per line, written only on a boot that renamed a real file, i.e. the home came from 3.x. Part 2's ThinLinc auto-tick reads it.

- [ ] **Step 1: Write the failing tests** in `test-initramfs-home.sh`, in a new section `# --- home names: dbrrg -> thinfloe ---`. Helper: build a fake `newroot/home/tluser` holding the 4.0 default files (`.thinfloe-environment` = `default-env`, `.thinfloe-sessionrc` = `default-rc`), and an archive from a directory with `tar -C dir -czf archive .`.
  - `3.x archive with a user XKB layout over the 4.0 default home`: archive has `.dbrrg-environment` = `XKB_DEFAULT_LAYOUT=ch`, `.dbrrg-sessionrc`, `.dbrrg-password`, `.dbrrg-ssh-host-keys/ssh_host_ed25519_key`, `.config/dbrrg/menu/x.desktop`. After `restore_home` (USB path, archive as `home.tar.gz` in the fake ESP): `.thinfloe-environment` contains `XKB_DEFAULT_LAYOUT=ch`; `.thinfloe-ssh-host-keys/ssh_host_ed25519_key` and `.config/thinfloe/menu/x.desktop` exist; each of the five old names is a symlink (`-L`) with `readlink` equal to its link target; `state/home-renamed` lists all five old names; `state/home-restore` is `ok`.
  - `4.0 archive (second boot)`: archive has `.thinfloe-environment` = `mine` and `.dbrrg-environment` as a symlink to it. After restore: content `mine`, link intact, `state/home-renamed` does not exist. (Review Focus 2)
  - `both names as real files`: archive has `.dbrrg-password` = `old` and `.thinfloe-password` = `new`. After restore: `.thinfloe-password` is `new`, `.dbrrg-password` is a link to it. (Review Focus 3)
  - `members without ./`: archive made with `tar -C dir -czf archive .dbrrg-environment`. The rename happens. (Review Focus 1)
  - `absent archive`: no `home.tar.gz`. `.dbrrg-environment` and `.dbrrg-sessionrc` are links to the defaults; no link for names whose new file does not exist; no `home-renamed`. Same for a corrupt archive (`home-restore` = `failed`). (Review Focus 5)
  - `netboot`: the same 3.x archive served through the test's existing `curl` stub to `restore_home` with an `http://` ramroot and a recorded `boot-mac` → the same renames and `home-renamed` (spec item 5).
  - `idempotent`: `tf_migrate_home_names` and `tf_link_old_home_names` run twice give the same tree.
  - Add `tf_migrate_home_names` and `tf_link_old_home_names` to the existing initramfs-PATH sandbox block (`PATH="$BIN"`), on the 3.x archive case, with empty stderr and no warnings.

- [ ] **Step 2: Run** `test/integration/test-initramfs-home.sh`. Expected: the new cases FAIL (functions not defined), all earlier cases still `ok`.

- [ ] **Step 3: Implement** the functions and calls listed under Interfaces in `thinfloe-lib.sh`, inside the `_TF_LIB_LOADED` guard, each with a comment block in the file's style that states the precedence rule ("decided by what is in the archive, not by what is on disk") and why the old names stay as links (3.x in `tl.old/`).

- [ ] **Step 4: Run** `test/integration/test-initramfs-home.sh`. Expected: every case `ok`, exit 0.

- [ ] **Step 5: CHANGES.md** under `### Changed`: the user files in the home directory have new names (`~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys`, `~/.config/thinfloe/`); the first boot renames the old ones, and the old names stay as links so the previous version still boots with the same settings.

- [ ] **Step 6: Commit** `feat(initramfs): rename the home's dbrrg files on restore, keep the old names as links`.

---

### Task 3: The menu reads `X-DBRRG-*` keys as `X-TF-*`

**Files:**
- Modify: `src/thinfloe-menu/src/tiles.rs` (`tile_from`, `SHIPPED_ONLY_KEYS`, the "runs nothing" check near line 310)
- Test: `src/thinfloe-menu/src/tiles.rs` (its `#[cfg(test)]` module)

**Interfaces:**
- Produces: `fn tf_key<'a>(entry: &'a Entry, suffix: &str) -> Option<&'a str>` — `X-TF-<suffix>` if present, else `X-DBRRG-<suffix>`. Used for `Action`, `Save-On-Exit`, `Multiple`, for shipped and user files alike. `SHIPPED_ONLY_KEYS` lists both spellings of the three keys (8 entries), so a reword naming an old key still shows it as ignored.

- [ ] **Step 1: Write the failing tests:**
  - `old_keys_are_read`: a user file `Exec=u` + `X-DBRRG-Multiple=true` → `multiple == true`; a shipped file `X-DBRRG-Action=save-home` → `Action::SaveHome`.
  - `new_key_wins_over_old`: `X-TF-Multiple=false` + `X-DBRRG-Multiple=true` → `multiple == false`.
  - `old_action_key_still_limited_for_user_files`: user file `X-DBRRG-Action=reboot` + `Exec=y` → broken, message contains `may only use the run action`.
  - `reword_names_old_shipped_only_keys`: a reword setting `X-DBRRG-Multiple` → `Origin::Reworded { ignored }` contains `"X-DBRRG-Multiple"`.
  - `old_action_key_counts_for_the_runs_nothing_rule`: a user file matching no shipped name, with `Name=X` and `X-DBRRG-Action=run` and no `Exec` → it is drawn as a broken tile (`has no Exec`), not dropped as "runs nothing" (mirror the existing test of that rule near `tiles.rs:302`).
  - `upgrade_tile_keeps_its_file_name` (Review Focus 4): the existing test that reads `../../overlay/etc/thinfloe/menu` also asserts a file `50-upgrade-image.desktop` exists, its `Exec` is `thinfloe-upgrade`, and a user file of that name with `Name=Neu` rewords it.

- [ ] **Step 2: Run** `make test-unit`. Expected: the new tests FAIL.

- [ ] **Step 3: Implement `tf_key`** and use it at every former `X-DBRRG-*` lookup (Task 1 turned those into `X-TF-*` string lookups).

- [ ] **Step 4: Run** `make test-unit`, then `cargo fmt --check` and `cargo clippy --locked -j 4 -- -D warnings` in `src/thinfloe-menu`. Expected: all pass.

- [ ] **Step 5: CHANGES.md** under `### Changed`: tile files use `X-TF-Action`, `X-TF-Save-On-Exit`, `X-TF-Multiple`; files with the old `X-DBRRG-` keys keep working, and the new key wins when a file has both.

- [ ] **Step 6: Commit** `feat(menu): accept X-DBRRG-* tile keys as X-TF-*`.

---

### Task 4: Old names the boot, the session and the upgrade still accept

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/parse-thinfloe.sh`, `overlay/usr/bin/thinfloe-session`, `overlay/usr/bin/thinfloe-upgrade` (`IDENTITY_EXCLUDES`), `overlay/home/tluser/.thinfloe-sessionrc` (its comment on the variable)
- Test: `test/integration/test-initramfs-home.sh` or a new `test/integration/test-cmdline.sh` (builder's choice; add a new script to the `test:` target), `test/integration/test-session-lifecycle.sh`, `test/unit/test_upgrade_image.py`

**Interfaces:**
- Produces: debug mode on `rd.thinfloe.debug` **or** `rd.dbrrg.debug` (the debug entry of every 3.x `syslinux.cfg` carries the old one, and an A/B upgrade never rewrites it; it is the only old-name parameter in the file's whole history). `TF_IDLE_TIMEOUT` defaults to `DBRRG_IDLE_TIMEOUT`, then `300`, because a user's `~/.dbrrg-sessionrc` (now `~/.thinfloe-sessionrc`) may set the old name. `IDENTITY_EXCLUDES = ("./.thinfloe-ssh-host-keys", "./.dbrrg-ssh-host-keys", "./wg0.conf")`.

- [ ] **Step 1: Write the failing tests:**
  - cmdline: source `parse-thinfloe.sh` with stubbed `getarg`/`getargbool` (as dracut-lib provides them; read `/proc/cmdline` from a test file) and a writable `/tmp` substitute; `rd.dbrrg.debug` and `rd.thinfloe.debug` each create the debug marker, neither → no marker. If the hook's fixed `/tmp` paths make this impractical offline, make the paths overridable the way `thinfloe-save-home` does (`TF_…:-default`) rather than skipping the test.
  - `test-session-lifecycle.sh`: stub `swayidle` to record its arguments; a home whose `.thinfloe-sessionrc` sets `DBRRG_IDLE_TIMEOUT=900` → swayidle got `timeout 900`; setting both `TF_IDLE_TIMEOUT=600` and `DBRRG_IDLE_TIMEOUT=900` → `timeout 600`; neither → `timeout 300`.
  - `test_upgrade_image.py`: `test_old_host_key_name_is_excluded_too`: a home with `.dbrrg-ssh-host-keys` as a symlink to `.thinfloe-ssh-host-keys/` (holding a key) → `home_archive_argv` output, run, gives an archive with neither name in it.

- [ ] **Step 2: Run** those three test files. Expected: the new cases FAIL.

- [ ] **Step 3: Implement** the three changes listed under Interfaces.

- [ ] **Step 4: Run** the three test files and `make test-unit`. Expected: all pass.

- [ ] **Step 5: CHANGES.md** under `### Changed`: the screen-blanking setting is now `TF_IDLE_TIMEOUT` in `~/.thinfloe-sessionrc`; the old `DBRRG_IDLE_TIMEOUT` still works. (The boot parameter and identity exclude need no entry: nothing the reader sees changes.)

- [ ] **Step 6: Commit** `feat: accept rd.dbrrg.debug, DBRRG_IDLE_TIMEOUT and the old host-key name`.

---

### Task 5: Prove the fallback to 3.x

**Files:**
- Create: `test/fixtures/3.x/` holding, byte for byte from commit `53d72c1` (main, the release in the field): `dbrrg-lib.sh`, `dbrrg-ssh-hostkeys`, `dbrrg-password`, `dbrrg-save-home`, and a `README` naming the commit and saying these files are frozen and excluded from renames.
- Create: `test/integration/test-3x-fallback.sh`; add it to the `test:` target.

**Interfaces:**
- Consumes: `tf_migrate_home_names`, `tf_link_old_home_names` (Task 2).

- [ ] **Step 1: Write the test.** Build a 4.0 home by running Task 2's functions on a 3.x archive (as in Task 2's first case), then archive it with the command form 4.0's `thinfloe-save-home` uses (`tar --warning=no-file-changed -zcf - .`, run from the home). Then, with the 3.x fixtures:
  - `3.x restore keeps the links`: a fresh 3.x default home (`.dbrrg-environment` = `default-env` as a regular file); `dbrrg_restore_home_from_file` from the fixture lib (stub dracut logging as `test-initramfs-home.sh` does) → `.dbrrg-environment` is a symlink and reads `XKB_DEFAULT_LAYOUT=ch`.
  - `3.x host keys follow the link`: fixture `dbrrg-ssh-hostkeys` with `DBRRG_HOME_DIR`, `DBRRG_SSH_DIR`, `DBRRG_KEY_OWNER` pointing into the work dir (chown stubbed as `test-ssh-hostkeys.sh` does) → installs the key; `--stage` afterwards leaves `.dbrrg-ssh-host-keys` a symlink.
  - `3.x password follows the link`: fixture `dbrrg-password --apply` with stubbed `getent`/`chpasswd` (pattern of `test-password.sh`) → `chpasswd -e` received the stored hash.
  - `saves keep links as links`: neither the fixture `dbrrg-save-home` nor `overlay/usr/bin/thinfloe-save-home` passes `-h` or `--dereference` to tar, and `tar -tvzf` of the archive built above shows the five old names as `l` entries.

- [ ] **Step 2: Run** `test/integration/test-3x-fallback.sh`. Expected: all `ok`. A failure here is a finding against Task 2's design, not a test to adjust: report it to the controller.

- [ ] **Step 3: Commit** `test: a 4.0 home restored and used by 3.x keeps its settings`.

---

### Task 6: Build, full tests, and a boot with a 3.x home

**Files:**
- Modify: `scripts/run-qemu-upgrade-smoke.sh` (put a 3.x home archive on the stick; check the renames)
- Modify: `scripts/publish.sh` (bridge for 3.x sticks)

**Interfaces:**
- Consumes: everything above. The log lines `thinfloe: renamed ~/.dbrrg-environment to ~/.thinfloe-environment` (Task 2) and the host name `thinfloe-…`.

- [ ] **Step 1: Extend the upgrade smoke.** This boot is exactly the first ThinFloe boot after an upgrade (spec item 4: kernel and initramfs from `tl.new/`). Before booting, `mcopy` a `home.tar.gz` built from a 3.x-style home (`.dbrrg-environment`, `.dbrrg-sessionrc`, `.dbrrg-ssh-host-keys/` with a generated key, `.config/dbrrg/menu/x.desktop`) to `::home.tar.gz`. Passing additionally requires the log line for each of those four renames.

- [ ] **Step 2: Publish bridge.** `scripts/publish.sh` uploads `thinfloe-usb.img.zst` and its `.sha256`, and also the same image as `dbrrg-usb.img.zst` with its own `.sha256` naming that file (a hard link in a staging directory under `artifacts/images/`), because 3.x's `upgrade-image` fetches only the old name. Do not run it.

- [ ] **Step 3: Build** `make image` (4 cores). Expected: `artifacts/images/thinfloe-usb.img.zst` exists; `podman images` shows `thinfloe-ubuntu:3.0.0`.

- [ ] **Step 4: Run** `make test`. Expected: every script passes; compare against Task 1's baseline plus the new cases. `test-initramfs-commands.sh` must pass: it proves the new functions call only tools the initrd has.

- [ ] **Step 5: Run** `make qemu-smoke`, `make qemu-smoke-netboot`, `make qemu-smoke-upgrade`. Expected: all pass; the netboot host name is `thinfloe-123456`; the upgrade smoke shows the four renames.

- [ ] **Step 6: Run** `make test-runtime`. Expected: pass (it reads the labwc config from `/etc/thinfloe/labwc` and the `thinfloe-menu` app_id rule).

- [ ] **Step 7: Check the image by listing it**: `unsquashfs -l artifacts/rootfs/ramroot.sqsh | grep -i dbrrg` lists nothing (report any hit with the package that owns it); `dpkg -s labwc` in the container shows `0.9.3-1+thinfloe1`; `lsinitramfs artifacts/rootfs/initrd.img | grep thinfloe` shows `lib/thinfloe-lib.sh` and both `90-thinfloe-after-cmdline.conf` drop-ins.

- [ ] **Step 8: Commit** `test(smoke): the first boot after an upgrade renames a 3.x home` (the smoke and publish changes).

---

### Task 7: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `CHANGES.md`

- [ ] **Step 1: Rewrite the intro** of `CLAUDE.md` and `README.md`: ThinFloe, one sentence on the name (the system floats in RAM and is gone at power-off, except the saved home), the old name once for searchability.

- [ ] **Step 2: Add a Standing Constraint** to `CLAUDE.md`, "Old home-file names stay as links while the migration code exists", stating: the precedence rule (archive content, not disk); the five renamed entries; why the links (3.x in `tl.old/`, `LABEL previous`); that `IDENTITY_EXCLUDES` lists both host-key names; that `rd.dbrrg.debug`, `X-DBRRG-*` and `DBRRG_IDLE_TIMEOUT` are still read; that shipped tile file names stay; Review Focus 3's consequence; and which tests guard it (`test-initramfs-home.sh`, `test-3x-fallback.sh`, the tiles tests, `test_upgrade_image.py`, `make qemu-smoke-upgrade`). Update the constraint count in the section intro.

- [ ] **Step 3: Read `CLAUDE.md` and `README.md` once end to end** for leftovers of Task 1's substitution that read wrong (`thinfloe` where prose wants `ThinFloe`, sentences about "the dbrrg session").

- [ ] **Step 4: Check `CHANGES.md`**: every entry from Tasks 1 to 4 present, most important first (host name), none over three sentences.

- [ ] **Step 5: Commit** `docs: ThinFloe in CLAUDE.md and README.md`.

---

### Task 8: Rename the GitHub repository (owner's go required)

Not executed by an implementer. The controller asks the owner first.

- [ ] **Step 1:** Owner renames `oposs/dbrrg` to `oposs/thinfloe` (`gh repo rename thinfloe -R oposs/dbrrg`); GitHub redirects the old address.
- [ ] **Step 2:** Replace the remaining `oposs/dbrrg` URLs and update `git remote set-url origin`. `git grep -n -i dbrrg` then lists only `tobi.oetiker.ch/dbrrg`, `public_html/dbrrg`, `test/fixtures/3.x/`, the compatibility code of Tasks 2 to 4, `CHANGES.md` and the history in `docs/`.
- [ ] **Step 3:** Commit `docs: the repository is oposs/thinfloe`.
