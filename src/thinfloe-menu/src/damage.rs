//! Raster less. egui-winit asks for a repaint on every CursorMoved, so the
//! loop fingerprints each clipped primitive and rasters only where the set
//! changed. Moving the mouse across a static grid must not re-raster it.

use crate::raster::IRect;
use egui::epaint::{ClippedPrimitive, Primitive};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

fn fingerprint(p: &ClippedPrimitive) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in [
        p.clip_rect.min.x,
        p.clip_rect.min.y,
        p.clip_rect.max.x,
        p.clip_rect.max.y,
    ] {
        f.to_bits().hash(&mut h);
    }
    if let Primitive::Mesh(m) = &p.primitive {
        m.texture_id.hash(&mut h);
        m.indices.hash(&mut h);
        for v in &m.vertices {
            v.pos.x.to_bits().hash(&mut h);
            v.pos.y.to_bits().hash(&mut h);
            v.uv.x.to_bits().hash(&mut h);
            v.uv.y.to_bits().hash(&mut h);
            v.color.to_array().hash(&mut h);
        }
    }
    h.finish()
}

fn bounds(p: &ClippedPrimitive, ppp: f32) -> IRect {
    let clip = IRect::from_points(p.clip_rect, ppp);
    let Primitive::Mesh(m) = &p.primitive else { return clip };
    let mut r = egui::Rect::NOTHING;
    for v in &m.vertices {
        r.extend_with(v.pos);
    }
    if !r.is_positive() {
        return IRect::EMPTY;
    }
    IRect::from_points(r, ppp).intersect(&clip)
}

/// The previous frame's primitives, as fingerprints and pixel bounds.
#[derive(Default)]
pub struct Tracker {
    prev: Vec<(u64, IRect)>,
}

impl Tracker {
    /// Forget the previous frame, so the next `damage` covers everything
    /// drawn. Used after a resize and when the background changes.
    pub fn reset(&mut self) {
        self.prev.clear();
    }

    /// The pixels that must be redrawn to turn the previous frame into this
    /// one, or `None` when the two are identical and neither raster nor
    /// present is needed.
    pub fn damage(&mut self, prims: &[ClippedPrimitive], ppp: f32) -> Option<IRect> {
        let now: Vec<(u64, IRect)> = prims.iter().map(|p| (fingerprint(p), bounds(p, ppp))).collect();
        if now.iter().map(|x| x.0).eq(self.prev.iter().map(|x| x.0)) {
            return None;
        }
        let mut counts: HashMap<u64, i32> = HashMap::new();
        for (f, _) in &self.prev {
            *counts.entry(*f).or_default() += 1;
        }
        for (f, _) in &now {
            *counts.entry(*f).or_default() -= 1;
        }
        let changed = |list: &[(u64, IRect)]| {
            list.iter()
                .filter(|(f, _)| counts.get(f).copied().unwrap_or(0) != 0)
                .fold(IRect::EMPTY, |acc, (_, r)| acc.union(r))
        };
        let mut dmg = changed(&self.prev).union(&changed(&now));
        if dmg.is_empty() {
            // Same primitives in a new order: the stacking changed.
            dmg = now
                .iter()
                .chain(self.prev.iter())
                .fold(IRect::EMPTY, |acc, (_, r)| acc.union(r));
        }
        self.prev = now;
        Some(dmg)
    }
}

/// What a frame redraws, `None` for nothing: everything when it is forced
/// (the dialog opened or closed, a new scale) or a texture was uploaded (the
/// font atlas can change any text), else what `Tracker::damage` found.
pub fn frame_damage(force_full: bool, uploaded: bool, tracked: Option<IRect>, all: IRect) -> Option<IRect> {
    if force_full || uploaded { Some(all) } else { tracked }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::Mesh;
    use egui::{Color32, Rect, pos2};

    fn quad(x: f32, c: Color32) -> ClippedPrimitive {
        let mut m = Mesh::default();
        m.add_colored_rect(Rect::from_min_max(pos2(x, 0.0), pos2(x + 10.0, 10.0)), c);
        ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)),
            primitive: Primitive::Mesh(m),
        }
    }

    #[test]
    fn identical_frame_needs_nothing() {
        let mut t = Tracker::default();
        let f = vec![quad(0.0, Color32::RED), quad(20.0, Color32::BLUE)];
        assert!(t.damage(&f, 1.0).is_some(), "first frame draws");
        assert_eq!(t.damage(&f, 1.0), None, "a mouse move over a static grid draws nothing");
    }

    #[test]
    fn damage_covers_old_and_new_place_of_what_changed() {
        let mut t = Tracker::default();
        t.damage(&[quad(0.0, Color32::RED), quad(50.0, Color32::BLUE)], 1.0);
        let d = t
            .damage(&[quad(0.0, Color32::RED), quad(60.0, Color32::BLUE)], 1.0)
            .unwrap();
        assert_eq!(
            d,
            IRect {
                x0: 50,
                y0: 0,
                x1: 70,
                y1: 10
            }
        );
    }

    #[test]
    fn reorder_redraws_both() {
        let mut t = Tracker::default();
        let a = quad(0.0, Color32::RED);
        let b = quad(5.0, Color32::BLUE);
        t.damage(&[a.clone(), b.clone()], 1.0);
        let d = t.damage(&[b, a], 1.0).unwrap();
        assert_eq!(
            d,
            IRect {
                x0: 0,
                y0: 0,
                x1: 15,
                y1: 10
            }
        );
    }

    #[test]
    fn reset_forces_full_damage() {
        let mut t = Tracker::default();
        let f = vec![quad(0.0, Color32::RED)];
        t.damage(&f, 1.0);
        t.reset();
        assert_eq!(
            t.damage(&f, 1.0),
            Some(IRect {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10
            })
        );
    }

    #[test]
    fn a_frame_redraws_what_changed_or_everything() {
        let r = |x0, x1| IRect { x0, y0: 0, x1, y1: 10 };
        let all = r(0, 100);
        assert_eq!(frame_damage(true, false, None, all), Some(all));
        assert_eq!(frame_damage(true, false, Some(r(1, 2)), all), Some(all));
        assert_eq!(frame_damage(false, true, Some(r(1, 2)), all), Some(all));
        assert_eq!(frame_damage(false, false, Some(r(1, 2)), all), Some(r(1, 2)));
        // Nothing changed: no raster and no present.
        assert_eq!(frame_damage(false, false, None, all), None);
    }
}
