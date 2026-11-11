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
  stores the hash the system produced in `~tfuser/.thinfloe-password`, mode 600
  and **owned by tfuser**, like the rest of the home. Both save paths of
  `thinfloe-save-home` run tar as root, so a file tfuser cannot read is still
  archived; until 2026-10 the netboot path ran tar as tluser (the account's name before 4.0), and one such
  file failed every netboot save with exit 5.
  `thinfloe-password.service`, ordered `Before=ssh.service`, reinstalls it on
  every boot with `chpasswd -e`. `--clear` removes it and locks the account
  again.

  Consequence worth stating plainly: that hash sits in `home.tar.gz` on a FAT
  EFI partition with no file permissions, or is POSTed to the boot server over
  plain HTTP. Anyone holding the boot medium can read it and attack it
  offline. This is the same exposure the SSH host *private* keys in
  `~/.thinfloe-ssh-host-keys` already carry, which is why it was accepted.

- `thinfloe-save-home` leaves out what makes the archive slow to unpack on every
  boot (a 233MB Claude binary did this): patterns come from
  `~/.save-home-exclude`, falling back to `/etc/thinfloe/save-home-exclude`, and
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
