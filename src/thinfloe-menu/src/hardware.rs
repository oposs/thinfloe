//! What the status area's hardware tiles draw from: temperatures, CPU load
//! and clock, RAM and zram use, fan speed, uptime, maker and model of the
//! machine.
//!
//! A thread of the menu (`spawn`) reads these every `EVERY` from /sys and
//! /proc, without root and without a subprocess, and rests while the element
//! cannot be seen. Everything read is treated as hostile: bounded reads,
//! bounded directory scans, values outside a plausible range dropped, names
//! made printable. What a machine does not provide is `None` or absent, and
//! the element draws no placeholder. Every path starts at a `Roots`, so tests
//! read a directory.

use crate::battery::sanitise;
use crate::bounded::read_text;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the thread reads.
pub const EVERY: Duration = Duration::from_secs(5);
/// How soon after a first reading the second comes: a load needs two.
pub const SOON: Duration = Duration::from_secs(1);
/// How often a resting thread looks whether it may read again.
const LOOK: Duration = Duration::from_secs(1);
/// A sysfs attribute. The kernel reports 4096 for most of them.
const MAX_ATTR: u64 = 8 * 1024;
/// /proc/stat and /proc/cpuinfo grow with the cores and interrupts; a larger
/// one is not read and its value drops out.
const MAX_PROC: u64 = 256 * 1024;
/// How many entries a directory scan looks at, whatever a directory holds.
const MAX_SCANNED: usize = 16 * 1024;
/// How many entries it keeps of those it wants.
const MAX_CHIPS: usize = 32;
const MAX_ZONES: usize = 32;
const MAX_ENTRIES: usize = 512;
const MAX_BLOCK_ENTRIES: usize = 256;
const MAX_ZRAM: usize = 16;
const MAX_CPUS: usize = 256;
const MAX_TRIPS: usize = 16;
/// `tempN_*` and `fanN_*` above this are not looked at.
const MAX_INDEX: u32 = 64;
/// Lines kept.
pub const MAX_TEMPS: usize = 8;
pub const MAX_FANS: usize = 4;
/// The longest sensor name kept, in characters; the longest DMI name.
const MAX_LABEL: usize = 16;
const MAX_DMI: usize = 40;
/// A reading outside this is a missing or broken sensor: chips report 0, 127
/// or -128 for an input with nothing on it.
const TEMP_RANGE: std::ops::RangeInclusive<f32> = 1.0..=125.0;
/// A limit outside this says nothing.
const LIMIT_RANGE: std::ops::RangeInclusive<f32> = 40.0..=150.0;
/// A fan at 0 is stopped or not there, which a chip cannot tell apart, so it
/// drops out; 65535 is a chip's "no signal".
const FAN_RANGE: std::ops::RangeInclusive<u32> = 1..=30_000;
/// A clock outside this, in MHz, is not a clock.
const CLOCK_RANGE: std::ops::RangeInclusive<u64> = 10..=20_000;
const MAX_UPTIME: f64 = 100.0 * 365.0 * 86_400.0;

/// Where /sys and /proc are.
#[derive(Debug, Clone)]
pub struct Roots {
    pub sys: PathBuf,
    pub proc_: PathBuf,
}

impl Roots {
    pub fn system() -> Roots {
        Roots {
            sys: "/sys".into(),
            proc_: "/proc".into(),
        }
    }

    /// Both under one directory: `<dir>/sys`, `<dir>/proc`.
    #[cfg(test)]
    pub fn under(dir: &Path) -> Roots {
        Roots {
            sys: dir.join("sys"),
            proc_: dir.join("proc"),
        }
    }
}

fn attr(path: &Path) -> Option<String> {
    read_text(path, MAX_ATTR)
}

/// A directory's entries that `name` accepts, at most `max` of them, sorted by
/// the number it returns, so the order does not depend on the file system.
/// Others do not count: `/sys/class/thermal` also lists every cooling device.
fn numbered(dir: &Path, max: usize, name: impl Fn(&str) -> Option<u32>) -> Vec<(u32, String)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(u32, String)> = rd
        .take(MAX_SCANNED)
        .filter_map(Result::ok)
        .filter_map(|e| {
            let n = e.file_name().to_str()?.to_string();
            Some((name(&n)?, n))
        })
        .take(max)
        .collect();
    out.sort();
    out
}

/// The number in `<prefix><digits>`.
fn index_of(name: &str, prefix: &str) -> Option<u32> {
    let digits = name.strip_prefix(prefix)?;
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// How hot a temperature is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Heat {
    #[default]
    Normal,
    /// Drawn in the warning colour.
    Hot,
    /// Drawn in the alert colour.
    Critical,
}

/// What the kernel says a sensor can take, in degrees Celsius.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Limits {
    /// `tempN_max`, or a `hot` trip point.
    pub max: Option<f32>,
    /// `tempN_crit`, or a `critical` trip point.
    pub crit: Option<f32>,
}

/// With no limit known a sensor is hot from here, and critical from here.
pub const HOT_DEFAULT: f32 = 80.0;
pub const CRITICAL_DEFAULT: f32 = 95.0;
/// Critical this far below `crit`: machines switch off at `crit`.
pub const CRITICAL_MARGIN: f32 = 5.0;
/// Hot this far below `crit`, when `max` is not usefully below the critical
/// point (coretemp often reports `max` equal to `crit`).
pub const HOT_MARGIN: f32 = 15.0;
/// Critical this far above `max`, when there is no `crit`.
pub const OVER_MAX: f32 = 15.0;

/// From where a sensor with these limits is hot and critical.
pub fn thresholds(l: Limits) -> (f32, f32) {
    match (l.max, l.crit) {
        (None, None) => (HOT_DEFAULT, CRITICAL_DEFAULT),
        (Some(m), None) => (m, m + OVER_MAX),
        (None, Some(c)) => (c - HOT_MARGIN, c - CRITICAL_MARGIN),
        (Some(m), Some(c)) => {
            let critical = c - CRITICAL_MARGIN;
            (if m < critical { m } else { c - HOT_MARGIN }, critical)
        }
    }
}

pub fn heat(celsius: f32, limits: Limits) -> Heat {
    let (hot, critical) = thresholds(limits);
    if celsius >= critical {
        Heat::Critical
    } else if celsius >= hot {
        Heat::Hot
    } else {
        Heat::Normal
    }
}

/// Millidegrees as written by hwmon and thermal, to degrees.
pub fn millidegrees(text: &str) -> Option<f32> {
    let v: i64 = text.trim().parse().ok()?;
    (v.unsigned_abs() < 1_000_000).then(|| v as f32 / 1000.0)
}

fn temperature(text: &str) -> Option<f32> {
    millidegrees(text).filter(|c| TEMP_RANGE.contains(c))
}

fn limit(text: Option<String>) -> Option<f32> {
    millidegrees(&text?).filter(|c| LIMIT_RANGE.contains(c))
}

