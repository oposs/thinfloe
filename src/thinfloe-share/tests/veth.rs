//! The daemon against Owly's own sender and receiver on a veth pair, as
//! Owly's AirDrop tests do: each test runs itself again under
//! `unshare --user --map-root-user --net`, so no root and no radio are needed.
//! The fake helper says the radio is up on `owb`; the test is on `owa`.
//! Where unprivileged user namespaces are blocked, each test writes
//! "SKIP: user namespaces blocked" to stderr (past the test harness's capture, so the
//! line shows in a passing run) and passes, also when `CI` is set; with
//! `TF_REQUIRE_USERNS=1` it fails instead.

use std::net::{Ipv6Addr, SocketAddrV6};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use owly_airdrop::driver::mdns::{Browse, Mdns};
use owly_airdrop::driver::receiver::{Options, Receiver, ReceiverEvent};
use owly_airdrop::driver::sender::{Payload, Sender};
use owly_airdrop::driver::tls::Identity;
use owly_airdrop::proto::mdns::{BrowseEvent, RECEIVER_FLAGS, Service};
use owly_airdrop::proto::receiver::Config;
use owly_airdrop::proto::sender::{Config as SenderConfig, Outcome};

const BIN: &str = env!("CARGO_BIN_EXE_thinfloe-share");
const ENV: &str = "TF_SHARE_TEST_NETNS";

#[derive(Clone, Copy)]
struct End {
    index: u32,
    addr: Ipv6Addr,
}

/// In the test process: run the test again in a namespace and check it; None.
/// In the namespace: the veth pair (a, b).
fn enter(test: &str) -> Option<(End, End)> {
    if std::env::var_os(ENV).is_some() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        return Some(setup());
    }
    let probe = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "true"])
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        if std::env::var_os("TF_REQUIRE_USERNS").is_some_and(|v| v == "1") {
            panic!("{test}: user namespaces are blocked and TF_REQUIRE_USERNS=1 needs them");
        }
        // Not eprintln!: the harness captures it and shows it only for a failed test.
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "SKIP: user namespaces blocked ({test})");
        return None;
    }
    let status = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(ENV, "1")
        .status()
        .unwrap();
    assert!(status.success(), "{test} failed in its network namespace");
    None
}

fn ip(args: &[&str]) {
    let ok = Command::new("ip").args(args).status().unwrap().success();
    assert!(ok, "ip {args:?} failed");
}

