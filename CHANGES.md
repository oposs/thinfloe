# Changelog

## [Unreleased]

### New

### Changed

- When a newer release is out, the Upgrade tile keeps its name and says so in small blue type below it, for example "4.12.33 available". Before, the title changed to "Upgrade to …", which did not fit the tile.
- A click on the small "update" mark of an app tile opens Settings → Packages, where the update can be applied; before, it started the app. A click anywhere else on the tile still starts the app.
- The Eject tile has an eject symbol instead of the USB symbol it shared with the Upgrade tile.
- The FloeDrop line in Settings → Network and the menu log now name the Wi-Fi card FloeDrop uses and its driver, for example "Ready on wlxc03a55f0d060 (rtw88_8822bu, exclusive, channel 6)", instead of "awdl0", which was the same on every machine.

### Fixed

- On a machine with an internal Intel Wireless 3160 and a USB Wi-Fi stick, FloeDrop used the internal card and iPhones did not show the machine. FloeDrop now prefers a USB Wi-Fi stick and never uses the Intel Wireless 3160, whose firmware crashes in AirDrop mode. If that card is the only free one, Settings → Network says "The Wi-Fi card … (Intel Wireless 3160) cannot run AirDrop; plug in a USB Wi-Fi stick that can."

## 4.12.0 - 2026-11-25

### New

- FloeDrop: send and receive files with iPhones nearby over AirDrop, through the folder `/media/tfuser/FloeDrop` (an inbox and one outbox folder per phone). Turn it on in Settings → Network; phones must be set to AirDrop "Everyone for 10 Minutes". The machine needs a Wi-Fi card that it does not use for its own network and that supports AirDrop mode; it was tested with a TP-Link USB stick with the Realtek RTL8812BU chip.
- Receiving works without Bluetooth; sending to a phone needs Bluetooth on the machine, and Settings → Network says "Sending needs Bluetooth; this machine has none." without it.
- Files put into a phone's outbox folder are removed from it after each offer, also when the phone declines. Drag with copy in the remote session to keep the original.
- USB sticks, SD cards and USB disks are opened by themselves and appear under /media/tfuser, where ThinLinc's and OxuLnk's folder sharing can reach them. A card swapped in a card reader appears in place of the old one. A stick labelled FloeDrop, in any case, appears with a number added (FLOEDROP1), because /media/tfuser/FloeDrop is FloeDrop's folder.
- A new Eject tile appears while a stick or card is plugged in and tells when it is safe to pull it; with several plugged in, it lists them, each with its own Eject button, and offers Eject all. An ejected USB stick is switched off; a card reader stays on, ready for the next card.
- Restart and Power off write pending data to the sticks first and show "Writing to …" while they do. A stick that is in use is reported like a failed home save, with the choice to wait or go on. After a minute the menu says "Releasing the sticks takes too long"; a release that still ends after that goes on with the restart or power off.
- A note appears at the bottom of the screen for four seconds when a stick or card is plugged in ("MYSTICK connected"), when it has been ejected and can be pulled ("MYSTICK can be removed"), and when it was pulled without eject ("MYSTICK removed without eject — files may be lost"), the last one in the warning colour.

### Changed

- Writing ThinFloe onto a stick that is open under /media/tfuser now closes it first instead of refusing it as mounted. A stick that is in use is refused with "the stick is in use", and a stick that was not written after all is opened again, shown as "Mounting the stick again" in Upgrade.
- Before an erase of a stick that is open under /media/tfuser, the confirmation in Upgrade lists the files on it ("These files will be erased:", up to 20 names, then "and N more"). If the stick cannot be read in time, the list is left out and Erase works as before.

### Fixed

- Settings → Sound showed notes about other keyboard settings under its switch, such as "Num Lock follows at the next start." It now shows only a note about turning the volume switch on or off.
- After the session ended, the file `thinfloe-volume` could stay behind in the user's runtime directory when the volume was being changed at that moment. It is now always removed when the session ends.
- On Settings → Keyboard and Settings → Sound, an error from a failed change stayed on screen after the setting was switched back to its saved value. It now goes away.
- With only a network change being applied, Log out, Restart and Power off asked with "Add-ons are being changed" and "The add-on work is finished before you restart." They now say "Network settings are being applied" and "The network settings are applied before you restart.", and the waiting dialog names the network settings.

## 4.11.0 - 2026-11-25

### New

- Settings has a new Sound tab with a volume slider and a mute switch, for keyboards without volume keys. Its switch "Set the volume on this machine" is the same setting as "Volume keys control this machine" on the Keyboard tab; while it is off, the slider and mute are greyed out and the remote session sets the loudness.

### Fixed

- With "Volume keys control this machine" on, the speaker volume set with the volume keys could come back at 100 % after a restart. The volume and mute from before the restart now stay.
- While the mouse was on an attached Deskflow computer, ThinFloe's mouse pointer stayed visible at the edge of the screen. It is now hidden until the mouse comes back to ThinFloe.

