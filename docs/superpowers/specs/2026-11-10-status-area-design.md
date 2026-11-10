# Status area: elements in Rust, drawn with egui, chosen in Settings

Date: 2026-11-10. Replaces the drawing, configuration and element parts of
`2026-11-03-status-area.md` (PR #6, head `89e107a`, rebased locally as
`9626a21` on branch `peripheral-battery`). The data side of PR #6 stays.

## Intent

The menu shows a status area below the tiles: batteries of keyboards and
mice, network traffic, hardware sensors, a clock. The owner wants two things
from it (2026-11-10):

1. **Adding a status element is simple.** A contributor writes one Rust file
   and sends a PR to this repository. No plugin system, no scripting, no
   templates: nobody will write elements outside the repository.
2. **The user chooses the status area interactively**: which elements show,
   at which size, in which order, from the Settings screen.

PR #6 built fixed elements that draw SVG text rendered with resvg, placed by
shipped files in `/etc/thinfloe/status/`. That design makes a new element
cost an SVG builder, a config file and a parser entry; it bypasses the theme
(the palette was made `pub(crate)` so SVG builders could name colours), and
it has a config format nobody can change. Drawing with egui removes all
three.

## Goals

- A new element is one module in `src/thinfloe-menu/src/status/` plus one
  line in `status::ELEMENTS`.
- Elements draw with egui, with colours from `theme()` and text sizes from
  named constants in `brand.rs`, like every other screen of the menu.
  `ui_code_has_no_style_literals` covers `status/` too.
- Settings has a tab **Status** (after Screen) where the user ticks
  elements, picks each one's size from the sizes it offers, and orders them.
  The result is visible at once, in a live preview and in the real area.
- The choice is per machine, stored in the home, carried by the home
  archive.
- Everything PR #6 shows today keeps working and looks equivalent (same
  facts, same colour rules, same hide rules), drawn by egui.

## Non-goals

- Plugins loaded at run time, scripting (Lua or other), SVG templates.
- Free placement (drag and drop, a chosen cell). Placement is automatic.
- Clicks on elements. The status area takes no input.
- Changing what the battery reader, its units and its file do.
- Sharing the Network tab's data source (see Data sources).

## Anti-goals

- A status element that can stall, crash or grow the menu: sampling stays in
  bounded background threads, drawing is pure.
- Two elements overlapping, or an element drawn outside the area.
- A layout file that can break the menu: any file the menu cannot read gives
  the shipped layout and one log line.
- The tiles move when the status area changes.

## Constraints (carried from main and PR #6)

- The menu draws on the CPU (no GPU libraries); egui only.
- Tile cell 220 x 76 pt, `GROUP_W` 889, 4 columns; the status area uses the
  same cell and gap, at most 3 rows.
- Short screen: window height under `SHORT_SCREEN_H` (900 pt).
- The status area is hidden while a fullscreen window covers the menu, and
  samplers that need the screen (hardware) rest then (PR #6's `covered`
  logic, `frame_due`, `is_covered`, `hardware_on`).
- Zero keybindings, lyra style, nothing rounded.

## Design

### Element trait

`status/mod.rs` defines (names are the builder's to refine):

```rust
pub struct Size { pub w: u8, pub h: u8 }      // cells, 1..=4 x 1..=3

pub enum Source { Battery, Network, Hardware } // shared samplers

pub trait Element: Sync {
    fn id(&self) -> &'static str;              // stored in the layout file
    fn name(&self) -> &'static str;            // shown in Settings
    fn sizes(&self) -> &'static [Size];        // offered; the first is the default
    fn short_size(&self) -> Option<Size>;      // its 1-row size on a short screen
    fn sources(&self) -> &'static [Source];
    fn has_content(&self, data: &Data) -> bool;
    fn draw(&self, ui: &mut egui::Ui, rect: egui::Rect, size: Size,
            data: &Data, now: Instant);
    fn next_change(&self, data: &Data, now: Instant) -> Option<Instant>;
}

pub static ELEMENTS: &[&dyn Element] = &[ /* ... */ ];
```

- `Data` holds the latest snapshot of every running source (`Option` each)
  and the clock's time zone. Elements read it, never write it.
- `draw` is pure: same inputs, same picture. Paging (battery lines that do
  not fit) derives the page from `now`.
- `next_change` says when the element's picture changes on its own (the next
  minute for the clock, the next page for the battery). Sampler wakes
  request frames as in PR #6.
