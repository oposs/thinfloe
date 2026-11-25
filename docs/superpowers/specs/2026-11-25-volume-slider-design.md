# Volume slider on a Sound tab (2026-11-25)

Owner request: "we have to cater for people without volume keys, could there
be a volume slider in the settings screen?"

## Decisions

1. **A new Settings tab "Sound"**, between Keyboard and Status
   (`SettingsTab::Sound`, label `Sound`). It holds, in this order:
   - Switch **"Set the volume on this machine"**. It is the same setting as
     "Volume keys control this machine" on the Keyboard tab
     (`[keyboard] volume_keys`, `THINFLOE_VOLUME_KEYS`); both switches show the
     one value and both send `KbEvent::VolumeKeys` through the existing
     keyboard apply. No new config key, no migration. The Keyboard tab row
     stays as it is (people with keys look there).
     Note under it (muted text, like `media_keys_note`): "When on, the slider
     and the volume keys set every speaker, and the volume keys no longer
     reach the ThinLinc session or any other program. When off, every speaker
     is at 100 % and the remote session sets the loudness."
   - **"Volume"**: a shadcn-egui `Slider` 0..=100, step 1, with the value as
     "NN %" beside it.
   - **"Mute"**: a `Switch`.
   - Slider and Mute are disabled (greyed, not hidden) while the switch is
     off, and while the volume is not known (no state file, see 3); then one
     muted line says why: "The remote session sets the loudness." (switch
     off) or "The volume is not known yet." (no state file). While the
     switch is shown on but the file does not say `on` (the keyboard apply
     still runs, or failed), they stay disabled with "Turning on…". A
     failed keyboard apply's note shows under the switch, as on the
     Keyboard tab.
   Why a tab of its own: a person without volume keys does not look on the
   Keyboard tab. Why the same switch: the switch already means "this machine
   sets the volume" (with it off the OSD holds every speaker at 100 %), so a
   slider without it would be undone at the next start.

2. **The OSD stays the only owner of the shared state** (`osd::sync::Shared`).
   The menu sends two new datagrams on the OSD socket (`thinfloe-osd.sock`):
   - `volume-set <n>\n`, n in 0..=100 (anything else is not an action);
   - `volume-mute-set 0\n` / `volume-mute-set 1\n`.
   `Action` gets `VolumeSet(u8)` and `VolumeMuteSet(bool)` (parse/as_str
   round-trip; `Action::ALL` users adapted). `--osd-key` refuses them like an
   unknown action (exit 2): labwc never binds them.
   The OSD maps them to new `AudioRequest::SetLevel(u8)` /
   `AudioRequest::SetMute(bool)`; `Shared::set_level(n, sinks)` and
   `Shared::set_mute(m, sinks)` set `L`/`M` (and mark the sink state known,
   like `step`) and return the sets for every sink. In `Mode::Off` they do
   nothing (no sets, state unchanged). They show **no OSD panel** (the slider
   is the feedback). Several `SetLevel` waiting in the worker's queue collapse
   to the last one (a drag must not queue dozens of pactl rounds).
   Without an OSD (send gives `NoServer`), the menu does nothing and logs
   once; the tab shows "The volume is not known yet." since there is no
   state file.

3. **Readback: a state file** `$XDG_RUNTIME_DIR/thinfloe-volume`, written by
   the OSD atomically (temp file + rename, like the scale file), one line
   `<percent> <muted> <on|off>\n` with muted `0` or `1` and the mode the
   OSD acts on (`on` is `Mode::Keys`); readers ignore further fields, and
   a 2-field file counts as mode unknown, i.e. not yet on. Written whenever
   the shared sink state or the mode is set or changes (start, keys,
   slider, mode change) — in `Mode::Off` too (it then says `100 0 off`).
   Removed by the OSD on a clean exit (SIGTERM). The menu reads it
   (`bounded` reader, not a sysfs read) when the Sound tab is shown and
   then about once a second; while the person drags, the menu shows its own
   value and sends at most one `volume-set` per 50 ms plus the final value
   on release, and takes the file's value again 1 s after the last send,
   from a read made after that second (each send pulls the next read in to
   the end of its hold), never from a read made during it.

4. Saving needs nothing new: WirePlumber stores the volume in
   `~/.local/state/wireplumber/`, which travels with the home archive (and
   the OSD start keeps it since #52).

## Docs and tests
- CLAUDE.md, Menu rules: one line for the `thinfloe-volume` file contract
  and the two datagrams.
- docs/notes/media-keys.md: the datagrams, the state file, the Sound tab.
- docs/notes/menu.md: the Sound tab.
- CHANGES.md "New": one entry.
- Unit tests: sync set_level/set_mute (Keys and Off), Action parse/round
  trip and rejects (`volume-set 101`, `volume-set x`), queue collapse,
  state-file writer (atomic, format, Off says `100 0`), the menu side's
  parsing of the file (missing, garbage, trailing fields), the Sound tab's
  enable/disable logic, and the throttle.
