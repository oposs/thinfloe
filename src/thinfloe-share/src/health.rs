//! The wedge watch: owlyd logs a `STATS` line every 10 s while the radio is up; a card that
//! keeps giving up on the frames it is handed has stopped sending (seen on an RTL8812BU stick
//! on 2026-11-26: "Ready", but iPhones never saw the machine).

/// The counters of one `STATS` line that the rule looks at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// `tx_action` + `tx_unicast` + `tx_multicast`.
    pub sent: u64,
    pub radio_full: u64,
    pub gave_up: u64,
}

/// An interval is stuck when at least this many frames were given up ...
pub const STUCK_GAVE_UP: u64 = 20;
/// ... and the card is wedged after this many stuck intervals in a row (30 s).
pub const STUCK_INTERVALS: u32 = 3;

fn value(tokens: &[&str], key: &[&str]) -> Option<u64> {
    let i = tokens.windows(key.len()).position(|w| w == key)?;
    tokens.get(i + key.len())?.parse().ok()
}

/// The counters of an owlyd `STATS` line; the text after `STATS ` is read wherever it is in
/// the line, as owlyd's tracing prefix and `journalctl -o cat` vary. None for other lines.
pub fn parse_stats(line: &str) -> Option<Counters> {
    let (_, rest) = line.split_once("STATS ")?;
    let t: Vec<&str> = rest.split_whitespace().collect();
    let sent = value(&t, &["tx_action"])?
        .checked_add(value(&t, &["tx_unicast"])?)?
        .checked_add(value(&t, &["tx_multicast"])?)?;
    Some(Counters {
        sent,
        radio_full: value(&t, &["radio", "full"])?,
        gave_up: value(&t, &["gave", "up"])?,
    })
}

/// The rule over consecutive `STATS` lines.
#[derive(Debug, Default)]
pub struct Health {
    last: Option<Counters>,
    stuck: u32,
    /// Frames given up and sent over the current run of stuck intervals.
    run_gave_up: u64,
    run_sent: u64,
}

impl Health {
    /// One `STATS` line; true once the card is wedged. A counter lower than before (owlyd
    /// restarted) makes the line the new baseline and starts the count again.
    pub fn feed(&mut self, c: Counters) -> bool {
        let Some(p) = self.last.replace(c) else {
            return false;
        };
        if c.sent < p.sent || c.gave_up < p.gave_up || c.radio_full < p.radio_full {
            self.stuck = 0;
            (self.run_gave_up, self.run_sent) = (0, 0);
            return false;
        }
        let (gave_up, sent) = (c.gave_up - p.gave_up, c.sent - p.sent);
        if gave_up >= STUCK_GAVE_UP && gave_up.saturating_mul(2) >= sent {
            self.stuck += 1;
            self.run_gave_up += gave_up;
            self.run_sent += sent;
        } else {
            self.stuck = 0;
            (self.run_gave_up, self.run_sent) = (0, 0);
        }
        self.stuck >= STUCK_INTERVALS
    }

    /// Frames given up and frames sent over the stuck run.
    pub fn run(&self) -> (u64, u64) {
        (self.run_gave_up, self.run_sent)
    }
}

/// Whether an error text is the kernel's EBUSY: owlyd could not bring the radio up on a card
/// that stopped sending in an earlier session.
pub fn busy(text: &str) -> bool {
    text.contains("os error 16") || text.contains("Device or resource busy")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(sent: u64, full: u64, gave_up: u64) -> String {
        format!(
            "2026-11-26T10:00:00.000000Z  INFO owly_awdl::driver: STATS tx_action {sent} tx_unicast 0 tx_multicast 0 tx_amsdu 0 | radio full {full} gave up {gave_up} | rx_action 300 rx_data 50 rx_malformed 0 delivered 50 | dropped no_peer 0 queue 0 expired 0 | syn held 0 released 0 | no_overlap 0 | timer late avg 12 µs max 80 µs"
        )
    }

    /// Feeds lines growing by (sent, gave_up) each, from `start`; the results.
    fn feed_steps(h: &mut Health, start: (u64, u64), steps: &[(u64, u64)]) -> Vec<bool> {
        let (mut s, mut g) = start;
        steps
            .iter()
            .map(|(ds, dg)| {
                s += ds;
                g += dg;
                h.feed(parse_stats(&line(s, 0, g)).unwrap())
            })
            .collect()
    }

    const STUCK: (u64, u64) = (100, 60);
    const GOOD: (u64, u64) = (120, 2);

    #[test]
    fn stats_line_parses() {
        let want = Counters {
            sent: 120,
            radio_full: 7,
            gave_up: 3,
        };
        assert_eq!(parse_stats(&line(120, 7, 3)), Some(want));
        let bare = line(120, 7, 3);
        let bare = &bare[bare.find("STATS").unwrap()..];
        assert_eq!(parse_stats(bare), Some(want));
        assert_eq!(parse_stats("add peer aa:bb"), None);
        assert_eq!(
            parse_stats(&line(120, 7, 3).replace("gave up 3", "gave up x")),
            None
        );
    }

    #[test]
    fn a_healthy_radio_is_never_wedged() {
        let mut h = Health::default();
        assert!(!h.feed(parse_stats(&line(0, 0, 0)).unwrap()));
        assert!(feed_steps(&mut h, (0, 0), &[GOOD; 20]).iter().all(|w| !w));
    }

    #[test]
    fn three_stuck_intervals_in_a_row_are_a_wedge() {
        let mut h = Health::default();
        assert!(!h.feed(parse_stats(&line(0, 0, 0)).unwrap()));
        assert_eq!(
            feed_steps(&mut h, (0, 0), &[STUCK; 3]),
            [false, false, true]
        );
        assert_eq!(h.run(), (180, 300));
    }

    #[test]
    fn a_good_interval_resets_the_count() {
        let mut h = Health::default();
        h.feed(parse_stats(&line(0, 0, 0)).unwrap());
        assert_eq!(
            feed_steps(&mut h, (0, 0), &[STUCK, STUCK, GOOD, STUCK, STUCK, STUCK]),
            [false, false, false, false, false, true]
        );
    }

    #[test]
    fn counters_that_go_back_start_again() {
        let mut h = Health::default();
        h.feed(parse_stats(&line(1000, 0, 500)).unwrap());
        assert_eq!(
            feed_steps(&mut h, (1000, 500), &[STUCK, STUCK]),
            [false, false]
        );
        // owlyd restarted: every counter from 0.
        assert!(!h.feed(parse_stats(&line(0, 0, 0)).unwrap()));
        assert_eq!(
            feed_steps(&mut h, (0, 0), &[STUCK, STUCK, STUCK]),
            [false, false, true]
        );
    }

    #[test]
    fn few_frames_given_up_are_not_stuck() {
        let mut h = Health::default();
        h.feed(parse_stats(&line(0, 0, 0)).unwrap());
        assert!(
            feed_steps(&mut h, (0, 0), &[(19, 19); 10])
                .iter()
                .all(|w| !w)
        );
    }

    #[test]
    fn a_busy_device_is_seen() {
        assert!(busy(
            "Couldn't bring owlyibss0 up: Device or resource busy (os error 16)"
        ));
        assert!(!busy("AirDrop's radio is in use (lease 3)."));
    }
}
