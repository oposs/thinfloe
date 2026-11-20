# Switch knob that seems not to move: design

## Intent

The owner tested ThinFloe 4.6.0 on a thin client: on the switches of the
menu (Settings → Keyboard, "Show the public IP address" on the Network tab,
"Advanced" on the Upgrade screen) the colour changes when toggled, but the
knob does not move.

## Root cause

The knob moves. Its colour in the "on" state makes it look as if it had not.

- The menu turns all egui and shadcn-egui animation off on purpose
  (`app.rs`: `style.animation_time = 0.0`, "an animated widget on a CPU
  rasteriser arrives as visibly staged full frames"). shadcn-egui's
  `motion::transition` treats that as reduced motion: the thumb jumps to its
  end position in the same pass as the click. A headless probe (egui pass +
  tessellation, brand theme, `animation_time` 0) shows the thumb at x 1.5..16.5
  when off and 15.5..30.5 in the very pass of the click. The repaint loop
  honours egui's `repaint_delay` (`app.rs`, after the raster) and the damage
  tracker (`damage.rs`) covers the old and the new place of any changed
  primitive, so nothing in the loop drops the move.
- lyra's thumb colour (`style/lyra.rs`, `switch_thumb().bg`) is
  `background` by default, `foreground` when dark and unchecked, and
  `primary-foreground` when dark and checked. The menu is always dark. The
  brand (`brand.rs`, `theme()`) sets `primary` to ice blue `#7cc4ff` and
  `primary_foreground` to `#04121f`, dark text for the ice button. The page
  background is `#05070a`.
- So the switch shows, off: a near-white 16 px square at the left of a faint
  grey track; on: an ice blue track whose right half is a `#04121f` square,
  about the colour of the page. The brand made the switch square (lyra's own
  is round), so a round knob no longer shows as a knob: the eye reads the
  dark right half as background and the ice left half as the knob, at the
  same place where the white knob was. "The colour changes, the knob stays."

## Other animations

None is broken, because none runs: with `animation_time` 0 every shadcn-egui
transition (colours, the switch thumb, dialog and popover open, the
spinner), egui's collapsing headers and scroll areas jump by design. This
change keeps that. The OSD sets `animation_time` 0 too (`osd/draw.rs`).

## Design

The knob is the theme's `foreground` (`#fafafa`) in every state: in
`brand::theme()`, `style.switch_thumb.bg = Prop::fixed(Paint::Role(Role::Foreground, 1.0))`.
The track keeps lyra's colours (ice blue when on, `input` at 80 % when off).
The knob is 14 points (lyra: 16) and steps 2 points in from the track's
edge at the off end (`shift_x` `Px(2)`) and by `calc(100% + 2px)` when on,
so the track shows 2 points around it on every side (controller,
2026-11-20: the edge of the light knob must read against the ice track,
1.8:1). The track's transparent 1 px border is dropped
(`border_width` 0): it is one device pixel at every scale, so with it the
gaps at the two ends differed by up to two pixels at a fractional scale.
Now 2 + 14 + 14 + 2 is the track's 32 points; at a fractional scale the
raster's pixel-centre rule may still round the two gaps one pixel apart,
which only a thumb snapped to the pixel grid inside shadcn-egui would
avoid. Keyboard focus still shows as lyra's ring outside the track.

- *(architect)* One knob colour in both states, rather than a dark knob with
  a frame or a different track colour: a knob of constant colour is what
  lets the eye follow it from left to right; it is how shadcn's light mode
  and most toggle switches look. The knob reads against the page in both
  states (contrast with `#05070a` about 19:1). Against the ice track it is
  about 1.8:1; the position is carried by where the light square sits, the
  state also by the track colour (ice against grey).
- *(architect)* No animation: turning it on would bring back the staged
  full frames the CPU raster avoids, and the bug is not the missing slide.
- *(architect)* Square knob and track stay (owner, 2026-11-06: square in
  every component).
- `Prop::fixed`, not `Prop::set`: `set` keeps lyra's `dark:` rules, which
  outrank the base.

## Upstream (shadcn-egui)

No defect in the crate: the Switch and its reduced-motion path work. The
cause is the brand's palette meeting lyra's dark checked thumb. One
possible upstream improvement: the thumb rect is not snapped to device
pixels, so at a fractional scale its gaps to the two track ends can round
one pixel apart; snapping the thumb inside the snapped track (as
`on_pixels` does for the hidden part) would make them equal.

## Tests

In `brand.rs`'s tests, through the menu's own path (brand theme,
`animation_time` 0, `raster::Canvas` over the page background,
`damage::Tracker`):

1. `the_switch_knob_moves_and_reads_on_the_page`: a pass with the switch off,
   a click, a pass with it on. In both rasters the knob (pixels of the
   knob colour inside the track) is found; it is the same colour in both,
   its centre moves right by at least a third of the track width, and its
   contrast with the page background is at least 4.5:1; the track colour
   shows in a ring 2 px wide around the knob on all four sides, and the gap
   at the off end equals the gap at the on end (at most 1 px apart at a
   fractional scale); at scales 1, 1.25, 1.5, 1.75 and 2, with the switch
   at whole and fractional positions. Red before the fix (the on
   knob is `#04121f`, contrast about 1.1:1; the inset was 1 px).
2. `a_switch_click_damages_the_old_and_new_knob`: the tracker's damage for
   the click pass covers both knob rects. Green before and after; it pins
   that the loop draws the move.

## Docs

- `CHANGES.md`, Fixed: the knob looked as if it stayed put; it is now light
  in both positions.
- `docs/notes/menu.md`: one paragraph on the switch knob colour and why.
