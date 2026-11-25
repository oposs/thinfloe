//! The OSD process's loop: keys from the socket, results from the audio
//! and monitor workers, `pactl subscribe` events, the Wayland connection,
//! and a redraw of every output after each change.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::audio::{AudioEvent, AudioRequest, AudioWorker, Pactl, Watch};
use super::ddc::{Bright, DdcEvent, Ddcutil, Monitors};
use super::draw::{Painter, PanelView};
use super::notes::{self, Notes, Watcher};
use super::socket::Datagram;
use super::state::{OsdState, Shown};
use super::surface::{SurfaceKind, Surfaces, mark_placement};
use super::sync::{Mode, keys_mode};
use super::{Action, POLL_MAX_MS, STOP, VOLUME_STEP, socket};
use crate::raster::Canvas;
use crate::removable;

/// How long after the outputs change the monitors are detected again:
/// a monitor that just came answers DDC/CI only after a while.
const REDETECT_DELAY: Duration = Duration::from_secs(2);

/// What the worker threads report to the loop.
enum Msg {
    Audio(AudioEvent),
    /// A `pactl subscribe` line, or its restart.
    Watch(Watch),
    Ddc(DdcEvent),
}

/// The workers' way into the loop: a channel, and a byte on a socket pair
/// that wakes the loop's `poll`.
struct Notify {
    tx: mpsc::Sender<Msg>,
    wake: UnixStream,
}

impl Notify {
    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        // A full pair already wakes the loop.
        let _ = (&self.wake).write(&[1]);
    }
}

/// Messages logged once per process: a missing sound device would
/// otherwise log on every key.
#[derive(Default)]
pub(super) struct Logged(BTreeSet<String>);

impl Logged {
    /// Log `msg` unless it was logged before; true when it was logged now.
    fn once(&mut self, msg: String) -> bool {
        if self.0.contains(&msg) {
            return false;
        }
        eprintln!("{msg}");
        self.0.insert(msg);
        true
    }
}

/// The socket file goes with the OSD, however it ends, so `--osd-key`
/// knows there is none. (The volume file goes with the audio worker.)
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One monitor detection at a time, and one more at most: outputs that
/// change while a detection runs, as in a hotplug storm, give one
/// detection after it, not one each. Each change moves the start
/// `REDETECT_DELAY` on.
#[derive(Default)]
struct Redetect {
    at: Option<Instant>,
    running: bool,
    again: bool,
}

impl Redetect {
    fn outputs_changed(&mut self, now: Instant) {
        self.at = Some(now + REDETECT_DELAY);
    }

    /// True when a detection is to start now.
    fn due(&mut self, now: Instant) -> bool {
        if self.at.is_none_or(|t| now < t) {
            return false;
        }
        self.at = None;
        if self.running {
            self.again = true;
            return false;
        }
        self.running = true;
        true
    }

    /// A detection started outside `due` (the first one).
    fn started(&mut self) {
        self.running = true;
    }

    /// A detection finished; true when the next one is to start now.
    fn finished(&mut self) -> bool {
        self.running = std::mem::take(&mut self.again);
        self.running
    }

    fn deadline(&self) -> Option<Instant> {
        self.at
    }
}

/// What each output shows now, to draw only what changed.
#[derive(Default)]
struct Drawn {
    /// The view, the scale and the output's width it was drawn for.
    panel: Option<(PanelView, f32, Option<u32>)>,
    mark: Option<f32>,
}

