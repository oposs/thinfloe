//! Icon lookup and rasterisation. Icons are drawn once at startup into
//! pixmaps; nothing here runs per frame.

use crate::bounded::read_bounded;
use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use resvg::usvg;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Icon files larger than this are not read.
pub const MAX_ICON_BYTES: u64 = 1024 * 1024;
/// A PNG declaring a larger side is refused before it is decoded, so a small
/// file cannot ask for gigabytes.
pub const MAX_PNG_SIDE: u32 = 4096;
/// Deeper SVG nesting is refused. Among the 20000 SVGs under
/// /usr/share/icons on a desktop host the deepest nests 15 levels.
pub const MAX_SVG_DEPTH: usize = 64;
/// SVGs with more elements are refused. The largest icon on the same host
/// has 6741.
pub const MAX_SVG_ELEMENTS: usize = 10_000;

/// Where `Icon=` names are looked up.
#[derive(Debug, Clone)]
pub struct IconRoots {
    pub thinfloe: PathBuf,
    pub hicolor: PathBuf,
    pub adwaita: PathBuf,
}

impl IconRoots {
    pub fn system() -> IconRoots {
        IconRoots {
            thinfloe: PathBuf::from("/usr/share/thinfloe/icons"),
            hicolor: PathBuf::from("/usr/share/icons/hicolor"),
            adwaita: PathBuf::from("/usr/share/icons/Adwaita"),
        }
    }
}

fn is_file(p: &Path) -> bool {
    // metadata follows symlinks: icon themes are full of them. A device node
    // or FIFO is not a file and is refused here.
    fs::metadata(p).is_ok_and(|m| m.is_file())
}

fn sorted_subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// `NxN` with equal sides, as hicolor names its pixel directories. `128x128@2`
/// and `scalable` are not pixel directories.
fn pixel_size(dir_name: &str) -> Option<u32> {
    let (w, h) = dir_name.split_once('x')?;
    let w: u32 = w.parse().ok()?;
    (h.parse::<u32>().ok()? == w).then_some(w)
}

/// Resolve an `Icon=` value, in the order the spec fixes. `None` means the
/// tile draws the first letter of its name instead.
pub fn resolve(icon: &str, roots: &IconRoots) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }
    if icon.starts_with('/') {
        let p = PathBuf::from(icon);
        return is_file(&p).then_some(p);
    }
    if icon.contains('/') || icon.starts_with('.') {
        return None;
    }
    let svg = format!("{icon}.svg");
    let png = format!("{icon}.png");
    let ours = roots.thinfloe.join(&svg);
    if is_file(&ours) {
        return Some(ours);
    }
    let scalable = roots.hicolor.join("scalable/apps").join(&svg);
    if is_file(&scalable) {
        return Some(scalable);
    }
    let mut sizes: Vec<(u32, PathBuf)> = fs::read_dir(&roots.hicolor)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let size = pixel_size(e.file_name().to_str()?)?;
            Some((size, e.path()))
        })
        .collect();
    sizes.sort_by_key(|s| std::cmp::Reverse(s.0));
    if let Some(p) = sizes
        .iter()
        .map(|(_, d)| d.join("apps").join(&png))
        .find(|p| is_file(p))
    {
        return Some(p);
    }
    for sub in ["symbolic", "scalable"] {
        if let Some(p) = sorted_subdirs(&roots.adwaita.join(sub))
            .into_iter()
            .map(|d| d.join(&svg))
            .find(|p| is_file(p))
        {
            return Some(p);
        }
    }
    None
}

