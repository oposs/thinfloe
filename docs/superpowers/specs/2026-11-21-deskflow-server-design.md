# Deskflow server in ThinFloe: design

Date: 2026-11-21. Status: draft for approval.

## Intent

ThinFloe's keyboard and mouse should drive the other computers on the desk
(a laptop, a Mac, a Windows box) the way they drive a second monitor: move
the pointer off the edge of a ThinFloe monitor and it continues on the
other computer, typing goes there, and text copied on one side can be
pasted on the other. The other computers run the stock Deskflow client
(also Barrier and Input Leap clients, which speak the same protocol).
ThinFloe is the Deskflow **server**.

The owner's picture of it: the multi-monitor arrangement in Settings →
Screen grows a second kind of screen. With the listen switch on, a device
that connects shows up there; the user checks its fingerprint and decides
where it attaches, exactly like placing a monitor.

## Goals

- ThinFloe acts as a Deskflow server (protocol 1.6 to 1.8) for stock
  clients on Linux, macOS and Windows that present a TLS client
  certificate: current Deskflow, and Input Leap and Barrier releases that
  send one. Clients that send no certificate are not supported (see
  Identity and pairing). The first Deskflow, Input Leap and Barrier
  releases that send a client certificate are named in
  `docs/notes/deskflow.md`; the interop test runs current Deskflow.
- Pairing at the machine: a connecting device appears as *pending* with its
  name and fingerprint; the user attaches it to a side of a ThinFloe
  monitor, or rejects it. Nothing connects without that step.
- Mutual TLS with fingerprint pinning, compatible with Deskflow's own
  scheme. ThinFloe shows its own fingerprint for the user to compare on the
  client.
- Keyboard, pointer, buttons and wheel go to the attached device while the
  pointer is on it, also when a ThinLinc or OxuLnk window fills the screen.
- Text clipboard in both directions (UTF-8 text only, size-capped).
- No root. One labwc patch is in scope for the media keys (see Keyboard);
  a second only if the ThinLinc spike (Risk 1) shows one is needed.

## Non-goals

- ThinFloe as a Deskflow **client** (being driven by another computer).
- Image, HTML or file clipboard; drag and drop (DFTR/DDRG are deprecated in
  Deskflow anyway).
- Plain-text (non-TLS) clients.
- Devices attached to other devices (a chain laptop → tablet). Every device
  attaches to a ThinFloe monitor.
- Hotkeys that switch screens; the screen-saver sync (CSEC is sent, nothing
  more); the macOS secure-input message (SECN) and language sync (LSYN) are
  accepted silently or not sent.

## Anti-goals (a failure even if it works)

- A ThinFloe screen that no longer takes keys because the pointer is "on"
  a device that is gone, hung or unreachable. Getting back must never depend
  on the network or on the client.
- A device that can connect without the user having attached it at this
  machine.
- A crash or hang in the Deskflow code that takes the menu (and with it the
  home save) down.
- A new compositor keybinding (standing constraint: labwc has none unless a
  key switch is on).

## Constraints from the project

- The menu crate draws on the CPU; no new GPU dependencies.
- `config.toml` is written only by the menu (`store.rs`); readers that are
  not the menu read it, never write it.
- labwc 0.9.3 in the image already offers every protocol needed (checked in
  the built `labwc` binary, 2026-11-21): `zwlr_layer_shell_v1`,
  `zwp_pointer_constraints_v1`, `zwp_relative_pointer_manager_v1`,
  `zwlr_data_control_manager_v1` / `ext_data_control_manager_v1`, and the
  shortcuts inhibitor of patch `0004`.
- The identity (private key) must not be copied by a fresh install; it
  rides the home archive like the SSH host keys (in the clear on the ESP,
  over plain HTTP on netboot — the same accepted risk).
- No firewall exists in the image; the listen switch is the only gate.
- New Makefile inputs (none expected beyond the crate) and the image size
  budget (`test-image-size.sh`) apply.

## Approaches considered

- **A. Inside the menu process.** Simplest wiring, but a fault in network or
  capture code can crash or stall the menu, which must save the home before
  it exits, and the menu's own UI would have to cope with "the keyboard is
  away". Rejected.
- **B. A second mode of the same binary, `thinfloe-menu --deskflow`**
  (chosen by the owner). Shares the crate's config schema, monitor geometry
  and Wayland code; runs as its own process, so a crash only ends Deskflow.
  The menu stays the only UI and the only writer of `config.toml`.
- **C. A separate crate and binary.** Strongest isolation, but duplicates the
  config, store and monitor code or forces a library split. No gain over B.

