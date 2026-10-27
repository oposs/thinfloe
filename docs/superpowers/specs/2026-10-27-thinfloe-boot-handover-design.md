# Boot screen until the menu, with a step line — design

Date: 2026-10-27. Owner decisions are marked *(owner)*; the rest is the
designer's call and may be changed by the builder where noted.

## Intent

Two things a person sees at the end of every ThinFloe boot:

1. **The text console flashes up.** When `plymouth-quit.service` runs,
   Plymouth ends and tty1 returns to text mode: old boot lines
   (`thinfloe: ramroot=tl/ramroot.sqsh`), then getty's banner,
   `login: tfuser (automatic login)` and Ubuntu's welcome text, for 2 to 3 s
   until labwc draws the menu (QEMU capture, 2026-10-27:
   splash until 13.0 s, text until 15.0 s, menu at 15.5 s).
   `plymouth quit --retain-splash` alone does not help: the console still
   draws over the picture (tried and measured, same day).
2. **The person cannot tell whether the machine is still working.** The
   progress line follows the elapsed time only; during a long step (add-on
   installs, up to 100 s; a netboot download) nothing says what is going on.

Goal *(owner)*: the boot picture stays until the menu draws, and a line under
the progress bar tells the person *that* the machine is working and *roughly
what* it is doing.

## Goals

- No text console between the boot picture and the menu on a normal boot
  (`LABEL current`, `quiet splash`).
- A step line under the progress bar, in English *(owner)*, short plain
  words. It may change often *(owner)*.
- It shows ThinFloe's own steps, naming the add-on during installs,
  e.g. "Installing ThinLinc" *(owner)*.
- It also shows what systemd starts, translated to plain words
  ("Starting the network", "Starting SSH") *(owner)*.
- If the session cannot start, the picture stays until the failure screen
  replaces it *(owner)*, and the failure screen is readable as today.

## Non-goals

- Errors on the step line. Failures stay in the menu log, the session log
  and the failure screen.
- Changes to the `console`, `verbose` and `debug` boot entries. They pass
  `plymouth.enable=0`; nothing changes for them.
- A branded syslinux screen (discussed, not wanted for 4.0).
- Fixing the single black frame when the firmware display hands over to the
  real GPU driver (seen at 9.0 s in QEMU); separate topic.

## Anti-goals (a failure even if it "works")

- Raw unit names, file paths or kernel messages on the boot screen.
- A boot that hangs on a picture forever with no way to see why. Every path
  must end in either the menu or the readable failure screen.
- Breaking the labwc start: labwc cannot take DRM master while Plymouth
  holds it, which is why getty waits for Plymouth today
  (`overlay/etc/systemd/system/getty@tty1.service.d/autologin.conf`).
- Weakening `thinfloe-wait-kms.service` and the one labwc retry in
  `10-thinfloe-session.sh` (the i915 coldplug race, 273 ms on a NUC7i3BNK).
- Any new `Before=` on a sysinit.target unit (Standing Constraint).

## Constraints

- Initramfs: only tools from `inst_multiple` in `90thinfloe/module-setup.sh`
  plus dracut's base set. The `plymouth` client is there already
  (`tf_write_hostname` uses `plymouth --ping` and `display-message`); every
  call must stay guarded the same way (`command -v plymouth && plymouth
  --ping`) and never fail a hook. `test-initramfs-commands.sh` checks the
  commands the hooks call.
- The host name keeps its own foot line through `display-message`; the step
  line uses a different channel (`plymouth update --status=`).
- Plymouth script language: no regex; string methods are limited (check
  `script-lib-string.c` for `CharAt`/`SubString`/length before relying on
  them). Sources of the shipped version are in
  `/scratch/oetiker/claude-tmp/thinfloe-tests/bug2/src/` (partial; fetch the
  rest of plymouth 24.x as needed).
- Ubuntu's `getty@.service` template has `After=plymouth-quit-wait.service`,
  and a drop-in cannot remove an `After=`.
- `plymouth-quit.service` (and `plymouth-quit-wait.service`) are pulled in by
  `multi-user.target`; left alone, `plymouth-quit.service` ends Plymouth at
  `systemd-user-sessions` time, before the session.
- No polkit; tfuser escalates with `sudo -n` (NOPASSWD).
- Max 4 cores; QEMU tests only when no build runs.

## How it works (designer's call unless marked)

### The step line

- **Channel.** Every text reaches the theme through Plymouth's update
  status (`Plymouth.SetUpdateStatusFunction` in `thinfloe.script`).
- **ThinFloe's own steps** are sent as `plymouth update --status="tf:<text>"`.
  The script shows the text after `tf:` as it is.
