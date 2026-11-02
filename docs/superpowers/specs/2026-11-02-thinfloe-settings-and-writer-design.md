# ThinFloe: Settings tabs and the image writer in the menu

Date: 2026-11-02. Status: approved by the owner (2026-11-02).

The owner asked for this design without a question round ("skip the
questions, write the design"). What the owner said in chat is marked
*owner*; everything else is *my call*, and is open to change.

## Why

ThinFloe 4.0.0 is about to go public. Before that, the owner wants to invest
in usability (*owner*, 2026-11-02):

- the image updater must be part of the menu, not a shell script in a
  terminal;
- the user must see progress while a USB stick is written; today there is
  none at all;
- the write options must be chosen **before** the write starts, not asked
  one at a time while it runs;
- the Settings screen gets tabs: **Packages** (what Settings shows today),
  **Network** (network settings, WiFi included) and **Screen** (power-save
  timeout and the arrangement of several monitors).

Today a person who sets up a machine must edit `~/wifi.yaml`,
`~/.thinfloe-sessionrc` (`wlr-randr`, `TF_IDLE_TIMEOUT`) or
`~/.config/kanshi/config` by hand, from a terminal, with no keyboard
shortcuts to reach one. With zero labwc keybindings the menu is the only
way in, so the menu must be able to do these jobs.

## Success

- A person with no Linux knowledge can, from the menu alone: join a WiFi
  network, set when the screen blanks, arrange two monitors, upgrade this
  machine and write a new stick.
- While a stick is written, the screen shows which step runs, a bar that
  follows the real speed of the stick, and the time left.
- Nothing that works today breaks: a hand-written `~/wifi.yaml`,
  `~/.thinfloe-sessionrc` or kanshi config keeps working and is not
  overwritten by the menu.

## Non-goals

- 802.1X / enterprise WiFi, WiFi with certificates, captive portals.
  Hand-written `~/wifi.yaml` still covers them.
- Editing WireGuard (`~/wg0.conf`). The Network tab only shows its state.
- Mirrored monitors, per-output scale. labwc outputs **stay at scale 1**
  (*owner*, earlier decision).
- Lua or any other scripting in the menu (*owner* asked; my answer was no,
  see the chat of 2026-11-02).
- Moving `labwc-scale` into the menu. Worth doing, but separate.
- Rewriting `thinfloe-upgrade` or `thinfloe-addon` in Rust.

## Anti-goals

- The menu overwrites a file the user wrote by hand.
- A network change cuts a running remote session without a warning.
- A layout change leaves a person with no visible menu.
- The progress bar lies: 90 % after five seconds, then minutes of nothing.
- The menu runs as root, or gains root code inside its own process.

## Constraints (from the project)

- The menu runs as tfuser, draws on the CPU (no GPU libraries), and has
  zero labwc keybindings.
- Root work happens in separate tools behind `sudo -n`, with a status file
  and a log, the way `thinfloe-addon` does it. tfuser has `NOPASSWD:ALL`.
- Every slow call (drive list, WiFi scan, release check) runs off the UI
  thread; the menu must keep answering the compositor (see `jobs.rs`).
- Per-machine network files are installed by the **initramfs**
  (`install_local_network()`), never by a unit, because the netplan
  generator runs before any unit. A change the menu makes must therefore
  both take effect now and land in the home, so the next boot installs it
  the normal way.
- The home travels in the home archive. Settings changed in the menu are
  lost at a power cut until the next save.
- The ThinLinc client reads the monitor layout once, when it starts.
- Restart, Power off and Log out already list and wait for running add-on
  jobs. New root jobs join that list.

## Order of work (my call)

Each part gets its own plan and is finished before the next starts.

| part | what | depends on |
| --- | --- | --- |
| 0 | menu moves to egui 0.37 and the crates.io `shadcn-egui` 0.8; the copy in `src/thinfloe-menu/egui_shadcn/` goes | shadcn-egui 0.8.0 released (another session works on it) |
| A | the image writer in the menu | 0 |
| B | Settings tabs, and the Screen tab's power-save timeout | 0 |
| C | Network tab | B |
| D | Screen tab: monitor arrangement | B |

A comes first because it is the largest gap in feedback today, and because
the public release depends on it (the Upgrade tile with its daily check was
decided for 4.0.0 on 2026-10-21).

## Part 0: one component set

- `src/thinfloe-menu/Cargo.toml`: `egui`, `egui-winit`, `egui_extras` to
  0.37; `shadcn-egui = "0.8"` replaces `egui_shadcn = { path = … }`.
- `brand.rs` builds the ThinFloe theme with the crate's API: the palette as
  app colours, Geo and the plain sans through `Theme::apply_with_fonts`,
  Oxanium kept off headings. No colour or size literal in UI code (the
  skill's grep with its positive control must stay clean).
- Acceptance: `shot_desktop_and_settings_at_densities` renders the desktop
  and today's Settings screen at 1080p, 4K and 21-inch 4K, and the owner
  compares the pictures before and after. `make test` and `make
  test-runtime` stay green, and the "no GPU library" guard in
  `test-session-packages.sh` too.
- Components ThinFloe needs that the crate lacks (a row list like
  OxuLnk's folder rows, a radio group if used) are ported **into the
  crate** with `shadcn-egui:port-component`, not painted in the menu.

## Part A: the image writer

### What the person sees

The tile `50-upgrade-image.desktop` keeps its file name (user rewordings
match by file name) and becomes a menu action:
`X-TF-Action=upgrade`, honoured only in shipped files, like `settings`.
Its name becomes **Upgrade**. `Terminal=true` and `Exec=` go.

Pressing it shows the Upgrade screen in place of the tile grid, like
Settings; the log stays below. The screen has two stages.

**Stage 1: choose** (one page, nothing happens until the last button):

1. **Release.** Installed version, newest version available, and when it
   was checked. A *Check now* button. Under *Advanced*: the source address
   (default the release address; a custom URL stays possible for testing,
   my call on the open "builder's call" of 2026-10-21).
2. **Target.** One row per drive: model, size, and a tag: *this machine's
   stick*, *ThinFloe stick (version, has fallback / pending upgrade)*,
   *other drive*. A row that cannot be used is drawn but disabled, with the
   reason: *too small (needs N GB)*, *the boot drive of this machine
   (fresh install not possible)*. The list follows plug and unplug
   (looked at every 2 s, off the UI thread).
3. **Action**, offered per target as today:
   - this machine's stick: *Upgrade* (A/B, active after a restart);
   - another ThinFloe stick: *Upgrade* or *Erase and install*;
   - any other drive: *Erase and install*.
4. **Options**, only those that apply:
   - *Copy this machine's home directory* (fresh install). A line under it
     says what is always left out (SSH host keys, WireGuard key) and that
     the remote-access password travels. Disabled with the reason when
     this boot's home restore failed.
   - *Check the stick after writing* (fresh install; on by default, my
     call).
5. **Summary and one confirmation.** A sentence that names the target
   ("Erase SanDisk Cruzer, 16 GB, and install ThinFloe 4.1.0"). For an
   erase the button uses the destructive variant and the drive's model and
   size are on the button.

**Stage 2: progress.** A list of the steps with a mark each (waiting,
running, done, failed), and for the running step a bar, MB/s and time
left:

| fresh install | upgrade |
| --- | --- |
| Downloading and writing (one bar: bytes written of the image size) | Saving the home directory (boot stick only) |
| Checking the stick (optional) | Downloading `vmlinuz`, `initrd.img`, `ramroot.sqsh` (one bar over the three) |
| Marking the stick (stick id) | Checking the files |
| Copying the home directory (optional) | Switching to the new release (`tl.new`, `DEFAULT new`) |
| Saving this machine's home | |

The end shows the result in one sentence and the next action: *Restart
now* (upgrade of this stick), *You can remove the stick* (another stick),
or the failure with *Try again* and the reason.

A **Cancel** button works during download and write. On a fresh install
it says first that the stick will be unusable until it is written again.

While a write job runs, Restart, Power off and Log out list it by name
("Writing SanDisk Cruzer") and wait for it, as for add-on jobs. *Restart
anyway* on a fresh install warns that the stick will be unusable.

### The daily check and the tile marker

Decided 2026-10-21 and kept: once a day, randomised within the day, a timer
runs `thinfloe-upgrade check`, which reads the small version file of the
newest release and writes the result to `/run/thinfloe/upgrade/status`.
When it is newer than `/etc/thinfloe/version`, the tile shows the update
marker and the name *Upgrade to X.Y.Z*. USB machines only; the tile is
hidden on netboot. Pressing the tile checks again.

This needs Part 5's version file (`/etc/thinfloe/version` and the release's
version asset). Until Part 5 lands, the screen says *version unknown* and
the tile shows no marker; nothing else waits for it.

### The root tool

`thinfloe-upgrade` stays the root tool, in Python, with its tests
(`test/unit/test_upgrade_image.py`). It gains a batch interface the menu
calls with `sudo -n`; the interactive mode stays for use over SSH (my
call).

- `thinfloe-upgrade drives` prints the drive list as JSON: device, model,
  size in bytes, ESP, ThinFloe version on it, `tl.old`/`tl.new`, boot
  drive, and whether it is large enough for the image.
- `thinfloe-upgrade check` updates the status file (above).
- `thinfloe-upgrade write --device DEV --mode fresh|upgrade
  [--source URL] [--copy-home] [--verify]` runs with no questions. It
  refuses, with exit 2 and a reason, anything the menu should not have
  offered (boot drive and `fresh`, a device that changed since `drives`
  by model or size, too small).
- Progress goes to stdout as lines the menu parses and does **not** put in
  the log; every other line goes to the log as today:

  ```
  @step <id> <text>
  @progress <bytes done> <bytes total or ->
  @done <id>
  @result ok|failed <one-sentence reason>
  ```

  Exit codes: 0 done, 1 failed (reason in `@result` and the log), 2
  misuse, 130 cancelled (SIGTERM).
- The menu runs the job in a process group of its own, as for add-on jobs,
  so Cancel and *anyway* can signal it.

### Progress that tells the truth

- Fresh install: the total is the uncompressed size from the zstd frame
  header, which the tool already reads for its space check. Without it
  (no range support), the bar shows bytes written without a percentage.
- The write must follow the stick, not the page cache: the tool writes in
  blocks and calls `fdatasync` every 32 MiB, or opens the device with
  `O_DIRECT` (builder's call, measure both on a real stick). It does not
  use `dd`.
- The check after writing reads the image size back from the stick and
  compares its SHA-256 with the hash computed while writing, so it needs
  no second download.
- The download checks the release checksum file (Part 5). A mismatch on a
  fresh install fails the job after the write and says the stick must be
  written again.
- Upgrade: the total is the sum of `Content-Length` of the three files.

## Part B: Settings tabs and power-save

### The tabs

The Settings screen gets a tab bar with **Packages**, **Network** and
**Screen** (*owner*). Packages is today's add-on list, unchanged in
behaviour. The screen keeps the height of its tallest tab, so it does not
jump when the tab changes (OxuLnk's `tallest_inactive_tab`). Rows follow
OxuLnk's folder rows (`oxulnk-desktop/src/settings_ui.rs`, `folders_tab`):
icon, label, controls on the right, an error line below. Label and control
pairs use a form row with a fixed label width.

### Who owns which file (my call, applies to B, C and D)

The menu only writes files it owns. A file it owns starts with the line
`# Written by the ThinFloe menu. Edits here are replaced.` A file without
that line was written by hand: the tab shows its settings read-only with
*This is set by hand in ~/<file>*, and a button *Let the menu manage it*,
which renames the file to `<file>.by-hand` and starts from its values
where it can read them.

After a change the menu queues a background home save, the same queue the
Save-On-Exit tiles use, so the change survives a power cut.

### Power-save

- A select: *never, 1, 2, 5, 10, 15, 30, 60 minutes*; default 5 (today's
  300 s).
- Stored as `idle_timeout=<seconds>` in `~/.config/thinfloe/screen`, owned
  by the menu.
- `TF_IDLE_TIMEOUT` in `~/.thinfloe-sessionrc` still wins. When it is set
  there, the select is disabled with *set in ~/.thinfloe-sessionrc*.
- It takes effect at once: the menu starts `swayidle` itself and restarts
  it on a change; `thinfloe-session` no longer starts it (my call). The
  `wlopm` command and the rule *never `wlr-randr --off`* stay.
  `test-session-packages.sh` follows the move.

## Part C: Network

### What the tab shows

- **Connections now**: one row per interface (wired, WiFi, WireGuard
  `wg0`) with state, address, and for WiFi the network name and signal.
  Read-only, refreshed every 2 s while the tab is open.
- **Wired**: *Automatic (DHCP)* or *Fixed address* with address/prefix,
  gateway and DNS servers (IPv4). Default automatic, as today.
- **WiFi networks**: the saved networks, each a row with its name and
  *Forget*; *Add network* opens a dialog with the networks in range (scan,
  strongest first, with a lock for protected ones), a password field and
  *Hidden network* (name typed). Open, WPA2-PSK and WPA3-SAE.
- **Apply**. Network changes are collected and applied together, because
  applying restarts the network. While ThinLinc or OxuLnk runs, Apply asks
  first: *The remote session may disconnect for a few seconds.*

### Files and root work

- WiFi goes to `~/wifi.yaml`, the path the initramfs already installs.
  The menu writes it with an interface `match` for wireless devices rather
  than a device name, so the file works on any machine it is copied to.
  The builder must check that netplan's generator still creates the
  `netplan-wpa-*` unit for a `match`ed WiFi definition; if it does not,
  the root tool fills in the device name found in
  `/sys/class/net/*/wireless` when it applies.
- A fixed wired address goes to `~/ethernet.yaml`, and
  `install_local_network()` installs it over `/etc/netplan/ethernet.yaml`
  when it exists (one more file next to `wifi.yaml`; the same
  symlink refusal and mode 600). Back to *Automatic* removes it.
- A new root tool `thinfloe-network` (Python, like `thinfloe-addon`), called
  with `sudo -n`:
  - `status` prints the interfaces as JSON;
  - `scan` runs `iw dev <if> scan` and prints the networks as JSON;
  - `apply` checks the home files with `netplan generate` into a scratch
    root, copies them to `/etc/netplan` (mode 600), and runs
    `netplan apply`. A file that does not pass `netplan generate` is not
    applied, and the reason goes to the log.
- The WiFi password is stored as typed, as in today's `~/wifi.yaml`. The
  tab says under the password field that it is saved with the home
  directory, readable by anyone holding the stick. This is the same
  exposure as the SSH host keys, accepted before.

## Part D: monitor arrangement

- The tab draws the connected monitors as boxes in their real proportions,
  each with its output name and model. One is selected; for it the tab
  offers: *On/Off*, *Resolution* (the modes the monitor offers),
  *Rotation* (normal, 90, 180, 270), and *Position* relative to another
  monitor (*left of, right of, above, below*, aligned at the top or left).
  No dragging in the first version (my call); a drag editor can follow.
- The arrangement is stored as a kanshi profile in
  `~/.config/kanshi/config`, owned by the menu, one profile per set of
  connected monitors. kanshi applies it again on hotplug and after DPMS
  wake. `thinfloe-session` starts kanshi when that file exists, after
  `.thinfloe-sessionrc` (so a `wlr-randr` line there still runs first).
  A change is applied with `kanshictl reload` (builder: confirm the
  image's kanshi has it, else restart kanshi).
- The menu reads the monitors and their modes through
  `zwlr_output_manager_v1` (`wayland-protocols-wlr` is already a
  dependency), never by parsing `wlr-randr` text.
- **Menu monitor** (*owner*, 2026-11-02): the person chooses on which
  monitor the menu sits, with *Show the menu here* on the selected
  monitor. It is stored with the arrangement, per set of connected
  monitors, so a laptop alone and the laptop at its dock each keep their
  own choice. Without a choice, or when the chosen monitor is not
  connected, labwc places the menu as today.
  - How (my call): the menu is a maximized toplevel, and labwc picks its
    output when the window maps. `thinfloe-compose-labwc-config` already
    composes the config directory labwc reads; it gains a generated
    `rc.xml` whose `thinfloe-menu` window rule adds
    `<action name="MoveToOutput" output="NAME"/>` before the existing
    `ToggleAlwaysOnBottom`, then maximizes. On a change the menu
    rewrites it, runs `labwc --reconfigure`, and unmaps and maps its
    window so the rule runs again. The zero-keybindings rule and its
    test cover the generated `rc.xml` too.
  - Builder: confirm the image's labwc 0.9.3 accepts `MoveToOutput` with
    an `output` attribute in a window rule, and that an unmap and map of
    a winit window runs the rule again. If either fails, say so before
    choosing another way (a layer-shell surface on the chosen output
    would change how the menu stays behind other windows, and is the
    owner's decision).
  - The density scale follows on its own: the menu already rereads its
    scale when it lands on another output.
- **Safety:** after Apply, a dialog *Keep this arrangement?* counts down 15
  seconds and goes back to the previous one unless confirmed. The monitor
  that shows the menu cannot be switched off; to switch it off, the person
  first moves the menu to another monitor.
- While a ThinLinc client runs, the tab says that ThinLinc uses the new
  arrangement from its next start.

## Testing

- Part 0: the render test pictures, compared by the owner; the grep for
  literals with its positive control.
- Part A: `test/unit/test_upgrade_image.py` for the batch interface, the
  refusals and the progress lines (a fake device file, a local HTTP
  server); menu unit tests for the protocol parser and the state machine
  of the Upgrade screen; a new `make qemu-smoke-write`: boot from one
  disk, run `thinfloe-upgrade write --mode fresh --verify` to a second,
  empty disk from the namespace HTTP server, then boot the second disk and
  require a clean boot and its own stick id.
- Part B: unit tests for the timeout file and the precedence of
  `TF_IDLE_TIMEOUT`; `test-session-lifecycle.sh` for who starts swayidle.
- Part C: unit tests for the YAML the menu writes (with
  `netplan generate --root-dir` in the image container, as
  `test-addon-deps.sh` uses the image); `test-initramfs-home.sh` for the
  `ethernet.yaml` install. WiFi association: hardware only.
- Part D: unit tests for the kanshi profile text from a layout and for the
  relative-position rules; the runtime test (`test-labwc-runtime.sh`) has
  headless outputs and can check that kanshi applies a profile, and that
  the menu maps on the chosen output of two headless outputs, also after
  a change.

## Settled after the builder check

A reader with no part in the conversation listed what they would still have
to guess. The answers, all my calls unless marked:

1. **Version and checksum contract.** Defined in
   `docs/superpowers/specs/2026-10-21-thinfloe-design.md`, §"Part 5: GitHub
   releases" (version source `CHANGES.md`, `/etc/thinfloe/version`, a
   version asset and one checksum file in the release, read from
   `https://github.com/oposs/thinfloe/releases/latest/download/`). Part A
   plans against that contract. Until Part 5 is built, `check` reports
   *version unknown*, and the checksum step is skipped with a line in the
   log; its test uses a local HTTP server that serves the Part 5 files.
2. **The shipped `~/wifi.yaml` template** counts as *no file*, not as a
   hand-written one: a file that contains `device name of your wifi card`,
   is empty, or holds only comments, may be replaced without asking. The
   menu's own file never contains that marker string (the initramfs skips
   files that do). A file with the menu's header line belongs to the menu
   even when someone edited it later, as the header says.
3. **WiFi device name.** The first task of Part C is to check whether
   netplan creates `netplan-wpa-*` for a `match`ed WiFi definition. If it
   does not, the menu writes the device name (from
   `/sys/class/net/*/wireless`) into `~/wifi.yaml` itself, so the file is
   complete at the next boot too. When a home moves to a machine whose
   WiFi device has another name, the Network tab shows *WiFi adapter
   changed* and rewrites the file on Apply.
4. **swayidle stays with the menu.** When the menu crashes,
   `thinfloe-session` starts it again at once, so blanking stops only for
   that moment; on the failure screen, no blanking is fine.
   `thinfloe-session` passes the value from `.thinfloe-sessionrc` to the
   menu in the environment (`TF_IDLE_TIMEOUT_RC`), so the menu never parses
   a shell file.
5. **`thinfloe-upgrade`.** The write path is a rewrite (a hashing writer
   replaces the `dd` pipe), and `test/unit/test_upgrade_image.py` may be
   reshaped; every behaviour it guards today (space check, identity
   excludes, stick stamp, refusals) stays guarded. The interactive mode
   uses the same code underneath. The home save in an upgrade of the boot
   stick stays in `thinfloe-upgrade`, as `save_home_before_finish` does
   today, and shows as its own step.
6. **Waiting for shadcn-egui 0.8.** The root-tool work of Parts A and C
   (`thinfloe-upgrade` batch mode, `thinfloe-network`) and the kanshi text
   generation of Part D touch no UI and may start before Part 0. All menu
   screens wait for Part 0.
7. **Menu monitor check first.** The first task of Part D checks
   `MoveToOutput` and the unmap and map in `test-labwc-runtime.sh`. If it
   fails, *Show the menu here* leaves Part D's first plan, the rest of
   Part D goes ahead, and the owner decides the alternative.
8. **kanshi is the only one that sets monitors.** The menu reads monitors
   through `zwlr_output_manager_v1` but never applies a configuration
   itself. Apply and the 15-second revert both rewrite
   `~/.config/kanshi/config` and reload kanshi. A monitor is identified as
   kanshi does it: make, model and serial; when the serial is missing,
   the output name.
9. **`~/ethernet.yaml`** is the shipped `/etc/netplan/ethernet.yaml` with
   only the addressing changed: id `eth`, `match: name: 'en*'`,
   `optional: true` and the time-zone drop-in for `eth` all stay.
10. **Root jobs in the menu.** The write job and the network apply join the
    same list as add-on jobs; whether `jobs.rs` gets a general *root job*
    type or not is the builder's call. *Restart anyway* during a fresh
    install warns and is allowed, as written above.

## Left to the builder

- `O_DIRECT` or periodic `fdatasync`, measured on a real stick.
- The exact JSON fields of `drives`, `status` and `scan`.
- How often the release check retries after a failure.
- Whether `thinfloe-network` and `thinfloe-upgrade` share a small Python
  module for the status-file and log handling `thinfloe-addon` already
  has.

## Not verified, to check on hardware

- Write speed and the truth of the bar on real sticks.
- WiFi association from a menu-written `~/wifi.yaml`, at once and after a
  restart, on the Intel 8265.
- kanshi on real monitors after hotplug and DPMS wake.
