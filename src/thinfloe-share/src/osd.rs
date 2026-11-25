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

/// The datagram for a note: tabs and newlines in the text become spaces.
pub fn datagram(tone: Tone, text: &str) -> String {
    let clean: String = text
        .chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
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
        // The socket sits under XDG_RUNTIME_DIR: a path below the crate's target
        // directory is longer than the 108 bytes of sun_path.
        let base = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is set");
        let dir = Path::new(&base).join(format!("tfshare-osd-test-{}", std::process::id()));
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
    fn no_server_drops_silently() {
        assert_eq!(datagram(Tone::Info, "a\nb"), "note\tinfo\ta b\n");
    }
}
