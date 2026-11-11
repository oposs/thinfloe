# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository. It keeps the rules; how finished features work, their history and hardware results are in [docs/notes/](#notes).

## Project Overview

ThinFloe creates bootable diskless client images that run entirely from RAM. The name is an ice floe: the system floats in RAM, carries you for a while, and is gone at power-off, except for the saved home. Releases before 4.0 called the project dbrrg (Docker-Based RamRoot Generator); the old name survives in a few places (the upgrade server path, `dbrrg-usb.img.zst`, the 3.x migration). The system builds Ubuntu 26.04 images with Podman containers, packages them for USB and PXE boot, and boots them with its own dracut module. ThinLinc and OxuLnk are add-ons the user ticks, not part of the image.

Boot in short: syslinux loads the kernel with `ramroot=` (a path on the stick's ESP, or an `http://` URL for netboot); the initramfs mounts the SquashFS, puts a ZRAM-backed OverlayFS over it, persists the machine-id in `/config/machine-id` on the ESP, restores `/home/tfuser` and pivots. tty1 autologs in `tfuser`, `thinfloe-session` starts labwc and the tile menu `thinfloe-menu`. Details: [docs/notes/boot-flow.md](docs/notes/boot-flow.md).

## Commands

```bash
make image               # all stages, bootable files in artifacts/
make rootfs              # Ubuntu container; kernel, initrd, ramroot.sqsh to artifacts/rootfs/
make ipxe                # iPXE loaders to artifacts/rootfs/
make test                # test-unit, then the integration tests against the built image
make test-unit           # Python unit tests in test/unit/ and the menu's cargo test
make test-runtime        # labwc and the menu headless
make qemu-smoke          # boot the USB image in QEMU (also: -addons, -write, -upgrade,
                         #   -netboot, -two-sticks, -grow, -timezone, -timezone-netboot)
make qemu-test           # QEMU with boot, target and empty drives for upgrade tests
dd if=artifacts/images/thinfloe-usb.img of=/dev/sdX bs=1M status=progress conv=fsync
scripts/publish.sh       # publish a release for thinfloe-upgrade
```

Menu crate: `cargo test`, `cargo clippy` and `cargo fmt` in `src/thinfloe-menu`.

`scripts/publish.sh` uploads `version`, `vmlinuz`, `initrd.img`,
`ramroot.sqsh` and `thinfloe-usb.img.zst` with its checksum to
`https://tobi.oetiker.ch/dbrrg` (the server kept its name), and the image
again as `dbrrg-usb.img.zst` (a hard link under `artifacts/images/bridge/`)
because the 3.x `upgrade-image` fetches only that name; `vmlinuz`,
`initrd.img` and `ramroot.sqsh` keep their names. `version` (read by
`thinfloe-upgrade check`) is `artifacts/rootfs/version`, copied from the
image's `/etc/thinfloe/version`,
which the build writes from `RELEASE` (one line `X.Y.Z`, else the build and
the publish fail). Raise `RELEASE` before a release.

## Where code lives

- Build: `containers/ubuntu/Dockerfile` (rootfs, labwc rebuild, `menu-build` stage with Rust 1.96.0), `containers/ipxe/`, `containers/image-builder/`, `scripts/` (export, image, QEMU smoke tests), `Makefile`.
- Image customisation: `overlay/` (standard filesystem layout, applied by `tar --exclude="*~"`); rebuild with `make image`.
- Initramfs: `overlay/usr/lib/dracut/modules.d/90thinfloe/` (`module-setup.sh`, hooks, `thinfloe-lib.sh`).
- Boot menu and kernel parameters: `configs/syslinux.cfg`.
- Session: `overlay/usr/bin/thinfloe-session`, `overlay/etc/profile.d/10-thinfloe-session.sh`, `overlay/usr/bin/thinfloe-compose-labwc-config`, `overlay/etc/systemd/system/getty@tty1.service`.
- Compositor config: `overlay/etc/thinfloe/labwc/` (`rc.xml`, `environment`), outside `$HOME`.
- Tile menu: `src/thinfloe-menu/` (look in `brand.rs`, layout in `ui.rs`, state in `menu.rs`, event loop in `app.rs`, status area in `status/`).
- Shipped tiles: `overlay/etc/thinfloe/menu/*.desktop`; user tiles `~/.config/thinfloe/menu/`.
- Home save: `overlay/usr/bin/thinfloe-save-home`; restore: `restore_home()` in `thinfloe-lib.sh`.
- Stick writer and upgrades: `overlay/usr/bin/thinfloe-upgrade`.
- Add-ons: `overlay/usr/bin/thinfloe-addon` (its docstring lists the commands), hooks in `overlay/usr/libexec/thinfloe/addon-hooks/`.
- Battery reader: `overlay/usr/libexec/thinfloe/peripheral-battery` and its units.
- Boot screen: `overlay/usr/share/plymouth/themes/thinfloe/`.
- User defaults: `overlay/home/tfuser/` (`.thinfloe-environment`, `.thinfloe-sessionrc`, `.thinlinc/tlclient.conf`, `wifi.yaml`).
- Tests: `test/integration/` (`make test`), `test/unit/` (`make test-unit`), `test/runtime/` (`make test-runtime`).

## Rules that are easy to break

Not standing constraints, but each has broken something or is guarded by a test.

### Build

- `MENU_FILES`, `MENU_DIRS`, `PATCH_FILES` and `OVERLAY_DIRS` in the `Makefile` are prerequisites of `.ubuntu-container`; a new input needs a line there, or tests validate a stale image. Each container stamp holds the image ID it built; build with a distinct `VERSION` per checkout. Guards: `test-container-inputs.sh`, `test-container-stamp.sh`.
- Netplan files are made mode 0600 by a `chmod` in the Dockerfile (git stores only 644/755).
- `isc-dhcp-client` is not installed and nothing may call `dhclient`; DHCP is `systemd-networkd`'s job, in the initramfs too.

### Initramfs

- No `exit` in any 90thinfloe hook: dracut sources every hook of a stage into one shell, so an `exit` ends the stage. Guard: `test-esp-grow.sh`.
- `tf_log` writes to stderr only; a log line on stdout inside a helper's answer broke `finalize-upgrade.sh`.
- Only the commands `module-setup.sh` installs exist there (see the standing constraint below).

### Home and session

- The home restore runs in the initramfs, never in `thinfloe-session` or `10-thinfloe-session.sh`: `.thinfloe-environment` must be on disk before labwc reads `XKB_DEFAULT_*`, and `thinfloe-ssh-hostkeys` needs the home before `multi-user.target`.
- `~/.thinfloe-environment` is sourced before labwc (startup variables: keyboard, cursor, `TZ=`, `LABWC_FULLSCREEN_SPAN_OUTPUTS`); `~/.thinfloe-sessionrc` inside the session (commands that need a compositor: `wlr-randr`, `kanshi`, `TF_IDLE_TIMEOUT`).
- labwc runs with `-C` on a directory composed by `thinfloe-compose-labwc-config` (system files symlinked, `environment` = system defaults then the user's file). Pointing `-C` at `/etc/thinfloe/labwc` again makes labwc reset user keys to the system values. Guards: `test-labwc-config-merge.sh`, `test-labwc-runtime.sh`. Story: [docs/notes/session.md](docs/notes/session.md).
- `thinfloe-save-home` takes the home from `getent passwd tfuser`, never `$HOME` (`sudo` sets `HOME=/root`), and every read under `/run/thinfloe/state` ends in `|| true` (`set -e`). On USB it writes only to the ESP mounted at `/run/thinfloe/storage/efi` (tested with `mountpoint -q`), atomically; the netboot upload uses `curl -f`. Exit codes 0 to 5 are a contract. Guard: `test-save-home.sh`.
- The remote-access password hash (`~/.thinfloe-password`) and the SSH host keys (`~/.thinfloe-ssh-host-keys`) ride the home archive in the clear: on the FAT ESP, or POSTed over plain HTTP on netboot. Accepted; see [docs/notes/home-persistence.md](docs/notes/home-persistence.md).
- A fresh install's home copy always leaves out `~/.thinfloe-ssh-host-keys`, `~/.dbrrg-ssh-host-keys` and `~/wg0.conf` (`IDENTITY_EXCLUDES`). Guard: `test_upgrade_image.py`.
- Reboot and power off from the session need `sudo systemctl ...`: the image has no polkit.
- Screen blanking uses `swayidle` and `wlopm`, never `wlr-randr --off`, which changes the monitor layout. No lock screen: tfuser has no password.
- No keybinding for a terminal or screenshots (see zero keybindings); from a VT: `XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=wayland-0 foot` (or `grim`).

### Menu

- UI code takes colours from the theme and text sizes from named constants in `brand.rs` (`status/` too); `ui_code_has_no_style_literals` fails on a literal. Widgets are shadcn-egui, `lyra` style, nothing rounded.
- User icons render in a child process (`thinfloe-menu --render-icon`) under rlimits and a kill timer; never render a user icon in-process. Limits in `icons.rs`.
- `X-TF-Action=settings` and `=upgrade` are honoured only in shipped tile files; a user file rewording a shipped tile may change `Name`, `Comment` and `Icon` only, and cannot set `X-TF-Multiple` or `X-TF-Source-*`.
- The menu sends a running `thinfloe-upgrade write` SIGTERM, never SIGKILL (SIGKILL reaches only sudo and the lock stays held). Jobs run in their own process group.
- A fullscreen window that leaves winit no monitor for the menu is not a move: the last known monitor counts (`seen_monitor` in `app.rs`). Taken as a move, it gave the largest mode's scale and the title bars grew (2026-10-30). See [docs/notes/menu.md](docs/notes/menu.md).
- The scale file `$XDG_RUNTIME_DIR/thinfloe-scale` is written atomically (one number and a newline); `labwc-scale` and `thinfloe-terminal` read it.
- Status area: an element is one module in `status/` plus a line in `status::ELEMENTS`; `draw` is pure and stays in its rect. The layout file `~/.config/thinfloe/status` is written only by the menu. Notes: [docs/notes/status-area.md](docs/notes/status-area.md).
- The battery file `/run/thinfloe/peripherals/battery` is a contract between the root reader and the menu: one line per device, `name<TAB>percent<TAB>state<TAB>read_epoch<TAB>kind`; the menu still reads a 4-field line (kind unknown). See [docs/notes/peripheral-battery.md](docs/notes/peripheral-battery.md).

### Boot screen

- The Plymouth progress follows elapsed time, never Plymouth's `progress`; the logo is drawn from pre-rendered widths, never `Image.Scale`d; `layout()` runs again on display hotplug; `DeviceScale=1` stays in `plymouthd.conf`. Guard: `test-initramfs-commands.sh`. Notes: [docs/notes/boot-screen.md](docs/notes/boot-screen.md).
- The Geo font path `/usr/share/fonts/truetype/thinfloe/` is also copied into the initrd; moving it breaks the boot screen.

### Add-ons

- Lock order: `install.lock` -> `/run/thinfloe/esp.lock` -> `.lock`; never take an earlier one while a later one is held. `thinfloe-save-home` takes only `esp.lock`.
- An add-on source is fetched automatically only when every URL is `https:` or it is APT with a key (`auto_allowed`).
- Add-on ids: `ID_RE` in the tool and `valid_id` in the menu apply one rule; `test/unit/addon-ids.tsv` pins both.
- A post-install hook path never comes from a tile or the home, only from `/usr/libexec/thinfloe/addon-hooks/<id>`.
- Every store on the ESP renames the payload before its meta, and `_drop_slot` and `remove` delete the metas before the payloads; `finish_promote`'s recovery after a power loss mid-promote depends on that order. See [docs/notes/add-ons.md](docs/notes/add-ons.md).
- Details: [docs/notes/add-ons.md](docs/notes/add-ons.md).

### Upgrade and netboot

- `thinfloe-upgrade` takes `esp.lock` only on an upgrade of the boot stick and only in two short places of the `switch` step (removing `tl.old/` and a stale `tl.new/`; the final `sync` and the boot label). The copy into `tl.new/` runs unlocked, because a save gives up waiting after 30 s and then writes unlocked. See [docs/notes/add-ons.md](docs/notes/add-ons.md) (Locks).
- Every child process of `thinfloe-upgrade` runs in a session of its own (`SPAWN`), because the menu signals the tool's whole process group. See [docs/notes/esp-and-upgrade.md](docs/notes/esp-and-upgrade.md).
- Netboot: the boot MAC comes from the interface holding the default route and is read back from `/run/thinfloe/state/boot-mac` by restore and save, so both agree; `parse-thinfloe.sh` sets `rd.neednet=1` for an http ramroot; `dracut-network` must stay installed (it ships the `systemd-networkd` and `url-lib` dracut modules). See [docs/notes/network-boot.md](docs/notes/network-boot.md).

## Standing Constraints

Twenty rules here look like ordinary configuration but are load-bearing; most
have caused a shipped-image bug. Each has a full story in
[docs/notes/history-of-constraints.md](docs/notes/history-of-constraints.md)
under the same heading.

### No login password ships in the image

tfuser is created with `adduser --disabled-password` and its shadow field is
`!` alone (not `passwd -l` over a hash); root is `passwd -d root && passwd -l
root`, and `PermitRootLogin no`. A shipped password plus `NOPASSWD:ALL` made
every machine root-accessible with one known login. `PasswordAuthentication
yes` stays: the user sets a password with `sudo thinfloe-password`.
Guards: `test-session-packages.sh`, `test-password.sh`.
[Story](docs/notes/history-of-constraints.md#no-login-password-ships-in-the-image).

### Firmware is selected by package, never by cleanup

Never sweep `/usr/lib/firmware` with `find`/`rm`; change the package list in
the Dockerfile instead. A sweep once deleted i915 firmware and CPU microcode
and left dangling symlinks. Guard: `test-firmware.sh`.
[Story](docs/notes/history-of-constraints.md#firmware-is-selected-by-package-never-by-cleanup).

### The WiFi core stack must come from the kernel, never from a backport

Do not install `linux-modules-iwlwifi-generic`: it ships its own `mac80211`
and `cfg80211` in `ubuntu/dkms/iwlwifi/`, which outrank the kernel's and break
WiFi for every vendor. Guard: `test-wifi-stack.sh`.
[Story](docs/notes/history-of-constraints.md#the-wifi-core-stack-must-come-from-the-kernel-never-from-a-backport).

### dracut must be invoked with --no-hostonly

dracut 110 defaults to hostonly, which filters modules against the build
machine's loaded modules: zram and the NIC drivers were dropped and the boot
aborted. `--add-drivers` is no substitute.
[Story](docs/notes/history-of-constraints.md#dracut-must-be-invoked-with---no-hostonly).

### labwc must have zero keybindings

`overlay/etc/thinfloe/labwc/rc.xml` has a `<keyboard>` section with no
`<default />` and no `<keybind>`. Our labwc's keyboard-grab patch would
suspend bindings while ThinLinc holds the keyboard, but that is unverified on
hardware; adding one is a deliberate decision followed by `make test-runtime`.
Guard: `test-session-packages.sh`.
[Story](docs/notes/history-of-constraints.md#labwc-must-have-zero-keybindings).

### ssh.socket must stay masked

The Dockerfile runs `systemctl disable ssh.socket`, then `mask`, enables
`ssh.service` alone, and masks `sshd-keygen.service`. An active socket puts
`thinfloe-ssh-hostkeys.service` in an ordering cycle; `disable` before `mask`
removes the `requires` symlink; `sshd-keygen` would race the key unit.
Guard: `test-session-packages.sh`.
[Story](docs/notes/history-of-constraints.md#sshsocket-must-stay-masked).

### A multi-user.target unit must never declare Before= on a sysinit.target unit

Such a unit is implicitly after `basic.target`, so the `Before=` is a cycle and
systemd deletes a job on every boot. `systemd-hwdb-update.service` is masked.
Guards: `scripts/check-boot-smoke.sh` (any `Found ordering cycle`),
`test-session-packages.sh`.
[Story](docs/notes/history-of-constraints.md#a-multi-usertarget-unit-must-never-declare-before-on-a-sysinittarget-unit).

### labwc is a local rebuild, not the archive package

The Dockerfile builds `labwc_0.9.3-1+thinfloe1` with
`containers/ubuntu/patches/` (fullscreen across outputs,
`LABWC_FULLSCREEN_SPAN_OUTPUTS`; `zwp_xwayland_keyboard_grab_manager_v1`).
The archive package silently loses both. Patches, overlay and brand
directories are `Makefile` prerequisites. Guards: `make test`, `make
test-runtime`, `test-container-inputs.sh`, `test-container-stamp.sh`.
[Story](docs/notes/history-of-constraints.md#labwc-is-a-local-rebuild-not-the-archive-package).

### Per-machine network config is installed by the initramfs, never by a unit

`install_local_network()` copies `~/wifi.yaml`, `~/wg0.conf` (and creates the
`wg-quick@wg0` link) before switch-root, so the netplan generator creates the
WPA units. Guards: `test-field-report.sh` (no `thinfloe-local-network.service`),
`test-session-packages.sh` (mode 600).
[Story](docs/notes/history-of-constraints.md#per-machine-network-config-is-installed-by-the-initramfs-never-by-a-unit).

### The initramfs has no cut, basename, head, install, wc or date

Only what `inst_multiple` in `module-setup.sh` installs plus dracut's base
set exists; builtins are fine. A missing command fails silently at boot. The
initrd carries no WiFi drivers or firmware. Guards:
`test-initramfs-home.sh` (helpers with a restricted PATH),
`test-initramfs-commands.sh` (every hook command against `lsinitramfs`; no
`iwlwifi`).
[Story](docs/notes/history-of-constraints.md#the-initramfs-has-no-cut-basename-head-install-wc-or-date).

### The ThinFloe pre-mount and mount hooks run after dracut-cmdline

`thinfloe-after-cmdline.conf` orders `dracut-pre-mount.service` and
`dracut-mount.service` `After=dracut-cmdline.service`; without it they race
the hook that sets `root=thinfloe` and the boot stops in the emergency shell.
Guards: `test-initramfs-commands.sh`, `scripts/check-boot-smoke.sh`.
[Story](docs/notes/history-of-constraints.md#the-thinfloe-pre-mount-and-mount-hooks-run-after-dracut-cmdline).

### /etc/hostname is excluded from the squashfs and written by the initramfs

podman's bind-mounted `/etc/hostname` gave every machine the container ID.
`tf_write_hostname` writes `thinfloe-` plus six hex digits of the machine-id
(USB) or the boot MAC (netboot) before switch-root. Guard:
`test-field-report.sh`.
[Story](docs/notes/history-of-constraints.md#etchostname-is-excluded-from-the-squashfs-and-written-by-the-initramfs).

### thinfloe-menu draws on the CPU

No `wgpu`, `glow` or `eframe`: the images have no Vulkan ICD and GL would tie
the menu's start to EGL. Guard: `test-session-packages.sh`.
[Story](docs/notes/history-of-constraints.md#thinfloe-menu-draws-on-the-cpu).

### thinfloe-session never saves; thinfloe-menu saves before it exits 0, 10 or 11

The menu saves the home and shows a failed save at the machine; 0 is log out,
10 restart, 11 power off. Any other status is a failure (the default branch
in `thinfloe-session`), so a crash never archives the home. Guard:
`test-session-lifecycle.sh`.
[Story](docs/notes/history-of-constraints.md#thinfloe-session-never-saves-thinfloe-menu-saves-before-it-exits-0-10-or-11).

### The first boot after an upgrade takes its kernel from tl.new

An upgrade of the running stick stages `tl.new/` and sets `DEFAULT new` in
both `syslinux.cfg` copies; `tf_finalize_upgrade` rotates `tl` -> `tl.old`,
`tl.new` -> `tl` only when `ramroot=` is `tl.new/ramroot.sqsh`. Rotating on a
boot from `tl/` ran an old kernel on new modules. `tl.old/` is removed before
staging, after a space check. Guards: `test_upgrade_image.py`,
`test-initramfs-home.sh`, `make qemu-smoke-upgrade`.
[Story](docs/notes/history-of-constraints.md#the-first-boot-after-an-upgrade-takes-its-kernel-from-tlnew).

### The ESP is chosen by stick ID, never by the partlabel link

Every stick has an `EFI-SYSTEM` partition, so the by-partlabel link names
either of two sticks. The ESP is chosen by `/config/stick-id` and
`thinfloe.stick=<id>` on the `APPEND` lines (`tf_find_esp`, `tf_choose_esp`,
cached in `/run/thinfloe-esp-device`); `tf_stamp_stick` and `thinfloe-upgrade`
write the ID byte for byte alike. Guards: `test-esp-choice.sh`,
`test_upgrade_image.py`, `make qemu-smoke-two-sticks`.
[Story](docs/notes/history-of-constraints.md#the-esp-is-chosen-by-stick-id-never-by-the-partlabel-link).

### The ESP grows once, in the initramfs, and keeps the hybrid MBR

`90thinfloe/grow-esp.sh` grows partition 1 to the end of the stick, at most
6 GiB, and the FAT32 with `fat32expander`. Use `sgdisk`, never `sfdisk`
(which drops the hybrid MBR every stick has); no `exit` in the hook. Guards:
`test-esp-grow.sh`, `test-initramfs-commands.sh`, `make qemu-smoke`, `make
qemu-smoke-grow`.
[Story](docs/notes/history-of-constraints.md#the-esp-grows-once-in-the-initramfs-and-keeps-the-hybrid-mbr).

### Add-on runtime libraries are listed by name in the Dockerfile

`dpkg -i` resolves nothing, so every library ThinLinc and OxuLnk need is in
one `apt-get install` block by name (`libxrandr2` for ThinLinc's dlopen). A
new add-on library needs a line there. Guard: `test-addon-deps.sh`.
[Story](docs/notes/history-of-constraints.md#add-on-runtime-libraries-are-listed-by-name-in-the-dockerfile).

### A 3.x home is migrated once

`tf_migrate_home_names` renames the `.dbrrg-*` files and `.config/dbrrg` of
an archive without `.thinfloe-home-version` and writes the marker; the
archive's listing decides, not the disk. A failed listing or extraction
leaves `home-restore` at `failed`. Old keys (`rd.dbrrg.debug`,
`X-DBRRG-*`, `DBRRG_IDLE_TIMEOUT`) are still read; shipped tile files keep
their names. Guards: `test-initramfs-home.sh`, `test-session-packages.sh`,
`test-cmdline.sh`, `tiles.rs`, `test_upgrade_image.py`, `make
qemu-smoke-upgrade`.
[Story](docs/notes/history-of-constraints.md#a-3x-home-is-migrated-once).

### tty1 does not wait for Plymouth; the session ends it

`getty@tty1.service` is a whole unit without `After=plymouth-quit-wait.service`
that runs `login -f tfuser` (agetty crashed plymouthd at 4K), with
`TTYReset=no`, `TTYVHangup=no`, `TTYVTDisallocate=no`. The session hands the
display over through `sudo -n plymouth-step`; `plymouth-quit.service` is
masked and `thinfloe-plymouth-fallback.timer` ends Plymouth after 300 s. i915
and its firmware are in the initrd. `plymouth.use-simpledrm` stays on the
command line, else machines with amdgpu, nouveau or radeon (left out of the
initrd) show no boot picture. Guards: `test-session-packages.sh`,
`test-session-lifecycle.sh`, `test-plymouth-step.sh`, `test_addon.py`.
[Story](docs/notes/history-of-constraints.md#tty1-does-not-wait-for-plymouth-the-session-ends-it).

## Notes

`docs/notes/` holds how finished features work, their history, measurements
and hardware results:

- [boot-flow.md](docs/notes/boot-flow.md): build containers, boot flow, SquashFS/ZRAM layering, machine-id, old enhancement ideas.
- [boot-screen.md](docs/notes/boot-screen.md): palette, fonts, shadcn-egui, the Plymouth theme and its step line.
- [menu.md](docs/notes/menu.md): window rules, tiles and icons, the Upgrade screen, density, monitor arrangement, the menu's log and failure handling, known limits.
- [status-area.md](docs/notes/status-area.md): the status area, its elements, layout file, Status tab and samplers.
- [peripheral-battery.md](docs/notes/peripheral-battery.md): the battery reader, its units, the HID++ details and hardware results.
- [session.md](docs/notes/session.md): session startup, the user files, the labwc config merge, audio volume, screen blanking, GPU wait, terminals, screenshots.
- [time-zone.md](docs/notes/time-zone.md): `TZ=`, the DHCP time zone and its tests.
- [home-persistence.md](docs/notes/home-persistence.md): home restore and save, the archive, the password file, exit codes.
- [esp-and-upgrade.md](docs/notes/esp-and-upgrade.md): `thinfloe-upgrade`: home copy, size check, the batch interface, check timers.
- [network-boot.md](docs/notes/network-boot.md): netboot networking and its smoke test.
- [add-ons.md](docs/notes/add-ons.md): add-on sources, storage, locks, boot install, checks, the menu.
- [history-of-constraints.md](docs/notes/history-of-constraints.md): the full story behind each standing constraint.
