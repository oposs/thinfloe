This note covers netboot: how the initramfs gets its network, the boot MAC, and the netboot smoke test.

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
The running system's `/etc/resolv.conf` is the image's link to
`../run/systemd/resolve/stub-resolv.conf`. The Dockerfile installs
`systemd-resolved` in a `RUN --network=none`, where podman mounts no
resolv.conf of its own, so the package's postinst can make the link (in a
networked RUN it printed "mv: cannot move '/etc/resolv.conf'"), and
`export-rootfs.sh` refuses an image without it. Guard:
`test-session-packages.sh`.
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
