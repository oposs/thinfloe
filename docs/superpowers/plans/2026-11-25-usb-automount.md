# USB Stick Automount Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** USB sticks, USB card readers and built-in SD slots are mounted at `/media/tfuser/<name>`, ejected from a menu tile, flushed before restart and power off, announced by the OSD, and released by `thinfloe-upgrade` before it writes a stick.

**Architecture:** A udev rule starts `thinfloe-automount@<kname>.service` per partition; the root helper `/usr/libexec/thinfloe/thinfloe-automount` (Python, like `peripheral-battery`) mounts, unmounts, ejects and keeps the contract file `/run/thinfloe/removable`. The menu reads that file for the Eject tile and calls the helper through `sudo -n`; the OSD reads it for its notes; `thinfloe-upgrade` calls `release`/`remount` around a write.

**Tech Stack:** Python 3 (helper, `thinfloe-upgrade`), Rust (`src/thinfloe-menu`, shadcn-egui), udev, systemd, tmpfiles.d, QEMU smoke scripts (bash).

**Spec:** [docs/superpowers/specs/2026-11-24-usb-automount-design.md](../specs/2026-11-24-usb-automount-design.md)

## Global Constraints

- Mount root `/media/tfuser`, mode 0755, owner root:root; link `/run/media/tfuser -> /media/tfuser`.
- Contract file `/run/thinfloe/removable`, mode 0644, written atomically by the helper only; line: `device<TAB>disk<TAB>name<TAB>mountpoint<TAB>fstype<TAB>size_bytes<TAB>model<TAB>kind`, `kind` is `stick` or `card`. Readers ignore unknown trailing fields.
- Event file `/run/thinfloe/removable.event` (plan's call for the spec's "second small file"), one line `seq<TAB>name<TAB>clean|unclean`, written atomically.
- Helper state `/run/thinfloe/automount/` (0700 root): `lock/files`, `lock/<disk>`, `hold/<disk>` (one line `pid<TAB>id`, id as `thinfloe-upgrade drives` reports it).
- Helper exit codes: 0 done, 1 failure, 2 usage, 3 busy (nothing unmounted, nothing powered off).
- Mount options: vfat `uid=1001,gid=1001,shortname=mixed,utf8=1,showexec,flush`; exfat `uid=1001,gid=1001,iocharset=utf8,errors=remount-ro`; ntfs3 `uid=1001,gid=1001,windows_names`; ext2/3/4, btrfs, xfs no owner option; all `nosuid,nodev`; any other type is not mounted. Always `mount --no-canonicalize`.
- Never mount: the boot disk, partlabel `EFI-SYSTEM`, a disk with a live hold, `mmcblk` whose card type is not `SD`, a device without medium or filesystem, a device already mounted.
- No lock held across mount, umount or sync; flush with `syncfs` per mount (`sync -f <mountpoint>`), never a global `sync`.
- Card readers (`ID_DRIVE_FLASH_SD|MS|CF|SM` set, or more than one LUN on the USB device) and built-in SD slots are never powered off.
- Callers use the full path `sudo -n /usr/libexec/thinfloe/thinfloe-automount …`.
- Project rules from `CLAUDE.md`: no `Before=` on sysinit units; new overlay files are picked up by `OVERLAY_DIRS`/`OVERLAY_FILES` (check `test-container-inputs.sh`); UI colours and sizes from theme/`brand.rs` (`ui_code_has_no_style_literals`); `X-TF-Action=eject` honoured only in shipped tiles; OSD never takes input; the menu saves before exit 0/10/11; `tf_log`/initramfs untouched.
- Build and test with at most 4 cores (`make … TEST_THREADS=4`, `CARGO_BUILD_JOBS=4`).
- `CHANGES.md` under `## [Unreleased]`, no release header, reader-facing wording.

## Review Focus

1. A stick whose label is `..`, empty, all spaces, 300 bytes long or contains `/` or newlines: gets a safe single-component name, never a path outside `/media/tfuser`. Test in Task 1 (`test_name_hostile_labels`).
2. Two sticks with the same label plugged in at once, then the first pulled and a third with that label plugged: names `X`, `X1`, then `X` again — never a mount over a live one. Test in Task 2 (`test_add_same_label_three_times`).
3. A stick pulled while a file on it is open in a remote session: the lazy unmount succeeds, the line goes, the event says `unclean`; the next stick with that name mounts. Test in Task 2 (`test_pull_with_busy_mount`).
4. Eject tapped while the writer holds the disk, or twice quickly: the second call answers busy or "nothing to eject", never a traceback or a half-removed line. Test in Task 2 (`test_eject_twice_and_under_hold`).
5. The contract file missing, empty, truncated or with garbage lines when the menu or the OSD starts: no tile, no note, no panic. Tests in Task 5 (`removable_parse_tolerates_garbage`) and Task 8 (`first_read_of_a_missing_file_is_empty`).

