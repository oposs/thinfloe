//! Monitor brightness through the `ddcutil` CLI (DDC/CI, VCP feature 10).
//!
//! DDC/CI is slow: a set takes a tenth of a second to seconds, and a monitor
//! may not answer at all. Each display has a setter thread with a single
//! slot for its target, so a held key runs at most one set at a time and
//! leaves one target waiting; once the key is let go, the last target is
//! set and nothing else runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The VCP feature code of brightness.
const BRIGHTNESS: &str = "10";
/// How long one `ddcutil` call may take.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdcDisplay {
    pub bus: u32,
    /// The DRM connector without its `card<k>-` prefix, as labwc names
    /// outputs (`DP-1`).
    pub connector: String,
    /// `Display <n>`; `Invalid display` and `Phantom display` are not.
    pub valid: bool,
}

/// The displays of `ddcutil detect --terse`.
pub fn parse_detect(out: &str) -> Vec<DdcDisplay> {
    // (valid, bus, connector) of the block being read.
    type Block = (bool, Option<u32>, String);
    fn finish(block: Option<Block>, found: &mut Vec<DdcDisplay>) {
        if let Some((valid, Some(bus), connector)) = block {
            found.push(DdcDisplay { bus, connector, valid });
        }
    }
    let mut found = Vec::new();
    let mut block: Option<Block> = None;
    for line in out.lines() {
        let l = line.trim();
        let starts_valid = l
            .strip_prefix("Display ")
            .is_some_and(|n| n.trim().parse::<u32>().is_ok());
        if starts_valid || l == "Invalid display" || l == "Phantom display" {
            finish(block.take(), &mut found);
            block = Some((starts_valid, None, String::new()));
            continue;
        }
        let Some(b) = block.as_mut() else { continue };
        if let Some(dev) = l.strip_prefix("I2C bus:") {
            b.1 = dev.trim().strip_prefix("/dev/i2c-").and_then(|n| n.parse().ok());
        } else if let Some(name) = l
            .strip_prefix("DRM connector:")
            .or_else(|| l.strip_prefix("DRM_connector:"))
        {
            b.2 = strip_card(name.trim()).to_string();
        }
    }
    finish(block, &mut found);
    found
}

/// `card1-DP-1` is `DP-1`; a name without the prefix stays as it is.
fn strip_card(name: &str) -> &str {
    name.strip_prefix("card")
        .and_then(|rest| {
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            (digits > 0).then(|| rest[digits..].strip_prefix('-')).flatten()
        })
        .unwrap_or(name)
}

/// `(current, max)` of `ddcutil --terse getvcp 10`: `VCP 10 C 60 100`.
pub fn parse_getvcp(out: &str) -> Option<(u16, u16)> {
    out.lines().find_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w[..] {
            ["VCP", code, "C", cur, max] if code.eq_ignore_ascii_case(BRIGHTNESS) => {
                Some((cur.parse().ok()?, max.parse().ok()?))
            }
            _ => None,
        }
    })
}

/// One brightness step from `cur`: a tenth of the monitor's maximum,
/// rounded, within `0..=max`.
pub fn step(cur: u16, max: u16, up: bool) -> u16 {
    let size = ((u32::from(max) + 5) / 10) as u16;
    let cur = cur.min(max);
    if up {
        cur.saturating_add(size).min(max)
    } else {
        cur.saturating_sub(size)
    }
}

/// `cur` as a percentage of `max`, rounded; 0 when `max` is 0.
pub fn percent(cur: u16, max: u16) -> u8 {
    if max == 0 {
        return 0;
    }
    let (cur, max) = (u32::from(cur.min(max)), u32::from(max));
    ((cur * 100 + max / 2) / max) as u8
}

#[derive(Clone)]
pub struct Ddcutil {
    cmd: Vec<String>,
    timeout: Duration,
}

impl Ddcutil {
    /// `TF_DDCUTIL` split on whitespace, `ddcutil` when unset or empty.
    pub fn from_env() -> Ddcutil {
        Ddcutil {
            cmd: super::cmd_from_env("TF_DDCUTIL", "ddcutil"),
            timeout: TIMEOUT,
        }
    }

    pub fn with_timeout(self, t: Duration) -> Ddcutil {
        Ddcutil { timeout: t, ..self }
    }

    pub fn detect(&self) -> Result<Vec<DdcDisplay>, String> {
        self.run(&["detect", "--terse"]).map(|out| parse_detect(&out))
    }

