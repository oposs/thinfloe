//! The Settings info panels: facts as labelled rows ("Driver: iwlwifi"),
//! drawn alike on the Network tab ("Connections now"), the Screen tab
//! ("Graphics") and the This machine tab, which shows them all and copies
//! them as text.

use crate::brand::{self, role};
use crate::ui::{ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{RichText, Ui};
use shadcn_egui::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A labelled value: `Driver` / `iwlwifi`.
    Row { label: String, value: String },
    /// A line without a value: `Reading…`, an error (`warn`).
    Note { text: String, warn: bool },
    /// Lines that belong to one thing: an interface, the video codecs.
    Group { title: String, lines: Vec<Line> },
}

impl Line {
    pub fn row(label: &str, value: impl Into<String>) -> Line {
        Line::Row {
            label: label.to_string(),
            value: value.into(),
        }
    }

    pub fn note(text: impl Into<String>) -> Line {
        Line::Note {
            text: text.into(),
            warn: false,
        }
    }

    pub fn warn(text: impl Into<String>) -> Line {
        Line::Note {
            text: text.into(),
            warn: true,
        }
    }

    pub fn group(title: impl Into<String>, lines: Vec<Line>) -> Line {
        Line::Group {
            title: title.into(),
            lines,
        }
    }
}

/// One panel: a title and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub title: &'static str,
    pub lines: Vec<Line>,
}

/// The clipboard form: each panel's title, then one line per fact
/// (`Label: value`), a group's lines indented by two spaces under its
/// title, and a blank line between panels.
pub fn text(panels: &[Panel]) -> String {
    let mut out = String::new();
    for (i, p) in panels.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(p.title);
        out.push('\n');
        text_lines(&mut out, &p.lines, "");
    }
    out
}

fn text_lines(out: &mut String, lines: &[Line], indent: &str) {
    for line in lines {
        match line {
            Line::Row { label, value } => out.push_str(&format!("{indent}{label}: {value}\n")),
            Line::Note { text, .. } => out.push_str(&format!("{indent}{text}\n")),
            Line::Group { title, lines: inner } => {
                out.push_str(&format!("{indent}{title}\n"));
                text_lines(out, inner, &format!("{indent}  "));
            }
        }
    }
}

/// The label column for `lines`: the widest label in the body font (a
/// group's labels count with their indent), plus the gap before the value.
/// Fitted to the labels, not the form column, which lines up input fields.
pub fn label_width(ui: &Ui, lines: &[Line]) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let widest = widest(ui, &font, lines);
    widest + ui.spacing().item_spacing.x
}

fn widest(ui: &Ui, font: &egui::FontId, lines: &[Line]) -> f32 {
    lines
        .iter()
        .map(|line| match line {
            Line::Row { label, .. } => {
                ui.painter()
                    .layout_no_wrap(label.clone(), font.clone(), egui::Color32::TRANSPARENT)
                    .size()
                    .x
            }
            Line::Note { .. } => 0.0,
            Line::Group { lines, .. } => brand::INFO_INDENT + widest(ui, font, lines),
        })
        .fold(0.0, f32::max)
}

/// The lines in a band's body: a row's label in the foreground colour in a
/// column `label_w` wide, its value muted and cut to fit beside it; a
/// group's title, then its lines indented; a note muted, or in the warning
/// colour.
pub fn draw(ui: &mut Ui, t: &Theme, lines: &[Line], label_w: f32) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = brand::INFO_ROW_GAP;
        // A row is as high as its text, not as a button: nothing in a
        // panel takes a click.
        ui.spacing_mut().interact_size.y = 0.0;
        for line in lines {
            match line {
                Line::Row { label, value } => {
                    ui.horizontal(|ui| {
                        ui.scope(|ui| {
                            ui.set_min_width(label_w);
                            ui.set_max_width(label_w);
                            ui.label(RichText::new(label).color(t.palette().foreground));
                        });
                        ui.add(egui::Label::new(RichText::new(value).color(t.palette().muted_foreground)).truncate());
                    });
                }
                Line::Note { text, warn } => {
                    let color = if *warn {
                        t.role(role::WARN)
                    } else {
                        t.palette().muted_foreground
                    };
                    ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
                }
                Line::Group { title, lines: inner } => {
                    ui.add_space(brand::INFO_GROUP_GAP);
                    ui.label(RichText::new(title).color(t.palette().foreground));
                    ui.horizontal(|ui| {
                        ui.add_space(brand::INFO_INDENT);
                        ui.vertical(|ui| draw(ui, t, inner, (label_w - brand::INFO_INDENT).max(0.0)));
                    });
                }
            }
        }
    });
}

