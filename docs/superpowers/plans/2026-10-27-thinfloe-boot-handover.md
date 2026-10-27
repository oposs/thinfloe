# Boot Screen Until the Menu, with a Step Line — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the Plymouth picture up until the menu draws, with no tty1 text console in between, and show a plain-English step line under the progress bar.

**Architecture:** One root helper, `/usr/libexec/thinfloe/plymouth-step`, wraps every Plymouth call of the running system (guarded, always exit 0); the initramfs has its own `tf_status` in `thinfloe-lib.sh`. The theme shows `tf:`-prefixed statuses as they are and maps systemd's unit messages through a table. tty1's getty stops waiting for Plymouth; the session deactivates Plymouth before labwc and quits it afterwards (GDM's model), with a fallback timer.

**Tech Stack:** Plymouth 24.x script plugin, systemd units, POSIX sh (initramfs: only `inst_multiple` tools), bash, Python 3 (`thinfloe-addon`).

**Spec:** `docs/superpowers/specs/2026-10-27-thinfloe-boot-handover-design.md` (read it; this plan argues from it).

## Global Constraints

- Step texts are English, short and plain; no unit names, paths or kernel messages on screen.
- Own statuses are sent as `plymouth update --status="tf:<text>"`.
- Every Plymouth call is guarded (`plymouth --ping` first), ignores errors and never fails its caller.
- Initramfs: only tools from `inst_multiple` in `90thinfloe/module-setup.sh` plus dracut's base set; `test-initramfs-commands.sh` must stay green.
- tfuser escalates with `sudo -n`; there is no polkit.
- No new `Before=` on a sysinit.target unit; `check-boot-smoke.sh` must find no ordering cycle.
- The `console`, `verbose`, `debug` entries (`plymouth.enable=0`) behave as today.
- Builds: one `make` at a time under `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock`, 4 cores; no QEMU while a build runs. Tests use `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`.
- Every existing smoke test keeps passing unchanged: `qemu-smoke`, `qemu-smoke-netboot`, `qemu-smoke-addons`, `qemu-smoke-upgrade`, `qemu-smoke-two-sticks`.

## Review Focus

1. **labwc or the menu dies after `deactivate`** but before `quit --retain-splash`: Plymouth must be quit (no retain) before `session-verdict` and the failure window — test in Task 4.
2. **A boot without Plymouth** (`plymouth.enable=0`, or no daemon): every helper call is a silent no-op and the session starts as today — test in Task 4 (stub `plymouth --ping` failing) and Task 2.
3. **A step text with characters the shell or Plymouth treats specially** (an add-on `Name` with quotes, `$`, or 200 characters): sent as one argument, cut to 64 characters, never executed — test in Task 3.
4. **An unknown systemd unit id** arrives: the line keeps its previous text — guard in Task 1.
5. **Logout and session restart** (exit 0, getty starts a new session): Plymouth is long gone; `plymouth-step` must not hang or print — test in Task 4 (second run with `--ping` failing).

---

### Task 0: Spike (throwaway) — does the picture survive the hand-over?

**Files:** a throwaway worktree only, e.g. `/scratch/oetiker/claude-worktrees/dbrrg-spike` (branch `spike-handover`, never merged), built with `make VERSION=spike image`.

- [ ] **Step 1:** In the spike worktree, by hand: add `overlay/etc/systemd/system/getty@tty1.service` (copy of the image's `/usr/lib/systemd/system/getty@.service` without `After=plymouth-quit-wait.service`, with the autologin `ExecStart` and `After=thinfloe-wait-kms.service`), delete `getty@tty1.service.d/autologin.conf`, `systemctl mask plymouth-quit.service` in the Dockerfile; in `10-thinfloe-session.sh` before `tf_run_labwc`: `sudo -n plymouth update --status="tf:Starting the session"; sudo -n plymouth deactivate`; first line of `thinfloe-session` body: `sudo -n plymouth quit --retain-splash`. Add a temporary `Plymouth.SetUpdateStatusFunction` to the theme that draws the raw status string (to learn what systemd sends).
- [ ] **Step 2:** Build, then capture with the rig (`/scratch/oetiker/claude-tmp/thinfloe-tests/bug2/capture.py`, `CAP_VGA="VGA,xres=1280,yres=800,edid=on"`, 40 s; and 4K with `CAP_FW`/`vars4k.fd`). Classify frames with `/scratch/oetiker/claude-tmp/thinfloe-tests/bug3/classify.py`.
  Pass: every frame from the first splash frame to the first menu frame is splash or menu; no text console frame.
- [ ] **Step 3:** Record in the plan's "Spike result" section below: pass/fail with frame names, what systemd sends (unit id like `systemd-networkd.service`, or the description), and whether `sudo -n plymouth --ping` from the session succeeded (session log).
- [ ] **Step 4:** If it fails: stop, show the owner the frames. Do not continue to Task 1. If it passes: remove the spike worktree and branch (ask the owner first).

**Spike result:** _(fill in)_

### Task 1: Step line in the theme

**Files:**
- Modify: `overlay/usr/share/plymouth/themes/thinfloe/thinfloe.script`
- Test: `test/integration/test-initramfs-commands.sh` (theme section, after the logo guard)

**Interfaces:**
- Produces: the theme shows any status `tf:<text>` as `<text>`; systemd statuses through the table below. Later tasks only send statuses.

- [ ] **Step 1: Write the failing guard** in `test-initramfs-commands.sh`: on the initrd's `thinfloe.script` (`$pscript`), require `Plymouth.SetUpdateStatusFunction(` and the literal `"tf:"`; message `ok "the boot screen shows a step line"` / `bad "the boot screen has no step line"`.
- [ ] **Step 2: Run it** against the current build: `bash test/integration/test-initramfs-commands.sh` → `FAIL - the boot screen has no step line`.
- [ ] **Step 3: Implement** in `thinfloe.script`: `fun update_status(status)` registered with `Plymouth.SetUpdateStatusFunction`. If status starts with `tf:` show the rest; else look the status up in a table (keys as found in the spike) and show the mapped text; no match: change nothing. Table (spec): `systemd-udevd.service` → Detecting hardware; `systemd-networkd.service` → Starting the network; `systemd-resolved.service` → Starting name lookup; `chrony.service` → Setting the clock; `wpa_supplicant*`/`netplan-wpa-*` → Connecting to WiFi (prefix match); `wg-quick@wg0.service` → Starting WireGuard; `ssh.service`, `thinfloe-ssh-hostkeys.service` → Starting SSH; `thinfloe-wait-kms.service` → Waiting for the graphics driver; `thinfloe-addons.service` → Installing add-ons. Look: one line, `"Geo " + size` with size `Math.Int(13 * k)` (min 9), colour 0.541, 0.592, 0.651, centred, y midway between the bar's bottom and the foot line; text longer than 64 characters is cut. Use only string methods present in `script-lib-string.c` of the shipped Plymouth.
- [ ] **Step 4: Rebuild and run the guard** (`flock … make image`, then the test) → `ok - the boot screen shows a step line`, `PASSED`.
- [ ] **Step 5: Commit** `boot screen: a step line under the progress bar`.

### Task 2: Initramfs steps

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (new `tf_status`; calls in `tf_find_esp` wait loop, `restore_home`), `…/mount-squashfs.sh`
- Test: `test/integration/test-initramfs-home.sh`

**Interfaces:**
- Produces: `tf_status <text>` — sh, in `thinfloe-lib.sh`; runs `plymouth update --status="tf:<text>"` only when `command -v plymouth` and `plymouth --ping` succeed; output to /dev/null; always returns 0.

- [ ] **Step 1: Write the failing test** in `test-initramfs-home.sh`, with the restricted initramfs PATH it already uses and a `plymouth` stub that appends its arguments to `$WORK/plymouth.calls` (`--ping` exits 0): `tf_status "Restoring the home directory"` → calls file has `update --status=tf:Restoring the home directory`; with a stub whose `--ping` exits 1 → no `update` line and return 0; with no `plymouth` on PATH → return 0. Also: `restore_home` on a test archive records `tf:Restoring the home directory`.
- [ ] **Step 2: Run** `bash test/integration/test-initramfs-home.sh` → fails (`tf_status: not found`).
- [ ] **Step 3: Implement** `tf_status`; call it with "Finding the boot stick" once when `tf_find_esp` starts waiting (not per poll), "Loading the system" in `mount-squashfs.sh` before the local mount / the netboot download, "Restoring the home directory" at the start of `restore_home`.
- [ ] **Step 4: Run** the test → PASS; run `test-initramfs-commands.sh` after the next build → still PASSED.
- [ ] **Step 5: Commit** `initramfs: boot steps on the boot screen`.

### Task 3: `plymouth-step` helper and add-on steps

**Files:**
- Create: `overlay/usr/libexec/thinfloe/plymouth-step` (mode 0755, sh)
- Modify: `overlay/usr/bin/thinfloe-addon` (`cmd_boot`)
- Test: `test/integration/test-plymouth-step.sh` (new, wired into `make test` like its neighbours), `test/unit/test_addon.py`

**Interfaces:**
- Produces: `plymouth-step status <text>` | `deactivate` | `reactivate` | `quit` | `quit-retain`. Each: no-op unless `plymouth --ping` succeeds; `status` sends `update --status=tf:<text cut to 64 chars>`; `quit-retain` runs `plymouth quit --retain-splash`; stdout/stderr discarded; exit 0 always. Plymouth binary overridable by `TF_PLYMOUTH` for tests.
- Produces (Python): in `thinfloe-addon`, `PLYMOUTH_STEP = "/usr/libexec/thinfloe/plymouth-step"` (overridable by env `TF_PLYMOUTH_STEP`) and `boot_step(text: str) -> None` (runs it with `["status", text]`, timeout 2 s, ignores every error, skips when the file is not executable).

- [ ] **Step 1: Write failing tests.** `test-plymouth-step.sh` with a recording `plymouth` stub: each verb maps to the expected stub call; a 200-character text with `"`, `$(id)` and a space arrives as one argument, cut to 64, and `id` never runs; `--ping` failing → no further call, exit 0; stub missing → exit 0, no output. `test_addon.py`: `test_boot_sends_a_step_per_addon` — with `TF_PLYMOUTH_STEP` pointing at a recording script, `cmd_boot` with two ticked add-ons named "ThinLinc" and "OxuLnk" records `status Installing ThinLinc` then `status Installing OxuLnk`; `test_boot_step_failure_is_ignored` — the recorder exits 1 and `cmd_boot` still installs.
- [ ] **Step 2: Run** `bash test/integration/test-plymouth-step.sh` and `make test-unit` → both fail.
- [ ] **Step 3: Implement** the helper and `boot_step`; call `boot_step("Installing %s" % a.name)` in `cmd_boot` right after `a = todo.pop(0)`.
- [ ] **Step 4: Run** both → PASS.
- [ ] **Step 5: Commit** `boot screen: add-on installs name the add-on`.

### Task 4: The hand-over

**Files:**
- Create: `overlay/etc/systemd/system/getty@tty1.service`, `overlay/etc/systemd/system/thinfloe-plymouth-fallback.service`, `overlay/etc/systemd/system/thinfloe-plymouth-fallback.timer`
- Delete: `overlay/etc/systemd/system/getty@tty1.service.d/autologin.conf` (grep first that nothing else reads it)
- Modify: `containers/ubuntu/Dockerfile` (mask `plymouth-quit.service`; enable the timer), `overlay/etc/profile.d/10-thinfloe-session.sh`, `overlay/usr/bin/thinfloe-session`
- Test: `test/integration/test-session-packages.sh`, `test/integration/test-session-lifecycle.sh`

**Interfaces:**
- Consumes: `plymouth-step` (Task 3), always as `sudo -n /usr/libexec/thinfloe/plymouth-step <verb>` from tfuser code; path overridable by `TF_PLYMOUTH_STEP` for tests.

- [ ] **Step 1: Write failing image guards** in `test-session-packages.sh`: `/etc/systemd/system/getty@tty1.service` exists, contains `--autologin tfuser` and `After=thinfloe-wait-kms.service`, has no `plymouth-quit-wait`; `getty@tty1.service.d/autologin.conf` absent; `plymouth-quit.service` masked (symlink to /dev/null); `thinfloe-plymouth-fallback.timer` enabled, with `OnBootSec=300`, and its service runs `plymouth-step quit`; no `Before=` in either fallback unit.
- [ ] **Step 2: Write failing session tests** in `test-session-lifecycle.sh` with `TF_PLYMOUTH_STEP` pointing at a recorder: (a) labwc stub exits 0 → order `status Starting the session`, `deactivate`, (labwc), `quit`; (b) labwc stub exits 1 twice → `status Starting the session`, `deactivate`, `reactivate`, `status Waiting for the graphics driver`, `deactivate`, `quit`, then the failure text (no `quit` between the two runs: it would leave nothing to reactivate); the last `quit` is recorded before the first failure line is printed; (c) `thinfloe-session` records `quit-retain` before it starts the menu stub; (d) Review Focus 5: a second session run with a recorder that behaves like a dead daemon finishes normally and prints nothing extra.
- [ ] **Step 3: Run** both tests against the current tree/build → FAIL.
- [ ] **Step 4: Implement.** The getty unit copies the built image's `getty@.service` (record its source version in a comment) minus `After=plymouth-quit-wait.service`, with the `ExecStart=-/sbin/agetty --autologin tfuser --noclear %I $TERM`, `RestartSec=5`, `StartLimitIntervalSec=0` and the comment block from the old drop-in, plus `After=thinfloe-wait-kms.service` and `Wants=thinfloe-wait-kms.service`. Fallback: oneshot service `ExecStart=/usr/libexec/thinfloe/plymouth-step quit`, timer `OnBootSec=300`, `WantedBy=timers.target`. Session: as tested in Step 2.
- [ ] **Step 5: Run** both tests (after `flock … make image` for the image guards) → PASS.
- [ ] **Step 6: Commit** `boot: the boot picture stays until the menu draws`.

### Task 5: Verify in QEMU, smoke suite, documentation

**Files:**
- Modify: `CLAUDE.md`, `CHANGES.md`

- [ ] **Step 1: QEMU normal boot**, 1280x800 and 4K (rig as in Task 0) → no text-console frame between the first splash frame and the menu; the step line shows at least "Restoring the home directory" and "Starting the session" in some frame.
- [ ] **Step 2: QEMU failure boot**: `-device VGA` with `modprobe.blacklist=bochs` on the `current` entry's command line (via `CAP_APPEND` and `-kernel/-initrd` with `plymouth.use-simpledrm`, or a copied image with an edited `syslinux.cfg`) → picture with "Waiting for the graphics driver", then the readable failure text.
- [ ] **Step 3: Gates**: `make test` (with `THINLINC_DEB`), `make test-unit`, `make test-runtime`, and the five smoke targets; all pass, `check-boot-smoke.sh` reports no ordering cycle. Report counts, not adjectives.
- [ ] **Step 4: Docs.** `CLAUDE.md`: replace the autologin/plymouth ordering text (Customizing → Autologin, the boot-screen bullet) and add a Standing Constraint "tty1's getty does not wait for Plymouth; the session owns deactivate/quit; plymouth-quit.service stays masked; the fallback timer ends Plymouth at 300 s", with its guards. `CHANGES.md` (Changed): the step line under the progress bar, naming "Installing ThinLinc" as an example; (Fixed): the text console with the Ubuntu welcome text no longer flashes up between the boot screen and the menu.
- [ ] **Step 5: Commit** `docs: boot hand-over and step line`; ask the owner before pushing.
