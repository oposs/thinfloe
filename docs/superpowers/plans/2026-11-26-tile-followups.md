# Tile follow-ups (Branch C) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the PR #62 tile-notices follow-up list (C1 to C6 of the round spec): the update mark no longer covers a long comment, a greyed tile fades its mark highlight and focus rings, the text-column test reads the drawing code, the mark's Tab stop is documented, the image test checks the Eject tile and the terminal icon, and the pointer turns into a hand over what takes a click.

**Architecture:** One pure layout function, `tile_columns`, gives `paint_tile` and the tests the same icon box and text widths, with the second line narrowed to end before the marks. The fade becomes a named function used for the fill, the mark highlight and both focus rings. The pointing hand is one rule at the top of `ui::show`: when the widget egui would give a click to is enabled and senses click but not drag, the cursor is `CursorIcon::PointingHand`; widgets that set their own cursor later in the pass (text fields, disabled shadcn widgets) still win. `app.rs:1348` already passes `platform_output` to `egui_winit::State::handle_platform_output`, which sets the winit cursor.

**Tech Stack:** Rust 1.96.0, egui 0.37.0, egui-winit 0.37.0, winit 0.30.13 (Wayland), shadcn-egui 0.8.0; bash image test `test/integration/test-session-packages.sh`.

**Spec:** [docs/superpowers/plans/2026-11-26-round-fix-everything.md](2026-11-26-round-fix-everything.md), "Branch C: fix/tile-followups" (C1 to C6). Source list: PR #62 body, "Follow-up list / feat/tile-notices".

## Deviations from the round spec

- **C6 covers every enabled click-only widget, not a list of tiles, marks and buttons.** shadcn-egui 0.8.0 buttons set no cursor (only its `item.rs` and `badge.rs` do), and egui's `Visuals::interact_cursor` reaches only `egui::Button`. A per-widget `on_hover_cursor` would be dozens of call sites (buttons, tabs, switches, rows); one rule on egui's click hit (`ViewportState::hits.click`) gives them all the hand. Widgets that also sense drag (scroll bars, sliders, text fields) keep their cursor.
- **C6 "if winit's cursor works on labwc" is settled for the theme, not yet seen on screen.** The shared 4.13 ramroot.sqsh has `usr/share/icons/whiteglass/cursors/pointer -> hand` (checked with `unsquashfs -ll`), the name labwc's cursor-shape path and winit's themed fallback both load for `PointingHand`; the text-field I-beam already travels the same egui-winit path. So the plan implements it, adds an image check for the theme file, and records the on-screen sight as an owner check in menu.md, not as a known limit.
- **C1 reserves the width of every mark, including the update mark's hover highlight.** The "user" mark sits left of "update" and is overrun the same way; the highlight is `MARK_HIT_PAD` wider than the word. The reserved edge is the left of the leftmost mark area.
- **C2 fades the highlight and both focus rings (tile and mark), not the word "update".** The word stays at full strength, like the tile's reason line: it is the one part of a greyed tile that still takes a click.

## Global Constraints

- Branch `fix/tile-followups` from `round/2026-11-26b`, in a worktree under `/scratch/oetiker/claude-worktrees/`: `git worktree add /scratch/oetiker/claude-worktrees/thinfloe-tile-followups -b fix/tile-followups round/2026-11-26b`.
- At most 4 cores: every cargo call carries `-j 4` and `--test-threads=4`; the make targets set `BUILD_JOBS`/`TEST_THREADS` themselves. No `/tmp`: scratch files under `/scratch/oetiker/claude-tmp/plan-C/`.
- UI code takes colours from the theme and text sizes from named constants; `brand::tests::ui_code_has_no_style_literals` fails on a literal. New numeric factors get a named `const`.
- Widgets are shadcn-egui `lyra`, nothing rounded; the menu draws on the CPU.
- Name and Comment come from user files: each line stays one line, cut with an ellipsis, clipped to the tile.
- CHANGES.md: what the user sees, at most three sentences per entry, no function names, no release header.
- Single tests: `cd src/thinfloe-menu && cargo test --locked -j 4 <filter> -- --test-threads=4`. Menu gate: `make test-menu` (fmt, clippy `-D warnings`, tests).

## Review Focus

