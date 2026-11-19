# Volume and brightness keys

This note covers the two key switches of Settings → Keyboard, the keybinding
path through labwc, the on-screen display (OSD) and the microphone mark, audio
through `pactl`, brightness through DDC/CI, the tests and what is left to check
on hardware. Design:
`docs/superpowers/specs/2026-11-18-media-keys-osd-design.md`
(its amendment of 2026-11-19 wins over the text above it).

## What the switches do

Settings → Keyboard has two switches, both off by default:

- `Volume keys control this machine`: `XF86AudioRaiseVolume`,
  `XF86AudioLowerVolume`, `XF86AudioMute`, `XF86AudioMicMute`.
- `Brightness keys control the monitors`: `XF86MonBrightnessUp`,
  `XF86MonBrightnessDown`, through DDC/CI on every monitor that answers.

Under each switch the menu shows "When on, these keys no longer reach the
ThinLinc session or any other program." They are `volume_keys` and
`brightness_keys` in `[keyboard]` of `config.toml` (`false` when missing) and
`THINFLOE_VOLUME_KEYS`, `THINFLOE_BRIGHTNESS_KEYS` (`on` or `off`) in
`generated/environment`, both managed keys in
`/etc/thinfloe/menu-managed-keys`. With a switch off, its keys go to the
focused program, as since #28.

## The keybinding path

`thinfloe-compose-labwc-config` reads both values (`on`, `"on"` or `'on'`
count as on) and, when one is on, writes its keys before `</keyboard>`:

```xml
<keybind key="XF86AudioRaiseVolume" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-up" /></keybind>
```

The actions are `volume-up`, `volume-down`, `volume-mute`, `mic-mute`,
`brightness-up` and `brightness-down`. `TF_OSD_KEY_CMD` replaces the command
prefix (tests only); the composer escapes it for XML.

- `allowWhenInhibited="yes"` comes from
  `containers/ubuntu/patches/0006-keybind-allow-when-inhibited.patch`. labwc
  then runs the binding also while ThinLinc holds an X11 grab (patch `0002`)
  or OxuLnk a shortcuts inhibitor (patch `0004`); the grabbing program does
  not get the key. Every other key still reaches it. Only the composer
  writes this attribute.
- labwc repeats a held keybinding (500 ms delay, 30 per second). The volume
  and brightness keys step on while held; the two mute keys carry
  `onRelease="yes"`, so a held mute key toggles once.
- When a binding cannot be written, the composer logs
  `thinfloe-compose-labwc-config: cannot write the key bindings; the volume
  and brightness keys stay off` and leaves the keys off; its exit status does
  not change.

## Live switching

The composed directory `$XDG_RUNTIME_DIR/thinfloe-labwc` keeps the source of
`rc.xml` in `.source/`:

- `rc.xml`: a link to `.source/rc.xml`, always. Once the session runs,
  `labwc-scale` is its only writer.
- `.source/base.xml`: written at session start only, a link to the system
  `rc.xml`, or a copy with `<numlock>off</numlock>` for `THINFLOE_NUMLOCK=off`.
- `.source/rc.xml`: a link to `base.xml` while both switches are off, else
  `base.xml` with the bindings (a temporary file in `.source/`, then
  `mv -f`).

A switch change in the menu runs the keyboard apply: `generated/environment`,
`thinfloe-compose-labwc-config --env-only` (rewrites `environment` and
`.source/rc.xml`), `labwc --reconfigure`, then a `reload` datagram to the OSD.
`labwc-scale` keeps the `cksum` of the source in its state, so a changed
`.source/rc.xml` counts like a new scale: it writes the scaled `rc.xml` again
and reconfigures labwc. At scale 1 with links in place nothing is copied.
labwc applies keybindings on `--reconfigure`, unlike `<numlock>`.
`--env-only` on a directory without `.source/base.xml` writes only
`environment` and logs `thinfloe-compose-labwc-config: no .source/base.xml;
the key switches follow at the next start`.

