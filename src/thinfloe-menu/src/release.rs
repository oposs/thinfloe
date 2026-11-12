//! The release status the tool writes to `/run/thinfloe/upgrade/status`
//! (`key=value` lines) and the comparison of version numbers.

use crate::addons::{Stamp, stamp};
use std::cmp::Ordering;
use std::io::Read;
use std::path::Path;

pub const STATUS_PATH: &str = "/run/thinfloe/upgrade/status";
pub const STATUS_BYTES: u64 = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Release {
    pub latest: Option<String>,
    pub image_bytes: Option<u64>,
    pub checked: Option<u64>,
    pub source: Option<String>,
    pub error: Option<String>,
}

/// Parse the status text. Unknown keys and lines without `=` are ignored;
/// an empty value means unknown.
pub fn parse_status(text: &str) -> Release {
    let mut r = Release::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        match k.trim() {
            "latest" => r.latest = Some(v.to_string()),
            "image_bytes" => r.image_bytes = v.parse().ok(),
            "checked" => r.checked = v.parse().ok(),
            "source" => r.source = Some(v.to_string()),
            "error" => r.error = Some(v.to_string()),
            _ => {}
        }
    }
    r
}

/// Read the status file. A missing or unusable file is `Release::default()`.
/// Only the first `STATUS_BYTES` count; a line cut by the limit is dropped.
pub fn read_status(path: &Path) -> Release {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Release::default();
    };
    if !meta.file_type().is_file() {
        return Release::default();
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(STATUS_BYTES).read_to_end(&mut bytes));
    if read.is_err() {
        return Release::default();
    }
    if bytes.len() as u64 == STATUS_BYTES
        && let Some(i) = bytes.iter().rposition(|&b| b == b'\n')
    {
        bytes.truncate(i + 1);
    }
    parse_status(&String::from_utf8_lossy(&bytes))
}

struct Version<'a> {
    core: [u64; 3],
    pre: Option<Vec<&'a str>>,
}

fn parse_version(s: &str) -> Option<Version<'_>> {
    let s = s.trim();
    let s = s.split_once('+').map_or(s, |(a, _)| a);
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let mut nums = core.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        let n = nums.next()?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = n.parse().ok()?;
    }
    if nums.next().is_some() {
        return None;
    }
    let pre = match pre {
        Some(p) => {
            let ids: Vec<&str> = p.split('.').collect();
            if ids.iter().any(|i| i.is_empty()) {
                return None;
            }
            Some(ids)
        }
        None => None,
    };
    Some(Version { core: out, pre })
}

fn cmp_pre(a: &[&str], b: &[&str]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let xn = x.bytes().all(|c| c.is_ascii_digit());
        let yn = y.bytes().all(|c| c.is_ascii_digit());
        let o = match (xn, yn) {
            (true, true) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(p), Ok(q)) => p.cmp(&q),
                _ => x.len().cmp(&y.len()).then(x.cmp(y)),
            },
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// True when `latest` is a higher version than `installed`. Either side
/// that does not parse gives false.
pub fn newer(latest: &str, installed: &str) -> bool {
    let (Some(l), Some(i)) = (parse_version(latest), parse_version(installed)) else {
        return false;
    };
    let o = l.core.cmp(&i.core).then_with(|| match (&l.pre, &i.pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => cmp_pre(a, b),
    });
    o == Ordering::Greater
}

/// The latest version when it is newer than the installed one. An unknown
/// installed version gives no marker.
pub fn update_available<'a>(r: &'a Release, installed: Option<&str>) -> Option<&'a str> {
    let latest = r.latest.as_deref()?;
    newer(latest, installed?).then_some(latest)
}

/// Decides when the status file needs reading again.
#[derive(Default)]
pub struct Watcher {
    seen: Option<Option<Stamp>>,
}

