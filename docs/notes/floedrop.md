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
- `sudo thinfloe-share --radio` (root helper, started by the daemon) holds the radio from `owlyd`, picks the card and country, writes `/etc/owly/radio.toml` (station and country only), and starts the BlueZ advertisement for the Bluetooth wake-up. It reports on stdout `up<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver` or `error<TAB>text`; the daemon also reads an `up` line without the last two fields. The channel is the link's, read from `iw dev` in the station's phy section once `Acquire` returned (owlyd's `Link` property has none, and the probe's channel is 6 before the country applies); 0 when it cannot be read, and then the texts leave ", channel N" out. The flags may hold `no-ble` and `no-country`. The menu log line names the card: `share | ready on wlxc03a55f0d060 (rtw88_8822bu, exclusive, channel 44) as "FloeDrop Tobi"`, and so does the band's "Ready on …" line. While the radio is up the helper follows `journalctl -u owlyd.service` for owlyd's STATS lines (the wedge watch, below).
- `owlyd` (Owly 0.4.3, commit 21171d8, pinned in Cargo.toml and the Dockerfile) owns the Wi-Fi card in AWDL mode. It is started through D-Bus. No polkit: `owlyd` accepts root.

## Card choice and country

- The helper takes a Wi-Fi card only when the machine does not use it: no IP address on it (an IPv6 link-local address does not count as "in use"; an IPv4 link-local address, 169.254/16, still does), no default route through it, and not named in the netplan Wi-Fi file (matched by name or MAC). Without such a card the band shows "AirDrop needs a Wi-Fi card this machine does not use for its network."
- If the network state cannot be read (for example `ip -j` output does not parse), no card is taken.
- The country comes from the time zone (`zone1970.tab`); else from the keyboard layout in tfuser's `generated/environment` (`XKB_DEFAULT_LAYOUT`): the first layout of the list, when it is two letters, not `us` or `eu` and, upper-cased, a code of `zone1970.tab` (`ch` gives CH; `latam` none). The helper logs "country CH from the keyboard layout ch" then. With neither, radio.toml has no country (regdom 00), the up line carries `no-country` and the band warns "No country set: set the time zone in the Time band below for faster FloeDrop." With country CH the radio ran on channel 44 (5 GHz); under UTC on channel 6.
- Whether a card can run AWDL is decided by Owly's own probe (`allow_untested` stays at its default, off) and by ThinFloe's skip list `AWDL_BROKEN` in `card.rs`: cards the probe accepts but that fail in AWDL mode, by PCI vendor and device from `/sys/class/net/<if>/device/{vendor,device}`. It holds the Intel Wireless 3160 (8086:08b3, 8086:08b4) and the Intel Wireless 7260 (8086:08b1, 8086:08b2); the 7260 is skipped by family (same firmware as the 3160), not tested on hardware. When the only unused card is on the list, the band shows "The Wi-Fi card wlp2s0 (Intel Wireless 3160) cannot run AirDrop; plug in a USB Wi-Fi stick that can.", or, when a USB stick carries the network, "… cannot run AirDrop; the USB stick wlx… is used for the network." Other Intel cards stay usable. See the hardware results below.
- The usable cards are tried in order of preference, a USB card first (`device/subsystem` is `usb`), then the sorted interface name: each is written to radio.toml and probed, and the first the probe accepts (empty `missing` list) is taken; a refusal is logged as "Owly refuses <card>: <reason>". When the probe refuses every card, the error is Owly's reason for the first, and no `Acquire` is made.

## Contracts

