//! One bounded file reader for everything the menu loads from a place the
//! user can write to: tile files and icons.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Read at most `max_bytes` from a regular file. `follow_symlinks` says
/// whether a symlink to a regular file counts: tile files must not (a
/// symlink to /dev/zero named x.desktop), icons may (icon themes are
/// full of symlinks).
pub fn read_bounded(path: &Path, max_bytes: u64, follow_symlinks: bool) -> Result<Vec<u8>, String> {
    // The metadata call decides before anything opens the path: opening a
    // FIFO blocks, and a symlink to /dev/zero never ends.
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
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(max_bytes + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(bytes)
}

fn too_large(max_bytes: u64) -> String {
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