pub(super) fn run() -> Result<(), String> {
    super::install_sigterm();
    let mut surfaces = Surfaces::connect()?;
    let path = super::socket_path().ok_or("neither TF_OSD_SOCKET nor XDG_RUNTIME_DIR is set")?;
    let sock = socket::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _socket_file = RemoveOnDrop(path);

    let (wake_rx, wake_tx) = UnixStream::pair().map_err(|e| format!("socket pair: {e}"))?;
    wake_rx
        .set_nonblocking(true)
        .and_then(|()| wake_tx.set_nonblocking(true))
        .map_err(|e| format!("socket pair: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let notify = Arc::new(Notify { tx, wake: wake_tx });

    // The watch first: a device that comes while the worker starts is
    // then not missed.
    let n = notify.clone();
    let _watch = super::audio::watch(&Pactl::from_env(), move |w| n.send(Msg::Watch(w)));
    let env = env_file();
    let mut logged = Logged::default();
    let mut mode = start_mode(&env, &mut logged);
    // The speaker state for the menu's Sound tab: one left by an OSD that
    // did not end cleanly (SIGKILL) is not this one's. The worker's drop
    // removes the file at the end, so that no write of a `pactl` call
    // still running can put it back.
    let volume_file = super::volume_file_path();
    if let Some(p) = &volume_file {
        let _ = crate::atomic::remove(p);
    }
    let n = notify.clone();
    let audio = AudioWorker::spawn_with_file(Pactl::from_env(), mode, volume_file, move |e| n.send(Msg::Audio(e)));
    let n = notify.clone();
    let monitors = Monitors::spawn(Ddcutil::from_env(), move |e| n.send(Msg::Ddc(e)));
    let mut redetect = Redetect::default();
    monitors.redetect();
    redetect.started();
    drop(notify);

    let mut state = OsdState::new();
    let (contract, event) = (
        removable_path("TF_REMOVABLE_FILE", removable::FILE),
        removable_path("TF_REMOVABLE_EVENT", removable::EVENT_FILE),
    );
    let mut stick_notes = Notes::new();
    // The baseline: what is plugged in at start is no news.
    let (sticks, last) = notes::read_files(&contract, &event);
    stick_notes.update(&sticks, last.as_ref());
    let watcher = Watcher::new(&[&contract, &event])
        .map_err(|why| logged.once(format!("thinfloe-menu --osd: no notes about sticks: {why}")))
        .ok();
    let mut painter = Painter::new();
    let mut drawn: BTreeMap<String, Drawn> = BTreeMap::new();
    let mut dirty = true;
    while !STOP.load(Ordering::SeqCst) {
        if dirty {
            redraw(&mut surfaces, &mut painter, &state, &mut drawn, &mut logged);
            dirty = false;
        }
        surfaces.flush()?;

        let now = Instant::now();
        let timeout = [state.next_deadline(), redetect.deadline()]
            .into_iter()
            .flatten()
            .map(|t| t.saturating_duration_since(now).as_micros().div_ceil(1000))
            .fold(POLL_MAX_MS as u128, u128::min) as libc::c_int;
        let watch_fd = watcher.as_ref().map_or(-1, Watcher::raw_fd);
        let mut fds = [
            surfaces.fd().as_raw_fd(),
            sock.as_raw_fd(),
            wake_rx.as_raw_fd(),
            watch_fd,
        ]
        .map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        // SAFETY: four pollfds (a negative one is ignored) for the duration of the call.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("poll: {e}"));
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err("the compositor closed the connection".into());
        }

        let now = Instant::now();
        if surfaces.dispatch()?.0 {
            // Every surface is drawn again: a new output has none, and a
            // closed one may now be made again.
            drawn.clear();
            dirty = true;
            redetect.outputs_changed(now);
        }
        for d in drain_socket(&sock) {
            let a = match d {
                Datagram::Key(a) => a,
                Datagram::Note { warn, text } => {
                    state.show(Shown::Note { warn, text }, now);
                    dirty = true;
                    continue;
                }
            };
            if a == Action::Reload {
                reload(&audio, &env, &mut mode, &mut logged);
                continue;
            }
            dirty |= on_action(&mut state, a, &audio, &monitors, now);
        }
        if watcher.as_ref().is_some_and(Watcher::drain) {
            let (sticks, last) = notes::read_files(&contract, &event);
            // Only the newest note shows; the others were overtaken.
            if let Some(n) = stick_notes.update(&sticks, last.as_ref()).last() {
                let (text, warn) = notes::text(n);
                state.show(Shown::Note { text, warn }, now);
                dirty = true;
            }
        }
        let mut buf = [0u8; 64];
        while matches!((&wake_rx).read(&mut buf), Ok(n) if n > 0) {}
        let outputs = surfaces.outputs();
        while let Ok(m) = rx.try_recv() {
            dirty |= match m {
                Msg::Audio(e) => on_audio(&mut state, e, now, &mut logged),
                Msg::Watch(w) => {
                    audio.request(watch_request(w));
                    false
                }
                Msg::Ddc(e) => {
                    if matches!(e, DdcEvent::Detected(_)) && redetect.finished() {
                        monitors.redetect();
                    }
                    on_ddc(&mut state, e, &outputs, &mut logged)
                }
            };
        }
        if redetect.due(now) {
            monitors.redetect();
        }
        dirty |= state.expire(now);
    }
    Ok(())
}

/// A file path from the environment (tests), else the default.
fn removable_path(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(default), PathBuf::from)
}

