This note keeps the full story behind each standing constraint in CLAUDE.md: the bug it fixed, the measurements, and what was and was not verified on hardware. Headings match CLAUDE.md.

Twenty rules in this repository look like ordinary configuration but are
load-bearing. All but the patched-labwc, hook-order, menu-rendering,
logout-save, migrate-once and add-on-libraries ones have each caused a real shipped-image bug. The
patched-labwc one is preventive - nothing has shipped broken from it yet, but
reverting it silently would ship regressions in both patched behaviours. The
hook-order one was caught in QEMU: the race it closes is present in every
image built before it. The menu-rendering and logout-save ones are preventive
too: they were decided while the menu was designed, and no image has shipped
without them. The migrate-once and add-on-libraries ones are
preventive as well: nothing has shipped broken from them. The stick-id one fixes a bug every
image before it shipped with; it was found in QEMU, whose `qemu-test`
attaches a second stick.
The tty1 one is preventive too: the Plymouth crash it avoids was found in
QEMU before any image kept the boot picture up.
The ESP-grow one is preventive: it was decided before the grow first ran.

## No login password ships in the image

`containers/ubuntu/Dockerfile` creates tfuser with `adduser
--disabled-password` and nothing else. There used to be an `echo
"tluser:tluser" | chpasswd` on the next line. Re-adding any shipped password
gives every machine in the field the same login, and because tfuser has
`NOPASSWD:ALL` two lines below, that login is root: anyone who could reach
port 22 and knew the default owned every deployed machine. It also
contradicted `overlay/usr/bin/thinfloe-session`, which already documented
"tfuser has no password" as the reason the session has no lock screen.

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
See [Persistent Home Directory](home-persistence.md) for how it
survives a reboot. `test/integration/test-session-packages.sh` guards the
shadow fields, both sshd settings and the unit, and
`test/integration/test-password.sh` guards the script - both run via `make
test`.

**Verified on a QEMU boot (2026-10-01):** `getty@tty1` started exactly once
across a 300 s boot, so `login -f tfuser` succeeds against a `!` field. A
failing autologin would have restarted it every 5 seconds
(`RestartSec=5`, `StartLimitIntervalSec=0`). Not verified on hardware.

## Firmware is selected by package, never by cleanup

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

The one exception is `containers/ubuntu/slim/slim-rootfs.sh`, the last step
of the image build (owner, 2026-11-12). It keeps, per Intel WiFi series, only
the ucode file the driver loads (the rule is in
`containers/ubuntu/slim/iwlwifi_keep.py`) and removes the compat symlinks
that pointed at the dropped files. It also removes `intel-ucode` from the
squashfs after dracut has built the early microcode archive (O3): the kernel
applies the microcode from the initrd. The build stops if
`kernel/x86/microcode/GenuineIntel.bin` is not in `/boot/initrd.img-<kver>`;
`test-firmware.sh` checks that the squashfs has no `intel-ucode` and
`test-initramfs-drivers.sh` that the initrd carries the Intel microcode.
The iwlwifi cut is guarded by `test/unit/test_iwlwifi_keep.py`, the Intel
WiFi checks in `test-firmware.sh` (at most one ucode per series, none above
the driver's maximum, no dangling firmware symlink, a ucode for every chip
in the field), and the selector itself, which stops the build rather than
drop a file that `modinfo` names. Two build-time checks in `slim-rootfs.sh`
cover what the cuts must not touch: every firmware file a module still asks
for must still exist unless a cut dropped it on purpose, and no firmware
link may dangle, before or after the cuts. See O1, O3 and A1 in
`docs/superpowers/specs/2026-11-12-image-size-design.md`.