- State file `$XDG_RUNTIME_DIR/thinfloe-share.state`, atomic, tab separated. First line `starting`, `error<TAB>text` or `ready<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver`; `iface` is the AWDL link (`awdl0`), `station` the Wi-Fi card under it and `driver` that card's driver (both empty when unknown; a line from an older daemon has neither). Then one line per receiver in range: `name<TAB>folder<TAB>state<TAB>since_epoch` with state `idle`, `sending`, `declined` or `failed`; after them one line per transfer to this machine under way, `sender<TAB>folder<TAB>receiving<TAB>since_epoch<TAB>files<TAB>bytes<TAB>first` (folder the sender's inbox folder, files without folders, first the first file's name), from the accept to its end (received, failed or withdrawn; Owly sends no event when the phone goes before its upload, so an entry with no event 75 s after the accept ends with "the offer from … ended without an upload" unless something in `.incoming/` changed in the last 60 s, then it is looked at again every 15 s; a name change ends the old receiver's entries). Sender and file name are shown without control and format characters and cut at 128 characters; the inbox folder comes from the name the phone gave. It is a line of its own, not a state of an outbox line: the sending phone need not be in the outbox list. Readers ignore unknown trailing fields. The menu parses it on its own and shows "Receiving IMG_0042.DNG from Jane's iPhone" or "Receiving 3 files from …".
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
- Wedge watch: owlyd logs a STATS line every 10 s. An interval is stuck when at least 20 frames were given up and at least half as many as were sent; three stuck intervals in a row (30 s) are a wedge. A counter that goes back (owlyd restarted) starts the count again. An `Acquire` that fails with EBUSY ("Device or resource busy", os error 16) counts too. Then the helper releases the lease and the state is `error` with "The Wi-Fi stick <card> stopped sending. Unplug it and plug it in again." (a card not on USB: "The Wi-Fi card <card> stopped sending. Restart the machine."). Without journalctl there is no watch, never a false error.
- The mDNS service id is kept in `$XDG_RUNTIME_DIR/thinfloe-share/service-id` (12 hex digits, mode 0600, gone at logout), so FloeDrop off and on is the same receiver for the phones. At stop the daemon drops the mDNS registration (the goodbye, 200 ms to go out) before it closes the helper's stdin, while the link is still up. The menu's off switch sends SIGTERM to the whole group, the helper too: on SIGTERM or SIGINT the helper waits up to 700 ms for its stdin to close before it releases the radio (the menu sends SIGKILL after 1 s).
- Log lines of a receive: `offer from "Jane's iPhone": 2 files, 75000000 bytes`, `accepted the offer from …`, `declined the offer from …: 2 files, 75000000 bytes, 1000 bytes free`, `received 2 files, 75000000 bytes from …`, `receiving … failed: <why>`, `the offer from … was withdrawn`, `links from … declined`.
- The daemon runs only while the menu lives (`PR_SET_PDEATHSIG`, which fires when the parent thread exits: the menu spawns it from a long-lived thread). A menu that dies before the daemon sets it sends nothing, so the daemon then compares its parent with the menu pid in `TF_SHARE_PARENT` and exits 0 when they differ.

## Tests