/// What the worker does with a report of `pactl subscribe`.
fn watch_request(w: Watch) -> AudioRequest {
    match w {
        Watch::Event(e) => AudioRequest::Device(e),
        Watch::Connected => AudioRequest::Start,
        Watch::Restarted => AudioRequest::Resync,
    }
}

/// The datagrams waiting on the socket; undecodable ones are dropped.
fn drain_socket(sock: &UnixDatagram) -> Vec<Datagram> {
    let mut found = Vec::new();
    let mut buf = [0u8; socket::DATAGRAM_MAX];
    loop {
        match sock.recv(&mut buf) {
            Ok(n) => found.extend(socket::decode(&buf[..n])),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return found,
        }
    }
}

/// Start what `a` asks for; the brightness panel shows at once, the
/// volume and microphone panels when `pactl` has answered, and the Sound
/// tab's sets show none. True when
/// something to draw changed. `Reload` is the loop's (`reload`).
fn on_action(state: &mut OsdState, a: Action, audio: &AudioWorker, monitors: &Monitors, now: Instant) -> bool {
    match a {
        Action::Reload => {}
        Action::VolumeUp => audio.request(AudioRequest::Step(VOLUME_STEP)),
        Action::VolumeDown => audio.request(AudioRequest::Step(-VOLUME_STEP)),
        Action::VolumeMute => audio.request(AudioRequest::ToggleMute),
        Action::MicMute => audio.request(AudioRequest::ToggleMic),
        // The Sound tab: the slider is the feedback, no panel.
        Action::VolumeSet(n) => audio.request(AudioRequest::SetLevel(n)),
        Action::VolumeMuteSet(m) => audio.request(AudioRequest::SetMute(m)),
        Action::BrightnessUp | Action::BrightnessDown => {
            monitors.step(a == Action::BrightnessUp);
            state.show(Shown::Brightness, now);
            return true;
        }
    }
    false
}

/// The generated environment the menu writes, where the volume keys
/// switch is: found as the menu finds it (`TF_CONFIG_DIR`, else
/// `$HOME/.config/thinfloe`).
fn env_file() -> PathBuf {
    crate::config::ConfigDir::from_env().generated_file("environment")
}

