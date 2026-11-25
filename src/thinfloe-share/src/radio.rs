//! `thinfloe-share --radio`: the root helper that holds AWDL's radio from owlyd.
//!
//! Protocol on stdout (private to thinfloe-share), one line each:
//! `up<TAB>iface<TAB>mode<TAB>channel<TAB>flags` once the radio is up (flags a
//! comma list, `no-ble` when BlueZ offers no adapter), then `error<TAB>text`
//! and exit 1 when the radio is lost or cannot be had. Exit 0 after the lease
//! is released on stdin EOF or SIGTERM.
//!
//! The lease is a plain `Acquire(0)`, never a detached one: it ends when this
//! process leaves the bus, so a crash gives the card back too.

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use owlyd::api::{self, LinkInfo, RadioProxy, StateChangedStream};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::card::{CardInputs, choose_station, country};

/// The file owlyd reads each time the radio comes up (no restart needed).
pub const RADIO_TOML: &str = "/etc/owly/radio.toml";
const ZONEINFO: &str = "/usr/share/zoneinfo/";
/// How long the up line waits for BlueZ before it reports `no-ble`.
const BLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the helper needs from owlyd; the zbus implementation is [`Owlyd`].
#[allow(async_fn_in_trait)]
pub trait RadioApi {
    /// The mode and channel owlyd's probe says the radio would run on (0: unknown).
    async fn mode_channel(&self) -> Result<(String, u32), String>;
    async fn acquire(&self) -> Result<(u64, LinkInfo), String>;
    async fn release(&self, lease: u64);
    /// A lease held while this helper starts: one a crashed run left
    /// detached. Releasing a lease of a live owner fails, harmlessly.
    async fn stale_detached(&self) -> Option<u64>;
    /// Waits until `lease` ends without this helper releasing it; the reason.
    async fn wait_lost(&self, lease: u64) -> String;
}

fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn status_line(r: &Result<(String, String, u32, Vec<String>), String>) -> String {
    match r {
        Ok((iface, mode, channel, flags)) => format!(
            "up\t{}\t{}\t{channel}\t{}",
            one_line(iface),
            one_line(mode),
            one_line(&flags.join(","))
        ),
        Err(text) => format!("error\t{}", one_line(text)),
    }
}

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// radio.toml with `station` and, when known, `country`; everything else
/// stays at owlyd's defaults (mode from the quirks table, `allow_untested` false).
pub fn radio_toml(station: &str, country: Option<&str>) -> String {
    let mut s = format!("station = {}\n", toml_string(station));
    if let Some(c) = country {
        s.push_str(&format!("country = {}\n", toml_string(c)));
    }
    s
}

/// Two JSON arrays as one (`ip -j route` and `ip -6 -j route`); "" when
/// either is not an array, which the card choice reads as unreadable state.
pub fn merge_arrays(a: &str, b: &str) -> String {
    use serde_json::Value;
    match (
        serde_json::from_str::<Vec<Value>>(a),
        serde_json::from_str::<Vec<Value>>(b),
    ) {
        (Ok(mut x), Ok(y)) => {
            x.extend(y);
            Value::Array(x).to_string()
        }
        _ => String::new(),
    }
}

