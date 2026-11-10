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

/// The pixels a frame's texture uploads damage. A texture `own` says is
/// a status element's damages only the meshes that draw it: an element
/// rendered again (a page turn, a graph's next second) redraws its cell
/// and nothing else. Any other upload (the font atlas) can change any
/// text, so it damages everything: `None`.
pub fn texture_damage(
    set: &[egui::TextureId],
    own: impl Fn(egui::TextureId) -> bool,
    prims: &[ClippedPrimitive],
    ppp: f32,
) -> Option<IRect> {
    if set.iter().any(|id| !own(*id)) {
        return None;
    }
    Some(
        prims
            .iter()
            .filter(|p| matches!(&p.primitive, Primitive::Mesh(m) if set.contains(&m.texture_id)))
            .fold(IRect::EMPTY, |acc, p| acc.union(&bounds(p, ppp))),
    )
}

/// What a frame redraws, `None` for nothing. `tracked` is `Tracker::damage`,
/// `textures` is `texture_damage`, `all` the whole canvas. A forced frame (the
/// dialog opened or closed, a new scale) and an upload that is no status
/// element's draw everything.
pub fn frame_damage(force_full: bool, tracked: Option<IRect>, textures: Option<IRect>, all: IRect) -> Option<IRect> {
    match textures {
        _ if force_full => Some(all),
        None => Some(all),
        Some(r) if !r.is_empty() => Some(tracked.map_or(r, |d| d.union(&r))),
        Some(_) => tracked,
    }
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
        // A forced frame (the dialog, a new scale) draws all, whatever else.
        assert_eq!(frame_damage(true, None, Some(IRect::EMPTY), all), Some(all));
        assert_eq!(frame_damage(true, Some(r(1, 2)), Some(r(3, 4)), all), Some(all));
        // An upload that is no status element's draws all.
        assert_eq!(frame_damage(false, None, None, all), Some(all));
        assert_eq!(frame_damage(false, Some(r(1, 2)), None, all), Some(all));
        // A status picture adds its own cell to what changed.
        assert_eq!(frame_damage(false, None, Some(r(20, 30)), all), Some(r(20, 30)));
        assert_eq!(frame_damage(false, Some(r(1, 2)), Some(r(20, 30)), all), Some(r(1, 30)));
        // No upload: what changed, or nothing at all, so no raster and no
        // present.
        assert_eq!(
            frame_damage(false, Some(r(1, 2)), Some(IRect::EMPTY), all),
            Some(r(1, 2))
        );
        assert_eq!(frame_damage(false, None, Some(IRect::EMPTY), all), None);
    }

    fn textured(x: f32, id: u64) -> ClippedPrimitive {
        let mut m = Mesh::with_texture(egui::TextureId::User(id));
        m.add_rect_with_uv(
            Rect::from_min_max(pos2(x, 0.0), pos2(x + 10.0, 10.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)),
            primitive: Primitive::Mesh(m),
        }
    }

    #[test]
    fn a_status_texture_damages_only_its_meshes() {
        let prims = [quad(0.0, Color32::RED), textured(20.0, 7), textured(50.0, 8)];
        let own = |id| matches!(id, egui::TextureId::User(7 | 8));
        let one = texture_damage(&[egui::TextureId::User(7)], own, &prims, 1.0).unwrap();
        assert_eq!(
            one,
            IRect::from_points(Rect::from_min_max(pos2(20.0, 0.0), pos2(30.0, 10.0)), 1.0)
        );
        let both = texture_damage(&[egui::TextureId::User(7), egui::TextureId::User(8)], own, &prims, 2.0).unwrap();
        assert_eq!(
            both,
            IRect::from_points(Rect::from_min_max(pos2(20.0, 0.0), pos2(60.0, 10.0)), 2.0)
        );
        // The font atlas or any other texture: everything.
        assert_eq!(
            texture_damage(
                &[egui::TextureId::User(7), egui::TextureId::default()],
                own,
                &prims,
                1.0
            ),
            None
        );
        assert_eq!(texture_damage(&[], own, &prims, 1.0), Some(IRect::EMPTY));
    }
}
