//! Fixture directories for the tests. Each run creates thousands of files;
//! without the removal on drop they piled up in $TMPDIR run after run.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh, empty directory under $TMPDIR, removed with its contents when
/// dropped. The pid in the name keeps parallel runs apart, a sequence
/// number the tests of one run: two tests with the same tag never share a
/// directory, so one's `make` cannot remove the other's files.
pub struct TestDir(PathBuf);

/// The next sequence number of this process.
static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A socket directory's sequence number takes this many base-36 digits.
const SEQ_DIGITS: usize = 2;

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
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        TestDir::make(format!("thinfloe-menu-{module}-{tag}-{}-{n}", std::process::id()))
    }

    /// A directory for Unix sockets, `$TMPDIR/tf-<tag>-<pid>-<seq>` with
    /// the sequence number in two base-36 digits: short, so a socket path in
    /// it fits `sun_path` also under a long $TMPDIR. Take the socket paths
    /// from `socket`.
    pub fn sockets(tag: &str) -> TestDir {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        TestDir::make(format!("tf-{tag}-{}-{}", std::process::id(), base36(n)))
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
        // `sockets` names end in `-<seq>`, always SEQ_DIGITS long.
        let worst = worst_socket_len(dir - pid + PID_DIGITS, name);
        assert!(
            worst <= SOCKET_PATH_MAX,
            "socket path {}/{name} can reach {worst} bytes (at most {SOCKET_PATH_MAX}): shorten the tag or the name",
            self.0.display()
        );
        self.0.join(name)
    }
}

/// `n` in `SEQ_DIGITS` base-36 digits. Panics past the last one: a run
/// with that many test directories needs a longer sequence.
fn base36(n: usize) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    assert!(
        n < 36usize.pow(SEQ_DIGITS as u32),
        "test directory sequence {n} needs more than {SEQ_DIGITS} digits"
    );
    (0..SEQ_DIGITS)
        .rev()
        .map(|i| DIGITS[n / 36usize.pow(i as u32) % 36] as char)
        .collect()
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
        // tf-ab-1234567-zz is 16 bytes.
        assert_eq!(worst_socket_len(16, "st"), 78 + 1 + 16 + 1 + 2);
        assert!(worst_socket_len(16, "st") <= SOCKET_PATH_MAX);
        // A $TMPDIR longer than LONG_TMPDIR fails here, not at a bind.
        assert!(s.as_os_str().len() <= SOCKET_PATH_MAX);
    }

    #[test]
    #[should_panic(expected = "shorten the tag or the name")]
    fn a_socket_path_that_could_overflow_panics() {
        TestDir::sockets("ab").socket("a-socket-name-that-is-too-long");
    }

    /// Two tests with one tag get two directories: neither's `make` or drop
    /// removes the other's files.
    #[test]
    fn the_same_tag_gives_two_directories() {
        let a = TestDir::new("testdir", "same");
        let b = TestDir::new("testdir", "same");
        assert_ne!(a.to_path_buf(), b.to_path_buf());
        fs::write(a.join("f"), "x").unwrap();
        drop(TestDir::new("testdir", "same"));
        assert!(a.join("f").exists(), "a later TestDir left the first alone");
        let (c, d) = (TestDir::sockets("sa"), TestDir::sockets("sa"));
        assert_ne!(c.to_path_buf(), d.to_path_buf());
        fs::write(c.join("f"), "x").unwrap();
        drop(d);
        assert!(c.join("f").exists());
    }

    #[test]
    fn the_sequence_is_two_base36_digits() {
        assert_eq!(base36(0), "00");
        assert_eq!(base36(35), "0z");
        assert_eq!(base36(36), "10");
        assert_eq!(base36(1295), "zz");
    }
}
