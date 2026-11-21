# Deskflow server

ThinFloe is a Deskflow server (protocol 1.6 to 1.8). Computers that run the Deskflow client attach to a side of a ThinFloe monitor; pushing the pointer past that edge moves keyboard and mouse to them. Design: [the spec](../superpowers/specs/2026-11-21-deskflow-server-design.md).

## Processes

- The daemon is `thinfloe-menu --deskflow`, a process of its own. The menu starts it when `[deskflow] listen = true`, restarts it after a crash with a back-off (1 s doubling to 60 s), sends SIGHUP after each write of `[deskflow]`, SIGTERM when the switch goes off, and SIGTERM before it exits 0, 10 or 11 (waiting up to 1 s). The menu shows "Deskflow stopped: <reason>" when the daemon keeps failing.
- The daemon ends by itself when its parent dies or its Wayland connection closes, so no daemon keeps port 24800 after a logout, a restart or a killed menu. Guard: `test/runtime/test-labwc-runtime.sh` (`test-session-lifecycle.sh` runs stub menus without Wayland and only points there).
- A watchdog thread ends the daemon when its event loop stalls for 2 s. All capture state lives in Wayland objects the daemon owns, so a crash returns pointer and keyboard to ThinFloe.
- The daemon reads `~/.config/thinfloe/generated/deskflow` (derived by the menu from `[deskflow]` in `config.toml`), never `config.toml`. An absent file means the switch is off. It writes only its state file and its identity.

## Contracts

- State file `$XDG_RUNTIME_DIR/thinfloe-deskflow.state`, written atomically by the daemon. First line `self<TAB><fingerprint><TAB><listen addr>` or `error<TAB><text>`; then one line per (name, fingerprint) pair: `name<TAB>fingerprint<TAB>state<TAB>since_epoch<TAB>width<TAB>height<TAB>flags`. State is `pending`, `rejected`, `connected`, `active` or `offline`; the size comes from the device's last DINF (0 when unknown); flags is a comma list, so far `fp-changed`. The parser ignores unknown trailing fields.
- Command socket `$XDG_RUNTIME_DIR/thinfloe-deskflow.sock` (datagrams): `reject <name> <fingerprint>`, `reload`.
- Identity: `~/.config/thinfloe/deskflow/identity.pem` (mode 600), a self-signed ECDSA P-256 certificate made at the first start, valid 2000-01-01 to 2099-12-31 so that the boot clock does not shape it. A damaged file is reported and never regenerated. A fresh install leaves the directory out (`IDENTITY_EXCLUDES`); it travels with the home like the SSH host keys.

## Pairing

- The fingerprint is the SHA-256 of the DER certificate, shown as Deskflow's picture and as colon hex.
- The server always asks for a client certificate and drops a client without one: approving by name alone would let any computer on the LAN take over an attached slot. Trust is the fingerprint; the chain is not checked.
- After Hello and HelloBack, a known (name, fingerprint) pair proceeds. An unknown pair is recorded as `pending` and answered with EUNK. A known name with another fingerprint is refused the same way and marked `fp-changed`; the user must attach it again. A second connection under an attached name gets EBSY.
- A refused connection stays open up to 2 s after EUNK. Without that, the 1.26 client never parses EUNK and treats the close as a network failure.
- Limits: 8 pending devices (the oldest drops), 4 connections in the TLS and Hello phase, 10 s to finish both, messages up to 1 MiB.
- Order on first contact: the client checks ThinFloe's fingerprint during the TLS handshake and drops the connection before HelloBack when it does not trust it. The user trusts ThinFloe on the client first (comparing the picture), and only then does the device show up as pending.

## Settings → Screen

- The Deskflow band has the switch "Let other computers use this keyboard and mouse (Deskflow)", ThinFloe's address, port and fingerprint, and the edge resistance select.
- A pending device lists its name and fingerprint with a Position select and Reject. Picking an edge in the Position select attaches the device and saves it at once; there is no confirm button. Cancel ends an attach. "No monitor edge is free in the current arrangement." shows when no edge is free.
- An attached device is drawn in the monitor arrangement beside its monitor, scaled from the size it reports, with a status dot. Selecting it shows Position and Forget.
- A device whose edge another monitor took (or whose monitor is gone) is not drawn in the arrangement. The band lists it with "This edge is not free in the current arrangement.", Position and Forget.
- `[deskflow]` in `config.toml`: `listen`, `port`, `edge_resistance` and one `[[deskflow.device]]` per device (`name`, `fingerprint`, `output`, `side`). The position belongs to the monitor and applies in every profile.

## Capture and the way back

- For each attached, connected device the daemon puts a 1 px layer-shell strip on the overlay layer along the attached edge. Only edges that touch no other monitor qualify.
- Edge resistance: the pointer goes to the device only after the user keeps pushing outward. Outward motion from the relative pointer is summed, and the sum resets when the pointer leaves the strip or stops pushing for 300 ms. Thresholds: `off` 0, `low` 15, `medium` 40 (default), `high` 100 logical pixels.
- Only motion outward starts a capture, at every level: with `off` the first outward motion on the strip does. Reaching the strip, and motion inward or along the edge, never do, so the pointer that comes back from a device stays on ThinFloe although it still sits on the strip.
- At the threshold the daemon creates the pointer lock first, then takes exclusive keyboard focus on the strip, waits for `locked`, and only then sends CINN. The strip has no keyboard-shortcuts inhibitor yet (step 3, patch 0007). labwc 0.9.3 activates a new lock only for the active view or on a focus change, so the lock must exist before the focus moves.
- The pointer returns when the tracked position crosses back over the attached edge (no resistance).
- Release always wins and does not wait for the client: Ctrl+Alt+Shift+Escape on the ThinFloe keyboard (seen by the strip's own keyboard focus, not a compositor binding; by key code when the keymap cannot be read), a missed keep-alive (3 s, 3 misses) or a failing socket, the lock cancelled, the strip losing focus or its output going away, and the watchdog.
- Keys held at entry are not sent; keys held at exit get DKUP before COUT. Repeats are generated at the compositor's rate and delay. Ctrl+Alt+Fn falls back to the level-0 keysym, as Deskflow does.

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
- No clipboard yet. The spec's per-device "Clipboard on/off" switch comes with the clipboard (step 2).
- Media keys act on ThinFloe while the pointer is on a device (step 3 adds patch 0007 and the microphone rule).
- A client that sends no certificate is not supported.
- First contact needs the order above: trust ThinFloe on the client first.
- The primary selection, files and images are out of scope.
- Layout mismatches between ThinFloe and a client are a Deskflow limit (keys travel by key id).
