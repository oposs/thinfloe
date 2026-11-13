# One config directory, and keyboard settings in the menu

Date: 2026-11-13. Branch: `keyboard-settings`. Replaces the keyboard-only
design of 2026-11-12 (same file, renamed).

## Intent

ThinFloe started without a config frontend. Settings grew as files in many
places of tfuser's home, each with its own format, and the menu learned to
manage some of them while still accepting hand edits (`owned.rs`, `.by-hand`
copies, a session profile that wins over the menu). Now that the menu covers
what a user can reasonably expect to set, the owner wants to cut the ties to
that legacy instead of adding more duplication: one directory,
`~/.config/thinfloe/`, one TOML file for everything the menu manages, and a
one-time migration that parks the old files in `~/cfg-attic/`.

The first new setting on top of it is the keyboard: layout, variant,
CapsLock as Ctrl and the Compose key, picked in a new Settings tab and applied
to the running session at once.

The work has two parts, built in this order:

- Part A: the config directory, `config.toml`, generated files, the
  migration.
- Part B: the Keyboard tab.

## Goals

- Every ThinFloe-owned file in the home lives under `~/.config/thinfloe/`.
- Everything the menu UI manages lives in `~/.config/thinfloe/config.toml`,
  written only by the menu.
- Two escape hatches stay, in the same directory, for things the menu does
  not manage: `environment` (variables) and `sessionrc` (commands at session
  start).
- An upgraded machine keeps all its settings, WiFi included, without the user
  doing anything.
- The old files are kept, unchanged, in `~/cfg-attic/`.
- Keyboard: pick layout and variant, CapsLock as Ctrl on or off, Compose key;
  a change applies at once.

## Non-goals

- Hand-editing `config.toml`. A hand edit may work; nothing promises it, and
  the menu rewrites the file.
- Moving files that belong to other programs: `~/.thinlinc/tlclient.conf`,
  `~/.config/oxulnk`, `~/.local/state/wireplumber/`.
- Converting the files back for a downgrade. The attic holds the originals;
  putting them back is a hand job.
- Keyboard: two layouts with a toggle key, models other than `pc105`, XKB
  options other than `ctrl:nocaps` and `compose:*`, a keyboard choice in the
  initramfs.

## Anti-goals

- A machine on WiFi is offline after the upgrade.
- A setting is read from two places, so that a hand file and the menu can
  disagree. The pushback review showed one way this breaks the keyboard and
  can stop labwc: a menu variant plus a hand layout gives `fr(de_nodeadkeys)`,
  which XKB cannot build, and labwc's single `us` fallback keeps the variant.
- A failed migration loses a file. Every old file ends in the attic or stays
  where it was.

## Part A: the config directory

### Layout of `~/.config/thinfloe/`

| Path | Writer | Readers | Holds |
|---|---|---|---|
| `config.toml` | menu | menu | everything the menu manages |
| `environment` | by hand | session profile, labwc config composer | extra variables (cursor, `TZ`, `LABWC_FULLSCREEN_SPAN_OUTPUTS`, anything else) |
| `sessionrc` | by hand | `thinfloe-session` | shell commands run at session start |
| `generated/` | menu | see below | files derived from `config.toml` |
| `menu/*.desktop` | by hand | menu, `thinfloe-addon` | user tiles (unchanged location) |
| `addons` | `thinfloe-addon` (root) | `thinfloe-addon` | ticked add-ons (unchanged location) |
| `password` | `thinfloe-password` | `thinfloe-password --apply` | remote-access hash (was `~/.thinfloe-password`) |
| `ssh-host-keys/` | `thinfloe-ssh-hostkeys` | same | host keys (was `~/.thinfloe-ssh-host-keys/`) |
| `wg0.conf` | by hand | initramfs | WireGuard (was `~/wg0.conf`) |
| `save-home-exclude` | by hand | `thinfloe-save-home`, `thinfloe-upgrade` | was `~/.save-home-exclude` |
| `home-version` | migration | initramfs, every reader with an old-path fallback | home format marker (was `~/.thinfloe-home-version`) |