The same step keeps, of `linux-firmware-amd-graphics`, only the firmware of
AMD APUs (A1, owner, 2026-11-12: Raven and newer, so that AMD mini PCs
show their picture; owner, 2026-11-13: also Carrizo and Stoney, for HP t530
and t630 thin clients; graphics cards get no firmware). The rule is in
`containers/ubuntu/slim/amdgpu_keep.py`: a file stays when its name starts
with an APU chip name (`carrizo_`, `stoney_`, `raven_`, `picasso_`,
`renoir_`, `green_sardine_`, `yellow_carp_`, `vangogh_` and more), with an IP version of an APU row of
the kernel's table (`gc_11_5_0`, `dcn_3_5`, `psp_14_0_0`; MP0 is `psp`) or
with an APU-only block (`vpe`, `umsch_mm`, `isp`), plus the target of every
kept link. Everything else in `amdgpu/` goes, and `radeon/` whole. The APU
and dGPU tables are copies of `Documentation/gpu/amdgpu/` of the kernel
(`amdgpu-apu-ip.csv`, `amdgpu-dgpu-ip.csv`, tag `v7.0`) and must be
refreshed from the kernel when it changes. The build stops when the table
has fewer than 15 APU rows or fewer than 100 files are kept, when the
package ships files for a `gc`, `psp`, `dcn`, `vcn` or `sdma` version that
`modinfo` names and neither table knows (exit 3, the new-APU guard), when a
`psp` version with a `toc` and no `sos` file (the APU pattern) is not kept,
and when a file `modinfo` names under a kept prefix is missing after the
prune. `amdgpu` stays out of the initrd. No QEMU test runs `amdgpu`: a
missing file shows only on a real AMD machine, where the boot waits
about 20 s and then runs the menu unaccelerated on simpledrm or, worst case, shows
the failure screen after about 40 s. `test-firmware.sh` checks a pinned list
of files per APU and that no `navi10_` file and no `radeon/` is left.

## The WiFi core stack must come from the kernel, never from a backport

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

## dracut must be invoked with --no-hostonly

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

## labwc must have zero keybindings

`overlay/etc/thinfloe/labwc/rc.xml` has a `<keyboard>` section with no
`<default />` and no `<keybind>` entries. This is mandatory. The config
directory lives at `/etc/thinfloe/labwc`, not under `$HOME` - see
[Persistent Home Directory](home-persistence.md) for why - and is
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

## ssh.socket must stay masked

`containers/ubuntu/Dockerfile` runs `systemctl disable ssh.socket` then
`systemctl mask ssh.socket`, and enables `ssh.service` alone. Re-enabling the
socket reintroduces an ordering cycle.

`thinfloe-ssh-hostkeys.service` must run before sshd. With the socket active,
sshd has two entry points with different ordering (the socket from
`sockets.target`, the service from `multi-user.target`), so the key unit had
to be ordered `Before=ssh.socket`. But a `DefaultDependencies=yes` unit is
implicitly ordered `After=basic.target`, and `basic.target` comes after
`sockets.target`, which comes after `ssh.socket`. Before and after the same
unit is a cycle, and systemd resolves a cycle by deleting a job, and which job
is not contractual: under QEMU it deleted `sockets.target` and sshd came up
fine, while on real hardware it deleted the key-generation service, leaving
no host keys and no sshd.

`disable` before `mask` is required, not stylistic. Ubuntu's `ssh.socket`
declares `RequiredBy=ssh.service` in its `[Install]` section, so once it is
enabled, `/etc/systemd/system/ssh.service.requires/ssh.socket` exists.
Masking the socket while that symlink is still there makes `ssh.service`
fail outright with "Unit ssh.socket is masked". `disable` removes the
symlink (and the `sockets.target.wants` one) first; `mask` then stops
anything from re-enabling it. Doing `mask` first would leave the requires
symlink in place, since `disable` is a no-op on an already-masked unit.

`sshd-keygen.service` must ALSO be masked, for a reason that's easy to
miss: it ships enabled via `Wants=` from **both** `ssh.socket.wants/` and
`ssh.service.wants/`, so disabling `ssh.socket` only removes the first of
those two symlinks; the second survives on its own. Left alone, it races
`thinfloe-ssh-hostkeys.service` on a genuine first boot: its
`ConditionFirstBoot=yes`, and neither unit is ordered against the other,
both only declaring `Before=ssh.service`. If `sshd-keygen.service` wins, it
writes keys straight into `/etc/ssh`; `thinfloe-ssh-hostkeys.service` then
finds live keys already present and skips generation, so those keys are
never staged into the keystore and are lost on the next boot, reproducing
the regenerate-every-boot bug this whole mechanism exists to fix. It is only
a `Wants=` (soft) dependency of `ssh.service`, so masking it cannot break
sshd startup the way masking `ssh.socket` alone would have.

