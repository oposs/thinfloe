//! Entry point. The exit status is the contract with thinfloe-session:
//! 0 means log out, 10 restart and 11 power off, each only after this
//! program's own save or the person's choice of "Log out anyway" (or
//! "Restart anyway", "Power off anyway") following a failed save.
//! thinfloe-session saves on no status at all; any other status means the menu
//! failed.
//!
//! Debugging, through the environment:
//!
//! - `TF_MENU_DEBUG` (any value): stderr gets the window's configure sizes,
//!   the scale and the monitor it was taken from, the frames rasterised,
//!   the tile icons drawn, folded windows, the menu output asked of
//!   labwc-scale and each window replacement.
//! - `TF_MENU_DEBUG_MOVE=<connector>`: from the start and for as long as
//!   it runs, the menu acts as if *Show the menu here* chose that monitor,
//!   whatever the Screen tab holds: it writes the name to its output file
//!   and replaces its window once labwc-scale confirms (a test hook for
//!   the runtime tests).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use thinfloe_menu::addons;
use thinfloe_menu::app::{self, Config, ScreenFile};
use thinfloe_menu::battery;
use thinfloe_menu::config::{self, ConfigDir};
use thinfloe_menu::generated::Inputs;
use thinfloe_menu::header;
use thinfloe_menu::icons::{self, IconRoots};
use thinfloe_menu::jobs::{self, Paths};
use thinfloe_menu::log::Kind;
use thinfloe_menu::menu::{ConfigNotes, LEGACY_NOTE, Menu};
use thinfloe_menu::store::{self, OpenMode};
use thinfloe_menu::{bounded, migrate, release, screentab, settings, status, tiles, ui, windows};

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

/// The XKB registry file: `TF_XKB_REGISTRY` when set and not empty (tests),
/// else the image's. Read-only on the environment.
// Task B2 calls this from build_menu; until then only the test does.
#[allow(dead_code)]
fn registry_path() -> PathBuf {
    match std::env::var_os("TF_XKB_REGISTRY") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(thinfloe_menu::kbregistry::REGISTRY_PATH),
    }
}

fn user_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TF_MENU_USER_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/tfuser"));
    home.join(".config/thinfloe/menu")
}

fn program_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into())
}

fn addon_status_path() -> PathBuf {
    env_path("TF_ADDON_STATUS", addons::STATUS_PATH)
}

fn upgrade_status_path() -> PathBuf {
    env_path("TF_UPGRADE_STATUS", release::STATUS_PATH)
}

/// Whether `esp` is a mount point, as Python's `os.path.ismount` decides
/// for a directory: it lies on another device than its parent. The
/// initramfs mounts the boot stick's ESP there on a USB boot only.
/// A symlink is no mount point, as there.
fn usb_boot(esp: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(dir), Ok(parent)) = (std::fs::symlink_metadata(esp), std::fs::metadata(esp.join(".."))) else {
        return false;
    };
    dir.is_dir() && dir.dev() != parent.dev()
}

/// `sudo -n /usr/bin/thinfloe-network`; `TF_NETWORK_CMD` (space-separated)
/// replaces it for tests.
fn network_cmd() -> Vec<String> {
    match std::env::var("TF_NETWORK_CMD") {
        Ok(v) if !v.trim().is_empty() => v.split_whitespace().map(String::from).collect(),
        _ => jobs::default_network_cmd(),
    }
}

fn battery_file() -> PathBuf {
    env_path("TF_BATTERY_FILE", battery::FILE)
}

fn addon_last_check_path() -> PathBuf {
    env_path("TF_ADDON_LAST_CHECK", addons::LAST_CHECK_PATH)
}

/// What rendering the generated files needs besides the settings: the
/// shipped wired default (unreadable: empty, and a fixed wired address then
/// fails to render with its message) and whether this is a netboot.
fn config_inputs() -> Inputs {
    let default_wired = thinfloe_menu::netfiles::Homes::from_env().default_wired;
    Inputs {
        default_wired: bounded::read_bounded(&default_wired, MIGRATE_MAX_BYTES, true)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default(),
        netboot: !usb_boot(&env_path("TF_ESP_DIR", "/run/thinfloe/storage/efi")),
    }
}

fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// What `build_menu` hands the caller besides the menu.
struct Built {
    menu: Menu,
    paths: Paths,
    roots: IconRoots,
    /// Starting up regenerated `generated/kanshi`: the running kanshi must
    /// read it again.
    kanshi_changed: bool,
    /// What the store says about the settings files, for `--check` to print;
    /// a session menu has them in its log.
    notes: Vec<String>,
}