`addons` stays a file of its own because a root tool writes it; two writers on
`config.toml` would race. `menu/` stays a directory of desktop files because
that is the format the tiles already are. Secrets (`password`,
`ssh-host-keys/`, `wg0.conf`) stay out of `config.toml` so the fresh-install
copy of a home can keep leaving them out (`IDENTITY_EXCLUDES` in
`thinfloe-upgrade` follows the new paths). WiFi passwords move into
`config.toml`; they are in `wifi.yaml` today, so the exposure described in
[home-persistence.md](../../notes/home-persistence.md) does not change.

The 3.x check in the initramfs (`tf_migrate_home_names`, "archive without
`.thinfloe-home-version`") must count `.config/thinfloe/home-version` in the
listing as a marker too, or a migrated home would be renamed again.

### Which layout a home has (my call, raised by the builder check)

The shipped default home has the new layout: `home-version` = 2 and the new
`environment` and `sessionrc` templates, and none of the old templates. The
restore extracts the archive over it (`thinfloe-lib.sh:447`), so the marker
on disk says nothing about the archive. As with the 3.x rule, the archive
listing decides:

- No archive (fresh install): new layout.
- Archive listing with `.config/thinfloe/home-version`: new layout.
- Archive listing without it: legacy layout. The initramfs deletes the
  shipped `home-version` after the extraction, so a save before a successful
  migration cannot archive a marker the home has not earned.

The initramfs writes the answer to `/run/thinfloe/state/home-layout`
(`legacy` or `new`). Every reader in the fallback rule below reads that
state file, never the marker on disk; the menu's migration runs when it says
`legacy` and rewrites it to `new` after step 4.

### `config.toml`

Written by the menu, atomically (temp file and rename), with the menu's header
comment. It has a `version = 1` key and one table per area:

- `[keyboard]`: `layout`, `variant`, `caps_as_ctrl`, `compose` (Part B).
- `[screen]`: `idle_timeout` (seconds, 0 = never), from
  `~/.config/thinfloe/screen` and `TF_IDLE_TIMEOUT`.
- `[status]`: the status area layout, from `~/.config/thinfloe/status`.
- `[monitors]`: the monitor arrangement the Screen tab edits today in
  `~/.config/kanshi/config`.
- `[network]`: the WiFi networks, the Ethernet settings and DNS servers the
  Network tab edits today in `~/wifi.yaml` and `~/ethernet.yaml`.

The schema of each table is left to the builder, with one rule: it holds
exactly what the menu UI can express today, so a write and a read give the same
UI state. The Rust `toml` crate (a new direct dependency) reads and writes it.
A file the menu cannot parse is moved to `config.toml.broken-<epoch>`. The
menu then rebuilds `config.toml` from the files in `generated/` with the
migration's converters (they read netplan, kanshi and the environment format
anyway) and says so on the Settings screen. It never starts from defaults
while `generated/` has files, because a default `[network]` would write an
empty `generated/wifi.yaml` and the next boot would be offline.

### Generated files

Some readers cannot parse TOML or must run before the menu. The initramfs
(shell only, no Python, see the standing constraint on its commands) copies
the netplan files before switch-root, labwc needs the keyboard before it
starts, and kanshi reads its own format. For them the menu writes derived
files into `~/.config/thinfloe/generated/` every time `config.toml` changes:

- `wifi.yaml`, `ethernet.yaml`: netplan, mode 0600, read by
  `install_local_network()` in the initramfs and by `thinfloe-network`.
- `kanshi`: the kanshi config; `thinfloe-session` starts kanshi with
  `-c` on it.
- `environment`: the `XKB_DEFAULT_*` lines (and any later menu-managed
  variable), read by the session profile and the labwc config composer.
- `screen`: `idle_timeout=N` for `thinfloe-session`.

They are in the home, so the save keeps them and they exist at the next boot
before the menu runs. Each starts with a comment that it is generated and
edits are lost. A generated file is written only when its content changes, so
a keyboard change leaves `generated/wifi.yaml` alone. The menu writes the
generated files first and renames `config.toml` last. A power loss in between
leaves the old `config.toml` with possibly newer generated files; at every
start the menu regenerates them from `config.toml`, so they agree again. When
that start-up regeneration changes `generated/kanshi` (kanshi runs before the
menu), the menu reloads kanshi.