## 4.10.0 - 2026-11-24
### New
- Settings → Screen shows the monitors as a list (Monitor, Resolution, On) and the computers attached with Deskflow as a list (name, On, Clipboard, Compose). The switches are always visible; ⚙ or a click on the monitor or computer opens a dialog with the rest, such as Position, Rotation and Forget.
- Each attached computer has an "On" switch, on by default. With it off, the mouse stops at that edge, the computer stays connected and keeps its place, and no clipboard is shared with it. Its box in the monitor arrangement is dimmed, and switching it off while the pointer is on it brings the pointer back to ThinFloe at once.

### Changed
- In Settings → Screen, the Deskflow fingerprint of ThinFloe is now behind "Fingerprint…" and a new computer is attached through "Check and attach…", a dialog with the fingerprint and the free edges.

### Fixed
- With "Handle Compose key on ThinFloe" on for a computer, the Compose key did nothing at all when ThinFloe had no Compose table for its language: the key reached neither ThinFloe nor that computer. It now goes to the computer, as with the switch off.
- With "Handle Compose key on ThinFloe" on, Caps Lock, Num Lock and Scroll Lock pressed during a Compose key sequence did not reach that computer, so its Caps Lock could end up the other way round from ThinFloe's. These keys now always reach the computer, and the sequence still gives its character.
- With "Handle Compose key on ThinFloe" on and no Compose table for ThinFloe's language, pressing a Compose key that does not repeat while another key was held stopped that held key repeating on the computer. The held key now keeps repeating, as with the switch off.

## 4.9.0 - 2026-11-23
### New
- Each attached computer in Settings → Screen has a "Handle Compose key on ThinFloe" switch, off by default. With it on, Compose key sequences are combined on ThinFloe and the finished character is sent to that computer, while dead keys still go as keys of their own.

### Changed
- Returning the pointer from a computer attached with Deskflow now places ThinFloe's cursor at the matching point along that monitor edge. Before, ThinFloe's cursor stayed where it had crossed onto the computer.

## 4.8.1 - 2026-11-22
### Changed
- A computer whose Deskflow connection fails before it shows in Settings → Screen, for example over its certificate, now leaves a line in the log at the bottom of the menu: `deskflow | a connection from 192.168.1.20 ended before pairing: …` with the reason. The same failure from the same computer is logged once every 5 minutes. All other messages of the Deskflow server appear in that log too, as `deskflow`.

### Fixed
- A Mac, Windows or Linux computer whose Deskflow certificate was made by the Deskflow app could not connect: the client logged `tls error occurred (generic failure)` and `ssl/tls alert certificate unknown`, and Settings → Screen never showed it. Such a computer now shows up there for its fingerprint to be compared.

## 4.8.0 - 2026-11-22
### New
- Settings → Screen has a switch "Let other computers use this keyboard and mouse (Deskflow)", off by default. Once it is on, computers that run the Deskflow client show up there, and after you compare the fingerprint you attach one beside a monitor. Pushing the pointer past that edge then moves keyboard and mouse to the computer, and Ctrl+Alt+Shift+Escape always brings them back.
- Text copied on ThinFloe can be pasted on a computer attached with Deskflow, and text copied there can be pasted on ThinFloe; images, files and texts over 1 MiB are not carried, and copying one on ThinFloe empties the clipboard of the attached computers. Each attached computer has a "Share the clipboard" switch in Settings → Screen, on by default.
- While the pointer is on a computer attached with Deskflow, the volume and brightness keys act on that computer and ThinFloe shows no volume or brightness display. The microphone mute key still mutes ThinFloe, when "Volume keys control this machine" is on.

## 4.7.0 - 2026-11-20
### New
- The clock showed UTC on a network that sends no time zone. Settings → Network has a new Time band: pick the region and city of the time zone, which takes effect at once and is kept across restarts; Automatic keeps the zone DHCP sends, else UTC. A saved zone this system does not know shows as Automatic with a line naming it, and the NTP servers still take effect.
- The Time band has an "NTP servers" field: servers typed there are the only ones the machine asks for the time; left empty, it uses the servers DHCP sends and the Ubuntu pool. Below it the band says whether the clock is synchronised, with which server, and lists the servers in use.

### Changed
- The Connection tile in the status area shows each label on the left and its value on the right, and has four new lines: `Gateway`, `DNS` (the server in use, with `+1` and so on for more), `Traffic` (bytes received and sent, for example `↓ 1.23 GB ↑ 456 MB`) and `Uptime` (how long the machine has run). The `VPN` line shows only while the VPN is up; `wg0 down` and `no VPN` are no longer shown. A missing DNS server, or a missing default route, shows `none` in amber, and a connection without a gateway (a mobile modem or PPP link) shows `direct` in grey.
- The facts on the Network and Screen tabs now carry a label each ("Driver: iwlwifi" instead of a bare "iwlwifi"), and both tabs show them in the last band: "Connections now" moved from the top of the Network tab to its end. The video formats are listed under "Video processing", one per line ("H.264: decode, encode").
- The This machine tab also shows the network and graphics facts of the Network and Screen tabs, in two columns on a wide screen, and Copy puts them on the clipboard as well. While the machine's facts are still being read, only the This machine panel says "Reading…" and the other panels show already. Copy shows greyed out until the facts arrive; before, it was hidden.

