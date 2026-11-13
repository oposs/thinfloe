//! The progress lines `thinfloe-upgrade write` prints on stdout:
//! `@step <id> <text>`, `@progress <done> <total|->`, `@done <id>`,
//! `@touched` (the first byte goes to the stick) and
//! `@result ok|failed <reason>`. Anything else is program output.

/// Longest text or reason kept, in characters.
pub const TEXT_MAX: usize = 300;
const ID_MAX: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Step {
        id: String,
        text: String,
    },
    Progress {
        done: u64,
        total: Option<u64>,
    },
    Done {
        id: String,
    },
    /// A fresh install changes the stick from here on; before it, a cancel
    /// or a failure leaves the stick as it was.
    Touched,
    Result {
        ok: bool,
        reason: String,
    },
}

fn valid_id(s: &str) -> bool {
    (1..=ID_MAX).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn cut(s: &str) -> String {
    s.trim().chars().take(TEXT_MAX).collect()
}

/// Parse one line. `None` means the line is not part of the protocol and
/// the caller logs it as output.
pub fn parse(line: &str) -> Option<Msg> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (verb, rest) = match line.split_once(' ') {
        Some((v, r)) => (v, r),
        None => (line, ""),
    };
    match verb {
        "@step" => {
            let (id, text) = match rest.split_once(' ') {
                Some((i, t)) => (i, t),
                None => (rest, ""),
            };
            valid_id(id).then(|| Msg::Step {
                id: id.to_string(),
                text: cut(text),
            })
        }
        "@progress" => {
            let mut it = rest.split(' ');
            let done = it.next()?.parse::<u64>().ok()?;
            let total = match it.next()? {
                "-" => None,
                t => Some(t.parse::<u64>().ok()?).filter(|&n| n != 0),
            };
            if it.next().is_some() {
                return None;
            }
            Some(Msg::Progress { done, total })
        }
        "@done" => valid_id(rest).then(|| Msg::Done { id: rest.to_string() }),
        "@touched" => rest.is_empty().then_some(Msg::Touched),
        "@result" => {
            let (word, reason) = match rest.split_once(' ') {
                Some((w, r)) => (w, r),
                None => (rest, ""),
            };
            let ok = match word {
                "ok" => true,
                "failed" => false,
                _ => return None,
            };
            Some(Msg::Result {
                ok,
                reason: cut(reason),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step() {
        assert_eq!(
            parse("@step write Downloading and writing"),
            Some(Msg::Step {
                id: "write".into(),
                text: "Downloading and writing".into()
            })
        );
    }

    #[test]
    fn progress() {
        assert_eq!(
            parse("@progress 10 100"),
            Some(Msg::Progress {
                done: 10,
                total: Some(100)
            })
        );
        assert_eq!(parse("@progress 10 -"), Some(Msg::Progress { done: 10, total: None }));
        assert_eq!(parse("@progress 10 0"), Some(Msg::Progress { done: 10, total: None }));
    }

    #[test]
    fn done_and_result() {
        assert_eq!(parse("@done verify"), Some(Msg::Done { id: "verify".into() }));
        assert_eq!(parse("@touched"), Some(Msg::Touched));
        assert_eq!(
            parse("@result ok"),
            Some(Msg::Result {
                ok: true,
                reason: String::new()
            })
        );
        assert_eq!(
            parse("@result failed the stick was removed"),
            Some(Msg::Result {
                ok: false,
                reason: "the stick was removed".into()
            })
        );
    }

    #[test]
    fn not_protocol() {
        for l in [
            "@step",
            "@step WRITE x",
            "@progress x 1",
            "@result maybe",
            "plain text",
            "@progress 18446744073709551616 -",
            "@done",
            "@bogus x",
            "@touched now",
        ] {
            assert_eq!(parse(l), None, "{l}");
        }
    }

    #[test]
    fn long_text_is_cut() {
        let line = format!("@result failed {}", "x".repeat(2000));
        match parse(&line) {
            Some(Msg::Result { reason, .. }) => assert_eq!(reason.chars().count(), TEXT_MAX),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn id_length_limit() {
        assert!(parse(&format!("@done {}", "a".repeat(32))).is_some());
        assert!(parse(&format!("@done {}", "a".repeat(33))).is_none());
    }
}
