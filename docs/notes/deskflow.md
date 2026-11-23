# Deskflow server

ThinFloe is a Deskflow server (protocol 1.6 to 1.8). Computers that run the Deskflow client attach to a side of a ThinFloe monitor; pushing the pointer past that edge moves keyboard and mouse to them. Design: [the spec](../superpowers/specs/2026-11-21-deskflow-server-design.md).

## Processes

- The daemon is `thinfloe-menu --deskflow`, a process of its own. The menu starts it when `[deskflow] listen = true`, restarts it after a crash with a back-off (1 s doubling to 60 s), sends SIGHUP after each write of `[deskflow]` and after each saved keyboard choice, SIGTERM when the switch goes off, and SIGTERM before it exits 0, 10 or 11 (waiting up to 1 s). The menu shows "Deskflow stopped: <reason>" when the daemon keeps failing.
- The daemon's stderr goes into the menu's log as source `deskflow`, without the daemon's own prefix; the log copies every line to the session log. stdout stays on the session log.
- The daemon ends by itself when its parent dies or its Wayland connection closes, so no daemon keeps port 24800 after a logout, a restart or a killed menu. Guard: `test/runtime/test-labwc-runtime.sh` (`test-session-lifecycle.sh` runs stub menus without Wayland and only points there).
- A watchdog thread ends the daemon when its event loop stalls for 2 s. All capture state lives in Wayland objects the daemon owns, so a crash returns pointer and keyboard to ThinFloe.
- The daemon reads `~/.config/thinfloe/generated/deskflow` (derived by the menu from `[deskflow]` in `config.toml`, plus `mic_mute_local`, the volume key switch of `[keyboard]`), never `config.toml`. An absent file means the switch is off; a file without `mic_mute_local` reads as false. It writes only its state file and its identity.

## Contracts

- State file `$XDG_RUNTIME_DIR/thinfloe-deskflow.state`, written atomically by the daemon. First line `self<TAB><fingerprint><TAB><listen addr>` or `error<TAB><text>`; then one line per (name, fingerprint) pair: `name<TAB>fingerprint<TAB>state<TAB>since_epoch<TAB>width<TAB>height<TAB>flags`. State is `pending`, `rejected`, `connected`, `active` or `offline`; the size comes from the device's last DINF (0 when unknown); flags is a comma list, so far `fp-changed`. The parser ignores unknown trailing fields.
- Command socket `$XDG_RUNTIME_DIR/thinfloe-deskflow.sock` (datagrams): `reject <name> <fingerprint>`, `reload`.
- Identity: `~/.config/thinfloe/deskflow/identity.pem` (mode 600), a self-signed ECDSA P-256 certificate made at the first start, valid 2000-01-01 to 2099-12-31 so that the boot clock does not shape it. A damaged file is reported and never regenerated. A fresh install leaves the directory out (`IDENTITY_EXCLUDES`); it travels with the home like the SSH host keys.

## Pairing

- The fingerprint is the SHA-256 of the DER certificate, shown as Deskflow's picture and as colon hex.
- The server always asks for a client certificate and drops a client without one: approving by name alone would let any computer on the LAN take over an attached slot. Trust is the fingerprint; neither the chain nor the certificate's version is checked. The Deskflow GUI (1.26) makes X.509 v1 certificates, which webpki refuses, so the handshake signature is checked against the certificate's public key alone (`tls.rs`). Guards: `a_deskflow_gui_v1_certificate_is_taken`, the interop cases with `test/runtime/deskflow-gui-certs/`.
- A connection that ends before its client sent its name (TLS, Hello, the 10 s deadline) reaches neither the state file nor Settings, so the daemon logs it: `a connection from <address> ended before pairing: <reason>`, the same address and reason once per 5 minutes (`faillog.rs`), since a client tries again every few seconds.
- After Hello and HelloBack, a known (name, fingerprint) pair proceeds. An unknown pair is recorded as `pending` and answered with EUNK. A known name with another fingerprint is refused the same way and marked `fp-changed`; the user must attach it again. A second connection under an attached name gets EBSY.
- A refused connection stays open up to 2 s after EUNK. Without that, the 1.26 client never parses EUNK and treats the close as a network failure.
- Limits: 8 pending devices (the oldest drops), 4 connections in the TLS and Hello phase, 10 s to finish both, messages up to 1 MiB.
- Order on first contact: the client checks ThinFloe's fingerprint during the TLS handshake and drops the connection before HelloBack when it does not trust it. The user trusts ThinFloe on the client first (comparing the picture), and only then does the device show up as pending.

