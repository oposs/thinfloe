This note covers how the tile menu works: the window rules, tiles and icons, the Upgrade screen, density, monitor arrangement, the Network tab, the Graphics band, the This machine tab, its log, failure handling and its known limits.

## Window rules and folded windows

- Session/compositor: `overlay/etc/thinfloe/labwc/` (`rc.xml`, `environment`) - kept outside `$HOME` because home is captured/restored wholesale by the persistence machinery, see [Persistent Home Directory](home-persistence.md).
  `rc.xml` also holds the window rule that keeps the window with app_id
  `thinfloe-menu` at the bottom (`ToggleAlwaysOnBottom`) and out of the
  window list that feeds the menu's folded-window strip (`skipTaskbar`).
- No window list: waybar and GTK 3 left the image. labwc draws an iconify button
  and, with no keybinding but the optional media keys, a minimized window
  needs a way back: it shows as a chip in the folded-window strip at the
  bottom of `thinfloe-menu`, and
  one click restores it. A fullscreen ThinLinc client covers the menu, so
  the strip is visible exactly when no window is fullscreen, which is when a
  window can go missing.

## Upgrade screen, tiles and icons

- Upgrade screen: the Upgrade tile (`X-TF-Action=upgrade`, shipped files
  only, hidden on netboot) swaps the grid for a screen drawn by
  `src/thinfloe-menu/src/upgrade_ui.rs` from the state in `upgrade.rs`. It
  writes through `sudo -n thinfloe-upgrade write` and never sends SIGKILL to
  it. Restart, Power off and Log out wait for a running write ("Writing
  SanDisk Cruzer"); on a fresh install that has written its first byte
  (`@touched`) the dialog says the stick will not start until it is written
  again; before that byte a cancel says "Cancelled before anything was
  written. The stick is as it was." After a successful write, a step the tool
  skipped is left out and one it began without `@done` is drawn done.
- A stick that already has the target release, or a newer one, is named
  so in the footer ("Write ThinFloe 4.5.0 again to this machine's stick,
  which already has it", "Downgrade this machine's stick from ThinFloe 4.6.0
  to 4.5.0") and the confirm button reads Write again or Downgrade (not "Go
  back", one click from the screen's Back). With a `tl.old/` on the stick
  the footer adds that the older fallback release is dropped: the switch
  removes `tl.old/` and the same release becomes the fallback. Writing
  again stays possible: it repairs a damaged `tl/`. `Upgrade::not_older`
  compares with `release::newer`, so a `-dev` stick is an ordinary upgrade
  (the Makefile sorts it below its release); the running write keeps the
  wording it started with. An unknown stick version is an ordinary upgrade.
- What's new is disabled when the stick's X.Y.Z is not below the target's.
  A dev build `4.5.0-dev+sha` is made after release 4.5.0 and has its
  changes, so it gets no button for 4.5.0; `4.4.0-dev+sha` gets the notes
  after 4.4.0.
- What's new: a button left of Check now runs `thinfloe-upgrade notes` and
  opens a dialog with the notes as headings (`4.4.0 · New`, one per `###`
  group) and bullets, backticks removed (`notes.rs`, no Markdown renderer).
  The range comes from `Upgrade::notes_range`: Upgrade of a stick with a
  known version gives `--from <version>`; a stick without one gives every
  release up to the target and the line "The version on this stick is not
  known"; Erase and install, or no drive chosen, gives `--only` (the target
  alone). A stick version or target that does not start with `X.Y.Z`
  counts as unknown, or gives no button, since `notes` refuses it. There is
  no button while an address is entered under Advanced:
  the notes are fetched from the tag of the default release only. Exit 3
  shows "No changes between A and B.", a failure the tool's last stderr line.
  The dialog is modal, closes with Close or Escape, and its list scrolls
  inside 1280x720. A result is dropped unless it answers the load that runs
  now (same from, to and `--only`), so a late answer after Close is ignored.
  The list's ScrollArea sets `min_scrolled_height` as well as `max_height`:
  without it the anchored dialog grew a few points each frame, and a click
  on Close missed for the first frames.
- Tiles: the shipped tiles are `.desktop` files in `overlay/etc/thinfloe/menu/`,
  outside `$HOME` for the reason `rc.xml` is. Per-machine tiles go in
  `~/.config/thinfloe/menu/*.desktop`: at most 32 files, and they may only `run`
  a program. `X-TF-Action=upgrade` (the Upgrade tile,
  `50-upgrade-image.desktop`, shown only on USB boots) is honoured only in
  shipped files, like `settings`. `X-TF-Multiple=true` lets a run tile start again while a
  copy of it still runs (the ThinLinc, oxulnk and Terminal tiles set it); without it a
  tile runs once at a time. A user file rewording a shipped tile cannot set
  it. A user file named like a shipped one may reword `Name`, `Comment`
  and `Icon` only; its other keys are ignored. The files travel with the home
  directory through `save-home`.
  A user file that matches no shipped file and sets neither `Exec` nor
  `X-TF-Action` is not drawn; the banner names it. That is what a reword
  of the old `80-logout.desktop` became when Restart and Power off replaced
  it, and it used to draw a grey "has no Exec" tile.
  `Icon=` is user data the menu parses on every boot, so each icon renders
  in a child process, `thinfloe-menu --render-icon`, one at a time, under
  `RLIMIT_AS` 256 MiB, `RLIMIT_STACK` 256 MiB, `RLIMIT_CPU` 3 s,
  `RLIMIT_FSIZE` 0 and `RLIMIT_NOFILE` 16. The menu kills and reaps it
  after 2 s, or when the 5 s for all icons are used up. Any failure draws
  the tile's first letter. In-process guards cannot bound memory: a
  700-byte SVG of nested `<pattern>`s asked resvg for 2.88 GB, and the
  thread the deadline abandoned kept allocating inside the menu; three
  such tiles reach about 8.6 GB, enough for the OOM killer to pick the
  menu on a machine that holds its rootfs in RAM. A renderer dies with
  the menu (`PR_SET_PDEATHSIG`), and `RLIMIT_CORE` is 0, so an aborted
  render leaves no core file in the home. A release renderer needs 64 MiB
  for every legitimate icon measured, and the debug build (the tests)
  needs 192 MiB. Before
  rendering, the child still refuses an SVG that nests deeper than 64
  elements, has more than 10000, declares entities or uses filters (one
  `feTurbulence` ran for over a minute). usvg's `<image>` resolvers are off
  (`svg_options` in `icons.rs`): the default ones read any path and decode
  `data:` of any size. Limits and guards are in
  `src/thinfloe-menu/src/icons.rs`, and `src/thinfloe-menu/tests/render_icon.rs`
  renders through the real binary.

## Menu density

- Menu density: `thinfloe-menu` picks a scale `s` per output
  (`src/thinfloe-menu/src/density.rs`) and applies it as egui zoom
  (`apply_scale` in `app.rs`), so a tile is about the same size on paper on
  every screen. `s = ppi / 92`, with `ppi` = mode width in px / (physical width
  in mm / 25.4), never below 1 from density alone, rounded to the nearest 0.25.
  The reported size is not trusted when a side is 0, when the aspect of mm and
  px differs by more than 10 %, when `ppi` is outside 50..800, or when the
  width is under 200 mm (this catches EDIDs that store cm, such as 160 x 90);
  then `s` is 2 if the shorter side of the mode is 1800 px or more, else 1. The
  fit rule then takes the largest 0.25 step with `889 * s <= 0.9 * window width`
  (never below 0.5), which shrinks the menu on a small dense screen. The tile
  group stays 4 x 220 x 76 points (`GROUP_W` 889).
  The output is the one winit names for the menu window. winit names the
  outputs the window is *visible* on, so a fullscreen window over the whole
  menu leaves none; that is no move, and the last one counts
  (`seen_monitor` in `app.rs`). Taken as a move, it gave the guess for the
  largest mode and the title bars grew while OxuLnk ran (2026-10-30). winit
  sends no event when the window first appears on an output, so until one
  is known the menu looks again every 100 ms for 10 s (`monitor_recheck`).
  `test/runtime/test-labwc-runtime.sh` covers the menu with a fullscreen foot.
  The menu writes `s` atomically (one number and a newline) to
  `$XDG_RUNTIME_DIR/thinfloe-scale` (`TF_SCALE_FILE` overrides), and
  `thinfloe-session` removes a stale one before the menu starts.
  `/usr/libexec/thinfloe/labwc-scale` waits for that file and scales the labwc
  title bars (font `round(10 * s)`, titlebar padding and button keys), then runs
  `labwc --reconfigure`. It keeps running for the session and reads the file
  once a second: the menu rewrites it after the real window width is known, a
  move to another monitor and a scale change, and each change is applied again
  from the system files (a change to 1 restores the symlinks). When anything
  fails it logs `thinfloe: title bar scale not applied: <why>`, the bars keep
  size 10, the same value is tried up to 3 more times, 5 s apart
  (`TF_SCALE_RETRY`), and then the helper waits for a change. labwc's outputs stay at scale
  1. `thinfloe-terminal` reads the file too (see
  [Reaching a terminal](session.md#reaching-a-terminal)). Guards: the unit tests in
  `density.rs` and `ui.rs`, `test/integration/test-labwc-scale.sh`,
  `test/integration/test-terminal.sh`. The ignored test
  `shot_desktop_and_settings_at_densities` writes PPM pictures at 1080p, 4K
  and a 21 inch 4K.

## Monitor arrangement

  **Monitor arrangement** (Settings → Screen). Apply is offered once the
  arrangement on the tab differs from the one it was made from (`changed`
  in `monitors.rs`); a Keep or a revert starts it anew. The menu reads the monitors
  through `zwlr_output_manager_v1` and never sets them. The arrangement is
  `[monitors]` in `config.toml`, written when the arrangement is confirmed;
  the menu writes `~/.config/thinfloe/generated/kanshi` and reloads kanshi
  with SIGHUP. A legacy kanshi file the menu cannot fully read, or whose
  profile names fail the name check, becomes `[monitors] kept`: shown
  read-only with "Let the menu manage it" and left as it is until then. When
  no kanshi runs, the menu starts `kanshi -c` on `generated/kanshi` itself, in
  a session of its own; that kanshi is not in the session's `HELPER_PIDS` and
  ends when labwc's socket goes. `thinfloe-session` starts `kanshi -c
  generated/kanshi` when that file is readable and no kanshi of this user
  runs (found through `/proc/<pid>/comm` and the `Uid:` line of
  `/proc/<pid>/status`). Before that it undoes an unconfirmed layout: an Apply
  keeps the previous file as `generated/kanshi.prev` and writes the marker
  `generated/layout-unconfirmed` (in the home so that a home save carries it),
  which holds `prev` or `none`. `prev` restores `kanshi.prev` over `kanshi`;
  with no `kanshi.prev` it removes `kanshi` and logs "thinfloe: unconfirmed
  layout without config.prev: removing the kanshi config". `none` removes
  `kanshi` and `kanshi.prev`. At start the menu regenerates `generated/kanshi`
  from `config.toml` and reloads kanshi when that changed it. A home in the
  4.x layout keeps `~/.config/kanshi/config` and the markers beside it until
  it is migrated. The monitor for the menu
  goes through `$XDG_RUNTIME_DIR/thinfloe-menu-output`: the menu writes the
  output name there (empty means no choice), `labwc-scale` writes a
  `MoveToOutput` window rule into the composed `rc.xml`, runs
  `labwc --reconfigure` and confirms in `thinfloe-menu-output.applied`, and the
  menu then replaces its window to move.

## Network tab: DNS servers

  **DNS** (Settings → Network) is a band of its own below WiFi, with one
  row, "DNS servers", in either wired mode; the Wired band keeps the
  address mode, address and gateway. Empty means the servers DHCP gives
  (none for a fixed address). Filled, up to three IPv4 addresses are one
  setting for the whole machine: kept in `[network]` of `config.toml` and
  written into `generated/ethernet.yaml` and into the WiFi definition of
  `generated/wifi.yaml`, because systemd-resolved queries the DNS servers of
  all links, and servers set on the wired link alone would still send queries
  to the WiFi link's DHCP servers. With DHCP both files carry
  `dhcp4-overrides: {use-dns: false}` and `nameservers` (networkd:
  `UseDNS=false` and `DNS=` lines); address, gateway and search domains still
  come from DHCP. A fixed address keeps `dhcp4: false` with `nameservers`.
  DHCP without servers writes no `generated/ethernet.yaml`. The initramfs
  installs the generated files at every boot (`install_local_network()`);
  `thinfloe-network apply` puts a change in force at once. With no network
  left there is no `generated/wifi.yaml`, and the tool removes
  `/etc/netplan/wifi.yaml`, so the machine leaves the forgotten network at
  once; an unedited template there still leaves the installed file as it is.

  A WiFi or wired file the menu could not fully read at migration (an EAP
  entry, a form the menu cannot express) is kept verbatim in `[network]`
  (`kept_wifi`, `kept_wired`). The tab shows it with "Kept from your old
  settings; the menu cannot change it." and the DNS row is greyed;
  "Let the menu manage it" replaces it with the menu's own settings when the
  menu can keep everything in it (same networks, passwords and addressing,
  with the input checks passed); otherwise it says the kept settings have
  something the menu cannot keep. WiFi beside a kept wired file keeps the
  servers it has ("WiFi keeps the DNS servers from your old settings."). An
  unrenderable converted network file is kept verbatim in the same way.
  With more than three servers the row shows them all and Apply says "DNS
  servers: up to three IPv4 addresses" until the row holds three or fewer; a
  kept file with more than three is not offered for take-over. When WiFi has
  other servers than the wired connection (after a take-over, or servers set
  before WiFi got them), the tab is dirty and Apply writes the saved wired
  setting again to bring both in step; the band then says "WiFi uses other DNS
  servers. Apply sets the ones above." A change of the servers alone rewrites
  both files. A rewrite for the servers never drops a network: a saved network
  that today's checks refuse stops it with that network named.

  Netboot shows "A netbooted machine always uses the DNS servers DHCP sends."
  instead of the row and "A netbooted machine always uses DHCP." for the wired
  address. `generated/ethernet.yaml` is not installed there. A new fixed
  address or DNS servers are refused when the change is planned ("A fixed
  address cannot be set on netboot."); a `[network]` that carries a stick's
  fixed address keeps it, still renders it into `generated/ethernet.yaml` for
  the next stick boot, and takes WiFi changes. The menu writes `[network]` only after
  `thinfloe-network apply` succeeded and puts the old generated files back
  when it fails.

  The parser keeps a file manageable only with `dhcp4-overrides` exactly
  `{use-dns: false}` beside a non-empty `nameservers` on a `dhcp4: true`
  definition, or with neither key. The menu writes its WiFi definition
  with `dhcp4: true` and `optional: true`, so a kept WiFi file is manageable
  only when `dhcp4` is a flag that is true and `optional` is missing or
  true (as `optional` on the wired `eth`); a take-over would otherwise turn
  DHCP on, or stop boot waiting for the device. A file with no `wifis` key
  is manageable, with no device and no networks; a `wifis` key must hold
  exactly one definition. Text with another `network.version` than 2 or
  another `renderer` than `networkd` (for the file or one definition) is
  not manageable, under the 4.x menu header too: the migration keeps such
  an old file as written, and "Let the menu manage it" is not offered,
  since the menu would render it as version 2 for networkd (netplan's
  defaults, so either key may be left out). WiFi text with more than three
  DNS servers is not manageable, as the wired one. On 1080p a full tab scrolls; every
  control scrolls into reach above the pinned Apply. A home in the 4.x layout
  opens Settings read-only: the Network, Screen, Status and Keyboard tabs are disabled.
  Guards: the `netfiles` and `network` tests,
  `every_network_control_scrolls_into_reach`,
  `test/integration/test-network-netplan.sh`.

## Settings facts: Network, Screen and This machine

Facts that do not change while the machine runs live in Settings, not in
the status area (see [status-area.md](status-area.md)).

**Info panels.** The Network tab's "Connections now", the Screen tab's
"Graphics" and the This machine tab draw their facts the same way
(`infopanel.rs`): a card band with a title, then one row per fact, the
label in the foreground colour and the value muted beside it ("Driver" /
"iwlwifi", never a bare value). A group (an interface, "Video
processing") indents its rows under its title. The label column is as
wide as the widest label of the band, not the 240 pt form column, and a
row is as high as its text (the interact height is set to 0 inside a
panel): nothing in a panel takes a click. On the Network and Screen tabs the info panel is
the last band (2026-11-20, asked for one place on both tabs). The top was
tried first and measured: on a 1920x1080 screen the Screen tab's page is
visible to y=676, the monitors' Position select ended at about 700 even
under a one-line Graphics band, and a full band (GPU, Driver, three
codecs) pushed the row out of view; a band fits there only within about
48 px, one line with its title, which a GPU name and three codecs do not.
Guard: `the_position_row_is_in_view_on_a_1080_screen`. The producers are pure: `nettab::info_lines`,
`graphics::info_lines`, `machine::rows`.

**Sysfs sizes.** sysfs reports every attribute file as 4096 bytes,
whatever it holds. `bounded::read_attr` reads it as a regular file to a
byte cap and does not look at the reported size; `netdetail` and
`hardware.rs` use it. In 4.6.0 the Network tab read attributes with a
size-checked reader and a 256-byte cap, so no GPU id, MAC or WiFi chip was
ever read: the Graphics band said "No graphics device found" on every
machine (owner's thin client: `card1`, `8086:1616`, `i915`, attribute
size 4096). Its facts are in `src/thinfloe-menu/src/testdata/thinclient-drm/`.

**Network tab, "Connections now".** The last band, after "Public IP
address". Per interface a group `WiFi wlp58s0`
(`Wired`, `WireGuard`) with the rows State (connected, off, no cable, not
connected), Network (the SSID), Signal (`strong (-55 dBm)`), Address (all
of them, only when connected), Handshake (WireGuard: `42 s ago` or
`none`), MAC address (an all-zero MAC is left out), Gateway (for the
interface holding the default route), and for WiFi Chip and Driver
(`device/driver` link name); an unknown row is left out. The chip is the
PCI vendor and device name from `/usr/share/misc/pci.ids` (package
`pciutils`), else `8086:2723`, or `USB 0bda:8179` for a USB adapter. After
the interfaces a row "DNS in use" (`192.168.1.1, 9.9.9.9` or `none`) gives
the servers the machine uses now (`netstat::dns_in_use`, also read by the
status area): `/run/systemd/resolve/resolv.conf`, else `/etc/resolv.conf`;
it shows only after the first status answer without an error, so it never
says "none" too early. The "DNS servers" field further down holds only
configured servers. `netdetail::read` runs in the job thread after each
successful `thinfloe-network status`, so the facts are read when the tab
opens and on each of its refreshes; it reads `/sys` and `/proc`, bounded,
and starts no subprocess. A failed status call keeps the last list and the
details read with it.

**Network tab, "Public IP address".** A band after DNS with the switch
"Show the public IP address" and the field "Lookup address" (hint
`https://api.ipify.org`, the address used while the field is empty). It is
outside the Apply group: it takes clicks before the network files are
read and while an Apply or a take-over runs. The switch saves
`public_ip` in `[network]` at once; the address saves as `public_ip_url`
on Enter, when the field loses focus, and when the user leaves the Network
tab. Escape puts the saved address back, and Back discards a typed one;
neither saves. An address `publicip::url_ok` refuses is not saved and the
field shows "Only https:// addresses are used." Writes run one at a time,
the latest click waits behind the running one, and a logout waits for
them. A failed save puts the switch and the saved address back to what
`config.toml` holds, shows the error line and logs a warning; text typed
since the commit stays in the field. A network Apply or take-over keeps
both keys. The lookup itself: [status-area.md](status-area.md#public-ip-lookup).

**Screen tab, "Graphics".** The last band of the tab: per DRM card
(`/sys/class/drm/card<N>`, at most 16, a card without PCI ids such as
simpledrm left out) a row GPU with the `pci.ids` name or the ids
(`8086:1916`) and a row Driver (left out without a driver link), or `GPU:
none found`; then a group "Video processing" with a row per codec
(`H.264: decode, encode`, `HEVC: decode`; H.264, HEVC, VP9 and AV1 only,
in that order), or the row `Video processing: not available`; `Reading…`
until the job answers. The job runs once per menu run, when the Settings
screen first shows the Screen or the This machine tab (from `tick`, since
the click that leaves the Network tab already carries the public IP write
as its one effect). It runs `vainfo --display drm` (`TF_VAINFO_CMD`
replaces `vainfo`) through `quiet::run`, 5 s timeout, at most 64 KiB of
output. A failure, a timeout, a full cap or a listing without codecs is
"not available", and the menu log gets one line `VA-API not available:
<cause>.` under the source `graphics`. The codecs are those of libva's
default render node, not tied to one GPU row on a machine with two. tfuser
is not in the `render` or `video` group; the render node is reached
through logind's ACL for the tty1 session (`/dev/dri` nodes carry an ACL,
and `vainfo --display drm` as tfuser works on the owner's thin client,
iHD driver, 2026-11-20).

**This machine tab.** The last Settings tab, four info panels: This
machine (Maker, Model, CPU model, Cores, RAM, ThinFloe version, Kernel,
Boot `USB` or `Netboot`, Hostname, Uptime), Add-ons (one row per installed
add-on with its version; left out without one), Network and Graphics (the
lines of the other two tabs). From a panel width of 720 pt
(`INFO_TWO_COLUMNS_W`) there are two columns: This machine and Add-ons
left with Copy under them, Network and Graphics right; narrower, one
column with Copy at the end. An unknown value leaves its row out. The
facts (`machine::read`: DMI, `/proc/cpuinfo`, `/proc/meminfo`,
`/proc/uptime`, `/proc/sys/kernel/osrelease`, bounded) are read each time
the tab opens; the uptime counts on from that reading and the loop wakes
for it once a minute only while the tab is on screen. While the tab is
open the network status is asked every 2 s as on the Network tab
(`NetView::Facts`: the status only, never the network files). Copy puts
the four panels on the clipboard (`infopanel::text`): each panel's title,
its rows as `Label: value` lines, a group's rows indented by two spaces
under its title, a blank line between panels. It runs `wl-copy`
(`TF_WL_COPY_CMD`), 3 s timeout; `wl-copy` forks a server that keeps the
text, and the menu leaves that fork running. The button is off while a
copy runs; then it says "Copied" or shows the error in the warning colour,
and a failure is also logged as "Could not copy to the clipboard:
<error>." The tab changes nothing, so it stays enabled after a failed
migration of the settings.

## Keyboard tab

Settings -> Keyboard (`keyboard.rs` state, `keyboardtab_ui.rs` drawing,
`kbregistry.rs` the layout list). The layout and its variant are chosen from
the full list of `/usr/share/X11/xkb/rules/evdev.xml` (package `xkb-data`, in
the image); CapsLock as Ctrl is on or off; the Compose key is Right Alt by
default. A change is written to `[keyboard]` of `config.toml` and applied to
the running session at once (see [session.md](session.md)), one apply at a
time; "Try it here" is a text field to test the layout right there. A failed
apply or an unreadable registry shows a note on the tab: the error text, or
"Takes effect at the next start." when labwc runs without the composed
directory (the change is saved, only the live apply is skipped). Only a
failed save is a failed apply ("The keyboard setting was not applied: ..."
in the menu's log); when the save worked but the composer or `labwc
--reconfigure` failed, the tab counts the choice as applied, so choosing the
old value again writes it back, and the note gives the reason plus "The
setting is saved and takes effect at the next start." (log: "The keyboard
setting was saved, labwc does not use it yet: ..."). When
the move of the settings failed at this boot, the tab is read-only like
Network, Screen and Status.

"Num Lock on at start" (default on; off suits small keyboards with a number
pad over the letter keys) is `numlock` in `[keyboard]` and
`THINFLOE_NUMLOCK=on|off` in `generated/environment`. labwc 0.9.3 sets Num
Lock from `<numlock>` in `rc.xml` only when a keyboard appears (at start and
on hot-plug, `new_keyboard()` in `src/seat.c`), never on `--reconfigure`, so
a live apply that changed it says "Num Lock follows at the next start." At
session start, `thinfloe-compose-labwc-config` writes `.source/base.xml` of
the composed directory as a copy of the system `rc.xml` that says off;
`labwc-scale` scales what `rc.xml` points to. Guards:
`test-labwc-config-merge.sh`, `test-labwc-scale.sh`, the `keyboard`,
`generated` and `convert` tests.

"Volume keys control this machine" and "Brightness keys control the
monitors" (both off by default) are `volume_keys` and `brightness_keys` in
`[keyboard]` and `THINFLOE_VOLUME_KEYS=on|off`, `THINFLOE_BRIGHTNESS_KEYS=on|off`
in `generated/environment` (managed keys, listed in
`/etc/thinfloe/menu-managed-keys`). Under each switch the note "When on,
these keys no longer reach the ThinLinc session or any other program." in
the muted colour (`MEDIA_KEYS_NOTE`). A change takes effect at once: the
composer adds or drops the bindings and the OSD gets a `reload`. See
[media-keys.md](media-keys.md).

labwc 0.9.3 segfaults at start (status 139, before the session runs) on a
layout and variant XKB cannot build, so no value reaches
`generated/environment` unchecked. The render of that file refuses a
`[keyboard]` that `keyboard::validate` rejects against the layout list.
Values from outside the tab are repaired by `keyboard::repair` (a variant the
layout lacks is dropped, an unknown layout becomes `us`): the store does it
when it opens `config.toml` (a rebuilt file, an upgrade that changed the
list), the migration when it converts a 4.x `~/.thinfloe-environment`. A
value already on disk when the list changes is caught by the composer, see
[session.md](session.md). Guards: the `keyboard`, `generated`, `store` and
`migrate` tests, `test-labwc-config-merge.sh`, `test-labwc-runtime.sh`.

## Switches

Every switch in the menu (Keyboard tab, the public IP switch, Advanced on
the Upgrade screen) is lyra's, square, with these edits in `brand::theme()`:
the knob is the foreground colour in both positions, and it is 14 points
with 2 points of track around it on every side, the same gap at either end;
the track has no border (lyra's transparent 1 px border is one device pixel
at every scale and made the gaps unequal at a fractional scale). lyra's
dark knob is `primary-foreground` when on, which the brand sets to
`#04121f` for the ice button, next to the page's `#05070a`: on 4.6.0 the
knob vanished into the page and the ice half of the track looked like a
knob that had not moved (field report, 2026-11-20). The knob jumps, as every
animation in the menu does (`animation_time` 0, see `app.rs`). Guards:
`the_switch_knob_moves_and_reads_on_the_page`,
`a_switch_click_damages_the_old_and_new_knob` in `brand.rs`.

## Known limits

Open items found during the Ubuntu 26.04/Wayland/PipeWire upgrade. These are
not fixed; they are recorded so they aren't rediscovered from scratch.

- The grid is on one monitor. The span patch covers Xwayland windows only.
- The status area is not seen while a fullscreen window covers the menu.
  Program output still wakes the covered loop and every wake requests a
  redraw (`user_event` in `app.rs`), so a fullscreen client that writes a
  line a second keeps a 1 s render going behind the cover.
- Clicking a tile is not covered by the headless runtime test, which has no
  input devices. The end-of-session dialogs and their buttons (Stay / Stop
  them and restart, Stay / Restart anyway, and the same for Power off and
  Log out) are proven by unit tests of the state machine only.
- Program output keeps its SGR colours (16, 256, RGB, bold); other escape
  sequences are removed. At most 64 colour runs per line.
- Full-colour theme icons with drop shadows draw the letter, because
  filters are refused: 15 of 70 Adwaita `scalable` and 53 of 425 hicolor
  `scalable` icons on a desktop host. Every Adwaita symbolic icon, the
  shipped Lucide set and the image's `foot.svg` render.
- Each icon costs one process start at menu startup, and the icons render
  one after another, so that at most one renderer holds its 256 MiB.
- A long log line is cut to one row on screen; the session log has it in
  full, up to the 1 KiB cut.
- **Confirmed on hardware (2026-10-07):** tiles start their programs,
  Save-On-Exit, Back up home and the logout Stay / Log out anyway dialog
  work. The square tiles and the log area have not been run on hardware.
- **Folded windows.** `thinfloe-menu` watches toplevels through
  `wlr-foreign-toplevel-management` (`src/windows.rs`); a folded window
  gets a chip in the strip below the log, and a chip click unfolds it. A
  broken Wayland connection empties the list. From a VT or SSH,
  `thinfloe-menu --windows` prints `app_id<TAB>title<TAB>folded|open` per
  window and `thinfloe-menu --unfold <app_id or title>` unfolds a window
  whose chip is gone.
- Not verified on hardware (Part 4): the boot screen on real firmware (and
  its host-name line), the title bars, the strip and chip clicks with a real
  mouse, the DHCP time zone from a real DHCP server (on netboot as well; only
  QEMU with dnsmasq has run it).
- Not verified on hardware (menu density): the scale chosen from real EDIDs
  (including monitors that report no or a wrong size), and the scaled title
  bars after labwc's reload.
- Not verified on hardware (Upgrade screen): choosing and clicking a drive
  with a real mouse, a write to a real stick with its bar, speed and time
  left, Cancel during a write, and Restart while a stick is written. At
  1080p and on a 21 inch 4K screen the Choose page fits without scrolling:
  Release on top (with Advanced), the drives in a left column under a
  Target heading that carries "Show all drives", Action and Options in a
  right one (one column below `UPGRADE_TWO_COLUMNS_W` in `brand.rs`), and
  the log shrinks to `LOG_CHOOSE_ROWS` (4) rows while choosing. The footer
  (summary and confirm button; Cancel, Stop; Restart now, Write another stick, Try again) sits
  one block gap below the last block, and is pinned to the bottom of the
  panel only when the page overflows, so its buttons never scroll away;
  then a solid scroll bar shows (egui's floating default drew none until
  hovered). `the_choose_blocks_fit_above_the_footer` and
  `the_footer_follows_the_page_and_is_pinned_only_on_overflow` guard this.
  On opening with nothing chosen, the screen chooses this machine's stick
  (Upgrade) as soon as a drive list shows it examined; a click on a drive
  ends that for the opening (`prefer_boot` in `upgrade.rs`).
  After an upgrade of this machine's stick the outcome stays when the screen
  is opened again, for Restart now; Write another stick goes back to Choose
  with no drive chosen. Any other outcome resets to Choose on opening.
- Not seen on hardware: two programs at once, Stop them and log out, the
  colours of a real `oxulnk-desktop` log, the menu staying behind a clicked
  program, the Restart and Power off tiles.

## A failed menu is shown and counted

`labwc` always exits 0, whatever its `-S` command returned (measured: a `-S`
command exiting 10, 0 or 101 all give `labwc` exit 0). `thinfloe-session`
therefore writes the menu's exit status to `$XDG_RUNTIME_DIR/thinfloe-session.status`
and, on any status but 0, opens a `foot` window with the status and the last
lines of the session log. `overlay/usr/libexec/thinfloe/session-verdict`, called
from `overlay/etc/profile.d/10-thinfloe-session.sh`, reads that file after labwc
returns. After three consecutive failed sessions, counted in
`/tmp/thinfloe-session-failures.<uid>`, it reports the status and
`10-thinfloe-session.sh` stops the restarts and shows the failure screen.

The session log also carries labwc's own line:
`[ERROR] [../src/server.c:167] spawned child 12 exited with 10`.

From a VT, `thinfloe-menu --check` lists every tile, why one is grey, and why
an icon draws its letter.

## The menu's log

The bottom 30 % of the menu (at least eight rows) is a log (four rows
while the Upgrade screen offers its choices; normal again once a write
starts):
`HH:MM:SS  source | text`, in local time. It shows every tile start and
exit (with the exit status or signal), each save and its outcome, and every
line a tile's program or `thinfloe-save-home` writes to stdout or stderr, with
the tile name or `save-home` as source. Program output is untrusted, so the
bounds are fixed in `src/thinfloe-menu/src/log.rs`: lines are cut at 1 KiB, the
log and the queue from the reader threads keep 500 lines each, and output
wakes the menu at most ten times a second. A program's last lines appear
before its exit line; a background child that keeps the pipes open delays
the tile by at most 300 ms, and its later lines are still logged. Colours a
program writes (SGR) are drawn; other escape sequences are removed. Every log
line also goes to stderr, as plain text without escape sequences, so the
session log has all of it.