/// Lucide draws with `stroke="currentColor"`, which resvg resolves to black,
/// and Adwaita's symbolic icons hardcode `#2e3436` for GTK to recolour. Both
/// are replaced with `rgb` in the source before parsing. A full-colour SVG is
/// left alone unless it says `currentColor`.
pub fn recolour_svg(source: &str, symbolic: bool, rgb: [u8; 3]) -> String {
    let hex = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
    let s = source.replace("currentColor", &hex);
    if symbolic { s.replace("#2e3436", &hex) } else { s }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

/// Whether an icon is a one-colour glyph to be drawn in the tile's text
/// colour: the shipped Lucide set and Adwaita's `-symbolic` icons.
pub fn is_symbolic(path: &Path, roots: &IconRoots) -> bool {
    path.starts_with(&roots.thinfloe)
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-symbolic.svg"))
}

/// Index just past the first `needle` at or after `from`, or the end.
fn skip_past(b: &[u8], from: usize, needle: &[u8]) -> usize {
    b.get(from..)
        .and_then(|rest| rest.windows(needle.len()).position(|w| w == needle))
        .map_or(b.len(), |at| from + at + needle.len())
}

/// Refuse an SVG whose shape alone can overflow the stack, before any parser
/// sees it. Parsing and rendering recurse once per nesting level and per
/// `url(#…)` reference followed. The overflow would only kill the renderer
/// process, but this costs one pass over the text instead of a process
/// that dies. The depth limit covers nesting; the element limit bounds how
/// long a chain of references can be, and ICON_STACK is sized for the
/// longest one it lets through. An entity can expand into markup this scan
/// never sees, and icons have no use for entities.
///
/// The scan only has to be right for files roxmltree would accept: a file
/// it misreads is either refused here or rejected by the parser.
fn svg_shape(text: &str, max_depth: usize, max_elements: usize) -> Result<(), String> {
    if text.contains("<!ENTITY") {
        return Err("SVG declares entities".to_string());
    }
    let b = text.as_bytes();
    let (mut i, mut depth, mut elements) = (0, 0usize, 0usize);
    while let Some(at) = b[i..].iter().position(|&c| c == b'<') {
        i += at;
        let rest = &b[i..];
        i = if rest.starts_with(b"<!--") {
            skip_past(b, i + 4, b"-->")
        } else if rest.starts_with(b"<![CDATA[") {
            skip_past(b, i + 9, b"]]>")
        } else if rest.starts_with(b"<?") {
            skip_past(b, i + 2, b"?>")
        } else if rest.starts_with(b"<!") {
            skip_past(b, i + 2, b">")
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            skip_past(b, i + 2, b">")
        } else {
            // A start tag. Attribute values may hold `>` and `/>`.
            let mut j = i + 1;
            let mut quote = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (Some(q), c) if c == q => quote = None,
                    (None, c @ (b'"' | b'\'')) => quote = Some(c),
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            elements += 1;
            if elements > max_elements {
                return Err(format!("more than {max_elements} elements"));
            }
            if depth >= max_depth {
                return Err(format!("elements nested deeper than {max_depth}"));
            }
            if b[j - 1] != b'/' {
                depth += 1;
            }
            j + 1
        };
        if i >= b.len() {
            break;
        }
    }
    Ok(())
}

/// Parser options that load nothing a tile icon names. usvg's default `<image
/// href>` resolver does an unbounded `fs::read` of any path (`/dev/zero` never
/// ends, a FIFO blocks) and decodes `data:` URIs of any declared size, which
/// would bypass `MAX_ICON_BYTES` and `MAX_PNG_SIDE` from a file the user can
/// write. Icons need no embedded images, so both resolvers return nothing.
fn svg_options() -> usvg::Options<'static> {
    let mut opts = usvg::Options::default();
    opts.image_href_resolver.resolve_string = Box::new(|_, _| None);
    opts.image_href_resolver.resolve_data = Box::new(|_, _, _| None);
    opts
}

/// Rasterise the icon at `path` into a `side` x `side` premultiplied RGBA
/// image, keeping its aspect ratio and centring it.
pub fn render(path: &Path, side: u32, rgb: [u8; 3], symbolic: bool) -> Result<egui::ColorImage, String> {
    let bytes = read_bounded(path, MAX_ICON_BYTES, true)?;
    let mut out = Pixmap::new(side, side).ok_or("bad icon size")?;
    let is_svg = path.extension().is_some_and(|e| e == "svg");
    if is_svg {
        let text = String::from_utf8(bytes).map_err(|_| "SVG is not UTF-8".to_string())?;
        svg_shape(&text, MAX_SVG_DEPTH, MAX_SVG_ELEMENTS)?;
        let text = recolour_svg(&text, symbolic, rgb);
        let tree = usvg::Tree::from_str(&text, &svg_options()).map_err(|e| e.to_string())?;
        // Filters are where a small file costs unbounded time: one
        // feTurbulence with a huge numOctaves rendered for over a minute.
        // A tile icon needs none, and the tree lists every one in use,
        // including those written as CSS (`filter: blur()`).
        if !tree.filters().is_empty() {
            return Err("SVG uses filters".to_string());
        }
        let size = tree.size();
        let scale = side as f32 / size.width().max(size.height());
        let dx = (side as f32 - size.width() * scale) / 2.0;
        let dy = (side as f32 - size.height() * scale) / 2.0;
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale).post_translate(dx, dy),
            &mut out.as_mut(),
        );
    } else {
        let (w, h) = png_dimensions(&bytes).ok_or("not a PNG file")?;
        if w == 0 || h == 0 || w > MAX_PNG_SIDE || h > MAX_PNG_SIDE {
            return Err(format!("PNG is {w}x{h}, larger than {MAX_PNG_SIDE}x{MAX_PNG_SIDE}"));
        }
        let src = Pixmap::decode_png(&bytes).map_err(|e| e.to_string())?;
        let scale = side as f32 / w.max(h) as f32;
        let dx = (side as f32 - w as f32 * scale) / 2.0;
        let dy = (side as f32 - h as f32 * scale) / 2.0;
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..Default::default()
        };
        out.draw_pixmap(
            0,
            0,
            src.as_ref(),
            &paint,
            Transform::from_scale(scale, scale).post_translate(dx, dy),
            None,
        );
    }
    Ok(egui::ColorImage::from_rgba_premultiplied(
        [side as usize, side as usize],
        out.data(),
    ))
}