### Fixed
- The Screen tab said "No graphics device found" on every machine, also when the picture worked. It now names the graphics chip and its driver. The Network tab now also shows the MAC address of each connection and the WiFi chip, which were missing for the same reason, and names the chip and driver of a wired adapter too.
- The microphone mute key of Logitech keyboards on a Logi Bolt receiver, such as the MX Mechanical, did nothing. It now mutes the microphones when "Volume keys control this machine" is on, and otherwise reaches the program in front, ThinLinc included.
- On the switches in Settings, such as those of the Keyboard tab and "Show the public IP address", only the colour seemed to change when a switch was turned on, and the knob seemed to stay at the left. The knob is now light in both positions, with a margin of track colour around it, and shows at the right when the switch is on.
- On a network without internet access the clock was never synchronised, although the DHCP server named an NTP server: those servers were ignored. They are now used, and the journal shows `thinfloe: NTP servers … from DHCP`.
- In a fresh clone of the repository, `make` stopped at once with `mkdir: cannot create directory 'artifacts': File exists`, because `artifacts` was a link to a directory on the developer's machine. `artifacts` is now an ordinary directory that the build makes; a link to share it between checkouts can still be made by hand, and a link whose target is missing now stops `make` with a message that names the target.
- `make clean && make`, as the README shows it, stopped after `make clean` with exit code 1 when `artifacts` is a link. `make clean` now exits 0: it removes the container stamps, leaves the link target alone unless `FORCE=1` is given, and the next `make` builds everything again.
- A build could ship a damaged tile menu program without notice; the build now compares the installed program with the one it compiled and stops when they differ.

## 4.6.0 - 2026-11-19
### New
- Settings → Keyboard has two new switches, "Volume keys control this machine" and "Brightness keys control the monitors", both off by default: with them on, the volume keys (up, down, mute, microphone mute) change the volume of every speaker and the mute of every microphone of this machine, and the brightness keys change the brightness of every monitor that supports DDC/CI, also inside a fullscreen ThinLinc session. A panel at the bottom of each monitor shows the new level, and a microphone sign shows at the top right of every monitor whenever the microphone is muted, also with the switch off. Volume and mute set with the keys are kept across restarts, a speaker or microphone plugged in later takes the same setting, and with the switch off every speaker and microphone is kept at full volume and unmuted.
- Settings has a new tab, This machine: maker, model, CPU model and cores, RAM, ThinFloe version, kernel, boot from USB or network, host name, uptime, and each installed add-on with its version. Copy puts all of it on the clipboard as `Label: value` lines, ready to paste into a support request.
- The Network tab shows the MAC address of each connection, the gateway of the one that carries the traffic, and for WiFi the chip and its driver. A new line "DNS in use" names the DNS servers the machine asks now, also when they come from DHCP.
- The Screen tab has a Graphics band: the graphics chip, its driver, and the video formats (H.264, HEVC, VP9, AV1) it decodes or encodes in hardware, or "VA-API not available". When VA-API is not available, the menu log says why under `graphics`.
- The Connection tile can show the machine's public IP address: switch on "Show the public IP address" on the Network tab (off by default). The address comes from `https://api.ipify.org`, or from an `https://` address typed under "Lookup address", and is asked only while the tile is on screen: at first, when the network changes and every 15 minutes. A failed lookup shows `Public –` and one line in the menu log.

### Changed
- The "Network details" tile is now the "Connection" tile, with the link, the IPv4 address, the VPN and, when switched on, the public address. Down, Up, Peak, Total and Since are gone: the traffic graph shows the rates, and the gateway and DNS servers are on the Network tab. A saved status layout keeps the tile at its place and size.
- The "Fans, uptime, model" tile is gone, and the fan speeds are now a line in the "Temperatures and fans" tile; uptime, maker and model are on the This machine tab. A saved status layout drops the old tile without a note; without a saved layout, the clock now sits under the batteries and the bottom row holds temperatures, CPU and memory.
- The clock tile has a third line with the time zone, for example `Zone Europe/Zurich`, or `CET` for a zone set without a name.
- On a low screen (the status area has one row), the traffic graph is two cells wide instead of three.

### Fixed
- Alt-Tab switched between the windows on the ThinFloe desktop, the tile menu included, instead of reaching the ThinLinc session, even when that session was fullscreen; Alt-F4, Alt-Space, Super-Return, Super-a and Super with the arrow keys also acted on the desktop. ThinFloe now has no key shortcuts at all, so these keys reach the program that has the focus. The volume and brightness keys therefore go to the focused program too, unless they are given back to the machine in Settings → Keyboard.
- The time zone from the DHCP server could stay at UTC after the start when the server's answer came in while an earlier one was still being read; it now always takes effect. A renewal that brings the same zone no longer logs `thinfloe: time zone <zone> from DHCP` again.
- The Upgrade screen offered to upgrade a stick to the release it already had ("Upgrade this machine's stick to ThinFloe 4.5.0" on a 4.5.0 stick), and What's new then said "No changes between 4.5.0 and 4.5.0". It now says the stick already has that release and the button reads Write again, or Downgrade for a stick with a newer release; when the stick keeps an older release as fallback, it says that this one is dropped. What's new is off for such a stick, and also for a test build of the release it would get.
- A tile of your own whose `Icon=` names an image by a path longer than 120 characters showed the first letter of its name instead of the image. Paths up to 4096 bytes now work; a longer `Icon=` still shows the letter.
- `ls -l /var/log` showed `README` as a link to a file that does not exist, and `/etc/vconsole.conf` and the `LC_TIME/coreutils.mo` files under `/usr/share/locale` pointed at nothing too. The image now has no such links, and a build that would ship one stops.

