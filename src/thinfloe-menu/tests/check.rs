//! `thinfloe-menu --check` run as an operator at a VT runs it.

use std::fs;
use std::path::PathBuf;

const MENU: &str = env!("CARGO_BIN_EXE_thinfloe-menu");

/// A fresh directory under $TMPDIR, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("thinfloe-menu-check-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        for sub in ["shipped", "user", "state"] {
            fs::create_dir_all(d.join(sub)).unwrap();
        }
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn check(dir: &Dir, status: &PathBuf) -> String {
    let out = std::process::Command::new(MENU)
        .arg("--check")
        .env("HOME", &dir.0)
        .env("TF_MENU_SHIPPED_DIR", dir.0.join("shipped"))
        .env("TF_MENU_USER_DIR", dir.0.join("user"))
        .env("TF_STATE_DIR", dir.0.join("state"))
        .env("TF_STATUS_FILE", status)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn check_names_skipped_layout_lines() {
    let dir = Dir::new("layout");
    let file = dir.0.join("status");
    fs::write(&file, "clock 1x1\nups 2x1\nclock 2x1\ncpu\n").unwrap();
    let out = check(&dir, &file);
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with("status:")).collect();
    assert_eq!(
        lines,
        [
            format!("status: layout file {}", file.display()).as_str(),
            "status: status line 2: unknown element 'ups' kept but not shown",
            "status: status line 3 skipped: 'clock' is already listed",
            "status: status line 4 skipped: expected '<id> <w>x<h>', got 'cpu'",
        ],
        "{out}"
    );
    // The file is only read.
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "clock 1x1\nups 2x1\nclock 2x1\ncpu\n"
    );
}

#[test]
fn check_says_when_the_layout_file_is_refused_or_missing() {
    let dir = Dir::new("refused");
    let file = dir.0.join("status");
    fs::write(&file, [0xff, b'\n']).unwrap();
    let out = check(&dir, &file);
    assert!(
        out.contains("status: status layout file ignored: it is not UTF-8 text; the shipped layout is used\n"),
        "{out}"
    );
    let missing = dir.0.join("none");
    let out = check(&dir, &missing);
    assert!(
        out.contains(&format!(
            "status: layout file {} (missing: the shipped layout)\n",
            missing.display()
        )),
        "{out}"
    );
}