---

## File Structure

| File | Responsibility |
|---|---|
| `overlay/usr/libexec/thinfloe/thinfloe-automount` (new) | the root helper: `add`, `remove`, `eject`, `release`, `remount` |
| `overlay/etc/udev/rules.d/90-thinfloe-automount.rules` (new) | matches the devices, starts the unit |
| `overlay/etc/systemd/system/thinfloe-automount@.service` (new) | runs `add`/`remove` per device |
| `overlay/usr/lib/tmpfiles.d/thinfloe-automount.conf` (new) | `/media/tfuser`, the link, `/run/thinfloe/automount` |
| `test/unit/test_automount.py` (new) | helper unit tests with fake sysfs, fake commands |
| `test/integration/test-automount-image.sh` (new) | files, modes, modules and unit state in the built rootfs |
| `overlay/usr/bin/thinfloe-upgrade` | `automounted` field, hold + release + remount, `contents` |
| `test/unit/test_upgrade_batch.py`, `test_upgrade_image.py` | the writer changes |
| `src/thinfloe-menu/src/removable.rs` (new) | parse the contract and event files |
| `src/thinfloe-menu/src/eject_ui.rs` (new) | the eject panel |
| `src/thinfloe-menu/src/tiles.rs`, `menu.rs`, `jobs.rs`, `app.rs`, `ui.rs`, `main.rs` | tile action, state, jobs, file refresh, flush before exit |
| `overlay/etc/thinfloe/menu/70-eject.desktop` (new) | the shipped tile |
| `src/thinfloe-menu/src/osd/notes.rs` (new), `osd/run.rs`, `osd/state.rs`, `osd/draw.rs` | OSD notes |
| `src/thinfloe-menu/src/upgrade_ui.rs`, `upgrade.rs`, `drives.rs` | file list on the erase confirmation |
| `scripts/run-qemu-automount-smoke.sh`, `scripts/smoke-automount-report.sh` (new), `Makefile` | `make qemu-smoke-automount` |
| `docs/notes/automount.md` (new), `CLAUDE.md`, `CHANGES.md` | docs |

---

## Part A — automount, eject, writer, flush

### Task 1: Helper decisions (pure functions)

**Files:**
- Create: `overlay/usr/libexec/thinfloe/thinfloe-automount` (mode 755, `#!/usr/bin/env python3`, entry guarded by `if __name__ == "__main__"`)
- Test: `test/unit/test_automount.py` (loaded with `importlib.machinery.SourceFileLoader`, as `test_peripheral_battery.py` does)

**Interfaces:**
- Produces (all in the helper module):
  - `paths() -> Paths`: a dataclass of `sysfs`, `dev`, `run` (`/run/thinfloe`), `media` (`/media/tfuser`), `cmdline` (`/proc/cmdline`), `esp_device_file` (`/run/thinfloe-esp-device`), `efi_mount` (`/run/thinfloe/storage/efi`); each from env `TF_SYSFS`, `TF_DEV`, `TF_RUN`, `TF_MEDIA`, `TF_CMDLINE`, `TF_ESP_DEVICE_FILE`, `TF_EFI_MOUNT`.
  - `@dataclass Dev: kname, disk, devnode, fstype, label, uuid, partlabel, size_bytes, model, id_bus, removable: bool, mmc_type: Optional[str], flash_flags: set[str], usb_parent: Optional[str]`
  - `read_dev(kname: str, props: dict[str,str], p: Paths) -> Dev` (props are udev properties, from `udevadm info --query=property --name=<kname>` in production)
  - `boot_disk(p: Paths, findmnt) -> Union[str, None, BootUnknown]` — `findmnt` is a callable returning the source of `p.efi_mount` or None; `BootUnknown` is a sentinel meaning "stick boot, disk not found"
  - `skip_reason(d: Dev, boot, holds: dict[str, Hold], mounted: set[str], alive) -> Optional[str]` (`alive(pid) -> bool`)
  - `choose_name(label: str, uuid: str, taken: set[str]) -> str`
  - `mount_options(fstype: str) -> Optional[str]`
  - `is_card(d: Dev, luns_on_parent: int) -> bool`
  - `@dataclass Line: device, disk, name, mountpoint, fstype, size_bytes: int, model, kind`; `parse_lines(text) -> list[Line]`; `format_lines(lines) -> str`
  - `@dataclass Hold: pid: int, id: str`; `parse_hold(text) -> Optional[Hold]`

