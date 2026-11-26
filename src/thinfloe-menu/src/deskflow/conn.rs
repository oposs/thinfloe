//! One client connection of the Deskflow daemon: a non-blocking TCP stream
//! under rustls, the frames cut from it and its protocol [`Session`] (spec,
//! Identity and pairing). The daemon passes the time in and answers the
//! client's name with a verdict.

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::{ServerConfig, ServerConnection};

use super::clipdata;
use super::pair::Verdict;
use super::proto::{self, Framer, Msg};
use super::session::{Out, Session};
use super::tls;

/// The most plaintext taken from the TLS layer at once; its frames are cut
/// before the next read, so the framer never holds much more than one
/// message.
pub const READ_CHUNK: usize = 64 * 1024;
/// TLS, Hello and HelloBack together must be done within this time.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
/// After a refusal (EUNK, EBSY, EICV) the connection stays open this long
/// for the client to read it and hang up, as a Deskflow server does: a
/// Deskflow 1.26 client that sees the end of the stream with the message
/// drops the message and does not log the refusal.
pub const LINGER: Duration = Duration::from_secs(2);
/// What TLS may hold for the client before it counts as not reading: a
/// whole clipboard transfer fits, with room for the input around it.
/// rustls's own default, 64 KiB, cut a larger clipboard short.
pub const SEND_BUFFER: usize = 2 * clipdata::CLIP_MAX;

/// What a connection tells the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The client's name: the daemon owes [`Conn::set_verdict`] in the
    /// same loop pass.
    Named(String),
    /// The size of the client's screen, from its DINF.
    Screen { w: i16, h: i16 },
    /// The client's text clipboard, received in full.
    Clipboard(Vec<u8>),
    /// The client took the clipboard.
    Grabbed,
}

pub struct Conn {
    /// The daemon's number for it, unique for the life of the process.
    id: usize,
    stream: TcpStream,
    /// The client's address, read at the accept: a reset socket no longer
    /// knows it.
    peer: Option<std::net::IpAddr>,
    tls: ServerConnection,
    framer: Framer,
    /// Made when the TLS handshake is done.
    session: Option<Session>,
    accepted: Instant,
    /// The fingerprint of the client's certificate, once TLS is done.
    fingerprint: Option<String>,
    /// The client's name, once its HelloBack is in.
    name: Option<String>,
    /// The verdict was Trusted.
    trusted: bool,
    /// The last DINF size.
    screen: Option<(i16, i16)>,
    /// The session ended it: why, and until when the client may read.
    ending: Option<(String, Instant)>,
    /// Why the connection ends; the daemon drops it after the next flush.
    closing: Option<String>,
}

impl Conn {
    pub fn new(id: usize, stream: TcpStream, cfg: Arc<ServerConfig>, now: Instant) -> io::Result<Conn> {
        stream.set_nonblocking(true)?;
        let mut tls = ServerConnection::new(cfg).map_err(io::Error::other)?;
        tls.set_buffer_limit(Some(SEND_BUFFER));
        Ok(Conn {
            id,
            peer: stream.peer_addr().ok().map(|a| a.ip()),
            stream,
            tls,
            framer: Framer::default(),
            session: None,
            accepted: now,
            fingerprint: None,
            name: None,
            trusted: false,
            screen: None,
            ending: None,
            closing: None,
        })
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    /// TLS, the HelloBack or the verdict are still to come.
    pub fn in_handshake(&self) -> bool {
        !self.trusted && self.closing.is_none()
    }

    pub fn wants_write(&self) -> bool {
        self.tls.wants_write()
    }

    pub fn fingerprint(&self) -> Option<&str> {
        self.fingerprint.as_deref()
    }

    /// The client's address, as of the accept.
    pub fn peer(&self) -> Option<std::net::IpAddr> {
        self.peer
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Trusted and not ending.
    pub fn trusted(&self) -> bool {
        self.trusted && self.ending.is_none() && self.closing.is_none()
    }

    pub fn screen(&self) -> Option<(i16, i16)> {
        self.screen
    }

    /// Why it ends, once it does.
    pub fn closing(&self) -> Option<&str> {
        self.closing.as_deref()
    }

    pub fn close(&mut self, why: &str) {
        if self.closing.is_none() {
            self.closing = Some(why.to_string());
            self.tls.send_close_notify();
        }
    }

    /// The socket is readable: one TLS read, then every frame in the
    /// plaintext, cut [`READ_CHUNK`] bytes at a time.
    pub fn on_readable(&mut self, now: Instant) -> Vec<Event> {
        if self.closing.is_some() {
            return Vec::new();
        }
        match self.tls.read_tls(&mut self.stream) {
            Ok(0) => {
                self.close("the client closed the connection");
                return Vec::new();
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => return Vec::new(),
            Err(e) => {
                self.close(&format!("read: {e}"));
                return Vec::new();
            }
        }
        if let Err(e) = self.tls.process_new_packets() {
            // rustls has queued an alert; the next flush sends it.
            self.close(&format!("TLS: {e}"));
            return Vec::new();
        }
        let mut events = Vec::new();
        if self.ending.is_some() {
            self.drain();
            return events;
        }
        if self.session.is_none() && !self.tls.is_handshaking() {
            let Some(fp) = tls::peer_fingerprint(&self.tls) else {
                self.close("no client certificate");
                return events;
            };
            self.fingerprint = Some(fp);
            // The Hello deadline counts from the accept: TLS and Hello
            // share the 10 s.
            let (session, out) = Session::new(self.accepted);
            self.session = Some(session);
            self.apply(out, now, &mut events);
        }
        let mut buf = vec![0u8; READ_CHUNK];
        while self.session.is_some() && self.ending.is_none() && self.closing.is_none() {
            let n = match self.tls.reader().read(&mut buf) {
                Ok(0) => {
                    self.close("the client ended TLS");
                    break;
                }
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    self.close(&format!("read: {e}"));
                    break;
                }
            };
            self.framer.push(&buf[..n]);
            self.cut_frames(now, &mut events);
        }
        events
    }

    /// While ending: what the client still sends is read and dropped,
    /// until it hangs up.
    fn drain(&mut self) {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match self.tls.reader().read(&mut buf) {
                Ok(0) => return self.close("the client hung up"),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) => return self.close(&format!("read: {e}")),
            }
        }
    }

