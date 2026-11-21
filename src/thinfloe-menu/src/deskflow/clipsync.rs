//! The clipboard policy of the Deskflow daemon, without IO: who gets
//! ThinFloe's text, who only learns that ThinFloe took the clipboard, and
//! which device text becomes ThinFloe's selection. `conn` is the
//! daemon's connection id ([`super::conn::Conn::id`]), stable for the
//! life of a connection; `live` lists the trusted connections with their
//! device's Clipboard switch.

/// What the daemon does for the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipAct {
    /// CCLP and DCLP: the device gets this text.
    Send { conn: usize, text: Vec<u8> },
    /// CCLP alone: the device learns that someone else owns the clipboard.
    Grab { conn: usize },
    /// The text becomes ThinFloe's selection.
    SetLocal(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct ClipSync {
    /// ThinFloe's text as last seen, local or from a device.
    text: Option<Vec<u8>>,
    /// The device that text came from: it has it already.
    from: Option<usize>,
}

impl ClipSync {
    /// ThinFloe's selection changed: the device with the pointer gets the
    /// text, every other device learns of the grab. The same bytes a device
    /// just gave are not sent back to it.
    pub fn on_local(&mut self, text: Vec<u8>, active: Option<usize>, live: &[(usize, bool)]) -> Vec<ClipAct> {
        if self.text.as_ref() != Some(&text) {
            self.from = None;
        }
        let mut acts = Vec::new();
        if let Some(a) = active
            && Some(a) != self.from
            && live.contains(&(a, true))
        {
            acts.push(ClipAct::Send {
                conn: a,
                text: text.clone(),
            });
        }
        acts.extend(
            live.iter()
                .filter(|&&(c, on)| on && Some(c) != active && Some(c) != self.from)
                .map(|&(conn, _)| ClipAct::Grab { conn }),
        );
        self.text = Some(text);
        acts
    }

    /// The pointer entered a device: it gets ThinFloe's text, unless it
    /// gave it.
    pub fn on_enter(&mut self, conn: usize, clipboard: bool) -> Vec<ClipAct> {
        match &self.text {
            Some(text) if clipboard && self.from != Some(conn) => vec![ClipAct::Send {
                conn,
                text: text.clone(),
            }],
            _ => Vec::new(),
        }
    }

    /// A device sent its text: it becomes ThinFloe's selection.
    pub fn on_remote(&mut self, conn: usize, text: Vec<u8>) -> Vec<ClipAct> {
        self.text = Some(text.clone());
        self.from = Some(conn);
        vec![ClipAct::SetLocal(text)]
    }

    /// A device took the clipboard: the others learn of it.
    pub fn on_grabbed(&mut self, conn: usize, live: &[(usize, bool)]) -> Vec<ClipAct> {
        live.iter()
            .filter(|&&(c, on)| on && c != conn)
            .map(|&(conn, _)| ClipAct::Grab { conn })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    #[test]
    fn enter_sends_remembered_text() {
        let mut s = ClipSync::default();
        s.on_local(t("hello"), None, &[(1, true)]);
        assert_eq!(
            s.on_enter(1, true),
            [ClipAct::Send {
                conn: 1,
                text: t("hello")
            }]
        );
        // Each enter sends it again: the device may have changed its own.
        assert_eq!(
            s.on_enter(1, true),
            [ClipAct::Send {
                conn: 1,
                text: t("hello")
            }]
        );
    }

    #[test]
    fn enter_without_text_sends_nothing() {
        let mut s = ClipSync::default();
        assert!(s.on_enter(1, true).is_empty());
    }

    #[test]
    fn local_change_goes_to_active_and_grab_to_others() {
        let mut s = ClipSync::default();
        let acts = s.on_local(t("x"), Some(2), &[(1, true), (2, true), (3, true)]);
        assert_eq!(
            acts,
            [
                ClipAct::Send { conn: 2, text: t("x") },
                ClipAct::Grab { conn: 1 },
                ClipAct::Grab { conn: 3 },
            ]
        );
        // Without a capture every device only learns of the grab.
        let acts = s.on_local(t("y"), None, &[(1, true), (2, true)]);
        assert_eq!(acts, [ClipAct::Grab { conn: 1 }, ClipAct::Grab { conn: 2 }]);
    }

    #[test]
    fn switch_off_device_gets_nothing() {
        let mut s = ClipSync::default();
        let acts = s.on_local(t("x"), Some(2), &[(1, false), (2, false), (3, true)]);
        assert_eq!(acts, [ClipAct::Grab { conn: 3 }]);
        assert!(s.on_enter(2, false).is_empty());
        assert_eq!(s.on_grabbed(3, &[(1, false), (2, false), (3, true)]), []);
    }

    #[test]
    fn remote_text_sets_local() {
        let mut s = ClipSync::default();
        assert_eq!(s.on_remote(1, t("from laptop")), [ClipAct::SetLocal(t("from laptop"))]);
        // Remembered: the next device that is entered gets it.
        assert_eq!(
            s.on_enter(2, true),
            [ClipAct::Send {
                conn: 2,
                text: t("from laptop")
            }]
        );
    }

    #[test]
    fn own_selection_is_not_sent_back() {
        let mut s = ClipSync::default();
        s.on_remote(1, t("same"));
        // The same bytes offered again on ThinFloe: device 1 has them.
        let acts = s.on_local(t("same"), Some(1), &[(1, true), (2, true)]);
        assert_eq!(acts, [ClipAct::Grab { conn: 2 }]);
        assert!(s.on_enter(1, true).is_empty());
        // A new text on ThinFloe reaches device 1 again.
        let acts = s.on_local(t("new"), Some(1), &[(1, true), (2, true)]);
        assert_eq!(
            acts,
            [
                ClipAct::Send {
                    conn: 1,
                    text: t("new")
                },
                ClipAct::Grab { conn: 2 }
            ]
        );
        assert_eq!(
            s.on_enter(1, true),
            [ClipAct::Send {
                conn: 1,
                text: t("new")
            }]
        );
    }

    #[test]
    fn grab_from_one_device_reaches_the_others() {
        let mut s = ClipSync::default();
        let acts = s.on_grabbed(2, &[(1, true), (2, true), (3, true)]);
        assert_eq!(acts, [ClipAct::Grab { conn: 1 }, ClipAct::Grab { conn: 3 }]);
    }
}