Two flows can be undone and keep today's behaviour (my call, raised by the
builder check):

- Monitors: an Apply writes `generated/kanshi`, keeps the previous one as
  `generated/kanshi.prev` and writes the marker
  `generated/layout-unconfirmed`, as today in `~/.config/kanshi/`.
  `thinfloe-session` undoes an unconfirmed layout on these files before it
  starts kanshi. `[monitors]` in `config.toml` is written only at Confirm, so
  the start-up regeneration produces the confirmed layout, which is also what
  the session's undo puts back.
- Network: `network_apply` (`jobs.rs:680`) writes `generated/wifi.yaml` and
  `generated/ethernet.yaml`, runs `thinfloe-network apply` and puts the old
  generated files back on failure, as it does with the home files today.
  `[network]` is written only after the apply succeeded.

The `owned.rs` take-over machinery (`.by-hand` copies, "set by hand" notes,
the session profile winning over the menu) goes away: the menu is the only
writer of `config.toml` and `generated/`.

### The two escape hatches

`environment` is merged after the system defaults and after
`generated/environment`. Keys the menu manages must not come from it. The
list of managed keys lives in one file, `/etc/thinfloe/menu-managed-keys`
(today: `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_OPTIONS`,
`XKB_DEFAULT_MODEL`). The composer drops those lines from the user file
before it merges, and the profile sources the composer's merged output instead
of the separate files, so the shell and labwc see the same values. The menu
shows a warning on the Settings screen naming every dropped line.

`sessionrc` is sourced by `thinfloe-session` inside the session, as
`~/.thinfloe-sessionrc` is today. `TF_IDLE_TIMEOUT` is no longer read from it
(the timeout is `[screen]`). The shipped home has both files as comment-only
templates naming what they are for and what the menu manages instead.

### The migration

Runs once, in the menu binary (`thinfloe-menu --migrate-home`), called by the
session profile before the labwc config is composed, so the first session
already uses the converted keyboard and screen settings. Trigger:
`/run/thinfloe/state/home-layout` says `legacy`. The 3.x renames in
the initramfs run first and are unchanged.

Steps:

1. Read every old file that exists and convert what the menu manages into
   `config.toml`:
   - `~/.thinfloe-environment`: the `XKB_DEFAULT_*` lines go to
     `[keyboard]` (layout, variant, `ctrl:nocaps`, `compose:*`; other XKB
     options are dropped and reported); every other active line goes to the
     new `environment`. A comma list in `XKB_DEFAULT_LAYOUT` or
     `XKB_DEFAULT_VARIANT` keeps its first entry, and a model other than
     `pc105` is dropped; both are reported in the attic README and named in
     CHANGES. The owner confirmed (2026-11-13) that no machine uses two
     layouts today.
   - `~/.thinfloe-sessionrc`: the `TF_IDLE_TIMEOUT`/`DBRRG_IDLE_TIMEOUT`
     assignment goes to `[screen]`; the rest of the file is copied to the new
     `sessionrc`.
   - `~/.config/thinfloe/screen`, `~/.config/thinfloe/status`,
     `~/.config/kanshi/config`, `~/wifi.yaml`, `~/ethernet.yaml`: through the
     menu's existing parsers. A hand-written file the parser does not fully
     understand is converted as far as the parser goes and reported.
   - `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys/`, `~/wg0.conf`,
     `~/.save-home-exclude`: copied unchanged to their new paths, modes kept.
     Their attic copies join `IDENTITY_EXCLUDES`, so a fresh install does not
     clone the old machine's identity from the attic.
2. Write `config.toml`, the generated files and the copied files.
3. Move every old file and directory into `~/cfg-attic/`, keeping its
   relative path (`~/cfg-attic/.thinfloe-environment`,
   `~/cfg-attic/.config/kanshi/config`). The shipped templates count as old
   files too. The kanshi `config.prev` and `thinfloe-layout-unconfirmed`
   state files and the `.by-hand` copies move with them.
4. Write `home-version` = 2 and `~/cfg-attic/README` listing what was
   converted, what was copied and what was dropped or only partly converted.

A failure in step 1 or 2 leaves every old file in place, writes no
`home-version`, logs the reason and lets the session start on the old files
(see the fallback rule below). The next start tries again. A failure in step 3
leaves the remaining old files where they are; once `home-version` is 2 they
are no longer read.

