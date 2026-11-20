# Microphone mute key on Logi Bolt keyboards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The mic mute key of a Logi Bolt keyboard (MX Mechanical) sends `KEY_F20`, so labwc's `XF86AudioMicMute` binding or the focused program gets it.

**Architecture:** A root Python program diverts HID++ control `0x011C` (feature `0x1B04`) on every paired Bolt device and turns its diverted-button events into `KEY_F20` on a uinput keyboard. It loads `peripheral-battery` as a module for the HID++ code and is started by that reader's udev rule.

**Tech Stack:** Python 3 stdlib only (`fcntl.ioctl`, `select`, `struct`), systemd, udev; tests with `unittest`.

**Spec:** `docs/superpowers/specs/2026-11-20-logitech-mic-key-design.md`

## Global Constraints

- Program `/usr/libexec/thinfloe/logitech-keys`, unit `thinfloe-logitech-keys.service`; no new package.
- SWID `0x0B` (the battery reader has `0x0A`): set `pb.SWID = 0x0B` on this program's own loaded copy of `peripheral-battery`, whose `_feature_call` reads the module global.
- Divert only control `0x011C`, only when getCidInfo flags have `0x20`; setCidReporting (function 3) params `01 1c 03` to divert, `01 1c 02` to undivert; never the persist bits.
- The key is `KEY_F20` (evdev code 190), uinput device name `ThinFloe Logitech keys`.
- Every HID++ request under the battery reader's lock (`pb._take_lock(pb._paths()[3])`).
- Unit: not enabled, no `[Install]`, no `Before=`, `Restart=on-failure`, `RestartSec=5`; wanted by `90-thinfloe-peripheral-battery.rules`.
- Per node or slot failure: one journal line, skip. Exit 1 only when the uinput device cannot be made.
- English for code and docs; CHANGES.md entry under `[Unreleased]` → Fixed, written for users.

## Review Focus

1. A keyboard that sleeps and wakes (0x1D4B status 1) forgets the divert: the slot must be diverted again, or the key dies after the first sleep. Test in Task 2.
2. A second key held together with the mic key (event lists two CIDs, e.g. `00 c7 01 1c`): still a press of `0x011C`; its later absence is the release. Test in Task 1.
3. The battery watcher's own requests on the same node: replies with SWID `0x0A` must not be taken as answers. Test in Task 1 (`_feature_call` with SWID 0x0B ignores a 0x0A reply).
4. The receiver unplugged while the key is held: release sent, no traceback, the node rescanned later. Test in Task 2.
5. A mouse in another slot (MX Anywhere 3S has `0x1B04` but no `0x011C`): left alone, no divert sent. Test in Task 1.

---

### Task 1: Divert and key core