- [ ] **Step 1: Write the failing tests** in `test/unit/test_automount.py`:
  - `test_boot_disk_from_esp_file`: esp file `/dev/sdb1` → `"sdb"`; `/dev/mmcblk0p1` → `"mmcblk0"`; `/dev/nvme0n1p1` → `"nvme0n1"`.
  - `test_boot_disk_fallback_findmnt`: no esp file, findmnt gives `/dev/sdc1` → `"sdc"`.
  - `test_boot_disk_netboot`: no file, findmnt None, cmdline `ramroot=http://10.0.2.2/x.sqsh` → `None`.
  - `test_boot_disk_unknown_on_stick`: no file, findmnt None, cmdline `ramroot=tl/ramroot.sqsh` → `BootUnknown`, and `skip_reason` for any device returns a reason containing `"boot disk unknown"`.
  - `test_skip_rules`: one case each, expected reason substring: on boot disk `"boot disk"`; partlabel `EFI-SYSTEM` `"ThinFloe stick"`; live hold (pid alive, id matches) `"held"`; dead-pid hold → no skip; id-mismatch hold → no skip; `mmc_type="MMC"` `"eMMC"`; `mmc_type="SD"` → no skip; `fstype=""` `"no filesystem"`; `size_bytes=0` `"no medium"`; devnode in `mounted` `"mounted"`; `fstype="iso9660"` `"filesystem"`.
  - `test_name_from_label_uuid`: `("MYSTICK","",∅)` → `MYSTICK`; `("","1234-ABCD",∅)` → `1234-ABCD`; `("MYSTICK","",{"MYSTICK"})` → `MYSTICK1`; with `{"MYSTICK","MYSTICK1"}` → `MYSTICK2`.
  - `test_name_hostile_labels` (Review Focus 1): `"a/b"` → `a_b`; `".."` → `_.`; `"   "` with uuid `U` → `U`; `"x\ny"` → `x_y`; 300×`"a"` → length ≤ 255 bytes; no result contains `/` or starts with `.`; empty label and empty uuid → `disk`.
  - `test_mount_options_table`: each type of Global Constraints maps to the exact string plus `,nosuid,nodev`; `ext4` → `nosuid,nodev`; `iso9660` → `None`.
  - `test_is_card`: `ID_DRIVE_FLASH_SD=1` → True; `mmc_type="SD"` → True; 2 LUNs on parent → True; plain stick → False.
  - `test_lines_round_trip_and_tolerance`: `format_lines`/`parse_lines` round trip; a line with 9 fields keeps the first 8; a line with 3 fields, a blank line and a non-integer size are dropped.
  - `test_parse_hold`: `"1234\tsn-AA:16:3f"` → `Hold(1234,"sn-AA:16:3f")`; garbage → None.
- [ ] **Step 2: Run** `python3 -m unittest test/unit/test_automount.py -v` — expected: FAIL (module file missing).
- [ ] **Step 3: Implement** the functions above. Disk of a partition from sysfs (`/sys/class/block/<kname>/..`), not by stripping digits. Name rule: replace `/`, control characters and a leading `.` with `_`, strip spaces, cut to 255 bytes on a UTF-8 boundary; empty → uuid → `disk`.
- [ ] **Step 4: Run** the same command — expected: all PASS.
- [ ] **Step 5: Commit** `feat(automount): the helper's mount decisions`.