`test/integration/test-session-packages.sh` guards all three (the
`ssh.socket` mask, the absence of `ssh.service.requires/ssh.socket`, and the
`sshd-keygen.service` mask), run via `make test`.

## A multi-user.target unit must never declare Before= on a sysinit.target unit

A unit that is only `WantedBy=multi-user.target` (no explicit
`DefaultDependencies=` or `Before=`/`After=` of its own) is implicitly
ordered `After=basic.target`, which is itself ordered after
`sysinit.target`. Giving such a unit `Before=<something in sysinit.target>`
therefore orders it both before and after the same target, and systemd
resolves the resulting cycle by silently deleting one of the jobs, on every
single boot, not just the first.

Two real instances of this bug have shipped in this repo:

- The predecessor `regenerate_ssh_host_keys.service` (deleted on this
  branch, replaced by `thinfloe-ssh-hostkeys.service`) was ordered
  `Before=ssh.socket` (which pulls in `sockets.target`, itself before
  `basic.target`); see "ssh.socket must stay masked" above for the full
  cycle and its fix.
- `un-dockerize.service` was ordered `Before=systemd-hwdb-update.service`
  (part of `sysinit.target`). systemd broke the cycle by deleting the hwdb
  update job on every boot; nothing `un-dockerize.service` did (resolv.conf,
  `/etc/hosts`, `systemd-resolved`; since 4.3 only `/etc/hosts`) had
  anything to do with the hardware database. The `Before=` was vestigial, left over from commented-out
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
`Before=systemd-hwdb-update` line. Both run via `make test`.

## labwc is a local rebuild, not the archive package

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
The same holds for deleting a file: the directories of `overlay/` and
`brand/` are prerequisites too (`OVERLAY_DIRS`), because a deleted file
leaves the file list with it, and the image kept a removed systemd drop-in
until they were added. `test/integration/test-container-inputs.sh` guards
this.

The same false green has a second source: the image tag. `.ubuntu-container`
belongs to one checkout, but every checkout builds `thinfloe-ubuntu:$(VERSION)`,
so another checkout's build replaces the image under this checkout's stamp.
Each container stamp therefore holds the image ID its build produced, and the
`Makefile` drops it at parse time when the tag names a different image or
none. Build with a distinct `VERSION` per checkout, or two checkouts rebuild
over each other. `test/integration/test-container-stamp.sh` guards this - run
via `make test`.

## Per-machine network config is installed by the initramfs, never by a unit

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

## The initramfs has no cut, basename, head, install, wc or date

Only what the `inst_multiple` lines in `90thinfloe/module-setup.sh` install,
plus dracut's base set, exists there. Shell builtins (`printf`, `test`,
`read`) are fine. A missing command fails silently at boot while every
offline test passes on the dev host - that is how the hostname shipped as
`thinfloe`, why netboot never recorded the boot MAC, and why every `sync` in
the `tl.new` upgrade rotation failed. `sync` and `rmdir` are now installed
explicitly for that rotation. `dhclient` is no longer installed: DHCP is
`systemd-networkd`'s job (see [Network Boot vs USB Boot](network-boot.md)).
`test/integration/test-initramfs-home.sh` runs
the helpers, `tf_finalize_upgrade` included, with PATH restricted to that
set. `test/integration/test-initramfs-commands.sh` checks every command the
90thinfloe hooks call against `lsinitramfs` of the built `initrd.img`, so a new
call to a missing tool fails `make test` even where no offline test runs it.
Its command extractor (`test/integration/initramfs-commands.py`) is a
heuristic: it skips comments, quoted text, `for` lists and `case` patterns,
and descends into `$( )`; a command it cannot see is not checked.