- `has_content` false (no battery device, no sensor, no network) means the
  element is left out of the packing on the main screen.

The doc comment at the top of `status/mod.rs` lists the steps to add an
element, including adding a `Source` when the element needs new data.

### Elements shipped

The eight of PR #6, same content and colour rules:

| id | name | sizes (first = default) | short size | source |
|---|---|---|---|---|
| `battery` | Keyboard and mouse batteries | 1x1, 2x1 | 1x1 | Battery |
| `network` | Network traffic | 2x2, 3x1, 2x1 | 3x1 | Network |
| `network-info` | Network details | 1x2, 2x1 | none | Network |
| `temperatures` | Temperatures | 1x1, 1x2 | none | Hardware |
| `cpu` | CPU | 1x1 | none | Hardware |
| `memory` | Memory | 1x1 | none | Hardware |
| `system` | Fans, uptime, model | 1x1, 2x1 | none | Hardware |
| `clock` | Clock | 1x1 | none | none |

Sizes beyond the PR's own are the builder's call per element (an element
offers only sizes it draws well; a size it cannot fill is not offered). The
short sizes reproduce PR #6: on a short screen battery and the graph (3x1,
legend inside) fill the one row.

### Placement

Packing, in `status/layout.rs`, pure and unit-tested:

- Input: the ordered list of `(element, size)` the user chose, the elements
  that have content, short screen or not.
- On a short screen the grid has 1 row; an element whose chosen size is
  taller uses its `short_size`, or is left out when it has none.
- Each element, in order, goes to the first free cell (row by row, left to
  right) where its size fits. One that fits nowhere is left out ("no room").
- Only the rows used are drawn; with nothing placed the area is gone and the
  menu looks as before the feature. The tiles do not move.

The shipped default list packs to exactly PR #6's layout: battery (0,0),
network 2x2 (1,0), network-info 1x2 (3,0), temperatures (0,1), cpu (0,2),
memory (1,2), system (2,2), clock (3,2). A unit test asserts this.

### Layout file

`~/.config/thinfloe/status`, plain text, one element per line in order:
`<id> <w>x<h>` for a shown element, `-<id> <w>x<h>` for a hidden one, `#`
comments allowed. The menu always writes every element it knows, shown or
hidden, so an unticked element keeps its place and size across restarts and
re-ticking puts it back where it was. Written atomically by the menu only
(temp file + rename in the same directory, like the other menu files).

- Missing file, or empty (comments only): the shipped default list.
- An element of `ELEMENTS` the file does not name (a newer image added it):
  appended at the end, shown, at its default size. The packing places it
  only where room is left, so it never pushes an element the user chose out.
  (Architect's call, 2026-11-10: new elements announce themselves; one
  untick hides them for good.)
- A line with an unknown id (an older image reads a newer file): skipped,
  one log line; the menu keeps the line when it rewrites the file, at the
  same place, so a newer image finds it again.
- A size the element does not offer: the element's default size, one log
  line.
- A second line for the same id: ignored, one log line.
- A file that is not UTF-8, over 4 KiB or over 64 lines: the default list,
  one log line. The menu does not rewrite it until the user changes
  something in the Status tab.