- `cargo test` in `src/thinfloe-share` (part of `make test-unit`); the veth tests need user namespaces and are skipped without them; with CI set they fail instead.
- veth tests (`tests/veth.rs`): receive and send against Owly's own sender and receiver, the receiving line gone after a delivery, the goodbye before the radio goes (`the_service_is_withdrawn_before_the_radio_goes`) and one service name across a restart (`a_restart_keeps_the_service_name`).
- `test/integration/test-floedrop-image.sh`: Owly files, `iw` and `journalctl` (the helper's channel and wedge watch), the mount unit and its options in the image.
- Menu: `share_state.rs`, `sharecfg.rs` tests; the style test covers the band.
- QEMU has no AWDL card: `make qemu-smoke` checks that boot and menu are unaffected; no test checks the band's text there.

## Hardware results (2026-11-25, thinfloe-56db68)

Stock `owly` 0.4.3 command line, not yet the FloeDrop image.

- Stick: USB ID 2357:012e ("TP-Link 802.11ac NIC", Realtek RTL8812BU, driver `rtw88_8822bu`), not the T3U Plus 2357:0138. `owly doctor`: exclusive mode. With country CH in `radio.toml` the radio ran on channel 44.
- iPhone to thin client: a 216 KB JPEG arrived in under 1 s.
- Thin client to iPhone: a text file arrived after the phone accepted; the Bluetooth wake-up came from the Intel card's `hci0`.
- The kernel logged once `rtw88_8822bu ...: failed to get tx report from firmware`; harmless. The stick once failed at plug-in (error -71) and reconnected by itself.
- The Intel AC 3160 crashes in AWDL mode: not usable.

## Hardware results (2026-11-26, thinfloe-56db68, ThinFloe 4.12.0)

- Cards: internal Intel Wireless 3160 [8086:08b3] (`wlp2s0`, `iwlwifi`) and the TP-Link stick 2357:012e (`wlxc03a55f0d060`, `rtw88_8822bu`). The 4.12.0 helper took `wlp2s0`, the first name in sorted order. Owly's probe accepted it ("exclusive mode up"); then the firmware crashed about every 11 s ("Microcode SW error detected", 40 times in one boot) and the iPhone never showed the receiver.
- With the 3160 unbound from its driver, the helper took the stick and the iPhone showed "FloeDrop Tobi" at once. This led to the skip list and the preference for USB cards.
- The owner reports that newer Intel cards (model not recorded) worked with FloeDrop.

## Hardware results (2026-11-26, thinfloe-2b3967, ThinFloe 4.13.0)

- TP-Link RTL8812BU stick. The stick stopped sending while FloeDrop said "Ready": owlyd's STATS showed "radio full" and "gave up" climbing, iPhones did not see the machine, and later starts failed with "Couldn't bring owlyibss0 up: Device or resource busy (os error 16)". Only a replug helped. This led to the wedge watch; its threshold rests on this one stick.
- Under zone UTC (no zone from menu or DHCP) there was no country: regdom 00, channel 6, a slow start and a slow transfer, and nothing said why.
- With a country set the radio ran on channel 44, but the band and the log said "channel 6": the probe's channel, read before the country applied.
- After FloeDrop off and on the iPhone listed the receiver twice and sent the files to both (" (2)" copies in the inbox).

## Owner checklist (hardware, FloeDrop image)

1. Switch FloeDrop on in Settings → Network; check the first switch-on after a fresh boot (`owlyd` is started through D-Bus; no "owlyd stopped" message).
2. Receive a photo, a video and a folder from an iPhone.
3. Send one file and three files to the iPhone; the phone shows one prompt per batch.
4. Decline on the phone: the files leave the outbox and a warning note appears.
5. Without Bluetooth: receive still works. With no adapter the band shows "Sending needs Bluetooth; this machine has none." and the daemon log has "no Bluetooth: receiving only". With Bluetooth switched off and not turnable on, the helper error is "Bluetooth is off and couldn't be turned on".
6. Pull the stick with the radio up: the band shows an error line and no card stays in AWDL mode (`iw dev`).
7. Log out with the radio up and check `iw dev`.
8. Read a healthy stick's STATS (`journalctl -u owlyd -g STATS`) during a transfer and check that the wedge threshold leaves it alone.
9. FloeDrop off and on: the iPhone shows one receiver.
10. Send a large file from the iPhone: the band shows "Receiving …" until it lands.
11. With zone UTC and keyboard layout ch the band shows channel 44.

## Known limits and next

- Only AirDrop. Quick Share for Android is planned as a second transport (own spec and branch). Phones on the office Wi-Fi in the same network as the wired thin clients are a possible route over the LAN with mDNS.
- Sequential discovery of receivers; the Probe failure stops the radio; a held batch is retried after 1 s, not 3 s and has no time limit.
- An unknown sender is called "iPhone" in the note.
- No byte progress while receiving: Owly reports only start and end.
- A change of the time zone or keyboard layout takes effect at the next FloeDrop start.
- The wedge threshold rests on one wedged stick; a large send to a phone moving out of range is not tested (`radio full` is not counted: it counts retries that may still succeed). The journal follow is tested on recorded lines only.
- The veth goodbye test cannot show the old order failing: the daemon sends the goodbye faster than a shell helper can take the link down.
