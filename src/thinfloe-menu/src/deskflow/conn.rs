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

/// What a connection tells the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The client's name: the daemon owes [`Conn::set_verdict`] in the
    /// same loop pass.
    Named(String),
    /// The size of the client's screen, from its DINF.
    Screen { w: i16, h: i16 },
}

pub struct Conn {
    /// The daemon's number for it, unique for the life of the process.
    id: usize,
    stream: TcpStream,
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
        let tls = ServerConnection::new(cfg).map_err(io::Error::other)?;
        Ok(Conn {
            id,
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

    /// The bytes of the session's answer (enter and send make no other).
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