    fn cut_frames(&mut self, now: Instant, events: &mut Vec<Event>) {
        while self.ending.is_none() && self.closing.is_none() {
            match self.framer.next() {
                Ok(Some(payload)) => {
                    let Some(s) = self.session.as_mut() else {
                        return;
                    };
                    let out = s.on_payload(&payload, now);
                    self.apply(out, now, events);
                }
                Ok(None) => return,
                Err(e) => {
                    self.close(&format!("bad frame: {e:?}"));
                    return;
                }
            }
        }
    }

    fn apply(&mut self, out: Vec<Out>, now: Instant, events: &mut Vec<Event>) {
        for o in out {
            match o {
                Out::Send(bytes) => self.write(&bytes),
                Out::Named { name } => {
                    self.name = Some(name.clone());
                    events.push(Event::Named(name));
                }
                Out::Screen { w, h } => {
                    self.screen = Some((w, h));
                    events.push(Event::Screen { w, h });
                }
                Out::Close(why) => self.end(why, now),
                Out::Clipboard { text } => events.push(Event::Clipboard(text)),
                Out::Grabbed => events.push(Event::Grabbed),
            }
        }
    }

    /// The session is over: the client gets [`LINGER`] to read the last
    /// message and hang up.
    fn end(&mut self, why: &str, now: Instant) {
        if self.ending.is_none() && self.closing.is_none() {
            self.ending = Some((why.to_string(), now + LINGER));
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.ending.is_some() || self.closing.is_some() {
            return;
        }
        // rustls buffers a bounded amount of plaintext; a client that does
        // not read fills it and is dropped.
        match self.tls.writer().write(bytes) {
            Ok(n) if n == bytes.len() => {}
            Ok(_) => self.close("the client does not read"),
            Err(e) => self.close(&format!("write: {e}")),
        }
    }

    /// The answer to [`Event::Named`].
    pub fn set_verdict(&mut self, v: Verdict, now: Instant) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let out = s.set_verdict(v);
        if matches!(v, Verdict::Trusted(_)) && !out.is_empty() {
            self.trusted = true;
        }
        self.apply(out, now, &mut Vec::new());
    }

    /// CINN: the pointer enters the client's screen at (x, y).
    pub fn enter(&mut self, x: i16, y: i16, mods: u16) {
        if let Some(s) = self.session.as_mut() {
            let out = s.enter(x, y, mods);
            self.apply_sends(out);
        }
    }

    /// A message for a client that is past the handshake; dropped before.
    pub fn send(&mut self, m: Msg) {
        if let Some(s) = self.session.as_mut() {
            let out = s.send(m);
            self.apply_sends(out);
        }
    }

    /// Turns clipboard sharing on or off (DSOP to an active client).
    pub fn set_clipboard(&mut self, on: bool) {
        if let Some(s) = self.session.as_mut() {
            let out = s.set_clipboard(on);
            self.apply_sends(out);
        }
    }

    /// CCLP and DCLP: the client gets ThinFloe's text.
    pub fn clipboard(&mut self, text: &[u8]) {
        if let Some(s) = self.session.as_mut() {
            let out = s.clipboard(text);
            self.apply_sends(out);
        }
    }

