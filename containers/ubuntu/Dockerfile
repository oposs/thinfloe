# Local labwc rebuild.
#
# Ubuntu's labwc 0.9.3 clamps fullscreen windows to a single output and does
# not implement zwp_xwayland_keyboard_grab_manager_v1. Both are patched here;
# see docs/superpowers/specs/2026-07-27-labwc-multimonitor-fullscreen-design.md.
#
# Patches live in containers/ubuntu/patches/ and are appended to the source
# package's quilt series in filename order. dpkg-buildpackage applies them, so
# a patch that no longer applies fails the build loudly instead of being
# silently skipped.
FROM ubuntu:26.04 AS labwc-build
ARG DEBIAN_FRONTEND=noninteractive
ENV DEBEMAIL=thinfloe@localhost DEBFULLNAME=thinfloe

RUN sed -i 's/^Types: deb$/Types: deb deb-src/' \
        /etc/apt/sources.list.d/ubuntu.sources && \
    apt-get update && \
    apt-get install -yq --no-install-recommends \
        build-essential devscripts dpkg-dev quilt ca-certificates && \
    apt-get build-dep -yq labwc

WORKDIR /build
RUN apt-get source labwc

COPY containers/ubuntu/patches/ /patches/
RUN set -eu; \
    cd /build/labwc-*/; \
    mkdir -p debian/patches; \
    touch debian/patches/series; \
    for p in /patches/*.patch; do \
        [ -e "$p" ] || continue; \
        cp "$p" debian/patches/; \
        basename "$p" >> debian/patches/series; \
    done; \
    dch --local +thinfloe --distribution unstable "thinfloe local patches"; \
    dpkg-buildpackage -b -uc -us; \
    mkdir -p /out; \
    cp /build/labwc_*.deb /out/

# thinfloe-menu, the tile grid that is the body of the session. See
# docs/superpowers/specs/2026-10-01-tile-menu-design.md.
#
# The toolchain comes from rustup, pinned by the crate's rust-toolchain.toml:
# egui 0.36 needs Rust 1.95 and Ubuntu 26.04's archive rustc is 1.93.
# Cargo.lock is committed and --locked makes a drifted lock fail the build
# instead of silently resolving something untested.
FROM ubuntu:26.04 AS menu-build
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && \
    apt-get install -yq --no-install-recommends rustup ca-certificates gcc libc6-dev
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
COPY src/thinfloe-menu/ /build/thinfloe-menu/
WORKDIR /build/thinfloe-menu
RUN set -eu; \
    toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml); \
    rustup toolchain install "$toolchain" --profile minimal; \
    CARGO_BUILD_JOBS=4 rustup run "$toolchain" cargo build --release --locked -j 4; \
    install -D -m 0755 target/release/thinfloe-menu /out/usr/bin/thinfloe-menu; \
    install -D -m 0644 -t /out/usr/share/thinfloe/icons icons/*.svg icons/LICENSE; \
    install -D -m 0644 egui_shadcn/assets/Oxanium-OFL.txt /out/usr/share/thinfloe/fonts/Oxanium-OFL.txt

FROM ubuntu:26.04
ARG DEBIAN_FRONTEND=noninteractive

# Update and install base packages in single layer to avoid stale cache.
#
# xcursor-themes ships Xcursor pointer theme files (whiteglass, redglass,
# handhelds) in this image. It used to arrive transitively via lxterminal
# (GTK) / xorg; both are gone now. Installing the package alone is not
# enough to get a visible pointer, though: it registers its themes with
# update-alternatives at priority 20, below x11-common's core.theme at
# priority 30, so the x-cursor-theme alternative still resolves to
# core.theme - which inherits from "core", the X server's built-in cursor
# font, meaningless to a Wayland compositor. XCURSOR_THEME in
# overlay/etc/thinfloe/labwc/environment is what actually selects
# xcursor-themes' whiteglass theme; both the package and that setting are
# required.
RUN apt-get update && apt-get upgrade -y && apt-get install -yq \
    --no-install-recommends --no-install-suggests \
    systemd-sysv \
    systemd-resolved \
    vim \
    wireguard \
    resolvconf-admin \
    openssh-server \
    sudo \
    curl \
    ca-certificates \
    iproute2 \
    iw \
    wpasupplicant \
    netplan.io \
    less \
    chrony \
    tzdata \
    joe \
    net-tools \
    pciutils \
    iputils-ping \
    locales \
    rsync \
    unzip \
    bluetooth \
    xwayland \
    foot \
    foot-terminfo \
    grim \
    slurp \
    wl-clipboard \
    kanshi \
    wlr-randr \
    swayidle \
    wlopm \
    waybar \
    xfonts-base \
    fonts-dejavu-core \
    xcursor-themes \
    dialog \
    pipewire \
    pipewire-pulse \
    pipewire-alsa \
    pipewire-audio \
    wireplumber \
    libspa-0.2-bluetooth \
    pulseaudio-utils \
    alsa-utils \
    plymouth \
    plymouth-themes \
    plymouth-theme-spinner \
    intel-microcode \
    linux-image-generic \
    linux-firmware-minimal \
    linux-firmware-intel-graphics \
    linux-firmware-intel-wireless \
    linux-firmware-realtek \
    firmware-sof-signed \
    dracut \
    dracut-network \
    squashfs-tools \
    zstd \
    wget \
    dosfstools \
    parted \
    fdisk \
    gdisk \
    python3 \
    vainfo \
    intel-media-va-driver \
    i965-va-driver

# Set ping setuid
RUN chmod u+s /usr/bin/ping

# Install ThinLinc
RUN wget https://www.cendio.com/downloads/clients/thinlinc-client_4.21.0-4588_amd64.deb && \
    apt install -y ./thinlinc-client_4.21.0-*_amd64.deb && \
    rm -f thinlinc-client_*.deb

# Install locally built packages staged by the Makefile into vendor/
COPY vendor/oxulnk-desktop.deb /tmp/oxulnk-desktop.deb
RUN apt-get install -yq --no-install-recommends /tmp/oxulnk-desktop.deb && \
    rm -f /tmp/oxulnk-desktop.deb

# labwc comes from the local rebuild above, not the archive. apt resolves its
# dependencies from the .deb's own control fields. 0.9.3-1+thinfloe1 sorts above
# 0.9.3-1, so a later apt-get upgrade will not replace it.
COPY --from=labwc-build /out/ /tmp/labwc-deb/
RUN apt-get install -yq --no-install-recommends --no-install-suggests /tmp/labwc-deb/labwc_*.deb && \
    rm -rf /tmp/labwc-deb

# thinfloe-menu and its icons, from the menu-build stage above. The tiles
# themselves are plain files in overlay/etc/thinfloe/menu.
COPY --from=menu-build /out/ /

# Lock root rather than blanking it.
#
# `passwd -d root` alone leaves the shadow field EMPTY, and Ubuntu's
# /etc/pam.d/common-auth carries `nullok`, so root logged in at a VT by
# pressing Enter. sshd refused it only because PermitEmptyPasswords defaults
# to no, which made the hole invisible from the network and easy to keep.
# `-d` then `-l` gives a field of `!`: no hash to recover and nothing to
# authenticate with. Recovery at the machine goes through tfuser's autologin
# on tty1 and its passwordless sudo.
RUN passwd -d root && passwd -l root

# Create the session user with NO password.
#
# There used to be an `echo "tluser:tluser" | chpasswd` here. Every image in
# the field therefore shipped the same login, and because tfuser has
# NOPASSWD:ALL below, anyone who could reach port 22 and knew that default had
# root on every deployed machine. It also contradicted
# overlay/usr/bin/thinfloe-session, which already documents "tfuser has no
# password" as the reason the session has no lock screen.
#
# --disabled-password leaves `!` in the shadow field, so password
# authentication cannot succeed. A person at the machine sets their own with
# `sudo thinfloe-password`, which stores the hash in the home directory so it
# survives a reboot. Do not reintroduce a shipped password here: make it a
# per-machine secret or none at all.
#
# `passwd -l` would NOT be a substitute for leaving it unset. It prefixes the
# existing hash with `!` and leaves it recoverable.
RUN adduser --disabled-password --gecos "ThinUser" tfuser && \
    adduser tfuser sudo && \
    echo "tfuser ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers && \
    adduser tfuser audio

# Apply overlay
ADD overlay/ /__overlay
# netplan refuses world-readable config ("Permissions for ... are too open")
# and git cannot store mode 0600, so set it here.
RUN tar cf - --exclude="*~" -C /__overlay . | tar xf - && rm -rf /__overlay \
    && chmod 600 /etc/netplan/*.yaml
RUN chown -R tfuser:tfuser /home/tfuser
# The account was called tluser until 4.0; saved files and local notes may
# still name /home/tluser.
RUN ln -s tfuser /home/tluser

# Generate locale
RUN locale-gen

# Configure Plymouth theme to bgrt (keeps vendor logo, shows spinner below)
# The default bgrt theme works with EFI framebuffer
RUN printf '#!/bin/sh\nupdate-alternatives --query default.plymouth 2>/dev/null | grep "^Value:" | cut -d/ -f6 || echo bgrt\n' > /usr/sbin/plymouth-set-default-theme && \
    chmod +x /usr/sbin/plymouth-set-default-theme

# Remove SSH keys and enable services
# ssh.service is enabled explicitly even though 26.04 ships sshd as
# socket-activated (ssh.socket is enabled by the package). Socket activation
# alone means sshd only starts on the first inbound connection, so a broken
# sshd config or a missing host key shows up as a failed login rather than a
# logged unit failure at boot. Starting it eagerly makes such problems visible
# in `journalctl -b` on a machine whose graphical session may not be coming up.
# seatd is disabled deliberately. It arrives as a labwc dependency and enables
# itself, but this image has systemd-logind, which is the correct seat manager
# here: logind grants the active session ACL access to /dev/dri and /dev/input.
# With seatd also running, wlroots' libseat tries the seatd backend first, hits
# "could not access socket /run/seatd.sock: permission denied" because tfuser is
# in no seat group, and only then falls back to logind. That produced an alarming
# error on every start and left DRM access depending on which backend won.
# On 26.04 labwc pulls in libseat1 only and seatd is not installed; the guard
# stays for the day a package brings it back. `systemctl disable` on the
# missing unit printed "Unit seatd.service does not exist" on every build.
RUN if [ -e /usr/lib/systemd/system/seatd.service ]; then \
        systemctl disable seatd.service; \
    fi

# waybar is started by thinfloe-session, not by systemd. The package ships a user
# unit enabled into graphical-session.target whose ExecStart is a bare
# `/usr/bin/waybar` with no -c/-s, so if that target ever became active it
# would run waybar with its DEFAULT config - a full desktop panel over the
# ThinLinc client - and Restart=on-failure would keep bringing it back.
# It is dormant today only because Requisite=graphical-session.target fails
# when nothing activates that target, and labwc has no systemd session
# integration. Disabling it removes the ambiguity: thinfloe-session owns waybar's
# lifetime, and the trap in that script is what stops it.
RUN systemctl --global disable waybar.service || true

# ssh.socket is disabled AND masked, in that order, on purpose.
#
# sshd had two entry points with different ordering - the socket from
# sockets.target and the service from multi-user.target - and the host key
# unit has to run before sshd. Since a DefaultDependencies=yes unit is
# implicitly ordered after basic.target, and basic.target comes after
# sockets.target and therefore after ssh.socket, "Before=ssh.socket" made
# that unit both before and after the same target. systemd breaks such a
# cycle by deleting a job, and which job it picks is not contractual: under
# QEMU it deleted sockets.target and sshd came up fine, while on hardware it
# deleted the key generation service, leaving no host keys and no sshd.
#
# disable BEFORE mask is required, not stylistic. Ubuntu's ssh.socket
# declares "RequiredBy=ssh.service" in its [Install] section, so enabling it
# creates /etc/systemd/system/ssh.service.requires/ssh.socket. Masking the
# socket while that symlink exists makes ssh.service fail outright with
# "Unit ssh.socket is masked". systemctl disable removes both that symlink
# and the sockets.target.wants one; the mask then stops anything re-enabling
# it. Masking first would leave the requires symlink in place, because
# disable is a no-op on an already-masked unit.
#
# sshd-keygen.service must ALSO be masked. It ships enabled via Wants= in
# BOTH ssh.socket.wants/ and ssh.service.wants/ - disabling ssh.socket only
# removes the first symlink, and the second survives independently. Left
# alone, it races thinfloe-ssh-hostkeys.service on a genuine first boot (its
# ConditionFirstBoot=yes; neither unit orders itself against the other, both
# only declare Before=ssh.service). If sshd-keygen.service wins that race it
# writes keys straight into /etc/ssh, thinfloe-ssh-hostkeys.service then finds
# live keys already present and skips generation - so those keys never get
# staged into the keystore and are lost on the next boot, reproducing the
# regenerate-every-boot bug this unit exists to fix. It is only a Wants=
# (soft) dependency of ssh.service, so masking it cannot break sshd startup
# the way masking ssh.socket alone would have.
#
# systemd-hwdb-update.service is masked because systemd/udev itself already
# ships a prebuilt usr/lib/udev/hwdb.bin in this image (confirmed via
# `unsquashfs -l ramroot.sqsh | grep hwdb.bin`); regenerating it at boot
# would only write megabytes into the ZRAM overlay for no benefit. This is
# unrelated to the un-dockerize.service ordering-cycle fix above - that
# service no longer declares Before=systemd-hwdb-update.service at all - but
# masking it here still avoids the pointless regeneration now that nothing
# stops the unit from actually running.
RUN rm -f /etc/ssh/ssh_host_*_key* && \
    systemctl disable ssh.socket || true && \
    systemctl mask ssh.socket || true && \
    systemctl mask sshd-keygen.service || true && \
    systemctl enable thinfloe-ssh-hostkeys.service || true && \
    systemctl enable thinfloe-password.service || true && \
    systemctl enable ssh.service || true && \
    systemctl enable un-dockerize.service || true && \
    systemctl enable systemd-networkd && \
    systemctl enable systemd-resolved && \
    systemctl enable thinfloe-wait-kms.service || true && \
    systemctl enable thinfloe-addons.service && \
    systemctl enable thinfloe-addon-check.timer && \
    systemctl mask systemd-hwdb-update.service || true

# Build initramfs with dracut
# Keep it generic but omit unnecessary modules.
#
# --no-hostonly is required explicitly (not just implied by omitting --hostonly):
# dracut's own default is "hostonly=-h" unless hostonly is explicitly set to "no"
# (see /usr/bin/dracut: "hostonly shell variable defaults to -h if not specified").
# With that default active, every instmods call - including the ones in the thinfloe
# module below - gets silently filtered against a "hostonly module list" that dracut
# auto-detects from whatever happens to be loaded on the machine BUILDING the image
# (via the container's leaked /sys, since podman shares the host kernel). On this
# build host that dropped zram (not loaded here) and would have silently dropped
# needed NIC drivers (e1000/e1000e/igb/r8169/atlantic/iwlwifi) too, depending on
# what the build host happens to have loaded - fine on one machine, broken on
# another. --no-hostonly forces the true generic path this comment always intended.
#
# systemd-networkd and systemd-resolved bring up the network for netboot
# (http ramroot=). dracut 110 has no network-legacy module, and the thinfloe
# hook's own dhclient call never worked: the initramfs had no dhclient-script.
# networkd also pulls in kernel-network-modules, so every NIC driver is in the
# initrd, not only the handful 90thinfloe lists; resolved makes a ramroot URL
# with a host name resolve.
RUN KERNEL_VERSION=$(ls /lib/modules | head -1) && \
    dracut --force --no-hostonly --add "thinfloe plymouth systemd-networkd systemd-resolved" \
    --omit "drm dash crypt dmraid multipath iscsi nfs nbd fcoe fcoe-uefi biosdevname dmsquash-live dmsquash-live-ntfs" \
    --omit-drivers "btrfs xfs reiserfs jfs ocfs2 gfs2 f2fs bcachefs cramfs minix hfs hfsplus qnx4 qnx6 ufs nouveau radeon amdgpu i915 vmwgfx qxl virtio-gpu bochs" \
    --compress "zstd -3 -T4" \
    --kver $KERNEL_VERSION

# Cleanup. Firmware content is determined by package selection alone -
# see the linux-firmware-minimal + per-vendor packages above. Never
# re-introduce a find/rm sweep over /usr/lib/firmware.
RUN rm -rf /var/lib/apt/lists/* /usr/share/doc/* /usr/share/man/* && \
    apt-get -qqy clean && apt-get -qqy autoremove