## 4.5.0 - 2026-11-17
### New
- The Upgrade screen has a "What's new" button beside Check now. It shows the release notes of every version after the one on the chosen stick up to the release that will be written, or for a fresh install the notes of that release alone; with an address entered under Advanced there is no button. Sticks written or upgraded from now on keep their version in `tl/version`, and the drive list names it beside each ThinFloe stick; for an older stick the notes of all releases up to the new one are shown.
- Settings -> Keyboard has a switch "Num Lock on at start". Turn it off for a small keyboard with a number pad over the letter keys, where Num Lock on types digits instead of letters. It takes effect at the next start; until now Num Lock was always on.

### Changed
- The Upgrade screen now opens with this machine's stick already chosen and Upgrade selected, so the confirm button is ready without a click on the drive. A click on another drive still picks that one, and after Write another stick nothing is chosen.

### Fixed
- `sudo thinfloe-password --clear` now also removes the old password hash that the 4.4.0 move of the settings left in `~/cfg-attic/.thinfloe-password`. Before, that copy stayed in the saved home on the stick, or on the boot server, after the password was cleared.
- `sudo thinfloe-password --clear` stopped with "rm: cannot remove ... Is a directory" and left the password in force when a folder had the name of one of the password files. It now locks the account in every case and names what it could not remove.

## 4.4.0 - 2026-11-14
### New
- The Settings screen has a Keyboard tab: pick the layout and its variant from the full list, CapsLock as Ctrl on or off, and the Compose key (Right Alt by default). A change applies at once to the running session, with a line to try it right there, and is kept with the home directory.

### Changed
- All settings the menu manages are now kept in `~/.config/thinfloe/config.toml`, and the first start after the upgrade moves the old files there by itself, WiFi included. The old files stay unchanged in `~/cfg-attic/`, with a README listing what was taken over, and the Settings screen says so once.
- `~/.thinfloe-environment` and `~/.thinfloe-sessionrc` are no longer read: extra variables go in `~/.config/thinfloe/environment` and session commands in `~/.config/thinfloe/sessionrc`. `TF_IDLE_TIMEOUT` is no longer read from either; the screen timeout is set on the Screen tab.
- `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys`, `~/wg0.conf` and `~/.save-home-exclude` moved to `password`, `ssh-host-keys`, `wg0.conf` and `save-home-exclude` in `~/.config/thinfloe/`. A new stick written from this machine still leaves out the host keys and `wg0.conf`, also their copies in `~/cfg-attic/`.
- The previous version, started from the boot menu after the upgrade (`current` at the boot prompt), no longer finds the WiFi, wired and WireGuard settings, the remote-access password or the SSH host keys: it comes up without WiFi, with DHCP on the wired link, without WireGuard and remote password, and with new host keys. The settings are in `~/.config/thinfloe/`, the old files in `~/cfg-attic/`, and changes made while the previous version runs are not carried back.
- When the old keyboard settings named two layouts (`XKB_DEFAULT_LAYOUT=us,de`), only the first is kept; a keyboard model other than `pc105` and XKB options other than CapsLock as Ctrl and the Compose key are dropped. `~/cfg-attic/README` names what was dropped.
- `XKB_DEFAULT_*` lines in `~/.config/thinfloe/environment` are ignored, and the Settings screen names each one.
- A WiFi, wired or monitor file the menu could not fully read, or a WiFi or wired file for another netplan version or renderer, is kept as it was and shown read-only, with "Let the menu manage it" to replace it.
- When the move of the settings fails, the Settings screen says they stay as they were until the next start, and its Network, Keyboard, Screen and Status tabs take no changes. The move is tried again at every login.
- A `config.toml` the menu cannot read is kept as `config.toml.broken-<time>` and the settings are rebuilt from the files in `~/.config/thinfloe/generated/`, or reset to the defaults when that folder is empty, as the Settings screen says. When that fails too, changes are refused with "The settings are read-only."

### Fixed
- A keyboard layout and variant XKB cannot combine (for example `fr` with `de_nodeadkeys`) no longer leaves tty1 at "graphical session exited with status 139" after login. The session starts with the layout's default variant instead, or with `us` for a layout the image does not list, and the session log names the replaced value.
- The menu repairs such a value in `[keyboard]` of `config.toml` the same way when it starts and names it in its log; the move of the 4.x settings repairs it too and lists it in `~/cfg-attic/README`.

