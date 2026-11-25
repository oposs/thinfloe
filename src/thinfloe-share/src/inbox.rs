//! The FloeDrop inbox: the size cap, moving received files from staging into the
//! sender's folder, and the notes shown about it.

use crate::names;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Folder name when a sender gives no usable name.
const SENDER_FALLBACK: &str = "iPhone";
/// File name when a received entry has no usable name.
const FILE_FALLBACK: &str = "file";

/// Accept an offer of `ask_total` bytes iff it fits in `free`.
pub fn decide(ask_total: u64, free: u64) -> bool {
    ask_total <= free
}

/// Bytes available to an unprivileged user on the file system holding `root`.
pub fn free_bytes(root: &Path) -> io::Result<u64> {
    let c = std::ffi::CString::new(root.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

fn names_in(dir: &Path) -> io::Result<Vec<String>> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir)? {
        v.push(e?.file_name().to_string_lossy().into_owned());
    }
    Ok(v)
}

/// Set 0644 on files and 0755 on directories below `p`; a symlink below a directory is removed.
fn normalise(p: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        fs::set_permissions(p, fs::Permissions::from_mode(0o755))?;
        for e in fs::read_dir(p)? {
            let child = e?.path();
            normalise(&child)?;
        }
    } else if md.is_file() {
        fs::set_permissions(p, fs::Permissions::from_mode(0o644))?;
    } else {
        // A symlink or special file is not content: drop it.
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Move the top-level entries `staged` of `staging_root` into `inbox/<safe sender>/`.
/// Names that exist already get a suffix. A staged symlink (or special file) is removed,
/// not moved, and not followed; so is every symlink inside a staged folder. Entries that
/// are not directly below `staging_root` are ignored. Returns the new paths.
pub fn place(
    staged: &[PathBuf],
    staging_root: &Path,
    inbox: &Path,
    sender: &str,
) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(inbox)?;
    let folder_name = names::safe(sender, SENDER_FALLBACK);
    let existing = names_in(inbox)?;
    let is_real_dir = |n: &str| {
        fs::symlink_metadata(inbox.join(n))
            .map(|m| m.is_dir())
            .unwrap_or(false)
    };
    // The same sender keeps one folder; any other thing of that name makes a new folder.
    let folder_name = if existing.contains(&folder_name) && is_real_dir(&folder_name) {
        folder_name
    } else {
        names::unique_dir(&existing, &folder_name)
    };
    let folder = inbox.join(&folder_name);
    fs::create_dir_all(&folder)?;
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755))?;

    let mut placed = Vec::new();
    for src in staged {
        if src.parent() != Some(staging_root) {
            continue;
        }
        let Some(file_name) = src.file_name() else {
            continue;
        };
        let md = match fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !md.is_file() && !md.is_dir() {
            let _ = fs::remove_file(src);
            continue;
        }
        let wanted = names::safe(&file_name.to_string_lossy(), FILE_FALLBACK);
        let taken = names_in(&folder)?;
        let name = if md.is_dir() {
            names::unique_dir(&taken, &wanted)
        } else {
            names::unique(&taken, &wanted)
        };
        let dest = folder.join(name);
        normalise(src)?;
        fs::rename(src, &dest)?;
        placed.push(dest);
    }
    Ok(placed)
}

