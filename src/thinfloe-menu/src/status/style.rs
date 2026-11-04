//! One design for every status element: the fonts, a fixed set of sizes
//! derived from the cell height, the colours, the paddings and line heights,
//! and the drawing of text lines and graph axes made from them. Labels and
//! titles are in Geo, as the tiles; every number, unit and address is in
//! Hack, egui's monospace font, so columns line up and a value's width is
//! known without measuring. No element sets a font size or a colour of its
//! own.

use super::render::{self, GEO_FAMILY, MONO_FAMILY};
use crate::brand;
use egui::Color32;
use resvg::usvg;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::{Mutex, OnceLock};

/// Hack's advance, 1233 of its 2048 units per em, the same for every glyph.
pub const MONO_ADVANCE: f32 = 1233.0 / 2048.0;

/// The colours of the status elements.
pub const BACKGROUND: Color32 = brand::TILE;
pub const MUTED: Color32 = brand::MUTED;
/// A battery about to run out.
pub const ALERT: Color32 = brand::RED;
/// Something missing that should be there (no address, VPN down).
pub const WARN: Color32 = brand::WARN;
/// Something up that may be down (the VPN).
pub const GOOD: Color32 = brand::GOOD;
/// Received traffic: the filled area of the graph and its numbers.
pub const RX: Color32 = brand::ICE;
/// Sent traffic: the line of the graph and its numbers.
pub const TX: Color32 = brand::GOOD;
pub const GRID: Color32 = brand::GRID;
pub const AXIS: Color32 = brand::AXIS;

/// The text colour of the tiles.
pub fn foreground() -> Color32 {
    brand::theme().palette().foreground
}

/// `#rrggbb` for an SVG attribute.
pub fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Text made safe for an SVG text node or attribute. A character XML does
/// not allow becomes `?`: one in a name from the system (a DNS line, say)
/// would otherwise make the whole SVG unparsable and blank the element.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            '\0'..='\x1f' | '\u{fffe}' | '\u{ffff}' => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

/// The sizes, in points, of everything an element draws. All of them
/// follow the cell height: the set for a 76 pt cell, the set for a 64 pt
/// cell, and in between a value interpolated and rounded to half a point.
/// The menu's scale needs nothing here: the picture is rendered at the
/// element's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// A name or title in Geo (a battery's kind).
    pub title: f32,
    /// A label in Geo beside values (`Peak`, `Gateway`, `Since`): as large as a
    /// title, since Geo's small letters are much smaller than Hack's.
    pub label: f32,
    /// A number, unit or address in Hack.
    pub value: f32,
    /// Axis ticks, unit captions and the legend, in Hack.
    pub small: f32,
    /// One line of titles (the battery's rows).
    pub row: f32,
    /// One line of values.
    pub line: f32,
    /// One line of small text.
    pub small_line: f32,
    /// Left and right of the content, and above and below it.
    pub pad_x: f32,
    pub pad_y: f32,
    /// Between two parts of an element (graph and text, label and value
    /// column).
    pub gap: f32,
}

/// The set for the full tile height, 76 pt.
pub const LARGE: Style = Style {
    title: 15.0,
    label: 15.0,
    value: 12.0,
    small: 10.0,
    row: 16.0,
    line: 16.0,
    small_line: 12.0,
    pad_x: 12.0,
    pad_y: 6.0,
    gap: 6.0,
};

/// The set for the smallest tile height, 64 pt.
pub const SMALL: Style = Style {
    title: 13.0,
    label: 13.0,
    value: 11.0,
    small: 9.0,
    row: 14.0,
    line: 14.0,
    small_line: 11.0,
    pad_x: 10.0,
    pad_y: 5.0,
    gap: 5.0,
};

/// The cell heights the two sets are made for (`ui::TILE_H_MIN`, `ui::TILE_H`).
pub const CELL_SMALL: f32 = 64.0;
pub const CELL_LARGE: f32 = 76.0;

