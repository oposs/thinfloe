//! The event loop: winit for the Wayland window, egui-winit for input,
//! egui for layout, raster.rs for pixels, softbuffer to put them on screen.
//! Driven by hand rather than through eframe so that nothing here needs a
//! GPU. The rules this file follows are in the spec under "Rendering
//! without a GPU".

use crate::addons::{self, AddonLog};
use crate::battery;
use crate::damage::Tracker;
use crate::density::{self, OutputInfo};
use crate::icons::IconRoots;
use crate::jobs::{self, JobResult, Paths};
use crate::log::Feed;
use crate::menu::{Effect, Job, Menu, Screen};
use crate::menu_move::{self, MenuMove, Replace};
use crate::netfiles;
use crate::network;
use crate::raster::{Background, Canvas, Textures};
use crate::release;
use crate::settings::SettingsTab;
use crate::ui;
use crate::upgrade::{self, Stage};
use egui::ViewportId;
use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

/// How much of the grid's brightness survives behind the logout dialog.
pub(crate) const DIM_KEEP: u32 = 90;
/// The shortest time between two wake-ups for program output: a program
/// that floods its output costs at most ten frames a second.
const OUTPUT_PACE: Duration = Duration::from_millis(100);

/// Why the loop was woken from another thread.
#[derive(Debug)]
pub enum Wake {
    Job(JobResult),
    /// Lines are waiting in the feed.
    Output,
    /// The add-on status or log file changed.
    Addons,
    /// The battery file changed.
    Battery,
    /// The set of folded windows changed.
    Windows,
    /// The release status file of thinfloe-upgrade changed.
    Release,
    /// The compositor reported the monitors anew.
    Heads,
    /// The network element appeared or went: a hidden element asks for no
    /// frames of its own.
    Network,
    /// The hardware element appeared or went.
    Hardware,
    /// The status layout file changed.
    Status,
}

/// How often the add-on files are looked at, while the window is visible.
const ADDON_POLL: Duration = Duration::from_secs(1);
/// Polls of the log in one wake-up: a log of 500 lines read at 200 a poll.
const ADDON_LOG_POLLS: usize = 5;

pub struct Config {
    pub menu: Menu,
    pub paths: Paths,
    pub icon_roots: IconRoots,
    pub debug: bool,
    pub addon_status: PathBuf,
    pub addon_log: PathBuf,
    pub addon_last_check: PathBuf,
    /// The file the battery reader writes.
    pub battery_file: PathBuf,
    /// `config.toml`, watched for changes to its `[status]` table.
    pub config_toml: PathBuf,
    /// Starting up regenerated `generated/kanshi`: the running kanshi is told
    /// once to read it again.
    pub kanshi_changed: bool,
    /// Where the network element's sampler reads /sys, /proc, /run, /etc.
    pub network_roots: crate::netstat::Roots,
    /// Where the hardware element's sampler reads /sys and /proc.
    pub hardware_roots: crate::hardware::Roots,
    /// Where the programs of add-on tiles are looked for.
    pub program_path: OsString,
    /// Host name and version for the header.
    pub header: crate::header::Header,
    /// The release status file `thinfloe-upgrade check` writes.
    pub upgrade_status: PathBuf,
    pub screen: ScreenFile,
    /// `TF_MENU_DEBUG_MOVE`: the output the menu belongs on whatever the
    /// Screen tab says, for the runtime tests.
    pub debug_move: Option<String>,
}

/// Who is told when the screen settings change.
pub struct ScreenFile {
    /// The idle helper, told by SIGUSR1 to read the file again.
    pub idle_pid: Option<i32>,
}

struct Live {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    egui: egui_winit::State,
    canvas: Canvas,
    textures: Textures,
    tracker: Tracker,
    icons: Vec<Option<ui::Icon>>,
    /// What the header shows; the clock text is refreshed every frame.
    chrome: ui::Chrome,
    /// The scale the last frame was laid out and damaged at.
    ppp: f32,
    /// The menu's own scale `s`: 1 point is `s` pixels, before the
    /// compositor's scale factor.
    scale: f32,
    /// Name of the monitor `scale` was computed for.
    monitor: Option<String>,
    /// When the window was made.
    started: Instant,
    /// The outputs the compositor reported at the last read.
    outputs: Vec<OutputInfo>,
    /// Whether a frame reached the screen.
    presented: bool,
    /// Whether the canvas holds a whole frame. A new canvas (a new window,
    /// a resize) holds none, so there is nothing to freeze under a dialog.
    painted: bool,
    /// When the next frame is due; what a look at a covered menu must not
    /// skip.
    frame_due: Option<Instant>,
}

pub struct App {
    cfg: Config,
    ctx: egui::Context,
    proxy: EventLoopProxy<Wake>,
    /// Program output on its way into the menu's log.
    feed: Arc<Feed>,
    live: Option<Live>,
    /// Set when the menu must end; `run` returns it as the process status.
    exit: Option<i32>,
    /// Whether the canvas background is the frozen grid.
    frozen: bool,
    addon_log: AddonLog,
    /// Whether the log was read once already.
    addon_log_read: bool,
    /// Whether the window is on screen; the add-on poll rests when not.
    shown: Arc<AtomicBool>,
    /// Folded windows; `None` when the compositor cannot tell, then no strip.
    windows: Option<crate::windows::Windows>,
    /// The folded list at the last wake, to name what changed.
    folded_seen: Vec<crate::windows::Folded>,
    /// The monitors; `None` when the compositor cannot tell, then the
    /// Screen tab shows none.
    heads: Option<crate::heads::Heads>,
    /// The menu's output file for labwc-scale; `None` without
    /// `XDG_RUNTIME_DIR`, then the menu stays where labwc puts it.
    output_file: Option<PathBuf>,
    /// The output the menu asked labwc-scale for, and the wait for it.
    menu_move: MenuMove,
    /// The samplers of the status area, started when first needed.
    samplers: Samplers,
    /// Whether a window covered the menu, and its height in points, at the
    /// last frame: what `sync_samplers` decides from.
    covered: bool,
    height: f32,
}

/// Starts a sampler thread with its `on` flag and returns its shared copy.
type Spawn<T> = Box<dyn FnMut(Arc<AtomicBool>) -> Arc<Mutex<T>>>;

/// The samplers of the status area. Each starts the first time a ticked
/// element draws from it (placed or not: an element without content must
/// still get data to gain some) and is never started again; afterwards its
/// `on` flag says whether it reads. A resting sampler reads nothing.
struct Samplers {
    network: Option<Arc<Mutex<crate::netstat::Series>>>,
    /// The network sampler reads while a ticked element needs it and no
    /// window covers the menu.
    network_on: Arc<AtomicBool>,
    hardware: Option<Arc<Mutex<Option<crate::hardware::Snapshot>>>>,
    /// The hardware sampler reads as `hardware_wanted` says.
    hardware_on: Arc<AtomicBool>,
    spawn_network: Spawn<crate::netstat::Series>,
    spawn_hardware: Spawn<Option<crate::hardware::Snapshot>>,
}

impl Samplers {
    fn new(
        spawn_network: Spawn<crate::netstat::Series>,
        spawn_hardware: Spawn<Option<crate::hardware::Snapshot>>,
    ) -> Samplers {
        Samplers {
            network: None,
            network_on: Arc::new(AtomicBool::new(false)),
            hardware: None,
            hardware_on: Arc::new(AtomicBool::new(false)),
            spawn_network,
            spawn_hardware,
        }
    }

    /// The samplers' latest copies, taken under their locks for as long as
    /// a copy of 600 numbers takes; `None` from a sampler that has not read
    /// yet, so its elements wait rather than have nothing to show.
    fn copies(&self) -> (Option<crate::netstat::Snapshot>, Option<crate::hardware::Snapshot>) {
        let network = self
            .network
            .as_ref()
            .and_then(|n| n.lock().ok().filter(|s| s.has_read()).map(|s| s.snapshot()));
        let hardware = self
            .hardware
            .as_ref()
            .and_then(|h| h.lock().ok().and_then(|s| s.clone()));
        (network, hardware)
    }

    /// Start what `area` needs for the first time and set the flags, for a
    /// menu `covered` or not and `height` points high.
    fn sync(&mut self, area: &crate::status::Area, covered: bool, height: f32) {
        use crate::status::Source;
        let network = area.needs(Source::Network);
        // The flags first, so a new thread's first look finds them set.
        self.network_on.store(network && !covered, Ordering::Relaxed);
        self.hardware_on
            .store(hardware_wanted(area, covered, height), Ordering::Relaxed);
        if network && self.network.is_none() {
            self.network = Some((self.spawn_network)(self.network_on.clone()));
        }
        if area.needs(Source::Hardware) && self.hardware.is_none() {
            self.hardware = Some((self.spawn_hardware)(self.hardware_on.clone()));
        }
    }
}

/// Run the menu until it exits. Returns the exit status for thinfloe-session.
pub fn run(mut cfg: Config) -> Result<i32, String> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let ctx = egui::Context::default();
    // Once per context, before the style writes below, which it would undo.
    crate::brand::apply(&ctx);
    // Feathering assumes coverage blending, which an integer rasteriser does
    // not do: it shows up as fringes on rounded corners.
    ctx.options_mut(|o| o.tessellation_options.feathering = false);
    // An animated widget on a CPU rasteriser arrives as visibly staged full
    // frames.
    ctx.global_style_mut(|s| s.animation_time = 0.0);
    let feed = Arc::new(Feed::default());
    let waker = event_loop.create_proxy();
    let waiting = feed.clone();
    std::thread::spawn(move || {
        loop {
            waiting.wait();
            // The loop is gone only when the menu is exiting.
            if waker.send_event(Wake::Output).is_err() {
                return;
            }
            std::thread::sleep(OUTPUT_PACE);
        }
    });
    let shown = Arc::new(AtomicBool::new(true));
    watch_files(&cfg, &shown, event_loop.create_proxy());
    if cfg.kanshi_changed {
        let kanshi = cfg.paths.kanshi.clone();
        std::thread::spawn(move || {
            if let Err(e) = jobs::kanshi_reload(&kanshi) {
                eprintln!("thinfloe-menu: {e}");
            }
        });
    }
    let windows = {
        let waker = event_loop.create_proxy();
        match crate::windows::Windows::connect(move || {
            let _ = waker.send_event(Wake::Windows);
        }) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("thinfloe-menu: no folded-window list: {e}");
                None
            }
        }
    };
    // No change event comes for a window folded before the menu started, so
    // the first frame reads the list itself.
    if let Some(w) = windows.as_ref() {
        cfg.menu.folded = w.folded();
    }
    let heads = {
        let waker = event_loop.create_proxy();
        match crate::heads::Heads::connect(move || {
            let _ = waker.send_event(Wake::Heads);
        }) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("thinfloe-menu: no monitor list: {e}");
                None
            }
        }
    };
    // The monitors present now came before the change callback was set:
    // the loop reads them first thing.
    if heads.is_some() {
        let _ = event_loop.create_proxy().send_event(Wake::Heads);
    }
    let samplers = {
        let (net_waker, hw_waker) = (event_loop.create_proxy(), event_loop.create_proxy());
        let (net_roots, hw_roots) = (cfg.network_roots.clone(), cfg.hardware_roots.clone());
        Samplers::new(
            Box::new(move |on| {
                let waker = net_waker.clone();
                crate::netstat::spawn(net_roots.clone(), on, move || {
                    // The loop is gone only when the menu is exiting.
                    let _ = waker.send_event(Wake::Network);
                })
            }),
            Box::new(move |on| {
                let waker = hw_waker.clone();
                crate::hardware::spawn(hw_roots.clone(), on, move || {
                    let _ = waker.send_event(Wake::Hardware);
                })
            }),
        )
    };
    let mut app = App {
        samplers,
        covered: false,
        height: ui::SHORT_SCREEN_H,
        windows,
        folded_seen: Vec::new(),
        heads,
        addon_log: AddonLog::new(cfg.addon_log.clone()),
        addon_log_read: false,
        shown,
        cfg,
        ctx,
        proxy: event_loop.create_proxy(),
        feed,
        live: None,
        exit: None,
        frozen: false,
        output_file: menu_move::output_file_path(),
        menu_move: MenuMove::default(),
    };
    // Before the first frame, so a sampler the layout needs has a reading
    // by the time the area shows. The first frame corrects the height.
    app.sync_samplers();
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.exit
        .ok_or_else(|| "the event loop ended without an exit request".to_string())
}

