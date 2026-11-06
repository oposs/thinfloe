//! No code under `src/` changes the process environment. Tests run in
//! parallel threads, and `set_var`/`remove_var` race with every other
//! thread that reads the environment (libc's `localtime_r` reads `TZ`
//! unlocked). Zones and similar inputs are passed in instead.

use std::path::Path;

fn scan(dir: &Path, hits: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, hits);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.contains(concat!("set", "_var(")) || line.contains(concat!("remove", "_var(")) {
                    hits.push(format!("{}:{}", path.display(), n + 1));
                }
            }
        }
    }
}

#[test]
fn nothing_under_src_changes_the_environment() {
    let mut hits = Vec::new();
    scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
    assert!(hits.is_empty(), "environment changed at {hits:?}");
}
