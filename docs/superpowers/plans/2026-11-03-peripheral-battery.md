# Peripheral Battery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inside the ThinLinc session, show the battery charge of the wireless
keyboard and mouse on the thin client, as a file under `~/thindrives/` on the
ThinLinc server.

**Architecture:** A root helper on the thin client reads the batteries every
5 minutes and writes `/run/thinfloe/peripherals/battery`. It reads Logitech
devices behind a receiver the kernel does not drive over HID++ 2.0 on hidraw,
and every other device through `/sys/class/power_supply` (`scope=Device`).
The ThinLinc add-on hook exports that directory read-only through the
client's local drive redirection (`NFS_SERVER_ENABLED`, `NFS_EXPORTS` in
`/opt/thinlinc/etc/tlclient_mandatory.conf`), so the server session sees
the file.

**Tech Stack:** Python 3 (stdlib only, already in the image), systemd
timer, tmpfiles.d, POSIX sh hook, ThinLinc client 4.21 local drives.

**Spec:** none written. The design is the conversation of 2026-11-03,
summarised here:

- Measured on 10.46.102.86 (kernel 7.0.0-38): a Logi Bolt receiver
  (`046d:c548`) is bound to `hid-generic`/`hid-multitouch`. Mainline
  `hid-logitech-dj` does not support Bolt, so `/sys/class/power_supply` is
  empty. A HID++ query on `/dev/hidraw4` (as root) returned
  `MX Anywhere 3S for Business 90 % discharging` and
  `MX Mechanical 100 % full`. The prototype is
  `/scratch/moetiker/claude-tmp/claude-1000/-home-moetiker-checkouts-thinfloe/0ff446b4-de92-4afc-bd39-529f3440a911/scratchpad/hidpp_battery.py`.
- Solaar (Ubuntu package 1.1.19) was tried on the same machine and
  rejected (owner, 2026-11-03: too large for the image): with
  `--no-install-recommends` it pulls in 44 packages, 39 MB installed,
  GTK 3 among them, which left the image on purpose. It read both
  batteries (as tfuser only after its udev rule was triggered); one
  `solaar show` took about 11 s.
