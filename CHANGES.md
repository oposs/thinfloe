# Changelog

## [Unreleased]

### New

### Changed

- Machines are now named `thinfloe-` followed by the same six digits, instead of `dbrrg-`. DNS entries, monitoring and ThinLinc server settings that know a machine by its name need updating.
- The commands an administrator types have new names: `sudo thinfloe-password` (was `dbrrg-password`), `thinfloe-upgrade` (was `upgrade-image`) and `thinfloe-menu --check` (was `dbrrg-menu --check`).
- Boot and session log lines now start with `thinfloe:` instead of `dbrrg:`.
