# Num Lock at start: a Keyboard tab setting

## Intent

Small keyboards with a number pad laid over the letter keys type digits
instead of letters while Num Lock is on. Today Num Lock is always on
(`<numlock>on</numlock>` in `overlay/etc/thinfloe/labwc/rc.xml`). The owner
wants it settable per machine in Settings -> Keyboard.

## Goals and non-goals

- Goal: a switch "Num Lock on at start" on the Keyboard tab; default on, so
  nothing changes for existing homes.
- Non-goal: changing Num Lock in the running session. labwc 0.9.3 sets the
  state only when a keyboard is added (`new_keyboard()` in `src/seat.c`,
  at start and on hot-plug); `labwc --reconfigure` does not touch it.
- Non-goal: ThinLinc's own Num Lock handling.

## Approaches

1. **Chosen:** the value travels in `generated/environment` as
   `THINFLOE_NUMLOCK=on|off`; `thinfloe-compose-labwc-config` (full
   compose, at session start) writes a copy of the system `rc.xml` with the
   `<numlock>` value into `<output-dir>/.numlock/rc.xml` when it is `off` and
   links `rc.xml` there. `labwc-scale` already reads its source through the
   link, so its scaled copies keep the value.
2. A separate `generated/numlock` file: one more file for the store to
   rebuild `config.toml` from; no gain over the environment line.
3. A Wayland virtual keyboard from the menu: labwc keeps virtual keyboards
   out of the keyboard group, so it cannot change the real keyboard's lock.

## Design

- `config.toml`: `[keyboard] numlock = true|false`, serde default true.
- `generated/environment` gets `THINFLOE_NUMLOCK=on|off`; the rebuild from
  generated files (`convert::keyboard_from_env`) reads it back (missing:
  on; another value: note, on).
- `/etc/thinfloe/menu-managed-keys` lists `THINFLOE_NUMLOCK`, so a line in
  the user's `environment` cannot override the tab.
- Composer, full mode only: value `off` -> copy with `<numlock>off</numlock>`;
  anything else -> plain link as today. `--env-only` leaves `rc.xml` alone.
- Tab: after an apply that changed only or also Num Lock, the note says
  "Num Lock follows at the next start." (the keymap part may be live).
- Docs: CHANGES (New), `docs/notes/menu.md` Keyboard tab,
  `docs/notes/home-persistence.md` table, CLAUDE.md line on dropped keys,
  `test-session-packages.sh` managed-keys check.

## Left to the builder

Exact wording of the help text under the switch; test placement.
