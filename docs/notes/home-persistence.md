This note covers how the home directory is restored at boot and saved at restart, power off and after a tile, and what the archive holds.

The system implements home directory persistence across reboots:

- On boot: the initramfs restores `/home/tfuser`, from `home.tar.gz` on the
  EFI partition when booting from USB, or from `home.pkg` on the boot server
  when netbooting. This happens in `restore_home()`
  (`overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh`), called from
  `setup-overlay.sh`, **not** at login. It moved there so the SSH host keys
  stored inside the home directory are available before sshd starts.
  Netboot identifies the machine by the MAC recorded at
  `/run/thinfloe/state/boot-mac` (the interface the initramfs actually used)
  rather than by re-deriving it, so restore and save cannot disagree.
- On restart or power off: `thinfloe-menu` runs `/usr/bin/thinfloe-save-home`,
  which saves the home directory back to USB or uploads it to the boot
  server via HTTP POST. A tile with `X-TF-Save-On-Exit=true` (ThinLinc,
  oxulnk) after its program exits and the Back up home tile save in the
  background, one at a time, with at most one more queued; no dialog. The
  Restart and Power off tiles ask Stay / Stop them and restart (power off)
  while programs run (SIGTERM to each process group, SIGKILL after 5 s),
  wait for a running save, then save behind their dialog before the menu
  exits 10 (11); a failed save asks Stay / Restart anyway (Power off
  anyway). A program stopped this way gets no Save-On-Exit save, and neither
  does the last program ending while the question is up: the menu then goes
  on without asking (if an add-on job still runs, the question stays and
  the save is left to the logout save that follows the job, or done on
  Stay; see [Add-ons](add-ons.md)). The menu greys the save tile out when
  `/run/thinfloe/state/home-restore` says `failed`; after a Save-On-Exit tile
  exits on such a boot, the save is skipped with a line in the menu's log.
- `thinfloe-save-home` resolves the directory to archive from `getent passwd
  tfuser`, never from `$HOME`, and every read of a recorded value under
  `/run/thinfloe/state` ends in `|| true`. Both are load-bearing. `sudo` on this
  image sets `HOME=/root` (`Defaults env_reset`, no `env_keep` for HOME), so a
  save called from a root process archived `/root` over the user's home; and
  `set -e` ends a script on `X=$(cat missing)`, which disabled every save on
  USB boot, where `boot-home-base` is never written.
  `test/integration/test-save-home.sh` guards both.