The menu shows the migration report once, on the Settings screen, until the
user dismisses it.

Fallback rule for every reader that runs before the menu (initramfs,
`thinfloe-password`, `thinfloe-ssh-hostkeys`, the session profile,
`thinfloe-session`, `thinfloe-save-home`): read the new path; if
`/run/thinfloe/state/home-layout` says `legacy`, read the old path. On the first boot
after the upgrade the initramfs runs before the migration and installs the
old `~/wifi.yaml`, so the machine is online; from the second boot it reads
`generated/wifi.yaml`. A failed migration is harmless for the same reason.

### Callers that change paths

All of these follow the new paths, with the fallback above where they run
before the migration can have happened: the initramfs
(`install_local_network`, the home-version check), `thinfloe-session`, the
session profile, `thinfloe-compose-labwc-config`, `thinfloe-save-home`,
`thinfloe-upgrade` (`IDENTITY_EXCLUDES`, home copy), `thinfloe-password`,
`thinfloe-ssh-hostkeys`, `thinfloe-network`, the menu. The guarding tests
(`test-initramfs-home.sh`, `test-save-home.sh`,
`test-session-lifecycle.sh`, `test-session-packages.sh`,
`test-password.sh`, `test-ssh-hostkeys.sh`, `test-labwc-config-merge.sh`,
`test-labwc-runtime.sh`, `test-field-report.sh`,
`test-network-netplan.sh`, `test_upgrade_image.py`, `test_network.py`,
the Rust tests in `owned.rs`, `app.rs`, `jobs.rs`, `netfiles.rs`, `ui.rs`,
`monitors_ui.rs`) move with them. CLAUDE.md, `docs/notes/` and the README
change wherever they name an old path.

## Part B: the Keyboard tab

### The tab (decided with the owner, mockup A)

A new tab "Keyboard" between Screen and Status. One card, rows with a label
column of `brand::FORM_LABEL_W` like the Screen tab's idle band:

- `Layout`: select of every layout in the XKB registry, shown by its
  description ("German (Switzerland)"), sorted by description.
- `Variant`: select of the chosen layout's variants. The first entry is
  "Default" (no variant). A change of layout resets the variant to Default.
- `CapsLock acts as Ctrl`: switch. Default on.
- `Compose key`: None, Right Alt (default), Menu, Right Ctrl, Left Win, Right
  Win, Scroll Lock, Pause. They map to no option, `compose:ralt`,
  `compose:menu`, `compose:rctrl`, `compose:lwin`, `compose:rwin`,
  `compose:sclk`, `compose:paus`.
- A note under the card in the muted colour: nothing when the apply worked,
  the error otherwise.

A second card holds `Try it here`: a single-line text field, kept only in
memory. Every change applies immediately; there is no Apply button.

Defaults (as today): `us`, no variant, CapsLock as Ctrl, Compose on Right Alt.

### The registry

Read from `/usr/share/X11/xkb/rules/evdev.xml` (xkb-data, 253 kB on the build
host), bounded, with `roxmltree` (already in `Cargo.lock` through `resvg`;
make it a direct dependency). `layoutList/layout/configItem` gives `name` and
`description`; `variantList/variant` the same. If the file is missing or
broken, the Layout select shows only the current value and the note says why.
`test-session-packages.sh` checks that the image has the file.

### Applying a change

1. Validate: the variant must be one of the layout's variants in the registry
   or empty, and the options must come from the fixed set. `labwc
   --reconfigure` exits 0 even when labwc cannot build the keymap (labwc
   `src/main.c:108-121`), so the menu cannot learn about a bad keymap
   afterwards.
2. Write `config.toml` and `generated/environment`. All three keys are always
   written, an empty value included (`XKB_DEFAULT_VARIANT=`): on reconfigure
   labwc `setenv()`s only the keys in its environment file, so a key left out
   keeps its old value. An empty variant gives the layout's default variant
   (checked with libxkbcommon by the pushback review).
