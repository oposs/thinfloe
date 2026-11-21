//! The Wayland side of the text clipboard (spec, Clipboard): a data-control
//! client on a connection of its own, so a clipboard fault never touches
//! the capture's. It reports a selection another client set (text read in
//! full) and offers text received from the remote computer.
//!
//! The protocol exists twice with the same shape: ext_data_control_v1 is
//! preferred, zwlr_data_control_v1 the fallback. The reads and writes of
//! the selection data run on pipes the daemon's poll loop watches
//! ([ Clip::pipe_fds ]); nothing here blocks.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1, ext_data_control_manager_v1, ext_data_control_offer_v1, ext_data_control_source_v1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1, zwlr_data_control_source_v1,
};

use super::clipdata::CLIP_MAX;

/// A transfer that makes no progress for this long is dropped.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(2);
/// The mime type only ThinFloe's own source offers.
const MARKER: &str = "application/x-thinfloe-deskflow";
/// What a source offers, in this order, besides the marker.
const OFFERED: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];
/// What a selection is read as, most wanted first.
const PREFERRED: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];

#[derive(Debug, PartialEq, Eq)]
pub enum ClipEvent {
    /// Another client set the selection: its text, at most [CLIP_MAX] bytes.
    Changed(Vec<u8>),
}

/// The mime type to read from an offer, or None when it has no text.
pub fn pick_mime(mimes: &[String]) -> Option<&str> {
    PREFERRED
        .iter()
        .find_map(|want| mimes.iter().find(|m| m.as_str() == *want).map(String::as_str))
}

/// An offer that lists the marker comes from ThinFloe's own source.
fn is_own(mimes: &[String]) -> bool {
    mimes.iter().any(|m| m == MARKER)
}

/// What a [CappedRead] has come to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ReadState {
    Open,
    /// More than [CLIP_MAX] bytes arrived.
    Over,
    /// Open for longer than the timeout.
    Expired,
}

/// The bytes of one read, bounded in size and time.
#[derive(Debug)]
pub struct CappedRead {
    buf: Vec<u8>,
    started: Instant,
    over: bool,
}

impl CappedRead {
    pub fn new(started: Instant) -> Self {
        CappedRead {
            buf: Vec::new(),
            started,
            over: false,
        }
    }

    /// Takes more bytes; at most CLIP_MAX+1 are kept, which is enough to
    /// know the read is over the cap.
    pub fn feed(&mut self, data: &[u8]) {
        let room = (CLIP_MAX + 1).saturating_sub(self.buf.len());
        self.buf.extend_from_slice(&data[..data.len().min(room)]);
        if self.buf.len() > CLIP_MAX {
            self.over = true;
        }
    }

