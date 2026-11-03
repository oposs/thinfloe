# Peripheral battery: a switch in Settings

Follow-up to `docs/superpowers/plans/2026-11-03-peripheral-battery.md`
(implemented on this branch). Owner's request, 2026-11-03: "a PR that puts
the function under Settings, to switch it on or off".

## Decisions (owner, 2026-11-03)

- **Default off.** A machine nobody switched on reads no batteries and
  exports nothing. Reason: on means every ThinLinc session runs the
  client's NFS server and exports `/run/thinfloe/peripherals` to the
  ThinLinc server.
- The switch is per machine and travels with the home archive, like the
  add-on ticks.

## What the switch controls

| | off (default) | on |
| --- | --- | --- |
| `thinfloe-peripheral-battery.service` | does not run | runs every 5 min (timer) |
| `/run/thinfloe/peripherals/battery` | absent (removed when switched off) | written |
| `tlclient_mandatory.conf` | no `NFS_SERVER_ENABLED` / `NFS_EXPORTS` lines | the two lines of today |

## Design

- **State:** the file `~/.config/thinfloe/peripheral-battery` (tfuser's
  home, resolved with `getent passwd tfuser`, never `$HOME`) exists = on.
  Content `on\n`, owned by tfuser, mode 0644. Written and removed only by
  the root command below. It travels with `save-home`.
- **Root command:** the helper `/usr/libexec/thinfloe/peripheral-battery`
  gets subcommands; no argument keeps today's behaviour (one reading run).
  - `enable`: create the state file, rewrite the ThinLinc mandatory config
    with the two keys (when `/opt/thinlinc/etc` exists), start the
    service once. Exit 0, or 1 with a reason on stderr.
  - `disable`: remove the state file, remove the two keys from the
    mandatory config, remove `/run/thinfloe/peripherals/battery`.
  - `thinlinc-config`: make the mandatory config match the state (keys
    present when on, absent when off). Called by the add-on hook.
  - `status`: print `on` or `off`.
  The mandatory-config rewrite moves from the sh hook into the helper
  (same rules as today: temp file + rename in the same directory, keep
  every other line, mode 0644, never fail the ThinLinc install).
- **Timer and service:** the timer stays enabled in the image; the service
  gets `ConditionPathExists=/home/tfuser/.config/thinfloe/peripheral-battery`,
  so it skips while off. No `Before=` anywhere.
- **Hook `10-thinlinc`:** the shim part is unchanged; the export part calls
  `peripheral-battery thinlinc-config` and only logs a failure (as today).
  `TF_THINLINC_DIR` must still reach the helper for the tests.
- **Menu (`src/thinfloe-menu`):** the Settings screen gets one more row
  after the add-on rows: name "Keyboard and mouse battery", status line
  `off` or `on · in ThinLinc as ~/thindrives/peripherals/battery`, plus
  `takes effect when ThinLinc starts again` while a ThinLinc client
  (`tlclient.bin`) runs or the last switch happened after it started (a
  simple rule is fine: always show it right after a change in this menu
  run). The tick box runs `sudo -n /usr/libexec/thinfloe/peripheral-battery
  enable|disable` through the existing job machinery (as add-on jobs do:
  own process group, output in the log with source `battery`, row busy
  while it runs). The row's ticked state is the state file's existence,
  read when the Settings screen is shown and after the job ends.
  The end-of-session dialogs need not know about this job beyond what
  the job machinery already gives every job (follow what add-on jobs get;
  if that is add-on specific, a battery job may simply be short and not
  waited for — decide and record).
- **Docs:** CLAUDE.md "Peripheral battery" bullet: default off, the
  switch, the state file, what on/off change.

## Tests

- Python (`test/unit/test_peripheral_battery.py`): enable/disable/status/
  thinlinc-config against temp dirs (`TF_HOME` or similar override for the
  home, `TF_THINLINC_DIR`); disable removes the battery file and the keys
  and keeps other mandatory lines; enable without `/opt/thinlinc/etc`
  still creates the state file; idempotent.
- `test/integration/test-thinlinc-hook.sh`: export only when the state
  file exists; keys removed when it does not.
- `test-session-packages.sh`: the service has the `ConditionPathExists=`.
- Rust: the row (off/on text, busy), the event, the command line the job
  runs. Run with `RUSTUP_TOOLCHAIN=1.96.0 PATH=$HOME/.cargo/bin:$PATH
  cargo test --locked` (the mise shim for cargo is broken on this host).

## Delivery

On branch `peripheral-battery`, then push and open a PR against `main`
covering the whole branch (the battery feature and this switch).
