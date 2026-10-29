# ThinFloe menu: size by screen density, not by screen width

Date: 2026-10-29. Branch `boot-handover`. Owner's hardware feedback of
2026-10-28 (the "menu-look points"), worked out in a brainstorming session
with mockups (`.superpowers/brainstorm/`, git-ignored; the decisions are
written out here).

## Why

The menu sizes its tile group as 58 % of the screen width
(`GROUP_SHARE` in `src/thinfloe-menu/src/ui.rs`), and caps a tile's height at
76 points. The session runs every output at scale 1, so a point is a pixel.
On 1920 px a tile is about 276 x 76; on a 4K screen it is about 554 x 76: the
tiles grow massively in width while the text stays the same pixel size. The
Settings screen uses the same 58 % rule. The labwc title bars and the
terminal have the same problem in other forms (the terminal has a stop-gap
rule, see below).

The owner wants the menu to take the real size of the screen into account:
"a 21" 4K display is different from a 43" 4K". A tile, a Settings row and the
text should look about the same size to the eye on every screen, and keep
their shape.

## Goals

1. Tiles keep one shape, 220 x 76 points (2.9 : 1, "shape B" in the
   mockups), on every screen.
2. One point is drawn about as large as one pixel of a 24" 1080p screen,
   whatever the screen. A larger screen of the same density shows more empty
   space around the menu; the tile group does not grow.