/// Remove a staging directory after a failed or finished offer; a missing one is fine.
pub fn discard(staging: &Path) -> io::Result<()> {
    match fs::remove_dir_all(staging) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// OSD text after a successful receive.
pub fn received_note(files: &[PathBuf], sender: &str) -> String {
    if let [one] = files {
        let name = one
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{name} received from {sender}")
    } else {
        format!("{} files received from {sender}", files.len())
    }
}

/// OSD text when an offer does not fit.
pub fn refused_note(sender: &str) -> String {
    format!("Files from {sender} refused: the FloeDrop folder is full")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let p = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
                .join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str) -> (Dir, PathBuf, PathBuf) {
        let d = Dir::new(name);
        let staging = d.path().join("staging");
        let inbox = d.path().join("inbox");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir_all(&inbox).unwrap();
        (d, staging, inbox)
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn exactly_fits_is_accepted() {
        assert!(decide(100, 100));
    }

    #[test]
    fn one_byte_over_is_refused() {
        assert!(!decide(101, 100));
    }

    #[test]
    fn free_bytes_reads_the_file_system() {
        let (_d, staging, _) = setup("free");
        assert!(free_bytes(&staging).unwrap() > 0);
    }

    #[test]
    fn place_moves_into_the_sender_folder() {
        let (_d, staging, inbox) = setup("move");
        let f = staging.join("a.txt");
        fs::write(&f, "x").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        let out = place(std::slice::from_ref(&f), &staging, &inbox, "Jane's iPhone").unwrap();
        assert_eq!(out, vec![inbox.join("Jane's iPhone/a.txt")]);
        assert_eq!(fs::read_to_string(&out[0]).unwrap(), "x");
        assert_eq!(mode(&out[0]), 0o644);
        assert!(!f.exists());
    }

    #[test]
    fn place_uses_the_fallback_for_a_bad_sender() {
        let (_d, staging, inbox) = setup("fallback");
        let f = staging.join("a.txt");
        fs::write(&f, "x").unwrap();
        let out = place(&[f], &staging, &inbox, "..").unwrap();
        assert_eq!(out, vec![inbox.join("iPhone/a.txt")]);
    }

    #[test]
    fn place_suffixes_an_existing_name() {
        let (_d, staging, inbox) = setup("suffix");
        fs::create_dir_all(inbox.join("Jane")).unwrap();
        fs::write(inbox.join("Jane/a.txt"), "old").unwrap();
        let f = staging.join("a.txt");
        fs::write(&f, "new").unwrap();
        let out = place(&[f], &staging, &inbox, "Jane").unwrap();
        assert_eq!(out, vec![inbox.join("Jane/a (2).txt")]);
        assert_eq!(fs::read_to_string(inbox.join("Jane/a.txt")).unwrap(), "old");
    }

    #[test]
    fn place_keeps_a_folder_whole() {
        let (_d, staging, inbox) = setup("folder");
        let dir = staging.join("Trip");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/p.jpg"), "p").unwrap();
        fs::set_permissions(dir.join("sub/p.jpg"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(dir.join("sub"), fs::Permissions::from_mode(0o700)).unwrap();
        let out = place(&[dir], &staging, &inbox, "Jane").unwrap();
        assert_eq!(out, vec![inbox.join("Jane/Trip")]);
        assert!(inbox.join("Jane/Trip/sub/p.jpg").is_file());
        assert_eq!(mode(&inbox.join("Jane/Trip/sub/p.jpg")), 0o644);
        assert_eq!(mode(&inbox.join("Jane/Trip/sub")), 0o755);
        // A second folder of the same name gets the suffix at the end.
        let dir2 = staging.join("Trip");
        fs::create_dir_all(&dir2).unwrap();
        let out = place(&[dir2], &staging, &inbox, "Jane").unwrap();
        assert_eq!(out, vec![inbox.join("Jane/Trip (2)")]);
    }

    #[test]
    fn place_drops_symlinks_without_following_them() {
        let (d, staging, inbox) = setup("symlink");
        let secret = d.path().join("secret");
        fs::write(&secret, "s").unwrap();
        symlink(&secret, staging.join("link")).unwrap();
        let dir = staging.join("D");
        fs::create_dir_all(&dir).unwrap();
        symlink(&secret, dir.join("inner")).unwrap();
        fs::write(dir.join("ok"), "ok").unwrap();
        let out = place(&[staging.join("link"), dir], &staging, &inbox, "Jane").unwrap();
        assert_eq!(out, vec![inbox.join("Jane/D")]);
        assert!(fs::symlink_metadata(inbox.join("Jane/link")).is_err());
        assert!(fs::symlink_metadata(staging.join("link")).is_err());
        assert!(fs::symlink_metadata(inbox.join("Jane/D/inner")).is_err());
        assert!(inbox.join("Jane/D/ok").is_file());
        assert_eq!(mode(&secret), 0o644 & mode(&secret));
        assert_eq!(fs::read_to_string(&secret).unwrap(), "s");
    }

    #[test]
    fn a_full_disk_mid_upload_leaves_nothing() {
        let (_d, staging, inbox) = setup("discard");
        let job = staging.join("job");
        fs::create_dir_all(&job).unwrap();
        fs::write(job.join("half.bin"), "partial").unwrap();
        discard(&job).unwrap();
        assert!(!job.exists());
        assert_eq!(fs::read_dir(&inbox).unwrap().count(), 0);
        discard(&job).unwrap();
    }

    #[test]
    fn notes_for_one_and_many() {
        let one = [PathBuf::from("/x/Jane/IMG_0042.HEIC")];
        assert_eq!(
            received_note(&one, "Jane's iPhone"),
            "IMG_0042.HEIC received from Jane's iPhone"
        );
        let three = [
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ];
        assert_eq!(
            received_note(&three, "Jane's iPhone"),
            "3 files received from Jane's iPhone"
        );
        assert_eq!(
            refused_note("Jane's iPhone"),
            "Files from Jane's iPhone refused: the FloeDrop folder is full"
        );
    }
}