/// The files the watcher thread looks at, and what it last saw of them.
struct Watch {
    addon_status: PathBuf,
    addon_log: PathBuf,
    upgrade_status: PathBuf,
    battery_file: PathBuf,
    config_toml: PathBuf,
    addons: addons::Watcher,
    release: release::Watcher,
    battery: battery::Watcher,
    /// A stamp watcher like the battery file's.
    status: battery::Watcher,
}

impl Watch {
    fn new(cfg: &Config) -> Watch {
        Watch {
            addon_status: cfg.addon_status.clone(),
            addon_log: cfg.addon_log.clone(),
            upgrade_status: cfg.upgrade_status.clone(),
            battery_file: cfg.battery_file.clone(),
            config_toml: cfg.config_toml.clone(),
            // The first tick reads the files whatever they hold, so a change
            // between the menu's first read and now is not lost.
            addons: addons::Watcher::default(),
            release: release::Watcher::default(),
            battery: battery::Watcher::default(),
            status: battery::Watcher::default(),
        }
    }

    /// The wake-ups one look at the files calls for.
    fn tick(&mut self) -> Vec<Wake> {
        let mut wakes = Vec::new();
        if self.addons.tick(&self.addon_status, &self.addon_log) {
            wakes.push(Wake::Addons);
        }
        if self.release.tick(&self.upgrade_status) {
            wakes.push(Wake::Release);
        }
        if self.battery.tick(&self.battery_file) {
            wakes.push(Wake::Battery);
        }
        if self.status.tick(&self.config_toml) {
            wakes.push(Wake::Status);
        }
        wakes
    }
}

/// A thread that looks at the add-on files, the release status, the battery
/// file and the status layout once a second and wakes the loop only when one
/// of them changed: the loop sleeps in between.
fn watch_files(cfg: &Config, shown: &Arc<AtomicBool>, waker: EventLoopProxy<Wake>) {
    let shown = shown.clone();
    let mut watch = Watch::new(cfg);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(ADDON_POLL);
            if !shown.load(Ordering::Relaxed) {
                continue;
            }
            for wake in watch.tick() {
                if waker.send_event(wake).is_err() {
                    return;
                }
            }
        }
    });
}

/// When the loop must wake for the Upgrade screen's drive poll: while the
/// person chooses a drive and the window is on screen.
fn upgrade_wake(screen: Screen, stage: &Stage, shown: bool) -> Option<Duration> {
    (screen == Screen::Upgrade && *stage == Stage::Choose && shown).then_some(upgrade::POLL)
}

/// When the loop must wake for the Network tab's status poll: while the tab
/// is open and the window is on screen.
fn network_wake(screen: Screen, tab: SettingsTab, shown: bool) -> Option<Duration> {
    (screen == Screen::Settings && tab == SettingsTab::Network && shown).then_some(network::POLL)
}

/// The body of a job's worker thread. Every result goes through `send`:
/// the start of a program first, a write's progress lines in the order the
/// tool printed them, and the final result last.
fn run_job(
    job: Job,
    paths: &Paths,
    feed: &Arc<Feed>,
    netboot: bool,
    send: impl Fn(JobResult) + Clone + Send + 'static,
) {
    // A panic in the job still sends a result: without one the screen's
    // poll or check would wait for ever.
    let on_panic = panic_result(&job);
    let s = send.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        job_result(job, paths, feed, netboot, s)
    }))
    .unwrap_or(on_panic);
    send(result);
}

/// The failed result of `job`, for a worker that panicked.
fn panic_result(job: &Job) -> JobResult {
    let e = "the menu's worker failed; the session log shows why".to_string();
    match job {
        Job::Save => JobResult::Saved(jobs::SaveOutcome::Broken(e)),
        Job::Run { id, .. } => JobResult::Ran {
            id: *id,
            status: Err(e),
        },
        Job::Addon { args } => JobResult::Addon {
            args: args.clone(),
            status: Err(e),
        },
        Job::UpgradeWrite { .. } => JobResult::UpgradeEnded { status: Err(e) },
        Job::Drives { all } => JobResult::Drives {
            all: *all,
            result: Err(e),
        },
        Job::UpgradeCheck { source } => JobResult::Release {
            custom: source.is_some(),
            result: Err(e),
        },
        Job::NetLoad | Job::NetStatus | Job::NetScan | Job::NetApply { .. } | Job::NetTakeOver { .. } => {
            net_failed(job, e)
        }
        // Unreadable: nothing is written over a file nobody could read.
        Job::MonLoad => JobResult::MonLoaded(Err(e), crate::monitors::MonSource::Kept),
        Job::MonApply { .. } => JobResult::MonApplied(Err(e)),
        // The marker may still be there: the next session would go back.
        Job::MonKeep { .. } => JobResult::MonKept(Err(e)),
        Job::MonRevert => JobResult::MonReverted(Err(e)),
        Job::MonTakeOver { .. } => JobResult::MonTakenOver(Err(e)),
        Job::StatusSave(_) => JobResult::StatusSaved(Err(e)),
        Job::KeyboardApply(kb) => JobResult::KeyboardApplied {
            kb: kb.clone(),
            result: Err(e),
        },
        Job::DismissReport => JobResult::ReportDismissed(Err(e)),
    }
}

/// The failed result of a network job: the files read as unreadable, the
/// calls as failed.
fn net_failed(job: &Job, e: String) -> JobResult {
    let wifi = || netfiles::WifiFile {
        state: netfiles::FileState::Hand,
        device: None,
        networks: Vec::new(),
        dns: Vec::new(),
        manageable: false,
        error: Some(e.clone()),
    };
    match job {
        Job::NetLoad => JobResult::NetLoaded(
            wifi(),
            netfiles::WiredFile {
                state: netfiles::FileState::Hand,
                wired: netfiles::Wired::dhcp(),
                manageable: false,
                error: Some(e.clone()),
            },
            Vec::new(),
        ),
        Job::NetStatus => JobResult::NetStatus(Err(e)),
        Job::NetScan => JobResult::NetScan(Err(e)),
        Job::NetApply { .. } => JobResult::NetApplied(Err(jobs::ApplyFailed::new(e))),
        _ => JobResult::NetTakenOver(Err(e)),
    }
}

fn job_result(
    job: Job,
    paths: &Paths,
    feed: &Arc<Feed>,
    netboot: bool,
    send: impl Fn(JobResult) + Clone + Send + 'static,
) -> JobResult {
    match job {
        Job::Save => JobResult::Saved(jobs::save(paths, feed)),
        Job::Run { id, name, argv } => {
            let started = send.clone();
            jobs::run(id, &name, &argv, feed, move |pgid| {
                started(JobResult::Started { id, pgid })
            })
        }
        Job::Addon { args } => {
            let started = send.clone();
            let a = args.clone();
            jobs::addon(&paths.addon_cmd, &args, feed, move |pgid| {
                started(JobResult::AddonStarted { args: a, pgid })
            })
        }
        // A tool that cannot start comes back as an `Err` result, so the
        // Upgrade screen never waits for an answer that is not coming.
        Job::UpgradeWrite { args } => {
            let started = send.clone();
            let lines = send.clone();
            let status = jobs::upgrade_write(
                &paths.upgrade_cmd,
                &args,
                feed,
                move |pgid| started(JobResult::UpgradeStarted { pgid }),
                move |m| lines(JobResult::UpgradeMsg(m)),
            );
            JobResult::UpgradeEnded { status }
        }
        Job::Drives { all } => JobResult::Drives {
            all,
            result: jobs::upgrade_drives(&paths.upgrade_cmd, all, feed),
        },
        Job::UpgradeCheck { source } => JobResult::Release {
            custom: source.is_some(),
            result: jobs::upgrade_check(&paths.upgrade_cmd, source.as_deref(), feed),
        },
        Job::NetLoad => {
            let (wifi, wired) = jobs::net_load(paths);
            JobResult::NetLoaded(wifi, wired, netfiles::wifi_devices(&paths.homes.sys_net))
        }
        Job::NetStatus => JobResult::NetStatus(jobs::network_status(paths, feed)),
        Job::NetScan => JobResult::NetScan(jobs::network_scan(paths, feed)),
        // The root tool's group is reported at once, so a logout can stop it.
        Job::NetApply { change } => {
            let started = send.clone();
            JobResult::NetApplied(jobs::network_apply(paths, &change, netboot, feed, move |pgid| {
                started(JobResult::NetApplyStarted { pgid })
            }))
        }
        Job::NetTakeOver { which } => JobResult::NetTakenOver(jobs::net_take_over(paths, which)),
        Job::MonLoad => {
            let (profiles, source) = jobs::mon_load(&paths.config);
            JobResult::MonLoaded(profiles, source)
        }
        // The connector name is for the window move, which the menu does.
        Job::MonApply { profiles, prev, .. } => JobResult::MonApplied(jobs::mon_apply(
            &paths.config,
            &paths.kanshi,
            &profiles,
            prev.as_deref(),
        )),
        Job::MonKeep { profiles } => JobResult::MonKept(jobs::mon_keep(&paths.config, &paths.kanshi, profiles)),
        Job::MonRevert => JobResult::MonReverted(jobs::mon_revert(&paths.kanshi)),
        Job::MonTakeOver { heads } => {
            JobResult::MonTakenOver(jobs::mon_take_over(&paths.config, &paths.kanshi, &heads))
        }
        Job::StatusSave(text) => JobResult::StatusSaved(jobs::status_save(&paths.config, text.as_deref())),
        Job::KeyboardApply(kb) => {
            let result = jobs::keyboard_apply(paths, &kb, feed);
            JobResult::KeyboardApplied { kb, result }
        }
        Job::DismissReport => JobResult::ReportDismissed(jobs::dismiss_report(&paths.config)),
    }
}

