# Menu on shadcn-egui 0.8 (Part 0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `thinfloe-menu` builds on egui 0.37 and the crates.io `shadcn-egui` 0.8.0, the vendored `src/thinfloe-menu/egui_shadcn/` is gone, and the menu looks the same as before.

**Architecture:** `brand.rs` stays the one place that knows the ThinFloe look. It builds a `shadcn_egui::Theme` (vega style, radius 0, the ThinFloe palette in both modes, four app colours) and installs Geo and egui's Ubuntu Light through `Theme::apply_with_fonts`, once per context. UI code reads every colour from `Theme::current(ctx)` and every size from named constants in `brand.rs`; a unit test scans the UI files for literals.

**Tech Stack:** Rust 1.96, egui / egui-winit 0.37.0, shadcn-egui 0.8.0, epaint_default_fonts 0.37, softbuffer CPU raster (unchanged).

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`, §"Part 0: one component set" (line 96) and §Testing (line 386).

## Global Constraints

- Worktree: `/scratch/oetiker/claude-worktrees/thinfloe-menu-shadcn`, branch `feature/menu-shadcn-crate`. Crate dir below: `src/thinfloe-menu` (call it `$MENU`).
- Cargo: at most 4 jobs (`-j4`, `CARGO_BUILD_JOBS=4`); `--locked` once `Cargo.lock` is updated.
- Image builds: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=menu-shadcn …` with `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`. Use `VERSION=menu-shadcn` on **every** make call in this worktree. Before the first image build: `mv artifacts artifacts.link && mkdir artifacts && git update-index --skip-worktree artifacts`; undo before the worktree is removed.
- No GPU library: no `wgpu`, `glow`, `eframe` in `cargo tree` (the `test-session-packages.sh` guard).
- Body font stays egui's **Ubuntu Light** (*owner*, 2026-11-05). Geo stays the font of names and titles; no theme font may land in the Geo family or on headings (`Oxanium` was the old trap; with 0.8 it is `Inter`).
- The menu has a dark look only. Both palettes of the theme are the ThinFloe palette, so a system light preference changes nothing.
- Square corners: theme radius `0.0`.
- The spec lists `egui_extras` 0.37 too; the menu does not use it (only the vendored copy did), so it is not added.
- Today's screens need no component the crate lacks (`Button`, `ButtonVariant`, `checkbox`). A missing one found on the way is ported into the crate with `shadcn-egui:port-component`, never painted in the menu.
- Probe (2026-11-05, a copy of the crate in the scratchpad): with only the dependency switch, `cargo check --all-targets` gives 46 errors, all `.palette` → `.palette()`, `Theme::dark()` and `Event::MouseWheel { source }`. The raster path compiles unchanged.
- Pixel values the spec fixes stay: tile group 4 x 220 x 76 pt, `GROUP_W` 889.
- `Theme::apply*` runs once per `egui::Context`, never per frame (crate rule: a per-frame apply undoes style writes and rebuilds fonts).

## Review Focus

1. **A text drawn in the wrong font after the first frame.** `apply_with_fonts` lands one pass late; a name drawn in pass 0 must not stay in Ubuntu Light, and the Geo family must still start with `Geo-Oblique`. → Task 1, test `geo_and_the_body_font_survive_the_theme`.
2. **A system set to light mode.** `ThemePreference::System` or a light `RawInput` must not turn the menu light. → Task 1, test `a_light_system_keeps_the_dark_look`.
3. **Settings rows with square, aligned buttons.** The existing test that requires unrounded rects and one x for button labels (`9d9c636`) must pass unchanged on the crate's `Button` and checkbox. → Task 1, Step 5 (existing test, not rewritten).
4. **A literal that sneaks back into UI code.** → Task 2, test `ui_code_has_no_style_literals` with its positive control.
5. **A window replacement (monitor move) after startup.** The theme belongs to the context, which outlives the window; a new window must draw in the brand look without a second `apply`. → Task 1, test `the_theme_outlives_a_new_window_pass` (a second `run_ui` with a different `screen_rect` keeps `Theme::current(ctx).palette().background == BG`).

---

### Task 0: Baseline pictures (no commit)