    pub fn state(&self, now: Instant) -> ReadState {
        if self.over {
            ReadState::Over
        } else if now.saturating_duration_since(self.started) >= TRANSFER_TIMEOUT {
            ReadState::Expired
        } else {
            ReadState::Open
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// An offer of either protocol.
#[derive(Clone, PartialEq)]
enum Offer {
    Ext(ext_data_control_offer_v1::ExtDataControlOfferV1),
    Wlr(zwlr_data_control_offer_v1::ZwlrDataControlOfferV1),
}

impl Offer {
    fn receive(&self, mime: &str, fd: BorrowedFd<'_>) {
        match self {
            Offer::Ext(o) => o.receive(mime.to_string(), fd),
            Offer::Wlr(o) => o.receive(mime.to_string(), fd),
        }
    }
    fn destroy(&self) {
        match self {
            Offer::Ext(o) => o.destroy(),
            Offer::Wlr(o) => o.destroy(),
        }
    }
}

/// A source of either protocol.
#[derive(Clone, PartialEq)]
enum Source {
    Ext(ext_data_control_source_v1::ExtDataControlSourceV1),
    Wlr(zwlr_data_control_source_v1::ZwlrDataControlSourceV1),
}

impl Source {
    fn offer(&self, mime: &str) {
        match self {
            Source::Ext(s) => s.offer(mime.to_string()),
            Source::Wlr(s) => s.offer(mime.to_string()),
        }
    }
    fn destroy(&self) {
        match self {
            Source::Ext(s) => s.destroy(),
            Source::Wlr(s) => s.destroy(),
        }
    }
}

/// The manager and device of whichever protocol the compositor has.
enum Control {
    Ext {
        mgr: ext_data_control_manager_v1::ExtDataControlManagerV1,
        dev: ext_data_control_device_v1::ExtDataControlDeviceV1,
    },
    Wlr {
        mgr: zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
        dev: zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    },
}

impl Control {
    fn new_source(&self, qh: &QueueHandle<State>) -> Source {
        match self {
            Control::Ext { mgr, .. } => Source::Ext(mgr.create_data_source(qh, ())),
            Control::Wlr { mgr, .. } => Source::Wlr(mgr.create_data_source(qh, ())),
        }
    }
    fn set_selection(&self, source: &Source) {
        match (self, source) {
            (Control::Ext { dev, .. }, Source::Ext(s)) => dev.set_selection(Some(s)),
            (Control::Wlr { dev, .. }, Source::Wlr(s)) => dev.set_selection(Some(s)),
            _ => {}
        }
    }
}

/// A selection being read from a pipe, or a text being written to one.
enum Transfer {
    Read {
        fd: OwnedFd,
        offer: Offer,
        read: CappedRead,
    },
    Write {
        fd: OwnedFd,
        data: Rc<Vec<u8>>,
        done: usize,
        started: Instant,
    },
}

impl Transfer {
    fn fd(&self) -> RawFd {
        match self {
            Transfer::Read { fd, .. } | Transfer::Write { fd, .. } => fd.as_raw_fd(),
        }
    }
    fn started(&self) -> Instant {
        match self {
            Transfer::Read { read, .. } => read.started,
            Transfer::Write { started, .. } => *started,
        }
    }
    /// Gives the offer back to the compositor.
    fn finish(self) {
        if let Transfer::Read { offer, .. } = self {
            offer.destroy();
        }
    }
}

struct State {
    /// Offers announced and their mime types, until a selection takes them.
    pending: Vec<(Offer, Vec<String>)>,
    own: Option<(Source, Rc<Vec<u8>>)>,
    transfers: Vec<Transfer>,
    now: Instant,
}

pub struct Clip {
    conn: Connection,
    backend: Backend,
    queue: EventQueue<State>,
    control: Control,
    state: State,
}

fn nonblocking_pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0; 2];
    // SAFETY: fds has room for the two descriptors pipe2 writes.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(format!("pipe2: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: pipe2 made both and nothing else owns them.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn set_nonblocking(fd: &OwnedFd) {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let fl = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if fl >= 0 {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
    }
}

impl State {
    /// A new selection: a read in progress is cancelled; text of a foreign
    /// owner starts a read.
    fn on_selection(&mut self, offer: Option<Offer>) {
        let (reads, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.transfers)
            .into_iter()
            .partition(|t| matches!(t, Transfer::Read { .. }));
        self.transfers = rest;
        reads.into_iter().for_each(Transfer::finish);
        let Some(offer) = offer else { return };
        let mimes = match self.pending.iter().position(|(o, _)| *o == offer) {
            Some(i) => self.pending.remove(i).1,
            None => Vec::new(),
        };
        let mime = match pick_mime(&mimes) {
            Some(m) if !is_own(&mimes) => m.to_string(),
            _ => {
                offer.destroy();
                return;
            }
        };
        match nonblocking_pipe() {
            Ok((rd, wr)) => {
                offer.receive(&mime, wr.as_fd());
                self.transfers.push(Transfer::Read {
                    fd: rd,
                    offer,
                    read: CappedRead::new(self.now),
                });
            }
            Err(e) => {
                eprintln!("thinfloe-menu --deskflow: clipboard: {e}");
                offer.destroy();
            }
        }
    }

    /// The compositor wants our text on a pipe.
    fn on_send(&mut self, source: &Source, fd: OwnedFd) {
        let Some((own, data)) = &self.own else { return };
        if own != source {
            return;
        }
        set_nonblocking(&fd);
        self.transfers.push(Transfer::Write {
            fd,
            data: Rc::clone(data),
            done: 0,
            started: self.now,
        });
    }

    fn on_cancelled(&mut self, source: &Source) {
        if self.own.as_ref().is_some_and(|(o, _)| o == source) {
            source.destroy();
            self.own = None;
        }
    }

    fn on_offer_mime(&mut self, offer: &Offer, mime: String) {
        if let Some((_, mimes)) = self.pending.iter_mut().find(|(o, _)| o == offer) {
            mimes.push(mime);
        }
    }

    fn on_primary(&mut self, offer: Option<Offer>) {
        if let Some(offer) = offer {
            self.pending.retain(|(o, _)| *o != offer);
            offer.destroy();
        }
    }
}