/// Completes when `r` reaches EOF (or fails).
pub async fn stdin_closed(mut r: impl AsyncRead + Unpin) {
    let mut buf = [0u8; 256];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.thinfloe-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

fn say(out: &mut impl Write, r: &Result<(String, String, u32, Vec<String>), String>) {
    let _ = writeln!(out, "{}", status_line(r));
    let _ = out.flush();
}

/// The whole helper after its inputs are read: card choice, radio.toml, the
/// lease, the BLE advert (`ble`, run once the radio is up) and the wait for
/// `stop` or the loss of the lease. Returns the exit code.
pub async fn hold<R: RadioApi, G>(
    api: &R,
    card: &CardInputs,
    country: Option<&str>,
    radio_toml_path: &Path,
    ble: impl Future<Output = (Option<G>, Vec<String>)>,
    stop: impl Future<Output = ()>,
    out: &mut impl Write,
) -> i32 {
    let station = match choose_station(card) {
        Ok(s) => s,
        Err(e) => {
            say(out, &Err(e.to_string()));
            return 1;
        }
    };
    if let Err(e) = write_atomic(radio_toml_path, &radio_toml(&station, country)) {
        say(
            out,
            &Err(format!("Cannot write {}: {e}", radio_toml_path.display())),
        );
        return 1;
    }
    if let Some(stale) = api.stale_detached().await {
        api.release(stale).await;
    }
    let (mode, channel) = match api.mode_channel().await {
        Ok(m) => m,
        Err(e) => {
            say(out, &Err(e));
            return 1;
        }
    };
    let (lease, link) = match api.acquire().await {
        Ok(l) => l,
        Err(e) => {
            say(out, &Err(e));
            return 1;
        }
    };
    let (guard, flags) = ble.await;
    say(out, &Ok((link.interface.clone(), mode, channel, flags)));
    let code = tokio::select! {
        _ = stop => {
            api.release(lease).await;
            0
        }
        reason = api.wait_lost(lease) => {
            say(out, &Err(format!("The radio stopped: {reason}")));
            1
        }
    };
    drop(guard);
    code
}

/// owlyd on the system bus. The state stream is opened at connect, before
/// any Acquire, so a loss right after it is not missed.
pub struct Owlyd {
    radio: RadioProxy<'static>,
    states: tokio::sync::Mutex<StateChangedStream>,
    owner: tokio::sync::Mutex<zbus::proxy::OwnerChangedStream<'static>>,
}

fn unreachable(e: &zbus::Error) -> String {
    format!("owlyd cannot be reached ({e}).")
}

fn api_text(e: &zbus::Error) -> String {
    match api::api_error(e) {
        Some((_, m)) if !m.is_empty() => m,
        _ if api::unreachable(e) => unreachable(e),
        _ => e.to_string(),
    }
}

impl Owlyd {
    pub async fn connect() -> Result<Self, String> {
        let conn = api::connect(None).await.map_err(|e| unreachable(&e))?;
        let radio = api::radio(&conn).await.map_err(|e| unreachable(&e))?;
        let states = radio
            .receive_radio_state()
            .await
            .map_err(|e| unreachable(&e))?;
        let owner = radio
            .inner()
            .receive_owner_changed()
            .await
            .map_err(|e| unreachable(&e))?;
        Ok(Owlyd {
            radio,
            states: tokio::sync::Mutex::new(states),
            owner: tokio::sync::Mutex::new(owner),
        })
    }
}

impl RadioApi for Owlyd {
    async fn mode_channel(&self) -> Result<(String, u32), String> {
        let json = self.radio.probe().await.map_err(|e| api_text(&e))?;
        let v: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| format!("owlyd's probe: {e}"))?;
        let mode = v["mode"].as_str().unwrap_or("unknown").to_string();
        let channel = v["channel"].as_u64().unwrap_or(0) as u32;
        Ok((mode, channel))
    }

    async fn acquire(&self) -> Result<(u64, LinkInfo), String> {
        let (lease, dict) = self.radio.acquire(0).await.map_err(|e| api_text(&e))?;
        match LinkInfo::from_dict(&dict) {
            Some(l) => Ok((lease, l)),
            None => {
                self.release(lease).await;
                Err("owlyd gave a link this helper cannot read.".into())
            }
        }
    }

    async fn release(&self, lease: u64) {
        if let Err(e) = self.radio.release(lease).await {
            eprintln!("thinfloe-share: releasing lease {lease}: {}", api_text(&e));
        }
    }

    async fn stale_detached(&self) -> Option<u64> {
        match self.radio.lease().await {
            Ok(0) | Err(_) => None,
            Ok(l) => Some(l),
        }
    }

    async fn wait_lost(&self, lease: u64) -> String {
        let mut states = self.states.lock().await;
        // Ended before the wait began: the signal went by in the stream, or
        // the properties tell.
        if let Ok(now) = self.radio.lease().await
            && now != lease
            && let Ok((last, reason)) = self.radio.last_end().await
            && last == lease
        {
            return reason;
        }
        let mut owner = self.owner.lock().await;
        loop {
            tokio::select! {
                s = states.next() => {
                    let Some(s) = s else { return "owlyd left the bus".into() };
                    let Ok(a) = s.args() else { continue };
                    if a.lease == lease && a.state == "idle" {
                        return a.reason;
                    }
                }
                change = owner.next() => {
                    if owner_lost(change) {
                        return "owlyd stopped".into();
                    }
                }
            }
        }
    }
}

