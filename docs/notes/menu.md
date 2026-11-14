This note covers how the tile menu works: the window rules, tiles and icons, the Upgrade screen, density, monitor arrangement, its log, failure handling and its known limits.

## Window rules and folded windows

- Session/compositor: `overlay/etc/thinfloe/labwc/` (`rc.xml`, `environment`) - kept outside `$HOME` because home is captured/restored wholesale by the persistence machinery, see [Persistent Home Directory](home-persistence.md).
  `rc.xml` also holds the window rule that keeps the window with app_id
  `thinfloe-menu` at the bottom (`ToggleAlwaysOnBottom`) and out of the
  window list that feeds the menu's folded-window strip (`skipTaskbar`).
- No window list: waybar and GTK 3 left the image. labwc draws an iconify button
  and, with zero keybindings, a minimized window needs a way back: it shows
  as a chip in the folded-window strip at the bottom of `thinfloe-menu`, and
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
  `thinfloe-network apply` puts a change in force at once.

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
  address. `generated/ethernet.yaml` is not installed there; a `[network]`
  that carries a stick's fixed address still refuses WiFi changes ("A fixed
  address cannot be set on netboot."). The menu writes `[network]` only after
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
