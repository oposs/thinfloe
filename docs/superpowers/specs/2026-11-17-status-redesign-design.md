# Status area redesign

Date: 2026-11-17. Owner decisions are marked (owner); everything else is the
controller's call and marked (call).

## Intent

The status area below the tiles is for the person at the machine, not for
support (owner). Today it mixes live values with facts that never change
(gateway, DNS, model, uptime), and it lacks what that person asks about: the
time zone next to the clock, and how fast the remote session answers. The
redesign keeps live values in the tiles, moves fixed facts into Settings tabs,
and adds a Response tile and history graphs.

## Goals

- Tiles show only values that change while the machine runs.
- Fixed facts are in the Settings tab they belong to, or in a new "This
  machine" tab.
- A Response tile shows the round-trip time of the running ThinLinc or OxuLnk
  session (owner).
- History graphs for temperature, fan speed, CPU clock and WiFi signal
  (owner asked for the first three; WiFi signal is a call).
- The clock shows the time zone (owner).

## Non-goals

- No system battery tile; laptops are rare (owner).
- No tile meant for support staff (owner).
- No change to the area's grid (4 columns, at most 3 rows, 1 row on a short
  screen), to placement, or to the Status tab's controls.
- No new layout format in `config.toml`.

## Anti-goals

- The Response tile sends no packets of its own. It reads what the kernel
  already measures.
- The public IP lookup never runs unless the user switched it on (owner).
- No new root helper and no subprocess in a sampler that runs every second.

## Constraints

- Every rule of `docs/notes/status-area.md` holds: `draw` is pure and stays in
  its rect, colours from the theme, sizes from `brand.rs`,
  `ui_code_has_no_style_literals` covers new code, samplers rest while a
  window covers the menu (one exception: the Response sampler, part 2).
- `config.toml` is written only by the menu through the settings store.
- OxuLnk connects over WebSocket (`oxulnk-desktop/src/ws_connect.rs`) and
  ThinLinc over SSH, both TCP, so the kernel's TCP RTT covers both.
- Max 4 cores and `MemoryMax` for builds and tests (machine rule).

## Approaches

- A. Three pull requests in sequence under this one design: tabs and tile
  split, then Response, then History. Each ships and can be tried on hardware
  before the next builds on it. (picked by the owner)
- B. One pull request with everything. One review, but a diff of several
  thousand lines, and a problem in one part holds back the rest.
- C. A history store first, then all tiles on top. Clean code, but nothing
  visible until the end, and the tab move does not need the store.

## Part 1: tabs and tile split

### Elements after part 1

| id | name | sizes (first = default) | short size | change |
|---|---|---|---|---|
| `battery` | Keyboard and mouse batteries | 1x1, 2x1 | 1x1 | none |
| `network` | Network traffic | 2x2, 3x1, 2x1 | 2x1 | short size 3x1 -> 2x1 |
| `connection` | Connection | 1x2, 2x1 | none | new |
| `clock` | Clock | 1x1 | none | adds the zone |
| `temperatures` | Temperatures and fans | 1x1, 1x2 | none | adds fans |
| `cpu` | CPU | 1x1 | none | none (load, clock) |
| `memory` | Memory | 1x1 | none | none |
| `network-info` | | | | removed |
| `system` | | | | removed |

- The traffic graph's short size becomes 2x1 (call, from the pushback): the
  short screen's one row of 4 cells then holds battery 1x1, traffic 2x1 and,
  from part 2, response 1x1. With 3x1 a machine with a wireless keyboard or
  mouse never shows the Response tile on a short screen.
- Ids that keep their meaning keep their id, so a saved layout keeps its
  places and sizes (call).
- `connection` rows: `Link` (`eth0 1 Gbit/s`, `wlan0 WiFi -56 dBm`), `IPv4`,
  `VPN` (`wg0 up`/`down`, `no VPN`), and `Public` when the lookup is on and
  has an answer. Down, Up, Peak, Total and Since are dropped (owner); the
  traffic graph keeps showing the rates. Part 1 deletes the session-total
  tracking (`network_bases`, `netstat_since` in `menu.rs`, the total in
  `netstat.rs`) and its tests. Gateway and DNS go to the Network tab
  (owner). Rows are left out from the bottom when the rect is short:
  `Public`, then `VPN`; `Link` and `IPv4` stay (call). Content: as
  `network-info` today (an address other than loopback and link-local).