## 4.3.0 - 2026-11-13
### New
- ThinFloe now shows its picture on thin clients and mini PCs with AMD processors and their built-in Radeon graphics (those with Carrizo or Stoney graphics, for example the HP t530 and t630 thin clients, and Ryzen 2000 series and newer, up to Ryzen AI): the image carries the graphics firmware for these processors. AMD graphics cards still get no firmware.

### Changed
- The daily release check, the Upgrade tile and `thinfloe-upgrade` take releases from `https://github.com/oposs/thinfloe/releases` instead of tobi.oetiker.ch. Every file of an upgrade or a fresh install comes from one release, the newest when the write starts, and is checked against that release's `SHA256SUMS`; a release without a line for a file fails with "the release has no checksum for <file>" before the stick changes, and a write this machine refuses anyway (a swapped stick, an internal disk, a mounted stick) says so before the release is read. A source entered by hand is used as given and still accepts the older `<file>.sha256` checksum files.
- The menu header of a self-built image shows a version such as `4.2.0-dev+85034f2` (with `.dirty` when built from changed or new files), and such an image is offered the current release as an upgrade. A build outside a git checkout stops at once with "TF_VERSION ... is neither RELEASE (...) nor a dev version" unless `TF_VERSION=X.Y.Z` is given, and a `RELEASE` file that is not one line `X.Y.Z` stops it with "RELEASE must be one line X.Y.Z".
- A new stick no longer comes with ThinLinc settings for the owner's server: ThinLinc starts with its own defaults. Upgraded sticks keep the settings they have.
- An upgrade or a netboot now downloads about 493 MB instead of 611 MB. The running system keeps Intel WiFi firmware only in the version its driver loads, and has no drivers for DVB, TV tuner and radio cards, none for older AMD (radeon) graphics, and no CPU microcode files, which are applied from `initrd.img` at boot as before. During boot, before the system is loaded, drivers for data-centre network cards, Fibre Channel and SAS storage controllers, InfiniBand and network file systems (CIFS, Ceph, AFS, NFS) are no longer available.
- `locale-gen` and `systemd-hwdb update` no longer work on a running machine: the locale and hardware database sources are left out. The locales of `/etc/locale.gen` and the hardware database ship ready to use.
- A ~/wifi.yaml or ~/ethernet.yaml for another netplan version than 2, or with another renderer than `networkd` (for the file or for one connection in it), counts as written by hand even when the menu wrote it, and "Let the menu manage it" is not offered for it. Nor is it offered for a ~/wifi.yaml with more than three DNS servers, the limit the wired connection already had; when the menu wrote such a file, a change of the DNS servers says "~/wifi.yaml has settings the menu cannot keep, so the DNS servers cannot be set for WiFi."
- In the Settings Screen tab, Apply is offered only once something has changed.
- In the Settings Status tab, a click on a part's name ticks it, and a note such as "no room" that does not fit its row is cut short instead of running into the next column.

### Fixed
- After the monitor woke from power save, the keyboard of a fullscreen ThinLinc session could stop working: the keys went to the hidden menu behind it, where Tab and Enter could start Restart. A fullscreen window on top now keeps the keyboard when the menu opens its window again; beside a window that is not fullscreen, the menu still takes the keyboard as before.
- OxuLnk no longer shows its keyboard grab as "not working" and no longer logs "keyboard grab unavailable reason=compositor does not support shortcut inhibition": the session now lets a window switch the desktop's key shortcuts off.
- The image no longer contains the build and test scripts in `/scripts`.
- Programs that looked up a host name early in the boot, before `un-dockerize.service` ran, found no name server: the image shipped no `/etc/resolv.conf`. It now ships the link to systemd-resolved, and `un-dockerize.service` no longer stops systemd-resolved and makes the link again, which made lookups fail for a moment.
- A fresh install from the Upgrade screen or over SSH that is cancelled or fails before its first byte is written leaves the stick as it was instead of clearing its start, and the menu says "Cancelled before anything was written. The stick is as it was." Restart, Power off and Log out warn that the stick will not start, and "Stop writing?" says so, only once writing has begun; during an upgrade "Stop writing?" says "The stick still starts its current release."

## 4.2.0 - 2026-11-11

### New

