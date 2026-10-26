//! The header's text: host name, version and clock.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The longest version string shown; the file is user-visible data.
const VERSION_MAX: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    pub host: String,
    /// `None` until something writes the version file.
    pub version: Option<String>,
}

/// The host name from the kernel and the version from `version_file`.
pub fn read_header(version_file: &Path) -> Header {
    Header {
        host: host_name(),
        version: read_version(version_file),
    }
}

fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into buf.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// The first line, trimmed and cut to `VERSION_MAX` characters; `None` when
/// the file is missing, unreadable or says nothing.
fn read_version(file: &Path) -> Option<String> {
    let text = std::fs::read(file).ok()?;
    let text = String::from_utf8_lossy(&text);
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    Some(line.chars().take(VERSION_MAX).collect())
}

/// `HH:MM` in local time.
pub fn clock_text(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r writes only into the struct passed to it, and a
    // zeroed tm is a valid value of it.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// How long until the clock text changes.
pub fn until_next_minute(now: SystemTime) -> Duration {
    let into = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() % 60_000) as u64;
    // A little past the boundary, so the frame reads the new minute.
    Duration::from_millis(60_000 - into + 20)
}

/// `host · version · clock`, without the version and its separator when it
/// is unknown.
pub fn header_text(h: &Header, clock: &str) -> String {
    match &h.version {
        Some(v) => format!("{} · {v} · {clock}", h.host),
        None => format!("{} · {clock}", h.host),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn the_header_leaves_out_an_unknown_version() {
        let h = Header {
            host: "thinfloe-3f9a2c".into(),
            version: None,
        };
        assert_eq!(header_text(&h, "14:32"), "thinfloe-3f9a2c · 14:32");
        let h = Header {
            version: Some("4.0.0".into()),
            ..h
        };
        assert_eq!(header_text(&h, "14:32"), "thinfloe-3f9a2c · 4.0.0 · 14:32");
    }

    #[test]
    fn a_version_file_is_read_trimmed_and_capped() {
        let dir = TestDir::new("header", "version");
        let at = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            read_version(&p)
        };
        assert_eq!(read_version(&dir.join("missing")), None);
        assert_eq!(at("empty", ""), None);
        assert_eq!(at("blank", "  \n"), None);
        assert_eq!(at("two", "4.0.0\nx\n"), Some("4.0.0".into()));
        assert_eq!(at("padded", "  4.0.1  \n"), Some("4.0.1".into()));
        assert_eq!(at("long", &"9".repeat(100)).unwrap().chars().count(), 32);
        assert_eq!(read_header(&dir.join("two")).version.as_deref(), Some("4.0.0"));
    }

    #[test]
    fn the_clock_reads_hours_and_minutes_and_waits_for_the_next_minute() {
        let c = clock_text(SystemTime::now());
        assert_eq!(c.len(), 5, "{c}");
        assert_eq!(&c[2..3], ":");
        let at = UNIX_EPOCH + Duration::from_secs(600 + 59);
        let wait = until_next_minute(at);
        assert!(
            wait > Duration::from_secs(1) / 2 && wait < Duration::from_secs(2),
            "{wait:?}"
        );
        let at = UNIX_EPOCH + Duration::from_secs(600);
        assert!(until_next_minute(at) > Duration::from_secs(59));
    }
}