The initrd carries no WiFi drivers and no WiFi firmware (2026-10-24).
`instmods iwlwifi iwlmvm cfg80211` in `90thinfloe/module-setup.sh` brought
82 MB of Intel firmware that nothing in the initrd used: it has no
`wpa_supplicant`, and netboot runs over a cable. The initrd shrank from
179.4 MB to 93.9 MB. The running system loads WiFi drivers and firmware
from the squashfs after switch-root. `test-initramfs-commands.sh` fails
on `iwlwifi.ko` or `firmware/intel/iwlwifi/` in the initrd. **Not
verified on hardware** until a USB boot on an Intel WiFi machine (the
8265) associates from `~/wifi.yaml`.

## The ThinFloe pre-mount and mount hooks run after dracut-cmdline

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

## /etc/hostname is excluded from the squashfs and written by the initramfs

podman bind-mounts `/etc/hostname` in the build container, so `mksquashfs`
packed the container ID (`fa0ad0f31bfa`) and every machine had the same name.
`tf_write_hostname` writes `thinfloe-` plus the first six hex digits of the
persisted machine-id on USB, or the last six hex digits of the boot MAC on
netboot, where the machine-id is new every boot. It runs before switch-root,
so PID 1 reads the name and `%H` in `un-dockerize.service` is correct.
`test/integration/test-field-report.sh` guards the exclusion and the call.

## thinfloe-menu draws on the CPU

The crate has no `wgpu`, `glow` or `eframe`. The target images have no Vulkan
ICD (`/usr/share/vulkan/icd.d` does not exist), and GL would make the menu's
start depend on EGL. `test/integration/test-session-packages.sh` fails when
the binary links or names a GPU library.

## thinfloe-session never saves; thinfloe-menu saves before it exits 0, 10 or 11

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

