# Peripheral battery: a tile in the menu

The keyboard and mouse batteries are shown on a tile of `thinfloe-menu` on
the thin client. A root reader (`/usr/libexec/thinfloe/peripheral-battery`)
writes them to `/run/thinfloe/peripherals/battery`; the menu reads that file.

Two follow-ups refine this spec: `2026-11-03-peripheral-battery-tile-lines.md`
(one line per device by kind, red below 10 %, paging, `Refreshing…`; it
replaces "What the tile shows" below) and
`2026-11-03-peripheral-battery-on-connect.md` (boot run, read when a device
connects, 5-minute hold-off).

## Decisions (owner, 2026-11-03)

- **Own tile** in the ThinFloe menu shows the batteries.
- **No switch.** The reader always runs, and the tile appears by itself as
  soon as the battery file lists at least one device; with no device it is
  not drawn.
- **Click = read now + log**: a click runs the reader at once and then
  writes every device as one log line (source `battery`).
- **Interval 5 minutes.**
- Solaar (Ubuntu package 1.1.19) was tried on 10.46.102.86 and rejected:
  with `--no-install-recommends` it pulls in 44 packages, 39 MB installed,
  GTK 3 among them, which left the image on purpose. One `solaar show`
  took about 11 s.

## What the tile shows (superseded by tile-lines)

- Name: the lowest percent of the reachable devices, e.g. `95 %`
  (`–` when no reachable device has a percent).
- Comment line(s): one short entry per device.
- Icon: a battery icon (a shipped Lucide icon, like the other shipped
  tiles).
- Limitation to document: the menu is hidden while a fullscreen window
  (the ThinLinc client) covers it.

## Mechanics

- Shipped tile file `overlay/etc/thinfloe/menu/70-battery.desktop` with
  `X-TF-Action=battery`, honoured only in shipped files (as
  `X-TF-Action=settings` is). A user file of the same name may reword
  Name/Comment/Icon like any shipped tile, but the drawn text comes from
  the battery data while devices exist.
- The menu reads `/run/thinfloe/peripherals/battery` (bounded read,
  tab-separated, malformed lines ignored, names sanitised for display) when
  it starts, after a click's job ends, and whenever the file changes (the
  1 s add-on file watcher).
- Click: `sudo -n /usr/libexec/thinfloe/peripheral-battery` (no argument =
  one reading run) as a job in its own process group, output logged under
  source `battery`, one job at a time; when it ends the menu re-reads the
  file and logs one line per device.
- End-of-session dialogs: the read job is short and harmless when cut
  off; it is not listed or waited for.
- Directories: `/run/thinfloe/peripherals` 0755 root, `battery` 0644 (the
  menu runs as tfuser and only reads); `/run/thinfloe/peripherals-state`
  0700 root holds the temporary file, the name cache and the lock. A run
  holds the lock, so a click and the timer take turns.

## The reader

- Names come from devices and are untrusted: control characters (tab and
  newline included) become `?`, then the name is cut to 64 characters; an
  empty name becomes `unknown device`.
- Every per-device or per-node error is logged as
  `thinfloe: battery: <node or device>: <why>` and skipped. Exit 0
  whenever the file was written.
- Whole run at most 15 s; each HID++ request waits at most 2 s.
- Measured on 10.46.102.86 (kernel 7.0.0-38): a Logi Bolt receiver
  (`046d:c548`) is bound to `hid-generic`/`hid-multitouch`. Mainline
  `hid-logitech-dj` does not support Bolt, so `/sys/class/power_supply` is
  empty. The reader asks over HID++ on the receiver's hidraw node.
- Node selection: `/sys/class/hidraw/*/device/uevent` has `HID_ID` with
  vendor `0000046D`, `report_descriptor` contains `06 00 ff` and `85 11`,
  and the driver is not one of `logitech-djreceiver`, `logitech-djdevice`,
  `logitech-hidpp-device` (those give the kernel a `power_supply` entry).
- Measured replies (Bolt `046d:c548`, 2026-11-03, mouse switched off,
  keyboard idle); the fake receiver in `test/unit/test_peripheral_battery.py`
  reproduces exactly these:

  | request | slot state | reply |
  | --- | --- | --- |
  | ping `11 n 00 1a 00 00 5a` | device on | `11 n 00 1a 04 05 5a ...` after 850 ms (keyboard woke up) |
  | ping | paired, switched off | `10 n 8f 00 1a 04 00` (code `0x04`) after 52 ms |
  | ping | empty slot | `10 n 8f 00 1a 09 00` (code `0x09`) after 52 ms |
  | register `10 ff 83 b5 50+n 00 00` | paired | `11 ff 83 b5 50+n <pairing info>` |
  | register `10 ff 83 b5 60+n 01 00` | paired | `11 ff 83 b5 60+n 01 <len> <name bytes>`, e.g. `0e "MX Anywhere3S"`, `09 "MX MCHNCL"` |
  | register `b5 50+n` / `60+n` | empty slot | `10 ff 8f 83 b5 08 00` (code `0x08`) |

  An empty slot answers the ping with `0x09`, not `0x08` as assumed from
  Solaar, so pairing is never decided from the ping. Slot `n` is paired
  when register `b5`/`50+n` answers; then it is pinged: a reply means
  reachable, `0x8f` with any code or no reply within the timeout means
  `not reachable`.
- The timeout is 2 s, not 0.5 s: the idle keyboard needed 850 ms to answer
  its first ping. An unreachable device answers in about 50 ms.
- Battery features in order `0x1004` (function 1: soc, level bitmap,
  charging status), `0x1000` (function 0: level, next level, status),
  `0x1001` (function 0: mV). `0x1004` charging status: 0 `discharging`
  (or `low`/`critical` when the level bitmap says 2/1), 1/2 `charging`,
  3 `full`, other `unknown`. `0x1000` status: 0 `discharging`, 1 and 2
  `charging`, 3 `full`, other `unknown`.
- Names: the receiver knows a short pairing name for every paired slot,
  also while the device is off (register `b5`/`60+n`, chunks numbered from
  1). The device knows its full name (feature `0x0005`): `MX Anywhere 3S
  for Business` vs. receiver `MX Anywhere3S`, `MX Mechanical` vs.
  `MX MCHNCL`. The full name is used when the device answers and kept in
  `$TF_BATTERY_STATE/names` (`hid_phys<TAB>slot<TAB>receiver_name<TAB>full_name<TAB>kind`).
  A device that does not answer gets the cached full name while slot and
  receiver name still match (a re-paired slot has another receiver name),
  else the receiver name; it keeps its last reading as `not reachable`.

## Tests

- Python: the reading run against a fake receiver, node selection,
  `power_supply`, the name cache, the lock, the usage error.
- `test-session-packages.sh`: units, timer, tile file with
  `X-TF-Action=battery`, icon, directories.
- Rust: parsing the file (good/malformed lines, sanitising), the tile
  drawing from given devices, hidden with no devices, the click starting
  exactly one job, the log lines after the job.
