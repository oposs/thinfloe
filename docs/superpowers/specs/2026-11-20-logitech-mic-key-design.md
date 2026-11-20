# Microphone mute key on Logi Bolt keyboards

Short design. Approved approach: owner, 2026-11-20 ("build the service you
proposed"). Calls marked *(architect)* are the controller's.

## Intent

On the owner's MX Mechanical (Logi Bolt receiver `046d:c548`) the
microphone mute key does nothing. The kernel's `hid-logitech-dj` does not
drive the Bolt receiver, so `hid-generic` has it, and in that mode the
keyboard does not send the key at all: no input event, no HID report
(recorded on all event and hidraw nodes, 2026-11-20). The keyboard sends
the key only when host software diverts it over HID++ (feature
REPROG_CONTROLS_V4, `0x1B04`). Diverting control ID `0x011C` for 60 s gave a
HID++ event per press and per release (`… 08 00 01 1c …`, then `… 08 00 00
00 …`).

Goal: the key works like the mic mute key of any other keyboard.

- Switch "Volume keys control this machine" on: the labwc binding on
  `XF86AudioMicMute` runs the OSD, which mutes every microphone.
- Switch off: the key goes to the focused program, ThinLinc included.

Non-goals: the other diverted keys of the top row (they already send key
combinations); keyboards on receivers the kernel drives
(`hid-logitech-dj` exposes their keys itself); Bluetooth connections.

Anti-goal: a key that changes how the keyboard behaves on another computer.
The divert is temporary (no persist flag): the keyboard drops it when it
loses power or reconnects to another host.

## Approaches

1. **Root service: HID++ divert plus uinput** (picked by the owner). Works
   for every Bolt keyboard that has control `0x011C`, keeps the switch
   semantics, needs no change in labwc or the menu.
2. Install Solaar. Brings GTK and a user daemon to a RAM image, and its
   rules engine would act on the key outside the switch. Rejected.
3. Document a known limit. Leaves the key dead. Rejected by the owner.

## Design

- New program `/usr/libexec/thinfloe/logitech-keys` (Python, root), unit
  `thinfloe-logitech-keys.service`, started by the existing udev rule
  `90-thinfloe-peripheral-battery.rules` (a second `SYSTEMD_WANTS`), like
  the battery watcher: not enabled, stays once started, `Restart=on-failure`,
  no `Before=`.
- HID++ code is not copied: the program loads `peripheral-battery` as a
  module (`SourceFileLoader`) and uses its transport, `find_hidpp_nodes`,
  `_receiver_register`, `_feature_index` and `_feature_call`. *(architect:
  one HID++ implementation; a refactor into a shared module is not needed
  for one more user.)* It uses its own SWID (`0x0B`, the battery reader has
  `0x0A`), so neither takes the other's replies, and holds the battery
  reader's lock while it sends requests.
- For every paired slot of every HID++ node: find `0x1B04`, look up control
  `0x011C` with getCount / getCidInfo, and when it is there and divertable
  (flag `0x20`), setCidReporting with divert on and dvalid (`0x03`), no
  persist. Again for a slot on each WIRELESS DEVICE STATUS (`0x1D4B`)
  reconnection event of that slot, and for a node that appears later
  (rescan like the battery watcher).
- A diverted-buttons event (`0x1B04` event 0) that lists `0x011C` is a
  press, one that no longer lists it the release. The program sends them as
  `KEY_F20` down and up on one uinput keyboard named
  `ThinFloe Logitech keys`. *(architect: `KEY_F20`, not `KEY_MICMUTE`.
  XKB maps both to `XF86AudioMicMute` (`inet(evdev)`: `<FK20>` and
  `<I256>`), but X11 key codes end at 255, so `<I256>` cannot reach Xwayland
  or ThinLinc; `<FK20>` is code 198. Laptops send their mic key as F20 for
  the same reason.)* uinput through `fcntl.ioctl` (`UI_SET_EVBIT`,
  `UI_SET_KEYBIT`, `UI_DEV_SETUP`, `UI_DEV_CREATE`), no new package.
- A press still down when its node goes away, or at SIGTERM, is released.
  On SIGTERM the program also turns the divert off on every slot it
  diverted, under the lock, so a stopped service gives the key back.
- Every failure per node or slot goes to the journal once and is skipped.
  Exit 1 only when the uinput device cannot be made.

## Left to the builder

Names of functions and the file layout inside the program; how the rescan
timer and the select loop are shaped (follow `peripheral-battery watch`).

## Tests

- Unit tests (`test/unit/test_logitech_keys.py`, in `make test-unit`): a
  fake HID++ transport and a fake uinput. Divert sent only for a slot with
  a divertable `0x011C`; a slot without `0x1B04` or without the control is
  left alone; press and release give F20 down and up; a reconnection event
  diverts again; SIGTERM releases a held key and undiverts; the SWID is
  `0x0B`; requests run under the lock.
- Image test (`test-session-packages.sh`): the program and unit exist, the
  unit has no `Before=` and no `[Install]`, the udev rule wants it.
- Owner on hardware: the key with the switch on (OSD and microphone mark),
  with it off inside ThinLinc (the remote side sees `XF86AudioMicMute`),
  after the keyboard sleeps and wakes, after the receiver is plugged in
  again.

## Docs

`docs/notes/media-keys.md` gets a section on Bolt keyboards and the owner
checks; `docs/notes/peripheral-battery.md` names the second user of its
HID++ code; CLAUDE.md "Where code lives" gets the program; CHANGES.md under
Fixed.