## The OSD process

`thinfloe-session` starts `thinfloe-menu --osd` next to the menu, always (the
mark shows with the switches off too), logging to the session log
(`TF_OSD` replaces the program). Nothing restarts it; a crash of the OSD or
of the menu leaves the other running and the session's exit codes unchanged.

- Socket: `$XDG_RUNTIME_DIR/thinfloe-osd.sock` (`TF_OSD_SOCKET` overrides),
  a datagram socket, mode 0600, removed when the OSD ends. A key runs
  `thinfloe-menu --osd-key <action>`, which sends one datagram `<action>\n`
  and never waits. The OSD also takes `reload`, which the composer does not
  bind.
- Without an OSD process (crashed, not started), `--osd-key` does the volume
  keys itself on every device, starting from the default device's current
  value, and logs `thinfloe-menu --osd-key: no OSD running; <action> done
  without a display`. Brightness then does nothing (`no OSD running;
  brightness not changed`).
- Surfaces: one `zwlr_layer_shell_v1` surface per output and kind (panel,
  mark), layer overlay, namespace `thinfloe-osd`, exclusive zone -1, keyboard
  interactivity none and an empty input region. Overlay is above a
  fullscreen window in labwc 0.9.3, also one that spans outputs; a click on
  the mark reaches the window below. A new output gets its surfaces, a
  removed one loses them. A surface the compositor closes is not made again
  until the outputs change.
- Size: every length is logical and multiplied by the menu's scale file
  `$XDG_RUNTIME_DIR/thinfloe-scale` (read at each redraw, 0.5 to 8, else 1),
  each term rounded on its own.
- Panel: `OSD_W` wide, `OSD_BOTTOM` above the bottom edge, centred; padding
  `OSD_PAD_X`, `OSD_PAD_Y`; icon `OSD_ICON`, gap `OSD_GAP`; Geo `OSD_TEXT_PT`;
  bar `OSD_BAR_H` high, `OSD_BAR_GAP` below the row. Fill `TILE`, border
  `LINE`, square corners. Icon in `ICE`, name left, value right in `MUTED`.
  The bar fill is `ICE`, the track `LINE`, `BAR_OFF` while muted. The panel
  goes `HIDE_AFTER` (1.5 s) after the last key.
- Panel states: Volume (`volume-2`, percent, bar); Muted (`volume-x` in
  `MUTED`, the level, grey bar); Microphone off (`mic-off` in `WARN`, no bar);
  Microphone on (`mic`, no bar); Brightness (`sun`, that monitor's own
  percent and bar); Brightness on a monitor without DDC/CI (`sun` in
  `MUTED`, "not supported" in `WARN`, no bar, on that monitor only);
  Brightness with an unknown value (`sun` in `ICE`, no value, no bar: before the
  first detection, on an output that came after it, or on a monitor whose
  value could not be read).
- Mark: `MARK_SIDE` square, `MARK_RIGHT` from the right edge and `MARK_TOP`
  from the top of every output, `TILE` fill, `WARN` border, `mic-off`
  (`MARK_ICON`) in `WARN`. It follows the default source's real mute state,
  whoever set it.
- SIGTERM ends the OSD with 0. A Wayland protocol or connection error ends it
  with 1.

## Audio

The OSD keeps a shared state: speaker level `L`, speaker mute `M`, microphone
mute `Q`. It does not read it back from the default device later, because a
headset plugged in often becomes the default and its own 100 % must not become
the shared level. Commands (`TF_PACTL` replaces `pactl`):

- `pactl -f json list sinks`, `pactl -f json list sources`,
  `pactl get-default-sink`, `pactl get-default-source`.
- `pactl set-sink-volume <name> <n>%`, `pactl set-sink-mute <name> 0|1`,
  `pactl set-source-volume <name> <n>%`, `pactl set-source-mute <name> 0|1`.
