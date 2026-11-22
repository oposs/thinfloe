//! Which failed connections the daemon logs. A connection that ends
//! before its client said its name (TLS, the Hello) never reaches the state
//! file, so the log is the only place it shows. A Deskflow client tries
//! again every few seconds, so the same failure from the same address is
//! logged once per `REPEAT_AFTER`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// How long the same failure from the same address stays quiet.
pub const REPEAT_AFTER: Duration = Duration::from_secs(300);
/// The most failures remembered; more addresses than this are logged
/// every time rather than grow the table.
pub const REMEMBERED: usize = 64;

#[derive(Debug, Default)]
pub struct FailLog {
    last: HashMap<(Option<IpAddr>, String), Instant>,
}

impl FailLog {
    /// Whether the failure `why` of a connection from `peer` is to be
    /// logged now.
    pub fn should_log(&mut self, peer: Option<IpAddr>, why: &str, now: Instant) -> bool {
        let key = (peer, why.to_string());
        if let Some(t) = self.last.get(&key)
            && now.duration_since(*t) < REPEAT_AFTER
        {
            return false;
        }
        self.last.retain(|_, t| now.duration_since(*t) < REPEAT_AFTER);
        if self.last.len() < REMEMBERED {
            self.last.insert(key, now);
        }
        true
    }
}

/// The log line for a connection that ended before its client was named.
pub fn line(peer: Option<IpAddr>, why: &str) -> String {
    match peer {
        Some(ip) => format!("a connection from {ip} ended before pairing: {why}"),
        None => format!("a connection ended before pairing: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn a_repeated_failure_is_logged_once_per_interval() {
        let mut f = FailLog::default();
        let t = Instant::now();
        assert!(f.should_log(ip("192.0.2.1"), "TLS: x", t));
        assert!(!f.should_log(ip("192.0.2.1"), "TLS: x", t + Duration::from_secs(3)));
        assert!(!f.should_log(ip("192.0.2.1"), "TLS: x", t + REPEAT_AFTER - Duration::from_secs(1)));
        assert!(f.should_log(ip("192.0.2.1"), "TLS: x", t + REPEAT_AFTER));
    }

    #[test]
    fn another_address_or_reason_is_logged_at_once() {
        let mut f = FailLog::default();
        let t = Instant::now();
        assert!(f.should_log(ip("192.0.2.1"), "TLS: x", t));
        assert!(f.should_log(ip("192.0.2.2"), "TLS: x", t));
        assert!(f.should_log(ip("192.0.2.1"), "no Hello", t));
        assert!(f.should_log(None, "TLS: x", t));
    }

    #[test]
    fn the_table_stays_bounded() {
        let mut f = FailLog::default();
        let t = Instant::now();
        for i in 0..1000u32 {
            assert!(f.should_log(Some(IpAddr::from(i.to_be_bytes())), "TLS: x", t));
        }
        assert_eq!(f.last.len(), REMEMBERED);
        // Past the interval the old entries make room again.
        assert!(f.should_log(ip("192.0.2.9"), "TLS: x", t + REPEAT_AFTER));
        assert!(!f.should_log(ip("192.0.2.9"), "TLS: x", t + REPEAT_AFTER));
    }

    #[test]
    fn the_line_names_the_address_and_the_reason() {
        assert_eq!(
            line(ip("192.0.2.1"), "TLS: invalid peer certificate"),
            "a connection from 192.0.2.1 ended before pairing: TLS: invalid peer certificate"
        );
        assert_eq!(line(None, "no Hello"), "a connection ended before pairing: no Hello");
    }
}