/// How long one icon may take to render before its tile draws the letter.
pub const ICON_DEADLINE: Duration = Duration::from_secs(2);
/// How long the first frame may wait for all icons together.
pub const ICONS_BUDGET: Duration = Duration::from_secs(5);
/// Address space of one renderer process. Nothing in-process bounds what
/// usvg and resvg allocate: a 700-byte SVG of nested patterns asked for
/// 2.88 GB. The kernel refuses the allocation at this size, the renderer
/// aborts, and the tile draws its letter. A release build renders every
/// shipped icon, an 880 KB theme icon and the longest chain MAX_SVG_ELEMENTS
/// lets through in 64 MiB; the debug build (the tests) needs 192 MiB for
/// the chain.
pub const ICON_MEMORY: u64 = 256 * 1024 * 1024;
/// Stack of the renderer's main thread. usvg and resvg recurse once per
/// level of nesting and per `url(#…)` reference they follow; the longest
/// chain MAX_SVG_ELEMENTS lets through needs 32 MiB in a release build and
/// 128 MiB in a debug build. Only the pages a render touches count against
/// ICON_MEMORY.
pub const ICON_STACK: u64 = 256 * 1024 * 1024;
/// CPU seconds of one renderer process, a backstop for the wall-clock
/// deadline should the menu itself stop before it kills the renderer.
const ICON_CPU_SECONDS: u64 = 3;
/// The argument that makes thinfloe-menu a renderer process.
pub const RENDER_ICON_ARG: &str = "--render-icon";

const REPLY_IMAGE: &[u8; 4] = b"RGBA";
const REPLY_ERROR: &[u8; 4] = b"FAIL";
const NO_IMAGE: &str = "the renderer returned no usable image";
/// How long the reason of a renderer that has exited may take to arrive.
const STDERR_GRACE: Duration = Duration::from_millis(100);
/// Longest error message read back from a renderer.
const MAX_ERROR_BYTES: usize = 1024;

/// One icon to rasterise: what a renderer process gets on its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconJob {
    pub path: PathBuf,
    pub side: u32,
    pub rgb: [u8; 3],
    pub symbolic: bool,
}

impl IconJob {
    fn args(&self) -> Vec<OsString> {
        let [r, g, b] = self.rgb;
        vec![
            self.side.to_string().into(),
            format!("{r:02x}{g:02x}{b:02x}").into(),
            if self.symbolic { "1" } else { "0" }.into(),
            self.path.clone().into_os_string(),
        ]
    }

    fn from_args(args: &[OsString]) -> Option<IconJob> {
        let [side, rgb, symbolic, path] = args else {
            return None;
        };
        let side: u32 = side.to_str()?.parse().ok().filter(|s| (1..=1024).contains(s))?;
        let rgb = rgb.to_str().filter(|s| s.len() == 6)?;
        let byte = |i: usize| u8::from_str_radix(rgb.get(i..i + 2)?, 16).ok();
        let symbolic = match symbolic.to_str()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        Some(IconJob {
            path: PathBuf::from(path),
            side,
            rgb: [byte(0)?, byte(2)?, byte(4)?],
            symbolic,
        })
    }
}

/// `thinfloe-menu --render-icon SIDE RRGGBB SYMBOLIC PATH`: the renderer
/// process. It runs under the limits `render_all` set before exec and
/// answers on stdout: `RGBA`, width and height as little-endian u32, then
/// the premultiplied pixels; or `FAIL` and why, with exit status 1.
pub fn render_icon_main(args: &[OsString]) -> ExitCode {
    let result = IconJob::from_args(args)
        .ok_or_else(|| "usage: thinfloe-menu --render-icon SIDE RRGGBB 0|1 PATH".to_string())
        .and_then(|job| render(&job.path, job.side, job.rgb, job.symbolic));
    let mut reply = Vec::new();
    let status = match result {
        Ok(img) => {
            reply.extend_from_slice(REPLY_IMAGE);
            reply.extend_from_slice(&(img.size[0] as u32).to_le_bytes());
            reply.extend_from_slice(&(img.size[1] as u32).to_le_bytes());
            reply.extend_from_slice(img.as_raw());
            ExitCode::SUCCESS
        }
        Err(e) => {
            reply.extend_from_slice(REPLY_ERROR);
            reply.extend_from_slice(e.as_bytes());
            ExitCode::from(1)
        }
    };
    let mut out = std::io::stdout().lock();
    if out.write_all(&reply).and_then(|_| out.flush()).is_err() {
        return ExitCode::from(1);
    }
    status
}