1. A tile with both "user" and "update" marks and a long comment: the comment ends before the leftmost mark, not only before "update". Test `a_long_comment_ends_before_both_marks` (Task 1).
2. A tile without marks keeps the full 126 pt text column of a 220 pt tile; the narrowing must not reach unmarked tiles. `the_upgrade_tile_names_the_release_below_its_name_in_blue` asserts `note_w == text_w` (Task 1).
3. A greyed tile (problem set) with a pending update: its amber reason line also ends before the mark and stays at full strength. Asserted in `a_greyed_tile_fades_the_mark_highlight_and_rings` (Task 2).
4. The pointer over a greyed tile shows no hand; over that tile's update mark it does. `the_pointer_is_a_hand_over_what_takes_a_click` (Task 3).
5. Drag and disabled widgets keep their cursor (scroll bar, slider, text-field I-beam, shadcn's not-allowed). `only_enabled_click_widgets_want_the_hand` on the predicate (Task 3).

---

### Task 1: The tile's text column comes from one layout function and ends before the marks (C1, C3)

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs:1645-1671` (mark constants, `mark_rect`, `update_mark_target`)
- Modify: `src/thinfloe-menu/src/ui.rs:1673-1806` (`paint_tile`: icon box 1704-1710, text column 1735-1745, second line 1746-1765, marks 1781-1805)
- Test: `src/thinfloe-menu/src/ui.rs` `mod tests` (from 1899; the hand-computed column is 3336-3343)

**Interfaces:**
- Consumes: `mark_rect(ui: &Ui, rect: Rect, right: f32, text: &str) -> Rect`, `update_mark_target(ui: &Ui, rect: Rect) -> Rect`, `MARK_RIGHT`, `MARK_GAP`, `MARK_HIT_PAD`, `UPDATE_MARK`, `ICON_BOX`.
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq)] struct TileColumns { icon: Rect, text_x: f32, text_w: f32, note_w: f32 }`: icon box, text column's left edge and width (name line), second line's width.
  - `fn tile_columns(rect: Rect, ppp: f32, marks_left: Option<f32>) -> TileColumns`, pure: `icon` is today's snapped box (`rect.left() + ICON_LEFT`, vertically centred, `ICON_BOX` square); `text_x = icon.right() + TEXT_PAD`; `text_w = (rect.right() - TEXT_PAD - text_x).max(0.0)`; `note_w = text_w` without marks, else `text_w.min((marks_left - MARK_GAP - text_x).max(0.0))`.
  - `fn marks_left(ui: &Ui, rect: Rect, update: bool, user: bool) -> Option<f32>`: left edge of the leftmost mark area, `None` without marks. "update" counts as `update_mark_target`; "user" as its `mark_rect` where the painting puts it (left of "update" by `MARK_GAP`, or at `rect.right() - MARK_RIGHT` alone).
  - `const ICON_LEFT: f32 = 22.0;` and `const TEXT_PAD: f32 = 14.0;` replace the literals in `paint_tile`.

- [ ] **Step 1: Write the failing tests** in `mod tests`, beside `hovering_the_update_mark_lights_the_mark_not_the_tile`. The tile as there (`plain_tile("T0")`, `addon = true`, `AddonState::Pending`) with `comment = Some("W".repeat(80))`; shapes after two `grid_frame` calls.

```rust
#[test]
fn a_long_comment_ends_before_the_update_mark() {
    // (pos, comment) = the TextShape whose galley text is "W".repeat(80)
    // mark = text_shape(&shapes, "update").visual_bounding_rect()
    let comment_right = comment.pos.x + comment.galley.size().x;
    assert!(comment_right <= mark.left() - MARK_HIT_PAD - MARK_GAP + 0.5, "{comment_right} vs {mark:?}");
    assert!(drawn_glyphs.ends_with('…'), "cut with an ellipsis");
}

#[test]
fn a_long_comment_ends_before_both_marks() {
    // same tile, origin = Origin::User: marks "update" and "user"
    let user = text_shape(&shapes, "user").unwrap().1.visual_bounding_rect();
    assert!(comment_right <= user.left() - MARK_GAP + 0.5);
}
```

  In `the_upgrade_tile_names_the_release_below_its_name_in_blue` replace lines 3336-3343 with:

```rust
let tile = layout(screen(1280.0, 720.0), 1, 0, 0, 0).tile;
let cols = tile_columns(Rect::from_min_size(pos2(0.0, 0.0), tile), 1.0, None);
assert_eq!(cols.note_w, cols.text_w, "no mark, no cut");
assert!(note.galley.size().x <= cols.note_w, "{} pt of {} pt", note.galley.size().x, cols.note_w);
```

- [ ] **Step 2: Run them to see them fail**

  Run: `cd src/thinfloe-menu && cargo test --locked -j 4 ui::tests::a_long_comment_ends_before -- --test-threads=4`
  Expected: with `tile_columns` stubbed to today's arithmetic, both FAIL on `comment_right` (the comment runs to `rect.right() - 14`).

- [ ] **Step 3: Implement `TileColumns`, `tile_columns` and `marks_left`** beside `update_mark_target`, and make `paint_tile` use them: icon box from `cols.icon`; the name line cut at `cols.text_w`; the second line (all four branches: problem, release, comment/note, reworded text) at `cols.note_w`. The `line` closure takes the width as a parameter. The mark painting keeps its order (update rightmost, then user) and takes its positions from `mark_rect`, so `marks_left` and the drawing cannot disagree.

- [ ] **Step 4: Run the tile and style tests**

  Run: `cd src/thinfloe-menu && cargo test --locked -j 4 ui::tests -- --test-threads=4` and `cargo test --locked -j 4 ui_code_has_no_style_literals -- --test-threads=4`
  Expected: PASS, including `tile_text_stays_on_one_line_inside_its_tile`, `the_upgrade_tile_names_the_release_below_its_name_in_blue`, `a_click_on_the_update_mark_opens_packages_and_elsewhere_starts_the_tile`, `hovering_the_update_mark_lights_the_mark_not_the_tile`.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/ui.rs
git commit -m "fix(menu): a tile's comment ends before its marks

The second line of a tile was cut at the tile's right edge, so a long
comment ran under the update mark and its hover highlight, and under the
user mark. tile_columns() now gives paint_tile and the tests one text
column; the second line ends MARK_GAP before the leftmost mark area. The
Upgrade tile test takes the column from it instead of recomputing it."
```

### Task 2: A greyed tile fades its mark highlight and focus rings (C2)

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs:1677-1702` (`fade` closure, tile fill, tile focus ring) and the update mark's highlight and ring (1793-1800 before Task 1)
- Test: `src/thinfloe-menu/src/ui.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1's `TileColumns` (unchanged).
- Produces: `const UNUSABLE_FADE: f32 = 0.35;` and `fn faded(c: Color32, usable: bool) -> Color32` (`c.gamma_multiply(UNUSABLE_FADE)` when not usable), replacing the closure. Applied to the tile fill and every colour the closure faded today, plus the tile focus ring, the mark highlight fill and the mark focus ring. Not to: the reason line (WARN), the word "update", the ice edge, the running count.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_greyed_tile_fades_the_mark_highlight_and_rings() {
    // plain_tile("T0"), addon = true, problem = Some("W".repeat(80)), pending update.
    // Hover: PointerMoved to the mark's centre, one more grid_frame:
    //   exactly one Rect filled with faded(TILE_HOVER, false), none with TILE_HOVER.
    // Focus: Tab presses (egui::Event::Key { key: egui::Key::Tab, pressed: true, .. }),
    //   at most 3, until a stroke appears; ring = Theme::current(&ctx).palette().ring;
    //   its colour == faded(ring, false), and no stroke has colour == ring.
    // The reason line: its TextShape keeps the WARN colour and ends before the mark
    //   (Task 1's comment_right check).
}
```

- [ ] **Step 2: Run it to see it fail**

  Run: `cd src/thinfloe-menu && cargo test --locked -j 4 ui::tests::a_greyed_tile_fades -- --test-threads=4`
  Expected: with `faded` in place but unused at the mark, FAIL: the highlight is `TILE_HOVER`.

- [ ] **Step 3: Implement `faded` and use it at the places listed under Produces.**

- [ ] **Step 4: Run the tile tests**

  Run: `cd src/thinfloe-menu && cargo test --locked -j 4 ui::tests -- --test-threads=4`
  Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/ui.rs
git commit -m "fix(menu): a greyed tile fades its mark highlight and focus rings

The fill of a tile that cannot be used was faded, but the update mark's
hover highlight and the focus rings of tile and mark were drawn at full
strength on top of it. One faded() helper now covers them; the reason
line and the word update stay at full strength."
```

### Task 3: The pointer is a hand over what takes a click (C6)

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs`, `pub fn show` (the grid entry point; its body ends about line 935), one call as its first statement
- Test: `src/thinfloe-menu/src/ui.rs` `mod tests` (helper next to `grid_frame`, line ~3361)

**Interfaces:**
- Consumes: egui 0.37 `Context::viewport(|v| v.hits.click)` (`Option<egui::WidgetRect>` with `sense`, `enabled`), `Context::set_cursor_icon`.
- Produces:
  - `fn wants_pointing_hand(w: &egui::WidgetRect) -> bool`: `w.enabled && w.sense.senses_click() && !w.sense.senses_drag()`.
  - `fn pointing_hand(ctx: &egui::Context)`: sets `CursorIcon::PointingHand` when the click hit wants it. First statement of `show`, so a widget that sets its own cursor later in the pass wins.
  - Test helper `fn cursor_at(ctx: &egui::Context, menu: &Menu, p: egui::Pos2) -> egui::CursorIcon`: one layout pass, then a pass with `PointerMoved(p)`; returns that pass's `platform_output.cursor_icon`. It shares the frame code with `grid_frame` (factor out the `RawInput`/`run_ui` part), not a copy.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn only_enabled_click_widgets_want_the_hand() {
    let w = |sense, enabled| egui::WidgetRect {
        id: egui::Id::new(1), parent_id: egui::Id::new(0), layer_id: egui::LayerId::background(),
        rect: Rect::ZERO, interact_rect: Rect::ZERO, sense, enabled, visible: true,
    };
    assert!(wants_pointing_hand(&w(Sense::click(), true)));
    assert!(!wants_pointing_hand(&w(Sense::click(), false)));
    assert!(!wants_pointing_hand(&w(Sense::hover(), true)));
    assert!(!wants_pointing_hand(&w(Sense::click_and_drag(), true)));
    assert!(!wants_pointing_hand(&w(Sense::drag(), true)));
}