/// One temperature line.
#[derive(Debug, Clone, PartialEq)]
pub struct Temp {
    pub label: String,
    pub celsius: f32,
    pub heat: Heat,
}

/// How a sensor chip or thermal zone is called on the screen, and where it
/// sorts: the CPU first.
pub fn sensor_name(chip: &str) -> (u8, String) {
    let c = chip.to_ascii_lowercase();
    let starts = |list: &[&str]| list.iter().any(|p| c.starts_with(p));
    if starts(&[
        "coretemp",
        "k10temp",
        "k8temp",
        "zenpower",
        "cpu",
        "x86_pkg_temp",
        "via_cputemp",
        "soc_thermal",
    ]) {
        (0, "CPU".into())
    } else if starts(&["amdgpu", "radeon", "nouveau", "i915", "xe", "gpu"]) {
        (1, "GPU".into())
    } else if starts(&["nvme", "drivetemp"]) {
        (2, "Disk".into())
    } else if starts(&["spd5118", "jc42"]) {
        (3, "RAM".into())
    } else if starts(&["iwlwifi", "ath", "mt7", "rtw", "brcm"]) {
        (4, "WiFi".into())
    } else if starts(&[
        "r8169", "igc", "igb", "e1000", "atlantic", "tg3", "ixgbe", "phy", "enp", "eth",
    ]) {
        (5, "LAN".into())
    } else if starts(&["pch"]) {
        (6, "Chipset".into())
    } else if starts(&[
        "acpitz", "nct", "it87", "it86", "f71", "w83", "thinkpad", "dell_smm", "hp_wmi", "asus",
    ]) {
        (7, "Board".into())
    } else {
        let name: String = sanitise(chip).chars().take(MAX_LABEL).collect();
        (8, name)
    }
}

/// The input of a chip that stands for the whole chip, when it labels its
/// inputs: per-core readings would fill the element.
const MAIN_LABELS: [&str; 5] = ["Package id", "Tdie", "Tctl", "Composite", "edge"];

/// Numbers labels that several sensors share (`Disk 1`, `Disk 2`) and keeps
/// the first `MAX_TEMPS` in their sort order.
fn finish_temps(mut found: Vec<(u8, String, Temp)>) -> Vec<Temp> {
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let mut seen = std::collections::HashMap::new();
    let totals = found.iter().fold(std::collections::HashMap::new(), |mut m, (_, _, t)| {
        *m.entry(t.label.clone()).or_insert(0) += 1;
        m
    });
    found
        .into_iter()
        .map(|(_, _, mut t)| {
            if totals[&t.label] > 1 {
                let n = seen.entry(t.label.clone()).or_insert(0);
                *n += 1;
                t.label = format!("{} {n}", t.label);
            }
            t
        })
        .take(MAX_TEMPS)
        .collect()
}

/// The temperatures and fans of `/sys/class/hwmon`: one temperature a chip
/// (its main input, else its hottest), and every fan that turns.
fn hwmon(roots: &Roots) -> (Vec<(u8, String, Temp)>, Vec<u32>) {
    let base = roots.sys.join("class/hwmon");
    let mut temps = Vec::new();
    let mut fans = Vec::new();
    for (_, dir) in numbered(&base, MAX_CHIPS, |n| index_of(n, "hwmon")) {
        let dir = base.join(dir);
        let chip = attr(&dir.join("name")).map_or_else(|| "sensor".to_string(), |n| n.trim().to_string());
        let mut best: Option<(bool, f32, Limits)> = None;
        for (i, _) in numbered(&dir, MAX_ENTRIES, |n| {
            index_of(n.strip_suffix("_input")?, "temp").filter(|i| *i <= MAX_INDEX)
        }) {
            let at = |what: &str| attr(&dir.join(format!("temp{i}_{what}")));
            let Some(c) = at("input").as_deref().and_then(temperature) else {
                continue;
            };
            let main = at("label").is_some_and(|l| MAIN_LABELS.iter().any(|m| l.trim().starts_with(m)));
            let limits = Limits {
                max: limit(at("max")),
                crit: limit(at("crit")),
            };
            let better = match &best {
                None => true,
                Some((bm, bc, _)) => (main, c) > (*bm, *bc),
            };
            if better {
                best = Some((main, c, limits));
            }
        }
        if let Some((_, celsius, limits)) = best {
            let (rank, label) = sensor_name(&chip);
            temps.push((
                rank,
                chip.clone(),
                Temp {
                    label,
                    celsius,
                    heat: heat(celsius, limits),
                },
            ));
        }
        for (i, _) in numbered(&dir, MAX_ENTRIES, |n| {
            index_of(n.strip_suffix("_input")?, "fan").filter(|i| *i <= MAX_INDEX)
        }) {
            let rpm = attr(&dir.join(format!("fan{i}_input"))).and_then(|t| t.trim().parse::<u32>().ok());
            if let Some(r) = rpm.filter(|r| FAN_RANGE.contains(r))
                && fans.len() < MAX_FANS
            {
                fans.push(r);
            }
        }
    }
    (temps, fans)
}

/// The zones of `/sys/class/thermal` not already seen through hwmon: most
/// zones are registered there too, under the same name.
fn thermal(roots: &Roots, known: &[(u8, String, Temp)]) -> Vec<(u8, String, Temp)> {
    let base = roots.sys.join("class/thermal");
    let mut out = Vec::new();
    for (_, name) in numbered(&base, MAX_ENTRIES, |n| index_of(n, "thermal_zone"))
        .into_iter()
        .take(MAX_ZONES)
    {
        let dir = base.join(name);
        let Some(kind) = attr(&dir.join("type")).map(|t| t.trim().to_string()) else {
            continue;
        };
        // The DPTF manager's own zone reads nothing real.
        if kind.starts_with("INT3400") {
            continue;
        }
        let Some(celsius) = attr(&dir.join("temp")).as_deref().and_then(temperature) else {
            continue;
        };
        let (rank, label) = sensor_name(&kind);
        if known.iter().any(|(_, _, t)| t.label == label) {
            continue;
        }
        let mut limits = Limits::default();
        for i in 0..MAX_TRIPS {
            let Some(t) = attr(&dir.join(format!("trip_point_{i}_type"))) else {
                break;
            };
            let value = limit(attr(&dir.join(format!("trip_point_{i}_temp"))));
            match t.trim() {
                "critical" => limits.crit = limits.crit.or(value),
                "hot" => limits.max = limits.max.or(value),
                _ => {}
            }
        }
        out.push((
            rank,
            kind,
            Temp {
                label,
                celsius,
                heat: heat(celsius, limits),
            },
        ));
    }
    out
}

/// Every temperature and fan of the machine.
pub fn sensors(roots: &Roots) -> (Vec<Temp>, Vec<u32>) {
    let (mut temps, fans) = hwmon(roots);
    temps.extend(thermal(roots, &temps));
    (finish_temps(temps), fans)
}