impl Live {
    /// Pixels per point the icons and the logo are rendered at.
    fn icon_ppp(&self) -> f32 {
        self.scale * self.window.scale_factor() as f32
    }
}

/// Whether egui has not yet taken the zoom factor `scale`: the pass that
/// takes it is laid out at a guessed size and must not be shown.
fn settling(zoom: f32, scale: f32) -> bool {
    (zoom - scale).abs() > f32::EPSILON
}

/// The scale for a window, and the output it was taken from: the output
/// the window is on, else the fallback for the monitor's size, else the
/// largest mode, else 1920x1080. A window without a size yet counts as
/// as wide as the output's mode.
fn pick_scale<'a>(
    outputs: &'a [OutputInfo],
    monitor_name: Option<&str>,
    monitor_px: Option<(u32, u32)>,
    window_w_px: u32,
) -> (f32, Option<&'a OutputInfo>) {
    let output = density::choose_output(outputs, monitor_name, monitor_px);
    let fallback_px = monitor_px
        .or_else(|| {
            outputs
                .iter()
                .map(|o| o.mode_px)
                .max_by_key(|m| m.0 as u64 * m.1 as u64)
        })
        .unwrap_or((1920, 1080));
    let width = if window_w_px > 0 {
        window_w_px
    } else {
        output.map_or(fallback_px.0, |o| o.mode_px.0)
    };
    (density::menu_scale(output, fallback_px, width), output)
}

/// How often the menu looks again for the monitor its window is on, and for
/// how long after the start.
const MONITOR_RECHECK: Duration = Duration::from_millis(100);
const MONITOR_RECHECK_FOR: Duration = Duration::from_secs(10);

/// When to look again for the window's monitor, if at all. winit reports
/// the monitor only once the compositor has shown the window there, and
/// sends no event when it does (`surface_enter` is empty in winit 0.30), so
/// the first scale is taken before it is known. With one output that is
/// still the right one; with several, or none, the scale is a guess (the
/// largest mode's fallback), which would stay until something else redrew
/// the menu.
fn monitor_recheck(known: bool, outputs: usize, since_start: Duration) -> Option<Duration> {
    (!known && outputs != 1 && since_start < MONITOR_RECHECK_FOR).then_some(MONITOR_RECHECK)
}

/// True when the monitor named `name` has a size other than the mode stored
/// for that output. A rotated monitor swaps width and height, so the sides
/// are compared as a sorted pair.
fn mode_changed(outputs: &[OutputInfo], name: Option<&str>, monitor_px: Option<(u32, u32)>) -> bool {
    let (Some(name), Some(px)) = (name, monitor_px) else {
        return false;
    };
    let Some(o) = outputs.iter().find(|o| o.name.as_deref() == Some(name)) else {
        return false;
    };
    let sorted = |p: (u32, u32)| (p.0.min(p.1), p.0.max(p.1));
    sorted(o.mode_px) != sorted(px)
}

/// True when the window is on another monitor than `known`, or the monitor
/// `name` shows a mode other than the stored one. Compares known values only:
/// no monitor is no news (see `seen_monitor`).
fn monitor_moved(outputs: &[OutputInfo], known: Option<&str>, name: Option<&str>, px: Option<(u32, u32)>) -> bool {
    (name.is_some() && name != known) || mode_changed(outputs, name, px)
}

/// The monitor to take the scale from: the one winit reports, else the last
/// known one. winit reports the outputs the window is visible on, and a
/// window that another one covers whole, such as the menu behind a
/// fullscreen OxuLnk or ThinLinc window, is visible on none. Taken as a move,
/// that gave the guess for the largest mode (2 next to a 4K monitor) and the
/// title bars grew until the menu happened to draw again uncovered.
fn seen_monitor(name: Option<String>, known: Option<&str>) -> Option<String> {
    name.or_else(|| known.map(str::to_string))
}

/// How often a covered menu looks whether it still is.
const COVERED_LOOK: Duration = Duration::from_secs(1);

/// Whether a window covers the menu whole: winit names no monitor (see
/// `seen_monitor`), and one was named before, since a window that was never
/// shown names none either. winit's Wayland backend never sends
/// `WindowEvent::Occluded`, and this is the only sign there is.
fn is_covered(seen: bool, known: bool) -> bool {
    !seen && known
}

/// When the loop must wake: for the next frame, and, while covered, to look
/// again, because uncovering sends no event (`surface_enter` is empty in winit
/// 0.30). `None`: only an event wakes it.
fn next_wake(frame_due: Option<Instant>, covered: bool, now: Instant) -> Option<Instant> {
    frame_due.into_iter().chain(covered.then(|| now + COVERED_LOOK)).min()
}

/// Whether a reached wake-up draws, with `covered` what the look found. A look
/// that finds the menu still covered draws nothing unless a frame is due: the
/// save timer and the pause before logout advance through those frames.
fn frame_wanted(covered: bool, frame_due: Option<Instant>, now: Instant) -> bool {
    !covered || frame_due.is_some_and(|due| due <= now)
}

fn wake_flow(at: Option<Instant>) -> ControlFlow {
    at.map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
}

/// Read the `[status]` table of `config.toml` into `menu` and start and rest
/// the samplers for it, with the cover and height of the last frame. A file
/// that cannot be read or parsed leaves the layout on screen as it is.
fn reload_status(menu: &mut Menu, samplers: &mut Samplers, file: &std::path::Path, covered: bool, height: f32) {
    if let Some(layout) = crate::status::read_config_layout(file) {
        menu.set_status_layout(layout);
    }
    samplers.sync(&menu.status, covered, height);
}

/// Whether the hardware sampler reads: not while a window covers the menu,
/// and only when an element that draws from it has cells on this screen. On a
/// short screen no hardware element has a size, so it rests there too.
fn hardware_wanted(status: &crate::status::Area, covered: bool, height: f32) -> bool {
    !covered && status.places(crate::status::Source::Hardware, ui::short_screen(height))
}

impl App {
    /// Computes the menu's scale again. When it changed, or with `force`,
    /// the zoom factor, the icons, the logo and the scale file follow.
    fn apply_scale(&mut self, force: bool) {
        let Some(live) = self.live.as_mut() else { return };
        let monitor = live.window.current_monitor();
        let name = seen_monitor(monitor.as_ref().and_then(|m| m.name()), live.monitor.as_deref());
        let px = monitor.as_ref().map(|m| (m.size().width, m.size().height));
        // The first look at a monitor needs no second read of the outputs.
        // A mode change on the same output (wlr-randr --mode, kanshi) needs one too.
        if (live.monitor.is_some() && name != live.monitor) || mode_changed(&live.outputs, name.as_deref(), px) {
            live.outputs = read_outputs(self.cfg.debug);
        }
        let (s, chosen) = pick_scale(&live.outputs, name.as_deref(), px, live.window.inner_size().width);
        let chosen = chosen.cloned();
        live.monitor = name;
        // Without a menu monitor chosen, this one carries the menu and
        // stays on in the Screen tab.
        self.cfg.menu.monitors.menu_is_on(live.monitor.as_deref());
        if !force && (s - live.scale).abs() < f32::EPSILON {
            return;
        }
        live.scale = s;
        let ppp = live.icon_ppp();
        self.ctx.set_zoom_factor(s);
        live.icons = ui::load_icons(&self.ctx, &self.cfg.menu.tiles, &self.cfg.icon_roots, ppp);
        let logo_px = (ui::LOGO_H * ppp).round() as u32;
        live.chrome.logo = Some(self.ctx.load_texture(
            "thinfloe-logo",
            crate::brand::logo(logo_px),
            egui::TextureOptions::LINEAR,
        ));
        if let Some(path) = density::scale_file_path()
            && let Err(e) = density::write_scale_file(&path, s)
        {
            eprintln!("thinfloe-menu: cannot write the scale file: {e}");
        }
        if self.cfg.debug {
            let (n, m, mm) = match &chosen {
                Some(o) => (o.name.as_deref().unwrap_or("?").to_string(), o.mode_px, o.phys_mm),
                None => ("?".to_string(), (0, 0), (0, 0)),
            };
            eprintln!(
                "thinfloe-menu: scale {s} on {n} ({}x{} px, {}x{} mm)",
                m.0, m.1, mm.0, mm.1
            );
        }
        live.window.request_redraw();
    }

    /// Start and rest the samplers for the layout and the cover as they are
    /// now. Called after every layout change and on every frame, which is
    /// where a cover change is seen.
    fn sync_samplers(&mut self) {
        self.samplers.sync(&self.cfg.menu.status, self.covered, self.height);
    }

    /// Read the status layout file again: the person or the menu changed it.
    fn refresh_status(&mut self) {
        reload_status(
            &mut self.cfg.menu,
            &mut self.samplers,
            &self.cfg.config_toml,
            self.covered,
            self.height,
        );
    }

    /// Read the battery file again.
    fn refresh_battery(&mut self) {
        self.cfg
            .menu
            .set_battery(battery::read(&self.cfg.battery_file), Instant::now());
    }

    /// Read the add-on files again. A program or icon that appeared
    /// (the install runs while the menu does) is picked up here, the icon in
    /// a renderer process like every other.
    fn refresh_addons(&mut self) {
        let changed = self.cfg.menu.set_addons(addons::read_status(&self.cfg.addon_status));
        self.cfg.menu.last_check = addons::read_last_check(&self.cfg.addon_last_check);
        if !changed.is_empty() {
            self.cfg.menu.refresh_programs(&self.cfg.program_path);
            if let Some(live) = self.live.as_mut() {
                for i in changed {
                    // A tile that is not drawn needs no icon.
                    let icon = if self.cfg.menu.visible(i) {
                        ui::load_icon(
                            &self.ctx,
                            &self.cfg.menu.tiles[i],
                            &self.cfg.icon_roots,
                            live.icon_ppp(),
                        )
                    } else {
                        None
                    };
                    if let Some(slot) = live.icons.get_mut(i) {
                        *slot = icon;
                    }
                }
            }
        }
        let mut lines = Vec::new();
        for _ in 0..ADDON_LOG_POLLS {
            let got = self.addon_log.poll();
            let more = got.len() >= addons::POLL_LINES;
            lines.extend(got);
            if !more {
                break;
            }
        }
        // The first read finds what was written before the menu started:
        // the last 200 lines are enough.
        if !self.addon_log_read {
            self.addon_log_read = true;
            let skip = lines.len().saturating_sub(addons::POLL_LINES);
            lines.drain(..skip);
        }
        for (time, text) in lines {
            self.cfg.menu.log.push(addons::log_line(time, &text));
        }
    }

