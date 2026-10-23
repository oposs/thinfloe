# ThinFloe Part 2: Add-ons and tfuser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ThinLinc and OxuLnk leave the image and become add-ons the person
at the machine ticks in a new Settings screen; the user account becomes
`tfuser`.

**Architecture:** One root program, `thinfloe-addon` (Python 3, stdlib only,
like `thinfloe-upgrade`), reads add-on definitions from tile files, checks
and downloads sources, stores payloads on the ESP, installs them with
`dpkg -i` or `tar -x`, and writes a status file and a log file under
`/run/thinfloe/addons/`. A boot unit installs the stored copies before the
autologin; a timer checks daily. `thinfloe-menu` only reads the status and
log files and calls `sudo -n thinfloe-addon …`.

**Tech Stack:** Python 3 (stdlib), POSIX sh, systemd units, Rust 1.96
(`thinfloe-menu`, egui 0.36 on the CPU), dpkg/apt.

**Spec:** `docs/superpowers/specs/2026-10-21-thinfloe-design.md`, section
"Part 2: Add-ons". The tfuser rename is an owner decision of 2026-10-22
(recorded in the controller handoff, not in the spec): `tluser` → `tfuser`,
with a `/home/tluser` symlink for absolute paths in user files.

## Global Constraints

- Every Standing Constraint in `CLAUDE.md` holds (no shipped password,
  `--no-hostonly`, zero keybindings, masked `ssh.socket`, network config
  installed by the initramfs, no `Before=` on a sysinit unit from a
  multi-user unit, the initramfs command set, the menu draws on the CPU).
- A public image contains neither the ThinLinc client nor OxuLnk.
- A source that is neither HTTPS nor signed is never checked or fetched
  automatically; it installs and updates only on Install now / Update now.
- An update found by the daily check or the check at boot becomes active
  at the next boot. Install now / Update now install at once, also while
  the program runs.
- An add-on that is installed boots from its ESP copy with no network.
- A failed check or download leaves the installed copy in place and logs why.
- Every check, download and install writes to the menu log with source
  `add-ons`.
- User tile files: at most 32; a user file named like a shipped one may
  reword `Name`, `Comment`, `Icon` only; a user file may carry source keys;
  a user file may only use the run action.
- Key prefix `X-TF-`; shell/env prefix `TF_`; functions `tf_`.
- 4 cores for builds and tests; cargo under
  `systemd-run --user --scope -p MemoryMax=2G`; `cargo fmt` and
  `cargo clippy -- -D warnings` clean.
- `CHANGES.md` entries under `### New` / `### Changed` / `### Fixed`, never
  `### Added`; written for admins.

## Decisions taken by this plan (builder's calls in the spec)

- **Reinstall at boot: `dpkg -i` of the stored `.deb`, `tar -x -C /` of a
  stored tar.** Measured 2026-10-23 in the built image (podman, 2 CPUs):
  `dpkg -i thinlinc-client_4.21.0-4588_amd64.deb` 4.0 s wall. sysext was
  rejected: it skips maintainer scripts and cannot carry a tar overlay that
  writes `/etc`.
- **ThinLinc has no `Depends:`** (`thinlinc-client_4.21.0-4588`); its
  postinst only refreshes desktop/icon/mime caches. OxuLnk's `Depends:` are
  `libasound2t64 libc6 libopus0 libstdc++6 libva-drm2 libva2 libx11-6
  libx11-xcb1 libxcb1 libxcursor1 libxi6 libxkbcommon-x11-0 libxkbcommon0`.