**Files:**
- Create: `overlay/usr/libexec/thinfloe/logitech-keys` (mode 755, `#!/usr/bin/env python3`, module docstring in the style of `peripheral-battery`'s: what it does, the measured CID, why F20)
- Test: `test/unit/test_logitech_keys.py` (loads the program with `SourceFileLoader` like `test_peripheral_battery.py`; reuse that file's `FakeReceiver` pattern, extended with `0x1B04` functions 0–3)

**Interfaces:**
- Produces:
  - `MIC_CID = 0x011C`, `KEY_F20 = 190`, `REPROG = 0x1B04`
  - `find_mic(t, slot: int, deadline: float) -> Optional[int]` — the `0x1B04` feature index when the slot has a divertable `0x011C`, else None (no request beyond root getFeature, getCount, getCidInfo)
  - `set_divert(t, slot: int, idx: int, on: bool, deadline: float) -> bool`
  - `mic_down(rep: bytes, idx: int) -> Optional[bool]` — for a diverted-buttons event of that index (via `pb.event_head`): True when CIDs in `rep[4:12]` (pairs) contain `0x011C`, False when not; None for any other report
  - `class Uinput` with `__init__(path="/dev/uinput")`, `key(code: int, down: bool) -> None` (EV_KEY then EV_SYN), `close()`; ioctl numbers `UI_SET_EVBIT=0x40045564`, `UI_SET_KEYBIT=0x40045565`, `UI_DEV_SETUP=0x405c5503`, `UI_DEV_CREATE=0x5501`, `UI_DEV_DESTROY=0x5502`; `struct input_event` as `"llHHi"`

- [ ] **Step 1: Write failing tests** in `test_logitech_keys.py`:
  - `test_find_mic_on_keyboard` — fake slot 1 with `0x1B04` at index 8 and CIDs `[0x00c7, 0x011c]`, flags `0x3a` → `find_mic` returns 8.
  - `test_find_mic_skips_mouse` — slot 2 with `0x1B04` index 10, CIDs `[0x0050, 0x00c4]` → None; `test_find_mic_not_divertable` — `0x011c` with flags `0x04` → None; `test_find_mic_no_feature` → None.
  - `test_set_divert_bytes` — the request written is `11 01 08 3b 01 1c 03` padded to 20 (function 3, SWID 0x0B); off sends `… 01 1c 02`.
  - `test_reply_with_other_swid_ignored` — the fake first answers with SWID 0x0A, then 0x0B: `set_divert` takes the second.
  - `test_mic_down` — `11 01 08 00 01 1c 00…` → True; `11 01 08 00 00 00 …` → False; `11 01 08 00 00 c7 01 1c …` → True; same bytes with index 7 → None; a reply with SWID (`11 01 08 3b …`) → None.
  - `test_uinput_events` — `Uinput` on a fake fd (patch `os.open`, `fcntl.ioctl`, `os.write`): ioctls in order EVBIT(EV_KEY=1), KEYBIT(190), DEV_SETUP with name `ThinFloe Logitech keys`, DEV_CREATE; `key(190, True)` writes EV_KEY 190 value 1 then EV_SYN 0 0.

- [ ] **Step 2: Run** `python3 -m unittest test.unit.test_logitech_keys -v` — expected: errors, the program does not exist.

- [ ] **Step 3: Implement** the interfaces above in `logitech-keys`. Load `peripheral-battery` once at import (`SourceFileLoader("peripheral_battery", "/usr/libexec/thinfloe/peripheral-battery")`, path overridable by env `TF_PERIPHERAL_BATTERY` for tests) and set its `SWID = 0x0B`. Requests via `pb._feature_index` / `pb._feature_call`.

- [ ] **Step 4: Run** the same command — expected: all pass; also `make test-unit`, expected exit 0.

- [ ] **Step 5: Commit** `feat(keys): divert the Bolt mic mute key and send F20 through uinput`

### Task 2: Service loop

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/logitech-keys`
- Test: `test/unit/test_logitech_keys.py`

**Interfaces:**
- Consumes: Task 1's functions; `pb.find_hidpp_nodes`, `pb.open_listener`, `pb.wait_readable`, `pb._paths`, `pb._take_lock`, `pb._receiver_register`, `pb.open_transport`, `pb.WIRELESS_STATUS`, `pb.log`.
- Produces: `class Keys` with `rescan()`, `step(timeout: float)`, `stop()`; `main(argv) -> int` (no arguments; any argument → usage, exit 2).

Behaviour: `rescan()` every 30 s like the battery `Watcher` opens a listener per new node and, under the lock, for each paired slot (register `b5/50+n`) runs `find_mic` and `set_divert(on=True)`; it records `(phys, slot) -> (node, idx, ws_idx)` (ws from root getFeature `0x1D4B`). `step()` reads ready listeners: a `0x1D4B` event with `rep[4] == 1` for a recorded slot diverts that slot again (under the lock); `mic_down` True/False sends F20 down/up on change only. A node read error drops the node, releases a held key, logs once. `stop()` (SIGTERM, SIGINT) releases a held key, undiverts every recorded slot under the lock, closes uinput, exits 0.

- [ ] **Step 1: Write failing tests:** `test_rescan_diverts_keyboard_only` (keyboard slot 1 diverted, mouse slot 2 gets no setCidReporting); `test_press_release_gives_f20` (events down, up → `key(190,True)`, `key(190,False)`, a repeated down sends nothing new); `test_reconnect_diverts_again` (a `11 01 04 00 01 …` event with ws index 4 → a second divert request); `test_node_gone_releases_key` (listener raises OSError(19) while held → `key(190,False)`, node dropped, no exception); `test_stop_undiverts_and_releases`; `test_requests_take_the_lock` (patched `_take_lock` counts; every divert inside it); `test_main_rejects_arguments` → 2; `test_main_exits_1_without_uinput` (Uinput raises OSError) → 1.

- [ ] **Step 2: Run** the module tests — expected: the new tests fail.

- [ ] **Step 3: Implement** `Keys` and `main`; `main` sets the signal handlers and loops `rescan` / `step(1.0)`.

- [ ] **Step 4: Run** the module tests and `make test-unit` — expected: pass, exit 0.

- [ ] **Step 5: Commit** `feat(keys): watch Bolt receivers, divert again on reconnect, give the key back on stop`

### Task 3: Unit, udev rule, image test, docs

**Files:**
- Create: `overlay/etc/systemd/system/thinfloe-logitech-keys.service` (comment block like the battery watcher's; `ExecStart=/usr/libexec/thinfloe/logitech-keys`)
- Modify: `overlay/etc/udev/rules.d/90-thinfloe-peripheral-battery.rules` (add `ENV{SYSTEMD_WANTS}+="thinfloe-logitech-keys.service"`, comment names both)
- Modify: `test/integration/test-session-packages.sh` (program executable in the rootfs, unit present, no `Before=`, no `[Install]`, the rule wants it)
- Modify: `docs/notes/media-keys.md` (section "Logi Bolt keyboards": cause, CID `0x011C`, F20 reason, owner checks from the spec), `docs/notes/peripheral-battery.md` (second user of its HID++ code, SWID 0x0B), `CLAUDE.md` ("Where code lives": the program next to the battery reader), `CHANGES.md` (Fixed: "The microphone mute key of Logitech keyboards on a Logi Bolt receiver, such as the MX Mechanical, did nothing. It now mutes the microphones when "Volume keys control this machine" is on, and otherwise reaches the program in front, ThinLinc included.")
- Check: `OVERLAY_DIRS` in the `Makefile` covers both new files (no new directory; `test-container-inputs.sh` passes)

- [ ] **Step 1: Write the checks** in `test-session-packages.sh`. No red run: each costs a 30-minute image rebuild; instead run the script's new block against the current image once (`test/integration/test-session-packages.sh` on the existing artifacts) and see the new checks FAIL.
- [ ] **Step 2: Add** the unit, rule line and docs.
- [ ] **Step 3: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test VERSION=mickey`, then `make test-runtime` the same way (Bash `timeout: 600000`, `run_in_background`, poll until it exits; never end the turn while it runs) — expected: exit 0, no `FAIL`, the new check names present as `ok`.
- [ ] **Step 4: Commit** `feat(keys): start the Bolt key service with the receiver; docs and changelog`