impl Clip {
    /// The data-control protocol of the compositor, ext first.
    pub fn connect() -> Result<Clip, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
        let (globals, mut queue): (_, EventQueue<State>) =
            registry_queue_init(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("wl_seat: {e}"))?;
        let control = if let Ok(mgr) =
            globals.bind::<ext_data_control_manager_v1::ExtDataControlManagerV1, _, _>(&qh, 1..=1, ())
        {
            let dev = mgr.get_data_device(&seat, &qh, ());
            Control::Ext { mgr, dev }
        } else if let Ok(mgr) =
            globals.bind::<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, _, _>(&qh, 1..=2, ())
        {
            let dev = mgr.get_data_device(&seat, &qh, ());
            Control::Wlr { mgr, dev }
        } else {
            return Err("the compositor has neither ext_data_control_v1 nor zwlr_data_control_v1".into());
        };
        let mut state = State {
            pending: Vec::new(),
            own: None,
            transfers: Vec::new(),
            now: Instant::now(),
        };
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        let backend = conn.backend();
        Ok(Clip {
            conn,
            backend,
            queue,
            control,
            state,
        })
    }

    /// The connection's file descriptor, to poll for readable events.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.backend.poll_fd()
    }

    /// Reads what the compositor sent, without blocking, and acts on it: a
    /// foreign selection starts a read, a request for our text starts a
    /// write. The text itself arrives through [Clip::on_pipe_ready], so no
    /// event comes from here; the Vec is for the poll loop's symmetry.
    pub fn dispatch(&mut self, now: Instant) -> Result<Vec<ClipEvent>, String> {
        self.state.now = now;
        self.flush()?;
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("Wayland dispatch: {e}"))?;
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("Wayland read: {e}")),
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("Wayland dispatch: {e}"))?;
        self.flush()?;
        Ok(Vec::new())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(format!("Wayland flush: {e}")),
        }
    }

    /// The pipes to poll: POLLIN for a read, POLLOUT for a write.
    pub fn pipe_fds(&self) -> Vec<(RawFd, libc::c_short)> {
        self.state
            .transfers
            .iter()
            .map(|t| match t {
                Transfer::Read { .. } => (t.fd(), libc::POLLIN),
                Transfer::Write { .. } => (t.fd(), libc::POLLOUT),
            })
            .collect()
    }

    /// A pipe from [Clip::pipe_fds] is ready: moves what it can. A text read
    /// in full is the event; a transfer that failed or went over the cap
    /// is dropped.
    pub fn on_pipe_ready(&mut self, fd: RawFd, now: Instant) -> Vec<ClipEvent> {
        let Some(i) = self.state.transfers.iter().position(|t| t.fd() == fd) else {
            return Vec::new();
        };
        let mut t = self.state.transfers.swap_remove(i);
        let mut events = Vec::new();
        let keep = match &mut t {
            Transfer::Read { fd, read, .. } => {
                let mut buf = [0u8; 65536];
                loop {
                    // SAFETY: buf is valid for buf.len() bytes.
                    let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                    if n > 0 {
                        read.feed(&buf[..n as usize]);
                        if read.state(now) == ReadState::Over {
                            break false;
                        }
                    } else if n == 0 {
                        let text = std::mem::replace(read, CappedRead::new(now)).into_bytes();
                        if !text.is_empty() {
                            events.push(ClipEvent::Changed(text));
                        }
                        break false;
                    } else {
                        let e = std::io::Error::last_os_error().kind();
                        match e {
                            std::io::ErrorKind::WouldBlock => break true,
                            std::io::ErrorKind::Interrupted => {}
                            _ => break false,
                        }
                    }
                }
            }
            Transfer::Write { fd, data, done, .. } => loop {
                if *done >= data.len() {
                    break false;
                }
                let rest = &data[*done..];
                // SAFETY: rest is valid for rest.len() bytes. A closed
                // reader gives EPIPE (the runtime ignores SIGPIPE).
                let n = unsafe { libc::write(fd.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
                if n >= 0 {
                    *done += n as usize;
                } else {
                    match std::io::Error::last_os_error().kind() {
                        std::io::ErrorKind::WouldBlock => break true,
                        std::io::ErrorKind::Interrupted => {}
                        _ => break false,
                    }
                }
            },
        };
        if keep {
            self.state.transfers.push(t);
        } else {
            t.finish();
        }
        events
    }

    /// When the oldest transfer runs out of time.
    pub fn deadline(&self) -> Option<Instant> {
        self.state
            .transfers
            .iter()
            .map(|t| t.started() + TRANSFER_TIMEOUT)
            .min()
    }

    /// Drops the transfers older than the timeout.
    pub fn tick(&mut self, now: Instant) -> Vec<ClipEvent> {
        let (old, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.state.transfers)
            .into_iter()
            .partition(|t| now.saturating_duration_since(t.started()) >= TRANSFER_TIMEOUT);
        self.state.transfers = keep;
        old.into_iter().for_each(Transfer::finish);
        Vec::new()
    }

    /// Makes this text the selection; a reader's pipe is served from the
    /// poll loop.
    pub fn set_text(&mut self, text: Vec<u8>) {
        let qh = self.queue.handle();
        let source = self.control.new_source(&qh);
        for mime in OFFERED.iter().copied().chain([MARKER]) {
            source.offer(mime);
        }
        self.control.set_selection(&source);
        if let Some((old, _)) = self.state.own.replace((source, Rc::new(text))) {
            old.destroy();
        }
        let _ = self.flush();
    }
}

macro_rules! protocol {
    ($variant:ident, $mgr:ty, $dev:ty, $dev_mod:ident, $src:ty, $src_mod:ident, $off:ty, $off_mod:ident) => {
        impl Dispatch<$mgr, ()> for State {
            fn event(_: &mut Self, _: &$mgr, _: <$mgr as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }

        impl Dispatch<$dev, ()> for State {
            fn event(
                state: &mut Self,
                _: &$dev,
                event: <$dev as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use $dev_mod::Event;
                match event {
                    Event::DataOffer { id } => state.pending.push((Offer::$variant(id), Vec::new())),
                    Event::Selection { id } => state.on_selection(id.map(Offer::$variant)),
                    Event::PrimarySelection { id } => state.on_primary(id.map(Offer::$variant)),
                    _ => {}
                }
            }

            event_created_child!(State, $dev, [
                0 => ($off, ()),
            ]);
        }

        impl Dispatch<$src, ()> for State {
            fn event(
                state: &mut Self,
                source: &$src,
                event: <$src as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use $src_mod::Event;
                match event {
                    Event::Send { fd, .. } => state.on_send(&Source::$variant(source.clone()), fd),
                    Event::Cancelled => state.on_cancelled(&Source::$variant(source.clone())),
                    _ => {}
                }
            }
        }

        impl Dispatch<$off, ()> for State {
            fn event(
                state: &mut Self,
                offer: &$off,
                event: <$off as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use $off_mod::Event;
                if let Event::Offer { mime_type } = event {
                    state.on_offer_mime(&Offer::$variant(offer.clone()), mime_type);
                }
            }
        }
    };
}

protocol!(
    Ext,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_device_v1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
    ext_data_control_source_v1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_offer_v1
);
protocol!(
    Wlr,
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_device_v1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
    zwlr_data_control_source_v1,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    zwlr_data_control_offer_v1
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pick_mime_prefers_utf8() {
        assert_eq!(
            pick_mime(&v(&[
                "TEXT",
                "STRING",
                "text/plain",
                "UTF8_STRING",
                "text/plain;charset=utf-8"
            ])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(
            pick_mime(&v(&["STRING", "text/plain", "UTF8_STRING"])),
            Some("UTF8_STRING")
        );
        assert_eq!(pick_mime(&v(&["STRING", "text/plain"])), Some("text/plain"));
        assert_eq!(pick_mime(&v(&["TEXT", "STRING"])), Some("STRING"));
        assert_eq!(pick_mime(&v(&["TEXT"])), Some("TEXT"));
        assert_eq!(pick_mime(&v(&["image/png", "text/html"])), None);
        assert_eq!(pick_mime(&[]), None);
    }

    #[test]
    fn own_marker_is_skipped() {
        assert!(is_own(&v(&["text/plain", MARKER])));
        assert!(!is_own(&v(&["text/plain", "UTF8_STRING"])));
        // Every mime the source offers is one that would be read.
        assert!(pick_mime(&v(&OFFERED)).is_some());
    }

    #[test]
    fn reader_stops_at_cap_and_deadline() {
        let t0 = Instant::now();
        let mut r = CappedRead::new(t0);
        r.feed(&vec![b'a'; CLIP_MAX]);
        assert_eq!(r.state(t0), ReadState::Open);
        r.feed(b"a");
        assert_eq!(r.state(t0), ReadState::Over);
        r.feed(&vec![b'a'; 1000]);
        assert_eq!(r.into_bytes().len(), CLIP_MAX + 1);

        let idle = CappedRead::new(t0);
        assert_eq!(idle.state(t0 + Duration::from_millis(1999)), ReadState::Open);
        assert_eq!(idle.state(t0 + Duration::from_secs(2)), ReadState::Expired);
    }
}
