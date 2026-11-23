# Screen tab as lists, with dialogs per monitor and per client

Date: 2026-11-23. Status: approved in conversation (mockup "B, version 3").
Mockups: `.superpowers/brainstorm/3057687-1795418722/content/screen-layout-b3.html`
(git-ignored; the screens below describe them in full).

## Intent

On the Screen tab, a monitor's settings and a Deskflow client's settings show
only after the user clicks its box in the arrangement picture. The owner did
not find the new per-client Compose switch, nor "Share the clipboard", for
that reason: nothing tells you to click the box. The tab also takes a lot of
height (the fingerprint picture of this computer, the rows of a selected
client stacked one under the other).

The fix: every monitor and every client gets one row in a list, with the
settings people change often always visible, and a ⚙ button that opens a
dialog with the rest. Nothing on the tab is hidden behind a selection any
more. Monitors and clients look and work the same.

## Goals

- Every attached client shows its On, Clipboard and Compose switches without
  a click.
- Every monitor shows its Resolution and On without a click.
- Less height than today for the same content.
- One consistent pattern for monitors and clients: row, ⚙, dialog.
- A per-client On switch (new): off means the edge goes dead and the client
  stays connected.

## Non-goals

- No change to the bands' order or to the screen-off and Graphics bands.
- No drag-and-drop in the picture; Position stays a select.
- No change to how monitor changes are applied (draft, Apply, Keep / Go back).
- No change to pairing, the protocol, or the daemon's capture.

## Anti-goals

