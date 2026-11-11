This note covers `thinfloe-upgrade`: the home copy and size check of a fresh install, the batch interface the menu uses, and the check timers.

- `thinfloe-upgrade`'s fresh install can copy this machine's home onto the new
  drive: `home.tar.gz` on its ESP, addressed by device node
  (`find_efi_partition(device)`), never by partlabel - the new stick carries
  the same `EFI-SYSTEM` label as the running one. It uses the save-home
  exclude list and **always** leaves out `~/.thinfloe-ssh-host-keys` and
  `~/wg0.conf` (`IDENTITY_EXCLUDES`, outside the user-editable list). Those
  are machine identities: two machines would share one SSH host key or one
  WireGuard private key, which breaks both tunnels. `~/.dbrrg-ssh-host-keys`
  is left out too: a save on a 3.x fallback boot leaves the keys 3.x made
  there. `~/.thinfloe-password` travels on purpose. Like `thinfloe-save-home`,
  it refuses when this boot's restore failed
  (`/run/thinfloe/state/home-restore` is `failed`), since the live home is
  then a default one. `test/unit/test_upgrade_image.py` guards this, run via
  `make test-unit`.
- Before the fresh install asks to erase the drive, it compares the drive
  (`lsblk -bdno SIZE`) with the image's uncompressed size, read from the
  zstd frame header of `thinfloe-usb.img.zst` (`curl -r 0-17`), and stops when
  the drive is smaller: `dd` onto such a stick failed partway and left it
  unbootable. When the size cannot be learned (no range support, a stream
  compressed from a pipe) it warns and goes on.

- `thinfloe-upgrade drives [--all] [--source URL] [--no-size]` and `write --device DEV
  --mode fresh|upgrade --expect-id ID [--source URL] [--copy-home] [--verify]
  [--allow-internal]` are the question-free batch interface the menu calls
  with `sudo -n`; the interactive mode over SSH uses the same functions.
  `drives` prints a JSON list (`device`, `model`, `size_bytes`, `serial`,
  `transport`, `removable`, `mounted`, `is_boot`, `is_thinfloe`, `id`,
  `fits`, ...) of USB or removable drives only, unless `--all`; `write`
  refuses a drive that is not in that list (internal ones need
  `--allow-internal`), whose `id` differs from `--expect-id`, or that is
  mounted (fresh mode: always; upgrade mode: unless it is the boot stick),
  with exit 2. Progress goes to stdout as `@step <id> <text>`, `@progress
  <done> <total or ->`, `@done <id>` and one `@result ok|failed <reason>`;
  exit 0 done, 1 failed, 2 refused, 130 cancelled by SIGTERM. A fresh write
  that is cancelled or fails wipes the start of the stick, so it does not
  try to boot half-written; an upgrade that is cancelled or fails leaves a
  stick that starts its current release (`tl/` whole, `DEFAULT` on a release
  that boots), though `tl.old/` may be gone and a complete `tl.new/` may stay
  staged. The `id` is `sn-<serial>` (`nosn` without a serial),
  size and partition table, joined by `:`, so two blank sticks of one
  model without a serial get the same id; it never starts with `-`, so
  `--expect-id ID` and `--expect-id=ID` both work. A write
  holds `/run/thinfloe/upgrade.lock` for the whole job, opens a fresh
  target with `O_EXCL` (busy: exit 2) and checks the id again through that
  open; `drives` waits at most 2 s for the lock and then mounts no ESP
  (`examined: false`). A wipe closes that open before `partprobe`, which
  fails with EBUSY while it is held. The interactive mode takes the lock
  for each drive scan and then again only after the last question, for the
  write, and checks the chosen drive with the same refusals as `write`
  (changed id, mounted, boot drive for a fresh install): an SSH session
  left at a prompt blocks no menu write. An ESP that is mounted already (the boot stick's) is
  read in place: the kernel refuses a second, read-only mount of it, and
  the boot stick looked like no ThinFloe drive. Every child process runs in
  a session of its own, because the menu signals the tool's whole process
  group. `make qemu-smoke-write` writes two blank USB disks in QEMU from a
  boot server in a namespace: one write cancelled, one complete with
  `--verify`, then an upgrade of the boot disk from a stand-in release and
  a boot of the written stick.
  `drives --no-size` sends no request and sets `fits` to null in every row.
  `check [--source URL]` reads `<source>/version` (one line `X.Y.Z`) and the
  size of `thinfloe-usb.img.zst`, prints `latest`, `image_bytes`, `checked`,
  `source` and `error` as `key=value` lines and exits 0, or 1 when a request
  failed. Without `--source` it also writes the lines, plus `installed=` from
  `/etc/thinfloe/version`, atomically to `/run/thinfloe/upgrade/status`.
  A failed check of the default source keeps what the last one found: an
  unreadable version keeps the previous `latest=`, and an unreadable size
  keeps the previous `image_bytes=` only when `latest=` is the version that
  size was read for; `error=` says what failed.
  `test/unit/upgrade-write-args.tsv` lists the `write` calls the menu may make.
- `thinfloe-upgrade-boot-check.timer` runs `thinfloe-upgrade check` 5 min
  after boot without random delay, and `thinfloe-upgrade-check.timer` runs it
  once a day (`OnUnitActiveSec=1d`, `RandomizedDelaySec=1h`), so the Upgrade
  tile sees a fresh `/run/thinfloe/upgrade/status`. The
  service is skipped on netboot (`ConditionPathIsMountPoint` on the ESP) and
  retried after an hour when a check fails (`Restart=on-failure`,
  `RestartSec=1h`). The unit list is `overlay/etc/systemd/system/`, and
  `test/integration/test-session-packages.sh` asserts the enable link and the
  schedule.
