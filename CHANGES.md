# Changelog

## [Unreleased]

### New

- Add-ons listed in `~/.config/thinfloe/addons` are installed at every boot from their copy on the boot stick, with no network, and are checked for new versions one minute after boot and once a day at a random time within an hour; a new version becomes active at the next boot. On netboot, that check fetches and installs them once the network is up. The first boot after an upgrade from 3.x lists ThinLinc and OxuLnk when the home holds their settings (`~/.thinlinc`, `~/.config/oxulnk`), or both when the stick had no saved home.
- The menu shows the tile of an add-on only while the add-on is installed, marks it "update" when a new version waits for the next boot, and enables it as soon as an install finishes, without a restart. Everything the add-on tool does (checks, downloads, installs, failures) appears in the menu log under `add-ons`.
- A new Settings tile opens the Add-ons screen: ticking an add-on installs it at once, unticking removes it, and each row shows its source, its installed version and any update ready for the next boot, with Install now and Update now buttons. Check now looks for updates of every ticked add-on whose source is HTTPS or signed; any other source shows "updates only by hand" and changes only through its buttons.
- When a ticked add-on could not be installed, the menu asks once per boot "<Name> could not be installed: <reason>" with Retry and Later.

### Changed

- ThinLinc is no longer part of the image. It is installed from cendio.com when ticked in Settings, and a machine that had it is ticked on its first ThinFloe boot. OxuLnk works the same way once its package source is published.
- The login user is now `tfuser` instead of `tluser`, with the same user ID, home contents and saved settings. `/home/tluser` still leads to the home, and SSH logins and any `sudo thinfloe-password` password now use `tfuser`.
- Machines are now named `thinfloe-` followed by the same six digits, instead of `dbrrg-`. DNS entries, monitoring and ThinLinc server settings that know a machine by its name need updating.
- The commands an administrator types have new names: `sudo thinfloe-password` (was `dbrrg-password`), `thinfloe-upgrade` (was `upgrade-image`) and `thinfloe-menu --check` (was `dbrrg-menu --check`).
- Boot and session log lines now start with `thinfloe:` instead of `dbrrg:`.
- Tile files now use `X-TF-Action`, `X-TF-Save-On-Exit` and `X-TF-Multiple`. Files that still carry the old `X-DBRRG-` keys keep working, and the new key wins when a file has both.
- The screen-blanking setting in `~/.thinfloe-sessionrc` is now `TF_IDLE_TIMEOUT`. The old `DBRRG_IDLE_TIMEOUT` still works when `TF_IDLE_TIMEOUT` is not set.
- The settings files in the home directory have new names: `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys` and `~/.config/thinfloe/`. The first ThinFloe boot renames the old `~/.dbrrg-*` files once, so keyboard layout, password, SSH host keys and tiles carry over, and the boot log names each file, for example `thinfloe: renamed ~/.dbrrg-environment to ~/.thinfloe-environment`.
- The previous version, started from the boot menu after the upgrade, no longer finds these settings: it starts with the default keyboard, display settings and tiles, no remote-access password and new SSH host keys. WiFi, WireGuard and ThinLinc settings still work there, and changes made while it runs are not carried back to ThinFloe. `sudo thinfloe-password --clear` also removes a password set while the previous version was running, so it cannot come back there.
- The boot option `rd.thinfloe.debug` replaces `rd.dbrrg.debug`. The old option still works.
