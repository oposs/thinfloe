//! The text clipboard on the wire (spec, Clipboard): the marshalled form,
//! the DCLP transfer split and the inbound assembler. Pure data, no I/O.

use super::proto::Msg;

/// The largest marshalled clipboard sent or accepted, in bytes (1 MiB).
pub const CLIP_MAX: usize = 1 << 20;
/// The largest data chunk of one DCLP message.
pub const CHUNK: usize = 512 * 1024;

const FORMAT_TEXT: u32 = 0;
const MARK_START: u8 = 1;
const MARK_DATA: u8 = 2;
const MARK_END: u8 = 3;

const fn code(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}
/// Option codes of the DSOP message (OptionTypes.h).
pub const CLPS: u32 = code(b"CLPS");
pub const CLSZ: u32 = code(b"CLSZ");

/// The DSOP options for the clipboard switch; CLSZ is in KiB.
pub fn options(clipboard: bool) -> Vec<u32> {
    if clipboard {
        vec![CLPS, 1, CLSZ, 1024]
    } else {
        vec![CLPS, 0]
    }
}

/// One format (Text, id 0) in the marshalled form: a count, then per format
/// id, size and bytes, all big-endian u32.
pub fn marshal_text(text: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + text.len());
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend_from_slice(&FORMAT_TEXT.to_be_bytes());
    v.extend_from_slice(&(text.len() as u32).to_be_bytes());
    v.extend_from_slice(text);
    v
}

/// The Text format of a marshalled clipboard, other formats skipped;
/// `None` when it is truncated or has no Text.
pub fn unmarshal_text(data: &[u8]) -> Option<Vec<u8>> {
    let u32_at =
        |p: usize| -> Option<u32> { Some(u32::from_be_bytes(data.get(p..p.checked_add(4)?)?.try_into().ok()?)) };
    let count = u32_at(0)?;
    let mut pos = 4usize;
    for _ in 0..count {
        let id = u32_at(pos)?;
        let size = u32_at(pos.checked_add(4)?)? as usize;
        let start = pos.checked_add(8)?;
        let end = start.checked_add(size)?;
        let body = data.get(start..end)?;
        if id == FORMAT_TEXT {
            return Some(body.to_vec());
        }
        pos = end;
    }
    None
}

/// The DCLP messages of one transfer: start, data chunks, end. An empty
/// transfer has one empty data chunk.
pub fn chunks(id: u8, seq: u32, marshalled: &[u8]) -> Vec<Msg> {
    let dclp = |mark: u8, data: Vec<u8>| Msg::Dclp { id, seq, mark, data };
    let mut v = vec![dclp(MARK_START, marshalled.len().to_string().into_bytes())];
    if marshalled.is_empty() {
        v.push(dclp(MARK_DATA, Vec::new()));
    } else {
        v.extend(marshalled.chunks(CHUNK).map(|c| dclp(MARK_DATA, c.to_vec())));
    }
    v.push(dclp(MARK_END, Vec::new()));
    v
}

/// What one DCLP message did to the assembler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asm {
    Started,
    InProgress,
    /// The transfer ended; the marshalled bytes.
    Finished(Vec<u8>),
    /// Part of a transfer too large to keep, or of an unused clipboard id.
    Dropped,
    /// The stream broke the protocol.
    Error,
}

#[derive(Default)]
enum Slot {
    #[default]
    Idle,
    Receiving {
        declared: usize,
        buf: Vec<u8>,
    },
    Oversize,
}

/// Inbound DCLP transfers, one slot per clipboard id (0 and 1 exist).
#[derive(Default)]
pub struct Assembler {
    slots: [Slot; 2],
}

impl Assembler {
    pub fn push(&mut self, id: u8, _seq: u32, mark: u8, data: &[u8]) -> Asm {
        let Some(slot) = self.slots.get_mut(id as usize) else {
            return Asm::Dropped;
        };
        match (mark, std::mem::take(slot)) {
            (MARK_START, _) => {
                let declared = std::str::from_utf8(data).ok().and_then(|s| s.parse::<usize>().ok());
                match declared {
                    None => Asm::Error,
                    Some(n) if n > CLIP_MAX => {
                        *slot = Slot::Oversize;
                        Asm::Dropped
                    }
                    Some(declared) => {
                        *slot = Slot::Receiving {
                            declared,
                            buf: Vec::with_capacity(declared),
                        };
                        Asm::Started
                    }
                }
            }
            (MARK_DATA, Slot::Receiving { declared, mut buf }) => {
                if buf.len() + data.len() > declared {
                    return Asm::Error;
                }
                buf.extend_from_slice(data);
                *slot = Slot::Receiving { declared, buf };
                Asm::InProgress
            }
            (MARK_DATA, Slot::Oversize) => {
                *slot = Slot::Oversize;
                Asm::Dropped
            }
            (MARK_END, Slot::Receiving { declared, buf }) => {
                if buf.len() == declared {
                    Asm::Finished(buf)
                } else {
                    Asm::Error
                }
            }
            (MARK_END, Slot::Oversize) => Asm::Dropped,
            _ => Asm::Error,
        }
    }

