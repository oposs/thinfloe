This note covers the build containers, the boot flow, the SquashFS/ZRAM layering, machine-id persistence and old enhancement ideas.

## Build System Architecture

The build process uses containers in `containers/` directory:

1. **containers/ubuntu/Dockerfile** - Base system container that creates the Ubuntu rootfs with all packages and custom overlay files (no ThinLinc client and no OxuLnk: those are add-ons)
2. **containers/ipxe/Dockerfile** - Builds iPXE network boot loaders (PXE, KPXE, EFI formats)
3. **containers/image-builder/Dockerfile** - Final packaging container with tools to create bootable EFI images

The `menu-build` stage of `containers/ubuntu/Dockerfile` compiles the tile menu
`thinfloe-menu` from the crate at `src/thinfloe-menu/` (Rust 1.96.0 via rustup,
`cargo build --release --locked`) and the final stage copies the result into
the image. `MENU_FILES` and `MENU_DIRS` in the `Makefile` list the crate's
files and directories as prerequisites of `.ubuntu-container`, so editing the
crate rebuilds the image.

The `overlay/` directory contains all customizations that get layered onto the base Ubuntu system during the build.

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
   - **SquashFS Mount**: Mounts compressed read-only rootfs from USB partition or HTTP URL (on USB, the ESP of the stick the firmware booted from, chosen by stick id; see [Standing Constraints](history-of-constraints.md#the-esp-is-chosen-by-stick-id-never-by-the-partlabel-link))
   - **ZRAM Setup**: Creates compressed RAM device for writable overlay
   - **OverlayFS**: Combines read-only SquashFS with writable ZRAM layer
   - **machine-id Persistence**: Reads/generates machine-id and stores it in /config/machine-id on EFI partition
   - **Home Restore**: Restores `/home/tfuser` from the boot medium
     (`home.tar.gz` on the EFI partition, or `home.pkg` from the boot server)
     before the pivot, so the home directory is in place before
     `multi-user.target`

4. **Un-dockerization** (overlay/etc/systemd/system/un-dockerize.service) - First-boot service removes Docker artifacts, fixes /etc/hosts (resolv.conf ships as the systemd-resolved stub link)

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
- **overlay/home/tfuser/.config/thinfloe/** - the home templates: `environment`, `sessionrc` and `home-version`; see [home-persistence.md](home-persistence.md)
- **overlay/usr/lib/dracut/modules.d/90thinfloe/** - Dracut module for boot process

## Container Build Best Practices

The containers/ubuntu/Dockerfile follows several important patterns:

1. **SSH Host Keys**: Host keys are removed at build time. On first boot
   `thinfloe-ssh-hostkeys.service` generates them and stages them into
   `~tfuser/.config/thinfloe/ssh-host-keys`, from where `thinfloe-save-home` captures them
   into the home archive at logout; the initramfs restores them on every
   subsequent boot. `ssh.socket` is **masked**; see Standing Constraints.
   Keys do not persist until the first clean logout: a machine hard-powered
   off before then generates fresh keys next boot.

2. **Initramfs Generation**: The build uses Dracut (`dracut --force --no-hostonly --add "thinfloe plymouth"`) to create initramfs. The `--no-hostonly` flag is mandatory - see [Standing Constraints](../../CLAUDE.md#standing-constraints). The custom module in `overlay/usr/lib/dracut/modules.d/90thinfloe/` is automatically included. This must run AFTER overlay files are applied and SSH keys are removed.

3. **Systemd Service Management**: Services are explicitly enabled/disabled during build to control first-boot behavior. The `un-dockerize.service` runs once and disables itself.

4. **Locale Generation**: `locale-gen` must run after overlay files are applied to process locale.gen configuration.

5. **User Configuration**: The default user (tfuser, uid 1001; called `tluser` before 4.0) is created with no password but added to sudo group with NOPASSWD privileges for management tasks. `/home/tluser` is a symlink to `/home/tfuser`, so old paths in saved files still resolve.

## Performance Optimizations

## SquashFS + OverlayFS + ZRAM Architecture

The system uses a layered approach for optimal memory usage:

- **SquashFS**: Compressed read-only root filesystem (zstd compression)
- **ZRAM**: Compressed RAM device for writable overlay layer
- **OverlayFS**: Combines SquashFS (lower) with ZRAM (upper) for a writable system

This architecture significantly reduces memory pressure compared to extracting the entire rootfs to RAM.

## machine-id Persistence

The system persists systemd's machine-id across reboots by storing it in `/config/machine-id` on the EFI partition. This is important because:

- Systemd requires a stable machine-id for proper service operation
- Without persistence, services like journald, networkd, and DHCP may behave unexpectedly
- The machine-id is generated once on first boot and reused thereafter

The EFI partition `/config/` directory can also store other persistent configuration.

## Potential Enhancements

Future improvements to consider:

1. **Firmware manifest** - Add version/checksum metadata to ramroot packages
2. **A/B update structure** - Support rollback by keeping previous version
3. **Additional output formats** - Generate qcow2 for testing in QEMU/KVM
4. **Serial console support** - Add console=ttyS0 to kernel parameters for headless debugging

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