/// The counters of `/proc/stat`'s `cpu` line, in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

pub fn parse_stat(text: &str) -> Option<CpuTimes> {
    let line = text.lines().next()?;
    let mut f = line.split_whitespace();
    if f.next() != Some("cpu") {
        return None;
    }
    // user nice system idle iowait irq softirq steal; guest time is already
    // in user. A kernel prints at least the first four.
    let v: Vec<u64> = f.take(8).map(|x| x.parse::<u64>().ok()).collect::<Option<_>>()?;
    if v.len() < 4 {
        return None;
    }
    let total = v.iter().fold(0u64, |a, b| a.saturating_add(*b));
    let idle = v[3].saturating_add(v.get(4).copied().unwrap_or(0));
    Some(CpuTimes {
        busy: total.saturating_sub(idle),
        total,
    })
}

/// The load between two readings in percent; `None` when no time passed or a
/// counter went back (iowait may).
pub fn load(before: CpuTimes, now: CpuTimes) -> Option<u8> {
    let total = now.total.checked_sub(before.total).filter(|t| *t > 0)?;
    let busy = now.busy.checked_sub(before.busy)?;
    Some(((busy.min(total) as f64 / total as f64 * 100.0).round()) as u8)
}

/// The highest `cpu MHz` of `/proc/cpuinfo`.
pub fn cpuinfo_mhz(text: &str) -> Option<u32> {
    text.lines()
        .filter_map(|l| {
            let (key, value) = l.split_once(':')?;
            if key.trim() != "cpu MHz" {
                return None;
            }
            let mhz: f64 = value.trim().parse().ok()?;
            mhz.is_finite().then_some(mhz.round() as i64)
        })
        .filter_map(|m| u64::try_from(m).ok())
        .filter(|m| CLOCK_RANGE.contains(m))
        .max()
        .map(|m| m as u32)
}

/// The fastest core's clock in MHz: cpufreq when it is there, else what
/// `/proc/cpuinfo` says. The fastest, since an average of idle cores says
/// little about the one doing the work.
pub fn clock(roots: &Roots) -> Option<u32> {
    let base = roots.sys.join("devices/system/cpu");
    let mut best = None;
    for i in 0..MAX_CPUS {
        let dir = base.join(format!("cpu{i}"));
        if !dir.is_dir() {
            break;
        }
        let khz = attr(&dir.join("cpufreq/scaling_cur_freq")).and_then(|t| t.trim().parse::<u64>().ok());
        if let Some(m) = khz.map(|k| k / 1000).filter(|m| CLOCK_RANGE.contains(m)) {
            best = best.max(Some(m as u32));
        }
    }
    best.or_else(|| read_text(&roots.proc_.join("cpuinfo"), MAX_PROC).and_then(|t| cpuinfo_mhz(&t)))
}

/// Bytes in use of bytes there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}

/// RAM in use: `MemTotal` less `MemAvailable`, what `free` calls used plus
/// the part of the caches that cannot be given back.
pub fn parse_meminfo(text: &str) -> Option<Usage> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.split(':').next() == Some(name))?;
        let mut w = line.split(':').nth(1)?.split_whitespace();
        let v: u64 = w.next()?.parse().ok()?;
        (w.next() == Some("kB")).then_some(v)?.checked_mul(1024)
    };
    let total = field("MemTotal").filter(|t| *t > 0)?;
    let available = field("MemAvailable")?;
    Some(Usage {
        used: total.saturating_sub(available),
        total,
    })
}

/// What `mm_stat` says is stored: its first field, the data before it was
/// compressed, in bytes.
pub fn parse_mm_stat(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// The zram devices together: data stored of their size. The image's
/// overlay is one of them, so this is how full the writable layer is. A
/// device with no size is not set up and does not count.
pub fn zram(roots: &Roots) -> Option<Usage> {
    let base = roots.sys.join("block");
    let mut sum: Option<Usage> = None;
    for (_, name) in numbered(&base, MAX_BLOCK_ENTRIES, |n| index_of(n, "zram"))
        .into_iter()
        .take(MAX_ZRAM)
    {
        let dir = base.join(name);
        let Some(total) = attr(&dir.join("disksize"))
            .and_then(|t| t.trim().parse::<u64>().ok())
            .filter(|t| *t > 0)
        else {
            continue;
        };
        let used = attr(&dir.join("mm_stat"))
            .as_deref()
            .and_then(parse_mm_stat)
            .unwrap_or(0);
        let s = sum.get_or_insert(Usage { used: 0, total: 0 });
        s.used = s.used.saturating_add(used.min(total));
        s.total = s.total.saturating_add(total);
    }
    sum
}

/// The first number of `/proc/uptime`, in whole seconds.
pub fn parse_uptime(text: &str) -> Option<u64> {
    let s: f64 = text.split_whitespace().next()?.parse().ok()?;
    (s.is_finite() && (0.0..MAX_UPTIME).contains(&s)).then_some(s as u64)
}

/// A DMI string fit to show: printable and short, and `None` for the
/// placeholders firmware ships.
pub fn clean_dmi(text: &str) -> Option<String> {
    let t: String = sanitise(text.trim()).chars().take(MAX_DMI).collect();
    let t = t.trim();
    let low = t.to_ascii_lowercase();
    const JUNK: [&str; 21] = [
        "unknown device",
        "default string",
        "system manufacturer",
        "system product name",
        "system version",
        "not specified",
        "not applicable",
        "none",
        "n/a",
        "na",
        "o.e.m.",
        "oem",
        "unknown",
        "invalid",
        "type1productconfigid",
        "x.x",
        "0",
        "1",
        "123456789",
        "default",
        "empty",
    ];
    if t.is_empty() || JUNK.contains(&low.as_str()) || low.contains("filled by") || low.contains("o.e.m.") {
        return None;
    }
    Some(t.to_string())
}

/// A maker without its company form: `Gigabyte Technology Co., Ltd.` is
/// `Gigabyte Technology`.
fn short_maker(maker: &str) -> String {
    let mut m = maker.trim();
    loop {
        let before = m;
        for suffix in [
            "Co., Ltd.",
            "Co.,Ltd.",
            "Co., Ltd",
            "Ltd.",
            "Inc.",
            "Inc",
            "Corporation",
            "Corp.",
            "GmbH",
            "Limited",
            "LLC",
        ] {
            if let Some(rest) = m.strip_suffix(suffix) {
                m = rest.trim_end_matches([' ', ',']);
            }
        }
        if m == before {
            return m.to_string();
        }
    }
}

/// The machine's maker and model from `/sys/class/dmi/id`. Lenovo keeps the
/// name people know in `product_version` and a type code in `product_name`.
/// Whitebox machines name only their board. A model that starts with the
/// maker's name does not repeat it.
pub fn machine(roots: &Roots) -> (Option<String>, Option<String>) {
    let dir = roots.sys.join("class/dmi/id");
    let read = |f: &str| attr(&dir.join(f)).as_deref().and_then(clean_dmi);
    let vendor = read("sys_vendor").or_else(|| read("board_vendor"));
    let lenovo = vendor.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("lenovo"));
    let model = lenovo
        .then(|| read("product_version"))
        .flatten()
        .or_else(|| read("product_name"))
        .or_else(|| read("board_name"));
    let maker = vendor.map(|v| short_maker(&v)).filter(|v| !v.is_empty());
    let model = match (&maker, model) {
        (Some(mk), Some(md)) => {
            let head = md.get(..mk.len()).filter(|h| h.eq_ignore_ascii_case(mk));
            let rest = head.map(|_| md[mk.len()..].trim_start().to_string());
            match rest {
                Some(r) if !r.is_empty() => Some(r),
                _ => Some(md),
            }
        }
        (_, md) => md,
    };
    (maker, model)
}

