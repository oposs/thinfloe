# Changelog

## [Unreleased]

### New

- Add-ons listed in `~/.config/thinfloe/addons` are installed at every boot from their copy on the boot stick, with no network, and are checked for new versions one minute after boot and once a day at a random time within an hour; a new version becomes active at the next boot. On netboot, that check fetches and installs them once the network is up. The first boot after an upgrade from 3.x lists ThinLinc and OxuLnk when the home holds their settings (`~/.thinlinc`, `~/.config/oxulnk`), or both when the stick had no saved home.

### Changed

- The login user is now `tfuser` instead of `tluser`, with the same user ID, home contents and saved settings. `/home/tluser` still leads to the home, and SSH logins and any `sudo thinfloe-password` password now use `tfuser`.
- Machines are now named `thinfloe-` followed by the same six digits, instead of `dbrrg-`. DNS entries, monitoring and ThinLinc server settings that know a machine by its name need updating.
- The commands an administrator types have new names: `sudo thinfloe-password` (was `dbrrg-password`), `thinfloe-upgrade` (was `upgrade-image`) and `thinfloe-menu --check` (was `dbrrg-menu --check`).
- Boot and session log lines now start with `thinfloe:` instead of `dbrrg:`.
- Tile files now use `X-TF-Action`, `X-TF-Save-On-Exit` and `X-TF-Multiple`. Files that still carry the old `X-DBRRG-` keys keep working, and the new key wins when a file has both.
- The screen-blanking setting in `~/.thinfloe-sessionrc` is now `TF_IDLE_TIMEOUT`. The old `DBRRG_IDLE_TIMEOUT` still works when `TF_IDLE_TIMEOUT` is not set.
- The settings files in the home directory have new names: `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys` and `~/.config/thinfloe/`. The first ThinFloe boot renames the old `~/.dbrrg-*` files once, so keyboard layout, password, SSH host keys and tiles carry over, and the boot log names each file, for example `thinfloe: renamed ~/.dbrrg-environment to ~/.thinfloe-environment`.
- The previous version, started from the boot menu after the upgrade, no longer finds these settings: it starts with the default keyboard, display settings and tiles, no remote-access password and new SSH host keys. WiFi, WireGuard and ThinLinc settings still work there, and changes made while it runs are not carried back to ThinFloe. `sudo thinfloe-password --clear` also removes a password set while the previous version was running, so it cannot come back there.
- The boot option `rd.thinfloe.debug` replaces `rd.dbrrg.debug`. The old option still works.
