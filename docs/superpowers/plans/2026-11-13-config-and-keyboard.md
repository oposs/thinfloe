# One Config Directory and Keyboard Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every ThinFloe-owned file of tfuser's home under `~/.config/thinfloe/`, keep everything the menu manages in one `config.toml` with derived files in `generated/`, migrate 4.x homes once into it (old files parked in `~/cfg-attic/`), then add a Keyboard tab that applies layout, variant, CapsLock-as-Ctrl and Compose to the running session.

**Architecture:** The menu crate gets a config layer (`config.rs` schema and TOML, `generated.rs` renderers, `convert.rs` old-format readers, `store.rs` the single writer with a lock, `migrate.rs` the one-time migration behind `thinfloe-menu --migrate-home`). The initramfs decides the home layout from the archive listing and writes `/run/thinfloe/state/home-layout`; every shell reader before the menu picks new or old paths from that file. Part B adds `kbregistry.rs` (evdev.xml), `keyboard.rs` (tab state, validation) and `keyboardtab_ui.rs`, applied through the composer's new `--env-only` mode and `labwc --reconfigure`.

**Tech Stack:** Rust 1.96 (egui 0.37, shadcn-egui 0.8 `lyra`), new direct deps `toml` + `serde` (derive) + `roxmltree` 0.21; POSIX sh (dracut module, composer, session); Python 3 (thinfloe-upgrade, thinfloe-network); podman builds via `make`.

**Spec:** [docs/superpowers/specs/2026-11-13-config-and-keyboard-design.md](../specs/2026-11-13-config-and-keyboard-design.md). Read it with this plan; the "Spec issues found" section at the end records where this plan resolves a contradiction, and the plan is written with those resolutions.

## Global Constraints

- Every rule in the worktree's `CLAUDE.md` applies; the ones this work touches are repeated here.
- Builds and tests use at most 4 cores: `CARGO_BUILD_JOBS=4`, `cargo … -j 4`, `make` passes `BUILD_JOBS`.
- Cargo tests run under a memory cap: `systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4 …` (this is `MEMCAP` in the Makefile). Any new test that reads files of unbounded size (registry XML, config.toml, old home files) reads through `bounded::read_bounded` with a fixed cap.
- Containers: `podman`, never docker. No `git grep` over the repo.
- New layout paths (spec table): `~/.config/thinfloe/config.toml`, `environment`, `sessionrc`, `generated/`, `menu/*.desktop` (unchanged), `addons` (unchanged), `password`, `ssh-host-keys/`, `wg0.conf`, `save-home-exclude`, `home-version`.
- `home-version` = `2`. `config.toml` has `version = 1`. State file `/run/thinfloe/state/home-layout` holds `legacy` or `new`.
- Generated files: `wifi.yaml`, `ethernet.yaml` (mode 0600), `kanshi`, `environment`, `screen`; plus `kanshi.prev` and `layout-unconfirmed` for the monitor undo. Written only when their content changes; generated files first, `config.toml` renamed last.
- Managed keys list: `/etc/thinfloe/menu-managed-keys` = `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_OPTIONS`, `XKB_DEFAULT_MODEL`.
- Merge order for labwc's environment: system, then `generated/environment`, then user `environment` minus managed keys.
- Keyboard defaults: layout `us`, no variant, CapsLock as Ctrl on, Compose on Right Alt. Compose choices and options: None (no option), Right Alt `compose:ralt`, Menu `compose:menu`, Right Ctrl `compose:rctrl`, Left Win `compose:lwin`, Right Win `compose:rwin`, Scroll Lock `compose:sclk`, Pause `compose:paus`. Model stays `pc105`.
- `generated/environment` always carries all three of `XKB_DEFAULT_LAYOUT=`, `XKB_DEFAULT_VARIANT=`, `XKB_DEFAULT_OPTIONS=`, an empty value included.
- Initramfs: shell only, only commands `module-setup.sh` installs, no `exit` in any hook, `tf_log` on stderr only. Netplan files mode 0600. The per-machine network config is installed by the initramfs, never by a unit. The home restore stays in the initramfs.
- Composer stays POSIX `sh`. labwc keeps zero keybindings; nothing writes `rc.xml` except `labwc-scale`.
- UI: theme colours, text sizes from named constants in `brand.rs`, shadcn-egui `lyra`, nothing rounded; every new UI file is added to the list in `brand.rs` `ui_code_has_no_style_literals`. No `set_var`/`remove_var` under `src/` (`tests/no_env_mutation.rs`).
- New overlay files and menu sources are picked up by the `find`-based `OVERLAY_FILES`/`OVERLAY_DIRS`/`MENU_FILES`/`MENU_DIRS` in the `Makefile` (lines 94-95, 122-123); no Makefile line is needed, but `test-container-inputs.sh` must stay green.
- CHANGES.md: entries under `## [Unreleased]` (`### New`, `### Changed`, `### Fixed` exist), written for users and administrators, lead with what they see, at most 3 sentences, no function/module names, no release headers.
- docs/notes/, README.md and CLAUDE.md change wherever they name an old path; CLAUDE.md keeps rules and pointers only.
- Commit messages: conventional (`feat:`, `fix:`, `docs:`, `test:`), ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

Commands used throughout (abbreviated in tasks):

- `MENUTEST` = `cd src/thinfloe-menu && CARGO_BUILD_JOBS=4 systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4`
- `MENULINT` = `cd src/thinfloe-menu && cargo fmt --check && CARGO_BUILD_JOBS=4 cargo clippy --locked -j 4 --all-targets -- -D warnings`

## Review Focus

1. **An unconfirmed monitor layout at migration time.** The profile runs the migration before `thinfloe-session` undoes an unconfirmed layout. Expected: with marker `prev` and a `config.prev`, `[monitors]` is converted from `config.prev` (the layout the session would put back); with marker `none`, `[monitors]` is empty. Test in Task A5.
2. **A migration that runs again after a partial earlier run** (`home-version` could not be written, some old files already in the attic, a valid `config.toml` present). Expected: no converted setting is lost (tables are taken from the existing `config.toml` where no old file is left to convert) and no attic file is overwritten (a second copy goes to `<name>.<epoch>`). Test in Task A5.
3. **Keyboard values that would break the environment file** (newline, `=`, space, quote) arriving from a migrated `~/.thinfloe-environment`, a rebuilt `config.toml` or a registry entry. Expected: such a layout or variant is refused (migration: dropped and reported; tab: never offered), never written into `generated/environment`. Tests in Tasks A1 and B1.
4. **A fresh install: no archive, no `config.toml`, empty `generated/`.** Expected: the menu starts with defaults, writes `config.toml`, `generated/environment` and `generated/screen`, writes no `generated/wifi.yaml` or `generated/ethernet.yaml`, and `thinfloe-menu --check` writes nothing at all. Tests in Task A4.
5. **A keyboard or screen change while a network Apply runs.** Expected: the per-table regeneration leaves the staged `generated/wifi.yaml` alone, and a failed apply puts back exactly the generated network files it staged. Test in Task A8.

---

## File Structure

New menu modules (all `pub mod` in `src/thinfloe-menu/src/lib.rs`):

| File | Responsibility |
|---|---|
| `config.rs` | `ThinfloeConfig` and its tables, TOML parse/print, `ConfigDir` paths, `Keyboard`/`Compose`, `xkb_options`, `dropped_env_lines` |
| `atomic.rs` | link-safe bounded read and temp-file-plus-rename write with a mode (replaces `owned::replace`/`read_bytes`) |
| `generated.rs` | render each generated file from its table, write-if-changed, `Table` → file names |
| `convert.rs` | readers of the old formats and of `generated/` into tables (shared by migration and broken-config rebuild) |
| `store.rs` | `ConfigStore`: open at start, broken-file handling, start-up regeneration, per-table update, staged writes for network and monitors |
| `migrate.rs` | `--migrate-home`: steps 1-4, attic, README, `home-version`, `home-layout` rewrite; `HomeLayout` |
| `kbregistry.rs` (Part B) | evdev.xml parser |
| `keyboard.rs` (Part B) | `KeyboardTab` state, events, validation, apply queue |
| `keyboardtab_ui.rs` (Part B) | the tab's two cards |

Removed: `owned.rs` (its callers move to `atomic.rs` and the store; `owned::HEADER` survives as `migrate::LEGACY_MENU_HEADER` for reading old menu files and as the first line of `config.toml`).

Shell and overlay files touched: `overlay/usr/lib/dracut/modules.d/90thinfloe/{thinfloe-lib.sh,setup-overlay.sh,module-setup.sh}`, `overlay/etc/profile.d/10-thinfloe-session.sh`, `overlay/usr/bin/{thinfloe-session,thinfloe-compose-labwc-config,thinfloe-save-home,thinfloe-upgrade,thinfloe-password,thinfloe-ssh-hostkeys,thinfloe-network}`, `overlay/etc/thinfloe/menu-managed-keys` (new), `overlay/home/tfuser/` (old templates removed, `.config/thinfloe/{home-version,environment,sessionrc}` added), `containers/ubuntu/Dockerfile` (Part B: `xkb-data`), `scripts/run-qemu-upgrade-smoke.sh`, `scripts/smoke-home-report.sh` (new).

---

# Part A: the config directory

### Task A1: `config.toml` schema and the keyboard model

**Files:**
- Create: `src/thinfloe-menu/src/config.rs`
- Modify: `src/thinfloe-menu/src/lib.rs` (add `pub mod config;`), `src/thinfloe-menu/Cargo.toml`, `src/thinfloe-menu/Cargo.lock`

