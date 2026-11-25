# FloeDrop: AirDrop share folder

Product name: **FloeDrop** (owner, 2026-11-25). It names the feature in
Settings, the OSD, `CHANGES.md` and the notes; the program stays
`thinfloe-share`; the folder is `FloeDrop` (owner, 2026-11-25).

Date: 2026-11-24. Status: design, awaiting owner approval.

## Intent

The people who use ThinFloe carry iPhones. They want to move a photo or a
file between the phone and the remote desktop they work in, the way they
would move it to a Mac: AirDrop. ThinFloe becomes an AirDrop device. A
folder `/media/tfuser/FloeDrop/` holds two parts:

- `inbox/<sender>/`: every delivery from a phone lands here, one folder per
  sender. ThinFloe accepts every delivery.
- `outbox/<device>/`: one folder for each AirDrop receiver in range. A file
  put into that folder is offered to that device.

A received file stays in the inbox, and the OSD shows that it arrived. A
file sent from the outbox: the OSD shows that it was delivered (owner,
2026-11-24).

ThinFloe has no file manager. People reach the folder from inside their
remote session: `/media/tfuser` is the place that ThinLinc's local drive
redirection and OxuLnk's folder sharing export (USB automount design,
`2026-11-24-usb-automount-design.md`). The share folder rides the same
path.

AirDrop on Linux is the work of Owly (github.com/t4t5/owly, MIT, Rust):
`owlyd` runs Apple's Wi-Fi link AWDL on an ordinary Wi-Fi card, and the
`owly-airdrop` library speaks AirDrop over it. A test on 2026-11-24 showed
that Owly runs on a ThinFloe 4.10.0 image without polkit and
NetworkManager; the Intel AC 3160 of that machine crashes in AWDL mode, so
the hardware test waits for a TP-Link Archer T3U Plus. This design assumes
the stick works.

## Goals

- iPhone → ThinFloe: a phone set to AirDrop "Everyone for 10 Minutes" sees
  the ThinFloe machine in its share sheet and sends files and folders. They
  land in `inbox/<sender>/` without a question on ThinFloe; the OSD shows a
  note.
- ThinFloe → iPhone: while a phone in "Everyone" mode is awake nearby, a
  folder for it appears in `outbox/`. Files put there are sent to it as
  one AirDrop offer; when the phone accepts and the transfer is done, the
  OSD shows a note and the sent files leave the folder.
- One switch in Settings turns the whole feature on or off. Off by default.
- Settings shows whether it works, and why not when it does not (no usable
  card, the card carries the network, the radio failed).

## Non-goals

- Android / Quick Share. The folder layout does not depend on the protocol,
  so a second transport can come later; it is not part of this design.
- AirDrop "Contacts Only". It needs an Apple-signed identity, which nobody
  outside Apple can get. Phones must use "Everyone for 10 Minutes".
- Links and text (AirDrop can send a URL). Only files and folders.
- A file browser or a transfer list in the menu.
- Keeping received files across a power off. The inbox is in RAM; it is not
  part of the home save.
- An accept prompt, a block list or a per-sender policy. The owner asked for
  "accept any incoming delivery".

## Anti-goals (a failure even if it works)

- The Wi-Fi card left in AWDL mode, or the machine's network lost, after a
  crash, a log out or the switch going off.
- AirDrop taking the Wi-Fi card that carries the machine's network.
- A slow or failing radio delaying the boot, the login or the menu.
- Received files filling the RAM until the session dies.
- A received file written outside `inbox/<sender>/` (path traversal in the
  archive), executed, or opened by anything.
- A phone asked again and again: a declined or failed offer must not repeat
  by itself (the folder is emptied after every offer).
- The OSD taking a click or focus (standing rule).

## Constraints from the project

- No polkit, no NetworkManager (standing). Privileged steps go through
  `sudo -n`, as `thinfloe-upgrade` does.
- The WiFi core stack comes from the kernel (standing). Owly needs none
  other.
