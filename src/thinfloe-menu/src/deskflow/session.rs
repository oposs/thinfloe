//! One connection's protocol state machine (spec, Protocol subset and
//! Connection flow). Sans-IO: the daemon feeds it payloads cut by
//! [`proto::Framer`], the current time and the pairing verdict, and writes
//! the [`Out::Send`] bytes to the TLS stream. It never reads a clock.

use std::time::{Duration, Instant};

use super::clipdata::{self, Asm, Assembler};
use super::pair::Verdict;
use super::proto::{self, Msg};

/// The version the server offers.
const MAJOR: u16 = 1;
const MINOR_MAX: u16 = 8;
const MINOR_MIN: u16 = 6;
/// The longest client name kept, in bytes.
pub const NAME_MAX: usize = 64;
/// The client must answer the Hello within this time of `new`.
const HELLO_DEADLINE: Duration = Duration::from_secs(10);
const CALV_EVERY: Duration = Duration::from_secs(3);
/// A trusted client that sent nothing for this long is gone.
const DEAD_AFTER: Duration = Duration::from_secs(9);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// Framed bytes for the TLS stream.
    Send(Vec<u8>),
    /// The client's name (capped, control characters replaced by `?`); the
    /// caller answers with [`Session::set_verdict`].
    Named { name: String },
    /// The size of the client's screen, from its DINF.
    Screen { w: i16, h: i16 },
    /// A finished inbound text clipboard (id 0), only while sharing is on.
    Clipboard { text: Vec<u8> },
    /// The client took the clipboard (CCLP for id 0), only while sharing is on.
    Grabbed,
    /// End the connection after the bytes before this were written.
    Close(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// The Hello is out; no HelloBack yet.
    AwaitHello,
    /// HelloBack accepted, the caller owes a verdict.
    AwaitVerdict,
    /// Trusted: QINF sent, DINF not yet seen.
    Trusted,
    /// The first DINF has been answered.
    Active,
    Closed,
}

pub struct Session {
    state: State,
    minor: u16,
    created: Instant,
    last_rx: Instant,
    last_calv: Instant,
    seq: u32,
    clipboard: bool,
    clip_seq: u32,
    asm: Assembler,
}

fn send(m: &Msg, minor: u16) -> Out {
    Out::Send(proto::frame(&proto::encode(m, minor)))
}

/// Unicode format characters (category Cf) and the line and paragraph
/// separators: invisible, and some reorder the text around them.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{890}'..='\u{891}'
        | '\u{8e2}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}'
        | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}'
        | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

/// The client's name as shown: at most 64 raw bytes, lossily decoded, every
/// control or format character replaced by `?`, cut again at 64 bytes on a
/// character boundary (the replacement character takes three bytes).
fn clean_name(raw: &[u8]) -> String {
    let raw = &raw[..raw.len().min(NAME_MAX)];
    let mut s: String = String::from_utf8_lossy(raw)
        .chars()
        .map(|c| if c.is_control() || is_format_char(c) { '?' } else { c })
        .collect();
    let mut end = s.len().min(NAME_MAX);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}

impl Session {
    pub fn new(now: Instant) -> (Session, Vec<Out>) {
        let s = Session {
            state: State::AwaitHello,
            minor: MINOR_MAX,
            created: now,
            last_rx: now,
            last_calv: now,
            seq: 0,
            clipboard: true,
            clip_seq: 0,
            asm: Assembler::default(),
        };
        let hello = send(
            &Msg::Hello {
                major: MAJOR,
                minor: MINOR_MAX,
            },
            MINOR_MAX,
        );
        (s, vec![hello])
    }

    /// The negotiated minor version (the server's until the HelloBack).
    pub fn minor(&self) -> u16 {
        self.minor
    }

    fn close(&mut self, why: &'static str) -> Vec<Out> {
        self.state = State::Closed;
        vec![Out::Close(why)]
    }

    pub fn on_payload(&mut self, p: &[u8], now: Instant) -> Vec<Out> {
        if self.state == State::Closed {
            return Vec::new();
        }
        self.last_rx = now;
        match self.state {
            State::AwaitHello => self.hello_back(p),
            State::AwaitVerdict | State::Closed => Vec::new(),
            State::Trusted | State::Active => self.message(p),
        }
    }

