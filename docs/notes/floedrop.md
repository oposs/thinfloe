# FloeDrop

FloeDrop sends and receives files with iPhones nearby over AirDrop. The phone sees the thin client as a receiver named `FloeDrop <id>`; the thin client sees each phone as a folder. Files travel through the folder `/media/tfuser/FloeDrop`. Design: `docs/superpowers/specs/2026-11-24-airdrop-share-design.md`.

## Use

- The switch is in Settings → Network (band FloeDrop), off by default. The name phones show is `FloeDrop <host suffix>`, for example `FloeDrop 56db68`; the name can be changed in the band.
- Phones must have AirDrop set to "Everyone for 10 Minutes".
- `/media/tfuser/FloeDrop` holds the inbox (what phones send) and one outbox folder per phone in range. Files put into a phone's folder are offered to that phone.
- Received files are placed in the inbox; an OSD note says how many.
- The outbox is a move, not a copy: after each offer the folder's offered entries are deleted, whatever the outcome (accepted, declined, failed). A drag in the remote session must be a copy to keep the original.

## Processes

- `thinfloe-menu` (Network tab) writes `[share]` to `config.toml` and then `generated/share` (`enabled=`, `name=`), and sends the daemon SIGHUP. It supervises the daemon like the Deskflow daemon.
- `thinfloe-share` (the daemon, runs as tfuser) reads only `generated/share`. It watches the outbox with inotify, receives into the inbox, and writes the state file and OSD notes. On SIGHUP it rereads the name.
- `sudo thinfloe-share --radio` (root helper, started by the daemon) holds the radio from `owlyd`, picks the card and country, writes `/etc/owly/radio.toml` (station and country only), and starts the BlueZ advertisement for the Bluetooth wake-up. It reports on stdout `up<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver` or `error<TAB>text`; the daemon also reads an `up` line without the last two fields. The menu log line names the card: `share | ready on wlxc03a55f0d060 (rtw88_8822bu, exclusive, channel 6) as "FloeDrop Tobi"`, and so does the band's "Ready on …" line.
- `owlyd` (Owly 0.4.3, commit 21171d8, pinned in Cargo.toml and the Dockerfile) owns the Wi-Fi card in AWDL mode. It is started through D-Bus. No polkit: `owlyd` accepts root.

## Card choice and country

- The helper takes a Wi-Fi card only when the machine does not use it: no IP address on it (an IPv6 link-local address does not count as "in use"; an IPv4 link-local address, 169.254/16, still does), no default route through it, and not named in the netplan Wi-Fi file (matched by name or MAC). Without such a card the band shows "AirDrop needs a Wi-Fi card this machine does not use for its network."
- If the network state cannot be read (for example `ip -j` output does not parse), no card is taken.
- The country comes from the time zone (`zone1970.tab`). It must be set: with country CH the radio ran on channel 44 (5 GHz).
- Owly's own probe decides whether a card can run AWDL (`allow_untested` stays at its default, off). See the hardware results below.

## Contracts

- State file `$XDG_RUNTIME_DIR/thinfloe-share.state`, atomic, tab separated. First line `starting`, `error<TAB>text` or `ready<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver`; `iface` is the AWDL link (`awdl0`), `station` the Wi-Fi card under it and `driver` that card's driver (both empty when unknown; a line from an older daemon has neither). Then one line per receiver in range: `name<TAB>folder<TAB>state<TAB>since_epoch` with state `idle`, `sending`, `declined` or `failed`. Readers ignore unknown trailing fields. The menu parses it on its own.
- OSD note: one datagram to the OSD socket, `note<TAB>tone<TAB>text<LF>`, tone `info` or `warn`, shown 4 s. Tabs and newlines inside the text become spaces. Sending never blocks; without an OSD the note is lost.
- Mount: `media-tfuser-FloeDrop.mount` is a tmpfs, options exactly `size=25%,uid=1001,gid=1001,mode=0755,nosuid,nodev,noexec`. Files in it live in RAM. Guard: `test/integration/test-floedrop-image.sh`.
- `generated/share` is `key=value` lines; the daemon never reads `config.toml`.

## Behaviour rulings

