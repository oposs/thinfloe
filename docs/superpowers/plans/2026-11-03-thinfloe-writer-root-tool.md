# Image writer, root-tool half (Part A1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `thinfloe-upgrade` gains a question-free batch interface (`drives`, `write`) with machine-readable progress, a hashing writer that follows the stick, an uncached verify, USB-only drive selection, stick identity checks and a wipe of a cut-off write, proven by a new `make qemu-smoke-write`.

**Architecture:** Everything stays in the single executable `overlay/usr/bin/thinfloe-upgrade` (Python 3, stdlib only), the same pattern as `thinfloe-addon`. Batch mode is chosen by a first argument (`drives`, `write`); with no argument the interactive mode runs as today, on the same functions. The menu screens (Part A2) come after Part 0 and are not in this plan.

**Tech Stack:** Python 3 stdlib (`argparse`, `hashlib`, `json`, `fcntl`, `signal`, `subprocess`), `curl`, `zstd`, `lsblk`; tests with `unittest` (`make test-unit`); QEMU smoke as bash under `unshare`.

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md` (Part A; "Settled after the builder check" 5, 10; "Settled after the pushback review" 1, 2, 5, 10, 11).

## Global Constraints

- Never more than 4 cores: `BUILD_JOBS ?= 4`, QEMU `-smp $(BUILD_JOBS)`. Tests that write large data stay under `MEMCAP` (`systemd-run --user --scope -q -p MemoryMax=2G --`).
- No temp data in `/tmp` on the dev host; tests use `tempfile` under `TMPDIR` (the Makefile and the user set it to `/scratch/...`).
- Python stdlib only; the image has no pip packages.
- Progress lines, verbatim: `@step <id> <text>`, `@progress <bytes done> <bytes total or ->`, `@done <id>`, `@result ok|failed <one-sentence reason>`.
- Exit codes of `write`: 0 done, 1 failed (reason in `@result` and the log), 2 misuse/refused, 130 cancelled (SIGTERM).
- No `dd`. The write calls `os.fdatasync` every 32 MiB (`SYNC_EVERY = 32 * 1024 * 1024`; this plan picks fdatasync over `O_DIRECT`, measured on hardware later per the design).
- Default drive list: removable or `TRAN=usb` only; others only with `--all` (list) and `--allow-internal` (write).
- A drive with a mounted partition is never written by `--mode fresh`.
- The interactive mode keeps working over SSH; it asks every question before the first write.
- Changelog entry in `CHANGES.md` under `[Unreleased]`, written for users (house rules in `~/.claude/CLAUDE.md`).
- `test/unit/test_upgrade_image.py` may be reshaped; every behaviour it guards today stays guarded.

## Review Focus

1. **The stick is pulled out mid-write** (`write` raises `OSError` EIO/ENODEV): `@result failed` naming the device, exit 1, the wipe is attempted and its own failure only logged. → Task 6 test `test_write_error_fails_and_tries_wipe`.
2. **The network drops mid-download** (curl exits non-zero after some bytes): fresh install fails and wipes; upgrade fails without touching `tl/` or `DEFAULT`. → Task 6 `test_curl_failure_fresh_wipes`, Task 7 `test_download_failure_leaves_stick`.
3. **The server gives no size** (no zstd content size, no `Content-Length`): progress lines carry `-` as total, the write still finishes. → Task 3 `test_unknown_total_reports_dash`.
4. **The device node changed between `drives` and `write`** (sdb is now another stick of the same model): refused, exit 2, nothing written. → Task 5 `test_expect_id_mismatch_refuses`.
5. **SIGTERM during an upgrade** (not a fresh install): no wipe, a partial `tl.new/` is removed, `DEFAULT` is not set to `new`, exit 130. → Task 7 `test_sigterm_during_upgrade_keeps_stick_bootable`.

---

### Task 1: Progress reporter

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (new section `# ============ Progress ============` after the constants)
- Test: `test/unit/test_upgrade_batch.py` (new; loads the script with the same `importlib` loader as `test_upgrade_image.py`)

**Interfaces:**
- Produces: `class Progress` with `__init__(self, out: TextIO, batch: bool, min_interval: float = 0.5)`, `step(id: str, text: str) -> None`, `progress(done: int, total: Optional[int]) -> None`, `done(id: str) -> None`, `result(ok: bool, reason: str) -> None`. Module global `PROGRESS: Progress` (interactive by default), replaced in batch mode.

