# Connection element: aligned rows, VPN only when up, more facts

## Intent

The owner tested 4.6.0 on a thin client and asked for a change to the
Connection element of the menu's status area
(`src/thinfloe-menu/src/status/connection.rs`).

## What the owner asked for

- Labels left, values right, everywhere in the element (consistent
  alignment).
- The VPN line only when the VPN is up.
- Add: gateway, DNS servers in use, total bytes up / down, uptime.

## What this reverses of PR #27

PR #27 (commits `07401df`, `0dae575`, `3457850`) made the element. This
change reverses four of its rulings; the owner's wish wins:

1. Values started from one column after the widest label (`LABELS`).
   Now each value is right-aligned at the right padding, as in the
   hardware elements and the clock. `LABELS` goes.
2. The VPN row showed `wg0 down` (warning) and `no VPN` (muted). Now the
   row is there only while `wg0` is up.
3. "Gateway and DNS are on the Network tab", not in the element. Now they
   are in both. The once-a-second sampler reads the DNS servers again
   (#27 stopped that because nothing in the status area showed them).
4. "Session totals dropped". Byte totals return, but not the session
   total of #27 with its per-program bases and start time: the element
   shows the kernel's counters of the measured interface, so none of the
   removed tracking (`Since`, `network_bases`, `Running.started`) comes
   back.

The rest of #27 stays: one fact per row, Geo labels, monospace values,
the public address row, the content rule (content when the graph has),
rows leave out by priority under squeeze, the word `WiFi` goes first in a
tight link row, a long value is cut at the padding, names are cleaned of
control characters.

## Design

### Rows

In this order, with the drop rank (0 stays; of the others the highest
leaves first):

| Label | Value | Unknown | Drop |
|---|---|---|---|
| `Link` | `eth0 1 Gbit/s`, `wlan0 WiFi -61 dBm` | `no interface` (warn) | 0 |
| `IPv4` | `192.168.1.23/24` | `no IPv4 address` (warn) | 0 |
| `Gateway` | `192.168.1.1`, `direct` (muted) for a device route | `none` (warn) | 3 |
| `DNS` | `192.168.1.1`, `9.9.9.9 +1` | `none` (warn) | 4 |
| `VPN` | `wg0 up` (`up` in the good colour) | no row | 1 |
| `Public` | `203.0.113.7`, `–` after a failure | no row while no answer | 2 |
| `Traffic` | `↓ 1.23 GB ↑ 456 MB` | no row | 5 |
| `Uptime` | `3 d 4 h`, `4 h 12 m`, `12 m` | no row | 6 |

At most eight rows.

- *(architect)* **Unknown values.** Rows that say where the machine is on
  the network (Link, IPv4, Gateway, DNS) always show, with a warning word
  when the fact is missing: a missing gateway or DNS server is the fault
  a user looks for. A default route without a gateway (a device route,
  `RTF_GATEWAY` clear: PPP, wwan) is no fault: Gateway says a muted
  `direct`; `none` is only for no default route. Rows that are measures (Traffic, Uptime) drop out
  with no placeholder, as values do in the hardware elements. VPN shows
  only when up (owner). Public keeps its #27 behaviour.
- *(architect)* **DNS.** The first server in use (resolved's upstream
  servers, else `/etc/resolv.conf`, as `netstat::dns_in_use`), and a
  muted `+N` for the others (at most `MAX_DNS` = 3 are read). One row
  keeps the element at eight rows; the full list is on the Network tab.
- *(architect)* **Traffic.** One row, `↓` received and `↑` sent, the
  arrows of the graph's legend (`↓ rx ↑ tx`), both in the monospace
  font. Values are the kernel's `rx_bytes`/`tx_bytes` of the measured
  interface (the one the graph draws), in decimal units with three
  digits, the same scale as the graph's bit/s: `850 B`, `12.3 kB`,
  `1.20 MB`, `456 MB`, `1.23 GB`, `1.50 TB`. Decimal because the rates
  beside it are decimal. One row rather than `Down`/`Up` rows: the
  labels `Up` and `Uptime` side by side read alike, and eight rows fit
  where nine would not.
- *(architect)* **Uptime is the machine's uptime** (`/proc/uptime`),
  formatted as on the This machine tab (`status::hardware::uptime`).
  Reasons: the kernel counts an interface's bytes since the interface
  appeared, which for a thin client's network card is the boot, so the
  Uptime row says over what time the Traffic row ran; a connection uptime
  has no source (the kernel keeps no time of the carrier coming up, and
  the menu restarts at every log out, so it could not track one either).
  A WiFi stick plugged in later counts from its plugging: accepted.

### Layout

- *(architect)* Each row: the label in Geo, muted, at the left padding;
  its runs right-aligned so the last run ends at the right padding. The
  room of a value runs from its own label's end plus `gap` to the right
  padding. A row too wide for its room leaves out the word `WiFi`; a row
  still too wide starts at the room's left edge and its last run is cut
  at the right padding (as in #27).
- *(architect)* **1x2**: one column. Eight rows fit at full row height at
  both cell heights (large: 8 x 16 = 128 of 143 pt; small: 8 x 14 = 112
  of 121 pt).
- *(architect)* **2x1**: two columns side by side, each the width of
  half the rect minus padding, with `2 * pad_x` between them; the left
  holds the fixed rows (Link, IPv4, Gateway, DNS), the right the rows
  that come and go and the measures (VPN, Public, Traffic, Uptime, in
  that order), both from the top, so no row changes column when VPN or
  Public appears (review, 2026-11-20; a `ceil(n / 2)` split moved DNS to
  the right and back). Four rows
  per column fit (large: 4 x 16 = 64 of 64 pt; small: 4 x 13.5 of 54 pt,
  above `min_row` 12). Both columns use one row height, so the rows line
  up across.
- `fit`: rows leave out by drop rank (Uptime, Traffic, DNS, Gateway,
  Public, VPN) while a column holds more than `floor(avail / min_row)`
  rows; Link and IPv4 stay. At shipped sizes and cell heights
  nothing leaves out.
- `label_in_row` stays: only on the tightest rows is a Geo label half a
  point smaller.
- `draw` stays pure and inside its rect.

### Sampler (`netstat.rs`)

- `Info` gets `dns: Vec<String>`.
- *(architect)* **DNS source and rate.** The servers come from a file
  read, never a subprocess: `netstat::dns_in_use`, which reads
  systemd-resolved's `/run/systemd/resolve/resolv.conf` (else
  `/etc/resolv.conf`). That file is what is in use: resolved writes into
  it the servers it sends queries to, merged from every link (DHCP and
  static, `wg0` included) and from the global `DNS=` of `resolved.conf`.
  networkd's `/run/systemd/netif/links/<ifindex>` lists only one link's
  own servers and misses the global ones and the VPN's. The sampler keeps
  the answer in a `DnsCache` and reads the file at most every 5 s
  (`DNS_EVERY`); the other values keep the one-second rate.
- `Reading` gets `uptime: Option<u64>` (read with
  `hardware::parse_uptime` from `roots.proc_/uptime`).
- `Series` keeps the last reading's counters and uptime; `Snapshot` gets
  `totals: Option<(u64, u64)>` (rx, tx bytes) and `uptime: Option<u64>`.
- `netstat::bytes(n: u64) -> String`; `scaled` gains the `T` prefix.

The Network graph, `nettab.rs` and the info panels are not touched
(feat/ui-panels works on them).

## Tests

In `status/connection.rs` (rewritten where they pinned the column):

- every label at the left padding; every row's last run ends at the right
  padding (within 0.5 pt) at both sizes and both cell heights;
- the full sample draws the eight rows in order in 1x2, and in 2x1 the
  first four in the left column and four in the right, rows level; the
  left column holds Link IPv4 Gateway DNS also without VPN or Public;
- VPN down and absent: no VPN row;
- unknown gateway and DNS: `none` in the warning colour; no counters, no
  uptime: no Traffic and no Uptime row;
- DNS with three servers: `9.9.9.9 +2`, the `+2` muted;
- drop order under squeeze: Uptime, Traffic, DNS, Gateway, Public, VPN;
  Link and IPv4 stay;
- nothing leaves out at shipped sizes and heights, with the public answer;
- the existing WiFi-word, cut, control-character, no-overlap and
  inside-rect tests, extended to the new rows and the hostile case (a
  40-char interface, IPv6 DNS and public address of 39 chars).

In `netstat.rs`: `bytes` formats (`0 B`, `850 B`, `12.3 kB`, `1.20 MB`,
`1.23 GB`, `1.50 TB`); a reading picks up DNS, counters and uptime from a
fake tree; a snapshot carries the totals and uptime of the last reading.

## Docs

- `docs/notes/status-area.md`: the Connection paragraph rewritten (rows,
  alignment, columns, drop order); the Hardware paragraph's "Uptime ... on
  the This machine tab" gets "and in Connection".
- `CHANGES.md` under Changed.
