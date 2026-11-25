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

/// The datagram for a note: control characters (tabs, newlines) in the text become
/// spaces, format characters (bidi overrides, zero-width spaces) are dropped.
pub fn datagram(tone: Tone, text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|&c| !crate::names::is_format(c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("note\t{}\t{}\n", tone.word(), clean)
}

fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TF_OSD_SOCKET") {
        return Some(PathBuf::from(p));
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(Path::new(&dir).join("thinfloe-osd.sock"))
}

/// Show `text` on the OSD. Without a server, or when the socket is full, the note is dropped.
pub fn note(tone: Tone, text: &str) {
    let Some(path) = socket_path() else { return };
    let Ok(sock) = UnixDatagram::unbound() else {
        return;
    };
    if sock.set_nonblocking(true).is_err() {
        return;
    }
    let _ = sock.send_to(datagram(tone, text).as_bytes(), path);
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
    fn no_server_drops_silently() {
        assert_eq!(datagram(Tone::Info, "a\nb"), "note\tinfo\ta b\n");
    }
}