### Task 2: Helper commands and locks

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/thinfloe-automount`
- Test: `test/unit/test_automount.py`

**Interfaces:**
- Consumes: Task 1.
- Produces: CLI `thinfloe-automount add|remove <kname>`, `eject <disk>|all`, `release <disk>`, `remount <disk>`, exit codes of Global Constraints. Commands run through `run(argv, timeout)` which resolves `mount`, `umount`, `sync`, `udevadm`, `findmnt` from `PATH` (tests put fakes first). Power-off writes `1` to `<sysfs>/bus/usb/devices/<usb_parent>/remove` (plan's call: udisks2's method). `main(argv) -> int`.

- [ ] **Step 1: Write the failing tests** (temp dir as `TF_RUN`/`TF_MEDIA`/`TF_SYSFS`, fake `mount`/`umount`/`sync`/`udevadm` scripts that log their argv to a file and create or remove a marker; `TF_UDEV_PROPS_<kname>` env or a fixture dir stands in for `udevadm info`):
  - `test_add_mounts_and_writes_line`: `add sdb1` (vfat, label `MYSTICK`) → exit 0; `mount` argv is `["--no-canonicalize","-t","vfat","-o","uid=1001,…,nosuid,nodev","/dev/sdb1","<media>/MYSTICK"]`; contract line matches; mount point is a directory.
  - `test_add_refuses_existing_entry_or_symlink`: a pre-existing `<media>/MYSTICK` symlink to a temp dir → the stick is mounted at `MYSTICK1` and the symlink target is untouched; a pre-existing file named `MYSTICK1` too → `MYSTICK2`.
  - `test_add_failed_mount_removes_dir`: fake `mount` exits 32 → exit 1, no directory, no line.
  - `test_add_same_label_three_times` (Review Focus 2).
  - `test_remove_after_pull_is_unclean`: line present, device node gone → `umount -l` called, line gone, event `1\tMYSTICK\tunclean`.
  - `test_remove_after_eject_records_nothing`: `eject sdb` then `remove sdb1` → event seq unchanged after `remove`, the eject's event says `clean`.
  - `test_remove_card_pulled_from_reader`: device node present, size 0 → treated as pull (lazy umount, `unclean`).
  - `test_remove_at_shutdown_is_quiet`: device present with medium, fake `systemctl is-system-running` prints `stopping` → `sync -f` then `umount` without `-l`, line gone, no event.
  - `test_pull_with_busy_mount` (Review Focus 3).
  - `test_eject_busy`: fake `umount` exits 32 with "target is busy" → exit 3, line still present, no power-off write.
  - `test_eject_powers_off_stick_not_card`: stick → `remove` attribute written; card reader → not written; built-in SD → not written.
  - `test_eject_all`: two disks → both gone, two `clean` events.
  - `test_release_records_no_event`: `release sdb` → lines gone, event file unchanged.
  - `test_remount_triggers_change`: `udevadm trigger --action=change /dev/sdb1` (one per partition) called.
  - `test_hung_disk_does_not_block_others`: fake `mount` sleeps 30 s for `sdb1` only; start `add sdb1` in a thread; `add sdc1` finishes in < 3 s; `eject sdb` answers 3 within 6 s.
  - `test_eject_twice_and_under_hold` (Review Focus 4): second `eject sdb` → exit 0, prints `nothing to eject`; with a live hold, `add` skips.
  - `test_usage`: no args or unknown command → exit 2.
- [ ] **Step 2: Run** `python3 -m unittest test/unit/test_automount.py -v` — expected: new tests FAIL.
- [ ] **Step 3: Implement.** `fcntl.flock` on `lock/files` around read-modify-write of the contract file and the `mkdir` of a name; `lock/<disk>` (wait 5 s, then busy) around a disk's mount/umount/power-off. `mkdir` without `exist_ok`; after it, `os.lstat` must show a directory owned by uid 0 (tests: the current uid, via `TF_OWNER_UID`). Atomic writes: temp file in the same dir, `os.replace`. Event seq = previous + 1. Logs to stderr (journal).
- [ ] **Step 4: Run** the tests — expected: all PASS. Also `make test-unit TEST_THREADS=4` — PASS.
- [ ] **Step 5: Commit** `feat(automount): add, remove, eject, release and remount`.

### Task 3: udev rule, unit, tmpfiles, image checks

**Files:**
- Create: `overlay/etc/udev/rules.d/90-thinfloe-automount.rules`, `overlay/etc/systemd/system/thinfloe-automount@.service`, `overlay/usr/lib/tmpfiles.d/thinfloe-automount.conf`, `test/integration/test-automount-image.sh`
- Modify: `test/integration/test-session-packages.sh` only if it lists every unit/rule (check), `containers/ubuntu/Dockerfile` only if a filesystem module or `exfatprogs`-like tool is missing (no tool is needed: mounting needs only the kernel).

**Interfaces:**
- Consumes: Task 2 CLI.
- Produces: the running service in the image; `make test-integration` checks it.

- [ ] **Step 1: Write `test/integration/test-automount-image.sh`** (pattern of `test-wifi-stack.sh`: reads the exported rootfs), asserting:
  - the helper is 0755 root; the rule and the unit exist; the unit is not enabled (no `*.wants` link) and has no `Before=`;
  - the unit has `BindsTo=dev-%i.device`, `After=dev-%i.device`, `TimeoutStartSec=30`, `TimeoutStopSec=30`, `ExecStart=/usr/libexec/thinfloe/thinfloe-automount add %i`, `ExecStop=/usr/libexec/thinfloe/thinfloe-automount remove %i`;
  - tmpfiles has `d /media/tfuser 0755 root root`, `L /run/media/tfuser - - - - /media/tfuser`, `d /run/thinfloe/automount 0700 root root`;
  - kernel modules `vfat`, `exfat`, `ntfs3`, `ext4`, `btrfs`, `xfs`, `usb-storage`, `uas`, `mmc_block`, `sdhci-pci` are present (`modules.dep` or built-in in `modules.builtin`).
- [ ] **Step 2: Run** `bash test/integration/test-automount-image.sh` against the current artifacts — expected: FAIL (files missing).
- [ ] **Step 3: Write the rule** (one `ACTION!="remove"` line set; whole disks without a table via `ENV{DEVTYPE}=="disk", ENV{ID_PART_TABLE_TYPE}==""`; `ATTRS{removable}=="1"` or `ENV{ID_BUS}=="usb"` or `KERNEL=="mmcblk*", ATTRS{type}=="SD"`; `ENV{ID_FS_USAGE}=="filesystem"`; `TAG+="systemd", ENV{SYSTEMD_WANTS}+="thinfloe-automount@%k.service"`). Add a `change`-event line so a card inserted into a reader starts the unit (`ACTION=="change"` with `ID_FS_USAGE=="filesystem"`). Write the unit and tmpfiles per the test. Comment headers in the style of `thinfloe-peripheral-battery-watch.service`.
- [ ] **Step 4: Run** `make image TEST_THREADS=4` then `make test-integration` — expected: PASS, `test-container-inputs.sh` included, `test-image-size.sh` within budget.
- [ ] **Step 5: Commit** `feat(automount): mount sticks and cards when they appear`.

### Task 4: Writer: automounted drives, hold, release, remount

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`DriveInfo` ~330, `enumerate_drives` 775, `drives_json` 871, `_write_refusal` 2909, `fresh_install` 2333)
- Modify: `src/thinfloe-menu/src/drives.rs` (`Drive` gets `#[serde(default)] pub automounted: bool`)
- Test: `test/unit/test_upgrade_batch.py`, `test/unit/test_upgrade_image.py`, `drives.rs` tests

