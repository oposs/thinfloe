//! The event loop: winit for the Wayland window, egui-winit for input,
//! egui for layout, raster.rs for pixels, softbuffer to put them on screen.
//! Driven by hand rather than through eframe so that nothing here needs a
//! GPU. The rules this file follows are in the spec under "Rendering
//! without a GPU".

use crate::addons::{self, AddonLog};
use crate::damage::Tracker;
use crate::density::{self, OutputInfo};
use crate::icons::IconRoots;
use crate::jobs::{self, JobResult, Paths};
use crate::log::Feed;
use crate::menu::{Effect, Job, Menu};
use crate::raster::{Background, Canvas, Textures};
use crate::ui;
use egui::ViewportId;
use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

/// How much of the grid's brightness survives behind the logout dialog.
const DIM_KEEP: u32 = 90;
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
    /// The set of folded windows changed.
    Windows,
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
    /// Where the programs of add-on tiles are looked for.
    pub program_path: OsString,
    /// Host name and version for the header.
    pub header: crate::header::Header,
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
    /// The outputs the compositor reported at the last read.
    outputs: Vec<OutputInfo>,
    /// Whether a frame reached the screen.
    presented: bool,
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
}

/// Run the menu until it exits. Returns the exit status for thinfloe-session.
pub fn run(mut cfg: Config) -> Result<i32, String> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let ctx = egui::Context::default();
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
    watch_addons(&cfg, &shown, event_loop.create_proxy());
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
    let mut app = App {
        windows,
        folded_seen: Vec::new(),
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
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.exit
        .ok_or_else(|| "the event loop ended without an exit request".to_string())
}

/// A thread that looks at the status and log files once a second and wakes
/// the loop only when one of them changed: the loop sleeps in between.
fn watch_addons(cfg: &Config, shown: &Arc<AtomicBool>, waker: EventLoopProxy<Wake>) {
    let (status, log) = (cfg.addon_status.clone(), cfg.addon_log.clone());
    let shown = shown.clone();
    // The first tick reads the files whatever they hold, so a change between
    // the menu's first read of the status and now is not lost.
    let mut watcher = addons::Watcher::default();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(ADDON_POLL);
            if !shown.load(Ordering::Relaxed) {
                continue;
            }
            if watcher.tick(&status, &log) && waker.send_event(Wake::Addons).is_err() {
                return;
            }
        }
    });
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
/// `name` shows a mode other than the stored one. Compares known values only.
fn monitor_moved(outputs: &[OutputInfo], known: Option<&str>, name: Option<&str>, px: Option<(u32, u32)>) -> bool {
    name != known || mode_changed(outputs, name, px)
}

