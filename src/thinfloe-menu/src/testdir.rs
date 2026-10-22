//! Fixture directories for the tests. Each run creates thousands of files;
//! without the removal on drop they piled up in $TMPDIR run after run.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A fresh, empty directory under $TMPDIR, removed with its contents when
/// dropped. The pid in the name keeps parallel runs apart.
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(module: &str, tag: &str) -> TestDir {
        let d = std::env::temp_dir().join(format!("thinfloe-menu-{module}-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        TestDir(d)
    }
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
}