    /// Bytes buffered over all slots.
    #[cfg(test)]
    fn buffered(&self) -> usize {
        self.slots
            .iter()
            .map(|s| match s {
                Slot::Receiving { buf, .. } => buf.len(),
                _ => 0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marshal_roundtrip() {
        let t = b"h\xc3\xa4llo\n";
        assert_eq!(unmarshal_text(&marshal_text(t)), Some(t.to_vec()));
    }

    #[test]
    fn unmarshal_skips_other_formats() {
        let mut d = 2u32.to_be_bytes().to_vec();
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&3u32.to_be_bytes());
        d.extend_from_slice(b"<b>");
        d.extend_from_slice(&0u32.to_be_bytes());
        d.extend_from_slice(&2u32.to_be_bytes());
        d.extend_from_slice(b"hi");
        assert_eq!(unmarshal_text(&d), Some(b"hi".to_vec()));
    }

    #[test]
    fn unmarshal_truncated_is_none() {
        let m = marshal_text(b"hello");
        for n in 0..m.len() {
            assert_eq!(unmarshal_text(&m[..n]), None, "{n}");
        }
        assert_eq!(unmarshal_text(&u32::MAX.to_be_bytes()), None);
    }

    fn parts(v: &[Msg]) -> Vec<(u8, usize)> {
        v.iter()
            .map(|m| match m {
                Msg::Dclp { mark, data, .. } => (*mark, data.len()),
                _ => panic!("not DCLP"),
            })
            .collect()
    }

    #[test]
    fn chunks_split_at_512k() {
        let m = vec![7u8; CHUNK * 2 + 1];
        let c = chunks(0, 5, &m);
        assert_eq!(
            c[0],
            Msg::Dclp {
                id: 0,
                seq: 5,
                mark: 1,
                data: b"1048577".to_vec()
            }
        );
        assert_eq!(parts(&c), [(1, 7), (2, CHUNK), (2, CHUNK), (2, 1), (3, 0)]);
    }

    #[test]
    fn empty_transfer_has_one_data_chunk() {
        let c = chunks(0, 1, &[]);
        assert_eq!(parts(&c), [(1, 1), (2, 0), (3, 0)]);
    }

    #[test]
    fn chunks_reassemble() {
        let m = marshal_text(&vec![b'x'; CHUNK + 10]);
        let mut a = Assembler::default();
        let mut last = Asm::Error;
        for c in chunks(0, 1, &m) {
            let Msg::Dclp { id, seq, mark, data } = c else { panic!() };
            last = a.push(id, seq, mark, &data);
        }
        assert_eq!(last, Asm::Finished(m));
    }

    #[test]
    fn assembler_errors() {
        let mut a = Assembler::default();
        assert_eq!(a.push(0, 0, 2, b"x"), Asm::Error);
        assert_eq!(a.push(0, 0, 3, b""), Asm::Error);
        assert_eq!(a.push(0, 0, 1, b"2"), Asm::Started);
        assert_eq!(a.push(0, 0, 2, b"abc"), Asm::Error);
        assert_eq!(a.push(0, 0, 1, b"3"), Asm::Started);
        assert_eq!(a.push(0, 0, 2, b"ab"), Asm::InProgress);
        assert_eq!(a.push(0, 0, 3, b""), Asm::Error);
        assert_eq!(a.push(0, 0, 1, b"x"), Asm::Error);
    }

    #[test]
    fn assembler_oversize_is_dropped_not_error() {
        let mut a = Assembler::default();
        assert_eq!(a.push(0, 0, 1, b"2000000"), Asm::Dropped);
        assert_eq!(a.push(0, 0, 2, &[0; 1000]), Asm::Dropped);
        assert_eq!(a.buffered(), 0);
        assert_eq!(a.push(0, 0, 3, b""), Asm::Dropped);
        assert_eq!(a.buffered(), 0);
        // The slot is free again.
        assert_eq!(a.push(0, 0, 1, b"0"), Asm::Started);
    }

    #[test]
    fn assembler_ids_are_independent() {
        let mut a = Assembler::default();
        assert_eq!(a.push(0, 0, 1, b"1"), Asm::Started);
        assert_eq!(a.push(1, 0, 1, b"1"), Asm::Started);
        assert_eq!(a.push(1, 0, 2, b"y"), Asm::InProgress);
        assert_eq!(a.push(1, 0, 3, b""), Asm::Finished(b"y".to_vec()));
        assert_eq!(a.push(0, 0, 2, b"x"), Asm::InProgress);
        assert_eq!(a.push(0, 0, 3, b""), Asm::Finished(b"x".to_vec()));
        assert_eq!(a.push(9, 0, 2, b"x"), Asm::Dropped);
    }

    #[test]
    fn options_codes() {
        assert_eq!(CLPS, u32::from_be_bytes(*b"CLPS"));
        assert_eq!(options(true), [CLPS, 1, CLSZ, 1024]);
        assert_eq!(options(false), [CLPS, 0]);
    }
}