3. On a small, dense screen (owner's example: a 13" 8K laptop) the menu
   shrinks until the four tile columns fit with a reasonable gap at the left
   and right.
4. Settings, header, log, strip and dialogs follow the same scale as the
   tiles.
5. The terminal and the labwc title bars follow the same density, so all
   three agree.

## Non-goals

- No labwc output scale. Every output stays at scale 1 (owner's decision of
  2026-10-28, confirmed 2026-10-29): Xwayland windows on a scaled output are
  enlarged and the ThinLinc client loses pixels and sharpness. Per-monitor
  scale through labwc was offered and declined.
- The OxuLnk icon (head only, no black square) is changed in the
  `oxulnk-desktop` package by the owner, not in ThinFloe.
- No change to the tile set, the dialogs' wording, the log contents or the
  add-on logic.

## Anti-goals

- Tiles that change shape with the screen.
- A ThinLinc client that gets fewer pixels than the monitor has.
- A menu that is wider than its screen, or that draws edge to edge.

## The density rule

- **Scale** `s` (pixels per point) of the screen the menu is on:
  `s = ppi / 92`, where `ppi` is the screen's horizontal pixels divided by
  its physical width in inches. 92 ppi is a 24" 1080p screen, the screen the
  owner approved the look on.
  - Examples: 24" 1080p gives 1.0; 21" 4K about 2.3; 43" 4K about 1.1; 13"
    8K about 7.4 (before the fit below).
- **Physical size** comes from the output's `wl_output` geometry
  (`physical_width`, `physical_height` in mm, from the monitor's EDID), the
  same values `wlr-randr` prints as "Physical size".
- **Untrustworthy sizes** fall back. A size is not used when either side is
  0, when its aspect differs from the mode's aspect by more than 10 %
  (TVs and projectors report e.g. 160 x 90, the aspect ratio in cm), or when
  the resulting ppi is outside 50..800. The fallback is today's terminal
  rule: `s = 2` when the shorter side of the mode is 1800 px or more, else
  `s = 1`.
- **Never below 1 from density alone** (my call): a large, low-density
  screen (a 32" 1080p, 69 ppi) keeps today's 1080p look rather than
  shrinking; such screens are viewed from further away.
- **Rounded** to steps of 0.25 (my call), so text and icons land on a small
  set of sizes.
- **Fit:** after that, when the tile group (4 x 220 + 3 x 3 = 889 points)
  plus a gap of 5 % of the width at each side is wider than the menu
  window's width in pixels, `s` becomes the largest 0.25 step that fits, but
  never below 0.5 (my call). This is the only way `s` drops below 1.
  Header, Settings, log, strip, terminal and title bars all use this one
  fitted `s`. Only the width is fitted; on a short screen the tiles flatten
  to `TILE_H_MIN` and then the grid scrolls, as today.
- **Which screen:** the one the menu window is on. The grid is on one
  monitor (Known Limitations: "The grid is on one monitor"). The menu does
  not choose a monitor; it takes the one labwc put it on.
- **One computation** (my call): only the menu computes `s`. It writes it to
  `$XDG_RUNTIME_DIR/thinfloe-scale` (one number, written atomically), and
  the terminal and the title bar code read that file. With no file (the menu
  has not run yet, or failed) they use the fallback rule above. This keeps
  the 0.25 rounding and the fit in one place.
- **First frame** (my call): the boot picture stays until the menu draws, so
  the menu must not show a frame at a wrong scale. It learns its output and
  `s` before the first frame it presents; when its output is not known yet
  and there is exactly one output, it uses that one. If the output turns out
  different, it lays out again once.
- **Fractional scales** are fine: egui and the icon renderer take any
  pixels-per-point; icons are re-rendered at `render_side(s)`, never
  stretched. The builder may snap rect edges to whole pixels.

## Tiles

- 220 x 76 points each, 4 columns, 3-point gaps, the group centred
  horizontally in the space between header and log, as today. The group's
  width is fixed at 889 points; `GROUP_SHARE` and `GROUP_MIN` go away.
- The short-screen rule stays: a tile may lose height down to `TILE_H_MIN`
  (64) when the area is too short; it does not change width then (my call:
  a slightly flatter band on a very short screen is better than a scroll).
- Inside the tile, nothing changes: icon left, name in Geo Italic, comment,
  running edge, `x2` count.

## The blue edge (unchanged, confirmed)

A vertical ice-blue bar on the left of a tile means its program runs; on a
Settings row it means an update waits for the next boot. Hover only lightens
the background. The owner asked and confirmed these meanings stay.

## Settings screen

- The panel is as wide as the tile group (889 points) and centred; on a
  large screen it stays that width.
- Each add-on row (layout "D" in the mockups):
  - first line: the name in Geo, then the source address in the small grey
    monospace, cut with "…" when it does not fit;
  - second line: the status items separated by " · ", for example
    "4.21.0 · up to date", "1.4.2 installed · 1.5.0 ready for next boot",
    "2.0 · up to date · updates only by hand". "updates only by hand" is an
    item of this line, in its amber colour, not a line of its own. Each
    item keeps its colour (green when current, ice when an update waits).
  - the button (Install now / Update now) at the right end of the row,
    vertically centred. A row with no button reserves no space for one; the
    text then reaches to the right padding.
- The tick box, the icon, the top bar ("Add-ons", "checked … ago", Check
  now, Back) and the ice edge stay as they are.

## Header

- The logo sits 32 points from the top edge and 32 from the left (64 px on
  a screen with `s = 2`, which follows from the rule; no separate setting).
  The host name, version and clock on the right keep 32 points from the
  right edge and stay centred on the logo's height.
- New: "by oetiker.ai" in small text under the "ThinFloe" wordmark,
  left-aligned with it. My call: Geo, about 14 points, the muted grey of the
  host name. Header only; the boot screen does not get it.
- The header grows by what the larger top margin and the new line need; the
  builder sets the exact height.

## Folded-window strip

(All sizes in this section are points at the menu's `s`.)

When a window is folded, the strip appears at the bottom and takes its
height (strip plus its gap) from the log only. The header and the tile grid
do not move. If that would leave the log fewer than `LOG_MIN_ROWS` rows, the
log keeps its minimum and the rest is taken from the space between header and
log (my call).

## Terminal

`thinfloe-terminal` reads `s` from `$XDG_RUNTIME_DIR/thinfloe-scale`; without
the file it uses the fallback rule (today's rule: shorter side 1800 px or
more gives 2). Smalti is sharp only at whole multiples of 14 px, so the font
is `14 x round(s)` px, at least 14, and the padding `12 x round(s)`. So on a
21" 4K screen the menu draws at 2.25 and the terminal at 2: intended, the
font allows only whole steps.

## labwc title bars

labwc has one title bar look for all outputs. It follows the menu's `s`:
the whole bar, not only the text. The `<theme>` fonts (`ActiveWindow`,
`InactiveWindow`, and any other place set today) get `10 x s`, rounded to
whole points; the title bar's padding and button size (labwc theme keys such
as `padding.height`, `window.button.width`, set through a `themerc-override`
in the composed config dir) scale by the same `s`, so the bar and its
iconify, maximise and close buttons grow together. The builder checks in
labwc 0.9.3 which keys exist and what the font size unit is. The value is
set once per session, after the menu has written `thinfloe-scale`; a monitor
plugged in later does not change it. A short resize of title bars after the
session starts is fine: no program window is open yet. How the value
reaches labwc is the builder's call; constraints:

- `/etc/thinfloe/labwc/rc.xml` stays the system file (Standing Constraints);
  the session already composes a merged config dir
  (`thinfloe-compose-labwc-config`), which is the natural place for a
  generated `rc.xml`, applied with `labwc --reconfigure`.
- Zero keybindings stays: the generated file must keep the `<keyboard>`
  section exactly as the system file has it.
- When anything fails, the title bars keep size 10.

## Constraints that stay

- The menu draws on the CPU (no GPU crates).
- Icons render in the sandboxed child at the sizes the menu draws them at
  (`render_side(ppp)`); a new `s` must re-render them, not stretch them.
- Zero labwc keybindings; `rc.xml` outside `$HOME`.

## Tests and proof

- Unit tests for the rule: ppi to `s`, the 0.25 rounding, the floor of 1,
  the fit on a narrow dense screen, every fallback case (0 mm, aspect
  mismatch, ppi out of range), each with the examples above (24" 1080p,
  21" 4K, 43" 4K, 13" 8K, a 160 x 90 mm TV).
- Layout tests: tile size is 220 x 76 points (or flatter on a short screen)
  at every screen size; the group never exceeds the screen minus the side
  gaps; folding a window changes only the log rect when the log has room;
  the Settings panel width equals the group width; a row without a button
  has its text reaching the right padding.
- `test/integration/test-terminal.sh`: the new rule with stubbed
  `wlr-randr` output, including the fallback.
- A test for the title bar size path, and that the generated `rc.xml` keeps
  zero keybindings (`test-session-packages.sh` guards the system file
  today).
- Screenshots from the headless rig (podman, `WLR_BACKENDS=headless`,
  `wlr-randr --custom-mode`) at 1920x1080 and 3840x2160; the headless output
  reports no physical size, so this exercises the fallback.
- CHANGES.md entries for what users see; CLAUDE.md updated where it
  describes the layout, `thinfloe-terminal` and the title bars.

## Left to the builder

- How the menu learns its output's physical size and mode, and which output
  its window is on. winit 0.30 gives no physical size, so the menu binds
  `wl_output` itself (its own Wayland connection already exists in
  `src/windows.rs`) and can match winit's current monitor by output name
  (`wl_output` version 4 sends it).
- Whether the menu follows a change of monitor while running; at least it
  must use the right scale at start.
- Exact header height and the vertical placement of "by oetiker.ai".
- How the title bar size reaches labwc, within the constraints above.

Not verified on hardware until built: everything here.
