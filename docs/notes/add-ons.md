This note covers how add-ons (ThinLinc, OxuLnk) are described, stored, installed, checked and shown in the menu.

ThinLinc and OxuLnk are not in the image. They are add-ons: the user ticks
them in the menu's Settings screen, and the root tool
`/usr/bin/thinfloe-addon` (`overlay/usr/bin/thinfloe-addon`, its top
docstring lists the commands) fetches, stores and installs them. The image
carries only the libraries they need (see
[Standing Constraints](history-of-constraints.md#add-on-runtime-libraries-are-listed-by-name-in-the-dockerfile)).
`overlay/home/tfuser/.thinlinc/tlclient.conf` still ships in the default
home; removing it is later work.

- **What an add-on is.** A tile file (`.desktop`) with `X-TF-Source-*` keys,
  in `/etc/thinfloe/menu/` or `~/.config/thinfloe/menu/`. Its id is the file
  name without `.desktop` (`10-thinlinc`). Sources: APT
  (`X-TF-Source-Apt`, `-Package`, optional `-Key`), a fixed address
  (`-URL`), or a page plus a Python regex (`-Page`, `-Pattern`, named group
  `version` optional). `X-TF-Source-Format=deb|tar` overrides the format,
  taken from the URL otherwise. `X-TF-Source-Config=<path in home>` seeds the
  tick from 3.x. A user file rewording a shipped tile cannot set any source
  key (`SHIPPED_ONLY_KEYS`); a user file that matches no shipped one may.
- **Tick state:** `~/.config/thinfloe/addons`, one id per line, owned by
  tfuser, written only by `thinfloe-addon` (the menu calls `sudo -n
  /usr/bin/thinfloe-addon <verb> <id>`). It travels with the home archive.
- **Commands:** `boot`, `status`, `check [ID...]`, `install ID`, `update ID`,
  `remove ID`. Exit 0 on success, 1 on a failure that is in the log, 2 on
  misuse.
- **ESP layout**, `/run/thinfloe/storage/efi/addons/<id>/`: `current.<ext>`
  + `current.meta` (the installed copy), `next.<ext>` + `next.meta` (ready
  for the next boot), `*.new` while downloading. `meta` is `key=value`
  lines: `version`, `fingerprint`, `url`, `format`, `package` (deb),
  `fetched` (epoch). A slot without its meta is incomplete and ignored, with
  one exception: a power loss between the two renames of a promote leaves
  `current.<F>` (the former `next.<F>`) and `next.meta` (format `F`) with
  no `current.meta` and no `next.<F>`, and `boot` and `check` first finish
  that promote under `esp.lock` (`finish_promote`, logged as `version X was
  being made current when the power went`). Nothing else leaves that state:
  every store renames the payload before its meta, and `_drop_slot` and
  `remove` delete the metas before the payloads. A download is stored only
  if the ESP keeps 64 MiB plus the size of `home.tar.gz` free after it
  (a save writes `home.tar.gz.new` beside the old archive), else the
  status says `not enough space on the boot stick`; the RAM store on
  netboot keeps 64 MiB. On netboot there is no
  ESP: the same layout lives in `/run/thinfloe/addons-cache/`, rebuilt each
  boot. `thinfloe-upgrade`'s space check adds the size of every `current.*`,
  room for one `next.*` each.
- **Locks.** Order is `install.lock` (dpkg/tar, promotion, the installed
  record) -> `/run/thinfloe/esp.lock` (every ESP write, tmpfiles, 0644 root)
  -> `.lock` (log, status, ticks); `install.lock` and `.lock` are under
  `/run/thinfloe/addons/`, `esp.lock` is not. Never take an earlier one
  while a later one is held. `thinfloe-save-home` takes
  only `esp.lock` (`flock -w 30`), so a save never waits for dpkg.
  `thinfloe-upgrade` takes `esp.lock` (`esp_lock()`, 30 s each, then `the
  boot stick is busy`) only on an upgrade of the boot stick and only in two
  short places of the `switch` step: around the removal of `tl.old/` and any
  stale `tl.new/`, and around the final `sync` and the boot label
  (`DEFAULT new` in both `syslinux.cfg` copies). The copy into `tl.new/`, and
  the `sync` that flushes it, run without it, because a save gives up waiting after 30 s and then writes
  unlocked, while an add-on store write waits with no limit. If the second
  wait times out, `tl.new/` stays staged, `DEFAULT` is unchanged and the
  stick starts the old release; the space check includes the size of
  `home.tar.gz`, since a save beside the copy writes `home.tar.gz.new`. It
  never locks for the download or for another stick. It holds
  `/run/thinfloe/upgrade.lock` for the whole write and takes `esp.lock`
  inside it; `upgrade.lock` is not in the add-on order, so there is no
  cycle. A missing lock file is skipped with a log line, as
  `thinfloe-save-home` does.
- **Status file** `/run/thinfloe/addons/status`, rewritten atomically, one
  line per known add-on, tab-separated: `id ticked state installed_version
  pending_version auto message`. `ticked` and `auto` are `0`/`1`; `state` is
  `absent`, `installed`, `pending`, `failed` or `install-failed`; an empty
  field is `-`. `last-check` holds the epoch of the last full check.
- **Log file** `/run/thinfloe/addons/log`: `epoch<TAB>text`, appended by
  every run (boot unit and timers included), also printed to stdout, kept to
  the last 500 lines. The menu shows these lines with source `add-ons`.
- **Boot order.** `thinfloe-addons.service` runs `thinfloe-addon boot` with
  no network, `Before=getty@tty1.service`, and deliberately no `Before=` on
  a sysinit unit (see the hook-order constraint). It seeds the ticks, then
  per ticked add-on installs `next.*` first and promotes it to `current.*`
  only after it installed; if it fails, `next.*` is deleted and `current.*`
  is installed again. With no `next.*` it installs `current.*`. Budget:
  `TimeoutStartSec=120`; boot spends at most `BOOT_BUDGET` 100 s on installs
  and gives each dpkg or tar call at most `BOOT_TOOL_TIMEOUT` 25 s. An
  add-on whose turn comes after the budget is spent is not installed at that
  boot and its status says `install-failed`. A `next.*` whose install runs
  out of time is kept (a timeout says nothing about the copy) and
  `current.*` is not tried. The check after boot installs what boot left
  out (below). An installed add-on boots from its ESP copy with no network.
- **Checks.** `thinfloe-addon-boot-check.timer` (`OnBootSec=1min`, no random
  delay) and `thinfloe-addon-check.timer` (`OnUnitActiveSec=1d`,
  `RandomizedDelaySec=1h`) start `thinfloe-addon-check.service`
  (`After=network-online.target`), which runs `check` with no ids. The two
  timers are separate because systemd applies `RandomizedDelaySec` to every
  firing, and on netboot the first check is what installs the add-ons. An
  update found by a check is stored as `next.*` and becomes active at the
  next boot. A ticked add-on with no `current.*` (always so on netboot) is
  fetched and installed at once. Before it looks for updates, every check
  installs a ticked add-on's stored copy that this boot has not installed
  (boot ran out of time, or the install failed): `next.*` the way boot
  does, else `current.*`. That fetches nothing, so it applies to a
  manual-only source too (the auto rule below forbids only fetching). A failed check or download leaves the
  installed copy and logs why.
- **The auto rule.** A source is fetched automatically only when every URL
  involved (page, matched link, fixed URL, APT URI, every redirect hop) is
  `https:`, or the source is APT with a key (`auto_allowed`). Any other
  source is never checked or fetched by `check` with no ids, nor by the
  timers; only Install now / Update now (`install ID` / `update ID`) change
  it, and its Settings row says "updates only by hand". Check now in
  Settings runs `check` with no ids, so it follows the same rule. `check
  ID` on such a source only reports.
- **First-boot seeding.** `boot` creates the tick file when none exists and
  either `/run/thinfloe/state/home-renamed` exists (a 3.x home was migrated)
  or `home-restore` is `absent`, `tl.old/` is on the ESP and
  `/config/addons-seeded` is not (a 3.x stick upgraded without a saved
  home). It ticks every shipped add-on whose `X-TF-Source-Config` exists in
  the home (ThinLinc `.thinlinc`, OxuLnk `.config/oxulnk`), or every shipped
  add-on with that key when no home was stored. On every USB boot that
  finds `/config/addons-seeded` missing, the marker is written, so a stick
  that booted 4.x once is never seeded again. Netboot seeds too; its check
  unit then installs. `make qemu-smoke-upgrade` requires ThinLinc ticked
  from a 3.x home, with status `failed` (no network).
- **dpkg timing.** Measured 2026-10-23 in the built image: `dpkg -i` of the
  ThinLinc client takes about 4 s. Reinstall at boot is `dpkg -i` of the
  stored `.deb` (`--force-confdef --force-confold`, stdin `/dev/null`) or
  `tar -x -C /` of a stored tar. sysext was rejected: it skips maintainer
  scripts and cannot carry a tar overlay that writes `/etc`.
- **Post-install hooks.** After a successful install the tool runs
  `/usr/libexec/thinfloe/addon-hooks/<id>` if that file, shipped in the
  image, is executable (the tool checks only that; tfuser can become root
  anyway, so an owner check would add nothing); a hook path never comes
  from a tile or the home. A failing
  hook fails the install. Hook `10-thinlinc` puts the PipeWire shim
  (`/usr/libexec/thinfloe/tlclient-pulseaudio`) over the client's own
  `/opt/thinlinc/lib/tlclient/pulseaudio`, because `dpkg -i` overwrites the
  file the old overlay put there.
- **Removing a tar add-on** deletes the files its listing named, even if
  they overwrote image files; those come back at the next reboot. A tar
  overlay should therefore not overwrite image files.
- **The menu.** The Settings tile is `60-settings.desktop` with
  `X-TF-Action=settings`, honoured only in shipped files. Each row has a tick
  box (on runs `install`, off runs `remove`) and shows the state; Install now,
  Update now and Check now run the verbs above.
  Add-on tiles are drawn only when an installed version exists; a pending
  one shows the update marker. A failed install asks Retry once per boot per
  add-on, remembered in `/tmp/thinfloe-addon-retry.<uid>` (`TF_RETRY_MARKER`
  overrides). The menu reads the add-on files at start and again whenever
  they change. A tile whose file name `thinfloe-addon` refuses (see ids
  below) is never drawn; its Settings row says "file name not usable", with
  the tick box disabled and no button. Log out, Restart and Power off list a
  running add-on job (Install now, Update now, a tick, Check now) by the
  add-on's name and wait for it after stopping the programs, with Stay
  offered. Log out only waits: the system keeps running, and a dpkg cut
  short would stay half-installed. Restart and Power off also offer
  "Restart anyway" / "Power off anyway": SIGTERM to the job's process
  group, then the save and exit 10 / 11 once the job ended or after 5 s,
  since the shutdown ends what is left. The job runs in a process group of
  its own (`sudo -n thinfloe-addon` started with `process_group(0)`),
  because the stop signals a process group (`kill(-pgid)`), which would
  otherwise be the menu's own. sudo keeps tfuser's real uid, so it receives
  the SIGTERM and relays it to the tool; tfuser cannot signal the root tool
  itself. A stick write started from the Upgrade screen is listed and
  waited for the same way ("Writing SanDisk Cruzer"). Restart anyway and
  Power off anyway send it SIGTERM, never SIGKILL: a SIGKILL reaches only
  sudo, and the tool keeps holding `/run/thinfloe/upgrade.lock`. A
  Save-On-Exit program that ends while the question waits only for an
  add-on job starts no save: the logout save follows, or, on Stay, the
  owed save starts then.
- **Ids.** An add-on's id is its tile's file name without `.desktop`: 1 to
  64 letters, digits and `. _ + -`, starting with a letter or digit and not
  ending in `.` (vfat strips it). An id that differs from another add-on's
  only in case is refused, since both would share one directory on the
  ESP; shipped tiles win, then user tiles in file-name order. `ID_RE` in
  the tool and `valid_id` in the menu apply the rule, and
  `test/unit/addon-ids.tsv` pins both sides.
- **Add-ons run as root, by design.** Tar and deb add-ons write anywhere on
  the system. With the tile files and the tick file in the home archive this
  means a home archive can bring software onto a machine: whoever can write
  `home.tar.gz` on the ESP (or `home.pkg` on the boot server) and name a
  source can have that source's package installed as root at the next boot
  or check. A source that is neither HTTPS nor signed is fetched only by a
  manual Install now, but it is then installed with no further check.

Guards: `test/integration/test-addon-deps.sh`, `test/unit/` (the tool, via
`make test-unit`), the tile and settings tests in `src/thinfloe-menu`, `make
qemu-smoke-addons` (a stored test `.deb` ticked by the home archive is
installed before `getty@tty1.service` starts, with no network, no ordering
cycle, status `installed`) and `make qemu-smoke-upgrade`. The smoke VMs
report through `systemd.run=` and `scripts/smoke-addons-report.sh` on the
ESP, so the image carries no test code.