## Settings → Screen

- The Deskflow band has the switch "Let other computers use this keyboard and mouse (Deskflow)". While the server runs, one line shows the address it listens on ("This computer: <address>:<port>", a wildcard address replaced by the machine's IPv4 address when known) and "Fingerprint…" (a dialog with the random art and the fingerprint in colon-separated hex), with "Edge resistance" at its right end. "Edge resistance" shows with Deskflow off too, alone on its line, so it can be set beforehand.
- Below the line a table lists the computers: state dot, name, On, Clipboard, Compose and ⚙. The three switches are always visible; Clipboard ("Share the clipboard") defaults to on, Compose ("Handle Compose key on ThinFloe", `compose = true/false`) to off.
- A pending computer shows "Check and attach…" and "Reject". "Check and attach…" opens a dialog with the fingerprint, a Position select of the free edges, Attach and Cancel; there is no attach without it. "No monitor edge is free in the current arrangement." shows when no edge is free.
- The dialog of an attached computer (⚙, or a click on its box in the arrangement) has Position, the three switches, the fingerprint, "Forget <name>" and Close. Re-attaching a known computer under a new fingerprint keeps its `clipboard`, `compose` and `enabled` settings.
- On (`enabled` in `[[deskflow.device]]`, default true): off, the pointer stops at that edge, the computer stays connected and keeps its place (the edge stays reserved), and no clipboard is shared with it. Its box in the arrangement is drawn dimmed: name and state dot muted. Switching it off while the pointer is on it returns the pointer to ThinFloe at once.
- A computer whose edge another monitor took (or whose monitor is gone) is not drawn in the arrangement. The table lists it with "This edge is not free in the current arrangement.", and its dialog has Position and Forget.
- Only one dialog of the Screen tab is open at a time (a monitor's, a computer's, the attach check or this computer's fingerprint); opening one closes the others, and the screen under it takes no click. A dialog of a computer that vanished closes.
- `[deskflow]` in `config.toml`: `listen`, `port`, `edge_resistance` and one `[[deskflow.device]]` per device (`name`, `fingerprint`, `output`, `side`, `enabled`, `clipboard`, `compose`). The position belongs to the monitor and applies in every profile.

## Capture and the way back

- For each attached, connected device the daemon puts a 1 px layer-shell strip on the overlay layer along the attached edge. Only edges that touch no other monitor qualify.
- Edge resistance: the pointer goes to the device only after the user keeps pushing outward. Outward motion from the relative pointer is summed, and the sum resets when the pointer leaves the strip or stops pushing for 300 ms. Thresholds: `off` 0, `low` 15, `medium` 40 (default), `high` 100 logical pixels.
- Only motion outward starts a capture, at every level: with `off` the first outward motion on the strip does. Reaching the strip, and motion inward or along the edge, never do, so the pointer that comes back from a device stays on ThinFloe although it still sits on the strip.
- At the threshold the daemon creates the pointer lock first, then takes exclusive keyboard focus on the strip, puts a keyboard-shortcuts inhibitor on the strip, waits for `locked`, and only then sends CINN. With the inhibitor ThinFloe's labwc runs no keybinding while the strip has the keyboard, the media keys included (patch 0007); `end_capture` destroys it. labwc 0.9.3 activates a new lock only for the active view or on a focus change, so the lock must exist before the focus moves.
- The pointer returns when the tracked position crosses back over the attached edge (no resistance). `edge::reentry_point` maps the position along the client edge proportionally back to the strip (`0.0..=1.0`) in layout coordinates. After `Capture::end_capture` destroys the lock and drops exclusive keyboard interactivity, it creates a `zwlr_virtual_pointer_v1` (bound to the strip's output on manager v2, or across the layout bounding box on v1), warps to the layout pixel center with `motion_absolute` and `frame`, and destroys the virtual pointer (`labwc 0.9.3` omits the layer-surface origin on `zwp_locked_pointer_v1::set_cursor_position_hint`, which sent the cursor to layout `0` on a right-edge strip). `Daemon::release_at` updates `hover.at` to that layout position and resets push resistance.
- Release always wins and does not wait for the client: Ctrl+Alt+Shift+Escape on the ThinFloe keyboard (seen by the strip's own keyboard focus, not a compositor binding; by key code when the keymap cannot be read), a missed keep-alive (3 s, 3 misses) or a failing socket, the lock cancelled, the strip losing focus or its output going away, and the watchdog.
- Keys held at entry are not sent; keys held at exit get DKUP before COUT. Repeats are generated at the compositor's rate and delay. Ctrl+Alt+Fn falls back to the level-0 keysym, as Deskflow does.
- Per-device local Compose (`compose = true`, updated live on `reload`): while active on a device with local Compose enabled, `Multi_key` (`0xff20`) is kept on ThinFloe and fed to `libxkbcommon`'s `xkb_compose_table` (read on the first `Multi_key` after each keymap event, so no table is parsed while no device uses Compose; loaded from `LC_ALL`, `LC_CTYPE` or `LANG`, falling back to `en_US.UTF-8` when unset or `C`/`POSIX`/`C.*`). Subsequent keys stay swallowed until the sequence completes, cancels, or resets on release or `escape_chord`; lock keys (Caps Lock, Num Lock, Scroll Lock: `keymap::is_lock_key`) still go to the client mid-sequence, press and release, so its lock state follows ThinFloe's, and the compose state never sees them. Shift, Ctrl, Alt, Super and AltGr pressed mid-sequence stay held back, press and release; one held before the sequence is still released. A completed single Unicode scalar in the Basic Multilingual Plane (`U+0020..=U+007E`, `U+00A0..=U+D7FF`, `U+E000..=U+FFFF`) is sent as `KeyDown` and `KeyUp` with `COMPOSED_BUTTON = 1` and only lock modifiers (`Caps`, `Num`, `Scroll`). Dead keys (`dead_*`) bypass local Compose and still go to the client unchanged as combining key ids.
- Runtime test (`test/runtime/test-labwc-runtime.sh`, `run_deskflow_compose_reentry_case`): `deskflow-core` and `x11-probe.py --record` run on `Xvfb :99` (`setxkbmap -layout de -option compose:ralt`) while `x11-probe.py --buttons` runs fullscreen on labwc's Xwayland; with `compose = true`, `Multi_key` + `quotedbl` + `a` delivers `adiaeresis` without forwarding `Multi_key` while `dead_acute` still forwards; moving down to `y = 600` on the client and returning releases without bouncing, lands a click in `x = 1150..=1200`, `y = 550..=670` and re-enters at `MOTION 0 <y>` in `550..=670`; reloading `compose = false` forwards `Multi_key`. Guard for the Compose table files in the rootfs: `test/integration/test-session-packages.sh`.

## Media keys

- While a device has the keyboard, the strip's inhibitor keeps labwc's media key bindings from running, so the volume, mute and brightness keys go to the device (Deskflow key ids `0xE0AD` to `0xE0B9`) and ThinFloe shows no OSD for them.
- The microphone mute key (`XF86AudioMicMute`) has no Deskflow key id and stays on ThinFloe: its press and release are never sent, and on the release, when `mic_mute_local` is true, the daemon runs `thinfloe-menu --osd-key mic-mute` (`TF_OSD_KEY_CMD` replaces `thinfloe-menu --osd-key`, as for the composer) in a process group of its own without waiting; the loop reaps it. While 4 such commands still run, a further release runs nothing. A release whose press came before the capture does nothing.
- The menu writes `mic_mute_local` with the volume key switch (`Table::Keyboard` renders `generated/deskflow` too) and sends the daemon SIGHUP after a saved keyboard choice.
- Runtime test (`test-labwc-runtime.sh`, Deskflow media keys): with the stock client attached and captured, `XF86AudioRaiseVolume` reaches the client and not the recorder, `XF86AudioMicMute` reaches the recorder as `mic-mute` and not the client, and after the escape chord the volume key runs the binding again.

## Clipboard

- Text only. The daemon watches ThinFloe's clipboard through the data-control protocol (ext_data_control_v1, else zwlr_data_control_v1) on a Wayland connection of its own, so a clipboard fault never touches the capture. It reads a selection as UTF-8 text (`text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain`, `STRING`, `TEXT`), and offers a device's text under the same types plus `application/x-thinfloe-deskflow`. A selection carrying that marker is the daemon's own and is not reported back.
- ThinFloe's clipboard changes: the device with the pointer gets the text at once (CCLP, then DCLP); every other device that shares the clipboard only learns of the grab (CCLP). When the pointer enters a device later, the device gets the remembered text, unless it already holds those bytes (it gave them, or was sent them).
- ThinFloe's clipboard becomes something the bridge does not carry (no text type, an empty text, over the cap, or a read that failed or took over 2 s): the daemon forgets the remembered text and every device that shares the clipboard, the one with the pointer too, gets CCLP, so its clipboard empties and no older text reaches it later. A selection that ends because its program exited keeps the remembered text.
- A device whose Clipboard switch changes no longer counts as holding ThinFloe's text: the next entry sends it again.
- A device copies: the Deskflow client sends CCLP at once and its text (DCLP) when the pointer leaves it, or at once when the pointer is not there. The text becomes ThinFloe's selection, and the other devices learn of the grab.
- Cap: 1 MiB of text either way. DSOP tells the client `CLPS 1, CLSZ 1024` (KiB); a larger local selection is not sent and counts as one the bridge does not carry.
- Per device: Settings → Screen, the attached device's "Share the clipboard" switch, saved as `clipboard = true/false` in its `[[deskflow.device]]` (absent means on). Off sends DSOP `CLPS 0`, and the daemon sends that device nothing and takes nothing from it.
- Probe: `thinfloe-menu --deskflow --probe-clip` runs the bridge alone: it prints `changed <len> <sha256>` for each selection another client sets, and a `set <text>` line on its stdin makes that text the selection.
- Runtime test (`test-labwc-runtime.sh`): the stock 1.26 client runs on an Xvfb of its own, because labwc's Xwayland would carry the text between the X and the Wayland clipboard by itself and a test there would pass without Deskflow. The 1.26 X11 client notices a copy on its side only through the SelectionClear it gets when it owned the clipboard before, so the case from the device runs after the device received ThinFloe's text.

## Clients that send a certificate

ThinFloe needs a client that presents a TLS client certificate. The first releases that do, from their release notes and source history (only Deskflow 1.26.0 is tested):

- `Deskflow 1.20.0` (2025-03-03): "Peer Id Checking", deskflow/deskflow#7931, the fix for CVE-2021-42072 and CVE-2021-42073. Deskflow 1.17.0 to 1.19.0 are not checked.
- `Barrier 2.4.0` (2021-11-01): "Implement client identity verification" (debauchee/barrier commit 229abab), the same CVE fix. It is Barrier's last release.
- `Input Leap 3.0.0` (2024-10-04): the first Input Leap release; its history contains the Barrier commit.

## Verified against Deskflow 1.26.0

- The ECDSA P-256 server certificate is accepted, and the client ignores validity dates.
- `deskflow-core client --new-instance -s <dir>/Deskflow.conf` takes everything from the INI file; its TLS directory is `<dir>/tls` with the client certificate in `deskflow.pem` and trusted servers as `v2:sha256:<hex>` lines in `trusted-servers`. The runtime test makes the certificate with openssl.
- After EUNK the client retries after 1 s.

## Known limits

- While Deskflow is on, the strip covers the outermost row or column of pixels at each edge where a connected device is attached. A click there reaches the strip and does nothing, and a full-screen ThinLinc or OxuLnk session never sees the pointer in that row: an auto-hide taskbar, a hot corner or a close button in the corner at that edge does not react. Other edges are not affected. The owner decides on hardware whether this stays (Task 14).
- Clipboard, text only: images, files and rich text are not bridged, nor is the primary selection (select and middle-click). Text over 1 MiB is not carried.
- Clipboard, stale text: after a device took the clipboard and before its text arrives (while the pointer is still on it, or for the moment its DCLP is on the way), ThinFloe's clipboard still holds the old text.
- A client that sends no certificate is not supported.
- First contact needs the order above: trust ThinFloe on the client first.
- Layout mismatches between ThinFloe and a client are a Deskflow limit (keys travel by key id).
- Local Compose requires the client's layout to map the composed character from its key id; Compose outputs of multiple codepoints or characters outside the Basic Multilingual Plane are dropped because Deskflow key ids are 16-bit.