/// One reading, what the element draws.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// The CPU first, then the rest (`sensor_name`), at most `MAX_TEMPS`.
    pub temps: Vec<Temp>,
    /// Percent, once two readings were made.
    pub load: Option<u8>,
    /// The fastest core, in MHz.
    pub clock_mhz: Option<u32>,
    pub ram: Option<Usage>,
    pub zram: Option<Usage>,
    /// Revolutions a minute, at most `MAX_FANS`.
    pub fans: Vec<u32>,
    /// Seconds since boot.
    pub uptime: Option<u64>,
    pub maker: Option<String>,
    pub model: Option<String>,
}

impl Snapshot {
    /// Whether there is anything to show.
    pub fn any(&self) -> bool {
        *self != Snapshot::default()
    }
}

/// Reads the machine; keeps what a reading is computed from.
#[derive(Debug)]
pub struct Sampler {
    roots: Roots,
    /// The last CPU counters and when they were read.
    last: Option<(Instant, CpuTimes)>,
    last_load: Option<u8>,
    /// Maker and model do not change while the machine runs: read once.
    machine: Option<(Option<String>, Option<String>)>,
}

impl Sampler {
    pub fn new(roots: Roots) -> Sampler {
        Sampler {
            roots,
            last: None,
            last_load: None,
            machine: None,
        }
    }

    /// A reading made at `now`, and how long until the next. After a rest
    /// (the menu was covered) the counters span the rest, so they only start
    /// a new span: the old load stands until the next reading, which comes
    /// soon.
    pub fn sample(&mut self, now: Instant) -> (Snapshot, Duration) {
        let r = &self.roots;
        let (temps, fans) = sensors(r);
        let stat = read_text(&r.proc_.join("stat"), MAX_PROC).and_then(|t| parse_stat(&t));
        let fresh = self
            .last
            .is_some_and(|(at, _)| now.saturating_duration_since(at) <= 3 * EVERY);
        let mut next = EVERY;
        match (self.last, stat) {
            (Some((_, before)), Some(times)) if fresh => self.last_load = load(before, times).or(self.last_load),
            (_, Some(_)) => next = SOON,
            (_, None) => self.last_load = None,
        }
        self.last = stat.map(|s| (now, s));
        let (maker, model) = self.machine.get_or_insert_with(|| machine(&self.roots)).clone();
        let r = &self.roots;
        let snap = Snapshot {
            temps,
            load: self.last_load,
            clock_mhz: clock(r),
            ram: read_text(&r.proc_.join("meminfo"), MAX_PROC).and_then(|t| parse_meminfo(&t)),
            zram: zram(r),
            fans,
            uptime: read_text(&r.proc_.join("uptime"), MAX_ATTR).and_then(|t| parse_uptime(&t)),
            maker,
            model,
        };
        (snap, next)
    }
}

/// A sampler that reads when it is on and its time has come.
struct Gated {
    sampler: Sampler,
    due: Instant,
}

impl Gated {
    fn poll(&mut self, on: bool, now: Instant) -> Option<Snapshot> {
        if !on || now < self.due {
            return None;
        }
        let (snap, next) = self.sampler.sample(now);
        self.due = now + next;
        Some(snap)
    }
}

