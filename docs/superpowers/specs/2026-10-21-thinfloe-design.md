# ThinFloe: rename, slimmer image, new look, add-ons, GitHub releases

Date: 2026-10-21. Status: draft for review.

## Intent

dbrrg (Docker-Based RamRoot Generator) gets a name people can say and a look
that goes with it: **ThinFloe**. An ice floe floats, carries you for a while,
and melts; the system floats in RAM and is gone at power-off, except for the
saved home.

At the same time the image becomes a product other people can use:

- Images are released on GitHub with proper version numbers, and a machine
  on a USB stick sees and installs a new release by itself.
- The two site-specific clients, ThinLinc and OxuLnk, leave the image and
  become add-ons the person at the machine chooses. Image releases and
  client releases are decoupled, the public image redistributes neither
  client, and anyone can add their own package the same way.
- The image gets smaller, mainly so that downloads and upgrades are faster.
  RAM use is a secondary goal; the squashfs already keeps it low.

## Order of work

The five parts are separate plans, done in this order:

1. **Rename** (with migration of config files in the field).
2. **Add-ons** (ThinLinc and OxuLnk out of the image, Settings tile).
3. **Slim down** (survey, then cuts).
4. **New look** (boot screen, desktop, Settings screen).
5. **GitHub releases** (repo-infra, version numbers, Upgrade tile checks).

The rename goes first so no new work is written under the old name. Add-ons
come before slimming because they remove the two largest site-specific
packages anyway, and the survey should measure what is left. The look
comes after the Settings screen exists in function. Releases come last so
the first public release already carries the new name, the smaller image
and the new look, and contains no ThinLinc client.

*Builder's call:* parts 2 and 4 may overlap where the Settings screen is
built (function first, then styling, or both at once).

## Goals

- One consistent name, ThinFloe, everywhere a user, admin or developer
  reads it.
- A machine in the field upgrades to ThinFloe and keeps everything it had:
  keyboard layout, display setup, password, SSH host keys, WireGuard and
  WiFi config, user tiles.
- ThinLinc and OxuLnk installed on demand, updated automatically, available
  offline after the first install.
- A release is built on GitHub Actions from a tag, through repo-infra, and
  carries a semantic version that the image itself knows.
- A USB machine tells its user when a new image release exists.
- A noticeably smaller download.

## Non-goals

- Netboot machines getting images or add-ons from GitHub. The boot server
  is a later round. Netboot machines keep updating as today; their Upgrade
  tile is hidden. (Add-ons on netboot: see "Open for later".)
- Changing the save contract, the dialogs, the log, or the tile rules
  beyond what this document adds. The menu does change: Settings screen,
  tiles that appear with their add-on, update markers, a header with a
  clock, wide bands instead of squares, and an eighth tile (Settings). The
  square-tile sizing of `1edb4f0` and its test are redone; 8 tiles must fit
  above the log at 1280x720.
- Keybindings. The zero-keybindings constraint stays.

## Anti-goals

- A field machine that loses its settings, password or SSH host keys on the
  first ThinFloe boot.
- A machine that cannot start a ThinLinc session because the network or a
  vendor web page is down, when it had ThinLinc installed before. The copy
  on the EFI partition must make an installed add-on independent of the
  network at boot.
- A public release that contains the ThinLinc client.
- Any of the Standing Constraints in `CLAUDE.md` broken along the way (no
  shipped password, `--no-hostonly`, zero keybindings, masked
  `ssh.socket`, network config installed by the initramfs, and the rest).

---

## Part 1: Rename

### Naming rule (decided)

- `thinfloe` in full wherever a name stands alone: programs, services,
  directories, the crate, the host name prefix, dotfiles.
- `tf_` / `TF_` where a short namespace prefix is needed: shell functions,
  environment variables. Desktop-file keys use `X-TF-`.

### New names