- The Upgrade tile opens an Upgrade screen in the menu instead of a terminal: choose the release, the USB stick and whether to upgrade or erase it, confirm once, then follow each step with a bar, the write speed and the time left, and cancel while it downloads or writes. Only USB sticks are listed unless you tick "Show all drives", and erasing another drive needs its name and size typed in. The tile reads "Upgrade to X.Y.Z" when a newer release is known; the check runs five minutes after boot and once a day, and the menu header now shows the installed version.
- Add-ons listed in `~/.config/thinfloe/addons` are installed at every boot from their copy on the boot stick, with no network, and are checked for new versions one minute after boot and once a day at a random time within an hour; a new version becomes active at the next boot. On netboot, that check fetches and installs them once the network is up. The first boot after an upgrade from 3.x lists ThinLinc and OxuLnk when the home holds their settings (`~/.thinlinc`, `~/.config/oxulnk`), or both when the stick had no saved home.
- An add-on copy on the boot stick always leaves room for a second copy of the saved home directory, and an update cut off by a power failure while it was being made current is finished at the next boot. A ticked add-on that the boot did not install, because its time ran out or the install failed, is installed from the boot stick by the check one minute after boot.
- The menu shows the tile of an add-on only while the add-on is installed, marks it "update" when a new version waits for the next boot, and enables it as soon as an install finishes, without a restart. Everything the add-on tool does (checks, downloads, installs, failures) appears in the menu log under `add-ons`.
- A new Settings tile opens the Settings screen, whose Packages tab lists the add-ons: ticking an add-on installs it at once, unticking removes it, and each row shows its source, its installed version and any update ready for the next boot, with Install now and Update now buttons. Check now looks for updates of every ticked add-on whose source is HTTPS or signed; any other source shows "updates only by hand" and changes only through its buttons. A tile whose file name cannot be an add-on name (over 64 characters, not starting with a letter or digit, ending in a dot, or another add-on's name apart from case) shows "file name not usable" there, with its tick box off and no button.
- The Settings screen has three tabs: Packages (the add-on list), Network and Screen. On Screen, "Turn the screen off after" sets when the screen blanks: Never, or 1, 2, 5, 10, 15, 30 or 60 minutes, 5 by default; a change applies at once and is saved with the home directory.
- The screen timeout can also be set as `idle_timeout=<seconds>` in `~/.config/thinfloe/screen`: the Screen tab then shows it read-only with "This is set by hand", and "Let the menu manage it" keeps the old file as `~/.config/thinfloe/screen.by-hand`. `TF_IDLE_TIMEOUT` in `~/.thinfloe-sessionrc` still wins over both, and the tab then shows its value with "set in ~/.thinfloe-sessionrc".
- The Settings Screen tab arranges the monitors: each connected monitor is drawn as a box with its name and model, and the one you click gets On, Resolution, Rotation and Position (left of, right of, above or below another monitor). After Apply, "Keep this arrangement?" counts down 15 seconds and goes back to the previous arrangement unless you choose Keep; if the session ends first, the next login goes back. The arrangement is kept in `~/.config/kanshi/config`, one for each set of connected monitors, and while ThinLinc runs the tab says "ThinLinc uses the new arrangement from its next start."
- "Show the menu here" chooses the monitor the menu sits on: the menu moves there with Apply, and the choice is kept with the arrangement. The monitor that shows the menu cannot be switched off until the menu sits on another one, also when no monitor was chosen. A `~/.config/kanshi/config` written by hand is shown with "The monitor arrangement is set by hand" and changes only after "Let the menu manage it", which keeps the old file as `~/.config/kanshi/config.by-hand`.
- The Settings screen has a Network tab. It shows each connection with its address (for WiFi the network name and signal, for WireGuard the last handshake), sets the wired address to automatic (DHCP) or fixed, and saves or forgets WiFi networks found by a scan (each with a signal icon, its strength in words and in dBm, and a lock when it needs a password) or typed by name; Apply, right below the settings, puts the change in force at once and at every boot, and asks first while ThinLinc or OxuLnk runs. A network in the scan list that the menu cannot join (WEP, or a sign-in such as eduroam) says "This security type is not supported".
- A fixed wired address is kept in ~/ethernet.yaml, which the machine installs at boot like ~/wifi.yaml; a netbooted machine ignores it and always uses DHCP. A ~/wifi.yaml or ~/ethernet.yaml written by hand is shown but not changed until you choose "Let the menu manage it", which is offered when the menu can keep every setting in the file. A WiFi network name with a backslash (`\`) cannot be saved: the Network tab says "This name cannot be used."
- The Network tab has a "DNS" section with a "DNS servers" row: servers typed there replace the ones the DHCP server sends, with Automatic (DHCP) as with a fixed address, while the address and gateway still come from DHCP; left empty, the servers DHCP sends are used. The servers apply to WiFi as well, except when ~/wifi.yaml or ~/ethernet.yaml is written by hand, and Apply is offered when WiFi has other servers than the wired connection. A netbooted machine always uses the DNS servers DHCP sends and keeps those of ~/wifi.yaml.
- When a ticked add-on could not be installed, the menu asks once per boot "<Name> could not be installed: <reason>" with Retry and Later.
- Log out, Restart and Power off wait for an add-on install, update, removal or check started from Settings: the dialog names it ("Installing ThinLinc"), and Stay stays available while it finishes. Restart and Power off also offer "Restart anyway" and "Power off anyway", which stop the add-on work and go on after at most 5 seconds. Running programs are stopped as before.
- Restart, Power off and Log out wait while a stick is written from the Upgrade screen and list it ("Writing SanDisk Cruzer"); Restart anyway and Power off anyway stop the write, and after a cancelled fresh install the stick does not start until it is written again.
- The clock in the menu and the time stamps in its log show the time zone set with `TZ=` in `~/.thinfloe-environment`, else the one the DHCP server sends (option 101), else UTC; a zone that arrives after the menu started shows within a minute. The system log says `thinfloe: time zone <zone> from DHCP`, or `thinfloe: ignored DHCP time zone '<value>': <reason>` for a value it refuses. WiFi profiles from `~/wifi.yaml` do not ask the DHCP server for a zone.
- Over SSH, `thinfloe-upgrade` now lists USB and removable drives and the boot stick, with "a) show all drives" for the rest, marks any other mounted drive "(mounted - unmount it to write it)" without offering to write it, and shows how many MiB are written so far. A fresh install that fails, is interrupted (also by a dropped SSH session) or whose download does not match its published checksum clears the start of the stick, so the stick does not try to boot half-written; an upgrade whose download does not match fails before the stick is changed. After the last question the stick is checked again and refused before anything is downloaded when it was swapped or repartitioned meanwhile, is mounted (except the boot stick for an upgrade) or, for a fresh install, is in use elsewhere; only one `thinfloe-upgrade` writes at a time, and one left waiting at a question does not hold up another.
- The menu has a status area below the tiles, showing the time and date and what this machine can report. Tiles and status area are centred together, the tiles do not move when a part of the area has nothing to show, and the other parts move into its place. It takes no click, and a fullscreen window hides it.
- The status area shows the battery of keyboards and mice, Logi Bolt and Bluetooth devices included, one line per device such as `Keyboard 95 %`, red below 10 %. A device is read at boot and every 5 minutes, and a Logi Bolt device also when it connects; one that does not answer shows `–` in a muted colour.
- The status area shows a graph of received and sent traffic over the last 5 minutes and, beside it, the rates, link, address, gateway, DNS servers and VPN state; after a fullscreen window closes, the graph starts again empty. On a low screen such as 1280 x 720 only the battery and the graph show, in one row.
- The status area shows temperatures, CPU load and frequency, memory, fans, uptime and the machine's model in four parts; a hot temperature changes colour, sensors that do not fit leave out the least important, never the CPU, and fans that do not fit beside their label are left out. They are left out on a low screen such as 1280 x 720.
- Settings has a Status tab: tick, size and order the parts of the status area, with a preview of the area as the main screen shows it. Each change is saved at once to `~/.config/thinfloe/status` and travels with the saved home; a ticked part that gets no place says "no room", "not on a small screen" or "nothing to show on this machine", and "Reset to default" removes the file. A part that a newer release adds shows at the end of the area until it is unticked.

### Changed

- The menu's tiles keep their shape and look the same size on every screen. A 4K screen that reports its size no longer gets stretched tiles (one that reports none or an implausible size still gets double size), and a small, dense laptop screen shrinks the menu to fit.
- Each row in Settings shows one status line, with its button at the right edge. Long text in a row is cut with "…" and the button stays visible.
- A long add-on name in Settings is cut with "…", so the source address, the status line and the button stay visible.
- The menu and the window title bars now follow a change of screen resolution on a monitor even when the menu window keeps its size.
- When the title bars cannot be scaled, the system tries the same scale again up to three times, five seconds apart, before it waits for a change.
- The menu header reads "by oetiker.ai" under the name and sits further from the screen edges.
- The image now carries the licences of every font built into the menu: `Inter-OFL.txt` and, for the plain, monospace and emoji fonts, the files in `egui/`, both in `/usr/share/thinfloe/fonts/`. The Oxanium font and its licence are gone.
- The Terminal tile's font and margin, and the window title bars, follow the same screen density as the menu.
- The first boot of a stick larger than its image enlarges the boot partition to the end of the stick, at most 6 GiB, so there is room for later upgrades, add-ons and the saved home. The boot screen shows "Enlarging the boot stick, once: NN % - do not switch off" while it works. Sticks already in use with 2000 MB are enlarged the same way.
- New images format the boot partition with 4 KiB clusters instead of 512-byte ones, so "Enlarging the boot stick, once" at the first boot and the check of the boot partition at every boot have an eighth of the clusters to handle. Sticks already in use keep their clusters.
- "Enlarging the boot stick, once" finishes much sooner, on sticks already in use too: the initramfs carries `fat32expander` 0.1.2, which moves the data in 4 MiB writes and flushes the stick 58 times instead of 2970 for a 3000 MB boot partition.
- The Terminal tile opens a terminal in the menu's colours, with the Smalti pixel font, a margin around the text and room for 100 columns; the font and margin grow with the screen's pixel density. While you scroll back, a box on the right edge shows where you are.
- On Intel graphics the boot picture is meant to stay on screen until the menu appears, instead of going dark for about 10 seconds or not showing at all on some older machines.
- The boot screen shows what the machine is doing on a line above the host name, for example "Restoring the home directory", "Installing ThinLinc" or "Starting the session", and stays up until the menu appears.
- The boot screen, the menu, Settings, the dialogs and the window title bars share one new look: the ThinFloe logo on a near-black background with ice-blue accents and the Geo typeface, a header with the clock, and the host name at the foot of the boot screen once it is known. Window title bars use the same colours and font.
- The Terminal and Back up home tile icons have square frames, like the tiles.
- The taskbar at the bottom of the screen is gone. A folded window now shows in a strip at the bottom of the menu and comes back with one click.
- The image download (`thinfloe-usb.img.zst`) is now 599 MB instead of 772 MB, and an upgrade downloads 602 MB instead of 775 MB. `initrd.img` on the boot stick no longer carries WiFi firmware; the running system loads it from the main image, as it did before.
- The unused `ubuntu` login from the Ubuntu base image is gone. `tfuser` keeps its user ID.
- `vim` on the machine is now the small `vim-tiny`: the command `vim` still opens it, but without syntax colours and plugins. `joe` is unchanged.
- ThinLinc is no longer part of the image. It is installed from cendio.com when ticked in Settings, and a machine that had it is ticked on its first ThinFloe boot. OxuLnk works the same way; it comes from the oposs package repository at gitea.oetiker.ch.
- The login user is now `tfuser` instead of `tluser`, with the same user ID, home contents and saved settings. `/home/tluser` still leads to the home, and SSH logins and any `sudo thinfloe-password` password now use `tfuser`.
- Machines are now named `thinfloe-` followed by the same six digits, instead of `dbrrg-`. DNS entries, monitoring and ThinLinc server settings that know a machine by its name need updating.
- The commands an administrator types have new names: `sudo thinfloe-password` (was `dbrrg-password`), `thinfloe-upgrade` (was `upgrade-image`) and `thinfloe-menu --check` (was `dbrrg-menu --check`).
- Boot and session log lines now start with `thinfloe:` instead of `dbrrg:`.
- Tile files now use `X-TF-Action`, `X-TF-Save-On-Exit` and `X-TF-Multiple`. Files that still carry the old `X-DBRRG-` keys keep working, and the new key wins when a file has both.
- The screen-blanking setting in `~/.thinfloe-sessionrc` is now `TF_IDLE_TIMEOUT`. The old `DBRRG_IDLE_TIMEOUT` still works when `TF_IDLE_TIMEOUT` is not set.
- The settings files in the home directory have new names: `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys` and `~/.config/thinfloe/`. The first ThinFloe boot renames the old `~/.dbrrg-*` files once, so keyboard layout, password, SSH host keys and tiles carry over, and the boot log names each file, for example `thinfloe: renamed ~/.dbrrg-environment to ~/.thinfloe-environment`.
- The previous version, started from the boot menu after the upgrade, no longer finds these settings: it starts with the default keyboard, display settings and tiles, no remote-access password and new SSH host keys. WiFi, WireGuard and ThinLinc settings still work there, and changes made while it runs are not carried back to ThinFloe. `sudo thinfloe-password --clear` also removes a password set while the previous version was running, so it cannot come back there.
- The boot option `rd.thinfloe.debug` replaces `rd.dbrrg.debug`. The old option still works.

### Fixed

- A named pipe put in place of a tile file, an icon or a status file while the menu reads it no longer hangs the menu, which left the screen black at login.
- Screen blanking that stops because its helper exited is restarted after 5 seconds; before, the screen no longer blanked for the rest of the session.
- Sound from ThinLinc and OxuLnk played quietly, because the speakers and headphones started at 40 % on every boot and the machine has no volume control. Outputs and microphones now start at 100 %, so the remote desktop sets the loudness. A volume set by hand with `wpctl` or `pactl` is kept.
- With two monitors, window title bars grew while a fullscreen program such as OxuLnk or ThinLinc covered the menu, and shrank back only when the menu happened to redraw, for example when a window was moved. The title bars and the menu now keep their size.
- When the graphics driver took over the screen during boot, the boot picture shrank into the top-left corner of a large monitor. It now fills the largest monitor again. A smaller second monitor shows the middle of the picture.
- On machines whose graphics driver loads late (AMD, NVIDIA), the boot screen showed "Starting SSH" for up to 20 seconds while the machine waited for the driver; it now shows "Waiting for the graphics driver".
- Each `thinfloe:` line of the early boot appeared two or three times in a row on the console and twice in the system log; it now appears once.
- The boot log no longer shows `cannot create /sys/block/zram1/max_comp_streams: Permission denied`.
- The text console with the Ubuntu welcome text no longer shows up between the boot screen and the menu.
- The ThinFloe logo on the boot screen is sharp on every screen size: on a 4K screen it was enlarged from a small picture and its edges were soft, and on small screens they were slightly soft too. On some screen sizes the logo is now a little smaller than before.
- The progress line on the boot screen no longer stops at about 15 % when the menu appears: it now follows the time since the boot screen appeared, reaching about 85 % after 10 seconds.
- With two ThinFloe sticks plugged in, a boot could take the home directory and the add-ons from the other stick, so an add-on installed before a restart was gone after it, and the home could be saved to the wrong stick. Each stick now gets an ID (`thinfloe.stick=` in its `syslinux.cfg`) when `thinfloe-upgrade` writes it or at its first boot, and the boot uses the stick it started from.
- Tile icons in the menu no longer look jagged, with broken or dotted lines: each icon is now drawn at the exact size it appears at, on the tiles, in the Settings rows and on the folded-window chips.