- `pactl subscribe`, one long-running child.

A set is made only where the device differs from the state. Monitor sources
(`monitor_source` or `monitor_of_sink` set, or `device.class` `monitor`) are
never set.

- Start with the volume switch on: `L` and `M` from the default sink, `Q`
  from the default source, then every sink gets `L`/`M` and every source
  `Q`.
- Start with the switch off, or the switch turned off: every speaker and
  microphone at 100 % and unmuted.
- Volume up or down: `L` ± 5, between 0 and 100, on every sink. A volume key
  does not unmute (owner): while `M` is set, the panel shows Muted with the
  new level.
- Mute: `M` toggled on every sink. Microphone mute: `Q` toggled on every
  source, then the mark is read again from the default source.
- A `new` event: a new sink gets `L`/`M`, a new source `Q` (switch on), or
  100 % and unmuted (switch off). A microphone plugged in during a muted
  call is muted within the time PipeWire takes to report it (about 100 ms
  in the runtime test).
- `change` events of a source or of the server refresh the mark, coalesced;
  they never set anything, so the OSD's own sets cause no loop. Changes made
  by other programs are not copied to the other devices; the next key sets
  every device from the shared state again.
- `reload`: the OSD reads `THINFLOE_VOLUME_KEYS` from
  `~/.config/thinfloe/generated/environment` (`TF_CONFIG_DIR` overrides the
  directory) and acts only when the switch changed. Only a missing file
  means off; a file that cannot be read keeps the current mode, logged once.
- `pactl subscribe` ends (pipewire-pulse restarted, `pactl` missing): the OSD
  starts it again after 1, 2, 4, 8, 16, then 30 s, and back to 1 s after a
  run of 30 s or more. It logs `thinfloe-menu --osd: pactl subscribe ended;
  retrying in N s` once per distinct wait and each distinct stderr line of
  `pactl` once (at most 32 lines), so one outage logs about 8 lines. Once a
  restarted child has run for 500 ms or printed a line, the shared state is
  applied to every device again. A start that failed (the server was not up
  yet, or a 3 s timeout) is tried again when the first `pactl subscribe`
  reaches the server, and at every later key, new device, switch change or
  restart. The child gets `PR_SET_PDEATHSIG`, so it dies with the OSD,
  also on SIGKILL.
- The switch turned off while the devices cannot be listed: the shared
  state is still off, so the next restart or new device gets 100 % and
  unmuted.
- No sound server or device: no panel, one log line, no mark.
- WirePlumber stores the levels it is given in
  `~/.local/state/wireplumber/`; they travel with the home and win over the
  1.0 default of `50-thinfloe-volume.conf` at the next start (see
  [session.md](session.md), Audio volume).

## Brightness