**Interfaces:**
- Consumes: Task 2 CLI (`release`, `remount`, exit 3), contract file path `/run/thinfloe/removable` (env `TF_REMOVABLE` in tests), hold dir `/run/thinfloe/automount/hold` (env `TF_AUTOMOUNT_HOLD`).
- Produces: `DriveInfo.automounted: bool`; JSON key `"automounted"`; `automounted_only(node: dict, lines) -> bool`; constant `AUTOMOUNT = "/usr/libexec/thinfloe/thinfloe-automount"`.

- [ ] **Step 1: Write the failing tests:**
  - `test_drives_json_has_automounted`: a node whose only mountpoint is `/media/tfuser/X` listed in the contract file → `automounted: true`, `mounted: false`; plus a non-automount mountpoint → `automounted: false`, `mounted: true`.
  - `test_write_refusal_allows_automounted`: fresh mode, `automounted=True, mounted=False` → None.
  - `test_fresh_install_holds_and_releases`: fakes record order: hold file written with `<pid>\t<id>` → `release sdb` → `open_target`; on return, hold file gone.
  - `test_fresh_install_release_busy`: release exits 3 → `outcome.refused`, reason `"the stick is in use"`, `remount` called, hold gone.
  - `test_fresh_install_unwritten_remounts`: `open_target` EBUSY / cancel before first byte / checksum refusal → `remount sdb` called once; a write that wrote bytes → `remount` not called.
  - `drives.rs`: `parse_accepts_automounted` and `parse_without_automounted_defaults_false`.
- [ ] **Step 2: Run** `python3 -m unittest test/unit/test_upgrade_batch.py test/unit/test_upgrade_image.py -v` and `cd src/thinfloe-menu && cargo test drives` — expected: FAIL.
- [ ] **Step 3: Implement.** In `fresh_install`, before `open_target`: write the hold, then `run_cmd([AUTOMOUNT, "release", disk])` in its own session (`SPAWN`); register hold removal with `_register_cleanup` (new type `"file"`). Track `written` from `outcome.written`/`writing`; in a `finally`, if nothing was written, run `remount`. A missing helper (tests without it, old images) is not an error: log and go on.
- [ ] **Step 4: Run** the tests — expected: PASS; `make test-unit TEST_THREADS=4` PASS.
- [ ] **Step 5: Commit** `feat(upgrade): write an automounted stick after releasing it`.

### Task 5: Menu: contract file, Eject tile and panel

**Files:**
- Create: `src/thinfloe-menu/src/removable.rs`, `src/thinfloe-menu/src/eject_ui.rs`, `overlay/etc/thinfloe/menu/70-eject.desktop` (`Name=Eject`, `Icon=usb` or an existing lucide name, `X-TF-Action=eject`)
- Modify: `tiles.rs` (`Action::Eject`, `parse("eject")`, shipped-only like `settings`, `exit_code` None), `menu.rs` (state + requests), `jobs.rs` (`eject` job), `app.rs` (refresh the file as the battery file is refreshed, line ~1011), `main.rs` (`TF_REMOVABLE_FILE` env path like `TF_BATTERY_FILE`), `ui.rs` (hide the tile, open the panel), `brand.rs` (any new size constants)

