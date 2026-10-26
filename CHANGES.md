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

- The boot screen shows the ThinFloe logo and name with a thin slanted progress line on a black background with a faint blue glow, and the host name at the foot once it is known, instead of the firmware logo and a spinner.
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