- The session's long-running helpers follow the Deskflow pattern: the menu
  starts the daemon while its switch is on, restarts it with a back-off,
  SIGTERMs it when the switch goes off and before it exits 0, 10 or 11; the
  daemon sets `PR_SET_PDEATHSIG` and reads only a `generated/` file, never
  `config.toml`. `config.toml` is written only by the menu (`store.rs`).
- Contract files: written atomically, tab-separated, one item per line,
  readers ignore unknown trailing fields.
- New build inputs need their `Makefile` prerequisite lines (as
  `MENU_FILES`/`MENU_DIRS`), or tests validate a stale image. Guard:
  `test-container-inputs.sh`.
- `test-image-size.sh`: the budget is raised only with a reason in the
  commit message.
- Nothing after `slim-rootfs.sh` runs `dpkg`, `apt-get` or dracut.
- `/media/tfuser` is created by the USB automount work (`tmpfiles.d`,
  0755, tfuser). Whichever of the two lands first adds that entry; the other
  reuses it.

## Approaches considered

1. **Wrap the stock Owly program.** Ship Owly's release binaries and a small
   daemon (Python) that runs `owly receive`, `owly browse`, `owly send`
   and parses their text. Least own code, but the CLI's text is no
   interface, each send is a process, and every Owly update can break the
   parser.
2. **Own Rust daemon on the Owly libraries (chosen).** A new crate
   `src/thinfloe-share/` builds `thinfloe-share`, which uses `owly-airdrop`
   (its sans-IO core and tokio driver) and `owlyd`'s D-Bus client, pinned to
   one Owly commit. `owlyd` is built from the same commit. Typed events, one
   process, Rust like the rest of ThinFloe.
3. **Approach 2 as a mode of `thinfloe-menu`** (as `--deskflow` is).
   Rejected: Owly needs tokio and zbus, which the menu does not use; they
   would grow the menu and its build for nothing.
4. **Our own AirDrop stack, inspired by Owly** (owner, 2026-11-24, as a
   fallback). Rejected for the radio layer: `owly-awdl`/`owly-radio` carry
   the AWDL timing and a per-driver quirks table built from dozens of
   hardware reports; rewriting it means owning the hardest part alone. The
   parts that are ThinFloe's own (folders, BLE wake-up, supervision, UI)
   are written here anyway. Owly is MIT: if upstream stops, we vendor the
   pinned crates and carry them.

## The design

### Processes

- **`owlyd`** (root, from Owly): owns the Wi-Fi card, runs AWDL, lends
  `awdl0`. Installed with its unit, D-Bus activation file, D-Bus policy and
  `10-owly-awdl0.link` as Owly ships them, without the NetworkManager
  snippet and without the polkit action (no polkit). `After=NetworkManager`
  in its unit is harmless and stays.
- **`thinfloe-share --radio`** (root, through `sudo -n`): holds the radio.
  Owly's `Acquire()` asks polkit for a user caller, so tfuser cannot lease
  the radio here; for root, `owlyd` checks the caller before polkit and
  grants a plain `Acquire()` (`authorized()` in owlyd's `service.rs`,
  v0.4.3; the builder confirms it on the pinned commit). The helper uses
  that plain lease, **never `AcquireDetached()`**: a plain lease ends when
  the helper's bus connection drops, so a panic or SIGKILL of the helper
  gives the card back too, where a detached lease would hold it in AWDL
  mode with no owner. At start the helper releases any detached lease left
  behind. The user daemon starts the helper as a child with a pipe on its
  stdin; the helper prints one status line (`up <iface> <mode> <channel>`
  or `error <text>`) and gives the lease back when its stdin closes, on
  SIGTERM, or when `owlyd` drops it (`owlyd`'s own recovery journal in
  `/run/owly` covers a crash of `owlyd`). It also runs the BLE
  advertisement (below), which needs root on BlueZ's D-Bus. tfuser's
  NOPASSWD covers the `sudo -n`; no sudoers change.
