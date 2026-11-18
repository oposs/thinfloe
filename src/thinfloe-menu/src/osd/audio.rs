//! Volume and microphone mute through `wpctl` (PipeWire). The keys run on
//! a worker thread (`AudioWorker`), so a slow `wpctl` never stalls the OSD;
//! presses that arrive while it runs are merged.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";
/// How long one `wpctl` call may take.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The default sink's volume and mute state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub percent: u8,
    pub muted: bool,
}

/// `wpctl get-volume` prints `Volume: 0.45`, with ` [MUTED]` when muted.
/// Anything else (`Translate ID error ...` without a device) is `None`.
pub fn parse_get_volume(out: &str) -> Option<Level> {
    let rest = out.lines().find_map(|l| l.trim().strip_prefix("Volume:"))?;
    let mut words = rest.split_whitespace();
    let volume: f64 = words.next()?.parse().ok()?;
    if !volume.is_finite() || volume < 0.0 {
        return None;
    }
    Some(Level {
        percent: (volume * 100.0).round().min(f64::from(u8::MAX)) as u8,
        muted: words.any(|w| w == "[MUTED]"),
    })
}

pub struct Wpctl {
    cmd: Vec<String>,
    timeout: Duration,
}

impl Wpctl {
    /// `TF_WPCTL` split on whitespace, `wpctl` when unset or empty.
    pub fn from_env() -> Wpctl {
        Wpctl {
            cmd: super::cmd_from_env("TF_WPCTL", "wpctl"),
            timeout: TIMEOUT,
        }
    }

    /// Raise or lower the default sink by `delta_pct` percent, at most to
    /// 100 %; the mute state stays as it is. Returns the new level.
    pub fn change(&self, delta_pct: i32) -> Result<Level, String> {
        if delta_pct != 0 {
            let sign = if delta_pct > 0 { '+' } else { '-' };
            let amount = format!("{}%{sign}", delta_pct.unsigned_abs());
            self.run(&["set-volume", "-l", "1.0", SINK, &amount])?;
        }
        self.level(SINK)
    }

    pub fn toggle_sink_mute(&self) -> Result<Level, String> {
        self.run(&["set-mute", SINK, "toggle"])?;
        self.level(SINK)
    }

    /// Toggle the default source's mute; returns whether it is now muted.
    pub fn toggle_source_mute(&self) -> Result<bool, String> {
        self.run(&["set-mute", SOURCE, "toggle"])?;
        self.source_muted()
    }

    pub fn source_muted(&self) -> Result<bool, String> {
        self.level(SOURCE).map(|l| l.muted)
    }

    fn level(&self, node: &str) -> Result<Level, String> {
        let out = self.run(&["get-volume", node])?;
        parse_get_volume(&out).ok_or_else(|| format!("{} get-volume {node} printed no volume", self.cmd[0]))
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        super::run_tool(&self.cmd, &args, self.timeout)
    }
}

pub enum AudioRequest {
    /// Change the volume by this many percent.
    Step(i32),
    ToggleMute,
    ToggleMic,
}

pub enum AudioEvent {
    Sink(Level),
    Mic(bool),
    Failed(String),
}

/// The requests not yet taken by the worker; `closed` ends it.
#[derive(Default)]
struct Queue {
    pending: VecDeque<AudioRequest>,
    closed: bool,
}

/// Runs `wpctl` on a thread of its own, one request at a time. A `Step`
/// that arrives while the previous `Step` still waits is added to it, so a
/// held key runs one `wpctl` per call, not one per press; toggles are kept
/// one by one. Dropping the worker ends the thread after its current call.
pub struct AudioWorker {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl AudioWorker {
    pub fn spawn(w: Wpctl, on_event: impl Fn(AudioEvent) + Send + 'static) -> AudioWorker {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let s = shared.clone();
        std::thread::spawn(move || {
            while let Some(r) = next(&s) {
                on_event(
                    match r {
                        AudioRequest::Step(d) => w.change(d).map(AudioEvent::Sink),
                        AudioRequest::ToggleMute => w.toggle_sink_mute().map(AudioEvent::Sink),
                        AudioRequest::ToggleMic => w.toggle_source_mute().map(AudioEvent::Mic),
                    }
                    .unwrap_or_else(AudioEvent::Failed),
                );
            }
        });
        AudioWorker { shared }
    }

    /// Queue `r` for the worker. Never blocks on `wpctl`.
    pub fn request(&self, r: AudioRequest) {
        let (m, cv) = &*self.shared;
        let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
        match (q.pending.back_mut(), r) {
            (Some(AudioRequest::Step(a)), AudioRequest::Step(b)) => *a = a.saturating_add(b),
            (_, r) => q.pending.push_back(r),
        }
        cv.notify_one();
    }
}

impl Drop for AudioWorker {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        cv.notify_one();
    }
}

