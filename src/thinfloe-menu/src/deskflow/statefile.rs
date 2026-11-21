//! The Deskflow daemon's state file, read by the menu (spec, Processes):
//! the first line is the head, then one line per (name, fingerprint) pair,
//! tab separated. The parser ignores unknown trailing fields and lines it
//! cannot read, so a newer daemon never breaks an older menu.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevState {
    Pending,
    Rejected,
    Connected,
    /// The pointer is on this device.
    Active,
    Offline,
}

impl DevState {
    fn as_str(self) -> &'static str {
        match self {
            DevState::Pending => "pending",
            DevState::Rejected => "rejected",
            DevState::Connected => "connected",
            DevState::Active => "active",
            DevState::Offline => "offline",
        }
    }

    fn parse(s: &str) -> Option<DevState> {
        Some(match s {
            "pending" => DevState::Pending,
            "rejected" => DevState::Rejected,
            "connected" => DevState::Connected,
            "active" => DevState::Active,
            "offline" => DevState::Offline,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevLine {
    pub name: String,
    pub fingerprint: String,
    pub state: DevState,
    /// Epoch seconds of the last state change.
    pub since: i64,
    /// From the device's last DINF; 0 when unknown.
    pub width: u32,
    pub height: u32,
    /// So far only "fp-changed".
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    SelfInfo { fingerprint: String, listen: String },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFile {
    pub head: Head,
    pub devices: Vec<DevLine>,
}

/// Where the daemon writes: `TF_DESKFLOW_STATE`, else
/// `$XDG_RUNTIME_DIR/thinfloe-deskflow.state`; an empty variable is unset.
pub fn path() -> Option<PathBuf> {
    path_from(
        std::env::var_os("TF_DESKFLOW_STATE"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

fn path_from(file: Option<OsString>, runtime_dir: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = file.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-deskflow.state"))
}

/// Escape `s` so it holds no TAB, newline or other control byte: a
/// backslash, TAB and LF become `\\`, `\t`, `\n`, any other byte below 0x20 and
/// 0x7f become `\xNN`. Works on the bytes, so it does not rely on any
/// earlier sanitising.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    // Only ASCII bytes are rewritten, and a byte of a multi-byte character
    // is never ASCII, so walking the chars equals walking the bytes.
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\0'..='\x1f' | '\x7f' => out.push_str(&format!("\\x{:02x}", c as u8)),
            _ => out.push(c),
        }
    }
    out
}

/// The inverse of `escape`, lossy: bytes that are no UTF-8 become U+FFFD,
/// an escape it does not know stays as written.
pub(crate) fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match b[i + 1] {
            b'\\' => out.push(b'\\'),
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'x' if i + 3 < b.len() && hex(b[i + 2]).is_some() && hex(b[i + 3]).is_some() => {
                out.push(hex(b[i + 2]).unwrap_or(0) * 16 + hex(b[i + 3]).unwrap_or(0));
                i += 4;
                continue;
            }
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub fn render(s: &StateFile) -> String {
    let mut out = match &s.head {
        Head::SelfInfo { fingerprint, listen } => {
            format!("self\t{}\t{}\n", escape(fingerprint), escape(listen))
        }
        Head::Error(text) => format!("error\t{}\n", escape(text)),
    };
    for d in &s.devices {
        let flags: Vec<String> = d.flags.iter().map(|f| escape(f).replace(',', "\\x2c")).collect();
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            escape(&d.name),
            escape(&d.fingerprint),
            d.state.as_str(),
            d.since,
            d.width,
            d.height,
            flags.join(",")
        ));
    }
    out
}

/// Read a state file. An unreadable head gives an empty error head; a
/// device line that does not parse is skipped.
pub fn parse(text: &str) -> StateFile {
    let mut lines = text.lines();
    let head = lines
        .next()
        .and_then(parse_head)
        .unwrap_or_else(|| Head::Error(String::new()));
    let devices = lines.filter_map(parse_device).collect();
    StateFile { head, devices }
}

fn parse_head(line: &str) -> Option<Head> {
    let f: Vec<&str> = line.split('\t').collect();
    match f.as_slice() {
        ["self", fp, listen, ..] => Some(Head::SelfInfo {
            fingerprint: unescape(fp),
            listen: unescape(listen),
        }),
        ["error", text, ..] => Some(Head::Error(unescape(text))),
        _ => None,
    }
}

fn parse_device(line: &str) -> Option<DevLine> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    Some(DevLine {
        name: unescape(f[0]),
        fingerprint: unescape(f[1]),
        state: DevState::parse(f[2])?,
        since: f[3].parse().ok()?,
        width: f[4].parse().ok()?,
        height: f[5].parse().ok()?,
        flags: f[6].split(',').filter(|s| !s.is_empty()).map(unescape).collect(),
    })
}

/// Write the file atomically, mode 0600.
pub fn write(path: &Path, s: &StateFile) -> io::Result<()> {
    crate::atomic::write(path, render(s).as_bytes(), 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, fp: &str, state: DevState) -> DevLine {
        DevLine {
            name: name.into(),
            fingerprint: fp.into(),
            state,
            since: 1_700_000_000,
            width: 1920,
            height: 1080,
            flags: vec![],
        }
    }

    fn head() -> Head {
        Head::SelfInfo {
            fingerprint: "v2:sha256:ab".into(),
            listen: "0.0.0.0:24800".into(),
        }
    }

    #[test]
    fn render_parse_roundtrip() {
        let mut changed = dev("mac", "v2:sha256:02", DevState::Pending);
        changed.flags = vec!["fp-changed".into()];
        let s = StateFile {
            head: head(),
            devices: vec![
                dev("laptop", "v2:sha256:01", DevState::Active),
                changed,
                dev("ünï ☃", "v2:sha256:03", DevState::Offline),
            ],
        };
        let text = render(&s);
        assert!(text.starts_with("self\tv2:sha256:ab\t0.0.0.0:24800\n"));
        assert!(text.contains("laptop\tv2:sha256:01\tactive\t1700000000\t1920\t1080\t\n"));
        assert!(text.contains("\tpending\t1700000000\t1920\t1080\tfp-changed\n"));
        assert_eq!(parse(&text), s);
    }

    #[test]
    fn state_line_escapes_hostile_names() {
        let name = "a\tb\nc\x01d\\e\x7f\r";
        let s = StateFile {
            head: head(),
            devices: vec![dev(name, "v2:sha256:01", DevState::Connected)],
        };
        let text = render(&s);
        assert_eq!(text.lines().count(), 2, "one line per device: {text:?}");
        assert!(text.contains("a\\tb\\nc\\x01d\\\\e\\x7f\\x0d\t"));
        assert_eq!(parse(&text), s);
        // A hand-made line with bytes that are no UTF-8 stays one device.
        let raw = "self\tfp\t:1\nbad\\xff\\xfename\tfp\tpending\t1\t0\t0\t\n";
        let p = parse(raw);
        assert_eq!(p.devices.len(), 1);
        assert_eq!(p.devices[0].name, "bad\u{fffd}\u{fffd}name");
        assert_eq!(render(&p).lines().count(), 2);
    }

    #[test]
    fn parse_ignores_extra_fields() {
        let text = "self\tfp\t:24800\textra\nn\tf\tactive\t5\t10\t20\tfp-changed\tfuture\tmore\n\
                    garbage line\nn2\tf\tbogus\t5\t1\t1\t\nn3\tf\tactive\tx\t1\t1\t\n";
        let p = parse(text);
        assert_eq!(
            p.head,
            Head::SelfInfo {
                fingerprint: "fp".into(),
                listen: ":24800".into()
            }
        );
        assert_eq!(p.devices.len(), 1);
        assert_eq!(p.devices[0].flags, vec!["fp-changed".to_string()]);
        assert_eq!(p.devices[0].width, 10);
    }

    #[test]
    fn same_name_two_lines() {
        let mut changed = dev("laptop", "v2:sha256:bb", DevState::Pending);
        changed.flags = vec!["fp-changed".into()];
        let s = StateFile {
            head: head(),
            devices: vec![dev("laptop", "v2:sha256:aa", DevState::Connected), changed],
        };
        let p = parse(&render(&s));
        assert_eq!(p.devices.len(), 2);
        assert_eq!(p, s);
    }

    #[test]
    fn error_head() {
        let s = StateFile {
            head: Head::Error("cannot bind\t0.0.0.0:24800\nin use".into()),
            devices: vec![],
        };
        let text = render(&s);
        assert_eq!(text.lines().count(), 1);
        assert_eq!(parse(&text), s);
        assert_eq!(parse("").head, Head::Error(String::new()));
    }

    #[test]
    fn write_is_atomic_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::testdir::TestDir::new("statefile", "w");
        let p = d.join("state");
        let s = StateFile {
            head: head(),
            devices: vec![dev("a", "f", DevState::Rejected)],
        };
        write(&p, &s).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(parse(&std::fs::read_to_string(&p).unwrap()), s);
    }

    #[test]
    fn path_prefers_the_override() {
        let o = |s: &str| Some(OsString::from(s));
        assert_eq!(path_from(o("/x/s"), o("/run")), Some(PathBuf::from("/x/s")));
        assert_eq!(
            path_from(o(""), o("/run")),
            Some(PathBuf::from("/run/thinfloe-deskflow.state"))
        );
        assert_eq!(path_from(None, None), None);
    }
}
