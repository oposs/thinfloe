# Changelog

## [Unreleased]

### New

- Add-ons listed in `~/.config/thinfloe/addons` are installed at every boot from their copy on the boot stick, with no network, and are checked for new versions one minute after boot and once a day at a random time within an hour; a new version becomes active at the next boot. On netboot, that check fetches and installs them once the network is up. The first boot after an upgrade from 3.x lists ThinLinc and OxuLnk when the home holds their settings (`~/.thinlinc`, `~/.config/oxulnk`), or both when the stick had no saved home.
- An add-on copy on the boot stick always leaves room for a second copy of the saved home directory, and an update cut off by a power failure while it was being made current is finished at the next boot. A ticked add-on that the boot did not install, because its time ran out or the install failed, is installed from the boot stick by the check one minute after boot.
- The menu shows the tile of an add-on only while the add-on is installed, marks it "update" when a new version waits for the next boot, and enables it as soon as an install finishes, without a restart. Everything the add-on tool does (checks, downloads, installs, failures) appears in the menu log under `add-ons`.
- A new Settings tile opens the Add-ons screen: ticking an add-on installs it at once, unticking removes it, and each row shows its source, its installed version and any update ready for the next boot, with Install now and Update now buttons. Check now looks for updates of every ticked add-on whose source is HTTPS or signed; any other source shows "updates only by hand" and changes only through its buttons. A tile whose file name cannot be an add-on name (over 64 characters, not starting with a letter or digit, ending in a dot, or another add-on's name apart from case) shows "file name not usable" there, with its tick box off and no button.
- When a ticked add-on could not be installed, the menu asks once per boot "<Name> could not be installed: <reason>" with Retry and Later.
- Log out, Restart and Power off wait for an add-on install, update, removal or check started from Settings: the dialog names it ("Installing ThinLinc"), and Stay stays available while it finishes. Restart and Power off also offer "Restart anyway" and "Power off anyway", which stop the add-on work and go on after at most 5 seconds. Running programs are stopped as before.
- The clock in the menu and the time stamps in its log show the time zone set with `TZ=` in `~/.thinfloe-environment`, else the one the DHCP server sends (option 101), else UTC; a zone that arrives after the menu started shows within a minute. The system log says `thinfloe: time zone <zone> from DHCP`, or `thinfloe: ignored DHCP time zone '<value>': <reason>` for a value it refuses. WiFi profiles from `~/wifi.yaml` do not ask the DHCP server for a zone.

### Changed

- The menu's tiles keep their shape and look the same size on every screen. A 4K screen that reports its size no longer gets stretched tiles (one that reports none or an implausible size still gets double size), and a small, dense laptop screen shrinks the menu to fit.
- Each row in Settings shows one status line, with its button at the right edge. Long text in a row is cut with "…" and the button stays visible.
- A long add-on name in Settings is cut with "…", so the source address, the status line and the button stay visible.
- The menu and the window title bars now follow a change of screen resolution on a monitor even when the menu window keeps its size.
- When the title bars cannot be scaled, the system tries the same scale again up to three times, five seconds apart, before it waits for a change.
- The menu header reads "by oetiker.ai" under the name and sits further from the screen edges.
- The Terminal tile's font and margin, and the window title bars, follow the same screen density as the menu.
- The first boot of a stick larger than its image enlarges the boot partition to the end of the stick, at most 6 GiB, so there is room for later upgrades, add-ons and the saved home. The boot screen shows "Enlarging the boot stick, once: NN % - do not switch off" while it works. Sticks already in use with 2000 MB are enlarged the same way.
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

- On machines whose graphics driver loads late (AMD, NVIDIA), the boot screen showed "Starting SSH" for up to 20 seconds while the machine waited for the driver; it now shows "Waiting for the graphics driver".
- Each `thinfloe:` line of the early boot appeared two or three times in a row on the console and twice in the system log; it now appears once.
- The boot log no longer shows `cannot create /sys/block/zram1/max_comp_streams: Permission denied`.
- The text console with the Ubuntu welcome text no longer shows up between the boot screen and the menu.
- The ThinFloe logo on the boot screen is sharp on every screen size: on a 4K screen it was enlarged from a small picture and its edges were soft, and on small screens they were slightly soft too. On some screen sizes the logo is now a little smaller than before.
- The progress line on the boot screen no longer stops at about 15 % when the menu appears: it now follows the time since the boot screen appeared, reaching about 85 % after 10 seconds.
- With two ThinFloe sticks plugged in, a boot could take the home directory and the add-ons from the other stick, so an add-on installed before a restart was gone after it, and the home could be saved to the wrong stick. Each stick now gets an ID (`thinfloe.stick=` in its `syslinux.cfg`) when `thinfloe-upgrade` writes it or at its first boot, and the boot uses the stick it started from.
- Tile icons in the menu no longer look jagged, with broken or dotted lines: each icon is now drawn at the exact size it appears at, on the tiles, in the Settings rows and on the folded-window chips.