/// The next request, waiting for one; `None` once the worker is dropped.
fn next(shared: &(Mutex<Queue>, Condvar)) -> Option<AudioRequest> {
    let (m, cv) = shared;
    let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if q.closed {
            return None;
        }
        if let Some(r) = q.pending.pop_front() {
            return Some(r);
        }
        q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::path::Path;
    use std::time::Instant;

    /// A fake `wpctl`: a shell script that logs its arguments to `log` and
    /// then runs `body`.
    fn fake(d: &Path, body: &str) -> Wpctl {
        let log = d.join("log");
        let script = d.join("wpctl");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Wpctl {
            cmd: vec![script.to_string_lossy().into_owned()],
            timeout: TIMEOUT,
        }
    }

    fn log(d: &Path) -> Vec<String> {
        std::fs::read_to_string(d.join("log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn get_volume_parses_level_and_mute() {
        assert_eq!(
            parse_get_volume("Volume: 0.45\n"),
            Some(Level {
                percent: 45,
                muted: false
            })
        );
        assert_eq!(
            parse_get_volume("Volume: 0.45 [MUTED]\n"),
            Some(Level {
                percent: 45,
                muted: true
            })
        );
        assert_eq!(
            parse_get_volume("Volume: 1.00\n"),
            Some(Level {
                percent: 100,
                muted: false
            })
        );
        assert_eq!(parse_get_volume("Translate ID error\n"), None);
    }

    #[test]
    fn a_failing_wpctl_is_an_error_not_a_level() {
        let d = TestDir::new("osd-audio", "fail");
        let w = fake(&d, "echo 'Translate ID error: node not found' >&2; exit 1");
        assert!(w.change(5).is_err());
        assert!(w.toggle_sink_mute().is_err());
        assert!(w.source_muted().is_err());
        // Exit 0 with no level is no level either.
        let w = fake(&d, "exit 0");
        assert!(w.change(5).is_err());
    }

    #[test]
    fn change_runs_set_volume_with_the_cap_then_get_volume() {
        let d = TestDir::new("osd-audio", "change");
        let w = fake(&d, "case \"$1\" in get-volume) echo 'Volume: 0.50';; esac");
        assert_eq!(
            w.change(5),
            Ok(Level {
                percent: 50,
                muted: false
            })
        );
        assert_eq!(
            log(&d),
            [
                "set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+",
                "get-volume @DEFAULT_AUDIO_SINK@"
            ]
        );
        w.change(-10).unwrap();
        assert_eq!(log(&d)[2], "set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 10%-");
    }

    #[test]
    fn sink_mute_toggles_the_default_sink() {
        let d = TestDir::new("osd-audio", "mute");
        let w = fake(&d, "case \"$1\" in get-volume) echo 'Volume: 0.30 [MUTED]';; esac");
        assert_eq!(
            w.toggle_sink_mute(),
            Ok(Level {
                percent: 30,
                muted: true
            })
        );
        assert_eq!(
            log(&d),
            [
                "set-mute @DEFAULT_AUDIO_SINK@ toggle",
                "get-volume @DEFAULT_AUDIO_SINK@"
            ]
        );
    }

    #[test]
    fn steps_merge_while_wpctl_runs() {
        let d = TestDir::new("osd-audio", "merge");
        let w = fake(
            &d,
            "case \"$1\" in set-volume) sleep 0.3;; get-volume) echo 'Volume: 0.50';; esac",
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let worker = AudioWorker::spawn(w, move |ev| {
            if let AudioEvent::Sink(l) = ev {
                e.lock().unwrap().push(l);
            }
        });
        for _ in 0..6 {
            worker.request(AudioRequest::Step(5));
        }
        let sets = || -> Vec<i32> {
            log(&d)
                .iter()
                .filter_map(|l| l.strip_prefix("set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ "))
                .map(|n| n.trim_end_matches("%+").parse().unwrap())
                .collect()
        };
        wait_for("the steps add up to 30 %", || sets().iter().sum::<i32>() >= 30);
        // A third run would start at once; give it room to show.
        std::thread::sleep(Duration::from_millis(400));
        let sets = sets();
        assert!(sets.len() <= 2, "{sets:?}");
        assert_eq!(sets.iter().sum::<i32>(), 30);
        wait_for("an event per run", || events.lock().unwrap().len() == sets.len());
    }

    #[test]
    fn toggles_are_not_merged() {
        let d = TestDir::new("osd-audio", "toggles");
        let w = fake(
            &d,
            "case \"$1\" in set-mute) sleep 0.2;; get-volume) echo 'Volume: 0.50';; esac",
        );
        let worker = AudioWorker::spawn(w, |_| {});
        for _ in 0..3 {
            worker.request(AudioRequest::ToggleMute);
        }
        let toggles = || log(&d).iter().filter(|l| l.starts_with("set-mute")).count();
        wait_for("three toggles", || toggles() == 3);
    }

    #[test]
    fn mic_toggle_targets_the_default_source() {
        let d = TestDir::new("osd-audio", "mic");
        let w = fake(&d, "case \"$1\" in get-volume) echo 'Volume: 1.00 [MUTED]';; esac");
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let worker = AudioWorker::spawn(w, move |ev| {
            if let AudioEvent::Mic(m) = ev {
                e.lock().unwrap().push(m);
            }
        });
        worker.request(AudioRequest::ToggleMic);
        wait_for("a Mic event", || !events.lock().unwrap().is_empty());
        assert_eq!(*events.lock().unwrap(), [true]);
        assert_eq!(
            log(&d),
            [
                "set-mute @DEFAULT_AUDIO_SOURCE@ toggle",
                "get-volume @DEFAULT_AUDIO_SOURCE@"
            ]
        );
    }

    #[test]
    fn a_failure_reaches_the_worker_as_failed() {
        let d = TestDir::new("osd-audio", "workerfail");
        let w = fake(&d, "exit 1");
        let failed = Arc::new(Mutex::new(0));
        let f = failed.clone();
        let worker = AudioWorker::spawn(w, move |ev| {
            assert!(matches!(ev, AudioEvent::Failed(_)), "a level from a failing wpctl");
            *f.lock().unwrap() += 1;
        });
        worker.request(AudioRequest::Step(5));
        wait_for("a Failed event", || *failed.lock().unwrap() == 1);
    }
}
