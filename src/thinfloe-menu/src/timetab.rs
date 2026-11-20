//! The time half of the Network tab's status: which zone is in use and
//! where it comes from, and whether chrony keeps the clock in step. The
//! reads run as the user (chronyc answers monitoring requests from
//! localhost); the lines the tab shows are made here, pure.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::log::Feed;

/// How long one chronyc call may take; the tab polls every few seconds.
pub const CHRONYC_TIMEOUT: Duration = Duration::from_secs(2);
const CHRONYC_BYTES: usize = 64 * 1024;
/// At most this many servers are listed.
pub const SERVERS_MAX: usize = 16;
const NAME_MAX: usize = 64;

/// Where the time settings and their state live, and the two tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimePaths {
    /// `sudo -n /usr/bin/thinfloe-time`; `TF_TIME_CMD` (space-separated).
    pub tool: Vec<String>,
    /// `chronyc`; `TF_CHRONYC` (space-separated).
    pub chronyc: Vec<String>,
    pub localtime: PathBuf,
    /// The root copy of the menu's setting.
    pub time_file: PathBuf,
    /// The zone dhcp-timezone set last.
    pub dhcp_state: PathBuf,
}

fn cmd_from_env(var: &str, default: &[&str]) -> Vec<String> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v.split_whitespace().map(String::from).collect(),
        _ => default.iter().map(|s| s.to_string()).collect(),
    }
}

impl TimePaths {
    pub fn system() -> TimePaths {
        TimePaths {
            tool: cmd_from_env("TF_TIME_CMD", &["sudo", "-n", "/usr/bin/thinfloe-time"]),
            chronyc: cmd_from_env("TF_CHRONYC", &["chronyc"]),
            localtime: PathBuf::from("/etc/localtime"),
            time_file: PathBuf::from("/etc/thinfloe/time"),
            dhcp_state: PathBuf::from("/run/thinfloe/state/dhcp-timezone"),
        }
    }

    /// Paths under `root` and tools that do not exist: for tests.
    pub fn under(root: &Path) -> TimePaths {
        TimePaths {
            tool: vec![root.join("no-thinfloe-time").to_string_lossy().into_owned()],
            chronyc: vec![root.join("no-chronyc").to_string_lossy().into_owned()],
            localtime: root.join("localtime"),
            time_file: root.join("time"),
            dhcp_state: root.join("dhcp-timezone"),
        }
    }
}

/// Where the zone in use comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ZoneSource {
    /// Set in the menu.
    Menu,
    /// The zone DHCP sent.
    Dhcp,
    /// Neither: UTC, or a zone set some other way.
    #[default]
    Default,
}

/// The source chrony follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    pub server: String,
    /// How far the system clock is from NTP time, in microseconds.
    pub offset_us: i64,
    pub stratum: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeStatus {
    pub zone: String,
    pub source: ZoneSource,
    pub synced: Option<Sync>,
    /// Name and state word, as `parse_sources` gives them.
    pub servers: Vec<(String, &'static str)>,
    /// chronyc answered; false: the time service does not run.
    pub answered: bool,
}

fn cut(s: &str) -> String {
    s.chars()
        .take(NAME_MAX)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// `chronyc -c -n tracking`: the source and the offset; `None` when chrony
/// is not synchronised or the line is not what chronyc prints.
pub fn parse_tracking(csv: &str) -> Option<Sync> {
    let f: Vec<&str> = csv.lines().next()?.split(',').collect();
    if f.len() < 14 || f[13].trim() == "Not synchronised" {
        return None;
    }
    let stratum: u32 = f[2].parse().ok()?;
    let offset_s: f64 = f[4].parse().ok()?;
    if stratum == 0 || f[1].is_empty() || !offset_s.is_finite() || offset_s.abs() > 1e9 {
        return None;
    }
    Some(Sync {
        server: cut(f[1]),
        offset_us: (offset_s * 1e6).round() as i64,
        stratum,
    })
}

/// `chronyc -c -N sources`: each server's name and state, at most
/// `SERVERS_MAX`; rows that are not a server (`#` local clocks) are skipped.
pub fn parse_sources(csv: &str) -> Vec<(String, &'static str)> {
    csv.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            if f.len() < 3 || !matches!(f[0], "^" | "=") || f[2].is_empty() {
                return None;
            }
            let state = match f[1] {
                "*" => "in use",
                "+" => "combined",
                "-" => "not combined",
                "?" => "unreachable",
                "x" => "may be wrong",
                "~" => "too variable",
                _ => "unknown",
            };
            Some((cut(f[2]), state))
        })
        .take(SERVERS_MAX)
        .collect()
}

