//! The Deskflow wire protocol, the subset the server speaks (spec,
//! Protocol subset). Pure functions on bytes: every integer is big-endian,
//! a string is a u32 length and its bytes, and each message travels in a
//! frame with a u32 length in front (the Hello included).
//!
//! Decoding takes input from a peer that may be hostile: it never panics
//! and never allocates more than the frame it is given holds.

// Task 5 (session) and Task 7 (run) use the items that are not used yet.
#![allow(dead_code)]

/// The largest message payload the framer accepts.
pub const MAX_MSG: usize = 1 << 20;

/// The protocol name in the Hello; Deskflow clients still expect it.
const HELLO_NAME: &[u8; 7] = b"Barrier";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// The payload ends before the message does.
    Truncated,
    /// A frame announces more than MAX_MSG bytes.
    Oversize,
    /// The Hello does not start with the protocol name.
    BadHello,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Hello {
        major: u16,
        minor: u16,
    },
    HelloBack {
        major: u16,
        minor: u16,
        name: Vec<u8>,
    },
    Qinf,
    Ciak,
    Calv,
    Cnop,
    Crop,
    Dsop(Vec<u32>),
    Csec(u8),
    Cinn {
        x: i16,
        y: i16,
        seq: u32,
        mods: u16,
    },
    Cout,
    Cbye,
    Dinf {
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        warp: i16,
        mx: i16,
        my: i16,
    },
    KeyDown {
        id: u16,
        mods: u16,
        button: u16,
    },
    KeyRepeat {
        id: u16,
        mods: u16,
        count: u16,
        button: u16,
    },
    KeyUp {
        id: u16,
        mods: u16,
        button: u16,
    },
    MouseDown(i8),
    MouseUp(i8),
    MouseMove {
        x: i16,
        y: i16,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    Cclp {
        id: u8,
        seq: u32,
    },
    Dclp {
        id: u8,
        seq: u32,
        mark: u8,
        data: Vec<u8>,
    },
    Eicv {
        major: u16,
        minor: u16,
    },
    Ebsy,
    Eunk,
    Ebad,
    /// A message code this subset does not know.
    Unknown([u8; 4]),
}

/// The first minor version that sends the language string with a key.
const MINOR_LANG: u16 = 8;

fn put16(v: &mut Vec<u8>, n: u16) {
    v.extend_from_slice(&n.to_be_bytes());
}

fn put32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_be_bytes());
}

fn put_str(v: &mut Vec<u8>, s: &[u8]) {
    put32(v, s.len() as u32);
    v.extend_from_slice(s);
}

fn code(c: &[u8; 4]) -> Vec<u8> {
    c.to_vec()
}

