# ThinFloe Part 4: New Look Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give ThinFloe its own look (brand, boot screen B, desktop C, Settings, title bars), replace waybar with a strip of folded windows inside `thinfloe-menu`, and show the clock in the machine's time zone.

**Architecture:** The menu gets a brand module (palette, Geo font, logo) that every drawing call uses, a new layout (header, wide tiles, log, strip), and a window watcher on a Wayland connection of its own that reads `zwlr_foreign_toplevel_manager_v1`. The image loses waybar and GTK 3, gains a Plymouth script theme, a labwc theme override and a small root unit that applies the DHCP time zone.

**Tech Stack:** Rust (egui 0.36, winit 0.30, softbuffer, resvg, wayland-client 0.31, wayland-protocols-wlr 0.3), Plymouth script plugin, labwc 0.9.3 theme files, systemd units, bash tests, QEMU smoke tests, dnsmasq (host, test only).

**Spec:** `docs/superpowers/specs/2026-10-21-thinfloe-design.md`, section "Part 4: New look" (commit b621192), and its mockups in `docs/superpowers/specs/2026-10-21-thinfloe-mockups/` (`brand-and-boot.html`, `desktop.html`, `settings.html`, `folded-strip.html`). Read the mockup sources: their CSS carries the sizes and colours this plan refers to.

## Global Constraints

- Palette, verbatim from the spec: accent ice blue `#7cc4ff`; background `#05070a`; tiles `#0d1218`; active tile `#101b26`. From the mockups: hover `#131c26`, lines/borders `#151b22`, muted text `#7d8996`, time grey `#5d6976`, warning amber `#c79a4a`, success green `#98c379`.
- Logo paths, verbatim (unslanted, baseline y = 632): T `M278 388H625V447H433V632H360V447H278Z`, F `M498 502H624V562H570V632H498Z`, slant `translate(0 632) skewX(-9) translate(0 -632)`, pure white.
- Font: Geo oblique, `Geo-Oblique.ttf` from google/fonts `ofl/geo/` (OFL; ship the licence text next to it). Geo for the name, titles and tile names; small text a plain sans; the log monospace.
- The menu draws on the CPU: no `wgpu`, `glow`, `eframe`, no GL/Vulkan library (Standing Constraint).
- Zero labwc keybindings; `rc.xml` keeps an empty `<keyboard>`.
- Icons from tile files still render only in the `--render-icon` child process. The logo is a constant compiled into the binary and may render in-process.
- No site-specific content in the image: no time zone, no host names.
- The version is shown only when `/etc/thinfloe/version` exists; nothing in Part 4 writes it.
- Time zone order: `TZ` from `~/.thinfloe-environment` wins for the session; else the DHCP zone (option 101); else UTC.
- CHANGES.md: user-facing wording under `## [Unreleased]`, `### New` / `### Changed`, at most three sentences per entry, no release header.
- Machine rules: at most 4 cores (`cargo … -j4`, `make` with the defaults); podman; every `make` runs as `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=look <target>`; every `cargo test` runs as `systemd-run --user --scope -p MemoryMax=2G -- cargo test -j4 --locked` in `src/thinfloe-menu`; `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`; never `rm` without asking the owner; never push.
- Work in the worktree `/scratch/oetiker/claude-worktrees/dbrrg-look` on branch `thinfloe-look` (switch the primary checkout back to `main` first). `artifacts/` is a symlink to the shared `/home/oetiker/scratch/dbrrg-artifacts`: record sizes right after your own build.
- `make test` needs `THINLINC_DEB=/scratch/oetiker/claude-tmp/thinfloe-spike/dl/thinlinc-client_4.21.0-4588_amd64.deb OXULNK_DEB=/home/oetiker/checkouts/dbrrg/vendor/oxulnk-desktop.deb`, and stops at the first failing script.
- Look at the screen, not only at unit tests: after each menu task take a screenshot in the runtime-test container (`labwc -C /etc/thinfloe/labwc -S "sh -c \"(timeout 10 thinfloe-menu &) ; sleep 5; grim /out/m.png\""`, `WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`) and compare it with the mockup. Attach the PNG path to the task report.

Baseline (main @ 848bd2b): `thinfloe-usb.img.zst` 608.5 MB, `ramroot.sqsh` 500.1 MB, `initrd.img` 93.9 MB. Re-measure at the start of Task 6 from your own build.

## Review Focus