/// A home still in the old layout is only read, whatever was asked.
fn open_mode(requested: OpenMode, layout: migrate::HomeLayout) -> OpenMode {
    if layout == migrate::HomeLayout::Legacy {
        OpenMode::ReadOnly
    } else {
        requested
    }
}

/// Open the settings store as the home layout in `state_dir` allows.
fn open_settings(
    state_dir: &Path,
    dir: ConfigDir,
    inputs: Inputs,
    requested: OpenMode,
    now: i64,
) -> (store::Opened, migrate::HomeLayout) {
    let layout = migrate::home_layout(state_dir);
    (store::open(dir, inputs, open_mode(requested, layout), now), layout)
}

/// What the Settings screen says about the config directory. On a legacy
/// home the store's own notes (it reads a `config.toml` that is not the
/// truth there) and the environment lines give way to the one legacy note.
fn collect_notes(
    layout: migrate::HomeLayout,
    store_notes: Vec<String>,
    user_env: Option<&str>,
    managed: &[String],
    report: Option<String>,
) -> ConfigNotes {
    if layout == migrate::HomeLayout::Legacy {
        return ConfigNotes {
            legacy: Some(LEGACY_NOTE.to_string()),
            ..ConfigNotes::default()
        };
    }
    ConfigNotes {
        broken: store_notes,
        dropped_env: user_env.map_or_else(Vec::new, |t| config::dropped_env_lines(t, managed)),
        report,
        legacy: None,
    }
}

/// `marker` remembers the add-ons offered a retry; `--check` passes one in
/// memory, so that listing the tiles asks nothing away. `mode` is how the
/// settings store is opened: the one open of the whole run. A home still in
/// the old layout (its migration failed) is only read, never rebuilt into
/// the new one.
fn build_menu(marker: settings::RetryMarker, mode: OpenMode) -> Built {
    let shipped = env_path("TF_MENU_SHIPPED_DIR", "/etc/thinfloe/menu");
    let state_dir = env_path("TF_STATE_DIR", "/run/thinfloe/state");
    let session = mode == OpenMode::Session;
    let dir = ConfigDir::from_env();
    let (opened, layout) = open_settings(&state_dir, dir.clone(), config_inputs(), mode, epoch_now());
    let config_notes = collect_notes(
        layout,
        opened.notes,
        bounded::read_text(&dir.environment(), ENV_MAX_BYTES).as_deref(),
        &config::read_managed_keys(&env_path("TF_MANAGED_KEYS", "/etc/thinfloe/menu-managed-keys")),
        opened.store.cfg.notice.migration.clone(),
    );
    // What `--check` prints and the session log keeps.
    let notes: Vec<String> = config_notes
        .broken
        .iter()
        .chain(&config_notes.legacy)
        .cloned()
        .collect();
    let (screen_tab, status_layout) = (
        screentab::from_config(&opened.store.cfg.screen),
        status::layout_from(&opened.store.cfg.status),
    );
    let paths = Paths {
        save_home: env_path("TF_SAVE_HOME", "/usr/bin/thinfloe-save-home"),
        state_dir,
        addon_cmd: jobs::default_addon_cmd(),
        upgrade_cmd: jobs::default_upgrade_cmd(),
        network_cmd: network_cmd(),
        homes: thinfloe_menu::netfiles::Homes::from_env(),
        kanshi: jobs::Kanshi::from_env(),
        config: std::sync::Arc::new(std::sync::Mutex::new(opened.store)),
        layout,
    };
    let mut grid = tiles::load(&shipped, &user_dir());
    let path = program_path();
    tiles::mark_missing_programs(&mut grid.tiles, &path);
    let mut menu = Menu::new(grid, jobs::restore_failed(&paths.state_dir));
    if session {
        for note in &notes {
            menu.log.note(Kind::Warn, note.clone());
        }
    }
    menu.set_retry_marker(marker);
    // The app lets it go once the grid is on screen.
    menu.hold_retry(true);
    menu.last_check = addons::read_last_check(&addon_last_check_path());
    // Before the first icon is drawn, so an add-on that is installed has its
    // tile, program and icon from the start.
    menu.set_addons(addons::read_status(&addon_status_path()));
    menu.refresh_programs(&path);
    // The Upgrade tile is drawn on a USB boot only; a netbooted machine has
    // no stick of its own to upgrade.
    menu.set_usb_boot(usb_boot(&env_path("TF_ESP_DIR", "/run/thinfloe/storage/efi")));
    menu.upgrade.release_file(release::read_status(&upgrade_status_path()));
    menu.screen_tab = screen_tab;
    menu.config_notes = config_notes;
    menu.set_battery(battery::read(&battery_file()), std::time::Instant::now());
    menu.set_status_layout(status_layout);
    Built {
        menu,
        paths,
        roots: IconRoots::system(),
        kanshi_changed: opened.kanshi_changed,
        notes,
    }
}