    /// CCLP: the client learns that someone else owns the clipboard.
    pub fn grab(&mut self) {
        if let Some(s) = self.session.as_mut() {
            let out = s.grab();
            self.apply_sends(out);
        }
    }

    /// The bytes of the session's answer (these calls make no other).
    fn apply_sends(&mut self, out: Vec<Out>) {
        for o in out {
            if let Out::Send(bytes) = o {
                self.write(&bytes);
            }
        }
    }

    /// Another connection holds this client's name: EBSY, then the end.
    pub fn refuse_busy(&mut self, now: Instant) {
        if let Some(s) = &self.session {
            let bytes = proto::frame(&proto::encode(&Msg::Ebsy, s.minor()));
            self.write(&bytes);
        }
        self.end("a client with this name is connected", now);
    }

    /// Time passes: the TLS deadline and the session's timers.
    pub fn tick(&mut self, now: Instant) {
        if self.closing.is_some() {
            return;
        }
        if let Some((why, until)) = &self.ending {
            if now >= *until {
                let why = why.clone();
                self.close(&why);
            }
            return;
        }
        match self.session.as_mut() {
            None => {
                if now.saturating_duration_since(self.accepted) >= HANDSHAKE_DEADLINE {
                    self.close("TLS handshake timed out");
                }
            }
            Some(s) => {
                let out = s.tick(now);
                self.apply(out, now, &mut Vec::new());
            }
        }
    }

    /// Writes what TLS has queued, as far as the socket takes it.
    pub fn flush(&mut self) {
        while self.tls.wants_write() {
            match self.tls.write_tls(&mut self.stream) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    self.close(&format!("write: {e}"));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deskflow::proto::{self, Msg};
    use crate::deskflow::tls::testclient;
    use crate::deskflow::{ident, tls};
    use std::net::TcpListener;

    fn read_msg(c: &mut impl Read) -> Msg {
        let mut len = [0u8; 4];
        c.read_exact(&mut len).unwrap();
        let mut p = vec![0u8; u32::from_be_bytes(len) as usize];
        c.read_exact(&mut p).unwrap();
        proto::decode(&p).unwrap()
    }

    #[test]
    fn a_clipboard_larger_than_the_tls_default_reaches_the_client() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let cert = rcgen::generate_simple_self_signed(vec!["big".to_string()]).unwrap();
            let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
            // 30 s: only reached when the test fails.
            tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let conn = rustls::ClientConnection::new(testclient::config(&cert), testclient::server_name()).unwrap();
            let mut c = rustls::StreamOwned::new(conn, tcp);
            let mut len = [0u8; 4];
            c.read_exact(&mut len).unwrap();
            let mut hello = vec![0u8; u32::from_be_bytes(len) as usize];
            c.read_exact(&mut hello).unwrap();
            let back = Msg::HelloBack {
                major: 1,
                minor: 8,
                name: b"big".to_vec(),
            };
            c.write_all(&proto::frame(&proto::encode(&back, 8))).unwrap();
            c.flush().unwrap();
            assert_eq!(read_msg(&mut c), Msg::Qinf);
            let mut asm = clipdata::Assembler::default();
            loop {
                match read_msg(&mut c) {
                    Msg::Cclp { .. } => {}
                    Msg::Dclp { id, seq, mark, data } => {
                        if let clipdata::Asm::Finished(m) = asm.push(id, seq, mark, &data) {
                            return clipdata::unmarshal_text(&m).unwrap();
                        }
                    }
                    m => panic!("{m:?}"),
                }
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let id = ident::load_or_create(
            &crate::testdir::TestDir::new("deskflow-conn", "big").join("identity.pem"),
            "test",
        )
        .unwrap();
        // The connection runs on a frozen clock: its handshake and hello
        // deadlines (10 s each) measure test load, not the code, when a
        // debug rcgen key and identity are made next to dozens of other
        // tests in make test-unit. Only the loop's own end is wall time.
        let t0 = Instant::now();
        let mut c = Conn::new(1, stream, tls::server_config(&id), t0).unwrap();
        let text = vec![b'x'; 900 * 1024];
        let mut sent = false;
        // 60 s: only reached when the test fails.
        let end = Instant::now() + Duration::from_secs(60);
        while !client.is_finished() && Instant::now() < end {
            let now = t0;
            for ev in c.on_readable(now) {
                if let Event::Named(_) = ev {
                    c.set_verdict(Verdict::Trusted(0), now);
                }
            }
            if c.trusted() && !sent {
                c.clipboard(&text);
                sent = true;
            }
            c.flush();
            // The client closes its end once it has the text, a moment
            // before it counts as finished; whether the close came too
            // soon is the client's to say.
            if c.closing().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let closing = c.closing().map(str::to_owned);
        let got = client.join();
        assert!(got.is_ok(), "the client failed; the server's end: {closing:?}");
        assert_eq!(got.unwrap(), text);
    }
}