/// Set both limits of `resource` to `value`, or to the hard limit already
/// in force if that is lower: only root may raise it.
fn set_limit(resource: libc::__rlimit_resource_t, value: u64) -> io::Result<()> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit and setrlimit only touch the struct they are given,
    // and both are async-signal-safe, which code between fork and exec has
    // to be.
    if unsafe { libc::getrlimit(resource, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let value = value.min(limit.rlim_max);
    limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Runs in the forked child before exec. A limit the kernel refuses fails
/// the spawn, and the tile draws its letter. `menu` is the pid of the
/// process that spawns the renderer.
fn limit_renderer(menu: u32) -> io::Result<()> {
    // The menu enforces the deadline, so a renderer whose menu died must
    // die too rather than run on until RLIMIT_CPU ends it. A menu that died
    // before the prctl took effect shows as a different parent.
    // SAFETY: prctl and getppid are async-signal-safe and touch no memory
    // of ours.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } as u32 != menu {
        return Err(io::ErrorKind::Interrupted.into());
    }
    set_limit(libc::RLIMIT_AS, ICON_MEMORY)?;
    set_limit(libc::RLIMIT_STACK, ICON_STACK)?;
    set_limit(libc::RLIMIT_CPU, ICON_CPU_SECONDS)?;
    // The renderer writes no files and opens one; the dynamic loader opens
    // the libraries one at a time.
    set_limit(libc::RLIMIT_FSIZE, 0)?;
    set_limit(libc::RLIMIT_NOFILE, 16)?;
    // Every refused allocation ends in abort(); no core file may land in
    // the menu's directory, which can be the home that save-home archives.
    set_limit(libc::RLIMIT_CORE, 0)
}

/// A renderer process that is killed and reaped when this goes out of
/// scope, on every path: no renderer outlives `render_one`, none is left a
/// zombie.
struct Renderer(Child);

impl Drop for Renderer {
    fn drop(&mut self) {
        // Both are no-ops on a child that has already been reaped.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Check a renderer's reply and exit status. Anything but a well-formed
/// image of at most `side` x `side` is an error.
fn parse_reply(reply: &[u8], said: &[u8], status: ExitStatus, side: u32) -> Result<egui::ColorImage, String> {
    // What a failed renderer wrote to stderr, such as Rust's "memory
    // allocation of 1474560000 bytes failed", on one line.
    let said: Vec<&str> = std::str::from_utf8(said)
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("note: "))
        .collect();
    let said = if said.is_empty() {
        String::new()
    } else {
        format!(" ({})", said.join(" "))
    };
    if let Some(signal) = status.signal() {
        return Err(format!("the renderer was killed by signal {signal}{said}"));
    }
    if let Some(message) = reply.strip_prefix(REPLY_ERROR)
        && status.code() == Some(1)
    {
        let end = message.len().min(MAX_ERROR_BYTES);
        return Err(String::from_utf8_lossy(&message[..end]).into_owned());
    }
    let malformed = || NO_IMAGE.to_string();
    if !status.success() {
        return Err(format!("the renderer failed with {status}{said}"));
    }
    let pixels = reply.strip_prefix(REPLY_IMAGE).ok_or_else(malformed)?;
    let (w, rest) = pixels.split_first_chunk::<4>().ok_or_else(malformed)?;
    let (h, data) = rest.split_first_chunk::<4>().ok_or_else(malformed)?;
    let (w, h) = (u32::from_le_bytes(*w), u32::from_le_bytes(*h));
    if !(1..=side).contains(&w) || !(1..=side).contains(&h) || data.len() as u64 != w as u64 * h as u64 * 4 {
        return Err(malformed());
    }
    Ok(egui::ColorImage::from_rgba_premultiplied(
        [w as usize, h as usize],
        data,
    ))
}

/// Rasterise one icon in a renderer process started from `program`, and
/// kill it once `wait` has passed.
fn render_one(program: &Path, job: &IconJob, wait: Duration) -> Result<egui::ColorImage, NoImage> {
    let end = Instant::now() + wait;
    let mut command = Command::new(program);
    command
        .arg(RENDER_ICON_ARG)
        .args(job.args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // A pipe, not the menu's stderr: that is the session log, a file,
        // and RLIMIT_FSIZE kills a renderer that writes to a file.
        .stderr(Stdio::piped());
    let menu = std::process::id();
    // SAFETY: limit_renderer only makes async-signal-safe calls and does
    // not allocate.
    unsafe { command.pre_exec(move || limit_renderer(menu)) };
    let mut renderer = Renderer(
        command
            .spawn()
            .map_err(|e| NoImage::Failed(format!("cannot start the renderer: {e}")))?,
    );
    let no_pipe = || NoImage::Failed("no pipe to the renderer".into());
    // Never more than the largest valid reply, plus one byte to tell a
    // reply that is too long.
    let max = (12 + job.side as u64 * job.side as u64 * 4).max(4 + MAX_ERROR_BYTES as u64) + 1;
    let reply = read_aside(renderer.0.stdout.take().ok_or_else(no_pipe)?, max)?;
    let said = read_aside(renderer.0.stderr.take().ok_or_else(no_pipe)?, MAX_ERROR_BYTES as u64)?;
    let reply = reply
        .recv_timeout(end.saturating_duration_since(Instant::now()))
        .map_err(|_| NoImage::Late)?;
    if reply.len() as u64 == max {
        return Err(NoImage::Failed(NO_IMAGE.to_string()));
    }
    // The renderer closes its stdout when it exits.
    let status = loop {
        if let Some(status) = renderer.0.try_wait().map_err(|e| NoImage::Failed(e.to_string()))? {
            break status;
        }
        if Instant::now() >= end {
            return Err(NoImage::Late);
        }
        thread::sleep(Duration::from_millis(1));
    };
    // The renderer has exited, so its stderr is at its end, also when that
    // was right at the deadline: the grace only waits for the reader thread.
    let said = said
        .recv_timeout(end.saturating_duration_since(Instant::now()).max(STDERR_GRACE))
        .unwrap_or_default();
    parse_reply(&reply, &said, status, job.side).map_err(NoImage::Failed)
}

/// Read at most `max` bytes from a renderer's pipe on a thread of its own,
/// since the read blocks. Killing the renderer closes the pipe and ends the
/// thread.
fn read_aside(pipe: impl Read + Send + 'static, max: u64) -> Result<mpsc::Receiver<Vec<u8>>, NoImage> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("thinfloe-menu-icon".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(max).read_to_end(&mut bytes);
            let _ = tx.send(bytes);
        })
        .map_err(|e| NoImage::Failed(e.to_string()))?;
    Ok(rx)
}

/// Why `render_one` has no image: its time ran out, or something else.
enum NoImage {
    Late,
    Failed(String),
}

/// Rasterise every job, each in a renderer process of its own started from
/// `program` (thinfloe-menu itself), one at a time, and give each its image or
/// why it has none.
///
/// The renderers parse files the user can write. The kernel bounds each
/// one's memory, stack and CPU time (`limit_renderer`); this bounds the
/// wall-clock time: a renderer that misses `per_icon`, or the `budget` all
/// of them share, is killed and reaped and its tile draws the letter. One at
/// a time, at most one renderer holds ICON_MEMORY.
pub fn render_all(
    program: &Path,
    jobs: &[IconJob],
    per_icon: Duration,
    budget: Duration,
) -> Vec<Result<egui::ColorImage, String>> {
    let end = Instant::now() + budget;
    jobs.iter()
        .map(|job| {
            let wait = per_icon.min(end.saturating_duration_since(Instant::now()));
            if wait.is_zero() {
                return Err("no time left at startup".to_string());
            }
            match render_one(program, job, wait) {
                Ok(img) => Ok(img),
                Err(NoImage::Late) if wait < per_icon => Err("no time left at startup".to_string()),
                Err(NoImage::Late) => Err(format!("took longer than {per_icon:?}")),
                Err(NoImage::Failed(e)) => Err(e),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    /// The directory goes when the returned guard drops, so callers bind it
    /// to a named variable for the length of the test.
    fn roots(tag: &str) -> (TestDir, IconRoots) {
        let base = TestDir::new("icons", tag);
        let r = IconRoots {
            thinfloe: base.join("thinfloe"),
            hicolor: base.join("hicolor"),
            adwaita: base.join("Adwaita"),
        };
        for d in [
            "thinfloe",
            "hicolor/scalable/apps",
            "hicolor/48x48/apps",
            "hicolor/256x256/apps",
            "hicolor/1024x1024@2/apps",
            "Adwaita/symbolic/actions",
            "Adwaita/scalable/devices",
        ] {
            fs::create_dir_all(base.join(d)).unwrap();
        }
        (base, r)
    }

    fn touch(p: &Path) {
        fs::write(p, "x").unwrap();
    }

    #[test]
    fn resolution_order() {
        let (_dir, r) = roots("order");
        touch(&r.thinfloe.join("usb.svg"));
        touch(&r.hicolor.join("scalable/apps/usb.svg"));
        assert_eq!(
            resolve("usb", &r),
            Some(r.thinfloe.join("usb.svg")),
            "shipped Lucide first"
        );

        touch(&r.hicolor.join("scalable/apps/foot.svg"));
        touch(&r.hicolor.join("48x48/apps/foot.png"));
        assert_eq!(
            resolve("foot", &r),
            Some(r.hicolor.join("scalable/apps/foot.svg")),
            "scalable before pixels"
        );

        touch(&r.hicolor.join("48x48/apps/ox.png"));
        touch(&r.hicolor.join("256x256/apps/ox.png"));
        touch(&r.hicolor.join("1024x1024@2/apps/ox.png"));
        assert_eq!(
            resolve("ox", &r),
            Some(r.hicolor.join("256x256/apps/ox.png")),
            "largest NxN, @2 ignored"
        );

        touch(&r.adwaita.join("symbolic/actions/edit-symbolic.svg"));
        assert_eq!(
            resolve("edit-symbolic", &r),
            Some(r.adwaita.join("symbolic/actions/edit-symbolic.svg"))
        );
        touch(&r.adwaita.join("scalable/devices/drive-harddisk.svg"));
        assert_eq!(
            resolve("drive-harddisk", &r),
            Some(r.adwaita.join("scalable/devices/drive-harddisk.svg"))
        );
    }

    #[test]
    fn absolute_and_fallback() {
        let (_dir, r) = roots("abs");
        let p = r.thinfloe.join("logo.png");
        touch(&p);
        assert_eq!(resolve(p.to_str().unwrap(), &r), Some(p));
        assert_eq!(resolve("/dev/zero", &r), None, "a device is not an icon");
        assert_eq!(resolve("/nonexistent/x.png", &r), None);
        assert_eq!(resolve("no-such-icon", &r), None);
        assert_eq!(resolve("../../etc/passwd", &r), None);
        assert_eq!(resolve("", &r), None);
    }

    #[test]
    fn recolours_lucide_and_symbolic_only() {
        let rgb = [0x12, 0x34, 0x56];
        assert_eq!(
            recolour_svg(r#"stroke="currentColor""#, false, rgb),
            r##"stroke="#123456""##
        );
        assert_eq!(recolour_svg(r##"fill="#2e3436""##, true, rgb), r##"fill="#123456""##);
        assert_eq!(recolour_svg(r##"fill="#2e3436""##, false, rgb), r##"fill="#2e3436""##);
    }

    #[test]
    fn renders_lucide_svg_in_the_given_colour() {
        let (_dir, r) = roots("render");
        let p = r.thinfloe.join("power.svg");
        fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 2v10"/></svg>"#,
        )
        .unwrap();
        assert!(is_symbolic(&p, &r));
        let img = render(&p, 48, [255, 0, 0], true).unwrap();
        assert_eq!(img.size, [48, 48]);
        assert!(
            img.pixels.iter().any(|c| c.r() > 200 && c.g() == 0 && c.a() > 200),
            "red stroke drawn"
        );
    }

    // A referenced SVG is the one image kind usvg loads without a raster
    // decoder, so drawing it is observable; the files are small and real, so
    // a regression cannot run unbounded.
    #[test]
    fn embedded_images_are_not_loaded() {
        let (_dir, r) = roots("embed");
        let sub = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><rect width="24" height="24" fill="#ff0000"/></svg>"##;
        let sub_path = r.thinfloe.join("sub.svg");
        fs::write(&sub_path, sub).unwrap();
        let data_uri = format!("data:image/svg+xml;base64,{}", b64(sub.as_bytes()));
        for (tag, href) in [("file", sub_path.to_str().unwrap().to_string()), ("data", data_uri)] {
            let p = r.thinfloe.join(format!("outer-{tag}.svg"));
            fs::write(
                &p,
                format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="24" height="24"><image width="24" height="24" xlink:href="{href}"/></svg>"#
                ),
            )
            .unwrap();
            let img = render(&p, 24, [0; 3], false).unwrap();
            assert!(img.pixels.iter().all(|c| c.a() == 0), "{tag} image drawn");
        }
    }

    fn b64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn svg(body: &str) -> String {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24">{body}</svg>"#)
    }

    fn nested(depth: usize) -> String {
        svg(&format!(
            r#"{}<rect width="24" height="24"/>{}"#,
            "<g>".repeat(depth - 2),
            "</g>".repeat(depth - 2)
        ))
    }

    /// A chain of patterns, each filled with the next: flat in the file, but
    /// usvg and resvg recurse once per link.
    fn pattern_chain(links: usize) -> String {
        let mut body = String::from("<defs>");
        for i in 0..links {
            body += &format!(
                r#"<pattern id="p{i}" width="1" height="1"><rect width="1" height="1" fill="url(#p{})"/></pattern>"#,
                i + 1
            );
        }
        body += r#"</defs><rect width="24" height="24" fill="url(#p0)"/>"#;
        svg(&body)
    }

    fn render_text(tag: &str, text: &str) -> Result<egui::ColorImage, String> {
        let (_dir, r) = roots(tag);
        let p = r.thinfloe.join("x.svg");
        fs::write(&p, text).unwrap();
        render(&p, 24, [0; 3], false)
    }

    // Before the scan, the deep file overflowed the stack: an abort, not a
    // panic, so it ended the whole test binary rather than failing a test.
    #[test]
    fn deep_nesting_is_refused_before_parsing() {
        assert_eq!(
            render_text("deep", &nested(100_000)).unwrap_err(),
            "elements nested deeper than 64"
        );
        assert!(
            render_text("deep64", &nested(64)).is_ok(),
            "the limit itself is allowed"
        );
        assert!(render_text("deep65", &nested(65)).is_err());
    }

    #[test]
    fn a_long_reference_chain_is_refused_before_parsing() {
        // 1000 links overflowed a 2 MiB stack.
        assert_eq!(
            render_text("chain", &pattern_chain(6_000)).unwrap_err(),
            "more than 10000 elements"
        );
    }

    #[test]
    fn every_shipped_icon_passes_the_guards_and_draws() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
        let mut svgs: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "svg"))
            .collect();
        svgs.sort();
        assert!(!svgs.is_empty());
        for p in svgs {
            let img = render(&p, 96, [255, 255, 255], true).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(img.pixels.iter().any(|c| c.a() > 200), "{} drew nothing", p.display());
        }
    }

    #[test]
    fn entity_declarations_are_refused() {
        let text = r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "<g><rect width='1' height='1'/></g>">]><svg xmlns="http://www.w3.org/2000/svg" width="24" height="24">&a;</svg>"#;
        assert_eq!(render_text("entity", text).unwrap_err(), "SVG declares entities");
    }

    #[test]
    fn the_scan_reads_comments_cdata_and_quoted_brackets_as_text() {
        let body = r#"<!-- <g><g><g> --><style><![CDATA[ <g><g> ]]></style><?pi <g> ?><g title="a > b <g" id='x/>'><rect width="1" height="1"/></g>"#;
        assert_eq!(svg_shape(&svg(body), 3, 100), Ok(()));
        assert_eq!(
            svg_shape(&svg(body), 2, 100),
            Err("elements nested deeper than 2".to_string())
        );
        assert_eq!(svg_shape(&svg(body), 3, 3), Err("more than 3 elements".to_string()));
    }

    #[test]
    fn a_job_survives_the_command_line() {
        let job = IconJob {
            path: PathBuf::from("/usr/share/thinfloe/icons/usb.svg"),
            side: 96,
            rgb: [0xfa, 0x0b, 0x01],
            symbolic: true,
        };
        assert_eq!(job.args()[1], "fa0b01");
        assert_eq!(IconJob::from_args(&job.args()), Some(job.clone()));
        let mut bad = job.args();
        bad[0] = "0".into();
        assert_eq!(IconJob::from_args(&bad), None, "side 0");
        assert_eq!(IconJob::from_args(&job.args()[..3]), None, "no path");
    }

    /// A stand-in for thinfloe-menu --render-icon: a shell script that acts on
    /// the name of the icon it is asked for. A renderer that lingers leaves
    /// its pid next to that name, so the test can see it was reaped; as a
    /// symlink, since RLIMIT_FSIZE kills a renderer that writes a file.
    ///
    /// A child shell writes the script, never this process: a test thread
    /// that forks while this process holds the file open for writing hands
    /// the descriptor to its child until that child execs, and starting the
    /// renderer in that window fails with ETXTBSY ("Text file busy").
    fn fake_renderer(dir: &Path) -> PathBuf {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let p = dir.join("renderer");
        let mut sh = Command::new("/bin/sh")
            .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
            .arg(&p)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        sh.stdin
            .take()
            .unwrap()
            .write_all(
                br#"#!/bin/sh
case "$5" in
*ok) printf 'RGBA\002\000\000\000\001\000\000\000\377\000\000\377\377\000\000\377' ;;
*hang) ln -s $$ "$5.pid"; exec sleep 60 ;;
*closed) ln -s $$ "$5.pid"; exec sleep 60 >&- ;;
*endless) exec cat /dev/zero ;;
*garbage) printf 'hello' ;;
*wide) printf 'RGBA\005\000\000\000\001\000\000\000' ;;
*short) printf 'RGBA\002\000\000\000\001\000\000\000\377' ;;
*long) printf 'RGBA\001\000\000\000\001\000\000\000\377\377\377\377\377' ;;
*fail) printf 'FAILno such icon'; exit 1 ;;
*panic) exit 101 ;;
*stderr) echo 'out of luck' >&2; echo 'note: ignored' >&2; exit 3 ;;
*killed) kill -TERM $$ ;;
esac
"#,
            )
            .unwrap();
        assert!(sh.wait().unwrap().success(), "writing {}", p.display());
        p
    }

    fn fake_jobs(dir: &Path, names: &[&str]) -> Vec<IconJob> {
        names
            .iter()
            .map(|n| IconJob {
                path: dir.join(n),
                side: 4,
                rgb: [0; 3],
                symbolic: false,
            })
            .collect()
    }

    /// The renderer that wrote `<name>.pid` is gone: killed and reaped, not
    /// left running and not a zombie.
    fn assert_reaped(dir: &Path, name: &str) {
        let pid = fs::read_link(dir.join(format!("{name}.pid"))).unwrap();
        let proc = Path::new("/proc").join(&pid);
        assert!(!proc.exists(), "renderer {} of {name} is still there", pid.display());
    }

    /// Deadlines for the tests that must not meet one. The production
    /// ICON_DEADLINE is a wall-clock bound, and a loaded host stretches the
    /// shell-script renderers past it; these tests check the reply, not the
    /// clock. That none hit a deadline shows in the expected results, since a
    /// late renderer reports "took longer than ...".
    const PATIENT_ICON: Duration = Duration::from_secs(60);
    const PATIENT_BUDGET: Duration = Duration::from_secs(300);

    #[test]
    fn a_well_formed_reply_is_the_icon() {
        let dir = TestDir::new("icons", "child-ok");
        let out = render_all(
            &fake_renderer(&dir),
            &fake_jobs(&dir, &["ok"]),
            PATIENT_ICON,
            PATIENT_BUDGET,
        );
        let img = out[0].as_ref().unwrap();
        assert_eq!(img.size, [2, 1]);
        assert_eq!(img.pixels[0], egui::Color32::from_rgba_premultiplied(255, 0, 0, 255));
    }

    #[test]
    fn a_bad_reply_draws_the_letter() {
        let dir = TestDir::new("icons", "child-bad");
        let names = [
            "garbage", "wide", "short", "long", "endless", "fail", "panic", "stderr", "killed",
        ];
        let out = render_all(
            &fake_renderer(&dir),
            &fake_jobs(&dir, &names),
            PATIENT_ICON,
            PATIENT_BUDGET,
        );
        let unusable = Err("the renderer returned no usable image".to_string());
        let errors: Vec<_> = out.into_iter().map(|r| r.map(|_| ())).collect();
        assert_eq!(
            errors,
            vec![
                unusable.clone(),
                unusable.clone(),
                unusable.clone(),
                unusable.clone(),
                unusable,
                Err("no such icon".to_string()),
                Err("the renderer failed with exit status: 101".to_string()),
                Err("the renderer failed with exit status: 3 (out of luck)".to_string()),
                Err("the renderer was killed by signal 15".to_string()),
            ]
        );
    }

    #[test]
    fn a_renderer_that_cannot_start_draws_the_letter() {
        let dir = TestDir::new("icons", "child-missing");
        let out = render_all(
            &dir.join("nothing"),
            &fake_jobs(&dir, &["ok"]),
            ICON_DEADLINE,
            ICONS_BUDGET,
        );
        assert!(out[0].as_ref().unwrap_err().starts_with("cannot start the renderer"));
    }

    // The hanging renderers sleep far past every deadline here, so the test
    // only passes when the deadline killed them.
    #[test]
    fn a_slow_renderer_is_killed_and_reaped_at_the_deadline() {
        let dir = TestDir::new("icons", "child-slow");
        let start = Instant::now();
        let out = render_all(
            &fake_renderer(&dir),
            &fake_jobs(&dir, &["ok", "hang", "closed", "ok"]),
            Duration::from_millis(200),
            Duration::from_secs(2),
        );
        assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
        let errors: Vec<_> = out.into_iter().map(|r| r.map(|_| ())).collect();
        assert_eq!(
            errors,
            vec![
                Ok(()),
                Err("took longer than 200ms".to_string()),
                Err("took longer than 200ms".to_string()),
                Ok(()),
            ],
            "the icons after a slow one still render"
        );
        assert_reaped(&dir, "hang");
        assert_reaped(&dir, "closed");
    }

    #[test]
    fn the_startup_budget_bounds_the_wait_for_all_icons() {
        let dir = TestDir::new("icons", "child-budget");
        let start = Instant::now();
        let out = render_all(
            &fake_renderer(&dir),
            &fake_jobs(&dir, &["1-hang", "2-hang", "3-hang", "4-hang"]),
            Duration::from_millis(300),
            Duration::from_millis(500),
        );
        assert!(start.elapsed() < Duration::from_millis(800), "{:?}", start.elapsed());
        let errors: Vec<_> = out.into_iter().map(|r| r.map(|_| ())).collect();
        assert_eq!(errors[0], Err("took longer than 300ms".to_string()));
        assert_eq!(errors[1..], vec![Err("no time left at startup".to_string()); 3]);
        assert_reaped(&dir, "1-hang");
        assert_reaped(&dir, "2-hang");
        assert!(
            fs::symlink_metadata(dir.join("3-hang.pid")).is_err(),
            "no renderer started without time left"
        );
    }
    #[test]
    fn refuses_png_bombs_before_decoding() {
        let (_dir, r) = roots("bomb");
        let p = r.thinfloe.join("big.png");
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&100_000u32.to_be_bytes());
        b.extend_from_slice(&100_000u32.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        fs::write(&p, b).unwrap();
        assert_eq!(
            render(&p, 64, [0; 3], false).unwrap_err(),
            "PNG is 100000x100000, larger than 4096x4096"
        );
        assert!(render(Path::new("/dev/zero"), 64, [0; 3], false).is_err());
    }
}