/// The zone `/etc/localtime` names; UTC when it names none.
pub fn zone_name(localtime: &Path) -> String {
    std::fs::read_link(localtime)
        .ok()
        .and_then(|t| t.to_string_lossy().rsplit_once("zoneinfo/").map(|(_, n)| cut(n)))
        .unwrap_or_else(|| "UTC".to_string())
}

/// The `zone=` line of the menu's root file.
fn menu_zone(time_file: &Path) -> Option<String> {
    let text = crate::bounded::read_text(time_file, 4096)?;
    text.lines().find_map(|l| l.strip_prefix("zone=").map(str::to_string))
}

/// Where `zone` comes from, given the menu's zone and the one DHCP set.
pub fn source_of(zone: &str, menu: Option<&str>, dhcp: Option<&str>) -> ZoneSource {
    if menu == Some(zone) {
        ZoneSource::Menu
    } else if dhcp == Some(zone) {
        ZoneSource::Dhcp
    } else {
        ZoneSource::Default
    }
}

fn chronyc(paths: &TimePaths, args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    // Its complaints, every few seconds while chronyd is down, are not
    // worth the log: the tab says the service does not answer.
    let quiet = Arc::new(Feed::default());
    match crate::jobs::capture_killable(&paths.chronyc, &args, CHRONYC_BYTES, CHRONYC_TIMEOUT, &quiet, "time") {
        Ok((out, Some(0), _)) => Some(out),
        _ => None,
    }
}

/// Read the status now.
pub fn read(paths: &TimePaths) -> TimeStatus {
    let zone = zone_name(&paths.localtime);
    let dhcp = crate::bounded::read_text(&paths.dhcp_state, 256).map(|t| t.trim().to_string());
    let source = source_of(&zone, menu_zone(&paths.time_file).as_deref(), dhcp.as_deref());
    let tracking = chronyc(paths, &["-c", "-n", "tracking"]);
    let sources = chronyc(paths, &["-c", "-N", "sources"]);
    let servers = sources.as_deref().map(parse_sources).unwrap_or_default();
    let mut synced = tracking.as_deref().and_then(parse_tracking);
    // The name the server was given, not the address it resolved to.
    if let (Some(s), Some((name, _))) = (synced.as_mut(), servers.iter().find(|(_, st)| *st == "in use")) {
        s.server = name.clone();
    }
    TimeStatus {
        zone,
        source,
        synced,
        servers,
        answered: tracking.is_some(),
    }
}

/// `UTC+01:00` for the zone at `now`; `UTC` for UTC.
fn offset_text(zone: &str, now: jiff::Timestamp) -> String {
    let Ok(tz) = jiff::tz::db().get(zone) else {
        return "UTC".to_string();
    };
    let secs = tz.to_offset(now).seconds();
    if secs == 0 {
        return "UTC".to_string();
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, a % 3600 / 60)
}

/// The zone line below the selects.
pub fn zone_line(st: &TimeStatus, now: jiff::Timestamp) -> String {
    let off = offset_text(&st.zone, now);
    match st.source {
        ZoneSource::Menu => format!("In use: {} ({off}), set here.", st.zone),
        ZoneSource::Dhcp => format!("In use: {} ({off}), from DHCP.", st.zone),
        ZoneSource::Default if st.zone == "UTC" => "UTC: DHCP sends no time zone.".to_string(),
        ZoneSource::Default => format!("In use: {} ({off}).", st.zone),
    }
}