- ThinLinc has no battery channel. Local drives (unfsd inside the client,
  tunnelled over the client's SSH connection) adds no listener on the thin
  client's network side; it does add an NFS service reachable from the
  ThinLinc server for the session's lifetime. Rejected: SSH from the
  server back to the client (needs credentials per client), an HTTP
  status port (new listener).
- The installed client's `/opt/thinlinc/etc/tlclient.conf` holds
  `NFS_SERVER_ENABLED=0` and `NFS_EXPORTS=/media,rw`; the shipped home's
  `~/.thinlinc/tlclient.conf` holds `NFS_SERVER_ENABLED=0`, and saved homes
  in the field carry their own copy. A setting in the user file beats the
  system file, so only `tlclient_mandatory.conf` (empty after install) can
  turn the export on for every machine.

## Global Constraints

- Output file: `/run/thinfloe/peripherals/battery`, directory
  `/run/thinfloe/peripherals` mode `0755 root root`, file mode `0644`. It
  is the only file ever in that directory, because the directory is
  exported. The temp file and the name cache live in
  `/run/thinfloe/peripherals-state` (`0700 root root`); the file is moved
  into place with `os.replace` (both on the `/run` tmpfs, so atomic).
- Line format, one device per line, tab-separated, no header:
  `name<TAB>percent<TAB>state<TAB>read_epoch`. `percent` is `0`..`100` or
  `-`; `state` is one of `discharging`, `charging`, `full`, `low`,
  `critical`, `unknown`, `not reachable`. Lines sorted by name.
- Names come from devices and are untrusted: control characters (including
  tab and newline) become `?`, then the name is cut to 64 characters; an
  empty name becomes `unknown device`.
- The helper never fails the unit because of a device: every per-device or
  per-node error is logged as `thinfloe: battery: <node or device>: <why>`
  and skipped. Exit 0 whenever the file was written.
- Whole run at most 15 s; each HID++ request waits at most 2 s (measured:
  an idle keyboard answers its first request after 850 ms).
- Every file name, unit name and log line in English, prefix `thinfloe-` /
  `thinfloe:` as elsewhere. No `Before=` on any sysinit.target unit (see
  CLAUDE.md, the hook-order constraint).
- The export is read-only: `NFS_EXPORTS=/run/thinfloe/peripherals,ro`.
  Nothing else is exported.
- Paths in the helper and the hook take an environment override for tests
  only: `TF_SYSFS` (default `/sys`), `TF_DEV` (default `/dev`),
  `TF_BATTERY_DIR` (default `/run/thinfloe/peripherals`),
  `TF_BATTERY_STATE` (default `/run/thinfloe/peripherals-state`),
  `TF_THINLINC_DIR` (default `/opt/thinlinc`), `TF_SHIM` (default
  `/usr/libexec/thinfloe/tlclient-pulseaudio`).
- The battery export must never fail the ThinLinc install: a failing hook
  fails the install (`_run_hook` in `overlay/usr/bin/thinfloe-addon`),
  also at every boot, and ThinLinc would vanish from the machine.

## Review Focus

1. **A device that is asleep or switched off** (keyboard idle for an hour,
   mouse switched off). Expected: its last known line stays, with state
   `not reachable` and the old `read_epoch`, so the server still shows
   "90 %, last read 10:15". Test: `test_unreachable_device_keeps_last_reading`.
2. **Receiver unplugged during a run** (`OSError` from `read`/`write` on the
   hidraw fd). Expected: that node is skipped, other sources still written,
   exit 0. Test: `test_node_vanishing_mid_run_is_skipped`.
3. **A hostile or broken name** (tab, newline, ESC, 300 characters).
   Expected: one line, sanitised per Global Constraints. Test:
   `test_device_name_is_sanitised`.
4. **A server-side reader reading during a write.** Expected: it sees the
   old or the new file, never a partial one. Test:
   `test_write_is_atomic` (asserts `os.replace` is used and no temp file is
   left after a run that fails during formatting).
5. **The ThinLinc client updated by `Update now`.** `dpkg -i` rewrites
   `tlclient_mandatory.conf` and the export would silently disappear.
   Expected: the hook runs after every install and update and re-applies
   the two keys. Test: `test-thinlinc-hook.sh` runs the hook twice and on a
   file that already has other mandatory keys.

---

## File Structure

| file | responsibility |
| --- | --- |
| `overlay/usr/libexec/thinfloe/peripheral-battery` | create: the reader, Python, no extension |
| `overlay/etc/systemd/system/thinfloe-peripheral-battery.service` | create: oneshot running the reader |
| `overlay/etc/systemd/system/thinfloe-peripheral-battery.timer` | create: `OnBootSec=30s`, `OnUnitActiveSec=5min` |
| `overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf` | create: the output directory |
| `containers/ubuntu/Dockerfile` | modify: enable the timer (block at line 376-391) |
| `overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc` | modify: write the two mandatory keys |
| `test/unit/test_peripheral_battery.py` | create: reader tests with a fake receiver and a fake sysfs |
| `test/integration/test-thinlinc-hook.sh` | create: the hook against a temp dir |
| `test/integration/test-session-packages.sh` | modify: image guards (units enabled, tmpfiles present) |
| `Makefile` | modify: run `test-thinlinc-hook.sh` in `make test` like the other integration scripts |
| `CLAUDE.md` | modify: new section "Peripheral battery", hardware status |

---

### Task 1: The reader

**Files:**
- Create: `overlay/usr/libexec/thinfloe/peripheral-battery`
- Test: `test/unit/test_peripheral_battery.py`

**Interfaces:**
- Produces (module-level, importable via `importlib` the way
  `test_addon.py` loads `thinfloe-addon`; entry point guarded by
  `if __name__ == "__main__"`):
  - `class Reading(NamedTuple): name: str; percent: Optional[int]; state: str; read_epoch: int`
  - `class Transport(Protocol): def write(self, data: bytes) -> None; def read(self, timeout: float) -> Optional[bytes]`
  - `class HidrawTransport(Transport)` wrapping an fd (`select` + `os.read`)
  - `def find_hidpp_nodes(sysfs: Path, dev: Path) -> List[Path]`
  - `def read_receiver(t: Transport, now: int) -> Tuple[List[Tuple[int, str, Reading]], List[Tuple[int, str]]]`
    returns `(slot, receiver_name, reading)` for reachable devices and
    `(slot, receiver_name)` for paired slots that did not answer
  - `def read_power_supply(sysfs: Path, now: int) -> List[Reading]`
  - `def merge(new: List[Reading], previous: List[Reading], unreachable_names: List[str]) -> List[Reading]`
  - `def sanitise(name: str) -> str`
  - `def format_lines(readings: List[Reading]) -> str`
  - `def parse_lines(text: str) -> List[Reading]` (reads the previous file)
  - `def main() -> int`

- [ ] **Step 1: Write the failing tests**

  The fake receiver `FakeReceiver(Transport)` in the test file answers
  HID++ 2.0 long reports (`0x11`, device index, feature index,
  `func << 4 | swid`) for a table of devices: `{index: {"name": str,
  "features": {feature_id: index}, "battery": (soc, level, charging)}}`,
  answers `0x10 idx 0x8f ...` for an unpaired index, and nothing (timeout)
  for an index in `asleep`. Tests:

  ```python
  def test_reads_unified_battery(self):
      t = FakeReceiver({1: dev("MX Anywhere 3S for Business", ub=(90, 8, 0)),
                        2: dev("MX Mechanical", ub=(100, 8, 3))})
      got, asleep = mod.read_receiver(t, now=1000)
      self.assertEqual(sorted(got), [
          mod.Reading("MX Anywhere 3S for Business", 90, "discharging", 1000),
          mod.Reading("MX Mechanical", 100, "full", 1000)])
      self.assertEqual(asleep, [])

  def test_falls_back_to_battery_status_1000(self):   # feature 0x1000: percent, status 0 -> discharging
  def test_voltage_only_device_has_no_percent(self):  # 0x1001 -> percent None, state "unknown"
  def test_unreachable_device_keeps_last_reading(self):
      prev = [mod.Reading("MX Mechanical", 80, "discharging", 500)]
      out = mod.merge([], prev, ["MX Mechanical"])
      self.assertEqual(out, [mod.Reading("MX Mechanical", 80, "not reachable", 500)])
  def test_device_off_since_boot_shows_receiver_name(self):  # no cache, slot paired, ping 0x8f/0x04 -> ("MX Anywhere3S", None, "not reachable", now)
  def test_empty_slot_is_not_a_device(self):          # register b5/50+n -> 0x8f code 0x08: no line, even though its ping says 0x09
  def test_slow_wake_is_reachable(self):              # first reply after 0.85 s -> reading, not "not reachable"
  def test_repaired_slot_drops_cached_name(self):     # cache says slot 1 = "MX Anywhere3S"/full name, receiver now says "MX Keys" -> name "MX Keys"
  def test_node_vanishing_mid_run_is_skipped(self):   # transport raises OSError on 2nd read; main() writes the power_supply lines, exit 0
  def test_device_name_is_sanitised(self):
      self.assertEqual(mod.sanitise("a\tb\nc\x1b" + "x" * 300), ("a?b?c?" + "x" * 300)[:64])
      self.assertEqual(mod.sanitise(""), "unknown device")
  def test_power_supply_device_scope_only(self):      # fake /sys/class/power_supply: BAT0 (scope System) skipped,
                                                      # hidpp_battery_0 (scope=Device, capacity 55, status Discharging) kept,
                                                      # capacity_level only -> percent None
  def test_find_nodes_skips_kernel_driven_logitech(self):
      # fake /sys/class/hidraw/hidraw{0..2}: 0 = 046D with ff00/0x11 descriptor, driver hid-generic -> kept
      # 1 = 046D, driver logitech-djreceiver -> skipped (kernel has the battery)
      # 2 = non-Logitech -> skipped
  def test_format_and_parse_round_trip(self):
      # sorted by name, "-" for None, exactly "name\tpercent\tstate\tepoch\n" per line
  def test_write_is_atomic(self):
      # TF_BATTERY_DIR=tmp; after main(): only "battery" in the dir, mode 0644;
      # with format_lines patched to raise: old file unchanged, no temp file left, exit 1
  def test_run_respects_budget(self):
      # six paired slots that never answer, request timeout 2 s -> main returns within 15 s,
      # devices not reached in time get "not reachable"
  ```

- [ ] **Step 2: Run them, see them fail**

  Run: `python3 -m unittest test.unit.test_peripheral_battery -v`
  Expected: errors loading `overlay/usr/libexec/thinfloe/peripheral-battery` (file does not exist).

- [ ] **Step 3: Implement the reader**

  Port the prototype. Decisions it leaves open:
  - Node selection: `/sys/class/hidraw/*/device/uevent` has `HID_ID` with
    vendor `0000046D`, `report_descriptor` contains `06 00 ff` and `85 11`,
    and the driver (`device/driver` link name) is not one of
    `logitech-djreceiver`, `logitech-djdevice`, `logitech-hidpp-device`.
  - Measured on 10.46.102.86 (Bolt `046d:c548`, 2026-11-03, mouse
    switched off, keyboard idle); the fake receiver reproduces exactly
    these replies:

    | request | slot state | reply |
    | --- | --- | --- |
    | ping `11 n 00 1a 00 00 5a` | device on | `11 n 00 1a 04 05 5a ...` after 850 ms (keyboard woke up) |
    | ping | paired, switched off | `10 n 8f 00 1a 04 00` (code `0x04`) after 52 ms |
    | ping | empty slot | `10 n 8f 00 1a 09 00` (code `0x09`) after 52 ms |
    | register `10 ff 83 b5 50+n 00 00` | paired | `11 ff 83 b5 50+n <pairing info>` |
    | register `10 ff 83 b5 60+n 01 00` | paired | `11 ff 83 b5 60+n 01 <len> <name bytes>`, e.g. `0e "MX Anywhere3S"`, `09 "MX MCHNCL"` |
    | register `b5 50+n` / `60+n` | empty slot | `10 ff 8f 83 b5 08 00` (code `0x08`) |

    The error codes are not the ones assumed from Solaar: an empty slot
    answers the ping with `0x09`, not `0x08`. Do not decide pairing from
    the ping. Slot `n` is paired when register `b5`/`50+n` answers; then
    ping it: a reply means reachable, `0x8f` with any code or no reply
    within the request timeout means `not reachable`.
  - Request timeout 2 s, not 0.5 s: the idle keyboard needed 850 ms to
    answer its first ping. An unreachable device answers in about 50 ms,
    so the 15 s budget only bites when several devices wake at once.
  - Battery features in order `0x1004` (function 1: soc, level bitmap,
    charging status), `0x1000` (function 0: level, next level, status),
    `0x1001` (function 0: mV). State mapping for 0x1004 charging status:
    0 `discharging` (or `low`/`critical` when the level bitmap says 2/1),
    1/2 `charging`, 3 `full`, other `unknown`. For 0x1000 status: 0
    `discharging`, 1 and 2 `charging`, 3 `full`, other `unknown`.
  - Names: the receiver knows a short pairing name for every paired slot,
    also while the device is off (register `b5`/`60+n`, third byte =
    chunk number starting at 1, then length, then the bytes; read chunks
    until `len` bytes are collected). The device knows its full name
    (feature `0x0005`, function 0 length, function 1 chunks):
    `MX Anywhere 3S for Business` vs. receiver `MX Anywhere3S`,
    `MX Mechanical` vs. `MX MCHNCL`. Use the full name when the device
    answers and remember it in `$TF_BATTERY_STATE/names`
    (`hid_phys<TAB>slot<TAB>receiver_name<TAB>full_name`, mode 0600,
    `hid_phys` = `HID_PHYS` from the node's uevent), written atomically
    after the main file. For a device that does not answer, use the
    cached full name while slot and receiver name still match (a
    re-paired slot has another receiver name), else the receiver name.
    `main` passes the resulting names to `merge`. A device that is off
    from boot on gets a line with its receiver name, `-` and
    `not reachable`.
  - Log to stderr, which the unit sends to the journal.

- [ ] **Step 4: Run the tests, see them pass**

  Run: `python3 -m unittest test.unit.test_peripheral_battery -v`
  Expected: all OK. Then `make test-unit`: OK.

- [ ] **Step 5: Commit**

  ```bash
  git add overlay/usr/libexec/thinfloe/peripheral-battery test/unit/test_peripheral_battery.py
  git commit -m "peripherals: read keyboard and mouse batteries"
  ```

### Task 2: Run it on the machine

**Files:**
- Create: `overlay/etc/systemd/system/thinfloe-peripheral-battery.service`,
  `overlay/etc/systemd/system/thinfloe-peripheral-battery.timer`,
  `overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf`
- Modify: `containers/ubuntu/Dockerfile` (the `systemctl enable` chain, lines 376-391)
- Test: `test/integration/test-session-packages.sh`

**Interfaces:**
- Consumes: `/usr/libexec/thinfloe/peripheral-battery` (Task 1), exit 0 on success.
- Produces: `/run/thinfloe/peripherals/battery` refreshed every 5 minutes from 30 s after boot.

- [ ] **Step 1: Write the failing guards** in `test-session-packages.sh`, in
  the style of lines 607-619:
  - `present "battery tmpfiles" 'usr/lib/tmpfiles\.d/thinfloe-peripherals\.conf$'`
  - the timer is enabled: `etc/systemd/system/timers\.target\.wants/thinfloe-peripheral-battery\.timer$`
  - the timer has `OnBootSec=30s` and `OnUnitActiveSec=5min`, and no
    `RandomizedDelaySec`
  - neither unit has a `Before=` line
  - the helper is executable in the image (`-rwxr-xr-x`)

- [ ] **Step 2: Run, see them fail**

  Run: `make test` (rebuilds the image). Expected: FAIL on the new lines.

- [ ] **Step 3: Write the units, the tmpfiles line and the enable**
  - tmpfiles: `d /run/thinfloe/peripherals 0755 root root` and
    `d /run/thinfloe/peripherals-state 0700 root root`
  - service: `Type=oneshot`, `ExecStart=/usr/libexec/thinfloe/peripheral-battery`,
    `StandardOutput=journal`, no `[Install]` (the timer starts it)
  - timer: `[Timer] OnBootSec=30s`, `OnUnitActiveSec=5min`,
    `[Install] WantedBy=timers.target`
  - Dockerfile: `systemctl enable thinfloe-peripheral-battery.timer && \`
    after the add-on timers.

- [ ] **Step 4: Run, see them pass**

  Run: `make test`. Expected: all ok, no `Found ordering cycle` in `make qemu-smoke`.

- [ ] **Step 5: Commit**

  ```bash
  git add overlay/etc/systemd/system/thinfloe-peripheral-battery.* overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf containers/ubuntu/Dockerfile test/integration/test-session-packages.sh
  git commit -m "peripherals: refresh the battery file every 5 minutes"
  ```

### Task 3: Export it to the ThinLinc session

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc`
- Create: `test/integration/test-thinlinc-hook.sh`
- Modify: `Makefile` (add the script to `make test`)

**Interfaces:**
- Consumes: `/run/thinfloe/peripherals` (Task 2).
- Produces: `$TF_THINLINC_DIR/etc/tlclient_mandatory.conf` containing
  exactly one `NFS_SERVER_ENABLED=1` and one
  `NFS_EXPORTS=/run/thinfloe/peripherals,ro`, every other line kept in order.

- [ ] **Step 1: Write the failing test** `test-thinlinc-hook.sh`
  (stub `install` is not needed: give the temp dir a fake
  `lib/tlclient/pulseaudio`). Cases, each printing `ok`/`FAIL` like the
  other integration scripts:
  - empty mandatory file -> the two lines
  - file with `PRINTER_ENABLED=0` and `NFS_EXPORTS=/media,rw` -> `PRINTER_ENABLED=0` kept, `NFS_EXPORTS` replaced
  - running the hook twice -> same file, byte for byte
  - mandatory file missing -> created, mode 0644
  - `etc/` missing -> created, file written
  - `etc/` not writable (a file named `etc`) -> hook exits 0, prints
    `thinfloe: battery export not configured: <why>`, shim still installed
  - the PipeWire shim is still installed (existing behaviour), source via
    `TF_SHIM=overlay/usr/libexec/thinfloe/tlclient-pulseaudio`; a failing
    shim copy still fails the hook (unchanged)

- [ ] **Step 2: Run, see it fail**

  Run: `sh test/integration/test-thinlinc-hook.sh`. Expected: FAIL on the mandatory lines.

- [ ] **Step 3: Extend the hook**

  `TL=${TF_THINLINC_DIR:-/opt/thinlinc}`; the shim path becomes `$TL/lib/...`,
  its source `${TF_SHIM:-/usr/libexec/thinfloe/tlclient-pulseaudio}`.
  The export is a function called as `export_battery || echo "thinfloe:
  battery export not configured: ..."`, so `set -e` does not end the hook
  on it: `mkdir -p "$TL/etc"`, then rewrite `tlclient_mandatory.conf`
  through a temp file in the same directory and `mv`: drop lines starting
  `NFS_SERVER_ENABLED=` or `NFS_EXPORTS=`, append the two lines. The shim
  branch must no longer `exit 0` before the export runs.

- [ ] **Step 4: Run, see it pass**

  Run: `sh test/integration/test-thinlinc-hook.sh` and `make test-unit`
  (`test_addon.py` runs hooks). Expected: all ok.

- [ ] **Step 5: Commit**

  ```bash
  git add overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc test/integration/test-thinlinc-hook.sh Makefile
  git commit -m "thinlinc: export the battery file to the session"
  ```

### Task 4: Hardware check and documentation

**Files:**
- Modify: `CLAUDE.md` (new subsection under "Customizing the System", one
  line in "Known Limitations" for what stays unverified)

- [ ] **Step 1: On 10.46.102.86** (after `make image`, publish or copy the
  stick; ask before every ssh): `cat /run/thinfloe/peripherals/battery`
  shows both devices with percentages; `systemctl list-timers
  thinfloe-peripheral-battery.timer` shows the next run; `grep NFS
  /opt/thinlinc/etc/tlclient_mandatory.conf` shows both lines.
- [ ] **Step 2: In a ThinLinc session** on `thinlinc-aarweg-01`: `ls
  ~/thindrives/` and `cat ~/thindrives/*/battery`; record the mount path
  (the name ThinLinc gives the export) in CLAUDE.md. Check that `touch`
  in it fails (read-only). Switch the mouse off, wait 5 minutes: its line
  says `not reachable`.
- [ ] **Step 3: Write the CLAUDE.md section:** what reads what, the file
  format, the mandatory keys and that they keep a user from choosing other
  drive exports in the client's options, the Bolt/kernel reason, why
  Solaar is not used, and the guards. Record the exposure next to the
  SSH-key and password-hash note: every session with the ThinLinc add-on
  runs an NFS server reachable from the ThinLinc server, exporting this
  one read-only directory. Mark anything Step 1-2 could not show as **Not verified on
  hardware**.
- [ ] **Step 4: Commit**

  ```bash
  git add CLAUDE.md
  git commit -m "docs: peripheral battery file and its ThinLinc export"
  ```

## Not in this plan

- Showing the value on the server (tray icon, notification): the server's
  desktop is not known yet, and it is not this repository.
- A battery line in `thinfloe-menu`.
- Devices other than Logitech HID++ and kernel `power_supply` (e.g.
  Unifying receivers already have a kernel driver and arrive through
  `power_supply`).
