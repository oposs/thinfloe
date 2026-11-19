//! The key client's way to the OSD process: one Unix datagram per key,
//! `<action>\n`. Neither side ever blocks on the other: a full queue or a
//! missing OSD is an error the client reports at once.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

use super::Action;

#[derive(Debug)]
pub enum SendError {
    /// No OSD process listens at the path.
    NoServer,
    Io(std::io::Error),
}

/// Bind the OSD's socket at `path`: a file left by an earlier OSD is
/// removed first, the socket is mode 0600 and non-blocking.
pub fn bind(path: &Path) -> std::io::Result<UnixDatagram> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let sock = UnixDatagram::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    sock.set_nonblocking(true)?;
    Ok(sock)
}

/// Send `a` to the OSD at `path` as one datagram, without blocking.
pub fn send(path: &Path, a: Action) -> Result<(), SendError> {
    let sock = UnixDatagram::unbound().map_err(SendError::Io)?;
    sock.set_nonblocking(true).map_err(SendError::Io)?;
    let msg = format!("{}\n", a.as_str());
    match sock.send_to(msg.as_bytes(), path) {
        Ok(_) => Ok(()),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => Err(SendError::NoServer),
        Err(e) => Err(SendError::Io(e)),
    }
}

/// The action in one datagram; anything else is `None`.
pub fn decode(buf: &[u8]) -> Option<Action> {
    let s = std::str::from_utf8(buf).ok()?;
    Action::parse(s.strip_suffix('\n').unwrap_or(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn a_datagram_reaches_the_bound_socket() {
        let d = TestDir::sockets("re");
        let path = d.socket("s");
        let server = bind(&path).unwrap();
        send(&path, Action::VolumeUp).unwrap();
        let mut buf = [0u8; 64];
        let n = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"volume-up\n");
        assert_eq!(decode(&buf[..n]), Some(Action::VolumeUp));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn send_without_a_server_is_no_server() {
        let d = TestDir::sockets("no");
        let missing = d.socket("x");
        assert!(matches!(send(&missing, Action::VolumeUp), Err(SendError::NoServer)));
        // A socket file left by an OSD that is gone.
        let stale = d.socket("st");
        drop(UnixDatagram::bind(&stale).unwrap());
        assert!(stale.exists());
        assert!(matches!(send(&stale, Action::MicMute), Err(SendError::NoServer)));
    }

    #[test]
    fn bind_replaces_a_stale_socket_file() {
        let d = TestDir::sockets("st");
        let path = d.socket("s");
        drop(UnixDatagram::bind(&path).unwrap());
        let server = bind(&path).unwrap();
        send(&path, Action::BrightnessDown).unwrap();
        let mut buf = [0u8; 64];
        let n = server.recv(&mut buf).unwrap();
        assert_eq!(decode(&buf[..n]), Some(Action::BrightnessDown));
    }

    #[test]
    fn a_receive_without_a_datagram_does_not_block() {
        let d = TestDir::sockets("nb");
        let server = bind(&d.socket("s")).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(server.recv(&mut buf).unwrap_err().kind(), ErrorKind::WouldBlock);
    }

    #[test]
    fn decode_takes_one_action_name() {
        assert_eq!(decode(b"mic-mute\n"), Some(Action::MicMute));
        assert_eq!(decode(b"mic-mute"), Some(Action::MicMute));
        assert_eq!(decode(b"mic-mute\n\n"), None);
        assert_eq!(decode(b"volume"), None);
        assert_eq!(decode(&[0xff, 0xfe]), None);
    }
}