- [ ] **Step 1: Write the failing tests** `TestProgress`:
  - batch: `step("write", "Writing")` → `"@step write Writing\n"`; `progress(10, 100)` → `"@progress 10 100\n"`; `progress(10, None)` → `"@progress 10 -\n"`; `done("write")` → `"@done write\n"`; `result(False, "the stick was removed")` → `"@result failed the stick was removed\n"`.
  - newlines in `text`/`reason` become spaces (one line per message).
  - `progress` called twice within `min_interval` prints once, except when `done == total` (always printed).
  - interactive (`batch=False`): `step` prints `"==> Writing"`, `progress` prints nothing, `result(True, ...)` prints nothing.
- [ ] **Step 2:** `python3 -m unittest test.unit.test_upgrade_batch -v` → FAIL (`Progress` missing).
- [ ] **Step 3:** Implement `Progress`; flush after every line; `time.monotonic()` for the interval.
- [ ] **Step 4:** Same command → PASS.
- [ ] **Step 5:** Commit `upgrade: progress lines for the menu`.

### Task 2: Drive list with identity, USB filter and JSON

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`DriveInfo`, `enumerate_drives`, new `drive_identity`, `ptable_fingerprint`)
- Test: `test/unit/test_upgrade_batch.py`

**Interfaces:**
- Consumes: `find_efi_partition`, `examine_efi_partition`, `detect_boot_device` (existing).
- Produces:
  - `DriveInfo` gains `size_bytes: int`, `serial: str`, `transport: str`, `removable: bool`, `mounted: bool` (any partition or the disk has a mountpoint), `fingerprint: str`. Existing fields stay.
  - `enumerate_drives(include_internal: bool = False) -> List[DriveInfo]`: one `lsblk -J -b -o NAME,PATH,SIZE,MODEL,SERIAL,WWN,TRAN,RM,TYPE,MOUNTPOINTS` call, `TYPE == "disk"`, names `sd*`, `nvme*`, `vd*`; keeps a drive when `include_internal` or `RM` or `TRAN == "usb"`; the boot drive is always kept.
  - `ptable_fingerprint(device: str) -> str`: first 16 hex digits of SHA-256 over the first 17408 bytes (LBA 0 to 33: MBR, GPT header and entries); `"-"` when unreadable.
  - `drive_identity(d: DriveInfo) -> str`: `f"{serial or wwn or '-'}:{d.size_bytes}:{d.fingerprint}"`.
  - `drives_json(drives: List[DriveInfo], image_bytes: Optional[int]) -> str`: a JSON list; per drive `device, model, size_bytes, serial, transport, removable, mounted, is_boot, is_thinfloe, has_tl_old, has_tl_new, efi_partition, id` (= `drive_identity`), `fits` (`true`/`false`/`null` when `image_bytes` is unknown).

- [ ] **Step 1: Write the failing tests** `TestDriveList` with `lsblk` replaced by a fake returning a fixed JSON (one USB stick `sdb` `TRAN usb`, one internal `nvme0n1` `RM false TRAN nvme`, one `vda` boot drive, one `loop0`):
  - default list = `sdb` and `vda`; `include_internal=True` adds `nvme0n1`; `loop0` never.
  - a drive with `MOUNTPOINTS ["/media/x"]` on a partition has `mounted True`.
  - `ptable_fingerprint` of a temp file with known bytes equals the precomputed value; of a missing path is `"-"`.
  - `drive_identity` of two drives that differ only in fingerprint differ.
  - `drives_json` round-trips through `json.loads`; `fits` is `False` for a 2 GB drive and image 3148873728, `None` with `image_bytes=None`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. The interactive `show_drives_with_operations` uses the default list and offers `a) show all drives`.
- [ ] **Step 4:** Run `make test-unit` → PASS (old tests included).
- [ ] **Step 5:** Commit `upgrade: list USB drives only by default, with an identity per drive`.

### Task 3: Hashing writer that follows the stick

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (replaces `download_and_write_image`)
- Test: `test/unit/test_upgrade_batch.py`