lan-mouse (GPL-3.0) is the reference for the capture technique; ThinFloe is
MIT, so no code is copied from it or from Deskflow.

## Design

### Processes

- The **menu** starts `thinfloe-menu --deskflow` when `[deskflow] listen =
  true`, in a process group of its own (child sets `PR_SET_PDEATHSIG`
  SIGTERM), restarts it after a crash with a
  back-off (1 s, 2 s, 4 s … 60 s cap), stops it with SIGTERM when the switch
  goes off, and sends SIGHUP after each write of `[deskflow]` so it reloads
  its config. The menu shows "Deskflow stopped: <reason>" from the state
  file when the daemon keeps failing. Before it exits (0, 10, 11) the menu
  sends the daemon SIGTERM and waits up to 1 s.
- The daemon also exits by itself when its Wayland connection closes, so a
  menu crash (labwc kept up by the failure window) or the end of labwc never
  leaves a daemon holding port 24800. Guard: `test-labwc-runtime.sh`
  (no `--deskflow` process and no listener on 24800 after logout, restart
  and a killed menu).
- The **daemon** (`--deskflow`) owns: the TCP listener, TLS, the protocol,
  the edge strips and pointer lock, the clipboard bridge. It reads
  `~/.config/thinfloe/generated/deskflow` (see Configuration), never
  `config.toml`, which holds the WiFi passwords and is read by nobody but
  the menu (`store.rs`). It writes only its state file and its identity.
- Commands from the menu go over a datagram socket
  `$XDG_RUNTIME_DIR/thinfloe-deskflow.sock`, like the OSD's
  (`osd/socket.rs`): `reject <name> <fingerprint>`, `reload`. SIGHUP stays
  as the reload fallback.
- State file `$XDG_RUNTIME_DIR/thinfloe-deskflow.state`, written atomically
  by the daemon, read by the menu on change (inotify or the menu's existing
  polling): one line per (name, fingerprint) pair,
  `name<TAB>fingerprint<TAB>state<TAB>since_epoch<TAB>width<TAB>height<TAB>flags`,
  state one of `pending`, `rejected`, `connected`, `active` (pointer on
  it), `offline`; width and height from the device's last DINF (0 when
  unknown); flags a comma list, so far `fp-changed` (the name is attached
  with another fingerprint). The same name may appear twice (the attached
  fingerprint and a changed one). First line
  `self<TAB><fingerprint><TAB><listen addr>` or `error<TAB><text>`. A
  contract like the battery file, with a unit-tested parser that ignores
  unknown trailing fields.

### Identity and pairing

- First start: the daemon makes a self-signed certificate (ECDSA P-256,
  CN `ThinFloe <hostname>`, valid 2000-01-01 to 2099-12-31 so that the
  boot clock does not shape it) and stores key and certificate
  in `~/.config/thinfloe/deskflow/identity.pem` (mode 600). This path goes
  into `IDENTITY_EXCLUDES` of `thinfloe-upgrade`; a fresh stick gets its own.
  Guard: an assertion in `test_upgrade_image.py`; the fresh-install rule in
  CLAUDE.md and `docs/notes/home-persistence.md` name the file.
  *Verify in the interop test that Deskflow clients accept an ECDSA server
  key; fall back to RSA-2048 if not.*
- Fingerprint = SHA-256 over the DER certificate, as Deskflow computes it.
  Shown as Deskflow's random-art picture plus the colon hex, so the user
  can compare it with what the Deskflow client shows.
