//! Tiles: where they come from, how a user file may change them, and what
//! keeps one bad file in a restored home from making the machine unusable.

use crate::bounded;
use crate::desktop::{self, Entry};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// At most this many user files are read. The cap is in the spec.
pub const MAX_USER_FILES: usize = 32;
/// The shipped directory is ours, but it goes through the same reader.
pub const MAX_SHIPPED_FILES: usize = 64;
/// Directory entries looked at before giving up, so a directory holding
/// 200000 files cannot stall the grid.
pub const MAX_DIR_ENTRIES: usize = 4096;
/// A tile file larger than this is not a tile file.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Name, Comment and Icon are cut to this many characters. A 60 KB Name
/// would otherwise be laid out and rasterised on every frame.
pub const MAX_TEXT_CHARS: usize = 120;

/// What a tile does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Run,
    SaveHome,
    Logout,
    Reboot,
    Poweroff,
}

impl Action {
    pub fn parse(value: Option<&str>) -> Result<Action, String> {
        match value {
            None | Some("run") => Ok(Action::Run),
            Some("save-home") => Ok(Action::SaveHome),
            Some("logout") => Ok(Action::Logout),
            Some("reboot") => Ok(Action::Reboot),
            Some("poweroff") => Ok(Action::Poweroff),
            Some(other) => Err(format!("unknown action '{other}'")),
        }
    }

    /// The status the menu exits with once this action is done, for the
    /// actions that end the menu. The menu saves the home itself before it
    /// exits, so thinfloe-session saves on none of them; it reboots on 10 and
    /// powers off on 11. Any other status thinfloe-menu exits with is a failure.
    pub fn exit_code(self) -> Option<i32> {
        match self {
            Action::Logout => Some(0),
            Action::Reboot => Some(10),
            Action::Poweroff => Some(11),
            Action::Run | Action::SaveHome => None,
        }
    }

    /// Whether this action ends the session.
    pub fn ends(self) -> bool {
        self.exit_code().is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Shipped,
    /// A user file of the same name changed Name, Comment or Icon. `ignored`
    /// lists the keys it set that only the shipped file may set.
    Reworded {
        ignored: Vec<String>,
    },
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub file: String,
    pub name: String,
    pub comment: Option<String>,
    pub icon: Option<String>,
    pub action: Action,
    pub argv: Vec<String>,
    pub terminal: bool,
    pub save_on_exit: bool,
    /// Whether the tile may run again while a copy of it still runs.
    pub multiple: bool,
    pub origin: Origin,
    /// Set when the tile cannot be used. The grid draws it disabled with
    /// this text on it.
    pub problem: Option<String>,
    /// Shown on the tile without disabling it.
    pub note: Option<String>,
}

impl Tile {
    fn broken(file: &str, origin: Origin, why: String) -> Tile {
        Tile {
            file: file.to_string(),
            name: file.trim_end_matches(".desktop").to_string(),
            comment: None,
            icon: None,
            action: Action::Run,
            argv: Vec::new(),
            terminal: false,
            save_on_exit: false,
            multiple: false,
            origin,
            problem: Some(why),
            note: None,
        }
    }

    pub fn usable(&self) -> bool {
        self.problem.is_none()
    }
}

/// One file as read from a tile directory.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub name: String,
    pub entry: Result<Entry, String>,
}

/// The files of one directory plus anything worth telling the user about
/// how it was read.
#[derive(Debug, Default)]
pub struct SourceDir {
    pub files: Vec<SourceFile>,
    pub notes: Vec<String>,
}