- `clock`: a third line `Zone Europe/Zurich` (call): the IANA name of the zone
  the clock already uses (`log::current_zone`); when that zone has no IANA
  name (a POSIX `TZ=CET-1CEST`, a copied `/etc/localtime`), its current
  abbreviation (`CET`); `UTC` only for the UTC zone. A name too long for the
  cell is cut at the start, keeping the city.
- `temperatures`: the fans follow the sensors, one row `Fans 1200 900 rpm`,
  dropped first when rows run out; a fan at 0 rpm drops out as today. An
  element with sensors but no fans, or fans but no sensors, has content.
- Retired ids (call): a saved `network-info` line becomes `connection` at the
  same place and size (both offer 1x2 and 2x1), so a saved layout keeps the
  link, address and VPN where they were. A `system` line is dropped without a
  note (its fans move into `temperatures`). Neither is written back. An
  unknown id stays as today (a newer image may know it).
- Hidden by default (call): the element descriptor gets a flag for elements
  that are off by default. `layout::default_layout` and `layout::complete`
  honour it, so an element with the flag that a saved list does not name is
  appended unticked, at its default size. `connection` and `response` are
  shown by default; the `history-*` elements are not. The test
  `new_elements_are_appended_shown` keeps covering the shown case and gains
  the hidden one.
- Default list, in order: `battery`, `network` 2x2, `connection` 1x2, `clock`,
  `temperatures`, `cpu`, `memory`, and from part 2 `response`. It packs to
  battery 0,0; network 1,0 to 2,1; connection 3,0 to 3,1; clock 0,1;
  temperatures 0,2; cpu 1,2; memory 2,2; response 3,2. A test pins it,
  replacing `the_default_list_packs_to_the_pr6_layout`.

### Public IP

- `[network]` in `config.toml` gains `public_ip = false` and
  `public_ip_url = ""` (owner: off by default, an own lookup address is
  optional).
- Network tab: a switch "Show the public IP address" and, below it, a text
  field "Lookup address", hint `https://api.ipify.org` (the address used when
  the field is empty; call). A non-`https:` address is refused in the field
  with a muted note (call, matches the add-on rule for automatic fetches).
- The lookup runs in its own thread, never in the network sampler (call). It
  runs only while the switch is on and the `connection` element has cells on
  this screen and the network sampler reads (so not while covered, not on a
  short screen, not when unticked): once when these first hold, when the
  sampler's default route or IPv4 address changes, and every 15 minutes. The
  last answer is kept and shown until the next lookup replaces it. Answer: the body trimmed, accepted only when it parses as
  an IPv4 or IPv6 address, at most 64 bytes read, 5 s timeout. The lookup
  checks the scheme itself before each request (`config.toml` can be edited
  by hand and travels with the home) and follows no redirects (call). A failure
  shows `Public –` and one log line per change from success to failure.
- How the HTTP request is made (subprocess `curl -fsS --max-time 5` or a
  crate) is the builder's choice; the menu has no HTTP crate today.

### Network tab additions

- Per interface in the existing "Connections now" band (`network_ui.rs`): MAC
  address, gateway (for the interface holding the default route), and for
  WiFi the chip and driver (`/sys/class/net/<if>/device/driver` link name and
  the PCI or USB ids).
- A row "DNS in use" in the same band: the servers the machine uses now, read
  from `/run/systemd/resolve/resolv.conf` (else `/etc/resolv.conf`) as
  `netstat.rs` reads them (call, from the pushback). The "DNS servers" edit
  field further down holds only configured servers and is empty with DHCP and
  absent on netboot, so without this row a DHCP machine would show its DNS
  servers nowhere.
- Read when the tab opens and on each refresh the tab already does; no new
  sampler.

### Screen tab additions

- A "Graphics" band: GPU name, kernel driver, and the VA-API decode profiles.
- GPU: `/sys/class/drm/card*/device` vendor and device ids and the driver link
  name; the name from `/usr/share/misc/pci.ids` when the image has it, else
  the ids (builder checks the image).
- VA-API: `vainfo` (in the image) runs once when the tab first opens, with a
  timeout, and the codec names are listed (H.264, HEVC, VP9, AV1, each with
  decode or encode). A failed or slow `vainfo` shows "not available".
- Monitors are already in the Screen tab; nothing to add.

### This machine tab

- New `SettingsTab::Machine`, labelled "This machine", last (call).
- Rows: Maker, Model, CPU model, cores, RAM, ThinFloe version, kernel, boot
  (`USB` or `Netboot`), hostname, uptime, and one row per installed add-on
  with its version.
- A button "Copy" puts all rows as `Label: value` lines on the clipboard with
  `wl-copy` (in the image) (call; owner did not object).