**Interfaces:**
- Produces:
  - `removable::FILE = "/run/thinfloe/removable"`, `removable::EVENT_FILE = "/run/thinfloe/removable.event"`
  - `pub struct Stick { pub disk: String, pub names: Vec<String>, pub size_bytes: u64, pub model: String, pub kind: Kind }`, `pub enum Kind { Stick, Card }`
  - `pub fn parse(text: &str) -> Vec<Stick>` (groups lines by disk), `pub fn read(path: &Path) -> Vec<Stick>` (bounded read, as `battery::read`)
  - `pub struct Event { pub seq: u64, pub name: String, pub clean: bool }`, `pub fn parse_event(text: &str) -> Option<Event>`
  - `Menu::set_removable(&mut self, sticks: Vec<Stick>)`; `Menu.removable: Vec<Stick>`; `Menu.eject: EjectState { panel_open: bool, running: Option<String>, message: Option<String> }`
  - `Job::Eject { target: String }` (disk or `"all"`); `jobs::eject(cmd: &[String], target: &str, feed) -> EjectResult` with `enum EjectResult { Done, Busy, Failed(String) }` from exit codes 0/3/other; `jobs::default_eject_cmd() -> Vec<String>` = `["sudo","-n","/usr/libexec/thinfloe/thinfloe-automount","eject"]`
  - `Menu::eject_finished(&mut self, target: &str, r: EjectResult) -> Option<Effect>`
- Consumes: Task 2 exit codes.

- [ ] **Step 1: Write the failing tests:**
  - `removable::tests::parse_groups_by_disk`: two lines of `sdb` + one of `sdc` → 2 sticks, `sdb` has 2 names.
  - `removable::tests::removable_parse_tolerates_garbage` (Review Focus 5): empty, missing file, a 3-field line, invalid UTF-8, 10 000 lines → no panic; garbage lines dropped; at most a bounded number of sticks.
  - `tiles::tests`: `eject` parses; a user file with `X-TF-Action=eject` is not honoured (same assertion as the `settings` test at tiles.rs:742).
  - `menu::tests::eject_tile_hidden_without_sticks`, `…shown_with_one`.
  - `menu::tests::one_stick_tap_ejects_at_once` → `Effect::Start(Job::Eject{target:"sdb"})`; `…several_sticks_tap_opens_panel`; `…eject_all_from_panel` → target `"all"`.
  - `menu::tests::eject_results`: `Done` → message `"MYSTICK can be removed"`; `Busy` → `"MYSTICK is in use: close the files on it"`; `Failed(e)` → `"Ejecting MYSTICK failed: e"`.
  - `ui` test: the panel shows one row per stick with name, size (`drives::format_size`), model and an Eject button, plus "Eject all"; `ui_code_has_no_style_literals` passes.
- [ ] **Step 2: Run** `cd src/thinfloe-menu && cargo test` — expected: new tests FAIL.
- [ ] **Step 3: Implement.** The panel is a dialog in the style of the existing Upgrade/Settings dialogs (shadcn-egui, `lyra`, nothing rounded). The file is re-read on the same tick as the battery file and on inotify of `/run/thinfloe` if `app.rs` already watches there; otherwise every 1 s.
- [ ] **Step 4: Run** `make test-menu` — expected: PASS (fmt, clippy, tests).
- [ ] **Step 5: Commit** `feat(menu): an Eject tile for sticks and cards`.

### Task 6: Menu: flush the sticks before restart and power off

**Files:**
- Modify: `src/thinfloe-menu/src/menu.rs` (`enum Logout` line 82, `logout_after_jobs`, the save-finished paths ~1499–1640), `ui.rs` (the two new dialog states)

**Interfaces:**
- Consumes: Task 5 (`Job::Eject`, `EjectResult`, `Menu.removable`).
- Produces: `Logout::Ejecting { since: Instant }`, `Logout::EjectFailed { message: String }`; `Request::EjectWait` (stay) and `Request::EjectGoOn` (go on).

- [ ] **Step 1: Write the failing tests** (next to `restart_saves_then_exits_ten` at menu.rs:2160):
  - `restart_with_a_stick_saves_then_ejects_then_exits_ten`: effects in order `Start(Job::Save)` → `Start(Job::Eject{target:"all"})` → after `Done`, `Exit(10)` after the leaving pause.
  - `power_off_with_a_busy_stick_asks`: `Busy` → `Logout::EjectFailed` with `"MYSTICK is in use: close the files on it"`; `EjectGoOn` → `Exit(11)`; `EjectWait` → menu stays, `logout` None.
  - `restart_without_sticks_does_not_eject`.
  - `logout_never_ejects`: Log out with a stick → no `Job::Eject`, `Exit(0)`.
  - `a_failed_save_still_asks_before_eject`: save failure path unchanged (`Logout::Failed` first).
