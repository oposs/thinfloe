# Makefile for ThinFloe - SquashFS + OverlayFS + ZRAM + Dracut

PROJECT_NAME := thinfloe
VERSION := 3.0.0
BUILD_DATE := $(shell date -u +%Y-%m-%d)
BUILD_ID := $(shell date -u +%Y%m%d-%H%M%S)

CONTAINER_RUNTIME ?= podman

# Parallelism cap. This machine is shared - never use more than this many
# cores for compression, container builds or QEMU.
BUILD_JOBS ?= 4

# Directories
ARTIFACT_DIR := artifacts
ROOTFS_DIR := $(ARTIFACT_DIR)/rootfs
IMAGE_DIR := $(ARTIFACT_DIR)/images

# Container images
UBUNTU_IMAGE := $(PROJECT_NAME)-ubuntu:$(VERSION)
IMAGE_BUILDER := $(PROJECT_NAME)-image-builder:$(VERSION)
IPXE_BUILDER := $(PROJECT_NAME)-ipxe:$(VERSION)

# Artifacts
KERNEL := $(ROOTFS_DIR)/vmlinuz
INITRD := $(ROOTFS_DIR)/initrd.img
SQUASHFS := $(ROOTFS_DIR)/ramroot.sqsh
USB_IMAGE := $(IMAGE_DIR)/$(PROJECT_NAME)-usb.img
USB_IMAGE_COMPRESSED := $(USB_IMAGE).zst
USB_CHECKSUM := $(USB_IMAGE_COMPRESSED).sha256
IPXE_PXE := $(ROOTFS_DIR)/ipxe.pxe
IPXE_KPXE := $(ROOTFS_DIR)/undionly.kpxe
IPXE_EFI := $(ROOTFS_DIR)/ipxe.efi

# Configuration. These shape the shipped image, so a variable of the same
# name in the environment must not change them (`:=`, not `?=`); a
# `make SQUASHFS_COMP_LEVEL=…` on the command line still does.
SQUASHFS_COMP := zstd
SQUASHFS_COMP_LEVEL := 19
EFI_PARTITION_SIZE := 3000

# QEMU settings
QEMU_MEMORY ?= 2G
QEMU_EXTRA_ARGS ?=
# Screen size of the qemu-test window, e.g. QEMU_XRES=2560 QEMU_YRES=1440.
QEMU_XRES ?= 1920
QEMU_YRES ?= 1080

.PHONY: all clean rootfs image ipxe qemu-test qemu-smoke qemu-smoke-netboot qemu-smoke-timezone qemu-smoke-timezone-netboot qemu-smoke-upgrade qemu-smoke-addons qemu-smoke-two-sticks test-unit test test-runtime help

all: image
	@echo "✓ Build complete!"

help:
	@echo "ThinFloe Build System"
	@echo ""
	@echo "Targets:"
	@echo "  all (default)     - Build complete USB image"
	@echo "  rootfs            - Build kernel, initrd, squashfs"
	@echo "  ipxe              - Build iPXE network boot loaders"
	@echo "  image             - Build bootable USB image"
	@echo "  qemu-test         - Test boot image in QEMU (EFI)"
	@echo "  qemu-smoke        - Headless USB-style boot smoke test"
	@echo "  qemu-smoke-netboot - Headless HTTP netboot smoke test"
	@echo "  qemu-smoke-timezone - Headless boot with a DHCP time zone (option 101)"
	@echo "  qemu-smoke-timezone-netboot - Same, netbooted (the initramfs lease)"
	@echo "  qemu-smoke-upgrade - Headless first boot of a staged upgrade"
	@echo "  qemu-smoke-addons - Headless boot of a stick with a ticked add-on"
	@echo "  qemu-smoke-two-sticks - Three headless boots with a second stick attached"
	@echo "  test-unit         - Run offline unit tests (no rootfs needed)"
	@echo "  test              - Run integration guard tests against rootfs"
	@echo "  test-runtime      - Run runtime session tests (needs network, compositor)"
	@echo "  clean             - Remove artifacts (FORCE=1 if artifacts is a symlink)"
	@echo "  help              - Show this help"
	@echo ""
	@echo "Variables:"
	@echo "  SQUASHFS_COMP=zstd         - Compression algorithm"
	@echo "  SQUASHFS_COMP_LEVEL=19     - Compression level"
	@echo "  EFI_PARTITION_SIZE=3000    - EFI partition size (MB)"
	@echo "  QEMU_MEMORY=2G             - QEMU RAM allocation"
	@echo "  QCOW2_TEST_SIZE=4G         - Size for qemu-test"
	@echo "  QEMU_EXTRA_ARGS=\"\"          - Additional QEMU arguments"