**Interfaces:**
- Consumes: `Progress`, `image_size`.
- Produces:
  - `class WriteResult` (dataclass): `written: int`, `image_sha256: str` (uncompressed), `download_sha256: str` (compressed bytes).
  - `stream_image(url: str, device: str, total: Optional[int], progress: Progress) -> WriteResult`: runs `curl -fsSL <url>`; a reader thread hashes curl's stdout and feeds `zstd -dc`'s stdin; the main thread reads zstd's stdout in 1 MiB blocks, hashes and writes them to `device` opened `O_WRONLY` (no `O_TRUNC`, so a regular file in tests and a block device behave alike), `os.fdatasync` every `SYNC_EVERY` and once at the end, `progress.progress(written, total)` after each sync. Raises `WriteFailed(reason: str)` when curl or zstd exit non-zero or a write fails.
  - `class WriteFailed(Exception)` with `.reason`.

- [ ] **Step 1: Write the failing tests** `TestStreamImage` (fixture: 70 MiB of random bytes compressed with `zstd` into a temp dir, served as `file://` URL; device = temp file):
  - the device file afterwards equals the fixture; `image_sha256` and `download_sha256` equal `hashlib` of fixture and `.zst`.
  - `os.fdatasync` (patched with a counter wrapping the real one) is called at least 3 times (70 MiB / 32 MiB, plus the final one).
  - progress lines are non-decreasing and the last is `@progress 73400320 73400320`.
  - `test_unknown_total_reports_dash`: `total=None` → every progress line ends in ` -`.
  - a URL to a missing file → `WriteFailed`, reason mentions the download.
  - `test_write_error_raises` : `os.write` patched to raise `OSError(errno.EIO)` after 2 blocks → `WriteFailed`, reason names the device.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement; close pipes and `wait()` both children on every path, kill them on an exception.
- [ ] **Step 4:** Run under `MEMCAP` → PASS.
- [ ] **Step 5:** Commit `upgrade: write the stick without dd, in step with the stick`.

### Task 4: Uncached verify and the download checksum

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade`
- Test: `test/unit/test_upgrade_batch.py`

**Interfaces:**
- Consumes: `WriteResult`, `Progress`.
- Produces:
  - `drop_cache(fd: int) -> None`: `fcntl.ioctl(fd, BLKFLSBUF)` (`BLKFLSBUF = 0x1261`) when `fd` is a block device, then `os.posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED)` always.
  - `verify_device(device: str, length: int, expected_sha256: str, progress: Progress) -> bool`: `drop_cache`, read `length` bytes in 1 MiB blocks, report progress, compare.
  - `expected_download_sha256(base_url: str, name: str) -> Optional[str]`: fetches `<base_url>/<name>.sha256` (`sha256sum` format, file name in the second field must equal `name`); `None` when missing or malformed. (Part 5 may replace this with its one checksum file; keep the function as the single place to change.)

- [ ] **Step 1: Write the failing tests** `TestVerify`:
  - a temp file equal to the fixture verifies `True`; with one byte flipped at offset 40 MiB, `False`.
  - `drop_cache` on a regular file does not call `ioctl` (patched) but calls `posix_fadvise`.
  - `expected_download_sha256` with a `file://` dir holding `thinfloe-usb.img.zst.sha256` = `"<hex>  thinfloe-usb.img.zst\n"` returns `<hex>`; with another file name in it, `None`; missing file, `None`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `upgrade: check the written stick and the download`.

### Task 5: The `write` command line and its refusals

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`main()` dispatch, new `batch_main(argv: List[str]) -> int`)
- Test: `test/unit/test_upgrade_batch.py`

**Interfaces:**
- Consumes: `enumerate_drives`, `drive_identity`, `drives_json`, `image_size`.
- Produces:
  - `main()` calls `batch_main(sys.argv[1:])` when `sys.argv[1]` is `drives` or `write`; otherwise the interactive mode.
  - `thinfloe-upgrade drives [--all] [--source URL]` prints `drives_json` and exits 0.
  - `thinfloe-upgrade write --device DEV --mode fresh|upgrade --expect-id ID [--source URL] [--copy-home] [--verify] [--allow-internal]`. `--source` defaults to `DEFAULT_URL`. Refusals, each `@result failed <reason>` and exit 2, before anything is downloaded: device not in `enumerate_drives(include_internal=allow_internal)`; `drive_identity` ≠ `--expect-id`; `fresh` on the boot drive; `fresh` on a mounted drive; `fresh` when the image is known not to fit; `upgrade` on a drive that is not ThinFloe; `--copy-home` with `upgrade`, or when `home_restore_failed()`.
  - `ensure_root()` runs for both commands (sudo re-exec keeps the arguments).

