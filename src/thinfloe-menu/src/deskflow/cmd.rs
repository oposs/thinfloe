//! The menu's commands to the Deskflow daemon: one Unix datagram each,
//! `reject\t<escaped name>\t<fingerprint>\n` or `reload\n`. Like the OSD's
//! socket, neither side ever blocks on the other.

use std::ffi::OsString;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use super::statefile::{escape, unescape};

/// The largest datagram the daemon reads.
pub const DATAGRAM_MAX: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Reject the pending device with this name and fingerprint.
    Reject { name: String, fingerprint: String },
    /// Read `generated/deskflow` again.
    Reload,
}

#[derive(Debug)]
pub enum SendError {
    /// No daemon listens at the path.
    NoServer,
    Io(io::Error),
}

/// Where the daemon listens: `TF_DESKFLOW_SOCKET`, else
/// `$XDG_RUNTIME_DIR/thinfloe-deskflow.sock`; an empty variable is unset.
pub fn socket_path() -> Option<PathBuf> {
    socket_path_from(
        std::env::var_os("TF_DESKFLOW_SOCKET"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

fn socket_path_from(sock: Option<OsString>, runtime_dir: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = sock.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-deskflow.sock"))
}

pub fn encode(c: &Cmd) -> Vec<u8> {
    match c {
        Cmd::Reject { name, fingerprint } => {
            format!("reject\t{}\t{}\n", escape(name), escape(fingerprint)).into_bytes()
        }
        Cmd::Reload => b"reload\n".to_vec(),
    }
}

/// The command in one datagram; anything malformed is `None`.
pub fn decode(buf: &[u8]) -> Option<Cmd> {
    if buf.len() > DATAGRAM_MAX {
        return None;
    }
    let s = std::str::from_utf8(buf).ok()?;
    let line = s.strip_suffix('\n')?;
    if line.contains('\n') {
        return None;
    }
    if line == "reload" {
        return Some(Cmd::Reload);
    }
    let mut f = line.split('\t');
    if f.next()? != "reject" {
        return None;
    }
    let (name, fp) = (f.next()?, f.next()?);
    if f.next().is_some() || fp.is_empty() {
        return None;
    }
    Some(Cmd::Reject {
        name: unescape(name),
        fingerprint: unescape(fp),
    })
}

/// Bind the daemon's socket at `path`: a file left by an earlier daemon is
/// removed first, the socket is mode 0600 and non-blocking.
pub fn bind(path: &Path) -> io::Result<UnixDatagram> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let sock = UnixDatagram::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    sock.set_nonblocking(true)?;
    Ok(sock)
}

/// Send `c` to the daemon at `path` as one datagram, without blocking.
pub fn send(path: &Path, c: &Cmd) -> Result<(), SendError> {
    let msg = encode(c);
    if msg.len() > DATAGRAM_MAX {
        return Err(SendError::Io(io::Error::new(
            ErrorKind::InvalidInput,
            "command too long",
        )));
    }
    let sock = UnixDatagram::unbound().map_err(SendError::Io)?;
    sock.set_nonblocking(true).map_err(SendError::Io)?;
    match sock.send_to(&msg, path) {
        Ok(_) => Ok(()),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => Err(SendError::NoServer),
        Err(e) => Err(SendError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn cmd_roundtrip() {
        let r = Cmd::Reject {
            name: "lap\ttop\n\\x01".into(),
            fingerprint: "v2:sha256:ab".into(),
        };
        let bytes = encode(&r);
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
        assert_eq!(decode(&bytes), Some(r));
        assert_eq!(encode(&Cmd::Reload), b"reload\n");
        assert_eq!(decode(b"reload\n"), Some(Cmd::Reload));
    }

    #[test]
    fn cmd_garbage_is_none() {
        let big = vec![b'a'; DATAGRAM_MAX + 1];
        let cases: [&[u8]; 12] = [
            b"",
            b"\n",
            b"reload",
            b"reload\n\n",
            b"reload\textra\n",
            b"reject\n",
            b"reject\tname\n",
            b"reject\tname\t\n",
            b"reject\tn\tfp\textra\n",
            b"explode\n",
            &[0xff, 0xfe, b'\n'],
            &big,
        ];
        for c in cases {
            assert_eq!(decode(c), None, "{c:?}");
        }
        assert_eq!(decode(b"reject\ta\nb\tfp\n"), None);
    }

    #[test]
    fn a_datagram_reaches_the_bound_socket() {
        let d = TestDir::sockets("df");
        let path = d.socket("s");
        let server = bind(&path).unwrap();
        let c = Cmd::Reject {
            name: "x".into(),
            fingerprint: "fp".into(),
        };
        send(&path, &c).unwrap();
        let mut buf = [0u8; DATAGRAM_MAX];
        let n = server.recv(&mut buf).unwrap();
        assert_eq!(decode(&buf[..n]), Some(c));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(server.recv(&mut buf).unwrap_err().kind(), ErrorKind::WouldBlock);
    }

    #[test]
    fn send_without_a_server_is_no_server() {
        let d = TestDir::sockets("dn");
        assert!(matches!(send(&d.socket("x"), &Cmd::Reload), Err(SendError::NoServer)));
        let stale = d.socket("st");
        drop(UnixDatagram::bind(&stale).unwrap());
        assert!(matches!(send(&stale, &Cmd::Reload), Err(SendError::NoServer)));
        let server = bind(&stale).unwrap();
        send(&stale, &Cmd::Reload).unwrap();
        drop(server);
    }

    #[test]
    fn an_oversized_command_is_refused() {
        let d = TestDir::sockets("do");
        let c = Cmd::Reject {
            name: "a".repeat(DATAGRAM_MAX),
            fingerprint: "fp".into(),
        };
        assert!(matches!(send(&d.socket("s"), &c), Err(SendError::Io(_))));
    }

    #[test]
    fn socket_path_prefers_the_override() {
        let o = |s: &str| Some(OsString::from(s));
        assert_eq!(socket_path_from(o("/x/s"), o("/run")), Some(PathBuf::from("/x/s")));
        assert_eq!(
            socket_path_from(o(""), o("/run")),
            Some(PathBuf::from("/run/thinfloe-deskflow.sock"))
        );
        assert_eq!(socket_path_from(None, None), None);
    }
}