    fn hello_back(&mut self, p: &[u8]) -> Vec<Out> {
        let Ok(Msg::HelloBack { major, minor, name }) = proto::decode_hello_back(p) else {
            return self.close("bad hello");
        };
        if major != MAJOR || !(MINOR_MIN..=MINOR_MAX).contains(&minor) {
            let mut out = vec![send(
                &Msg::Eicv {
                    major: MAJOR,
                    minor: MINOR_MAX,
                },
                MINOR_MAX,
            )];
            out.extend(self.close("incompatible version"));
            return out;
        }
        let name = clean_name(&name);
        if name.is_empty() {
            return self.close("empty client name");
        }
        self.minor = minor;
        self.state = State::AwaitVerdict;
        vec![Out::Named { name }]
    }

    fn message(&mut self, p: &[u8]) -> Vec<Out> {
        let Ok(m) = proto::decode(p) else {
            return self.close("bad message");
        };
        match m {
            // The client echoes the server's keep-alive; answering its own
            // would start an endless exchange. It only shows the client lives.
            Msg::Calv => Vec::new(),
            Msg::Dinf { w, h, .. } => {
                let mut out = vec![send(&Msg::Ciak, self.minor)];
                if self.state == State::Trusted {
                    self.state = State::Active;
                    out.push(send(&Msg::Crop, self.minor));
                    out.push(send(&Msg::Dsop(clipdata::options(self.clipboard)), self.minor));
                }
                out.push(Out::Screen { w, h });
                out
            }
            Msg::Cclp { id: 0, .. } if self.clipboard => vec![Out::Grabbed],
            Msg::Dclp { id, seq, mark, data } => match self.asm.push(id, seq, mark, &data) {
                Asm::Error => self.close("bad clipboard"),
                Asm::Finished(m) if id == 0 && self.clipboard => clipdata::unmarshal_text(&m)
                    .map(|text| vec![Out::Clipboard { text }])
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Time passes: the Hello deadline, the keep-alive and the dead-client
    /// check.
    pub fn tick(&mut self, now: Instant) -> Vec<Out> {
        match self.state {
            State::AwaitHello => {
                if now.saturating_duration_since(self.created) >= HELLO_DEADLINE {
                    return self.close("no hello");
                }
            }
            State::Trusted | State::Active => {
                if now.saturating_duration_since(self.last_rx) >= DEAD_AFTER {
                    return self.close("keep-alive lost");
                }
                if now.saturating_duration_since(self.last_calv) >= CALV_EVERY {
                    self.last_calv = now;
                    return vec![send(&Msg::Calv, self.minor)];
                }
            }
            State::AwaitVerdict => {
                if now.saturating_duration_since(self.created) >= HELLO_DEADLINE {
                    return self.close("no verdict");
                }
            }
            State::Closed => {}
        }
        Vec::new()
    }

    /// The pairing decision for the name given by [`Out::Named`].
    pub fn set_verdict(&mut self, v: Verdict) -> Vec<Out> {
        if self.state != State::AwaitVerdict {
            return Vec::new();
        }
        match v {
            Verdict::Trusted(_) => {
                self.state = State::Trusted;
                // The keep-alive runs from the HelloBack.
                self.last_calv = self.last_rx;
                vec![send(&Msg::Qinf, self.minor)]
            }
            Verdict::Pending | Verdict::FingerprintChanged => {
                let why = if v == Verdict::Pending {
                    "not paired"
                } else {
                    "fingerprint changed"
                };
                let mut out = vec![send(&Msg::Eunk, self.minor)];
                out.extend(self.close(why));
                out
            }
        }
    }

    fn live(&self) -> bool {
        matches!(self.state, State::Trusted | State::Active)
    }

    /// Turn clipboard sharing on or off; an active session tells the client.
    pub fn set_clipboard(&mut self, on: bool) -> Vec<Out> {
        if self.clipboard == on {
            return Vec::new();
        }
        self.clipboard = on;
        if self.state == State::Active {
            vec![send(&Msg::Dsop(clipdata::options(on)), self.minor)]
        } else {
            Vec::new()
        }
    }

    /// CCLP only: an inactive device learns that ThinFloe owns the clipboard.
    pub fn grab(&mut self) -> Vec<Out> {
        if !self.live() || !self.clipboard {
            return Vec::new();
        }
        self.clip_seq = self.clip_seq.wrapping_add(1);
        vec![send(
            &Msg::Cclp {
                id: 0,
                seq: self.clip_seq,
            },
            self.minor,
        )]
    }

    /// CCLP and the DCLP transfer of `text` (clipboard id 0).
    pub fn clipboard(&mut self, text: &[u8]) -> Vec<Out> {
        let m = clipdata::marshal_text(text);
        if !self.live() || !self.clipboard || m.len() > clipdata::CLIP_MAX {
            return Vec::new();
        }
        self.clip_seq = self.clip_seq.wrapping_add(1);
        let seq = self.clip_seq;
        let mut out = vec![send(&Msg::Cclp { id: 0, seq }, self.minor)];
        out.extend(clipdata::chunks(0, seq, &m).iter().map(|c| send(c, self.minor)));
        out
    }

    /// CINN: the pointer enters the client's screen at (x, y).
    pub fn enter(&mut self, x: i16, y: i16, mods: u16) -> Vec<Out> {
        if !self.live() {
            return Vec::new();
        }
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        vec![send(&Msg::Cinn { x, y, seq, mods }, self.minor)]
    }

    /// COUT: the pointer leaves the client's screen.
    pub fn leave(&mut self) -> Vec<Out> {
        if !self.live() {
            return Vec::new();
        }
        vec![send(&Msg::Cout, self.minor)]
    }

    /// Any other message for the client, in the form its minor needs.
    pub fn send(&mut self, m: Msg) -> Vec<Out> {
        if !self.live() {
            return Vec::new();
        }
        vec![send(&m, self.minor)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn hello_back(minor: u16, name: &[u8]) -> Vec<u8> {
        proto::encode(
            &Msg::HelloBack {
                major: 1,
                minor,
                name: name.to_vec(),
            },
            8,
        )
    }

    fn dinf(w: i16, h: i16) -> Vec<u8> {
        proto::encode(
            &Msg::Dinf {
                x: 0,
                y: 0,
                w,
                h,
                warp: 0,
                mx: 0,
                my: 0,
            },
            8,
        )
    }

    /// The messages in the Send outputs, decoded.
    fn sent(out: &[Out]) -> Vec<Msg> {
        out.iter()
            .filter_map(|o| match o {
                Out::Send(b) => {
                    assert_eq!(u32::from_be_bytes(b[..4].try_into().unwrap()) as usize, b.len() - 4);
                    Some(proto::decode(&b[4..]).unwrap())
                }
                _ => None,
            })
            .collect()
    }

    fn closes(out: &[Out]) -> Option<&'static str> {
        out.iter().find_map(|o| match o {
            Out::Close(w) => Some(*w),
            _ => None,
        })
    }

    /// A session past HelloBack with the verdict Trusted.
    fn trusted(minor: u16, t0: Instant) -> Session {
        let (mut se, _) = Session::new(t0);
        se.on_payload(&hello_back(minor, b"laptop"), t0);
        se.set_verdict(Verdict::Trusted(0));
        se
    }

    #[test]
    fn hello_first() {
        let (se, out) = Session::new(Instant::now());
        assert_eq!(out.len(), 1);
        assert_eq!(
            out,
            [Out::Send(proto::frame(&proto::encode(
                &Msg::Hello { major: 1, minor: 8 },
                8
            )))]
        );
        assert_eq!(se.minor(), 8);
    }

    #[test]
    fn old_minor_gets_eicv() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        let out = se.on_payload(&hello_back(5, b"laptop"), t);
        assert_eq!(sent(&out), [Msg::Eicv { major: 1, minor: 8 }]);
        assert_eq!(closes(&out), Some("incompatible version"));
        // A wrong major as well, and a minor above 8.
        for (major, minor) in [(2u16, 8u16), (1, 9)] {
            let (mut se, _) = Session::new(t);
            let p = proto::encode(
                &Msg::HelloBack {
                    major,
                    minor,
                    name: b"x".to_vec(),
                },
                8,
            );
            assert!(closes(&se.on_payload(&p, t)).is_some());
        }
    }

    #[test]
    fn garbage_hello_back_closes() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        assert_eq!(closes(&se.on_payload(b"Bar", t)), Some("bad hello"));
        assert!(se.on_payload(&hello_back(8, b"x"), t).is_empty());
    }

    #[test]
    fn hello_back_negotiates_down() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        let out = se.on_payload(&hello_back(6, b"laptop"), t);
        assert_eq!(out, [Out::Named { name: "laptop".into() }]);
        assert_eq!(se.minor(), 6);
    }

    #[test]
    fn hello_back_name_capped() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        let out = se.on_payload(&hello_back(8, &vec![b'a'; 10_000]), t);
        match &out[..] {
            [Out::Named { name }] => assert_eq!(name.len(), 64),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn name_is_made_safe() {
        assert_eq!(clean_name(b"a\tb\r\nc\x7fd\x1b[0m"), "a?b??c?d?[0m");
        assert_eq!(clean_name("a\u{85}b".as_bytes()), "a?b");
        for c in [
            '\u{202e}', '\u{200b}', '\u{200f}', '\u{2066}', '\u{2069}', '\u{feff}', '\u{2028}', '\u{2029}',
        ] {
            assert_eq!(clean_name(format!("a{c}b").as_bytes()), "a?b", "{c:?}");
        }
        // A cut in the middle of a character, and invalid bytes grown to
        // three-byte replacement characters, stay within 64 bytes.
        let mut raw = vec![b'a'; 63];
        raw.extend("\u{e9}".as_bytes());
        let n = clean_name(&raw);
        assert!(n.len() <= 64 && n.starts_with(&"a".repeat(63)));
        let n = clean_name(&[0xff; 100]);
        assert!(n.len() <= 64 && n.chars().all(|c| c == '\u{fffd}'));
        assert_eq!(clean_name("l\u{e4}ptop".as_bytes()), "l\u{e4}ptop");
    }

    #[test]
    fn empty_name_closes_without_naming() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        let out = se.on_payload(&hello_back(8, b""), t);
        assert_eq!(out, [Out::Close("empty client name")]);
    }

    #[test]
    fn trusted_gets_qinf_then_ciak_after_dinf() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        se.on_payload(&hello_back(8, b"laptop"), t);
        let out = se.set_verdict(Verdict::Trusted(0));
        assert_eq!(sent(&out), [Msg::Qinf]);
        let out = se.on_payload(&dinf(1920, 1080), t);
        assert_eq!(sent(&out), [Msg::Ciak, Msg::Crop, Msg::Dsop(clipdata::options(true))]);
        assert_eq!(out.last(), Some(&Out::Screen { w: 1920, h: 1080 }));
        let out = se.on_payload(&dinf(1280, 720), t);
        assert_eq!(sent(&out), [Msg::Ciak]);
        assert_eq!(out.last(), Some(&Out::Screen { w: 1280, h: 720 }));
    }