impl Style {
    /// The sizes for cells `cell_h` points high.
    pub fn for_cell(cell_h: f32) -> Style {
        let h = if cell_h.is_finite() { cell_h } else { CELL_LARGE };
        let k = (h.clamp(CELL_SMALL, CELL_LARGE) - CELL_SMALL) / (CELL_LARGE - CELL_SMALL);
        let mix = |a: f32, b: f32| ((a + (b - a) * k) * 2.0).round() / 2.0;
        let (s, l) = (SMALL, LARGE);
        Style {
            title: mix(s.title, l.title),
            label: mix(s.label, l.label),
            value: mix(s.value, l.value),
            small: mix(s.small, l.small),
            row: mix(s.row, l.row),
            line: mix(s.line, l.line),
            small_line: mix(s.small_line, l.small_line),
            pad_x: mix(s.pad_x, l.pad_x),
            pad_y: mix(s.pad_y, l.pad_y),
            gap: mix(s.gap, l.gap),
        }
    }

    /// The space between two runs of one piece of a line.
    pub fn space(&self) -> f32 {
        0.35 * self.value
    }
}

/// Geo's ink from the top of a `k` to the bottom of a `p`, in em.
const GEO_INK: f32 = 0.88;

/// The size of a Geo label in lines `row` apart: `size`, or the half point
/// below it at which a descender no longer reaches the line under it.
pub fn label_in_row(size: f32, row: f32) -> f32 {
    size.min((row / GEO_INK * 2.0).floor() / 2.0)
}

/// Which font a run of text is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Geo: labels and titles.
    Label,
    /// Hack: numbers, units, addresses, names of interfaces.
    Value,
    /// A `↓` or `↑` (the run's text) drawn as a vector arrow in the cell of
    /// one Hack character: Hack's own arrows are thin and only as tall as
    /// its small letters.
    Arrow,
}

impl Face {
    pub fn family(self) -> &'static str {
        match self {
            Face::Label => GEO_FAMILY,
            Face::Value | Face::Arrow => MONO_FAMILY,
        }
    }
}

/// The width of `text` in Hack at `size`: its characters times the advance.
pub fn mono_width(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * MONO_ADVANCE * size
}

/// How many label widths are kept before the cache starts again; the
/// labels are a small fixed set, this only bounds odd ones.
const LABEL_CACHE: usize = 256;

/// The width of `text` in Geo at `size`, measured once by laying it out
/// with resvg (to the right edge of its ink) and kept.
pub fn label_width(text: &str, size: f32) -> f32 {
    static CACHE: OnceLock<Mutex<HashMap<(String, u32), f32>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (text.to_string(), size.to_bits());
    if let Some(w) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return w;
    }
    let w = measure(text, GEO_FAMILY, size);
    if let Ok(mut c) = cache.lock() {
        if c.len() >= LABEL_CACHE {
            c.clear();
        }
        c.insert(key, w);
    }
    w
}

/// The right edge of `text`'s ink in `family` at `size`, drawn from x 0.
pub fn measure(text: &str, family: &str, size: f32) -> f32 {
    if text.trim().is_empty() {
        return 0.0;
    }
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10000\" height=\"100\">\
         <text x=\"0\" y=\"50\" font-family=\"{family}\" font-size=\"{size}\">{}</text></svg>",
        escape(text)
    );
    let right = render::guarded(|| {
        let tree = usvg::Tree::from_str(&svg, &render::options()).map_err(|e| e.to_string())?;
        Ok(tree.root().abs_bounding_box().right().max(0.0))
    });
    // The text is escaped, so never an error. A guess wider than any Geo glyph.
    right.unwrap_or(text.chars().count() as f32 * size)
}

/// The width of `text` in `face` at the face's size in `st`.
pub fn width(face: Face, text: &str, st: &Style) -> f32 {
    match face {
        Face::Label => label_width(text, st.label),
        Face::Value => mono_width(text, st.value),
        Face::Arrow => mono_width("x", st.value),
    }
}

/// One stretch of a line in one font and colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub face: Face,
    pub colour: Color32,
}

pub fn label(text: impl Into<String>, colour: Color32) -> Run {
    Run {
        text: text.into(),
        face: Face::Label,
        colour,
    }
}

pub fn value(text: impl Into<String>, colour: Color32) -> Run {
    Run {
        text: text.into(),
        face: Face::Value,
        colour,
    }
}

pub const DOWN: &str = "↓";
pub const UP: &str = "↑";

