//! The OSD process's loop: keys from the socket, results from the audio
//! and monitor workers and the microphone poll, the Wayland connection,
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

use super::audio::{AudioEvent, AudioRequest, AudioWorker, Wpctl};
use super::ddc::{Bright, DdcEvent, Ddcutil, Monitors};
use super::draw::{Painter, PanelView};
use super::state::{OsdState, Shown};
use super::surface::{SurfaceKind, Surfaces, mark_placement, panel_placement};
use super::{Action, POLL_MAX_MS, STOP, VOLUME_STEP, socket};

/// How often the microphone's mute state is read for the mark.
const MIC_POLL: Duration = Duration::from_secs(1);
/// How long after the outputs change the monitors are detected again:
/// a monitor that just came answers DDC/CI only after a while.
const REDETECT_DELAY: Duration = Duration::from_secs(2);

/// What the worker threads report to the loop.
enum Msg {
    Audio(AudioEvent),
    /// The microphone poll's answer.
    MicPoll(Result<bool, String>),
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
/// otherwise log every second.
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
/// knows there is none.
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
    panel: Option<(PanelView, f32)>,
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

    let n = notify.clone();
    let audio = AudioWorker::spawn(Wpctl::from_env(), move |e| n.send(Msg::Audio(e)));
    let n = notify.clone();
    let monitors = Monitors::spawn(Ddcutil::from_env(), move |e| n.send(Msg::Ddc(e)));
    let mut redetect = Redetect::default();
    monitors.redetect();
    redetect.started();
    let n = notify.clone();
    std::thread::spawn(move || {
        let w = Wpctl::from_env();
        while !STOP.load(Ordering::SeqCst) {
            n.send(Msg::MicPoll(w.source_muted()));
            std::thread::sleep(MIC_POLL);
        }
    });
    drop(notify);

    let mut state = OsdState::new();
    let mut logged = Logged::default();
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
        let mut fds = [surfaces.fd().as_raw_fd(), sock.as_raw_fd(), wake_rx.as_raw_fd()].map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        // SAFETY: three valid pollfds for the duration of the call.
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
        for a in drain_socket(&sock) {
            dirty |= on_action(&mut state, a, &audio, &monitors, now);
        }
        let mut buf = [0u8; 64];
        while matches!((&wake_rx).read(&mut buf), Ok(n) if n > 0) {}
        let outputs = surfaces.outputs();
        while let Ok(m) = rx.try_recv() {
            dirty |= match m {
                Msg::Audio(e) => on_audio(&mut state, e, now, &mut logged),
                Msg::MicPoll(r) => on_mic_poll(&mut state, r, &mut logged),
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

/// The actions waiting on the socket; other datagrams are dropped.
fn drain_socket(sock: &UnixDatagram) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        match sock.recv(&mut buf) {
            Ok(n) => actions.extend(socket::decode(&buf[..n])),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return actions,
        }
    }
}

/// Start what `a` asks for; the brightness panel shows at once, the
/// volume and microphone panels when `wpctl` has answered. True when
/// something to draw changed.
fn on_action(state: &mut OsdState, a: Action, audio: &AudioWorker, monitors: &Monitors, now: Instant) -> bool {
    match a {
        Action::VolumeUp => audio.request(AudioRequest::Step(VOLUME_STEP)),
        Action::VolumeDown => audio.request(AudioRequest::Step(-VOLUME_STEP)),
        Action::VolumeMute => audio.request(AudioRequest::ToggleMute),
        Action::MicMute => audio.request(AudioRequest::ToggleMic),
        Action::BrightnessUp | Action::BrightnessDown => {
            monitors.step(a == Action::BrightnessUp);
            state.show(Shown::Brightness, now);
            return true;
        }
    }
    false
}

/// The answer of a volume or microphone key. A failure (no sound device)
/// is logged once per message and shows nothing.
pub(super) fn on_audio(state: &mut OsdState, e: AudioEvent, now: Instant, logged: &mut Logged) -> bool {
    match e {
        AudioEvent::Sink(l) => state.show(Shown::Sink(l), now),
        AudioEvent::Mic(m) => {
            state.set_mic(m);
            state.show(Shown::Mic(m), now);
        }
        AudioEvent::Failed(why) => {
            logged.once(format!("thinfloe-menu --osd: {why}"));
            return false;
        }
    }
    true
}

/// The microphone poll keeps the mark; an error counts as not muted.
fn on_mic_poll(state: &mut OsdState, r: Result<bool, String>, logged: &mut Logged) -> bool {
    let muted = r.unwrap_or_else(|why| {
        logged.once(format!("thinfloe-menu --osd: {why}"));
        false
    });
    state.set_mic(muted)
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
    let mut panels: Vec<(PanelView, Vec<u32>)> = Vec::new();
    let mut mark_px: Option<Vec<u32>> = None;
    for out in outputs {
        let d = drawn.entry(out.clone()).or_default();
        let want = state.panel_for(&out, now).map(|v| (v, scale));
        if d.panel != want {
            let shown = match &want {
                Some((v, _)) => {
                    let place = panel_placement(scale, v.has_bar());
                    let px = match panels.iter().find(|(p, _)| p == v) {
                        Some((_, px)) => px,
                        None => {
                            panels.push((v.clone(), painter.panel(v, scale).pixels));
                            &panels.last().expect("just pushed").1
                        }
                    };
                    surfaces.show(&out, SurfaceKind::Panel, &place, place.size.0, px)
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
    fn a_failed_audio_event_shows_no_panel() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let mut logged = Logged::default();
        let why = || AudioEvent::Failed("wpctl get-volume @DEFAULT_AUDIO_SINK@ printed no volume".into());
        assert!(!on_audio(&mut s, why(), t0, &mut logged));
        assert_eq!(s.panel_for("DP-1", t0), None);
        assert!(!s.mark());
        // Logged once, not on every key.
        assert!(!logged.once("thinfloe-menu --osd: wpctl get-volume @DEFAULT_AUDIO_SINK@ printed no volume".into()));
    }

    #[test]
    fn a_sink_event_shows_the_level_and_a_mic_event_sets_the_mark() {
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
        assert!(s.mark());
    }

    #[test]
    fn the_mic_poll_keeps_the_mark_without_a_panel() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let mut logged = Logged::default();
        assert!(on_mic_poll(&mut s, Ok(true), &mut logged));
        assert!(s.mark());
        assert_eq!(s.panel_for("DP-1", t0), None);
        assert!(!on_mic_poll(&mut s, Ok(true), &mut logged));
        // No sound device: no mark.
        assert!(on_mic_poll(&mut s, Err("no device".into()), &mut logged));
        assert!(!s.mark());
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