- TLS: rustls, TLS 1.2 and 1.3, server always requests a client
  certificate. A client without one is dropped: approving by name alone
  would let any computer on the LAN take over an attached device's slot by
  sending its name. Client certificates are not
  chain-checked (Deskflow's are self-signed); trust is the fingerprint.
- Connection flow:
  1. TLS handshake; fingerprint F of the client certificate.
  2. Hello / HelloBack; client name N (capped at 64 bytes, shown escaped).
  3. If (N, F) is in `[[deskflow.device]]`: proceed (QINF/DINF, keep-alive).
  4. If N is known with a **different** F: refuse (EUNK, close), state
     `pending` with a warning "fingerprint changed" — the user must
     re-attach explicitly.
  5. Otherwise: record N, F as `pending`, send EUNK and close. The Deskflow
     client sets its retry flag on EUNK (`Client.cpp`) and tries again;
     once the user attaches it, the next try succeeds.
- **Order on first contact.** The Deskflow client checks ThinFloe's
  fingerprint during the TLS handshake and drops the connection before
  HelloBack if it does not trust it (`SecureSocket.cpp`). So the user first
  trusts ThinFloe on the client (comparing with the picture in the menu);
  only then does the device appear as pending here. The Screen band says
  so next to ThinFloe's fingerprint. The interop test pre-fills the client's
  `tls/trusted-servers` (`v2:sha256:<hex>`).
- Rejected devices are remembered in the state file for the session only
  and not shown again until they change fingerprint or the session ends.
- Limits against a noisy LAN: at most 8 pending devices (oldest dropped),
  4 simultaneous handshakes, 10 s to finish TLS and Hello, messages capped
  at 1 MiB except clipboard chunks.

### Configuration (`config.toml`)

```toml
[deskflow]
listen = false            # the switch; off by default
port = 24800

[[deskflow.device]]
name = "laptop"
fingerprint = "v2:sha256:…"   # Deskflow's own trusted-db line format
output = "DP-1"               # the ThinFloe monitor it attaches to
side = "right-of"             # left-of | right-of | above | below
clipboard = true
```

The menu derives `generated/deskflow` from this table (store order:
generated first, `config.toml` last), and `store.rs`'s rebuild of a broken
`config.toml` from `generated/` reads it back, so a broken `config.toml`
does not lose the pairings. The daemon reads only the generated file.

The output is named by the same criteria string the `[monitors]` profiles
use, so a device follows its monitor when the monitor arrangement changes.
The position belongs to the monitor, not to a profile: it applies in every
profile. In a profile where that monitor is missing, or where the chosen
edge touches another ThinFloe monitor, the device is shown but inactive
("this edge is not free in the current arrangement") and gets no strip.
The Position control offers the edges that are free in the active profile.

### Screen tab

- Settings → Screen gets a **Deskflow** band under the monitor arrangement:
  the switch "Let other computers use this keyboard and mouse (Deskflow)",
  ThinFloe's address and port, and its fingerprint picture.
- Pending devices are listed with name, fingerprint picture and two buttons:
  **Attach…** and **Reject**. Attach opens the same Position control the
  monitors use ("Right of DP-1"); confirming writes the device to
  `config.toml`.
- Attached devices are drawn in the arrangement view next to their monitor,
  as outlined rectangles scaled from the size the client reports in DINF,
  with a status dot (connected, active, offline). Selecting one shows
  Position, Clipboard on/off and **Forget**.
- All colours and text sizes from the theme and `brand.rs` (the style-literal
  test applies).

### Edge capture and the way back

- For each attached, connected device the daemon puts a layer-shell surface
  on the **overlay** layer along the attached edge of its monitor: 1 px deep,
  as long as the edge, transparent buffer, input region = the strip,
  exclusive zone 0, keyboard interactivity `none` while idle. Only edges
  that do not touch another ThinFloe monitor qualify (the Position control
  offers only those).
- **Edge resistance** (owner, 2026-11-21). Touching the edge is not
  enough: the pointer goes to the device only when the user keeps pushing
  into the edge. While the pointer is on the strip (the cursor cannot move
  further, but `zwp_relative_pointer_v1` still reports motion to the
  surface under the pointer), the daemon sums the outward component of the
  relative motion. The sum resets when the pointer leaves the strip or stops
  pushing outward for 300 ms. When it reaches the threshold, capture starts.
  A brush against the edge thus stays on ThinFloe. The strip still covers
  the edge pixel: a click there, and the pointer in the last row of a
  full-screen remote desktop at that edge (an auto-hide taskbar, a hot
  corner), reach the strip. Step 1 keeps this; the owner decides on
  hardware (notes, Known limits).
  - Setting `[deskflow] edge_resistance`: `off` (0 px), `low` (15),
    `medium` (40, default), `high` (100), in logical pixels of unaccelerated
    motion. One value for all devices, shown as a select in the Deskflow band.
  - Back from the device: no resistance (the tracked position crossing the
    attached edge ends the capture, as in Deskflow).
  - No visual hint while resistance builds up in the first version.
  - Unit tests: the accumulator (outward only, reset on leave and on the
    300 ms pause, each level's threshold).
- Resistance reached → the daemon:
  1. creates the pointer lock first (`zwp_pointer_constraints_v1`, lock,
     lifetime oneshot) and the relative-pointer object;
  2. then sets keyboard interactivity `exclusive` on the strip and
     activates a keyboard-shortcuts inhibitor on it (honoured by the new
     labwc patch, see Keyboard). Order matters: labwc 0.9.3 activates a new
     lock only for the active view (`create_constraint`,
     `src/input/cursor.c`) or on a keyboard focus change (`seat_focus`), so
     the lock must exist before the focus moves;
  3. waits for the `locked` event, and only then sends CINN with the entry point mapped proportionally along the edge,
     and from then forwards DMMV (absolute, tracked by the daemon inside the
     device's DINF size), DMDN/DMUP, DMWM, DKDN/DKRP/DKUP.
- The pointer leaves the device when the tracked position crosses back over
  the attached edge → COUT, unlock, interactivity `none`, warp-free return
  (the real pointer never moved while locked).
- **Release always wins.** The capture is dropped at once, without waiting
  for the client, on any of:
  - the escape combination **Ctrl+Alt+Shift+Escape** pressed on the ThinFloe
    keyboard (seen by the daemon's own keyboard focus; not a compositor
    binding);
  - a missed keep-alive (3 s rate, 3 misses) or a closed or failing socket;
  - the lock being cancelled by the compositor (`unlocked` event), the strip
    losing keyboard focus, or its output going away;
  - a watchdog thread that sees the event loop stall for more than 2 s: it
    aborts the process, and the compositor drops lock and focus when the
    client dies.
  Because all capture state lives in Wayland objects owned by the daemon, a
  crash of the daemon always returns pointer and keyboard to ThinFloe.

### Keyboard

- Keys come from the strip's `wl_keyboard` (evdev code plus the compositor's
  keymap); xkbcommon (already in the dependency tree through winit's
  `xkbcommon-dl`) turns them into keysyms and modifier state.
- KeyID: Deskflow's key ids (Unicode for characters, Deskflow's `kKey…`
  values for the rest, from `KeyTypes.h`). KeyButton: the evdev code + 8
  (the X11 keycode). Modifier mask from XKB state using Deskflow's
  `KeyModifierMask` bits. *Builder: take the KeyID table and mask bits from
  Deskflow's `KeyTypes.h`; they were not checked yet.*
- Keys held at entry are not sent; keys held at exit are released (DKUP) on
  the device before COUT. Repeat: Wayland sends no repeats, so the daemon
  generates DKRP at the rate and delay `wl_keyboard.repeat_info` reports.
- **Media keys follow the pointer** (owner, 2026-11-21). While the pointer
  is on a device, volume, mute, microphone mute and brightness act on that
  device, not on ThinFloe, and ThinFloe shows no OSD.
  - Today labwc runs the six media-key bindings (`allowWhenInhibited`,
    patch `0006`) before any client sees the key, and patch `0004` counts
    shortcut inhibitors only on toplevel views, so the strip cannot stop them.
  - New patch `0007-layer-inhibitor-blocks-all-keybinds.patch`: a
    keyboard-shortcuts inhibitor on a layer surface that has exclusive
    keyboard focus suppresses **every** keybinding, `allowWhenInhibited`
    ones too. Inhibitors on toplevel views keep today's meaning (OxuLnk and
    ThinLinc grabs still pass the media keys to ThinFloe). The daemon
    activates the inhibitor at capture and destroys it at release; a crash
    destroys it with the surface, so ThinFloe's media keys come back by
    themselves.
  - Rejected: rewriting `rc.xml` without the media keys and running
    `labwc --reconfigure` at each capture. It is slow, races fast edge
    crossings, and a crash in the captured state leaves ThinFloe without
    media keys.
  - The keysyms `XF86AudioRaiseVolume`, `…LowerVolume`, `…Mute`,
    `XF86MonBrightnessUp`, `…Down` map to Deskflow's audio and brightness
    KeyIDs (`KeyTypes.h`: `kKeyAudioMute` 0xE0AD, `kKeyBrightnessDown`
    0xE0B8, `kKeyBrightnessUp` 0xE0B9, the volume keys next to them).
  - **The microphone mute key stays on ThinFloe** (owner, 2026-11-21):
    Deskflow has no KeyID for it. While captured, the daemon does not send
    `XF86AudioMicMute` (F20 from `logitech-keys`, mapped by XKB) to the
    device; on its release it runs the same command the labwc binding runs,
    `thinfloe-menu --osd-key mic-mute` (`thinfloe-compose-labwc-config`),
    and only when the mic-mute key switch is on. So muting during a ThinLinc
    call works with the pointer on the laptop, and the OSD mark updates.
  - Guards: `make test-runtime` (the existing check that `allowWhenInhibited`
    keys pass a toplevel inhibitor, plus a new one that no binding fires
    while a layer surface with an inhibitor has the focus); the interop
    test checks that a volume key while captured reaches the client as
    DKDN and that ThinFloe's volume does not change.
  - The standing constraint "labwc has no keybindings unless a key switch is
    on" and the patch list in CLAUDE.md gain a line for `0007`.
