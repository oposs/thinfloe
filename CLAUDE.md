# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ThinFloe creates bootable diskless client images that run entirely from RAM. The name is an ice floe: the system floats in RAM, carries you for a while, and is gone at power-off, except for the saved home. Releases before 4.0 called the project dbrrg (Docker-Based RamRoot Generator); the old name survives in a few places listed under Standing Constraints. The system builds Ubuntu-based images using Podman/Docker containers, packages them into bootable formats (USB/PXE), and includes a custom initramfs boot system. The final images are designed for thin client deployments using ThinLinc.

## Build System Architecture

The build process uses containers in `containers/` directory:

1. **containers/ubuntu/Dockerfile** - Base system container that creates the Ubuntu rootfs with all packages, ThinLinc client, and custom overlay files
2. **containers/ipxe/Dockerfile** - Builds iPXE network boot loaders (PXE, KPXE, EFI formats)
3. **containers/image-builder/Dockerfile** - Final packaging container with tools to create bootable EFI images

The `menu-build` stage of `containers/ubuntu/Dockerfile` compiles the tile menu
`thinfloe-menu` from the crate at `src/thinfloe-menu/` (Rust 1.96.0 via rustup,
`cargo build --release --locked`) and the final stage copies the result into
the image. `MENU_FILES` and `MENU_DIRS` in the `Makefile` list the crate's
files and directories as prerequisites of `.ubuntu-container`, so editing the
crate rebuilds the image.

The `overlay/` directory contains all customizations that get layered onto the base Ubuntu system during the build.

## Common Commands

### Build the complete bootable image:
```bash
make image
```
This runs the build stages and creates bootable files in `artifacts/`.

### Build only the Ubuntu base system:
```bash
make rootfs
```
Builds the container and exports kernel, initramfs, and ramroot.sqsh to `artifacts/rootfs/`.

### Build only iPXE boot loaders:
```bash
make ipxe
```
Compiles iPXE from source and exports PXE/EFI boot files to `artifacts/rootfs/`.

### Write image to USB stick:
```bash
dd if=artifacts/images/thinfloe-usb.img of=/dev/sdX bs=1M status=progress conv=fsync
```

### Publish the image for thinfloe-upgrade:
```bash
scripts/publish.sh
```
Uploads `vmlinuz`, `initrd.img`, `ramroot.sqsh` and
`thinfloe-usb.img.zst` with its checksum to the upgrade server
`https://tobi.oetiker.ch/dbrrg`, which kept its name through the rename.
It also uploads the image as `dbrrg-usb.img.zst` (a hard link under
`artifacts/images/bridge/`), because the 3.x `upgrade-image` fetches only
that name. `vmlinuz`, `initrd.img` and `ramroot.sqsh` keep their names.

## Boot Flow Architecture

The boot process involves several interconnected components:

1. **SYSLINUX/EFI Boot** - Initial bootloader (syslinux.cfg) loads kernel with `ramroot=` parameter

2. **Dracut Module** (overlay/usr/lib/dracut/modules.d/90thinfloe/) - Modern initramfs using Dracut:
   - **module-setup.sh** - Installs hooks and tools into initramfs
   - **parse-thinfloe.sh** - Parses kernel command line parameters
   - **mount-squashfs.sh** - Mounts SquashFS rootfs from local or network source
   - **setup-overlay.sh** - Sets up ZRAM-backed OverlayFS for writes
   - **thinfloe-lib.sh** - Shared library functions
   - **thinfloe-cleanup.sh** - Pre-pivot cleanup

3. **Boot Process**:
   - **SquashFS Mount**: Mounts compressed read-only rootfs from USB partition or HTTP URL
   - **ZRAM Setup**: Creates compressed RAM device for writable overlay
   - **OverlayFS**: Combines read-only SquashFS with writable ZRAM layer
   - **machine-id Persistence**: Reads/generates machine-id and stores it in /config/machine-id on EFI partition
   - **Home Restore**: Restores `/home/tluser` from the boot medium
     (`home.tar.gz` on the EFI partition, or `home.pkg` from the boot server)
     before the pivot, so the home directory is in place before
     `multi-user.target`

4. **Un-dockerization** (overlay/etc/systemd/system/un-dockerize.service) - First-boot service removes Docker artifacts, fixes /etc/hosts, reconfigures systemd-resolved

5. **Home Persistence** - the initramfs restores the home directory before
   the pivot (`restore_home()` in the dracut module). `thinfloe-menu` saves it
   (`thinfloe-save-home`). Saves after a tile with `X-TF-Save-On-Exit=true`
   (ThinLinc, oxulnk) exits and from the Back up home tile run in the
   background, one at a time, with at most one more queued. The Restart and
   Power off tiles ask Stay / Stop them and restart (power off) while
   programs run (SIGTERM to each process group, SIGKILL after 5 s), wait for
   a running save, then save behind their dialog before the menu exits 10
   (11); a failed save asks Stay / Restart anyway (Power off anyway).
   `thinfloe-session` never saves; on 10 and 11 it runs `sudo systemctl
   reboot` or `poweroff`. The `logout` action (exit 0) takes the same steps;
   no shipped tile uses it.

## Key Configuration Files