/// The payload of a message (without the frame length). `minor` is the
/// negotiated protocol minor version and picks the key message form.
pub fn encode(m: &Msg, minor: u16) -> Vec<u8> {
    let lang = minor >= MINOR_LANG;
    let mut v;
    match m {
        Msg::Hello { major, minor } => {
            v = HELLO_NAME.to_vec();
            put16(&mut v, *major);
            put16(&mut v, *minor);
        }
        Msg::HelloBack { major, minor, name } => {
            v = HELLO_NAME.to_vec();
            put16(&mut v, *major);
            put16(&mut v, *minor);
            put_str(&mut v, name);
        }
        Msg::Qinf => v = code(b"QINF"),
        Msg::Ciak => v = code(b"CIAK"),
        Msg::Calv => v = code(b"CALV"),
        Msg::Cnop => v = code(b"CNOP"),
        Msg::Crop => v = code(b"CROP"),
        Msg::Dsop(opts) => {
            v = code(b"DSOP");
            put32(&mut v, opts.len() as u32);
            for o in opts {
                put32(&mut v, *o);
            }
        }
        Msg::Csec(b) => {
            v = code(b"CSEC");
            v.push(*b);
        }
        Msg::Cinn { x, y, seq, mods } => {
            v = code(b"CINN");
            put16(&mut v, *x as u16);
            put16(&mut v, *y as u16);
            put32(&mut v, *seq);
            put16(&mut v, *mods);
        }
        Msg::Cout => v = code(b"COUT"),
        Msg::Cbye => v = code(b"CBYE"),
        Msg::Dinf {
            x,
            y,
            w,
            h,
            warp,
            mx,
            my,
        } => {
            v = code(b"DINF");
            for n in [x, y, w, h, warp, mx, my] {
                put16(&mut v, *n as u16);
            }
        }
        Msg::KeyDown { id, mods, button } => {
            v = code(if lang { b"DKDL" } else { b"DKDN" });
            put16(&mut v, *id);
            put16(&mut v, *mods);
            put16(&mut v, *button);
            if lang {
                put_str(&mut v, b"");
            }
        }
        Msg::KeyRepeat {
            id,
            mods,
            count,
            button,
        } => {
            v = code(b"DKRP");
            put16(&mut v, *id);
            put16(&mut v, *mods);
            put16(&mut v, *count);
            put16(&mut v, *button);
            if lang {
                put_str(&mut v, b"");
            }
        }
        Msg::KeyUp { id, mods, button } => {
            v = code(b"DKUP");
            put16(&mut v, *id);
            put16(&mut v, *mods);
            put16(&mut v, *button);
        }
        Msg::MouseDown(b) => {
            v = code(b"DMDN");
            v.push(*b as u8);
        }
        Msg::MouseUp(b) => {
            v = code(b"DMUP");
            v.push(*b as u8);
        }
        Msg::MouseMove { x, y } => {
            v = code(b"DMMV");
            put16(&mut v, *x as u16);
            put16(&mut v, *y as u16);
        }
        Msg::Wheel { dx, dy } => {
            v = code(b"DMWM");
            put16(&mut v, *dx as u16);
            put16(&mut v, *dy as u16);
        }
        Msg::Cclp { id, seq } => {
            v = code(b"CCLP");
            v.push(*id);
            put32(&mut v, *seq);
        }
        Msg::Dclp { id, seq, mark, data } => {
            v = code(b"DCLP");
            v.push(*id);
            put32(&mut v, *seq);
            v.push(*mark);
            put_str(&mut v, data);
        }
        Msg::Eicv { major, minor } => {
            v = code(b"EICV");
            put16(&mut v, *major);
            put16(&mut v, *minor);
        }
        Msg::Ebsy => v = code(b"EBSY"),
        Msg::Eunk => v = code(b"EUNK"),
        Msg::Ebad => v = code(b"EBAD"),
        Msg::Unknown(c) => v = code(c),
    }
    v
}

/// A cursor over one payload; every read checks what is left.
struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        if self.b.len() < n {
            return Err(ProtoError::Truncated);
        }
        let (head, rest) = self.b.split_at(n);
        self.b = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtoError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn i16(&mut self) -> Result<i16, ProtoError> {
        Ok(self.u16()? as i16)
    }

    fn u32(&mut self) -> Result<u32, ProtoError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A u32 length and that many bytes; a length beyond what is left is
    /// an error before anything is allocated.
    fn string(&mut self) -> Result<Vec<u8>, ProtoError> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
}

