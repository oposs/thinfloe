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
    /// The devices that hold that text: the one it came from and those it
    /// was sent to. They are not sent it again.
    holders: Vec<usize>,
}

impl ClipSync {
    /// ThinFloe's selection changed: the device with the pointer gets the
    /// text, every other device learns of the grab. A device that holds
    /// the same bytes already (it gave them) gets neither.
    pub fn on_local(&mut self, text: Vec<u8>, active: Option<usize>, live: &[(usize, bool)]) -> Vec<ClipAct> {
        if self.text.as_ref() != Some(&text) {
            self.holders.clear();
        }
        let mut acts = Vec::new();
        if let Some(a) = active
            && !self.holders.contains(&a)
            && live.contains(&(a, true))
        {
            acts.push(ClipAct::Send {
                conn: a,
                text: text.clone(),
            });
            self.holders.push(a);
        }
        acts.extend(
            live.iter()
                .filter(|&&(c, on)| on && Some(c) != active && !self.holders.contains(&c))
                .map(|&(conn, _)| ClipAct::Grab { conn }),
        );
        self.text = Some(text);
        acts
    }

    /// The pointer entered a device: it gets ThinFloe's text, unless it
    /// holds it already.
    pub fn on_enter(&mut self, conn: usize, clipboard: bool) -> Vec<ClipAct> {
        match &self.text {
            Some(text) if clipboard && !self.holders.contains(&conn) => {
                self.holders.push(conn);
                vec![ClipAct::Send {
                    conn,
                    text: text.clone(),
                }]
            }
            _ => Vec::new(),
        }
    }

    /// A device sent its text: it becomes ThinFloe's selection, and that
    /// device is the only one holding it.
    pub fn on_remote(&mut self, conn: usize, text: Vec<u8>) -> Vec<ClipAct> {
        self.text = Some(text.clone());
        self.holders = vec![conn];
        vec![ClipAct::SetLocal(text)]
    }

    /// A device took the clipboard: the others learn of it. None holds
    /// ThinFloe's text now (a Deskflow client empties its clipboard on a
    /// grab), so the next enter sends it again.
    pub fn on_grabbed(&mut self, conn: usize, live: &[(usize, bool)]) -> Vec<ClipAct> {
        self.holders.clear();
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
        // The device holds it now: the next enter sends nothing.
        assert!(s.on_enter(1, true).is_empty());
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
        assert!(s.on_enter(1, true).is_empty());
    }

    #[test]
    fn a_grab_or_remote_text_clears_what_a_device_holds() {
        let mut s = ClipSync::default();
        let live = [(1, true), (2, true)];
        s.on_local(t("x"), None, &live);
        assert_eq!(s.on_enter(1, true), [ClipAct::Send { conn: 1, text: t("x") }]);
        assert!(s.on_enter(1, true).is_empty());
        // Device 1 takes the clipboard: its copy of "x" is gone.
        s.on_grabbed(1, &live);
        assert_eq!(s.on_enter(1, true), [ClipAct::Send { conn: 1, text: t("x") }]);
        // Device 2's text: device 1 no longer holds ThinFloe's text.
        s.on_remote(2, t("y"));
        assert_eq!(s.on_enter(1, true), [ClipAct::Send { conn: 1, text: t("y") }]);
        assert!(s.on_enter(2, true).is_empty());
    }

    #[test]
    fn grab_from_one_device_reaches_the_others() {
        let mut s = ClipSync::default();
        let acts = s.on_grabbed(2, &[(1, true), (2, true), (3, true)]);
        assert_eq!(acts, [ClipAct::Grab { conn: 1 }, ClipAct::Grab { conn: 3 }]);
    }
}