    pub fn get(&self, bus: u32) -> Result<(u16, u16), String> {
        let out = self.run(&["--bus", &bus.to_string(), "--terse", "getvcp", BRIGHTNESS])?;
        parse_getvcp(&out).ok_or_else(|| format!("bus {bus} has no brightness"))
    }

    pub fn set(&self, bus: u32, v: u16) -> Result<(), String> {
        self.run(&[
            "--bus",
            &bus.to_string(),
            "--noverify",
            "setvcp",
            BRIGHTNESS,
            &v.to_string(),
        ])
        .map(|_| ())
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        super::run_tool(&self.cmd, &args, self.timeout)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bright {
    Unknown,
    Unsupported,
    Percent(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdcEvent {
    Detected(Vec<(String, Bright)>),
    Level { connector: String, percent: u8 },
}

/// The target of one display's setter thread.
#[derive(Default)]
struct SlotState {
    /// The latest target, or the value read at detection.
    target: u16,
    /// A target the setter has not taken yet.
    pending: Option<u16>,
    stop: bool,
}

#[derive(Default)]
struct Slot {
    state: Mutex<SlotState>,
    wake: Condvar,
}

impl Slot {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stop(&self) {
        self.lock().stop = true;
        self.wake.notify_one();
    }
}

/// A valid display whose brightness could be read.
struct Display {
    connector: String,
    max: u16,
    slot: Arc<Slot>,
}

struct Shared {
    ddc: Ddcutil,
    on_event: Box<dyn Fn(DdcEvent) + Send + Sync>,
    displays: Mutex<Vec<Display>>,
    /// Held for a whole detection, so two never interleave.
    detecting: Mutex<()>,
    closed: AtomicBool,
}

/// The monitors whose brightness the keys change. Detection runs off the
/// caller's thread; each display then has a setter thread of its own.
/// Dropping `Monitors` ends the setter threads after their current set.
pub struct Monitors {
    shared: Arc<Shared>,
}

impl Monitors {
    pub fn spawn(d: Ddcutil, on_event: impl Fn(DdcEvent) + Send + Sync + 'static) -> Monitors {
        Monitors {
            shared: Arc::new(Shared {
                ddc: d,
                on_event: Box::new(on_event),
                displays: Mutex::new(Vec::new()),
                detecting: Mutex::new(()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Detect the displays again and read each valid one's brightness, on
    /// a thread of its own; `DdcEvent::Detected` reports the result. A
    /// failed detection counts as no displays.
    pub fn redetect(&self) {
        let shared = self.shared.clone();
        std::thread::spawn(move || detect(&shared));
    }

    /// One step up or down on every display known to support brightness.
    /// The new target is reported at once (`DdcEvent::Level`) and set by
    /// the display's setter thread; before the first detection has
    /// finished, nothing happens.
    pub fn step(&self, up: bool) {
        let mut events = Vec::new();
        for d in lock(&self.shared.displays).iter() {
            let mut st = d.slot.lock();
            let new = step(st.target, d.max, up);
            if new != st.target {
                st.target = new;
                st.pending = Some(new);
                d.slot.wake.notify_one();
            }
            events.push(DdcEvent::Level {
                connector: d.connector.clone(),
                percent: percent(new, d.max),
            });
        }
        for e in events {
            (self.shared.on_event)(e);
        }
    }
}

impl Drop for Monitors {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        for d in lock(&self.shared.displays).iter() {
            d.slot.stop();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn detect(shared: &Shared) {
    let _one = lock(&shared.detecting);
    let found = shared.ddc.detect().unwrap_or_else(|e| {
        eprintln!("thinfloe-menu --osd: {e}");
        Vec::new()
    });
    let mut report = Vec::new();
    let mut displays = Vec::new();
    for f in found {
        let bright = if !f.valid {
            Bright::Unsupported
        } else {
            match shared.ddc.get(f.bus) {
                Ok((cur, max)) => {
                    let slot = Arc::new(Slot::default());
                    slot.lock().target = cur.min(max);
                    displays.push((
                        f.bus,
                        Display {
                            connector: f.connector.clone(),
                            max,
                            slot,
                        },
                    ));
                    Bright::Percent(percent(cur, max))
                }
                Err(e) => {
                    eprintln!("thinfloe-menu --osd: {e}");
                    Bright::Unknown
                }
            }
        };
        report.push((f.connector, bright));
    }
    {
        let mut current = lock(&shared.displays);
        for d in current.iter() {
            d.slot.stop();
        }
        current.clear();
        if shared.closed.load(Ordering::SeqCst) {
            return;
        }
        for (bus, d) in displays {
            let (ddc, slot) = (shared.ddc.clone(), d.slot.clone());
            std::thread::spawn(move || set_loop(&ddc, bus, &slot));
            current.push(d);
        }
    }
    (shared.on_event)(DdcEvent::Detected(report));
}

/// A display's setter thread: set the pending target, and again while a
/// newer one waits, until the slot is stopped.
fn set_loop(ddc: &Ddcutil, bus: u32, slot: &Slot) {
    loop {
        let v = {
            let mut st = slot.lock();
            loop {
                if st.stop {
                    return;
                }
                if let Some(v) = st.pending.take() {
                    break v;
                }
                st = slot.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        if let Err(e) = ddc.set(bus, v) {
            eprintln!("thinfloe-menu --osd: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::path::Path;
    use std::time::Instant;

    const DETECT: &str = "Display 1\n   I2C bus:  /dev/i2c-6\n   DRM connector:           card1-DP-1\n   Monitor:   DEL:DELL U2720Q:ABC\n\nInvalid display\n   I2C bus:  /dev/i2c-7\n   DRM connector:           card1-HDMI-A-1\n   Monitor:   GSM:LG:XYZ\n";

    /// A fake `ddcutil` for one display on bus 6 (DP-1) at 50 of 100. It
    /// logs its arguments; `setvcp` runs `set_body` after the log line.
    fn fake(d: &Path, set_body: &str) -> Ddcutil {
        let log = d.join("log");
        let script = d.join("ddcutil");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$*\" in\n\
                 detect*) printf 'Display 1\\n   I2C bus:  /dev/i2c-6\\n   DRM_connector:  card0-DP-1\\n';;\n\
                 *getvcp*) echo 'VCP 10 C 50 100';;\n\
                 *setvcp*) {set_body};;\n\
                 esac\n",
                log = log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Ddcutil {
            cmd: vec![script.to_string_lossy().into_owned()],
            timeout: TIMEOUT,
        }
    }

    fn sets(d: &Path) -> Vec<String> {
        std::fs::read_to_string(d.join("log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("setvcp"))
            .map(String::from)
            .collect()
    }

    fn wait_for(what: &str, within: Duration, cond: impl Fn() -> bool) {
        let until = Instant::now() + within;
        while !cond() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    type Events = Arc<Mutex<Vec<DdcEvent>>>;

    fn detected(d: Ddcutil) -> (Monitors, Events) {
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let m = Monitors::spawn(d, move |ev| e.lock().unwrap().push(ev));
        m.redetect();
        let found = || {
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, DdcEvent::Detected(_)))
        };
        wait_for("detection", Duration::from_secs(10), found);
        (m, events)
    }

    fn levels(events: &Events) -> Vec<u8> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                DdcEvent::Level { percent, .. } => Some(*percent),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn detect_parses_connectors_and_invalid_displays() {
        assert_eq!(
            parse_detect(DETECT),
            vec![
                DdcDisplay {
                    bus: 6,
                    connector: "DP-1".into(),
                    valid: true
                },
                DdcDisplay {
                    bus: 7,
                    connector: "HDMI-A-1".into(),
                    valid: false
                }
            ]
        );
    }

    /// ddcutil 2.2.5 (Ubuntu 26.04) also prints `Phantom display` blocks
    /// and the `DRM_connector:` label; a block without a bus is skipped.
    #[test]
    fn detect_takes_phantoms_as_invalid_and_skips_blocks_without_a_bus() {
        let out = "Phantom display\n   I2C bus:  /dev/i2c-3\n   DRM_connector:  card0-eDP-1\n\n\
                   Display 2\n   DRM_connector:  card0-DP-2\n\n\
                   Display 3\n   I2C bus:  /dev/i2c-9\n   DRM_connector:  card0-DP-3\n";
        assert_eq!(
            parse_detect(out),
            vec![
                DdcDisplay {
                    bus: 3,
                    connector: "eDP-1".into(),
                    valid: false
                },
                DdcDisplay {
                    bus: 9,
                    connector: "DP-3".into(),
                    valid: true
                }
            ]
        );
        assert_eq!(parse_detect(""), vec![]);
    }

    #[test]
    fn getvcp_terse() {
        assert_eq!(parse_getvcp("VCP 10 C 60 100\n"), Some((60, 100)));
        assert_eq!(parse_getvcp("VCP 10 ERR\n"), None);
        assert_eq!(parse_getvcp(""), None);
    }

    #[test]
    fn steps_are_a_tenth_of_the_maximum() {
        assert_eq!(step(60, 100, true), 70);
        assert_eq!(step(95, 100, true), 100);
        assert_eq!(step(5, 100, false), 0);
        assert_eq!(step(100, 255, true), 126);
        assert_eq!(percent(126, 255), 49);
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(100, 100), 100);
    }

    #[test]
    fn brightness_keeps_only_the_latest_target_while_a_set_runs() {
        let d = TestDir::new("osd-ddc", "latest");
        let (m, events) = detected(fake(&d, "sleep 0.3"));
        for _ in 0..10 {
            m.step(true);
        }
        wait_for("the set to 100", Duration::from_secs(10), || {
            sets(&d).last().is_some_and(|l| l.ends_with("setvcp 10 100"))
        });
        // A third set would start at once; give it room to show.
        std::thread::sleep(Duration::from_millis(500));
        let sets = sets(&d);
        assert!(sets.len() <= 2, "{sets:?}");
        assert_eq!(sets.last().unwrap(), "--bus 6 --noverify setvcp 10 100");
        assert_eq!(levels(&events).last(), Some(&100));
    }

    #[test]
    fn a_hanging_ddcutil_is_killed_and_the_next_set_runs() {
        let d = TestDir::new("osd-ddc", "hang");
        let first = d.join("first");
        let body = format!("if [ ! -e '{f}' ]; then : > '{f}'; sleep 5; fi", f = first.display());
        let (m, _) = detected(fake(&d, &body).with_timeout(Duration::from_millis(200)));
        m.step(true);
        wait_for("the first set", Duration::from_secs(10), || sets(&d).len() == 1);
        m.step(true);
        let asked = Instant::now();
        wait_for("the second set", Duration::from_secs(10), || sets(&d).len() == 2);
        // The first call sleeps 5 s; the second must not wait for it.
        assert!(asked.elapsed() < Duration::from_secs(3), "{:?}", asked.elapsed());
        assert_eq!(sets(&d)[1], "--bus 6 --noverify setvcp 10 70");
    }

    #[test]
    fn step_before_detection_does_nothing() {
        let d = TestDir::new("osd-ddc", "early");
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let m = Monitors::spawn(fake(&d, ""), move |ev| e.lock().unwrap().push(ev));
        m.step(true);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(sets(&d), Vec::<String>::new());
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn detection_reports_each_display_and_steps_skip_invalid_ones() {
        let d = TestDir::new("osd-ddc", "detect");
        let script = d.join("ddcutil");
        let log = d.join("log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n\
                 detect*) printf '{}';;\n*getvcp*) echo 'VCP 10 C 60 100';;\nesac\n",
                log.display(),
                DETECT.replace('\n', "\\n")
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ddc = Ddcutil {
            cmd: vec![script.to_string_lossy().into_owned()],
            timeout: TIMEOUT,
        };
        assert_eq!(ddc.get(6), Ok((60, 100)));
        let (m, events) = detected(ddc);
        assert_eq!(
            events.lock().unwrap()[0],
            DdcEvent::Detected(vec![
                ("DP-1".into(), Bright::Percent(60)),
                ("HDMI-A-1".into(), Bright::Unsupported)
            ])
        );
        m.step(false);
        wait_for("the set", Duration::from_secs(10), || sets(&d).len() == 1);
        assert_eq!(sets(&d), ["--bus 6 --noverify setvcp 10 50"]);
        assert_eq!(
            events.lock().unwrap()[1],
            DdcEvent::Level {
                connector: "DP-1".into(),
                percent: 50
            }
        );
    }

    #[test]
    fn a_failing_ddcutil_is_an_error() {
        let d = TestDir::new("osd-ddc", "fail");
        let script = d.join("ddcutil");
        std::fs::write(&script, "#!/bin/sh\necho 'No monitor' >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ddc = Ddcutil {
            cmd: vec![script.to_string_lossy().into_owned()],
            timeout: TIMEOUT,
        };
        assert!(ddc.detect().is_err());
        assert!(ddc.get(6).is_err());
        assert!(ddc.set(6, 10).is_err());
    }
}
