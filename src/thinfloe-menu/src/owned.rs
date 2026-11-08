//! Files the menu writes for the user. Each starts with a header line, so
//! the menu can tell its own file from one somebody edited by hand and
//! never overwrites the latter.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// First line of every file the menu owns.
pub const HEADER: &str = "# Written by the ThinFloe menu. Edits here are replaced.";

/// Config files are tiny; anything bigger is not ours to parse.
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Missing,
    Menu,
    ByHand,
}

/// Who wrote `text`, the content of a file or `None` when it is missing.
/// An empty file is by hand: the menu never leaves one behind.
pub fn owner(text: Option<&str>) -> Owner {
    match text {
        None => Owner::Missing,
        Some(t) if t.lines().next() == Some(HEADER) => Owner::Menu,
        Some(_) => Owner::ByHand,
    }
}

/// The text of `path`, or `None` when there is no such file.
pub fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let bytes = crate::bounded::read_bounded(path, MAX_BYTES, false).map_err(io::Error::other)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Replace `path` with the header and `body`. A file written by hand is
/// refused with `PermissionDenied`; see `take_over`.
pub fn write(path: &Path, body: &str) -> io::Result<()> {
    write_with(path, body, |_| false)
}

/// As `write`, but a file written by hand whose text `replaceable` accepts
/// may be replaced too (the shipped template of ~/wifi.yaml). A link is
/// never replaced: `read` refuses it.
pub fn write_with(path: &Path, body: &str, replaceable: fn(&str) -> bool) -> io::Result<()> {
    let old = read(path)?;
    if owner(old.as_deref()) == Owner::ByHand && !old.as_deref().is_some_and(replaceable) {
        return Err(by_hand(path));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Write beside the target and rename, so a crash never leaves half a file.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // A stray one, a symlink above all, goes first; create_new never
    // follows a link that appears in between.
    match fs::remove_file(&tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let written = (|| {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        writeln!(f, "{HEADER}")?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Remove a file the menu wrote. A missing file is fine; one written by
/// hand, or a link (never followed), is refused with `PermissionDenied`.
pub fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(m) if !m.file_type().is_file() => return Err(by_hand(path)),
        Ok(_) => {}
    }
    if owner(read(path)?.as_deref()) != Owner::Menu {
        return Err(by_hand(path));
    }
    fs::remove_file(path)
}

fn by_hand(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} was written by hand", home_relative(path)),
    )
}

/// Move a hand-written `path` aside to `<path>.by-hand` and return its old
/// text, so the menu can own the name. An earlier `.by-hand` is never
/// overwritten.
pub fn take_over(path: &Path) -> Result<Option<String>, String> {
    let aside = aside(path);
    if fs::symlink_metadata(&aside).is_ok() {
        return Err(format!("{} exists already", home_relative(&aside)));
    }
    let old = read(path).map_err(|e| e.to_string())?;
    if old.is_some() {
        fs::rename(path, &aside).map_err(|e| e.to_string())?;
    }
    Ok(old)
}

/// Where `take_over` keeps a hand-written `path`: `<path>.by-hand`.
pub fn aside(path: &Path) -> PathBuf {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".by-hand");
    PathBuf::from(aside)
}