- A setting reachable only by clicking a box in the picture.
- A quick-change control for something people do not change on a whim
  (Rotation stays in the dialog, owner's ruling).
- An "off" client that drops its connection, frees its edge, or fills the
  client's log with refused connections.

## The tab, top to bottom

1. **Screen off band**, unchanged: "Turn the screen off after" [select].
2. **Monitors band**: title, the arrangement picture (unchanged drawing:
   monitors, client boxes at their edges, "Menu" marker), then a list:

   | Monitor | Resolution | On | |
   |---|---|---|---|
   | DELL U2720 · Menu | [2560×1440 60 Hz ▾] | [switch] | [⚙] |
   | LG 24MK | [1920×1080 60 Hz ▾] | [switch] | [⚙] |

   - "· Menu" marks the monitor the menu is on.
   - The On control keeps today's rules: the menu's monitor and the last
     monitor on cannot be switched off (control disabled).
   - Resolution and On change the draft, as today; Apply at the foot.
   - The "hand-written file" state (`MonSource::Kept`) keeps today's note and
     "Let the menu manage it"; the list's controls are disabled then, as
     today's rows are not offered.
3. **Deskflow band**:
   - "Let other computers use this keyboard and mouse (Deskflow)" [switch].
   - The "Deskflow stopped: …" warning, unchanged.
   - One line: today's `self_line` text ("This computer: 192.168.1.23:24800"), a "Fingerprint…"
     button, and at the right end "Edge resistance" [select]. The button
     opens a dialog with the random-art picture, the colon hex and the
     "Check this fingerprint on the other computer before you trust it
     there." note. The line and the button show only when today's
     `self_line` / `self_fingerprint` give something; no state-file change. When they do not show (Deskflow off), "Edge resistance" keeps a
     line of its own, so it can be set before Deskflow is switched on, as
     today (designer's call).
   - A list of clients:

     | | Computer | On | Clipboard | Compose | |
     |---|---|---|---|---|---|
     | ● | macbook | [switch] | [switch] | [switch] | [⚙] |
     | ○ | win-pc | [switch] | [switch] | [switch] | [⚙] |
     | ! | linux-box wants to attach | [Check and attach…] [Reject] | | | |

     - The first column is the state dot from `device_state`.
     - An off client's row is drawn muted (its switches still work).
     - A client whose edge is not free in the current arrangement gets the
       "This edge is not free in the current arrangement." warning under its
       row (today's `NOT_FREE` text), so the user knows to open ⚙.
     - Pending clients (today's `pending` list) are rows with "Check and
       attach…" and "Reject"; a changed fingerprint keeps today's warning text
       as the row's label.
   - The tab's Deskflow error line, unchanged.
4. **Graphics band**, unchanged.
5. **Footer**, unchanged: ThinLinc note, Apply, the tab's error.

## Dialogs

All dialogs follow the Network tab's dialogs (`network_ui.rs`): the screen
keeps drawing, disabled (`s_ui.disable()`), with the dialog in an Area on
top; a Close button, Escape closes; one dialog at a time. They get their own
`dialog_open` check. They do not use the Keep / Go back path
(`monitors_ui::dialog_open` gating `show_screen` in `ui.rs`), which freezes
and dims the whole screen: that gate stays for Keep / Go back only, so the
picture can keep highlighting the box whose dialog is open. (Builder check,
2026-11-23.)

- **Monitor ⚙ "<monitor name>"**: Position [select, today's choices],
  Rotation [select], "Show the menu here" [button, today's rules]. Changes go
  into the draft; Apply at the foot applies them with Keep / Go back.
- **Client ⚙ "<client name>"**: Position [select, today's free edges plus its
  own], the On, Clipboard and Compose switches again (same state as the row),
  the client's fingerprint (colon hex), "Forget <name>" [button]. Changes
  take effect at once, as today, no Apply.
- **"Check and attach…" "<client name>"**: the client's random-art picture and
  colon hex, the check note, Position [select of free edges], "Attach"
  (enabled once an edge is chosen) and "Cancel". This replaces today's inline
  Attach… / Position / Cancel. When no edge is free: today's
  "No monitor edge is free in the current arrangement." in place of the
  select, Attach disabled.
- **"Fingerprint of this computer"**: random art, colon hex, check note.

A click on a monitor box or a client box in the picture opens that box's ⚙
dialog. The picture keeps highlighting the box whose dialog is open.

## Per-client On switch

- New field `enabled: bool` in each `[[deskflow.devices]]` entry of
  `config.toml`, serde default `true` (existing homes keep working clients);
  carried into `generated/deskflow` like `clipboard` and `compose`, and
  preserved on re-attach with a new fingerprint like `compose`.
- New `DeskflowEvent` for the switch, saved like Clipboard and Compose, and
  the daemon reloads (same path as today).
- Daemon, for a client that is off:
  - It still accepts the connection and keeps it; the state file shows it as
    connected as usual.
  - Its edge does not capture: the pointer stops at that edge and stays on
    ThinFloe (no resistance, no switch).
  - It gets no clipboard (treat as clipboard off while off).
  - Its edge stays reserved: the arrangement still counts it as taken, so no
    other client can be placed there.
  - If it is the active client when it is turned off (only possible from a
    reload, e.g. a config written while active), the daemon releases it back
    to ThinFloe as on Ctrl+Alt+Shift+Escape.
- Picture: an off client's box is drawn dimmed.
- Where it goes in the daemon (builder check): `wanted_strips` gives an off
  client no strip; `update_strips` then releases an active one the way it
  does for an edge that is no longer free; `shares_clipboard` is false for
  it. `free_edges` already counts every configured device, so the edge stays
  reserved without extra work.

## Approaches

- **Chosen: rows plus modal dialogs**, built on the existing dialog overlay.
  It removes the selection-dependent rows and the height they took; the owner
  asked for a popup.
- Rejected: rows that expand in place (accordion). It brings the height back
  and is still a hidden-until-click pattern.
- Rejected: a Deskflow tab of its own (mockup C). The arrangement would show
  in two places; the owner picked B.

## Constraints (from CLAUDE.md)

- UI colours from the theme, text sizes from `brand.rs` constants
  (`ui_code_has_no_style_literals`); shadcn-egui widgets, `lyra`, nothing
  rounded.
- Decisions live in `monitors.rs` / `deskflowtab.rs`; `*_ui.rs` only paint
  and report events.
- The menu is the only writer of `[deskflow]`; the daemon reads only
  `generated/deskflow`. The state-file and socket contract stays as it is.
- Getting back to ThinFloe never waits for the network or the client.
- Layout tests in `ui.rs` (positions of labels, the Graphics band closing the
  tab, rows fitting a 1080 screen) must be updated to the new layout, not
  deleted.

## Left to the builder

- Event shapes: today `MonEvent::Mode` / `Enabled` act on the selected
  monitor; rows need them per monitor (by criteria). Whether "selected"
  becomes "the monitor whose dialog is open" or the events carry the
  criteria is the builder's call.
- Column widths and alignment of the two lists, within the brand constants.
- Exact dialog titles and the muted style of an off row, from the theme.
- Where the new tests go (`deskflowtab.rs`, `deskflowcfg.rs`, daemon edge
  tests, `ui.rs` layout tests, `test-labwc-runtime.sh` if a runtime case
  covers it).
- `docs/notes/deskflow.md`, `docs/notes/menu.md` and `CHANGES.md` updates.