1. **A tile name or window title Geo has no glyph for** (umlauts, CJK, emoji): it must draw through the fallback font, never as boxes. Test in Task 1.
2. **Many folded windows** (ten ThinLinc sessions folded on a 1280x720 screen): every one must stay reachable; no chip may fall off screen unreachable. Test in Task 4.
3. **A window that closes while folded, or changes its title while folded:** the chip goes away or is renamed at once; clicking a chip whose window just closed does nothing harmful. Test in Task 3 (tracker) and Task 4.
4. **The DHCP time zone arrives after the menu started:** the header clock and new log lines switch zone within a minute, without a restart. glibc caches `/etc/localtime` by the `TZ` string, so `localtime_r` alone does not see the change. Test in Task 9.
5. **A garbage or hostile option 101 value** (`../../etc/passwd`, an unknown zone, 300 bytes): nothing is changed and one line says why. Test in Task 9.

---

### Task 1: Brand module: palette, fonts, logo

**Files:**
- Create: `src/thinfloe-menu/src/brand.rs`
- Create: `src/thinfloe-menu/assets/Geo-Oblique.ttf`, `src/thinfloe-menu/assets/Geo-OFL.txt`
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/app.rs` (every `egui_shadcn::Theme::dark()`), `src/thinfloe-menu/src/ui.rs` (`render_icons` uses `Theme::dark()`), `src/thinfloe-menu/Cargo.toml` only if a dependency is needed

**Interfaces:**
- Produces:
  - `pub const ICE: Color32`, `BG`, `TILE`, `TILE_ACTIVE`, `TILE_HOVER`, `LINE`, `MUTED`, `TIME`, `WARN`, `GOOD` (values from Global Constraints; `ui::WARN` becomes a re-export or is replaced by `brand::WARN`).
  - `pub fn theme() -> egui_shadcn::Theme` — `Theme::dark()` with `background = BG`, `card = TILE`, `accent = TILE_HOVER`, `ring = ICE`, `border = LINE`, `muted_foreground = MUTED`.
  - `pub fn apply(ctx: &egui::Context)` — applies `theme()` and installs the fonts once per context, after egui_shadcn's own font install so that ours win.
  - `pub const GEO: &str = "geo"` — the font family name; `pub fn geo(size: f32) -> FontId`.
  - `pub fn logo(height_px: u32) -> ColorImage` — the slanted TF monogram, white on transparent, rendered with resvg from a constant SVG built from the Global Constraints paths.

- [ ] **Step 1: Fetch Geo-Oblique.ttf and its OFL text** from `https://raw.githubusercontent.com/google/fonts/<commit>/ofl/geo/` at a fixed commit; put the commit id and the sha256 of the TTF in the commit message.

- [ ] **Step 2: Write the failing tests in `brand.rs`**

```rust
#[test]
fn the_palette_is_the_spec_palette() {
    assert_eq!(ICE, Color32::from_rgb(0x7c, 0xc4, 0xff));
    assert_eq!(BG, Color32::from_rgb(0x05, 0x07, 0x0a));
    assert_eq!(TILE, Color32::from_rgb(0x0d, 0x12, 0x18));
    assert_eq!(TILE_ACTIVE, Color32::from_rgb(0x10, 0x1b, 0x26));
    let t = theme();
    assert_eq!((t.palette.background, t.palette.card, t.palette.ring), (BG, TILE, ICE));
}

#[test]
fn geo_falls_back_for_glyphs_it_lacks() {
    let ctx = egui::Context::default();
    apply(&ctx);
    // fonts land one frame late
    let _ = ctx.run_ui(Default::default(), |_| {});
    ctx.fonts(|f| {
        let fam = f.definitions().families.get(&egui::FontFamily::Name(GEO.into())).unwrap().clone();
        assert_eq!(fam[0], "Geo-Oblique");
        assert!(fam.len() >= 2, "Geo needs a fallback: {fam:?}");
        assert!(f.has_glyphs(&geo(20.0), "Über 東京"), "fallback must cover umlauts and CJK");
    });
}

#[test]
fn the_logo_is_white_slanted_and_about_three_by_two() {
    let img = logo(52);
    assert_eq!(img.size[1], 52);
    let w = img.size[0] as f32 / 52.0;
    assert!((1.2..1.8).contains(&w), "aspect {w}");
    assert!(img.pixels.iter().any(|p| *p == Color32::WHITE));
    assert!(img.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b()), "white only");
}
```

