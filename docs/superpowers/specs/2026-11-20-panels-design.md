# Settings info panels: one layout, labelled values, working GPU detection

Date: 2026-11-20. Status: design. Topic `panels`, branch `feat/ui-panels`.

## Intent

The owner tested ThinFloe 4.6.0 on a real thin client. Three Settings tabs
show facts about the machine (Network "Connections now", Screen "Graphics",
This machine), and they show them three different ways. One of them is
wrong on real hardware. This change gives the three one look and one set of
lines, and makes the Graphics band find the GPU.

## What the owner asked for (as given)

- G Info panels: on the Network screen the info panel is at the top, on the
  Screen screen at the bottom: make them consistent (decide where, give the
  reason). Every value gets a label ("Driver: iwlwifi", not a bare
  "iwlwifi").
- F Screen tab Graphics band says "no graphics device found" on working
  hardware (the thin client shows a picture fine). Find why detection fails
  there, fix it. The video decoding info needs a title ("Video
  processing:").
- E This machine tab: two columns, tighter. Also show everything the
  Network and Screen info panels show, and the Copy button includes it.

## F: root cause

`graphics::gpus` reads each card's `device/vendor` and `device/device`
through `netdetail::hex_id` -> `netdetail::attr` ->
`bounded::read_text(path, ATTR_MAX = 256)` -> `bounded::read_bounded`.
`read_bounded` first refuses a file whose `metadata().len()` is above the
limit. sysfs reports **every** attribute file as one page, 4096 bytes,
whatever it holds. Measured on the build host (2026-11-20):

```
$ stat -c '%s %n' /sys/class/drm/card0/device/vendor /sys/class/net/ens18/address
4096 /sys/class/drm/card0/device/vendor
4096 /sys/class/net/ens18/address
```

So `hex_id` is `None` for every card, every card is skipped as "no PCI ids",
and the band says "No graphics device found" on every machine. The unit
tests did not see it: their fixture files are regular files of 7 bytes.
The same reader drops, on the Network tab, the MAC address
(`netdetail.rs:55`) and the WiFi chip (`chip`, PCI and USB). Only the
driver showed, because it is a symlink, not an attribute read. This matches
the owner's report: the Network panel showed a bare "iwlwifi".

Thin-client evidence (owner's test thin client, 4.6.0, read-only ssh
approved by the owner, 2026-11-20; listing in
`/scratch/oetiker/claude-tmp/ui-round/panels/drm.txt`):

- `/sys/class/drm`: `card1` (no `card0`), `card1-DP-1`, `card1-HDMI-A-1`,
  `card1-HDMI-A-2`, `renderD128`, all under `pci0000:00/0000:00:02.0`.
- `card1/device/vendor` is `0x8086`, `device` `0x1616` (Broadwell-U GT2,
  HD Graphics 5500), driver `i915`; `stat -L` gives **4096** for `vendor`.
- Network: `enp3s0` `fc:aa:14:da:04:d0`, `0x10ec:0x8168`, `r8169`;
  `wlp2s0` `f4:06:69:0c:1d:c7`, `0x8086:0x08b3` (Wireless 3160),
  `iwlwifi`; `address` also 4096.
- `/dev/dri/card1` `root:video`, `renderD128` `root:render`, both with an
  ACL (`+`); `vainfo --display drm` as tfuser works (iHD 26.1.2): H.264,
  MPEG-2, JPEG, VP8 decode. So VA-API was fine; only the GPU rows were
  missing. tfuser has logind session 1 on seat0, tty1.

The cause does not depend on the GPU: any PCI GPU fails the same way.

Not touched, reported: `status/hardware.rs` reads sysfs with an 8 KiB cap,
which works only because 4096 < 8192 (`status/` belongs to another agent in
this round).

### Fix *(architect)*

A new `bounded::read_attr(path, max)` for kernel attribute files: it checks
that the path is a regular file (as `open_regular` does, `O_NONBLOCK`), does
**not** trust `metadata().len()`, and reads at most `max + 1` bytes; more
than `max` is `None`. `netdetail::attr` uses it. Reason: sysfs sizes are
meaningless, the byte cap on the read is the real bound, and the FIFO guard
stays. `read_bounded` keeps its early size check for user files (tiles,
icons), where the size is real.

## G: where the info panel goes *(architect)*

**At the top of both tabs.** The Graphics band moves above "Turn the screen
off after". Reason: an info panel says what the machine has and what it is
doing now; that is what a user reads before changing anything, and after an
Apply on the Network tab it is where the result shows. "Connections now"
has been at the top since it exists and grows with the number of
interfaces; moving it under the WiFi list would put it out of view on a
small screen. The Graphics band is three to six lines, so moving it up
costs the Screen tab little.

## One line model, one helper

New module `infopanel.rs`:

```rust
pub enum Line {
    /// A group inside a panel: `WiFi wlp58s0`, `Video processing`.
    Group(String),
    /// A labelled value: `Driver` / `iwlwifi`.
    Row { label: String, value: String },
    /// A line without a value: `Reading…`, an error (warn colour).
    Note { text: String, warn: bool },
}
pub struct Panel { pub title: &'static str, pub lines: Vec<Line> }

pub fn draw(ui: &mut Ui, t: &Theme, lines: &[Line], label_w: f32);
pub fn label_width(ui: &Ui, lines: &[Line]) -> f32;   // widest label + gap
pub fn text(panels: &[Panel]) -> String;              // clipboard form
```

Rows draw the label in `foreground`, the value in `muted_foreground`
beside it, truncated; groups in `foreground`, a little space before them;
the row spacing is `brand::INFO_ROW_GAP`. The label column is as wide as
the widest label of the panel (`label_width`), not the form column
`FORM_LABEL_W` (240 pt): that is what makes the panels tighter
*(architect: the form column exists to line up input fields, which info
panels have none of)*.

Producers, pure and unit-tested:

- `nettab::info_lines(ifaces, details, known, error) -> Vec<Line>`. Per
  interface a group `WiFi wlp58s0` (`Wired`, `WireGuard`), then rows:
  `State` (connected, off, no cable, not connected), `Network` (SSID),
  `Signal` (strong/…), `Address` (all, comma-joined), `Handshake`
  (`42 s ago`, `none`; WireGuard), `MAC address`, `Gateway`, `Chip`,
  `Driver`; unknown rows left out. After the interfaces a row `DNS in use`
  (`192.168.1.1, 9.9.9.9` or `none`, under the same rule as today: only
  after a status answer without error). Notes: `Looking for network
  adapters…`, `No network adapter found`, the status error (warn).
  `iface_line`, `detail_line` and `dns_in_use_line` go away.
- `graphics::info_lines(Option<&Info>) -> Vec<Line>`. `Reading…` until the
  job answers; per GPU rows `GPU` and `Driver`; no GPU: row `GPU` / `none
  found`; then group `Video processing` with one row per codec (`H.264` /
  `decode, encode`), or row `Video processing` / `not available`.
  `codec_line` becomes `codec_ways` (`decode, encode`).
- `machine::rows` keeps its rows; the add-ons become their own panel
  `Add-ons` (rows `ThinLinc` / `4.19.0`), left out when none is installed.

## E: This machine tab

Panels, in this order: `This machine` (the existing rows), `Add-ons`,
`Network` (`nettab::info_lines`), `Graphics` (`graphics::info_lines`).
Two columns when the tab is at least `brand::INFO_TWO_COLUMNS_W` wide:
left `This machine` and `Add-ons`, right `Network` and `Graphics`
*(architect: the machine facts are one fixed-height block; the network
block varies with the interfaces; splitting this way keeps each column's
panels together and the left column short)*. Narrower: one column in the
same order. Each panel is a card as the other bands, title in
`SECTION_PT`, `brand::INFO_COL_GAP` between the columns. Copy below the
columns, as today.

Data for the tab:

- The network status is asked as on the Network tab, every `POLL` (2 s)
  while the tab is open: `NetTab::poll` takes which tab is open and asks
  only for the status (no file load) when it is This machine. The details
  come with each status answer as today.
- The graphics job runs once a menu run, when the Screen **or** This
  machine tab first shows (`graphics_asked` stays the one guard).
- Copy: `infopanel::text(&panels)` of the four panels as drawn at the
  click. Format *(architect: readable in a ticket, one fact per line,
  greppable `Label: value`)*:

  ```
  This machine
  Maker: HP
  …

  Network
  WiFi wlp58s0
    State: connected
    Driver: iwlwifi
  DNS in use: 192.168.1.1

  Graphics
  GPU: AMD … Picasso
  Driver: amdgpu
  Video processing
    H.264: decode, encode
  ```

  A group indents its rows by two spaces up to the next group or the end of
  the panel; a note is copied as its text. Nothing read yet: Copy stays
  off, as today.

## The Network and Screen tabs

- Network: the "Connections now" band draws `nettab::info_lines` with
  `infopanel::draw`; its place (top) is unchanged.
- Screen: the "Graphics" band draws `graphics::info_lines` with
  `infopanel::draw` and moves to the top.

## Tests

- `bounded::read_attr`: reads a real sysfs attribute
  (`/sys/devices/system/cpu/online`, skipped when absent) that
  `read_text(…, 256)` refuses; a FIFO is refused without blocking; an
  oversize file is `None`.
- `graphics::gpus` from the thin client's captured facts in
  `src/thinfloe-menu/src/testdata/thinclient-drm/` (card list, the
  `vendor`/`device` texts, the driver name, a connector and a simpledrm
  card if present), built into a fixture tree by the test.
- `netdetail::read` MAC and chip from captured facts (same directory).
- `nettab::info_lines`, `graphics::info_lines`, `infopanel::text`: exact
  lines for the cases the old line tests covered.
- `ui.rs` render tests: Graphics band before the idle band on the Screen
  tab; no bare driver text (`iwlwifi` only as a row value next to
  `Driver`); This machine in two columns at a wide width, one at a narrow
  one; the Network and Graphics lines on the This machine tab.
- `menu.rs`: This machine asks for the status every `POLL` and for the
  graphics once; Copy text holds the four panels.
- `ui_code_has_no_style_literals` covers `infopanel.rs`.

## Docs

- `docs/notes/menu.md`, "Settings facts": the shared panel, the place, the
  labels, the This machine panels and Copy format, the sysfs size rule.
- `CHANGES.md` `[Unreleased]`: the Graphics band said "No graphics device
  found" (fixed; the Network tab now shows MAC and chip too); labelled
  values and the panel at the top; This machine shows network and graphics
  in two columns and Copy takes them along.
- CLAUDE.md: one new rule line under Menu: sysfs attribute files are read
  with `bounded::read_attr`, never a size-checked reader (sysfs reports
  4096 bytes for every attribute).

## Amendment (2026-11-20, after the build)

The controller ruled: keep the top only if the Screen tab still fits a
1920x1080 screen with a full Graphics band, else both panels at the
bottom. Measured: the page is visible to y=676; the Position select ended
at about 700 under a one-line band and out of view under a full one; a
band may take about 48 px, which is a single line with its title.
"GPU Intel Corporation HD Graphics 5500 · Driver i915 · Video processing
H.264: decode, encode · HEVC… · VP9…" is about 930 px on an 811 px line,
and columns need a title line plus a row (about 66 px). So both info
panels are the last band of their tab. Guard:
`the_position_row_is_in_view_on_a_1080_screen`; the monitor click tests
stay at 1920x1080.

## Amendment (2026-11-20, after the review)

- Copy: in two columns it stands under the left column, not below both.
  The left column (the machine's facts and the add-ons) has a known,
  short height; the right one grows with every interface and can run
  past the screen, so Copy below both would need a scroll on a machine
  with several adapters. Under the left column it stays in view, beside
  the network. One column: Copy after the last panel, as specified.
  Guard: `the_machine_tab_has_two_columns_when_wide`.
- `INFO_COL_GAP` is the gap between the two columns only; inside them
  the item spacing stays as on the Network and Screen tabs, so a
  panel's label column and label-value gap are the same on every tab.
  Guard: `a_panels_values_stand_as_far_from_their_labels_on_every_tab`.
- Until the machine's facts are read, only the `This machine` panel
  says "Reading…"; the add-ons, network and graphics show what they
  have, and Copy stays off. Guards:
  `the_machine_tab_shows_what_is_read_while_the_facts_are_waited_for`,
  `the_machine_tab_reads_its_facts_when_it_opens`.