- **`thinfloe-share`** (tfuser): the AirDrop receiver, the browser, the
  sender and the folder logic, on `awdl0`. Supervised by the menu exactly
  like the Deskflow daemon (`deskflow_sup.rs` is the model; the builder
  factors a shared supervisor or copies it, builder's call). Its stderr goes
  to the menu's log as source `share`.

### Card choice and the network guard

- The root helper writes `/etc/owly/radio.toml` before it asks for the
  radio: `station` is the first Wi-Fi interface that carries no address,
  no route, **and has no network configured**: a card named in
  `generated/wifi.yaml` (or the netplan Wi-Fi config the initramfs
  installed) is never taken, even while it is out of range or
  reconnecting, because exclusive mode would hold it and fight its WPA
  unit. A machine with a configured built-in card and a USB stick uses the
  stick. *My call.*
- Country: `country` in `radio.toml` is the ISO code of the machine's time
  zone (the first country of the zone's line in `zone1970.tab`; the zone
  as `thinfloe-time` resolves it from the Time band, DHCP or `TZ=`).
  Without it the kernel stays at world domain `00`, where 44 and 149 are
  no-IR, and AWDL falls back to channel 6 (slow: Owly reports 10 to 110 s
  per photo there). No zone, or `UTC`: no `country`, channel 6. *My call:*
  the time zone is already set per site, and a Settings field for a
  regulatory country would be one more thing to get wrong. Intel cards use
  channel 6 regardless (Owly).
- If every Wi-Fi interface carries the network, or the only card Owly can
  use would need exclusive mode while connected, the helper does not start
  the radio and reports `error` with the text "AirDrop needs a Wi-Fi card
  this machine does not use for its network." Shared mode (card stays
  connected; some Realtek and MediaTek cards, AP on channel 6, 44 or 149)
  is used when `owly doctor`'s probe (`Probe()` on D-Bus) says it is
  available.
- A card Owly marks untested is not used (`allow_untested` stays false).

### Folders

- Root: `/media/tfuser/FloeDrop/`, a **tmpfs of its own** (owner,
  2026-11-25), mounted at every boot whatever the switch says, so the path
  exists before a remote session exports it: a mount unit
  `media-tfuser-FloeDrop.mount` in `overlay/`, options
  `size=25%,uid=1001,gid=1001,mode=0755,nosuid,nodev,noexec`. The daemon
  makes `inbox/` and `outbox/` in it at start. Everything vanishes at power
  off. The folder is named after the feature (owner, 2026-11-25).
- The tmpfs is the cap: inbox and outbox share 25 % of the RAM, enforced by
  the kernel (a copy into a full outbox fails in the remote file manager
  with "no space"). `noexec` keeps a received file from being run from
  there.
- The folder survives the switch going off: received files stay until the
  user deletes them or the machine powers off. Outbox device folders are
  removed when the daemon stops, except folders that still hold files.

#### Inbox

- Every `/Ask` is accepted unless it would break the cap (below). Files go
  to `inbox/<sender>/`, where `<sender>` is the sender's name from `/Ask`
  (`SenderComputerName`, e.g. "Jane's iPhone"), made safe: `/`, control
  characters and a leading dot replaced; empty becomes "iPhone".
- A file whose name exists already gets a suffix: `IMG_0042 (2).HEIC`.
- Owly's driver extracts the archive in a sandbox; the daemon moves the
  finished files into place only after the transfer completed, so the
  remote session never sees a half-written file. A failed transfer leaves
  nothing.
- Cap: the tmpfs (above). An `/Ask` whose total size is larger than the
  free space of the tmpfs (`statvfs`) is declined before any byte comes, and the OSD says "Files from
  <sender> refused: the FloeDrop folder is full". The user frees space by deleting files from the session.
- Files are written mode 0644, owned by tfuser. Nothing is opened or run.
- OSD: "<file> received from <sender>" for one file, "<n> files received
  from <sender>" for more.

#### Outbox

