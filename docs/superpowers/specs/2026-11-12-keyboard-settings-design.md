# Keyboard settings in the menu

Date: 2026-11-12. Branch: `keyboard-settings`.

## Intent

A ThinFloe stick ships with a US layout, CapsLock as Ctrl and Compose on
Right Alt. Today a user who wants another layout edits `XKB_DEFAULT_*` lines
in `~/.thinfloe-environment` from a VT and logs out. The Settings screen gets
a Keyboard tab that does this with three controls, and the change takes effect
at once, without a log-out. The owner asked for the immediate effect
explicitly.

## Goals

- Pick the keyboard layout and its variant.
- Switch "CapsLock acts as Ctrl" on or off.
- Pick the Compose key.
- A change applies to the running session at once: the menu, programs started
  from it and, if possible, an open ThinLinc client.
- The choice survives a reboot (it lives in the home, which the save keeps).

## Non-goals

- Two or more layouts with a toggle key.
- Keyboard models other than the system `pc105`.
- XKB options other than `ctrl:nocaps` and `compose:*`.
- A keyboard choice in the initramfs or on the boot screen.

## Anti-goals

- A machine configured by hand in `~/.thinfloe-environment` changes its
  keyboard after the upgrade. Hand settings keep winning.
- A failed apply leaves the session with no working keyboard.
- The menu edits `~/.thinfloe-environment`. It holds TZ, cursor and other
  hand-written lines, and it is the user's file.

## The tab (decided with the owner, mockup A)

A new tab "Keyboard" between Screen and Status. One card, rows with a label
column of `brand::FORM_LABEL_W` like the Screen tab's idle band:

- `Layout`: select of every layout in the XKB registry, shown by its
  description ("German (Switzerland)"), sorted by description.
- `Variant`: select of the chosen layout's variants. The first entry is
  "Default" (no variant). A change of layout resets the variant to Default.
- `CapsLock acts as Ctrl`: switch. Default on.
- `Compose key`: select with these entries, in this order: None, Right Alt
  (default), Menu, Right Ctrl, Left Win, Right Win, Scroll Lock, Pause. They map
  to no option, `compose:ralt`, `compose:menu`, `compose:rctrl`,
  `compose:lwin`, `compose:rwin`, `compose:sclk`, `compose:paus`.
- A note under the card, in the muted colour, telling the state: nothing when
  the apply worked; the error text when it failed.

A second card holds `Try it here`: a single-line text field, kept only in
memory, to try the layout at once.

Every control change applies immediately; there is no Apply button (the Screen
tab works the same way).

## Where the setting lives (decided)

The menu owns a new file `~/.config/thinfloe/keyboard`, written through
`owned::write` with the menu's header line, atomically. Content:

```
# Written by the ThinFloe menu. Edits here are replaced.
XKB_DEFAULT_LAYOUT=ch
XKB_DEFAULT_VARIANT=de_nodeadkeys
XKB_DEFAULT_OPTIONS=ctrl:nocaps,compose:ralt
```

All three keys are always written, an empty value included
(`XKB_DEFAULT_VARIANT=`, `XKB_DEFAULT_OPTIONS=` when both options are off).
Reason: on `--reconfigure` labwc `setenv()`s only the keys present in its
environment file, so a key left out keeps its previous value. Going from
`ch`/`de_nodeadkeys` back to `us` without an empty variant line would ask
XKB for `us(de_nodeadkeys)`. The builder verifies in the runtime test that an
empty `XKB_DEFAULT_VARIANT` gives the layout's default variant. If it does not,
the builder finds a value that does and records the finding in
`docs/notes/session.md`.

The file follows the `owned.rs` rules like `~/.config/thinfloe/screen`:
missing means defaults; a file without the header is by hand, and the tab
then shows a note and a button to replace it, as the Screen tab does.

## Order of sources (decided)

1. `/etc/thinfloe/labwc/environment` (system defaults).
2. `~/.config/thinfloe/keyboard` (the menu).
3. `~/.thinfloe-environment` (by hand).

Both places that read them use this order: `10-thinfloe-session.sh` (sourced
with `set -a` before labwc starts) and the merged `environment` file that
`thinfloe-compose-labwc-config` writes for `labwc -C`. The composer takes the
user files as an ordered list instead of one file. The list of user files
lives in one place, so the startup and the menu's apply cannot use different
lists.

If `~/.thinfloe-environment` has an active (not commented) assignment to any
`XKB_DEFAULT_*` key, the hand file decides. The tab then shows the values
in effect, disables every control except the test field, and shows the note
"set by hand in ~/.thinfloe-environment; remove its XKB_DEFAULT_ lines to
use this tab". An `export ` prefix counts as an assignment too.

The menu's file stays in the source list in this case too (my call, raised by
the builder check). The hand file wins key by key, which is what the shell
order already gives; the shown values are the merged result in effect. Making
the list conditional would need the same detection in the composer, the
profile and the menu. The detection exists only in the menu, for the lock.

