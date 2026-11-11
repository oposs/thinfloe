//! One bounded file reader for everything the menu loads from a place the
//! user can write to: tile files, icons and the status layout.

use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Read at most `max_bytes` from a regular file. `follow_symlinks` says
/// whether a symlink to a regular file counts: tile files must not (a
/// symlink to /dev/zero named x.desktop), icons may (icon themes are
/// full of symlinks).
pub fn read_bounded(path: &Path, max_bytes: u64, follow_symlinks: bool) -> Result<Vec<u8>, String> {
    // The metadata call refuses early, with no open at all; `open_regular`
    // checks again on the descriptor, since the path can change in between.
    let meta = if follow_symlinks {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    }
    .map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > max_bytes {
        return Err(too_large(max_bytes));
    }
    // The length above can be stale by the time the file is read, so the
    // read is capped too: one byte past the limit proves it was exceeded.
    let file = open_regular(path, follow_symlinks)?;
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(bytes)
}

/// Open `path` for reading and refuse anything but a regular file, judged
/// on the open descriptor. The metadata check in `read_bounded` runs on the
/// path, so a FIFO put in its place before the open would still be opened,
/// and a plain open of a FIFO blocks until a writer comes: on the main
/// thread at start, a black session. `O_NONBLOCK` makes that open return at
/// once; `O_NOFOLLOW` keeps a symlink swapped in from being followed when
/// symlinks do not count. `O_NONBLOCK` stays set for the read: Linux ignores
/// it for regular files, and nothing else gets that far.
fn open_regular(path: &Path, follow_symlinks: bool) -> Result<fs::File, String> {
    let mut flags = libc::O_NONBLOCK;
    if !follow_symlinks {
        flags |= libc::O_NOFOLLOW;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            // O_NOFOLLOW on a symlink.
            Some(libc::ELOOP) if !follow_symlinks => "not a regular file".to_string(),
            _ => e.to_string(),
        })?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() {
        return Err("not a regular file".to_string());
    }
    Ok(file)
}

/// A file of the system as text, `None` when it cannot be read or is larger
/// than `max_bytes`. Symlinks count (sysfs is full of them); bytes that are
/// not UTF-8 are replaced.
pub fn read_text(path: &Path, max_bytes: u64) -> Option<String> {
    read_bounded(path, max_bytes, true)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The error `read_bounded` gives for a file over `max_bytes`.
pub fn too_large(max_bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if max_bytes < MIB {
        format!("larger than {} KiB", max_bytes / 1024)
    } else {
        format!("larger than {} MiB", max_bytes / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::os::unix::fs::symlink;

    fn tmpdir(tag: &str) -> TestDir {
        TestDir::new("bounded", tag)
    }

    #[test]
    fn reads_a_regular_file_within_the_limit() {
        let d = tmpdir("ok");
        fs::write(d.join("a"), b"hello").unwrap();
        assert_eq!(read_bounded(&d.join("a"), 5, false).unwrap(), b"hello");
    }

    #[test]
    fn refuses_a_file_over_the_limit() {
        let d = tmpdir("big");
        fs::write(d.join("a"), vec![b'x'; 2049]).unwrap();
        assert_eq!(
            read_bounded(&d.join("a"), 2048, false).unwrap_err(),
            "larger than 2 KiB"
        );
        assert_eq!(
            read_bounded(&d.join("a"), 1024 * 1024, false).map(|b| b.len()),
            Ok(2049)
        );
        fs::write(d.join("b"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert_eq!(
            read_bounded(&d.join("b"), 1024 * 1024, false).unwrap_err(),
            "larger than 1 MiB"
        );
    }

    #[test]
    fn text_is_lossy_and_bounded_and_follows_symlinks() {
        let d = tmpdir("text");
        fs::write(d.join("a"), b"x\xffy").unwrap();
        symlink(d.join("a"), d.join("link")).unwrap();
        assert_eq!(read_text(&d.join("link"), 10).as_deref(), Some("x\u{FFFD}y"));
        assert_eq!(read_text(&d.join("a"), 2), None);
        assert_eq!(read_text(&d.join("missing"), 10), None);
    }

    #[test]
    fn symlink_refused_unless_following() {
        let d = tmpdir("link");
        fs::write(d.join("real"), b"x").unwrap();
        symlink(d.join("real"), d.join("link")).unwrap();
        assert_eq!(
            read_bounded(&d.join("link"), 10, false).unwrap_err(),
            "not a regular file"
        );
        assert_eq!(read_bounded(&d.join("link"), 10, true).unwrap(), b"x");
    }

    /// The type check before the open is not enough: a FIFO put in place
    /// between the two blocked the open until a writer came. The open itself
    /// must refuse it at once, with no check before it.
    #[test]
    fn the_open_itself_does_not_block_on_a_fifo() {
        let d = tmpdir("fifo-open");
        let st = std::process::Command::new("mkfifo")
            .arg(d.join("fifo"))
            .status()
            .unwrap();
        assert!(st.success());
        for follow in [false, true] {
            let path = d.join("fifo");
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(open_regular(&path, follow).map(|_| ()));
            });
            let got = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("opening a FIFO blocked");
            assert_eq!(got.unwrap_err(), "not a regular file");
        }
    }

    #[test]
    fn the_open_refuses_what_the_check_refuses() {
        let d = tmpdir("open");
        fs::write(d.join("real"), b"x").unwrap();
        symlink(d.join("real"), d.join("link")).unwrap();
        fs::create_dir(d.join("dir")).unwrap();
        assert!(open_regular(&d.join("real"), false).is_ok());
        assert!(open_regular(&d.join("link"), true).is_ok());
        assert_eq!(open_regular(&d.join("link"), false).unwrap_err(), "not a regular file");
        assert_eq!(open_regular(&d.join("dir"), true).unwrap_err(), "not a regular file");
        assert_eq!(
            open_regular(Path::new("/dev/zero"), true).unwrap_err(),
            "not a regular file"
        );
    }

    #[test]
    fn directories_and_dangling_symlinks_keep_their_errors() {
        let d = tmpdir("dir-dangling");
        fs::create_dir(d.join("dir")).unwrap();
        symlink(d.join("gone"), d.join("dangling")).unwrap();
        for follow in [false, true] {
            assert_eq!(
                read_bounded(&d.join("dir"), 10, follow).unwrap_err(),
                "not a regular file"
            );
        }
        assert_eq!(
            read_bounded(&d.join("dangling"), 10, false).unwrap_err(),
            "not a regular file"
        );
        let e = read_bounded(&d.join("dangling"), 10, true).unwrap_err();
        assert!(e.contains("No such file"), "{e}");
    }

    #[test]
    fn non_regular_paths_are_refused_even_when_following() {
        let d = tmpdir("special");
        symlink("/dev/zero", d.join("zero")).unwrap();
        assert_eq!(
            read_bounded(&d.join("zero"), 10, false).unwrap_err(),
            "not a regular file"
        );
        assert_eq!(
            read_bounded(&d.join("zero"), 10, true).unwrap_err(),
            "not a regular file"
        );
        let st = std::process::Command::new("mkfifo")
            .arg(d.join("fifo"))
            .status()
            .unwrap();
        assert!(st.success());
        assert_eq!(
            read_bounded(&d.join("fifo"), 10, true).unwrap_err(),
            "not a regular file"
        );
        assert!(read_bounded(&d.join("dir-missing"), 10, true).is_err());
    }
}