If `has_glyphs` does not exist in egui 0.36, assert through `f.glyph_width` on each character falling back to a font that has it; the assertion stays "no replacement glyph for Ü and 東". Pick the CJK-capable fallback from what egui `default_fonts` carries; if nothing covers CJK, drop 東京 from the test and say so in the report.

- [ ] **Step 3: Run them**, expect FAIL (module missing).

Run: `cd src/thinfloe-menu && systemd-run --user --scope -p MemoryMax=2G -- cargo test -j4 --locked brand::`

- [ ] **Step 4: Implement `brand.rs`.** Fonts: Proportional = egui's built-in proportional font(s) (plain sans) in place of Oxanium; Monospace unchanged; family `geo` = `[Geo-Oblique, <egui proportional fonts…>, <emoji>]`. Replace every `egui_shadcn::Theme::dark()` / `.apply()` in `app.rs` and `ui.rs` with `brand::theme()` / `brand::apply()`.

- [ ] **Step 5: Run the whole crate's tests**, expect PASS.

Run: `cd src/thinfloe-menu && systemd-run --user --scope -p MemoryMax=2G -- cargo test -j4 --locked`

- [ ] **Step 6: Commit** `menu: ThinFloe palette, Geo font and logo`.

---

### Task 2: Desktop C: header, wide tiles, running edge, log colours

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`layout`, `Layout`, `paint_tile`, `show`, `show_log`, `log_job`, tests), `src/thinfloe-menu/src/app.rs` (logo texture in `Live`, clock repaint), `src/thinfloe-menu/src/menu.rs` (`running` becomes `pub`)
- Create: `src/thinfloe-menu/src/header.rs`

**Interfaces:**
- Consumes: Task 1's `brand::*`.
- Produces:
  - `pub struct Header { pub host: String, pub version: Option<String> }` and `pub fn read_header(version_file: &Path) -> Header` in `header.rs` — host from `gethostname`; version = first line of the file, trimmed, at most 32 characters, `None` when missing, unreadable or empty. `main.rs` passes `TF_VERSION_FILE` or `/etc/thinfloe/version`.
  - `pub fn clock_text(now: SystemTime) -> String` (`HH:MM`, local time) in `header.rs`; Task 9 changes how local time is found, not this signature.
  - `pub fn header_text(h: &Header, clock: &str) -> String` — `host · version · clock`, the version and its separator left out when `None`.
  - `Layout` gains `header: Rect`; tiles are bands: `Layout.side` is replaced by `tile: Vec2`.
  - `Menu::running(&self, tile: usize) -> usize` is `pub`.