impl Watcher {
    /// True on the first tick and whenever the file changed.
    pub fn tick(&mut self, path: &Path) -> bool {
        let now = stamp(path);
        let changed = self.seen != Some(now);
        self.seen = Some(now);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;

    #[test]
    fn parses_every_field() {
        let r = parse_status("latest=4.1.0\nimage_bytes=3148873728\nchecked=1762412345\nsource=https://x\nerror=\n");
        assert_eq!(
            r,
            Release {
                latest: Some("4.1.0".into()),
                image_bytes: Some(3148873728),
                checked: Some(1762412345),
                source: Some("https://x".into()),
                error: None,
            }
        );
    }

    #[test]
    fn unknown_keys_and_junk_are_ignored() {
        let r = parse_status("installed=4.0.0\nno equals sign\nlatest=1.2.3\n");
        assert_eq!(r.latest.as_deref(), Some("1.2.3"));
        assert_eq!(r.error, None);
    }

    #[test]
    fn a_missing_file_is_default() {
        let d = TestDir::new("release", "missing");
        assert_eq!(read_status(&d.join("none")), Release::default());
    }

    #[test]
    fn a_big_file_reads_as_its_first_4096_bytes() {
        let d = TestDir::new("release", "big");
        let p = d.join("status");
        let mut text = String::from("latest=4.1.0\n");
        while text.len() < 10 * 1024 {
            text.push_str("zzz=");
            text.push_str(&"x".repeat(60));
            text.push('\n');
        }
        text.push_str("error=late\n");
        fs::write(&p, &text).unwrap();
        let r = read_status(&p);
        assert_eq!(r.latest.as_deref(), Some("4.1.0"));
        assert_eq!(r.error, None);
    }

    #[test]
    fn a_symlink_is_not_followed() {
        let d = TestDir::new("release", "link");
        fs::write(d.join("real"), "latest=9.9.9\n").unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("status")).unwrap();
        assert_eq!(read_status(&d.join("status")), Release::default());
    }

    #[test]
    fn version_order() {
        assert!(newer("4.1.0", "4.0.0"));
        assert!(newer("4.0.10", "4.0.9"));
        assert!(newer("4.0.0", "4.0.0-dev+abc"));
        assert!(newer("4.0.0-rc.10", "4.0.0-rc.9"));
        assert!(newer("4.0.0-rc.1", "4.0.0-beta"));
        assert!(newer("4.0.0-rc.1.1", "4.0.0-rc.1"));
        assert!(!newer("4.0.0-dev+abc", "4.0.0"));
        // A local build (X.Y.Z-dev+<sha>[.dirty]) is offered its release
        // and every later one, and never offered back to a release.
        assert!(newer("4.2.0", "4.2.0-dev+abc.dirty"));
        assert!(newer("4.3.0", "4.2.0-dev+abc"));
        assert!(!newer("4.2.0-dev+abc", "4.2.0"));
        assert!(!newer("4.0.0", "4.0.0"));
        assert!(!newer("4.0.0+a", "4.0.0+b"));
        assert!(!newer("4.1", "4.0.0"));
        assert!(!newer("4.1.0", ""));
        assert!(!newer("garbage", "4.0.0"));
    }

    #[test]
    fn update_marker() {
        let r = Release {
            latest: Some("4.1.0".into()),
            ..Default::default()
        };
        assert_eq!(update_available(&r, Some("4.0.0")), Some("4.1.0"));
        assert_eq!(update_available(&r, Some("4.1.0")), None);
        assert_eq!(update_available(&r, None), None);
        assert_eq!(update_available(&Release::default(), Some("4.0.0")), None);
    }

    #[test]
    fn watcher_notices_a_rewrite() {
        let d = TestDir::new("release", "watch");
        let p = d.join("status");
        fs::write(&p, "latest=1.0.0\n").unwrap();
        let mut w = Watcher::default();
        assert!(w.tick(&p));
        assert!(!w.tick(&p));
        // Another length, so the change shows without relying on mtime.
        fs::write(&p, "latest=1.0.10\n").unwrap();
        assert!(w.tick(&p));
        assert!(!w.tick(&p));
        let mut w = Watcher::default();
        assert!(w.tick(&d.join("none")));
        assert!(!w.tick(&d.join("none")));
    }
}