/// The volume keys switch in `env`. A missing file is off (the menu has
/// not written one yet); another read error is an error, so a reload does
/// not take a passing failure for the switch turned off.
fn read_mode(env: &Path) -> Result<Mode, String> {
    match std::fs::read_to_string(env) {
        Ok(text) => Ok(keys_mode(&text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Mode::Off),
        Err(e) => Err(format!("{}: {e}; the volume keys switch is not read", env.display())),
    }
}

/// The switch at start: a read error is logged and counts as off.
fn start_mode(env: &Path, logged: &mut Logged) -> Mode {
    read_mode(env).unwrap_or_else(|why| {
        logged.once(format!("thinfloe-menu --osd: {why}"));
        Mode::Off
    })
}

/// The menu applied a keyboard change, which may or may not have turned
/// the switch. Only a change reaches the worker: with the switch off, a
/// mode request puts every device at 100 % and unmuted, which must not
/// undo a microphone muted elsewhere on every layout change. The loop
/// keeps the mode rather than the worker comparing, so a reload that
/// changes nothing costs no `pactl` call at all. A read error keeps the
/// mode and is logged once.
fn reload(audio: &AudioWorker, env: &Path, mode: &mut Mode, logged: &mut Logged) {
    match read_mode(env) {
        Ok(m) if m != *mode => {
            *mode = m;
            audio.request(AudioRequest::Mode(m));
        }
        Ok(_) => {}
        Err(why) => {
            logged.once(format!("thinfloe-menu --osd: {why}"));
        }
    }
}

/// The answer of a volume or microphone key, or a fresh mark. A failure
/// (no sound device) is logged once per message and shows nothing. The
/// mic key's panel shows the shared state at once; the mark changes only
/// with a `Mark`, which the worker reads from the default source after
/// the key's sets, so it never flickers when that source does not follow
/// (a monitor as the default).
pub(super) fn on_audio(state: &mut OsdState, e: AudioEvent, now: Instant, logged: &mut Logged) -> bool {
    match e {
        AudioEvent::Sink(l) => state.show(Shown::Sink(l), now),
        AudioEvent::Mic(m) => state.show(Shown::Mic(m), now),
        AudioEvent::Mark(m) => return state.set_mic(m),
        AudioEvent::Failed(why) => {
            logged.once(format!("thinfloe-menu --osd: {why}"));
            return false;
        }
    }
    true
}

/// A detection's result or a brightness step's new level. An output that
/// does not answer DDC/CI, or that the detection does not list, is
/// logged once per process.
fn on_ddc(state: &mut OsdState, e: DdcEvent, outputs: &[String], logged: &mut Logged) -> bool {
    match e {
        DdcEvent::Level { connector, percent } => state.set_bright(&connector, Bright::Percent(percent)),
        DdcEvent::Detected(found) => {
            let unsupported = |name: &str, logged: &mut Logged| {
                logged.once(format!(
                    "thinfloe-menu --osd: monitor {name} does not answer DDC/CI; the brightness keys leave it alone"
                ));
            };
            for out in outputs.iter().filter(|o| !found.iter().any(|(c, _)| c == *o)) {
                unsupported(out, logged);
                state.set_bright(out, Bright::Unsupported);
            }
            for (connector, b) in found {
                if b == Bright::Unsupported {
                    unsupported(&connector, logged);
                }
                state.set_bright(&connector, b);
            }
        }
    }
    true
}

/// The scale the menu wrote, by the `labwc-scale` rule: one number from
/// 0.5 to 8 alone on the first line; anything else is 1.
fn parse_scale(text: &str) -> f32 {
    let line = text.lines().next().unwrap_or("");
    let mut words = line.split_whitespace();
    let (Some(w), None) = (words.next(), words.next()) else {
        return 1.0;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let well_formed = match w.split_once('.') {
        Some((i, f)) => digits(i) && digits(f),
        None => digits(w),
    };
    match w.parse::<f32>() {
        Ok(s) if well_formed && (0.5..=8.0).contains(&s) => s,
        _ => 1.0,
    }
}

fn read_scale(path: Option<&Path>) -> f32 {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map_or(1.0, |t| parse_scale(&t))
}

/// Show or hide each output's panel and mark as `state` says, drawing
/// only what differs from `drawn`. A failed show is logged once and the
/// rest still drawn.
fn redraw(
    surfaces: &mut Surfaces,
    painter: &mut Painter,
    state: &OsdState,
    drawn: &mut BTreeMap<String, Drawn>,
    logged: &mut Logged,
) {
    let scale = read_scale(crate::density::scale_file_path().as_deref());
    let now = Instant::now();
    let outputs = surfaces.outputs();
    drawn.retain(|name, _| outputs.contains(name));
    let mut panels: Vec<(PanelView, Option<u32>, Canvas)> = Vec::new();
    let mut mark_px: Option<Vec<u32>> = None;
    for out in outputs {
        let d = drawn.entry(out.clone()).or_default();
        let width = surfaces.output_width(&out);
        let want = state.panel_for(&out, now).map(|v| (v, scale, width));
        if d.panel != want {
            let shown = match &want {
                Some((v, _, _)) => {
                    let c = match panels.iter().find(|(p, w, _)| p == v && *w == width) {
                        Some((_, _, c)) => c,
                        None => {
                            panels.push((v.clone(), width, painter.panel_on(v, scale, width)));
                            &panels.last().expect("just pushed").2
                        }
                    };
                    let place = v.placement(scale, (c.width as u32, c.height as u32));
                    surfaces.show(&out, SurfaceKind::Panel, &place, place.size.0, &c.pixels)
                }
                None => {
                    surfaces.hide(&out, SurfaceKind::Panel);
                    Ok(())
                }
            };
            match shown {
                Ok(()) => d.panel = want,
                Err(e) => {
                    logged.once(format!("thinfloe-menu --osd: {out}: {e}"));
                }
            }
        }
        let want = state.mark().then_some(scale);
        if d.mark != want {
            let shown = match want {
                Some(_) => {
                    let place = mark_placement(scale);
                    let px = mark_px.get_or_insert_with(|| painter.mark(scale).pixels);
                    surfaces.show(&out, SurfaceKind::Mark, &place, place.size.0, px)
                }
                None => {
                    surfaces.hide(&out, SurfaceKind::Mark);
                    Ok(())
                }
            };
            match shown {
                Ok(()) => d.mark = want,
                Err(e) => {
                    logged.once(format!("thinfloe-menu --osd: {out}: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osd::audio::Level;

    #[test]
    fn the_watch_asks_the_worker_to_start_resync_or_follow() {
        use crate::osd::audio::{Kind, PaEvent};
        assert_eq!(watch_request(Watch::Connected), AudioRequest::Start);
        assert_eq!(watch_request(Watch::Restarted), AudioRequest::Resync);
        let e = PaEvent::New(Kind::Sink, 3);
        assert_eq!(watch_request(Watch::Event(e)), AudioRequest::Device(e));
    }

    #[test]
    fn a_failed_audio_event_shows_no_panel() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let mut logged = Logged::default();
        let why = || AudioEvent::Failed("pactl -f json list sinks: Connection failure: Connection refused".into());
        assert!(!on_audio(&mut s, why(), t0, &mut logged));
        assert_eq!(s.panel_for("DP-1", t0), None);
        assert!(!s.mark());
        // Logged once, not on every key.
        assert!(
            !logged
                .once("thinfloe-menu --osd: pactl -f json list sinks: Connection failure: Connection refused".into())
        );
    }

    #[test]
    fn a_sink_event_shows_the_level_and_a_mic_event_shows_its_panel() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let mut logged = Logged::default();
        let l = Level {
            percent: 55,
            muted: false,
        };
        assert!(on_audio(&mut s, AudioEvent::Sink(l), t0, &mut logged));
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::Volume { percent: 55 }));
        assert!(on_audio(&mut s, AudioEvent::Mic(true), t0, &mut logged));
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::MicOff));
        // The mark waits for the default source's real state (a Mark
        // event), so a monitor as the default never flashes it.
        assert!(!s.mark());
    }

    #[test]
    fn a_mark_event_sets_the_mark_without_a_panel() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let mut logged = Logged::default();
        assert!(on_audio(&mut s, AudioEvent::Mark(true), t0, &mut logged));
        assert!(s.mark());
        assert_eq!(s.panel_for("DP-1", t0), None);
        assert!(
            !on_audio(&mut s, AudioEvent::Mark(true), t0, &mut logged),
            "nothing changed"
        );
        assert!(on_audio(&mut s, AudioEvent::Mark(false), t0, &mut logged));
        assert!(!s.mark());
        assert_eq!(s.panel_for("DP-1", t0), None);
    }

    #[test]
    fn a_reload_acts_only_when_the_switch_changes() {
        use crate::osd::audio::tests::{calls, fake, json, wait_for};
        use crate::osd::audio::{AudioEvent, AudioWorker};
        use std::sync::Mutex;
        let d = crate::testdir::TestDir::new("osd-run", "reload");
        let env = d.join("environment");
        std::fs::write(&env, "THINFLOE_VOLUME_KEYS=on\n").unwrap();
        let p = fake(
            &d,
            &json(&[(1, "a", 40, true)]),
            &json(&[(2, "mic", 60, true)]),
            "a",
            "mic",
            "",
        );
        let mut logged = Logged::default();
        let mut mode = start_mode(&env, &mut logged);
        assert_eq!(mode, Mode::Keys);
        let events = Arc::new(Mutex::new(Vec::<AudioEvent>::new()));
        let e = events.clone();
        let audio = AudioWorker::spawn(p, mode, move |ev| e.lock().unwrap().push(ev));
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        assert_eq!(calls(&d, "set-"), Vec::<String>::new(), "the keys mode keeps the start");
        // A reload after a layout change: the switch is still on, so the
        // devices others changed are left alone. A Step after it marks
        // the point where the reload's sets would have run.
        reload(&audio, &env, &mut mode, &mut logged);
        audio.request(AudioRequest::Step(0));
        wait_for("the step's answer", || {
            events.lock().unwrap().iter().any(|e| matches!(e, AudioEvent::Sink(_)))
        });
        assert_eq!(
            calls(&d, "set-"),
            Vec::<String>::new(),
            "no set for an unchanged switch"
        );
        // Turned off: every device to 100 % and unmuted.
        std::fs::write(&env, "THINFLOE_VOLUME_KEYS=on\nTHINFLOE_VOLUME_KEYS=off\n").unwrap();
        reload(&audio, &env, &mut mode, &mut logged);
        assert_eq!(mode, Mode::Off);
        wait_for("four sets", || calls(&d, "set-").len() == 4);
        assert_eq!(
            calls(&d, "set-"),
            [
                "set-sink-volume a 100%",
                "set-sink-mute a 0",
                "set-source-volume mic 100%",
                "set-source-mute mic 0"
            ]
        );
        // Off again: nothing more. (The fake's lists do not change, so a
        // Step here would set again; wait instead.)
        reload(&audio, &env, &mut mode, &mut logged);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(calls(&d, "set-").len(), 4);
    }

    #[test]
    fn only_a_missing_switch_file_is_off() {
        let d = crate::testdir::TestDir::new("osd-run", "readmode");
        assert_eq!(read_mode(&d.join("missing")), Ok(Mode::Off));
        std::fs::write(d.join("on"), "THINFLOE_VOLUME_KEYS=on\n").unwrap();
        assert_eq!(read_mode(&d.join("on")), Ok(Mode::Keys));
        // A directory at the path: an error, not the switch off.
        let dir = d.join("generated-environment");
        std::fs::create_dir(&dir).unwrap();
        let err = read_mode(&dir).unwrap_err();
        assert!(err.contains(&dir.display().to_string()), "{err}");
        // At start the error is logged and the switch counts as off.
        let mut logged = Logged::default();
        assert_eq!(start_mode(&dir, &mut logged), Mode::Off);
        assert!(!logged.once(format!("thinfloe-menu --osd: {err}")), "logged at start");
    }

    #[test]
    fn an_unreadable_switch_file_keeps_the_mode() {
        use crate::osd::audio::tests::{calls, fake, json, wait_for};
        use crate::osd::audio::{AudioEvent, AudioWorker};
        use std::sync::Mutex;
        let d = crate::testdir::TestDir::new("osd-run", "unread");
        let p = fake(&d, &json(&[(1, "a", 40, true)]), "[]", "a", "", "");
        let events = Arc::new(Mutex::new(Vec::<AudioEvent>::new()));
        let e = events.clone();
        let audio = AudioWorker::spawn(p, Mode::Keys, move |ev| e.lock().unwrap().push(ev));
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        let dir = d.join("env");
        std::fs::create_dir(&dir).unwrap();
        let mut logged = Logged::default();
        let mut mode = Mode::Keys;
        reload(&audio, &dir, &mut mode, &mut logged);
        reload(&audio, &dir, &mut mode, &mut logged);
        assert_eq!(mode, Mode::Keys, "kept");
        audio.request(AudioRequest::Step(0));
        wait_for("the step's answer", || {
            events.lock().unwrap().iter().any(|e| matches!(e, AudioEvent::Sink(_)))
        });
        assert_eq!(calls(&d, "set-"), Vec::<String>::new(), "no reset to 100 %");
        let err = read_mode(&dir).unwrap_err();
        assert!(!logged.once(format!("thinfloe-menu --osd: {err}")), "logged once");
    }

    #[test]
    fn an_output_detection_does_not_list_is_unsupported() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let mut logged = Logged::default();
        let outputs = ["DP-1".to_string(), "HDMI-A-1".to_string(), "eDP-1".to_string()];
        let found = vec![
            ("DP-1".to_string(), Bright::Percent(70)),
            ("HDMI-A-1".to_string(), Bright::Unsupported),
        ];
        on_ddc(&mut s, DdcEvent::Detected(found), &outputs, &mut logged);
        s.show(Shown::Brightness, t0);
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::Brightness { percent: 70 }));
        assert_eq!(s.panel_for("HDMI-A-1", t0), Some(PanelView::BrightnessUnsupported));
        assert_eq!(s.panel_for("eDP-1", t0), Some(PanelView::BrightnessUnsupported));
        let msg = |n: &str| {
            format!("thinfloe-menu --osd: monitor {n} does not answer DDC/CI; the brightness keys leave it alone")
        };
        assert!(!logged.once(msg("HDMI-A-1")));
        assert!(!logged.once(msg("eDP-1")));
        assert!(logged.once(msg("DP-1")));
        on_ddc(
            &mut s,
            DdcEvent::Level {
                connector: "DP-1".into(),
                percent: 80,
            },
            &outputs,
            &mut logged,
        );
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::Brightness { percent: 80 }));
    }

    #[test]
    fn redetections_coalesce() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = Redetect::default();
        r.started();
        // A storm while the first detection runs.
        for ms in [0, 100, 200, 300] {
            r.outputs_changed(at(ms));
        }
        assert_eq!(r.deadline(), Some(at(2300)));
        assert!(!r.due(at(2299)));
        assert!(!r.due(at(2300)), "the first detection still runs");
        assert!(!r.due(at(2400)));
        assert!(r.finished(), "one more after it");
        assert!(!r.finished());
        // A change while nothing runs starts one after the delay.
        r.outputs_changed(at(5000));
        assert!(r.due(at(7000)));
        r.outputs_changed(at(7100));
        assert!(!r.due(at(9100)));
        assert!(r.finished());
        assert!(!r.finished());
    }

    #[test]
    fn the_scale_follows_the_labwc_scale_rule() {
        assert_eq!(parse_scale("1.5\n"), 1.5);
        assert_eq!(parse_scale("2"), 2.0);
        assert_eq!(parse_scale("8\nmore\n"), 8.0);
        for bad in ["", "0.4", "9", "1.5 2", "-1", "1.", ".5", "1e0", "nan", "inf", "x"] {
            assert_eq!(parse_scale(bad), 1.0, "{bad:?}");
        }
        assert_eq!(read_scale(None), 1.0);
        assert_eq!(read_scale(Some(Path::new("/nonexistent/thinfloe-scale"))), 1.0);
    }
}
