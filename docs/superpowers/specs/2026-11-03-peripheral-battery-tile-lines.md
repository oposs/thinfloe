# Battery tile: one line per device, named by kind

Follow-up to `2026-11-03-peripheral-battery-tile.md`, after the owner saw
the tile on 10.46.102.86 (2026-11-03): with the big "lowest percent" name
and shortened model names it was not clear which device is the keyboard
and which the mouse.

## Decisions (owner, 2026-11-03)

- No big percent on top. The tile lists **one line per device**, in
  English: the device kind and its charge, e.g.

  ```
  Keyboard   100 %
  Mouse       95 % +
  ```

  `+` = charging; `–` instead of the number when the device is not
  reachable or reports no percent.
- All lines below each other. When they do not all fit in the tile, the
  tile shows them **page by page like slides**, switching every few
  seconds.

## Where the kind comes from

- **Bolt receiver:** register `b5` / `50+n` (pairing info, already read to
  decide whether slot `n` is paired). Measured on 10.46.102.86:
  slot 1 (MX Anywhere 3S, a mouse) `11 ff 83 b5 51 42 …`, slot 2
  (MX Mechanical, a keyboard) `11 ff 83 b5 52 01 …`: the low nibble of the
  first byte after the sub-address is the kind, `1` keyboard, `2` mouse
  (the same numbering Solaar uses for Bolt). Other values: `3` numpad,
  `4` presenter, `8` trackball, `9` touchpad (Solaar's list; unmeasured);
  anything else is unknown.
- **power_supply:** no reliable kind; unknown. (If the uevent offers a
  usable hint, e.g. the name contains "Keyboard" or "Mouse", it may be
  used; not required.)
- Unknown kind: the line uses the device name shortened to fit.

## File format

The battery file gets a fifth field, the kind, as a lowercase word:
`name<TAB>percent<TAB>state<TAB>read_epoch<TAB>kind` with kind one of
`keyboard`, `mouse`, `numpad`, `presenter`, `trackball`, `touchpad`,
`unknown`. The menu still accepts 4-field lines (kind unknown) so an old
file from before an upgrade within the same boot does not blank the tile.
The kind is kept with the cached name for a device that is not reachable
(so it keeps its line label), and the previous file's kind is carried for
a `not reachable` line.

## Tile drawing

- Line text: `Keyboard`, `Mouse`, `Numpad`, `Presenter`, `Trackball`,
  `Touchpad`, else the shortened name; then the percent right-aligned
  (`100 %`, `95 %`, `–`) and `+` when charging. Two devices of the same
  kind: `Keyboard 1`, `Keyboard 2` in file order.
- A line whose device is reachable and has **less than 10 %** (or reports
  state `critical`) is drawn **red** (owner, 2026-11-03: "rot wenn zu
  wenig <10%"). This replaces the earlier warn rule (<= 15 %, `low`).
  Use the palette's red if `brand.rs` has one, else add one there that
  reads on the tile background in the theme. `not reachable` lines are
  muted; all others the normal foreground.
- The log line of such a device stays a warning (as now), with the same
  < 10 % / `critical` rule.
- The lines use the space of the tile's name and comment; the number of
  lines that fit follows from the tile height at the current scale. If
  all fit, no paging. Otherwise pages of as many lines as fit, switching
  every 4 s, while the menu is shown (no repaint while covered); a page
  indicator is optional.
- Tile title: none, or a small "Batteries" only if it fits without
  costing a device line (implementer's choice, record it).
- **Click feedback** (owner, 2026-11-03): from the click until the read job
  ends, the tile shows `Refreshing…` visibly (in place of the device lines,
  or as their first line with the device lines dimmed — implementer's
  choice, record it), so a click is never without a visible answer, even
  when the job takes a few seconds (an idle keyboard answers after up to
  ~0.85 s, an unreachable device costs up to 2 s). The tile takes no
  second click while it shows this. Paging pauses while refreshing.
- The click (read now + log) and the log lines stay; the log line starts
  with the kind label too: `Keyboard (MX Mechanical): 100 %, discharging, read 13:47`.

## Tests

- Python: kind from the pairing-info byte (1, 2, other), kind field
  written, kind carried for an unreachable device (cache and previous
  file), 5-field round trip.
- Rust: parse 5 and 4 fields; labels incl. duplicates; line warn/muted;
  paging (which lines on which page, page switch by elapsed time with an
  injected clock); all-fit means no paging.
