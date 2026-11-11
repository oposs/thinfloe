This note covers the menu's status area as built on the status-area branch (design of 2026-11-10): elements, placement, the layout file, the Status tab, the samplers and what each element shows.

The design is `docs/superpowers/specs/2026-11-10-status-area-design.md`. It
replaces the drawing, configuration and elements of
`docs/superpowers/specs/2026-11-03-status-area.md` (PR #6), which built SVG
elements rendered with resvg and placed by files in `/etc/thinfloe/status/`.
None of that is in the code any more.

## Area

- Below the tile grid and above the log, a grid of the tiles' cell size and
  gap (`ui::TILE_GAP`), 4 columns (`status::COLUMNS`), at most 3 rows
  (`status::ROWS`). A screen under `ui::SHORT_SCREEN_H` (900 pt) is short:
  the grid has 1 row there.
- It takes no input. It is not drawn when nothing is placed, and the menu
  then looks as it did before the area existed.
- Tiles and area are centred together between header and log,
  `ui::STATUS_GAP` (32 pt) apart. The placement uses `Area::planned_rows`,
  the rows the shown elements take when every one has content, so the tiles
  and the top of the area stay where they are when an element appears or
  goes. The area never takes more rows than planned (see Placement). Where
  the planned rows do not fit, the area is pinned `ui::GAP`
  (16 pt) above the log and only the tile grid scrolls, in the band above it
  (`ui::layout_with`). The cells take the tiles' height, 76 down to 64 pt.
- A fullscreen window (the ThinLinc or OxuLnk client) covers the menu and so
  the area. Covered means winit names no monitor for the window any more
  (`is_covered` in `app.rs`), because winit's Wayland backend never sends
  `WindowEvent::Occluded`. Uncovering sends no event either, so a covered
  menu looks again once a second (`COVERED_LOOK`) and draws nothing unless a
  frame is due. `test/runtime/test-labwc-runtime.sh` covers the menu with a
  fullscreen foot and requires the `thinfloe-menu: covered` and `uncovered`
  lines under `TF_MENU_DEBUG`.

## Elements

- An element is a unit struct implementing `status::Element`
  (`src/thinfloe-menu/src/status/mod.rs`): `id`, `name`, `sizes` (the first
  is the default), `short_size`, `sources`, `has_content`, `draw`,
  `next_change` and `refresh`. `status::ELEMENTS` lists them; its order is
  the default order. The comment at the top of `status/mod.rs` lists the
  steps to add one, including a new `Source`.
- `draw` is pure (same data and time, same picture) and draws only inside
  its rectangle; `testing::assert_inside` checks every offered size at both
  cell heights. Colours come from the theme (`brand::role`), sizes from
  `brand::STATUS_LARGE` (76 pt cells) and `brand::STATUS_SMALL` (64 pt),
  interpolated by `status::sizes_for` on half points.
  `ui_code_has_no_style_literals` covers `status/`.
- Labels are in Geo; values, axis ticks and units in the monospace font.

Shipped elements:

| id | name | sizes (first = default) | short size | source | refresh |
|---|---|---|---|---|---|
| `battery` | Keyboard and mouse batteries | 1x1, 2x1 | 1x1 | Battery | on change |
| `network` | Network traffic | 2x2, 3x1, 2x1 | 3x1 | Network | 1 s |
| `network-info` | Network details | 1x2, 2x1 | none | Network | 1 s |
| `temperatures` | Temperatures | 1x1, 1x2 | none | Hardware | 5 s |
| `cpu` | CPU | 1x1 | none | Hardware | 5 s |
| `memory` | Memory | 1x1 | none | Hardware | 5 s |
| `system` | Fans, uptime, model | 1x1, 2x1 | none | Hardware | 5 s |
| `clock` | Clock | 1x1 | none | none | next minute |

## Placement

- `status::layout::pack`: each shown element, in the user's order, goes to
  the first free cell, row by row and left to right, where its size fits.
  One that fits nowhere is left out. On a short screen an element takes its
  `short_size()`, or is left out when it has none.
- An element whose `has_content` is false (no battery device, no sensor, no
  network address) takes no cells, so the elements after it move up into
  its place.
- The elements with content are packed into the planned rows only
  (`Area::shown_where`, `pack`'s `max_rows`). First fit over fewer elements
  can need more rows than over all of them (clock, battery, network 2x2,
  network-info 2x1 with no battery: 2 planned, 3 needed), and an unreserved
  row would cover the log. An element that would need one is left out
  ("no room"). So an element gaining content mid-session (a Bolt keyboard
  reconnecting) can push a later one out until it has none again.
  `shown_rows_never_exceed_planned_rows_for_any_content` tries every
  content subset of 302 layouts.
- The default list packs to the layout of PR #6: battery 0,0; network 2x2
  at 1,0; network-info 1x2 at 3,0; temperatures 0,1; cpu 0,2; memory 1,2;
  system 2,2; clock 3,2 (`the_default_list_packs_to_the_pr6_layout`). On a
  short screen: battery and the graph at 3x1.
- The menu asks for a frame at the soonest `refresh` or `next_change` of the
  elements drawn (`Area::wake`).

## Layout file

- `~/.config/thinfloe/status` (`status::FILE`; `TF_STATUS_FILE` moves it for
  tests). One element per line, `<id> <w>x<h>`, or `-<id> <w>x<h>` when
  hidden; `#` starts a comment anywhere on a line. The menu writes a header
  comment and every element it knows, shown or hidden, so an unticked
  element keeps its place and size.
- Missing, or only comments: the shipped list.
- Over 4 KiB, over 64 lines or not UTF-8: the shipped list, one note, and
  the file stays as it is until the user changes something in the Status
  tab. Reading stops one byte past 4 KiB.
- Not a regular file (a FIFO, a device, a directory; a symlink counts by
  its target), or unreadable: the same, with the note "it cannot be read".
  The file is read through `bounded::read_bounded`, which looks at the type
  before it opens anything: opening a FIFO blocked the menu's main thread,
  at start, on every watch wake and in `--check`.
- A line that is not `<id> <w>x<h>`: skipped, one note. A second line for an
  id: skipped, one note.
- An unknown id (a newer image wrote the file): kept, not drawn, written
  back at its place, one note.
- A size the element does not offer: its default size, one note.
- An element the file does not name (a newer image added it): appended,
  shown, at its default size. The packing places it only where room is
  left.
- Each note opens the menu's log as a warning once; the same file read
  again logs nothing new, nor does the menu's rewrite of a hand-edited file,
  which moves the lines (notes are compared without their line number,
  `layout::note_key`). `thinfloe-menu --check` prints `status: layout file
  <path>` (with `(missing: the shipped layout)` when there is none) and each
  note; a layout problem never fails `--check`.
- Read at start and when the menu's once-a-second file watch sees it
  change. Written only by the menu: `<file>.new` and a rename
  (`jobs::status_save`); a change while a write runs follows it, the last one
  wins. A re-read while a write runs is ignored.
- It travels with the home archive; there is no exclude entry.

## Status tab

- `SettingsTab::Status`, after Screen. One row per element this image knows,
  in the file's order: a tick box, a size select offering only the
  element's `sizes()`, and Up and Down buttons. Unknown ids get no row.
- A ticked element that is not drawn shows a muted note (`statustab::rows`),
  decided in this order: "not on a small screen" when the screen is short
  and the element has no short size; "no room" when the packing of every
  ticked element (`Area::planned`) gives it no cells, or it has content and
  the row limit of the shown packing leaves it out; "nothing to show on this
  machine" only when its sources have read at least once
  (`Data::has_read`: the battery file was read, the sampler took a
  reading; `Samplers::copies` passes no copy before the first one).
  An element whose source has not read yet gets no note, so the hardware
  elements on a short screen, where the sampler rests, never say "nothing".
- Every change writes the file at once, with no Apply. A failed write shows
  in the tab in the warning colour and in the log, and the screen keeps the
  change.
- "Reset to default" removes the file.
- Below the rows a preview draws the status area with the main screen's
  code at the main screen's cell size, for the current screen (short or
  not), scaled down only when the tab is narrower than the area. Code:
  `statustab.rs` (rows, notes, events), `statustab_ui.rs` (drawing),
  `Menu::status_event` in `menu.rs`.

## Samplers

- `Samplers` in `app.rs`. A sampler starts the first time a ticked element
  needs its source (`Area::needs`, ticked whether placed or not, since an
  element without content needs data to gain some) and is never started
  again; afterwards an `on` flag says whether it reads.
- Network (`netstat.rs`, every 1 s): reads while a ticked element needs it
  and no window covers the menu. No subprocess: `/proc/net/route` (lowest
  metric), `getifaddrs`, `statistics/{rx,tx}_bytes`, `speed` (`-1` is
  none), `/proc/net/wireless` or a `wireless/` directory,
  `/run/systemd/resolve/resolv.conf` (else `/etc/resolv.conf`), and `wg0`'s
  `flags` (`IFF_UP`; its operstate is `unknown` while up). 300 samples per
  direction; a rate is the byte delta over the real elapsed time. A counter
  that goes back or a new interface starts the graph again with no spike,
  and the total keeps counting. A reading more than 3 s after the last (the
  sampler rested while covered) clears the graph and pushes no rate; its
  bytes still count in the total. Without a default route the first
  interface with an address is measured (`no default route`); with no
  address but loopback and link-local the network elements have no content.
  It stays separate from the Network tab's `sudo -n thinfloe-network status`,
  which runs as root only while that tab is open.
- Hardware (`hardware.rs`, every 5 s): reads while no window covers the
  menu and a hardware element has cells on this screen
  (`hardware_wanted`), so it rests on a short screen. It reads `/sys` and
  `/proc`, no root and no subprocess, bounded and treated as hostile.
- Battery: the menu reads `/run/thinfloe/peripherals/battery` at start and
  on every change seen by its once-a-second file watch (`battery.rs`). See
  [peripheral-battery.md](peripheral-battery.md) for the reader.

## What each element shows

- Battery: one line per device: the kind in Geo (`Keyboard`, `Mouse`,
  `Numpad`, `Presenter`, `Trackball`, `Touchpad`; the name when the kind is
  unknown; `Keyboard 1`, `Keyboard 2` in file order when two share one), a
  battery symbol filled to the level, and the percent right-aligned (`95 %`,
  `–` when not reachable or unknown), `+` while charging in a slot of its
  own. Known kinds first. A reachable device below 10 %, at state
  `critical`, or at state `low` with no percent is in the alert colour; a
  not reachable one is muted. A label is clipped where the symbol column
  starts; a numbered label keeps its number and the name is clipped before
  it. Lines that do not fit turn page every 4 s (`battery::PAGE_EVERY`),
  counted from the last reading, with no page indicator.
- Network: the received (filled) and sent (line) rate of the measured
  interface over the last 5 minutes, a rate axis of 0 and up to three steps
  of 1, 2 or 5 times a power of ten at or above the peak (`rate_ticks`; at
  least 1 kbit/s; labels such as `500`, `1.5k`, `15M`; unit `bit/s` above),
  a time axis `-5m` to `now`, light grid lines and a legend `↓ rx ↑ tx`. In
  one row (3x1, 2x1) it is the graph alone edge to edge, with the rate
  labels (none for 0), unit and legend inside the plot on a backing and no
  time labels.
- Network details: one fact per row, label in Geo, value from one column:
  `Down`, `Up`, `Peak`, `Total` (since the click on the oldest running
  Save-On-Exit tile, ThinLinc or OxuLnk, else since the menu start; when the
  oldest ends, the next one's own start counts), `Since`, `Link`
  (`eth0 1 Gbit/s`, `wlan0 WiFi -56 dBm`), `IPv4`, `Gateway`, `DNS` (each
  further server a row without a label), `VPN` (`wg0 up`/`down`, `no VPN`).
  It has content when the graph has. Rows move closer down to a minimum,
  then rows are left out: further DNS servers (the last first), the peak,
  the since, the total, DNS, the gateway, the link; rates, address and VPN
  stay. A `Link` row too wide leaves out the word `WiFi`; a value still too
  long is cut at the padding. On the tightest rows the Geo labels are half a
  point smaller (`label_in_row`).
- Clock: `Time 14:05` and `Date Thu 5 Nov 2026` in the machine's zone, with
  English names whatever the locale; drawn again when the minute turns. It
  always has content. It has no short size; the header shows the time on a
  short screen. Wall time and zone come through `Data`, read once per frame
  by the app, so a DHCP zone that arrives later shows at the next minute.
- Hardware: four elements fed by one sampler, nothing pages. A label in Geo
  on the left, the value on the right; a value the machine does not provide
  drops out with no placeholder, and an element with nothing is not drawn.
  `temperatures`: the sensors that fit; those left out are the least
  important (the first CPU sensor stays, then hot ones before normal ones,
  then the order of `sensor_name`: CPU, GPU, Disk, RAM, WiFi, LAN, Chipset,
  Board, others), and those kept stay in that order. `cpu`: `CPU load`, `CPU
  clock`. `memory`: `RAM`, `zram` (`3.1 / 8.0 GiB`). `system`: the fans on
  one line (those that do not fit beside `Fans` with `rpm` are left out, the
  first ones stay; a fan at 0 rpm drops out), `Up 3 d 4 h`, `Model
  NUC8i3BEK` (`Maker` when the machine names no model), cut at its end when
  too long.
- Temperature colours (`hardware::thresholds`): critical 5 C below `crit`;
  hot at `max` when that is below critical, else 15 C below `crit`; with
  `max` alone, critical 15 C above it; with no limit 80 and 95 C. Limits
  outside 40 to 150 C count as none. Hot draws the line in the warning
  colour, critical in the alert colour.

## Tests

`status/` unit tests (packing, layout file, each element inside its rect at
every size), the Status tab tests in `menu.rs` and `ui.rs`, the sampler
tests in `netstat.rs` and `hardware.rs`, the covered test in
`test/runtime/test-labwc-runtime.sh`, and in
`test/integration/test-session-packages.sh` the absence of
`/etc/thinfloe/status/`. The ignored test that writes PPM pictures per
element and size is for PR reviews.

## Not verified on hardware

- The status area as a whole on real screens: its place, the fonts, the
  hardware elements and the network list, the battery symbol, the alert
  colour (no device was below 10 %) and pages (only two devices).
- The network sampler on real interfaces.
- Hardware sensors: the names and labels are from memory (the development
  host is a VM with no hwmon); `k10temp`'s `tempN_max`, if it reports 70,
  would warn at 70 C; reading sensors every 5 s may wake a suspended
  discrete GPU (unmeasured).
- The Status tab with a real mouse.
