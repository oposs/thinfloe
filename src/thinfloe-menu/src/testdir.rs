//! Fixture directories for the tests. Each run creates thousands of files;
//! without the removal on drop they piled up in $TMPDIR run after run.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A fresh, empty directory under $TMPDIR, removed with its contents when
/// dropped. The pid in the name keeps parallel runs apart.
pub struct TestDir(PathBuf);

/// The longest $TMPDIR the socket tests must fit under: a worktree's
/// `.tmp` (57 bytes for `/scratch/oetiker/claude-worktrees/thinfloe-mediakeys/.tmp`)
/// and 21 more for a longer checkout path in CI.
pub const LONG_TMPDIR: usize = 78;

/// The longest socket path a test may use under `LONG_TMPDIR`. `sun_path`
/// takes 107 bytes and the NUL; the rest is a margin.
pub const SOCKET_PATH_MAX: usize = 99;

/// pid_max is 4194304 on 64-bit Linux at most.
const PID_DIGITS: usize = 7;

impl TestDir {
    pub fn new(module: &str, tag: &str) -> TestDir {
        TestDir::make(format!("thinfloe-menu-{module}-{tag}-{}", std::process::id()))
    }

    /// A directory for Unix sockets, `$TMPDIR/tf-<tag>-<pid>`: short, so a
    /// socket path in it fits `sun_path` also under a long $TMPDIR. Take
    /// the socket paths from `socket`.
    pub fn sockets(tag: &str) -> TestDir {
        TestDir::make(format!("tf-{tag}-{}", std::process::id()))
    }

    fn make(name: String) -> TestDir {
        let d = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        TestDir(d)
    }

    /// The socket `name` in this directory. Panics if the path could pass
    /// `SOCKET_PATH_MAX` with a $TMPDIR of `LONG_TMPDIR` bytes and a 7-digit
    /// pid, whatever $TMPDIR and the pid are in this run.
    pub fn socket(&self, name: &str) -> PathBuf {
        let dir = self.0.file_name().map_or(0, |n| n.len());
        let pid = std::process::id().to_string().len();
        let worst = worst_socket_len(dir - pid + PID_DIGITS, name);
        assert!(
            worst <= SOCKET_PATH_MAX,
            "socket path {}/{name} can reach {worst} bytes (at most {SOCKET_PATH_MAX}): shorten the tag or the name",
            self.0.display()
        );
        self.0.join(name)
    }
}

/// `<LONG_TMPDIR>/<dir>/<name>`, in bytes.
fn worst_socket_len(dir: usize, name: &str) -> usize {
    LONG_TMPDIR + 1 + dir + 1 + name.len()
}

impl Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_and_its_contents_go_on_drop() {
        let d = TestDir::new("testdir", "drop");
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/c"), "x").unwrap();
        let path = d.to_path_buf();
        drop(d);
        assert!(!path.exists());
    }

    #[test]
    fn a_socket_path_fits_sun_path_under_a_long_tmpdir() {
        let d = TestDir::sockets("ab");
        let s = d.socket("st");
        assert_eq!(s.parent(), Some(&*d));
        // tf-ab-1234567 is 13 bytes.
        assert_eq!(worst_socket_len(13, "st"), 78 + 1 + 13 + 1 + 2);
        assert!(worst_socket_len(13, "st") <= SOCKET_PATH_MAX);
        // A $TMPDIR longer than LONG_TMPDIR fails here, not at a bind.
        assert!(s.as_os_str().len() <= SOCKET_PATH_MAX);
    }

    #[test]
    #[should_panic(expected = "shorten the tag or the name")]
    fn a_socket_path_that_could_overflow_panics() {
        TestDir::sockets("ab").socket("a-socket-name-that-is-too-long");
    }
}
