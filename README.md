# ThinFloe

ThinFloe boots a thin client from a USB stick or the network into RAM. Like an ice floe it carries you for a while and is gone at power-off, except for the saved home. Releases before 4.0 were called dbrrg (Docker-Based RamRoot Generator).

Modern diskless Linux system using SquashFS + OverlayFS + ZRAM + Dracut.

Features A/B firmware updates with automatic fallback support.

## Quick Start

```bash
# Build complete USB image
make

# Or just build rootfs
make rootfs

# Build iPXE network boot loaders
make ipxe

# Clean and rebuild
make clean && make
```

## Architecture

- **SquashFS**: Compressed read-only root filesystem
- **OverlayFS**: Writable layer on top of squashfs
- **ZRAM**: Compressed RAM storage for writes (2GB virtual, ~500MB actual)
- **Dracut**: Modern modular initramfs system

## Memory Usage

### USB Boot
- SquashFS: 0MB (on USB stick)
- Page cache: 200-800MB (adaptive)
- ZRAM overlay: ~500MB
- **Total: 0.7-1.3GB**

### Network Boot
- SquashFS: ~400MB (in RAM)
- Page cache: 200-800MB (adaptive)
- ZRAM overlay: ~500MB
- **Total: 1.1-1.7GB**

## Directory Structure

```
thinfloe/
├── Makefile                    # Build system
├── build/                      # Build scripts
│   ├── lib/common.sh
│   ├── export-rootfs.sh
│   └── make-bootable-image.sh
├── containers/                 # Container definitions
│   ├── ubuntu/Dockerfile
│   ├── image-builder/Dockerfile
│   └── ipxe/Dockerfile
├── configs/                    # Configuration files
│   └── syslinux.cfg
├── overlay/                    # Rootfs customizations
│   ├── usr/lib/dracut/modules.d/90thinfloe/  # Dracut module
│   └── etc/sysctl.d/99-thinfloe-memory.conf  # Memory tuning
└── artifacts/                  # Build outputs (gitignored)
    ├── rootfs/
    │   ├── vmlinuz
    │   ├── initrd.img
    │   ├── ramroot.sqsh
    │   ├── ipxe.pxe
    │   ├── undionly.kpxe
    │   └── ipxe.efi
    └── images/
        ├── thinfloe-usb.img
        ├── thinfloe-usb.img.zst
        └── thinfloe-usb.img.zst.sha256
```

## Boot Parameters

### USB Boot
```
ramroot=tl/ramroot.sqsh
```

### Network Boot
```
ramroot=http://server/boot/ramroot.sqsh rd.neednet=1 ip=dhcp
```

### Debug Mode
```
rd.debug rd.shell rd.thinfloe.debug rd.break=pre-mount
```

## Configuration

Edit `Makefile` variables:
- `SQUASHFS_COMP=zstd` - Compression algorithm (zstd, xz, lz4)
- `SQUASHFS_COMP_LEVEL=3` - Compression level (1-19, lower = faster)
- `EFI_PARTITION_SIZE=2000` - EFI partition size in MB

## Deployment

### USB Stick
```bash
# Write compressed image to USB (recommended)
zstd -d < artifacts/images/thinfloe-usb.img.zst | dd of=/dev/sdX bs=1M status=progress conv=fsync

# Or write uncompressed image
dd if=artifacts/images/thinfloe-usb.img of=/dev/sdX bs=1M status=progress conv=fsync
```

### Upgrading Firmware

Use the interactive `thinfloe-upgrade` script on a running system:

```bash
sudo thinfloe-upgrade
```

Features:
- **Drive selection**: Choose which USB to upgrade
- **Boot USB detection**: Safe additive upgrade when running from the target USB
- **A/B fallback**: Previous version kept as fallback (type `previous` at boot menu)
- **Home copy**: A fresh install offers to copy this machine's home onto the new drive (SSH host keys and `wg0.conf` stay behind, so each machine keeps its own identity)