| Old | New |
|---|---|
| `dbrrg-menu`, `src/dbrrg-menu/` | `thinfloe-menu`, `src/thinfloe-menu/` |
| `dbrrg-session`, `dbrrg-save-home`, `dbrrg-password`, `dbrrg-ssh-hostkeys`, `dbrrg-compose-labwc-config` | `thinfloe-session`, `thinfloe-save-home`, `thinfloe-password`, `thinfloe-ssh-hostkeys`, `thinfloe-compose-labwc-config` |
| `upgrade-image` | `thinfloe-upgrade` (decided) |
| `dbrrg-*.service` | `thinfloe-*.service` |
| `/etc/dbrrg`, `/run/dbrrg`, `/usr/libexec/dbrrg`, `/usr/share/dbrrg` | `/etc/thinfloe`, `/run/thinfloe`, `/usr/libexec/thinfloe`, `/usr/share/thinfloe` |
| dracut module `90dbrrg`, `parse-dbrrg.sh`, `dbrrg-lib.sh` … | `90thinfloe`, `parse-thinfloe.sh`, `thinfloe-lib.sh` … |
| `dbrrg_*` shell functions, `DBRRG_*` variables | `tf_*`, `TF_*` |
| `X-DBRRG-Action`, `X-DBRRG-Save-On-Exit`, `X-DBRRG-Multiple` | `X-TF-Action`, `X-TF-Save-On-Exit`, `X-TF-Multiple` |
| `~/.dbrrg-environment`, `~/.dbrrg-sessionrc`, `~/.dbrrg-password`, `~/.dbrrg-ssh-host-keys`, `~/.config/dbrrg/` | `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys`, `~/.config/thinfloe/` |
| host name `dbrrg-3f9a2c` | `thinfloe-3f9a2c` (same six hex digits) |
| `rd.dbrrg.debug`, `root=dbrrg` | `rd.thinfloe.debug`, `root=thinfloe` |
| `dbrrg-usb.img.zst` | `thinfloe-usb.img.zst` |
| boot menu title "dbrrg Boot Menu", `MENU LABEL dbrrg - …` | "ThinFloe" |
| `PROJECT_NAME` in the `Makefile`, container tags `dbrrg-ubuntu:…` | `thinfloe` |
| GitHub `oposs/dbrrg` | `oposs/thinfloe` (GitHub redirects the old address) |
| labwc package version suffix `+dbrrg1` | `+thinfloe1` |

`CLAUDE.md`, tests, scripts and the live documentation follow. Plans and
reports already in `docs/superpowers/` and `docs/reports/` stay as they are:
they are history and refer to the names of their time.

### Migration of data in the field (decided: there is one)

What exists on sticks today and must keep working after the upgrade:

1. **Home archive** (`home.tar.gz` on the ESP). On restore, the initramfs
   renames each `~/.dbrrg-*` file listed above to its `~/.thinfloe-*`
   name, and `~/.config/dbrrg/` to `~/.config/thinfloe/`. This must happen
   in the initramfs, inside the restore, because `.thinfloe-environment`
   must be in place before the compositor starts and
   `.thinfloe-ssh-host-keys` before sshd (see `CLAUDE.md`, "Persistent
   Home Directory").

   **Precedence is decided by what is in the archive, not by what is on
   disk.** The restore unpacks the archive over the image's default home
   and deletes nothing first (`restore_home` in the dracut lib), and the
   default home ships `.thinfloe-environment` and `.thinfloe-sessionrc`. So
   "the new name exists" is always true and says nothing. The rule:
   - any file from the archive beats an image default;
   - a new name in the archive beats an old name in the archive;
   - after the rename, the old name is replaced by a **symlink** to the
     new file (decided, see 1a).

   A test covers the case "3.x archive with a user XKB layout restored over
   the 4.0 default home".

1a. **Going back to 3.x keeps working (decided).** After the first 4.0
   boot, 3.x is in `tl.old/` and `LABEL previous` boots it. While the
   migration code exists, the home keeps `~/.dbrrg-*` (and
   `~/.config/dbrrg`) as symlinks to the new names, so they travel in every
   save and a 3.x boot finds its keyboard settings, password and SSH host
   keys. *Builder:* check that 3.x's restore, `dbrrg-ssh-hostkeys` and
   `dbrrg-password.service` follow a symlink where they read these files,
   and that 4.0's tar keeps the links as links.

1b. **Identity excludes keep the old names.** `IDENTITY_EXCLUDES` in
   `thinfloe-upgrade` (today `./.dbrrg-ssh-host-keys`, `./wg0.conf`) lists
   both the new and the old host-key name, so a fresh install never copies
   this machine's SSH host keys to another stick under either name.
2. **User tiles** in `~/.config/thinfloe/menu/` may still carry
   `X-DBRRG-*` keys (from old notes or copied files). The menu reads them as
   the `X-TF-*` keys; an `X-TF-*` key wins when both are present.
3. **`syslinux.cfg` on old sticks** is never rewritten by an A/B upgrade, so
   its `APPEND` lines keep `rd.dbrrg.debug` and whatever else they carry
   today. The initramfs accepts the old parameter names next to the new
   ones. *Builder:* check every name the old `syslinux.cfg` passes
   (`configs/syslinux.cfg` history) and every value the initramfs writes
   or reads on the ESP (`/config/machine-id`, the `tl*/` directories,
   `home.tar.gz`), and keep the ones the stick carries unchanged.
4. **The first ThinFloe boot after an upgrade** runs the *new* initramfs
   from `tl.new/` (Standing Constraint "The first boot after an upgrade
   takes its kernel from tl.new"), so the migration code is present on that
   boot. The rotation `tl.new` → `tl` must keep working with the renamed
   functions.
5. **Netboot:** the boot server stores `home.pkg` by MAC. Its contents get
   the same rename on restore. The server-side paths and URLs do not change
   in this round.

The migration code stays until it is removed on purpose (my call), not for
a fixed number of releases.

### Visible effects to tell admins (CHANGES.md)

- The host name prefix changes from `dbrrg-` to `thinfloe-`. DNS entries,
  monitoring, or ThinLinc server settings that know a machine by name need
  updating.
- The user files in the home directory have new names; old ones are
  renamed on the first boot.

---

## Part 2: Add-ons

### What the user sees (decided)

- A new **Settings** tile in the menu opens an **Add-ons** screen (mockup:
  `2026-10-21-thinfloe-mockups/settings.html`).
- One row per add-on: tick box, icon, name, source, state, and an action
  button.
  - Ticking installs at once. Unticking removes the tile and the copy on the
    ESP.
  - State is one of: not installed; installed and up to date (version shown
    when known); update ready for next boot; check failed (with the reason
    in the log).
  - A row with an update waiting has the ice-blue left edge and an
    **Update now** button. A row not installed has **Install now**.
  - **Update now** installs immediately, even while the program runs (decided:
    the user's choice; running processes keep their open files).
- **Check now** at the top checks every add-on at once and shows when the
  last check ran.
- In the main menu, the tile of an add-on with an update waiting shows a
  small "update" marker.
- A tile that belongs to an add-on is shown in the main menu only while the
  add-on is installed.
- Every check, download and install writes to the menu log with source
  `add-ons`.

### What an add-on is (decided)

An add-on is a tile file (`.desktop`) with extra keys that name a source.
The shipped ones are in `/etc/thinfloe/menu/` (ThinLinc, OxuLnk). Users add
their own in `~/.config/thinfloe/menu/`, which travels with the home archive.
The user-tile rules of today stay where they do not conflict: at most 32
user files; a user file with the name of a shipped one may only reword
`Name`, `Comment` and `Icon`. **A user file may now also carry the source
keys** of a new add-on (today a user tile may only `run` a program); it
still cannot set `X-TF-Action` other than run.

### Source types (decided: all three; the key names are the builder's call)

Each source type answers the same two questions: *where is the file*, and
*what identifies this version* (a version string, or a fingerprint).

1. **APT repository.** Repository URL, suite/component, package name,
   optional signing key. The check reads the repository index and compares
   the package version with the installed one.
2. **Fixed web address.** A URL to a `.deb` or a `.tar` overlay. The check
   follows redirects and requests headers only; the fingerprint is ETag,
   else Last-Modified, else Content-Length plus the final URL. No download
   to detect a change.
3. **Web page + pattern.** A page URL and a regular expression. The check
   fetches the page and matches every link. A named group `version`, when
   present, gives the version: it is shown in Settings, and when several
   links match, the highest version wins. Without a `version` group the
   first match wins and the fingerprint is the matched URL. The matched URL
   is then downloaded like a fixed address.

Example for ThinLinc (verified 2026-10-21: the page carries exactly one
amd64 client link; Cendio's "latest" redirect leads to a 404):

```
X-TF-Source-Page=https://www.cendio.com/thinlinc/download/
X-TF-Source-Pattern=https://[^"]*/thinlinc-client_(?P<version>[0-9.]+-[0-9]+)_amd64\.deb
```

OxuLnk comes from the OEP Debian repository (type 1); the owner uploads
`oxulnk-desktop` there. URL and key are supplied before the build.

Payload formats: `.deb` (all three types) and `.tar` overlay (types 2 and
3). A `.tar` overlay is unpacked over `/`.

**Security (decided):** HTTPS and repository signatures are optional; the
person who writes the tile file decides. But a source that is neither
HTTPS nor signed is **never checked or fetched automatically** (decided
2026-10-22): it installs and updates only when someone presses Install now
or Update now. Otherwise anyone on the network path could place a package
that the machine installs as root at the next boot with nobody pressing
anything. The two shipped add-ons use HTTPS, and OxuLnk the repository
signature if the server provides one (my call). A home archive can bring
software onto a machine; state that in the documentation.

### Where the tick lives (my call)

The tick state of each add-on is kept **in the home directory** (a file
under `~/.config/thinfloe/`), so it travels with the home archive: a home
copied to a new stick by `thinfloe-upgrade` installs the same add-ons there
on its first boot, and netboot can use the same state later. The stored
packages themselves stay on the ESP; they are a cache, re-downloaded when
missing.

### First boot after the upgrade from 3.x (decided)

The 3.x image had ThinLinc built in; 4.0.0 does not. The owner's rule:
**tick ThinLinc if there is a ThinLinc configuration in the home.**

Every 3.x default home ships `~/.thinlinc/tlclient.conf`, so "the home has
`~/.thinlinc/`" alone is always true for a 3.x home and also false for a
stick whose restore failed. The trigger is therefore **"this home came from
3.x"**: the restored archive held `.dbrrg-*` names, or the stick boots from
an upgrade out of 3.x (`tl.old/` holds a 3.x release) and no archive was
restored. It fires once, writes the tick file, and never again. With the
trigger true, ThinLinc is ticked if `~/.thinlinc/` exists or no archive
was restored. *My call:* OxuLnk follows the same rule with its own config
path (the builder names it). A fresh install of the public image starts
with nothing ticked.

That boot needs the network and cendio.com. If the download fails (no
network yet, Cendio down, page changed), **the menu shows an error with a
Retry button** (decided), and the Settings row shows the same state. The
regular checks keep retrying too. No owner-hosted fallback copy.

### Lifecycle (decided)

- **Install / Update now:** download, store on the ESP, install into the
  running system at once.
- **Daily check and check at boot:** a newer version is downloaded to the
  ESP immediately and becomes active **at the next boot**, so a running
  session is never changed without the user pressing a button.
- **Boot:** every ticked add-on is installed from its copy on the ESP,
  before the session starts, with no network needed. The check at boot runs
  afterwards, in the background.
- **Failure:** a failed check or download leaves the installed copy in
  place and logs why. A copy that fails to install at boot is logged, and
  its tile is not shown.

### Engine (from review; builder may refine)

- One root program, `thinfloe-addon` (`check`, `fetch`, `install`,
  `remove`, `status`), does all the work. A boot unit ordered before the
  session runs the install from the ESP copies, a timer runs the daily
  check (with `RandomizedDelaySec`, so a site does not check all at once),
  and the menu calls it through `sudo`. The menu itself only reads a status
  file.
- It shares a lock with `thinfloe-save-home`, because both write to the
  ESP, and writes `.new` then renames, like the home archive.
- APT sources: `apt-get download` with a generated sources entry; versions
  compared the way dpkg compares them (`dpkg --compare-versions`). Page
  sources with a `version` group use the same ordering.
- Unticking removes the program from the running system at once.
- The fingerprint fallback (Content-Length plus URL) can miss an update
  that keeps both the same; that is accepted.

### Left to the builder

- How an add-on is put back on each boot. The root is squashfs + a zram
  overlay, so anything installed is lost at power-off. Candidates: `dpkg -i`
  of the stored `.deb` into the overlay early in boot, unpacking a prepared
  tree, or building a `systemd-sysext` image per add-on on the ESP (ThinLinc
  installs under `/opt`; sysext covers `/usr` and `/opt`). Measure boot-time
  cost; the session must not start before the add-on is in place.
- Dependencies. An add-on brings its own files only. Libraries ThinLinc and
  OxuLnk need from Ubuntu stay in the base image; list them explicitly in
  the Dockerfile. `Depends:` is not enough: `vncviewer` loads
  `libXrandr.so.2` at runtime and no `Depends:` names it. The test installs
  the current client into the built rootfs, runs `ldd` over its binaries
  plus an explicit list of runtime-loaded libraries, and starts it
  headless. Part 3 must keep these packages, although nothing else in the
  image uses them.
- Where the tick-box state and the stored copies live on the ESP (`/config/`
  already holds `machine-id`), and the ESP space budget: the 2000 MB ESP of
  old sticks must still hold two releases, two home archives during a save
  and the add-on copies.
- ESP space on old 2000 MB sticks at the first 4.0 boot: the 4.0 release,
  the 3.x release in `tl.old/`, the home archive, a second archive during a
  save, and the ThinLinc download. The space check in `thinfloe-upgrade`
  counts the add-on cache too.
- Icons: the ThinLinc icon lives in `/opt/thinlinc`, so before install the
  Settings row has no icon of its own; Cendio's logo is not shipped in the
  public image.

### Open for later

- Add-ons on netboot machines (no ESP; the copy would live on the boot
  server). *My call for this round:* netboot machines install their ticked
  add-ons from the network on every boot, from the tick state in the
  restored home, so a netboot machine with ThinLinc keeps ThinLinc. A
  netboot boot without network access has no ThinLinc; that already holds,
  since it has no image either.

---

## Part 3: Slim down

- Goal: a smaller download (`thinfloe-usb.img.zst`, and `vmlinuz` +
  `initrd.img` + `ramroot.sqsh` for an upgrade). Today: 771.8 MB compressed
  USB image (2026-10-08 build), initrd 179.3 MB.
- Secondary: RAM use.
- Method: first a survey with no changes. Largest packages in
  `ramroot.sqsh` (`dpkg-query` sizes), largest directories, largest parts of
  the initrd (`lsinitramfs`), and for each whether ThinFloe needs it. Bring
  the list to the owner before cutting.
- Cuts must respect the Standing Constraints: firmware is selected by
  package, never by a cleanup sweep; no `find`/`rm` over `/usr/lib/firmware`;
  `--no-hostonly` stays; the WiFi core stack comes from the kernel. Every
  cut is a package-list change or an explicit dracut option, with the size
  delta measured and recorded in CHANGES.md and the commit.
- Part 2 removes ThinLinc and OxuLnk from the image; measure after it.

---

## Part 4: New look

Mockups in `docs/superpowers/specs/2026-10-21-thinfloe-mockups/` (open the
HTML files in a browser; they load Geo from Google Fonts).

### Brand (decided)

- **Logo:** the TF monogram (`logo-original.png`), redrawn as a vector and
  slanted **9°** to match the font. Each letter is one closed path, so no
  seam shows between bar and stem. In unslanted coordinates (baseline
  y = 632):
  - T: `M278 388H625V447H433V632H360V447H278Z`
  - F: `M498 502H624V562H570V632H498Z`
  - slant: `translate(0 632) skewX(-9) translate(0 -632)`
  Pure white on near-black.
- **Font:** Geo, oblique style (`Geo-Oblique.ttf` from google/fonts,
  `ofl/geo/`; OFL). Its header claims an italic angle of 11°, but the
  outlines slant 9.0° (`l`: 106/676, `T` stem: 79/500). Geo is for the
  name, titles and tile names. Small text uses a plain sans; the log stays
  monospace.
- **Accent:** ice blue `#7cc4ff`. Backgrounds near-black (`#05070a`, tiles
  `#0d1218`, active `#101b26`).

### Boot screen: option B (decided)

Plymouth theme replacing `bgrt`: logo and "ThinFloe" wordmark in the centre,
a thin slanted (9°) ice-blue progress line below, black background with a
faint ice-blue radial glow from the bottom. At the foot, small, in Geo: the
host name and the image version (`thinfloe-3f9a2c · 3.1.0`).
*Builder:* the host name is written by the initramfs (`tf_write_hostname`)
before switch-root; check what Plymouth can show at which point of the boot
and fall back to the version alone where the name is not yet known.

### Desktop: option C (decided)

- **Header:** top left, logo and "ThinFloe" at 60 % opacity, logo about
  52 px tall at 1080p; top right, host name, version and clock in Geo.
- **Tiles:** flat, wide bands, 4 columns, the group centred and about 58 %
  of the screen width (not edge to edge; must not overwhelm a 43" display).
  Icon left (unchanged, upright), name in Geo Italic and comment right.
  3 px gaps.
- **Running:** a vertical ice-blue edge on the left of a tile with at least
  one copy running; with two or more, a count (`×2`) in the top right
  corner. No slanted shapes on tiles.
- **Log:** bottom 30 % as now, same colour roles (time grey, source ice
  blue, warnings amber, success green).
- **Taskbar (waybar):** same near-black; the active window has an ice-blue
  underline.
- **labwc title bars and the menu dialogs:** same palette, Geo for titles
  (no separate mockup; *builder's call* within that palette).
- The menu still draws on the CPU (Standing Constraint). Geo is embedded in
  the menu binary next to the fonts it carries today.

### Settings screen

As in `settings.html`, in the desktop C style.

---

## Part 5: GitHub releases

### Versions (decided)

- Semantic versions. `CHANGES.md` is the version source (repo-infra D5).
  `CHANGES.md` was created empty on 2026-10-22 (`### New` under
  `[Unreleased]`); Parts 1 to 4 record their changes in it.
- `VERSION` in the `Makefile` today is the container tag and keeps
  checkouts apart (Standing Constraint on container stamps). It is split:
  an image tag for that job, and the release version from `CHANGES.md`.
  A local build writes a version that sorts below the release
  (`4.0.0-dev+<sha>`), so a dev stick always sees the release as newer.
  *My call:* the first ThinFloe release is **4.0.0**, because the rename
  changes user-visible names; it continues from `VERSION := 3.0.0`.
- The build writes the version into the image (`/etc/thinfloe/version`).
  Boot screen, menu header and `thinfloe-upgrade` show it.
- Tag, release title and asset file names carry the same number.

### Build (decided)

- Built and tested on GitHub Actions through repo-infra (`oposs/repo-infra`
  pieces at `@v2`; release pull request, build and test before merge).
  Onboarding follows the repo-infra skill.
- ThinLinc is not in the image (Part 2). OxuLnk is not in the image either.
  The `vendor/` directory goes away.
- *Builder:* the USB image is built with `mtools`, no loop devices. Check
  disk space on a GitHub-hosted runner (raw image about 3.1 GB, plus the
  labwc, iPXE and Rust builds) and `/dev/kvm` for the QEMU smoke tests, or
  say what it needs.

### Release assets

`vmlinuz`, `initrd.img`, `ramroot.sqsh` (A/B upgrade),
`thinfloe-usb.img.zst` (fresh install), a checksum file over all of
them, and the version file the Upgrade tile reads. Each is below GitHub's 2 GiB per-asset limit.

### Upgrade tile (decided)

- USB machines only; hidden on netboot.
- Once a day (randomised within the day) the machine reads a small
  version file from `https://github.com/oposs/thinfloe/releases/latest/download/`.
  That address skips drafts and pre-releases and is not subject to the API
  limit of 60 requests per hour per IP, which a site with many machines
  behind one address would exceed. If its version is
  higher than `/etc/thinfloe/version`, the tile shows "Upgrade to X.Y.Z"
  with the ice-blue edge.
- The tile can always be pressed; pressing it checks again first.
- `thinfloe-upgrade` downloads from the release instead of
  `https://tobi.oetiker.ch/dbrrg`, and refuses files whose checksum does not
  match the release's checksum file.
- *Builder's call:* whether the custom-URL option in `thinfloe-upgrade`
  stays (useful for testing).

### Bridge from 3.x (decided)

Sticks in the field run 3.x, whose `upgrade-image` only knows
`https://tobi.oetiker.ch/dbrrg` and the old file names (`dbrrg-usb.img.zst`,
`vmlinuz`, `initrd.img`, `ramroot.sqsh`). Only the finished 4.0.0 goes to
the field: the owner puts it there once, under those names. Every later
upgrade comes from GitHub. Intermediate builds of Parts 1 to 4 are not
deployed, so the end-to-end test is one step, 3.0.0 to 4.0.0, through that
bridge (`make qemu-smoke-upgrade` is the model). No 3.x maintenance branch
(decided: no more 3.x fixes).

Old sticks keep their `syslinux.cfg` text ("SAY dbrrg Boot Menu"), because
an upgrade only changes `DEFAULT` and `LABEL new`. Accepted.

### No site-specific content in the public image (decided)

- The default home loses everything specific to the owner's site:
  `overlay/home/tluser/.thinlinc/tlclient.conf` with
  `SERVER_NAME=thinlinc-aarweg-01.oetiker.ch` and its `HOST_ALIASES`
  entry, and anything else of that kind (the builder searches for host
  names, addresses and domains of the site). Defaults become neutral.
- The owner's machines keep their settings, because the home archive
  carries `~/.thinlinc/` through the upgrade.
- *My call:* the OxuLnk add-on is shipped only if the OEP package server is
  publicly reachable; otherwise its tile file becomes an example the owner
  installs into the home of their machines.

### Before the first public release (owner)

- Done by Part 2: the image no longer contains the ThinLinc client, so
  Cendio's redistribution terms do not apply to the image.
- Make sure every other package and asset in the image may be redistributed
  publicly.

---

## Decisions log

Decided by the owner in conversation: the name and naming rule; the logo
and font; 9° slant; boot screen B; desktop C with the changes above
(centred tiles, vertical running edge, count for multiple copies, bigger
and translucent header logo); migration for field data;
`thinfloe-upgrade`; releases built on GitHub with repo-infra; proper
version numbers; daily check plus check on button press; hidden on netboot;
boot server later; add-ons with Settings tile and tick boxes; three source
types including a page pattern with a version capture; extensible by user
tile files; HTTPS and signatures optional; daily check and check at boot,
update active at next boot; update indicator, Update now, Install now and
Check now; Update now while running is allowed; slimming mainly for
download size; only 4.0.0 goes to the field, via tobi.oetiker.ch under the
old names; ThinLinc is ticked on the first boot when the home holds a
ThinLinc configuration; site-specific defaults leave the image. From the
pushback review (2026-10-22): a failed first ThinLinc download shows an
error with Retry, no owner-hosted fallback; `~/.dbrrg-*` symlinks keep a
3.x fallback boot working; sources that are neither HTTPS nor signed are
never updated automatically; no 3.x maintenance branch.

My calls (check while reading): `X-TF-` key prefix; old docs unchanged;
migration code kept until removed on purpose; ice blue `#7cc4ff`; first
release 4.0.0; drafts and pre-releases ignored; shipped add-ons on HTTPS;
the `/etc/thinfloe/version` path; tick state in the home; OxuLnk auto-ticked
like ThinLinc; OxuLnk shipped only if its server is public; netboot
installs add-ons from the network on every boot.