/// `path` as `~/rest` when it lies under $HOME, else as it is.
pub fn home_relative(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(rest) = path.strip_prefix(home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const MENU_FILE: &str = "# Written by the ThinFloe menu. Edits here are replaced.\nidle_timeout=60\n";

    #[test]
    fn owner_of_missing_menu_and_hand_files() {
        assert_eq!(owner(None), Owner::Missing);
        assert_eq!(owner(Some(MENU_FILE)), Owner::Menu);
        assert_eq!(owner(Some("idle_timeout=60\n")), Owner::ByHand);
        assert_eq!(owner(Some("")), Owner::ByHand);
        assert_eq!(owner(Some(&format!("x\n{HEADER}\n"))), Owner::ByHand);
    }

    #[test]
    fn write_creates_missing_dirs() {
        let d = TestDir::new("owned", "mkdirs");
        let p = d.join(".config/thinfloe/screen");
        write(&p, "idle_timeout=60\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), format!("{HEADER}\nidle_timeout=60\n"));
        assert!(!d.join(".config/thinfloe/screen.tmp").exists());
    }

    #[test]
    fn write_never_follows_a_tmp_symlink() {
        let d = TestDir::new("owned", "tmp-symlink");
        let outside = d.join("outside");
        fs::write(&outside, "keep\n").unwrap();
        let p = d.join("screen");
        std::os::unix::fs::symlink(&outside, d.join("screen.tmp")).unwrap();
        write(&p, "idle_timeout=60\n").unwrap();
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep\n");
        assert_eq!(fs::read_to_string(&p).unwrap(), format!("{HEADER}\nidle_timeout=60\n"));
        assert!(!fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(d.join("screen.tmp")).is_err());
    }

    #[test]
    fn write_refuses_a_hand_file() {
        let d = TestDir::new("owned", "hand");
        let p = d.join("screen");
        fs::write(&p, "idle_timeout=60\n").unwrap();
        let e = write(&p, "idle_timeout=120\n").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&p).unwrap(), b"idle_timeout=60\n");
    }

    #[test]
    fn write_replaces_a_menu_file_edited_later() {
        let d = TestDir::new("owned", "edited");
        let p = d.join("screen");
        fs::write(&p, format!("{MENU_FILE}extra=1\n")).unwrap();
        write(&p, "idle_timeout=120\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), format!("{HEADER}\nidle_timeout=120\n"));
    }

    #[test]
    fn take_over_renames_and_returns_text() {
        let d = TestDir::new("owned", "take");
        let p = d.join("screen");
        fs::write(&p, "idle_timeout=60\n").unwrap();
        assert_eq!(take_over(&p).unwrap().as_deref(), Some("idle_timeout=60\n"));
        assert!(!p.exists());
        assert_eq!(
            fs::read_to_string(d.join("screen.by-hand")).unwrap(),
            "idle_timeout=60\n"
        );
    }

    #[test]
    fn take_over_refuses_when_by_hand_exists() {
        let d = TestDir::new("owned", "take-twice");
        let p = d.join("screen");
        fs::write(&p, "new\n").unwrap();
        fs::write(d.join("screen.by-hand"), "old\n").unwrap();
        let e = take_over(&p).unwrap_err();
        assert!(e.ends_with(".by-hand exists already"), "{e}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(d.join("screen.by-hand")).unwrap(), "old\n");
    }

    fn is_link(p: &Path) -> bool {
        fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
    }

    #[test]
    fn write_with_replaces_only_what_it_may() {
        let d = TestDir::new("owned", "write-with");
        let p = d.join("wifi.yaml");
        let template = |t: &str| t.starts_with("# template");
        fs::write(&p, "# template\n").unwrap();
        write_with(&p, "a: 1\n", template).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), format!("{HEADER}\na: 1\n"));
        write_with(&p, "a: 2\n", template).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), format!("{HEADER}\na: 2\n"));
        fs::write(&p, "hand: 1\n").unwrap();
        let e = write_with(&p, "a: 3\n", template).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&p).unwrap(), b"hand: 1\n");
        // A link is never replaced, whatever the rule says.
        let target = d.join("target");
        fs::write(&target, "# template\n").unwrap();
        fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(&target, &p).unwrap();
        assert!(write_with(&p, "a: 4\n", |_| true).is_err());
        assert!(is_link(&p));
        assert_eq!(fs::read(&target).unwrap(), b"# template\n");
    }

    #[test]
    fn remove_keeps_hand_and_link() {
        let d = TestDir::new("owned", "remove");
        let p = d.join("ethernet.yaml");
        remove(&p).unwrap();
        fs::write(&p, MENU_FILE).unwrap();
        remove(&p).unwrap();
        assert!(fs::symlink_metadata(&p).is_err());
        fs::write(&p, "hand: 1\n").unwrap();
        assert_eq!(remove(&p).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&p).unwrap(), b"hand: 1\n");
        // A link to a menu file counts as by hand: neither goes.
        fs::remove_file(&p).unwrap();
        let target = d.join("target");
        fs::write(&target, MENU_FILE).unwrap();
        std::os::unix::fs::symlink(&target, &p).unwrap();
        assert_eq!(remove(&p).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(is_link(&p));
        assert_eq!(fs::read_to_string(&target).unwrap(), MENU_FILE);
    }

    #[test]
    fn read_refuses_a_link() {
        let d = TestDir::new("owned", "read-link");
        let target = d.join("target");
        fs::write(&target, MENU_FILE).unwrap();
        let p = d.join("screen");
        std::os::unix::fs::symlink(&target, &p).unwrap();
        assert!(read(&p).is_err());
        assert_eq!(read(&target).unwrap().as_deref(), Some(MENU_FILE));
    }
}
