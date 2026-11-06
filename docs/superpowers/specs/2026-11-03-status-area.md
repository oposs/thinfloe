# Status area below the tiles

Owner, 2026-11-03, after the battery tile (PR #5) worked on hardware:

> "It should not be shown as a tile; below the tiles an area should appear
> with the same division as the tiles now, 4 wide 3 high. The whole thing
> rendered with resvg. The areas showing the single elements should be able
> to use several cells. First I want the battery level in the top-left cell,
> rendered as SVG, without a refresh option."

## Decisions

- A **status area** below the tile grid, with the same cell grid as the tiles:
  4 columns × 3 rows, same cell size and gaps as a tile (`GROUP_W` and the
  tile size at the current scale).
- Each **element** occupies a rectangle of cells (column, row, width, height;
  1×1 up to 4×3). Elements do not overlap.
- Elements are drawn as **SVG rendered with resvg** (the image already uses
  resvg for icons). The menu builds the SVG text itself from data (trusted,
  but names from devices are escaped for XML).
- **First element: battery**, top-left cell (column 0, row 0, 1×1): one line
  per device by kind (`Keyboard 100 %`, `Mouse 95 % +`), red below 10 % or
  `critical`, `–` when unknown/not reachable — same data and rules as the
  battery tile today (battery.rs). **No click, no `Refreshing…`, no read job
  from the menu.** Paging when lines do not fit may stay (decide in design).
- The battery **tile goes away** (70-battery.desktop, X-TF-Action=battery,
  the click job). The reader, boot read, 5-min timer and the on-connect
  watcher stay unchanged.

## Answers (owner, 2026-11-03)

- **Height:** at most 3 rows. An element moves up until a shown element above
  it in its columns, or the top, stops it (owner, 2026-11-04), and the area has
  the rows the elements then use, so none stays blank: with no battery device
  the temperatures take the top cell and the CPU the one under it; with nothing
  above them the hardware tiles are one row. The tiles do not move.
- **Position:** between the tile grid and the log; the log stays as it is.
  When space is short the tiles move up (as the layout does today).
- **Empty:** when no element has anything to show (e.g. no battery device),
  the area is not drawn at all; the menu looks as before the feature.
- **Rendering:** the menu builds the SVG itself (device names escaped for
  XML) and renders it **in-process** with resvg, using the fonts the menu
  already ships (Geo for labels, the proportional sans for numbers) loaded
  into resvg's font database. User icons keep their sandboxed child process.
- PR #5 was closed by the owner; a new PR only on the owner's go-ahead.

## Next element (idea, owner 2026-11-03, not designed yet)

"A second element could be the network monitoring usage during the sessions
with some statistics and graphs." To decide in its own design step:

- What is measured: throughput in/out of the default-route interface
  (`/sys/class/net/<if>/statistics/{rx,tx}_bytes`), or the ThinLinc/OxuLnk
  session's traffic only; latency to the session server; link speed, WiFi
  signal.
- Statistics: current rate, peak, total since the session started (a tile
  program's start/exit in the menu marks a session), and over what window.
- Graph: e.g. a sparkline of the last N minutes, drawn in the same generated
  SVG; sampling interval and memory bound.
- Size: 2 or 3 cells (owner, 2026-11-03), e.g. 2×1 or 3×1 to the right of
  the battery (col 1, row 0), or 2×1 below it.
- Layout (owner, 2026-11-03, confirmed): the 2 or 3 cells are mainly for the
  **graph** — a curve of received and sent traffic over the last minutes,
  spanning the element's width — with the numbers small at the side or on
  one line (current rate, peak, total since the session started).

## Third element (idea, owner 2026-11-03, not designed yet)

"In one [cell] could be shown the IP configuration and if there is a VPN
online." One cell. To decide in its design step:

- IP configuration: interface of the default route, IPv4 (and IPv6?)
  address/prefix, gateway, DNS server(s), maybe WiFi SSID; source: netlink /
  `ip -j addr`/`ip -j route`, `resolvectl`, or systemd-networkd state.
- VPN online: ThinFloe already supports WireGuard (`~/wg0.conf`,
  `wg-quick@wg0` started by the initramfs' network install). "Online" could be
  interface up, or a recent handshake (`wg show wg0 latest-handshakes`, needs
  root → a root helper writes it to /run like the battery file).
- Refresh: on change (netlink) or every few seconds.

## Element model

Owner, 2026-11-03: each element is rendered on its own and has its own
refresh; the area must take later elements (a network traffic graph over 2
or 3 cells, refreshing about every second; an IP/VPN cell, on change or
every few seconds) without a redesign. As built
(`src/thinfloe-menu/src/status/`):

- **Element interface** (`status::Element`): `visible(data)` (anything to
  show?), `svg(data, size, now)` (the picture now, built from the element's
  data for its rectangle in points), `next_change(data, size, now)` (when the
  picture changes on its own, e.g. the battery's page turn; `None` when only
  new data changes it). `data` is `status::Sources`, the menu's data sources
  (the battery devices and when they were read, the network snapshot and where its session total counts from).
- **Types** are registered in one place, `status::KINDS`: a `Kind` names the
  type (`Type=` value), lists the options its file may set and builds the
  element from the file. A new type is one module plus one line there plus a
  file in `/etc/thinfloe/status/`. `battery` and `network` exist.
- **Refresh.** The file's `Refresh=` is `change` (drawn again when the data
  source changes; for the battery, the 1 s watcher of the battery file) or an
  interval in seconds. The menu asks for its next frame at the soonest of
  every shown element's interval and `next_change`, and asks nothing while a
  window covers the menu. Covered means winit names no monitor for the window
  any more, after it had named one (`is_covered` in `app.rs`): winit's Wayland
  backend never sends `WindowEvent::Occluded`. Uncovering sends no event, so
  while covered the loop wakes once a second to look and draws nothing, except
  a frame that is due (the save timer and the pause before logout advance
  through those) or one a wake asks for: program output wakes the loop, so a
  fullscreen client that writes a line a second keeps the 1 s render going
  behind the cover. The watcher of the add-on and battery files rests while
  covered and catches up within about 2 s of uncovering.
- **One texture per element.** Each element's SVG is rendered in-process with
  resvg (menu fonts in its fontdb) into its own egui texture, cached under
  the element's file name and keyed on the SVG text (content and page) and
  the element's size in physical pixels (cell size in points times
  pixels per point). Either changing renders it again: the scale settles
  after the first frame and is checked again for 10 s, and the tile height
  moves between 64 and 76 pt with the screen, so a picture is never
  stretched to a size it was not rendered for. The same text and size reuse
  the texture. A broken SVG draws nothing and is not retried until it
  changes.
- **Damage.** A frame whose only texture uploads are status-element textures
  redraws only the rectangles of the meshes that draw them (the elements'
  cells); any other upload (the font atlas) still redraws the whole canvas.
  This keeps a 1 s graph from re-rastering the screen every second.
- **Space.** The status area sits between the tile grid and the log; its
  rows have the tiles' height and shrink with them (64 to 76 pt). It is
  pinned: when space runs short, it stays fully visible above the log and
  only the tile grid scrolls, in the band above it.
- **No tile jump** (owner; changed 2026-11-04). Tiles and area are centred
  together between header and log, `STATUS_GAP` apart, computed from the rows
  of the configured elements (`Area::planned_rows`; on a short screen their
  short forms), not from those shown: when an element appears or goes the grid
  and the top of the area keep their position. A menu with no status file has
  the tiles centred alone. Where the planned rows do not fit, the grid starts
  `GAP` under the header and the area is pinned `GAP` above the log. The grid
  moves only on a window size or scale change. Before this the tiles were
  centred as if the area did not exist, and three rows would have run into the
  second tile row at 1080p.

## Configuration

Owner, 2026-11-03: the elements come from configuration, one file per
element, not from code; for now only shipped files.

- **Files.** `/etc/thinfloe/status/*.conf` (in the overlay, outside `$HOME`
  like the tiles), one element each; the file name orders and names it
  (`10-battery.conf`, later `20-network.conf`, `30-ip.conf`). Adding or
  removing an element is adding or removing its file. Format, read by the
  tiles' bounded reader and desktop-entry parser (at most 16 files):

  ```
  [Status Element]
  Type=battery
  Cell=0,0
  Size=1x1
  Refresh=change
  PageEvery=4
  ```

  `Type` and `Cell` (column,row of the top-left cell) are required; `Size`
  is `WIDTHxHEIGHT` in cells (default `1x1`); `Refresh` is `change`
  (default) or 1 to 3600 seconds (`5` or `5s`); further keys must be
  options of the type (`PageEvery`, 1 to 60 s, default 4, for the
  battery).
- **Validation.** A file with an unknown type, an unknown key, a rectangle
  outside the 4 x 3 grid or of no size, a bad value, or a syntax error is
  refused. On overlap or any conflict the file with the lower name wins and
  the other is refused. Each refusal is a warning in the menu's log naming
  the file, and both files on overlap
  (`status: 20-network.conf refused: overlaps 10-battery.conf`); `thinfloe-menu
  --check` prints them and fails, and lists each element
  (`status 10-battery.conf: battery at 0,0 size 1x1`). A directory the menu
  cannot use (not a directory, unreadable, too many files) fails `--check` too;
  a missing one means no elements and is fine. Nothing crashes the menu.
- **No user files yet** (owner). A possible later extension that keeps the
  format: `~/.config/thinfloe/status/*.conf` with tile-like limits, where a
  file named like a shipped one may change only `Cell`, `Size` and
  `Refresh`.
- **Later: external elements** (not built). A type `program` could name a
  program that writes an SVG (or data) to a file; the menu would render that
  SVG in the sandboxed renderer child it uses for user icons
  (`thinfloe-menu --render-icon`, rlimits, deadline, refusal of deep or
  filtered SVG), never in-process, since that SVG is not the menu's own.
- **Long names.** A battery label is clipped (`clipPath`) where the
  right-aligned percent column starts: a long name is cut off cleanly and
  never overlaps the number; no ellipsis, no width measuring.

## Network element (owner decisions, 2026-11-04)

- **Measures** the whole network card: rx/tx bytes of the interface that holds
  the default route (cable or WiFi), from `/sys/class/net/<if>/statistics`;
  no root. If the default route moves to another interface, follow it.
- **Graph:** received and sent rate over the last **5 minutes**, one sample
  **every second** (300 points per direction, bounded memory), drawn in the
  element's SVG across most of its width.
- **Numbers** (all wanted): current rate in/out; peak in the window; total
  since the session started (the start of the running ThinLinc/OxuLnk tile
  program as the menu knows it, else since the menu started — say which in
  the element); interface and link (e.g. `eth0 1 Gbit/s`, or WiFi with signal
  from `/proc/net/wireless`).
- **Basic network information** in the same element (this replaces the
  separate IP/VPN cell idea): IPv4 address/prefix, gateway, DNS server(s),
  VPN status (WireGuard `wg0`: up/down from `/sys/class/net/wg0`; a handshake
  check would need root and is out of scope for now — say "up", not "online").
- **Place:** 3×1 to the right of the battery: `Cell=1,0`, `Size=3x1`,
  `Refresh=1` (one shipped file, e.g. `/etc/thinfloe/status/20-network.conf`).
- Hidden (the element, not the area) when there is no default route and no
  interface with traffic — then the IP line says "no network" or the element
  is not visible; decide and record.

### As built (2026-11-04)

Choices made in the implementation (`src/thinfloe-menu/src/network.rs`,
`src/thinfloe-menu/src/status/network.rs`,
`overlay/etc/thinfloe/status/20-network.conf`):

- **Sampling.** A thread of the menu reads once a second, also while a
  window covers the menu, so the graph has no hole when it shows again;
  it reads outside its lock and wakes the event loop only when
  `Snapshot::visible` changes, since a hidden element asks for no frames and
  would otherwise appear up to a minute late. The redraw copies the snapshot
  (600 numbers) under the lock. Other frames come from `Refresh=1`, only while
  the menu shows. No subprocess anywhere.
- **Series.** 300 rates per direction in bit/s, the byte delta divided by
  the real elapsed time (a late sample is not a spike; a sample under
  200 ms after the last is skipped). A counter that goes back, or a new
  interface (the default route moved), starts the graph again; the total
  keeps counting across both.
- **Sources.** Default route from `/proc/net/route` (destination and mask
  0, up, lowest metric). Addresses through `getifaddrs` (libc). Link: a
  `/proc/net/wireless` line or a `wireless/` directory is WiFi with the
  level in dBm, else `speed` (`-1` or unreadable: none). DNS from
  `/run/systemd/resolve/resolv.conf` (the real servers), else
  `/etc/resolv.conf`, at most 3. VPN: `wg0` exists and its `flags` carry
  `IFF_UP` (operstate is `unknown` while up); missing `wg0` reads `no VPN`.
  An interface name from `/proc/net/route` is joined to a path only when
  it is 1 to 15 printable bytes without `/` and not `.`/`..`.
- **Session total.** `Running` records the click's `Instant`; the oldest
  running Save-On-Exit program is the session. Its base is the sampler's
  total in the first snapshot after the click (at most a second late),
  kept for each running one, so that when the oldest ends the next one's
  time and base take over together; the label is `total since HH:MM` (local clock of the click), else
  `total since menu start`.
- **Layout.** Graph left, about 284 pt of the 666 pt; two text columns of
  190 and 140 pt in the sans at 11 pt, rows of 12.5 pt: five lines fit
  76 pt, four 64 pt. Lines that do not fit are left out, DNS first, then
  the gateway (stable text over paging). Traffic column: `↓ rate` and
  `max`, `↑ rate` and `max`, `X in, Y out`, `total since ...`. Units:
  three digits with k/M/G, `bit/s` and `B`, decimal.
- **Arrows.** Ubuntu Light has no `↓`/`↑`. With egui's monospace Hack
  loaded behind the sans, resvg drew the whole line containing an arrow
  in Hack, so each arrow is its own `<text>` in Hack and the number
  follows in the sans.
- **Hidden** when there is no default route and no interface but loopback
  with an address that is not IPv6 link-local. Without a default route
  but with an address, the first such interface (by name) is measured and
  the gateway line says `no default route`.
- **Not verified on hardware.**

## Design revision (owner, 2026-11-04, after seeing the first network picture)

The 3x2 network element of this section and its two text lines are replaced by
the split in "Hardware element and network split" below.

- The graph takes the **full width** of the network element and is **more
  technical**: x and y axes with scale labels (y: rate ticks with units, e.g.
  0 / 6M / 12M; x: time, e.g. -5m … -1m now), light grid lines.
- The network element becomes **3×2** (`Cell=1,0`, `Size=3x2`): top row the
  graph, bottom row two text lines — rates/peak/total, and
  iface/link · IP/prefix · gw · DNS · VPN.
- **One design for all status elements**, rendered as SVG with resvg: labels
  and titles in **Geo** (as the tiles), all numbers and addresses in a
  **monospace** font (Hack, already in the menu) so columns line up; a fixed
  set of sizes (title, value, small/axis), shared colours, paddings and line
  heights, defined once and used by every element (the battery element is
  restyled to match).

### As built (2026-11-04)

Choices made in the implementation (`src/thinfloe-menu/src/status/style.rs`,
`status/battery.rs`, `status/network.rs`,
`overlay/etc/thinfloe/status/20-network.conf`). The layout of the first
network element above ("Layout", "Arrows") is replaced by this.

- **One design module**, `status/style.rs`. No element sets a font size or
  colour of its own. Fonts: Geo for titles and labels, Hack (egui's
  monospace, already in the fontdb) for every number, unit, address and
  interface name. A Hack width is counted (characters times its advance
  1233/2048 em); a Geo width is measured once by laying the word out with
  usvg and cached (at most 256 entries). Every run of a line is its own
  `<text>` at a computed x, so the fonts never mix inside one text.
- **Sizes follow the cell height**, not the menu scale (the picture is
  rendered at the element's size in pixels). `Style::for_cell(h)`: the set
  for 76 pt, the set for 64 pt, interpolated in between and rounded to half
  a point; outside 64 to 76 the nearest set. Elements get their room as
  `Room { size, cell }`, so a 3 x 2 element uses the sizes of the cells
  beside it.

  | at cell height | title (Geo) | label (Geo) | value (Hack) | small (Hack) | row | line | small line | pad x / y | gap |
  | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
  | 76 pt | 15 | 15 | 12 | 10 | 16 | 16 | 12 | 12 / 6 | 6 |
  | 64 pt | 13 | 13 | 11 | 9 | 14 | 14 | 11 | 10 / 5 | 5 |

  Labels are as large as titles: Geo's small letters are much smaller than
  Hack's, and at 13 pt they read smaller than the 12 pt numbers beside
  them (seen in the first pictures).
- **Colours** (all from `brand.rs`): background `TILE`, text the theme's
  foreground, labels `MUTED`, alert `RED`, warning `WARN`, VPN up `GOOD`,
  received `ICE`, sent `GOOD`, and two new ones for graphs: grid `GRID`
  `#1c2530`, axes and tick marks `AXIS` `#3a4654`.
- **Arrows** are drawn as vector marks (`style::draw_arrow`), a stem as tall
  as a capital with an open head, in the cell of one Hack character: Hack's
  own `↓`/`↑` are thin and only as tall as its small letters, and read as
  dots at 1080p. Each is a group labelled with the character it stands for.
- **Battery**: the kind in Geo at the title size, the percent in Hack at
  the value size, right-aligned, the `+` in a slot of its own after it, so
  all percents end at one edge; the label is clipped one gap before the
  widest percent (`100 %`). Four lines fit a 76 pt cell, three a 64 pt one,
  eight a 1 x 2 element; more turn page as before.
- **Network, 3 x 2** (666 x 155 pt at 76 pt cells). Top to bottom: a header
  line (`bit/s` over the rate axis at the left, the legend `↓ rx  ↑ tx` at
  the right, small Hack), the plot, the minute labels, then the two text
  lines at the bottom. The plot spans from the rate labels (a fixed column
  four characters wide, so it does not move when the scale changes) to the
  right padding; its edges sit on half points so one-point lines are sharp.
  - *Rate axis.* `style::rate_ticks(peak)`: 0 and up to three equal steps
    of 1, 2 or 5 times a power of ten, the last at or above the window's
    peak of both directions: two to four values. The top is at least
    1 kbit/s (0, 500, 1k) and at most 1 Tbit/s; NaN, infinite or negative
    peaks count as 0. Labels without unit, as short as they are exact
    (`500`, `1.5k`, `15M`, `1G`); the unit `bit/s` stands above them. The
    curves are scaled to the top tick, so the peak sits below the top.
  - *Time axis.* Ticks and labels `-5m`, `-4m`, ..., `-1m`, `now`, one a
    minute; `-5m` starts at the axis, `now` ends at the right edge. The
    plot is exactly five minutes wide (300 steps; the oldest of 300 samples
    sits one step right of the axis).
  - *Grid.* A light line per rate tick but 0 and per minute but the axis,
    under the curves; the axes and short tick marks over them.
  - *Curves.* Received: filled area (opacity 0.25) with a 1.25 pt line;
    sent: a 1.5 pt line over it; both clipped to the plot.
  - *Line 1*, pieces in this order: `↓ 4.00 Mbit/s`, `↑ 600 kbit/s`,
    `max 11.2 Mbit/s` (the window's peak of both directions), `1.23 GB in ·
    98.7 MB out since 14:02` (or `since menu start`, all label).
  - *Line 2*: `eth0 1 Gbit/s` (or `wlan0 WiFi -61 dBm`), `192.168.1.23/24`,
    `gw 192.168.1.1`, `DNS 192.168.1.1 9.9.9.9` (all servers),
    `VPN wg0 up`; missing parts as before (`no IPv4 address`, `no default
    route`, `no gateway`, `no DNS server`, `no VPN`).
  - *Too wide.* Pieces are separated by 1.6 value-em and keep their order;
    while a line is wider than the element less its padding, the least
    important piece is left out (`style::fit`; of equal importance the
    rightmost). The order is fixed, not "from the right": line 1 drops the
    peak first (the rate axis still shows the scale), then the total, then
    `↑`; line 2 drops DNS, then the gateway, then the VPN, then the
    address (DNS first, then the gateway, as decided for the first
    version). The first piece always stays, cut at the padding. In the
    shipped size both lines of a usual network fit whole.
- **Rows.** With the network element shown the area has two rows (also
  without a battery); on a short screen, one (below).
- **Short screens** (owner, 2026-11-04). At 1280 x 720 and 1366 x 768 two
  rows were pinned above the log and covered half of the second row of the
  eight shipped tiles. Under 900 points high (`ui::SHORT_SCREEN_H`, the window
  height in points, so after the menu's scale) an element takes its
  `ShortSize=WxH`, from the same top-left cell and inside the grid; short forms
  do not overlap each other (see "As built": it may be larger than `Size`). `20-network.conf` says
  `ShortSize=3x1`. A network element one row high (`frame()`: less than two
  cells) is the graph alone, edge to edge across the element: grid and curves,
  the rate labels (none for 0: the bottom edge says it, owner, 2026-11-04), the
  unit and the legend inside the plot on backings, no time labels and neither
  text line. Centred together, eight or nine tiles fit with the one-row area and the full gap at
  1280 x 720; thirteen or more tiles (four rows) scroll above the area, by the
  pinned rule above. At 900 points three area rows and eight tiles fit with the
  full gap.
- **Checked in pictures** (the ignored test `shot_desktop_and_settings_at_densities`,
  1080p, 4K at scale 2, 1366 x 768): labels sharp, no text overlaps another
  or enters the plot (also asserted by `no_text_overlaps_another_or_leaves_the_element`
  with the real fonts at 64, 70 and 76 pt cells;
  `one_row_is_the_graph_alone_with_its_legend_inside` at 64 and 76 pt).
- **Not verified on hardware.**

## Hardware element and network split (owner, 2026-11-04)

Layout at 900 points and above: battery 0,0 1x1; network graph 1,0 2x2 with
axes; list `network-info` 3,0 1x2, one fact per row; temperatures 0,1 1x1; CPU
0,2 1x1; RAM and zram 1,2 1x1; system 2,2 1x1; clock with time and date in the local zone 3,2 1x1 (owner, 2026-11-05; a hole in the bottom row with no battery device stays too, no fill rule). Under 900 points: the graph alone,
3x1, edge to edge, no time labels, no `0`; the hardware tiles and the list
`ShortSize=none`. Battery: `Keyboard [#### ] 100 %`, a vector symbol, a
charging mark, red when low.

### As built (2026-11-04)

Code: `src/thinfloe-menu/src/hardware.rs`, `status/hardware.rs`,
`status/network_info.rs`, `status/network.rs`, `status/style.rs`,
`overlay/etc/thinfloe/status/{20-network,25-network-info,30-temperatures,31-cpu,32-memory,33-system}.conf`.
This replaces "Network, 3 x 2" above as far as the two text lines and their
`style::fit` go; `style::fit`, `line_width` and `Piece.rank` are gone. Rate axis,
time axis, grid and curves stand as described there.

- **ShortSize.** `WxH` from the same top-left cell and inside the grid, which
  replaces the earlier "never larger than `Size`": the graph goes 2x2 to 3x1. `none`:
  the element is not on a short screen, no row, no picture, no wake
  (`Placed.short: Option<Cells>`, `Placed::cells_on`). Short forms must not
  overlap each other either; a form that is not there overlaps nothing.
- **Battery symbol** (`style::draw_battery`). An outline with a nub, filled to
  the level, in the line's colour, on whole points; no level (not reachable, no
  percent) is a dashed empty outline, which reads differently from a solid empty
  one at 0 %. Labels are clipped where the symbol column starts.
- **Network list.** Rows `Down`, `Up`, `Peak`, `Total`, `Since`, `Link`, `IPv4`,
  `Gateway`, `DNS` (a further server is a row with no label), `VPN`. The value
  column starts after the widest label there can be, so it does not move when a
  row goes. A row is at most `line` high and at least the value size plus 1 pt;
  then rows go in this order: further DNS servers (the last first), peak, since,
  total, DNS, gateway, link. Rates, address and VPN stay. A usual network keeps
  all ten rows at 76 and at 64 pt cells. A `Link` row too wide for the column
  leaves out the word `WiFi` (`wlp0s20f3 -61 dBm`, owner, 2026-11-04); a value
  still too long is cut at the padding.
- **Tight rows.** At 76 pt cells with eleven rows (a network with two or more
  DNS servers) rows are 13 pt, and a `p` of a 15 pt Geo label reaches the `k` under
  it: Geo's ink from the top of a `k` to the bottom of a `p` is about 0.88 em.
  `style::label_in_row` then draws the labels half a point smaller, 14.5. Ten
  rows keep 15, and 64 pt cells keep 13. No row height changed.
- **Hardware** (owner, 2026-11-04: four tiles, no pages). One `Type=hardware`
  with a required `Show=temperatures|cpu|memory|system`, so one sampler and the
  existing `has_on` gate serve all four. Temperatures; `CPU load` and `CPU clock`;
  `RAM` and `zram`; fans on one line, `Up`, `Model` with the model on the right (`Maker` when there is none; cut at the
  padding). The system tile is one cell, 1x1 at 2,2 (owner, 2026-11-04): the fans
  that do not fit beside `Fans` with `rpm` showing are left out, the first stay
  (about three to five fans, fewer at 76 pt cells than at 64 pt), and a long
  model is cut at the padding. A value not provided drops out, with no placeholder; a tile left with
  nothing is not shown. The temperature tile holds 4 lines at 76 pt cells and 3 at
  64 pt: the first CPU sensor stays, then hot sensors before normal ones, then
  `sensor_name`'s order, so a critical sensor is not the one dropped.
  `Refresh=5`; the sampler rests while covered and on a short screen. Hot and
  critical temperatures use the warning and alert colours; the thresholds are in
  `hardware::thresholds`.
- **Checked in pictures** (`shot_desktop_and_settings_at_densities`, which also
  writes each element as SVG and at twice its size); `no_text_overlaps_*` with
  the real fonts at 64, 70 and 76 pt cells.
- **Not verified on hardware.** The sensor names and labels are from memory, the
  development host being a VM with no hwmon.