- [ ] **Step 1: Write the failing tests** `TestBatchRefusals` (drives faked as in Task 2; `stream_image` patched to fail the test if called):
  - `test_expect_id_mismatch_refuses`: exit 2, output contains `@result failed` and `does not match`.
  - one test per refusal above, each exit 2 and `stream_image` not called.
  - `drives` without `--all` omits `nvme0n1`; with `--all` includes it.
  - `write` with an unknown `--mode` → exit 2 (argparse).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement with `argparse` subparsers; batch mode sets `PROGRESS = Progress(sys.stdout, batch=True)`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `upgrade: a write command that asks nothing`.

### Task 6: Fresh install in batch mode, with the wipe of a cut-off write

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`do_fresh_install` split into `fresh_install(drive, base_url, copy_home: bool, verify: bool, progress) -> bool`, used by both modes)
- Test: `test/unit/test_upgrade_batch.py`

**Interfaces:**
- Consumes: `stream_image`, `verify_device`, `expected_download_sha256`, `stamp_new_stick`, `copy_home_to_drive`, `save_home_before_finish`.
- Produces:
  - `wipe_partition_tables(device: str) -> bool`: writes zeros over the first and the last MiB, `fdatasync`; `False` (logged) on any error, never raises.
  - `class Cancelled(Exception)`; batch mode installs a SIGTERM handler that raises it.
  - Steps and ids, in order: `write` "Downloading and writing", `verify` "Checking the stick" (only with `--verify`), `stamp` "Marking the stick", `home` "Copying the home directory" (only with `--copy-home`), `save` "Saving this machine's home".
  - A download checksum mismatch (Task 4, when a checksum exists) fails after the write: reason `the download was damaged; write the stick again`; when no checksum exists, a log line `checksum not checked: <name>.sha256 not found`.
  - Any `WriteFailed`, verify `False`, checksum mismatch or `Cancelled` during `write` or `verify` → `wipe_partition_tables`, `@result failed <reason>` (exit 1) or exit 130 for `Cancelled`.

- [ ] **Step 1: Write the failing tests** `TestFreshInstall` (device = 128 MiB temp file; `stamp_new_stick`, `copy_home_to_drive`, `save_home_before_finish` patched to record calls):
  - success: steps printed in the order above, `@result ok`, return/exit 0; stamp called once.
  - `test_write_error_fails_and_tries_wipe`: `stream_image` raises `WriteFailed("the stick stopped answering")` → first and last MiB of the file are zero; exit 1; with `wipe_partition_tables` patched to return `False`, still exit 1 and a log line `could not clear`.
  - `test_curl_failure_fresh_wipes`: missing URL → wiped, exit 1.
  - verify `False` → wiped, reason `the stick did not read back what was written`.
  - checksum mismatch → wiped, reason as above; checksum missing → log line, exit 0.
  - SIGTERM: `stream_image` patched to `os.kill(os.getpid(), SIGTERM)` → wiped, exit 130, `@result failed cancelled`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement; the interactive `do_fresh_install` asks its two questions (continue, copy home) **before** calling `fresh_install`, and keeps its texts. Reshape `TestHomeCopyQuestion` to the new order.
- [ ] **Step 4:** `make test-unit` → PASS.
- [ ] **Step 5:** Commit `upgrade: fresh install with progress; a cut-off write leaves no bootable half`.

### Task 7: A/B upgrade in batch mode

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`download_firmware`, `do_ab_upgrade` take a `progress` argument)
- Test: `test/unit/test_upgrade_batch.py`, existing `TestAbUpgradeBootDefault`, `TestAbUpgradeSpace`

**Interfaces:**
- Consumes: `Progress`, `expected_download_sha256`, `save_home_before_finish`.
- Produces:
  - `download_firmware(base_url, work_dir, progress) -> str`: total = sum of `Content-Length` from `curl -sSfIL` per file (`None` if any is missing); each file downloaded through a Python read loop over `curl -fsSL` stdout, so `@progress` covers the three files as one bar.
  - Steps and ids: `save` "Saving the home directory" (boot stick only, first), `download` "Downloading the new release", `check` "Checking the files" (per file against `expected_download_sha256`; missing checksum → log line), `switch` "Switching to the new release".
  - `Cancelled` or a failure before `switch`: no wipe; a partial `tl.new/` (boot stick) is removed; `syslinux.cfg` untouched; exit 130 / 1.

