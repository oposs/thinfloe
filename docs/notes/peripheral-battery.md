This note covers the root reader of keyboard and mouse batteries, its units, the battery file the menu reads, the HID++ details and what was measured on hardware.

The reader is `overlay/usr/libexec/thinfloe/peripheral-battery`; its
docstring has the file format and the watcher's protocol. The menu side is
in [status-area.md](status-area.md).

## Reader and file

The menu's status area shows the batteries of
keyboards and mice (owner, 2026-11-03). The reader `/usr/libexec/thinfloe/peripheral-battery`
(root; `thinfloe-peripheral-battery.service`, once at boot and then every
5 min from `thinfloe-peripheral-battery.timer`, which has no `OnBootSec`:
`OnUnitActiveSec` counts from the boot run) writes `/run/thinfloe/peripherals/battery`, one line per device,
sorted by name; devices of one name stay in a fixed order (slot, or place
in `/sys/class/power_supply`), never by value, because the menu numbers them
in file order. The boot run is `WantedBy=multi-user.target`, in parallel with the
menu and with no `Before=` (not on `getty@tty1` either: a slow device must
never delay the login). Measured 2026-11-03: the menu appeared about 16 s
after the kernel started, while a timer with `OnBootSec=30s` first read at
about 43 s, leaving the display empty for about 27 s. One reading takes 0.2 to
1 s (run budget up to 15 s). Line format: `name<TAB>percent<TAB>state<TAB>read_epoch<TAB>kind` (`percent`
`-` when unknown; `state` `discharging`, `charging`, `full`, `low`,
`critical`, `unknown` or `not reachable`; `kind` `keyboard`, `mouse`,
`numpad`, `presenter`, `trackball`, `touchpad` or `unknown`; the menu
still reads a 4-field line, kind unknown). A run holds
`/run/thinfloe/peripherals-state/lock`, so the boot run, the timer and
the watcher take turns. Any argument but `watch` is exit 2. A Logi Bolt receiver (`046d:c548`) is left
to `hid-generic` (mainline `hid-logitech-dj` does not support Bolt), so the
kernel has no battery for its devices: the reader asks them over HID++ on
the receiver's hidraw node. Every `power_supply` entry with scope Device
is added, and a `POWER_SUPPLY_CAPACITY` of 0 is no percent, as on HID++: a
driver reports it before it knows. Device names in a uevent file may be any
bytes: such a file is decoded with replacement and split on newlines only,
so a name cannot end a run and a Unicode line separator in one cannot add a
key. A literal newline in a name can: the file has no quoting, so what
follows it reads as a key of its own, and the last value of a key counts. Pairing comes from the receiver's register `b5`/`50+n`, never
from the ping, since an empty slot answers the ping with an error too
(code `0x09`; a switched-off device `0x04`, both after about 50 ms; an
idle keyboard took 850 ms to answer, hence the 2 s request timeout). Error
`0x08` (unknown device, measured) on the register is an empty slot; any other
error, or no reply in time, the run's budget included, says nothing about
the slot, which then keeps what the name cache knows of it as `not
reachable`, as do the slots of a node that fails during a run. So does a slot
whose pairing name does not arrive whole: any error or timeout on one of its
chunks. A slot nobody knows is logged and gets no line. A state file that
is not text, or holds digits `int()` refuses, counts as absent (logged
`corrupt, ignored`), for a run and for the watcher, and the next run writes
it anew. The
same register gives the kind: the low nibble of the first byte after the
sub-address, `1` keyboard, `2` mouse (measured: MX Mechanical `01`, MX
Anywhere 3S `42`), `3` numpad, `4` presenter, `8` trackball, `9`
touchpad (Solaar's numbers, unmeasured). A `power_supply` entry has no
kind; a name containing Keyboard or Mouse is taken as one. A device that
does not answer keeps its last reading and its kind as `not reachable`,
also one that stops answering partway through its reading (a feature it
lacks is told from one it did not answer about);
one off since boot gets the receiver's short pairing name (`MX MCHNCL`)
and `-`. A device that answers but does not give all of its full name keeps
the cached one. Full names and kinds are cached in
`/run/thinfloe/peripherals-state/names`, next to the temporary file.
Two devices can share a name, so a device's identity is `HID_PHYS/slot`
(a `power_supply` entry: its name, `#2` and so on for the next of that name,
since its sysfs name changes when it reconnects). Limit: that place is all
that tells two `power_supply` devices of one name apart. When the first one
goes, the second moves up to its place: the live line is right, but the line
shown as `not reachable` keeps the second device's last value, and the numbers
of the menu's two lines can swap. HID++ devices have no such limit. `readings` in the same
directory is the battery file with the identity in front of each line; the
last reading of a device comes from there, and the battery file keeps its
format. A new name under an identity is a device paired anew and gets no
old value. Solaar was tried and rejected: 44 packages,
39 MB, GTK 3.

The service has `TimeoutStartSec=60`; the helper waits up to 30 s for
its lock (`LOCK_NB`, retried), then logs `thinfloe: battery: lock busy`
and exits 1, so a hung run cannot block the next one for ever. The
directories come from `/usr/lib/tmpfiles.d/thinfloe-peripherals.conf`:
`/run/thinfloe/peripherals` 0755 (the menu reads the file as tfuser) and
`/run/thinfloe/peripherals-state` 0700 root.

## Read on connect

After a reboot a Bolt device reconnects only when it
is used (the mouse on first movement, the keyboard on first key press;
owner, 2026-11-03), so the boot run finds it asleep.
`thinfloe-peripheral-battery-watch.service` (`Type=simple`,
`Restart=on-failure`, `RestartSec=5`, no `Before=`) runs `peripheral-battery
watch`, which opens every HID++ node the reader would use read-only and
listens. It is not enabled: on a machine with no Logitech device it would be
14.5 MB of root Python doing nothing. `overlay/etc/udev/rules.d/90-thinfloe-peripheral-battery.rules`
starts it through `SYSTEMD_WANTS` on a `hidraw` node whose HID parent is named
`*:046D:*` (bus, vendor, product in upper-case hex, the vendor
`find_hidpp_nodes` takes from `HID_ID`), at boot through
`systemd-udev-trigger`'s replay and for a receiver plugged in later. It also
matches Logitech nodes without HID++ or with a kernel driver; the watcher
then idles. It stays running once started, and finds a receiver plugged in
again by its rescan. **Not verified on a booted system:** only `udevadm verify`
and a unit test of the pattern have run this rule. Measured on a test machine
(2026-11-03, mouse switched off and on): the receiver sends no `0x41`
connection notification (its register `0x00` reads `00`, notifications
off, and the watcher leaves it so); the device itself sends
`11 01 04 00 01 01 01` (slot 1, feature index 4 = WIRELESS DEVICE STATUS
`0x1D4B`, event 0, status `01` reconnection) and `11 01 09 00 64 08 00`
(index 9 = UNIFIED BATTERY `0x1004`, event 0: 100 %, full,
discharging). Nothing arrived at switch-off. The watcher takes a long
report from slot 1 to 6 with function 0 and software id 0 (a reply has
the reader's `0x0a`) as an event, and a `0x1D4B` event with status 1 or
any `0x1004` event as a connect. Feature indices differ per device (the
keyboard's `0x1004` is at 7), so the first event of a slot is answered
by the root feature's getFeature for both features, under the reader's
lock, cached per HID_PHYS and slot; it is the only request the watcher
sends. Only those indices tell a connect from another event, so while a
slot has none its reports are held (at most 16) and judged once it answers:
the lookup is tried up to 4 times (`LOOKUP_TRIES`), 30 s apart
(`LOOKUP_RETRY`), also with no further event, and then the timer reads the
device. When a slot's pairing name changes in the reader's name cache
(looked at every 5 s) it holds another device and its indices are asked
for again; until the reader has run, the old ones count. Events within 2 s (`DEBOUNCE`) give one
reading, the same code path as the plain run, under the lock, in the
watcher's process. **Hold-off** (owner, 2026-11-03): a device whose line
in the battery file has a percent, a state other than `not reachable`
and a `read_epoch` younger than 300 s (`HOLD_OFF`) is not read on its
events; the slot is mapped to that line through its identity in
`readings`. The
timer is not held off. Nodes are looked for every 5 s
(`RESCAN`); a node that fails a read is closed and found again then. A
bad report or a failed reading is logged and the watcher goes on. While
another run holds the lock, a lookup waits for it (up to 30 s) and the
watcher reads nothing meanwhile; the kernel queues up to 64 reports per
open file. Two things can let that queue overflow: a lookup waiting for
the lock, and a reading the watcher runs itself (its own listener fills
with the reader's replies). A connect event lost beyond 64 waits for the
timer.

## Tests

`test/unit/test_peripheral_battery.py` (a fake receiver that answers as the
Bolt receiver did on 2026-11-03; the watcher with the measured events, fake
nodes and an injected clock), `test-session-packages.sh` (units, timer,
watch service not enabled, the udev rule, no battery tile file,
directories), `battery.rs` (parsing, labels, pages) and the battery
element's tests in `src/thinfloe-menu/src/status/battery.rs`.

## Hardware results

These boots ran PR #6's menu, which drew the status area as SVG; the reader,
units and file are unchanged since, the egui elements have not run on
hardware.

**Confirmed on hardware (2026-11-03):** after a restart the
menu appeared at 16.9 s, the boot read at 12.2 s got the mouse, and the
keyboard's first key press at 22.6 s logged `thinfloe: battery: connected:
... slot 2, reading` and put its value in the menu; no ordering cycle. **Not measured:** what arrives when a device goes out of
range without being switched off.

**Confirmed on hardware (2026-11-03, Logi Bolt):** the built
images booted from the stick (three boots: the menu's battery lines, kind lines with the boot
read, the watcher) with no ordering cycle; both devices read in 0.5 to
1 s; the file carries the kind written by the built reader; the menu
showed `Keyboard` and `Mouse` lines (owner: "funktioniert"); a device that
had not reconnected after a restart showed as `not reachable`; the
watcher read the keyboard on its first key press (see above); and the
timer went on after the boot run (reads at 12 s, 343 s and 674 s after
boot).

Not verified on hardware: the battery symbol, the alert colour (no device
was below 10 %) and pages (only two devices).