/// Read `*.desktop` files from `dir`, bounded in every direction: a missing
/// directory is empty, a symlink or regular file in its place is refused,
/// at most `MAX_DIR_ENTRIES` entries are looked at, at most `cap` files are
/// read, and only regular files no larger than `MAX_FILE_BYTES` are opened.
/// `label` is how the notes name the directory.
pub fn read_dir_bounded(dir: &Path, cap: usize, label: &str) -> SourceDir {
    let mut out = SourceDir::default();
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.notes.push(format!("{label} cannot be read: {e}"));
            return out;
        }
        Ok(m) if !m.file_type().is_dir() => {
            out.notes.push(format!("{label} is not a directory, so it was ignored"));
            return out;
        }
        Ok(_) => {}
    }
    let iter = match fs::read_dir(dir) {
        Ok(i) => i,
        Err(e) => {
            out.notes.push(format!("{label} cannot be read: {e}"));
            return out;
        }
    };
    let mut names = Vec::new();
    let mut scanned = 0usize;
    for entry in iter {
        scanned += 1;
        if scanned > MAX_DIR_ENTRIES {
            out.notes.push(format!(
                "{label} holds more than {MAX_DIR_ENTRIES} entries; only the first {MAX_DIR_ENTRIES} were looked at"
            ));
            break;
        }
        let Ok(entry) = entry else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.ends_with(".desktop") && !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort();
    if names.len() > cap {
        out.notes.push(format!(
            "{label} has {} tile files; only the first {cap} are shown",
            names.len()
        ));
        names.truncate(cap);
    }
    for name in names {
        let entry = read_file_bounded(&dir.join(&name)).and_then(|t| desktop::parse(&t));
        out.files.push(SourceFile { name, entry });
    }
    out
}

fn read_file_bounded(path: &Path) -> Result<String, String> {
    // Tile files never follow symlinks; see bounded::read_bounded.
    let bytes = bounded::read_bounded(path, MAX_FILE_BYTES, false)?;
    String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())
}

/// `X-TF-<suffix>`, else the old `X-DBRRG-<suffix>` that tile files in a
/// home saved before the rename may still carry. The new key wins.
fn tf_key<'a>(entry: &'a Entry, suffix: &str) -> Option<&'a str> {
    entry
        .get(&format!("X-TF-{suffix}"))
        .or_else(|| entry.get(&format!("X-DBRRG-{suffix}")))
}

fn flag(entry: &Entry, suffix: &str) -> bool {
    tf_key(entry, suffix) == Some("true")
}

fn non_empty(entry: &Entry, key: &str) -> Option<String> {
    // A tile draws each text as one line. A `\n` escape or other control
    // character becomes a space, so one file cannot paint text down over
    // the rows below.
    let v: String = entry
        .get(key)?
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let v = v.trim();
    (!v.is_empty()).then(|| v.chars().take(MAX_TEXT_CHARS).collect())
}

/// Build a tile from one parsed file. `user` restricts the action to `run`.
fn tile_from(file: &str, entry: &Entry, origin: Origin) -> Tile {
    let user = origin == Origin::User;
    let Some(name) = non_empty(entry, "Name") else {
        return Tile::broken(file, origin, format!("{file} has no Name"));
    };
    let action = match Action::parse(tf_key(entry, "Action")) {
        Ok(a) => a,
        Err(e) => return Tile::broken(file, origin, format!("{file}: {e}")),
    };
    if user && action != Action::Run {
        let named = tf_key(entry, "Action").unwrap_or_default();
        return Tile::broken(
            file,
            origin,
            format!("{file}: a tile of your own may only use the run action, not '{named}'"),
        );
    }
    let mut argv = Vec::new();
    if action == Action::Run {
        match entry.get("Exec").map(desktop::split_exec) {
            Some(Ok(a)) => argv = a,
            Some(Err(e)) => return Tile::broken(file, origin, format!("{file}: {e}")),
            None => return Tile::broken(file, origin, format!("{file} has no Exec")),
        }
    }
    Tile {
        file: file.to_string(),
        name,
        comment: non_empty(entry, "Comment"),
        icon: non_empty(entry, "Icon"),
        action,
        argv,
        terminal: action == Action::Run && entry.get("Terminal") == Some("true"),
        // Ignored unless the action is run: the other actions have no
        // program, and save-home would save twice.
        save_on_exit: action == Action::Run && flag(entry, "Save-On-Exit"),
        // Only a program can run twice; the other actions are one at a time.
        multiple: action == Action::Run && flag(entry, "Multiple"),
        origin,
        problem: None,
        note: None,
    }
}

/// Keys only a shipped file may set. A user file that rewords a shipped tile
/// and sets one of them still rewords it; the key is named on the tile.
const SHIPPED_ONLY_KEYS: [&str; 8] = [
    "Exec",
    "Terminal",
    "X-TF-Action",
    "X-TF-Save-On-Exit",
    "X-TF-Multiple",
    "X-DBRRG-Action",
    "X-DBRRG-Save-On-Exit",
    "X-DBRRG-Multiple",
];

