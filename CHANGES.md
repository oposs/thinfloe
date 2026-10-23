# Changelog

## [Unreleased]

### New

### Changed

- Machines are now named `thinfloe-` followed by the same six digits, instead of `dbrrg-`. DNS entries, monitoring and ThinLinc server settings that know a machine by its name need updating.
- The commands an administrator types have new names: `sudo thinfloe-password` (was `dbrrg-password`), `thinfloe-upgrade` (was `upgrade-image`) and `thinfloe-menu --check` (was `dbrrg-menu --check`).
- Boot and session log lines now start with `thinfloe:` instead of `dbrrg:`.
- Tile files now use `X-TF-Action`, `X-TF-Save-On-Exit` and `X-TF-Multiple`. Files that still carry the old `X-DBRRG-` keys keep working, and the new key wins when a file has both.
- The screen-blanking setting in `~/.thinfloe-sessionrc` is now `TF_IDLE_TIMEOUT`. The old `DBRRG_IDLE_TIMEOUT` still works when `TF_IDLE_TIMEOUT` is not set.
- The settings files in the home directory have new names: `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys` and `~/.config/thinfloe/`. The first boot renames the old `~/.dbrrg-*` files, so keyboard layout, password and SSH host keys carry over, and leaves the old names as links so the previous version still starts with the same settings.
- A remote-access password removed with `sudo dbrrg-password --clear` while the previous version was running stays removed when the machine starts ThinFloe again, and the boot log says `thinfloe: ~/.dbrrg-password was removed by the previous version; clearing ~/.thinfloe-password`.
- A remote-access password set again with `sudo dbrrg-password` while the previous version was running is the one in effect when the machine starts ThinFloe again, and the boot log says `thinfloe: ~/.dbrrg-password was set by the previous version; it replaces ~/.thinfloe-password`.
- On a stick with an image from before 2026-10-07, the first boot after the upgrade to ThinFloe comes up with default settings (keyboard layout, remote-access password, SSH host keys) and cannot save the home directory. Restart once and the settings are back.
- The boot option `rd.thinfloe.debug` replaces `rd.dbrrg.debug`. The old option still works.