/// An arrow, `DOWN` or `UP`.
pub fn arrow(which: &'static str, colour: Color32) -> Run {
    Run {
        text: which.into(),
        face: Face::Arrow,
        colour,
    }
}

/// Append an arrow (`DOWN`, else up) standing on baseline `y` in the cell
/// of one Hack character at `size` starting at `x`: a stem as tall as a
/// capital and an open head, stroked about as heavy as the digits beside
/// it. The group is labelled with the character it stands for.
pub fn draw_arrow(s: &mut String, x: f32, y: f32, size: f32, which: &str, colour: Color32) {
    let a = MONO_ADVANCE * size;
    let cx = a / 2.0;
    let sw = (0.1 * size).max(1.0);
    let (top, bottom) = (-0.72 * size + sw / 2.0, -sw / 2.0);
    let (head, half) = (0.3 * size, 0.38 * a);
    let d = if which == DOWN {
        format!(
            "M{cx:.2},{top:.2}V{bottom:.2}M{:.2},{:.2}L{cx:.2},{bottom:.2}L{:.2},{:.2}",
            cx - half,
            bottom - head,
            cx + half,
            bottom - head
        )
    } else {
        format!(
            "M{cx:.2},{bottom:.2}V{top:.2}M{:.2},{:.2}L{cx:.2},{top:.2}L{:.2},{:.2}",
            cx - half,
            top + head,
            cx + half,
            top + head
        )
    };
    let _ = write!(
        s,
        "<g aria-label=\"{}\" transform=\"translate({x:.2} {y:.2})\"><path d=\"{d}\" fill=\"none\" stroke=\"{}\" \
         stroke-width=\"{sw:.2}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></g>",
        escape(which),
        hex(colour),
    );
}

/// The width of `draw_battery`'s symbol, nub included, for values at `size`.
pub fn battery_width(size: f32) -> f32 {
    (2.6 * size).round() + BATTERY_NUB
}

const BATTERY_NUB: f32 = 2.0;

/// Append a battery symbol from `x`, standing on baseline `y`, for values at
/// `size`: an outline with a nub on the right, filled to `level` percent. No
/// level (not reachable, no percent) is a dashed empty outline, which reads
/// differently from a solid empty one at 0 %. On whole points, so its edges
/// are sharp at whole scales. The group is labelled with the level.
pub fn draw_battery(s: &mut String, x: f32, y: f32, size: f32, level: Option<u8>, colour: Color32) {
    let c = hex(colour);
    let w = (2.6 * size).round();
    let h = (0.8 * size).round();
    // A one-point outline on half points; its fill one point inside it.
    let (x0, y0) = (x.floor() + 0.5, (y - h).round() + 0.5);
    let (body_w, body_h) = (w - 1.0, h - 1.0);
    let name = level.map_or("unknown".to_string(), |l| l.to_string());
    let dash = if level.is_none() {
        " stroke-dasharray=\"2 2\""
    } else {
        ""
    };
    let _ = write!(
        s,
        "<g id=\"symbol-{id}\" class=\"battery\" aria-label=\"{name}\">\
         <rect class=\"outline\" x=\"{x0}\" y=\"{y0}\" width=\"{body_w}\" height=\"{body_h}\" fill=\"none\" stroke=\"{c}\" stroke-width=\"1\"{dash}/>\
         <rect class=\"nub\" x=\"{nx}\" y=\"{ny}\" width=\"{BATTERY_NUB}\" height=\"{nh}\" fill=\"{c}\"/>",
        id = y.round(),
        nx = x0 + body_w + 0.5,
        ny = y0 + (body_h / 4.0).round(),
        nh = (body_h / 2.0).round(),
    );
    let inner = body_w - 3.0;
    if let Some(l) = level.filter(|l| *l > 0) {
        let fill = (inner * f32::from(l.min(100)) / 100.0).round().max(1.0);
        let _ = write!(
            s,
            "<rect class=\"level\" x=\"{}\" y=\"{}\" width=\"{fill}\" height=\"{}\" fill=\"{c}\"/>",
            x0 + 1.5,
            y0 + 1.5,
            body_h - 3.0,
        );
    }
    s.push_str("</g>");
}

/// One item of a line (`max 11.2 Mbit/s`): runs drawn a space apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub runs: Vec<Run>,
}