fn reword(mut tile: Tile, user: &SourceFile) -> Tile {
    match &user.entry {
        Err(e) => {
            tile.note = Some(format!(
                "your {} could not be read ({e}); the shipped text is shown",
                user.name
            ));
            tile
        }
        Ok(entry) => {
            if let Some(name) = non_empty(entry, "Name") {
                tile.name = name;
            }
            if let Some(comment) = non_empty(entry, "Comment") {
                tile.comment = Some(comment);
            }
            if let Some(icon) = non_empty(entry, "Icon") {
                tile.icon = Some(icon);
            }
            let ignored: Vec<String> = SHIPPED_ONLY_KEYS
                .iter()
                .filter(|k| entry.get(k).is_some())
                .map(|k| k.to_string())
                .collect();
            tile.origin = Origin::Reworded { ignored };
            tile
        }
    }
}

/// A user file that can only have been meant to reword a shipped tile, but
/// no shipped file carries its name: it sets no Exec and no X-TF-Action
/// (or X-DBRRG-Action).
/// 80-logout.desktop was shipped until Restart and Power off replaced it, and
/// a machine whose user had renamed Log out kept that file in its home; the
/// menu then drew it as a grey tile saying "has no Exec".
fn rewords_nothing(shipped: &[SourceFile], user: &SourceFile) -> bool {
    let Ok(entry) = &user.entry else {
        return false;
    };
    entry.get("Exec").is_none() && tf_key(entry, "Action").is_none() && !shipped.iter().any(|f| f.name == user.name)
}

/// Banner lines for the user files `merge` leaves out as rewords of a tile
/// that is no longer shipped.
pub fn leftover_rewords(shipped: &[SourceFile], user: &[SourceFile]) -> Vec<String> {
    user.iter()
        .filter(|u| rewords_nothing(shipped, u))
        .map(|u| {
            format!(
                "~/.config/thinfloe/menu/{} rewords a tile that is no longer shipped and runs nothing, so it is not shown.",
                u.name
            )
        })
        .collect()
}

/// Merge the two directories. Both are sorted together by file name; a user
/// file whose name equals a shipped one (case sensitive) rewords it. A user
/// file that rewords no shipped tile and runs nothing is left out, see
/// `leftover_rewords`.
pub fn merge(shipped: &[SourceFile], user: &[SourceFile]) -> Vec<Tile> {
    let mut tiles: Vec<Tile> = shipped
        .iter()
        .map(|f| match &f.entry {
            Ok(e) => tile_from(&f.name, e, Origin::Shipped),
            Err(e) => Tile::broken(&f.name, Origin::Shipped, format!("{}: {e}", f.name)),
        })
        .collect();
    for u in user {
        if let Some(pos) = tiles
            .iter()
            .position(|t| t.origin == Origin::Shipped && t.file == u.name)
        {
            let shipped = tiles[pos].clone();
            tiles[pos] = reword(shipped, u);
            continue;
        }
        if rewords_nothing(shipped, u) {
            continue;
        }
        tiles.push(match &u.entry {
            Ok(e) => tile_from(&u.name, e, Origin::User),
            Err(e) => Tile::broken(&u.name, Origin::User, format!("{}: {e}", u.name)),
        });
    }
    tiles.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| (a.origin == Origin::User).cmp(&(b.origin == Origin::User)))
    });
    tiles
}

/// How many tiles that come from shipped files can be used.
fn usable_shipped(tiles: &[Tile]) -> usize {
    tiles.iter().filter(|t| t.origin != Origin::User && t.usable()).count()
}

/// The tiles the grid shows, and a banner when something was left out.
#[derive(Debug)]
pub struct Grid {
    pub tiles: Vec<Tile>,
    pub banner: Vec<String>,
}

/// The fallback rule: if the user's files leave fewer usable shipped tiles
/// than the shipped set alone has, show the shipped set alone and say why.
pub fn choose(shipped_only: Vec<Tile>, merged: Vec<Tile>, mut banner: Vec<String>) -> Grid {
    if usable_shipped(&merged) < usable_shipped(&shipped_only) {
        banner.push("Your own tiles were left out because they disabled a shipped tile.".to_string());
        return Grid {
            tiles: shipped_only,
            banner,
        };
    }
    Grid { tiles: merged, banner }
}