#[test]
fn the_pointer_is_a_hand_over_what_takes_a_click() {
    // T0 usable; T1 with problem = Some("broken"), addon = true, pending update.
    // Over T0's name: PointingHand. Over T1's name: Default.
    // Over T1's "update" mark: PointingHand. Over the log area (below the grid): Default.
}
```

- [ ] **Step 2: Run them to see them fail**

  Run: `cd src/thinfloe-menu && cargo test --locked -j 4 hand -- --test-threads=4`
  Expected: with `wants_pointing_hand` stubbed to `false`, `the_pointer_is_a_hand_over_what_takes_a_click` FAILS at T0 (Default) and the predicate test FAILS.

- [ ] **Step 3: Implement `wants_pointing_hand` and `pointing_hand`, and call `pointing_hand(ui.ctx())` first in `show`.**

- [ ] **Step 4: Run the menu gate**

  Run: `make test-menu`
  Expected: fmt and clippy clean; all tests PASS; the passed count is the base run's (on `round/2026-11-26b`) plus the 6 tests added in Tasks 1 to 3.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/ui.rs
git commit -m "feat(menu): the pointer is a hand over what takes a click

egui sets PointingHand only for egui::Button and shadcn-egui buttons set
no cursor, so the pointer stayed an arrow over tiles, marks and buttons.
show() now asks for the hand when the widget egui would give a click to
is enabled and senses click but not drag; a widget that sets its own
cursor later in the pass (text field, disabled widget) still wins.
egui-winit already hands the cursor to winit; whiteglass has pointer."
```