- Key messages use the form the negotiated minor version requires: at 1.8
  DKDL and a DKRP with a trailing language string (`%2i%2i%2i%2i%s`, sent
  empty); at 1.6 and 1.7 DKDN and the four-field DKRP.

### Clipboard

- Text only, through `ext_data_control_manager_v1` (fall back to the wlr
  variant). Deskflow's flow (`Server.cpp`), which the client expects:
  - right after CINN the server sends DCLP with its current clipboard to
    the device it enters (no message lets a client ask for it);
  - while a device is active, every change of the ThinFloe selection is sent
    to it at once as DCLP;
  - CCLP tells devices that ThinFloe grabbed the clipboard;
  - after COUT the device sends CCLP and DCLP for what it copied; that text
    sets the ThinFloe selection.
  DCLP is chunked with marks 1/2/3 (all clients are ≥ 1.6).
- Cap: 1 MiB per transfer; larger content is not sent. Per-device switch.
- The primary selection (middle-click paste) is not bridged.

### Protocol subset

Server sends: Hello, QINF, CIAK, CALV, CROP, DSOP (empty), CSEC, CINN, COUT,
CCLP, DCLP, DKDL/DKDN, DKRP, DKUP, DMDN, DMUP, DMMV, DMWM, CBYE, EICV, EBSY,
EUNK, EBAD. Server handles: HelloBack, DINF, CALV, CNOP, CCLP, DCLP. Framing:
4-byte big-endian length before every message, the Hello included
(`PacketStreamFilter.cpp`, `ClientListener.cpp`). Hello name `Barrier`,
version 1.8, negotiate down to the client's minor (accept 6 to 8, else
EICV).