- Uptime refreshes once a minute while the tab is open; the rest is read
  when the tab opens.

## Part 2: Response tile

- Element `response`, name "Session response", sizes 1x1, 2x1; short size
  1x1 (call: it is the value the user most wants during a session).
- Source: a new `Rtt` sampler, every 1 s while a client job runs and the
  `response` element is ticked, also while a window covers the menu (call).
  This is the first sampler exempt from the rest-while-covered rule: a
  fullscreen client covers the menu for the whole session, and the user who
  leaves fullscreen because the session feels slow then sees the last 5
  minutes. One netlink query per second costs nothing worth saving. The
  graph clears when a new session starts (no client job ran at the last
  reading); there is no time-gap rule.
- Clients (call, from the pushback): only jobs of the shipped tiles
  `10-thinlinc.desktop` and `20-oxulnk.desktop` count, whatever a user tile
  sets; `X-TF-Save-On-Exit` is also allowed on user run tiles, and a browser
  tile with it would otherwise get a "Session response". The client name
  shown is the shipped tile's.
- Sockets: the processes in the process group of each running client job
  (the menu has the pgid), and their descendants (builder checks whether
  `ssh` under `tlclient` keeps the group). Their TCP sockets come from
  `/proc/<pid>/fd` inodes; tfuser owns the processes, so no root.
- RTT: `NETLINK_SOCK_DIAG` with `INET_DIAG_INFO` (`tcp_info.tcpi_rtt`,
  `tcpi_rttvar`), matched by inode, state ESTABLISHED, remote address not
  loopback. With several sockets: the one with the most bytes received since
  the last reading (call: the display stream). No `ss` subprocess.
- 1x1: the client name (`ThinLinc`, `OxuLnk`), the RTT `23 ms` large, and
  `± 4 ms`. 2x1: the same on the left, the RTT of the last 5 minutes as a
  line graph on the right, axis as the traffic graph's style. Colours: warning
  at 100 ms, alert at 250 ms (call).
- Content: only while a matching socket exists.
- Status tab (call, from the pushback): while no client job runs, the
  Response row's note says "shows during a session", never "nothing to show
  on this machine".

## Part 3: History tiles

- One element per metric, so the layout format stays `<id> <w>x<h>` (call):
  `history-temp`, `history-fan`, `history-clock` (CPU clock), `history-wifi`
  (signal of the measured interface, from the network sampler).
- A series keeps one device (call, from the pushback): keyed by hwmon chip
  and index, chosen at the first reading. `history-temp` takes the first CPU
  sensor, else the first sensor in `sensor_name` order; `history-fan` the
  first fan seen. A stopped fan is recorded as 0 rpm in the series (the
  sampler today drops a fan at 0, so the snapshot gains the fan's key and
  keeps stopped fans for the history; the `temperatures` tile still hides
  them). A device that disappears ends the series; the tile has no content
  until it returns.
- Sizes 2x1, 2x2, 1x1; short size none. Off in the default list.
- The last 5 minutes: 60 points from the 5 s hardware sampler, 300 from the
  1 s network sampler for WiFi. Line graph, a value axis in the traffic
  graph's style, the current value as text in the corner; temperature uses
  the `temperatures` heat colours.
- Content: when the metric has at least one reading. A gap longer than two
  sampling intervals (10 s hardware, 2 s network) since the last reading
  clears the graph (call; the hardware sampler has no gap rule today).
- The hardware sampler keeps the series; it runs when a hardware or history
  element has cells (`hardware_wanted` extends).

## Tests (each part)

- Every new element inside its rect at every size and both cell heights.
- Default list packing test; retired ids dropped from a layout.
- Public IP: parser, refusal of non-`https:` in the field and at lookup time,
  no redirect followed, no request while off or while `connection` has no
  cells (a test that counts lookups).
- Response: socket selection against a fake `/proc` and canned diag replies;
  loopback excluded; a user tile with `X-TF-Save-On-Exit` ignored.
- Network tab: "DNS in use" from a fake resolv.conf.
- Clock: zone line for an IANA zone, a POSIX `TZ=`, and UTC.
- History: series stays on its device when another fan stops; 0 rpm kept.
- History: series length, gap handling.
- `docs/notes/status-area.md` updated per part; CHANGES entries per part.

## Left to the builder

- HTTP mechanism for the public IP.
- GPU names (`pci.ids` present or not), WiFi chip naming.
- Process group versus descendant walk for the Response sockets.
- The exact layout of the new tab bands, within the existing tab style.
