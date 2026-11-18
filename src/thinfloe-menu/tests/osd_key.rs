//! `thinfloe-menu --osd-key <action>` as labwc runs it: without an OSD
//! process the volume keys still act, through `wpctl` directly, the
//! brightness keys do nothing, and labwc never waits.

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
        .env("TF_WPCTL", d.join("wpctl"))
        .env("TF_DDCUTIL", d.join("ddcutil"))
        .output()
        .unwrap()
}

#[test]
fn osd_key_without_a_server_runs_wpctl_itself() {
    let d = Dir::new("wpctl");
    d.fake("wpctl", "[ \"$1\" = get-volume ] && echo 'Volume: 0.50'; exit 0");
    let started = Instant::now();
    let out = osd_key(&d.0, &["volume-up"]);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(out.status.code(), Some(0));
    assert!(
        d.log()
            .lines()
            .any(|l| l == "set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"),
        "{}",
        d.log()
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("thinfloe-menu --osd-key: no OSD running; volume-up done without a display"),
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
    d.fake("wpctl", "exit 0");
    let out = Command::new(MENU)
        .args(["--osd-key", "mic-mute"])
        .env_clear()
        .env("TF_OSD_SOCKET", &path)
        .env("TF_WPCTL", d.0.join("wpctl"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let mut buf = [0u8; 64];
    let n = server.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"mic-mute\n");
    // The OSD acts, not the client.
    assert_eq!(d.log(), "");
}