- Batch delay: 3 s after the last change in an outbox folder. A file still being written (busy) holds the whole batch: leaving one file out would send the rest now and that file as a second prompt on the phone.
- Outbox folders left over from an earlier run are watched too, and a deleted folder of a device still in range is made again. The folder of a device that is gone is removed after 60 s when empty.
- One folder per sender: an existing folder is reused; a new name is made unique only when something that is not a folder holds the name.
- An outbox folder is named by the computer name the phone gives in its `/Discover` answer, never by its key. A phone found first in the radio's AWDL peer list (key `awdl-<mac>`) and later by mDNS (its instance name) is one device: the outbox matches it by the address it answered at and keeps the one folder and the files in it. A phone whose mDNS address differs from the link-local address of its AWDL MAC would still get a second folder `<name> (2)`; check on hardware.
- Sending spools the batch's archive in `/media/tfuser/FloeDrop/.spool` (the daemon's `TMPDIR`, mode 0700, emptied at start), inside the tmpfs: a send briefly needs the batch's space twice, and a batch that does not fit twice fails like any failed send (entries deleted, warning note). In `/tmp` the archive would be RAM outside the 25 % cap. Uploads from phones are staged in `.incoming/` next to it.
- Declined and failed offers delete the offered entries (owner decision 2026-11-25); the device line shows the state and an OSD warning note appears.
- A failed receive shows a warning note. A receive that placed no file makes no note.
- File names from phones are made safe (`names.rs`): no path parts, no control or format characters (a bidi override such as U+202E would show `invoice<U+202E>fdp.exe` as `invoiceexe.pdf` on the remote desktop), no leading dot, length limit, unique in the inbox. OSD notes drop format characters too, in the daemon and in the OSD.
- Reload: SIGHUP goes to the daemon pid only, never to the process group, because the group holds the root helper through `sudo` and SIGHUP would end it. The daemon starts with SIGHUP blocked and unblocks it after the handler is set, so an early reload from the menu does not kill it. `--radio` ignores SIGHUP.
- The radio helper uses a plain `Acquire(0)`, never `AcquireDetached`: the lease ends when the helper leaves the bus, so a crash gives the card back. At start the helper releases a lease an earlier crash left detached.
- The daemon runs only while the menu lives (`PR_SET_PDEATHSIG`, which fires when the parent thread exits: the menu spawns it from a long-lived thread). A menu that dies before the daemon sets it sends nothing, so the daemon then compares its parent with the menu pid in `TF_SHARE_PARENT` and exits 0 when they differ.

## Tests

- `cargo test` in `src/thinfloe-share` (part of `make test-unit`); the veth tests need user namespaces and are skipped without them; with CI set they fail instead.
- `test/integration/test-floedrop-image.sh`: Owly files, the mount unit and its options in the image.
- Menu: `share_state.rs`, `sharecfg.rs` tests; the style test covers the band.
- QEMU has no AWDL card: `make qemu-smoke` shows the band's error text, and boot and menu stay unaffected.

## Hardware results (2026-11-25, thinfloe-56db68)

Stock `owly` 0.4.3 command line, not yet the FloeDrop image.

- Stick: USB ID 2357:012e ("TP-Link 802.11ac NIC", Realtek RTL8812BU, driver `rtw88_8822bu`), not the T3U Plus 2357:0138. `owly doctor`: exclusive mode. With country CH in `radio.toml` the radio ran on channel 44.
- iPhone to thin client: a 216 KB JPEG arrived in under 1 s.
- Thin client to iPhone: a text file arrived after the phone accepted; the Bluetooth wake-up came from the Intel card's `hci0`.
- The kernel logged once `rtw88_8822bu ...: failed to get tx report from firmware`; harmless. The stick once failed at plug-in (error -71) and reconnected by itself.
- The Intel AC 3160 crashes in AWDL mode: not usable.

## Owner checklist (hardware, FloeDrop image)

1. Switch FloeDrop on in Settings → Network; check the first switch-on after a fresh boot (`owlyd` is started through D-Bus; no "owlyd stopped" message).
2. Receive a photo, a video and a folder from an iPhone.
3. Send one file and three files to the iPhone; the phone shows one prompt per batch.
4. Decline on the phone: the files leave the outbox and a warning note appears.
5. Without Bluetooth: receive still works. With no adapter the band shows "Sending needs Bluetooth; this machine has none." and the daemon log has "no Bluetooth: receiving only". With Bluetooth switched off and not turnable on, the helper error is "Bluetooth is off and couldn't be turned on".
6. Pull the stick with the radio up: the band shows an error line and no card stays in AWDL mode (`iw dev`).
7. Log out with the radio up and check `iw dev`.

## Known limits and next

- Only AirDrop. Quick Share for Android is planned as a second transport (own spec and branch). Phones on the office Wi-Fi in the same network as the wired thin clients are a possible route over the LAN with mDNS.
- Sequential discovery of receivers; the Probe failure stops the radio; a held batch is retried after 1 s, not 3 s and has no time limit.
- An unknown sender is called "iPhone" in the note.
