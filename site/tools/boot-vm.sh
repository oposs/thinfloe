#!/bin/bash
# Boot the released stick image in QEMU without a window, for the page's
# pictures and boot movie (see ../README.md).
#
#   boot-vm.sh IMAGE WORKDIR [XRES YRES]
#
# IMAGE is the decompressed thinfloe-usb.img; it is never written: the VM
# runs on a qcow2 layer in WORKDIR, sized like an 8 GB stick. The screen is
# a standard VGA device: with it the firmware, the boot screen and the
# session all run at XRES x YRES (virtio-vga starts at 1280x800). Two QMP
# sockets appear in WORKDIR: qmp-rec.sock (for the recorder) and
# qmp-ctl.sock (for clicks and keys). At most 2 cores and 4 GB.
set -euo pipefail
image=$(realpath "${1:?usage: boot-vm.sh IMAGE WORKDIR [XRES YRES]}")
work=$(realpath "${2:?missing WORKDIR}")
xres=${3:-1920}
yres=${4:-1080}
# The model the status area shows; QEMU's own name is too long for the tile.
model=${TF_VM_MODEL:-ThinkCentre M70q}

[[ -e $work/stick.qcow2 ]] || qemu-img create -q -f qcow2 -b "$image" -F raw "$work/stick.qcow2" 8G

exec qemu-system-x86_64 \
    -machine type=q35,accel=kvm \
    -cpu host,migratable=off \
    -smp 2 \
    -m 4G \
    -bios /usr/share/ovmf/OVMF.fd \
    -display none \
    -device VGA,xres="$xres",yres="$yres",vgamem_mb=64 \
    -smbios "type=1,product=$model" \
    -device qemu-xhci -device usb-tablet -device usb-kbd \
    -object rng-random,filename=/dev/urandom,id=rng0 \
    -device virtio-rng-pci,rng=rng0 \
    -net nic,model=virtio -net user \
    -drive file="$work/stick.qcow2",format=qcow2,if=virtio \
    -serial "file:$work/serial.log" \
    -qmp "unix:$work/qmp-rec.sock,server=on,wait=off" \
    -qmp "unix:$work/qmp-ctl.sock,server=on,wait=off"