### Task 4: The image test checks the Eject tile, the terminal icon and the hand cursor (C5)

**Files:**
- Modify: `test/integration/test-session-packages.sh:115-121`

**Interfaces:**
- Consumes: the script's `present NAME REGEX` helper.
- Produces: nothing for later tasks.

- [ ] **Step 1: Add the checks**
  - Tile loop, line 115: add `70-eject` between `60-settings` and `80-reboot`.
  - Icon loop, line 118: add `square-terminal` (`30-terminal.desktop` has `Icon=square-terminal`; the Dockerfile installs `icons/*.svg` at line 99).
  - After the Lucide licence line: `present "hand cursor" 'usr/share/icons/whiteglass/cursors/pointer$'`, with a comment that the menu asks for the pointing hand and labwc and winit load it as `pointer` from `XCURSOR_THEME=whiteglass`.

- [ ] **Step 2: Prove the checks see the image and can fail**, on the shared image without a build:

  Run: `TMPDIR=/scratch/oetiker/claude-tmp/plan-C BUILD_JOBS=4 test/integration/test-session-packages.sh ~/scratch/dbrrg-artifacts/rootfs/ramroot.sqsh 2>&1 | grep -E '70-eject|square-terminal|hand cursor'`
  Expected: three `ok` lines. Then rename `70-eject` to `70-ejectX` in the loop, run again: one `not ok` line; undo the rename.

