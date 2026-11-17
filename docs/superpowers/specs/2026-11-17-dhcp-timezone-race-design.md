# DHCP time zone race: a watcher that never looks away

Date: 2026-11-17. Size: a small change. Status: design, decided by the
controller (architect ruling), for the owner's OK.

## Intent

A DHCP server can send the time zone (option 101). Today a lease that
networkd writes while `thinfloe-dhcp-timezone.service` is running is never
read: the machine stays at UTC until the next renewal, hours later. It broke
`make qemu-smoke-timezone` twice (PR #18 run 43772759802, PR #20 run
43835147372) and it hits real machines the same way.

## Cause

`thinfloe-dhcp-timezone.path` uses `PathChanged=`. While the service it
starts is active, systemd removes the inotify watch; when the service ends,
the path unit arms again but does not look at what changed meanwhile (only
`PathExists=` and `DirectoryNotEmpty=` check the state on re-arm, and those
two retrigger in a loop on a non-empty directory). The once-at-boot run of
the service opens the same window: a lease written during it is lost.

## Goals

- No lease change is lost, at boot or later.
- The validation in `/usr/libexec/thinfloe/dhcp-timezone` stays as it is.
- Same log lines, same tests pass; the smoke test passes every time.

Non-goals: Wi-Fi profiles still get no option 101; no change to the `TZ=`
order (home `TZ=`, then DHCP, then UTC).

Anti-goal: a fix that only narrows the window (it passes the smoke test
most of the time and fails on hardware once a month).

## Approaches

1. **One long-running watcher service (chosen).** `thinfloe-dhcp-timezone.service`
   becomes `Type=simple`: it arms an inotify watch on the lease directory
   first, then scans once, then scans again after every event. Events that
   come in during a scan wait in the kernel queue and the pipe, so nothing is
   lost. The path unit goes away, and with it the boot-time ordering trick.
   Needs `inotify-tools` (`inotifywait`, 164 KB with its library, Ubuntu
   universe, which the image already uses).
2. Keep the path unit; the script scans again until two scans in a row see
   the same lease files. Narrows the window to the time between the last
   scan and the service's exit, never closes it. Rejected (anti-goal).
3. Keep the path unit and add a timer that reruns the scan every few
   minutes. Bounded delay, not lossless, logs on every run unless the script
   remembers the last zone, and the smoke test would wait for the timer.
   Rejected.

Also considered: the inotify watch in Python through `ctypes` (no new
package, but ~40 lines of syscall code running as root to review), and
`networkd-dispatcher` (fires on link state changes, not on a renewal that
only changes the zone). Both rejected.

## Design

- `thinfloe-dhcp-timezone.service`: `Type=simple`, `Restart=on-failure`,
  `After=systemd-networkd.service`, `WantedBy=multi-user.target`, still no
  `Before=` on anything. `ExecStart=/usr/libexec/thinfloe/dhcp-timezone --watch`.
- `dhcp-timezone --watch`: start `inotifywait -m -q -e close_write,moved_to,create,delete`
  on the lease directory, wait until it reports the watch is set up
  (`Watches established.` on stderr, read before the first scan), scan once,
  then for each event line: drain the lines that follow within 0.5 s (a
  burst becomes one scan), scan. Without `--watch` the script scans once as
  today, so `test-dhcp-timezone.sh` keeps working unchanged.
- The scan logs `thinfloe: time zone <zone> from DHCP` only when the zone it
  sets differs from the last one it set in this run (a renewal every few
  hours would otherwise log the same line again; the first scan always logs).
- If the lease directory does not exist yet, the watcher waits for it by
  watching `/run/systemd/netif` for its creation, then proceeds as above. It
  never creates the directory (networkd owns it).
- `thinfloe-dhcp-timezone.path` is removed from the overlay, and the
  Dockerfile no longer enables it; `inotify-tools` is added to the package
  list.

## Tests

- `test-dhcp-timezone.sh`: a new case runs `--watch` against a temp lease
  dir with the stub `timedatectl`, writes a lease while a scan is still
  running (the stub sleeps), and expects that zone to be set; and a case for
  the directory appearing late.
- `test-session-packages.sh`: no `thinfloe-dhcp-timezone.path`, the service
  enabled, `inotifywait` present.
- `make qemu-smoke-timezone` and `make qemu-smoke-timezone-netboot` unchanged.
- `test-image-size.sh` budget: 164 KB fits; no raise expected.

## Left to the builder

The exact drain loop, and the `docs/notes/time-zone.md` and history wording.
CHANGES entry under Fixed: the DHCP time zone could stay at UTC after the
start when the lease came in while an earlier one was being read.