/// Read both directories and build the grid. A panic while handling the
/// user's files falls back to the shipped set: a restored home is restored
/// again on every boot, so it must not be able to stop the grid appearing.
pub fn load(shipped_dir: &Path, user_dir: &Path) -> Grid {
    let shipped = read_dir_bounded(shipped_dir, MAX_SHIPPED_FILES, "the shipped tile directory");
    let shipped_only = merge(&shipped.files, &[]);
    let mut banner = shipped.notes.clone();
    let user = std::panic::catch_unwind(|| {
        let user = read_dir_bounded(user_dir, MAX_USER_FILES, "~/.config/thinfloe/menu");
        let merged = merge(&shipped.files, &user.files);
        let leftover = leftover_rewords(&shipped.files, &user.files);
        (merged, user.notes, leftover)
    });
    match user {
        Ok((merged, notes, leftover)) => {
            banner.extend(notes);
            banner.extend(leftover);
            choose(shipped_only, merged, banner)
        }
        Err(_) => {
            banner.push("Your own tiles could not be read; only the shipped tiles are shown.".to_string());
            Grid {
                tiles: shipped_only,
                banner,
            }
        }
    }
}

/// Find `program` the way a shell would: as given when it contains a slash,
/// else in each directory of `path`. Only executable regular files count.
pub fn find_program(program: &str, path: &OsStr) -> Option<PathBuf> {
    let executable = |p: &Path| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if program.contains('/') {
        let p = PathBuf::from(program);
        return executable(&p).then_some(p);
    }
    std::env::split_paths(path)
        .map(|d| d.join(program))
        .find(|p| executable(p))
}

/// Disable run tiles whose program is not installed. A terminal tile also
/// needs `foot`.
pub fn mark_missing_programs(tiles: &mut [Tile], path: &OsStr) {
    for t in tiles.iter_mut().filter(|t| t.usable() && t.action == Action::Run) {
        let mut needed: Vec<&str> = Vec::new();
        if t.terminal {
            needed.push("foot");
        }
        needed.push(&t.argv[0]);
        if let Some(missing) = needed.into_iter().find(|p| find_program(p, path).is_none()) {
            t.problem = Some(format!("{missing} is not installed"));
        }
    }
}

