This note covers the running session: startup, the per-machine user files, the labwc config merge, audio volume, screen blanking, the GPU wait, rebooting, terminals and screenshots.

## Audio volume

- Audio volume: `overlay/etc/wireplumber/wireplumber.conf.d/50-thinfloe-volume.conf`
  sets `device.routes.default-sink-volume` and `-source-volume` to 1.0, so
  the remote client (ThinLinc through `tlclient-pulseaudio`, OxuLnk through
  `pipewire-alsa`) sets the loudness. WirePlumber 0.5.13's own output
  default is 0.064 (40 % cubic, about -24 dB), and the image has no volume
  control at the machine. The default is applied with `save = false`
  (`apply-routes.lua`) and never stored; a volume set with `wpctl`/`pactl`
  is stored in `~/.local/state/wireplumber/default-routes`, travels with the
  home archive and wins. `test-session-packages.sh` asks the image's
  WirePlumber for both values. **Not verified on hardware.**

## Session startup and the per-machine user files

- Session startup: `overlay/usr/bin/thinfloe-session`, `overlay/etc/profile.d/10-thinfloe-session.sh`
- **Where the settings live:** everything the menu manages is in `~/.config/thinfloe/config.toml`, written only by the menu; the session reads the files the menu derives from it in `~/.config/thinfloe/generated/`. The layout table, the `home-layout` rule and the migration of a 4.x home are in [home-persistence.md](home-persistence.md). The two files below are the only ones a person edits by hand.
- **Per-machine user customisation:** `~/.config/thinfloe/sessionrc` (a comment-only template in `overlay/home/tfuser/.config/thinfloe/sessionrc`): the Wayland replacement for `~/.xsessionrc`. Sourced by `thinfloe-session` after the home restore and before the menu, and so before any tile starts `tlclient`. Because it lives in `$HOME` it is captured by `save-home` and restored each boot, so a user can configure an individual machine without rebuilding the image. The monitor arrangement is set in Settings → Screen (see "Monitor arrangement" in [menu.md](menu.md)). For a layout the tab cannot express, **`wlr-randr` replaces `xrandr`** here (`--output DP-1 --transform 90 --pos 1920,0`); a `kanshi` started here makes the session start no second one. It must run before `tlclient`, because the client reads the monitor layout once at startup.
  Screen blanking is not set here: `TF_IDLE_TIMEOUT` is no longer read from this file. The timeout is `[screen]` in `config.toml`, set on the Screen tab; `thinfloe-session` reads `generated/screen` (`idle_timeout=<seconds>`, default `300`, `0` never).