### Tests

- Unit tests (cargo): message encode/decode for every message above, version
  negotiation, framing limits, pairing decisions (known, pending, changed
  fingerprint, limits), KeyID mapping table, edge mapping and the
  tracked-position exit test, release triggers, state-file format.
- Interop test (`make test-runtime`, headless labwc): the real
  `deskflow-core client` (or `input-leapc`) in a container connects to the
  daemon; checks pending → attach via a written `config.toml` → connected;
  fingerprint changed → refused; injected pointer motion into the strip
  reaches the client (the client's log shows CINN/DMMV); escape combination
  and killing the client both release. Memory-capped like every adversarial
  input test (`systemd-run --user --scope -p MemoryMax=2G`).
- Hardware check by the owner: a ThinLinc full-screen session, a Mac or
  Windows client, typing with a non-US layout on both sides.
- Image size: record the growth in `test-image-size.sh` if needed.

## Risks

1. **Full-screen ThinLinc.** ThinLinc's X11 keyboard grab (patch `0002`) and
   patch `0003` (full-screen keeps focus) may keep the keyboard from going
   to the overlay strip. This is the first thing to build: a spike that puts
   a strip over a full-screen Xwayland window holding a grab and checks that
   pointer lock and exclusive keyboard focus move to it. The spike also
   tries patch `0007` against a grab, since an X11 grab is a toplevel
   inhibitor of its own. If they do not, a
   small labwc patch (an exclusive overlay layer surface takes focus over a
   grab) is the fall-back, with `make test-runtime`.
2. Deskflow client behaviour the source does not settle: the GUI's
   first-connect fingerprint dialog and ECDSA acceptance. Both are checked
   by the interop test before the UI work.
3. Non-US layouts on a client with a different layout: Deskflow maps by
   KeyID; mismatches are a known Deskflow limit, not fixed here.

## Build order

One design, built in three steps; each step ships something usable
(owner, 2026-11-21):

1. **Server core:** the ThinLinc spike (Risk 1), the interop harness,
   protocol, TLS and pairing, edge capture with edge resistance and the way
   back, keyboard and pointer, the Screen band. Media keys still act on
   ThinFloe in this step.
2. **Clipboard** (text).
3. **Media keys follow the pointer:** patch `0007`, the mic-mute rule, their
   runtime and interop checks.

## Left to the builder

The crate layout of the new modules (`deskflow/` with `proto`, `tls`,
`pair`, `capture`, `clip`, `state`), the choice of rustls crypto provider
and of the certificate crate (rcgen or hand-built DER), how the menu notices
state-file changes, the exact drawing of the device rectangles, and the
text of the Screen-tab labels (English, Simplified Technical English).