- **Add-on id** = tile file name without `.desktop` (`10-thinlinc`).
- **Source keys:**
  - APT: `X-TF-Source-Apt=<uri> <suite> <component>`,
    `X-TF-Source-Package=<name>`, optional `X-TF-Source-Key=<https URL of an
    OpenPGP key>`. Signed = a key is given.
  - Fixed address: `X-TF-Source-URL=<url>`.
  - Page + pattern: `X-TF-Source-Page=<url>`, `X-TF-Source-Pattern=<Python
    regex>`; named group `version` optional.
  - `X-TF-Source-Format=deb|tar` overrides the format; otherwise `.deb` →
    deb, `.tar`, `.tar.gz`, `.tgz`, `.tar.xz`, `.tar.zst` → tar, anything
    else is an error.
  - `X-TF-Source-Config=<path relative to home>`: the config whose presence
    ticks this add-on on the first boot from 3.x (ThinLinc `.thinlinc`,
    OxuLnk `.config/oxulnk` — verify the OxuLnk path against a running
    client: the binary names `oxulnk` and `desktop.toml`).
  - All source keys are shipped-only when a user file rewords a shipped tile
    (add them to `SHIPPED_ONLY_KEYS`).
- **Automatic allowed** (`auto_allowed`): every URL involved (page,
  matched link, fixed URL, APT URI, every redirect hop) is `https:`, **or**
  the source is APT with a key. Everything else is manual-only.
- **Tick state:** `~/.config/thinfloe/addons`, one add-on id per line, owned
  by tfuser, written only by `thinfloe-addon` (via sudo). Unknown ids are
  kept and logged once per run.
- **ESP layout** (`/run/thinfloe/storage/efi/addons/<id>/`):
  `current.<ext>` + `current.meta` (installed copy), `next.<ext>` +
  `next.meta` (ready for next boot), `*.new` while downloading. `meta` is
  `key=value` lines: `version`, `fingerprint`, `url`, `format`, `package`
  (deb), `fetched` (epoch). Netboot (no ESP mounted): the same layout under
  `/run/thinfloe/addons-cache/`, rebuilt each boot.
- **Lock:** `/run/thinfloe/esp.lock` (tmpfiles, 0644 root), `flock`ed by
  `thinfloe-addon` for every ESP write and by `thinfloe-save-home` around its
  USB write.
- **Space:** a download is stored only if the ESP keeps 64 MiB free after
  it; otherwise `failed` with "not enough space on the boot stick".
  `thinfloe-upgrade`'s space check adds the size of every `current.*`
  (room for one `next.*` each).
- **Status file** `/run/thinfloe/addons/status`, rewritten atomically, one
  line per known add-on, tab-separated, fixed columns:
  `id ticked state installed_version pending_version auto message` where
  `ticked` and `auto` are `0`/`1`, `state` is one of `absent`, `installed`,
  `pending`, `failed`, `install-failed`, and empty fields are `-`. A second
  file `/run/thinfloe/addons/last-check` holds the epoch of the last full
  check.
- **Log file** `/run/thinfloe/addons/log`: `epoch<TAB>text` lines, appended
  by every `thinfloe-addon` run (the boot unit and the timer included), also
  printed to stdout. Kept to the last 500 lines.
- **First boot from 3.x** (`thinfloe-addon boot`, before installing): seed
  the tick file when it does not exist and either
  `/run/thinfloe/state/home-renamed` exists, or `home-restore` is `absent`,
  `tl.old/` exists on the ESP and `/config/addons-seeded` does not. Then
  tick every shipped add-on whose `X-TF-Source-Config` exists in the home,
  or every shipped add-on with that key when `home-restore` is `absent`.
  Write `/config/addons-seeded` (USB only). Fires once.
- **No copy yet:** a ticked add-on without `current.*` is installed by the
  check at once (nothing runs that could be disturbed); one with
  `current.*` gets `next.*`.
- **Netboot:** the boot unit does nothing; the check unit (after
  `network-online.target`) fetches and installs every ticked add-on.

## Review Focus

1. **No network at boot, add-on installed:** the session starts with the
   tile, from the ESP copy, no wait. Test: Task 4 `boot installs from ESP
   with no network`.