impl App {
    /// Computes the menu's scale again. When it changed, or with `force`,
    /// the zoom factor, the icons, the logo and the scale file follow.
    fn apply_scale(&mut self, force: bool) {
        let Some(live) = self.live.as_mut() else { return };
        let monitor = live.window.current_monitor();
        let name = monitor.as_ref().and_then(|m| m.name());
        let px = monitor.as_ref().map(|m| (m.size().width, m.size().height));
        // The first look at a monitor needs no second read of the outputs.
        // A mode change on the same output (wlr-randr --mode, kanshi) needs one too.
        if (live.monitor.is_some() && name != live.monitor) || mode_changed(&live.outputs, name.as_deref(), px) {
            live.outputs = read_outputs(self.cfg.debug);
        }
        let (s, chosen) = pick_scale(&live.outputs, name.as_deref(), px, live.window.inner_size().width);
        let chosen = chosen.cloned();
        live.monitor = name;
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

    fn start(&self, job: Job) {
        let proxy = self.proxy.clone();
        let feed = self.feed.clone();
        let paths = Paths {
            save_home: self.cfg.paths.save_home.clone(),
            state_dir: self.cfg.paths.state_dir.clone(),
            addon_cmd: self.cfg.paths.addon_cmd.clone(),
        };
        std::thread::spawn(move || {
            let result = match job {
                Job::Save => JobResult::Saved(jobs::save(&paths, &feed)),
                Job::Run { id, name, argv } => {
                    let started = proxy.clone();
                    jobs::run(id, &name, &argv, &feed, move |pgid| {
                        let _ = started.send_event(Wake::Job(JobResult::Started { id, pgid }));
                    })
                }
                Job::Addon { args } => {
                    let started = proxy.clone();
                    let a = args.clone();
                    jobs::addon(&paths.addon_cmd, &args, &feed, move |pgid| {
                        let _ = started.send_event(Wake::Job(JobResult::AddonStarted { args: a, pgid }));
                    })
                }
            };
            // The loop is gone only when the menu is exiting; nothing to tell.
            let _ = proxy.send_event(Wake::Job(result));
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
        crate::brand::apply(&self.ctx);
        let input = live.egui.take_egui_input(&live.window);
        let now = Instant::now();
        let mut event = None;
        live.chrome.clock = crate::header::clock_text(std::time::SystemTime::now());
        // egui takes a new zoom factor at the start of the next pass and
        // lays that pass out with a guessed screen rect.
        let settling = settling(self.ctx.zoom_factor(), live.scale);
        let out = self.ctx.run_ui(input, |ui| {
            event = ui::show(ui, &self.cfg.menu, &live.icons, &live.chrome, now);
        });
        live.egui.handle_platform_output(&live.window, out.platform_output);

        // The dialog dims the grid once, when it opens, and un-freezes it
        // when it closes. Either way every pixel must be redrawn once.
        let saving = self.cfg.menu.dialog();
        let mut force_full = false;
        if saving != self.frozen {
            if saving {
                live.canvas.freeze_dimmed(DIM_KEEP);
            } else {
                let bg = crate::brand::theme().palette.background;
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
        let mut damage = live.tracker.damage(&prims, ppp);
        if force_full || !out.textures_delta.set.is_empty() {
            damage = Some(live.canvas.bounds());
        }
        if let Some(rect) = damage {
            let t0 = Instant::now();
            live.canvas.restore_background(rect);
            live.canvas.draw(&prims, &live.textures, ppp, rect);
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
        match delay {
            Some(d) if d.is_zero() => live.window.request_redraw(),
            Some(d) if d < std::time::Duration::from_secs(3600) => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + d))
            }
            _ => event_loop.set_control_flow(ControlFlow::Wait),
        }

        let effect = match event.clone() {
            Some(ui::UiEvent::Tile(i)) => self.cfg.menu.activate(i, now),
            Some(ui::UiEvent::Chose(c)) => self.cfg.menu.choose(c, now),
            Some(ui::UiEvent::Settings(e)) => self.cfg.menu.settings(e),
            Some(ui::UiEvent::Retry(r)) => self.cfg.menu.answer_retry(r),
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
        // The app_id labwc's window rule in /etc/thinfloe/labwc/rc.xml matches
        // to keep the menu at the bottom and out of the window list.
        let attrs = Window::default_attributes()
            .with_title("thinfloe-menu")
            .with_name("thinfloe-menu", "")
            .with_decorations(false)
            .with_maximized(true);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("thinfloe-menu: cannot create a window: {e}");
                self.exit = Some(1);
                event_loop.exit();
                return;
            }
        };
        let surface =
            softbuffer::Context::new(window.clone()).and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        let surface = match surface {
            Ok(s) => s,
            Err(e) => {
                eprintln!("thinfloe-menu: cannot create a drawing surface: {e}");
                self.exit = Some(1);
                event_loop.exit();
                return;
            }
        };
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
        };
        let bg = crate::brand::theme().palette.background;
        self.live = Some(Live {
            window: window.clone(),
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
            outputs: read_outputs(self.cfg.debug),
            presented: false,
        });
        // Icons and logo are drawn at the scale, so it comes first. Forced,
        // so the scale file exists even at 1.
        self.apply_scale(true);
        if self.cfg.debug
            && let Some(live) = self.live.as_ref()
        {
            // Hidden add-on tiles too: the Settings screen shows their icon
            // when it renders.
            let shown = self.cfg.menu.visible_tiles();
            let drawn = shown.iter().filter(|&&i| live.icons[i].is_some()).count();
            eprintln!("thinfloe-menu: {drawn} of {} tile icons drawn", shown.len());
        }
        window.request_redraw();
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if wake_needs_frame(&cause)
            && let Some(live) = self.live.as_ref()
        {
            live.window.request_redraw();
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
            Wake::Windows => self.windows_changed(),
            Wake::Output => {}
        }
        if let Some(live) = self.live.as_ref() {
            live.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.live.is_none() {
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
                    let bg = crate::brand::theme().palette.background;
                    live.canvas = Canvas::new(size.width as usize, size.height as usize, bg);
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
            WindowEvent::Occluded(hidden) => self.shown.store(!hidden, Ordering::Relaxed),
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn without_outputs_the_monitor_size_decides() {
        assert_eq!(pick_scale(&[], None, Some((3840, 2160)), 0).0, 2.0);
        assert_eq!(pick_scale(&[], None, None, 0).0, 1.0);
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
}