Upgrade modes:
- **Additive** (boot USB): Writes new firmware to `tl.new/`, keeps `tl/` as fallback
- **In-place** (other USB): Rotates `tl/` → `tl.old/`, installs new `tl/`
- **Full image**: Complete rewrite with data backup/restore

### The session menu

After login the session opens a grid of tiles. ThinLinc is one click.
Other shipped tiles start oxulnk, a terminal, a home backup, `thinfloe-upgrade`
and the logout. Logging out saves the home directory first; if the save
fails, the menu asks whether to stay or to log out anyway.

Add a tile for one machine as a `.desktop` file in `~/.config/thinfloe/menu/`:

```ini
[Desktop Entry]
Type=Application
Name=Screenshot
Exec=grim /home/tluser/screenshot.png
```

Without an `Icon` the tile shows the first letter of `Name`. The program must
be in the image; a tile whose program is not installed is drawn grey.

- `Name`, `Icon`, `Exec`: Label, icon and command of the tile.
- `Terminal=true`: Run the command in a terminal window.
- `X-TF-Save-On-Exit=true`: Save the home directory when the program exits.

The files are saved with the home directory. A file with the name of a
shipped tile (for example `10-thinlinc.desktop`) rewords that tile's `Name`,
`Comment` and `Icon`; its other keys are ignored. `thinfloe-menu --check` run
from a VT explains why a tile is grey.

### Network Boot (PXE/iPXE)
```bash
# Build iPXE boot loaders
make ipxe

# Copy files to TFTP/HTTP server
cp artifacts/rootfs/ipxe.pxe /var/lib/tftpboot/
cp artifacts/rootfs/ipxe.efi /var/lib/tftpboot/
cp artifacts/rootfs/vmlinuz artifacts/rootfs/initrd.img /var/www/html/boot/
cp artifacts/rootfs/ramroot.sqsh /var/www/html/boot/

# Configure DHCP to serve iPXE, then chainload to kernel with:
ramroot=http://your-server/boot/ramroot.sqsh
```

## Migration from v1.x

See `MIGRATION.md` for detailed migration guide.

## Troubleshooting

### Build Issues
```bash
# Check podman is working
podman --version

# Clean and rebuild
make clean && make
```

### Boot Issues
```bash
# Use debug mode in syslinux.cfg
rd.debug rd.shell rd.thinfloe.debug

# Check logs after boot
cat /var/log/dracut-boot.log
cat /var/log/thinfloe/*
```

### Memory Issues
Edit `overlay/etc/sysctl.d/99-thinfloe-memory.conf`:
- Increase `vm.vfs_cache_pressure` to 200 (more aggressive cache eviction)
- Decrease ZRAM size in dracut module

## Development

### Test Dracut Module
```bash
# Rebuild initramfs quickly
dracut --force --add thinfloe

# Test in QEMU
qemu-system-x86_64 -m 4G -enable-kvm \
  -drive file=artifacts/images/thinfloe-usb.img,format=raw
```

### Modify Boot Process
Edit files in `overlay/usr/lib/dracut/modules.d/90thinfloe/`:
- `parse-thinfloe.sh` - Command line parsing
- `finalize-upgrade.sh` - Activate pending tl.new/ upgrades
- `mount-squashfs.sh` - SquashFS mounting
- `setup-overlay.sh` - Overlay setup

Rebuild with `make rootfs`

## A/B Update System

The system supports A/B firmware updates with automatic fallback:

### Directory Structure
```
EFI-SYSTEM partition:
├── tl/           # Current firmware (or tl.new/ after additive upgrade)
├── tl.old/       # Previous firmware (fallback)
├── config/       # Persistent configuration
└── home.tar.gz   # User home backup
```

### Boot Menu Options
- `current` - Boot the active firmware version
- `previous` - Boot the previous version (fallback)
- `console` - Text-only boot
- `debug` - Debug shell

### Recovery
If a new firmware version fails to boot:
1. Reboot and press any key at the boot prompt
2. Type `previous` and press Enter
3. System boots from the previous known-good version