- Read at start and again when it changes (the menu's existing file watch).
- Travels with the home archive like the tile files; no exclude entry.

### The Status tab

`SettingsTab::Status`, after Screen. Top: the list; below: the preview.

```
 [x] Keyboard and mouse batteries   [1x1 v]   ^ v
 [x] Network traffic                [2x2 v]   ^ v
 [ ] Clock                          [1x1 v]   ^ v
                                        Reset to default
 Preview
 +--------+-----------------+--------+
 |Battery |  Network graph  |Details |
 ...
```

- One row per element in `ELEMENTS`, in the file's order; unticked elements
  keep their place and size (the file stores them as `-<id>` lines). The
  menu's own write fires the file watch; the re-read gives the same list, so
  rows do not jump.
- Tick box: show or hide. Size select: only that element's `sizes()`. Up and
  down buttons move the row.
- A ticked element that the packing leaves out shows a muted note in its
  row: "no room" or "nothing to show on this machine".
- Every change writes the file at once; no Apply, no confirm step (a status
  layout cannot lock anyone out). A write that fails shows the error in the
  tab in the warning colour and in the log, and the on-screen state stays.
- "Reset to default" removes the file (the default list counts again).
- The preview is the real status area drawn at the real cell size by the
  same code, for the current screen (short or not), scaled down only if the
  tab is narrower than `GROUP_W`.

### Data sources

Kept from PR #6, unchanged in behaviour:

- **Battery**: `/usr/libexec/thinfloe/peripheral-battery` (root), its
  service, timer, udev rule, watcher and tmpfiles entry write
  `/run/thinfloe/peripherals/battery`; the menu parses it (`battery.rs`) and
  watches it.
- **Network**: `netstat.rs`, a sampler thread at 1 s reading `/sys`, `/proc`
  and resolved's `resolv.conf` without root. It stays separate from the
  Network tab's `sudo -n thinfloe-network status`, which runs as root only
  while that tab is open and does not report gateway or DNS; polling it every
  second for the status area would run sudo forever.
- **Hardware**: `hardware.rs`, a sampler at 5 s, resting while covered or
  when no shown element needs it.

A source runs only while some ticked element needs it (ticked, not placed:
an element without content yet must still get its data to gain some). A
tick in the Status tab starts the sampler, the last untick stops it.

### Removed from PR #6

- `status/render.rs`, `status/style.rs`, the SVG builders in the element
  modules, `status/config.rs`, `overlay/etc/thinfloe/status/`, resvg's
  `text` feature, the `pub(crate)` colours in `brand.rs`.
- The "refused status files" log path and its `--check` output; `--check`
  reports the layout file instead (which line was skipped and why).

### Tests

- Packing: default layout, short screen, no room, hidden elements, order.
- Layout file: every case in "Layout file".
- Status tab: state-machine tests in `menu.rs` for tick, size, order,
  reset, write failure; UI tests in `ui.rs` that the rows and preview draw.
- Each element: a test draws it at each offered size with sample data and
  asserts nothing is drawn outside its rect; the ignored shot test writes a
  PPM per element and size (for PR reviews).
- `ui_code_has_no_style_literals` includes `status/`.
- Kept: battery reader tests (`test/unit/test_peripheral_battery.py`),
  sampler tests, the covered test in `test/runtime/test-labwc-runtime.sh`,
  `test-session-packages.sh` checks for the units.

## Delivery

- Branch `status-area` from main `c449210`. Bring in from `9626a21` what
  stays (data side, redraw loop, grid geometry, units, tests), rewrite the
  rest.
- Commits by topic: battery reader and units; samplers; status framework and
  packing; elements; Status tab; unrelated clean-ups of PR #6
  (`scripts/lib/common.sh` helpers, the `dialog` package, `.dockerignore`
  entries, `vendor/.gitignore`) in one commit of their own.
- CLAUDE.md: about 30 lines for the status area (the trait, the file, the
  reader and its file format, the limits). Battery protocol details live in
  the reader's comments.
- CHANGES.md: user-facing entries for the area and the Status tab.
- When merged, PR #6 is closed with a pointer to the merge.

## Left to the builder

- Exact trait and type names, how `Data` is shared with samplers.
- Extra sizes per element beyond the defaults above.
- Text and colour constants needed in `brand.rs` (named, not literals).
- Whether the preview reuses the main-screen area widget directly or draws
  through a shared function.