## Applying a change (decided)

After writing the file the menu:

1. rewrites only the `environment` file in `$XDG_RUNTIME_DIR/thinfloe-labwc`,
   through a new environment-only mode of the composer (temp file in the same
   directory, then rename). The full rebuild (`rm -rf` and new symlinks) stays
   for the session start only;
2. runs `labwc --reconfigure` (it reads `LABWC_PID`, which labwc sets for its
   children).

Reason for step 1 (my call, raised by the builder check):
`/usr/libexec/thinfloe/labwc-scale` replaces the `rc.xml` and
`themerc-override` symlinks in that directory with scaled copies and adds the
menu's `MoveToOutput` rule. A full rebuild would wipe them, and `labwc-scale`
reacts only to a change of the scale file, so it would never restore them.
The keyboard apply therefore touches `environment` and nothing else. Both
programs replace their files atomically and each ends with its own
`--reconfigure`; every SIGHUP re-reads all files, so the last one always sees
the final state and the two need no common owner.
`test-labwc-scale.sh` gets a case: scale applied, then a keyboard apply, and
the scaled `rc.xml` is still there.

labwc 0.9.3, on SIGHUP, calls `session_environment_init()` (re-reads
`<config_dir>/environment`) and then `seat_reconfigure()`, which builds a new
keymap from `XKB_DEFAULT_*` for each keyboard (`src/server.c`
`handle_sighup`, `src/seat.c`, `src/input/keyboard.c` `set_layout`). Wayland
clients and Xwayland get the new keymap from wlroots.

Both steps run as child processes of the menu with the existing job rules
(own process group, no blocking of the event loop). A non-zero exit puts the
error under the card; the file stays written, so the next session start uses
it anyway.

If the keymap cannot be compiled, labwc itself falls back to `us`
(`set_layout`), so a bad value cannot leave the session without a keyboard.
The menu offers only registry entries, so this needs a hand-made file.

The session started with the plain `/etc/thinfloe/labwc` fallback (the merge
failed at startup) cannot be reconfigured this way: the menu then shows "takes
effect at the next start" and skips step 2. The builder finds out how the menu
knows which config dir labwc uses (for example an environment variable the
profile exports).

## The layout list (decided)

Read at runtime from `/usr/share/X11/xkb/rules/evdev.xml` (xkb-data,
253 kB on the build host). Parse `layoutList/layout`: `configItem/name`,
`configItem/description`, and `variantList/variant` the same way. Read
bounded, like every other file the menu parses. `roxmltree` is already in
the tree through `resvg`/`usvg`; use it directly if it is. If the file is
missing or broken, the Layout select shows only the current value and the
note says why.

Values that the file does not list (a hand file with an unknown layout) are
shown as their raw name.

## Constraints

- UI rules in CLAUDE.md: theme colours, named text sizes in `brand.rs`,
  shadcn-egui `lyra`, no style literals (`ui_code_has_no_style_literals`).
- labwc keeps zero keybindings. The keyboard apply does not write `rc.xml`
  (`labwc-scale` is its only writer in a running session).
- `Makefile` inputs: a new overlay file or menu source needs its
  prerequisite line (`test-container-inputs.sh`).
- The composer stays POSIX `sh`; `test-labwc-config-merge.sh` and
  `test-labwc-runtime.sh` keep passing and get new cases.
- `xkb-data` must be in the image; add a check to `test-session-packages.sh`.
- `~/.thinfloe-environment`'s comment block and `docs/notes/session.md` name
  the new file and the order.
- CHANGES.md gets an entry.

## Tests the builder adds

- Unit (Rust): parsing the registry (a small fixture XML), building the
  options string from the switch and compose choice and back, detecting hand
  `XKB_DEFAULT_*` lines in `~/.thinfloe-environment`, the file content with
  empty values, ownership states.
- UI test in `ui.rs` like the idle band's: a select change yields the event.
- `test-labwc-config-merge.sh`: three sources in the right order, missing
  files skipped.
- `test-labwc-runtime.sh`: start labwc headless, write a keyboard file,
  compose, `labwc --reconfigure`, and show that the keymap a client receives
  changes (for example with `xkbcli` or a Wayland probe that prints the
  keymap's layout name). This is the proof that "at once" works.

## Left to verify on hardware

- An open ThinLinc client follows the new layout. Expected through
  Xwayland's keymap update, unproven. If it does not, the note under the card
  says "ThinLinc picks up the change when it starts again" and the
  hardware result goes into `docs/notes/session.md`.

## Left to the builder

- Event and state names in `settings.rs`/`menu.rs`, module name (suggest
  `keyboardtab.rs`).
- How the apply job reports back to the event loop (reuse the existing job
  machinery).
- Whether reading the registry happens once at start or on first visit of the
  tab.
