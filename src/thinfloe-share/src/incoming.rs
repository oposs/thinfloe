//! A transfer to this machine between the accepted offer and its end: what the state file's
//! `receiving` lines say, and the log line of each step.

use owly_airdrop::proto::messages::Offer;

/// The sender's name when the phone gave none.
const SENDER_FALLBACK: &str = "iPhone";

/// One accepted offer that has not ended yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub sender: String,
    /// Files, not folders.
    pub files: u32,
    pub bytes: u64,
    /// The name of the first file, as the offer gives it.
    pub first: String,
    /// When the offer was accepted, seconds since the epoch.
    pub since: u64,
}

/// Bytes an `/Ask` offers; a file without a size counts 0 (the tmpfs still caps it).
pub fn ask_total(offer: &Offer) -> u64 {
    match offer {
        Offer::Files(files) => files
            .iter()
            .fold(0u64, |t, f| t.saturating_add(f.size.unwrap_or(0))),
        Offer::Urls(_) => 0,
    }
}

/// The transfer an offer of `sender` announces.
pub fn from_offer(sender: &str, offer: &Offer, since: u64) -> Incoming {
    let files: Vec<_> = match offer {
        Offer::Files(f) => f.iter().filter(|f| !f.is_directory).collect(),
        Offer::Urls(_) => Vec::new(),
    };
    Incoming {
        sender: if sender.trim().is_empty() {
            SENDER_FALLBACK.to_string()
        } else {
            sender.to_string()
        },
        files: u32::try_from(files.len()).unwrap_or(u32::MAX),
        bytes: ask_total(offer),
        first: files.first().map(|f| f.name.clone()).unwrap_or_default(),
        since,
    }
}

impl Incoming {
    /// The transfer of an id no offer was seen for.
    pub fn unknown() -> Incoming {
        Incoming {
            sender: SENDER_FALLBACK.to_string(),
            files: 0,
            bytes: 0,
            first: String::new(),
            since: 0,
        }
    }

    fn what(&self) -> String {
        let files = if self.files == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", self.files)
        };
        format!("{files}, {} bytes", self.bytes)
    }

    pub fn offer_line(&self) -> String {
        format!("offer from {:?}: {}", self.sender, self.what())
    }

    pub fn accepted_line(&self) -> String {
        format!("accepted the offer from {:?}", self.sender)
    }

    pub fn declined_line(&self, free: u64) -> String {
        format!(
            "declined the offer from {:?}: {}, {free} bytes free",
            self.sender,
            self.what()
        )
    }

    pub fn received_line(&self) -> String {
        format!("received {} from {:?}", self.what(), self.sender)
    }

    pub fn failed_line(&self, why: &str) -> String {
        format!(
            "receiving {} from {:?} failed: {why}",
            self.what(),
            self.sender
        )
    }

    pub fn withdrawn_line(&self) -> String {
        format!("the offer from {:?} was withdrawn", self.sender)
    }
}

/// The log line of a links offer, which is always declined.
pub fn links_line(sender: &str) -> String {
    format!("links from {sender:?} declined")
}

#[cfg(test)]
mod tests {
    use super::*;
    use owly_airdrop::proto::messages::AskFile;

    fn entry(name: &str, size: Option<u64>, dir: bool) -> AskFile {
        AskFile {
            name: name.into(),
            file_type: if dir { "public.folder" } else { "public.jpeg" }.into(),
            size,
            bom_path: format!("./{name}"),
            is_directory: dir,
        }
    }

    #[test]
    fn ask_total_sums_sizes() {
        let offer = Offer::Files(vec![
            entry("a.jpg", Some(10), false),
            entry("b.jpg", None, false),
            entry("c.jpg", Some(5), false),
        ]);
        assert_eq!(ask_total(&offer), 15);
        let huge = Offer::Files(vec![
            entry("a", Some(u64::MAX), false),
            entry("b", Some(1), false),
        ]);
        assert_eq!(ask_total(&huge), u64::MAX);
        assert_eq!(ask_total(&Offer::Urls(vec!["x".into()])), 0);
    }

    #[test]
    fn an_offer_counts_files_not_folders() {
        let offer = Offer::Files(vec![
            entry("Album", None, true),
            entry("Album/a.jpg", Some(10), false),
            entry("Album/b.jpg", Some(5), false),
        ]);
        let i = from_offer("Jane's iPhone", &offer, 7);
        assert_eq!(
            i,
            Incoming {
                sender: "Jane's iPhone".into(),
                files: 2,
                bytes: 15,
                first: "Album/a.jpg".into(),
                since: 7,
            }
        );
        assert_eq!(from_offer(" ", &offer, 7).sender, "iPhone");
    }

    #[test]
    fn the_log_lines_name_sender_files_and_bytes() {
        let mut i = Incoming {
            sender: "Jane's iPhone".into(),
            files: 2,
            bytes: 75000000,
            first: "IMG_0042.DNG".into(),
            since: 1,
        };
        assert_eq!(
            i.offer_line(),
            "offer from \"Jane's iPhone\": 2 files, 75000000 bytes"
        );
        assert_eq!(
            i.accepted_line(),
            "accepted the offer from \"Jane's iPhone\""
        );
        assert_eq!(
            i.declined_line(1000),
            "declined the offer from \"Jane's iPhone\": 2 files, 75000000 bytes, 1000 bytes free"
        );
        assert_eq!(
            i.received_line(),
            "received 2 files, 75000000 bytes from \"Jane's iPhone\""
        );
        assert_eq!(
            i.failed_line("reset"),
            "receiving 2 files, 75000000 bytes from \"Jane's iPhone\" failed: reset"
        );
        assert_eq!(
            i.withdrawn_line(),
            "the offer from \"Jane's iPhone\" was withdrawn"
        );
        i.files = 1;
        assert_eq!(
            i.offer_line(),
            "offer from \"Jane's iPhone\": 1 file, 75000000 bytes"
        );
        assert_eq!(
            links_line("Jane's iPhone"),
            "links from \"Jane's iPhone\" declined"
        );
    }
}
