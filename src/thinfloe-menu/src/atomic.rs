//! Atomic file helpers for the files the menu writes: a read that refuses
//! links, a write through a file beside it and a rename, and a removal
//! that never follows a link.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The bytes of `path` (at most `max`), or `None` when there is no such
/// file. A link or anything but a regular file is an error.
pub fn read(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    crate::bounded::read_bounded(path, max, false)
        .map(Some)
        .map_err(io::Error::other)
}

/// Write `path` anew through a file beside it and a rename, so a crash
/// never leaves half a file and a link at `path` is replaced, not followed.
/// The parent directory is created; the file gets `mode`.
pub fn write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
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
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Remove `path`. A missing file is fine; a link is removed, never followed.
pub fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn write_never_follows_a_tmp_symlink() {
        let d = TestDir::new("atomic", "tmp-symlink");
        let outside = d.join("outside");
        fs::write(&outside, "keep\n").unwrap();
        let p = d.join("screen");
        std::os::unix::fs::symlink(&outside, d.join("screen.tmp")).unwrap();
        write(&p, b"idle_timeout=60\n", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep\n");
        assert_eq!(fs::read_to_string(&p).unwrap(), "idle_timeout=60\n");
        assert!(!fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(d.join("screen.tmp")).is_err());
    }

    #[test]
    fn write_sets_the_mode() {
        let d = TestDir::new("atomic", "mode");
        let p = d.join("sub/wifi.yaml");
        write(&p, b"x", 0o600).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn read_refuses_a_link() {
        let d = TestDir::new("atomic", "read-link");
        let target = d.join("target");
        fs::write(&target, "x\n").unwrap();
        let p = d.join("screen");
        std::os::unix::fs::symlink(&target, &p).unwrap();
        assert!(read(&p, 1024).is_err());
        assert_eq!(read(&target, 1024).unwrap().as_deref(), Some(&b"x\n"[..]));
        assert_eq!(read(&d.join("none"), 1024).unwrap(), None);
    }

    #[test]
    fn remove_takes_a_link_but_not_its_target() {
        let d = TestDir::new("atomic", "remove-link");
        let target = d.join("target");
        fs::write(&target, "x\n").unwrap();
        let p = d.join("screen");
        std::os::unix::fs::symlink(&target, &p).unwrap();
        remove(&p).unwrap();
        assert!(fs::symlink_metadata(&p).is_err());
        assert!(target.exists());
        remove(&p).unwrap();
    }
}