2. **The page changed or the pattern matches nothing:** state `failed`,
   reason in the log, installed copy and tile stay. Test: Task 2
   `pattern without match fails and keeps nothing`, Task 3
   `failed check keeps current`.
3. **Check and save-home write the ESP at once:** they serialize on
   `esp.lock`. Test: Task 3 `store waits for the esp lock`, Task 4
   `save-home takes the esp lock`.
4. **A truncated or non-package download** (captive portal HTML, cut
   transfer): refused before it is stored. Test: Task 3
   `html instead of a deb is refused`.
5. **ESP full:** the `.new` file is removed, the current copy stays. Test:
   Task 3 `no space keeps current and removes new`.

---

### Task 1: tluser becomes tfuser

**Files:**
- Modify: `containers/ubuntu/Dockerfile` (adduser, sudoers line, chown/chmod
  lines, `ln -s tfuser /home/tluser`)
- Move: `overlay/home/tluser/` → `overlay/home/tfuser/`
- Modify: every file listed by
  `grep -rl tluser --exclude-dir=.git --exclude-dir=docs --exclude-dir=.superpowers --exclude-dir=target .`
  (23 files at b189e3b: the dracut lib, `thinfloe-save-home`,
  `thinfloe-ssh-hostkeys`, `thinfloe-password`, `thinfloe-upgrade`
  (`TLUSER` → `TFUSER = "tfuser"`), `thinfloe-session`, `wait-kms`,
  `10-thinfloe-session.sh`, autologin drop-in, both services, sshd drop-in,
  `src/thinfloe-menu/src/main.rs`, the tests, `README.md`, `CLAUDE.md`)
- Test: `test/integration/test-initramfs-home.sh`,
  `test/integration/test-session-packages.sh`