    /// Read the release status again: a check by the timer, or one the
    /// Upgrade screen asked for, rewrote it.
    fn refresh_release(&mut self) {
        let r = release::read_status(&self.cfg.upgrade_status);
        self.cfg.menu.upgrade.release_file(r);
    }

    fn start(&self, job: Job) {
        // Read here, on the UI thread: the Network tab holds the one answer.
        let netboot = self.cfg.menu.network.netboot();
        let proxy = self.proxy.clone();
        let feed = self.feed.clone();
        let paths = Paths {
            save_home: self.cfg.paths.save_home.clone(),
            state_dir: self.cfg.paths.state_dir.clone(),
            addon_cmd: self.cfg.paths.addon_cmd.clone(),
            upgrade_cmd: self.cfg.paths.upgrade_cmd.clone(),
            network_cmd: self.cfg.paths.network_cmd.clone(),
            homes: self.cfg.paths.homes.clone(),
            kanshi: self.cfg.paths.kanshi.clone(),
            config: self.cfg.paths.config.clone(),
            layout: self.cfg.paths.layout,
            labwc: self.cfg.paths.labwc.clone(),
        };
        std::thread::spawn(move || {
            // The loop is gone only when the menu is exiting; nothing to tell.
            run_job(job, &paths, &feed, netboot, move |r| {
                let _ = proxy.send_event(Wake::Job(r));
            });
        });
    }

    fn apply(&mut self, effect: Option<Effect>, event_loop: &ActiveEventLoop) {
        match effect {
            Some(Effect::Start(job)) => self.start(job),
            Some(Effect::Signal(pgids, signal)) => {
                for pgid in pgids {
                    jobs::stop(pgid, signal);
                }
            }
            Some(Effect::Exit(code)) => {
                self.exit = Some(code);
                event_loop.exit();
            }
            Some(Effect::WriteScreen { idle }) => {
                let next = apply_screen(idle, &self.cfg.screen, &self.cfg.paths.config, &mut self.cfg.menu);
                self.apply(next, event_loop);
            }
            None => {}
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // The window moved to another monitor: lay out again for it.
        // Also a mode change on the same output, which need not resize the window.
        let moved = self.live.as_ref().is_some_and(|l| {
            let monitor = l.window.current_monitor();
            let name = monitor.as_ref().and_then(|m| m.name());
            let px = monitor.as_ref().map(|m| (m.size().width, m.size().height));
            monitor_moved(&l.outputs, l.monitor.as_deref(), name.as_deref(), px)
        });
        if moved {
            self.apply_scale(false);
        }
        let Some(live) = self.live.as_mut() else { return };
        let covered = is_covered(live.window.current_monitor().is_some(), live.monitor.is_some());
        if self.shown.swap(!covered, Ordering::Relaxed) == covered && self.cfg.debug {
            eprintln!("thinfloe-menu: {}", if covered { "covered" } else { "uncovered" });
        }
        let input = live.egui.take_egui_input(&live.window);
        let now = Instant::now();
        let mut event = None;
        // One reading of the wall clock and one look at the zone per frame,
        // for the header, the clock element and the network total's label.
        let wall = std::time::SystemTime::now();
        let zone = crate::log::current_zone(std::path::Path::new("/etc/localtime"));
        live.chrome.wall = Some(wall);
        live.chrome.clock = crate::header::clock_text_in(wall, &zone);
        live.chrome.covered = covered;
        self.cfg.menu.set_clock(wall, zone);
        // The sampler's latest copy, taken under its lock for as long as a
        // copy of 600 numbers takes.
        let (network, hardware) = self.samplers.copies();
        if let Some(snap) = network {
            self.cfg.menu.set_network(snap, now, wall);
        }
        if let Some(snap) = hardware {
            self.cfg.menu.set_hardware(snap);
        }
        self.covered = covered;
        self.height = live.window.inner_size().height as f32 / self.ctx.pixels_per_point();
        // `sync_samplers`, with the fields apart: `live` is borrowed.
        self.samplers.sync(&self.cfg.menu.status, self.covered, self.height);
        // egui takes a new zoom factor at the start of the next pass and
        // lays that pass out with a guessed screen rect.
        let settling = settling(self.ctx.zoom_factor(), live.scale);
        // A dialog freezes the frame under it, dimmed. A new canvas (the
        // window was replaced to move the menu, or resized by a new
        // monitor mode while *Keep this arrangement?* is up) holds no frame:
        // paint the screen without the dialog first, so the freeze below
        // has it. No input reaches that pass.
        // While the scale settles, egui lays the pass out with a guessed
        // rect: neither that backdrop nor a freeze of it may stay up.
        if needs_backdrop(self.cfg.menu.dialog(), self.frozen, live.painted, settling) {
            let mut under = input.clone();
            under.events.clear();
            let out = self.ctx.run_ui(under, |ui| {
                ui::show_screen(ui, &self.cfg.menu, &live.icons, &live.chrome, now);
            });
            live.textures.apply_set(&out.textures_delta);
            let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
            let all = live.canvas.bounds();
            live.canvas.restore_background(all);
            live.canvas.draw(&prims, &live.textures, out.pixels_per_point, all);
            live.textures.apply_free(&out.textures_delta);
            live.painted = true;
        }
        let out = self.ctx.run_ui(input, |ui| {
            event = ui::show(ui, &self.cfg.menu, &live.icons, &live.chrome, now);
        });
        live.egui.handle_platform_output(&live.window, out.platform_output);

        // The dialog dims the grid once, when it opens, and un-freezes it
        // when it closes. Either way every pixel must be redrawn once. Not
        // while the scale settles: that pass does not reach the screen, and
        // the next one freezes a frame laid out at the right scale.
        let saving = self.cfg.menu.dialog();
        let mut force_full = false;
        if freeze_changes(saving, self.frozen, settling) {
            if saving {
                live.canvas.freeze_dimmed(DIM_KEEP);
            } else {
                let bg = crate::brand::theme().palette().background;
                live.canvas.background = Background::Solid(bg);
            }
            self.frozen = saving;
            live.tracker.reset();
            force_full = true;
        }

        live.textures.apply_set(&out.textures_delta);
        if settling {
            // Nothing of this pass reaches the screen; the textures were
            // taken above, so the font atlas upload is not lost.
            live.textures.apply_free(&out.textures_delta);
            live.window.request_redraw();
            return;
        }
        let ppp = out.pixels_per_point;
        // Fingerprint bounds are in pixels, so a scale change invalidates
        // every one of them.
        if (ppp - live.ppp).abs() > f32::EPSILON {
            live.ppp = ppp;
            live.tracker.reset();
            force_full = true;
        }
        let prims = self.ctx.tessellate(out.shapes, ppp);
        let tracked = live.tracker.damage(&prims, ppp);
        let uploaded = !out.textures_delta.set.is_empty();
        if let Some(rect) = crate::damage::frame_damage(force_full, uploaded, tracked, live.canvas.bounds()) {
            let t0 = Instant::now();
            live.canvas.restore_background(rect);
            live.canvas.draw(&prims, &live.textures, ppp, rect);
            if rect == live.canvas.bounds() {
                live.painted = true;
            }
            match present(&mut live.surface, &live.canvas) {
                Ok(()) => live.presented = true,
                Err(e) => eprintln!("thinfloe-menu: present failed: {e}"),
            }
            if self.cfg.debug {
                eprintln!("thinfloe-menu: raster {:?} in {:?}", rect, t0.elapsed());
            }
        }
        live.textures.apply_free(&out.textures_delta);

        // Honour egui's own repaint requests. Ignoring them leaves the first
        // screen in fallback fonts until the mouse moves, because the font
        // atlas lands one frame late.
        let delay = out.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay);
        // The retry question freezes what is on screen behind it, so it
        // waits for a grid that is presented and settled (no repaint asked
        // for at once: the font atlas lands one frame late).
        let settled = !delay.is_some_and(|d| d.is_zero());
        if self.cfg.menu.retry_held() && live.presented && settled {
            self.cfg.menu.hold_retry(false);
            live.window.request_redraw();
        }
        // A wake-up redraws, and the redraw looks for the monitor again.
        let recheck = monitor_recheck(live.monitor.is_some(), live.outputs.len(), live.started.elapsed());
        match delay {
            Some(d) if d.is_zero() => live.window.request_redraw(),
            d => {
                let egui_wake = d.filter(|d| *d < Duration::from_secs(3600));
                // The drive list is polled from `tick`, which runs only in a frame.
                // This is chosen before the frame's event below is handled; an
                // event that moves the screen to or from Upgrade still gets its
                // wake, because every handled event requests a redraw, and that
                // frame chooses again.
                let menu = &self.cfg.menu;
                let shown = self.shown.load(Ordering::Relaxed);
                // The countdown of *Keep this arrangement?* runs, shown or not.
                let poll = upgrade_wake(menu.screen, &menu.upgrade.stage, shown)
                    .into_iter()
                    .chain(network_wake(menu.screen, menu.settings_tab, shown))
                    .chain(menu.monitors.next_wake(now))
                    .min();
                let at = Instant::now();
                live.frame_due = egui_wake.into_iter().chain(recheck).chain(poll).min().map(|d| at + d);
                event_loop.set_control_flow(wake_flow(next_wake(live.frame_due, covered, at)));
            }
        }

        let effect = match event.clone() {
            Some(ui::UiEvent::Tile(i)) => self.cfg.menu.activate(i, now),
            Some(ui::UiEvent::Chose(c)) => self.cfg.menu.choose(c, now),
            Some(ui::UiEvent::Settings(e)) => self.cfg.menu.settings(e),
            Some(ui::UiEvent::Retry(r)) => self.cfg.menu.answer_retry(r),
            Some(ui::UiEvent::Upgrade(e)) => self.cfg.menu.upgrade_event(e, now),
            Some(ui::UiEvent::Unfold(id)) => {
                if let Some(Err(e)) = self.windows.as_ref().map(|w| w.restore(id)) {
                    self.cfg
                        .menu
                        .log
                        .note(crate::log::Kind::Warn, format!("could not unfold the window: {e}"));
                }
                None
            }
            None => self.cfg.menu.tick(now),
        };
        if effect.is_some() || event.is_some() {
            self.apply(effect, event_loop);
            if let Some(live) = self.live.as_ref() {
                live.window.request_redraw();
            }
        }
    }
}

/// Whether the screen under a dialog must be painted before it is frozen:
/// the dialog is up, nothing is frozen yet, the canvas holds no frame, and
/// the scale has settled (else the pass is laid out with a guessed rect).
fn needs_backdrop(dialog: bool, frozen: bool, painted: bool, settling: bool) -> bool {
    dialog && !frozen && !painted && !settling
}

/// Whether the canvas freezes (or thaws) in this pass: the dialog opened
/// (or closed), and the scale has settled.
fn freeze_changes(dialog: bool, frozen: bool, settling: bool) -> bool {
    dialog != frozen && !settling
}