- **systemd's steps.** systemd sends the id of each unit it starts
  (e.g. `systemd-networkd.service`) through the same channel *(builder:
  verify in systemd's `manager_send_unit_plymouth`)*. The script looks the
  id up in a table and shows the plain text; an id not in the table changes
  nothing and the previous text stays. Starting table (builder may extend,
  keep it short):

  | unit | text |
  | --- | --- |
  | `systemd-udevd.service` | Detecting hardware |
  | `systemd-networkd.service` | Starting the network |
  | `systemd-resolved.service` | Starting name lookup |
  | `chrony.service` | Setting the clock |
  | `wpa_supplicant*`, `netplan-wpa-*` | Connecting to WiFi |
  | `wg-quick@wg0.service` | Starting WireGuard |
  | `ssh.service`, `thinfloe-ssh-hostkeys.service` | Starting SSH |
  | `thinfloe-wait-kms.service` | Waiting for the graphics driver |
  | `thinfloe-addons.service` | Installing add-ons |

- **ThinFloe's steps and where they are sent:**

  | text | sent from |
  | --- | --- |
  | Finding the boot stick | `tf_find_esp` (only while it waits) |
  | Loading the system | `mount-squashfs.sh` (netboot: before the download) |
  | Restoring the home directory | `restore_home` |
  | Installing <Name> | `thinfloe-addon boot`, per add-on, the tile's `Name` |
  | Starting the session | `10-thinfloe-session.sh`, before `deactivate` |
  | Waiting for the graphics driver | `10-thinfloe-session.sh`, before the retry |

  `thinfloe-addon` is Python and runs as root; it sends the status only when
  `plymouth --ping` succeeds and ignores every error.
- **Look.** One line, Geo, centred, the foot line's grey (#8a97a6), between
  the progress line and the foot line, about 13 px at 1080p scaled by `k`
  like the foot line. Text longer than fits is cut, never wrapped.

### The hand-over to the menu (GDM's model)

1. **getty no longer waits for Plymouth.** tty1 gets its own unit file,
   `/etc/systemd/system/getty@tty1.service` (an instance file overrides the
   template), with the template's content minus
   `After=plymouth-quit-wait.service`, plus the autologin settings from
   today's drop-in, plus `After=thinfloe-wait-kms.service`. The drop-in is
   deleted and `plymouth-quit.service` is masked, so nothing quits Plymouth
   before the session does (see "Decisions from the builder check"). Plymouth holds tty1 in graphics mode, so the banner and the welcome
   text go to an invisible screen. *(Builder: confirm Ubuntu's
   `getty@.service` content in the built image before copying it.)*
2. **The session sends "Starting the session" and runs `sudo -n plymouth
   deactivate`** in `10-thinfloe-session.sh`, right before `tf_run_labwc`.
   Plymouth stops drawing and drops DRM master; the picture stays.
3. **labwc takes the display.** `thinfloe-session` (running inside labwc)
   runs `sudo -n plymouth quit --retain-splash` as its first act. The menu
   draws over the retained picture.
4. **labwc does not start.** Before the retry, the session runs `sudo -n
   plymouth reactivate` and sends "Waiting for the graphics driver", so the
   bar moves again during the wait (up to 20 s). Before the second
   `tf_run_labwc` it deactivates again.
5. **The retry fails too.** Before printing the failure screen, the session
   runs `sudo -n plymouth quit` (no retain), which returns tty1 to text mode;
   then the failure text prints as today, readable. The picture stays until
   that moment *(owner)*.
6. **Every plymouth call is guarded** (`plymouth --ping` first, errors
   ignored), so a boot without Plymouth behaves as today.

### Safety net

A boot must never hang on the picture with no way to see why (anti-goal).
If getty or the login never reaches `10-thinfloe-session.sh`, nothing would
quit Plymouth. A fallback quits it: a oneshot ordered after
`getty@tty1.service` with a timer, `thinfloe-plymouth-fallback.timer`,
`OnBootSec=300`, running `plymouth quit` (no retain) only if Plymouth still
answers `--ping`. 300 s covers the add-on budget (120 s), the two 20 s KMS
waits and a slow netboot download. *(Builder may choose a different
mechanism with the same effect; it must not add a `Before=` on a
sysinit.target unit.)*

## Spike first (designer's call, after the builder check)

The central claim, that the picture stays unbroken across `plymouth
deactivate` → labwc's first modeset → `plymouth quit --retain-splash`, is
**not tested**. Only `quit --retain-splash` on its own was tried, and the
console drew over the picture. `deactivate` may also return tty1 to text
mode. The plan therefore starts with a throwaway QEMU spike on the capture
rig: the hand-over steps 1 to 3 wired up by hand, with frames captured at 0.5 s
or finer. It must also answer:

- what systemd sends through the update-status channel, the unit id or the
  description (decides how the table is keyed; the table then matches
  whatever it is);
- whether `sudo -n plymouth --ping/deactivate/quit` from the tfuser session
  reaches the daemon that the initramfs started.

Only if the spike shows splash → menu with no text frame is the rest built.
If it fails, stop and bring the frames to the owner; the alternative designs
(e.g. keeping Plymouth alive until labwc has drawn) are a new decision.

## Decisions from the builder check (designer's calls)

- **plymouth-quit.service is masked**, not cancelled with `Conflicts=`.
  Masking is deterministic; how a `Conflicts=` between two jobs pulled in by
  `multi-user.target` resolves is not. `plymouth-quit-wait.service` stays.
  Note: `autovt@.service` (tty2 and up) is ordered after it, so a VT getty
  appears only once Plymouth has quit. That happens on every path (next
  point), at the latest by the fallback.
- **Every path the session takes ends Plymouth.** `10-thinfloe-session.sh`
  runs a guarded `plymouth quit` (no retain) after every `tf_run_labwc`
  return, whenever Plymouth still answers `--ping`. This covers labwc or the
  menu dying before `thinfloe-session` sent its `quit --retain-splash`, and a
  session restart after a failed menu. It runs before `session-verdict`, the
  failure window and the failure text.
- **The fallback timer counts from boot**, `OnBootSec=300`, not from
  `multi-user.target`, so a late `multi-user.target` (add-on installs take up
  to 100 s) cannot push it further out.
- **The drop-in `getty@tty1.service.d/autologin.conf` is deleted** when the
  instance file `getty@tty1.service` is added, so the old ordering does not
  come back through it. Nothing else reads it (builder: grep to confirm).
  The instance file pins the template's content; a later change Ubuntu makes
  to `getty@.service` is not picked up. That is accepted; the file names the
  template version it was copied from.
- **The smoke tests keep passing unchanged**: `qemu-smoke`,
  `qemu-smoke-netboot`, `qemu-smoke-addons` (add-ons installed before
  `getty@tty1.service` starts, which `thinfloe-addons.service`'s own
  `Before=getty@tty1.service` keeps), `qemu-smoke-upgrade`,
  `qemu-smoke-two-sticks`, and `check-boot-smoke.sh` (no ordering cycle).
- **The netboot download gets no progress figure**, only "Loading the
  system".

## Verification

- **Offline guards** (`make test`):
  - `test-initramfs-commands.sh`: the theme registers an update-status
    function and handles the `tf:` prefix; the initrd hooks guard every
    `plymouth` call.
  - `test-session-packages.sh`: `getty@tty1.service` in the image has no
    `After=plymouth-quit-wait.service`, has the autologin line, no
    `getty@tty1.service.d/autologin.conf` exists, `plymouth-quit.service` is
    masked, and the fallback timer is enabled.
  - `test-session-lifecycle.sh` (stubbed `plymouth`): normal start runs
    `deactivate` before labwc; failure runs `reactivate` before the retry
    and `quit` (no retain) before the failure text; every labwc return is
    followed by a guarded `quit`.
- **QEMU** (capture rig in
  `/scratch/oetiker/claude-tmp/thinfloe-tests/bug2/capture.py`, 1280x800 and
  4K): every frame from the first splash frame to the menu shows either the
  splash or the menu, no text console; the step line shows at least
  "Restoring the home directory" and "Starting the session".
- **QEMU failure case:** a boot with no usable KMS driver (e.g.
  `modprobe.blacklist=bochs` on `-device VGA`) shows the picture with
  "Waiting for the graphics driver", then the readable failure screen.
- `scripts/check-boot-smoke.sh`: no ordering cycle.
- **Hardware** (owner): the hand-over on the NUC and the 4K screen.

## Left to the builder

- Exact table entries and wording beyond the starting set, within "short,
  plain, English".
- Font size and vertical position of the step line, within the look above.
- Whether `thinfloe-session` or the menu sends the final `quit
  --retain-splash` (it must run after labwc owns the display).
- The fallback mechanism, within the safety-net rules.

## Documentation

- `CLAUDE.md`: replace the plymouth ordering paragraph of the autologin
  section and the boot-screen bullet; record the hand-over and the fallback
  as a Standing Constraint (getty must not go back to waiting for
  `plymouth-quit-wait`, the session owns `deactivate`/`quit`).
- `CHANGES.md`: one entry for the step line, one for "no text console
  before the menu".