    #[test]
    fn pending_gets_eunk_and_close() {
        for (v, why) in [
            (Verdict::Pending, "not paired"),
            (Verdict::FingerprintChanged, "fingerprint changed"),
        ] {
            let t = Instant::now();
            let (mut se, _) = Session::new(t);
            se.on_payload(&hello_back(8, b"laptop"), t);
            let out = se.set_verdict(v);
            assert_eq!(sent(&out), [Msg::Eunk]);
            assert_eq!(closes(&out), Some(why));
            assert!(se.on_payload(&dinf(1, 1), t).is_empty());
            assert!(se.send(Msg::Calv).is_empty());
        }
    }

    #[test]
    fn nothing_goes_out_before_a_verdict() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        se.on_payload(&hello_back(8, b"laptop"), t);
        assert!(se.on_payload(&dinf(1, 1), t).is_empty());
        assert!(se.enter(1, 1, 0).is_empty());
        assert!(se.tick(t + s(8)).is_empty());
    }

    #[test]
    fn hello_deadline_10s() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        assert!(se.tick(t + s(9)).is_empty());
        assert_eq!(se.tick(t + s(10)), [Out::Close("no hello")]);
        assert!(se.tick(t + s(11)).is_empty());
        assert!(se.on_payload(&hello_back(8, b"x"), t + s(11)).is_empty());
    }

    #[test]
    fn verdict_deadline_10s() {
        let t = Instant::now();
        let (mut se, _) = Session::new(t);
        se.on_payload(&hello_back(8, b"laptop"), t + s(1));
        assert!(se.tick(t + s(9)).is_empty());
        assert_eq!(se.tick(t + s(10)), [Out::Close("no verdict")]);
        assert!(se.set_verdict(Verdict::Trusted(0)).is_empty());
    }

    #[test]
    fn keepalive_every_3s_dead_after_9s() {
        let t = Instant::now();
        let mut se = trusted(8, t);
        assert!(se.tick(t + s(2)).is_empty());
        assert_eq!(sent(&se.tick(t + s(3))), [Msg::Calv]);
        assert!(se.tick(t + s(5)).is_empty());
        assert_eq!(sent(&se.tick(t + s(6))), [Msg::Calv]);
        let out = se.tick(t + s(9));
        assert_eq!(closes(&out), Some("keep-alive lost"));
        assert!(se.tick(t + s(12)).is_empty());
    }

    #[test]
    fn any_message_postpones_the_dead_check() {
        let t = Instant::now();
        let mut se = trusted(8, t);
        se.on_payload(&proto::encode(&Msg::Cnop, 8), t + s(8));
        assert!(closes(&se.tick(t + s(9))).is_none());
        assert!(closes(&se.tick(t + s(16))).is_none());
        assert!(closes(&se.tick(t + s(17))).is_some());
    }

    #[test]
    fn a_client_calv_is_not_echoed() {
        // The stock client echoes every CALV it gets; an answer from the
        // server would start an endless exchange.
        let t = Instant::now();
        let mut se = trusted(8, t);
        assert!(se.on_payload(&proto::encode(&Msg::Calv, 8), t + s(1)).is_empty());
        let mut active = trusted(8, t);
        active.on_payload(
            &proto::encode(
                &Msg::Dinf {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                    warp: 0,
                    mx: 0,
                    my: 0,
                },
                8,
            ),
            t,
        );
        assert!(active.on_payload(&proto::encode(&Msg::Calv, 8), t + s(1)).is_empty());
    }

    #[test]
    fn a_client_calv_postpones_the_dead_check() {
        let t = Instant::now();
        let mut se = trusted(8, t);
        se.on_payload(&proto::encode(&Msg::Calv, 8), t + s(8));
        assert!(closes(&se.tick(t + s(16))).is_none());
        assert!(closes(&se.tick(t + s(17))).is_some());
    }

    #[test]
    fn undecodable_message_closes() {
        let t = Instant::now();
        let mut se = trusted(8, t);
        assert_eq!(closes(&se.on_payload(b"DINF", t)), Some("bad message"));
    }

    #[test]
    fn enter_leave_and_seq() {
        let t = Instant::now();
        let mut se = trusted(8, t);
        let a = sent(&se.enter(10, 20, 3));
        let b = sent(&se.enter(11, 21, 0));
        assert_eq!(
            a,
            [Msg::Cinn {
                x: 10,
                y: 20,
                seq: 1,
                mods: 3
            }]
        );
        assert!(matches!(b[0], Msg::Cinn { seq: 2, .. }));
        assert_eq!(sent(&se.leave()), [Msg::Cout]);
    }

    #[test]
    fn key_form_follows_minor() {
        let t = Instant::now();
        let key = Msg::KeyDown {
            id: 97,
            mods: 0,
            button: 38,
        };
        let code = |se: &mut Session| match &se.send(key.clone())[0] {
            Out::Send(b) => b[4..8].to_vec(),
            o => panic!("{o:?}"),
        };
        assert_eq!(code(&mut trusted(7, t)), b"DKDN");
        assert_eq!(code(&mut trusted(8, t)), b"DKDL");
    }

    /// An Active session with the setup output drained.
    fn active(t: Instant) -> Session {
        let mut se = trusted(8, t);
        se.on_payload(&dinf(1920, 1080), t);
        se
    }

    fn dclp(id: u8, mark: u8, data: &[u8]) -> Vec<u8> {
        proto::encode(
            &Msg::Dclp {
                id,
                seq: 1,
                mark,
                data: data.to_vec(),
            },
            8,
        )
    }

    fn feed_transfer(se: &mut Session, id: u8, text: &[u8], t: Instant) -> Vec<Out> {
        let m = clipdata::marshal_text(text);
        let mut out = Vec::new();
        for c in clipdata::chunks(id, 1, &m) {
            out.extend(se.on_payload(&proto::encode(&c, 8), t));
        }
        out
    }

    #[test]
    fn dsop_carries_clipboard_option() {
        let t = Instant::now();
        let mut se = trusted(8, t);
        let out = se.on_payload(&dinf(10, 10), t);
        assert!(sent(&out).contains(&Msg::Dsop(vec![clipdata::CLPS, 1, clipdata::CLSZ, 1024])));
    }

    #[test]
    fn set_clipboard_resends_dsop_when_active() {
        let t = Instant::now();
        let mut se = active(t);
        assert_eq!(sent(&se.set_clipboard(false)), [Msg::Dsop(vec![clipdata::CLPS, 0])]);
        assert!(se.set_clipboard(false).is_empty());
        assert_eq!(sent(&se.set_clipboard(true)), [Msg::Dsop(clipdata::options(true))]);
        // Before the first DINF only the flag is stored.
        let mut se = trusted(8, t);
        assert!(se.set_clipboard(false).is_empty());
        let out = se.on_payload(&dinf(10, 10), t);
        assert!(sent(&out).contains(&Msg::Dsop(vec![clipdata::CLPS, 0])));
    }

    #[test]
    fn inbound_transfer_yields_clipboard() {
        let t = Instant::now();
        let mut se = active(t);
        let out = feed_transfer(&mut se, 0, b"h\xc3\xa4llo", t);
        assert_eq!(
            out,
            [Out::Clipboard {
                text: b"h\xc3\xa4llo".to_vec()
            }]
        );
        let out = se.on_payload(&proto::encode(&Msg::Cclp { id: 0, seq: 2 }, 8), t);
        assert_eq!(out, [Out::Grabbed]);
        // The selection (id 1) is assembled and dropped, never an error.
        assert!(feed_transfer(&mut se, 1, b"sel", t).is_empty());
        let out = se.on_payload(&proto::encode(&Msg::Cclp { id: 1, seq: 3 }, 8), t);
        assert!(out.is_empty());
        assert_eq!(feed_transfer(&mut se, 0, b"x", t).len(), 1);
    }

    #[test]
    fn inbound_ignored_when_off() {
        let t = Instant::now();
        let mut se = active(t);
        se.set_clipboard(false);
        assert!(feed_transfer(&mut se, 0, b"x", t).is_empty());
        let out = se.on_payload(&proto::encode(&Msg::Cclp { id: 0, seq: 2 }, 8), t);
        assert!(out.is_empty());
    }

    #[test]
    fn bad_chunk_closes() {
        let t = Instant::now();
        let mut se = active(t);
        let out = se.on_payload(&dclp(0, 2, b"data without start"), t);
        assert_eq!(closes(&out), Some("bad clipboard"));
    }

    #[test]
    fn clipboard_out_sends_cclp_then_dclp() {
        let t = Instant::now();
        let mut se = active(t);
        let msgs = sent(&se.clipboard(b"hi"));
        let m = clipdata::marshal_text(b"hi");
        assert_eq!(msgs[0], Msg::Cclp { id: 0, seq: 1 });
        assert_eq!(msgs[1..], clipdata::chunks(0, 1, &m)[..]);
        assert_eq!(sent(&se.grab()), [Msg::Cclp { id: 0, seq: 2 }]);
        // Not live: nothing.
        let (mut se, _) = Session::new(t);
        assert!(se.clipboard(b"hi").is_empty() && se.grab().is_empty());
    }

    #[test]
    fn clipboard_over_cap_sends_nothing() {
        let t = Instant::now();
        let mut se = active(t);
        assert!(se.clipboard(&vec![b'a'; clipdata::CLIP_MAX]).is_empty());
        se.set_clipboard(false);
        assert!(se.clipboard(b"hi").is_empty());
    }
}