impl Piece {
    pub fn width(&self, st: &Style) -> f32 {
        let gaps = self.runs.len().saturating_sub(1) as f32 * st.space();
        self.runs.iter().map(|r| width(r.face, &r.text, st)).sum::<f32>() + gaps
    }
}

/// Where a text's `x` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// Append one `<text>` in `face` at `size`.
#[allow(clippy::too_many_arguments)]
pub fn text(s: &mut String, x: f32, y: f32, face: Face, size: f32, colour: Color32, anchor: Anchor, body: &str) {
    let anchor = match anchor {
        Anchor::Start => "",
        Anchor::Middle => " text-anchor=\"middle\"",
        Anchor::End => " text-anchor=\"end\"",
    };
    let _ = write!(
        s,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" font-family=\"{}\" font-size=\"{size}\" fill=\"{}\"{anchor}>{}</text>",
        face.family(),
        hex(colour),
        escape(body),
    );
}

/// Append `piece` as one line from `x`, baseline `y`, each run its own
/// `<text>` at a computed place, so the fonts never mix inside one text.
pub fn draw_line(s: &mut String, x: f32, y: f32, piece: &Piece, st: &Style) {
    let mut at = x;
    for (i, r) in piece.runs.iter().enumerate() {
        if i > 0 {
            at += st.space();
        }
        match r.face {
            Face::Label => text(s, at, y, r.face, st.label, r.colour, Anchor::Start, &r.text),
            Face::Value => text(s, at, y, r.face, st.value, r.colour, Anchor::Start, &r.text),
            Face::Arrow => draw_arrow(s, at, y, st.value, &r.text, r.colour),
        }
        at += width(r.face, &r.text, st);
    }
}

/// The baseline of a line `line` high whose top is `top`: the text's
/// x-height sits in the middle of the line.
pub fn baseline(top: f32, line: f32, size: f32) -> f32 {
    top + line / 2.0 + 0.36 * size
}

/// A round coordinate for a line one point wide: on the middle of a pixel
/// at scale 1, on a pixel edge pair at scale 2, so it is drawn sharp.
pub fn crisp(v: f32) -> f32 {
    v.floor() + 0.5
}

/// Rate ticks for a graph whose highest value is `peak` bit/s: 0 and up to
/// three equal steps of 1, 2 or 5 times a power of ten, the last at or
/// above the peak (and at least 1 kbit/s, at most 1 Tbit/s). Two to four
/// values; the last one is the top of the graph.
pub fn rate_ticks(peak: f64) -> Vec<f64> {
    let top = if peak.is_finite() { peak } else { 0.0 }.clamp(MIN_TOP, MAX_TOP);
    let mut exp = 0i32;
    loop {
        let unit = 10f64.powi(exp);
        for m in [1.0, 2.0, 5.0] {
            let step = m * unit;
            let n = (top / step - 1e-9).ceil().max(1.0);
            if n <= 3.0 {
                return (0..=n as usize).map(|i| i as f64 * step).collect();
            }
        }
        exp += 1;
    }
}

/// The lowest and highest top a rate graph gets.
pub const MIN_TOP: f64 = 1000.0;
pub const MAX_TOP: f64 = 1e12;

/// A tick value with k, M, G or T and no unit, as short as it is exact:
/// `0`, `500`, `1k`, `1.5k`, `15M`.
pub fn tick_label(v: f64) -> String {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    let mut x = v;
    let mut prefix = "";
    for p in ["k", "M", "G", "T"] {
        if x < 1000.0 {
            break;
        }
        x /= 1000.0;
        prefix = p;
    }
    let text = format!("{x:.1}");
    format!("{}{prefix}", text.trim_end_matches('0').trim_end_matches('.'))
}