Commands (`TF_DDCUTIL` replaces `ddcutil`, timeout 10 s each; a command past
its timeout gets SIGTERM, SIGKILL 0.5 s later, and is always reaped; the
same holds for the OSD's `pactl` calls):

- `ddcutil detect --terse`: the displays with their I2C bus and DRM
  connector. The connector name matches the Wayland output name.
  `Invalid display` and `Phantom display` blocks count as no DDC/CI.
- `ddcutil --bus <n> --terse getvcp 10`: the current value and maximum, read
  once per display after a detection.
- `ddcutil --bus <n> --noverify setvcp 10 <value>`: an absolute value.

Each key steps the target by a tenth of that monitor's maximum and shows the
new percent at once. Each monitor has its own setter thread; while a set
runs, only the latest target is kept, so a held key does not queue runs that
change the brightness after it is let go. At 0 or at the maximum nothing is
queued. A set that fails or times out is logged and the target kept.

Detection runs at start off the drawing thread and again 2 s after the
outputs change; at most one runs and one more waits. A monitor that does not
answer is logged once per session:
`thinfloe-menu --osd: monitor <output> does not answer DDC/CI; the brightness
keys leave it alone`. Its panel says "not supported".

`/etc/modules-load.d/thinfloe-i2c.conf` loads `i2c-dev` at boot (the
`ddcutil` package ships the same line in `/usr/lib/modules-load.d/`). Access
for tfuser comes from the package's
`/usr/lib/udev/rules.d/60-ddcutil-i2c.rules`, which tags the i2c devices of
display-class PCI devices `uaccess`. The image has no polkit.

## Tests

- `src/thinfloe-menu/src/osd/` unit tests: placement and drawing (`surface.rs`,
  `draw.rs`), the state machine and panel timeout (`state.rs`), the socket
  (`socket.rs`), the shared audio state (`sync.rs`), `pactl` parsing,
  merging of steps, the subscribe back-off and its log cap (`audio.rs`),
  `ddcutil` parsing, the single-slot setter and the hung-call kill (`ddc.rs`),
  the coalesced redetection and `reload` (`run.rs`). Fixtures in
  `src/thinfloe-menu/tests/fixtures/pactl-*` come from the real `pactl` 17.
- `src/thinfloe-menu/tests/osd_key.rs`: `--osd-key` with and without an OSD,
  with a fake `pactl`.
- `jobs.rs`: the keyboard apply sends `reload`.
- `test/integration/test-labwc-config-merge.sh`: the composer's bindings for
  each switch value, quoting, `--env-only`, the failure paths.
- `test/integration/test-labwc-scale.sh`: a changed `.source/rc.xml` is
  scaled and reconfigured once.
- `test/integration/test-session-packages.sh`: the image's composer gives no
  `<keybind` with both switches off and exactly the six media keys, each
  `allowWhenInhibited="yes"`, with both on; `pactl`, `ddcutil` and its
  libraries, `i2c-dev`, the `uaccess` rule.
- `test/integration/test-session-lifecycle.sh`: the session starts the OSD and
  ends it at logout.
- `test/runtime/test-labwc-runtime.sh`: under an X11 grab only the
  `allowWhenInhibited` binding runs; the OSD panel and mark above a spanning
  X11 window and a native fullscreen window, no focus taken, a click and a
  key reach the window below; with the switch off the key reaches `wev`,
  with it on it does not, also after a live switch; the Volume and
  Microphone off panels on both outputs, the panel gone after 1.5 s, the
  mark through mute, unmute and an output unplug; a microphone plugged in
  while muted is muted; the switch turned off resets every device; the
  subscribe back-off; no `pactl subscribe` left after SIGTERM or SIGKILL; a
  monitor without DDC/CI shows "not supported" and is logged once. Audio and
  DDC/CI run against fakes.

## Owner checks on hardware

None of these can be shown in QEMU or the headless rig:

- Brightness through DDC/CI on the monitors in use, also through the docks.
- tfuser gets `uaccess` on `/dev/i2c-*` (an active logind seat session from
  `login -f`); without it `ddcutil` fails as tfuser.
- Mic mute in a ThinLinc call: the remote side hears nothing.
- Volume keys inside a fullscreen ThinLinc session, and Alt-Tab still goes to
  ThinLinc there.
- XF86AudioMute and XF86AudioMicMute inside a fullscreen ThinLinc session:
  their bindings act on release (`onRelease="yes"`), and the runtime test
  covers only a press binding under a grab. Each key must act once, and the
  remote side must get no stray key release.
- OxuLnk: the keys pass its shortcuts inhibitor (no rig can create one).
- The real keyboard's keys send the keysyms the composer binds.
- Two speakers and two microphones (monitor speakers and a USB headset): the
  keys move both.
- A headset plugged in during a muted ThinLinc call is muted.
- Whether WirePlumber restores a stored route volume on a USB headset after
  the `new` event; then the headset keeps its own volume instead of the
  shared level.
- The OSD at an extreme scale: the configure handling was tested only at the
  normal sizes.
