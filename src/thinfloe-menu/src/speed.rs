//! Write rate, time left and progress texts for the Upgrade screen.
//!
//! Pure: the caller passes the clock in, so tests never sleep.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::drives::format_size;

/// The rate is measured over the samples of this long a stretch.
pub const WINDOW: Duration = Duration::from_secs(10);
/// No rate is shown until the samples span at least this long.
pub const MIN_SPAN: Duration = Duration::from_secs(3);
/// Most samples kept, whatever the call rate.
pub const SAMPLES_MAX: usize = 64;

#[derive(Debug, Default)]
pub struct Speed {
    samples: VecDeque<(Instant, u64)>,
}

impl Speed {
    pub fn new() -> Speed {
        Speed::default()
    }

    /// Records `done` bytes at `now`. A `done` below the last sample means a
    /// new counter (the download after the save step): start over.
    pub fn sample(&mut self, now: Instant, done: u64) {
        if let Some(&(_, last)) = self.samples.back() {
            if done < last {
                self.samples.clear();
            }
        }
        self.samples.push_back((now, done));
        while self.samples.len() > SAMPLES_MAX {
            self.samples.pop_front();
        }
        while let Some(&(t, _)) = self.samples.front() {
            if now.saturating_duration_since(t) > WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Bytes per second over the samples within the window, `None` until
    /// they span `MIN_SPAN`.
    pub fn rate(&self) -> Option<f64> {
        let &(t0, d0) = self.samples.front()?;
        let &(t1, d1) = self.samples.back()?;
        let span = t1.saturating_duration_since(t0);
        if span < MIN_SPAN {
            return None;
        }
        Some((d1 - d0) as f64 / span.as_secs_f64())
    }

    /// Time left for the rest of `total`; `None` without a total, without a
    /// rate (or a zero one), or when `done` is past the total.
    pub fn eta(&self, done: u64, total: Option<u64>) -> Option<Duration> {
        let total = total?;
        if done > total {
            return None;
        }
        let rate = self.rate()?;
        if rate <= 0.0 {
            return None;
        }
        Some(Duration::from_secs_f64((total - done) as f64 / rate))
    }
}

/// Bar fraction in 0..=1; `None` without a total or when `done` is past it.
pub fn fraction(done: u64, total: Option<u64>) -> Option<f32> {
    let total = total?;
    if done > total {
        return None;
    }
    if total == 0 {
        return Some(1.0);
    }
    Some((done as f64 / total as f64) as f32)
}

/// "12.3 MB/s" (decimal megabytes, like the sizes).
pub fn rate_text(bytes_per_s: f64) -> String {
    format!("{:.1} MB/s", bytes_per_s.max(0.0) / 1e6)
}

/// "less than a minute left", "about 4 min left", "about 1 h 05 min left".
/// Minutes are rounded up.
pub fn eta_text(d: Duration) -> String {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    if secs < 60 {
        return "less than a minute left".to_string();
    }
    let mins = secs.div_ceil(60);
    if mins < 60 {
        format!("about {mins} min left")
    } else {
        format!("about {} h {:02} min left", mins / 60, mins % 60)
    }
}

/// "1.2 GB of 3.1 GB", or "1.2 GB written" without a usable total.
pub fn bytes_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if done <= t => format!("{} of {}", format_size(done), format_size(t)),
        _ => format!("{} written", format_size(done)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1_000_000;

    fn at(base: Instant, s: u64) -> Instant {
        base + Duration::from_secs(s)
    }

    #[test]
    fn rate_follows_the_last_ten_seconds() {
        let base = Instant::now();
        let mut s = Speed::new();
        for t in 0..=20 {
            s.sample(at(base, t), t * 10 * MB);
        }
        let r = s.rate().unwrap();
        assert!((r - 10.0 * MB as f64).abs() < 1.0);
        let mut done = 200 * MB;
        for t in 21..=25 {
            done += 2 * MB;
            s.sample(at(base, t), done);
        }
        let r = s.rate().unwrap();
        assert!(r > 2.0 * MB as f64 && r < 10.0 * MB as f64);
        for t in 26..=35 {
            done += 2 * MB;
            s.sample(at(base, t), done);
        }
        let r = s.rate().unwrap();
        assert!((r - 2.0 * MB as f64).abs() < 0.05 * 2.0 * MB as f64, "{r}");
    }

    #[test]
    fn no_rate_before_three_seconds() {
        let base = Instant::now();
        let mut s = Speed::new();
        assert!(s.rate().is_none());
        s.sample(at(base, 0), 0);
        s.sample(at(base, 2), 20 * MB);
        assert!(s.rate().is_none());
        s.sample(at(base, 3), 30 * MB);
        assert!(s.rate().is_some());
    }

    #[test]
    fn a_counter_that_restarts_starts_over() {
        let base = Instant::now();
        let mut s = Speed::new();
        for t in 0..=5 {
            s.sample(at(base, t), t * 100 * MB);
        }
        assert!(s.rate().is_some());
        s.sample(at(base, 6), 0);
        assert!(s.rate().is_none());
        for t in 7..=10 {
            s.sample(at(base, t), (t - 6) * 5 * MB);
        }
        let r = s.rate().unwrap();
        assert!((r - 5.0 * MB as f64).abs() < 1.0);
    }

    #[test]
    fn unknown_or_overflowing_total_shows_no_percentage() {
        let base = Instant::now();
        let mut s = Speed::new();
        for t in 0..=5 {
            s.sample(at(base, t), t * MB);
        }
        assert_eq!(fraction(5, None), None);
        assert_eq!(fraction(11, Some(10)), None);
        assert_eq!(s.eta(5, None), None);
        assert_eq!(s.eta(11, Some(10)), None);
        assert!(s.eta(5 * MB, Some(100 * MB)).is_some());
        assert_eq!(bytes_text(11, Some(10)), "0 MB written");
        assert!(!bytes_text(11 * MB, Some(10 * MB)).contains(" of "));
        assert_eq!(fraction(5, Some(10)), Some(0.5));
        assert_eq!(fraction(10, Some(10)), Some(1.0));
    }

    #[test]
    fn texts() {
        assert_eq!(rate_text(12.34e6), "12.3 MB/s");
        assert_eq!(rate_text(0.0), "0.0 MB/s");
        let s = |n| Duration::from_secs(n);
        assert_eq!(eta_text(s(0)), "less than a minute left");
        assert_eq!(eta_text(s(59)), "less than a minute left");
        assert_eq!(eta_text(s(60)), "about 1 min left");
        assert_eq!(eta_text(s(190)), "about 4 min left");
        assert_eq!(eta_text(s(3599)), "about 1 h 00 min left");
        assert_eq!(eta_text(s(3900)), "about 1 h 05 min left");
        assert_eq!(eta_text(s(3901)), "about 1 h 06 min left");
        assert_eq!(bytes_text(1_200_000_000, Some(3_100_000_000)), "1.2 GB of 3.1 GB");
        assert_eq!(bytes_text(1_200_000_000, None), "1.2 GB written");
        assert_eq!(bytes_text(400 * MB, Some(3_100_000_000)), "400 MB of 3.1 GB");
    }

    #[test]
    fn samples_are_bounded() {
        let base = Instant::now();
        let mut s = Speed::new();
        for i in 0..10_000u64 {
            s.sample(base + Duration::from_millis(i), i);
        }
        assert!(s.samples.len() <= SAMPLES_MAX);
    }
}