/// Whether `--check` fails for a tile. An add-on's program and icon arrive
/// with the add-on; until it is ticked and installed, the grey tile with a
/// letter is correct.
fn is_defect(t: &tiles::Tile, shipped: bool, icon_drawn: bool) -> bool {
    shipped && !t.addon && (t.problem.is_some() || !icon_drawn)
}

/// One tile's line of `--check`. A reworded tile also says which keys of
/// the user's file were ignored.
fn check_line(t: &tiles::Tile, state: &str, icon_text: &str) -> String {
    let mut line = format!("{}\t{}\t{:?}\t{}\ticon: {}", t.file, t.name, t.action, state, icon_text);
    if let Some(r) = t.origin.reworded_text() {
        line.push_str(&format!("\t{r}"));
    }
    line
}

/// What `--check` says about the status layout: where it comes from, then
/// each line that was skipped or changed. A layout problem does not fail the
/// check: the menu runs with the shipped layout.
fn status_lines(config_toml: &Path, layout: &status::layout::Layout) -> Vec<String> {
    let missing = std::fs::symlink_metadata(config_toml).is_err();
    let mut lines = vec![if missing {
        format!("layout from {} (missing: the shipped layout)", config_toml.display())
    } else {
        format!("layout from {}", config_toml.display())
    }];
    for note in &layout.notes {
        lines.push(if layout.refused {
            format!("{note}; the shipped layout is used")
        } else {
            note.clone()
        });
    }
    lines
}

/// `thinfloe-menu --check`: print every tile and how its icon draws, and fail
/// if a shipped tile is unusable or draws its letter. For the image tests,
/// and for an operator at a VT asking why a tile is grey.
fn check() -> ExitCode {
    let Built {
        menu,
        paths,
        roots,
        notes,
        ..
    } = build_menu(settings::RetryMarker::memory(), OpenMode::ReadOnly);
    let config_toml = paths
        .config
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .dir
        .config_toml();
    let mut bad = false;
    for line in &menu.banner {
        println!("banner: {line}");
    }
    for note in &notes {
        println!("config: {note}");
    }
    for line in status_lines(&config_toml, &menu.status.layout) {
        println!("status: {line}");
    }
    let icons = ui::render_icons(&menu.tiles, &roots, ui::render_side(1.0));
    for (t, icon) in menu.tiles.iter().zip(icons) {
        let shipped = t.origin != tiles::Origin::User;
        let state = match &t.problem {
            Some(p) if t.addon => format!("DISABLED ({p}; add-on, not installed)"),
            Some(p) => format!("DISABLED ({p})"),
            None => "ok".to_string(),
        };
        let (icon_text, drawn) = match icon {
            None => ("letter fallback".to_string(), false),
            Some((path, Ok(_))) => (path.display().to_string(), true),
            Some((path, Err(e))) => (format!("letter fallback ({}: {e})", path.display()), false),
        };
        println!("{}", check_line(t, &state, &icon_text));
        if is_defect(t, shipped, drawn) {
            bad = true;
        }
    }
    if bad { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

/// `--windows`: one line per toplevel, `app_id<TAB>title<TAB>folded|open`.
fn list_windows() -> ExitCode {
    let windows = match windows::Windows::connect(|| {}) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("thinfloe-menu: {e}");
            return ExitCode::from(1);
        }
    };
    for (_, app_id, title, folded) in windows.all() {
        println!("{app_id}\t{title}\t{}", if folded { "folded" } else { "open" });
    }
    ExitCode::SUCCESS
}

/// `--unfold <app_id or title>`: for an operator whose window went missing.
fn unfold(what: Option<&str>) -> ExitCode {
    let Some(what) = what else {
        eprintln!("usage: thinfloe-menu --unfold <app_id or title>");
        return ExitCode::from(2);
    };
    let windows = match windows::Windows::connect(|| {}) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("thinfloe-menu: {e}");
            return ExitCode::from(1);
        }
    };
    let hits: Vec<_> = windows
        .folded()
        .into_iter()
        .filter(|f| f.app_id == what || f.title == what)
        .collect();
    for f in &hits {
        if let Err(e) = windows.restore(f.id) {
            eprintln!("thinfloe-menu: {e}");
            return ExitCode::from(1);
        }
    }
    if hits.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// `--migrate-home`: the session profile runs it while `home-layout` says