- [ ] **Step 3: Commit**

```bash
git add test/integration/test-session-packages.sh
git commit -m "test: the image ships the Eject tile, the terminal icon and the hand cursor

test-session-packages.sh listed the shipped tiles and Lucide icons
except 70-eject.desktop and square-terminal.svg, both added after the
lists were written. It also checks whiteglass's pointer cursor, which
the menu now asks for."
```

### Task 5: Notes and changelog (C4 and the user-visible effects)

**Files:**
- Modify: `docs/notes/menu.md:69-105` (the "Tiles" bullet under "Upgrade screen, tiles and icons")
- Modify: `docs/notes/menu.md:478` ff. ("Known limits", the "Not verified on hardware" bullets)
- Modify: `CHANGES.md` (`## [Unreleased]`)

- [ ] **Step 1: menu.md, Tiles bullet**: a paragraph on the update mark: a click on it opens Settings → Packages; it is a focus stop of its own right after its tile, so Tab reaches Packages without a mouse (kept on purpose: round 2026-11-26b, C4); a tile's second line ends `MARK_GAP` before the leftmost mark (`tile_columns` in `ui.rs`, the tests read it too); a greyed tile fades its fill, the mark's highlight and both focus rings, not its reason or the word "update"; the pointer is a hand over every enabled click-only widget (`pointing_hand`), drawn from whiteglass's `pointer`.
- [ ] **Step 2: menu.md, Known limits**: to the "Not verified on hardware" bullets add the hand cursor: proven in unit tests (egui asks for it) and by the image's `pointer` file; seen on labwc with a real mouse is an owner check.
- [ ] **Step 3: CHANGES.md** under `## [Unreleased]`:
  - `### Changed`: The mouse pointer turns into a pointing hand over tiles, buttons and everything else in the menu that reacts to a click.
  - `### Fixed`: A long description on an app tile no longer runs under the small "update" or "user" mark; it now ends with "…" before the marks.
  - `### Fixed`: On a greyed-out tile, the highlight and the focus frame of the "update" mark are now greyed out like the rest of the tile.
- [ ] **Step 4: Commit**

```bash
git add docs/notes/menu.md CHANGES.md
git commit -m "docs: tile marks, the Tab stop on the update mark, the hand cursor"
```

### Task 6: Gates

CLAUDE.md "Workflow": the full set before the branch's first push, and before it joins the round branch. The branch changes the menu (`ui.rs`) and one image test; no initramfs, upgrade, network, automount or FloeDrop code.

- [ ] **Step 1:** `make test-unit`. Expected: exit 0.
- [ ] **Step 2:** `make test` (builds the rootfs from this checkout, with a `VERSION` of its own as CLAUDE.md "Build" asks; runs test-unit and test-integration with the extended `test-session-packages.sh`). Expected: exit 0, `0 not ok`, and Task 4's three `ok` lines in the output.
- [ ] **Step 3:** `make test-runtime` (menu change: labwc and the menu headless). Expected: exit 0, `0 not ok`.
- [ ] **Step 4:** `make qemu-smoke`: the one smoke test this branch touches; it boots to the menu that draws the tiles. No other smoke target covers tile drawing or the cursor (`-addons`, `-upgrade`, `-automount` and the rest test code this branch leaves alone). Expected: exit 0.
- [ ] **Step 5:** Note the four results with the commit hash for the PR body's Gates section. After the rebase onto the round branch: no conflict outside `CHANGES.md`, only `make test-menu`; a code conflict, the full set again.

## Self-review

- Spec coverage: C1 Task 1; C2 Task 2; C3 Task 1 Step 1; C4 Task 5 Step 1; C5 Task 4; C6 Task 3, Task 4 (theme file), Task 5 Step 2 (owner check).
- Names: `TileColumns`, `tile_columns`, `marks_left`, `ICON_LEFT`, `TEXT_PAD` (Task 1); `UNUSABLE_FADE`, `faded` (Task 2); `wants_pointing_hand`, `pointing_hand`, `cursor_at` (Task 3) are used alike in later tasks and in Task 5's notes.
- Review Focus: each of the five lines has its test in Tasks 1 to 3.