/// Decodes the first message of a payload; bytes after the message are
/// ignored.
pub fn decode(payload: &[u8]) -> Result<Msg, ProtoError> {
    let mut r = Reader { b: payload };
    let c: [u8; 4] = r.take(4)?.try_into().map_err(|_| ProtoError::Truncated)?;
    Ok(match &c {
        b"QINF" => Msg::Qinf,
        b"CIAK" => Msg::Ciak,
        b"CALV" => Msg::Calv,
        b"CNOP" => Msg::Cnop,
        b"CROP" => Msg::Crop,
        b"COUT" => Msg::Cout,
        b"CBYE" => Msg::Cbye,
        b"EBSY" => Msg::Ebsy,
        b"EUNK" => Msg::Eunk,
        b"EBAD" => Msg::Ebad,
        b"DSOP" => {
            let n = r.u32()? as usize;
            // Each option takes 4 bytes, so a count beyond what is left is
            // an error before the vector is sized.
            if n > r.b.len() / 4 {
                return Err(ProtoError::Truncated);
            }
            let mut opts = Vec::with_capacity(n);
            for _ in 0..n {
                opts.push(r.u32()?);
            }
            Msg::Dsop(opts)
        }
        b"CSEC" => Msg::Csec(r.u8()?),
        b"CINN" => Msg::Cinn {
            x: r.i16()?,
            y: r.i16()?,
            seq: r.u32()?,
            mods: r.u16()?,
        },
        b"DINF" => Msg::Dinf {
            x: r.i16()?,
            y: r.i16()?,
            w: r.i16()?,
            h: r.i16()?,
            warp: r.i16()?,
            mx: r.i16()?,
            my: r.i16()?,
        },
        b"DKDN" | b"DKDL" => {
            let m = Msg::KeyDown {
                id: r.u16()?,
                mods: r.u16()?,
                button: r.u16()?,
            };
            if &c == b"DKDL" {
                r.string()?;
            }
            m
        }
        b"DKRP" => {
            let m = Msg::KeyRepeat {
                id: r.u16()?,
                mods: r.u16()?,
                count: r.u16()?,
                button: r.u16()?,
            };
            // The language string is present from 1.8; it is not needed.
            if !r.b.is_empty() {
                r.string()?;
            }
            m
        }
        b"DKUP" => Msg::KeyUp {
            id: r.u16()?,
            mods: r.u16()?,
            button: r.u16()?,
        },
        b"DMDN" => Msg::MouseDown(r.u8()? as i8),
        b"DMUP" => Msg::MouseUp(r.u8()? as i8),
        b"DMMV" => Msg::MouseMove {
            x: r.i16()?,
            y: r.i16()?,
        },
        b"DMWM" => Msg::Wheel {
            dx: r.i16()?,
            dy: r.i16()?,
        },
        b"CCLP" => Msg::Cclp {
            id: r.u8()?,
            seq: r.u32()?,
        },
        b"DCLP" => Msg::Dclp {
            id: r.u8()?,
            seq: r.u32()?,
            mark: r.u8()?,
            data: r.string()?,
        },
        b"EICV" => Msg::Eicv {
            major: r.u16()?,
            minor: r.u16()?,
        },
        _ => Msg::Unknown(c),
    })
}

/// The client's answer to the Hello: the protocol name, its version and its
/// screen name. The Hello has no four-letter code, so it has a decoder of
/// its own.
pub fn decode_hello_back(payload: &[u8]) -> Result<Msg, ProtoError> {
    let mut r = Reader { b: payload };
    if r.take(HELLO_NAME.len()).map_err(|_| ProtoError::BadHello)? != HELLO_NAME {
        return Err(ProtoError::BadHello);
    }
    Ok(Msg::HelloBack {
        major: r.u16()?,
        minor: r.u16()?,
        name: r.string()?,
    })
}

/// A payload with its u32 length in front.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + payload.len());
    put32(&mut v, payload.len() as u32);
    v.extend_from_slice(payload);
    v
}

/// Cuts a byte stream into frames. A length above MAX_MSG is refused as
/// soon as its four bytes are in, before any of the frame is buffered, and
/// the framer stays failed afterwards.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
    failed: bool,
}

impl Framer {
    pub fn push(&mut self, bytes: &[u8]) {
        if !self.failed {
            self.buf.extend_from_slice(bytes);
        }
    }