Geometry (at 1080p, points; scale from the mockup `desktop.html`): header row at the top, logo 52 px tall and "ThinFloe" in Geo 34 at 60 % opacity on the left, `header_text` in Geo 16 in `MUTED` on the right; the tile group is 4 columns, 3 px gaps, 58 % of the screen width, centred in the space between header and log; a tile is at least 64 tall with icon 44 left (upright), name in Geo 21, comment in plain sans 11 in `MUTED`; running = a 5 px `ICE` edge on the left and `TILE_ACTIVE` fill; two or more copies add `×N` in Geo 15 `ICE` in the top-right corner; a pending add-on update keeps a visible "update" marker (builder's call how, in the palette); disabled tiles at 35 % opacity, their reason cut to one line in `WARN` (the full reason stays in `--check`); a user tile keeps a small "user" mark. The log keeps its 30 % share and colour roles: time `TIME`, source `ICE`, warnings `WARN`, success `GOOD`.

- [ ] **Step 1: Write the failing tests** in `ui.rs` / `header.rs` (replace the square-tile tests `seven_tiles_on_full_hd_are_square_centred_and_above_the_log`, `the_shipped_seven_tiles_fit_above_the_log_on_1280_by_720`, `eight_tiles_fit_1280x720`, `tiles_shrink_to_fit_but_not_below_the_minimum`):

```rust
#[test]
fn eight_tiles_on_full_hd_are_two_rows_of_bands_at_58_percent() {
    let l = layout(screen(1920.0, 1080.0), 8);
    assert!((l.grid.width() - 0.58 * 1920.0).abs() < 1.0);
    assert!((l.grid.center().x - 960.0).abs() < 1.0);
    assert!(l.tile.x > l.tile.y * 2.5, "bands, not squares: {:?}", l.tile);
    assert!(l.tile.y >= 64.0);
    assert!(l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top());
}

#[test]
fn nine_tiles_fit_between_header_and_log_on_1280_by_720() {
    let l = layout(screen(1280.0, 720.0), 9);
    assert!(l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top());
}

#[test]
fn the_header_leaves_out_an_unknown_version() {
    let h = Header { host: "thinfloe-3f9a2c".into(), version: None };
    assert_eq!(header_text(&h, "14:32"), "thinfloe-3f9a2c · 14:32");
    let h = Header { version: Some("4.0.0".into()), ..h };
    assert_eq!(header_text(&h, "14:32"), "thinfloe-3f9a2c · 4.0.0 · 14:32");
}

#[test]
fn a_version_file_is_read_trimmed_and_capped() { /* missing → None; "" → None; "4.0.0\nx" → "4.0.0"; 100 chars → 32 */ }

#[test]
fn a_tile_with_two_copies_shows_the_edge_and_the_count() { /* paint a running tile via ui::show into a test canvas as a_running_tile_shows_its_status does; assert ICE pixels in the left 5 px column and the text "×2" in the shapes */ }
```

Adapt `a_running_tile_shows_its_status` and `the_painted_grid_is_where_the_layout_puts_it_with_centred_content` to the band layout rather than deleting them.

- [ ] **Step 2: Run them**, expect FAIL.
- [ ] **Step 3: Implement** the layout, header, tile painting and log colours. The clock asks for a repaint at the next minute boundary (`request_repaint_after`), nothing more often. The logo texture is made once in `resumed` from `brand::logo` at the scaled height.
- [ ] **Step 4: Run the crate's tests**, expect PASS.
- [ ] **Step 5: Screenshot** in the runtime-test container (Global Constraints recipe) at 1280x720 and 1920x1080; compare with `desktop.html`.
- [ ] **Step 6: Commit** `menu: desktop C layout with header, band tiles and running edge`.

---

### Task 3: Window watcher: folded windows from labwc, `--windows`, `--unfold`

**Files:**
- Create: `src/thinfloe-menu/src/windows.rs`
- Modify: `src/thinfloe-menu/Cargo.toml` (`wayland-client = "0.31"`, `wayland-protocols-wlr = { version = "0.3", features = ["client"] }`, same versions as already in `Cargo.lock` through winit; `cargo tree -d` must show no second copy), `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/main.rs`, `test/runtime/Dockerfile` (`wlrctl`), `test/runtime/test-labwc-runtime.sh`

**Interfaces:**
- Produces:
  - `pub struct Folded { pub id: u64, pub title: String, pub app_id: String }`
  - `pub enum Event { Title(u64, String), AppId(u64, String), Minimized(u64, bool), Done(u64), Closed(u64) }` and `pub struct Tracker` with `pub fn apply(&mut self, e: Event) -> bool` (true when the folded list changed) and `pub fn folded(&self) -> Vec<Folded>`. Pure, no Wayland: values become visible only at `Done` (the protocol's double-buffering); app_id `thinfloe-menu` is never listed; order is the order windows were folded, oldest first.
  - `pub struct Windows` with `pub fn connect(on_change: impl Fn() + Send + 'static) -> Result<Windows, String>` (own `wayland_client::Connection::connect_to_env`, binds `zwlr_foreign_toplevel_manager_v1` and `wl_seat`, dispatches on a thread of its own and calls `on_change` when `Tracker::apply` returns true), `pub fn folded(&self) -> Vec<Folded>`, `pub fn restore(&self, id: u64)` (`unset_minimized` then `activate(seat)`, then flush; an unknown id does nothing). `connect` returns `Err` when the global is missing; the caller then never shows a strip.
  - CLI in `main.rs`: `thinfloe-menu --windows` prints one line per toplevel, `app_id<TAB>title<TAB>folded|open`, exit 0, or exit 1 with the reason when the protocol is missing; `thinfloe-menu --unfold <app_id or title>` restores every folded window whose app_id or title equals the argument, exit 0 if it restored at least one, else 1. For an operator at a VT or over SSH (`WAYLAND_DISPLAY=wayland-0`) when a window went missing, and for the runtime test.
  - With `TF_MENU_DEBUG` the menu prints `thinfloe-menu: folded: <app_id> <title>` / `thinfloe-menu: unfolded: …` when the list changes.

- [ ] **Step 1: Write the failing tracker tests** in `windows.rs`:

```rust
#[test]
fn a_window_is_listed_only_after_done() {
    let mut t = Tracker::default();
    assert!(!t.apply(Event::Title(1, "ThinLinc – lab".into())));
    assert!(!t.apply(Event::Minimized(1, true)));
    assert!(t.folded().is_empty());
    assert!(t.apply(Event::Done(1)));
    assert_eq!(t.folded()[0].title, "ThinLinc – lab");
}

#[test]
fn the_menu_itself_is_never_listed() {
    let mut t = Tracker::default();
    for e in [Event::AppId(2, "thinfloe-menu".into()), Event::Minimized(2, true), Event::Done(2)] { t.apply(e); }
    assert!(t.folded().is_empty());
}

#[test]
fn closing_or_unfolding_removes_the_chip_and_a_title_change_renames_it() { /* fold 1, rename while folded → title updated after Done; Minimized(false)+Done → gone; fold again then Closed → gone; Closed on unknown id → false */ }

#[test]
fn folded_windows_keep_the_order_they_were_folded_in() { /* fold 3, then 1, then 2 → ids [3,1,2] */ }
```

- [ ] **Step 2: Run them**, expect FAIL.
- [ ] **Step 3: Implement `Tracker`, `Windows` and the two CLI flags.**
- [ ] **Step 4: Run the crate's tests**, expect PASS.
- [ ] **Step 5: Extend `test/runtime/test-labwc-runtime.sh`** with a block that starts `foot`, then `thinfloe-menu` with `TF_MENU_DEBUG=1`, folds foot with `wlrctl toplevel minimize app_id:foot`, and asserts: the menu printed `folded: foot`; `thinfloe-menu --windows` lists foot as `folded`; `thinfloe-menu --unfold foot` exits 0; `--windows` then lists it as `open`; the menu printed `unfolded: foot`. Add `wlrctl` to `test/runtime/Dockerfile` (check it is in the 26.04 archive; if not, fold the window with a few lines of Python via `pywayland` in the test image only, never in the product).

Run: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=look test-runtime`
Expected: the new `ok` lines, rc 0.

- [ ] **Step 6: Commit** `menu: watch folded windows through wlr-foreign-toplevel`.

---

### Task 4: The folded-window strip

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`Layout.strip: Option<Rect>`, `UiEvent::Unfold(u64)`, `show_strip`), `src/thinfloe-menu/src/menu.rs` (`pub folded: Vec<Folded>`), `src/thinfloe-menu/src/app.rs` (`Wake::Windows`, `Windows::connect` in `run`, `restore` on `UiEvent::Unfold`)

**Interfaces:**
- Consumes: Task 3's `Windows`, `Folded`; Task 2's `layout`.
- Produces: `pub fn layout(screen: Rect, tiles: usize, folded: usize) -> Layout`; `Layout.strip` is `None` when `folded == 0`, else a row at the bottom edge below the log, and the log ends above it.

Chips as in `folded-strip.html`: `TILE` fill, 1 px `LINE` border, icon then title in plain sans, no label. Chip icon (my call within the spec's choice): the icon of the tile whose `Exec` program name equals the window's app_id, case-insensitive, else the first letter as tiles draw it; no new icon lookups at runtime. Long titles are cut with an ellipsis at a maximum chip width. When the chips do not fit one row the strip scrolls horizontally (mouse wheel and drag), so every chip stays reachable. While a logout dialog is up the strip takes no input (input is refused as now). A click on a chip whose window closed meanwhile does nothing (`restore` of an unknown id).

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn no_folded_window_means_no_strip_and_the_log_reaches_the_bottom() {
    let l = layout(screen(1920.0, 1080.0), 8, 0);
    assert!(l.strip.is_none());
    assert!(1080.0 - l.log.bottom() <= GAP + 0.5);
}

#[test]
fn a_folded_window_puts_the_strip_below_the_log() {
    let l = layout(screen(1920.0, 1080.0), 8, 2);
    let s = l.strip.unwrap();
    assert!(s.top() >= l.log.bottom() && s.bottom() <= 1080.0);
}

#[test]
fn ten_folded_windows_on_1280_by_720_stay_reachable() { /* draw the strip with ten chips of long titles; scroll it to the end via egui input events; assert the last chip's rect is inside the strip and a click there yields UiEvent::Unfold(last id) */ }

#[test]
fn a_chip_click_restores_that_window() { /* menu.folded = [two]; click the second chip's rect → UiEvent::Unfold(second id) */ }
```

- [ ] **Step 2: Run them**, expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the crate's tests**, expect PASS.
- [ ] **Step 5: Screenshot** with two folded foot windows (fold with `wlrctl`) and compare with `folded-strip.html`; one more with nothing folded.
- [ ] **Step 6: Commit** `menu: strip of folded windows below the log`.

---

### Task 5: Settings screen and dialogs in the new style

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`show_settings`, `settings_row`, `dialog`)

As in `settings.html`, in the palette of Task 1: page title and add-on names in Geo, small text plain sans, tick box and buttons in the palette (`ICE` for the primary button, `WARN` for "updates only by hand"), rows as `TILE` bands with 3 px gaps. Dialogs: `TILE` card with a `LINE` border, title in Geo, destructive buttons keep their red. The state machine and every text stay as they are.

- [ ] **Step 1: Adapt the existing tests** `the_settings_screen_draws_a_row_per_addon`, `the_retry_dialog_names_the_addon_and_why`, `the_dialog_names_the_way_the_menu_ends` if layout changes break them; add `the_dialog_title_is_drawn_in_geo` (the title's galley uses `FontFamily::Name(GEO)`).
- [ ] **Step 2: Run them**, expect the new one to FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the crate's tests**, expect PASS.
- [ ] **Step 5: Screenshot** the Settings screen (`TF_ADDON_STATUS` pointing at a status file with one installed, one failed and one manual add-on) and the logout dialog; compare with `settings.html`.
- [ ] **Step 6: Commit** `menu: Settings and dialogs in the ThinFloe style`.

---

### Task 6: waybar and GTK 3 leave the image

**Files:**
- Modify: `containers/ubuntu/Dockerfile` (drop `waybar` and the `systemctl --global disable waybar.service` block), `overlay/usr/bin/thinfloe-session` (drop the waybar block; keep `kill_helpers` for swayidle), `test/integration/test-session-packages.sh` (waybar lines become `absent`; add `absent "GTK 3" 'usr/lib/x86_64-linux-gnu/libgtk-3\.so\.0$'`), `CLAUDE.md` (Taskbar bullet, `rc.xml` note on `skipTaskbar`), `CHANGES.md`
- Delete (ask the owner first, per machine rules): `overlay/etc/thinfloe/waybar/`

- [ ] **Step 1: Measure the baseline** from your own `make VERSION=look image` before the change: `ramroot.sqsh`, `thinfloe-usb.img.zst` bytes.
- [ ] **Step 2: Change the tests**, run `make test` on the old image, expect FAIL at the waybar line.
- [ ] **Step 3: Remove waybar.** If `libgtk-3` stays because another package depends on it, find out which (`apt-cache rdepends --installed libgtk-3-0t64` in the built container) and report; do not remove that package without asking.
- [ ] **Step 4: Rebuild and run** `make VERSION=look image`, then `make VERSION=look test` with the debs, `test-runtime`, `qemu-smoke`. Expected: rc 0 each.
- [ ] **Step 5: CHANGES.md** `### Changed`: the taskbar at the bottom of the screen is gone; a folded window now shows in a strip at the bottom of the menu and comes back with one click. Mention the download size change compared with the last 3.x release.
- [ ] **Step 6: Commit** `image: drop waybar and GTK 3`, with the byte deltas in the message.

---

### Task 7: labwc title bars in the ThinFloe palette

**Files:**
- Create: `overlay/etc/thinfloe/labwc/themerc-override`, `overlay/usr/share/fonts/truetype/thinfloe/Geo-Oblique.ttf` (same file as Task 1; the build may copy it from the crate instead of a second copy in git, builder's call), `overlay/usr/share/doc/thinfloe/Geo-OFL.txt`
- Modify: `overlay/etc/thinfloe/labwc/rc.xml` (`<theme><font place="ActiveWindow">` / `InactiveWindow` = Geo; no `<keybind>`), `overlay/usr/bin/thinfloe-compose-labwc-config` only if it does not already link every file of the system dir, `test/integration/test-session-packages.sh`, `test/integration/test-labwc-config-merge.sh`

Title bars: `BG`-family colours, active title `#e6edf3`, inactive `MUTED`, active border `ICE`; buttons as labwc draws them, coloured to match (builder's call within the palette).

- [ ] **Step 1: Write the failing assertions:** `themerc-override` is in the image and names `#7cc4ff`; `rc.xml` names the Geo font and still has zero `<keybind>`; the merged config dir links `themerc-override`; `fc-list` in the built image lists Geo.
- [ ] **Step 2: Run `make test`**, expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Rebuild; run `make test`, `make test-runtime`**, expect rc 0. Screenshot a `foot` window with its title bar.
- [ ] **Step 5: Commit** `labwc: title bars in the ThinFloe palette`.

---

### Task 8: Boot screen B (Plymouth)

**Files:**
- Create: `overlay/usr/share/plymouth/themes/thinfloe/thinfloe.plymouth`, `thinfloe.script`, `brand/logo-wordmark.svg` (logo plus "ThinFloe" as outlines), `scripts/wordmark-svg.py` (makes the outlines from `Geo-Oblique.ttf` with fontTools; run by hand, documented in its header)
- Modify: `containers/ubuntu/Dockerfile` (render the PNGs from the SVG in a build stage with `rsvg-convert`, so neither `librsvg2-bin` nor the SVG tool lands in the final image; register the theme with `update-alternatives` as `default.plymouth`; drop the `bgrt` fallback in the `plymouth-set-default-theme` shim), `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (`tf_write_hostname` also runs `plymouth display-message --text="$name"` when `plymouth --ping` succeeds), `overlay/usr/lib/dracut/modules.d/90thinfloe/module-setup.sh` (install the Geo TTF where the label plugin finds it), `test/integration/test-initramfs-commands.sh`, `test/integration/test-initramfs-home.sh`

As in `brand-and-boot.html`: logo and wordmark centred, a thin ice-blue progress line slanted 9° below it, black background with a faint ice-blue radial glow from the bottom, the host name small in Geo at the foot (no version until Part 5). The script shows the host name from `Plymouth.SetDisplayMessageFunction`. *Builder must check:* which label plugin Ubuntu's Plymouth uses and where it takes its font from (label-freetype reads a fixed path such as `/usr/share/fonts/Plymouth.ttf`), that `plymouth-populate-initrd` or dracut's plymouth module copies the theme, the script plugin and that font into the initrd, and what the screen shows with no host name yet (the foot line stays empty).

- [ ] **Step 1: Write the failing assertions** in `test-initramfs-commands.sh`: the initrd holds `usr/share/plymouth/themes/thinfloe/thinfloe.script`, its PNGs and the script plugin; `default.plymouth` in the initrd points at the thinfloe theme. In `test-initramfs-home.sh`: `tf_write_hostname` calls `plymouth display-message` with the name when a stub `plymouth --ping` succeeds, and does not fail when `plymouth` is missing.
- [ ] **Step 2: Run them** (`make test` on the current image), expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Rebuild; run `make test`, `qemu-smoke`, `qemu-smoke-upgrade`**, expect rc 0. Take a QEMU screendump during boot (QEMU monitor `screendump`, a few seconds after the kernel starts) and compare with `brand-and-boot.html`; attach the PNG path.
- [ ] **Step 5: Commit** `boot: ThinFloe Plymouth theme`.

---

### Task 9: Time zone: per machine, from DHCP, live in the clock

**Files:**
- Create: `overlay/etc/systemd/network/10-netplan-eth.network.d/50-timezone.conf` (`[DHCPv4]` `UseTimezone=yes`; `eth` is the id in `overlay/etc/netplan/ethernet.yaml`), `overlay/usr/libexec/thinfloe/dhcp-timezone`, `overlay/etc/systemd/system/thinfloe-dhcp-timezone.path`, `overlay/etc/systemd/system/thinfloe-dhcp-timezone.service`, `test/integration/test-dhcp-timezone.sh`, `scripts/run-qemu-timezone-smoke.sh`
- Modify: `overlay/home/tfuser/.thinfloe-environment` (a commented `#TZ=Europe/Zurich` block), `src/thinfloe-menu/src/header.rs` and `src/thinfloe-menu/src/log.rs` (local time that follows a zone change), `containers/ubuntu/Dockerfile` (enable the path unit), `Makefile` (`qemu-smoke-timezone`), `test/integration/test-session-packages.sh`, `CLAUDE.md`, `CHANGES.md`

Mechanism (my call; the spec asks only for a working path): networkd asks for option 101 through the drop-in; its own attempt to set the zone through `timedated` needs polkit, which the image lacks, so the path unit watches `/run/systemd/netif/leases/` and the root service runs `dhcp-timezone`, which reads `TIMEZONE=` from the lease files and, if the value names a file under `/usr/share/zoneinfo/` (letters, digits, `/ _ + -` only, no `..`, at most 64 characters, not a directory), runs `timedatectl set-timezone` and prints `thinfloe: time zone <zone> from DHCP`; anything else prints `thinfloe: ignored DHCP time zone '<cut value>': <why>` and changes nothing. If the first QEMU run shows networkd setting the zone by itself, keep the script anyway (it is the logged, validated path) and note it. WiFi profiles from `~/wifi.yaml` have user-chosen ids and get no drop-in; record this in CLAUDE.md.

The menu must show a zone change within a minute: glibc's `tzset` returns early while `TZ` is unchanged, so `localtime_r` keeps the old zone. Use a time zone source that re-reads the system zone (for example the `jiff` crate with `tz-system`, which honours `TZ` and re-checks `/etc/localtime`), for both `clock_text` and the log's timestamps. Programs started before the change keep their zone; that is accepted.

- [ ] **Step 1: Write `test/integration/test-dhcp-timezone.sh`** (offline; fake lease dir, fake zoneinfo dir and a stub `timedatectl` on PATH, `dhcp-timezone` takes both dirs from environment variables with the real paths as defaults): `Europe/Zurich` → stub called with it and the success line; `../../etc/passwd`, `Mars/Olympus`, a 300-byte value, a directory name (`Europe`) → stub not called, one `ignored` line each; no lease with `TIMEZONE=` → silent, exit 0.
- [ ] **Step 2: Write the menu test** `a_zone_change_reaches_the_clock_without_a_restart` (point the zone source at a temp zoneinfo/localtime pair if the chosen crate allows it; otherwise assert through `TZ` set before the first call and document why the file-change case is covered by the QEMU smoke only).
- [ ] **Step 3: Run both**, expect FAIL.
- [ ] **Step 4: Implement** the drop-in, units, script, menu change and the commented `TZ` block.
- [ ] **Step 5: Write `scripts/run-qemu-timezone-smoke.sh` and `make qemu-smoke-timezone`:** re-exec under `unshare -rn` (as `run-qemu-netboot-smoke.sh` does), create a tap device, run the host's `dnsmasq` on it unprivileged with `dhcp-option=101,Europe/Zurich`, boot the USB image with `-netdev tap`, and require the serial log line `thinfloe: time zone Europe/Zurich from DHCP` and no `Found ordering cycle`. If a tap device cannot be created in the user namespace on this host, stop and report rather than weaken the test.
- [ ] **Step 6: Rebuild; run** `make test`, `qemu-smoke`, `qemu-smoke-netboot`, `qemu-smoke-timezone`, expect rc 0.
- [ ] **Step 7: Docs.** CLAUDE.md: where the zone comes from and why the script exists (polkit). CHANGES.md `### New`: the clock in the menu; it shows the time zone set with `TZ=` in `~/.thinfloe-environment` or sent by the DHCP server, else UTC.
- [ ] **Step 8: Commit** `time zone from ~/.thinfloe-environment or DHCP`.

---

### Task 10: Documentation and the full gate run

**Files:**
- Modify: `CLAUDE.md` (session/compositor and taskbar bullets, the Menu limits list, the folded strip, `--windows` / `--unfold` under Debugging, Plymouth theme, Geo font), `CHANGES.md` (one entry for the new look: boot screen, desktop, Settings, title bars), `docs/superpowers/specs/2026-10-21-thinfloe-design.md` only if a builder's call needs recording

- [ ] **Step 1: Update the docs**; CHANGES entries name what the person at the machine sees.
- [ ] **Step 2: Run every gate from one fresh build** (a gate script like `/scratch/oetiker/claude-tmp/dbrrg-build/slim-gates.sh`, with `VERSION=look`): `image`, `test` (with the debs), `test-unit`, `test-runtime`, `qemu-smoke`, `qemu-smoke-netboot`, `qemu-smoke-upgrade`, `qemu-smoke-addons`, `qemu-smoke-timezone`, and the crate's `cargo test`. Expected: rc 0 each; record the test counts and the sizes of `thinfloe-usb.img.zst`, `ramroot.sqsh`, `initrd.img`.
- [ ] **Step 3: Record what is not verified on hardware** in CLAUDE.md: boot screen on real firmware, the strip and chip clicks with a real mouse, the DHCP zone from a real DHCP server.
- [ ] **Step 4: Commit** `docs: Part 4 new look`.