- [ ] **Step 2: Run** `cargo test menu::tests::restart` — expected: FAIL.
- [ ] **Step 3: Implement.** After the save succeeded (or "anyway"), for `Action::Reboot | Action::Poweroff` with `!removable.is_empty()`, go to `Ejecting` and start the job; the dialog says `"Writing to MYSTICK …"` (names joined with `, `). Eject failure message wording as in Task 5. The save stays first: the standing rule "the menu saves before it exits" is unchanged.
- [ ] **Step 4: Run** `make test-menu` — expected: PASS.
- [ ] **Step 5: Commit** `feat(menu): write pending data to sticks before restart and power off`.

### Task 7: QEMU smoke, writer smoke, docs, changelog

**Files:**
- Create: `scripts/run-qemu-automount-smoke.sh`, `scripts/smoke-automount-report.sh`, `docs/notes/automount.md`
- Modify: `Makefile` (`qemu-smoke-automount` target, `.PHONY`, help line, `PREBUILT_GOALS`), `scripts/smoke-write-report.sh` and `scripts/run-qemu-write-smoke.sh` (blank disks now get a FAT filesystem so they are automounted first), `CLAUDE.md` (Commands list; a rule under "Home and session" or a new "Automount" heading: contract file, root-owned `/media/tfuser`, never powered off readers; Notes list entry), `CHANGES.md`, `.github/workflows/release-build-local.yml` only if it lists the smoke targets by name (check; the CLAUDE.md says nine smoke tests)

**Interfaces:**
- Consumes: everything of Part A.

- [ ] **Step 1: Write the smoke scripts** after `run-qemu-two-sticks-smoke.sh`: boot a copy of the USB image (virtio) plus a 64 MiB `usb-storage` disk (`qemu-xhci`, serial `SMOKEU`) formatted vfat with label `SMOKEU` and a file `hello.txt` (`mkfs.vfat`/`mcopy` on the host, no root). The report script on the guest checks and prints `ok`/`FAIL` lines:
  - `/media/tfuser/SMOKEU/hello.txt` readable; `stat -c %U` of it is `tfuser`; `/media/tfuser` owned by root; `readlink -f /run/media/tfuser` is `/media/tfuser`;
  - the line in `/run/thinfloe/removable` names `SMOKEU`, kind `stick`;
  - no mount of the boot disk under `/media/tfuser`;
  - `thinfloe-automount eject <disk>` exits 0, the mount and line are gone, the event says `clean`;
  - `check-boot-smoke.sh` passes (no ordering cycle).
