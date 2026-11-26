//! Notes for the on-screen display: one datagram to the OSD socket, never blocking.

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

/// How a note looks on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warn,
}

impl Tone {
    fn word(self) -> &'static str {
        match self {
            Tone::Info => "info",
            Tone::Warn => "warn",
        }
    }
}

/// The longest datagram the OSD reads (the menu's `osd::socket::DATAGRAM_MAX`); a longer
/// one arrives cut and without its newline, and the OSD drops it.
pub const DATAGRAM_MAX: usize = 1024;

/// The datagram for a note: control characters (tabs, newlines) in the text become
/// spaces, format characters (bidi overrides, zero-width spaces) are dropped, and the text
/// is cut at a char boundary so the whole datagram fits [`DATAGRAM_MAX`].
pub fn datagram(tone: Tone, text: &str) -> String {
    let head = format!("note\t{}\t", tone.word());
    let room = DATAGRAM_MAX - head.len() - 1;
    let mut clean = String::new();
    for c in text
        .chars()
        .filter(|&c| !crate::names::is_format(c))
        .map(|c| if c.is_control() { ' ' } else { c })
    {
        if clean.len() + c.len_utf8() > room {
            break;
        }
        clean.push(c);
    }
    format!("{head}{clean}\n")
}

fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TF_OSD_SOCKET") {
        return Some(PathBuf::from(p));
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(Path::new(&dir).join("thinfloe-osd.sock"))
}

/// Send one note to the OSD socket at `path`, never blocking; false when it was dropped
/// (no server, or its socket is full).
pub fn send_note(path: &Path, tone: Tone, text: &str) -> bool {
    let Ok(sock) = UnixDatagram::unbound() else {
        return false;
    };
    if sock.set_nonblocking(true).is_err() {
        return false;
    }
    sock.send_to(datagram(tone, text).as_bytes(), path).is_ok()
}

/// Show `text` on the OSD. Without a server, or when the socket is full, the note is dropped.
pub fn note(tone: Tone, text: &str) {
    if let Some(path) = socket_path() {
        send_note(&path, tone, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_datagram_format() {
        // A relative path: cargo runs the tests in the crate directory, and the absolute
        // path below target/ can be longer than the 108 bytes of sun_path. No user
        // session (XDG_RUNTIME_DIR) is needed, so CI runners pass too.
        let dir = PathBuf::from(format!("target/test-tmp/osd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("osd.sock");
        let server = UnixDatagram::bind(&path).unwrap();
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        // Only this test touches the environment.
        unsafe { std::env::set_var("TF_OSD_SOCKET", &path) };
        note(Tone::Warn, "x\ty");
        let mut buf = [0u8; 256];
        let n = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"note\twarn\tx y\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn format_characters_never_reach_the_note() {
        let d = datagram(
            Tone::Info,
            "invoice\u{202E}fdp.exe from Jane\u{200B}'s iPhone\u{7f}",
        );
        assert_eq!(d, "note\tinfo\tinvoicefdp.exe from Jane's iPhone \n");
    }

    #[test]
    fn a_long_note_is_cut_to_one_datagram() {
        let d = datagram(Tone::Warn, &"J\u{e4}ne ".repeat(400));
        assert!(d.len() <= DATAGRAM_MAX, "{}", d.len());
        assert!(d.len() > DATAGRAM_MAX - 4, "{}", d.len());
        assert!(d.starts_with("note\twarn\t"));
        assert!(d.ends_with('\n'));
        assert!(std::str::from_utf8(d.as_bytes()).is_ok());
    }

    #[test]
    fn a_note_without_an_osd_is_lost_at_once() {
        let t = std::time::Instant::now();
        assert!(!send_note(
            Path::new("target/test-tmp/no-such-osd.sock"),
            Tone::Info,
            "x"
        ));
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn no_server_drops_silently() {
        assert_eq!(datagram(Tone::Info, "a\nb"), "note\tinfo\ta b\n");
    }
}