/// The outputs the compositor reports; none when it cannot be asked.
fn read_outputs(debug: bool) -> Vec<OutputInfo> {
    match crate::outputs::read() {
        Ok(o) => o,
        Err(e) => {
            if debug {
                eprintln!("thinfloe-menu: no output sizes: {e}");
            }
            Vec::new()
        }
    }
}

fn present(surface: &mut softbuffer::Surface<Rc<Window>, Rc<Window>>, canvas: &Canvas) -> Result<(), String> {
    let mut buf = surface.buffer_mut().map_err(|e| e.to_string())?;
    if buf.len() != canvas.pixels.len() {
        return Err(format!(
            "buffer is {} pixels, canvas {}",
            buf.len(),
            canvas.pixels.len()
        ));
    }
    buf.copy_from_slice(&canvas.pixels);
    buf.present().map_err(|e| e.to_string())
}

impl App {
    /// A new window with its surface and input state.
    fn create_live(&self, event_loop: &ActiveEventLoop) -> Result<Live, String> {
        // The app_id labwc's window rule in /etc/thinfloe/labwc/rc.xml matches
        // to keep the menu at the bottom and out of the window list.
        let attrs = Window::default_attributes()
            .with_title("thinfloe-menu")
            .with_name("thinfloe-menu", "")
            .with_decorations(false)
            .with_maximized(true);
        let window = event_loop
            .create_window(attrs)
            .map(Rc::new)
            .map_err(|e| format!("cannot create a window: {e}"))?;
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|c| softbuffer::Surface::new(&c, window.clone()))
            .map_err(|e| format!("cannot create a drawing surface: {e}"))?;
        let egui = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let chrome = ui::Chrome {
            header: self.cfg.header.clone(),
            clock: String::new(),
            logo: None,
            covered: false,
            wall: None,
        };
        let bg = crate::brand::theme().palette().background;
        Ok(Live {
            window,
            surface,
            egui,
            canvas: Canvas::new(1, 1, bg),
            textures: Textures::default(),
            tracker: Tracker::default(),
            icons: Vec::new(),
            chrome,
            ppp: 0.0,
            scale: 1.0,
            monitor: None,
            started: Instant::now(),
            outputs: read_outputs(self.cfg.debug),
            presented: false,
            painted: false,
            frame_due: None,
        })
    }

    /// Replace the window by a new one, which labwc places by its window
    /// rules: the way the menu moves to another monitor. The old window
    /// goes only once the new one exists, so a failure keeps the menu on
    /// screen. Everything but the window lives in `App` and stays: the
    /// menu, its log, running jobs, the countdown of *Keep this
    /// arrangement?*, the Settings tab.
    fn replace_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_none() {
            return;
        }
        let mut live = match self.create_live(event_loop) {
            Ok(live) => live,
            Err(e) => {
                eprintln!("thinfloe-menu: the window stays where it is: {e}");
                return;
            }
        };
        // egui sends each texture once: the new window takes over the
        // rasteriser's copies.
        if let Some(old) = self.live.as_mut() {
            live.textures = std::mem::take(&mut old.textures);
        }
        // The old window goes here, after the new one exists.
        self.live = Some(live);
        if self.cfg.debug {
            eprintln!("thinfloe-menu: window replaced");
        }
        // A new canvas: the dialog's dimmed grid is frozen again from it.
        self.frozen = false;
        self.apply_scale(true);
        if let Some(live) = self.live.as_ref() {
            live.window.request_redraw();
        }
    }

    /// Ask labwc-scale for the monitor the menu belongs on when that
    /// changed, and replace the window once it confirms, or after
    /// `menu_move::CONFIRM_WAIT` without an answer.
    ///
    /// The file is a few bytes in `XDG_RUNTIME_DIR`, a tmpfs, written like
    /// the scale file beside it: on the UI thread, a temp file and a rename.
    fn sync_menu_output(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_none() {
            return;
        }
        let Some(path) = self.output_file.clone() else { return };
        let now = Instant::now();
        let wanted = match &self.cfg.debug_move {
            Some(name) => Some(name.clone()),
            None => self.cfg.menu.monitors.menu_output().map(str::to_string),
        };
        let changes = self.menu_move.changes(wanted.as_deref());
        if !changes && !self.menu_move.waiting() {
            return;
        }
        let applied = menu_move::read_applied(&path);
        if self.menu_move.want(wanted.as_deref(), applied.as_ref(), now) && self.cfg.debug {
            eprintln!("thinfloe-menu: menu output {}", wanted.as_deref().unwrap_or("(none)"));
        }
        if self.menu_move.write_due(now) {
            let r = menu_move::write_output_file(&path, self.menu_move.asked());
            if self.menu_move.wrote(r.is_ok(), now)
                && let Err(e) = r
            {
                eprintln!("thinfloe-menu: cannot write the menu output file: {e}");
            }
        }
        match self.menu_move.poll(applied.as_ref(), now) {
            Some(Replace::Confirmed) => self.replace_window(event_loop),
            Some(Replace::Unconfirmed(asked)) => {
                if let Some(name) = asked {
                    self.cfg.menu.log.note(
                        crate::log::Kind::Warn,
                        format!("The menu could not be moved to {name}."),
                    );
                }
                self.replace_window(event_loop);
            }
            None => {}
        }
    }

    /// Logs, under TF_MENU_DEBUG, the windows folded or unfolded since the
    /// last call.
    fn windows_changed(&mut self) {
        let Some(w) = self.windows.as_ref() else { return };
        let now = w.folded();
        self.cfg.menu.folded = now.clone();
        if let Some(live) = self.live.as_ref() {
            live.window.request_redraw();
        }
        if self.cfg.debug {
            for f in now.iter().filter(|f| !self.folded_seen.contains(f)) {
                eprintln!("thinfloe-menu: folded: {} {}", f.app_id, f.title);
            }
            for f in self.folded_seen.iter().filter(|f| !now.contains(f)) {
                eprintln!("thinfloe-menu: unfolded: {} {}", f.app_id, f.title);
            }
        }
        self.folded_seen = now;
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        match self.create_live(event_loop) {
            Ok(live) => self.live = Some(live),
            Err(e) => {
                eprintln!("thinfloe-menu: {e}");
                self.exit = Some(1);
                event_loop.exit();
                return;
            }
        }
        // Icons and logo are drawn at the scale, so it comes first. Forced,
        // so the scale file exists even at 1.
        self.apply_scale(true);
        if let Some(live) = self.live.as_ref() {
            if self.cfg.debug {
                // Hidden add-on tiles too: the Settings screen shows their icon
                // when it renders.
                let shown = self.cfg.menu.visible_tiles();
                let drawn = shown.iter().filter(|&&i| live.icons[i].is_some()).count();
                eprintln!("thinfloe-menu: {drawn} of {} tile icons drawn", shown.len());
            }
            live.window.request_redraw();
        }
    }

    // After every batch of events: whatever changed the monitor the menu
    // belongs on (the monitors, a loaded file, an Apply, its revert) has
    // been handled by now.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.sync_menu_output(event_loop);
        let now = Instant::now();
        if let Some(d) = self.menu_move.next_wake(now) {
            let at = now + d;
            match event_loop.control_flow() {
                ControlFlow::WaitUntil(t) if t <= at => {}
                ControlFlow::Poll => {}
                _ => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            }
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if !wake_needs_frame(&cause) {
            return;
        }
        let Some(live) = self.live.as_ref() else { return };
        let covered = is_covered(live.window.current_monitor().is_some(), live.monitor.is_some());
        let now = Instant::now();
        if frame_wanted(covered, live.frame_due, now) {
            live.window.request_redraw();
        } else {
            // No frame follows to set the next wake, and a deadline already
            // past would spin the loop.
            event_loop.set_control_flow(wake_flow(next_wake(live.frame_due, covered, now)));
        }
    }

    // A finished job and program output arrive here through the proxy,
    // which wakes the loop. The output is taken first, so a program's last
    // lines are logged before its exit.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, wake: Wake) {
        for line in self.feed.drain() {
            self.cfg.menu.log.push(line);
        }
        match wake {
            Wake::Job(result) => {
                // The status changed with the job; the watcher would see it
                // only a second later.
                let addon = matches!(result, JobResult::Addon { .. });
                let effect = self.cfg.menu.finished(result, Instant::now());
                self.apply(effect, event_loop);
                if addon {
                    self.refresh_addons();
                }
            }
            Wake::Addons => self.refresh_addons(),
            Wake::Release => self.refresh_release(),
            Wake::Battery => self.refresh_battery(),
            Wake::Status => self.refresh_status(),
            Wake::Windows => self.windows_changed(),
            Wake::Heads => {
                if let Some(heads) = self.heads.as_ref() {
                    let effect = self.cfg.menu.heads_changed(heads.snapshot());
                    self.apply(effect, event_loop);
                }
            }
            Wake::Output | Wake::Network | Wake::Hardware => {}
        }
        if let Some(live) = self.live.as_ref() {
            live.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // An event still queued for a window the menu replaced is dropped.
        if self.live.as_ref().is_none_or(|l| l.window.id() != id) {
            return;
        }
        match &event {
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            WindowEvent::Resized(size) => {
                if self.cfg.debug {
                    eprintln!("thinfloe-menu: configure {}x{}", size.width, size.height);
                }
                if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    && let Some(live) = self.live.as_mut()
                {
                    if let Err(e) = live.surface.resize(w, h) {
                        eprintln!("thinfloe-menu: resize failed: {e}");
                    }
                    let bg = crate::brand::theme().palette().background;
                    live.canvas = Canvas::new(size.width as usize, size.height as usize, bg);
                    live.painted = false;
                    self.frozen = false;
                    live.tracker.reset();
                }
                // The fit to the window width depends on the width.
                self.apply_scale(false);
            }
            // labwc draws no close button and has no keybindings, so this
            // only comes from a client asking. Ending here would be exit 0,
            // which thinfloe-session reads as logout.
            WindowEvent::CloseRequested => return,
            _ => {}
        }
        // While the dialog is up, input is refused rather than handled.
        if self.cfg.menu.refuses_input() && is_input(&event) {
            return;
        }
        let Some(live) = self.live.as_mut() else { return };
        let resp = live.egui.on_window_event(&live.window, &event);
        if resp.repaint {
            live.window.request_redraw();
        }
    }
}

/// Whether a loop wake-up needs a frame. `ControlFlow::WaitUntil` only
/// ends the wait; winit then reports `ResumeTimeReached` and does nothing
/// else, so without a redraw here the save timer and the pause before
/// logout would never advance.
fn wake_needs_frame(cause: &StartCause) -> bool {
    matches!(cause, StartCause::ResumeTimeReached { .. })
}

