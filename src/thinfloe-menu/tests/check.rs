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

fn check(dir: &Dir, config_dir: &PathBuf) -> String {
    let out = std::process::Command::new(MENU)
        .arg("--check")
        .env("HOME", &dir.0)
        .env("TF_CONFIG_DIR", config_dir)
        .env("TF_MENU_SHIPPED_DIR", dir.0.join("shipped"))
        .env("TF_MENU_USER_DIR", dir.0.join("user"))
        .env("TF_STATE_DIR", dir.0.join("state"))
        .env("TF_NETWORK_DEFAULT_WIRED", dir.0.join("no-such-default-wired"))
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn listing(dir: &PathBuf) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn check_names_skipped_layout_lines() {
    let dir = Dir::new("layout");
    let cfg = dir.0.join("cfg");
    fs::create_dir_all(&cfg).unwrap();
    let text = "version = 1\n[status]\nlines = [\"clock 1x1\", \"ups 1x1\", \"clock 1x1\"]\n";
    fs::write(cfg.join("config.toml"), text).unwrap();
    let out = check(&dir, &cfg);
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with("status:")).collect();
    assert_eq!(
        lines,
        [
            format!("status: layout from {}", cfg.join("config.toml").display()).as_str(),
            "status: status line 2: unknown element 'ups' kept but not shown",
            "status: status line 3 skipped: 'clock' is already listed",
        ],
        "{out}"
    );
    // Only read: the directory holds what it held.
    assert_eq!(listing(&cfg), ["config.toml"]);
    assert_eq!(fs::read_to_string(cfg.join("config.toml")).unwrap(), text);
}

#[test]
fn check_says_when_config_toml_is_missing_and_never_makes_it() {
    let dir = Dir::new("missing");
    let cfg = dir.0.join("cfg");
    fs::create_dir_all(&cfg).unwrap();
    let out = check(&dir, &cfg);
    assert!(
        out.contains(&format!(
            "status: layout from {} (missing: the shipped layout)\n",
            cfg.join("config.toml").display()
        )),
        "{out}"
    );
    assert!(listing(&cfg).is_empty());
}

#[test]
fn check_ignores_the_old_status_file() {
    let dir = Dir::new("old-status");
    let cfg = dir.0.join("cfg");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("status"), "ups 2x1\n").unwrap();
    let out = check(&dir, &cfg);
    assert!(!out.contains("ups"), "{out}");
    assert_eq!(fs::read_to_string(cfg.join("status")).unwrap(), "ups 2x1\n");
}
