# Peripheral battery: a tile in the menu instead of the ThinLinc export

Replaces `2026-11-03-peripheral-battery-switch.md`. The battery reader
(`/usr/libexec/thinfloe/peripheral-battery`, timer every 5 min) stays; what
it writes is now shown on a tile of `thinfloe-menu` on the thin client.

## Decisions (owner, 2026-11-03)

- **Own tile** in the ThinFloe menu shows the batteries.
- **The ThinLinc export goes away**: no `NFS_SERVER_ENABLED` /
  `NFS_EXPORTS` lines, the `10-thinlinc` hook goes back to the PipeWire
  shim only. Nothing to clean on existing sticks: `/opt/thinlinc` is
  reinstalled from the stored `.deb` at every boot.
- **No Settings switch.** The reader always runs (timer as now, no
  `ConditionPathExists`), and the tile appears by itself as soon as the
  battery file lists at least one device; with no device it is not drawn.
  The Settings row, the switch file and the battery job of the switch
  plan are removed.
- **Click = read now + log**: a click runs the reader at once and then
  writes every device as one log line (source `battery`).
- **Interval stays 5 minutes.**

## What the tile shows

- Name: the lowest percent of the reachable devices, e.g. `95 %`
  (`–` when no reachable device has a percent).
- Comment line(s): one short entry per device, e.g.
  `MX Anywhere 3S… 95 · MX Mechanical 100 ⚡` — name shortened to fit,
  a mark for charging, `–` for `not reachable`. Exact layout follows the
  existing tile drawing (name + comment); keep within it.
- Warning look (the existing warn tone of the brand palette) when any
  device is `low`, `critical`, or at 15 % or less.
- Icon: a battery icon (a shipped Lucide icon, like the other shipped
  tiles), or the letter if none fits.
- Limitation to document: the menu is hidden while a fullscreen window
  (the ThinLinc client) covers it.

## Mechanics

- Shipped tile file `overlay/etc/thinfloe/menu/<NN>-battery.desktop` with
  a new `X-TF-Action=battery`, honoured only in shipped files (as
  `X-TF-Action=settings` is). A user file of the same name may reword
  Name/Comment/Icon like any shipped tile, but the drawn name/comment come
  from the battery data while devices exist.
- The menu reads `/run/thinfloe/peripherals/battery` (bounded read, the
  same line format and parsing rules as the helper: tab-separated, 4
  fields, ignore malformed lines, sanitise names for display) when it
  starts, after a click's job ends, and again whenever the file changes
  (poll its mtime about every 10 s, or reuse an existing file-watch
  mechanism if the menu has one).
- Click: `sudo -n /usr/libexec/thinfloe/peripheral-battery` (no
  argument = one reading run) as a job in its own process group, output
  logged under source `battery`, tile busy while it runs, one job at a
  time; when it ends the menu re-reads the file and logs one line per
  device: `<name>: <percent> %, <state>, read HH:MM`.
- End-of-session dialogs: the read job is short and harmless when cut
  off; it is not listed or waited for (same reasoning as before).
- Helper: remove `enable`, `disable`, `status`, `thinlinc-config`, the
  switch file handling and the mandatory-config code; keep the reading
  run, its hardening and tests. Remove the service's
  `ConditionPathExists`. The directory `/run/thinfloe/peripherals` stays
  (0755, `battery` 0644) — the menu runs as tfuser and only reads.
- The exported-directory wording goes everywhere (CLAUDE.md, comments,
  tmpfiles comment).

## Tests

- Python: the subcommand/switch/thinlinc tests go; reading tests stay.
- Hook test back to the shim-only cases (and: the hook never touches
  `tlclient_mandatory.conf`).
- `test-session-packages.sh`: no `ConditionPathExists` on the service,
  the battery tile file is shipped with `X-TF-Action=battery`.
- Rust: parsing the file (good/malformed lines, sanitising), the tile's
  name/comment/warn state from given devices, hidden with no devices,
  the click starting exactly one job, the log lines after the job.

## Delivery

Rework on branch `peripheral-battery` (PR #4 was closed by the owner);
open a new PR against `main` when done.
