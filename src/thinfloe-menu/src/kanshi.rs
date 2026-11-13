//! A monitor layout and the kanshi profile text the menu writes for it.
//!
//! The layout is a tree: one output sits at the origin and every other one
//! is placed beside another. Left and right align the tops, above and below
//! align the left edges. Positions are derived, never stored; scale is 1 and
//! nothing is mirrored. A monitor is named as kanshi names it: make, model
//! and serial, or the output name when the serial is missing or collides.

use crate::heads::{Head, Mode, Rotation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    LeftOf,
    RightOf,
    Above,
    Below,
}

impl Side {
    fn word(self) -> &'static str {
        match self {
            Side::LeftOf => "left-of",
            Side::RightOf => "right-of",
            Side::Above => "above",
            Side::Below => "below",
        }
    }

    fn from_word(w: &str) -> Option<Side> {
        Some(match w {
            "left-of" => Side::LeftOf,
            "right-of" => Side::RightOf,
            "above" => Side::Above,
            "below" => Side::Below,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Origin,
    /// Beside the output with criteria `to`.
    Next {
        to: String,
        side: Side,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputSet {
    pub criteria: String,
    pub enabled: bool,
    pub mode: Mode,
    pub rotation: Rotation,
    pub place: Place,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub outputs: Vec<OutputSet>,
    /// Criteria of the monitor that carries the menu.
    pub menu_output: Option<String>,
}

/// How kanshi identifies `head`: "<make> <model> <serial>", or the output
/// name when the serial is empty or another connected head gives the same
/// string.
pub fn criteria(head: &Head, all: &[Head]) -> String {
    let text = |h: &Head| {
        let or_unknown = |s: &str| {
            if s.is_empty() {
                "Unknown".to_string()
            } else {
                s.to_string()
            }
        };
        format!("{} {} {}", or_unknown(&h.make), or_unknown(&h.model), h.serial)
    };
    let mine = text(head);
    let clash = all.iter().filter(|h| text(h) == mine).count() > 1;
    if head.serial.is_empty() || clash {
        head.name.clone()
    } else {
        mine
    }
}

/// The size `o` takes on the desktop: its mode, turned by its rotation.
pub fn size(o: &OutputSet) -> (i32, i32) {
    let (w, h) = (o.mode.w as i32, o.mode.h as i32);
    match o.rotation {
        Rotation::R90 | Rotation::R270 => (h, w),
        _ => (w, h),
    }
}

fn index_of(p: &Profile, criteria: &str) -> Result<usize, String> {
    p.outputs
        .iter()
        .position(|o| o.criteria == criteria)
        .ok_or_else(|| format!("No monitor {criteria} in this arrangement."))
}

fn absolute(p: &Profile, i: usize, cache: &mut [Option<(i32, i32)>], depth: usize) -> Result<(i32, i32), String> {
    if let Some(pos) = cache[i] {
        return Ok(pos);
    }
    if depth > p.outputs.len() {
        return Err("The monitors are placed in a circle.".into());
    }
    let pos = match &p.outputs[i].place {
        Place::Origin => (0, 0),
        Place::Next { to, side } => {
            // A disabled anchor passes its own anchor on.
            let mut t = index_of(p, to)?;
            let mut hops = 0;
            while !p.outputs[t].enabled {
                hops += 1;
                match &p.outputs[t].place {
                    Place::Origin => break,
                    Place::Next { to, .. } if hops <= p.outputs.len() => t = index_of(p, to)?,
                    Place::Next { .. } => return Err("The monitors are placed in a circle.".into()),
                }
            }
            // A disabled origin is a zero-size anchor at 0,0.
            let ((tx, ty), (tw, th)) = if p.outputs[t].enabled {
                (absolute(p, t, cache, depth + 1)?, size(&p.outputs[t]))
            } else {
                ((0, 0), (0, 0))
            };
            let (w, h) = size(&p.outputs[i]);
            match side {
                Side::RightOf => (tx + tw, ty),
                Side::LeftOf => (tx - w, ty),
                Side::Below => (tx, ty + th),
                Side::Above => (tx, ty - h),
            }
        }
    };
    cache[i] = Some(pos);
    Ok(pos)
}

/// Positions without the overlap check, one per output (disabled: `None`),
/// shifted so the smallest x and y are 0. The Screen tab draws from it.
pub fn place_all(p: &Profile) -> Result<Vec<Option<(i32, i32)>>, String> {
    let mut cache = vec![None; p.outputs.len()];
    let mut out = vec![None; p.outputs.len()];
    for (i, o) in p.outputs.iter().enumerate() {
        if o.enabled {
            out[i] = Some(absolute(p, i, &mut cache, 0)?);
        }
    }
    let min_x = out.iter().flatten().map(|q| q.0).min().unwrap_or(0);
    let min_y = out.iter().flatten().map(|q| q.1).min().unwrap_or(0);
    for q in out.iter_mut().flatten() {
        *q = (q.0 - min_x, q.1 - min_y);
    }
    Ok(out)
}

/// Absolute positions of the enabled outputs, in the order of
/// `p.outputs` (disabled ones have none), with the smallest x and y at 0.
pub fn positions(p: &Profile) -> Result<Vec<(i32, i32)>, String> {
    let placed = place_all(p)?;
    for (i, a) in placed.iter().enumerate() {
        for (j, b) in placed.iter().enumerate().skip(i + 1) {
            let (Some(a), Some(b)) = (a, b) else { continue };
            let (sa, sb) = (size(&p.outputs[i]), size(&p.outputs[j]));
            if a.0 < b.0 + sb.0 && b.0 < a.0 + sa.0 && a.1 < b.1 + sb.1 && b.1 < a.1 + sa.1 {
                return Err(format!(
                    "{} and {} overlap.",
                    p.outputs[i].criteria, p.outputs[j].criteria
                ));
            }
        }
    }
    Ok(placed.into_iter().flatten().collect())
}

fn depends_on(p: &Profile, start: &str, who: &str) -> bool {
    let mut cur = start;
    for _ in 0..=p.outputs.len() {
        if cur == who {
            return true;
        }
        match p.outputs.iter().find(|o| o.criteria == cur).map(|o| &o.place) {
            Some(Place::Next { to, .. }) => cur = to,
            _ => return false,
        }
    }
    false
}

/// Give `who` a new place. When the target depends on `who`, the target
/// takes over `who`'s old place first, so the layout stays a tree.
pub fn set_place(p: &mut Profile, who: &str, place: Place) {
    if let Place::Next { to, .. } = &place {
        if to == who {
            return;
        }
        if depends_on(p, to, who) {
            let old = p.outputs.iter().find(|o| o.criteria == who).map(|o| o.place.clone());
            if let (Some(old), Some(t)) = (old, p.outputs.iter_mut().find(|o| &o.criteria == to)) {
                t.place = old;
            }
        }
    }
    if let Some(o) = p.outputs.iter_mut().find(|o| o.criteria == who) {
        o.place = place;
    }
}

/// kanshi's mode text. Headless outputs report refresh 0 and take no rate.
pub fn mode_text(m: &Mode) -> String {
    if m.refresh_mhz == 0 {
        format!("{}x{}", m.w, m.h)
    } else {
        format!("{}x{}@{}.{:03}Hz", m.w, m.h, m.refresh_mhz / 1000, m.refresh_mhz % 1000)
    }
}

fn transform_word(r: Rotation) -> &'static str {
    match r {
        Rotation::Normal => "normal",
        Rotation::R90 => "90",
        Rotation::R180 => "180",
        Rotation::R270 => "270",
    }
}

fn quoted(criteria: &str) -> String {
    if criteria.contains(' ') {
        format!("\"{criteria}\"")
    } else {
        criteria.to_string()
    }
}

/// The body of the file (`owned::write` adds the header): one kanshi
/// profile per entry. Callers check `positions` first; a layout that
/// cannot be placed renders its outputs at 0,0.
pub fn render(profiles: &[Profile]) -> String {
    let blocks: Vec<String> = profiles
        .iter()
        .map(|p| {
            let placed = place_all(p).unwrap_or_else(|_| vec![Some((0, 0)); p.outputs.len()]);
            let mut s = String::from("profile {\n");
            for (o, pos) in p.outputs.iter().zip(&placed) {
                match pos {
                    Some((x, y)) if o.enabled => s.push_str(&format!(
                        "\toutput {} enable mode {} position {x},{y} transform {} scale 1\n",
                        quoted(&o.criteria),
                        mode_text(&o.mode),
                        transform_word(o.rotation)
                    )),
                    _ => s.push_str(&format!("\toutput {} disable\n", quoted(&o.criteria))),
                }
            }
            for o in &p.outputs {
                if let Place::Next { to, side } = &o.place {
                    s.push_str(&format!(
                        "\t# thinfloe place {} {} {}\n",
                        quoted(&o.criteria),
                        side.word(),
                        quoted(to)
                    ));
                }
            }
            if let Some(m) = &p.menu_output {
                s.push_str(&format!("\t# thinfloe menu {}\n", quoted(m)));
            }
            s.push_str("}\n");
            s
        })
        .collect();
    blocks.join("\n")
}

fn tokens(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut in_quote, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                any = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if in_quote {
        return Err(format!("Unclosed quote in: {line}"));
    }
    if any {
        out.push(cur);
    }
    Ok(out)
}

fn bad<T>(line: &str) -> Result<T, String> {
    Err(format!("Not a line the menu wrote: {line}"))
}

/// The mode from kanshi's text (`WxH` or `WxH@R.RRRHz`); `preferred` is false.
pub fn parse_mode(text: &str) -> Option<Mode> {
    let (size, rate) = match text.split_once('@') {
        Some((s, r)) => (s, Some(r)),
        None => (text, None),
    };
    let (w, h) = size.split_once('x')?;
    let refresh_mhz = match rate {
        None => 0,
        Some(r) => {
            let (hz, mhz) = r.strip_suffix("Hz")?.split_once('.')?;
            hz.parse::<u32>().ok()? * 1000 + mhz.parse::<u32>().ok()?
        }
    };
    Some(Mode {
        w: w.parse().ok()?,
        h: h.parse().ok()?,
        refresh_mhz,
        preferred: false,
    })
}

/// A disabled output keeps no mode in the file; it reads back as 0x0.
fn parse_output(t: &[String], line: &str) -> Result<OutputSet, String> {
    let make = |enabled, mode, rotation| OutputSet {
        criteria: t[1].clone(),
        enabled,
        mode,
        rotation,
        place: Place::Origin,
    };
    let words: Vec<&str> = t.iter().map(String::as_str).collect();
    match words[..] {
        [_, _, "disable"] => Ok(make(
            false,
            Mode {
                w: 0,
                h: 0,
                refresh_mhz: 0,
                preferred: false,
            },
            Rotation::Normal,
        )),
        [
            _,
            _,
            "enable",
            "mode",
            mode,
            "position",
            _,
            "transform",
            rot,
            "scale",
            "1",
        ] => {
            let rotation = match rot {
                "normal" => Rotation::Normal,
                "90" => Rotation::R90,
                "180" => Rotation::R180,
                "270" => Rotation::R270,
                _ => return bad(line),
            };
            match parse_mode(mode) {
                Some(mode) => Ok(make(true, mode, rotation)),
                None => bad(line),
            }
        }
        _ => bad(line),
    }
}

/// Read the menu's own file (`render` output with the header). Anything
/// else, a hand-written file included, is an error. Positions in the file
/// are not read; they follow from the place lines.
pub fn parse(text: &str) -> Result<Vec<Profile>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(crate::owned::HEADER) {
        return Err("Not a file the menu wrote.".into());
    }
    let mut profiles = Vec::new();
    let mut cur: Option<Profile> = None;
    for raw in lines {
        let line = raw.trim_end();
        let t = tokens(line)?;
        let words: Vec<&str> = t.iter().map(String::as_str).collect();
        let Some(p) = cur.as_mut() else {
            match words[..] {
                [] => {}
                ["profile", "{"] => {
                    cur = Some(Profile {
                        outputs: Vec::new(),
                        menu_output: None,
                    })
                }
                _ => return bad(line),
            }
            continue;
        };
        match words[..] {
            [] => {}
            ["}"] => profiles.extend(cur.take()),
            ["output", ..] => p.outputs.push(parse_output(&t, line)?),
            ["#", "thinfloe", "place", who, side, to] => {
                let Some(side) = Side::from_word(side) else {
                    return bad(line);
                };
                match p.outputs.iter_mut().find(|o| o.criteria == who) {
                    Some(o) => {
                        o.place = Place::Next {
                            to: to.to_string(),
                            side,
                        }
                    }
                    None => return bad(line),
                }
            }
            ["#", "thinfloe", "menu", who] => p.menu_output = Some(who.to_string()),
            _ => return bad(line),
        }
    }
    if cur.is_some() {
        return Err("A profile is not closed.".into());
    }
    Ok(profiles)
}

/// The profile whose monitors are exactly the connected ones.
pub fn profile_for(profiles: &[Profile], heads: &[Head]) -> Option<usize> {
    let mut connected: Vec<String> = heads.iter().map(|h| criteria(h, heads)).collect();
    connected.sort();
    profiles.iter().position(|p| {
        let mut mine: Vec<&String> = p.outputs.iter().map(|o| &o.criteria).collect();
        mine.sort();
        mine.len() == connected.len() && mine.into_iter().zip(&connected).all(|(a, b)| a == b)
    })
}

/// The arrangement as it is now. The first enabled head by x then y is the
/// origin; each other one sits beside an earlier one it touches (right of
/// before below), else right of the previous. Disabled heads come last.
pub fn from_heads(heads: &[Head]) -> Profile {
    let chosen = |h: &Head| {
        h.current
            .and_then(|i| h.modes.get(i))
            .or_else(|| h.modes.iter().find(|m| m.preferred))
            .or_else(|| h.modes.first())
            .cloned()
            .unwrap_or(Mode {
                w: 0,
                h: 0,
                refresh_mhz: 0,
                preferred: false,
            })
    };
    let mut order: Vec<&Head> = heads.iter().filter(|h| h.enabled).collect();
    order.sort_by_key(|h| (h.pos.0, h.pos.1));
    order.extend(heads.iter().filter(|h| !h.enabled));

    let mut outputs: Vec<OutputSet> = Vec::new();
    // x, y, w, h of the enabled heads placed so far; they come first in `outputs`.
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
    for h in order {
        let mut o = OutputSet {
            criteria: criteria(h, heads),
            enabled: h.enabled,
            mode: chosen(h),
            rotation: h.rotation,
            place: Place::Origin,
        };
        let (w, hh) = size(&o);
        if h.enabled {
            let (x, y) = h.pos;
            let right = rects
                .iter()
                .position(|r| x == r.0 + r.2 && y < r.1 + r.3 && r.1 < y + hh);
            let below = rects
                .iter()
                .position(|r| y == r.1 + r.3 && x < r.0 + r.2 && r.0 < x + w);
            let found = right.map(|i| (i, Side::RightOf)).or(below.map(|i| (i, Side::Below)));
            let prev = rects.len().checked_sub(1).map(|i| (i, Side::RightOf));
            if let Some((i, side)) = found.or(prev) {
                o.place = Place::Next {
                    to: outputs[i].criteria.clone(),
                    side,
                };
            }
            rects.push((x, y, w, hh));
        } else if let Some(last) = outputs.last() {
            o.place = Place::Next {
                to: last.criteria.clone(),
                side: Side::RightOf,
            };
        }
        outputs.push(o);
    }
    Profile {
        outputs,
        menu_output: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: u32, h: u32, mhz: u32) -> Mode {
        Mode {
            w,
            h,
            refresh_mhz: mhz,
            preferred: false,
        }
    }

    fn set(c: &str, w: u32, h: u32, place: Place) -> OutputSet {
        OutputSet {
            criteria: c.into(),
            enabled: true,
            mode: mode(w, h, 60_000),
            rotation: Rotation::Normal,
            place,
        }
    }

    fn next(to: &str, side: Side) -> Place {
        Place::Next { to: to.into(), side }
    }

    fn profile(outputs: Vec<OutputSet>) -> Profile {
        Profile {
            outputs,
            menu_output: None,
        }
    }

    fn head(name: &str, make: &str, model: &str, serial: &str, pos: (i32, i32), w: u32, h: u32) -> Head {
        Head {
            name: name.into(),
            make: make.into(),
            model: model.into(),
            serial: serial.into(),
            phys_mm: (0, 0),
            enabled: true,
            modes: vec![mode(w, h, 60_000)],
            current: Some(0),
            pos,
            rotation: Rotation::Normal,
        }
    }

    #[test]
    fn renders_the_runtime_fixture() {
        let p = Profile {
            outputs: vec![
                OutputSet {
                    mode: mode(1280, 720, 0),
                    ..set("HEADLESS-1", 0, 0, Place::Origin)
                },
                OutputSet {
                    mode: mode(1280, 720, 0),
                    ..set("HEADLESS-2", 0, 0, next("HEADLESS-1", Side::RightOf))
                },
            ],
            menu_output: Some("HEADLESS-1".into()),
        };
        let text = format!("{}\n\n{}", crate::owned::HEADER, render(&[p]));
        assert_eq!(text, include_str!("../../../test/runtime/kanshi-two.conf"));
    }

    #[test]
    fn mode_text_keeps_millihertz() {
        assert_eq!(mode_text(&mode(1920, 1080, 59_940)), "1920x1080@59.940Hz");
        assert_eq!(mode_text(&mode(1920, 1080, 60_000)), "1920x1080@60.000Hz");
        assert_eq!(mode_text(&mode(1280, 720, 0)), "1280x720");
    }

    #[test]
    fn criteria_fall_back_to_the_name_when_make_model_serial_collide() {
        let two = [
            head("DP-1", "Dell Inc.", "U2419H", "", (0, 0), 1, 1),
            head("DP-2", "Dell Inc.", "U2419H", "", (0, 0), 1, 1),
        ];
        assert_eq!(criteria(&two[0], &two), "DP-1");
        assert_eq!(criteria(&two[1], &two), "DP-2");
        let same = [
            head("DP-1", "Dell Inc.", "U2419H", "ABC", (0, 0), 1, 1),
            head("DP-2", "Dell Inc.", "U2419H", "ABC", (0, 0), 1, 1),
        ];
        assert_eq!(criteria(&same[0], &same), "DP-1");
        let ok = [
            head("DP-1", "Dell Inc.", "U2419H", "ABC", (0, 0), 1, 1),
            head("DP-2", "Dell Inc.", "U2419H", "DEF", (0, 0), 1, 1),
        ];
        assert_eq!(criteria(&ok[0], &ok), "Dell Inc. U2419H ABC");
    }

    #[test]
    fn right_of_aligns_tops_below_aligns_left() {
        let p = profile(vec![
            set("A", 1920, 1080, Place::Origin),
            set("B", 1280, 1024, next("A", Side::RightOf)),
        ]);
        assert_eq!(positions(&p).unwrap(), vec![(0, 0), (1920, 0)]);
        let p = profile(vec![
            set("A", 1920, 1080, Place::Origin),
            set("B", 1280, 1024, next("A", Side::Below)),
        ]);
        assert_eq!(positions(&p).unwrap(), vec![(0, 0), (0, 1080)]);
    }

    #[test]
    fn rotation_swaps_the_size() {
        let mut a = set("A", 1920, 1080, Place::Origin);
        a.rotation = Rotation::R90;
        let p = profile(vec![a, set("B", 1280, 1024, next("A", Side::RightOf))]);
        assert_eq!(positions(&p).unwrap(), vec![(0, 0), (1080, 0)]);
    }

    #[test]
    fn left_of_is_shifted_to_zero() {
        let p = profile(vec![
            set("A", 1920, 1080, Place::Origin),
            set("B", 1280, 1024, next("A", Side::LeftOf)),
        ]);
        assert_eq!(positions(&p).unwrap(), vec![(1280, 0), (0, 0)]);
    }

    #[test]
    fn overlap_is_refused() {
        let p = profile(vec![
            set("A", 1920, 1080, Place::Origin),
            set("B", 1280, 1024, next("A", Side::RightOf)),
            set("C", 1280, 1024, next("A", Side::RightOf)),
        ]);
        assert_eq!(positions(&p), Err("B and C overlap.".to_string()));
    }

    #[test]
    fn set_place_swaps_a_cycle() {
        let mut p = profile(vec![
            set("A", 100, 100, Place::Origin),
            set("B", 100, 100, next("A", Side::RightOf)),
        ]);
        set_place(&mut p, "A", next("B", Side::RightOf));
        assert_eq!(p.outputs[1].place, Place::Origin);
        assert_eq!(p.outputs[0].place, next("B", Side::RightOf));
        assert_eq!(positions(&p).unwrap(), vec![(100, 0), (0, 0)]);
    }

    #[test]
    fn a_disabled_anchor_passes_its_place_on() {
        let mut b = set("B", 500, 500, next("A", Side::RightOf));
        b.enabled = false;
        let p = profile(vec![
            set("A", 1920, 1080, Place::Origin),
            b,
            set("C", 1280, 1024, next("B", Side::RightOf)),
        ]);
        assert_eq!(positions(&p).unwrap(), vec![(0, 0), (1920, 0)]);
    }

    #[test]
    fn a_disabled_origin_anchors_both_sides() {
        let mut x = set("X", 500, 500, Place::Origin);
        x.enabled = false;
        let p = profile(vec![
            x,
            set("L", 1000, 800, next("X", Side::LeftOf)),
            set("R", 1200, 800, next("X", Side::RightOf)),
        ]);
        assert_eq!(positions(&p).unwrap(), vec![(0, 0), (1000, 0)]);
    }

    fn sample() -> Vec<Profile> {
        let mut off = set("DP-3", 0, 0, next("Dell Inc. U2419H ABC", Side::Below));
        off.enabled = false;
        off.mode = mode(0, 0, 0);
        let mut rot = set("DP-2", 1080, 1920, next("Dell Inc. U2419H ABC", Side::RightOf));
        rot.rotation = Rotation::R270;
        vec![
            Profile {
                outputs: vec![set("Dell Inc. U2419H ABC", 1920, 1080, Place::Origin), rot, off],
                menu_output: Some("Dell Inc. U2419H ABC".into()),
            },
            profile(vec![set("eDP-1", 1920, 1080, Place::Origin)]),
        ]
    }

    #[test]
    fn parse_round_trips_render() {
        let text = format!("{}\n\n{}", crate::owned::HEADER, render(&sample()));
        assert_eq!(parse(&text).unwrap(), sample());
    }

    #[test]
    fn parse_refuses_a_hand_file() {
        assert!(parse("profile { output * enable }\n").is_err());
        let bad = format!(
            "{}\n\nprofile {{\n\toutput A enable mode 1x1 position 0,0 transform normal scale 2\n}}\n",
            crate::owned::HEADER
        );
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn profile_for_matches_the_connected_set_only() {
        let profiles = vec![profile(vec![
            set("A", 1, 1, Place::Origin),
            set("B", 1, 1, next("A", Side::RightOf)),
        ])];
        let a = head("A", "m", "x", "", (0, 0), 1, 1);
        let b = head("B", "m", "x", "", (1, 0), 1, 1);
        assert_eq!(profile_for(&profiles, &[a.clone(), b]), Some(0));
        assert_eq!(profile_for(&profiles, &[a]), None);
    }

    #[test]
    fn from_heads_reads_side_by_side_and_stacked() {
        let side = from_heads(&[
            head("A", "m", "x", "", (0, 0), 1920, 1080),
            head("B", "m", "x", "", (1920, 0), 1920, 1080),
        ]);
        assert_eq!(side.outputs[0].place, Place::Origin);
        assert_eq!(side.outputs[1].place, next("A", Side::RightOf));
        let stack = from_heads(&[
            head("A", "m", "x", "", (0, 0), 1920, 1080),
            head("B", "m", "x", "", (0, 1080), 1920, 1080),
        ]);
        assert_eq!(stack.outputs[1].place, next("A", Side::Below));
    }
}