fn setup() -> (End, End) {
    ip(&["link", "set", "lo", "up"]);
    ip(&["link", "add", "owa", "type", "veth", "peer", "name", "owb"]);
    for (name, mac) in [("owa", "02:00:00:00:00:0a"), ("owb", "02:00:00:00:00:0b")] {
        std::fs::write(format!("/proc/sys/net/ipv6/conf/{name}/accept_dad"), "0").unwrap();
        ip(&["link", "set", name, "address", mac]);
    }
    ip(&["link", "set", "owa", "up"]);
    ip(&["link", "set", "owb", "up"]);
    // The kernel adds an address tentative (IFA_F_TENTATIVE, 0x40) and clears the
    // flag from a work queue, with DAD off too; a bind in between fails with
    // EADDRNOTAVAIL (#90). So wait until the flag is gone, not only for the line.
    const TENTATIVE: u32 = 0x40;
    const DADFAILED: u32 = 0x08;
    let end = |name: &str, addr: &str| {
        let addr: Ipv6Addr = addr.parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut flags = None;
        loop {
            let text = std::fs::read_to_string("/proc/net/if_inet6").unwrap();
            let hex = format!("{:032x}", u128::from(addr));
            if let Some(l) = text
                .lines()
                .find(|l| l.starts_with(&hex) && l.ends_with(name))
            {
                let f: Vec<&str> = l.split_whitespace().collect();
                let index = u32::from_str_radix(f[1], 16).unwrap();
                let bits = u32::from_str_radix(f[4], 16).unwrap();
                assert!(bits & DADFAILED == 0, "{name}: DAD failed on {addr}");
                if bits & TENTATIVE == 0 {
                    return End { index, addr };
                }
                flags = Some(bits);
            }
            assert!(
                Instant::now() < deadline,
                "{name} got no usable link-local address (flags {flags:02x?})"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    (end("owa", "fe80::ff:fe00:a"), end("owb", "fe80::ff:fe00:b"))
}

struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
            .join(format!("veth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["run", "home", "media"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("share"), "enabled=true\nname=Front desk\n").unwrap();
        Env { dir }
    }

    /// The fake helper: the radio is up on owb with these flags until stdin closes.
    fn spawn(&self, flags: &str) -> Child {
        self.spawn_with(flags, "cat >/dev/null")
    }

    /// The fake helper with `tail` after its up line (`cat >/dev/null` waits for EOF).
    fn spawn_with(&self, flags: &str, tail: &str) -> Child {
        let p = self.dir.join("helper.sh");
        let script = format!(
            "#!/bin/sh\nprintf 'up\\towb\\texclusive\\t6\\t{flags}\\twlx1\\trtw88_8822bu\\n'\n{tail}\n"
        );
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Command::new(BIN)
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("HOME", self.dir.join("home"))
            .env("TF_SHARE_CONFIG", self.dir.join("share"))
            .env("TF_SHARE_RADIO", &p)
            .env("TF_SHARE_ROOT", self.dir.join("media"))
            .env("TF_OSD_SOCKET", self.dir.join("osd.sock"))
            // No owlyd here: the daemon reads owb's own address.
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                "unix:path=/nonexistent/floedrop-test-bus",
            )
            // A group of its own, with the helper in it, as the menu starts it.
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn state(&self) -> String {
        std::fs::read_to_string(self.dir.join("run/thinfloe-share.state")).unwrap_or_default()
    }

    fn media(&self, rel: &str) -> PathBuf {
        self.dir.join("media").join(rel)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn until(limit: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while !cond() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn stop(mut child: Child) {
    // SAFETY: kill with a valid pid and signal number.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn receive_from_owly_sender() {
    let Some((a, b)) = enter("receive_from_owly_sender") else {
        return;
    };
    runtime().block_on(async {
        let env = Env::new("recv");
        let child = env.spawn("no-ble");
        until(Duration::from_secs(10), "ready", || {
            env.state().starts_with("ready\towb\t")
        })
        .await;
        assert_eq!(
            env.state(),
            "ready\towb\texclusive\t6\tno-ble,no-peers\twlx1\trtw88_8822bu\n"
        );
        assert!(!env.media("outbox").exists(), "no outbox without Bluetooth");

        let photo = env.dir.join("IMG_0042.HEIC");
        std::fs::write(&photo, b"not really a photo").unwrap();
        let sender = Sender::new(SenderConfig::new("Jane's iPhone", "0a0b0c0d0e0f")).unwrap();
        let to = SocketAddrV6::new(b.addr, 8771, 0, a.index);
        let found = sender.discover(to).await.unwrap().expect("listed");
        assert_eq!(found.computer_name, "Front desk");
        let outcome = sender
            .send(to, &Payload::Files(vec![photo]), RECEIVER_FLAGS, None)
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Delivered);
        let landed = env.media("inbox/Jane's iPhone/IMG_0042.HEIC");
        until(Duration::from_secs(10), "the file in the inbox", || {
            landed.exists()
        })
        .await;
        assert_eq!(std::fs::read(&landed).unwrap(), b"not really a photo");
        // The transfer ended: no receiving line is left.
        until(Duration::from_secs(5), "no receiving line", || {
            !env.state().contains("\treceiving\t")
        })
        .await;
        let staged: Vec<_> = std::fs::read_dir(env.media(".incoming")).unwrap().collect();
        assert!(staged.is_empty(), "staging is empty again");

        // SIGHUP: a new name answers the next /Discover.
        std::fs::write(env.dir.join("share"), "enabled=true\nname=Back office\n").unwrap();
        // SAFETY: kill with a valid pid and signal number.
        unsafe { libc::kill(child.id() as i32, libc::SIGHUP) };
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(r)) = sender.discover(to).await
                && r.computer_name == "Back office"
            {
                break;
            }
            assert!(Instant::now() < end, "the new name was not taken");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        stop(child);
        assert!(env.state().starts_with("error\t"));
    });
}

/// The next `Found` of a receiver within `limit`; its instance.
async fn next_found(browse: &mut Browse, limit: Duration) -> String {
    let end = tokio::time::Instant::now() + limit;
    loop {
        match tokio::time::timeout_at(end, browse.events.recv()).await {
            Ok(Some(BrowseEvent::Found(f))) => return f.instance,
            Ok(Some(BrowseEvent::Lost(_))) => {}
            Ok(None) => panic!("the browse ended"),
            Err(_) => panic!("no receiver found within {limit:?}"),
        }
    }
}

/// The daemon's goodbye on the menu's SIGTERM to the whole group. It guards neither stop
/// order: the fake helper below has the helper's wait for EOF built in (the real helper's is
/// pinned by the unit test `a_sigterm_waits_for_the_daemons_eof` in radio.rs), and a daemon
/// that closed stdin before the goodbye sends it faster than the shell takes the link down.
#[test]
fn the_service_is_withdrawn_before_the_radio_goes() {
    let Some((a, _b)) = enter("the_service_is_withdrawn_before_the_radio_goes") else {
        return;
    };
    runtime().block_on(async {
        let env = Env::new("goodbye");
        // The helper's new order, built in: a SIGTERM to the group does not end it, the
        // radio goes away when its stdin closes.
        let child = env.spawn_with(
            "no-ble",
            "trap '' TERM INT\ncat >/dev/null\nip link set owb down",
        );
        let mut browse = Mdns::new(a.index).browse().await.unwrap();
        let instance = next_found(&mut browse, Duration::from_secs(10)).await;
        // The menu's off switch: SIGTERM to the whole group, helper included.
        // SAFETY: kill with a valid process group id and signal number.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
        let end = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(end, browse.events.recv()).await {
                Ok(Some(BrowseEvent::Lost(i))) if i == instance => break,
                Ok(Some(_)) => {}
                Ok(None) => panic!("the browse ended"),
                Err(_) => panic!("no goodbye for {instance} within 5 s"),
            }
        }
        let mut child = child;
        assert_eq!(child.wait().unwrap().code(), Some(0));
    });
}

#[test]
fn a_restart_keeps_the_service_name() {
    let Some((a, _b)) = enter("a_restart_keeps_the_service_name") else {
        return;
    };
    runtime().block_on(async {
        let env = Env::new("restart");
        let mut browse = Mdns::new(a.index).browse().await.unwrap();
        let child = env.spawn("no-ble");
        let first = next_found(&mut browse, Duration::from_secs(10)).await;
        stop(child);
        // The goodbye takes a second to count (RFC 6762 10.1); a new announcement within it
        // would only refresh the record, with no new Found.
        let end = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(end, browse.events.recv()).await {
                Ok(Some(BrowseEvent::Lost(i))) if i == first => break,
                Ok(Some(_)) => {}
                Ok(None) => panic!("the browse ended"),
                Err(_) => panic!("no goodbye for {first} within 5 s"),
            }
        }
        let child = env.spawn("no-ble");
        let second = next_found(&mut browse, Duration::from_secs(10)).await;
        stop(child);
        assert_eq!(second, first);
    });
}

#[test]
fn send_to_owly_receiver() {
    let Some((a, _b)) = enter("send_to_owly_receiver") else {
        return;
    };
    runtime().block_on(async {
        let env = Env::new("send");
        // Owly's receiver on owa, announced over mDNS.
        let identity = Identity::generate().unwrap();
        let got = env.dir.join("got");
        let addr = SocketAddrV6::new(a.addr, 8771, 0, a.index);
        let (receiver, mut events) = Receiver::bind(
            addr,
            &identity,
            Options::new(Config::new("Sam's Mac"), got.clone()),
        )
        .await
        .unwrap();
        tokio::spawn(receiver.run());
        let _reg = Mdns::new(a.index)
            .register(Service {
                instance: "a1a2a3a4a5a6".into(),
                host: "sams-mac".into(),
                port: 8771,
                addr: a.addr,
                flags: RECEIVER_FLAGS,
            })
            .await
            .unwrap();

        let child = env.spawn("");
        until(Duration::from_secs(10), "ready", || {
            env.state().starts_with("ready\towb\t")
        })
        .await;
        let folder = env.media("outbox/Sam's Mac");
        until(Duration::from_secs(30), "the device folder", || {
            folder.is_dir()
        })
        .await;
        // The daemon makes the folder before it writes the state file.
        until(
            Duration::from_secs(10),
            "the device in the state file",
            || env.state().contains("\nSam's Mac\tSam's Mac\tidle\t"),
        )
        .await;

        std::fs::write(folder.join("a.txt"), b"one").unwrap();
        std::fs::write(folder.join("b.txt"), b"two").unwrap();
        let received = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match events.recv().await.expect("receiver events") {
                    ReceiverEvent::Request(r) => {
                        assert_eq!(r.ask.sender_computer_name, "Front desk");
                        r.accept();
                    }
                    ReceiverEvent::Received { paths, .. } => return paths,
                    ReceiverEvent::Failed { reason, .. } => panic!("upload failed: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("an offer within 30 s");
        let mut names: Vec<String> = received
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a.txt", "b.txt"], "one offer with both files");
        assert_eq!(std::fs::read(got.join("b.txt")).unwrap(), b"two");
        until(Duration::from_secs(10), "the sent files deleted", || {
            std::fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none())
        })
        .await;
        stop(child);
        assert!(!folder.exists(), "an empty device folder goes at the stop");
    });
}

/// Owly's receiver "Sam's Mac" on `a`, announced over mDNS; files go to `got`.
async fn sams_mac(
    a: End,
    got: &Path,
) -> (
    tokio::sync::mpsc::Receiver<ReceiverEvent>,
    owly_airdrop::driver::mdns::Registration,
) {
    let identity = Identity::generate().unwrap();
    let addr = SocketAddrV6::new(a.addr, 8771, 0, a.index);
    let (receiver, events) = Receiver::bind(
        addr,
        &identity,
        Options::new(Config::new("Sam's Mac"), got.to_path_buf()),
    )
    .await
    .unwrap();
    tokio::spawn(receiver.run());
    let reg = Mdns::new(a.index)
        .register(Service {
            instance: "a1a2a3a4a5a6".into(),
            host: "sams-mac".into(),
            port: 8771,
            addr: a.addr,
            flags: RECEIVER_FLAGS,
        })
        .await
        .unwrap();
    (events, reg)
}

/// The names of the next accepted offer's files.
async fn next_offer(events: &mut tokio::sync::mpsc::Receiver<ReceiverEvent>) -> Vec<String> {
    let paths = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await.expect("receiver events") {
                ReceiverEvent::Request(r) => r.accept(),
                ReceiverEvent::Received { paths, .. } => return paths,
                ReceiverEvent::Failed { reason, .. } => panic!("upload failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("an offer within 30 s");
    let mut names: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_leftover_outbox_folder_is_watched() {
    let Some((a, _b)) = enter("a_leftover_outbox_folder_is_watched") else {
        return;
    };
    runtime().block_on(async {
        let env = Env::new("leftover");
        // An earlier run stopped with a file waiting for Sam's Mac.
        let folder = env.media("outbox/Sam's Mac");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("left.txt"), b"old").unwrap();
        let got = env.dir.join("got");
        let (mut events, _reg) = sams_mac(a, &got).await;
        let child = env.spawn("");
        assert_eq!(next_offer(&mut events).await, ["left.txt"]);
        until(Duration::from_secs(10), "left.txt deleted", || {
            !folder.join("left.txt").exists()
        })
        .await;
        // A file dropped in later goes too: the folder is watched in this run.
        std::fs::write(folder.join("new.txt"), b"new").unwrap();
        assert_eq!(next_offer(&mut events).await, ["new.txt"]);
        stop(child);
    });
}