3. Rewrite only the `environment` file in `$XDG_RUNTIME_DIR/thinfloe-labwc`
   through a new environment-only mode of the composer (temp file in the same
   directory, then rename). The full rebuild stays for the session start:
   `/usr/libexec/thinfloe/labwc-scale` replaces the `rc.xml` and
   `themerc-override` symlinks there with scaled copies and reacts only to a
   change of the scale file, so a rebuild would lose the scaled title bars for
   good. Both programs replace their files atomically and end with their own
   `--reconfigure`; every SIGHUP re-reads all files, so the last one sees the
   final state.
4. Run `labwc --reconfigure` (labwc sets `LABWC_PID` for its children).

labwc 0.9.3 on SIGHUP runs `session_environment_init()` and then
`seat_reconfigure()`, which builds a new keymap for each keyboard
(`src/server.c` `handle_sighup`, `src/seat.c`, `src/input/keyboard.c`
`set_layout`).

Steps 2 to 4 run as child processes with the existing job rules, one apply at
a time. A change made while an apply runs is merged into the next apply, so a
slow old job cannot report after a newer one. The note shows failures of the
write, of the composer and a missing `LABWC_PID`.

If the session started on the plain `/etc/thinfloe/labwc` fallback (the merge
failed at startup), the menu writes the files, skips steps 3 and 4 and shows
"takes effect at the next start". The profile exports the config dir it gave
labwc, so the menu can tell.

## Constraints

- UI rules in CLAUDE.md: theme colours, named text sizes in `brand.rs`,
  shadcn-egui `lyra`, nothing rounded, `ui_code_has_no_style_literals`.
- labwc keeps zero keybindings; the keyboard apply does not write `rc.xml`.
- The initramfs stays shell with the commands `module-setup.sh` installs, no
  `exit` in a hook, `tf_log` on stderr.
- The per-machine network config is still installed by the initramfs, never
  by a unit; netplan files stay mode 0600.
- The home restore stays in the initramfs. The migration restores nothing; it
  converts files that are already on disk.
- `Makefile` prerequisites for every new overlay file and menu source.
- The composer stays POSIX `sh`.
- CHANGES.md gets entries for both parts. The attic and the end of
  `~/.thinfloe-environment` and `~/.thinfloe-sessionrc` are a visible change
  for administrators who edit those files today.

## Tests the builder adds

- Rust unit: `config.toml` round trip per table; each generated file from a
  config; the migration from fixture homes (a 4.x home with hand files, one
  with menu-owned files, one with broken files, an already migrated home); the
  registry parser on a small fixture XML; the options string; validation.
- `test-initramfs-home.sh`: `home-layout` is `new` without an archive and
  with a marked archive, `legacy` with an unmarked one (and the shipped marker
  is gone from disk); the initramfs takes `generated/wifi.yaml` for `new` and
  `~/wifi.yaml` for `legacy`, the same for `wg0.conf`; a
  home with only `.config/thinfloe/home-version` is not renamed by the 3.x
  migration.
- `test-labwc-config-merge.sh`: managed keys are dropped from the user
  `environment`; the order is system, generated, user; the environment-only
  mode leaves the other files alone.
- `test-labwc-scale.sh`: scale applied, then a keyboard apply, and the scaled
  `rc.xml` is still there.
- `test-labwc-runtime.sh`: after a keyboard apply and `--reconfigure` the
  merged file has the new values and labwc's log has no
  `failed to create xkb keymap`. The headless rig has no keyboard device
  (`test-labwc-runtime.sh:77-82`), so it cannot show a client receiving the
  new keymap.
- `make qemu-smoke-upgrade`: a 4.x home with `wifi.yaml`, a keyboard line and
  a sessionrc boots, migrates, and the second boot reads the generated files.

## Left to verify on hardware

- The new keymap reaches the menu and other Wayland clients at once. No
  automated test can show this (see above).
- An open ThinLinc client follows the new layout. If it does not, the note
  says "ThinLinc picks up the change when it starts again" and the result goes
  into `docs/notes/session.md`.

## Left to the builder

- The TOML schema of each table, within the round-trip rule.
- Module names (suggest `config.rs`, `migrate.rs`, `keyboardtab.rs`), event
  and state names, how the apply job reports back.
- Whether the registry is read at start or on the first visit of the tab.
- The exact text of the migration report and the attic README.
