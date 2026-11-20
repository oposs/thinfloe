//! The This machine tab: fixed facts about the machine, and their text for
//! the clipboard. The facts are read in a job thread each time the tab
//! opens (`read`); only the uptime moves while it is open, and it is
//! counted on from the reading rather than read again.

use crate::battery::sanitise;
use crate::bounded::read_text;
use crate::hardware::{self, Roots};
use crate::header::Header;
use crate::menu::Job;
use std::time::{Duration, Instant};

/// `/proc/cpuinfo` grows with the cores; a larger one is not read.
const MAX_CPUINFO: u64 = 256 * 1024;
/// `/proc/meminfo` is a few KiB.
const MAX_MEMINFO: u64 = 64 * 1024;
/// `/proc/uptime` and `osrelease` are one short line.
const MAX_LINE: u64 = 256;
/// The longest kernel release kept.
const MAX_KERNEL: usize = 64;

/// What was read when the tab opened; `None` for what the machine does not
/// tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub maker: Option<String>,
    pub model: Option<String>,
    pub cpu_model: Option<String>,
    pub cores: Option<usize>,
    /// RAM in bytes.
    pub ram_total: Option<u64>,
    pub kernel: Option<String>,
    /// Seconds since boot at the reading.
    pub uptime: Option<u64>,
}

/// The facts from `/proc` and `/sys` under `roots`.
pub fn read(roots: &Roots) -> Facts {
    let proc_ = |name: &str, max: u64| read_text(&roots.proc_.join(name), max);
    let (maker, model) = hardware::machine(roots);
    let cpuinfo = proc_("cpuinfo", MAX_CPUINFO);
    // Tested before `sanitise`, which names an empty text "unknown device".
    let kernel = proc_("sys/kernel/osrelease", MAX_LINE)
        .filter(|t| !t.trim().is_empty())
        .map(|t| sanitise(t.trim()).chars().take(MAX_KERNEL).collect());
    Facts {
        maker,
        model,
        cpu_model: cpuinfo.as_deref().and_then(hardware::cpu_model),
        cores: cpuinfo.as_deref().and_then(hardware::cpu_count),
        ram_total: proc_("meminfo", MAX_MEMINFO)
            .as_deref()
            .and_then(hardware::parse_meminfo)
            .map(|u| u.total),
        kernel,
        uptime: proc_("uptime", MAX_LINE).as_deref().and_then(hardware::parse_uptime),
    }
}

/// The title of the tab's first panel.
pub const TITLE: &str = "This machine";
/// The title of the panel of installed add-ons.
pub const ADDONS_TITLE: &str = "Add-ons";

/// The machine's rows, label and value, in the order shown; what is
/// unknown is left out. `uptime_now` is the uptime counted on to this
/// frame. The add-ons, the network and the graphics are panels of their
/// own (`Menu::machine_panels`).
pub fn rows(f: &Facts, uptime_now: Option<u64>, header: &Header, usb_boot: bool) -> Vec<(String, String)> {
    let host = Some(header.host.clone()).filter(|h| !h.is_empty());
    let boot = if usb_boot { "USB" } else { "Netboot" };
    [
        ("Maker", f.maker.clone()),
        ("Model", f.model.clone()),
        ("CPU model", f.cpu_model.clone()),
        ("Cores", f.cores.map(|c| c.to_string())),
        ("RAM", f.ram_total.map(crate::status::hardware::size)),
        ("ThinFloe version", header.version.clone()),
        ("Kernel", f.kernel.clone()),
        ("Boot", Some(boot.to_string())),
        ("Hostname", host),
        ("Uptime", uptime_now.map(crate::status::hardware::uptime)),
    ]
    .into_iter()
    .filter_map(|(label, value)| Some((label.to_string(), value?)))
    .collect()
}

/// The tab's state between frames.
#[derive(Debug, Default)]
pub struct MachineTab {
    /// The tab was open at the last poll.
    open: bool,
    /// The last reading and when it answered.
    facts: Option<(Facts, Instant)>,
    /// How the last copy went, until the tab opens again.
    pub copied: Option<Result<(), String>>,
    /// A copy job runs: Copy is off until it answers.
    copying: bool,
}