/// `legacy`. The summary or the reason goes to stderr (the session log).
fn migrate_home() -> ExitCode {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map_or_else(|| PathBuf::from("/home/tfuser"), PathBuf::from);
    let state_dir = env_path("TF_STATE_DIR", "/run/thinfloe/state");
    let inputs = config_inputs();
    let now = epoch_now();
    match migrate::migrate_home(&home, &state_dir, inputs, now) {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(report)) => {
            eprintln!("thinfloe-menu: {}", report.summary());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("thinfloe-menu: the home was not migrated: {e}");
            ExitCode::from(1)
        }
    }
}

/// The shipped wired default is tiny.
const MIGRATE_MAX_BYTES: u64 = 64 * 1024;
/// The user's `environment` is a few lines; this bounds the read for the
/// notes about ignored ones.
const ENV_MAX_BYTES: u64 = 64 * 1024;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == icons::RENDER_ICON_ARG) {
        return icons::render_icon_main(&args[2..]);
    }
    if args.get(1).is_some_and(|a| a == "--check") {
        return check();
    }
    if args.get(1).is_some_and(|a| a == "--windows") {
        return list_windows();
    }
    if args.get(1).is_some_and(|a| a == "--unfold") {
        return unfold(args.get(2).and_then(|a| a.to_str()));
    }
    if args.get(1).is_some_and(|a| a == "--migrate-home") {
        return migrate_home();
    }
    let Built {
        mut menu,
        paths,
        roots: icon_roots,
        kanshi_changed,
        ..
    } = build_menu(
        settings::RetryMarker::load(&settings::retry_marker_path()),
        OpenMode::Session,
    );
    let config_toml = ConfigDir::from_env().config_toml();
    let debug = std::env::var_os("TF_MENU_DEBUG").is_some();
    if debug {
        eprintln!("thinfloe-menu: {} tiles", menu.tiles.len());
    }
    let header = header::read_header(&env_path("TF_VERSION_FILE", "/etc/thinfloe/version"));
    menu.set_installed(header.version.clone());
    match app::run(Config {
        menu,
        paths,
        icon_roots,
        debug,
        addon_status: addon_status_path(),
        addon_log: env_path("TF_ADDON_LOG", addons::LOG_PATH),
        addon_last_check: addon_last_check_path(),
        battery_file: battery_file(),
        config_toml,
        kanshi_changed,
        network_roots: thinfloe_menu::netstat::Roots::system(),
        hardware_roots: thinfloe_menu::hardware::Roots::system(),
        program_path: program_path(),
        header,
        upgrade_status: upgrade_status_path(),
        screen: ScreenFile {
            idle_pid: screentab::idle_pid(std::env::var_os("TF_IDLE_PID").as_deref()),
        },
        debug_move: std::env::var("TF_MENU_DEBUG_MOVE").ok().filter(|n| !n.is_empty()),
    }) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("thinfloe-menu: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
#[path = "testdir.rs"]
mod testdir;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn the_registry_path_is_the_override_or_the_image_file() {
        // The environment is read only, never set (no set_var under src/).
        let p = registry_path();
        match std::env::var_os("TF_XKB_REGISTRY").filter(|v| !v.is_empty()) {
            Some(v) => assert_eq!(p, PathBuf::from(v)),
            None => assert_eq!(p, PathBuf::from(thinfloe_menu::kbregistry::REGISTRY_PATH)),
        }
    }

    #[test]
    fn usb_boot_is_a_mount_point_test() {
        assert!(usb_boot(Path::new("/proc")));
        let d = TestDir::new("main", "usb-boot");
        assert!(!usb_boot(&d));
        assert!(!usb_boot(&d.join("missing")));
    }

    fn settings_in(layout_word: Option<&str>, tag: &str) -> (TestDir, TestDir, store::Opened) {
        let state = TestDir::new("main", &format!("{tag}-state"));
        let cfg = TestDir::new("main", &format!("{tag}-cfg"));
        if let Some(w) = layout_word {
            std::fs::write(state.join("home-layout"), w).unwrap();
        }
        let (opened, _) = open_settings(
            &state,
            ConfigDir {
                root: cfg.to_path_buf(),
            },
            Inputs::default(),
            OpenMode::Session,
            0,
        );
        (state, cfg, opened)
    }

    #[test]
    fn a_legacy_home_opens_read_only_and_leaves_the_directory_empty() {
        let (_s, cfg, mut opened) = settings_in(Some("legacy\n"), "legacy");
        assert_eq!(
            opened
                .store
                .update(thinfloe_menu::generated::Table::Screen, |c| c.screen.idle_timeout = 60),
            Err("The settings are read-only.".to_string())
        );
        assert_eq!(opened.store.cfg.screen.idle_timeout, screentab::DEFAULT_IDLE);
        assert_eq!(std::fs::read_dir(&*cfg).unwrap().count(), 0, "nothing was created");
    }

    #[test]
    fn a_new_or_unrecorded_home_opens_as_a_session() {
        for (word, tag) in [(Some("new\n"), "new"), (None, "none"), (Some("lega"), "torn")] {
            let (_s, cfg, mut opened) = settings_in(word, tag);
            assert!(cfg.join("config.toml").exists(), "{tag}: config.toml is made");
            opened
                .store
                .update(thinfloe_menu::generated::Table::Screen, |c| c.screen.idle_timeout = 60)
                .unwrap();
            assert!(cfg.join("generated/screen").exists(), "{tag}");
        }
    }

    #[test]
    fn open_mode_only_lowers_a_legacy_home() {
        use migrate::HomeLayout::{Legacy, New};
        assert_eq!(open_mode(OpenMode::Session, Legacy), OpenMode::ReadOnly);
        assert_eq!(open_mode(OpenMode::ReadOnly, Legacy), OpenMode::ReadOnly);
        assert_eq!(open_mode(OpenMode::Session, New), OpenMode::Session);
        assert_eq!(open_mode(OpenMode::ReadOnly, New), OpenMode::ReadOnly);
    }

    #[test]
    fn a_legacy_home_shows_the_legacy_note_instead_of_the_store_notes() {
        let notes = collect_notes(
            migrate::HomeLayout::Legacy,
            vec!["config.toml is missing; defaults are shown.".into()],
            Some("XKB_DEFAULT_LAYOUT=de\n"),
            &["XKB_DEFAULT_LAYOUT".to_string()],
            Some("ignored".into()),
        );
        assert_eq!(
            notes,
            ConfigNotes {
                legacy: Some(LEGACY_NOTE.to_string()),
                ..ConfigNotes::default()
            }
        );
    }

    #[test]
    fn a_new_home_shows_the_store_notes_the_ignored_lines_and_the_report() {
        let notes = collect_notes(
            migrate::HomeLayout::New,
            vec!["config.toml could not be read (x).".into()],
            Some("# c\nXKB_DEFAULT_LAYOUT=de\nXCURSOR_SIZE=32\n"),
            &["XKB_DEFAULT_LAYOUT".to_string()],
            Some("Moved 3 files.".into()),
        );
        assert_eq!(notes.broken, ["config.toml could not be read (x)."]);
        assert_eq!(notes.dropped_env, ["XKB_DEFAULT_LAYOUT=de"]);
        assert_eq!(notes.report.as_deref(), Some("Moved 3 files."));
        assert_eq!(notes.legacy, None);
        let none = collect_notes(migrate::HomeLayout::New, vec![], None, &[], None);
        assert_eq!(none, ConfigNotes::default());
    }

    fn tile(addon: bool, problem: Option<&str>) -> tiles::Tile {
        let mut t = tiles::merge(
            &[tiles::SourceFile {
                name: "10-x.desktop".into(),
                entry: thinfloe_menu::desktop::parse("[Desktop Entry]\nName=X\nExec=x\n"),
            }],
            &[],
        )
        .remove(0);
        t.addon = addon;
        t.problem = problem.map(String::from);
        t
    }

    #[test]
    fn check_names_the_keys_a_reworded_tile_ignored() {
        let mut t = tile(false, None);
        assert_eq!(check_line(&t, "ok", "x"), "10-x.desktop\tX\tRun\tok\ticon: x");
        t.origin = tiles::Origin::Reworded {
            ignored: vec!["Exec".into(), "X-TF-Multiple".into()],
        };
        assert!(
            check_line(&t, "ok", "x").ends_with("\treworded; ignored: Exec, X-TF-Multiple"),
            "{}",
            check_line(&t, "ok", "x")
        );
    }

    #[test]
    fn check_fails_for_a_broken_shipped_tile_but_not_for_a_missing_addon() {
        assert!(is_defect(&tile(false, Some("x is not installed")), true, true));
        assert!(is_defect(&tile(false, None), true, false));
        assert!(!is_defect(&tile(true, Some("x is not installed")), true, false));
        assert!(!is_defect(&tile(false, Some("broken")), false, false));
        assert!(!is_defect(&tile(false, None), true, true));
    }
}
