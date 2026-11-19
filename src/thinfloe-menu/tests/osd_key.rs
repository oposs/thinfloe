//! `thinfloe-menu --osd-key <action>` as labwc runs it: without an OSD
//! process the volume keys still act on every speaker and microphone,
//! through `pactl` directly, the brightness keys do nothing, and labwc
//! never waits.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const MENU: &str = env!("CARGO_BIN_EXE_thinfloe-menu");

/// A fresh directory under $TMPDIR, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("thinfloe-menu-osd-key-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    /// An executable shell script that logs its arguments to `log`, then
    /// runs `body`.
    fn fake(&self, name: &str, body: &str) -> PathBuf {
        let p = self.0.join(name);
        let log = self.0.join("log");
        fs::write(&p, format!("#!/bin/sh\necho \"$*\" >>'{}'\n{body}\n", log.display())).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn log(&self) -> String {
        fs::read_to_string(self.0.join("log")).unwrap_or_default()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn osd_key(d: &Path, args: &[&str]) -> Output {
    Command::new(MENU)
        .arg("--osd-key")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TF_OSD_SOCKET", d.join("missing.sock"))
        .env("TF_PACTL", d.join("pactl"))
        .env("TF_DDCUTIL", d.join("ddcutil"))
        .output()
        .unwrap()
}

/// A fake `pactl`: answers the device lists from `sinks.json` and
/// `sources.json` and the defaults from `default-sink`/`default-source`,
/// all in `d`.
fn fake_pactl(d: &Dir, sinks: &str, sources: &str, default_sink: &str, default_source: &str) {
    for (f, text) in [
        ("sinks.json", sinks),
        ("sources.json", sources),
        ("default-sink", default_sink),
        ("default-source", default_source),
    ] {
        fs::write(d.0.join(f), text).unwrap();
    }
    let dir = d.0.display();
    d.fake(
        "pactl",
        &format!(
            "case \"$*\" in\n\
             '-f json list sinks') cat '{dir}/sinks.json';;\n\
             '-f json list sources') cat '{dir}/sources.json';;\n\
             get-default-sink) cat '{dir}/default-sink';;\n\
             get-default-source) cat '{dir}/default-source';;\n\
             esac"
        ),
    );
}

/// The real `pactl -f json list sinks` fixture with both sinks at 40 %.
fn sinks_at_40() -> String {
    include_str!("fixtures/pactl-sinks.json")
        .replace("\"45%\"", "\"40%\"")
        .replace("\"47%\"", "\"40%\"")
        .replace("\"100%\"", "\"40%\"")
}

#[test]
fn osd_key_without_a_server_runs_pactl_itself() {
    let d = Dir::new("pactl");
    // tf_null_a at 40 %, tf_null_b at 100 %.
    let sinks = include_str!("fixtures/pactl-sinks.json")
        .replace("\"45%\"", "\"40%\"")
        .replace("\"47%\"", "\"40%\"");
    fake_pactl(&d, &sinks, "[]", "tf_null_a\n", "");
    let started = Instant::now();
    let out = osd_key(&d.0, &["volume-up"]);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(out.status.code(), Some(0));
    let log = d.log();
    let sets: Vec<&str> = log.lines().filter(|l| l.starts_with("set-")).collect();
    assert_eq!(
        sets,
        ["set-sink-volume tf_null_a 45%", "set-sink-volume tf_null_b 45%"],
        "{log}"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("thinfloe-menu --osd-key: no OSD running; volume-up done without a display"),
        "{err}"
    );
}

#[test]
fn osd_key_without_a_server_mutes_every_speaker_and_microphone() {
    let d = Dir::new("pamute");
    // Both sinks unmuted at 40 %; the default sink unmuted.
    let sinks = sinks_at_40().replace("\"mute\":true", "\"mute\":false");
    fake_pactl(
        &d,
        &sinks,
        include_str!("fixtures/pactl-sources.json"),
        "tf_null_a\n",
        "tf_mic\n",
    );
    assert_eq!(osd_key(&d.0, &["volume-mute"]).status.code(), Some(0));
    assert_eq!(osd_key(&d.0, &["mic-mute"]).status.code(), Some(0));
    let log = d.log();
    let sets: Vec<&str> = log.lines().filter(|l| l.starts_with("set-")).collect();
    // The monitor sources are never touched.
    assert_eq!(
        sets,
        [
            "set-sink-mute tf_null_a 1",
            "set-sink-mute tf_null_b 1",
            "set-source-mute tf_mic 1"
        ],
        "{log}"
    );
}

#[test]
fn osd_key_without_a_sound_server_says_so_and_exits_0() {
    let d = Dir::new("paserver");
    d.fake("pactl", "echo 'Connection failure: Connection refused' >&2; exit 1");
    let out = osd_key(&d.0, &["volume-down"]);
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no OSD running; volume-down: ") && err.contains("Connection refused"),
        "{err}"
    );
}

#[test]
fn osd_key_without_a_server_leaves_brightness_alone() {
    let d = Dir::new("ddc");
    d.fake("ddcutil", "exit 0");
    for a in ["brightness-up", "brightness-down"] {
        let out = osd_key(&d.0, &[a]);
        assert_eq!(out.status.code(), Some(0));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("no OSD running; brightness not changed"), "{err}");
    }
    assert_eq!(d.log(), "");
}

#[test]
fn osd_key_rejects_an_unknown_action() {
    let d = Dir::new("unknown");
    let out = osd_key(&d.0, &["volume"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("thinfloe-menu --osd-key: unknown action 'volume'"),
        "{err}"
    );
    assert_eq!(osd_key(&d.0, &[]).status.code(), Some(2));
}

#[test]
fn osd_key_reaches_a_listening_osd() {
    let d = Dir::new("server");
    let path = d.0.join("osd.sock");
    let server = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
    d.fake("pactl", "exit 0");
    let out = Command::new(MENU)
        .args(["--osd-key", "mic-mute"])
        .env_clear()
        .env("TF_OSD_SOCKET", &path)
        .env("TF_PACTL", d.0.join("pactl"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let mut buf = [0u8; 64];
    let n = server.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"mic-mute\n");
    // The OSD acts, not the client.
    assert_eq!(d.log(), "");
}

#[test]
fn osd_key_without_a_readable_default_sets_nothing() {
    let d = Dir::new("padefault");
    fake_pactl(&d, &sinks_at_40(), "[]", "tf_null_a\n", "");
    // get-default-sink fails before the case answers it.
    let body = fs::read_to_string(d.0.join("pactl")).unwrap().replacen(
        "case",
        "[ \"$1\" = get-default-sink ] && { echo 'Connection failure: Timeout' >&2; exit 1; }\ncase",
        1,
    );
    fs::write(d.0.join("pactl"), body).unwrap();
    let out = osd_key(&d.0, &["volume-up"]);
    assert_eq!(out.status.code(), Some(0));
    let log = d.log();
    assert!(!log.contains("set-"), "{log}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no OSD running; volume-up: ") && err.contains("Timeout"),
        "{err}"
    );
}