/// Time ticks of a five-minute graph from `x0` to `x0 + w`, newest at the
/// right: one a minute, `-5m` at the left edge and `now` at the right.
pub fn minute_ticks(x0: f32, w: f32) -> Vec<(f32, &'static str)> {
    const LABELS: [&str; 6] = ["-5m", "-4m", "-3m", "-2m", "-1m", "now"];
    LABELS
        .iter()
        .enumerate()
        .map(|(i, l)| (x0 + w * i as f32 / 5.0, *l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_xml_refuses_become_a_question_mark() {
        assert_eq!(escape("1.1.1.1\x01"), "1.1.1.1?");
        for c in ['\0', '\x08', '\x0b', '\x0c', '\x1f', '\u{fffe}', '\u{ffff}'] {
            assert_eq!(escape(&format!("a{c}b")), "a?b", "{c:?}");
        }
        // What XML allows stays.
        let fine = "a\tb\nc\rd \u{fffd} \u{e000} \u{10000} é";
        assert_eq!(escape(fine), fine);
        assert_eq!(escape("<\x01>"), "&lt;?&gt;");
    }

    #[test]
    fn the_sizes_follow_the_cell_height() {
        assert_eq!(Style::for_cell(76.0), LARGE);
        assert_eq!(Style::for_cell(64.0), SMALL);
        // Outside the tiles' range the nearest set.
        assert_eq!(Style::for_cell(200.0), LARGE);
        assert_eq!(Style::for_cell(10.0), SMALL);
        assert_eq!(Style::for_cell(f32::NAN), LARGE);
        // In between: between the two, on half points, never shrinking as
        // the cell grows.
        let mut last = Style::for_cell(64.0);
        for tenth in 641..=760 {
            let st = Style::for_cell(tenth as f32 / 10.0);
            for (a, b, name) in [
                (last.title, st.title, "title"),
                (last.value, st.value, "value"),
                (last.small, st.small, "small"),
                (last.row, st.row, "row"),
                (last.line, st.line, "line"),
                (last.pad_x, st.pad_x, "pad_x"),
            ] {
                assert!(b >= a, "{name} shrinks at {tenth}");
                assert_eq!(b * 2.0, (b * 2.0).round(), "{name} on half points");
            }
            last = st;
        }
        let mid = Style::for_cell(70.0);
        assert_eq!((mid.title, mid.value, mid.small), (14.0, 11.5, 9.5));
        // The hierarchy holds at every height: title over label over value
        // over small, and each fits its line.
        for h in [64.0, 70.0, 76.0] {
            let st = Style::for_cell(h);
            assert!(
                st.title >= st.label && st.label > st.value && st.value > st.small,
                "{st:?}"
            );
            assert!(st.row >= st.title && st.line >= st.value + 2.0 && st.small_line >= st.small + 1.0);
        }
    }

    #[test]
    fn hack_is_monospace_at_the_advance_used() {
        // Ten more characters are ten more advances.
        for size in [9.0, 12.0] {
            let ten = measure("0000000000", MONO_FAMILY, size);
            let twenty = measure("00000000000000000000", MONO_FAMILY, size);
            let advance = (twenty - ten) / 10.0;
            assert!((advance - MONO_ADVANCE * size).abs() < 0.05, "{advance} at {size}");
            // Digits, letters, arrows and the dot are one width.
            let mixed = measure("0↓↑·%/0", MONO_FAMILY, size) - measure("0", MONO_FAMILY, size);
            assert!((mixed - 6.0 * MONO_ADVANCE * size).abs() < 0.6, "{mixed} at {size}");
        }
        assert_eq!(mono_width("100 %", 10.0), 5.0 * MONO_ADVANCE * 10.0);
        assert_eq!(mono_width("↓", 10.0), MONO_ADVANCE * 10.0);
    }

    #[test]
    fn a_geo_label_is_measured_and_kept() {
        let w = label_width("since", 13.0);
        assert!(w > 13.0 && w < mono_width("since", 13.0) * 1.2, "{w}");
        assert_eq!(label_width("since", 13.0), w, "the same width again");
        assert!(label_width("since", 26.0) > 1.9 * w);
        assert_eq!(label_width("", 13.0), 0.0);
        assert_eq!(label_width("   ", 13.0), 0.0);
        // Text that needs escaping is measured, not refused.
        assert!(label_width("<&>", 13.0) > 0.0);
    }

    #[test]
    fn rate_ticks_are_nice_numbers_up_to_the_peak() {
        let k = |v: &[f64]| v.iter().map(|x| tick_label(*x)).collect::<Vec<_>>();
        // Nothing, or very little: the floor of 1 kbit/s.
        for quiet in [0.0, 1.0, 999.0, 1000.0, -5.0, f64::NAN, f64::INFINITY] {
            assert_eq!(k(&rate_ticks(quiet)), ["0", "500", "1k"], "{quiet}");
        }
        assert_eq!(k(&rate_ticks(1001.0)), ["0", "500", "1k", "1.5k"]);
        assert_eq!(k(&rate_ticks(4.0e6)), ["0", "2M", "4M"]);
        assert_eq!(k(&rate_ticks(4.1e6)), ["0", "2M", "4M", "6M"]);
        assert_eq!(k(&rate_ticks(11.2e6)), ["0", "5M", "10M", "15M"]);
        assert_eq!(k(&rate_ticks(25e6)), ["0", "10M", "20M", "30M"]);
        assert_eq!(k(&rate_ticks(940e6)), ["0", "500M", "1G"]);
        assert_eq!(k(&rate_ticks(1e15)), ["0", "500G", "1T"], "capped");
        // Every peak: 2 to 4 values from 0, equal steps, the top at or
        // above the peak.
        let mut p = 1.0;
        while p < 2e12 {
            let t = rate_ticks(p);
            assert!((2..=4).contains(&t.len()), "{p}: {t:?}");
            assert_eq!(t[0], 0.0);
            let step = t[1];
            for w in t.windows(2) {
                assert!((w[1] - w[0] - step).abs() < step * 1e-9, "{p}: {t:?}");
            }
            let top = *t.last().unwrap();
            assert!(top >= p.clamp(MIN_TOP, MAX_TOP) * (1.0 - 1e-9), "{p}: {t:?}");
            p *= 1.37;
        }
    }

    #[test]
    fn tick_labels_are_short_and_exact() {
        for (v, want) in [
            (0.0, "0"),
            (500.0, "500"),
            (1000.0, "1k"),
            (1500.0, "1.5k"),
            (2e6, "2M"),
            (15e6, "15M"),
            (500e6, "500M"),
            (1e9, "1G"),
            (1e12, "1T"),
            (-1.0, "0"),
        ] {
            assert_eq!(tick_label(v), want, "{v}");
        }
    }

    #[test]
    fn minute_ticks_span_the_graph() {
        let t = minute_ticks(40.0, 500.0);
        let labels: Vec<_> = t.iter().map(|(_, l)| *l).collect();
        assert_eq!(labels, ["-5m", "-4m", "-3m", "-2m", "-1m", "now"]);
        assert_eq!(t[0].0, 40.0);
        assert_eq!(t[5].0, 540.0);
        for w in t.windows(2) {
            assert!((w[1].0 - w[0].0 - 100.0).abs() < 1e-3);
        }
    }

    #[test]
    fn a_line_draws_each_run_in_its_font_where_measured() {
        let st = LARGE;
        let piece = Piece {
            runs: vec![
                value("↓ 4.00 Mbit/s", RX),
                label("max", MUTED),
                value("1 <M>", foreground()),
            ],
        };
        let mut s = String::new();
        draw_line(&mut s, 10.0, 20.0, &piece, &st);
        let wrapped = format!("<g>{s}</g>");
        let doc = resvg::usvg::roxmltree::Document::parse(&wrapped).unwrap();
        let t: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("text")).collect();
        assert_eq!(t.len(), 3);
        let x = |i: usize| t[i].attribute("x").unwrap().parse::<f32>().unwrap();
        assert_eq!(t[0].attribute("font-family"), Some(MONO_FAMILY));
        assert_eq!(t[1].attribute("font-family"), Some(GEO_FAMILY));
        assert_eq!(t[2].text(), Some("1 <M>"), "escaped in the source");
        assert_eq!(x(0), 10.0);
        let x1 = 10.0 + mono_width("↓ 4.00 Mbit/s", st.value) + st.space();
        assert!((x(1) - x1).abs() < 0.01);
        assert!((x(2) - (x1 + label_width("max", st.label) + st.space())).abs() < 0.01);
        assert_eq!(t[0].attribute("fill"), Some(hex(RX).as_str()));
    }

    #[test]
    fn an_arrow_fills_one_character_cell_and_points_its_way() {
        let size = 40.0;
        let a = MONO_ADVANCE * size;
        for (which, head_low) in [(DOWN, true), (UP, false)] {
            let mut s = String::from(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"60\" height=\"50\"><rect width=\"60\" height=\"50\" fill=\"#000\"/>",
            );
            draw_arrow(&mut s, 10.0, 40.0, size, which, Color32::WHITE);
            s.push_str("</svg>");
            assert!(s.contains(&format!("aria-label=\"{which}\"")));
            let img = render::render(&s, [60, 50]).unwrap();
            let ink = |x: usize, y: usize| img.pixels[y * 60 + x].r() > 100;
            let rows: Vec<usize> = (0..50).filter(|&y| (0..60).any(|x| ink(x, y))).collect();
            let cols: Vec<usize> = (0..60).filter(|&x| (0..50).any(|y| ink(x, y))).collect();
            // Inside its cell, standing on the baseline, as tall as a capital.
            assert!(cols[0] >= 10 && *cols.last().unwrap() as f32 <= 10.0 + a, "{cols:?}");
            assert!(*rows.last().unwrap() <= 40 && *rows.last().unwrap() >= 37, "{rows:?}");
            let tall = rows.len() as f32;
            assert!(tall > 0.6 * size && tall < 0.8 * size, "{tall}");
            // The head is the widest part, at the end it points to.
            let width = |y: usize| (0..60).filter(|&x| ink(x, y)).count();
            let widest = rows.iter().copied().max_by_key(|&y| (width(y), y)).unwrap();
            let middle = (rows[0] + rows.last().unwrap()) / 2;
            assert_eq!(widest > middle, head_low, "{which}: widest row {widest}");
        }
        // In a line it takes one character's width.
        let st = LARGE;
        let p = Piece {
            runs: vec![arrow(DOWN, RX), value("4", RX)],
        };
        assert!((p.width(&st) - (2.0 * mono_width("x", st.value) + st.space())).abs() < 1e-3);
        let mut s = String::new();
        draw_line(&mut s, 5.0, 20.0, &p, &st);
        assert!(
            s.starts_with("<g aria-label=\"↓\" transform=\"translate(5.00 20.00)\">"),
            "{s}"
        );
    }

    #[test]
    fn a_battery_symbol_stays_in_its_width_on_whole_points() {
        for (size, level) in [(12.0, Some(100)), (11.0, Some(100)), (11.5, Some(37)), (12.0, None)] {
            let width = battery_width(size);
            let mut s = String::from(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\"><rect width=\"80\" height=\"40\" fill=\"#000\"/>",
            );
            draw_battery(&mut s, 10.0, 30.0, size, level, Color32::WHITE);
            s.push_str("</svg>");
            let img = render::render(&s, [80, 40]).unwrap();
            let ink: Vec<(usize, usize)> = (0..40)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .filter(|&(x, y)| img.pixels[y * 80 + x].r() > 40)
                .collect();
            let (l, r) = (
                ink.iter().map(|p| p.0).min().unwrap(),
                ink.iter().map(|p| p.0).max().unwrap(),
            );
            let b = ink.iter().map(|p| p.1).max().unwrap();
            assert_eq!(l, 10, "{size}: starts at x");
            assert_eq!(r as f32 + 1.0, 10.0 + width, "{size}: ends at its width");
            assert!(b < 30, "{size}: stands on the baseline: {b}");
        }
        // Full and empty are told apart inside the outline; the middle row
        // of a full one is all ink.
        let mid = |level| {
            let mut s = String::from(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\"><rect width=\"80\" height=\"40\" fill=\"#000\"/>",
            );
            draw_battery(&mut s, 10.0, 30.0, 12.0, level, Color32::WHITE);
            s.push_str("</svg>");
            let img = render::render(&s, [80, 40]).unwrap();
            (0..80).filter(|x| img.pixels[25 * 80 + x].r() > 200).count()
        };
        assert!(mid(Some(100)) > 25);
        assert_eq!(
            mid(Some(0)),
            4,
            "an empty one: the two sides of the outline and the nub"
        );
        assert!(mid(Some(50)) > mid(Some(0)) + 8 && mid(Some(50)) < mid(Some(100)) - 8);
    }

    #[test]
    fn crisp_lines_sit_on_half_points() {
        assert_eq!(crisp(10.0), 10.5);
        assert_eq!(crisp(10.9), 10.5);
        assert_eq!(baseline(0.0, 16.0, 10.0), 8.0 + 3.6);
    }
}