    /// The next whole payload, or None when more bytes are needed. Not an
    /// Iterator: it can fail.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Vec<u8>>, ProtoError> {
        if self.failed {
            return Err(ProtoError::Oversize);
        }
        let Some(head) = self.buf.first_chunk::<4>() else {
            return Ok(None);
        };
        let len = u32::from_be_bytes(*head) as usize;
        if len > MAX_MSG {
            self.failed = true;
            self.buf = Vec::new();
            return Err(ProtoError::Oversize);
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_bytes() {
        assert_eq!(
            encode(&Msg::Hello { major: 1, minor: 8 }, 8),
            b"Barrier\x00\x01\x00\x08"
        );
    }

    #[test]
    fn hello_back_parses_name() {
        let m = decode_hello_back(b"Barrier\0\x01\0\x08\0\0\0\x06laptop").unwrap();
        assert_eq!(
            m,
            Msg::HelloBack {
                major: 1,
                minor: 8,
                name: b"laptop".to_vec()
            }
        );
        assert_eq!(encode(&m, 8), b"Barrier\0\x01\0\x08\0\0\0\x06laptop");
    }

    #[test]
    fn hello_back_rejects_bad_input() {
        assert_eq!(
            decode_hello_back(b"Synergy\0\x01\0\x08\0\0\0\0"),
            Err(ProtoError::BadHello)
        );
        assert_eq!(decode_hello_back(b"Bar"), Err(ProtoError::BadHello));
        assert_eq!(decode_hello_back(b"Barrier\0\x01\0"), Err(ProtoError::Truncated));
        // A name longer than the frame holds is an error, not an allocation.
        assert_eq!(
            decode_hello_back(b"Barrier\0\x01\0\x08\xff\xff\xff\xffab"),
            Err(ProtoError::Truncated)
        );
    }

    #[test]
    fn frame_is_be_length() {
        assert_eq!(frame(b"CALV"), [0, 0, 0, 4, b'C', b'A', b'L', b'V']);
    }

    #[test]
    fn framer_splits_and_joins() {
        let mut bytes = frame(b"CALV");
        bytes.extend(frame(b"QINF"));
        let mut f = Framer::default();
        let mut got = Vec::new();
        for b in bytes {
            f.push(&[b]);
            while let Some(p) = f.next().unwrap() {
                got.push(p);
            }
        }
        assert_eq!(got, vec![b"CALV".to_vec(), b"QINF".to_vec()]);
    }

    #[test]
    fn framer_refuses_oversize() {
        let mut f = Framer::default();
        f.push(&((MAX_MSG + 1) as u32).to_be_bytes());
        assert_eq!(f.next(), Err(ProtoError::Oversize));
        // It stays failed and keeps nothing.
        f.push(&[0; 64]);
        assert_eq!(f.next(), Err(ProtoError::Oversize));
        let mut ok = Framer::default();
        ok.push(&(MAX_MSG as u32).to_be_bytes());
        assert_eq!(ok.next(), Ok(None));
    }

    #[test]
    fn key_down_v18_has_lang() {
        let p = encode(
            &Msg::KeyDown {
                id: 0x61,
                mods: 2,
                button: 38,
            },
            8,
        );
        assert_eq!(&p[..4], b"DKDL");
        assert!(p.ends_with(&[0, 0, 0, 0]));
        assert_eq!(p.len(), 4 + 6 + 4);
    }

    #[test]
    fn key_down_v16_is_dkdn() {
        for minor in [6, 7] {
            let p = encode(
                &Msg::KeyDown {
                    id: 0x61,
                    mods: 2,
                    button: 38,
                },
                minor,
            );
            assert_eq!(&p[..4], b"DKDN");
            assert_eq!(p.len(), 4 + 6);
        }
    }

    #[test]
    fn key_repeat_v18_five_fields() {
        let m = Msg::KeyRepeat {
            id: 1,
            mods: 2,
            count: 3,
            button: 4,
        };
        assert_eq!(encode(&m, 8).len(), 4 + 8 + 4);
        assert_eq!(encode(&m, 7).len(), 4 + 8);
        assert_eq!(decode(&encode(&m, 8)), Ok(m.clone()));
        assert_eq!(decode(&encode(&m, 7)), Ok(m));
    }

    #[test]
    fn key_down_decodes_both_forms() {
        let m = Msg::KeyDown {
            id: 9,
            mods: 8,
            button: 7,
        };
        assert_eq!(decode(&encode(&m, 8)), Ok(m.clone()));
        assert_eq!(decode(&encode(&m, 6)), Ok(m));
    }

    #[test]
    fn dinf_roundtrip() {
        let m = Msg::Dinf {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            warp: 0,
            mx: 960,
            my: 540,
        };
        let p = encode(&m, 8);
        assert_eq!(&p[8..10], b"\x07\x80");
        assert_eq!(decode(&p), Ok(m));
    }

    #[test]
    fn cinn_layout() {
        let p = encode(
            &Msg::Cinn {
                x: -1,
                y: 300,
                seq: 0x01020304,
                mods: 5,
            },
            8,
        );
        assert_eq!(p, b"CINN\xff\xff\x01\x2c\x01\x02\x03\x04\x00\x05");
        assert_eq!(
            decode(&p),
            Ok(Msg::Cinn {
                x: -1,
                y: 300,
                seq: 0x01020304,
                mods: 5
            })
        );
    }

    #[test]
    fn every_message_roundtrips() {
        let all = [
            Msg::Qinf,
            Msg::Ciak,
            Msg::Calv,
            Msg::Cnop,
            Msg::Crop,
            Msg::Dsop(vec![]),
            Msg::Dsop(vec![1, 2]),
            Msg::Csec(1),
            Msg::Cout,
            Msg::Cbye,
            Msg::KeyUp {
                id: 1,
                mods: 2,
                button: 3,
            },
            Msg::MouseDown(1),
            Msg::MouseUp(-1),
            Msg::MouseMove { x: -5, y: 7 },
            Msg::Wheel { dx: 0, dy: -120 },
            Msg::Cclp { id: 0, seq: 77 },
            Msg::Dclp {
                id: 0,
                seq: 77,
                mark: 2,
                data: b"text".to_vec(),
            },
            Msg::Eicv { major: 1, minor: 8 },
            Msg::Ebsy,
            Msg::Eunk,
            Msg::Ebad,
        ];
        for m in all {
            assert_eq!(decode(&encode(&m, 8)), Ok(m));
        }
    }

    #[test]
    fn truncated_payload_is_error() {
        let fixed = [
            Msg::Csec(1),
            Msg::Cinn {
                x: 1,
                y: 2,
                seq: 3,
                mods: 4,
            },
            Msg::Dinf {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                warp: 0,
                mx: 0,
                my: 0,
            },
            Msg::KeyDown {
                id: 1,
                mods: 2,
                button: 3,
            },
            Msg::KeyRepeat {
                id: 1,
                mods: 2,
                count: 3,
                button: 4,
            },
            Msg::KeyUp {
                id: 1,
                mods: 2,
                button: 3,
            },
            Msg::MouseDown(1),
            Msg::MouseUp(1),
            Msg::MouseMove { x: 1, y: 2 },
            Msg::Wheel { dx: 1, dy: 2 },
            Msg::Cclp { id: 0, seq: 1 },
            Msg::Dclp {
                id: 0,
                seq: 1,
                mark: 1,
                data: b"abc".to_vec(),
            },
            Msg::Eicv { major: 1, minor: 8 },
            Msg::Dsop(vec![1]),
        ];
        for m in fixed {
            let p = encode(&m, 8);
            // Every cut inside the code is also an error; cuts after the
            // code are errors for these messages, except a DKRP without
            // the language string, which is the valid 1.6 form.
            for cut in 0..p.len() {
                let r = decode(&p[..cut]);
                let valid_short = matches!(m, Msg::KeyRepeat { .. }) && cut == 12;
                if !valid_short {
                    assert_eq!(r, Err(ProtoError::Truncated), "{m:?} cut at {cut}");
                }
            }
        }
    }

    #[test]
    fn hostile_lengths_do_not_allocate() {
        assert_eq!(decode(b"DSOP\xff\xff\xff\xff"), Err(ProtoError::Truncated));
        assert_eq!(
            decode(b"DCLP\0\0\0\0\x01\x01\xff\xff\xff\xffab"),
            Err(ProtoError::Truncated)
        );
        assert_eq!(
            decode(b"DKDL\0\x01\0\x02\0\x03\xff\xff\xff\xff"),
            Err(ProtoError::Truncated)
        );
    }

    #[test]
    fn unknown_code_kept() {
        assert_eq!(decode(b"XXXXrest"), Ok(Msg::Unknown(*b"XXXX")));
        assert_eq!(encode(&Msg::Unknown(*b"XXXX"), 8), b"XXXX");
    }

    #[test]
    fn decode_survives_every_short_input() {
        for len in 0..16u8 {
            for fill in [0u8, 0xff, b'D'] {
                let p = vec![fill; len as usize];
                let _ = decode(&p);
                let _ = decode_hello_back(&p);
            }
        }
    }
}
