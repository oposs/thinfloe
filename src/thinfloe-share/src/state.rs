//! The state file `$XDG_RUNTIME_DIR/thinfloe-share.state`, the contract with the menu.
//! First line `ready<TAB>iface<TAB>mode<TAB>channel<TAB>flags`, `starting` or `error<TAB>text`;
//! then one line per receiver: `name<TAB>folder<TAB>state<TAB>since_epoch`.
//! Readers ignore unknown trailing fields.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Starting,
    Ready {
        iface: String,
        mode: String,
        channel: u32,
        flags: Vec<String>,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevState {
    Idle,
    Sending,
    Declined,
    Failed,
}

impl DevState {
    fn word(self) -> &'static str {
        match self {
            DevState::Idle => "idle",
            DevState::Sending => "sending",
            DevState::Declined => "declined",
            DevState::Failed => "failed",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        Some(match w {
            "idle" => DevState::Idle,
            "sending" => DevState::Sending,
            "declined" => DevState::Declined,
            "failed" => DevState::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub folder: String,
    pub state: DevState,
    pub since: u64,
}

/// A field of the file: a tab or a line break inside it would split the record.
fn field(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                '_'
            } else {
                c
            }
        })
        .collect()
}

pub fn render(head: &Head, devices: &[Device]) -> String {
    let mut out = match head {
        Head::Starting => "starting\n".to_string(),
        Head::Ready {
            iface,
            mode,
            channel,
            flags,
        } => format!(
            "ready\t{}\t{}\t{}\t{}\n",
            field(iface),
            field(mode),
            channel,
            flags
                .iter()
                .map(|f| field(f).replace(',', "_"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Head::Error(t) => format!("error\t{}\n", field(t)),
    };
    for d in devices {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            field(&d.name),
            field(&d.folder),
            d.state.word(),
            d.since
        ));
    }
    out
}

/// Read the file's text back; a line that does not fit is skipped, unknown trailing fields are ignored.
pub fn parse(text: &str) -> (Head, Vec<Device>) {
    let mut lines = text.lines();
    let head = match lines.next() {
        Some(l) => {
            let f: Vec<&str> = l.split('\t').collect();
            match f[0] {
                "ready" if f.len() >= 5 => match f[3].parse() {
                    Ok(channel) => Head::Ready {
                        iface: f[1].to_string(),
                        mode: f[2].to_string(),
                        channel,
                        flags: f[4]
                            .split(',')
                            .filter(|x| !x.is_empty())
                            .map(String::from)
                            .collect(),
                    },
                    Err(_) => Head::Starting,
                },
                "error" => Head::Error(f.get(1).copied().unwrap_or("").to_string()),
                _ => Head::Starting,
            }
        }
        None => Head::Starting,
    };
    let devices = lines
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 4 {
                return None;
            }
            Some(Device {
                name: f[0].to_string(),
                folder: f[1].to_string(),
                state: DevState::from_word(f[2])?,
                since: f[3].parse().ok()?,
            })
        })
        .collect();
    (head, devices)
}

/// Write atomically (temp file, then rename), mode 0644.
pub fn write(path: &Path, head: &Head, devices: &[Device]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(render(head, devices).as_bytes())?;
    f.set_permissions(std::fs::Permissions::from_mode(0o644))?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// `$XDG_RUNTIME_DIR/thinfloe-share.state`.
pub fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-share.state"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jane() -> Device {
        Device {
            name: "Jane's iPhone".into(),
            folder: "Jane's iPhone".into(),
            state: DevState::Idle,
            since: 1764000000,
        }
    }

    #[test]
    fn render_ready_line() {
        let h = Head::Ready {
            iface: "wlx1".into(),
            mode: "exclusive".into(),
            channel: 6,
            flags: vec!["no-ble".into()],
        };
        assert_eq!(render(&h, &[]), "ready\twlx1\texclusive\t6\tno-ble\n");
        let h = Head::Ready {
            iface: "wlx1".into(),
            mode: "shared".into(),
            channel: 44,
            flags: vec![],
        };
        assert_eq!(render(&h, &[]), "ready\twlx1\tshared\t44\t\n");
        assert_eq!(render(&Head::Starting, &[]), "starting\n");
        assert_eq!(
            render(&Head::Error("no card".into()), &[]),
            "error\tno card\n"
        );
    }

    #[test]
    fn render_device_line() {
        assert_eq!(
            render(&Head::Starting, &[jane()]),
            "starting\nJane's iPhone\tJane's iPhone\tidle\t1764000000\n"
        );
    }

    #[test]
    fn parse_ignores_unknown_trailing_fields() {
        let text = "ready\twlx1\texclusive\t6\tno-ble\tfuture\nJane's iPhone\tJane's iPhone\tsending\t1764000000\textra\tmore\nbroken\nx\ty\tweird\t1\n";
        let (h, d) = parse(text);
        assert_eq!(
            h,
            Head::Ready {
                iface: "wlx1".into(),
                mode: "exclusive".into(),
                channel: 6,
                flags: vec!["no-ble".into()]
            }
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].state, DevState::Sending);
        assert_eq!(d[0].name, "Jane's iPhone");
    }

    #[test]
    fn tabs_in_names_cannot_split_lines() {
        let mut d = jane();
        d.name = "a\tb\nc".into();
        let out = render(&Head::Starting, &[d]);
        assert_eq!(out, "starting\na_b_c\tJane's iPhone\tidle\t1764000000\n");
        let (_, devs) = parse(&out);
        assert_eq!(devs[0].name, "a_b_c");
    }

    #[test]
    fn round_trip_and_atomic_write() {
        let dir = std::env::temp_dir().join(format!("tfshare-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("thinfloe-share.state");
        let h = Head::Error("x".into());
        write(&p, &h, &[jane()]).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!dir.join("thinfloe-share.state.tmp").exists());
        let (h2, d2) = parse(&std::fs::read_to_string(&p).unwrap());
        assert_eq!((h2, d2), (h, vec![jane()]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
