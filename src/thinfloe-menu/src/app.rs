//! The event loop: winit for the Wayland window, egui-winit for input,
//! egui for layout, raster.rs for pixels, softbuffer to put them on screen.
//! Driven by hand rather than through eframe so that nothing here needs a
//! GPU. The rules this file follows are in the spec under "Rendering
//! without a GPU".

use crate::addons::{self, AddonLog};
use crate::damage::Tracker;
use crate::icons::IconRoots;
use crate::jobs::{self, JobResult, Paths};
use crate::log::Feed;
use crate::menu::{Effect, Job, Menu};
use crate::raster::{Background, Canvas, Textures};
use crate::ui;
use egui::{TextureHandle, ViewportId};
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
    icons: Vec<Option<TextureHandle>>,
    /// What the header shows; the clock text is refreshed every frame.
    chrome: ui::Chrome,
    /// The scale the last frame was laid out and damaged at.
    ppp: f32,
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
}

/// Run the menu until it exits. Returns the exit status for thinfloe-session.
pub fn run(cfg: Config) -> Result<i32, String> {
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
    let mut app = App {
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

impl App {
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
                        ui::load_icon(&self.ctx, &self.cfg.menu.tiles[i], &self.cfg.icon_roots)
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
        let Some(live) = self.live.as_mut() else { return };
        crate::brand::apply(&self.ctx);
        let input = live.egui.take_egui_input(&live.window);
        let now = Instant::now();
        let mut event = None;
        live.chrome.clock = crate::header::clock_text(std::time::SystemTime::now());
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

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        // The app_id labwc's window rule in /etc/thinfloe/labwc/rc.xml matches
        // to keep the menu at the bottom and off the taskbar.
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
        // Hidden add-on tiles too: the Settings screen shows their icon when
        // it renders.
        let icons = ui::load_icons(&self.ctx, &self.cfg.menu.tiles, &self.cfg.icon_roots);
        if self.cfg.debug {
            let shown = self.cfg.menu.visible_tiles();
            let drawn = shown.iter().filter(|&&i| icons[i].is_some()).count();
            eprintln!("thinfloe-menu: {drawn} of {} tile icons drawn", shown.len());
        }
        let logo_px = (ui::LOGO_H * window.scale_factor() as f32).round() as u32;
        let chrome = ui::Chrome {
            header: self.cfg.header.clone(),
            clock: String::new(),
            logo: Some(self.ctx.load_texture(
                "thinfloe-logo",
                crate::brand::logo(logo_px),
                egui::TextureOptions::LINEAR,
            )),
        };
        let bg = crate::brand::theme().palette.background;
        self.live = Some(Live {
            window: window.clone(),
            surface,
            egui,
            canvas: Canvas::new(1, 1, bg),
            textures: Textures::default(),
            tracker: Tracker::default(),
            icons,
            chrome,
            ppp: 0.0,
            presented: false,
        });
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
            Wake::Output => {}
        }
        if let Some(live) = self.live.as_ref() {
            live.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(live) = self.live.as_mut() else { return };
        match &event {
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            WindowEvent::Resized(size) => {
                if self.cfg.debug {
                    eprintln!("thinfloe-menu: configure {}x{}", size.width, size.height);
                }
                if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    if let Err(e) = live.surface.resize(w, h) {
                        eprintln!("thinfloe-menu: resize failed: {e}");
                    }
                    let bg = crate::brand::theme().palette.background;
                    live.canvas = Canvas::new(size.width as usize, size.height as usize, bg);
                    self.frozen = false;
                    live.tracker.reset();
                }
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