## The first boot after an upgrade takes its kernel from tl.new

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
`ramroot.sqsh`) was 775 MB in 2026-10 (611 MB after the Part 3 slim-down,
2026-10-24), and the ESP was 2000 MB: with
`tl.old/` still present, staging `tl.new/` needed room for three and failed.
`thinfloe-upgrade` therefore removes `tl.old/` before it stages `tl.new/`;
while an upgrade is staged the fallback is `tl/`, the `current` entry. Before
it removes anything it checks that free space plus `tl.old/` and `tl.new/`
holds the release plus 10 %, and stops with "Nothing was changed" if not.
New images get a 3000 MB ESP (`EFI_PARTITION_SIZE`): two releases, two home
archives during a save, and room to grow. A fresh install therefore needs a
stick of 4 GB or more. The first boot grows the ESP to the end of the stick,
at most 6 GiB, older sticks' 2000 MB included; see
[The ESP grows once](#the-esp-grows-once-in-the-initramfs-and-keeps-the-hybrid-mbr).

`test/unit/test_upgrade_image.py` (via `make test-unit`) and
`test/integration/test-initramfs-home.sh` guard both halves, and `make
qemu-smoke-upgrade` boots a staged upgrade through OVMF and syslinux and
requires the rotation and `DEFAULT current` afterwards.

## The ESP is chosen by stick ID, never by the partlabel link

Every ThinFloe stick has a partition named `EFI-SYSTEM`, so with two sticks
attached `/dev/disk/by-partlabel/EFI-SYSTEM` is one udev link that names
either one, differently from boot to boot. The initramfs used to mount it:
the firmware loaded the kernel from one stick, while the squashfs, the home,
the add-ons and every save went to the other. An add-on installed before a
restart was gone after it. `make qemu-test` attaches a second copy of the
image (`vdb`), so two of three restarts there lost the add-on; in the field
it takes an old and a new stick plugged in together.

Each stick therefore carries an ID: a lowercase UUID in `/config/stick-id`
on its ESP, and `thinfloe.stick=<id>` at the end of every `APPEND` line of
both `syslinux.cfg` copies whose `ramroot=` is not a URL. `tf_find_esp`
(`thinfloe-lib.sh`) lists the `EFI-SYSTEM` partitions from sysfs, reads each
one's ID read-only, and `tf_choose_esp` picks: the one whose ID the command
line names (several: the first, logged); else, with one partition, that
one; else a partition without an ID (several: the first, logged), because
a stick with an ID always boots with it; else the first, logged. While the
wanted one is missing (or, with none wanted, several sticks and all with an
ID) it waits up to `TF_ESP_MATCH_WAIT` (10 s) after the
first stick appeared, for a second stick that enumerates late. The answer
is cached in `/run/thinfloe-esp-device`, so `finalize-upgrade.sh` and
`mount-squashfs.sh` mount the same partition. Do not go back to the link.
`tf_log` writes to stderr only: under systemd dracut's `info` writes to
stdout, and a log line inside `tf_find_esp`'s answer made
`finalize-upgrade.sh` fail to mount the ESP on every boot of the first
build of this change (caught by `make qemu-smoke-upgrade`).

The ID is written by `tf_stamp_stick` after the mount (the stored one,
else the command line's, only when this is the one `EFI-SYSTEM` partition
(with two, a guessed stick would take the booted stick's ID), else a new one;
`thinfloe: this stick is now <id>`), and by `thinfloe-upgrade` right after `dd` of a fresh install,
independent of the home-copy question (`stamp_new_stick`). Both make the
same edit, byte for byte (`stamp_syslinux`), and match `APPEND` in any
case. Each file is `sync`ed before its rename. A staged upgrade copies `LABEL
current` into `LABEL new`, so the ID rides along.

Limits: two attached sticks that both never booted and were not written by
`thinfloe-upgrade` (a `dd` of the download, `make image`) are still a guess,
the first by device name; `dd` copies of a used stick share one ID, one
machine-id and one host name; a 3.x fallback boot (`LABEL previous`) runs
the 3.x initramfs and still uses the link.

Guards: `test/integration/test-esp-choice.sh` (the rule, the wait, the
stamp, the shell/Python cross-check, with only the initramfs's tools),
`test/unit/test_upgrade_image.py` and `make qemu-smoke-two-sticks` (three
OVMF boots from a fresh stick with a stick that has an ID attached).

## The ESP grows once, in the initramfs, and keeps the hybrid MBR

`90thinfloe/grow-esp.sh` (pre-mount 20, before `finalize-upgrade.sh` at 25)
grows partition 1 of the boot stick to the end of the stick, at most
`TF_ESP_CAP_SECTORS` (6 GiB), and then the FAT32 inside it with
`fat32expander` (a pinned `.deb` in `containers/ubuntu/Dockerfile`). Code:
`tf_grow_esp` and its helpers in `thinfloe-lib.sh`. It runs on USB boots
only, before any hook mounts the ESP, and only for partition 1 with no
partition behind it on a disk of 512-byte sectors. The partition grows only
when it gains 256 MiB or more; the FAT32 is resized whenever it is 1 MiB or
more smaller than its partition (mkfs.fat leaves 51 sectors unused) or its
boot sector lacks `55 aa`. Neither failure stops the boot.

- **sgdisk, never sfdisk.** `make-bootable-image.sh` writes a hybrid MBR
  (`sgdisk --hybrid=1`). sfdisk keeps the GPT entry but writes a protective
  MBR. Every stick in the field has the hybrid entry (since 7e33c00, which
  gives no reason for it), so firmware that boots from it must keep doing so.
  `_tf_grow_partition` deletes and recreates entry 1 with the same start,
  type, unique GUID, attributes and name, moves the backup GPT to the end
  (`-e`), and passes `-h 1` again when the MBR had an entry other than
  `ee`.
- **No `exit` in any 90thinfloe hook.** dracut sources every hook of a
  stage into one shell (`source_hook`), so an `exit` ends the stage. The
  first build ended `grow-esp.sh` with `exit 0`: `finalize-upgrade.sh`
  (pre-mount 25) never ran and a staged upgrade never rotated (`make
  qemu-smoke-upgrade` caught it). `finalize-upgrade.sh` itself exited and
  skipped dracut's `99-mount-virtiofs.sh`; its body is now a function that
  returns. `test-esp-grow.sh` checks every hook `module-setup.sh` installs.
- **The cap.** The initramfs runs `fsck.fat` on the ESP at every boot,
  about 3 s for 15 GB; 6 GiB keeps that short. The stick has the rest of
  its space unused.
- **Cluster size.** `make-bootable-image.sh` formats the ESP with 4 KiB
  clusters (`-S 512 -s 8`). UEFI needs the 512-byte *sector* size to match
  the stick; it sets no cluster size, and 3000 MB at 4 KiB is about 768000
  clusters, far above the 65525 that FAT32 needs. Earlier images used
  `-s 1`: eight times the clusters, a FAT eight times larger, and a resize
  that moves 1.5 million clusters instead of 190000. Sticks made from them
  keep 512-byte clusters. **Not verified on hardware** yet with 4 KiB.
- **Progress.** `fat32expander` 0.1.2 prints none. `tf_resize_fat` takes the
  byte count from its dry run (`Planned data shift for N clusters (B
  bytes)`) and divides the sectors written to the disk since the start
  (`/proc/diskstats` field 10) by it, capped at 99: "Enlarging the boot
  stick, once: NN % - do not switch off" on the step line, every 10 % in
  the log. Its own report goes to the log only when it fails.
- **A power loss.** `fat32expander` clears the `55 aa` signature while it
  moves data, so a cut-off resize leaves a FAT32 nothing mounts, and the
  next boot runs it again. Whether it then finishes from its checkpoint
  has not been tested. A partition grown without its FAT32 (power lost
  between the two) is grown at the next boot.

Seen in QEMU (2026-10-28): from 3000 MB to 6144 MiB on an 8 GiB stick,
585 MiB moved in about 24 s; `fsck.fat -n` clean, every file intact, the
second boot does nothing. **Not verified on hardware**: the time on a real
stick, and that real firmware still boots it.

Limits: with two sticks, the stick id of one whose resize was cut off cannot
be read (its ESP does not mount), so the ESP choice may take the wrong
stick for that boot.

Guards: `test/integration/test-esp-grow.sh` (size rule, resize test,
progress, log; via `make test`), `test/integration/test-initramfs-commands.sh`
(`sgdisk`, `partx`, `fat32expander` in the initrd), `make qemu-smoke` (its
4G disk grows on every run) and `make qemu-smoke-grow` (two OVMF boots of an
8 GiB stick: 12582912 sectors at the old start, hybrid MBR kept, FAT32 the
size of its partition, `fsck.fat -n` clean, files unchanged, no resize at
the second boot).

## Add-on runtime libraries are listed by name in the Dockerfile

`containers/ubuntu/Dockerfile` installs the libraries ThinLinc and OxuLnk
need by name, in one `apt-get install` block next to the comment that says
so. An add-on `.deb` is installed with `dpkg -i`, which resolves nothing, so
whatever its `Depends:` would have pulled in must already be in the image.
ThinLinc's `.deb` has no `Depends:` at all, and its `vncviewer` dlopens
`libXrandr.so.2`, so `libxrandr2` is installed by name for it; nothing but
this list keeps it there. OxuLnk's `Depends:` are `libasound2t64 libc6
libstdc++6 libva-drm2 libva2 libx11-6 libx11-xcb1 libxcb1 libxcursor1
libxi6 libxkbcommon-x11-0 libxkbcommon0` (`libopus0` left the list with the
build of 2026-10-26; PipeWire still pulls it into the image). Dropping a line
leaves the add-on installed and its tile drawn, and the client fails when it
starts. A new add-on that needs another library needs a line here.
`test/integration/test-addon-deps.sh` installs each client the way
`thinfloe-addon` does into a throw-away container of the built image, checks
that every ELF file resolves its libraries and that the client starts; run
via `make test` with `THINLINC_DEB` (else the page and pattern of the shipped
tile are used) and `OXULNK_DEB` (else the shipped tile's signed APT source,
fetched with `thinfloe-addon`'s own code), downloaded into a directory of
the run unless `TF_ADDON_CACHE` names one to keep. A client it cannot get
fails the test.

## A 3.x home is migrated once

A 3.x home archive holds `~/.dbrrg-environment`, `~/.dbrrg-sessionrc`,
`~/.dbrrg-password`, `~/.dbrrg-ssh-host-keys` and `~/.config/dbrrg`. The
first ThinFloe boot that restores such an archive gives them the 4.0 names
`.thinfloe-environment`, `.thinfloe-sessionrc`, `.thinfloe-password`,
`.thinfloe-ssh-host-keys` and `.config/thinfloe` (`tf_migrate_home_names` in
`thinfloe-lib.sh`, list in `TF_HOME_RENAMES`), logs `thinfloe: renamed
~/<old> to ~/<new>` and writes `~/.thinfloe-home-version` (`1`). After that
the home is native and nothing reads or keeps the old names (owner,
2026-10-23).

- **The archive decides, not the disk.** The default home ships
  `.thinfloe-home-version`, `.thinfloe-environment` and `.thinfloe-sessionrc`
  (`overlay/home/tfuser/`), so all three are on disk after any extraction. An
  archive that lists the marker is native and is not touched; a `.dbrrg-*`
  entry in it came from a save on a 3.x fallback boot and is ignored. An
  archive without the marker is migrated: per name, the new name stays when
  the archive lists it, otherwise a real (not symlink) old entry replaces
  the image default whole (`_tf_move_over`, which moves the default aside
  and removes it only after the move worked).
- **Idempotent.** After the renames, old names that are symlinks are removed
  (development builds of 4.0 left them, without a marker), a real old entry
  beside a listed new name is left alone, and the marker is written. A
  second run changes nothing, so a marker that cannot be written is only a
  warning: the next boot migrates again. The same holds for a
  `~/.save-home-exclude` pattern that leaves the marker out of the save:
  every boot then migrates again, harmlessly.
- **`sudo thinfloe-password --clear` also removes `~/.dbrrg-password`.** A
  save on a 3.x fallback boot can leave one in the home. 4.0 never reads it,
  but the next 3.x boot would install it, and so would a migration on a
  boot whose archive lost the marker.
- **A failed listing fails the restore.** The restore lists the archive
  once, before extracting it, and the migration decides from that listing,
  so a boot decompresses the archive twice. When the listing is empty the
  archive is not extracted; when tar lists members but exits non-zero, or
  the extraction fails, nothing is migrated and no marker is written. All
  three leave `home-restore` at `failed`, so no save overwrites the stored
  archive.
- **A symlinked `~/.config` is not followed.** Renaming below it would
  change the directory it points to, so `.config/dbrrg` is skipped with the
  warning `thinfloe: ~/.config is a symlink, leaving ~/.config/dbrrg alone`,
  once per restore.
- **`/run/thinfloe/state/home-renamed`** lists one old name per line, and
  only when a real 3.x file was renamed. Nothing reads it yet; it is meant
  for the add-on auto-tick ("this home came from 3.x").
- **A 3.x fallback boot** (`LABEL previous`, `tl.old/`) finds none of its
  names and starts with a default keyboard, display layout and tiles, no
  remote-access password and new SSH host keys. `~/wifi.yaml`, `~/wg0.conf`
  and `~/.thinlinc/` never changed names and still work. What changes there
  is not carried back to 4.0. 3.x's fresh install, run from such a boot,
  excludes only `./.dbrrg-ssh-host-keys` and so copies
  `.thinfloe-ssh-host-keys` to the new stick; 3.x cannot be changed.
- A stick whose image predates 47c20bb (2026-10-07) upgrades with its old
  `upgrade-image`, which does not boot `LABEL new`, so its first ThinFloe
  boot runs the 3.x initramfs on the 4.0 squashfs: no rename, a default
  keyboard, no SSH password, fresh host keys, the host name `dbrrg-…`, and
  every save refused with exit 4 (the archive on the ESP stays intact). The
  next boot runs the 4.0 initramfs and migrates the home. The owner
  confirmed on 2026-10-23 that no stick in the field runs an image older
  than 2026-10-07.
- Still read, next to the new names, because migrated user files carry
  them: `rd.dbrrg.debug` (`rd.thinfloe.debug`), `X-DBRRG-Action`,
  `X-DBRRG-Save-On-Exit` and `X-DBRRG-Multiple` (the `X-TF-` key wins) and
  `DBRRG_IDLE_TIMEOUT` (`TF_IDLE_TIMEOUT` wins).
- Shipped tile files keep their file names (`50-upgrade-image.desktop`),
  because a user file rewording a shipped tile matches by file name.

Guards: `test/integration/test-initramfs-home.sh` (the migration, also with
only the initramfs's tools), `test/integration/test-session-packages.sh`
(the marker in the image), `test/integration/test-cmdline.sh`, the tile
tests in `src/thinfloe-menu/src/tiles.rs`, `test/unit/test_upgrade_image.py`
(`make test-unit`) and `make qemu-smoke-upgrade`.

## tty1 does not wait for Plymouth; the session ends it

The boot picture stays on screen until the menu draws, with no text console
in between (2026-10-27). Four pieces hold this up:

- `overlay/etc/systemd/system/getty@tty1.service` is a whole unit, a copy
  of systemd 259.9's `getty@.service` without
  `After=plymouth-quit-wait.service`. A drop-in cannot remove that line.
- It runs `login -f tfuser`, never `agetty`. agetty (util-linux 2.41,
  `termio_init`) quits Plymouth when `plymouth --ping` answers and then
  waits up to 30 s for the terminal. At 4K in QEMU this crashed plymouthd
  (SEGV, 3 of 3 boots) and delayed the login to 61 s. `TTYReset=no`,
  `TTYVHangup=no` and `TTYVTDisallocate=no` keep systemd from clearing the
  VT under the picture.
- The session hands the display over: `10-thinfloe-session.sh` sends
  "Starting the session" and `deactivate` before labwc, `reactivate` and
  "Waiting for the graphics driver" around the retry, and `quit` once labwc
  returns, before the failure screen. `thinfloe-session` runs `quit-retain`
  before the menu. All through `sudo -n plymouth-step`, which does nothing
  when Plymouth is gone, so later logins are unaffected.
- `plymouth-quit.service` is masked, and `thinfloe-plymouth-fallback.timer`
  ends Plymouth 300 s after boot when no session did. While Plymouth runs,
  `multi-user.target` waits for `plymouth-quit-wait.service`, so the boot
  counts as finished when the session ends Plymouth.

labwc does not keep the picture: it shows its own black screen until the
menu maps, about 0.5 s at 1280x800 and 1 to 2 s at 4K in QEMU. Accepted
(owner, 2026-10-28).

Seen in QEMU (2026-10-28) at 1280x800 and 4K: picture, black, menu, with
no text console after the first picture. The failure path (labwc does not
start, the picture comes back with "Waiting for the graphics driver", then
the failure text) cannot be produced there: with `bochs` blocked, labwc
runs on `simpledrm`, which is built into the kernel, and without it there
is no framebuffer at all. Its order is proven by the lifecycle test only;
the failure path is **not verified on hardware.**

**Confirmed on hardware (2026-10-28, Intel Alder Lake-P/Raptor Lake-U):**
no text console. The picture ends earlier there, though: Plymouth draws on
`simpledrm`, and when `i915` binds after switch-root (8.7 s after the
kernel started) Plymouth does not move to the new device, so the screen
stays dark until the menu draws, about 10 s with the two monitor resyncs
(i915 at 8.7 s, labwc at 10.2 s). On a Broadwell Brix (i3-5010U, 2014
firmware) the picture never showed: Plymouth ran on `simpledrm` for 15 s
and no monitor displayed the firmware framebuffer. So `i915` and the
firmware its modinfo names are in the initrd since 2026-10-28
(`installkernel()` in `90thinfloe/module-setup.sh`, not in
`--omit-drivers`; the initrd grew from 92.3 MB to 98.2 MB), to bind
before the squashfs loads. `test-initramfs-commands.sh` fails without
the module or its firmware. **Not verified on hardware**: that Plymouth
then draws on i915 on both machines. `plymouth.use-simpledrm` stays on
the command line, because amdgpu, nouveau and radeon are still left out
and their machines would otherwise show no picture.

Guards: `test/integration/test-session-packages.sh` (the unit, the mask, the
timer), `test/integration/test-session-lifecycle.sh` (the order of the
calls, also when labwc fails and when Plymouth is gone),
`test/integration/test-plymouth-step.sh` (the helper) and
`test/unit/test_addon.py` (the add-on steps).