- **Per-machine user environment:** `~/.config/thinfloe/environment` (template in `overlay/home/tfuser/.config/thinfloe/environment`): variables the compositor reads at **startup**, such as the cursor theme, `TZ=` and `LABWC_FULLSCREEN_SPAN_OUTPUTS`. The keyboard (`XKB_DEFAULT_*`) comes from `generated/environment`, written by the menu; `XKB_DEFAULT_*` lines in the user file are dropped by the composer (the keys are listed in `/etc/thinfloe/menu-managed-keys`) and the Settings screen names each one. `overlay/etc/profile.d/10-thinfloe-session.sh` sources the merged result after `/etc/thinfloe/labwc/environment`, so the user's value wins. Also persisted via `save-home`.

  | variable | default | effect |
  | --- | --- | --- |
  | `LABWC_FULLSCREEN_SPAN_OUTPUTS` | `1` | fullscreen Xwayland windows (the ThinLinc client) span every monitor |

  A per-machine override in `~/.config/thinfloe/environment` **works**, including for
  keys also set in `/etc/thinfloe/labwc/environment` - `LABWC_FULLSCREEN_SPAN_OUTPUTS`
  above and the `XCURSOR_*` cursor overrides.
  A home still in the 4.x layout (`home-layout` says `legacy`, see
  [home-persistence.md](home-persistence.md)) uses `~/.thinfloe-environment`
  and `~/.thinfloe-sessionrc` as before until the migration has run.

  This is not as simple as it looks: labwc's
  `session_environment_init()` (`src/config/session.c:77`) calls
  `setenv(key, value, 1)` - overwrite - and, run with `-C`, treats
  `<config_dir>/environment` as the *only* environment file it reads
  (`src/common/dir.c:153-157`, called from `src/main.c:211`). Pointing `-C`
  straight at `/etc/thinfloe/labwc` therefore let labwc's own re-read reset any
  key also present in the user's file back to the system value, silently
  discarding what `10-thinfloe-session.sh` had just exported into the shell.
  The fix, `overlay/usr/bin/thinfloe-compose-labwc-config`, composes a
  merged config directory at session start - the system dir's files
  symlinked in unchanged, plus an `environment` file that is the system
  defaults, then `generated/environment`, then the user's file - and points `-C` at that instead, so
  the file labwc re-reads already has the user's values last (and therefore
  winning, since labwc's `setenv` on a later duplicate overwrites the
  earlier one - `test/runtime/test-labwc-runtime.sh` asserts this against
  the real parser). If `XDG_RUNTIME_DIR` is unset/unwritable or the merge
  fails, the session falls back to the plain `/etc/thinfloe/labwc` (today's
  behaviour) rather than failing to start. This is proven by
  `test/integration/test-labwc-config-merge.sh` (the merge logic, offline)
  and `test/runtime/test-labwc-runtime.sh` (the parser assumption, against
  real labwc).

  **Confirmed on hardware (2026-07-27):** an `XKB_DEFAULT_*` override in
  the 4.x `~/.thinfloe-environment` took effect, which is what the merge exists to
  make possible - before it, that documented feature had never worked. The
  session also starts normally, i.e. the merged config dir is used without
  needing its fallback. Not separately exercised: setting
  `LABWC_FULLSCREEN_SPAN_OUTPUTS=0` to get single-monitor fullscreen. It
  travels through the same merge, so it is expected to work, but it has not
  been observed.

  **The two user files are split by timing, and it is not arbitrary:**

  | file | sourced | for |
  | --- | --- | --- |
  | `~/.config/thinfloe/environment` | before the compositor starts | variables read at startup: cursor, `TZ=` |
  | `~/.config/thinfloe/sessionrc` | inside the running session | commands needing a compositor: `wlr-randr`, `kanshi` |

  This is why the home restore happens in the **initramfs** rather than
  inside the session: a user's saved `generated/environment` and `environment` have to be on disk
  before the compositor reads `XKB_DEFAULT_*`. Restoring home inside the
  session would make a user keyboard change take effect only on the *next*
  boot. Do not move the restore into `thinfloe-session`, and do not move it
  back into `10-thinfloe-session.sh` either: `thinfloe-ssh-hostkeys` now needs
  the home directory before `multi-user.target`.

  Note the deliberate split from system config: user settings live in `~/.config/thinfloe`, but `overlay/etc/thinfloe/labwc/rc.xml` does **not**; see [Standing Constraints](../../CLAUDE.md#standing-constraints).
- Autologin: `overlay/etc/systemd/system/getty@tty1.service`, a whole unit
  running `login -f tfuser` (see [Standing Constraints](history-of-constraints.md#tty1-does-not-wait-for-plymouth-the-session-ends-it))
- User defaults: `overlay/home/tfuser/` (`.config/thinfloe/environment`, `sessionrc`, `home-version`, `.thinlinc/tlclient.conf`)

After modifying overlay files, rebuild with `make image`.

## Keyboard change in the running session

The Keyboard tab of the Settings screen (`jobs::keyboard_apply`, in
`src/thinfloe-menu/src/`) applies a change without a restart, in one job:
`config.toml` (`[keyboard]`, written by the store) -> `generated/environment`
(`XKB_DEFAULT_LAYOUT`, `_VARIANT`, `_OPTIONS`) ->
`thinfloe-compose-labwc-config --env-only` (rewrites only the merged
`environment` in `$XDG_RUNTIME_DIR/thinfloe-labwc`) -> `labwc --reconfigure`.
Only one apply runs at a time.

The full composition stays at session start. The composed directory holds
copies that `labwc-scale` made of `rc.xml` and `themerc-override`; a full
rebuild during the session would wipe the scaled copies, so `--env-only`
leaves everything but `environment` alone.

`test/runtime/test-labwc-runtime.sh` runs this sequence against the real
labwc: start with `us`, apply `ch`/`de_nodeadkeys`, and assert the merged
`environment` carries the new layout. It cannot show that labwc builds the
keymap: the headless rig has no keyboard device (see the Xwayland note in that
script), so labwc builds no keymap and prints no error whatever the menu
writes. A control run applies a pair XKB cannot build (`fr`/`de_nodeadkeys`)
through the same sequence, with the composer's keyboard check turned off
(`TF_KB_CHECK=/nonexistent`); only if labwc then prints
`failed to create xkb keymap` can the rig see keymap errors, and only then
does the keymap assertion count. On this rig the control prints nothing, so
the assertion is reported as a skip. (The bad pair must not go into the
start-up environment: labwc then prints the error but crashes before the
session script runs.)

### Keyboard check before labwc starts

labwc reads the composed `environment` before the menu runs, so a value the
menu wrote when it was valid can still crash it (status 139) after an upgrade
changes the XKB layout list, and the menu then never starts to repair it.
`thinfloe-compose-labwc-config` therefore reads `XKB_DEFAULT_LAYOUT` and
`XKB_DEFAULT_VARIANT` from the merged file the way labwc does (last line wins,
key and value trimmed, quotes kept) and runs
`thinfloe-menu --check-keyboard <layout> <variant>` (`TF_KB_CHECK` overrides
the program). It prints the two lines to append when XKB cannot build the
pair, with the same repair as the menu, and the reason to stderr (the session
log); a missing or failing checker changes nothing. Both the full composition
and `--env-only` run it. The check takes about 30 ms.
`test-labwc-runtime.sh` starts labwc with `fr`/`de_nodeadkeys` in the start-up
environment and asserts that the session runs with `fr` and no variant.

### Left to verify on hardware

- The new keymap reaches the menu and other Wayland clients at once.
- An open ThinLinc client follows the new layout. If it does not, the note on
  the Keyboard tab becomes "ThinLinc picks up the change when it starts
  again" and the result is recorded here.

## Screen blanking had to be rebuilt after the X11 removal

Nothing blanked the screen between the move to Wayland and its fix. On X11 the
server did this itself (`xset s`, server-side DPMS), so it came for free with
the `xorg` metapackage. Wayland splits the job: the compositor only *reports*
idleness, and a separate daemon decides what to do about it.

labwc 0.9.3 holds up its half - the shipped binary advertises
`ext_idle_notifier_v1`, `zwlr_output_power_manager_v1`,
`zwp_idle_inhibit_manager_v1` and `ext_session_lock_manager_v1`. What was
missing was any listener, so labwc reported idleness into an empty room
forever.

`swayidle` (the listener) and `wlopm` (the output power switch) are therefore
part of the session package set, launched from `thinfloe-session` at a 300 s
default. Two decisions are deliberate:

- **`wlopm`, never `wlr-randr --off`.** `wlr-randr` is already in the image
  and looks like the obvious tool. It speaks the output *management* protocol
  and disables the output outright, which changes the monitor layout and
  resizes the fullscreen ThinLinc client (and makes
  `LABWC_FULLSCREEN_SPAN_OUTPUTS` recompute over a different set of outputs).
  `wlopm` speaks output *power* management - real DPMS, layout untouched.
- **No locking.** `ext_session_lock_manager_v1` is available, but `tfuser` has
  no password, so a lock screen would have nothing to authenticate against.
  Session security is ThinLinc's job.

Known consequence: the ThinLinc client is an X11 app under Xwayland and cannot
send an idle inhibit, so a long video inside the remote session with no local
input blanks the local screen. Any keypress restores it. Raise
the timeout on the Settings Screen tab where that matters.

`test/integration/test-session-packages.sh` guards both halves - the two labwc
protocols, the presence of `swayidle`/`wlopm`, that `thinfloe-session` starts the
daemon, and that it does *not* blank with `wlr-randr --off`.

**Confirmed on hardware (2026-10-01):** the screen blanks at the 300 s default
and any keypress restores it.

## x11-xserver-utils: investigated and disproved

`xrandr`, `xset` and `xrdb` were a hard dependency of the `xorg` metapackage,
which was removed when the session moved to labwc/Wayland. This was flagged
as a risk (ThinLinc shelling out to `xrandr` for fullscreen resolution
negotiation) and then checked: `vncviewer`/`tlclient.bin` dlopen
`libXrandr.so.2`, which **is** still present in the image, and grepping for
the executables `xrandr`/`xset`/`xrdb` across `/opt/thinlinc/` returns zero
hits. ThinLinc uses the RandR *library*, never the CLI tools that
`x11-xserver-utils` shipped, so removing that package is safe.

That conclusion still holds, but note its scope: it only ever asked what
*ThinLinc* needed. `xset s` and server-side DPMS went out with the same
change and nothing replaced them - see "Screen blanking had to be rebuilt
after the X11 removal" above.

## The session waits for the GPU before it starts

`thinfloe-wait-kms.service` waits up to 20 s for a real KMS driver before the
tty1 autologin: amdgpu and nouveau are omitted from the initrd and bind only
after switch-root, and simpledrm's `card0` alone does not count. radeon is not
in the image at all, so its machines stay on simpledrm.
i915 is in the initrd, so on Intel graphics the wait ends at once. `10-thinfloe-session.sh`
retries labwc once after another wait-kms, so a machine with no GPU waits about
40 s before the failure screen.

On AMD APUs (Carrizo, Stoney, Ryzen) `amdgpu` binds after switch-root and replaces `simpledrm`,
which Plymouth and the boot used until then; the image carries only the APU
firmware (A1 in `docs/superpowers/specs/2026-11-12-image-size-design.md`).
When the firmware a chip needs is missing, amdgpu fails; the boot waits about
20 s, and then either the menu runs unaccelerated on simpledrm or, worst
case, the failure screen appears after about 40 s. A missing gfx, SDMA or PSP
file fails early and most likely leaves `simpledrm` in place; a missing
display file (DMCUB) fails after `simpledrm` is gone, so no `card0` remains
and labwc fails.

## Rebooting from inside the session needs sudo

There is no polkit in this image, no `polkitd` and no `pkexec`, and systemd
gates `Reboot` and `PowerOff` for non-root callers on a polkit authority. A
bare `systemctl reboot` as `tfuser` is refused. Use `sudo systemctl reboot`,
the way `thinfloe-save-home` and `thinfloe-session` (for the Restart and Power off
tiles) already escalate.

## Reaching a terminal

The Terminal tile of the menu starts `thinfloe-terminal`, which runs `foot`
with Smalti 7x14 (`fonts-smalti`, a pinned release `.deb` in
`containers/ubuntu/Dockerfile`) at `14 * n` px with `12 * n` padding, where `n` is the menu's scale `s` rounded
(at least 1), read from the scale file (see Menu density in [menu.md](menu.md)):
the session runs every output at scale 1, and Smalti is sharp only at whole
multiples of 14 px. Without a usable file (one number from 0.5 to 8 on line 1)
`n` is 2 when the shorter side of the largest current `wlr-randr` mode is 1800 px
or more, else 1. The rest of the look (menu colours, padding, a window of 100x30
characters) is `overlay/etc/xdg/foot/foot.ini`. foot has no scroll bar;
`indicator-position=relative` shows a box on the right edge while the view
is scrolled back. `test/integration/test-terminal.sh` guards the size rule
and the config. The failure windows of `thinfloe-session` start `foot`
directly and get the 14 px look. With zero
labwc keybindings ([Standing Constraints](../../CLAUDE.md#standing-constraints)) there is no
other in-session path, so when the session is down, reach a terminal from a VT
(Ctrl+Alt+F2, say) or over SSH instead. **SSH needs a password
first:** a machine nobody has run `sudo thinfloe-password` on refuses every
password login, because tfuser's shadow field is `!` and the image ships no
`authorized_keys`. On such a machine the VT is the only way in, and the VT
autologin gets there without a password.

```bash
XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=wayland-0 foot
```

Do not add a keybinding to launch it - that would violate the
zero-keybindings constraint.

## Screenshots

Screenshots use `grim`, with `slurp` for region selection and `wl-copy` to
put the result on the clipboard. Same constraint as `foot`: with zero
keybindings there is no in-session trigger, so take them from a VT or over
SSH.

```bash
XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=wayland-0 grim /tmp/shot.png
XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=wayland-0 \
    sh -c 'grim -g "$(slurp)" - | wl-copy'
```

Do not add a keybinding for this - that would violate the
zero-keybindings constraint.