impl MachineTab {
    /// Told each tick whether the tab is open; asks for the facts each time
    /// it opens.
    pub fn poll(&mut self, open: bool) -> Option<Job> {
        let opened = open && !self.open;
        self.open = open;
        if opened {
            // "Copied" belongs to the visit it answered.
            self.copied = None;
        }
        opened.then_some(Job::MachineFacts)
    }

    pub fn facts_done(&mut self, f: Facts, now: Instant) {
        self.facts = Some((f, now));
    }

    /// The last reading; `None` until the first answer.
    pub fn facts(&self) -> Option<&Facts> {
        self.facts.as_ref().map(|(f, _)| f)
    }

    /// The uptime at `now`: the reading plus the time since.
    pub fn uptime_at(&self, now: Instant) -> Option<u64> {
        let (f, at) = self.facts.as_ref()?;
        f.uptime
            .map(|u| u.saturating_add(now.saturating_duration_since(*at).as_secs()))
    }

    /// How long until the uptime row shows a new minute, while the tab is
    /// open; `None` otherwise, so the menu does not wake for it.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        if !self.open {
            return None;
        }
        let (f, at) = self.facts.as_ref()?;
        let since = now.saturating_duration_since(*at).as_millis() as u64;
        let ms = f.uptime?.saturating_mul(1000).saturating_add(since);
        // A little past the minute, so the frame reads the new one.
        Some(Duration::from_millis(60_000 - ms % 60_000 + 20))
    }

    /// A copy job was started; `false` when one runs already, and no
    /// second may start.
    pub fn start_copy(&mut self) -> bool {
        !std::mem::replace(&mut self.copying, true)
    }

    pub fn copying(&self) -> bool {
        self.copying
    }

    pub fn copy_done(&mut self, r: Result<(), String>) {
        self.copying = false;
        self.copied = Some(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;
    use std::path::Path;

    fn full() -> Facts {
        Facts {
            maker: Some("HP".into()),
            model: Some("t640 Thin Client".into()),
            cpu_model: Some("AMD Ryzen R1505G".into()),
            cores: Some(4),
            ram_total: Some(8 * 1024 * 1024 * 1024),
            kernel: Some("7.0.0-38-generic".into()),
            uptime: Some(100),
        }
    }

    fn header() -> Header {
        Header {
            host: "thinfloe-a1b2c3".into(),
            version: Some("4.4.0".into()),
        }
    }

    fn labels(rows: &[(String, String)]) -> Vec<&str> {
        rows.iter().map(|(l, _)| l.as_str()).collect()
    }

    #[test]
    fn rows_are_in_order_and_unknown_ones_left_out() {
        let r = rows(&full(), Some(3 * 3600 + 120), &header(), true);
        assert_eq!(
            labels(&r),
            [
                "Maker",
                "Model",
                "CPU model",
                "Cores",
                "RAM",
                "ThinFloe version",
                "Kernel",
                "Boot",
                "Hostname",
                "Uptime",
            ]
        );
        let value = |r: &[(String, String)], l: &str| r.iter().find(|(k, _)| k == l).map(|(_, v)| v.clone());
        assert_eq!(value(&r, "Maker").as_deref(), Some("HP"));
        assert_eq!(value(&r, "Cores").as_deref(), Some("4"));
        assert_eq!(value(&r, "RAM").as_deref(), Some("8.0 GiB"));
        assert_eq!(value(&r, "ThinFloe version").as_deref(), Some("4.4.0"));
        assert_eq!(value(&r, "Boot").as_deref(), Some("USB"));
        assert_eq!(value(&r, "Hostname").as_deref(), Some("thinfloe-a1b2c3"));
        assert_eq!(value(&r, "Uptime").as_deref(), Some("3 h 2 m"));

        let no_maker = Facts { maker: None, ..full() };
        let r = rows(&no_maker, Some(100), &header(), false);
        assert!(!labels(&r).contains(&"Maker"), "{r:?}");
        assert_eq!(value(&r, "Boot").as_deref(), Some("Netboot"));

        // Nothing known but what the menu knows itself.
        let r = rows(&Facts::default(), None, &Header::default(), false);
        assert_eq!(labels(&r), ["Boot"]);
        // Less than a GiB is in MiB.
        let small = Facts {
            ram_total: Some(512 * 1024 * 1024),
            ..Facts::default()
        };
        assert_eq!(
            value(&rows(&small, None, &Header::default(), true), "RAM").as_deref(),
            Some("512 MiB")
        );
    }

    fn put(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn facts_come_from_proc_and_sys() {
        let d = TestDir::new("machine", "facts");
        let cpu = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5-7260U\n\n";
        put(&d.join("proc"), "cpuinfo", &cpu.repeat(4));
        put(
            &d.join("proc"),
            "meminfo",
            "MemTotal:        8048836 kB\nMemFree:  100 kB\nMemAvailable:    4000000 kB\n",
        );
        put(&d.join("proc"), "uptime", "3725.18 7000.00\n");
        put(&d.join("proc/sys/kernel"), "osrelease", "7.0.0-38-generic\n");
        put(&d.join("sys/class/dmi/id"), "sys_vendor", "Intel Corporation\n");
        put(&d.join("sys/class/dmi/id"), "product_name", "NUC7i5BNH\n");
        assert_eq!(
            read(&Roots::under(&d)),
            Facts {
                maker: Some("Intel".into()),
                model: Some("NUC7i5BNH".into()),
                cpu_model: Some("Intel(R) Core(TM) i5-7260U".into()),
                cores: Some(4),
                ram_total: Some(8_048_836 * 1024),
                kernel: Some("7.0.0-38-generic".into()),
                uptime: Some(3725),
            }
        );
        // An empty tree knows nothing, nor do empty files.
        let none = TestDir::new("machine", "empty");
        assert_eq!(read(&Roots::under(&none)), Facts::default());
        for name in ["cpuinfo", "meminfo", "uptime"] {
            put(&none.join("proc"), name, "\n");
        }
        put(&none.join("proc/sys/kernel"), "osrelease", " \n");
        assert_eq!(read(&Roots::under(&none)), Facts::default());
    }

    #[test]
    fn the_tab_reads_on_each_opening_and_counts_the_uptime_on() {
        let mut tab = MachineTab::default();
        let t0 = Instant::now();
        assert_eq!(tab.poll(false), None);
        assert_eq!(tab.poll(true), Some(Job::MachineFacts));
        assert_eq!(tab.poll(true), None, "asked already");
        assert_eq!(tab.next_change(t0), None, "no reading yet");
        tab.facts_done(
            Facts {
                uptime: Some(119),
                ..Facts::default()
            },
            t0,
        );
        assert_eq!(tab.uptime_at(t0), Some(119));
        assert_eq!(tab.uptime_at(t0 + Duration::from_secs(61)), Some(180));
        // 119 s: the next minute is a second away, then a minute each.
        let next = tab.next_change(t0).unwrap();
        assert!(
            next >= Duration::from_secs(1) && next < Duration::from_secs(2),
            "{next:?}"
        );
        let next = tab.next_change(t0 + Duration::from_secs(1)).unwrap();
        assert!(
            next >= Duration::from_secs(60) && next < Duration::from_secs(61),
            "{next:?}"
        );
        tab.copy_done(Ok(()));
        assert_eq!(tab.copied, Some(Ok(())));
        // Closed: no wakes, and the next opening reads again and forgets
        // the copy.
        assert_eq!(tab.poll(false), None);
        assert_eq!(tab.next_change(t0), None);
        assert_eq!(tab.poll(true), Some(Job::MachineFacts));
        assert_eq!(tab.copied, None);
        assert!(tab.facts().is_some(), "the old reading stands until the new one");
    }
}