fn is_input(e: &WindowEvent) -> bool {
    matches!(
        e,
        WindowEvent::KeyboardInput { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::Touch(_)
    )
}

/// Carry out `WriteScreen`: keep `idle` in `config.toml` (which renders
/// `generated/screen`), tell the idle helper, and report to the menu at once,
/// as `Menu::screen_written` expects. Returns what the menu asks next. A
/// store that refuses the update is the error shown in the tab.
fn apply_screen(idle: u32, screen: &ScreenFile, store: &crate::store::Shared, menu: &mut Menu) -> Option<Effect> {
    let result = {
        let mut store = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        store
            .update(crate::generated::Table::Screen, |c| c.screen.idle_timeout = idle)
            .map(|_| crate::screentab::from_config(&store.cfg.screen))
    };
    match result {
        Ok(tab) => {
            let signalled = screen.idle_pid.is_some_and(signal_idle_helper);
            menu.screen_written(Ok(tab), signalled)
        }
        Err(message) => menu.screen_written(Err(message), false),
    }
}

/// Tell the idle helper to read the screen file again.
fn signal_idle_helper(pid: i32) -> bool {
    // Never init, never a process group.
    if pid <= 1 {
        return false;
    }
    // SAFETY: kill takes no pointers; a stale pid fails with ESRCH.
    unsafe { libc::kill(pid, libc::SIGUSR1) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use crate::upgrade_proto::Msg;
    use std::path::Path;
    use std::sync::mpsc;

    #[test]
    fn a_dialog_on_a_new_canvas_paints_its_backdrop_first() {
        // A replaced or resized window while the dialog is up.
        assert!(needs_backdrop(true, false, false, false));
        // The usual case: the frame under the dialog is on the canvas.
        assert!(!needs_backdrop(true, false, true, false));
        // Frozen already, or no dialog: nothing to paint.
        assert!(!needs_backdrop(true, true, false, false));
        assert!(!needs_backdrop(false, false, false, false));
        // A new scale still settles: wait for the pass laid out at it.
        assert!(!needs_backdrop(true, false, false, true));
    }

    #[test]
    fn the_freeze_waits_for_a_settled_scale() {
        assert!(freeze_changes(true, false, false), "a dialog opens");
        assert!(freeze_changes(false, true, false), "a dialog closes");
        assert!(!freeze_changes(true, true, false));
        assert!(!freeze_changes(false, false, false));
        // Settling: the guessed layout is never frozen; the next pass is.
        assert!(!freeze_changes(true, false, true));
        assert!(!freeze_changes(false, true, true));
    }

    #[test]
    fn a_keep_job_that_panics_does_not_report_success() {
        assert!(matches!(
            panic_result(&Job::MonKeep { profiles: Vec::new() }),
            JobResult::MonKept(Err(_))
        ));
        assert!(matches!(panic_result(&Job::MonRevert), JobResult::MonReverted(Err(_))));
    }

    fn paths(upgrade_cmd: Vec<String>) -> Paths {
        Paths {
            save_home: PathBuf::from("/nonexistent/save-home"),
            state_dir: PathBuf::from("/nonexistent/state"),
            addon_cmd: vec!["/nonexistent/addon".into()],
            upgrade_cmd,
            network_cmd: jobs::default_network_cmd(),
            homes: crate::netfiles::Homes::from_env(),
            kanshi: jobs::Kanshi::from_env(),
            config: crate::store::test_shared(Path::new("/nonexistent/cfg")),
            layout: crate::migrate::HomeLayout::New,
            labwc: jobs::LabwcCmds::from_env(),
        }
    }

    #[test]
    fn a_write_delivers_its_lines_between_start_and_end() {
        let cmd = ["sh", "-c", r#"echo "@step write W"; echo "@result ok""#, "sh"]
            .map(String::from)
            .to_vec();
        let (tx, rx) = mpsc::channel();
        let feed = Arc::new(Feed::default());
        let job = Job::UpgradeWrite { args: Vec::new() };
        run_job(job, &paths(cmd), &feed, false, move |r| {
            let _ = tx.send(r);
        });
        let mut got = Vec::new();
        while let Ok(r) = rx.recv_timeout(Duration::from_secs(10)) {
            let end = matches!(r, JobResult::UpgradeEnded { .. });
            got.push(r);
            if end {
                break;
            }
        }
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(matches!(got[0], JobResult::UpgradeStarted { .. }), "{got:?}");
        assert_eq!(
            got[1],
            JobResult::UpgradeMsg(Msg::Step {
                id: "write".into(),
                text: "W".into()
            })
        );
        assert_eq!(
            got[2],
            JobResult::UpgradeMsg(Msg::Result {
                ok: true,
                reason: String::new()
            })
        );
        assert_eq!(got[3], JobResult::UpgradeEnded { status: Ok(Some(0)) });
    }

    #[test]
    fn net_jobs_report_their_results() {
        let d = TestDir::new("app", "net-jobs");
        let tool = r#"case "$1" in
status) echo '{"interfaces":[{"name":"eno1","kind":"wired","state":"no-cable"}]}';;
scan) echo '{"device":"w","networks":[{"ssid":"a","signal_dbm":-40,"secured":true,"supported":true}]}';;
apply) echo '{"result":"ok","reason":""}';;
esac"#;
        let home = d.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(d.join("sys/wlan0/wireless")).unwrap();
        let default_wired = d.join("default-ethernet.yaml");
        std::fs::write(
            &default_wired,
            "network:\n  version: 2\n  ethernets:\n    eth:\n      match:\n        name: 'en*'\n      dhcp4: true\n",
        )
        .unwrap();
        let mut p = paths(Vec::new());
        p.network_cmd = ["/bin/sh", "-c", tool, "sh"].map(String::from).to_vec();
        p.config = std::sync::Arc::new(std::sync::Mutex::new(
            crate::store::open(
                crate::config::ConfigDir { root: d.join("cfg") },
                crate::generated::Inputs {
                    default_wired: include_str!("../../../overlay/etc/netplan/ethernet.yaml").into(),
                    netboot: false,
                },
                crate::store::OpenMode::Session,
                0,
            )
            .store,
        ));
        p.homes = netfiles::Homes {
            home: home.clone(),
            sys_net: d.join("sys"),
            netplan: "/bin/true".into(),
            default_wired,
        };
        let run = |job: Job| {
            let (tx, rx) = mpsc::channel();
            run_job(job, &p, &Arc::new(Feed::default()), false, move |r| {
                let _ = tx.send(r);
            });
            rx.try_iter().collect::<Vec<_>>()
        };
        let got = run(Job::NetLoad);
        assert!(
            matches!(&got[..], [JobResult::NetLoaded(_, _, devs)] if devs == &["wlan0".to_string()]),
            "{got:?}"
        );
        let got = run(Job::NetStatus);
        assert!(
            matches!(&got[..], [JobResult::NetStatus(Ok(v))] if v.len() == 1),
            "{got:?}"
        );
        let got = run(Job::NetScan);
        assert!(
            matches!(&got[..], [JobResult::NetScan(Ok(v))] if v.len() == 1),
            "{got:?}"
        );
        let change = netfiles::Change {
            wifi: Some(vec![netfiles::Net {
                ssid: "Home".into(),
                hidden: false,
                password: None,
            }]),
            wired: None,
        };
        let got = run(Job::NetApply { change });
        assert!(
            matches!(
                &got[..],
                [JobResult::NetApplyStarted { .. }, JobResult::NetApplied(Ok(()))]
            ),
            "{got:?}"
        );
        let text = std::fs::read_to_string(d.join("cfg/generated/wifi.yaml")).unwrap();
        assert!(text.starts_with(crate::generated::HEADER), "{text}");
        let got = run(Job::NetTakeOver { which: "wifi" });
        assert!(matches!(&got[..], [JobResult::NetTakenOver(_)]), "{got:?}");
    }

    #[test]
    fn dismiss_report_clears_the_notice() {
        let d = TestDir::new("app", "dismiss-report");
        let mut p = paths(Vec::new());
        p.config = crate::store::test_shared(&d.join("cfg"));
        p.config
            .lock()
            .unwrap()
            .update(crate::generated::Table::Notice, |c| {
                c.notice.migration = Some("3 files were moved".into());
            })
            .unwrap();
        assert!(
            std::fs::read_to_string(d.join("cfg/config.toml"))
                .unwrap()
                .contains("migration")
        );
        let (tx, rx) = mpsc::channel();
        run_job(Job::DismissReport, &p, &Arc::new(Feed::default()), false, move |r| {
            let _ = tx.send(r);
        });
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(got, [JobResult::ReportDismissed(Ok(()))]);
        let text = std::fs::read_to_string(d.join("cfg/config.toml")).unwrap();
        assert!(!text.contains("migration"), "{text}");
        assert_eq!(p.config.lock().unwrap().cfg.notice.migration, None);
    }

    #[test]
    fn a_worker_that_panics_still_sends_a_result() {
        // An empty command line panics in the job; the screen must still
        // hear back, or its poll and its check stay busy for ever.
        for job in [
            Job::Drives { all: true },
            Job::UpgradeCheck { source: None },
            Job::UpgradeWrite { args: Vec::new() },
        ] {
            let (tx, rx) = mpsc::channel();
            let feed = Arc::new(Feed::default());
            run_job(job.clone(), &paths(Vec::new()), &feed, false, move |r| {
                let _ = tx.send(r);
            });
            let r = rx.recv_timeout(Duration::from_secs(10)).expect("a result");
            let ok = matches!(
                (&job, &r),
                (
                    Job::Drives { .. },
                    JobResult::Drives {
                        all: true,
                        result: Err(_)
                    }
                ) | (
                    Job::UpgradeCheck { .. },
                    JobResult::Release {
                        custom: false,
                        result: Err(_)
                    }
                ) | (Job::UpgradeWrite { .. }, JobResult::UpgradeEnded { status: Err(_) })
            );
            assert!(ok, "{job:?} gave {r:?}");
        }
    }

    #[test]
    fn a_tool_that_cannot_start_still_ends_the_write() {
        let (tx, rx) = mpsc::channel();
        let feed = Arc::new(Feed::default());
        let job = Job::UpgradeWrite { args: Vec::new() };
        run_job(
            job,
            &paths(vec!["/nonexistent/thinfloe-upgrade".into()]),
            &feed,
            false,
            move |r| {
                let _ = tx.send(r);
            },
        );
        let r = rx.recv_timeout(Duration::from_secs(10)).expect("a result");
        assert!(matches!(r, JobResult::UpgradeEnded { status: Err(_) }), "{r:?}");
    }

    #[test]
    fn a_release_status_change_wakes_the_loop() {
        let d = TestDir::new("app", "release-wake");
        let release = d.join("upgrade-status");
        let mut w = Watch {
            addon_status: d.join("addon-status"),
            addon_log: d.join("addon-log"),
            upgrade_status: release.clone(),
            battery_file: d.join("battery"),
            config_toml: d.join("config.toml"),
            addons: addons::Watcher::default(),
            release: release::Watcher::default(),
            battery: battery::Watcher::default(),
            status: battery::Watcher::default(),
        };
        let is_release = |v: &[Wake]| v.iter().filter(|w| matches!(w, Wake::Release)).count();
        // The first look reads both, whatever the files hold.
        let first = w.tick();
        assert_eq!(is_release(&first), 1, "{first:?}");
        assert!(first.iter().any(|w| matches!(w, Wake::Addons)), "{first:?}");
        assert!(w.tick().is_empty());
        std::fs::write(&release, "latest=4.1.0\n").unwrap();
        let next = w.tick();
        assert_eq!(is_release(&next), 1, "{next:?}");
        assert!(!next.iter().any(|w| matches!(w, Wake::Addons)), "{next:?}");
        assert!(w.tick().is_empty());
    }

    #[test]
    fn the_drive_poll_wakes_the_loop_only_while_choosing_on_screen() {
        assert_eq!(upgrade_wake(Screen::Upgrade, &Stage::Choose, true), Some(upgrade::POLL));
        assert_eq!(upgrade_wake(Screen::Upgrade, &Stage::Choose, false), None);
        assert_eq!(upgrade_wake(Screen::Upgrade, &Stage::Running, true), None);
        assert_eq!(upgrade_wake(Screen::Grid, &Stage::Choose, true), None);
        assert_eq!(upgrade_wake(Screen::Settings, &Stage::Choose, true), None);
        assert_eq!(
            network_wake(Screen::Settings, SettingsTab::Network, true),
            Some(network::POLL)
        );
        assert_eq!(network_wake(Screen::Settings, SettingsTab::Network, false), None);
        assert_eq!(network_wake(Screen::Settings, SettingsTab::Packages, true), None);
        assert_eq!(network_wake(Screen::Grid, SettingsTab::Network, true), None);
    }

    fn area(text: &[u8]) -> crate::status::Area {
        let mut layout = crate::status::layout::parse(text, &crate::status::offered);
        crate::status::layout::complete(&mut layout, &crate::status::descriptors());
        crate::status::Area { layout }
    }

    #[test]
    fn samplers_start_once_and_rest_when_unticked() {
        use std::cell::Cell;
        let (nets, hws) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let (n, h) = (nets.clone(), hws.clone());
        let mut s = Samplers::new(
            Box::new(move |_| {
                n.set(n.get() + 1);
                Arc::default()
            }),
            Box::new(move |_| {
                h.set(h.get() + 1);
                Arc::default()
            }),
        );
        let on = |s: &Samplers| {
            (
                s.network_on.load(Ordering::Relaxed),
                s.hardware_on.load(Ordering::Relaxed),
            )
        };
        let tall = ui::SHORT_SCREEN_H;
        let none = area(
            b"-battery 1x1\n-network 2x2\n-network-info 1x2\n-temperatures 1x1\n\
              -cpu 1x1\n-memory 1x1\n-system 1x1\n-clock 1x1\n",
        );
        s.sync(&none, false, tall);
        assert_eq!((nets.get(), hws.get()), (0, 0), "nothing ticked: nothing starts");
        assert_eq!(on(&s), (false, false));
        let shipped = crate::status::Area::shipped();
        s.sync(&shipped, false, tall);
        assert_eq!((nets.get(), hws.get()), (1, 1));
        assert_eq!(on(&s), (true, true));
        // Unticked: both rest, and stay started.
        s.sync(&none, false, tall);
        assert_eq!(on(&s), (false, false));
        // Ticked again: the same threads read again, none is started anew.
        s.sync(&shipped, false, tall);
        assert_eq!((nets.get(), hws.get()), (1, 1));
        assert_eq!(on(&s), (true, true));
        // Covered: both rest.
        s.sync(&shipped, true, tall);
        assert_eq!(on(&s), (false, false));
        // A short screen places no hardware element: the network reads, the
        // hardware rests.
        s.sync(&shipped, false, tall - 1.0);
        assert_eq!(on(&s), (true, false));
        assert_eq!((nets.get(), hws.get()), (1, 1));
    }

    #[test]
    fn a_sampler_gives_no_copy_before_its_first_reading() {
        // Before the first reading the element has nothing yet, which the
        // Status tab must not take for "nothing to show on this machine".
        let net = Arc::new(Mutex::new(crate::netstat::Series::default()));
        let hw: Arc<Mutex<_>> = Arc::default();
        let (n, h) = (net.clone(), hw.clone());
        let mut s = Samplers::new(Box::new(move |_| n.clone()), Box::new(move |_| h.clone()));
        s.sync(&crate::status::Area::shipped(), false, ui::SHORT_SCREEN_H);
        let (network, hardware) = s.copies();
        assert!(network.is_none() && hardware.is_none(), "{network:?} {hardware:?}");
        net.lock()
            .unwrap()
            .push(Instant::now(), crate::netstat::Reading::default());
        *hw.lock().unwrap() = Some(crate::hardware::Snapshot::default());
        let (network, hardware) = s.copies();
        assert!(network.is_some(), "a reading, if empty, is a copy");
        assert_eq!(hardware, Some(crate::hardware::Snapshot::default()));
    }

    #[test]
    fn a_sampler_starts_for_a_ticked_element_that_has_no_cells() {
        use std::cell::Cell;
        let hws = Rc::new(Cell::new(0));
        let h = hws.clone();
        let mut s = Samplers::new(
            Box::new(|_| Arc::default()),
            Box::new(move |_| {
                h.set(h.get() + 1);
                Arc::default()
            }),
        );
        // Only cpu is ticked, on a short screen, where it has no size: it
        // still needs its data, but nothing is read for it.
        let cpu_only = area(
            b"-battery 1x1\n-network 2x2\n-network-info 1x2\n-temperatures 1x1\n\
              cpu 1x1\n-memory 1x1\n-system 1x1\n-clock 1x1\n",
        );
        s.sync(&cpu_only, false, ui::SHORT_SCREEN_H - 1.0);
        assert_eq!(hws.get(), 1);
        assert!(!s.hardware_on.load(Ordering::Relaxed));
    }

    fn write_status(file: &Path, lines: Option<&[&str]>) {
        let mut c = crate::config::ThinfloeConfig::default();
        c.status.lines = lines.map(|l| l.iter().map(|s| (*s).to_string()).collect());
        std::fs::write(file, crate::config::to_text(&c)).unwrap();
    }

    fn reread(menu: &mut Menu, file: &Path) {
        if let Some(l) = crate::status::read_config_layout(file) {
            menu.set_status_layout(l);
        }
    }

    #[test]
    fn a_changed_config_file_is_read_again() {
        let d = TestDir::new("app", "status-wake");
        let file = d.join("config.toml");
        let mut w = Watch {
            addon_status: d.join("addon-status"),
            addon_log: d.join("addon-log"),
            upgrade_status: d.join("upgrade-status"),
            battery_file: d.join("battery"),
            config_toml: file.clone(),
            addons: addons::Watcher::default(),
            release: release::Watcher::default(),
            battery: battery::Watcher::default(),
            status: battery::Watcher::default(),
        };
        let is_status = |v: &[Wake]| v.iter().filter(|w| matches!(w, Wake::Status)).count();
        // The first look reads it, also when there is none.
        assert_eq!(is_status(&w.tick()), 1);
        assert_eq!(is_status(&w.tick()), 0);
        let mut menu = Menu::new(
            crate::tiles::Grid {
                tiles: Vec::new(),
                banner: Vec::new(),
            },
            false,
        );
        reread(&mut menu, &file);
        assert!(
            menu.status.needs(crate::status::Source::Network),
            "no file: the default"
        );
        write_status(&file, Some(&["-network 2x2", "-network-info 1x2"]));
        assert_eq!(is_status(&w.tick()), 1);
        assert_eq!(is_status(&w.tick()), 0);
        reread(&mut menu, &file);
        assert!(!menu.status.needs(crate::status::Source::Network));
        assert!(menu.status.needs(crate::status::Source::Hardware), "the rest appended");
        // Removed: the default again.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(is_status(&w.tick()), 1);
        reread(&mut menu, &file);
        assert!(menu.status.needs(crate::status::Source::Network));
    }

    #[test]
    fn a_re_read_layout_rests_the_sampler_it_no_longer_needs() {
        let d = TestDir::new("app", "status-resync");
        let file = d.join("config.toml");
        let mut menu = Menu::new(
            crate::tiles::Grid {
                tiles: Vec::new(),
                banner: Vec::new(),
            },
            false,
        );
        let mut s = Samplers::new(Box::new(|_| Arc::default()), Box::new(|_| Arc::default()));
        let tall = ui::SHORT_SCREEN_H;
        // What `Wake::Status` runs.
        let refresh = |menu: &mut Menu, s: &mut Samplers| reload_status(menu, s, &file, false, tall);
        refresh(&mut menu, &mut s);
        assert!(s.network.is_some() && s.network_on.load(Ordering::Relaxed));
        write_status(&file, Some(&["-network 2x2", "-network-info 1x2"]));
        refresh(&mut menu, &mut s);
        assert!(!s.network_on.load(Ordering::Relaxed), "unticked: it rests");
        assert!(s.hardware_on.load(Ordering::Relaxed), "the rest still reads");
        write_status(&file, Some(&["network 2x2"]));
        refresh(&mut menu, &mut s);
        assert!(s.network_on.load(Ordering::Relaxed));
    }

    #[test]
    fn a_config_file_that_cannot_be_read_keeps_the_layout_shown() {
        let d = TestDir::new("app", "status-unreadable");
        let file = d.join("config.toml");
        let mut menu = Menu::new(
            crate::tiles::Grid {
                tiles: Vec::new(),
                banner: Vec::new(),
            },
            false,
        );
        write_status(&file, Some(&["-network 2x2", "-network-info 1x2"]));
        reread(&mut menu, &file);
        assert!(!menu.status.needs(crate::status::Source::Network));
        let mut s = Samplers::new(Box::new(|_| Arc::default()), Box::new(|_| Arc::default()));
        // Broken TOML, a directory, a huge file: none of them is the default.
        std::fs::write(&file, "this is [not toml\n").unwrap();
        reload_status(&mut menu, &mut s, &file, false, ui::SHORT_SCREEN_H);
        assert!(!menu.status.needs(crate::status::Source::Network), "parse error");
        assert_eq!(crate::status::read_config_layout(&d), None, "a directory");
        let big = d.join("big.toml");
        std::fs::write(&big, vec![b'#'; 2 << 20]).unwrap();
        assert_eq!(crate::status::read_config_layout(&big), None, "over the cap");
    }

    #[test]
    fn a_fifo_in_place_of_config_toml_is_refused_without_blocking() {
        let d = TestDir::new("app", "status-fifo");
        let fifo = d.join("config.toml");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::status::read_config_layout(&path));
        });
        let Ok(l) = rx.recv_timeout(Duration::from_secs(5)) else {
            // Unblock the reader so the test process can end.
            let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
            panic!("read_config_layout blocked on a FIFO");
        };
        assert_eq!(l, None);
    }

    #[test]
    fn the_hardware_sampler_rests_when_covered_on_a_short_screen_and_without_a_placed_element() {
        let shipped = crate::status::Area::shipped();
        let tall = ui::SHORT_SCREEN_H;
        assert!(hardware_wanted(&shipped, false, tall), "hardware elements have cells");
        assert!(!hardware_wanted(&shipped, true, tall), "covered");
        assert!(!hardware_wanted(&shipped, false, tall - 1.0), "short: no hardware size");
        let none = crate::status::Area::default();
        assert!(!hardware_wanted(&none, false, tall), "no element");
        // Hardware elements hidden: the rest of the list does not ask.
        let hidden = crate::status::layout::parse(
            b"-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n-system 1x1\n",
            &crate::status::offered,
        );
        let mut layout = hidden;
        crate::status::layout::complete(&mut layout, &crate::status::descriptors());
        let hidden = crate::status::Area { layout };
        assert!(!hardware_wanted(&hidden, false, tall), "all four hidden");
        // One shown is enough.
        let mut layout =
            crate::status::layout::parse(b"-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n", &crate::status::offered);
        crate::status::layout::complete(&mut layout, &crate::status::descriptors());
        assert!(
            hardware_wanted(&crate::status::Area { layout }, false, tall),
            "system shown"
        );
    }

    #[test]
    fn the_zoom_factor_is_the_pixels_per_point() {
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(2.25);
        let pass = |w: f32| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 2160.0))),
                ..Default::default()
            };
            let mut seen = 0.0;
            let mut out = ctx.run_ui(input, |ui| seen = ui.max_rect().width());
            out.textures_delta.clear();
            (out.pixels_per_point, seen)
        };
        // The scale is the pixels per point in the pass that follows the
        // call. The screen rect of that first pass is egui's own guess (the
        // old rect scaled), so the next pass, which egui-winit feeds with
        // the right rect, is the first that lays out at the scale; the
        // zoom's repaint request makes that pass follow at once.
        assert_eq!(pass(3840.0).0, 2.25);
        let (ppp, width) = pass(3840.0 / 2.25);
        assert_eq!(ppp, 2.25);
        assert!((width - 3840.0 / 2.25).abs() < 0.5, "{width}");
    }

    #[test]
    fn the_pass_that_takes_the_zoom_is_the_one_skipped() {
        let ctx = egui::Context::default();
        assert!(!settling(ctx.zoom_factor(), 1.0));
        ctx.set_zoom_factor(2.0);
        // Still the old zoom until a pass has run.
        assert!(settling(ctx.zoom_factor(), 2.0));
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert!(!settling(ctx.zoom_factor(), 2.0));
    }

    fn out(name: &str, px: (u32, u32), mm: (u32, u32)) -> OutputInfo {
        OutputInfo {
            name: Some(name.into()),
            mode_px: px,
            phys_mm: mm,
        }
    }

    #[test]
    fn the_scale_follows_the_output_of_the_monitor() {
        let outs = [
            out("DP-1", (1920, 1080), (527, 296)),
            out("eDP-1", (3840, 2160), (344, 193)),
        ];
        let (s, o) = pick_scale(&outs, Some("eDP-1"), Some((3840, 2160)), 3840);
        assert!(s > 2.0, "{s}");
        assert_eq!(o.unwrap().name.as_deref(), Some("eDP-1"));
        let (s, _) = pick_scale(&outs, Some("DP-1"), Some((1920, 1080)), 1920);
        assert_eq!(s, 1.0);
    }

    #[test]
    fn a_mode_change_on_the_same_output_is_noticed() {
        let outs = [out("DP-1", (1920, 1080), (527, 296))];
        assert!(!mode_changed(&outs, Some("DP-1"), Some((1920, 1080))));
        assert!(mode_changed(&outs, Some("DP-1"), Some((1280, 720))));
        // rotated: same mode, sides swapped
        assert!(!mode_changed(&outs, Some("DP-1"), Some((1080, 1920))));
        // nothing to compare
        assert!(!mode_changed(&outs, Some("HDMI-A-1"), Some((1280, 720))));
        assert!(!mode_changed(&outs, None, Some((1280, 720))));
        assert!(!mode_changed(&outs, Some("DP-1"), None));
    }

    #[test]
    fn the_redraw_check_sees_a_mode_change_and_a_move() {
        let outs = [out("DP-1", (1920, 1080), (527, 296))];
        assert!(!monitor_moved(&outs, Some("DP-1"), Some("DP-1"), Some((1920, 1080))));
        assert!(monitor_moved(&outs, Some("DP-1"), Some("DP-1"), Some((1280, 720))));
        assert!(monitor_moved(&outs, Some("DP-1"), Some("HDMI-A-1"), Some((1920, 1080))));
        // The first monitor after the start is a move too.
        assert!(monitor_moved(&outs, None, Some("DP-1"), Some((1920, 1080))));
    }

    #[test]
    fn a_covered_window_keeps_its_monitor() {
        // A window that another one covers whole is on no output, so winit
        // reports no monitor. That is not a move: the scale stays.
        let outs = [out("DP-1", (1920, 1080), (527, 296))];
        assert!(!monitor_moved(&outs, Some("DP-1"), None, None));
        assert_eq!(seen_monitor(None, Some("DP-1")).as_deref(), Some("DP-1"));
        assert_eq!(
            seen_monitor(Some("HDMI-A-1".into()), Some("DP-1")).as_deref(),
            Some("HDMI-A-1")
        );
        assert_eq!(seen_monitor(None, None), None);
    }

    #[test]
    fn an_unknown_monitor_is_looked_up_again_soon() {
        let soon = Some(MONITOR_RECHECK);
        let early = Duration::from_millis(50);
        // Two outputs and no monitor yet: the scale is a guess.
        assert_eq!(monitor_recheck(false, 2, early), soon);
        // Without outputs the monitor's own size still changes the guess.
        assert_eq!(monitor_recheck(false, 0, early), soon);
        // One output is the one the window is on; a known monitor needs nothing.
        assert_eq!(monitor_recheck(false, 1, early), None);
        assert_eq!(monitor_recheck(true, 2, early), None);
        // A window that never gets a monitor stops the lookups.
        assert_eq!(monitor_recheck(false, 2, MONITOR_RECHECK_FOR), None);
    }

    #[test]
    fn without_outputs_the_monitor_size_decides() {
        assert_eq!(pick_scale(&[], None, Some((3840, 2160)), 0).0, 2.0);
        assert_eq!(pick_scale(&[], None, None, 0).0, 1.0);
    }

    #[test]
    fn no_monitor_is_covered_once_one_was_known() {
        assert!(is_covered(false, true), "on no output any more");
        assert!(!is_covered(true, true));
        assert!(!is_covered(true, false));
        assert!(!is_covered(false, false), "not shown yet: not covered");
    }

    #[test]
    fn a_covered_menu_wakes_to_look_or_for_a_due_frame() {
        let now = Instant::now();
        let look = now + COVERED_LOOK;
        let soon = now + Duration::from_millis(100);
        let late = now + Duration::from_secs(60);
        assert_eq!(next_wake(None, false, now), None);
        assert_eq!(next_wake(Some(late), false, now), Some(late));
        assert_eq!(next_wake(None, true, now), Some(look));
        assert_eq!(next_wake(Some(late), true, now), Some(look));
        assert_eq!(next_wake(Some(soon), true, now), Some(soon));
    }

    #[test]
    fn a_covered_menu_draws_only_a_due_frame() {
        let now = Instant::now();
        let later = now + Duration::from_secs(10);
        let past = now - Duration::from_millis(1);
        // The look found it still covered: no frame, unless one is due. The
        // save timer and the pause before logout advance through those.
        assert!(!frame_wanted(true, None, now));
        assert!(!frame_wanted(true, Some(later), now));
        assert!(frame_wanted(true, Some(past), now));
        assert!(frame_wanted(true, Some(now), now));
        // Uncovered, or never covered: a frame.
        assert!(frame_wanted(false, Some(later), now));
        assert!(frame_wanted(false, None, now));
    }

    #[test]
    fn a_reached_deadline_needs_a_frame() {
        let now = Instant::now();
        let reached = StartCause::ResumeTimeReached {
            start: now,
            requested_resume: now,
        };
        assert!(wake_needs_frame(&reached));
        assert!(!wake_needs_frame(&StartCause::Poll));
        assert!(!wake_needs_frame(&StartCause::Init));
    }

    fn screen_menu() -> Menu {
        Menu::new(
            crate::tiles::Grid {
                tiles: Vec::new(),
                banner: Vec::new(),
            },
            false,
        )
    }

    fn screen_file(idle_pid: Option<i32>) -> ScreenFile {
        ScreenFile { idle_pid }
    }

    #[test]
    fn write_screen_writes_signals_and_saves() {
        use std::os::unix::process::ExitStatusExt;
        let d = TestDir::new("app", "write-screen");
        let store = crate::store::test_shared(&d);
        // sleep traps nothing, so SIGUSR1 ends it.
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let screen = screen_file(Some(pid));
        let mut menu = screen_menu();
        let next = apply_screen(120, &screen, &store, &mut menu);
        // Reap the child whatever the outcome.
        let _ = child.kill();
        let status = child.wait().unwrap();
        let text = std::fs::read_to_string(d.join("config.toml")).unwrap();
        assert_eq!(crate::config::parse(&text).unwrap().screen.idle_timeout, 120);
        let generated = std::fs::read_to_string(d.join("generated/screen")).unwrap();
        assert!(generated.ends_with("idle_timeout=120\n"), "{generated}");
        assert_eq!(status.signal(), Some(libc::SIGUSR1));
        assert_eq!(next, Some(Effect::Start(Job::Save)));
        assert_eq!(menu.screen_tab.idle, 120);
        assert_eq!(menu.screen_tab.error, None);
        assert!(menu.save.running);
        assert!(!menu.log.lines().any(|l| l.plain() == crate::menu::NEXT_SESSION));
    }

    #[test]
    fn write_screen_without_helper_says_next_session() {
        let d = TestDir::new("app", "write-screen-nopid");
        let store = crate::store::test_shared(&d);
        let screen = screen_file(None);
        let mut menu = screen_menu();
        let next = apply_screen(60, &screen, &store, &mut menu);
        assert_eq!(next, Some(Effect::Start(Job::Save)));
        assert!(menu.log.lines().any(|l| l.plain() == crate::menu::NEXT_SESSION));
    }

    #[test]
    fn write_screen_on_a_store_that_refuses_is_the_error_in_the_tab() {
        let d = TestDir::new("app", "write-screen-ro");
        let ro = crate::store::open(
            crate::config::ConfigDir { root: d.join("cfg") },
            crate::generated::Inputs::default(),
            crate::store::OpenMode::ReadOnly,
            0,
        )
        .store;
        let store: crate::store::Shared = Arc::new(std::sync::Mutex::new(ro));
        let mut menu = screen_menu();
        let next = apply_screen(60, &screen_file(None), &store, &mut menu);
        assert_eq!(next, None);
        assert_eq!(menu.screen_tab.error.as_deref(), Some("The settings are read-only."));
        assert_eq!(menu.screen_tab.idle, crate::screentab::DEFAULT_IDLE);
        assert!(!menu.save.running);
        assert!(!d.join("cfg").exists(), "nothing was written");
    }
}