/// A Settings band `width` wide holding `title` and `lines`, as the other
/// bands: card fill, the row margins, the title in the section size.
pub fn band(ui: &mut Ui, t: &Theme, width: f32, title: &str, lines: &[Line]) {
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(width - ROW_MARGIN_X);
            ui.label(
                RichText::new(title)
                    .font(brand::geo(brand::SECTION_PT))
                    .color(t.palette().foreground),
            );
            let w = label_width(ui, lines);
            draw(ui, t, lines, w);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Vec<Panel> {
        vec![
            Panel {
                title: "This machine",
                lines: vec![Line::row("Maker", "HP")],
            },
            Panel {
                title: "Network",
                lines: vec![
                    Line::group(
                        "WiFi wlp58s0",
                        vec![Line::row("State", "connected"), Line::row("Driver", "iwlwifi")],
                    ),
                    Line::row("DNS in use", "192.168.1.1"),
                ],
            },
            Panel {
                title: "Graphics",
                lines: vec![
                    Line::row("GPU", "AMD Picasso"),
                    Line::row("Driver", "amdgpu"),
                    Line::group("Video processing", vec![Line::row("H.264", "decode, encode")]),
                ],
            },
        ]
    }

    #[test]
    fn the_clipboard_text_is_titles_and_label_value_lines() {
        assert_eq!(
            text(&example()),
            "This machine\nMaker: HP\n\n\
             Network\nWiFi wlp58s0\n  State: connected\n  Driver: iwlwifi\nDNS in use: 192.168.1.1\n\n\
             Graphics\nGPU: AMD Picasso\nDriver: amdgpu\nVideo processing\n  H.264: decode, encode\n"
        );
        assert_eq!(text(&[]), "");
        let notes = [Panel {
            title: "Graphics",
            lines: vec![Line::note("Reading…"), Line::warn("vainfo failed")],
        }];
        assert_eq!(text(&notes), "Graphics\nReading…\nvainfo failed\n");
    }

    fn shapes(lines: &[Line]) -> Vec<(String, egui::Rect)> {
        let ctx = egui::Context::default();
        brand::apply(&ctx);
        let mut out = Vec::new();
        for _ in 0..2 {
            let mut o = ctx.run_ui(Default::default(), |ui| {
                let t = Theme::current(ui.ctx());
                band(ui, &t, 800.0, "Network", lines);
            });
            o.textures_delta.clear();
            out = o
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => {
                        let s: String = t.galley.rows.iter().map(|r| r.text()).collect();
                        Some((s, t.galley.rect.translate(t.pos.to_vec2())))
                    }
                    _ => None,
                })
                .collect();
        }
        out
    }

    #[test]
    fn a_row_draws_its_label_and_value_apart_and_values_line_up() {
        let lines = example().swap_remove(1).lines;
        let got = shapes(&lines);
        let at = |s: &str| {
            got.iter()
                .find(|(t, _)| t == s)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("{s} not in {got:?}"))
        };
        // Label and value are two texts on one line, the value to the right.
        let (label, value) = (at("Driver"), at("iwlwifi"));
        assert!(value.left() > label.right(), "{label:?} {value:?}");
        assert!((value.center().y - label.center().y).abs() < 1.0);
        // The group's rows are indented under its title.
        assert!(at("State").left() > at("WiFi wlp58s0").left());
        // Every value starts in one column, the group's rows' too.
        let (a, b, c) = (at("connected"), at("iwlwifi"), at("192.168.1.1"));
        assert!(
            (a.left() - b.left()).abs() < 0.5 && (b.left() - c.left()).abs() < 0.5,
            "{a:?} {b:?} {c:?}"
        );
        // One row follows the next by about a text line, not a button.
        let step = at("Driver").top() - at("State").top();
        assert!(step < 2.0 * at("State").height() + 2.0 * brand::INFO_ROW_GAP, "{step}");
        // Fitted to the labels: far tighter than the 240 pt form column.
        assert!(c.left() - at("DNS in use").left() < brand::FORM_LABEL_W, "{c:?}");
    }
}