/// A sampler thread for the menu. It reads only while `on` is set (the app
/// clears it while the element cannot be seen), outside the lock, so a frame
/// never waits for a sensor. It calls `wake` only when `Snapshot::any` changes:
/// a hidden element asks for no frames, and the elements' refresh interval
/// draws a shown one. The copy is `None` until the first reading.
pub fn spawn(roots: Roots, on: Arc<AtomicBool>, wake: impl Fn() + Send + 'static) -> Arc<Mutex<Option<Snapshot>>> {
    let shared = Arc::new(Mutex::new(None));
    let out = shared.clone();
    std::thread::spawn(move || {
        let mut gated = Gated {
            sampler: Sampler::new(roots),
            due: Instant::now(),
        };
        let mut had = false;
        loop {
            if let Some(snap) = gated.poll(on.load(Ordering::Relaxed), Instant::now()) {
                let has = snap.any();
                match shared.lock() {
                    Ok(mut s) => *s = Some(snap),
                    Err(_) => return,
                }
                if crate::netstat::flipped(&mut had, has) {
                    wake();
                }
            }
            std::thread::sleep(LOOK);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;

    fn put(dir: &Path, name: &str, text: &str) {
        let p = dir.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn entries_the_filter_refuses_do_not_use_up_the_limit() {
        let d = TestDir::new("hardware", "numbered");
        for n in 0..64 {
            put(&d, &format!("cooling_device{n}"), "");
        }
        put(&d, "thermal_zone3", "");
        put(&d, "thermal_zone1", "");
        let got = numbered(&d, 2, |n| index_of(n, "thermal_zone"));
        assert_eq!(
            got,
            [(1, "thermal_zone1".to_string()), (3, "thermal_zone3".to_string())]
        );
        assert_eq!(numbered(&d, 1, |n| index_of(n, "thermal_zone")).len(), 1);
    }

    #[test]
    fn a_sampler_turned_off_reads_no_further_until_it_is_on_again() {
        let d = TestDir::new("hardware", "gated");
        put(&d, "proc/uptime", "100.00 100.00\n");
        put(&d, "proc/stat", "cpu  1 2 3 4 5 6 7 8\n");
        let t0 = Instant::now();
        let mut gated = Gated {
            sampler: Sampler::new(Roots::under(&d)),
            due: t0,
        };
        // A read leaves its mark in the sampler: the CPU counters and when
        // they came, and the machine's names. Off from the start: none.
        assert!(gated.poll(false, t0).is_none());
        assert!(gated.sampler.last.is_none(), "a read while off");
        assert!(gated.sampler.machine.is_none(), "a read while off");
        assert_eq!(gated.poll(true, t0).and_then(|s| s.uptime), Some(100));
        let read_at = gated.sampler.last.map(|(at, _)| at);
        assert_eq!(read_at, Some(t0));
        // On, but its time has not come.
        assert!(gated.poll(true, t0).is_none());
        let later = t0 + EVERY + EVERY;
        put(&d, "proc/uptime", "200.00 200.00\n");
        // Off, with its time long come.
        assert!(gated.poll(false, later).is_none());
        assert_eq!(gated.sampler.last.map(|(at, _)| at), read_at, "a read while off");
        assert_eq!(gated.poll(true, later).and_then(|s| s.uptime), Some(200));
        assert_eq!(gated.sampler.last.map(|(at, _)| at), Some(later));
    }

    fn lim(max: Option<f32>, crit: Option<f32>) -> Limits {
        Limits { max, crit }
    }

    #[test]
    fn thresholds_come_from_what_the_kernel_reports() {
        // coretemp with a max below crit: hot at max, critical 5 below crit.
        assert_eq!(thresholds(lim(Some(84.0), Some(100.0))), (84.0, 95.0));
        // coretemp with max equal to crit, and an NVMe whose max is in the
        // last 5 degrees: hot 15 below crit.
        assert_eq!(thresholds(lim(Some(100.0), Some(100.0))), (85.0, 95.0));
        assert_eq!(thresholds(lim(Some(82.0), Some(85.0))), (70.0, 80.0));
        // Only one of them.
        assert_eq!(thresholds(lim(None, Some(100.0))), (85.0, 95.0));
        assert_eq!(thresholds(lim(Some(70.0), None)), (70.0, 85.0));
        // Nothing reported.
        assert_eq!(thresholds(lim(None, None)), (80.0, 95.0));
        // A crit below max is believed over it.
        assert_eq!(thresholds(lim(Some(100.0), Some(90.0))), (75.0, 85.0));
    }

    #[test]
    fn heat_turns_at_the_thresholds() {
        let l = lim(Some(84.0), Some(100.0));
        assert_eq!(heat(83.9, l), Heat::Normal);
        assert_eq!(heat(84.0, l), Heat::Hot);
        assert_eq!(heat(94.9, l), Heat::Hot);
        assert_eq!(heat(95.0, l), Heat::Critical);
        assert_eq!(heat(40.0, lim(None, None)), Heat::Normal);
        assert_eq!(heat(80.0, lim(None, None)), Heat::Hot);
    }

    #[test]
    fn millidegrees_are_parsed_and_nonsense_is_not() {
        assert_eq!(millidegrees("52000\n"), Some(52.0));
        assert_eq!(millidegrees(" 27800 "), Some(27.8));
        assert_eq!(millidegrees("-5000"), Some(-5.0));
        for bad in [
            "",
            "abc",
            "52.0",
            "0x10",
            "99999999999999999999999",
            "9999999",
            "\u{0}",
            "NaN",
        ] {
            assert_eq!(millidegrees(bad), None, "{bad:?}");
        }
        assert_eq!(millidegrees(&i64::MIN.to_string()), None);
        assert_eq!(millidegrees(&i64::MAX.to_string()), None);
        // Readings a chip gives for an input with nothing on it.
        assert_eq!(temperature("0"), None);
        assert_eq!(temperature("127000"), None);
        assert_eq!(temperature("-128000"), None);
        assert_eq!(temperature("45000"), Some(45.0));
    }

    #[test]
    fn sensors_have_the_names_people_know_the_cpu_first() {
        for (chip, rank, name) in [
            ("coretemp", 0, "CPU"),
            ("k10temp", 0, "CPU"),
            ("cpu_thermal", 0, "CPU"),
            ("x86_pkg_temp", 0, "CPU"),
            ("amdgpu", 1, "GPU"),
            ("i915", 1, "GPU"),
            ("nvme", 2, "Disk"),
            ("drivetemp", 2, "Disk"),
            ("iwlwifi_1", 4, "WiFi"),
            ("r8169_0_100:00", 5, "LAN"),
            ("pch_cannonlake", 6, "Chipset"),
            ("acpitz", 7, "Board"),
            ("nct6798", 7, "Board"),
            ("SEN1", 8, "SEN1"),
        ] {
            assert_eq!(sensor_name(chip), (rank, name.to_string()), "{chip}");
        }
        assert_eq!(
            sensor_name("a\x1b<b>&\u{202e}c").1,
            "a?<b>&?c",
            "printable; the SVG escapes it"
        );
        assert_eq!(sensor_name(&"W".repeat(200)).1.chars().count(), 16);
    }

    /// A Lenovo-like machine: coretemp with cores, NVMe, acpitz, a Super I/O
    /// with a fan and a dead input, WiFi.
    fn realistic(d: &Path) {
        let h = d.join("sys/class/hwmon");
        let chip = |n: &str| h.join(n);
        put(&chip("hwmon0"), "name", "acpitz\n");
        put(&chip("hwmon0"), "temp1_input", "27800\n");
        put(&chip("hwmon0"), "temp1_crit", "110000\n");
        put(&chip("hwmon3"), "name", "coretemp\n");
        put(&chip("hwmon3"), "temp1_input", "51000\n");
        put(&chip("hwmon3"), "temp1_label", "Package id 0\n");
        put(&chip("hwmon3"), "temp1_max", "84000\n");
        put(&chip("hwmon3"), "temp1_crit", "100000\n");
        put(&chip("hwmon3"), "temp2_input", "64000\n");
        put(&chip("hwmon3"), "temp2_label", "Core 0\n");
        put(&chip("hwmon1"), "name", "nvme\n");
        put(&chip("hwmon1"), "temp1_input", "38850\n");
        put(&chip("hwmon1"), "temp1_label", "Composite\n");
        put(&chip("hwmon1"), "temp1_max", "81850\n");
        put(&chip("hwmon1"), "temp1_crit", "84850\n");
        put(&chip("hwmon1"), "temp2_input", "60000\n");
        put(&chip("hwmon1"), "temp2_label", "Sensor 2\n");
        put(&chip("hwmon4"), "name", "nct6798\n");
        put(&chip("hwmon4"), "temp1_input", "33000\n");
        put(&chip("hwmon4"), "temp2_input", "127000\n");
        put(&chip("hwmon4"), "fan1_input", "1234\n");
        put(&chip("hwmon4"), "fan2_input", "0\n");
        put(&chip("hwmon4"), "fan3_input", "65535\n");
        put(&chip("hwmon5"), "name", "iwlwifi_1\n");
        put(&chip("hwmon5"), "temp1_input", "41000\n");
        let t = d.join("sys/class/thermal");
        // Registered in hwmon too.
        put(&t.join("thermal_zone0"), "type", "acpitz\n");
        put(&t.join("thermal_zone0"), "temp", "27800\n");
        // The same package as coretemp.
        put(&t.join("thermal_zone1"), "type", "x86_pkg_temp\n");
        put(&t.join("thermal_zone1"), "temp", "51000\n");
        // Reads nothing real.
        put(&t.join("thermal_zone2"), "type", "INT3400 Thermal\n");
        put(&t.join("thermal_zone2"), "temp", "20000\n");
        // Only here, hot, with trip points.
        put(&t.join("thermal_zone3"), "type", "SKIN\n");
        put(&t.join("thermal_zone3"), "temp", "71000\n");
        put(&t.join("thermal_zone3"), "trip_point_0_type", "passive\n");
        put(&t.join("thermal_zone3"), "trip_point_0_temp", "45000\n");
        put(&t.join("thermal_zone3"), "trip_point_1_type", "hot\n");
        put(&t.join("thermal_zone3"), "trip_point_1_temp", "70000\n");
        put(&t.join("thermal_zone3"), "trip_point_2_type", "critical\n");
        put(&t.join("thermal_zone3"), "trip_point_2_temp", "95000\n");
    }

    #[test]
    fn a_realistic_machine_gives_one_line_a_sensor_and_its_fans() {
        let d = TestDir::new("hardware", "real");
        realistic(&d);
        let (temps, fans) = sensors(&Roots::under(&d));
        let got: Vec<(&str, f32, Heat)> = temps.iter().map(|t| (t.label.as_str(), t.celsius, t.heat)).collect();
        assert_eq!(
            got,
            [
                ("CPU", 51.0, Heat::Normal),
                ("Disk", 38.85, Heat::Normal),
                ("WiFi", 41.0, Heat::Normal),
                ("Board 1", 27.8, Heat::Normal),
                ("Board 2", 33.0, Heat::Normal),
                ("SKIN", 71.0, Heat::Hot),
            ],
            "the package, not the hotter core; Composite, not its hotter sensor"
        );
        assert_eq!(fans, [1234], "a stopped fan and a chip's 65535 drop out");
    }

    #[test]
    fn a_hot_sensor_is_judged_by_its_own_limits() {
        let d = TestDir::new("hardware", "hot");
        let c = d.join("sys/class/hwmon/hwmon0");
        put(&c, "name", "coretemp\n");
        put(&c, "temp1_input", "96000\n");
        put(&c, "temp1_max", "84000\n");
        put(&c, "temp1_crit", "100000\n");
        let n = d.join("sys/class/hwmon/hwmon1");
        put(&n, "name", "nvme\n");
        put(&n, "temp1_input", "72000\n");
        put(&n, "temp1_max", "81850\n");
        put(&n, "temp1_crit", "84850\n");
        let g = d.join("sys/class/hwmon/hwmon2");
        put(&g, "name", "amdgpu\n");
        put(&g, "temp1_input", "79000\n");
        // A limit that is nonsense counts as no limit.
        put(&g, "temp1_crit", "99999999\n");
        put(&g, "temp1_max", "0\n");
        let (t, _) = sensors(&Roots::under(&d));
        let heats: Vec<(&str, Heat)> = t.iter().map(|t| (t.label.as_str(), t.heat)).collect();
        assert_eq!(
            heats,
            [("CPU", Heat::Critical), ("GPU", Heat::Normal), ("Disk", Heat::Hot)]
        );
    }

    #[test]
    fn hostile_sensor_directories_are_survived_and_bounded() {
        let d = TestDir::new("hardware", "hostile");
        let h = d.join("sys/class/hwmon");
        // Junk in every value, a very long name, no name, odd entries.
        let a = h.join("hwmon0");
        put(&a, "name", &format!("<x>&\x07{}\n", "N".repeat(5000)));
        put(&a, "temp1_input", "abc\n");
        put(&a, "temp2_input", "44000\n");
        put(&a, "temp2_label", "\u{202e}<script>\n");
        put(&a, "temp9999_input", "50000\n");
        put(&a, "tempX_input", "50000\n");
        put(&a, "fan1_input", "-5\n");
        put(&a, "fan2_input", "1e9\n");
        // Too large to be an attribute.
        put(&a, "fan3_input", &"1".repeat(100_000));
        // A FIFO must not block, and a link to a device must not be read.
        let fifo = a.join("temp3_input");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        std::os::unix::fs::symlink("/dev/zero", a.join("temp4_input")).unwrap();
        // A directory that is not a chip, and a chip with no name.
        put(&h.join("evil"), "temp1_input", "90000\n");
        let b = h.join("hwmon1");
        put(&b, "temp1_input", "30000\n");
        // A file where a chip's directory should be.
        fs::create_dir_all(&h).unwrap();
        fs::write(h.join("hwmon2"), "x").unwrap();
        let (t, f) = sensors(&Roots::under(&d));
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(
            t[0].label.chars().count() <= 16 && !t[0].label.contains('\x07'),
            "{t:?}"
        );
        assert!(t.iter().any(|x| x.celsius == 44.0), "{t:?}");
        assert!(t.iter().any(|x| x.label == "sensor" && x.celsius == 30.0), "{t:?}");
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn many_chips_and_fans_stay_bounded() {
        let d = TestDir::new("hardware", "many");
        for i in 0..40 {
            let c = d.join(format!("sys/class/hwmon/hwmon{i}"));
            put(&c, "name", "nvme\n");
            put(&c, "temp1_input", "30000\n");
            put(&c, "fan1_input", "1000\n");
        }
        let (t, f) = sensors(&Roots::under(&d));
        assert_eq!(t.len(), MAX_TEMPS);
        assert_eq!(f.len(), MAX_FANS);
        assert_eq!(t[0].label, "Disk 1");
    }

    #[test]
    fn a_machine_without_sensors_has_none() {
        let d = TestDir::new("hardware", "none");
        assert_eq!(sensors(&Roots::under(&d)), (vec![], vec![]));
    }

    #[test]
    fn stat_gives_the_load_between_two_readings() {
        let a = parse_stat("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 100 0 100 700 100 0 0 0 0 0\nintr 1 2 3\n").unwrap();
        assert_eq!(a, CpuTimes { busy: 200, total: 1000 });
        let b = parse_stat("cpu  150 0 150 750 150 0 0 0 0 0\n").unwrap();
        assert_eq!(load(a, b), Some(50));
        // Guest time is in user already: only the first eight fields count.
        assert_eq!(parse_stat("cpu 1 1 1 1 1 1 1 1 900 900\n").unwrap().total, 8);
        // An old kernel's four fields.
        assert_eq!(parse_stat("cpu 1 2 3 4\n"), Some(CpuTimes { busy: 6, total: 10 }));
        assert_eq!(load(a, a), None, "no time passed");
        assert_eq!(load(b, a), None, "a counter went back");
        assert_eq!(
            load(CpuTimes { busy: 900, total: 1000 }, CpuTimes { busy: 800, total: 2000 }),
            None
        );
        assert_eq!(
            load(CpuTimes { busy: 0, total: 0 }, CpuTimes { busy: 500, total: 100 }),
            Some(100),
            "never above 100"
        );
        for bad in [
            "",
            "intr 1 2 3\ncpu 1 2 3 4\n",
            "cpu0 1 2 3 4\n",
            "cpu 1 2 3\n",
            "cpu a b c d\n",
            "cpu -1 2 3 4\n",
            "cpu 99999999999999999999999 2 3 4\n",
        ] {
            assert_eq!(parse_stat(bad), None, "{bad:?}");
        }
        // Counters at their limit do not overflow.
        let top = u64::MAX.to_string();
        let line = format!("cpu {top} {top} {top} {top}\n");
        assert_eq!(parse_stat(&line).unwrap().total, u64::MAX);
    }

    #[test]
    fn the_clock_is_the_fastest_core_from_cpufreq_else_cpuinfo() {
        let d = TestDir::new("hardware", "clock");
        let cpu = d.join("sys/devices/system/cpu");
        put(&cpu.join("cpu0/cpufreq"), "scaling_cur_freq", "800000\n");
        put(&cpu.join("cpu1/cpufreq"), "scaling_cur_freq", "2401000\n");
        put(&cpu.join("cpu2/cpufreq"), "scaling_cur_freq", "junk\n");
        put(&cpu.join("cpu3/cpufreq"), "scaling_cur_freq", "99999999999999\n");
        assert_eq!(clock(&Roots::under(&d)), Some(2401));
        // No cpufreq driver (a VM): cpuinfo.
        let d = TestDir::new("hardware", "clock2");
        fs::create_dir_all(d.join("sys/devices/system/cpu/cpu0")).unwrap();
        put(
            &d.join("proc"),
            "cpuinfo",
            "processor\t: 0\ncpu MHz\t\t: 1996.250\nprocessor\t: 1\ncpu MHz\t\t: 2100.4\nmodel name\t: X\n",
        );
        assert_eq!(clock(&Roots::under(&d)), Some(2100));
        for bad in [
            "",
            "cpu MHz : NaN\n",
            "cpu MHz : inf\n",
            "cpu MHz : -5\n",
            "cpu MHz : 5\n",
            "cpu MHz :\n",
            "bogomips : 5000\n",
        ] {
            assert_eq!(cpuinfo_mhz(bad), None, "{bad:?}");
        }
        // Nothing at all: no clock.
        let d = TestDir::new("hardware", "clock3");
        assert_eq!(clock(&Roots::under(&d)), None);
    }

    #[test]
    fn meminfo_gives_what_is_in_use() {
        let text =
            "MemTotal:        8000000 kB\nMemFree:          100000 kB\nMemAvailable:    6000000 kB\nBuffers: 1 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some(Usage {
                used: 2_000_000 * 1024,
                total: 8_000_000 * 1024,
            })
        );
        // Not fooled by a key that only starts the same.
        let near = "MemTotalX: 5 kB\nMemTotal: 10 kB\nMemAvailable: 4 kB\n";
        assert_eq!(
            parse_meminfo(near),
            Some(Usage {
                used: 6 * 1024,
                total: 10 * 1024
            })
        );
        // More available than there is.
        assert_eq!(
            parse_meminfo("MemTotal: 10 kB\nMemAvailable: 40 kB\n"),
            Some(Usage {
                used: 0,
                total: 10 * 1024
            })
        );
        for bad in [
            "",
            "MemTotal: 10 kB\n",
            "MemAvailable: 10 kB\n",
            "MemTotal: 0 kB\nMemAvailable: 0 kB\n",
            "MemTotal: x kB\nMemAvailable: 1 kB\n",
            "MemTotal: 10 MB\nMemAvailable: 1 MB\n",
            "MemTotal: 10\nMemAvailable: 1\n",
            "MemTotal: 18446744073709551615 kB\nMemAvailable: 1 kB\n",
            "MemTotal: -5 kB\nMemAvailable: 1 kB\n",
        ] {
            assert_eq!(parse_meminfo(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn zram_is_the_devices_together_and_only_those_set_up() {
        let d = TestDir::new("hardware", "zram");
        let b = d.join("sys/block");
        put(&b.join("zram0"), "disksize", "2147483648\n");
        put(
            &b.join("zram0"),
            "mm_stat",
            " 429496729 180000000 190000000        0 200000000        5        0        0        0\n",
        );
        put(&b.join("zram1"), "disksize", "1073741824\n");
        // A size but no stat, a stat larger than its size, and one not set up.
        put(&b.join("zram3"), "disksize", "1024\n");
        put(&b.join("zram3"), "mm_stat", "99999999 1 1\n");
        put(&b.join("zram2"), "disksize", "0\n");
        put(&b.join("zram2"), "mm_stat", "5 5 5\n");
        // Not zram.
        put(&b.join("sda"), "disksize", "99\n");
        put(&b.join("zramx"), "disksize", "99\n");
        assert_eq!(
            zram(&Roots::under(&d)),
            Some(Usage {
                used: 429_496_729 + 1024,
                total: 2_147_483_648 + 1_073_741_824 + 1024,
            })
        );
        assert_eq!(parse_mm_stat("junk 1 2"), None);
        assert_eq!(parse_mm_stat(""), None);
        let none = TestDir::new("hardware", "zram0");
        assert_eq!(zram(&Roots::under(&none)), None);
    }

    #[test]
    fn uptime_is_whole_seconds_and_nonsense_is_none() {
        assert_eq!(parse_uptime("1706328.53 1706328.53\n"), Some(1_706_328));
        assert_eq!(parse_uptime("0.00 0.00\n"), Some(0));
        for bad in ["", "abc", "-5 1", "NaN 1", "inf 1", "1e300 1"] {
            assert_eq!(parse_uptime(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn firmware_placeholders_are_no_name() {
        assert_eq!(clean_dmi("HP\n").as_deref(), Some("HP"));
        assert_eq!(
            clean_dmi("  ThinkCentre M75q Gen 2 \n").as_deref(),
            Some("ThinkCentre M75q Gen 2")
        );
        for junk in [
            "",
            "   \n",
            "To Be Filled By O.E.M.\n",
            "To be filled by O.E.M.",
            "Default string\n",
            "System manufacturer\n",
            "System Product Name\n",
            "Not Specified\n",
            "O.E.M.\n",
            "None",
            "Unknown",
            "123456789",
        ] {
            assert_eq!(clean_dmi(junk), None, "{junk:?}");
        }
        assert_eq!(clean_dmi("A\x1bB\u{202e}").as_deref(), Some("A?B?"));
        assert_eq!(clean_dmi(&"W".repeat(500)).unwrap().chars().count(), 40);
    }

    fn dmi(pairs: &[(&str, &str)]) -> (Option<String>, Option<String>) {
        let d = TestDir::new(
            "hardware",
            &format!("dmi{}", pairs.len() * 10 + pairs.first().map_or(0, |p| p.1.len())),
        );
        for (k, v) in pairs {
            put(&d.join("sys/class/dmi/id"), k, v);
        }
        machine(&Roots::under(&d))
    }

    #[test]
    fn maker_and_model_read_as_people_call_them() {
        let s = |a: &str, b: &str| (Some(a.to_string()), Some(b.to_string()));
        assert_eq!(
            dmi(&[("sys_vendor", "HP\n"), ("product_name", "HP t640 Thin Client\n")]),
            s("HP", "t640 Thin Client"),
            "the maker once"
        );
        assert_eq!(
            dmi(&[
                ("sys_vendor", "LENOVO\n"),
                ("product_name", "11JN000AGE\n"),
                ("product_version", "ThinkCentre M75q Gen 2\n"),
            ]),
            s("LENOVO", "ThinkCentre M75q Gen 2"),
            "Lenovo's type code is not its name"
        );
        assert_eq!(
            dmi(&[
                ("sys_vendor", "LENOVO\n"),
                ("product_name", "11JN000AGE\n"),
                ("product_version", "None\n")
            ]),
            s("LENOVO", "11JN000AGE"),
        );
        assert_eq!(
            dmi(&[
                ("sys_vendor", "Gigabyte Technology Co., Ltd.\n"),
                ("product_name", "To Be Filled By O.E.M.\n"),
                ("board_name", "GB-BXi3-5010\n"),
            ]),
            s("Gigabyte Technology", "GB-BXi3-5010"),
            "a whitebox names its board"
        );
        assert_eq!(
            dmi(&[
                ("sys_vendor", "Default string\n"),
                ("board_vendor", "ASUSTeK COMPUTER INC.\n"),
                ("board_name", "PN50\n")
            ]),
            s("ASUSTeK COMPUTER INC.", "PN50"),
        );
        // The model is just the maker: not left empty.
        assert_eq!(
            dmi(&[("sys_vendor", "Acme Inc.\n"), ("product_name", "Acme\n")]),
            s("Acme", "Acme")
        );
        // A multi-byte name is not cut in the middle of a character.
        assert_eq!(dmi(&[("sys_vendor", "ab\n"), ("product_name", "é\n")]), s("ab", "é"));
        assert_eq!(dmi(&[("sys_vendor", "To Be Filled By O.E.M.\n")]), (None, None));
        assert_eq!(dmi(&[]), (None, None));
    }

    fn whole_machine(d: &Path) {
        realistic(d);
        put(&d.join("proc"), "stat", "cpu 100 0 100 800 0 0 0 0\n");
        put(
            &d.join("proc"),
            "meminfo",
            "MemTotal: 8000000 kB\nMemAvailable: 6000000 kB\n",
        );
        put(&d.join("proc"), "uptime", "3700.5 1.0\n");
        put(&d.join("sys/class/dmi/id"), "sys_vendor", "HP\n");
        put(&d.join("sys/class/dmi/id"), "product_name", "t640\n");
        put(
            &d.join("sys/devices/system/cpu/cpu0/cpufreq"),
            "scaling_cur_freq",
            "1500000\n",
        );
        put(&d.join("sys/block/zram0"), "disksize", "1000\n");
        put(&d.join("sys/block/zram0"), "mm_stat", "500 1 1\n");
    }

    #[test]
    fn the_load_needs_a_second_reading_soon_after_the_first() {
        let d = TestDir::new("hardware", "sampler");
        whole_machine(&d);
        let t0 = Instant::now();
        let mut s = Sampler::new(Roots::under(&d));
        let (first, next) = s.sample(t0);
        assert_eq!(next, SOON);
        assert_eq!(first.load, None);
        assert_eq!(first.uptime, Some(3700));
        assert_eq!(first.clock_mhz, Some(1500));
        assert_eq!(first.ram.unwrap().total, 8_000_000 * 1024);
        assert_eq!(first.zram, Some(Usage { used: 500, total: 1000 }));
        assert_eq!(first.fans, [1234]);
        assert_eq!(first.temps[0].label, "CPU");
        assert_eq!(
            (first.maker.as_deref(), first.model.as_deref()),
            (Some("HP"), Some("t640"))
        );
        assert!(first.any());
        put(&d.join("proc"), "stat", "cpu 150 0 150 900 0 0 0 0\n");
        let (second, next) = s.sample(t0 + SOON);
        assert_eq!(next, EVERY);
        assert_eq!(second.load, Some(50));
    }

    #[test]
    fn after_a_rest_the_old_load_stands_until_a_fresh_span_is_measured() {
        let d = TestDir::new("hardware", "rest");
        whole_machine(&d);
        let t0 = Instant::now();
        let mut s = Sampler::new(Roots::under(&d));
        s.sample(t0);
        put(&d.join("proc"), "stat", "cpu 150 0 150 900 0 0 0 0\n");
        let (a, _) = s.sample(t0 + EVERY);
        assert_eq!(a.load, Some(50));
        // Covered for ten minutes, with the CPU idle all that time: an
        // average over the rest is no load of now.
        put(&d.join("proc"), "stat", "cpu 150 0 150 90000 0 0 0 0\n");
        let later = t0 + Duration::from_secs(600);
        let (b, next) = s.sample(later);
        assert_eq!(b.load, Some(50), "the old value stands");
        assert_eq!(next, SOON, "the next comes soon");
        put(&d.join("proc"), "stat", "cpu 170 0 170 90060 0 0 0 0\n");
        let (c, next) = s.sample(later + SOON);
        assert_eq!(c.load, Some(40));
        assert_eq!(next, EVERY);
    }

    #[test]
    fn maker_and_model_are_read_once() {
        let d = TestDir::new("hardware", "once");
        whole_machine(&d);
        let mut s = Sampler::new(Roots::under(&d));
        let now = Instant::now();
        let a = s.sample(now).0;
        put(&d.join("sys/class/dmi/id"), "product_name", "changed\n");
        assert_eq!(s.sample(now).0.model, a.model);
    }

    #[test]
    fn a_machine_with_nothing_readable_has_nothing_to_show() {
        let d = TestDir::new("hardware", "empty");
        let (snap, _) = Sampler::new(Roots::under(&d)).sample(Instant::now());
        assert_eq!(snap, Snapshot::default());
        assert!(!snap.any());
    }

    #[test]
    fn the_thread_rests_while_it_is_off_and_wakes_once_when_something_appears() {
        let d = TestDir::new("hardware", "spawn");
        whole_machine(&d);
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on = Arc::new(AtomicBool::new(false));
        let counted = wakes.clone();
        let shared = spawn(Roots::under(&d), on.clone(), move || {
            counted.fetch_add(1, Ordering::Relaxed);
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(shared.lock().unwrap().is_none(), "off: nothing read");
        on.store(true, Ordering::Relaxed);
        let start = Instant::now();
        while !shared.lock().unwrap().as_ref().is_some_and(Snapshot::any) {
            assert!(start.elapsed() < Duration::from_secs(10), "no reading");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
    }
}