fn offset_words(us: i64) -> String {
    let a = us.unsigned_abs() as f64 / 1e6;
    if a < 0.001 {
        format!("{:.2} ms", a * 1000.0)
    } else if a < 1.0 {
        format!("{:.1} ms", a * 1000.0)
    } else {
        format!("{a:.1} s")
    }
}

/// Whether the clock is in step.
pub fn clock_line(st: &TimeStatus) -> String {
    if !st.answered {
        return "The time service does not answer.".to_string();
    }
    match &st.synced {
        Some(s) => format!("Synchronised with {}, {} off.", s.server, offset_words(s.offset_us)),
        None => "Not synchronised yet.".to_string(),
    }
}

/// The servers chrony uses, with the states worth naming.
pub fn servers_line(st: &TimeStatus) -> Option<String> {
    if !st.answered {
        return None;
    }
    if st.servers.is_empty() {
        return Some("Servers: none.".to_string());
    }
    let names: Vec<String> = st
        .servers
        .iter()
        .map(|(n, s)| match *s {
            "combined" | "not combined" => n.clone(),
            s => format!("{n} ({s})"),
        })
        .collect();
    Some(format!("Servers: {}", names.join(", ")))
}

/// The note when the session has its own `TZ`.
pub fn session_tz_note(tz: Option<&str>) -> Option<String> {
    let tz = tz.filter(|t| !t.is_empty())?;
    Some(format!(
        "This session uses TZ={} from ~/.config/thinfloe/environment.",
        cut(tz)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded on the owner's thin client, 2026-11-20.
    const TRACKING: &str = "B97DBE7B,185.125.190.123,3,1795183100.380211562,0.000034817,-0.000089137,0.000034945,2.893,-0.015,0.092,0.018956458,0.000610422,129.8,Normal\n";
    const SOURCES: &str = "^,+,1.ntp.ubuntu.com,2,10,377,127,-0.000093015,-0.000181889,0.010613814\n\
^,*,2.ntp.ubuntu.com,2,7,377,104,-0.000004743,-0.000093880,0.009920803\n\
^,-,3.ntp.ubuntu.com,2,9,377,2567,0.000113766,0.000119861,0.064191699\n\
^,?,195.176.26.215,0,6,0,-,0.0,0.0,0.0\n\
#,*,PPS,0,4,377,1,0.0,0.0,0.0\n";

    #[test]
    fn tracking_synced() {
        let s = parse_tracking(TRACKING).unwrap();
        assert_eq!(s.server, "185.125.190.123");
        assert_eq!(s.stratum, 3);
        assert_eq!(s.offset_us, 35);
    }

    #[test]
    fn tracking_not_synced() {
        let t = "00000000,,0,0.000000000,0.000000000,0.000000000,0.000000000,0.000,0.000,0.000,1.000000000,1.000000000,0.0,Not synchronised\n";
        assert_eq!(parse_tracking(t), None);
        assert_eq!(parse_tracking(""), None);
        assert_eq!(parse_tracking("a,b,c\n"), None);
    }

    #[test]
    fn sources_states() {
        let s = parse_sources(SOURCES);
        assert_eq!(
            s,
            [
                ("1.ntp.ubuntu.com".to_string(), "combined"),
                ("2.ntp.ubuntu.com".to_string(), "in use"),
                ("3.ntp.ubuntu.com".to_string(), "not combined"),
                ("195.176.26.215".to_string(), "unreachable"),
            ]
        );
        let many: String = (0..40).map(|i| format!("^,?,10.0.0.{i},0,6,0,-,0,0,0\n")).collect();
        assert_eq!(parse_sources(&many).len(), SERVERS_MAX);
    }

    #[test]
    fn chronyc_failure_gives_no_answer_line() {
        let st = TimeStatus::default();
        assert_eq!(clock_line(&st), "The time service does not answer.");
        assert_eq!(servers_line(&st), None);
        let d = crate::testdir::TestDir::new("timetab", "nochronyc");
        let st = read(&TimePaths::under(&d));
        assert!(!st.answered);
        assert_eq!(st.zone, "UTC");
    }

    #[test]
    fn clock_and_server_lines() {
        let st = TimeStatus {
            zone: "UTC".into(),
            synced: Some(Sync {
                server: "2.ntp.ubuntu.com".into(),
                offset_us: -35,
                stratum: 3,
            }),
            servers: parse_sources(SOURCES),
            answered: true,
            ..TimeStatus::default()
        };
        assert_eq!(clock_line(&st), "Synchronised with 2.ntp.ubuntu.com, 0.03 ms off.");
        assert_eq!(
            servers_line(&st).unwrap(),
            "Servers: 1.ntp.ubuntu.com, 2.ntp.ubuntu.com (in use), 3.ntp.ubuntu.com, 195.176.26.215 (unreachable)"
        );
        let unsynced = TimeStatus {
            answered: true,
            ..TimeStatus::default()
        };
        assert_eq!(clock_line(&unsynced), "Not synchronised yet.");
        assert_eq!(servers_line(&unsynced).unwrap(), "Servers: none.");
        assert_eq!(offset_words(250_000), "250.0 ms");
        assert_eq!(offset_words(-3_000_000), "3.0 s");
    }

    #[test]
    fn zone_lines() {
        let winter: jiff::Timestamp = "2026-01-15T12:00:00Z".parse().unwrap();
        let st = |zone: &str, source| TimeStatus {
            zone: zone.into(),
            source,
            ..TimeStatus::default()
        };
        assert_eq!(
            zone_line(&st("Europe/Zurich", ZoneSource::Menu), winter),
            "In use: Europe/Zurich (UTC+01:00), set here."
        );
        assert_eq!(
            zone_line(&st("America/New_York", ZoneSource::Dhcp), winter),
            "In use: America/New_York (UTC-05:00), from DHCP."
        );
        assert_eq!(zone_line(&st("UTC", ZoneSource::Default), winter), "UTC: DHCP sends no time zone.");
        assert_eq!(
            zone_line(&st("Asia/Kolkata", ZoneSource::Default), winter),
            "In use: Asia/Kolkata (UTC+05:30)."
        );
    }

    #[test]
    fn sources_of_the_zone() {
        assert_eq!(source_of("Europe/Zurich", Some("Europe/Zurich"), Some("Europe/Zurich")), ZoneSource::Menu);
        assert_eq!(source_of("Europe/Zurich", None, Some("Europe/Zurich")), ZoneSource::Dhcp);
        assert_eq!(source_of("Europe/Zurich", Some("Asia/Tokyo"), None), ZoneSource::Default);
    }

    #[test]
    fn zone_and_source_read_from_files() {
        let d = crate::testdir::TestDir::new("timetab", "files");
        let p = TimePaths::under(&d);
        std::os::unix::fs::symlink("../usr/share/zoneinfo/Europe/Zurich", &p.localtime).unwrap();
        std::fs::write(&p.dhcp_state, "Europe/Zurich\n").unwrap();
        let st = read(&p);
        assert_eq!((st.zone.as_str(), st.source), ("Europe/Zurich", ZoneSource::Dhcp));
        std::fs::write(&p.time_file, "zone=Europe/Zurich\nntp=10.0.0.1\n").unwrap();
        assert_eq!(read(&p).source, ZoneSource::Menu);
    }

    #[test]
    fn session_tz_note_text() {
        assert_eq!(
            session_tz_note(Some("Europe/Berlin")).unwrap(),
            "This session uses TZ=Europe/Berlin from ~/.config/thinfloe/environment."
        );
        assert_eq!(session_tz_note(Some("")), None);
        assert_eq!(session_tz_note(None), None);
    }
}