/// Whether an item of owlyd's name-owner stream means owlyd is gone (and
/// its leases with it): the name lost its owner, or the stream ended. A new
/// owner is not a loss: D-Bus activation by the helper's own first call
/// gives one, and a restart shows the name gone first.
fn owner_lost<T>(change: Option<Option<T>>) -> bool {
    !matches!(change, Some(Some(_)))
}

/// Wi-Fi interfaces: those with a `wireless` directory in sysfs.
fn wifi_ifaces() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok())
        .filter(|e| e.path().join("wireless").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

fn ip_json(args: &[&str]) -> String {
    std::process::Command::new("ip")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default()
}

fn tfuser_home() -> Option<PathBuf> {
    let o = std::process::Command::new("getent")
        .args(["passwd", "tfuser"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let line = String::from_utf8(o.stdout).ok()?;
    let home = line.trim_end().split(':').nth(5)?;
    (!home.is_empty()).then(|| PathBuf::from(home))
}

/// generated/wifi.yaml of tfuser and the netplan files the initramfs
/// installed; a card named in any of them is never taken.
fn wifi_yaml() -> Option<String> {
    let mut texts = Vec::new();
    if let Some(home) = tfuser_home()
        && let Ok(t) = std::fs::read_to_string(home.join(".config/thinfloe/generated/wifi.yaml"))
    {
        texts.push(t);
    }
    if let Ok(dir) = std::fs::read_dir("/etc/netplan") {
        let mut paths: Vec<PathBuf> = dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        paths.sort();
        texts.extend(paths.iter().filter_map(|p| std::fs::read_to_string(p).ok()));
    }
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// The zone /etc/localtime points to, as timedatectl sets it.
fn zone() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let t = target.to_str()?;
    let (_, z) = t.split_once(ZONEINFO.trim_start_matches('/'))?;
    Some(z.to_string())
}

fn gather() -> (CardInputs, Option<String>) {
    let card = CardInputs {
        ip_addr_json: ip_json(&["-j", "addr"]),
        ip_route_json: merge_arrays(&ip_json(&["-j", "route"]), &ip_json(&["-6", "-j", "route"])),
        wifi_yaml: wifi_yaml(),
        wifi_ifaces: wifi_ifaces(),
    };
    let country = zone().and_then(|z| {
        let tab = std::fs::read_to_string(format!("{ZONEINFO}zone1970.tab")).ok()?;
        country(&z, &tab)
    });
    (card, country)
}

fn random_tag() -> [u8; 3] {
    let mut tag = [0u8; 3];
    // SAFETY: getrandom writes at most tag.len() bytes into the buffer.
    let n = unsafe { libc::getrandom(tag.as_mut_ptr().cast(), tag.len(), 0) };
    if n != tag.len() as isize {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            ^ std::process::id();
        tag.copy_from_slice(&t.to_le_bytes()[..3]);
    }
    tag
}

async fn ble() -> (Option<crate::bluez::Registration>, Vec<String>) {
    use owly_airdrop::proto::ble::Advert;
    let started = tokio::time::timeout(
        BLE_TIMEOUT,
        crate::bluez::advertise(Advert::anonymous(random_tag())),
    )
    .await;
    match started {
        Ok(Ok(Some(r))) => (Some(r), Vec::new()),
        Ok(Ok(None)) => (None, vec!["no-ble".into()]),
        Err(_) => {
            eprintln!("thinfloe-share: no BLE advert: BlueZ did not answer");
            (None, vec!["no-ble".into()])
        }
        Ok(Err(e)) => {
            eprintln!("thinfloe-share: no BLE advert: {e}");
            (None, vec!["no-ble".into()])
        }
    }
}

async fn stop() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return;
    };
    tokio::select! {
        _ = stdin_closed(tokio::io::stdin()) => {}
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

/// A SIGHUP ends nothing here: sudo relays the signals its group gets, and
/// a reload of the daemon must not end the helper.
fn ignore_hangup() {
    // SAFETY: setting a signal to SIG_IGN has no memory effects.
    unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
}

/// `thinfloe-share --radio`; the exit code.
pub fn main() -> i32 {
    ignore_hangup();
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("thinfloe-share: --radio must run as root");
        return 2;
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("thinfloe-share: {e}");
            return 1;
        }
    };
    rt.block_on(async {
        let mut out = std::io::stdout();
        let (card, country) = gather();
        let owlyd = match Owlyd::connect().await {
            Ok(o) => o,
            Err(e) => {
                say(&mut out, &Err(e));
                return 1;
            }
        };
        let mut reg = None;
        let code = hold(
            &owlyd,
            &card,
            country.as_deref(),
            Path::new(RADIO_TOML),
            async {
                let (r, flags) = ble().await;
                reg = r;
                (None::<()>, flags)
            },
            stop(),
            &mut out,
        )
        .await;
        if let Some(r) = reg {
            r.stop().await;
        }
        code
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_helper_lives_through_a_sighup() {
        // In a child of its own, so the test binary keeps its handlers. The
        // child calls only async-signal-safe functions.
        // SAFETY: fork; the child uses signal, raise and _exit only.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                super::ignore_hangup();
                libc::raise(libc::SIGHUP);
                libc::_exit(7);
            }
        }
        let mut status = 0;
        // SAFETY: waitpid on our own child with a valid status pointer.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status), "ended by a signal: {status:#x}");
        assert_eq!(libc::WEXITSTATUS(status), 7);
    }

    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    const ADDR: &str = include_str!("../testdata/ip-addr.json");
    const ROUTE: &str = include_str!("../testdata/ip-route.json");

    fn link(iface: &str) -> LinkInfo {
        LinkInfo {
            interface: iface.into(),
            index: 7,
            mac: "aa:bb:cc:dd:ee:01".into(),
            address: "fe80::a8bb:ccff:fedd:ee01".into(),
            announce: None,
        }
    }

    #[derive(Default)]
    struct Fake {
        stale: Option<u64>,
        lost: Option<String>,
        calls: RefCell<Vec<String>>,
    }

    impl RadioApi for Fake {
        async fn mode_channel(&self) -> Result<(String, u32), String> {
            self.calls.borrow_mut().push("probe".into());
            Ok(("exclusive".into(), 44))
        }
        async fn acquire(&self) -> Result<(u64, LinkInfo), String> {
            self.calls.borrow_mut().push("acquire".into());
            Ok((5, link("awdl0")))
        }
        async fn release(&self, lease: u64) {
            self.calls.borrow_mut().push(format!("release {lease}"));
        }
        async fn stale_detached(&self) -> Option<u64> {
            self.stale
        }
        async fn wait_lost(&self, _lease: u64) -> String {
            match &self.lost {
                Some(r) => r.clone(),
                None => std::future::pending().await,
            }
        }
    }

    fn free_card() -> CardInputs {
        CardInputs {
            ip_addr_json: ADDR.into(),
            ip_route_json: ROUTE.into(),
            wifi_yaml: None,
            wifi_ifaces: vec!["wlp2s0".into(), "wlx00e04c123456".into()],
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
            .join(format!("radio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("owly").join("radio.toml")
    }

    async fn no_ble() -> (Option<()>, Vec<String>) {
        (None, vec!["no-ble".into()])
    }

    async fn run(
        fake: &Fake,
        card: &CardInputs,
        toml: &Path,
        stop: impl Future<Output = ()>,
    ) -> (i32, String) {
        let mut out = Vec::new();
        let code = hold(fake, card, Some("CH"), toml, no_ble(), stop, &mut out).await;
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn only_a_lost_owner_ends_the_wait() {
        // Activation: "" -> ":1.42" is not a loss.
        assert!(!owner_lost(Some(Some(":1.42"))));
        assert!(owner_lost(Some(None::<&str>)));
        assert!(owner_lost(None::<Option<&str>>));
    }

    #[test]
    fn up_line_format() {
        let up = Ok(("awdl0".to_string(), "exclusive".to_string(), 44, vec![]));
        assert_eq!(status_line(&up), "up\tawdl0\texclusive\t44\t");
        let flagged = Ok((
            "awdl0".to_string(),
            "shared".to_string(),
            6,
            vec!["no-ble".to_string(), "x".to_string()],
        ));
        assert_eq!(status_line(&flagged), "up\tawdl0\tshared\t6\tno-ble,x");
        let err: Result<(String, String, u32, Vec<String>), String> =
            Err("bad\tthing\nhere".into());
        assert_eq!(status_line(&err), "error\tbad thing here");
    }

    #[tokio::test]
    async fn no_free_card_prints_error_and_acquires_nothing() {
        let fake = Fake::default();
        let mut card = free_card();
        card.wifi_ifaces = vec!["wlp2s0".into()];
        let toml = tmp("nocard");
        let (code, out) = run(&fake, &card, &toml, async {}).await;
        assert_eq!(code, 1);
        assert_eq!(out, format!("error\t{}\n", crate::card::NO_CARD));
        assert!(fake.calls.borrow().is_empty());
        assert!(!toml.exists());
    }

    #[tokio::test]
    async fn a_stale_detached_lease_is_released_first() {
        let fake = Fake {
            stale: Some(3),
            ..Default::default()
        };
        let (code, out) = run(&fake, &free_card(), &tmp("stale"), async {}).await;
        assert_eq!(code, 0);
        assert_eq!(out, "up\tawdl0\texclusive\t44\tno-ble\n");
        assert_eq!(
            *fake.calls.borrow(),
            ["release 3", "probe", "acquire", "release 5"]
        );
    }

    #[tokio::test]
    async fn lease_lost_is_an_error_line() {
        let fake = Fake {
            lost: Some("link lost".into()),
            ..Default::default()
        };
        let (code, out) = run(&fake, &free_card(), &tmp("lost"), std::future::pending()).await;
        assert_eq!(code, 1);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("up\tawdl0\t"));
        assert_eq!(lines[1], "error\tThe radio stopped: link lost");
        // The lease is gone already; nothing to release.
        assert_eq!(*fake.calls.borrow(), ["probe", "acquire"]);
    }

    #[tokio::test]
    async fn stdin_eof_releases_the_lease() {
        let fake = Fake::default();
        let stop = stdin_closed(tokio::io::empty());
        let (code, out) = run(&fake, &free_card(), &tmp("eof"), stop).await;
        assert_eq!(code, 0);
        assert!(out.starts_with("up\t"));
        assert_eq!(fake.calls.borrow().last().unwrap(), "release 5");
    }

    #[test]
    fn radio_toml_has_station_and_country() {
        assert_eq!(
            radio_toml("wlx00e04c123456", Some("CH")),
            "station = \"wlx00e04c123456\"\ncountry = \"CH\"\n"
        );
    }

    #[test]
    fn radio_toml_without_zone_has_no_country() {
        assert_eq!(radio_toml("wlan1", None), "station = \"wlan1\"\n");
    }

    #[tokio::test]
    async fn radio_toml_is_written_before_the_radio_comes_up() {
        let fake = Fake::default();
        let toml = tmp("written");
        let (code, _) = run(&fake, &free_card(), &toml, async {}).await;
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read_to_string(&toml).unwrap(),
            "station = \"wlx00e04c123456\"\ncountry = \"CH\"\n"
        );
    }

    #[test]
    fn route_lists_merge() {
        assert_eq!(
            merge_arrays("[{\"a\":1}]", "[{\"b\":2}]"),
            "[{\"a\":1},{\"b\":2}]"
        );
        assert_eq!(merge_arrays("[]", "garbage"), "");
    }
}