- [ ] **Step 2: Extend the write smoke:** `SMOKEA`/`SMOKEB` get a vfat filesystem; the report checks both are automounted before the writes, the cancelled write to B leaves B automounted again (`remount`), the write to A succeeds.
- [ ] **Step 3: Run** `make qemu-smoke-automount`, `make qemu-smoke-write`, `make qemu-smoke-two-sticks` (second ThinFloe stick not under `/media/tfuser`: add that check to `smoke-stick-report.sh`), `make qemu-smoke` — expected: all PASS.
- [ ] **Step 4: Write docs.** `docs/notes/automount.md`: what is mounted where, the skip rules, the contract and event files, the hold, locks, the power-off rule, tests and owner checks (built-in SD slot, USB card reader card swap, a laptop's internal reader). `CHANGES.md` under `### New`, e.g.: "USB sticks, SD cards and USB disks are opened by themselves and appear under /media/tfuser, where ThinLinc's and OxuLnk's folder sharing can reach them. A new Eject tile appears while one is plugged in and tells when it is safe to pull it; Restart and Power off write pending data to the sticks first." and under `### Changed`: "Writing ThinFloe onto a stick that is open under /media/tfuser now closes it first instead of refusing it as mounted."
- [ ] **Step 5: Run gates** `make test-unit`, `make test`, `make test-runtime` (all with `TEST_THREADS=4`) — expected: PASS.
- [ ] **Step 6: Commit** `test(automount): QEMU smoke test, docs and changelog`.

## Part B — OSD notes

### Task 8: OSD notes from the contract file

**Files:**
- Create: `src/thinfloe-menu/src/osd/notes.rs`
- Modify: `osd/run.rs` (add an inotify fd on `/run/thinfloe` to the `poll` set at line 178; env `TF_REMOVABLE_FILE`, `TF_REMOVABLE_EVENT`), `osd/state.rs` (`Shown::Note`), `osd/draw.rs` (`PanelView::Note { text, warn }`), `osd/mod.rs` (`--probe` unchanged)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `removable::parse`, `removable::parse_event` (Task 5).
- Produces: `pub struct Notes { known: BTreeSet<String>, last_seq: Option<u64>, primed: bool }`, `pub enum Note { Connected(String), Removable(String), Unclean(String) }`, `impl Notes { pub fn new() -> Notes; pub fn update(&mut self, sticks: &[Stick], event: Option<&Event>) -> Vec<Note> }`, `pub fn text(n: &Note) -> (String, bool /* warn */)`.

- [ ] **Step 1: Write the failing tests** in `notes.rs`:
  - `first_read_is_a_baseline`: first `update` with 2 sticks and event seq 5 → no notes.
  - `first_read_of_a_missing_file_is_empty` (Review Focus 5): empty input → no notes; then a stick appears → `Connected`.
  - `new_stick_is_connected`, `clean_event_is_removable`, `unclean_event_is_a_warning`; same seq twice → one note.
  - `text`: `"MYSTICK connected"`, `"MYSTICK can be removed"`, `"MYSTICK removed without eject — files may be lost"` with `warn = true` only for the last.
  - `state` test: a note panel expires after the same 1.5 s as the volume panel (or the constant `NOTE_SHOWN` = 3 s; plan's call: 3 s, a sentence needs longer than a number).
  - `draw` test: the note panel has no bar (`has_bar() == false`), colours from the theme (warning colour for `warn`).
- [ ] **Step 2: Run** `cargo test osd::` — expected: FAIL.
- [ ] **Step 3: Implement.** Read both files on start (prime) and on every inotify event for their names; empty input region and no keyboard interactivity stay as for every OSD surface.
- [ ] **Step 4: Run** `make test-menu` and `make test-runtime` — expected: PASS. Add to the automount smoke report nothing (OSD is not drawn headless); add a `test/runtime` check only if the runtime harness already feeds files to the OSD.
- [ ] **Step 5: Commit** `feat(osd): notes when a stick is plugged in or pulled`.

## Part C — file list on the erase confirmation

### Task 9: `thinfloe-upgrade contents` and the confirmation list

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`_batch_parser` 2879: subcommand `contents --device DEV`; `_batch_run` 3090: handled before the lock), `src/thinfloe-menu/src/jobs.rs` (`upgrade_contents`), `upgrade.rs` (`Upgrade.contents: Contents`), `upgrade_ui.rs` (~line 418, both the internal and the USB confirmation), `test/unit/upgrade-write-args.tsv` only if it lists all commands the menu may call (it lists `write` calls; add a `contents` line if a sibling test checks menu argv against it)
- Test: `test/unit/test_upgrade_batch.py`, menu tests
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: contract file (Task 2), `Drive.automounted` (Task 4).
- Produces: JSON on stdout `{"device": str, "entries": [str], "more": int, "error": str|null}`; exit 0 even when `error` is set (a read problem is shown, not fatal); constants `CONTENTS_MAX = 20`, `CONTENTS_TIMEOUT = 3` s. Menu: `pub enum Contents { None, Loading, Listed { entries: Vec<String>, more: usize }, Failed }`; `jobs::upgrade_contents(cmd, device, feed) -> Contents`.

- [ ] **Step 1: Write the failing tests:**
  - `test_contents_lists_top_level`: 25 entries in a fake mount → 20 sorted names (directories with a trailing `/`), `more: 5`.
  - `test_contents_non_utf8_names`: a name with byte `\xff` → valid UTF-8 with U+FFFD, `json.loads` of the output succeeds and a serde parse in the menu test succeeds; a 500-char name cut to 80 chars plus `…`.
  - `test_contents_time_limit`: listing runs in a child with `CONTENTS_TIMEOUT`; a fake hung listing → `error: "could not read the stick"` within 4 s.
  - `test_contents_takes_no_lock`: with `upgrade.lock` held by another process, `contents` still answers.
  - `test_contents_not_automounted`: a drive with no automount → `entries: []`, `error: null`.
  - menu: `confirmation_lists_files`: `Listed` → text `"These files will be erased:"` followed by the names and `"and 5 more"`; `Loading`/`Failed`/`None` → no list and the Erase button still works; the job starts once when the confirmation opens, not on each 2 s drive refresh.
- [ ] **Step 2: Run** the Python and cargo tests — expected: FAIL.
- [ ] **Step 3: Implement.** Listing via `os.scandir` in a forked child (`multiprocessing` or `subprocess` of the tool itself) killed after `CONTENTS_TIMEOUT`; names decoded with `errors="replace"`.
- [ ] **Step 4: Run** `make test-unit`, `make test-menu`, `make qemu-smoke-write` (the report adds: `contents` of the automounted B lists its test file) — expected: PASS.
- [ ] **Step 5: Commit** `feat(upgrade): show the files a stick erase removes`.

---

## Gates before the first push

`make test-unit`, `make test`, `make test-runtime`, and the smoke tests the change touches: `qemu-smoke`, `qemu-smoke-automount`, `qemu-smoke-write`, `qemu-smoke-two-sticks` (all with `TEST_THREADS=4`). Then one whole-branch review, one fix wave, one scoped re-review (CLAUDE.md workflow).