- The remote-access password rides the same archive. `sudo thinfloe-password`
  stores the hash the system produced in `~tfuser/.config/thinfloe/password`, mode 600
  and **owned by tfuser**, like the rest of the home. Both save paths of
  `thinfloe-save-home` run tar as root, so a file tfuser cannot read is still
  archived; until 2026-10 the netboot path ran tar as tluser (the account's name before 4.0), and one such
  file failed every netboot save with exit 5.
  `thinfloe-password.service`, ordered `Before=ssh.service`, reinstalls it on
  every boot with `chpasswd -e`. `--clear` removes it and locks the account
  again. `thinfloe-password`, `thinfloe-ssh-hostkeys` and `thinfloe-network`
  run as root inside tfuser's home and refuse a symlink at any path they use
  under `~/.config/thinfloe`.

  Consequence worth stating plainly: that hash sits in `home.tar.gz` on a FAT
  EFI partition with no file permissions, or is POSTed to the boot server over
  plain HTTP. Anyone holding the boot medium can read it and attack it
  offline. This is the same exposure the SSH host *private* keys in
  `~/.config/thinfloe/ssh-host-keys` already carry, which is why it was accepted.

- `thinfloe-save-home` leaves out what makes the archive slow to unpack on every
  boot (a 233MB Claude binary did this): patterns come from
  `~/.config/thinfloe/save-home-exclude`, falling back to `/etc/thinfloe/save-home-exclude`, and
  a user file replaces the shipped one rather than extending it. The shipped
  list leaves out `~/Downloads`.
- The archive is for configuration, not data: every regular file over
  10 MB is left out, whatever the exclude list says, and named on stdout
  (`thinfloe: not saved (NN MB, over 10 MB): ./path`), which the menu log
  shows. Files the exclude list already leaves out are not named. A newline
  in such a file's name becomes `?` in the tar pattern, so a small file
  differing from it only there is left out too. `thinfloe-upgrade`'s home copy
  applies the same limit (`SAVE_HOME_MAX_FILE_BYTES`).
- On USB it writes to the ESP the initramfs already mounted at
  `/run/thinfloe/storage/efi`, never a second mount by partlabel - every stick
  carries that label, so with two sticks plugged in the home can land on the
  wrong one. The branch tests `mountpoint -q`, not `-d`: the directory exists
  on every boot, netboot included, and an unmounted one would take the
  archive into RAM. The write is atomic: `home.tar.gz.new`, then `gzip -t`,
  then `mv` over `home.tar.gz`, all through `sudo` because the ESP's vfat
  mount is root-owned. The ESP therefore has to hold two archives briefly.
  The netboot upload uses `curl -f`, otherwise an HTTP 413/500 reply is exit 0.
- Exit codes of `thinfloe-save-home`: 0 saved; 1 home directory missing; 2 this
  boot's restore failed, so saving would overwrite the stored home with a
  default one; 3 boot server unreachable; 4 nowhere to store; 5 save
  attempted and failed. `test/integration/test-save-home.sh` guards each.

This allows WiFi credentials, ThinLinc settings, and user customizations to persist.

## Layout of `~/.config/thinfloe/`

Every file ThinFloe owns in the home is under `~/.config/thinfloe/`. Everything
the menu manages is in `config.toml`, which only the menu writes (atomically,
mode 600, WiFi passwords included). Readers that run before the menu, or
cannot parse TOML, read files the menu derives from it into `generated/`.

| Path | Written by | Read by | Holds |
|---|---|---|---|
| `config.toml` | the menu | the menu | `[keyboard]`, `[screen]`, `[status]`, `[monitors]`, `[network]`, and the migration report until dismissed |
| `generated/environment` | the menu | the session profile, the labwc config composer | `XKB_DEFAULT_*` |
| `generated/screen` | the menu | `thinfloe-session` | `idle_timeout=<seconds>` |
| `generated/kanshi`, `kanshi.prev`, `layout-unconfirmed` | the menu | `thinfloe-session`, kanshi | the monitor arrangement and the undo state of an unconfirmed one |
| `generated/wifi.yaml`, `ethernet.yaml` | the menu | the initramfs (`install_local_network()`), `thinfloe-network` | netplan, mode 600 |
| `environment` | by hand | the session profile, the composer | extra variables (`TZ=`, cursor, `LABWC_FULLSCREEN_SPAN_OUTPUTS`); `XKB_DEFAULT_*` lines are dropped |
| `sessionrc` | by hand | `thinfloe-session` | commands run at session start |
| `menu/*.desktop`, `addons` | by hand, `thinfloe-addon` | the menu, `thinfloe-addon` | user tiles, ticked add-ons |
| `password`, `ssh-host-keys/`, `wg0.conf`, `save-home-exclude` | root tools, by hand | the same, the initramfs, `thinfloe-save-home`, `thinfloe-upgrade` | remote-access hash, SSH host keys, WireGuard, save exclude list |
| `home-version` | the migration | the initramfs | marks the new layout (`2`) |

The menu writes the generated files first and renames `config.toml` last, and
at every start regenerates the generated files from `config.toml`. A generated
file is rewritten only when its content changes. Each starts with a header
comment saying edits are lost.

A `config.toml` the menu cannot parse is kept as `config.toml.broken-<epoch>`
and the settings are rebuilt from the files in `generated/` (`[status]` and
the migration report are not in them and fall back to their defaults). Only
when `generated/` holds no file at all does the menu start from defaults and
write them. When a file in `generated/` cannot be read, the store is
read-only: changes are refused with "The settings are read-only." and a note
on the Settings screen, because defaults would write an empty
`generated/wifi.yaml` and the next boot would be offline. An unreadable old
generated file is put back as the committed settings render it, never
removed.

A WiFi, wired or monitor file the menu could not fully read is kept verbatim
in `config.toml` (`[network]` `kept_wifi` and `kept_wired`, `[monitors]`
`kept`), written under the generated header and shown read-only with "Let the
menu manage it". A kept file is also what an unrenderable converted network
file becomes, and a legacy kanshi file whose profile names fail the name
check. On netboot, a `[network]` with a stick's fixed address still refuses
WiFi changes.

## Which layout a home has

The initramfs writes `/run/thinfloe/state/home-layout`, `legacy` or `new`,
owned by tfuser (the menu rewrites it in place; the directory stays root's).
Only the whole content `legacy`, with or without one newline, means the 4.x
layout; a missing, empty or any other file means `new`. The archive's listing
decides: an archive without `.config/thinfloe/home-version` is `legacy` (the
initramfs then removes the `home-version` the image ships, so a save before
the migration cannot archive a marker the home has not earned); a marked
archive, no archive or a listing that does not exist is `new`. When the listing
is cut short, the marker on disk decides and nothing is removed.

Readers that run before the menu read the new path and, when the layout is
`legacy`, the old one: the initramfs (`install_local_network()`),
`thinfloe-session`, the session profile, `thinfloe-save-home`,
`thinfloe-upgrade`, `thinfloe-password`, `thinfloe-ssh-hostkeys`. Root tools
use the file only to choose between two paths inside tfuser's home. A legacy
home opens Settings read-only even in the session: the Network, Screen and
Status tabs are disabled and a note says the settings could not be moved.

## Migration of a 4.x home

`thinfloe-menu --migrate-home` runs once, called by the session profile before
the labwc config is composed, when the layout is `legacy`. It
converts `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`,
`~/.config/thinfloe/screen` and `status`, `~/.config/kanshi/config`,
`~/wifi.yaml` and `~/ethernet.yaml` into `config.toml`, the generated files
and the new `environment` and `sessionrc`; copies `~/.thinfloe-password`,
`~/.thinfloe-ssh-host-keys`, `~/wg0.conf` and `~/.save-home-exclude` to
`password`, `ssh-host-keys`, `wg0.conf` and `save-home-exclude` with their
modes; writes `home-version`, and sets `home-layout` to `new`; and only then
moves every old file, shipped templates and the `.by-hand` copies included,
into `~/cfg-attic/` under its old relative path. `~/cfg-attic/README` lists what
was converted, copied, dropped or only partly converted, and not moved.

- Only the first entry of a layout or variant list is kept, a keyboard model
  other than `pc105` is dropped, and XKB options other than CapsLock as Ctrl
  and the Compose key are dropped. `TF_IDLE_TIMEOUT` in the old `sessionrc`
  becomes the screen timeout.
- A file the menu cannot fully read (an EAP WiFi entry, a hand-written kanshi
  file) is kept verbatim, as above, so no WiFi password is lost.
- Nothing is deleted. A failure before `home-version` and `home-layout` are
  written leaves every old file where it was, is logged in the session log
  and is tried again at the next login; the session starts on the old files.
  A failure while moving to the attic leaves the remaining files in place;
  the new layout is already in force, so they are no longer read.
- The migration fails, and retries at every login, when the home is not
  owned by tfuser. Only the session log shows it.
- The Settings screen shows the report once, with a Dismiss button.

`~/cfg-attic/` holds the old identity files. A fresh install's home copy leaves
out the attic copies of the host keys and `wg0.conf` as well
(`IDENTITY_EXCLUDES`, see [esp-and-upgrade.md](esp-and-upgrade.md)). The 3.x
rename in the initramfs counts `.config/thinfloe/home-version` as a marker too,
so a migrated home is not renamed again.
