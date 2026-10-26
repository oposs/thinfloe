//! Entry point. The exit status is the contract with thinfloe-session:
//! 0 means log out, 10 restart and 11 power off, each only after this
//! program's own save or the person's choice of "Log out anyway" (or
//! "Restart anyway", "Power off anyway") following a failed save.
//! thinfloe-session saves on no status at all; any other status means the menu
//! failed.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use thinfloe_menu::addons;
use thinfloe_menu::app::{self, Config};
use thinfloe_menu::header;
use thinfloe_menu::icons::{self, IconRoots};
use thinfloe_menu::jobs::{self, Paths};
use thinfloe_menu::menu::Menu;
use thinfloe_menu::{settings, tiles, ui};

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
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

fn addon_last_check_path() -> PathBuf {
    env_path("TF_ADDON_LAST_CHECK", addons::LAST_CHECK_PATH)
}

/// `marker` remembers the add-ons offered a retry; `--check` passes one in
/// memory, so that listing the tiles asks nothing away.
fn build_menu(marker: settings::RetryMarker) -> (Menu, Paths, IconRoots) {
    let shipped = env_path("TF_MENU_SHIPPED_DIR", "/etc/thinfloe/menu");
    let paths = Paths {
        save_home: env_path("TF_SAVE_HOME", "/usr/bin/thinfloe-save-home"),
        state_dir: env_path("TF_STATE_DIR", "/run/thinfloe/state"),
        addon_cmd: jobs::default_addon_cmd(),
    };
    let mut grid = tiles::load(&shipped, &user_dir());
    let path = program_path();
    tiles::mark_missing_programs(&mut grid.tiles, &path);
    let mut menu = Menu::new(grid, jobs::restore_failed(&paths.state_dir));
    menu.set_retry_marker(marker);
    // The app lets it go once the grid is on screen.
    menu.hold_retry(true);
    menu.last_check = addons::read_last_check(&addon_last_check_path());
    // Before the first icon is drawn, so an add-on that is installed has its
    // tile, program and icon from the start.
    menu.set_addons(addons::read_status(&addon_status_path()));
    menu.refresh_programs(&path);
    (menu, paths, IconRoots::system())
}

/// Whether `--check` fails for a tile. An add-on's program and icon arrive
/// with the add-on; until it is ticked and installed, the grey tile with a
/// letter is correct.
fn is_defect(t: &tiles::Tile, shipped: bool, icon_drawn: bool) -> bool {
    shipped && !t.addon && (t.problem.is_some() || !icon_drawn)
}

/// `thinfloe-menu --check`: print every tile and how its icon draws, and fail
/// if a shipped tile is unusable or draws its letter. For the image tests,
/// and for an operator at a VT asking why a tile is grey.
fn check() -> ExitCode {
    let (menu, _, roots) = build_menu(settings::RetryMarker::memory());
    let mut bad = false;
    for line in &menu.banner {
        println!("banner: {line}");
    }
    let icons = ui::render_icons(&menu.tiles, &roots);
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
        println!("{}\t{}\t{:?}\t{}\ticon: {}", t.file, t.name, t.action, state, icon_text);
        if is_defect(t, shipped, drawn) {
            bad = true;
        }
    }
    if bad { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == icons::RENDER_ICON_ARG) {
        return icons::render_icon_main(&args[2..]);
    }
    if args.get(1).is_some_and(|a| a == "--check") {
        return check();
    }
    let (menu, paths, icon_roots) = build_menu(settings::RetryMarker::load(&settings::retry_marker_path()));
    let debug = std::env::var_os("TF_MENU_DEBUG").is_some();
    if debug {
        eprintln!("thinfloe-menu: {} tiles", menu.tiles.len());
    }
    match app::run(Config {
        menu,
        paths,
        icon_roots,
        debug,
        addon_status: addon_status_path(),
        addon_log: env_path("TF_ADDON_LOG", addons::LOG_PATH),
        addon_last_check: addon_last_check_path(),
        program_path: program_path(),
        header: header::read_header(&env_path("TF_VERSION_FILE", "/etc/thinfloe/version")),
    }) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("thinfloe-menu: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn check_fails_for_a_broken_shipped_tile_but_not_for_a_missing_addon() {
        assert!(is_defect(&tile(false, Some("x is not installed")), true, true));
        assert!(is_defect(&tile(false, None), true, false));
        assert!(!is_defect(&tile(true, Some("x is not installed")), true, false));
        assert!(!is_defect(&tile(false, Some("broken")), false, false));
        assert!(!is_defect(&tile(false, None), true, true));
    }
}
