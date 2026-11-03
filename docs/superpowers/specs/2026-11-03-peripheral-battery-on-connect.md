# Battery tile: read when a device connects

Follow-up after the owner tested the boot-time read on 10.46.102.86
(2026-11-03, commit 3f4baa4): the reader ran at 11.1–11.9 s, well before the
menu (~16 s), but neither Bolt device answered yet. After a reboot the Bolt
devices reconnect to the receiver only when used (mouse on first movement,
keyboard on first key press) — the owner saw the mouse after moving it and a
click, the keyboard later. Reading earlier cannot help.

## Decision (owner, 2026-11-03)

- **Read as soon as a device connects** (option 2). Not wanted: showing the
  last values from before the restart (option 1).
- **Hold-off:** once a device has a successful value, do **not** read it
  again on a connect event for about **5 minutes** ("wenn du einen
  erfolgreichen wert hast dann sicher ca 5 minuten kein neuen wert holst").
  The 5-minute timer and an explicit click on the tile stay as they are
  (a click is an explicit request and always reads).

## Mechanism

- A long-running root service (e.g. `thinfloe-peripheral-battery-watch.service`,
  `WantedBy=multi-user.target`, no `Before=`, `Restart=on-failure`) opens every
  HID++ hidraw node the reader would use (same selection rule as
  `find_hidpp_nodes`; rescans when nodes appear or vanish, e.g. every 5 s or
  via a udev trigger) and listens for the receiver's HID++ 1.0 **device
  connection notification** (sub-id `0x41`, short report `10 <slot> 41 …`; the
  link-status bit tells connected vs disconnected — see Solaar's
  `notifications.py`/`hidpp10` for the exact bits; **measure on the device
  before relying on it**).
- Bolt receivers may send `0x41` only when wireless notifications are enabled
  in HID++ 1.0 register `0x00` (Solaar sets the "wireless" flag). If the
  measurement shows no notification without it, the watcher enables it once
  per receiver (read register first, OR in the flag, write back; log it).
- On a "connected" notification for slot `n`: if the battery file holds a
  **successful** reading for that device (percent present, state not
  `not reachable`) younger than **300 s**, do nothing; else run one reading
  (the existing reader, under its flock, e.g. by starting
  `thinfloe-peripheral-battery.service` or calling the read function). Debounce
  bursts (several notifications within ~2 s → one read). A reading triggered
  this way that still finds the device unreachable may be retried once after
  ~2 s.
- The watcher never blocks on a device and never writes the battery file
  itself except through the reader's atomic write.
- Mapping slot → device for the hold-off check: the reader's name cache
  (`/run/thinfloe/peripherals-state/names`, keyed by HID_PHYS and slot) or a
  new small state entry; the simplest correct choice wins, record it.

## Measurement first (hardware, ask before ssh)

On 10.46.102.86 with the HID++ node found like the reader does (hidraw
numbers change across reboots): listen ~30 s on the node while the owner
switches the mouse off and on (and presses a key after a reboot); record the
raw `0x41` reports for connect and disconnect, with and without the
register-0x00 wireless flag. Put the table into the plan and the fake receiver.

## Tests

- Python: parsing of `0x41` (connect/disconnect bit), hold-off (fresh
  successful value → no read; stale or `not reachable` → read), debounce,
  node rescan, enabling notifications only when needed — with a fake transport
  and an injected clock.
- `test-session-packages.sh`: the watch service is enabled for
  multi-user.target, has no `Before=`.
- CLAUDE.md "Peripheral battery": the connect-triggered read, the 5-minute
  hold-off, the measured notification bytes.