**Interfaces:**
- Consumes: `kanshi::{Profile, OutputSet, Place, Side}`, `heads::{Mode, Rotation}`, `netfiles::{Net, Wired}` (types only).
- Produces:
  - `pub const CONFIG_HEADER: &str = "# Written by the ThinFloe menu. Edits here are replaced.";` (same text as today's `owned::HEADER`)
  - `pub struct ConfigDir { pub root: PathBuf }` with `fn from_home(home: &Path) -> ConfigDir` (`<home>/.config/thinfloe`), `fn from_env() -> ConfigDir` (`TF_CONFIG_DIR`, else `$HOME/.config/thinfloe`, else `/home/tfuser/.config/thinfloe`), and path methods `config_toml()`, `generated()`, `generated_file(name: &str)`, `environment()`, `sessionrc()`, `home_version()`, `password()`, `ssh_host_keys()`, `wg0()`, `save_home_exclude()`.
  - `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)] pub struct ThinfloeConfig { pub version: u32, pub keyboard: Keyboard, pub screen: ScreenCfg, pub status: StatusCfg, pub monitors: MonitorsCfg, pub network: NetworkCfg, pub notice: NoticeCfg }`, every table `#[serde(default)]`, unknown keys ignored.
  - `pub struct Keyboard { pub layout: String, pub variant: String, pub caps_as_ctrl: bool, pub compose: Compose }`; `Default` = `us`, `""`, `true`, `Compose::RightAlt`.
  - `pub enum Compose { None, RightAlt, Menu, RightCtrl, LeftWin, RightWin, ScrollLock, Pause }`, serialised as `"none" "ralt" "menu" "rctrl" "lwin" "rwin" "sclk" "paus"`; `pub const ALL: [Compose; 8]` in the spec's order; `fn option(self) -> Option<&'static str>`; `fn label(self) -> &'static str` ("None", "Right Alt", "Menu", "Right Ctrl", "Left Win", "Right Win", "Scroll Lock", "Pause"); `fn from_option(opt: &str) -> Option<Compose>`.
  - `pub fn xkb_options(kb: &Keyboard) -> String` — compose option first, then `ctrl:nocaps`, comma-joined; default gives `compose:ralt,ctrl:nocaps` (the order of `/etc/thinfloe/labwc/environment`).
  - `pub fn xkb_name_ok(s: &str) -> bool` — empty or 1-64 chars of `A-Z a-z 0-9 _ -`.
  - `pub struct ScreenCfg { pub idle_timeout: u32 }` (default `screentab::DEFAULT_IDLE` = 300, capped at `MAX_IDLE` 86400 on read).
  - `pub struct StatusCfg { pub lines: Option<Vec<String>> }` — the status layout file's lines (the `layout::to_text` syntax, one entry per line); `None` = the default list.
  - `pub struct MonitorsCfg { pub profiles: Vec<ProfileCfg>, pub kept: Option<String> }`, `pub struct ProfileCfg { pub menu_output: Option<String>, pub outputs: Vec<OutputCfg> }`, `pub struct OutputCfg { pub criteria: String, pub enabled: bool, pub mode: String, pub rotation: String, pub side: Option<String>, pub to: Option<String> }` (`mode` in `kanshi::mode_text` form, `rotation` one of `normal 90 180 270`, `side`/`to` absent for the origin); `fn to_profiles(&self) -> Result<Vec<kanshi::Profile>, String>` and `fn from_profiles(p: &[kanshi::Profile]) -> MonitorsCfg` (make `kanshi::parse_mode`, `kanshi.rs:316`, `pub` for the mode text). `kept`: see Spec issue 2.
  - `pub struct NetworkCfg { pub wifi_device: Option<String>, pub networks: Vec<NetCfg>, pub wifi_dns: Vec<String>, pub wired: WiredCfg, pub kept_wifi: Option<String>, pub kept_wired: Option<String> }`, `pub struct NetCfg { pub ssid: String, pub hidden: bool, pub password: Option<String> }`, `pub struct WiredCfg { pub mode: String /* "dhcp" | "static" */, pub address: Option<String>, pub prefix: Option<u8>, pub gateway: Option<String>, pub dns: Vec<String> }`; `fn nets(&self) -> Vec<netfiles::Net>`, `fn wired(&self) -> Result<netfiles::Wired, String>`, `fn set_wired(&mut self, w: &netfiles::Wired)`, `fn set_nets(&mut self, n: &[netfiles::Net])`. `wifi_dns` mirrors today's `WifiFile.dns` (the servers in the WiFi file, which differ from `wired.dns` on netboot).
  - `pub struct NoticeCfg { pub migration: Option<String> }` — the migration report the Settings screen shows until dismissed.
  - `pub fn parse(text: &str) -> Result<ThinfloeConfig, String>` (refuses `version` other than 1; refuses a keyboard layout/variant failing `xkb_name_ok`), `pub fn to_text(cfg: &ThinfloeConfig) -> String` (starts with `CONFIG_HEADER`, then `version = 1`).
  - `pub fn dropped_env_lines(user_env: &str, managed: &[String]) -> Vec<String>` — active lines of the user `environment` whose key (after optional leading blanks and `export `) is managed; same rule as the composer (Task A10).
  - `pub fn read_managed_keys(path: &Path) -> Vec<String>` (bounded, `#` comments and blanks skipped; missing file gives the four keys above).

- [ ] **Step 1: Add dependencies**

Run in `src/thinfloe-menu`: `cargo add toml serde --features serde/derive && cargo add roxmltree@0.21`. Check `Cargo.lock` reuses `serde 1.0.229`/`serde_derive` and `roxmltree 0.21.1`; commit the lock (the image build uses `--locked`).

- [ ] **Step 2: Write the failing tests in `config.rs`**

```rust
#[test] fn a_default_config_round_trips()            // parse(&to_text(&d)) == d; text starts with CONFIG_HEADER, has "version = 1"
#[test] fn each_table_round_trips()                  // one non-default value per table: keyboard {ch, de_nodeadkeys, false, Menu},
                                                     // screen 900, status Some(["clock 1x1","-battery 2x1"]),
                                                     // monitors one profile with origin + "right-of" output and menu_output,
                                                     // network: one hidden net with password "00012345" (stays a string), static wired 192.168.1.10/24 gw .1 dns [9.9.9.9],
                                                     // kept_wifi Some(text with a newline and quotes), notice Some("x")
#[test] fn xkb_options_order_and_empties()           // default -> "compose:ralt,ctrl:nocaps"; caps off + None -> ""; caps on + None -> "ctrl:nocaps"; caps off + Pause -> "compose:paus"
#[test] fn compose_maps_every_choice()               // ALL.map(option) == [None, "compose:ralt", "compose:menu", "compose:rctrl", "compose:lwin", "compose:rwin", "compose:sclk", "compose:paus"]; from_option round-trips
#[test] fn bad_xkb_names_are_refused()               // parse refuses layout "us\nX=1", "us de", "us=", "\"us\""; xkb_name_ok("de_nodeadkeys"), ok("")
#[test] fn an_unknown_version_is_refused()           // "version = 2" -> Err
#[test] fn unknown_keys_are_ignored()                // "[keyboard]\nlayout = \"de\"\nfoo = 1" parses, layout de
#[test] fn monitors_convert_to_kanshi_profiles_and_back() // from_profiles(&to_profiles()) identity on the sample; kanshi::render of both equal
#[test] fn dropped_env_lines_name_managed_keys_only() // "XKB_DEFAULT_LAYOUT=de\n  export XKB_DEFAULT_MODEL=pc104\n#XKB_DEFAULT_VARIANT=x\nXCURSOR_SIZE=32\n" -> the first two lines, verbatim
```

- [ ] **Step 3: Run them to see them fail** — `MENUTEST config::` → compile errors / FAIL.

- [ ] **Step 4: Implement `config.rs`** with serde derive on the structs above; `Compose` via `#[serde(rename_all…)]` or explicit renames to the eight strings.

- [ ] **Step 5: Run** — `MENUTEST config::` → PASS; `MENULINT` clean.

- [ ] **Step 6: Commit**

```bash
git add src/thinfloe-menu/Cargo.toml src/thinfloe-menu/Cargo.lock src/thinfloe-menu/src/config.rs src/thinfloe-menu/src/lib.rs
git commit -m "feat: config.toml schema for the menu's settings" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task A2: atomic file helpers and the generated files

**Files:**
- Create: `src/thinfloe-menu/src/atomic.rs`, `src/thinfloe-menu/src/generated.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/kanshi.rs:390-394` (`parse` accepts either header)

**Interfaces:**
- Consumes: `config::*` (A1), `kanshi::render`, `netfiles::{render_wifi, render_wired}`, `screentab::body`.
- Produces:
  - `atomic::read(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>>` — `None` when missing; a link or non-regular file is an error (as `owned::read_bytes` today).
  - `atomic::write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()>` — creates the parent dir, removes a stray `<path>.tmp`, `create_new` with `mode`, `sync_all`, rename (the body of today's `owned::replace`, `owned.rs:65-89`).
  - `atomic::remove(path: &Path) -> io::Result<()>` — missing is fine; a link is removed, never followed.
  - `generated::HEADER: &str = "# Generated by the ThinFloe menu from ~/.config/thinfloe/config.toml. Edits here are lost."`
  - `pub enum Table { Keyboard, Screen, Status, Monitors, Network, Notice }`; `fn files(self) -> &'static [&'static str]` = `["environment"]`, `["screen"]`, `[]`, `["kanshi"]`, `["wifi.yaml", "ethernet.yaml"]`, `[]`.
  - `pub struct Inputs { pub default_wired: String, pub netboot: bool }` (the text of `netfiles::DEFAULT_WIRED`).
  - `pub fn render(cfg: &ThinfloeConfig, name: &str, inp: &Inputs) -> Result<Option<String>, String>` — `None` means the file must be absent. `environment`: header + the three `XKB_DEFAULT_*=` lines. `screen`: header + `idle_timeout=N`. `kanshi`: header + `kanshi::render(profiles)`, or header + `kept` text; `None` when no profiles and nothing kept. `wifi.yaml`: header + `render_wifi(...)` or header + `kept_wifi`; `None` when there are no networks and nothing kept. `ethernet.yaml`: header + `render_wired(...)` or `kept_wired`; `None` for DHCP without servers (today's "remove" case).
  - `pub fn mode_of(name: &str) -> u32` — `0o600` for `wifi.yaml`, `ethernet.yaml`; `0o644` else.
  - `pub fn write_if_changed(dir: &Path, name: &str, body: Option<&str>) -> io::Result<bool>` — true when the file changed (written or removed); leaves an identical file alone, also its mtime.
  - `kanshi::parse(text)` accepts first line `migrate::LEGACY_MENU_HEADER` (today's `owned::HEADER`) or `generated::HEADER`. Until Task A5 exists, reference `config::CONFIG_HEADER`, which has the same text.

- [ ] **Step 1: Failing tests**

```rust
// atomic.rs
#[test] fn write_never_follows_a_tmp_symlink()      // moved from owned.rs:201, same assertions, via atomic::write
#[test] fn write_sets_the_mode()                    // atomic::write(p, b"x", 0o600) -> metadata mode & 0o777 == 0o600
#[test] fn read_refuses_a_link()                    // moved from owned.rs:340
// generated.rs
#[test] fn environment_has_all_three_keys()         // default -> "HEADER\nXKB_DEFAULT_LAYOUT=us\nXKB_DEFAULT_VARIANT=\nXKB_DEFAULT_OPTIONS=compose:ralt,ctrl:nocaps\n"
#[test] fn screen_file_reads_back()                 // render screen 900 -> screentab::parse_idle(text) == Some(900)
#[test] fn kanshi_file_parses_back()                // kanshi::parse(render(kanshi)) == profiles of the sample config
#[test] fn netplan_files_from_the_network_table()   // two nets -> wifi.yaml == HEADER + "\n" + render_wifi(..); no nets -> None; DHCP no dns -> ethernet None; static -> Some
#[test] fn kept_text_is_written_verbatim_under_the_header()
#[test] fn write_if_changed_leaves_an_equal_file_alone() // second call returns false and mtime unchanged; None removes and returns true
#[test] fn netplan_files_are_0600()                 // write_if_changed(dir, "wifi.yaml", ..) -> mode 0o600
```

- [ ] **Step 2: Run to fail** — `MENUTEST atomic:: generated::` → FAIL.
- [ ] **Step 3: Implement** `atomic.rs`, `generated.rs`, the `kanshi::parse` header change.
- [ ] **Step 4: Run** — `MENUTEST atomic:: generated:: kanshi::` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: generated files derived from config.toml`.

### Task A3: converters from the old formats and from `generated/`

**Files:**
- Create: `src/thinfloe-menu/src/convert.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Consumes: `netfiles::{parse_wifi, parse_wired, FileState, WifiFile, WiredFile}` (`netfiles.rs:297`, `:491`), `kanshi::parse`, `screentab::parse_idle`, `status::layout::parse` + `status::offered`, `config::*`.
- Produces (each returns the table plus report lines, English sentences naming the file):
  - `pub struct Converted<T> { pub value: T, pub notes: Vec<String> }`
  - `pub fn keyboard_from_env(text: &str) -> (Keyboard, Vec<String> /*other active lines*/, Vec<String> /*notes*/)` — reads `XKB_DEFAULT_LAYOUT/VARIANT/OPTIONS/MODEL`; a comma list keeps its first entry and notes it; `ctrl:nocaps` sets `caps_as_ctrl`, absent `XKB_DEFAULT_OPTIONS` keeps the default (on, ralt), a present one without `ctrl:nocaps` turns it off; `compose:*` from `Compose::from_option`; other options dropped and noted; a model other than `pc105` dropped and noted; a name failing `xkb_name_ok` dropped (default kept) and noted. Last assignment wins, as the shell does.
  - `pub fn idle_from_sessionrc(text: &str) -> (Option<u32>, String /*rest*/)` — the last `TF_IDLE_TIMEOUT=`/`export TF_IDLE_TIMEOUT=` assignment, else `DBRRG_IDLE_TIMEOUT`; value normalised as `thinfloe-session:142-155` does (digits after stripping blanks and `+`, leading zeros; not >0 → 0); the rest is the file without those lines.
  - `pub fn screen_from_file(text: &str) -> Converted<Option<u32>>` (via `parse_idle`; an unreadable value noted).
  - `pub fn status_from_file(bytes: &[u8]) -> Converted<StatusCfg>` (via `layout::parse`; its notes become report lines; a refused file gives `lines: None`).
  - `pub fn monitors_from_kanshi(text: &str) -> Converted<MonitorsCfg>` — `kanshi::parse` ok → `from_profiles`; else `kept = Some(text)` and a note "kept as written; the menu shows it read-only".
  - `pub fn network_from_files(wifi: Option<&str>, wired: Option<&str>) -> Converted<NetworkCfg>` — `parse_wifi`: `Template`/`Missing` → no networks; `manageable` → networks, device, dns; not manageable → `kept_wifi = Some(text)` and a note. Same for `parse_wired` → `wired`/`kept_wired`.
  - `pub fn from_generated(dir: &Path) -> Result<Option<ThinfloeConfig>, String>` — `Ok(None)` when `generated/` holds no file; else a config from `generated/environment` (via `keyboard_from_env`), `screen`, `kanshi`, `wifi.yaml`, `ethernet.yaml` (each with its `generated::HEADER` line stripped first); `[status]` stays default.

- [ ] **Step 1: Failing tests**

```rust
#[test] fn keyboard_from_a_hand_environment()  // "XKB_DEFAULT_LAYOUT=ch\nXKB_DEFAULT_VARIANT=de_nodeadkeys\nXKB_DEFAULT_OPTIONS=grp:alt_shift_toggle,compose:menu,ctrl:nocaps\nXCURSOR_SIZE=32\n"
                                               // -> Keyboard{ch, de_nodeadkeys, true, Menu}; other == ["XCURSOR_SIZE=32"]; one note names "grp:alt_shift_toggle"
#[test] fn two_layouts_keep_the_first()        // "XKB_DEFAULT_LAYOUT=us,de\nXKB_DEFAULT_VARIANT=,nodeadkeys" -> us, "" ; notes name both lists
#[test] fn a_model_other_than_pc105_is_dropped() // "XKB_DEFAULT_MODEL=pc104" -> note; "XKB_DEFAULT_MODEL=pc105" -> no note
#[test] fn options_without_nocaps_turn_caps_off() // "XKB_DEFAULT_OPTIONS=compose:ralt" -> caps_as_ctrl false, RightAlt
#[test] fn a_name_with_a_newline_is_dropped()  // "XKB_DEFAULT_LAYOUT=de x" -> layout "us", note
#[test] fn idle_from_sessionrc_takes_the_last()  // "TF_IDLE_TIMEOUT=60\nwlr-randr --output DP-1 --transform 90\nexport TF_IDLE_TIMEOUT=0900\n" -> Some(900), rest == "wlr-randr --output DP-1 --transform 90\n"
#[test] fn idle_from_sessionrc_reads_the_3x_name() // "DBRRG_IDLE_TIMEOUT=120" -> Some(120); "TF_IDLE_TIMEOUT=abc" -> Some(0)
#[test] fn an_eap_wifi_file_is_kept()          // a hand wifi.yaml whose AP has "auth:" -> kept_wifi == Some(text), networks empty, one note
#[test] fn a_hand_wifi_file_the_menu_can_keep_converts() // ssid "home" password "secret123" -> networks == [home], kept_wifi None
#[test] fn the_template_counts_as_no_wifi()    // overlay/home/tfuser/wifi.yaml text (include_str! of the old template copy in a fixture) -> no networks, no note
#[test] fn a_hand_kanshi_file_is_kept()        // "profile {\n output * enable\n}\n" -> kept Some
#[test] fn from_generated_is_none_for_an_empty_dir()
#[test] fn from_generated_reads_back_what_render_wrote() // render every file of a sample config into a dir, from_generated == sample minus [status] and [notice]
```

The template fixture: copy today's `overlay/home/tfuser/wifi.yaml` to `src/thinfloe-menu/src/testdata/wifi-template.yaml` before Task A11 deletes it.

- [ ] **Step 2: Run to fail** — `MENUTEST convert::` → FAIL.
- [ ] **Step 3: Implement `convert.rs`.**
- [ ] **Step 4: Run** — `MENUTEST convert::` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: read the old settings files into config tables`.

### Task A4: `ConfigStore`, the single writer

**Files:**
- Create: `src/thinfloe-menu/src/store.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Consumes: A1-A3; `bounded::read_bounded`.
- Produces:
  - `pub enum OpenMode { Session, ReadOnly }`
  - `pub struct ConfigStore { pub dir: ConfigDir, pub cfg: ThinfloeConfig, pub inputs: generated::Inputs, read_only: bool }`
  - `pub struct Opened { pub store: ConfigStore, pub notes: Vec<String> /* broken file, rebuild */, pub kanshi_changed: bool }`
  - `pub fn open(dir: ConfigDir, inputs: Inputs, mode: OpenMode, now_epoch: i64) -> Opened` — Session mode: parse `config.toml`; missing or unparsable → (unparsable only) rename to `config.toml.broken-<epoch>` and note "config.toml could not be read (…) and was kept as config.toml.broken-<epoch>; the settings were rebuilt from generated/"; then `convert::from_generated` → that config, else (generated empty) defaults; then regenerate every file (`write_if_changed`) and write `config.toml` if it changed or was missing; `kanshi_changed` = the regeneration changed `generated/kanshi`. ReadOnly mode (for `--check`): parse only, never writes, renames or regenerates; a broken or missing file gives defaults plus a note.
  - `pub type Shared = Arc<Mutex<ConfigStore>>`
  - `impl ConfigStore { pub fn update(&mut self, table: Table, f: impl FnOnce(&mut ThinfloeConfig)) -> Result<Vec<&'static str> /*changed files*/, String> }` — applies `f` to a copy, regenerates only `table.files()`, then writes `config.toml` (temp + rename); on a write error nothing in memory changes. Refused in ReadOnly.
  - `pub fn stage(&self, table: Table, candidate: &ThinfloeConfig) -> Result<Staged, String>` — writes `table.files()` rendered from `candidate`, returns `Staged { old: Vec<(&'static str, Option<Vec<u8>>)> }`; `config.toml` untouched.
  - `pub fn unstage(&self, s: Staged) -> Result<(), String>` — puts each old byte string back (or removes); a failure names the file.
  - `pub fn commit(&mut self, candidate: ThinfloeConfig) -> Result<(), String>` — writes `config.toml` only (the generated files are already the candidate's).
  - `pub fn has_generated(&self) -> bool`.

- [ ] **Step 1: Failing tests** (`TestDir` per test; `now_epoch` fixed at 1700000000)

```rust
#[test] fn a_fresh_home_gets_defaults_and_no_network_files()   // empty dir, Session -> config.toml written; generated/environment and generated/screen exist; generated/wifi.yaml and ethernet.yaml absent; notes empty
#[test] fn read_only_open_writes_nothing()                      // empty dir, ReadOnly -> dir still empty (Review Focus 4)
#[test] fn a_broken_config_is_kept_and_rebuilt_from_generated() // config.toml "garbage[", generated/wifi.yaml with one net -> config.toml.broken-1700000000 holds "garbage["; cfg.network has the net; one note naming the broken file
#[test] fn a_broken_config_never_becomes_defaults_while_generated_has_files() // as above with only generated/wifi.yaml: generated/wifi.yaml unchanged afterwards
#[test] fn start_up_regeneration_repairs_newer_generated_files() // config says idle 300, generated/screen says 900 (power loss) -> generated/screen back to 300; kanshi_changed false
#[test] fn kanshi_changed_is_reported()                         // config has a profile, generated/kanshi differs -> kanshi_changed true
#[test] fn update_regenerates_only_its_table()                  // stage Network candidate; update(Keyboard, ..) -> generated/wifi.yaml still the staged bytes (Review Focus 5); generated/environment new
#[test] fn generated_files_are_written_before_config_toml()      // make config.toml's dir entry unwritable after generated write? -> use a failing config.toml rename (config.toml is a directory): update returns Err, generated/environment already new, cfg in memory unchanged
#[test] fn unstage_puts_the_old_bytes_back()                    // stage, unstage -> both network files byte-identical to before (absent stays absent)
```

- [ ] **Step 2: Run to fail** — `MENUTEST store::` → FAIL.
- [ ] **Step 3: Implement `store.rs`.**
- [ ] **Step 4: Run** — `MENUTEST store::` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: one writer for config.toml and its generated files`.

### Task A5: the migration, `thinfloe-menu --migrate-home`

**Files:**
- Create: `src/thinfloe-menu/src/migrate.rs`, fixture homes under `src/thinfloe-menu/src/testdata/homes/` (built in the tests from strings is fine too; no binary fixtures)
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/main.rs:273-286` (dispatch `--migrate-home` before `build_menu`)

**Interfaces:**
- Consumes: A1-A4.
- Produces:
  - `pub const LEGACY_MENU_HEADER: &str` = today's `owned::HEADER` text.
  - `pub enum HomeLayout { Legacy, New }`; `pub fn home_layout(state_dir: &Path) -> HomeLayout` — exactly `legacy` (one trailing newline allowed) is `Legacy`, anything else, a missing or empty file included, is `New` (Spec issue 1).
  - `pub fn set_new(state_dir: &Path) -> io::Result<()>` — rewrites `home-layout` in place (open write+truncate, `new\n`), because the state dir is root's (Spec issue 1).
  - `pub const OLD_FILES: &[&str]` — every old path relative to the home: `.thinfloe-environment`, `.thinfloe-sessionrc`, `.thinfloe-home-version`, `.config/thinfloe/screen`, `.config/thinfloe/screen.by-hand`, `.config/thinfloe/status`, `.config/kanshi/config`, `.config/kanshi/config.prev`, `.config/kanshi/config.by-hand`, `.config/kanshi/thinfloe-layout-unconfirmed`, `wifi.yaml`, `wifi.yaml.by-hand`, `ethernet.yaml`, `ethernet.yaml.by-hand`, `.thinfloe-password`, `.thinfloe-ssh-host-keys`, `wg0.conf`, `.save-home-exclude`.
  - `pub struct Report { pub converted: Vec<String>, pub copied: Vec<String>, pub dropped: Vec<String>, pub not_moved: Vec<String> }` with `fn readme(&self) -> String` and `fn summary(&self) -> String` (two or three sentences for `[notice] migration`).
  - `pub fn migrate_home(home: &Path, state_dir: &Path, inputs: Inputs, now_epoch: i64) -> Result<Option<Report>, String>` — `Ok(None)` when `home_layout` is `New`. Steps as spec "The migration" 1-4:
    1. Read each old file that exists through `atomic::read` (cap 64 KiB as today's `owned::MAX_BYTES`); a read error is a step-1 failure. Start from the existing `config.toml` when it parses (Review Focus 2), else defaults; convert with A3. Kanshi: with `thinfloe-layout-unconfirmed` saying `prev` and a `config.prev`, convert `config.prev`; saying `none`, no profiles (Review Focus 1). Environment: `keyboard_from_env`; the other active lines go into the new `environment` after its template comment block. Sessionrc: `idle_from_sessionrc` (wins over `.config/thinfloe/screen`, as today); the rest is appended to the new `sessionrc` under `# From ~/.thinfloe-sessionrc:` only when it has an active line other than the idle assignment (Spec issue 6).
    2. Write `config.toml` and every generated file through `ConfigStore` (Session semantics, all tables), the new `environment` and `sessionrc`, and copy `.thinfloe-password` → `password`, `.thinfloe-ssh-host-keys/` → `ssh-host-keys/`, `wg0.conf` → `wg0.conf`, `.save-home-exclude` → `save-home-exclude`, modes kept (`std::fs::copy` keeps the mode; directories recreated with the source mode). Any failure here: remove nothing of the old files, return Err.
    3. Move each old file into `~/cfg-attic/<same relative path>` (`rename`, parent dirs created); an existing attic entry is never replaced: the second copy goes to `<path>.<epoch>`. A failed move is noted in `not_moved` and the rest go on.
    4. Write `home-version` (`2\n`, through `atomic::write` mode 0644), `~/cfg-attic/README` (`report.readme()`), set `[notice] migration` through `update`, then `set_new(state_dir)`.
  - `main.rs`: `--migrate-home` → `migrate_home(HOME, TF_STATE_DIR or /run/thinfloe/state, inputs, now)`; prints the summary or the error on stderr; exit 0 on `Ok`, 1 on `Err`.

- [ ] **Step 1: Failing tests** (each builds a home in a `TestDir`, with `state/home-layout` = `legacy\n`)

```rust
#[test] fn a_4x_home_with_hand_files_migrates()
    // .thinfloe-environment as in A3's first test; .thinfloe-sessionrc "TF_IDLE_TIMEOUT=900\nwlr-randr --output DP-1 --transform 90\n";
    // wifi.yaml hand-made ssid home/secret123; wg0.conf (0600); .thinfloe-password (0600); .thinfloe-ssh-host-keys/ssh_host_ed25519_key (0600, dir 0700); .save-home-exclude
    // -> config keyboard {ch,de_nodeadkeys,true,Menu}; screen 900; network.networks == [home]
    // -> .config/thinfloe/environment contains "XCURSOR_SIZE=32" and no "XKB_DEFAULT_"; sessionrc contains the wlr-randr line and no "TF_IDLE_TIMEOUT"
    // -> generated/wifi.yaml mode 0600 contains "home"; wg0.conf, password copied with mode 0600; ssh-host-keys/ dir 0700
    // -> cfg-attic/.thinfloe-environment == original bytes; cfg-attic/wifi.yaml, cfg-attic/.thinfloe-ssh-host-keys/ssh_host_ed25519_key exist; no old file left in place
    // -> home-version == "2\n"; state/home-layout == "new\n"; cfg-attic/README names "grp:alt_shift_toggle"
#[test] fn a_home_with_menu_files_migrates()
    // LEGACY_MENU_HEADER screen file "idle_timeout=600", status file "clock 1x1\n-battery 2x1\n", menu kanshi config, menu wifi.yaml + static ethernet.yaml
    // -> screen 600, status lines Some([...]), monitors one profile, wired static; kept_* all None
#[test] fn broken_files_are_converted_as_far_as_possible()
    // EAP wifi.yaml -> kept_wifi Some; hand kanshi -> monitors.kept Some; status file of 5000 bytes -> lines None and a README note; migration Ok
#[test] fn an_unreadable_old_file_stops_the_migration()
    // .thinfloe-environment is a FIFO -> Err; every old file still in place; no home-version; home-layout still "legacy\n"; no cfg-attic
#[test] fn an_unconfirmed_layout_converts_the_previous_one()   // marker "prev" + config.prev (profile A) + config (profile B) -> monitors == A; marker "none" -> no profiles
#[test] fn a_second_run_keeps_what_the_first_converted()
    // first run with home-version write made to fail (home-version is a directory): Err or Ok-without-marker, attic filled;
    // remove the obstacle, run again -> network still [home] (taken from config.toml), attic entries untouched, second copies as <name>.1700000000
#[test] fn an_already_migrated_home_is_left_alone()            // home-layout "new" -> Ok(None), no file touched
#[test] fn home_layout_reads_only_legacy_as_legacy()           // "legacy\n" Legacy; "legacy" Legacy; "" New; missing New; "new\n" New; "garbage" New
#[test] fn a_3x_renamed_home_without_templates_migrates()      // no .config/thinfloe/environment or sessionrc before (the 3.x rename replaced the dir) -> both created from the templates
```

The template text for the new `environment` and `sessionrc` comes from `include_str!("../../../overlay/home/tfuser/.config/thinfloe/environment")` and `…/sessionrc` (created in Task A11; until then create them here with the comment text of Task A11 Step 3 so the test compiles; Task A11 owns their wording).

- [ ] **Step 2: Run to fail** — `MENUTEST migrate::` → FAIL.
- [ ] **Step 3: Implement `migrate.rs` and the `main.rs` dispatch.**
- [ ] **Step 4: Run** — `MENUTEST migrate::` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: migrate a 4.x home into ~/.config/thinfloe once`.

### Task A6: the menu reads and writes through the store (paths, Screen, Status)

**Files:**
- Modify: `src/thinfloe-menu/src/main.rs:31-77,117-152,268-314` (drop `screen_path`, `status_path`, `idle_rc`, `TF_SCREEN_FILE`, `TF_STATUS_FILE`; open the store), `src/thinfloe-menu/src/jobs.rs:192-207` (`Paths`), `jobs.rs:808-816` (`status_save`), `src/thinfloe-menu/src/app.rs:73-110` (`Config`, `ScreenFile`), `app.rs:399` and `:786-795` (status watch and reload), `app.rs:594`, `app.rs:1446-1478` (`apply_screen`), `src/thinfloe-menu/src/screentab.rs` (drop `IdleSource`, `load`, `rc_idle`, `SCREEN_FILE`), `src/thinfloe-menu/src/status/mod.rs:239-252` (`FILE`, `read_layout`), `src/thinfloe-menu/src/menu.rs:202-216,717-745,975-1000` (`Effect::WriteScreen`/`TakeOverScreen`, `SettingsEvent::ManageScreen`), `src/thinfloe-menu/src/settings.rs:13-36`, `src/thinfloe-menu/src/ui.rs:1061-1110` (`idle_band` notes), `src/thinfloe-menu/tests/check.rs`

**Interfaces:**
- Consumes: `store::{open, Shared, ConfigStore}`, `config::ConfigDir::from_env`, `migrate::home_layout`.
- Produces:
  - `Paths` gains `pub config: store::Shared`, `pub layout: migrate::HomeLayout`; loses `status_file`.
  - `screentab::ScreenTab { pub idle: u32, pub error: Option<String> }`; `screentab::from_config(c: &ScreenCfg) -> ScreenTab`.
  - `status::layout_from(cfg: &StatusCfg) -> layout::Layout` (the `[status]` lines through `layout::parse` + `complete`; notes keep the "status line N" wording, N = 1-based index).
  - `jobs::status_save(store: &Shared, text: Option<&str>) -> Result<(), String>` — `update(Table::Status, …)`; text split into lines.
  - `app::Config.status_file` becomes `config_toml: PathBuf` (watched as before by `status.tick`; a change re-reads `[status]` from the file through `config::parse`, a parse error keeps the shown layout).
  - `apply_screen(idle, &ScreenFile, &mut Menu)` — `update(Table::Screen, |c| c.screen.idle_timeout = idle)`, then SIGUSR1 to `idle_pid` as today. `ScreenFile` keeps `idle_pid` only.
  - `--check`: `status:` first line becomes `layout from <dir>/config.toml` or `layout from <dir>/config.toml (missing: the shipped layout)`; opened with `OpenMode::ReadOnly`.

- [ ] **Step 1: Move and rewrite the tests**
  - `screentab.rs`: drop `load_*`, `rc_idle_*` tests; keep `parse_idle_*`, `options_*`, `labels`, `idle_pid_*`.
  - `menu.rs:2413-2505` idle tests: `IdleTimeout(120)` with tab idle 300 → `Some(Effect::WriteScreen { idle: 120 })`; same value → `None`; `idle_change_refused_when_rc_or_by_hand` and the `ManageScreen` case removed.
  - `app.rs:2166-2290` `write_screen_*`: `write_screen_writes_signals_and_saves` asserts `config.toml` parses with `screen.idle_timeout == 120` and `generated/screen` ends `idle_timeout=120\n`; `write_screen_refused_for_a_hand_file` and the take-over tests removed.
  - `jobs.rs:1867` `status_save_writes_through_a_new_file_and_none_removes_it` → `status_save_updates_the_status_table`: `Some("clock 1x1\n")` → `[status] lines == ["clock 1x1"]`; `None` → `lines == None`.
  - `tests/check.rs`: set `TF_CONFIG_DIR` to a dir whose `config.toml` has `[status] lines = ["clock 1x1", "ups 1x1", "clock 1x1"]`; expected lines `status: layout from <dir>/config.toml`, `status: status line 2: unknown element 'ups' kept but not shown`, `status: status line 3 skipped: 'clock' is already listed`; and the dir has no new file after the run.
- [ ] **Step 2: Run to fail** — `MENUTEST` → the changed tests fail to compile/fail.
- [ ] **Step 3: Implement** the path setup in `main.rs` (`build_menu` opens the store once: Session mode for the run, ReadOnly for `--check`; `inputs.default_wired` read from `netfiles::DEFAULT_WIRED` or `TF_NETWORK_DEFAULT_WIRED`, missing file → empty string and the network render of a static address fails with its message; `netboot` from the same rule `app.rs` uses today), and the rest of the interface changes. When `Opened.kanshi_changed`, the app calls `jobs::kanshi_reload` once after start.
- [ ] **Step 4: Run** — `MENUTEST` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: the Screen and Status tabs keep their settings in config.toml`.

### Task A7: monitors through `generated/kanshi`

**Files:**
- Modify: `src/thinfloe-menu/src/jobs.rs:731-1017` (`Kanshi`, `mon_load`, `mon_apply`, `mon_undo`, `mon_keep`, `mon_revert`, `mon_take_over`) and tests `jobs.rs:1835-2276`; `src/thinfloe-menu/src/monitors.rs:11,91,169-312,633-640` (`Owner` → `MonSource`); `src/thinfloe-menu/src/menu.rs:161-176,380`; `src/thinfloe-menu/src/app.rs:486-493,583-593`; `src/thinfloe-menu/src/monitors_ui.rs` (texts that name `~/.config/kanshi/config`)

**Interfaces:**
- Consumes: `store::{Shared, stage, unstage, commit}`, `generated::render`, `config::MonitorsCfg`.
- Produces:
  - `pub enum MonSource { Missing, Menu, Kept }` in `monitors.rs` (replaces `owned::Owner` there: `Kept` = `[monitors] kept`, read-only).
  - `Kanshi { config: PathBuf /* generated/kanshi */, marker: PathBuf /* generated/layout-unconfirmed */, cmd, proc_dir }`; `Kanshi::from_dir(d: &ConfigDir) -> Kanshi`; `prev()` = `generated/kanshi.prev`.
  - `jobs::mon_load(store: &Shared) -> (Result<Option<Vec<kanshi::Profile>>, String>, MonSource)`; `JobResult::MonLoaded` carries those.
  - `jobs::mon_apply(store: &Shared, k: &Kanshi, profiles: &[Profile], prev_body: Option<&str>) -> Result<(), String>` — same undo protocol as today (`jobs.rs:903-957`): prev kept, marker `prev\n` written, then `generated/kanshi` written through `store.stage(Table::Monitors, &candidate)`, `kanshi_reload`; on failure `mon_undo`. `[monitors]` unchanged.
  - `jobs::mon_keep(store: &Shared, k: &Kanshi, profiles: Vec<Profile>) -> Result<(), String>` — marker removed first, then `store.commit` of the candidate with `[monitors] = from_profiles(profiles)`, then `kanshi.prev` removed.
  - `jobs::mon_revert(k)` unchanged in protocol on the new paths.
  - `jobs::mon_take_over(store: &Shared, heads: &[Head]) -> Result<(), String>` — a kept file cannot be parsed, so the take-over replaces it with the layout on screen: `update(Table::Monitors, |c| { c.monitors.profiles = vec![MonitorsCfg-form of kanshi::from_heads(heads)]; c.monitors.kept = None })`. `Job::MonTakeOver { heads: Vec<Head> }` carries the heads.
  - `Job::MonApply { profiles: Vec<Profile>, menu_output, prev }` (was `text`).

- [ ] **Step 1: Rewrite the tests on the new paths**: `mon_apply_writes_prev_marker_and_config` (asserts `generated/kanshi.prev`, `generated/layout-unconfirmed` == `prev\n`, `generated/kanshi` starts with `generated::HEADER`, `config.toml` `[monitors]` still the old one), `mon_keep_removes_marker_and_prev` (plus: `[monitors]` now the applied profiles), `mon_revert_restores_prev`, `a_first_apply_then_revert_leaves_the_layout_on_screen`, `mon_apply_refuses_while_a_layout_is_unconfirmed`, `a_failed_config_write_is_rolled_back`, `mon_load_and_take_over` (kept text → `MonSource::Kept`; take-over → `Menu`, `kept == None`). `monitors.rs` tests at `:940`, `:1038-1073` use `MonSource`.
- [ ] **Step 2: Run to fail** — `MENUTEST mon_ monitors::` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — `MENUTEST` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: the monitor arrangement lives in config.toml and generated/kanshi`.

### Task A8: network through `generated/wifi.yaml` and `generated/ethernet.yaml`

**Files:**
- Modify: `src/thinfloe-menu/src/netfiles.rs:1-65,845-1140` (`Homes`, `Current`, `load`, `save`, `take_over`, `state`), `src/thinfloe-menu/src/jobs.rs:660-729` (`ApplyFailed`, `network_apply`, `put_back`) and tests `jobs.rs:1512-1815`, `src/thinfloe-menu/src/network.rs:201-207,372-383,463-475` (texts and `FileState`), `src/thinfloe-menu/src/network_ui.rs` (texts naming `~/wifi.yaml`, `~/ethernet.yaml`), `src/thinfloe-menu/src/app.rs:482,508,520,569-582`

**Interfaces:**
- Consumes: `store::{Shared, stage, unstage, commit, update}`, `config::NetworkCfg`, `convert::network_from_files` (only through the store's rebuild).
- Produces:
  - `FileState { Missing, Menu, Hand }` where `Hand` now means "kept as written" (`kept_wifi`/`kept_wired`); `Template` is used only inside `parse_wifi` for the converter.
  - `netfiles::view(cfg: &NetworkCfg) -> (WifiFile, WiredFile)` — what `NetTab::loaded` gets (`JobResult::NetLoaded` unchanged in shape).
  - `netfiles::plan(cfg: &NetworkCfg, c: &Change, netboot: bool, h: &Homes, inputs: &Inputs) -> Result<NetworkCfg, String>` — today's `save` rules (`netfiles.rs:1015-1088`: kept refuses, netboot refusals, one machine DNS copied into WiFi, input checks, adapter check, `check_set` with `netplan generate` on the rendered generated bodies) returning the candidate table instead of writing.
  - `netfiles::take_over(cfg: &NetworkCfg, which: &str, h: &Homes, inputs: &Inputs) -> Result<NetworkCfg, String>` — parses the kept text; refuses when not manageable (as today); the candidate has the parsed networks/wired and `kept_* = None`; written with `update(Table::Network, …)` (no apply, as today).
  - `Homes { home, sys_net, netplan, default_wired }` unchanged in fields; `wifi()`/`wired()` removed.
  - `jobs::network_apply(paths, change, netboot, feed, on_spawn) -> Result<(), ApplyFailed>` — `plan` → `store.stage(Table::Network, &candidate)` → `thinfloe-network apply` → on success `store.commit(candidate)`; on failure `store.unstage`, and an unstage error sets `home_changed = true` with the file named, as `put_back` does today (`jobs.rs:711-729`).

- [ ] **Step 1: Rewrite the tests**: `network_apply_skips_apply_after_failed_save`, `network_apply_runs_the_tool_after_a_save_and_reads_its_result` (generated files written, then `[network]` committed), `a_failed_network_apply_puts_the_home_back` → `…puts_the_generated_files_back` (byte-identical, `[network]` unchanged), `a_failed_dns_change_puts_the_wifi_file_back`, `a_home_that_cannot_be_put_back_says_so` (make `generated/` read-only after the stage: reason names `generated/wifi.yaml`, `home_changed`), `putting_back_never_writes_over_a_link_or_a_hand_file` → `unstage_never_follows_a_link`. New: `a_keyboard_change_during_an_apply_leaves_the_staged_files` (stage, `update(Keyboard)`, tool fails, unstage → original bytes; Review Focus 5). `netfiles` tests for `save`/`take_over` move to `plan`/`take_over` with the same values (DNS `9.9.9.9`, netboot refusals "A fixed address cannot be set on netboot.", "DNS servers cannot be set on netboot.", "No WiFi adapter found").
- [ ] **Step 2: Run to fail** — `MENUTEST net` → FAIL.
- [ ] **Step 3: Implement.** User-visible texts that named `~/wifi.yaml`/`~/ethernet.yaml` "set by hand" now read "kept from your old settings; the menu cannot change it" with the existing "Let the menu manage it" button.
- [ ] **Step 4: Run** — `MENUTEST` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: network settings live in config.toml and generated/`.

### Task A9: Settings notices, the legacy guard, and `owned.rs` removed

**Files:**
- Modify: `src/thinfloe-menu/src/menu.rs` (state, `settings()`), `src/thinfloe-menu/src/settings.rs:13-36`, `src/thinfloe-menu/src/ui.rs:906-955` (`show_settings`), `src/thinfloe-menu/src/main.rs`, `src/thinfloe-menu/src/app.rs`
- Delete: `src/thinfloe-menu/src/owned.rs`; `lib.rs` loses `pub mod owned;`

**Interfaces:**
- Consumes: `Opened.notes`, `config::{dropped_env_lines, read_managed_keys}`, `ThinfloeConfig.notice`, `Paths.layout`.
- Produces:
  - `pub struct ConfigNotes { pub broken: Vec<String>, pub dropped_env: Vec<String>, pub report: Option<String>, pub legacy: Option<String> }` on `Menu` (`pub config_notes`).
  - `SettingsEvent::DismissReport` → `Effect::Start(Job::DismissReport)` → `update(Table::Notice, |c| c.notice.migration = None)`.
  - When `Paths.layout == Legacy` (the migration failed this boot), `config_notes.legacy = Some("Your settings could not be moved to ~/.config/thinfloe; they stay as they were until the next start. See the session log.")` and the Network, Screen, Status (and in Part B Keyboard) tabs are disabled (`ui.disable()` on their body); events from them are ignored in `Menu::settings` (Spec issue 3).
  - `show_settings` draws the notes as one muted band under the title, before the tabs: broken notes, then "These lines in ~/.config/thinfloe/environment are ignored because the menu sets them:" plus each line, then the report with a "Dismiss" button, then the legacy note.

- [ ] **Step 1: Failing tests**
  - `menu.rs`: `a_dismissed_report_is_cleared` (`DismissReport` with a report → `Some(Effect::Start(Job::DismissReport))`; without → `None`); `the_legacy_layout_ignores_setting_changes` (`IdleTimeout(60)` → `None`, `Status(..)` → `None`, `Tab(Packages)` still works).
  - `ui.rs`: extend the settings snapshot cases (`ui.rs:3219-3250`) with one case that has all four notes and assert the texts "is ignored because the menu sets them", "Dismiss" are laid out inside the panel.
  - `app.rs`: `dismiss_report_clears_the_notice` (job run → `config.toml` has no `[notice] migration`).
- [ ] **Step 2: Run to fail** — `MENUTEST` → FAIL.
- [ ] **Step 3: Implement; delete `owned.rs`** (every remaining `crate::owned::` reference moves to `atomic::` or `migrate::LEGACY_MENU_HEADER`; the `owned.rs` tests that still matter were moved in A2).
- [ ] **Step 4: Run** — `MENUTEST` → PASS; `MENULINT`; `make test-unit` → PASS (Python suites untouched so far).
- [ ] **Step 5: Commit** — `feat: the Settings screen reports the config move and ignored lines`.

### Task A10: initramfs: home layout, 3.x marker, network paths

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh:403-466` (`tf_restore_home_from_file`), `:527-536` (constants), `:626-676` (`tf_migrate_home_names`), `:710-772` (`restore_home`), `:774-860` (`install_local_network`); `setup-overlay.sh:77-80`; `module-setup.sh:60` (add `chown`)
- Test: `test/integration/test-initramfs-home.sh` (sections at `:117`, `:262`, `:410`, `:483`, `:757`), `test/integration/test-initramfs-commands.sh` (no change expected; it checks `chown` against `lsinitramfs` at the gate)

**Interfaces:**
- Produces:
  - `TF_HOME_LAYOUT_MARKER=".config/thinfloe/home-version"`.
  - `tf_restore_home_from_file` keeps its listing in the global `_TF_HOME_LIST` (reset to empty by `restore_home` before the restore).
  - `tf_record_home_layout <home> <state-dir>` — with `_TF_HOME_LIST` non-empty and not listing `TF_HOME_LAYOUT_MARKER` (via `_tf_archive_lists`): `rm -f "<home>/$TF_HOME_LAYOUT_MARKER"`, write `legacy`; otherwise `new` (no archive, a marked archive, or no listing at all; Spec issue 4). Then `chown 1001:1001 <state>/home-layout` (warned on failure), and `tf_log "thinfloe: home layout <legacy|new>"`. Always returns 0. Called by `restore_home` after its `case "$_rh_rc"` block, on every path, including the early "no boot MAC" return (with an empty listing: `new`).
  - `tf_migrate_home_names`: returns 0 without touching anything when the listing has `TF_HOME_VERSION_FILE` **or** `TF_HOME_LAYOUT_MARKER`.
  - `install_local_network <newroot> [<ramroot-url>] [<state-dir>]` — `new` (anything but `legacy` in `<state-dir>/home-layout`): sources `~/.config/thinfloe/generated/wifi.yaml`, `~/.config/thinfloe/generated/ethernet.yaml`, `~/.config/thinfloe/wg0.conf`, no template check; `legacy`: today's paths and template check. Log lines name the source relative to the home: `thinfloe: installing ~/.config/thinfloe/generated/wifi.yaml into /etc/netplan`, `thinfloe: installing ~/wifi.yaml into /etc/netplan` (and the same for `ethernet.yaml`, `wg0.conf` into `/etc/wireguard`). `setup-overlay.sh` passes `"$TF_STATE"` as third argument.

- [ ] **Step 1: Failing tests in `test-initramfs-home.sh`** (stub `chown() { echo "$@" >>"$WORK/chown"; }` like the other stubs)

```
ok "home-layout is new without an archive"                 # restore_home with no home.tar.gz -> state/home-layout == new
ok "home-layout is new with a marked archive"              # archive lists ./.config/thinfloe/home-version -> new; marker still on disk
ok "home-layout is legacy with an unmarked archive, and the shipped marker is gone" # archive with ./.thinfloe-home-version only -> legacy; ~/.config/thinfloe/home-version absent
ok "home-layout is new when the restore failed without a listing" # netboot, no boot MAC -> new
ok "home-layout is given to tfuser"                        # $WORK/chown has "1001:1001 <state>/home-layout"
ok "a home with only .config/thinfloe/home-version is not renamed by the 3.x migration" # listing ./.config/thinfloe/home-version + ./.dbrrg-environment -> .dbrrg-environment stays, no .thinfloe-home-version written
ok "new layout installs generated/wifi.yaml"               # state new, generated/wifi.yaml + ~/wifi.yaml present -> /etc/netplan/wifi.yaml == generated copy, mode 600
ok "legacy layout installs ~/wifi.yaml"                    # state legacy -> ~/wifi.yaml copy
ok "new layout installs ~/.config/thinfloe/wg0.conf"        # and the wg-quick@wg0 link
ok "legacy layout installs ~/wg0.conf"
ok "the template check applies to the legacy layout only"     # state new, generated/wifi.yaml containing 'device name of your wifi card' -> installed; state legacy, same text in ~/wifi.yaml -> skipped
```

Existing `install_local_network` cases (`:410-540`) pass the state dir as third argument with `legacy`, so their assertions stay valid; netboot cases keep "netboot skips ethernet.yaml but installs wifi.yaml" for both layouts.

- [ ] **Step 2: Run to fail** — `test/integration/test-initramfs-home.sh` → FAIL lines.
- [ ] **Step 3: Implement** in `thinfloe-lib.sh`, `setup-overlay.sh`, `module-setup.sh` (`inst_multiple stat dd od tr mkdir mount umount cp ln chmod chown`). No `exit`; `tf_log` only to stderr; only installed commands.
- [ ] **Step 4: Run** — `test/integration/test-initramfs-home.sh` → all `ok`, exit 0; `test/integration/test-esp-grow.sh` → exit 0 (the no-`exit` guard).
- [ ] **Step 5: Commit** — `feat: the initramfs tells old and new home layouts apart`.

### Task A11: shipped home, managed keys, composer

**Files:**
- Delete: `overlay/home/tfuser/.thinfloe-environment`, `.thinfloe-sessionrc`, `.thinfloe-home-version`, `wifi.yaml`
- Create: `overlay/home/tfuser/.config/thinfloe/home-version` (`2`), `overlay/home/tfuser/.config/thinfloe/environment`, `overlay/home/tfuser/.config/thinfloe/sessionrc`, `overlay/etc/thinfloe/menu-managed-keys`
- Modify: `overlay/usr/bin/thinfloe-compose-labwc-config` (whole file), `overlay/etc/thinfloe/labwc/environment:21-34` (comment naming `~/.thinfloe-environment`)
- Test: `test/integration/test-labwc-config-merge.sh`, `test/integration/test-labwc-scale.sh:50-60`, `test/runtime/test-labwc-runtime.sh:575` (3-argument call → 4 arguments)

**Interfaces:**
- Produces:
  - `thinfloe-compose-labwc-config [--env-only] <system-dir> <generated-env> <user-env> <output-dir>` — missing env files are not errors. Full mode: as today (wipe, symlink every system file but `environment`), `environment` = system file, then `<generated-env>`, then `<user-env>` without lines whose key is in `${TF_MANAGED_KEYS:-/etc/thinfloe/menu-managed-keys}` (key = text before the first `=`, after leading blanks and an optional `export `; comment lines kept). `--env-only`: `<output-dir>` must exist (else exit 1 with `thinfloe-compose-labwc-config: no composed dir '<dir>'`); writes `<output-dir>/.environment.XXXXXX` with `mktemp` and renames it over `environment`; nothing else in the dir changes. Exit 0 on success, 1 on any failure.
  - `menu-managed-keys`: a comment saying the menu's Keyboard tab sets these, then the four keys, one per line.
  - `environment` template: comment-only; says it is sourced before labwc, variables only, saved with the home; names cursor, `TZ=`, `LABWC_FULLSCREEN_SPAN_OUTPUTS`; says the keyboard is set in Settings → Keyboard and `XKB_DEFAULT_*` lines here are ignored.
  - `sessionrc` template: comment-only; commands run at session start (wlr-randr, kanshi by hand); says the monitor arrangement, screen timeout and network are set in Settings, and `TF_IDLE_TIMEOUT` is no longer read here.

- [ ] **Step 1: Failing tests in `test-labwc-config-merge.sh`** (fixtures: system env with `XKB_DEFAULT_LAYOUT=us`, `LABWC_FULLSCREEN_SPAN_OUTPUTS=1`; generated env `XKB_DEFAULT_LAYOUT=de`, `XKB_DEFAULT_VARIANT=`, `XKB_DEFAULT_OPTIONS=ctrl:nocaps`; user env `XKB_DEFAULT_LAYOUT=fr`, `  export XKB_DEFAULT_VARIANT=oss`, `LABWC_FULLSCREEN_SPAN_OUTPUTS=0`, `XCURSOR_THEME=redglass`; `TF_MANAGED_KEYS` pointing at a fixture list)

```
ok "the order is system, generated, user"            # line numbers: us < de < LABWC_FULLSCREEN_SPAN_OUTPUTS=0
ok "managed keys are dropped from the user file"     # no "XKB_DEFAULT_LAYOUT=fr", no "XKB_DEFAULT_VARIANT=oss" in the merged file
ok "the last XKB_DEFAULT_LAYOUT is the generated one" # last match == XKB_DEFAULT_LAYOUT=de
ok "user-only and unmanaged keys survive"            # XCURSOR_THEME=redglass, LABWC_FULLSCREEN_SPAN_OUTPUTS=0 last
ok "--env-only rewrites only environment"           # replace rc.xml symlink with a regular file "scaled"; run --env-only with generated env changed to layout ch; rc.xml still the regular file "scaled", themerc-override link unchanged, environment has XKB_DEFAULT_LAYOUT=ch last
ok "--env-only without a composed dir fails"         # exit 1, message names the dir, nothing created
```

The existing idempotency and symlink assertions stay, with the 4-argument call.

- [ ] **Step 2: Add to `test-labwc-scale.sh`**: after the "scale 2" block, run the composer `--env-only` with a changed generated env and assert `ok "scale 2 survives a keyboard apply"` (`rc.xml` is a regular file with font size 20, `themerc-override` scaled values unchanged).
- [ ] **Step 3: Run to fail** — `test/integration/test-labwc-config-merge.sh`, `test/integration/test-labwc-scale.sh` → FAIL.
- [ ] **Step 4: Implement** the composer, the new home files, delete the old templates, fix the system environment comment.
- [ ] **Step 5: Run** — both scripts exit 0; `MENUTEST migrate::` (the templates are `include_str!`ed) → PASS.
- [ ] **Step 6: Commit** — `feat: shipped home in ~/.config/thinfloe; composer drops menu-managed keys`.

### Task A12: session profile and `thinfloe-session`

**Files:**
- Modify: `overlay/etc/profile.d/10-thinfloe-session.sh:44-122`, `overlay/usr/bin/thinfloe-session:12-75` (marker block, sessionrc), `:103-182` (idle), `:294-315` (kanshi start)
- Test: `test/integration/test-session-lifecycle.sh`, `test/integration/test-session-packages.sh:736-742` (default home marker), `test/integration/test-field-report.sh`

**Interfaces:**
- Consumes: `thinfloe-menu --migrate-home` (A5), the composer (A11), `home-layout` (A10).
- Produces:
  - Profile, in order: (1) `TF_STATE_DIR=${TF_STATE_DIR:-/run/thinfloe/state}`; if `home-layout` says `legacy` and `thinfloe-menu` is executable, run `thinfloe-menu --migrate-home >>"$TF_SESSION_LOG" 2>&1`; read the layout again. (2) `new`: generated env `$HOME/.config/thinfloe/generated/environment`, user env `$HOME/.config/thinfloe/environment`; `legacy`: generated env slot `$HOME/.thinfloe-environment` (so its XKB lines are kept, not filtered), user env `/nonexistent` (Spec issue 3). (3) compose as today (`:107-120`) with the 4-argument call. (4) source `"$LABWC_CONFIG_DIR/environment"` with `set -a` (replaces the two separate sources at `:58-62` and `:84-88`). (5) `export TF_LABWC_CONFIG_DIR="$LABWC_CONFIG_DIR"`. The comments that explain the order are rewritten to the new paths; the rule text "restore stays in the initramfs" stays.
  - `thinfloe-session`: one block at the top reads the layout (same file, same rule) and sets `tf_cfg=$HOME/.config/thinfloe`, `kanshi_cfg`, `kanshi_prev`, `layout_marker`, `tf_sessionrc`, `tf_screen`: new = `$tf_cfg/generated/kanshi`, `$tf_cfg/generated/kanshi.prev`, `$tf_cfg/generated/layout-unconfirmed`, `$tf_cfg/sessionrc`, `$tf_cfg/generated/screen`; legacy = today's paths. The undo block (`:31-48`) and the kanshi start (`:311-315`) use the variables. The `TF_IDLE_TIMEOUT` RC block (`:142-155`) runs only for `legacy`; for `new` it does `unset TF_IDLE_TIMEOUT_RC`. `tf_idle_timeout` reads `$tf_screen`.

- [ ] **Step 1: Failing tests in `test-session-lifecycle.sh`** (it runs the scripts with stubbed programs; follow its existing harness)

```
ok "the profile runs the migration on a legacy home"        # state legacy, stub thinfloe-menu records "--migrate-home" and writes "new"
ok "the profile does not run the migration on a new home"
ok "the profile sources the merged environment"             # stub composer writes FOO=merged into <out>/environment -> the stub labwc sees FOO=merged
ok "the profile exports TF_LABWC_CONFIG_DIR"                # composed dir; and /etc/thinfloe/labwc after a failing stub composer
ok "a legacy home composes with ~/.thinfloe-environment"    # stub composer args: <sys> $HOME/.thinfloe-environment /nonexistent <out>
ok "thinfloe-session undoes an unconfirmed layout in generated/"  # new layout: marker prev + kanshi.prev -> kanshi == old, marker gone
ok "thinfloe-session starts kanshi on generated/kanshi"
ok "thinfloe-session ignores TF_IDLE_TIMEOUT in the new sessionrc" # sessionrc sets TF_IDLE_TIMEOUT=5, generated/screen idle_timeout=600 -> swayidle stub gets 600, TF_IDLE_TIMEOUT_RC unset
ok "thinfloe-session keeps the sessionrc timeout on a legacy home" # legacy: today's behaviour
```

The existing cases that write `~/.config/kanshi/…`, `~/.thinfloe-sessionrc` or `~/.config/thinfloe/screen` set the state to `legacy` or move to the new paths with `new`; each keeps its assertion.

- [ ] **Step 2: `test-session-packages.sh:736-742`**: the default home carries `home/tfuser/.config/thinfloe/home-version` holding `2`, `environment` and `sessionrc`, and none of `.thinfloe-environment`, `.thinfloe-sessionrc`, `.thinfloe-home-version`, `wifi.yaml`; `/etc/thinfloe/menu-managed-keys` lists the four keys. Other old-path assertions in that file and in `test-field-report.sh` follow the new paths. (These run at the gate; they need the image.)
- [ ] **Step 3: Run to fail** — `test/integration/test-session-lifecycle.sh` → FAIL.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run** — `test/integration/test-session-lifecycle.sh` exit 0; `sh -n` on both scripts; `shellcheck` if the repo's tests call it (they do in `test-session-packages.sh`; run that at the gate).
- [ ] **Step 6: Commit** — `feat: the session reads ~/.config/thinfloe and runs the migration once`.

### Task A13: root tools on the new paths

**Files:**
- Modify: `overlay/usr/bin/thinfloe-save-home:109-110`, `overlay/usr/bin/thinfloe-upgrade:102-112,1739-1754,1915,2274`, `overlay/usr/bin/thinfloe-password:21,42-44,104-107`, `overlay/usr/bin/thinfloe-ssh-hostkeys:14,31`, `overlay/usr/bin/thinfloe-network:13-15,509-546`
- Test: `test/integration/test-save-home.sh`, `test/integration/test-password.sh`, `test/integration/test-ssh-hostkeys.sh`, `test/unit/test_upgrade_image.py`, `test/unit/test_network.py`, `test/integration/test-network-netplan.sh`

**Interfaces:**
- Produces (each reads `${TF_STATE_DIR:-/run/thinfloe/state}/home-layout`, exactly `legacy` → old path, else new):
  - `thinfloe-save-home`: exclude file `$TF_HOME_DIR/.config/thinfloe/save-home-exclude` (legacy: `$TF_HOME_DIR/.save-home-exclude`), still falling back to `TF_EXCLUDE_DEFAULT`.
  - `thinfloe-upgrade`: `IDENTITY_EXCLUDES = ("./.config/thinfloe/ssh-host-keys", "./.config/thinfloe/wg0.conf", "./.thinfloe-ssh-host-keys", "./.dbrrg-ssh-host-keys", "./wg0.conf", "./cfg-attic/.thinfloe-ssh-host-keys", "./cfg-attic/.dbrrg-ssh-host-keys", "./cfg-attic/wg0.conf", "./cfg-attic/.thinfloe-password", "./cfg-attic/.save-home-exclude")` (the attic copies per the spec; the live `password` stays copied, as today). `load_exclude_patterns(home_dir, layout)` reads the new exclude path, or the old one for `legacy`; a module-level `home_layout(state_dir) -> str` helper. Log texts at `:1915` and `:2274` unchanged in meaning.
  - `thinfloe-password`: `store=$home_dir/.config/thinfloe/password` (legacy: `$home_dir/.thinfloe-password`); `--set` creates `.config/thinfloe` (owned by tfuser) when missing; `--clear` removes both the new and the old store and `.dbrrg-password`.
  - `thinfloe-ssh-hostkeys`: `KEYSTORE=$HOME_DIR/.config/thinfloe/ssh-host-keys` (legacy: `.thinfloe-ssh-host-keys`); the `chown -R` of the keystore stays; when it creates `.config/thinfloe` it chowns that dir to tfuser too.
  - `thinfloe-network apply`: reads `<home>/.config/thinfloe/generated/wifi.yaml` and `…/ethernet.yaml` (only called by the menu, after the migration; no fallback). Log lines `wifi.yaml: from the home` keep their wording.

- [ ] **Step 1: Failing tests**
  - `test-save-home.sh`: `ok "the exclude list is read from ~/.config/thinfloe/save-home-exclude"`; `ok "a legacy home reads ~/.save-home-exclude"` (state fixture via `TF_STATE_DIR`).
  - `test-password.sh`: `ok "the hash is stored in ~/.config/thinfloe/password"` (mode 600, owner tfuser stub); `ok "--apply reads ~/.thinfloe-password on a legacy home"`; `ok "--clear removes the old and the new store"`.
  - `test-ssh-hostkeys.sh`: `ok "keys are staged into ~/.config/thinfloe/ssh-host-keys"`; `ok "a legacy home uses ~/.thinfloe-ssh-host-keys"`.
  - `test_upgrade_image.py`: `test_identity_excludes_cover_new_and_attic_paths` (every path in the tuple above is in `home_archive_argv(...)` as `--exclude=`); `test_fresh_copy_leaves_out_the_attic_identity` (a home with `cfg-attic/.thinfloe-ssh-host-keys/k` and `cfg-attic/wg0.conf` → neither in the tar listing, `.config/thinfloe/password` is); `test_exclude_file_follows_the_layout`.
  - `test_network.py`: the apply cases write their fixtures to `<home>/.config/thinfloe/generated/`.
- [ ] **Step 2: Run to fail** — `python3 -m unittest discover -s test/unit -v`; `test/integration/test-save-home.sh`, `test-password.sh`, `test-ssh-hostkeys.sh` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — `make test-unit` → PASS; the three scripts exit 0.
- [ ] **Step 5: Commit** — `feat: password, host keys, WireGuard and excludes move to ~/.config/thinfloe`.

### Task A14: QEMU upgrade smoke test covers the migration — **Gate A (`make image`)**

**Files:**
- Create: `scripts/smoke-home-report.sh`
- Modify: `scripts/run-qemu-upgrade-smoke.sh:57-164`

**Interfaces:**
- Consumes: everything in Part A, the built `artifacts/images/thinfloe-usb.img`.
- Produces:
  - `smoke-home-report.sh migrate` (run as root by `systemd.run` on boot 1): runs `/bin/sh /run/thinfloe/storage/efi/smoke-report.sh check` (the existing add-on report), prints `SMOKE-HOME layout <content of home-layout>`, runs `runuser -u tfuser -- env HOME=/home/tfuser TF_STATE_DIR=/run/thinfloe/state thinfloe-menu --migrate-home`, prints `SMOKE-HOME migrate rc=<n>` and `SMOKE-HOME layout-after <content>`, runs `thinfloe-save-home`, prints `SMOKE-HOME save rc=<n>`, then `SMOKE-HOME-DONE`. (The smoke boots stop at `multi-user.target`, so no session runs the migration; Spec issue 5.)
  - `smoke-home-report.sh check` (boot 2): prints `SMOKE-HOME layout <content>`, `SMOKE-HOME env <the XKB_DEFAULT_LAYOUT line of ~tfuser/.config/thinfloe/generated/environment>`, `SMOKE-HOME attic <yes|no>` for `~tfuser/cfg-attic/.thinfloe-environment`, then `SMOKE-HOME-DONE`.
  - The stick's 3.x home gets `wifi.yaml` (one network `smoke`/`smokepass1`, device `wlan0`) and `.dbrrg-sessionrc` gets `TF_IDLE_TIMEOUT=900`; after the initramfs renames it is a 4.x home, so one stick covers the 3.x renames and this migration.
  - Boot 2: after boot 1 the script rewrites both `syslinux.cfg` copies so `LABEL current` (the `DEFAULT` after the rotation) carries `APPEND`, the smoke options and `systemd.run=… smoke-home-report.sh check`, and boots again with `SMOKE_WAIT_FOR='^SMOKE-HOME-DONE'` into `$LOG.2`.

- [ ] **Step 1: Write the new assertions** in `run-qemu-upgrade-smoke.sh` (existing ones unchanged):

```
ok "boot 1: home layout legacy"                               # "thinfloe: home layout legacy" in $LOG and "SMOKE-HOME layout legacy"
ok "boot 1: the old ~/wifi.yaml is installed"                  # "thinfloe: installing ~/wifi.yaml into /etc/netplan"
ok "boot 1: the migration ran and the layout is new"           # "SMOKE-HOME migrate rc=0", "SMOKE-HOME layout-after new", "SMOKE-HOME save rc=0"
ok "boot 2: home layout new"                                   # "thinfloe: home layout new" in $LOG.2
ok "boot 2: generated/wifi.yaml is installed"                  # "thinfloe: installing ~/.config/thinfloe/generated/wifi.yaml into /etc/netplan"
ok "boot 2: the keyboard came across"                          # "SMOKE-HOME env XKB_DEFAULT_LAYOUT=ch"
ok "boot 2: the old files are in the attic"                    # "SMOKE-HOME attic yes"
ok "boot 2: no second 3.x rename"                              # no "thinfloe: renamed" in $LOG.2
```

- [ ] **Step 2: Build and run the gate** (one image build for Part A):

```bash
make image BUILD_JOBS=4
make test BUILD_JOBS=4
make test-runtime BUILD_JOBS=4
make qemu-smoke BUILD_JOBS=4
make qemu-smoke-upgrade BUILD_JOBS=4
make qemu-smoke-netboot BUILD_JOBS=4
```

Expected: every script prints only `ok` lines and exits 0; `make test` includes `test-initramfs-commands.sh` (`chown` now in the initrd), `test-session-packages.sh`, `test-network-netplan.sh`, `test-field-report.sh`, `test-container-inputs.sh`. `make test-runtime` passes with the 4-argument composer call. Record the count of `ok` lines per target in the task report; compare with the last green run on `main` and account for each difference.

- [ ] **Step 3: Commit** — `test: the upgrade smoke boot migrates a 4.x home and boots it again`.

### Task A15: documentation and CHANGES for Part A

**Files:**
- Modify: `CHANGES.md` (`## [Unreleased]`), `CLAUDE.md` (lines naming old paths: "Where code lives" home save/User defaults; "Home and session" rules on `~/.thinfloe-environment`, `~/.thinfloe-sessionrc`, `~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys`, `IDENTITY_EXCLUDES`; the network constraint naming `~/wifi.yaml`, `~/wg0.conf`), `README.md`, `docs/notes/{home-persistence,session,menu,status-area,esp-and-upgrade,boot-flow,time-zone,history-of-constraints}.md`

- [ ] **Step 1: CLAUDE.md**: one new rule under "Home and session": "Everything the menu manages is in `~/.config/thinfloe/config.toml`, written only by the menu; readers that run before it use `generated/` and pick old or new paths from `/run/thinfloe/state/home-layout` (only `legacy` means old). Guards: `test-initramfs-home.sh`, `test-session-lifecycle.sh`, `migrate.rs` tests." Every other line: path swaps only. Pointer to `docs/notes/home-persistence.md` for the layout table.
- [ ] **Step 2: docs/notes**: `home-persistence.md` gets the layout table, the `home-layout` rule, the migration and the attic; the other notes swap paths and drop the `.by-hand` and "set by hand" descriptions.
- [ ] **Step 3: CHANGES.md entries** (wording may be tightened, not widened; each ≤ 3 sentences):
  - Changed: "All settings the menu manages are now kept in `~/.config/thinfloe/config.toml`, and the first start after the upgrade moves the old files there by itself, WiFi included. The old files stay unchanged in `~/cfg-attic/`, with a README listing what was taken over, and the Settings screen says so once."
  - Changed: "`~/.thinfloe-environment` and `~/.thinfloe-sessionrc` are no longer read: extra variables go in `~/.config/thinfloe/environment` and session commands in `~/.config/thinfloe/sessionrc`. `TF_IDLE_TIMEOUT` is no longer read from either; the screen timeout is set on the Screen tab."
  - Changed: "`~/.thinfloe-password`, `~/.thinfloe-ssh-host-keys`, `~/wg0.conf` and `~/.save-home-exclude` moved to `password`, `ssh-host-keys`, `wg0.conf` and `save-home-exclude` in `~/.config/thinfloe/`. A new stick written from this machine still leaves out the host keys and `wg0.conf`, also their copies in `~/cfg-attic/`."
  - Changed: "When the old keyboard settings named two layouts (`XKB_DEFAULT_LAYOUT=us,de`), only the first is kept; a keyboard model other than `pc105` and XKB options other than CapsLock as Ctrl and the Compose key are dropped. `~/cfg-attic/README` names what was dropped."
  - Changed: "`XKB_DEFAULT_*` lines in `~/.config/thinfloe/environment` are ignored, and the Settings screen names each one."
  - Changed: "A WiFi, wired or monitor file the menu could not fully read is kept as it was and shown read-only, with \"Let the menu manage it\" to replace it."
- [ ] **Step 4: Commit** — `docs: the config directory in CLAUDE.md, notes and CHANGES`.

Part A ends here: image built and green at Gate A, docs current.

---

# Part B: the Keyboard tab

### Task B1: XKB registry

**Files:**
- Create: `src/thinfloe-menu/src/kbregistry.rs`, `src/thinfloe-menu/src/testdata/evdev-small.xml`
- Modify: `src/thinfloe-menu/src/lib.rs`, `containers/ubuntu/Dockerfile` (add `xkb-data` by name to the package list near `libxkbcommon0`, `:203-204`), `test/integration/test-session-packages.sh`

**Interfaces:**
- Produces:
  - `pub const REGISTRY_PATH: &str = "/usr/share/X11/xkb/rules/evdev.xml";` (`TF_XKB_REGISTRY` overrides, read in `main.rs`)
  - `pub const MAX_BYTES: u64 = 4 * 1024 * 1024;`
  - `pub struct Variant { pub name: String, pub description: String }`, `pub struct Layout { pub name: String, pub description: String, pub variants: Vec<Variant> }`, `pub struct Registry { pub layouts: Vec<Layout> }`
  - `pub fn parse(xml: &str) -> Result<Registry, String>` — `layoutList/layout/configItem/{name,description}`, `variantList/variant/configItem/{name,description}`; entries whose name fails `config::xkb_name_ok` or is empty are skipped; layouts and variants sorted by description; a duplicate layout name keeps the first.
  - `pub fn read(path: &Path) -> Result<Registry, String>` — `bounded::read_bounded(path, MAX_BYTES, true)`, error text names the path.
  - `impl Registry { fn layout(&self, name: &str) -> Option<&Layout>; fn description(&self, name: &str) -> Option<&str> }`

- [ ] **Step 1: Failing tests**

```rust
#[test] fn the_fixture_parses_sorted()       // evdev-small.xml with us ("English (US)"), ch ("German (Switzerland)") with variants de_nodeadkeys, fr; de ("German")
                                             // -> descriptions in order ["English (US)", "German", "German (Switzerland)"]; ch variants sorted by description
#[test] fn a_bad_name_is_skipped()          // fixture entry name "x y" absent
#[test] fn broken_xml_is_an_error()         // parse("<xkbConfigRegistry><layoutList>") -> Err
#[test] fn an_oversized_file_is_refused()   // 4 MiB + 1 bytes file -> Err naming the path
#[test] fn the_host_registry_parses_when_present() // if REGISTRY_PATH exists: Ok, contains "us" and "ch", every name passes xkb_name_ok (else the test returns early)
```

- [ ] **Step 2: Run to fail** — `MENUTEST kbregistry::` → FAIL.
- [ ] **Step 3: Implement**; `test-session-packages.sh` gains `ok "the image has the XKB registry"` (`/usr/share/X11/xkb/rules/evdev.xml` exists in the image, more than 100 kB).
- [ ] **Step 4: Run** — `MENUTEST kbregistry::` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: read the XKB layout list for the keyboard settings`.

### Task B2: Keyboard tab state, validation and apply queue

**Files:**
- Create: `src/thinfloe-menu/src/keyboard.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/settings.rs:13-62` (`SettingsEvent::Keyboard`, `SettingsTab::Keyboard` between Screen and Status, label "Keyboard"), `src/thinfloe-menu/src/menu.rs` (`Job::KeyboardApply`, state field, `settings()` arm, result handler), `src/thinfloe-menu/src/jobs.rs:102-163` (`JobResult::KeyboardApplied`)

**Interfaces:**
- Consumes: `config::{Keyboard, Compose, xkb_name_ok}`, `kbregistry::Registry`.
- Produces:
  - `pub enum KbEvent { Layout(String), Variant(String), CapsAsCtrl(bool), Compose(Compose), TryText(String) }`
  - `pub enum KbOutcome { Live, NextStart }`
  - `pub struct KeyboardTab { pub shown: Keyboard, pub applied: Keyboard, pub registry: Result<Registry, String>, pub applying: bool, pending: Option<Keyboard>, pub note: Option<String>, pub try_text: String }`
  - `impl KeyboardTab { pub fn new(kb: Keyboard, registry: Result<Registry, String>) -> KeyboardTab; pub fn event(&mut self, e: KbEvent) -> Option<Keyboard /* start an apply */>; pub fn done(&mut self, kb: &Keyboard, r: Result<KbOutcome, String>) -> Option<Keyboard /* next apply */>; pub fn layouts(&self) -> Vec<(String /*name*/, String /*label*/)>; pub fn variants(&self) -> Vec<(String, String)> /* first ("", "Default") */ }`
  - `pub fn validate(kb: &Keyboard, reg: Option<&Registry>) -> Result<(), String>` — names pass `xkb_name_ok`; with a registry the layout is in it and the variant is empty or one of its variants; without a registry only the applied layout and variant are allowed.
  - Behaviour: a layout change sets `variant = ""`; `event` returns `Some(shown)` when not applying and `shown != applied`, else stores `pending = Some(shown)` (a later change replaces it); `done` sets `applying = false`, `applied` on `Ok`, `note` = `None` for `Live`, `Some("Takes effect at the next start.")` for `NextStart`, the error text otherwise; then returns `pending.take()` when it differs from `applied`. `TryText` only updates `try_text`. A validation error sets `note` and starts nothing. With `registry` `Err(e)`, `layouts()` returns only the applied layout and `note` starts as `Some(e)`.
  - `Menu.keyboard: KeyboardTab`; `SettingsEvent::Keyboard(KbEvent)` → `Effect::Start(Job::KeyboardApply(kb))`; `JobResult::KeyboardApplied { kb: Keyboard, result: Result<KbOutcome, String> }`; ignored while `config_notes.legacy` is set (A9).

- [ ] **Step 1: Failing tests** (`keyboard.rs`, fixture registry from B1)

```rust
#[test] fn a_layout_change_resets_the_variant()          // shown {ch, de_nodeadkeys}; Layout("de") -> shown.variant == ""
#[test] fn every_change_starts_an_apply()                // CapsAsCtrl(false) -> Some(kb with caps false)
#[test] fn a_change_during_an_apply_waits_and_merges()   // first change -> Some; two more -> None, None; done(first, Ok(Live)) -> Some(state after the third change)
#[test] fn an_old_result_cannot_overwrite_a_newer_choice() // after the merge above, shown stays the third change while the second apply runs
#[test] fn validation_refuses_a_foreign_variant()         // {us, "de_nodeadkeys"} -> Err; {ch, ""} Ok; {ch, "fr"} Ok
#[test] fn without_a_registry_only_the_current_layout_is_offered() // Err("…evdev.xml…") -> layouts() == [(applied, applied)], note Some
#[test] fn next_start_says_so()                           // done(.., Ok(NextStart)) -> note == Some("Takes effect at the next start.")
#[test] fn try_text_starts_nothing()
// settings.rs / menu.rs
#[test] fn the_tabs_are_in_order()                        // update menu.rs:2802: ALL == [Packages, Network, Screen, Keyboard, Status]; Keyboard.label() == "Keyboard"
#[test] fn a_keyboard_event_starts_the_job()              // menu.settings(Keyboard(Compose(Menu))) -> Some(Effect::Start(Job::KeyboardApply(..compose Menu..)))
```

- [ ] **Step 2: Run to fail** — `MENUTEST keyboard:: settings menu::` → FAIL.
- [ ] **Step 3: Implement.** `build_menu` creates `KeyboardTab::new(store.cfg.keyboard.clone(), kbregistry::read(path))` (registry read at start).
- [ ] **Step 4: Run** — `MENUTEST` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: keyboard tab state with one apply at a time`.

### Task B3: the apply job

**Files:**
- Modify: `src/thinfloe-menu/src/jobs.rs` (`Paths`, new `keyboard_apply`), `src/thinfloe-menu/src/app.rs:480-595` (dispatch, panic result), `src/thinfloe-menu/src/main.rs` (`Paths` setup)

**Interfaces:**
- Consumes: `store::Shared::update(Table::Keyboard, …)`, the composer `--env-only` (A11).
- Produces:
  - `pub struct LabwcCmds { pub compose: Vec<String> /* TF_COMPOSE_LABWC or /usr/bin/thinfloe-compose-labwc-config */, pub labwc: Vec<String> /* TF_LABWC or labwc */, pub system_dir: PathBuf /* /etc/thinfloe/labwc */, pub config_dir: Option<PathBuf> /* TF_LABWC_CONFIG_DIR */, pub labwc_pid: Option<String> /* LABWC_PID */ }` in `Paths.labwc`; built in `main.rs` from the environment (read, never set).
  - `pub fn keyboard_apply(paths: &Paths, kb: &Keyboard) -> Result<KbOutcome, String>`:
    1. `update(Table::Keyboard, |c| c.keyboard = kb.clone())`; error → `Err("Could not save the keyboard settings: <e>")`.
    2. `config_dir` is `None` or equals `system_dir` → `Ok(NextStart)`.
    3. Run `compose --env-only <system_dir> <generated/environment> <environment> <config_dir>`; non-zero → `Err("The keyboard settings could not be handed to labwc: <stderr, first line>")`.
    4. `labwc_pid` `None` or empty → `Err("labwc is not known to the menu (LABWC_PID is not set); the change takes effect at the next start.")`.
    5. Run `labwc --reconfigure` (with `LABWC_PID` in its environment); non-zero → `Err("labwc --reconfigure failed (<status>)")`; else `Ok(Live)`.
    Children run through the existing `capture`/`run_logged` helpers with a 10 s deadline (`KEYBOARD_TIMEOUT`) and a process group of their own, logged under source `keyboard`.

- [ ] **Step 1: Failing tests** (`jobs.rs`; stub compose and labwc are `sh -c` scripts writing their argv to a file, like `jobs.rs:1141`)

```rust
#[test] fn keyboard_apply_writes_config_then_composes_then_reconfigures() // call order file == ["compose --env-only <sys> <gen> <env> <dir>", "labwc --reconfigure"]; config.toml keyboard == kb; generated/environment has "XKB_DEFAULT_VARIANT=" line
#[test] fn the_plain_fallback_dir_means_next_start()      // config_dir == system_dir -> Ok(NextStart), no child ran, config.toml written
#[test] fn a_failing_composer_is_reported_and_labwc_not_asked() // compose exits 1 with "boom" on stderr -> Err contains "boom", labwc not called
#[test] fn a_missing_labwc_pid_is_reported()
#[test] fn a_failing_reconfigure_is_reported()
```

- [ ] **Step 2: Run to fail** — `MENUTEST keyboard_apply` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — `MENUTEST` → PASS; `MENULINT`.
- [ ] **Step 5: Commit** — `feat: apply a keyboard change to the running session`.

### Task B4: the tab's UI

**Files:**
- Create: `src/thinfloe-menu/src/keyboardtab_ui.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/ui.rs:947-955` (`SettingsTab::Keyboard` arm), `ui.rs:3219-3250` (tab snapshot cases), `src/thinfloe-menu/src/brand.rs:344-361` (add `("keyboardtab_ui.rs", include_str!("keyboardtab_ui.rs"))`)

**Interfaces:**
- Consumes: `keyboard::KeyboardTab`, `config::Compose::{ALL, label}`, shadcn-egui `Select`, `components::switch::Switch`, `components::input::Input`.
- Produces: `pub fn keyboard_tab(ui: &mut Ui, tab: &KeyboardTab, width: f32) -> Option<KbEvent>` — card 1 (frame like `idle_band`, `ui.rs:1061-1085`: `t.palette().card`, `ROW_LEFT`/`ROW_RIGHT` margins, rows with a `brand::FORM_LABEL_W` label column): `Layout` (Select of `layouts()` labels), `Variant` (Select of `variants()`), `CapsLock acts as Ctrl` (Switch), `Compose key` (Select of the eight labels); under the card the note in `muted_foreground`, nothing when `note` is `None`. Card 2: `Try it here`, a single-line `Input` on `try_text`. Controls stay enabled while an apply runs (changes queue in B2).

- [ ] **Step 1: Failing tests** (`ui.rs` test module, as the other tab tests)

```rust
#[test] fn the_keyboard_tab_shows_its_rows()     // labels "Layout", "Variant", "CapsLock acts as Ctrl", "Compose key", "Try it here" laid out; label column width == brand::FORM_LABEL_W
#[test] fn the_keyboard_note_shows_an_error()   // tab.note Some("labwc --reconfigure failed (1)") -> text present; None -> no note text
// the existing tab snapshot loop (ui.rs:3219) now covers Keyboard at 1280x720 and the small size the other tabs use, with "Layout" expected
```

- [ ] **Step 2: Run to fail** — `MENUTEST ui::` → FAIL.
- [ ] **Step 3: Implement.** No style literal: sizes and colours from `brand`/theme (`ui_code_has_no_style_literals` must pass).
- [ ] **Step 4: Run** — `MENUTEST` → PASS (including `brand::tests::ui_code_has_no_style_literals`); `MENULINT`.
- [ ] **Step 5: Commit** — `feat: Keyboard tab in the Settings screen`.

### Task B5: runtime test, docs, CHANGES — **Gate B (`make image`)**

**Files:**
- Modify: `test/runtime/test-labwc-runtime.sh` (new case after the duplicate-assignment case, `:124-165`), `docs/notes/session.md`, `docs/notes/menu.md`, `CHANGES.md`, `CLAUDE.md` (Where code lives: keyboard files, one line)

- [ ] **Step 1: Runtime case** inside one `podman run` of the runtime image (same style as `:150`): compose fully with a generated env `XKB_DEFAULT_LAYOUT=us`, start `labwc -C … -S "<script>"`; the script writes a generated env `XKB_DEFAULT_LAYOUT=ch`, `XKB_DEFAULT_VARIANT=de_nodeadkeys`, `XKB_DEFAULT_OPTIONS=compose:ralt,ctrl:nocaps`, runs `thinfloe-compose-labwc-config --env-only …`, `labwc --reconfigure`, sleeps 1, prints the merged file's last `XKB_DEFAULT_LAYOUT` line. First a control run with `XKB_DEFAULT_LAYOUT=fr` and `XKB_DEFAULT_VARIANT=de_nodeadkeys` (a keymap XKB cannot build):
  - control prints `failed to create xkb keymap` in labwc's output → the rig can see keymap errors; then assert `ok "a keyboard apply reaches labwc's environment"` (`XKB_DEFAULT_LAYOUT=ch`) and `ok "labwc built the new keymap"` (no `failed to create xkb keymap` after the apply).
  - control prints nothing → the merged-file assertion still runs, and the keymap assertion prints `skip - labwc builds no keymap without a keyboard device (headless rig)`; the executor reports this skip (the spec notes the rig has no keyboard device, `test-labwc-runtime.sh:77-82`).
- [ ] **Step 2: Docs**: `session.md` — the keyboard apply path (config.toml → generated/environment → composer `--env-only` → `labwc --reconfigure`), why the full rebuild stays at session start (labwc-scale's copies), and a "Left to verify on hardware" entry: new keymap reaches the menu and other Wayland clients; an open ThinLinc client follows the layout (if not, the note text becomes "ThinLinc picks up the change when it starts again" and the result is recorded here). `menu.md` — the Keyboard tab.
- [ ] **Step 3: CHANGES.md** (New): "The Settings screen has a Keyboard tab: pick the layout and its variant from the full list, CapsLock as Ctrl on or off, and the Compose key (Right Alt by default). A change applies at once to the running session, with a line to try it right there, and is kept with the home directory."
- [ ] **Step 4: Gate B**

```bash
make image BUILD_JOBS=4
make test BUILD_JOBS=4
make test-runtime BUILD_JOBS=4
make qemu-smoke-upgrade BUILD_JOBS=4
```

Expected: all green, `test-session-packages.sh` shows `ok - the image has the XKB registry`; record ok/skip counts and the delta against Gate A.
- [ ] **Step 5: Commit** — `test: keyboard apply in the labwc runtime test` and `docs: Keyboard tab` (two commits).

---

## Spec issues found

1. **`home-layout` cannot be rewritten by the menu as specified.** `/run/thinfloe/state` is created by the initramfs as root (`thinfloe-lib.sh:286`) and the menu runs as tfuser, so "rewrites it to `new` after step 4" fails, and a temp-file-plus-rename is impossible in a directory tfuser cannot write. Resolution: the initramfs `chown`s the file to `1001:1001` (uid fixed by the Dockerfile, `:268-273`; `chown` added to `module-setup.sh`), the menu rewrites it in place, and every reader treats only the exact word `legacy` as legacy, so a torn write (empty file) after a successful migration reads as `new`. Tasks A5, A10.
2. **"Converted as far as the parser goes" takes a machine offline.** `parse_wifi` drops every password of a hand file it cannot fully keep (`netfiles.rs:381-389`), so an EAP or otherwise unusual `~/wifi.yaml` would become a generated file without credentials at the second boot, against the first anti-goal. A hand kanshi config cannot be parsed at all (`kanshi.rs:390-394`). Resolution: such a file is kept verbatim in `config.toml` (`kept_wifi`, `kept_wired`, `[monitors] kept`), written under the generated header, and shown read-only with the existing "Let the menu manage it" button. `config.toml` stays the only source and `generated/` stays derived; the `.by-hand` copies and the session profile winning still go away. Tasks A1, A3, A7, A8.
3. **What the menu does after a failed migration is not specified.** With `home-layout` still `legacy`, the shell readers use the old files, so a menu that wrote `config.toml` and `generated/` would change nothing visible and the next migration attempt could overwrite those changes. Resolution: the Network, Screen, Status and Keyboard tabs are read-only with a note for that session; Packages works. The profile composes a legacy session with `~/.thinfloe-environment` in the generated slot, so its `XKB_DEFAULT_*` lines are not dropped as "managed". Tasks A9, A12.
4. **Restore outcomes without a listing.** The spec covers no archive, a marked and an unmarked listing. A fetch failure or a missing boot MAC leaves the shipped default home with no listing; the plan writes `new` for it, the layout that home really has. Task A10.
5. **The QEMU smoke boots never start a session.** They boot to `multi-user.target` (`QEMU_SMOKE_APPEND`, `Makefile:299`), so the profile never runs the migration and nothing saves the home between boots. Resolution: the smoke report script runs `thinfloe-menu --migrate-home` as tfuser and `thinfloe-save-home`, then a second boot checks the generated files. The stick's 3.x home gets a `wifi.yaml` and an idle line; after the initramfs renames it is a 4.x home, so one stick covers both migrations. The 4.x-only listing rule (`.thinfloe-home-version` without the new marker) is pinned in `test-initramfs-home.sh`. Task A14.
6. **Comment-only templates.** Copying "the rest of" `~/.thinfloe-sessionrc` would carry the old template's long comments (which name `~/.thinfloe-environment`) into the new `sessionrc`. Resolution: the rest is appended only when it has an active line other than the idle assignment. `environment` takes only active lines, as the spec says. Task A5.
7. **`[keyboard]` is listed as Part B, but Part A's migration writes it** (step 1 converts `XKB_DEFAULT_*`), and Part A must leave the keyboard working. Resolution: the `[keyboard]` table, `Compose`, `xkb_options` and `generated/environment` are built in Part A; Part B adds the registry, the tab and the live apply. Tasks A1, A2.
8. **The headless runtime rig may not build a keymap at all** (no keyboard device, `test-labwc-runtime.sh:77-82`), which would make "no `failed to create xkb keymap`" pass whatever the menu writes. Resolution: a control run with a keymap XKB cannot build decides whether the assertion counts; if the control shows no error, the assertion is reported as a skip, not a pass. Task B5.