- [ ] **Step 1: Write the failing tests** `TestBatchUpgrade` (ESP = temp dir, firmware files served as `file://`):
  - success on a boot stick: `save` comes first, then the three steps, `DEFAULT new` set, exit 0.
  - `test_download_failure_leaves_stick`: second file missing → `tl/` unchanged, no `tl.new/`, `DEFAULT` unchanged, exit 1.
  - `test_sigterm_during_upgrade_keeps_stick_bootable`: SIGTERM during download → same assertions, exit 130.
  - a checksum mismatch for `ramroot.sqsh` → exit 1, `tl.new/` removed.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement; the existing space check and the add-on room stay before anything is removed.
- [ ] **Step 4:** `make test-unit` → PASS.
- [ ] **Step 5:** Commit `upgrade: A/B upgrade with progress over the three files`.

### Task 8: `make qemu-smoke-write`

**Files:**
- Create: `scripts/run-qemu-write-smoke.sh`, `scripts/smoke-write-report.sh` (copied onto the ESP of the boot disk and run as `systemd.run=`, like `smoke-addons-report.sh`)
- Modify: `Makefile` (target `qemu-smoke-write: $(USB_IMAGE)`, variables `QEMU_WRITE_PREFIX`, `QEMU_WRITE_LOG`, in `.PHONY`), `CLAUDE.md` (a short paragraph on the batch interface, the progress lines, the USB-only rule and the wipe, after the `thinfloe-upgrade` bullets in "Persistent Home Directory"; `make qemu-smoke-write` in the list of smoke tests)

**Interfaces:**
- Consumes: `thinfloe-upgrade drives` / `write` from Tasks 5 to 7; the namespace HTTP pattern of `scripts/run-qemu-netboot-smoke.sh` (`unshare --user --map-root-user --mount --net`, `python3 -m http.server` on 127.0.0.1, reached by the guest at 10.0.2.2).
- Produces: a target that prints `ok   - …` lines and `PASSED - write smoke`, exit 0 only when all pass.

- [ ] **Step 1: Write the script.** Run 1: boot disk = copy of `$(USB_IMAGE)` as virtio; two blank sparse 4 GiB raw disks attached as `usb-storage` on `qemu-xhci` (so `TRAN=usb`). The report script, on the guest:
  1. `thinfloe-upgrade drives` → takes the two USB disks' `device` and `id` (by serial, set with `-device usb-storage,serial=SMOKEA` / `SMOKEB`);
  2. starts `write --mode fresh` to disk B, sends SIGTERM after the first `@progress`, requires exit 130 and the first MiB of B all zero;
  3. `write --mode fresh --verify --source http://10.0.2.2:<port>` to disk A, requires exit 0 and `@result ok`;
  4. prints its results on the serial console.
  Run 2: boot disk A alone, `check-boot-smoke.sh` must pass, and its `/config/stick-id` (read with mtools on the host) must differ from the boot disk's.
- [ ] **Step 2:** `make VERSION=<tag> qemu-smoke-write` on a built image → expected `PASSED - write smoke`. Also check that the namespace leaves no process behind (`pgrep -f http.server` empty after the run).
- [ ] **Step 3:** Add the changelog entry to `CHANGES.md`, for example: "Writing a new stick from the Upgrade tile shows each step and how far it is, checks the written stick, and only offers USB drives unless you ask for all; a write that is cancelled or fails leaves a stick that does not try to boot." (The tile itself arrives with the menu screens; keep the entry about the tool's behaviour that users already see over SSH, and extend it in Part A2.)
- [ ] **Step 4:** `make test-unit` and `make test` → PASS; `make qemu-smoke-upgrade` → PASSED (the interactive path is unchanged for it).
- [ ] **Step 5:** Commit `test: qemu-smoke-write writes a USB stick from the boot server and boots it`.

## Not in this plan

- `thinfloe-upgrade check`, the daily timer and the tile marker: they read the version file whose name and place Part 5 fixes; they go into the Part 5 plan or into Part A2, whichever comes first.
- The Upgrade screen in the menu (Part A2), after Part 0.
- `O_DIRECT` versus `fdatasync` on a real stick: measured on hardware after this plan.