$(ARTIFACT_DIR) $(ROOTFS_DIR) $(IMAGE_DIR):
	mkdir -p $@

# Find all overlay files (excluding editor backups). The directories are
# prerequisites too: a deleted file takes its own name off OVERLAY_FILES, so
# without them make saw nothing changed and the image kept the deleted file
# (test-container-inputs.sh). Deleting a file changes its directory's mtime.
OVERLAY_FILES := $(shell find overlay -type f ! -name '*~' 2>/dev/null)
OVERLAY_DIRS := $(shell find overlay -type d 2>/dev/null)

# labwc is rebuilt from source inside the ubuntu container, patched with
# every file here (see containers/ubuntu/Dockerfile's labwc-build stage).
# They must be a build input like OVERLAY_FILES: without this, editing or
# adding a patch doesn't invalidate .ubuntu-container, so 'make rootfs'
# reports "Nothing to be done" and ships a stale image that still passes
# the test suite - false-confidence green, not a real pass.
#
# PATCH_FILES is a wildcard, expanded once at parse time: deleting a patch
# removes its own name from this list along with it, so the deleted file's
# prerequisite vanishes too and make sees nothing that changed. The bare
# directory containers/ubuntu/patches, added as a second prerequisite on
# .ubuntu-container below, closes that hole - its mtime changes whenever a
# file is added to or removed from it, even when no surviving patch file
# changed.
BRAND_FILES := $(wildcard brand/*)
PATCH_FILES := $(wildcard containers/ubuntu/patches/*.patch)

# thinfloe-menu is built from source in the ubuntu container's menu-build stage.
# Like PATCH_FILES, its sources must be prerequisites of .ubuntu-container:
# OVERLAY_FILES only finds files under overlay/, and the crate cannot live
# there because overlay/ is copied into the shipped rootfs. Without this,
# editing main.rs leaves the stamp valid, 'make rootfs' reports nothing to
# do, and 'make test' validates a stale binary while printing green. The
# directories are listed as well, so deleting a file still changes a
# prerequisite's mtime.
MENU_FILES := $(shell find src/thinfloe-menu -path src/thinfloe-menu/target -prune -o -type f ! -name '*~' -print 2>/dev/null)
MENU_DIRS := $(shell find src/thinfloe-menu -path src/thinfloe-menu/target -prune -o -type d -print 2>/dev/null)

# Each container stamp holds the ID of the image its build produced, and is
# removed at parse time unless the tag still names exactly that image.
#
# The stamp is per checkout, but the tag is not: every checkout of this repo
# builds thinfloe-ubuntu:$(VERSION). The old check only asked whether the tag
# existed, so after another checkout built and re-tagged the image, this
# checkout's stamp still said "up to date" and `make test` ran its guards
# against the other checkout's image and reported green. A stamp from before
# this check is empty and so is dropped once. Build with a distinct VERSION
# per checkout, or two checkouts will keep rebuilding over each other.
image_id = $(CONTAINER_RUNTIME) image inspect --format '{{.Id}}' $(1)
drop_stale_stamp = $(shell id=$$($(call image_id,$(1)) 2>/dev/null); [ -n "$$id" ] && [ "$$id" = "$$(cat $(2) 2>/dev/null)" ] || rm -f $(2))
$(call drop_stale_stamp,$(UBUNTU_IMAGE),.ubuntu-container)
$(call drop_stale_stamp,$(IMAGE_BUILDER),.image-builder-container)
$(call drop_stale_stamp,$(IPXE_BUILDER),.ipxe-container)

.ubuntu-container: containers/ubuntu/Dockerfile $(wildcard brand) $(BRAND_FILES) $(OVERLAY_FILES) $(OVERLAY_DIRS) $(PATCH_FILES) containers/ubuntu/patches $(MENU_FILES) $(MENU_DIRS) | $(ROOTFS_DIR)
	@echo "Building Ubuntu container..."
	$(CONTAINER_RUNTIME) build --pull --progress=plain --cpu-period=100000 --cpu-quota=$$(($(BUILD_JOBS)*100000)) \
		--build-arg VERSION=$(VERSION) \
		-t $(UBUNTU_IMAGE) \
		-f containers/ubuntu/Dockerfile \
		.
	$(call image_id,$(UBUNTU_IMAGE)) >$@

$(KERNEL) $(INITRD) $(SQUASHFS): .ubuntu-container | $(ROOTFS_DIR)
	@echo "Exporting rootfs artifacts..."
	$(CONTAINER_RUNTIME) run --rm --log-driver=none \
		-v $(PWD)/scripts:/scripts:ro \
		-v $(PWD)/artifacts:/artifacts \
		-e SQUASHFS_COMP=$(SQUASHFS_COMP) \
		-e SQUASHFS_COMP_LEVEL=$(SQUASHFS_COMP_LEVEL) \
		-e VERSION=$(VERSION) \
		-e BUILD_ID=$(BUILD_ID) \
		-e BUILD_JOBS=$(BUILD_JOBS) \
		$(UBUNTU_IMAGE) \
		/scripts/export-rootfs.sh

rootfs: $(KERNEL) $(INITRD) $(SQUASHFS)

# iPXE build
.ipxe-container: containers/ipxe/Dockerfile | $(ROOTFS_DIR)
	@echo "Building iPXE container..."
	$(CONTAINER_RUNTIME) build --pull --progress=plain --cpu-period=100000 --cpu-quota=$$(($(BUILD_JOBS)*100000)) \
		-t $(IPXE_BUILDER) \
		-f containers/ipxe/Dockerfile \
		containers/ipxe
	$(call image_id,$(IPXE_BUILDER)) >$@

$(IPXE_PXE) $(IPXE_KPXE) $(IPXE_EFI): .ipxe-container | $(ROOTFS_DIR)
	@echo "Exporting iPXE boot loaders..."
	$(CONTAINER_RUNTIME) run --rm --log-driver=none \
		-v $(PWD)/artifacts/rootfs:/artifacts \
		$(IPXE_BUILDER)

ipxe: $(IPXE_PXE) $(IPXE_KPXE) $(IPXE_EFI)
	@echo "✓ iPXE boot loaders built"

.image-builder-container: containers/image-builder/Dockerfile | $(IMAGE_DIR)
	@echo "Building image builder container..."
	$(CONTAINER_RUNTIME) build --pull --progress=plain --cpu-period=100000 --cpu-quota=$$(($(BUILD_JOBS)*100000)) \
		-t $(IMAGE_BUILDER) \
		-f containers/image-builder/Dockerfile \
		containers/image-builder
	$(call image_id,$(IMAGE_BUILDER)) >$@

# The files the image is built from, not the phony `rootfs`: through that,
# every target needing the image (qemu-smoke, qemu-test) built it again with
# new random GPT GUIDs, and the .zst from `make image` no longer matched it.
$(USB_IMAGE): $(KERNEL) $(INITRD) $(SQUASHFS) configs/syslinux.cfg \
		scripts/make-bootable-image.sh scripts/lib/common.sh \
		.image-builder-container | $(IMAGE_DIR)
	@echo "Creating bootable USB image..."
	$(CONTAINER_RUNTIME) run --rm --log-driver=none \
		-v $(PWD)/scripts:/scripts:ro \
		-v $(PWD)/artifacts:/artifacts \
		-v $(PWD)/configs:/configs:ro \
		-e EFI_PARTITION_SIZE=$(EFI_PARTITION_SIZE) \
		-e VERSION=$(VERSION) \
		-e BUILD_ID=$(BUILD_ID) \
		-e PROJECT_NAME=$(PROJECT_NAME) \
		$(IMAGE_BUILDER) \
		/scripts/make-bootable-image.sh

$(USB_IMAGE_COMPRESSED): $(USB_IMAGE)
	@echo "Compressing USB image..."
	zstd -f -3 -T$(BUILD_JOBS) $(USB_IMAGE) -o $(USB_IMAGE_COMPRESSED)
	@echo "✓ Compressed: $$(du -h $(USB_IMAGE_COMPRESSED) | cut -f1) (was $$(du -h $(USB_IMAGE) | cut -f1))"

$(USB_CHECKSUM): $(USB_IMAGE_COMPRESSED)
# Record the bare filename, not the path: the checksum is published next
# to the image on the web server, so 'sha256sum -c' must work in whatever
# directory someone downloads the pair into. Naming
# artifacts/images/... made it look for a nested tree that only exists
# in this repo.
	@cd $(IMAGE_DIR) && sha256sum $(notdir $(USB_IMAGE_COMPRESSED)) > $(notdir $(USB_CHECKSUM))
	@echo "✓ Checksum created"

image: $(USB_IMAGE_COMPRESSED) $(USB_CHECKSUM)
	@echo ""
	@echo "To write to USB: zstd -d < $(USB_IMAGE_COMPRESSED) | dd of=/dev/sdX bs=1M status=progress conv=fsync"

# Test upgrade with qcow2 drives
QCOW2_BOOT_IMAGE := $(IMAGE_DIR)/$(PROJECT_NAME)-boot.qcow2
QCOW2_TARGET_IMAGE := $(IMAGE_DIR)/$(PROJECT_NAME)-target.qcow2
QCOW2_EMPTY_IMAGE := $(IMAGE_DIR)/$(PROJECT_NAME)-empty.qcow2
QCOW2_TEST_SIZE ?= 4G

# Always rebuild qcow2 from current USB image (force recreate)
.PHONY: $(QCOW2_BOOT_IMAGE) $(QCOW2_TARGET_IMAGE) $(QCOW2_EMPTY_IMAGE)

$(QCOW2_BOOT_IMAGE): $(USB_IMAGE)
	@echo "Creating boot drive ($(QCOW2_TEST_SIZE) sparse qcow2)..."
	rm -f $(QCOW2_BOOT_IMAGE)
	qemu-img convert -f raw -O qcow2 $(USB_IMAGE) $(QCOW2_BOOT_IMAGE)
	qemu-img resize $(QCOW2_BOOT_IMAGE) $(QCOW2_TEST_SIZE)

$(QCOW2_TARGET_IMAGE): $(USB_IMAGE)
	@echo "Creating target drive ($(QCOW2_TEST_SIZE) sparse qcow2)..."
	rm -f $(QCOW2_TARGET_IMAGE)
	qemu-img convert -f raw -O qcow2 $(USB_IMAGE) $(QCOW2_TARGET_IMAGE)
	qemu-img resize $(QCOW2_TARGET_IMAGE) $(QCOW2_TEST_SIZE)

$(QCOW2_EMPTY_IMAGE):
	@echo "Creating empty drive ($(QCOW2_TEST_SIZE) sparse qcow2)..."
	rm -f $(QCOW2_EMPTY_IMAGE)
	qemu-img create -f qcow2 $(QCOW2_EMPTY_IMAGE) $(QCOW2_TEST_SIZE)

qemu-test: $(QCOW2_BOOT_IMAGE) $(QCOW2_TARGET_IMAGE) $(QCOW2_EMPTY_IMAGE)
	@echo "Starting QEMU with 3 drives for upgrade testing..."
	@echo "  Boot drive:   $(QCOW2_BOOT_IMAGE) (vda) - current ThinFloe"
	@echo "  Target drive: $(QCOW2_TARGET_IMAGE) (vdb) - ThinFloe for A/B test"
	@echo "  Empty drive:  $(QCOW2_EMPTY_IMAGE) (vdc) - fresh install test"
	@echo ""
	@echo "Run 'thinfloe-upgrade' to test upgrade workflow"
	@echo "Press Ctrl+A then X to exit QEMU"
	@echo ""
	qemu-system-x86_64 \
   	        -machine type=q35,accel=kvm \
   	        -cpu host,migratable=off \
   	        -object rng-random,filename=/dev/urandom,id=rng0 \
   	        -device virtio-rng-pci,rng=rng0 \
		-m $(QEMU_MEMORY) \
		-bios /usr/share/ovmf/OVMF.fd \
		-boot c \
		-display gtk \
		-device virtio-vga,xres=$(QEMU_XRES),yres=$(QEMU_YRES) \
		-net nic,model=virtio \
		-net user \
		-enable-kvm \
		-drive file=$(QCOW2_BOOT_IMAGE),format=qcow2,if=virtio \
		-drive file=$(QCOW2_TARGET_IMAGE),format=qcow2,if=virtio \
		-drive file=$(QCOW2_EMPTY_IMAGE),format=qcow2,if=virtio \
		-serial mon:stdio \
		$(QEMU_EXTRA_ARGS)

# Headless boot smoke test. QEMU boots the kernel and initramfs directly with
# -kernel/-initrd rather than going through syslinux. That is deliberate:
# driving the bootloader menu over a serial line is racy, and booting
# directly is deterministic while still exercising everything that matters -
# the kernel, the initramfs, the ThinFloe dracut module, the squashfs mount, the
# overlay setup and systemd startup. Syslinux itself is covered by the
# interactive qemu-test target.
#
# scripts/run-qemu-smoke.sh stops the VM QEMU_SMOKE_GRACE seconds after the
# boot reaches multi-user.target; a boot that never does runs into
# QEMU_SMOKE_TIMEOUT and is failed by check-boot-smoke.sh.
QEMU_SMOKE_LOG := $(IMAGE_DIR)/qemu-smoke.log
QEMU_SMOKE_TIMEOUT ?= 300
QEMU_SMOKE_GRACE ?= 15
# serial-getty@ttyS0 is masked because agetty's terminal reset, written to the
# same serial line, has landed inside systemd's "Reached target
# multi-user.target" line and failed a good boot. Nothing in the smoke test
# logs in on the serial console.
QEMU_SMOKE_APPEND := console=ttyS0,115200 systemd.unit=multi-user.target \
	systemd.log_target=console systemd.mask=serial-getty@ttyS0.service

qemu-smoke: $(QCOW2_BOOT_IMAGE) $(KERNEL) $(INITRD)
	@echo "Running headless boot smoke test..."
	scripts/run-qemu-smoke.sh $(QEMU_SMOKE_LOG) $(QEMU_SMOKE_TIMEOUT) $(QEMU_SMOKE_GRACE) -- \
		qemu-system-x86_64 \
		-machine type=q35,accel=kvm \
		-cpu host,migratable=off \
		-smp $(BUILD_JOBS) \
		-m $(QEMU_MEMORY) \
		-display none \
		-no-reboot \
		-object rng-random,filename=/dev/urandom,id=rng0 \
		-device virtio-rng-pci,rng=rng0 \
		-net nic,model=virtio -net user \
		-drive file=$(QCOW2_BOOT_IMAGE),format=qcow2,if=virtio \
		-kernel $(KERNEL) \
		-initrd $(INITRD) \
		-append "ramroot=tl/ramroot.sqsh $(QEMU_SMOKE_APPEND)"
	@scripts/check-boot-smoke.sh $(QEMU_SMOKE_LOG)

# Headless netboot smoke test: the rootfs artifacts are served over HTTP from
# the host and booted with no disk, as a PXE client would boot them. Besides
# the clean-boot checks it requires the MAC-based hostname, a home.pkg
# request under the boot MAC, and a ramroot URL whose host name only the
# DHCP-supplied DNS server knows. It runs in an unprivileged user and network
# namespace; see scripts/run-qemu-netboot-smoke.sh.
QEMU_NETBOOT_LOG := $(IMAGE_DIR)/qemu-netboot-smoke.log

qemu-smoke-netboot: $(KERNEL) $(INITRD) $(SQUASHFS) | $(IMAGE_DIR)
	@echo "Running headless netboot smoke test..."
	scripts/run-qemu-netboot-smoke.sh $(ROOTFS_DIR) $(QEMU_NETBOOT_LOG) \
		$(QEMU_SMOKE_TIMEOUT) $(QEMU_SMOKE_GRACE) \
		-machine type=q35,accel=kvm \
		-cpu host,migratable=off \
		-smp $(BUILD_JOBS) \
		-m 3G \
		-display none \
		-no-reboot \
		-object rng-random,filename=/dev/urandom,id=rng0 \
		-device virtio-rng-pci,rng=rng0

# Headless boot on a tap device whose DHCP server (dnsmasq, unprivileged, in a
# user and network namespace) sends time zone Europe/Zurich as option 101.
# Requires the line "thinfloe: time zone Europe/Zurich from DHCP" on the
# console. See scripts/run-qemu-timezone-smoke.sh.
QEMU_TIMEZONE_LOG := $(IMAGE_DIR)/qemu-timezone-smoke.log

qemu-smoke-timezone: $(QCOW2_BOOT_IMAGE) $(KERNEL) $(INITRD)
	@echo "Running headless time zone smoke test..."
	scripts/run-qemu-timezone-smoke.sh $(QEMU_TIMEZONE_LOG) \
		$(QEMU_SMOKE_TIMEOUT) $(QEMU_SMOKE_GRACE) \
		-machine type=q35,accel=kvm \
		-cpu host,migratable=off \
		-smp $(BUILD_JOBS) \
		-m $(QEMU_MEMORY) \
		-display none \
		-no-reboot \
		-object rng-random,filename=/dev/urandom,id=rng0 \
		-device virtio-rng-pci,rng=rng0 \
		-drive file=$(QCOW2_BOOT_IMAGE),format=qcow2,if=virtio \
		-kernel $(KERNEL) \
		-initrd $(INITRD) \
		-append "ramroot=tl/ramroot.sqsh $(QEMU_SMOKE_APPEND)"

# The same over a netboot: the initramfs's networkd gets the lease, so its
# drop-in (90thinfloe/module-setup.sh) has to ask for option 101.
QEMU_TIMEZONE_NETBOOT_LOG := $(IMAGE_DIR)/qemu-timezone-netboot-smoke.log

qemu-smoke-timezone-netboot: $(KERNEL) $(INITRD) $(SQUASHFS) | $(IMAGE_DIR)
	@echo "Running headless netboot time zone smoke test..."
	TF_TZ_NETBOOT_DIR=$(ROOTFS_DIR) scripts/run-qemu-timezone-smoke.sh \
		$(QEMU_TIMEZONE_NETBOOT_LOG) $(QEMU_SMOKE_TIMEOUT) $(QEMU_SMOKE_GRACE) \
		-machine type=q35,accel=kvm \
		-cpu host,migratable=off \
		-smp $(BUILD_JOBS) \
		-m 3G \
		-display none \
		-no-reboot \
		-object rng-random,filename=/dev/urandom,id=rng0 \
		-device virtio-rng-pci,rng=rng0

# Headless first boot of a staged upgrade: tl.new/ on the stick and
# syslinux.cfg on DEFAULT new, as thinfloe-upgrade leaves them, booted through
# OVMF and syslinux. Requires the initramfs to rotate tl.new into tl/ and set
# DEFAULT current again. See scripts/run-qemu-upgrade-smoke.sh.
QEMU_UPGRADE_LOG := $(IMAGE_DIR)/qemu-upgrade-smoke.log
QEMU_UPGRADE_IMAGE := $(IMAGE_DIR)/$(PROJECT_NAME)-upgrade-smoke.img

qemu-smoke-upgrade: $(USB_IMAGE)
	@echo "Running headless upgrade boot smoke test..."
	BUILD_JOBS=$(BUILD_JOBS) QEMU_MEMORY=$(QEMU_MEMORY) \
		scripts/run-qemu-upgrade-smoke.sh $(USB_IMAGE) $(QEMU_UPGRADE_IMAGE) \
		$(QEMU_UPGRADE_LOG) $(QEMU_SMOKE_TIMEOUT) $(QEMU_SMOKE_GRACE) \
		"$(QEMU_SMOKE_APPEND)"

# Headless boot of a stick that carries a ticked add-on (a tiny .deb in
# addons/90-smoke/ on the ESP). Requires thinfloe-addons.service to install it
# before getty@tty1.service starts, with no network. See
# scripts/run-qemu-addons-smoke.sh.
QEMU_ADDONS_LOG := $(IMAGE_DIR)/qemu-addons-smoke.log
QEMU_ADDONS_IMAGE := $(IMAGE_DIR)/$(PROJECT_NAME)-addons-smoke.img

qemu-smoke-addons: $(USB_IMAGE) $(KERNEL) $(INITRD)
	@echo "Running headless add-ons boot smoke test..."
	BUILD_JOBS=$(BUILD_JOBS) QEMU_MEMORY=$(QEMU_MEMORY) \
		scripts/run-qemu-addons-smoke.sh $(USB_IMAGE) $(QEMU_ADDONS_IMAGE) \
		$(KERNEL) $(INITRD) $(QEMU_ADDONS_LOG) $(QEMU_SMOKE_TIMEOUT) \
		$(QEMU_SMOKE_GRACE) "$(QEMU_SMOKE_APPEND)"

# Three headless boots through OVMF from a fresh stick while a second stick,
# one that already carries a stick id, is attached. Requires every boot to
# mount the ESP of the stick it booted from (not the other one, which the
# partlabel link could name) and that stick to keep the id its first boot
# gave it. See scripts/run-qemu-two-sticks-smoke.sh.
QEMU_TWO_STICKS_LOG := $(IMAGE_DIR)/qemu-two-sticks-smoke.log
QEMU_TWO_STICKS_PREFIX := $(IMAGE_DIR)/$(PROJECT_NAME)-two-sticks

qemu-smoke-two-sticks: $(USB_IMAGE)
	@echo "Running headless two-sticks boot smoke test..."
	BUILD_JOBS=$(BUILD_JOBS) QEMU_MEMORY=$(QEMU_MEMORY) \
		scripts/run-qemu-two-sticks-smoke.sh $(USB_IMAGE) $(QEMU_TWO_STICKS_PREFIX) \
		$(QEMU_TWO_STICKS_LOG) $(QEMU_SMOKE_TIMEOUT) $(QEMU_SMOKE_GRACE) \
		"$(QEMU_SMOKE_APPEND)"

# Host-side tests. No image, no container, no network: the fast feedback
# loop for the scripts in overlay/usr/bin and for thinfloe-menu's rules.
# Deliberately not dependent on 'rootfs'.
#
# MEMCAP bounds the cargo tests: the tile-file and icon tests feed
# deliberately oversized input, and a broken bound must fail the test rather
# than take the machine's memory. Set MEMCAP= where systemd-run is missing.
MENU_DIR := src/thinfloe-menu
MEMCAP ?= systemd-run --user --scope -q -p MemoryMax=2G --
test-unit:
	@python3 -m unittest discover -s test/unit -v
	cd $(MENU_DIR) && CARGO_BUILD_JOBS=$(BUILD_JOBS) $(MEMCAP) cargo test --locked -j $(BUILD_JOBS)

test: test-unit rootfs
	@test/integration/test-firmware.sh
	@test/integration/test-wifi-stack.sh
	@TF_UBUNTU_IMAGE=$(UBUNTU_IMAGE) test/integration/test-session-packages.sh
	@test/integration/test-squashfs-format.sh
	@test/integration/test-labwc-config-merge.sh
	@test/integration/test-initramfs-home.sh
	@test/integration/test-esp-choice.sh
	@test/integration/test-initramfs-commands.sh
	@test/integration/test-terminal.sh
	@test/integration/test-boot-smoke-check.sh
	@test/integration/test-ssh-hostkeys.sh
	@test/integration/test-save-home.sh
	@test/integration/test-password.sh
	@test/integration/test-dhcp-timezone.sh
	@test/integration/test-field-report.sh
	@test/integration/test-cmdline.sh
	@test/integration/test-session-lifecycle.sh
	@test/integration/test-container-stamp.sh
	@test/integration/test-usb-image-deps.sh
	@test/integration/test-container-inputs.sh
	@test/integration/test-format-bytes.sh
	@test/integration/test-plymouth-step.sh
	@test/integration/test-make-clean.sh
	@TF_UBUNTU_IMAGE=$(UBUNTU_IMAGE) test/integration/test-addon-deps.sh

# Runtime session tests. Needs network (installs python3-xlib into a
# test-only image) and runs a compositor, so it is deliberately not part of
# 'make test'.
test-runtime: rootfs
	$(CONTAINER_RUNTIME) build --progress=plain \
		--cpu-period=100000 --cpu-quota=$$(($(BUILD_JOBS)*100000)) \
		--build-arg BASE=$(UBUNTU_IMAGE) \
		-t $(PROJECT_NAME)-runtime-test:$(VERSION) \
		-f test/runtime/Dockerfile \
		test/runtime
	@test/runtime/test-labwc-runtime.sh $(PROJECT_NAME)-runtime-test:$(VERSION)

# A symlinked artifacts directory points somewhere this checkout may not own
# alone: `rm -rf artifacts/*` follows the link and empties the target, other
# checkouts' builds included. clean therefore refuses it unless FORCE=1. The
# stamps are this checkout's own and are always removed.
clean:
	rm -f .ubuntu-container .image-builder-container .ipxe-container
	@if [ -L "$(ARTIFACT_DIR)" ] && [ "$(FORCE)" != 1 ]; then \
		echo "$(ARTIFACT_DIR) is a symlink to $$(readlink "$(ARTIFACT_DIR)") - not emptying it."; \
		echo "Removed the container stamps only. To empty the link target too:"; \
		echo "  make clean FORCE=1"; \
		exit 1; \
	fi
	rm -rf $(ARTIFACT_DIR)/*
	@echo "✓ Cleaned artifacts"

