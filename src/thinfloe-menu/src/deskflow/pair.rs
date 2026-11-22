//! The trust decision for a connecting device and the list of devices that
//! wait for the user (spec, Identity and pairing). Sans-IO: the caller
//! passes the time in.

use crate::config::DeviceCfg;

/// At most this many pending devices are kept; the oldest is dropped.
pub const MAX_PENDING: usize = 8;
/// At most this many rejections are remembered; the oldest is forgotten.
/// A noisy LAN cannot grow the list without bound.
pub const MAX_REJECTED: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The (name, fingerprint) pair is attached; the index into the devices.
    Trusted(usize),
    /// Unknown name: waits for the user.
    Pending,
    /// The name is attached with another fingerprint.
    FingerprintChanged,
}

/// Decides on a connecting device by exact name and fingerprint.
pub fn judge(devices: &[DeviceCfg], name: &str, fp: &str) -> Verdict {
    if let Some(i) = devices.iter().position(|d| d.name == name && d.fingerprint == fp) {
        Verdict::Trusted(i)
    } else if devices.iter().any(|d| d.name == name) {
        Verdict::FingerprintChanged
    } else {
        Verdict::Pending
    }
}

/// Why a device waits: the flags argument of [`Pending::note`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// The name is not attached.
    New,
    /// The name is attached with another fingerprint (state file flag
    /// `fp-changed`).
    FingerprintChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEntry {
    pub name: String,
    pub fingerprint: String,
    pub kind: PendingKind,
    /// When the pair was first noted, seconds since the epoch.
    pub since_epoch: u64,
}

#[derive(Default)]
pub struct Pending {
    entries: Vec<PendingEntry>,
    rejected: Vec<(String, String)>,
}

impl Pending {
    /// Records a pair that connected and was not trusted. A rejected pair is
    /// not noted again; a pair already listed keeps its place and time and
    /// takes the new kind.
    pub fn note(&mut self, name: &str, fp: &str, kind: PendingKind, now_epoch: u64) {
        if self.is_rejected(name, fp) {
            return;
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == name && e.fingerprint == fp) {
            e.kind = kind;
            return;
        }
        if self.entries.len() >= MAX_PENDING {
            self.entries.remove(0);
        }
        self.entries.push(PendingEntry {
            name: name.to_string(),
            fingerprint: fp.to_string(),
            kind,
            since_epoch: now_epoch,
        });
    }

    /// The user rejects a pair: it leaves the list and is not noted again
    /// for this session.
    pub fn reject(&mut self, name: &str, fp: &str) {
        self.entries.retain(|e| !(e.name == name && e.fingerprint == fp));
        if self.is_rejected(name, fp) {
            return;
        }
        if self.rejected.len() >= MAX_REJECTED {
            self.rejected.remove(0);
        }
        self.rejected.push((name.to_string(), fp.to_string()));
    }

    pub fn is_rejected(&self, name: &str, fp: &str) -> bool {
        self.rejected.iter().any(|(n, f)| n == name && f == fp)
    }

    /// Oldest first.
    pub fn entries(&self) -> &[PendingEntry] {
        &self.entries
    }

    /// The rejected pairs of this session, oldest first.
    pub fn rejected(&self) -> &[(String, String)] {
        &self.rejected
    }

    /// Drops the entries that the devices now trust (after an attach).
    pub fn forget_trusted(&mut self, devices: &[DeviceCfg]) {
        self.entries.retain(|e| {
            !devices
                .iter()
                .any(|d| d.name == e.name && d.fingerprint == e.fingerprint)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanshi::Side;

    fn dev(name: &str, fp: &str) -> DeviceCfg {
        DeviceCfg {
            name: name.into(),
            fingerprint: fp.into(),
            output: "DP-1".into(),
            side: Side::RightOf,
            clipboard: true,
            compose: false,
        }
    }

    #[test]
    fn judge_known_pending_changed() {
        let devs = [dev("laptop", "fp1"), dev("tablet", "fp2")];
        assert_eq!(judge(&devs, "tablet", "fp2"), Verdict::Trusted(1));
        assert_eq!(judge(&devs, "phone", "fp9"), Verdict::Pending);
        assert_eq!(judge(&devs, "laptop", "fpX"), Verdict::FingerprintChanged);
        // The fingerprint of another device does not make a name known.
        assert_eq!(judge(&devs, "phone", "fp1"), Verdict::Pending);
    }

    #[test]
    fn pending_keeps_eight_newest() {
        let mut p = Pending::default();
        for i in 0..10 {
            p.note(&format!("d{i}"), "fp", PendingKind::New, 100 + i);
        }
        let names: Vec<_> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["d2", "d3", "d4", "d5", "d6", "d7", "d8", "d9"]);
        assert_eq!(p.entries()[0].since_epoch, 102);
    }

    #[test]
    fn noting_again_keeps_place_and_time() {
        let mut p = Pending::default();
        p.note("a", "f", PendingKind::New, 1);
        p.note("b", "f", PendingKind::New, 2);
        p.note("a", "f", PendingKind::FingerprintChanged, 3);
        assert_eq!(p.entries().len(), 2);
        assert_eq!(p.entries()[0].name, "a");
        assert_eq!(p.entries()[0].since_epoch, 1);
        assert_eq!(p.entries()[0].kind, PendingKind::FingerprintChanged);
    }

    #[test]
    fn rejected_not_noted_again() {
        let mut p = Pending::default();
        p.note("a", "f", PendingKind::New, 1);
        p.reject("a", "f");
        assert!(p.entries().is_empty());
        assert!(p.is_rejected("a", "f"));
        p.note("a", "f", PendingKind::New, 2);
        assert!(p.entries().is_empty());
        // Another fingerprint under the same name is a new pair.
        p.note("a", "g", PendingKind::New, 3);
        assert_eq!(p.entries().len(), 1);
        assert!(!p.is_rejected("a", "g"));
    }

    #[test]
    fn rejections_are_bounded() {
        let mut p = Pending::default();
        for i in 0..(MAX_REJECTED + 5) {
            p.reject(&format!("d{i}"), "f");
        }
        assert_eq!(p.rejected().len(), MAX_REJECTED);
        assert!(!p.is_rejected("d0", "f"));
        assert!(p.is_rejected(&format!("d{}", MAX_REJECTED + 4), "f"));
    }

    #[test]
    fn forget_trusted_drops_attached_pairs() {
        let mut p = Pending::default();
        p.note("a", "f", PendingKind::New, 1);
        p.note("b", "g", PendingKind::New, 2);
        p.forget_trusted(&[dev("a", "f"), dev("b", "other")]);
        assert_eq!(p.entries().len(), 1);
        assert_eq!(p.entries()[0].name, "b");
    }
}