- **configs/syslinux.cfg** - Boot menu and kernel parameters. The `ramroot=` parameter determines boot source (local: `tl/ramroot.sqsh`, network: `http://server/path/ramroot.sqsh`)
- **overlay/home/tluser/.thinlinc/tlclient.conf** - ThinLinc client configuration
- **overlay/home/tluser/wifi.yaml** - WiFi configuration template for netplan
- **overlay/usr/lib/dracut/modules.d/90thinfloe/** - Dracut module for boot process

## Customizing the System

To modify the deployed system, edit files in `overlay/` following the standard Linux filesystem hierarchy. The overlay is applied during Docker build using:
```dockerfile
ADD overlay/ /__overlay
RUN tar cf - --exclude="*~" -C /__overlay . | tar xf - && rm -rf /__overlay
```

This pattern excludes editor backup files (*~) and properly applies overlay permissions. Common customization points:

- System services: `overlay/etc/systemd/system/`
- Network configuration: `overlay/etc/netplan/`
- SSH configuration: `overlay/etc/ssh/sshd_config.d/`
- Session/compositor: `overlay/etc/thinfloe/labwc/` (`rc.xml`, `environment`) - kept outside `$HOME` because home is captured/restored wholesale by the persistence machinery, see [Persistent Home Directory](#persistent-home-directory).
  `rc.xml` also holds the window rule that keeps the window with app_id
  `thinfloe-menu` at the bottom (`ToggleAlwaysOnBottom`) and out of the taskbar
  (`skipTaskbar`).
- Taskbar: `overlay/etc/thinfloe/waybar/` (`config.jsonc`, `style.css`) - a
  single `wlr/taskbar` module, launched by `thinfloe-session`. It exists
  because labwc draws an iconify button and, with zero keybindings and no
  window list, a minimized window would otherwise be unreachable for the rest of
  the session. Kept outside `$HOME` for the same reason `rc.xml` is.
  Note a fullscreen ThinLinc client covers the bar - wlroots puts
  fullscreen surfaces above the layer-shell top layer and ignores exclusive
  zones - so it is visible exactly when no window is fullscreen, which is
  when a window can go missing.
- Tiles: the shipped tiles are `.desktop` files in `overlay/etc/thinfloe/menu/`,
  outside `$HOME` for the reason `rc.xml` is. Per-machine tiles go in
  `~/.config/thinfloe/menu/*.desktop`: at most 32 files, and they may only `run`
  a program. `X-TF-Multiple=true` lets a run tile start again while a
  copy of it still runs (ThinLinc, oxulnk and Terminal set it); without it a
  tile runs once at a time. A user file rewording a shipped tile cannot set
  it. A user file named like a shipped one may reword `Name`, `Comment`
  and `Icon` only; its other keys are ignored. The files travel with the home
  directory through `save-home`.
  A user file that matches no shipped file and sets neither `Exec` nor
  `X-TF-Action` is not drawn; the banner names it. That is what a reword
  of the old `80-logout.desktop` became when Restart and Power off replaced
  it, and it used to draw a grey "has no Exec" tile.
  `Icon=` is user data the menu parses on every boot, so each icon renders
  in a child process, `thinfloe-menu --render-icon`, one at a time, under
  `RLIMIT_AS` 256 MiB, `RLIMIT_STACK` 256 MiB, `RLIMIT_CPU` 3 s,
  `RLIMIT_FSIZE` 0 and `RLIMIT_NOFILE` 16. The menu kills and reaps it
  after 2 s, or when the 5 s for all icons are used up. Any failure draws
  the tile's first letter. In-process guards cannot bound memory: a
  700-byte SVG of nested `<pattern>`s asked resvg for 2.88 GB, and the
  thread the deadline abandoned kept allocating inside the menu; three
  such tiles reach about 8.6 GB, enough for the OOM killer to pick the
  menu on a machine that holds its rootfs in RAM. A renderer dies with
  the menu (`PR_SET_PDEATHSIG`), and `RLIMIT_CORE` is 0, so an aborted
  render leaves no core file in the home. A release renderer needs 64 MiB
  for every legitimate icon measured, and the debug build (the tests)
  needs 192 MiB. Before
  rendering, the child still refuses an SVG that nests deeper than 64
  elements, has more than 10000, declares entities or uses filters (one
  `feTurbulence` ran for over a minute). Limits and guards are in
  `src/thinfloe-menu/src/icons.rs`, and `src/thinfloe-menu/tests/render_icon.rs`
  renders through the real binary.
- Session startup: `overlay/usr/bin/thinfloe-session`, `overlay/etc/profile.d/10-thinfloe-session.sh`
- **Per-machine user customisation:** `overlay/home/tluser/.thinfloe-sessionrc` — the Wayland replacement for `~/.xsessionrc`. Sourced by `thinfloe-session` after the home restore and before the menu, and so before any tile starts `tlclient`. Because it lives in `$HOME` it is captured by `save-home` and restored each boot, so a user can configure an individual machine without rebuilding the image. This is where display layout goes: **`wlr-randr` replaces `xrandr`** (`--output DP-1 --transform 90 --pos 1920,0`), and `kanshi` is available for layouts that must survive hotplug or DPMS wake. It must run before `tlclient`, because the client reads the monitor layout once at startup.
  It is also where screen blanking is tuned: `TF_IDLE_TIMEOUT=<seconds>` (default `300`, `0` disables blanking entirely) is read by `thinfloe-session` right after this file is sourced.

- **Per-machine user environment:** `overlay/home/tluser/.thinfloe-environment` — variables the compositor reads at **startup**: keyboard layout (`XKB_DEFAULT_*`) and cursor theme. Sourced by `overlay/etc/profile.d/10-thinfloe-session.sh` after `/etc/thinfloe/labwc/environment`, so the user's value wins. Also persisted via `save-home`.

  | variable | default | effect |
  | --- | --- | --- |
  | `LABWC_FULLSCREEN_SPAN_OUTPUTS` | `1` | fullscreen Xwayland windows (the ThinLinc client) span every monitor |

  A per-machine override in `~/.thinfloe-environment` **works**, including for
  keys also set in `/etc/thinfloe/labwc/environment` - `LABWC_FULLSCREEN_SPAN_OUTPUTS`
  above and the pre-existing `XKB_DEFAULT_*` / `XCURSOR_*` keyboard and cursor
  overrides. This is not as simple as it looks: labwc's
  `session_environment_init()` (`src/config/session.c:77`) calls
  `setenv(key, value, 1)` - overwrite - and, run with `-C`, treats
  `<config_dir>/environment` as the *only* environment file it reads
  (`src/common/dir.c:153-157`, called from `src/main.c:211`). Pointing `-C`
  straight at `/etc/thinfloe/labwc` therefore let labwc's own re-read reset any
  key also present in the user's file back to the system value, silently
  discarding what `10-thinfloe-session.sh` had just exported into the shell.
  The fix, `overlay/usr/bin/thinfloe-compose-labwc-config`, composes a
  merged config directory at session start - the system dir's files
  symlinked in unchanged, plus an `environment` file that is the system
  defaults followed by the user's file - and points `-C` at that instead, so
  the file labwc re-reads already has the user's values last (and therefore
  winning, since labwc's `setenv` on a later duplicate overwrites the
  earlier one - `test/runtime/test-labwc-runtime.sh` asserts this against
  the real parser). If `XDG_RUNTIME_DIR` is unset/unwritable or the merge
  fails, the session falls back to the plain `/etc/thinfloe/labwc` (today's
  behaviour) rather than failing to start. This is proven by
  `test/integration/test-labwc-config-merge.sh` (the merge logic, offline)
  and `test/runtime/test-labwc-runtime.sh` (the parser assumption, against
  real labwc).

  **Confirmed on hardware (2026-07-27):** an `XKB_DEFAULT_*` override in
  `~/.thinfloe-environment` takes effect, which is what the merge exists to
  make possible - before it, that documented feature had never worked. The
  session also starts normally, i.e. the merged config dir is used without
  needing its fallback. Not separately exercised: setting
  `LABWC_FULLSCREEN_SPAN_OUTPUTS=0` to get single-monitor fullscreen. It
  travels through the same merge, so it is expected to work, but it has not
  been observed.

  **The two user files are split by timing, and it is not arbitrary:**

  | file | sourced | for |
  | --- | --- | --- |
  | `~/.thinfloe-environment` | before the compositor starts | variables read at startup — keyboard, cursor |
  | `~/.thinfloe-sessionrc` | inside the running session | commands needing a compositor — `wlr-randr`, `kanshi`, netplan |

  This is why the home restore happens in the **initramfs** rather than
  inside the session: a user's saved `.thinfloe-environment` has to be on disk
  before the compositor reads `XKB_DEFAULT_*`. Restoring home inside the
  session would make a user keyboard change take effect only on the *next*
  boot. Do not move the restore into `thinfloe-session`, and do not move it
  back into `10-thinfloe-session.sh` either — `thinfloe-ssh-hostkeys` now needs
  the home directory before `multi-user.target`.

  Note the deliberate split from system config: user-editable settings live in `$HOME`, but `overlay/etc/thinfloe/labwc/rc.xml` does **not** — see [Standing Constraints](#standing-constraints).
- Autologin: `overlay/etc/systemd/system/getty@tty1.service.d/autologin.conf`
- User defaults: `overlay/home/tluser/`

After modifying overlay files, rebuild with `make image`.

## Persistent Home Directory

The system implements home directory persistence across reboots:

- On boot: the initramfs restores `/home/tluser` — from `home.tar.gz` on the
  EFI partition when booting from USB, or from `home.pkg` on the boot server
  when netbooting. This happens in `restore_home()`
  (`overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh`), called from
  `setup-overlay.sh`, **not** at login. It moved there so the SSH host keys
  stored inside the home directory are available before sshd starts.
  Netboot identifies the machine by the MAC recorded at
  `/run/thinfloe/state/boot-mac` — the interface the initramfs actually used —
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
  on without asking. The menu greys the save tile out when
  `/run/thinfloe/state/home-restore` says `failed`; after a Save-On-Exit tile
  exits on such a boot, the save is skipped with a line in the menu's log.
- `thinfloe-save-home` resolves the directory to archive from `getent passwd
  tluser`, never from `$HOME`, and every read of a recorded value under
  `/run/thinfloe/state` ends in `|| true`. Both are load-bearing. `sudo` on this
  image sets `HOME=/root` (`Defaults env_reset`, no `env_keep` for HOME), so a
  save called from a root process archived `/root` over the user's home; and
  `set -e` ends a script on `X=$(cat missing)`, which disabled every save on
  USB boot, where `boot-home-base` is never written.
  `test/integration/test-save-home.sh` guards both.

- The remote-access password rides the same archive. `sudo thinfloe-password`
  stores the hash the system produced in `~tluser/.thinfloe-password`, mode 600
  and **owned by tluser**, like the rest of the home. Both save paths of
  `thinfloe-save-home` run tar as root, so a file tluser cannot read is still
  archived; until 2026-10 the netboot path ran tar as tluser, and one such
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

- `thinfloe-upgrade`'s fresh install can copy this machine's home onto the new
  drive: `home.tar.gz` on its ESP, addressed by device node
  (`find_efi_partition(device)`), never by partlabel - the new stick carries
  the same `EFI-SYSTEM` label as the running one. It uses the save-home
  exclude list and **always** leaves out `~/.thinfloe-ssh-host-keys` and
  `~/wg0.conf` (`IDENTITY_EXCLUDES`, outside the user-editable list). Those
  are machine identities: two machines would share one SSH host key or one
  WireGuard private key, which breaks both tunnels. `~/.thinfloe-password`
  travels on purpose. Like `thinfloe-save-home`, it refuses when this boot's
  restore failed (`/run/thinfloe/state/home-restore` is `failed`), since the
  live home is then a default one. `test/unit/test_upgrade_image.py` guards
  this, run via `make test-unit`.
- Before the fresh install asks to erase the drive, it compares the drive
  (`lsblk -bdno SIZE`) with the image's uncompressed size, read from the
  zstd frame header of `thinfloe-usb.img.zst` (`curl -r 0-17`), and stops when
  the drive is smaller: `dd` onto such a stick failed partway and left it
  unbootable. When the size cannot be learned (no range support, a stream
  compressed from a pipe) it warns and goes on.

This allows WiFi credentials, ThinLinc settings, and user customizations to persist.

## Network Boot vs USB Boot

The boot method follows from `ramroot=` on the kernel command line
(`is_remote_url` in `thinfloe-lib.sh`):

- **USB Boot**: a path such as `tl/ramroot.sqsh` → loaded from the
  `EFI-SYSTEM` partition, home persisted to that partition
- **Network Boot**: an `http://` URL → downloaded into RAM, home persisted
  to the boot server over HTTP

Both modes execute identical code paths after SquashFS mount.

Netboot networking in the initramfs is dracut's `systemd-networkd` and
`systemd-resolved` modules (`--add` in `containers/ubuntu/Dockerfile`).
`parse-thinfloe.sh` sets `rd.neednet=1` for an http ramroot, so
`systemd-networkd-wait-online` runs before the hooks; USB boots do not set
it and do not wait. dracut 110 has no `network-legacy` module, and the
hook's former `dhclient` call never worked - the initramfs had no
`dhclient-script`, so every netboot failed with `Download failed` until
2026-10. `mount-squashfs.sh` records the boot MAC from the interface holding
the default route, i.e. the one whose lease was applied, not from the first
`/sys/class/net` entry. A host name in the ramroot URL resolves through the
resolved stub. Cost: networkd pulls in `kernel-network-modules` (every NIC
driver), and the initrd grew from 165.6 MB to 179.3 MB.
`dracut-network` stays: it ships the `systemd-networkd` and `url-lib` dracut
modules. `isc-dhcp-client` is not installed: nothing in the image runs
`dhclient` (the running system's netplan renders for networkd), and nothing
depends on the package. Dropping it took 328 KiB off `ramroot.sqsh`.

`make qemu-smoke-netboot` boots the artifacts from a local HTTP server with
`ramroot=http://boot.thinfloe.test:<port>/ramroot.sqsh` and requires the
hostname `thinfloe-123456` for MAC 52:54:00:12:34:56, a `home.pkg` request under
that MAC, a query for `boot.thinfloe.test` at the test's DNS server and a
download of `ramroot.sqsh`. That name exists only in `scripts/smoke-dns.py`,
a stdlib one-name DNS server, so a boot that downloads the ramroot used the
DNS server from its DHCP lease: resolved forwards to QEMU's 10.0.2.3, and
slirp forwards that to the first `nameserver` in `/etc/resolv.conf`. To keep
this offline and unprivileged, `scripts/run-qemu-netboot-smoke.sh`
re-executes itself under `unshare -rmn` (user, mount and network namespace,
loopback only), bind-mounts a `resolv.conf` saying `127.0.0.1` there, and
runs the HTTP server and `smoke-dns.py` on that namespace's loopback, which
slirp's host address 10.0.2.2 reaches. The test therefore needs unprivileged
user namespaces on the host. With the name left unanswered the boot stops at
`dracut: FATAL: Download failed`. **Not verified on hardware.**

## Container Build Best Practices

The containers/ubuntu/Dockerfile follows several important patterns:

1. **SSH Host Keys**: Host keys are removed at build time. On first boot
   `thinfloe-ssh-hostkeys.service` generates them and stages them into
   `~tluser/.thinfloe-ssh-host-keys`, from where `thinfloe-save-home` captures them
   into the home archive at logout; the initramfs restores them on every
   subsequent boot. `ssh.socket` is **masked** — see Standing Constraints.
   Keys do not persist until the first clean logout: a machine hard-powered
   off before then generates fresh keys next boot.

2. **Initramfs Generation**: The build uses Dracut (`dracut --force --no-hostonly --add "thinfloe plymouth"`) to create initramfs. The `--no-hostonly` flag is mandatory - see [Standing Constraints](#standing-constraints). The custom module in `overlay/usr/lib/dracut/modules.d/90thinfloe/` is automatically included. This must run AFTER overlay files are applied and SSH keys are removed.

3. **Systemd Service Management**: Services are explicitly enabled/disabled during build to control first-boot behavior. The `un-dockerize.service` runs once and disables itself.

4. **Locale Generation**: `locale-gen` must run after overlay files are applied to process locale.gen configuration.

5. **User Configuration**: The default user (tluser) is created with no password but added to sudo group with NOPASSWD privileges for management tasks.

## Performance Optimizations

### SquashFS + OverlayFS + ZRAM Architecture

The system uses a layered approach for optimal memory usage:

- **SquashFS**: Compressed read-only root filesystem (zstd compression)
- **ZRAM**: Compressed RAM device for writable overlay layer
- **OverlayFS**: Combines SquashFS (lower) with ZRAM (upper) for a writable system

This architecture significantly reduces memory pressure compared to extracting the entire rootfs to RAM.

### machine-id Persistence

The system persists systemd's machine-id across reboots by storing it in `/config/machine-id` on the EFI partition. This is important because:

- Systemd requires a stable machine-id for proper service operation
- Without persistence, services like journald, networkd, and DHCP may behave unexpectedly
- The machine-id is generated once on first boot and reused thereafter

The EFI partition `/config/` directory can also store other persistent configuration.

## Standing Constraints

Sixteen rules in this repository look like ordinary configuration but are
load-bearing. All but the patched-labwc, hook-order, menu-rendering,
logout-save and old-names ones have each caused a real shipped-image bug. The
patched-labwc one is preventive - nothing has shipped broken from it yet, but
reverting it silently would ship regressions in both patched behaviours. The
hook-order one was caught in QEMU: the race it closes is present in every
image built before it. The menu-rendering and logout-save ones are preventive
too: the menu-rendering and logout-save ones were decided while the menu was
designed, and no image has shipped without them. The old-names one is
preventive as well: nothing has shipped broken from it.

### No login password ships in the image

`containers/ubuntu/Dockerfile` creates tluser with `adduser
--disabled-password` and nothing else. There used to be an `echo
"tluser:tluser" | chpasswd` on the next line. Re-adding any shipped password
gives every machine in the field the same login, and because tluser has
`NOPASSWD:ALL` two lines below, that login is root: anyone who could reach
port 22 and knew the default owned every deployed machine. It also
contradicted `overlay/usr/bin/thinfloe-session`, which already documented
"tluser has no password" as the reason the session has no lock screen.

`passwd -l` is **not** a substitute for leaving it unset. It prefixes the
existing hash with `!` and leaves it recoverable. The shadow field must hold
`!` alone.

Root is locked with `passwd -d root && passwd -l root`. `-d` on its own
leaves the field *empty*, and Ubuntu's `/etc/pam.d/common-auth` carries
`nullok`, so root logged in at a VT by pressing Enter. sshd refused it only
because `PermitEmptyPasswords` defaults to no, which kept the hole invisible
from the network. `PermitRootLogin no` is now set as well, for the day
somebody gives root a password.

`PasswordAuthentication yes` stays. It cannot succeed while no password is
set, and the user needs it once they set one; the image ships no
`authorized_keys`, so turning it off leaves no remote access at all.

A person at the machine sets their own password with `sudo thinfloe-password`.
See [Persistent Home Directory](#persistent-home-directory) for how it
survives a reboot. `test/integration/test-session-packages.sh` guards the
shadow fields, both sshd settings and the unit, and
`test/integration/test-password.sh` guards the script - both run via `make
test`.

**Verified on a QEMU boot (2026-10-01):** `getty@tty1` started exactly once
across a 300 s boot, so `login -f tluser` succeeds against a `!` field. A
failing autologin would have restarted it every 5 seconds
(`RestartSec=5`, `StartLimitIntervalSec=0`). Not verified on hardware.

### Firmware is selected by package, never by cleanup

`scripts/export-rootfs.sh` once ran
`find /usr/lib/firmware -type f -not -name "iwlwifi*" -delete` before
`mksquashfs`. It silently deleted i915 GPU firmware (wedged GPUs in the
field) and `intel-ucode` (no CPU microcode updates, ever) from every image.
The `-type f` predicate also does not match symlinks, and a large fraction
of `/usr/lib/firmware` is symlinks - roughly 93 of them pointed at files the
same `find` had just deleted, so the sweep also left the tree full of
dangling firmware symlinks with no missing-file error to flag it, a symptom
that could otherwise be misdiagnosed as something else entirely.

Ubuntu 26.04 splits `linux-firmware` per vendor, and `linux-firmware-minimal`
satisfies `linux-image-generic`'s hard dependency via `Provides` while only
*recommending* the rest. The installed set is therefore exactly what
`containers/ubuntu/Dockerfile` lists, given `--no-install-recommends`.

Never add a `find`/`rm -rf` sweep over `/usr/lib/firmware`. To change the
firmware set, change the package list. `test/integration/test-firmware.sh`
guards this - run via `make test`.

### The WiFi core stack must come from the kernel, never from a backport

`containers/ubuntu/Dockerfile` installs `linux-image-generic` and **not**
`linux-modules-iwlwifi-generic`. Re-adding that package silently breaks WiFi
for every wireless device in the image, not just Intel ones.

Despite the name, `linux-modules-iwlwifi-generic` is not an Intel driver
add-on. It ships its own `cfg80211.ko` and `mac80211.ko` - the shared core of
the Linux WiFi stack - into `/usr/lib/modules/<ver>/ubuntu/dkms/iwlwifi/`.
Ubuntu's depmod order is `search updates ubuntu built-in` (`/lib/depmod.d/`),
so `ubuntu/` outranks `kernel/` and the backport copies win. Measured on the
last image built with it (kernel 7.0.0-29): `modules.dep` resolved
`mac80211` and `cfg80211` *only* to `ubuntu/dkms/iwlwifi/`, and
`rtw89_core.ko` - the Realtek USB driver - depended on Intel's backported
core. The in-tree copies were still on disk, fully shadowed. Two copies of
`mac80211`/`cfg80211` in one image is what produces `ieee80211_*` symbol
mismatches at module load.

It entered in `2785aea` ("upgrade to ubntu 24.04", 2024-11-14), in the same
hunk that moved the base from 22.04 to 24.04, with no stated rationale, and
was carried verbatim through the SquashFS rewrite (`7e33c00`) into 26.04.
On 6.8 it was defensible - that kernel needed backported Intel support.
Kernel 7.x ships `iwlwifi`, `iwldvm`, `iwlmvm` and `iwlmld` in-tree, so the
backport adds no hardware coverage and only creates the conflict.

Two traps when debugging this in the field, both of which make a genuinely
affected machine look clean:

- the directory is `ubuntu/dkms/iwlwifi/`, **not** `updates/`, so the usual
  `find /lib/modules/$(uname -r)/updates` finds nothing;
- `dkms status` is empty too - despite the path name it is a prebuilt binary
  package, not a DKMS build.

Nothing depends on the package (`apt-cache rdepends` is empty, and
`linux-image-generic` has no relationship to it), so the package list is the
only thing keeping it out. `test/integration/test-wifi-stack.sh` guards
this - it asserts the backport directory is absent, the four in-tree Intel
drivers are present, and `modules.dep` resolves exactly one `mac80211` and
one `cfg80211`, both under `kernel/`. Run via `make test`.

**Confirmed on hardware (2026-10-01):** with the backport gone, an Intel 8265
associates. The ThinLinc 4.21.0 client also reads the existing
`tlclient.conf` unchanged, `HOST_ALIASES` and
`FULL_SCREEN_SELECTED_MONITORS` included.

### dracut must be invoked with --no-hostonly

`containers/ubuntu/Dockerfile` passes `--no-hostonly` to dracut. Removing it
silently breaks the initramfs.

dracut 110 defaults `hostonly` to `-h` unless told otherwise
(`/usr/bin/dracut:1361`). Because `podman build` shares the host kernel, that
makes every `instmods` call filter against **the build machine's** loaded
modules rather than the target hardware's. Measured on this repo: the shipped
initramfs went from 683 modules to 955 once the flag was added - zram, e1000,
e1000e, igb, r8169, atlantic and iwlwifi had all been silently dropped. ZRAM's
absence aborts the boot outright; the missing NIC drivers would have broken
network/PXE boot. (An isolated probe build without `--omit-drivers` showed 968;
955 is the real figure for the shipped flag set.)

It also made builds non-reproducible: the initramfs varied with whatever the
build machine happened to have loaded. `overlay` survived only because podman's
storage driver keeps overlayfs loaded.

The older dracut on 24.04 defaulted to generic, which is why the existing
comment ("Keep it generic (no --hostonly)") was true when written and became
false on dracut 110. `--add-drivers` is not a substitute: it bypasses the
filter for its own arguments only (`dracut:2768`), leaving every module
requested by `90thinfloe/module-setup.sh` still filtered.

### labwc must have zero keybindings

`overlay/etc/thinfloe/labwc/rc.xml` has a `<keyboard>` section with no
`<default />` and no `<keybind>` entries. This is mandatory. The config
directory lives at `/etc/thinfloe/labwc`, not under `$HOME` - see
[Persistent Home Directory](#persistent-home-directory) for why - and is
selected via `labwc -C /etc/thinfloe/labwc` in
`overlay/etc/profile.d/10-thinfloe-session.sh`.

The reason is *not* the missing `zwp_keyboard_shortcuts_inhibit_manager_v1` -
that was this repo's earlier explanation and it is wrong. Xwayland's only use
of that protocol is `maybe_fake_grab_devices()`, which returns immediately
when the server is rootless, and Xwayland is always rootless under labwc.

Rootless Xwayland instead forwards X11 grabs via
`zwp_xwayland_keyboard_grab_manager_v1`, which upstream labwc and every
wlroots compositor lack. Our labwc carries a patch implementing it
(`containers/ubuntu/patches/0002-xwayland-keyboard-grab.patch`), so a
keybinding *would* now be suspended while the ThinLinc client holds the
keyboard.

Keybindings nonetheless stay at zero until that path has been confirmed on
hardware with a real ThinLinc session. `vncviewer` calls `XGrabKeyboard`, but
the headless test only proves Xwayland *binds* the manager global - the rig
supplies no input devices, so `xwl_seat->keyboard` is never created and
Xwayland never installs the hook that would send a `grab_keyboard` request in
the first place (see the comment in `test/runtime/test-labwc-runtime.sh`).
Whether a real grab actually suspends keybindings for the session's duration
is therefore unverified and can only be confirmed on hardware with a keyboard
attached. Adding a keybinding is now a config decision rather than an
impossibility - make it deliberately, and re-run `make test-runtime` after.

`test/integration/test-session-packages.sh` guards the zero-keybindings
assertion on `rc.xml` itself - run via `make test`.

### ssh.socket must stay masked

`containers/ubuntu/Dockerfile` runs `systemctl disable ssh.socket` then
`systemctl mask ssh.socket`, and enables `ssh.service` alone. Re-enabling the
socket reintroduces an ordering cycle.

`thinfloe-ssh-hostkeys.service` must run before sshd. With the socket active,
sshd has two entry points with different ordering — the socket from
`sockets.target`, the service from `multi-user.target` — so the key unit had
to be ordered `Before=ssh.socket`. But a `DefaultDependencies=yes` unit is
implicitly ordered `After=basic.target`, and `basic.target` comes after
`sockets.target`, which comes after `ssh.socket`. Before and after the same
unit is a cycle, and systemd resolves a cycle by deleting a job — which job
is not contractual: under QEMU it deleted `sockets.target` and sshd came up
fine, while on real hardware it deleted the key-generation service, leaving
no host keys and no sshd.

`disable` before `mask` is required, not stylistic. Ubuntu's `ssh.socket`
declares `RequiredBy=ssh.service` in its `[Install]` section, so once it is
enabled, `/etc/systemd/system/ssh.service.requires/ssh.socket` exists.
Masking the socket while that symlink is still there makes `ssh.service`
fail outright with "Unit ssh.socket is masked" — `disable` removes the
symlink (and the `sockets.target.wants` one) first; `mask` then stops
anything from re-enabling it. Doing `mask` first would leave the requires
symlink in place, since `disable` is a no-op on an already-masked unit.

`sshd-keygen.service` must ALSO be masked, for a reason that's easy to
miss: it ships enabled via `Wants=` from **both** `ssh.socket.wants/` and
`ssh.service.wants/`, so disabling `ssh.socket` only removes the first of
those two symlinks — the second survives on its own. Left alone, it races
`thinfloe-ssh-hostkeys.service` on a genuine first boot: its
`ConditionFirstBoot=yes`, and neither unit is ordered against the other,
both only declaring `Before=ssh.service`. If `sshd-keygen.service` wins, it
writes keys straight into `/etc/ssh`; `thinfloe-ssh-hostkeys.service` then
finds live keys already present and skips generation, so those keys are
never staged into the keystore and are lost on the next boot — reproducing
the regenerate-every-boot bug this whole mechanism exists to fix. It is only
a `Wants=` (soft) dependency of `ssh.service`, so masking it cannot break
sshd startup the way masking `ssh.socket` alone would have.

`test/integration/test-session-packages.sh` guards all three — the
`ssh.socket` mask, the absence of `ssh.service.requires/ssh.socket`, and the
`sshd-keygen.service` mask — run via `make test`.

### A multi-user.target unit must never declare Before= on a sysinit.target unit

A unit that is only `WantedBy=multi-user.target` (no explicit
`DefaultDependencies=` or `Before=`/`After=` of its own) is implicitly
ordered `After=basic.target`, which is itself ordered after
`sysinit.target`. Giving such a unit `Before=<something in sysinit.target>`
therefore orders it both before and after the same target, and systemd
resolves the resulting cycle by silently deleting one of the jobs — on every
single boot, not just the first.

Two real instances of this bug have shipped in this repo:

- The predecessor `regenerate_ssh_host_keys.service` (deleted on this
  branch, replaced by `thinfloe-ssh-hostkeys.service`) was ordered
  `Before=ssh.socket` (which pulls in `sockets.target`, itself before
  `basic.target`) — see "ssh.socket must stay masked" above for the full
  cycle and its fix.
- `un-dockerize.service` was ordered `Before=systemd-hwdb-update.service`
  (part of `sysinit.target`). systemd broke the cycle by deleting the hwdb
  update job on every boot; nothing `un-dockerize.service` does (resolv.conf,
  `/etc/hosts`, `systemd-resolved`) has anything to do with the hardware
  database — the `Before=` was vestigial, left over from commented-out
  `depmod`/`modprobe` lines. It has been removed. Because this image's
  `usr/lib/udev/hwdb.bin` ships prebuilt (confirmed via `unsquashfs -l
  ramroot.sqsh | grep hwdb.bin`), `systemd-hwdb-update.service` is now also
  masked in `containers/ubuntu/Dockerfile` alongside the other `systemctl
  mask` calls, so removing the ordering cycle doesn't newly spend a boot
  regenerating a database the image already has.

`scripts/check-boot-smoke.sh` guards this class of bug directly: it fails
the smoke test on any `Found ordering cycle` line in the boot log, not just
on the two known instances. `test/integration/test-session-packages.sh`
additionally asserts `un-dockerize.service` has no
`Before=systemd-hwdb-update` line — both run via `make test`.

### labwc is a local rebuild, not the archive package

`containers/ubuntu/Dockerfile` builds `labwc_0.9.3-1+thinfloe1` from Ubuntu's
source package with the patches in `containers/ubuntu/patches/`, applied as a
quilt series. Two behaviours depend on it:

- fullscreen Xwayland windows span every output
  (`LABWC_FULLSCREEN_SPAN_OUTPUTS`), which is what makes the ThinLinc client
  fill both monitors;
- `zwp_xwayland_keyboard_grab_manager_v1` exists, so X11 keyboard grabs
  suspend compositor keybindings.

Reverting to the archive `labwc` silently loses both. On a labwc or wlroots
version bump, expect to rebase the patches; `dpkg-buildpackage` fails loudly
when a hunk no longer applies, and `make test` plus `make test-runtime` cover
the rest. `_NET_WM_FULLSCREEN_MONITORS` was investigated and rejected - see
the design spec for why it is unnecessary.

`containers/ubuntu/patches/*.patch` are listed as prerequisites of the
`.ubuntu-container` target (`PATCH_FILES` in the `Makefile`). Editing a patch
without that dependency would not trigger a rebuild, leaving `make test` and
`make test-runtime` validating a stale image while still reporting green.

The same false green has a second source: the image tag. `.ubuntu-container`
belongs to one checkout, but every checkout builds `thinfloe-ubuntu:$(VERSION)`,
so another checkout's build replaces the image under this checkout's stamp.
Each container stamp therefore holds the image ID its build produced, and the
`Makefile` drops it at parse time when the tag names a different image or
none. Build with a distinct `VERSION` per checkout, or two checkouts rebuild
over each other. `test/integration/test-container-stamp.sh` guards this - run
via `make test`.

### Per-machine network config is installed by the initramfs, never by a unit

The netplan systemd generator creates `netplan-wpa-<if>.service` when the
manager starts. A `~/wifi.yaml` or `~/wg0.conf` copied into place after that
never gets a WPA unit, and WiFi never associates. `install_local_network()`
in `thinfloe-lib.sh`, called from `setup-overlay.sh` after `restore_home`, does
the copy (and creates the `wg-quick@wg0` wants link) before switch-root, so
the generator sees the files. Netplan files are mode 0600 through a `chmod`
in `containers/ubuntu/Dockerfile`, because git stores only 644 and 755.
Do not move the copy into a `thinfloe-local-network.service`.
`test/integration/test-field-report.sh` asserts that unit does not exist and
`test/integration/test-session-packages.sh` asserts mode 600 in the image.

### The initramfs has no cut, basename, head, install, wc or date

Only what the `inst_multiple` lines in `90thinfloe/module-setup.sh` install,
plus dracut's base set, exists there. Shell builtins (`printf`, `test`,
`read`) are fine. A missing command fails silently at boot while every
offline test passes on the dev host - that is how the hostname shipped as
`thinfloe`, why netboot never recorded the boot MAC, and why every `sync` in
the `tl.new` upgrade rotation failed. `sync` and `rmdir` are now installed
explicitly for that rotation. `dhclient` is no longer installed: DHCP is
`systemd-networkd`'s job (see [Network Boot vs USB Boot](#network-boot-vs-usb-boot)).
`test/integration/test-initramfs-home.sh` runs
the helpers, `tf_finalize_upgrade` included, with PATH restricted to that
set. `test/integration/test-initramfs-commands.sh` checks every command the
90thinfloe hooks call against `lsinitramfs` of the built `initrd.img`, so a new
call to a missing tool fails `make test` even where no offline test runs it.
Its command extractor (`test/integration/initramfs-commands.py`) is a
heuristic: it skips comments, quoted text, `for` lists and `case` patterns,
and descends into `$( )`; a command it cannot see is not checked.

### The ThinFloe pre-mount and mount hooks run after dracut-cmdline

`90thinfloe/module-setup.sh` installs `thinfloe-after-cmdline.conf` as a drop-in
for `dracut-pre-mount.service` and `dracut-mount.service`
(`After=dracut-cmdline.service`). Upstream orders both only after
`dracut-initqueue.service`, which a ThinFloe boot does not pull in, so they ran
in parallel with the cmdline hook that sets `root=thinfloe` and writes
`/tmp/thinfloe-ramroot`. A boot that lost that race logged `Can't mount root
filesystem` and stopped in the emergency shell; QEMU netboot hit it, and the
USB smoke log showed pre-mount starting before cmdline finished. USB was
spared only by the time the pre-mount hook spends waiting for the ESP.
`test/integration/test-initramfs-commands.sh` asserts both drop-ins are in
the built initrd, and `scripts/check-boot-smoke.sh` fails a boot log in which
either hook starts before `dracut-cmdline.service` has finished.

### /etc/hostname is excluded from the squashfs and written by the initramfs

podman bind-mounts `/etc/hostname` in the build container, so `mksquashfs`
packed the container ID (`fa0ad0f31bfa`) and every machine had the same name.
`tf_write_hostname` writes `thinfloe-` plus the first six hex digits of the
persisted machine-id on USB, or the last six hex digits of the boot MAC on
netboot, where the machine-id is new every boot. It runs before switch-root,
so PID 1 reads the name and `%H` in `un-dockerize.service` is correct.
`test/integration/test-field-report.sh` guards the exclusion and the call.

### thinfloe-menu draws on the CPU

The crate has no `wgpu`, `glow` or `eframe`. The target images have no Vulkan
ICD (`/usr/share/vulkan/icd.d` does not exist), and GL would make the menu's
start depend on EGL. `test/integration/test-session-packages.sh` fails when
the binary links or names a GPU library.

### thinfloe-session never saves; thinfloe-menu saves before it exits 0, 10 or 11

Decided 2026-10-02, so a failed save is shown at the machine and answered
there (Stay / Log out anyway, Restart anyway, Power off anyway). 0 is log
out, 10 restart and 11 power off (decided 2026-10-08, when Restart and Power
off replaced the Log out tile); `thinfloe-session` runs `sudo systemctl reboot`
or `poweroff` for them and shows a window if that fails, and
`session-verdict` counts all three as a clean end. Failure is the default
branch of the status handling in `thinfloe-session`: a panic (101), a segfault
(139) or a failed exec (126/127) read as logout would be silent, and saving
on them would archive the home on every respawn.
`test/integration/test-session-lifecycle.sh` guards both.

### The first boot after an upgrade takes its kernel from tl.new

`thinfloe-upgrade` stages an upgrade of the running stick in `tl.new/` and then
sets `DEFAULT new` in both `syslinux.cfg` copies (`/syslinux.cfg` and
`/efi/boot/syslinux.cfg`; it adds `LABEL new` to a stick written before the
entry existed). `LABEL new` boots `/tl.new/vmlinuz` with
`ramroot=tl.new/ramroot.sqsh`. On that boot `tf_finalize_upgrade`
(`thinfloe-lib.sh`) sets `DEFAULT current` again, rotates `tl` → `tl.old` and
`tl.new` → `tl`, and points `/tmp/thinfloe-ramroot` at `tl/ramroot.sqsh`. It
rotates **only** when ramroot= is `tl.new/ramroot.sqsh`. The rotation has to
happen in the initramfs, because the squashfs is mounted from the stick and
cannot be moved afterwards.

The rotation used to run on a boot that had loaded the kernel from `tl/`. The
old kernel (7.0.0-34) then ran on the new squashfs (modules for 7.0.0-38), and
every module not already in the old initramfs failed to load: an i226-V NIC
(`igc`) stayed down until the next reboot, while WiFi (`iwlwifi`, in the
initramfs) worked. Seen in the field 2026-10-07.

Limits: the upgrade runs the *running* image's `thinfloe-upgrade`, so a machine
on an image from before this fix has the mismatch once more on its next
upgrade. Staging such an older image with the new `thinfloe-upgrade` (a
downgrade) boots it from `tl.new/`, where its initramfs rotates
unconditionally and then fails to find `tl.new/ramroot.sqsh`; type `current`
at the boot prompt, and set `DEFAULT current` in both copies by hand.

A stick holds at most two releases. One release (`vmlinuz`, `initrd.img`,
`ramroot.sqsh`) was 775 MB in 2026-10, and the ESP was 2000 MB: with
`tl.old/` still present, staging `tl.new/` needed room for three and failed.
`thinfloe-upgrade` therefore removes `tl.old/` before it stages `tl.new/`;
while an upgrade is staged the fallback is `tl/`, the `current` entry. Before
it removes anything it checks that free space plus `tl.old/` and `tl.new/`
holds the release plus 10 %, and stops with "Nothing was changed" if not.
New images get a 3000 MB ESP (`EFI_PARTITION_SIZE`): two releases, two home
archives during a save, and room to grow. A fresh install therefore needs a
stick of 4 GB or more; sticks already in the field keep their 2000 MB.

`test/unit/test_upgrade_image.py` (via `make test-unit`) and
`test/integration/test-initramfs-home.sh` guard both halves, and `make
qemu-smoke-upgrade` boots a staged upgrade through OVMF and syslinux and
requires the rotation and `DEFAULT current` afterwards.

### Old home-file names stay as links while the migration code exists

A 3.x home archive holds `~/.dbrrg-environment`, `~/.dbrrg-sessionrc`,
`~/.dbrrg-password`, `~/.dbrrg-ssh-host-keys` and `~/.config/dbrrg`. The
restore (`tf_migrate_home_names` in `thinfloe-lib.sh`, list in
`TF_HOME_RENAMES`) gives them the 4.0 names `.thinfloe-environment`,
`.thinfloe-sessionrc`, `.thinfloe-password`, `.thinfloe-ssh-host-keys` and
`.config/thinfloe`, and logs `thinfloe: renamed ~/<old> to ~/<new>`.

- **The archive decides, not the disk.** The default home already ships
  `.thinfloe-environment` and `.thinfloe-sessionrc`, so "the new name exists"
  is always true. A real old entry in the archive replaces the new name,
  whether the archive lists the new name or not (see "A file written on 3.x
  wins" below). The new name wins when the archive lists it and the old name
  is missing or a symlink. An old name that is a symlink is what an earlier
  4.0 boot left behind and is not touched.
- **The old names stay as relative symlinks** (`tf_link_old_home_names`, on
  every boot and every restore outcome). After the first 4.0 boot the
  previous release sits in `tl.old/` and the boot entry `LABEL previous`
  starts it; 3.x reads the old names and would otherwise find a default
  keyboard, no password and fresh SSH host keys. The links live in the home,
  so every save carries them.
- **A symlinked `~/.config` is not followed.** Renaming or linking
  `.config/dbrrg` through it would change the directory it points to, so the
  entry is skipped with the warning `thinfloe: ~/.config is a symlink,
  leaving ~/.config/dbrrg alone`.
- **The marker** `/run/thinfloe/state/home-renamed` lists one old name per
  line, and only when a real 3.x file was renamed. Nothing reads it yet; it
  is meant for the add-on auto-tick.
- **A failed listing fails the restore.** The restore lists the archive
  once, before extracting it, and the migration decides from that listing,
  so a boot decompresses the archive twice. When that listing is empty the
  archive is not extracted; when tar lists members but exits non-zero and the
  extraction still works, the rename is not attempted and the old names are
  not linked. Both leave `home-restore` at `failed`, so no save overwrites
  the stored archive with a half-migrated home.
- **The links are made at write time too.** `thinfloe-password --set` and
  `thinfloe-ssh-hostkeys` create `~/.dbrrg-password` and
  `~/.dbrrg-ssh-host-keys` when they write, owned by tluser, and `--clear`
  removes the link with the file. A home saved by 4.0 therefore always lists
  `.dbrrg-password` next to `.thinfloe-password`.
- **A password cleared on 3.x stays cleared.** 3.x's `dbrrg-password --clear`
  removes only the `~/.dbrrg-password` link and leaves the hash in
  `~/.thinfloe-password`. An archive that lists `.thinfloe-password` without
  `.dbrrg-password` is read as that clear: the restore removes
  `~/.thinfloe-password` and logs `thinfloe: ~/.dbrrg-password was removed by
  the previous version; clearing ~/.thinfloe-password`. If the reading is
  wrong, the user sets the password again; a revoked password never returns.
- **A file written on 3.x wins** (owner, 2026-10-23: the newer version of
  the dbrrg file wins). No 4.0 program leaves a real old name, so a real one
  beside the new name was written on a 3.x fallback that replaced the link:
  `dbrrg-password --clear` and then `--set`, or an editor that writes a new
  `~/.dbrrg-environment` and renames it over the link. When the archive
  lists both names and the old one comes out as a real file or directory,
  the restore removes the new one, moves the old one over it, logs
  `thinfloe: ~/<old> was written by the previous version; it replaces
  ~/<new>` and makes the link again. A directory is replaced whole, not
  merged. No `home-renamed` entry: the home went to 3.x and back.
  `_tf_move_over` moves the new name aside and removes it only after the
  move worked, so a failed `mv` loses neither.
- **Every save carries the links.** The two rules above read a missing or
  real old name in a 4.0 archive as something 3.x did. The 3.x default home
  ships a real `~/.dbrrg-environment` and `~/.dbrrg-sessionrc`: a 4.0 archive
  without the links (a user deleted one, an exclude pattern matched it) let
  that default survive a 3.x boot and replace the user's setting on the next
  4.0 boot, or cleared the password. `thinfloe-save-home` therefore, after the
  restore-failed check, stages the host keys, makes every missing old-name
  link whose new name exists (`TF_HOME_LINKS`, which must match
  `TF_HOME_RENAMES`) and drops each exclude pattern that leaves out a link
  but not its new name, logging `thinfloe: exclude pattern '<p>' ignored, it
  would leave out ~/<name>`. tar decides the match: `tar -cf /dev/null
  --no-recursion -v --exclude=<p> <names>` applies the exclude to
  command-line names and prints the ones kept. A real old name is left
  alone. `test/integration/test-save-home.sh` guards this and the two lists.
- `IDENTITY_EXCLUDES` in `thinfloe-upgrade` lists both host-key names, so an
  upgrade copy never carries either machine identity. 3.x's fresh install
  excludes only `./.dbrrg-ssh-host-keys`, which is now a link, and copies
  `.thinfloe-ssh-host-keys`: run from a fallback boot of 3.x, it gives the
  new stick this machine's SSH host keys. 3.x cannot be changed.
- A stick whose image predates 47c20bb (2026-10-07) upgrades with its old
  `upgrade-image`, which does not boot `LABEL new`, so its first ThinFloe
  boot runs the 3.x initramfs on the 4.0 squashfs: no rename, a default
  keyboard, no SSH password, fresh host keys, the host name `dbrrg-…`, and
  every save refused with exit 4 (the archive on the ESP stays intact). The
  next boot runs the 4.0 initramfs and migrates the home. The owner
  confirmed on 2026-10-23 that no stick in the field runs an image older
  than 2026-10-07.
- Still read, next to the new names: `rd.dbrrg.debug` (`rd.thinfloe.debug`),
  `X-DBRRG-Action`, `X-DBRRG-Save-On-Exit` and `X-DBRRG-Multiple` (the
  `X-TF-` key wins) and `DBRRG_IDLE_TIMEOUT` (`TF_IDLE_TIMEOUT` wins).
- Shipped tile files keep their file names (`50-upgrade-image.desktop`),
  because a user file rewording a shipped tile matches by file name.

Guards: `test/integration/test-initramfs-home.sh` (migration and links),
`test/integration/test-3x-fallback.sh` (frozen 3.x scripts in
`test/fixtures/3.x/` run against a 4.0 home), `test/integration/test-cmdline.sh`,
the tile tests in `src/thinfloe-menu/src/tiles.rs`,
`test/unit/test_upgrade_image.py` (`make test-unit`) and `make
qemu-smoke-upgrade`.

## Known Limitations

Open items found during the Ubuntu 26.04/Wayland/PipeWire upgrade. These are
not fixed; they are recorded so they aren't rediscovered from scratch.

### Menu limits

- The grid is on one monitor. The span patch covers Xwayland windows only.
- Clicking a tile is not covered by the headless runtime test, which has no
  input devices. The end-of-session dialogs and their buttons (Stay / Stop
  them and restart, Stay / Restart anyway, and the same for Power off and
  Log out) are proven by unit tests of the state machine only.
- Program output keeps its SGR colours (16, 256, RGB, bold); other escape
  sequences are removed. At most 64 colour runs per line.
- Full-colour theme icons with drop shadows draw the letter, because
  filters are refused: 15 of 70 Adwaita `scalable` and 53 of 425 hicolor
  `scalable` icons on a desktop host. Every Adwaita symbolic icon, the
  shipped Lucide set and the image's `foot.svg` render.
- Each icon costs one process start at menu startup, and the icons render
  one after another, so that at most one renderer holds its 256 MiB.
- A long log line is cut to one row on screen; the session log has it in
  full, up to the 1 KiB cut.
- **Confirmed on hardware (2026-10-07):** tiles start their programs,
  Save-On-Exit, Back up home and the logout Stay / Log out anyway dialog
  work. The square tiles and the log area have not been run on hardware.
- Not seen on hardware: two programs at once, Stop them and log out, the
  colours of a real `oxulnk-desktop` log, the menu staying behind a clicked
  program, the Restart and Power off tiles.

### Screen blanking had to be rebuilt after the X11 removal

Nothing blanked the screen between the move to Wayland and its fix. On X11 the
server did this itself (`xset s`, server-side DPMS), so it came for free with
the `xorg` metapackage. Wayland splits the job: the compositor only *reports*
idleness, and a separate daemon decides what to do about it.

labwc 0.9.3 holds up its half - the shipped binary advertises
`ext_idle_notifier_v1`, `zwlr_output_power_manager_v1`,
`zwp_idle_inhibit_manager_v1` and `ext_session_lock_manager_v1`. What was
missing was any listener, so labwc reported idleness into an empty room
forever.

`swayidle` (the listener) and `wlopm` (the output power switch) are therefore
part of the session package set, launched from `thinfloe-session` at a 300 s
default. Two decisions are deliberate:

- **`wlopm`, never `wlr-randr --off`.** `wlr-randr` is already in the image
  and looks like the obvious tool. It speaks the output *management* protocol
  and disables the output outright, which changes the monitor layout and
  resizes the fullscreen ThinLinc client (and makes
  `LABWC_FULLSCREEN_SPAN_OUTPUTS` recompute over a different set of outputs).
  `wlopm` speaks output *power* management - real DPMS, layout untouched.
- **No locking.** `ext_session_lock_manager_v1` is available, but `tluser` has
  no password, so a lock screen would have nothing to authenticate against.
  Session security is ThinLinc's job.

Known consequence: the ThinLinc client is an X11 app under Xwayland and cannot
send an idle inhibit, so a long video inside the remote session with no local
input blanks the local screen. Any keypress restores it. Raise
`TF_IDLE_TIMEOUT` in `~/.thinfloe-sessionrc` where that matters.

`test/integration/test-session-packages.sh` guards both halves - the two labwc
protocols, the presence of `swayidle`/`wlopm`, that `thinfloe-session` starts the
daemon, and that it does *not* blank with `wlr-randr --off`.

**Confirmed on hardware (2026-10-01):** the screen blanks at the 300 s default
and any keypress restores it.

### x11-xserver-utils: investigated and disproved

`xrandr`, `xset` and `xrdb` were a hard dependency of the `xorg` metapackage,
which was removed when the session moved to labwc/Wayland. This was flagged
as a risk (ThinLinc shelling out to `xrandr` for fullscreen resolution
negotiation) and then checked: `vncviewer`/`tlclient.bin` dlopen
`libXrandr.so.2`, which **is** still present in the image, and grepping for
the executables `xrandr`/`xset`/`xrdb` across `/opt/thinlinc/` returns zero
hits. ThinLinc uses the RandR *library*, never the CLI tools that
`x11-xserver-utils` shipped, so removing that package is safe.

That conclusion still holds, but note its scope: it only ever asked what
*ThinLinc* needed. `xset s` and server-side DPMS went out with the same
change and nothing replaced them - see "Screen blanking had to be rebuilt
after the X11 removal" above.

## Debugging

### A failed menu is shown and counted

`labwc` always exits 0, whatever its `-S` command returned (measured: a `-S`
command exiting 10, 0 or 101 all give `labwc` exit 0). `thinfloe-session`
therefore writes the menu's exit status to `$XDG_RUNTIME_DIR/thinfloe-session.status`
and, on any status but 0, opens a `foot` window with the status and the last
lines of the session log. `overlay/usr/libexec/thinfloe/session-verdict`, called
from `overlay/etc/profile.d/10-thinfloe-session.sh`, reads that file after labwc
returns. After three consecutive failed sessions, counted in
`/tmp/thinfloe-session-failures.<uid>`, it reports the status and
`10-thinfloe-session.sh` stops the restarts and shows the failure screen.

The session log also carries labwc's own line:
`[ERROR] [../src/server.c:167] spawned child 12 exited with 10`.

From a VT, `thinfloe-menu --check` lists every tile, why one is grey, and why
an icon draws its letter.

### The menu's log

The bottom 30 % of the menu (at least eight rows) is a log:
`HH:MM:SS  source | text`, in local time. It shows every tile start and
exit (with the exit status or signal), each save and its outcome, and every
line a tile's program or `thinfloe-save-home` writes to stdout or stderr, with
the tile name or `save-home` as source. Program output is untrusted, so the
bounds are fixed in `src/thinfloe-menu/src/log.rs`: lines are cut at 1 KiB, the
log and the queue from the reader threads keep 500 lines each, and output
wakes the menu at most ten times a second. A program's last lines appear
before its exit line; a background child that keeps the pipes open delays
the tile by at most 300 ms, and its later lines are still logged. Colours a
program writes (SGR) are drawn; other escape sequences are removed. Every log
line also goes to stderr, as plain text without escape sequences, so the
session log has all of it.

### The session waits for the GPU before it starts

`thinfloe-wait-kms.service` waits up to 20 s for a real KMS driver before the
tty1 autologin: i915 is omitted from the initrd and binds only after
switch-root, and simpledrm's `card0` alone does not count. `10-thinfloe-session.sh`
retries labwc once after another wait-kms, so a machine with no GPU waits about
40 s before the failure screen.

### Rebooting from inside the session needs sudo

There is no polkit in this image, no `polkitd` and no `pkexec`, and systemd
gates `Reboot` and `PowerOff` for non-root callers on a polkit authority. A
bare `systemctl reboot` as `tluser` is refused. Use `sudo systemctl reboot`,
the way `thinfloe-save-home` and `thinfloe-session` (for the Restart and Power off
tiles) already escalate.

### Reaching a terminal

The Terminal tile of the menu starts `foot` inside the session. With zero
labwc keybindings ([Standing Constraints](#standing-constraints)) there is no
other in-session path, so when the session is down, reach a terminal from a VT
(Ctrl+Alt+F2, say) or over SSH instead. **SSH needs a password
first:** a machine nobody has run `sudo thinfloe-password` on refuses every
password login, because tluser's shadow field is `!` and the image ships no
`authorized_keys`. On such a machine the VT is the only way in, and the VT
autologin gets there without a password.

```bash
XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 foot
```

Do not add a keybinding to launch it - that would violate the
zero-keybindings constraint.

### Screenshots

Screenshots use `grim`, with `slurp` for region selection and `wl-copy` to
put the result on the clipboard. Same constraint as `foot`: with zero
keybindings there is no in-session trigger, so take them from a VT or over
SSH.

```bash
XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 grim /tmp/shot.png
XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 \
    sh -c 'grim -g "$(slurp)" - | wl-copy'
```

Do not add a keybinding for this - that would violate the
zero-keybindings constraint.

## Potential Enhancements

Future improvements to consider:

1. **Firmware manifest** - Add version/checksum metadata to ramroot packages
2. **A/B update structure** - Support rollback by keeping previous version
3. **Additional output formats** - Generate qcow2 for testing in QEMU/KVM
4. **Serial console support** - Add console=ttyS0 to kernel parameters for headless debugging