**Files:** none in the repo. Output: `/scratch/oetiker/claude-tmp/thinfloe-tests/part0-shots/before/`.

- [ ] **Step 1: Render the pictures on main**

The worktree `/scratch/oetiker/claude-worktrees/thinfloe-writer-root-tool` is at `6a72d6c` = main. In its `src/thinfloe-menu`:

```bash
mkdir -p /scratch/oetiker/claude-tmp/thinfloe-tests/part0-shots/before
TF_SHOT_DIR=/scratch/oetiker/claude-tmp/thinfloe-tests/part0-shots/before \
  cargo test --locked -j4 shot_desktop_and_settings_at_densities -- --ignored
```

Expected: 6 files `desktop-1080.ppm`, `desktop-4k-fallback.ppm`, `desktop-21in-4k.ppm`, `desktop-1080-folded.ppm`, `settings-1080.ppm`, `settings-21in-4k.ppm`.

- [ ] **Step 2: Convert to PNG**

`python3 /scratch/oetiker/claude-tmp/thinfloe-tests/tools/topng.py <ppm>` for each (read the tool's usage line first). Expected: one `.png` per `.ppm`.

---

### Task 1: egui 0.37, shadcn-egui 0.8.0, theme built in brand.rs, vendored copy removed

**Files:**
- Modify: `$MENU/Cargo.toml`, `$MENU/Cargo.lock`
- Modify: `$MENU/src/brand.rs` (theme, fonts, `apply`, tests)
- Modify: `$MENU/src/app.rs:440` (per-frame apply removed; apply where the context is created), `:461`, `:647`, `:733`
- Modify: `$MENU/src/ui.rs` (imports, `.palette` → `.palette()`, the test `MouseWheel` event)
- Delete: `$MENU/egui_shadcn/` (whole directory, `git rm -r`)
- Modify: `containers/ubuntu/Dockerfile:60` (licence install)
- Modify: `test/integration/test-session-packages.sh:91`

**Interfaces:**
- Produces (brand.rs):
  - `pub fn theme() -> shadcn_egui::Theme` — vega style, radius 0, light = dark = brand palette, app colours defined (Task 2 adds the `Role`s; in this task the four `Color32` consts stay public).
  - `pub fn apply(ctx: &egui::Context)` — `theme().apply_with_fonts(ctx, fonts())` and `ctx.set_theme(egui::Theme::Dark)`; called once per context.
  - `pub static UBUNTU_LIGHT: shadcn_egui::FontFiles` — egui's sans as a theme font.
  - unchanged: `geo(size) -> FontId`, `logo(height_px) -> ColorImage`, `GEO`.

- [ ] **Step 1: Write the failing tests in `brand.rs`**

Replace `the_palette_is_the_spec_palette`'s last two lines and add three tests:

```rust
#[test]
fn the_palette_is_the_spec_palette() {
    // the existing five Color32 asserts stay
    let t = theme();
    for mode in [egui::Theme::Dark, egui::Theme::Light] {
        let p = t.palette_of(mode);
        assert_eq!((p.background, p.card, p.ring, p.primary), (BG, TILE, ICE, ICE));
        assert_eq!((p.accent, p.border, p.muted_foreground), (TILE_HOVER, LINE, MUTED));
    }
    assert_eq!(t.radius, 0.0);
}

#[test]
fn geo_and_the_body_font_survive_the_theme() {
    let ctx = Context::default();
    apply(&ctx);
    for _ in 0..3 { ctx.run_ui(Default::default(), |_| {}).textures_delta.clear(); }
    ctx.fonts(|f| {
        let d = f.definitions();
        let geo = &d.families[&FontFamily::Name(GEO.into())];
        assert_eq!(geo[0], "Geo-Oblique");
        assert!(!geo.iter().any(|n| n.contains("inter") || n.contains("oxanium")), "{geo:?}");
        let body = &d.families[&FontFamily::Proportional][0];
        assert_eq!(&*d.font_data[body].font, epaint_default_fonts::UBUNTU_LIGHT, "body is Ubuntu Light");
        let heading = &d.families[&FontFamily::Name(shadcn_egui::theme::FAMILY_HEADING.into())][0];
        assert_eq!(&*d.font_data[heading].font, epaint_default_fonts::UBUNTU_LIGHT, "headings too");
    });
}

#[test]
fn a_light_system_keeps_the_dark_look() {
    let ctx = Context::default();
    apply(&ctx);
    let input = egui::RawInput { system_theme: Some(egui::Theme::Light), ..Default::default() };
    for _ in 0..2 { ctx.run_ui(input.clone(), |_| {}).textures_delta.clear(); }
    assert_eq!(shadcn_egui::Theme::current(&ctx).palette().background, BG);
    assert_eq!(ctx.style().visuals.panel_fill, BG);
}

#[test]
fn the_theme_outlives_a_new_window_pass() {
    let ctx = Context::default();
    apply(&ctx);
    let at = |w, h| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
        ..Default::default()
    };
    ctx.run_ui(at(1920.0, 1080.0), |_| {}).textures_delta.clear();
    ctx.run_ui(at(3840.0, 2160.0), |_| {}).textures_delta.clear();
    assert_eq!(shadcn_egui::Theme::current(&ctx).palette().background, BG);
}
```

Field names (`FontData.font`, `RawInput.system_theme`, `visuals.panel_fill`) are egui 0.37's; adjust the access path if one moved, never the assertion. `geo_falls_back_for_glyphs_it_lacks` stays; change its `"oxanium"` check to `"inter"`.

- [ ] **Step 2: Switch the dependencies and see the tests fail to compile**

`Cargo.toml`: remove `egui_shadcn = { path = "egui_shadcn" }`; add `shadcn-egui = { version = "0.8.0", default-features = false }` (no `font-*`, no `icons-*` feature) and `epaint_default_fonts = "0.37.0"`; `egui` and `egui-winit` to `"0.37.0"` with today's features. Then `git rm -r src/thinfloe-menu/egui_shadcn`.

Run: `cd $MENU && cargo test -j4 --no-run 2>&1 | grep -c '^error'`
Expected: errors (about 46: `.palette` field, `Theme::dark`, `Event::MouseWheel` `source`).

- [ ] **Step 3: Implement `UBUNTU_LIGHT`, `theme()`, `apply()`, `fonts()` in `brand.rs`**

- `UBUNTU_LIGHT`: `FontFiles { name: "ubuntu-light", kind: FontKind::Sans, source: FontSource::Static { regular: epaint_default_fonts::UBUNTU_LIGHT, medium: None, semibold: None, bold: None }, y_offset_factor: 0.0, license: include_str!(<UFL.txt from epaint_default_fonts>) }`. If the licence file is not reachable by `include_str!` from a dependency, copy `UFL.txt` to `$MENU/assets/Ubuntu-UFL.txt` and include that.
- `theme()`: start from `shadcn_egui::Palette::dark()`, set `background BG`, `card TILE`, `accent TILE_HOVER`, `primary ICE`, `primary_foreground #04121f`, `ring ICE`, `border LINE`, `muted_foreground MUTED`; `Theme::new(Style::vega(), p, p, 0.0).fonts(&UBUNTU_LIGHT, None)`.
- `fonts()`: as today, minus the loop over `FAMILY_MEDIUM`/`FAMILY_SEMIBOLD` (the theme now fills those families with Ubuntu Light).
- `apply(ctx)`: `theme().apply_with_fonts(ctx, fonts()); ctx.set_theme(egui::Theme::Dark);` — no "installed" flag; the crate makes a repeat cheap, but the app calls it once anyway (Step 4).

- [ ] **Step 4: Fix the call sites**

- `app.rs:440`: delete the per-frame `crate::brand::apply(&self.ctx)`. Call `brand::apply` once where `self.ctx` is created (the `App` constructor). A window replacement keeps the same context and needs no call.
- `app.rs:461/647/733`, `ui.rs` everywhere: `.palette.x` → `.palette().x`; `brand::theme().palette.background` → `brand::theme().palette().background`.
- `ui.rs:18-19`: `use shadcn_egui::Theme;` and `use shadcn_egui::components::button::{Button, ButtonVariant};`; `ui.rs:770`: `shadcn_egui::components::checkbox::checkbox`.
- `ui.rs:2077` test: add `source:` with egui 0.37's mouse-wheel value for a real wheel.
- Test helpers that call `brand::apply(&ctx)` once per new context stay as they are.

- [ ] **Step 5: Run the whole crate's tests**

Run: `cd $MENU && CARGO_BUILD_JOBS=4 cargo test -j4`
Expected: all pass, the four new brand tests included, and the existing Settings-render test of `9d9c636` (unrounded rects, one x for button labels) unchanged. A test that fails on a moved pixel (a recipe's height or padding differs from the vendored copy) is fixed in the theme construction (`Style` field or important rule in `theme()`), never in the test and never by drawing the widget by hand; list each such fix in the commit body.

- [ ] **Step 6: Licences and the image guard**

- `Dockerfile:60`: drop the `Oxanium-OFL.txt` line (Oxanium is no longer in the binary: no `font-oxanium` feature). Add, after the cargo build, the Inter licence (its bytes are linked: `shadcn-egui-font-inter` is not optional): `install -D -m 0644 "$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/shadcn-egui-font-inter-0.8.0)/LICENSE-FONT.txt" /out/usr/share/thinfloe/fonts/Inter-OFL.txt`. Check `CARGO_HOME` in the `menu-build` stage first and use that path.
- `test-session-packages.sh:91`: `present "Inter licence" 'usr/share/thinfloe/fonts/Inter-OFL\.txt$'`, and an `absent` (or the script's equivalent) for `Oxanium-OFL\.txt$`.
- `cargo tree -j4 -e normal | grep -E 'wgpu|glow|eframe'` prints nothing.

- [ ] **Step 7: Commit**

```bash
git add -A src/thinfloe-menu containers/ubuntu/Dockerfile test/integration/test-session-packages.sh
git commit -m "menu: egui 0.37 and the crates.io shadcn-egui 0.8.0, vendored copy removed"
```

---

### Task 2: Colours through the theme, sizes as named constants, literal scan

**Files:**
- Modify: `$MENU/src/brand.rs` (roles, size constants, ANSI table, scan test)
- Modify: `$MENU/src/ui.rs` (every brand colour and literal size)

**Interfaces:**
- Consumes: `brand::theme()`, `brand::apply` from Task 1.
- Produces (brand.rs):
  - `pub const TILE_ACTIVE: Role`, `TIME: Role`, `WARN: Role`, `GOOD: Role` (`Role::custom("tf-…")`), defined in `theme()` with `.color(role, c, c)`. The `Color32` values move to private consts.
  - The palette-mapped colours are read as `t.palette().card` (TILE), `.accent` (TILE_HOVER), `.border` (LINE), `.muted_foreground` (MUTED), `.primary` (ICE), `.background` (BG). `ICE`, `BG`, `TILE`, `TILE_HOVER`, `LINE`, `MUTED` stop being `pub` (tests in brand.rs keep them).
  - `pub const ANSI_16: [Color32; 16]` — the table now at `ui.rs:918-935`.
  - Size constants (pt), one per role, each with a one-line doc comment: the name 34, byline 14, header 16, timer and Settings title 28, row title 20, tile name 21, tile reason 15, dialog title 24, small text 11, log text 12, log source (monospace) 11. Names are the implementer's; the scan forbids the literals.
  - `fn style_literal(line: &str) -> bool` (private, test-only is fine).

- [ ] **Step 1: Write the failing scan test in `brand.rs`**

```rust
#[test]
fn ui_code_has_no_style_literals() {
    // Positive control: the matcher sees what it must.
    for bad in ["Color32::from_rgb(1,2,3)", "Color32::WHITE", "from_black_alpha(9)",
                "FontId::proportional(11.0)", ".size(28.0)", "brand::geo(34.0)", "geo(21.0)"] {
        assert!(style_literal(bad), "{bad}");
    }
    assert!(!style_literal("brand::geo(NAME_PT)"));
    for (name, src) in [("ui.rs", include_str!("ui.rs")), ("app.rs", include_str!("app.rs")),
                        ("header.rs", include_str!("header.rs")), ("desktop.rs", include_str!("desktop.rs")),
                        ("settings.rs", include_str!("settings.rs"))] {
        let code = src.split("#[cfg(test)]").next().unwrap();
        let mut prev = "";
        for (n, line) in code.lines().enumerate() {
            let ok = prev.trim_start().starts_with("// token-ok:");
            assert!(ok || !style_literal(line), "{name}:{}: {}", n + 1, line.trim());
            prev = line;
        }
    }
}
```

`style_literal` is true when the line contains `Color32::from_`, `Color32::WHITE`, `Color32::BLACK`, `from_black_alpha`, `from_white_alpha`, or one of `FontId::<word>(`, `.size(`, `geo(` directly followed by a digit.

Run: `cargo test -j4 ui_code_has_no_style_literals`
Expected: FAIL, first hit in `ui.rs`.

- [ ] **Step 2: Move the colours and sizes**

- Every use of `ICE`, `TILE`, `TILE_HOVER`, `TILE_ACTIVE`, `MUTED`, `TIME`, `WARN`, `GOOD`, `LINE`, `BG` in `ui.rs` non-test code reads from `Theme::current(ctx)` (`t.palette().…` or `t.role(brand::…)`), through the `t` that the function already has or gets.
- The ANSI table goes to `brand::ANSI_16`; the computed 256-colour and RGB cases (`ui.rs:944-954`) and the untinted icon/logo draws with `Color32::WHITE` (`:318`, `:655`, `:780`, `:1044`, `:1092`) and the pixel conversion at `:133` keep their code with a `// token-ok: <reason>` line above (program output colour; white = texture drawn untinted; raw pixel data).
- Every `geo(<n>)`, `FontId::…(<n>)`, `.size(<n>)` uses a size constant. Multiplications of an existing named constant (`CHIP_ICON * 0.6`) are fine.

- [ ] **Step 3: Run the crate's tests**

Run: `cd $MENU && CARGO_BUILD_JOBS=4 cargo test -j4`
Expected: all pass. Test code in `ui.rs` that compared shapes with `ICE` (`:1483`) uses the theme value or a `brand` test accessor.

- [ ] **Step 4: Commit**

```bash
git add src/thinfloe-menu/src
git commit -m "menu: every colour from the theme, every size named, a test scans for literals"
```

---

### Task 3: Pictures, image gates, docs

**Files:**
- Modify: `CLAUDE.md` (only where it names the vendored copy or Oxanium in the menu; the Geo/plain-sans sentences stay true)
- Modify: `CHANGES.md` (only if the owner sees a difference in the pictures)
- Output: `/scratch/oetiker/claude-tmp/thinfloe-tests/part0-shots/after/`

- [ ] **Step 1: Render the after pictures**

Same command as Task 0, in this worktree's `$MENU`, with `TF_SHOT_DIR=…/part0-shots/after`; convert to PNG. Expected: the same file names as `before/`.

- [ ] **Step 2: Measure the difference**

For each pair, count differing pixels and the bounding box of the difference (`/scratch/oetiker/claude-tmp/thinfloe-tests/tools/bbox.py`, read its usage line). Write the counts into a short table for the owner. Any difference is explained (which component, which recipe value) before the owner looks.

- [ ] **Step 3: Owner comparison — STOP and ask**

Give the owner the `before/` and `after/` PNG paths and the table. Accepted differences are recorded in the commit body of Step 6; a rejected one goes back to Task 1, Step 5's rule (theme construction only).

- [ ] **Step 4: Image gates**

```bash
cd /scratch/oetiker/claude-worktrees/thinfloe-menu-shadcn
TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=menu-shadcn test
TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=menu-shadcn test-runtime
```

Pass `timeout: 600000` and poll until each exits. Expected: both green, including `ok   - thinfloe-menu uses no GPU library` and the Inter/Oxanium licence lines.

- [ ] **Step 5: Docs**

`grep -n 'egui_shadcn\|Oxanium' CLAUDE.md` and fix what names the old copy. CHANGES.md: one entry only if Step 3 accepted a visible change, phrased as what the user sees.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md CHANGES.md
git commit -m "menu: Part 0 done; pictures compared by the owner"
```