/// The argv a run tile is started with: `Terminal=true` wraps it in foot.
pub fn command_line(tile: &Tile) -> Vec<String> {
    let mut argv = Vec::new();
    if tile.terminal {
        argv.push("foot".to_string());
        argv.push("--".to_string());
    }
    argv.extend(tile.argv.iter().cloned());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::os::unix::fs::symlink;

    fn tmpdir(tag: &str) -> TestDir {
        TestDir::new("tiles", tag)
    }

    fn src(name: &str, text: &str) -> SourceFile {
        SourceFile {
            name: name.to_string(),
            entry: desktop::parse(text),
        }
    }

    fn thinlinc() -> SourceFile {
        src(
            "10-thinlinc.desktop",
            "[Desktop Entry]\nName=ThinLinc\nIcon=/opt/thinlinc/lib/tlclient/thinlinc_128.png\n\
             Exec=/opt/thinlinc/bin/tlclient\nX-TF-Save-On-Exit=true\n",
        )
    }

    fn save_home() -> SourceFile {
        src(
            "40-save-home.desktop",
            "[Desktop Entry]\nName=Back up home\nIcon=hard-drive-download\nX-TF-Action=save-home\n",
        )
    }

    #[test]
    fn sorts_both_directories_together_by_file_name() {
        let tiles = merge(
            &[thinlinc(), save_home()],
            &[src(
                "25-browser.desktop",
                "[Desktop Entry]\nName=Browser\nExec=firefox\n",
            )],
        );
        let files: Vec<&str> = tiles.iter().map(|t| t.file.as_str()).collect();
        assert_eq!(
            files,
            ["10-thinlinc.desktop", "25-browser.desktop", "40-save-home.desktop"]
        );
        assert_eq!(tiles[1].origin, Origin::User);
    }

    #[test]
    fn user_file_rewords_name_comment_icon_only() {
        let tiles = merge(
            &[thinlinc()],
            &[src(
                "10-thinlinc.desktop",
                "[Desktop Entry]\nName=Firmen-Desktop\nComment=Anmelden\nIcon=foot\n\
                 Exec=/bin/false\nTerminal=true\nX-TF-Action=logout\nX-TF-Save-On-Exit=false\nX-TF-Multiple=false\n",
            )],
        );
        assert_eq!(tiles.len(), 1);
        let t = &tiles[0];
        assert_eq!(t.name, "Firmen-Desktop");
        assert_eq!(t.comment.as_deref(), Some("Anmelden"));
        assert_eq!(t.icon.as_deref(), Some("foot"));
        assert_eq!(t.argv, ["/opt/thinlinc/bin/tlclient"]);
        assert!(!t.terminal);
        assert_eq!(t.action, Action::Run);
        assert!(t.save_on_exit);
        assert!(!t.multiple);
        assert_eq!(
            t.origin,
            Origin::Reworded {
                ignored: vec![
                    "Exec".into(),
                    "Terminal".into(),
                    "X-TF-Action".into(),
                    "X-TF-Save-On-Exit".into(),
                    "X-TF-Multiple".into()
                ]
            }
        );
        assert!(t.usable());
    }

    // The test the spec calls easy to leave out and the whole point of the
    // rule: a reword with a broken Exec still launches the shipped tlclient.
    #[test]
    fn reword_with_broken_exec_still_launches_shipped_tlclient() {
        let tiles = merge(
            &[thinlinc()],
            &[src(
                "10-thinlinc.desktop",
                "[Desktop Entry]\nName=TL\nExec=\"unterminated\n",
            )],
        );
        assert_eq!(tiles[0].argv, ["/opt/thinlinc/bin/tlclient"]);
        assert!(tiles[0].usable());
        assert_eq!(command_line(&tiles[0]), ["/opt/thinlinc/bin/tlclient"]);
    }

    #[test]
    fn unreadable_reword_keeps_shipped_text_and_says_so() {
        let tiles = merge(&[thinlinc()], &[src("10-thinlinc.desktop", "garbage\n")]);
        assert_eq!(tiles[0].name, "ThinLinc");
        assert!(tiles[0].usable());
        assert!(tiles[0].note.as_deref().unwrap().contains("could not be read"));
    }

    #[test]
    fn name_match_is_case_sensitive() {
        let tiles = merge(
            &[thinlinc()],
            &[src(
                "10-ThinLinc.desktop",
                "[Desktop Entry]\nName=Mine\nExec=tlclient\n",
            )],
        );
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].origin, Origin::User, "uppercase T sorts first");
        assert_eq!(tiles[1].name, "ThinLinc");
    }

    #[test]
    fn user_tile_may_only_run() {
        let tiles = merge(
            &[],
            &[
                src("60-x.desktop", "[Desktop Entry]\nName=X\nX-TF-Action=logout\n"),
                src("61-y.desktop", "[Desktop Entry]\nName=Y\nX-TF-Action=reboot\nExec=y\n"),
                src("62-z.desktop", "[Desktop Entry]\nName=Z\nX-TF-Action=run\nExec=z\n"),
            ],
        );
        assert!(
            tiles[0]
                .problem
                .as_deref()
                .unwrap()
                .contains("may only use the run action")
        );
        assert!(tiles[1].problem.is_some());
        assert!(tiles[2].usable());
    }

    #[test]
    fn multiple_only_on_run_tiles() {
        let tiles = merge(
            &[
                src("10-a.desktop", "[Desktop Entry]\nName=A\nExec=a\nX-TF-Multiple=true\n"),
                src("20-b.desktop", "[Desktop Entry]\nName=B\nExec=b\n"),
                src(
                    "40-save-home.desktop",
                    "[Desktop Entry]\nName=S\nX-TF-Action=save-home\nX-TF-Multiple=true\n",
                ),
            ],
            &[src(
                "70-u.desktop",
                "[Desktop Entry]\nName=U\nExec=u\nX-TF-Multiple=true\n",
            )],
        );
        assert!(tiles[0].multiple);
        assert!(!tiles[1].multiple, "default is one at a time");
        assert!(!tiles[2].multiple, "not on save-home");
        assert!(tiles[3].multiple, "a user's own run tile may set it");
    }

    #[test]
    fn the_shipped_program_tiles_allow_several_copies() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/menu");
        let read = |f: &str| {
            let text = std::fs::read_to_string(dir.join(f)).unwrap();
            merge(&[src(f, &text)], &[]).remove(0)
        };
        for f in ["10-thinlinc.desktop", "20-oxulnk.desktop", "30-terminal.desktop"] {
            assert!(read(f).multiple, "{f}");
        }
        for f in [
            "40-save-home.desktop",
            "50-upgrade-image.desktop",
            "80-reboot.desktop",
            "81-poweroff.desktop",
        ] {
            assert!(!read(f).multiple, "{f}");
        }
    }

    #[test]
    fn save_on_exit_ignored_outside_run() {
        let tiles = merge(
            &[src(
                "40-save-home.desktop",
                "[Desktop Entry]\nName=B\nX-TF-Action=save-home\nX-TF-Save-On-Exit=true\nTerminal=true\n",
            )],
            &[src(
                "70-u.desktop",
                "[Desktop Entry]\nName=U\nExec=u\nX-TF-Save-On-Exit=true\n",
            )],
        );
        assert!(!tiles[0].save_on_exit);
        assert!(!tiles[0].terminal);
        assert!(tiles[1].save_on_exit, "valid on a user run tile");
    }

    #[test]
    fn the_ending_actions_and_their_exit_codes() {
        assert_eq!(Action::parse(Some("reboot")), Ok(Action::Reboot));
        assert_eq!(Action::parse(Some("poweroff")), Ok(Action::Poweroff));
        assert_eq!(Action::Logout.exit_code(), Some(0));
        assert_eq!(Action::Reboot.exit_code(), Some(10));
        assert_eq!(Action::Poweroff.exit_code(), Some(11));
        assert_eq!(Action::Run.exit_code(), None);
        assert_eq!(Action::SaveHome.exit_code(), None);
    }

    #[test]
    fn one_malformed_file_disables_one_tile() {
        let tiles = merge(
            &[thinlinc()],
            &[
                src("60-bad.desktop", "[Desktop Entry]\nName=Bad\nnot a line\n"),
                src("61-good.desktop", "[Desktop Entry]\nName=Good\nExec=good\n"),
            ],
        );
        assert_eq!(tiles.len(), 3);
        assert!(tiles[0].usable());
        assert_eq!(
            tiles[1].problem.as_deref(),
            Some("60-bad.desktop: line 3 is not a desktop entry line")
        );
        assert!(tiles[2].usable());
    }

    fn reboot() -> SourceFile {
        src(
            "80-reboot.desktop",
            "[Desktop Entry]\nName=Restart\nIcon=rotate-ccw\nX-TF-Action=reboot\n",
        )
    }

    /// What a machine may carry from when 80-logout.desktop was shipped: a
    /// file that only reworded it.
    fn old_logout_reword() -> SourceFile {
        src(
            "80-logout.desktop",
            "[Desktop Entry]\nName=Abmelden\nComment=Sitzung beenden\nIcon=log-out\n",
        )
    }

    #[test]
    fn a_reword_of_a_tile_no_longer_shipped_is_not_drawn() {
        let tiles = merge(&[thinlinc(), reboot()], &[old_logout_reword()]);
        let files: Vec<&str> = tiles.iter().map(|t| t.file.as_str()).collect();
        assert_eq!(files, ["10-thinlinc.desktop", "80-reboot.desktop"]);
        assert!(tiles.iter().all(|t| t.usable()));
    }

    #[test]
    fn a_reword_of_a_tile_no_longer_shipped_is_named_in_the_banner() {
        let shipped = tmpdir("orphan-shipped");
        let user = tmpdir("orphan-user");
        fs::write(
            shipped.join("80-reboot.desktop"),
            "[Desktop Entry]\nName=Restart\nX-TF-Action=reboot\n",
        )
        .unwrap();
        fs::write(
            user.join("80-logout.desktop"),
            "[Desktop Entry]\nName=Abmelden\nIcon=log-out\n",
        )
        .unwrap();
        let grid = load(&shipped, &user);
        assert_eq!(grid.tiles.len(), 1);
        assert_eq!(grid.banner.len(), 1, "{:?}", grid.banner);
        assert!(grid.banner[0].contains("80-logout.desktop"), "{:?}", grid.banner);
    }

    #[test]
    fn a_user_file_without_exec_that_names_an_action_is_still_drawn_broken() {
        // Not a leftover reword: the file asks for something, so the grey
        // tile says why it cannot have it.
        let tiles = merge(
            &[],
            &[src("60-x.desktop", "[Desktop Entry]\nName=X\nX-TF-Action=logout\n")],
        );
        assert_eq!(tiles.len(), 1);
        assert!(tiles[0].problem.is_some());
    }

    #[test]
    fn a_user_run_tile_with_a_broken_exec_is_still_drawn_broken() {
        let tiles = merge(&[], &[src("60-x.desktop", "[Desktop Entry]\nName=X\nExec=\"x\n")]);
        assert_eq!(tiles.len(), 1);
        assert!(tiles[0].problem.is_some());
    }

    #[test]
    fn old_keys_are_read() {
        let tiles = merge(
            &[src(
                "40-save-home.desktop",
                "[Desktop Entry]\nName=B\nX-DBRRG-Action=save-home\n",
            )],
            &[src(
                "70-u.desktop",
                "[Desktop Entry]\nName=U\nExec=u\nX-DBRRG-Multiple=true\n",
            )],
        );
        assert_eq!(tiles[0].action, Action::SaveHome);
        assert!(tiles[1].multiple);
    }

    #[test]
    fn new_key_wins_over_old() {
        let tiles = merge(
            &[],
            &[src(
                "70-u.desktop",
                "[Desktop Entry]\nName=U\nExec=u\nX-TF-Multiple=false\nX-DBRRG-Multiple=true\n",
            )],
        );
        assert!(!tiles[0].multiple);
    }

    #[test]
    fn old_action_key_still_limited_for_user_files() {
        let tiles = merge(
            &[],
            &[src(
                "60-x.desktop",
                "[Desktop Entry]\nName=X\nX-DBRRG-Action=reboot\nExec=y\n",
            )],
        );
        assert_eq!(
            tiles[0].problem.as_deref(),
            Some("60-x.desktop: a tile of your own may only use the run action, not 'reboot'")
        );
    }

    #[test]
    fn reword_names_old_shipped_only_keys() {
        let tiles = merge(
            &[thinlinc()],
            &[src(
                "10-thinlinc.desktop",
                "[Desktop Entry]\nName=TL\nX-DBRRG-Multiple=true\n",
            )],
        );
        match &tiles[0].origin {
            Origin::Reworded { ignored } => assert!(ignored.contains(&"X-DBRRG-Multiple".to_string())),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn old_action_key_counts_for_the_runs_nothing_rule() {
        let tiles = merge(
            &[],
            &[src("60-x.desktop", "[Desktop Entry]\nName=X\nX-DBRRG-Action=run\n")],
        );
        assert_eq!(tiles.len(), 1);
        assert!(tiles[0].problem.as_deref().unwrap().contains("has no Exec"));
    }

    #[test]
    fn upgrade_tile_keeps_its_file_name() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/menu");
        let text = std::fs::read_to_string(dir.join("50-upgrade-image.desktop")).unwrap();
        let shipped = src("50-upgrade-image.desktop", &text);
        let tiles = merge(
            std::slice::from_ref(&shipped),
            &[src("50-upgrade-image.desktop", "[Desktop Entry]\nName=Neu\n")],
        );
        assert!(
            tiles[0].argv.contains(&"thinfloe-upgrade".to_string()),
            "{:?}",
            tiles[0].argv
        );
        assert_eq!(tiles[0].name, "Neu");
    }

    #[test]
    fn falls_back_to_shipped_set_when_merge_loses_a_shipped_tile() {
        let shipped_only = merge(&[thinlinc()], &[]);
        let mut merged = shipped_only.clone();
        merged[0].problem = Some("broken".into());
        let grid = choose(shipped_only, merged, Vec::new());
        assert!(grid.tiles[0].usable());
        assert_eq!(grid.banner.len(), 1);
    }

    #[test]
    fn reads_at_most_32_user_files_in_name_order() {
        let d = tmpdir("cap");
        for i in 0..40 {
            fs::write(
                d.join(format!("{i:02}.desktop")),
                format!("[Desktop Entry]\nName=T{i}\nExec=x\n"),
            )
            .unwrap();
        }
        let got = read_dir_bounded(&d, MAX_USER_FILES, "~/.config/thinfloe/menu");
        assert_eq!(got.files.len(), 32);
        assert_eq!(got.files[0].name, "00.desktop");
        assert_eq!(got.files[31].name, "31.desktop");
        assert!(got.notes[0].contains("only the first 32"));
    }

    #[test]
    fn refuses_symlinked_or_file_directory() {
        let d = tmpdir("dirkind");
        let real = d.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("a.desktop"), "[Desktop Entry]\nName=A\nExec=a\n").unwrap();
        symlink(&real, d.join("link")).unwrap();
        fs::write(d.join("file"), "x").unwrap();
        let link = read_dir_bounded(&d.join("link"), 32, "L");
        assert!(link.files.is_empty());
        assert_eq!(link.notes, ["L is not a directory, so it was ignored"]);
        assert!(read_dir_bounded(&d.join("file"), 32, "F").files.is_empty());
        let missing = read_dir_bounded(&d.join("nope"), 32, "N");
        assert!(missing.files.is_empty() && missing.notes.is_empty());
    }

    #[test]
    fn refuses_fifo_symlink_and_oversized_files() {
        let d = tmpdir("files");
        symlink("/dev/zero", d.join("a.desktop")).unwrap();
        let st = std::process::Command::new("mkfifo")
            .arg(d.join("b.desktop"))
            .status()
            .unwrap();
        assert!(st.success());
        fs::write(d.join("c.desktop"), vec![b'x'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        fs::write(d.join("d.desktop"), [0xff, 0xfe]).unwrap();
        let got = read_dir_bounded(&d, 32, "D");
        let errs: Vec<String> = got.files.iter().map(|f| f.entry.clone().unwrap_err()).collect();
        assert_eq!(
            errs,
            [
                "not a regular file",
                "not a regular file",
                "larger than 64 KiB",
                "not UTF-8 text"
            ]
        );
    }

    #[test]
    fn huge_directory_is_bounded() {
        let d = tmpdir("huge");
        for i in 0..(MAX_DIR_ENTRIES + 10) {
            fs::write(d.join(format!("f{i}")), "").unwrap();
        }
        let got = read_dir_bounded(&d, 32, "H");
        assert!(got.files.is_empty());
        assert!(got.notes[0].contains("more than 4096 entries"));
    }

    #[test]
    fn missing_program_disables_tile() {
        let d = tmpdir("path");
        let bin = d.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("present"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(bin.join("present"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(bin.join("noexec"), "").unwrap();
        let mut tiles = merge(
            &[],
            &[
                src("1.desktop", "[Desktop Entry]\nName=P\nExec=present\n"),
                src("2.desktop", "[Desktop Entry]\nName=Q\nExec=absent --x\n"),
                src("3.desktop", "[Desktop Entry]\nName=R\nExec=noexec\n"),
                src("4.desktop", "[Desktop Entry]\nName=S\nExec=present\nTerminal=true\n"),
            ],
        );
        mark_missing_programs(&mut tiles, bin.as_os_str());
        assert!(tiles[0].usable());
        assert_eq!(tiles[1].problem.as_deref(), Some("absent is not installed"));
        assert_eq!(tiles[2].problem.as_deref(), Some("noexec is not installed"));
        assert_eq!(tiles[3].problem.as_deref(), Some("foot is not installed"));
    }

    #[test]
    fn overlong_text_is_cut() {
        let long = "x".repeat(60_000);
        let tiles = merge(
            &[],
            &[src(
                "1.desktop",
                &format!("[Desktop Entry]\nName={long}\nComment={long}\nExec=x\n"),
            )],
        );
        assert_eq!(tiles[0].name.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(tiles[0].comment.as_ref().unwrap().chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn text_is_one_line() {
        let tiles = merge(
            &[],
            &[
                src(
                    "1.desktop",
                    "[Desktop Entry]\nName=a\\nb\tc\nComment=\\n\\n\\nlow\nExec=x\n",
                ),
                src("2.desktop", "[Desktop Entry]\nName=\\n\\n\\n\nExec=x\n"),
            ],
        );
        assert_eq!(tiles[0].name, "a b c");
        assert_eq!(tiles[0].comment.as_deref(), Some("low"));
        assert_eq!(tiles[1].problem.as_deref(), Some("2.desktop has no Name"));
    }

    #[test]
    fn terminal_tile_runs_in_foot() {
        let tiles = merge(
            &[src(
                "50-u.desktop",
                "[Desktop Entry]\nName=U\nExec=sudo thinfloe-upgrade\nTerminal=true\n",
            )],
            &[],
        );
        assert_eq!(command_line(&tiles[0]), ["foot", "--", "sudo", "thinfloe-upgrade"]);
    }
}