Comments that explain history ("thinfloe-save-home writes the archive as
tluser") are rewritten to the new name; `docs/` stays untouched.

- [ ] **Step 1: Write the failing tests**
  - `test-session-packages.sh`: `ok - tfuser exists with uid 1000, home /home/tfuser`;
    `ok - /home/tluser is a symlink to tfuser`; `ok - no tluser in /etc/passwd`;
    `ok - sudoers grants tfuser NOPASSWD`; `ok - getty autologin is tfuser`.
  - `test-initramfs-home.sh`: `ok - a 3.x archive restores into /home/tfuser`
    (3.x fixture archive, restore, assert `.thinfloe-environment` lands under
    `<newroot>/home/tfuser`).
- [ ] **Step 2: Run** `make test-unit` and
  `bash test/integration/test-initramfs-home.sh`; expect the new
  initramfs case to FAIL.
- [ ] **Step 3: Rename.** uid stays 1000, so archive member owners and
  `/run/user/1000` are unchanged. `thinfloe-password` writes `tfuser:<hash>`
  to `chpasswd -e`.
- [ ] **Step 4: Run** the offline gates (`make test-unit`, every
  `test/integration/test-*.sh` that needs no rootfs) and
  `grep -rn tluser --exclude-dir=.git --exclude-dir=docs --exclude-dir=.superpowers --exclude-dir=target .`
  — only the symlink line, the 3.x notes in `CLAUDE.md` and the new tests
  may remain.
- [ ] **Step 5: CHANGES.md** under `### Changed`: the user is `tfuser`;
  `/home/tluser` still leads to the home; SSH logins use `tfuser`.
- [ ] **Step 6: Commit** `refactor: the user is tfuser`.

### Task 2: thinfloe-addon — definitions and checks

**Files:**
- Create: `overlay/usr/bin/thinfloe-addon` (Python 3, executable, guarded
  `if __name__ == "__main__"`)
- Test: `test/unit/test_addon.py` (loaded through importlib like
  `test_upgrade_image.py`; added to `make test-unit`)

**Interfaces:**
- Produces:
  - `@dataclass Source: kind: str ("apt"|"url"|"page"); url: str; pattern: Optional[str]; apt_suite: Optional[str]; apt_component: Optional[str]; package: Optional[str]; key_url: Optional[str]; fmt: Optional[str]; config: Optional[str]`
  - `@dataclass Addon: id: str; name: str; source: Source; shipped: bool; tile_path: str`
  - `load_addons(shipped_dir: str, user_dir: str) -> Tuple[List[Addon], List[str]]` — second item: log lines for broken definitions. Same 32-file cap and reword rule as the menu: a user file named like a shipped one cannot change the source.
  - `auto_allowed(src: Source, urls_seen: List[str]) -> bool`
  - `@dataclass Found: url: str; version: Optional[str]; fingerprint: str; fmt: str; urls_seen: List[str]`
  - `check(src: Source) -> Found` raises `CheckError(str)` with a user-facing reason.
  - `version_gt(a: str, b: str) -> bool` via `dpkg --compare-versions a gt b`.

- [ ] **Step 1: Write the failing tests** (a `http.server` on 127.0.0.1 in a
  thread serves pages, HEAD responses and files; dpkg is real on the dev
  host):
  - `test_thinlinc_tile_parses`: the shipped `10-thinlinc.desktop` (Task 5
    content, use a copy in the test) gives `kind == "page"`.
  - `test_page_highest_version_wins`: page with links `…_4.20.0-1_amd64.deb`
    and `…_4.21.0-4588_amd64.deb` → `version == "4.21.0-4588"`.
  - `test_page_without_version_group_first_match`: fingerprint is the URL.
  - `test_pattern_without_match_fails`: `CheckError` "no link on <page>
    matches the pattern".
  - `test_url_fingerprint_order`: ETag, else Last-Modified, else
    Content-Length + final URL; HEAD only (server asserts no GET).
  - `test_url_redirects_followed_and_recorded`: `urls_seen` holds every hop.
  - `test_auto_allowed`: all-https → True; one `http:` hop → False; apt with
    key over http → True; apt without key over http → False.
  - `test_user_reword_cannot_change_source`.
  - `test_unknown_format_is_error`: `.zip` URL → `CheckError`.
  - `test_apt_check`: a local flat repo built in the test with
    `dpkg-scanpackages` (skip with a message if missing) → version of the
    package; uses `apt-get` with `-o Dir::Etc::sourcelist=<tmp>`,
    `-o Dir::Etc::sourceparts=-`, `-o Dir::State::Lists=<tmp>`, so the
    system lists are never touched.
- [ ] **Step 2: Run** `python3 -m unittest test/unit/test_addon.py`; FAIL.
- [ ] **Step 3: Implement.** Timeouts 20 s per request; page bodies capped
  at 2 MiB; at most 10 redirects; user-agent `thinfloe-addon/<version from
  /etc/thinfloe/version or "dev">`.
- [ ] **Step 4: Run** the tests; PASS.
- [ ] **Step 5: Commit** `feat(addon): read add-on sources and check them`.

### Task 3: thinfloe-addon — store, install, remove, status

**Files:**
- Modify: `overlay/usr/bin/thinfloe-addon`
- Test: `test/unit/test_addon.py`

**Interfaces:**
- Consumes: Task 2.
- Produces (subcommands, all as root, all append to the log file):
  - `thinfloe-addon status` — rewrite the status file, print it.
  - `thinfloe-addon check [ID…]` — no ids: every ticked add-on with
    `auto_allowed`, writes `last-check`; with ids: those, manual allowed.
    Not installed → fetch and install now; installed and newer → store as
    `next.*` (state `pending`).
  - `thinfloe-addon install ID` — tick, fetch if needed, install now.
  - `thinfloe-addon update ID` — install `next.*` now (fetch first if
    there is none), promote it to `current.*`.
  - `thinfloe-addon remove ID` — untick, uninstall (`dpkg -r <package>` /
    delete the recorded tar file list), delete the ESP directory.
  - Exit 0 on success, 1 on a failure that is in the log.
- Paths are module constants overridable by env for the tests:
  `TF_ADDON_ESP` (default `/run/thinfloe/storage/efi`), `TF_ADDON_RUN`
  (`/run/thinfloe/addons`), `TF_ADDON_LOCK` (`/run/thinfloe/esp.lock`),
  `TF_ADDON_ROOT` (`/`, target of tar installs), `TF_ADDON_HOME`
  (`getent passwd tfuser`), `TF_ADDON_DPKG` (`dpkg`).

- [ ] **Step 1: Write the failing tests** (fake `dpkg` script records
  argv; tar installs into a temp root):
  - `test_first_install_stores_current_and_installs`.
  - `test_newer_version_becomes_next_and_pending`: installed 1.0, found 1.1
    → `next.deb`, status `pending`, fake dpkg not called.
  - `test_update_promotes_next`.
  - `test_failed_check_keeps_current`: server down → status `failed`,
    `current.*` untouched, `installed_version` still shown.
  - `test_html_instead_of_a_deb_is_refused`: `dpkg-deb --info` fails →
    nothing stored, log names the URL.
  - `test_tar_must_be_relative`: a tar member that is absolute or contains
    `..` → refused.
  - `test_no_space_keeps_current_and_removes_new`: patch `free_bytes`.
  - `test_store_waits_for_the_esp_lock`: hold the lock in the test; a
    `check` in a thread does not write until released.
  - `test_remove_unticks_and_cleans`.
  - `test_manual_source_skipped_by_check_all`: log line
    "<name>: not checked automatically (neither HTTPS nor signed)".
  - `test_status_columns`: exact line for each state.
  - `test_log_kept_to_500_lines`.
  - `test_tick_file_owned_by_tfuser` (chown is patched; assert the call).
- [ ] **Step 2: Run**; FAIL.
- [ ] **Step 3: Implement.** Write `*.new`, verify (`dpkg-deb --info` /
  `tar -t` + member check), `os.replace` to its name, `fsync` the
  directory. Messages are plain English for the menu: "ThinLinc 4.22.0
  stored on the boot stick, active at next boot", "ThinLinc: up to date",
  "ThinLinc: check failed: <reason>".
- [ ] **Step 4: Run**; PASS.
- [ ] **Step 5: Commit** `feat(addon): store, install and remove add-ons`.

### Task 4: Boot, timer, first boot from 3.x, ESP lock in save-home

**Files:**
- Modify: `overlay/usr/bin/thinfloe-addon` (`boot` subcommand)
- Create: `overlay/etc/systemd/system/thinfloe-addons.service`,
  `thinfloe-addon-check.service`, `thinfloe-addon-check.timer`,
  `overlay/usr/lib/tmpfiles.d/thinfloe-addons.conf`
- Modify: `overlay/usr/bin/thinfloe-save-home` (flock around the USB
  write), `containers/ubuntu/Dockerfile` (enable the service and timer)
- Test: `test/unit/test_addon.py`, `test/integration/test-save-home.sh`,
  `test/integration/test-session-packages.sh`

**Interfaces:**
- `thinfloe-addon boot`: seed (rule in "Decisions"), promote `next.*` to
  `current.*`, install every ticked `current.*`, write status. Never
  touches the network.
- `thinfloe-addons.service`: `Type=oneshot`, `ExecStart=/usr/bin/thinfloe-addon boot`,
  `Before=getty@tty1.service`, `WantedBy=multi-user.target`, no other
  ordering (it must not create a cycle; see Standing Constraints).
- `thinfloe-addon-check.service`: `ExecStart=/usr/bin/thinfloe-addon check`,
  `After=network-online.target thinfloe-addons.service`,
  `Wants=network-online.target`.
- `thinfloe-addon-check.timer`: `OnBootSec=1min`, `OnUnitActiveSec=1d`,
  `RandomizedDelaySec=1h`, `WantedBy=timers.target`.
- tmpfiles: `d /run/thinfloe/addons 0755 root root`,
  `f /run/thinfloe/esp.lock 0644 root root`.

- [ ] **Step 1: Write the failing tests:**
  - `test_boot_installs_from_esp_with_no_network` (socket connect patched to
    raise; install still happens).
  - `test_boot_promotes_next`.
  - `test_seed_from_home_renamed`: `home-renamed` present, home has
    `.thinlinc/` but no `.config/oxulnk` → ticks `10-thinlinc` only.
  - `test_seed_absent_archive_after_3x_upgrade`: `home-restore` = `absent`,
    `tl.old/` present, no `/config/addons-seeded` → ticks every shipped
    add-on with a config key, writes the marker.
  - `test_seed_fires_once`: marker present → no tick file written.
  - `test_fresh_install_ticks_nothing`.
  - `test_boot_on_netboot_does_nothing` (no ESP mounted → no install, no
    seed marker; the check unit does the work).
  - `test_install_failure_hides_tile`: fake dpkg exit 1 →
    `install-failed`, message names dpkg's last line.
  - `test-save-home.sh`: `ok - save-home takes the esp lock` (hold the lock
    with `flock` in the test; the save waits).
  - `test-session-packages.sh`: the three units exist and are enabled;
    `thinfloe-addons.service` has no `Before=` other than
    `getty@tty1.service`.
- [ ] **Step 2: Run**; FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the unit and integration tests; PASS.
- [ ] **Step 5: Commit** `feat(addon): install add-ons at boot and check daily`.

### Task 5: The image ships add-on tiles, not the clients

**Files:**
- Modify: `containers/ubuntu/Dockerfile` (drop the ThinLinc `wget`/`apt
  install` and the `vendor/oxulnk-desktop.deb` COPY/install; add OxuLnk's
  `Depends:` list and `libxrandr2` explicitly, with a comment naming why),
  `Makefile` (drop `OXULNK_DEB`/`VENDOR_DEB` staging and its help line)
- Modify: `overlay/etc/thinfloe/menu/10-thinlinc.desktop` (source keys:
  `X-TF-Source-Page=https://www.cendio.com/thinlinc/download/`,
  `X-TF-Source-Pattern=https://[^"]*/thinlinc-client_(?P<version>[0-9.]+-[0-9]+)_amd64\.deb`,
  `X-TF-Source-Config=.thinlinc`), `20-oxulnk.desktop` (see Step 3)
- Modify: `overlay/usr/bin/thinfloe-upgrade` (space check counts add-on
  copies)
- Create: `test/integration/test-addon-deps.sh` (run via `make test`)
- Test: `test/unit/test_upgrade_image.py`,
  `test/integration/test-session-packages.sh`

- [ ] **Step 1: Write the failing tests:**
  - `test-session-packages.sh`: `ok - no /opt/thinlinc in the image`;
    `ok - no oxulnk-desktop in the image`; `ok - libXrandr.so.2 is present`.
  - `test-addon-deps.sh`: installs the ThinLinc deb (path from
    `$THINLINC_DEB`, else downloaded once into
    `${TMPDIR}/thinfloe-addon-cache/` from the URL the page pattern yields;
    the test fails, not skips, when neither works) into a copy of the built
    rootfs with `podman run`, then `ldd` over every ELF under `/opt/thinlinc`
    plus `libXrandr.so.2` → no `not found`; then
    `/opt/thinlinc/bin/tlclient --version` exits 0. Same for OxuLnk when
    `$OXULNK_DEB` is set.
  - `test_upgrade_image.py`: `test_space_check_counts_addon_copies`.
- [ ] **Step 2: Run**; FAIL.
- [ ] **Step 3: Change the image.** OxuLnk's APT address and key are not
  known yet (open question to the owner). Until they are, `20-oxulnk.desktop`
  is **removed** from the image; the task that adds it back writes
  `X-TF-Source-Apt`, `X-TF-Source-Package=oxulnk-desktop`,
  `X-TF-Source-Key`, `X-TF-Source-Config=.config/oxulnk`.
- [ ] **Step 4: Rebuild** (`make image` under the build flock) and run
  `make test`; PASS.
- [ ] **Step 5: CHANGES.md** under `### Changed`: ThinLinc is no longer in
  the image; it is installed from cendio.com when ticked in Settings, and a
  machine that had it is ticked on its first ThinFloe boot. OxuLnk: the
  same, once its source is set.
- [ ] **Step 6: Commit** `feat: ThinLinc is an add-on, not part of the image`.

### Task 6: Menu — add-on tiles, status, log

**Files:**
- Create: `src/thinfloe-menu/src/addons.rs`
- Modify: `src/thinfloe-menu/src/tiles.rs` (source keys, `addon: bool`
  on `Tile`, `SHIPPED_ONLY_KEYS`), `menu.rs`, `ui.rs`, `app.rs`, `lib.rs`

**Interfaces:**
- Produces:
  - `pub enum AddonState { Absent, Installed, Pending, Failed, InstallFailed }`
  - `pub struct AddonStatus { pub id: String, pub ticked: bool, pub state: AddonState, pub installed: Option<String>, pub pending: Option<String>, pub auto: bool, pub message: Option<String> }`
  - `pub fn read_status(path: &Path) -> Vec<AddonStatus>` — bounded: 64 KiB,
    64 lines, malformed lines skipped.
  - `pub struct AddonLog` with `pub fn poll(&mut self) -> Vec<(SystemTime, String)>`
    — reads new lines from the log file by offset, restarts at 0 when the
    file shrank, at most 200 lines and 1 KiB each per poll.
  - `Tile.addon: bool` (true when the file has any `X-TF-Source-*` key).
  - `Menu::set_addons(&mut self, status: Vec<AddonStatus>)`: an add-on tile
    whose state is not `Installed`/`Pending` is not drawn; `Pending` draws
    the "update" marker.
- Paths: `/run/thinfloe/addons/status`, `/run/thinfloe/addons/log`, read
  once a second while the window is visible (mtime check only).

- [ ] **Step 1: Write the failing tests** (`cargo test`):
  - `status_line_parses_every_state`, `status_bounded`,
    `log_poll_reads_only_new_lines`, `log_poll_restarts_after_truncation`,
    `uninstalled_addon_tile_hidden`, `pending_addon_tile_has_marker`,
    `user_file_may_carry_source_keys`,
    `reword_cannot_set_source_keys` (key listed in `ignored`).
  - Log lines appear with source `add-ons`.
- [ ] **Step 2: Run** under `systemd-run --user --scope -p MemoryMax=2G cargo test -j4`; FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** tests, `cargo fmt --check`, `cargo clippy -- -D warnings`; PASS.
- [ ] **Step 5: Commit** `feat(menu): show add-on tiles by their install state`.

### Task 7: Menu — Settings screen and Retry

**Files:**
- Create: `overlay/etc/thinfloe/menu/60-settings.desktop`
  (`Name=Settings`, `Icon=settings` from the shipped Lucide set,
  `X-TF-Action=settings`), `src/thinfloe-menu/src/settings.rs`
- Modify: `tiles.rs` (`Action::Settings`), `menu.rs`, `ui.rs`, `app.rs`

**Interfaces:**
- `Effect::Start(Job::Addon { args: Vec<String> })` runs
  `sudo -n /usr/bin/thinfloe-addon <args>`; its output is already in the
  add-on log, so the job's own stdout is not logged twice.
- Settings screen (function only; Part 4 styles it after the mockup
  `docs/superpowers/specs/2026-10-21-thinfloe-mockups/settings.html`):
  header "Add-ons", "last checked <time>" from `last-check`, **Check now**
  (`check` with every ticked id), **Back**; one row per add-on from
  `load`ed tiles joined with the status: tick box (tick → `install ID`,
  untick → `remove ID`), name, source (page/URL/`apt · <uri> <suite> ·
  <package>`), state text, and **Install now** (not installed) or
  **Update now** (`pending`) → `update ID`. A manual-only source shows
  "updates only by hand". The row icon is the tile's icon when it renders
  (ThinLinc's lives in `/opt/thinlinc`, so before install it does not),
  else the first letter; Cendio's logo is not shipped.
- While an add-on job runs, its row's buttons are disabled.
- **Retry:** a ticked add-on in state `failed` or `install-failed` with no
  installed version shows a dialog over the grid once per boot:
  "<Name> could not be installed: <message>", buttons **Retry** (`install
  ID`) and **Later**. The Settings row shows the same state.
- The grid must fit 8 tiles above the log at 1280x720 (spec non-goal note);
  adjust `ui::layout` and its test.

- [ ] **Step 1: Write the failing tests:** state machine tests in
  `menu.rs`/`settings.rs`: `tick_starts_install`, `untick_starts_remove`,
  `update_now_starts_update`, `check_now_checks_ticked`,
  `buttons_disabled_while_job_runs`, `retry_dialog_once_per_boot`,
  `later_closes_retry`, `settings_tile_parses`,
  `user_file_cannot_use_settings_action`, `eight_tiles_fit_1280x720`.
- [ ] **Step 2: Run**; FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** tests, fmt, clippy; `make image` and
  `make test-runtime` (the runtime test now expects 7 shipped tiles drawn:
  ThinLinc is hidden, Settings is new — update its count).
- [ ] **Step 5: CHANGES.md** under `### New`: Settings tile with the Add-ons
  screen (tick to install, Check now, Install now, Update now, the update
  marker, Retry).
- [ ] **Step 6: Commit** `feat(menu): Settings screen for add-ons`.

### Task 8: QEMU proof and docs

**Files:**
- Modify: `scripts/run-qemu-smoke.sh` or a new `make qemu-smoke-addons`
  target (implementer's choice; document it in `CLAUDE.md`)
- Modify: `CLAUDE.md`, `README.md`

- [ ] **Step 1:** Build a tiny test add-on `.deb` (`dpkg-deb --build`, one
  file `/usr/share/thinfloe-smoke-addon/ok`) and place it as
  `addons/90-smoke/current.deb` + `current.meta` on the smoke stick's ESP;
  the home archive carries `~/.config/thinfloe/addons` = `90-smoke` and
  `~/.config/thinfloe/menu/90-smoke.desktop` with a manual `http:` source.
  QEMU has no network route to it.
- [ ] **Step 2:** The smoke passes when the boot log shows
  `thinfloe-addons.service` finished before `getty@tty1.service` started,
  the file exists, the status file says `installed`, and there is no
  ordering cycle.
- [ ] **Step 3:** Run `make qemu-smoke`, `make qemu-smoke-netboot`,
  `make qemu-smoke-upgrade` (the upgrade boot from a 3.x home must tick
  ThinLinc; it has no network, so the status says `failed`), and the new
  smoke; all PASS.
- [ ] **Step 4: CLAUDE.md:** a section "Add-ons" (engine, files, ESP layout,
  lock, status/log format, boot order, the auto rule, first-boot seeding,
  the dpkg timing), a Standing Constraint "Add-on runtime libraries are
  listed by name in the Dockerfile" with `test-addon-deps.sh` as its guard,
  the tfuser name throughout, and remove the ThinLinc-in-image statements.
  Document that a home archive can bring software onto a machine.
- [ ] **Step 5: Commit** `docs: add-ons`.

---

## Open questions for the owner (do not block Tasks 1-4, 6-8)

- OxuLnk: APT repository URL, suite, component and key URL; is it public?
  Without them OxuLnk is not shipped as an add-on (Task 5 Step 3).
- OxuLnk config path for the 3.x tick rule (`~/.config/oxulnk`?).