- The daemon browses for receivers whenever the radio is up: BLE AirDrop
  advertisement (Apple manufacturer data 0x004C, AirDrop TLV type 0x05, zero
  contact hashes, random tag, per Owly's SPEC.md BLE-1 to BLE-5) plus mDNS
  browse on `awdl0` and `/Discover`. Owly's CLI does the BLE part in
  `owly-cli/src/bluez.rs`, outside the library; ThinFloe writes its own in
  the root helper (MIT allows a copy).
  *My call:* advertise continuously while the switch is on, since folders
  must appear when a phone opens AirDrop. The advertisement wakes only
  phones in "Everyone" mode.
- **No Bluetooth adapter** (the T3U Plus has none; some thin clients have
  none): receiving still works, sending cannot (a phone answers
  `/Discover` only after the BLE wake-up, SPEC BLE-1). The daemon then
  runs receive-only: no `outbox/`, state line `ready` with the flag
  `no-ble`, and the Settings status adds "Sending needs Bluetooth; this
  machine has none." *My call:* receiving is the main use, so it should
  not wait for a second adapter. An adapter that appears later (USB
  dongle) is picked up at the next radio start.
- A receiver that answers `/Discover` gets `outbox/<name>/` (name made safe
  as for the inbox; two devices with one name get `<name> (2)`; the folder
  belongs to the device's mDNS service ID while that stays).
- A device that is gone for 60 s loses its folder if the folder is empty.
  A folder with files stays; it is used again when a device with the same
  name comes back.
- Sending: inotify on each device folder (`IN_CLOSE_WRITE`, `IN_MOVED_TO`,
  `IN_CREATE` for directories). 3 s after the last change in a folder, all
  its entries go as one AirDrop offer (one prompt on the phone), folders
  included. Hidden files (leading dot) and files still open for writing are
  left out.
- Accepted and done: the sent entries are deleted from the folder; OSD
  "<n> files sent to <device>" ("<file> sent to <device>" for one).
- Declined, timed out (the phone's prompt: 60 s, Owly's default if it has
  one) or failed: the entries are deleted too (owner, 2026-11-25); OSD
  "<device> declined" or "Could not send to <device>" in the warning tone.
  Every offer ends with an empty folder, so a phone is never asked twice
  for the same files. *My call:* a failed transfer (link lost) counts like
  a decline; the user copies the files again to retry.
- Deleting in the outbox removes ThinFloe's copy only. Whether the
  original survives is decided on the remote side: the folder reaches the
  session as a redirected drive or shared folder, i.e. another filesystem
  there, and a drag onto another drive copies by default (Windows
  Explorer, GNOME Files, KDE Dolphin). A user who chooses *move* or
  *cut and paste* deletes the original on purpose; ThinFloe cannot see or
  stop that. The notes and `CHANGES.md` say so in one line.
- A file dropped into a folder of a device that is not in range waits until
  the device is back; then the 3 s rule applies.

### Settings

- On the Network tab, a band "FloeDrop" (*my call*: AirDrop uses the Wi-Fi
  card, and the Network tab already lists the cards). Switch "FloeDrop:
  receive and send files with iPhones nearby (AirDrop)". Below it, while on, one
  status line from the state file:
  - "Ready on <card> (<mode>, channel <n>). Phones must be set to AirDrop
    'Everyone for 10 Minutes'."
  - or the error text, in the warning tone.
  - and the folder path `/media/tfuser/FloeDrop`.
- "Name shown on phones": a text field, default empty = the machine's host
  name (`thinfloe-xxxxxx`). *My call:* the host name is unique per machine,
  which matters in an office full of thin clients.
- `config.toml`: `[share] enabled = false`, `name = ""`. The menu renders
  `generated/share` from it (`enabled`, `name`) and sends the daemon SIGHUP
  after a write; a name change takes effect at the next announcement.

### Contracts

- State file `$XDG_RUNTIME_DIR/thinfloe-share.state`, written atomically by
  the daemon. First line
  `ready<TAB><iface><TAB><mode><TAB><channel><TAB><flags>` (flags a comma
  list, so far `no-ble`),
  `starting` or `error<TAB><text>`; then one line per receiver in range:
  `name<TAB>folder<TAB>state<TAB>since_epoch`, state `idle`, `sending`,
  `declined` or `failed`. Readers ignore unknown trailing fields.
- OSD notes: the daemon sends a datagram to `thinfloe-osd.sock`:
  `note<TAB><tone><TAB><text>
`, tone `info` or `warn`. The OSD shows a
  note panel (the volume panel's place and style, text only, no icon that
  looks clickable) for 4 s; a new note replaces the old one. The USB
  automount design needs the same panel; whichever lands first builds it,
  the other reuses it (*my call*: one note panel, two callers).
- The root helper's stdin/stdout protocol is private to `thinfloe-share`.

### Build

- `src/thinfloe-share/`: own `Cargo.toml` and `Cargo.lock`, Owly crates as
  git dependencies pinned by commit (`rev = "21171d8f…"`, tag v0.4.3 or
  the newest at build time), built in the Dockerfile's `menu-build` stage
  with the menu's Rust toolchain, `--locked`, `-j 4`. `owlyd` is built
  there from the same commit.
- The Owly support files (unit, D-Bus files, `.link`) go into `overlay/`
  as copies taken from that commit, so a diff shows when they change.
- `Makefile`: `SHARE_FILES`/`SHARE_DIRS` as prerequisites of
  `.ubuntu-container`; `test-container-inputs.sh` learns them.
- Drivers and firmware: the T3U Plus needs `rtw88_8822bu` and
  `rtw88/rtw8822b_fw.bin`, both in the image now (checked on 4.10.0). A
  test pins them.
- Size: `owlyd` is 6.3 MB and the owly CLI 9.6 MB in Owly's package;
  `thinfloe-share` replaces the CLI. The builder measures and raises the
  budget once, with the measured delta in the commit message.

### Tests

- Unit tests in `thinfloe-share` for the folder logic against a fake
  transport (a trait between folders and AirDrop): name sanitising and
  suffixes, the cap, the 3 s batch, delete on success, keep on decline, no
  re-offer without a change, the 60 s folder removal.
- Owly's AirDrop tests run sender and receiver in a user network namespace
  (`unshare -rn`, veth, no radio). The builder adds a test of that kind
  in `make test-unit` that runs `thinfloe-share`'s receiver against Owly's
  sender and the reverse, if the harness is reachable from outside Owly's
  workspace; otherwise the fake transport covers it.
- `test-session-packages.sh` (or a new integration test): `owlyd`, its
  unit and D-Bus files, `thinfloe-share`, the `.link` file, the T3U Plus
  driver and firmware in the image; no NetworkManager, no polkit pulled in.
- Menu: `[share]` in `config.rs`, `generated/share` rendering, the
  supervisor, the Settings band (`ui_code_has_no_style_literals` holds).
- OSD: the note datagram and panel.
- Hardware (owner): T3U Plus and an iPhone, once with the machine's
  Bluetooth and once without (receive-only); receive a photo, a video, a
  folder; send one file and three; decline on the phone; pull the stick
  while the radio is up; log out with the radio up and check the built-in
  card works afterwards.

### Docs

- `docs/notes/airdrop.md`: processes, card choice, folders, contracts,
  hardware results.
- `CLAUDE.md`: a pointer under "Where code lives" and the contract rules.
- `CHANGES.md`: one entry for users (what they can do, the "Everyone for
  10 Minutes" condition, the stick).

## Left to the builder

- Shared supervisor module or a copy of the Deskflow one.
- How the root helper reaches BlueZ (D-Bus `LEAdvertisingManager1` as Owly
  does, or the management socket).
- Exact sanitising rules beyond the ones above; the OSD note's layout
  within the panel style.
- Whether the timeout for an offer comes from Owly's driver or is set here.
