//! A CPU rasteriser for egui's tessellated meshes, writing into a 0RGB
//! canvas that is copied to a softbuffer surface. No GPU, no GL, no Vulkan.
//!
//! It assumes feathering is off (see app.rs): every edge is hard, so a
//! pixel is either inside a triangle or not, and solid fills can take the
//! fast paths below.

use egui::epaint::{ClippedPrimitive, Primitive, Vertex, WHITE_UV};
use egui::{Color32, TextureId, TexturesDelta};
use std::collections::HashMap;

/// An integer pixel rectangle, `x0..x1` by `y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub const EMPTY: IRect = IRect {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }
    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    /// The pixels an egui rect in points covers, rounded outwards.
    pub fn from_points(r: egui::Rect, ppp: f32) -> IRect {
        IRect {
            x0: (r.min.x * ppp).floor() as i32,
            y0: (r.min.y * ppp).floor() as i32,
            x1: (r.max.x * ppp).ceil() as i32,
            y1: (r.max.y * ppp).ceil() as i32,
        }
    }
}

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

/// The textures egui has asked for, kept in sync from `TexturesDelta`.
#[derive(Default)]
pub struct Textures {
    map: HashMap<TextureId, Texture>,
}

impl Textures {
    /// Apply `delta.set` before rasterising a frame.
    pub fn apply_set(&mut self, delta: &TexturesDelta) {
        // A texture's deltas arrive in order: a full image, then patches.
        for (id, deltas) in &delta.set {
            for d in deltas {
                let egui::ImageData::Color(img) = &d.image;
                match d.pos {
                    None => {
                        let t = Texture {
                            width: img.size[0],
                            height: img.size[1],
                            pixels: img.pixels.clone(),
                        };
                        self.map.insert(*id, t);
                    }
                    Some([px, py]) => {
                        let Some(t) = self.map.get_mut(id) else { continue };
                        for row in 0..img.size[1] {
                            let dst = (py + row) * t.width + px;
                            let src = row * img.size[0];
                            t.pixels[dst..dst + img.size[0]].copy_from_slice(&img.pixels[src..src + img.size[0]]);
                        }
                    }
                }
            }
        }
    }

    /// Apply `delta.free` after the frame, so a skipped raster never drops an
    /// update.
    pub fn apply_free(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.map.remove(id);
        }
    }
}

/// What lies under egui: a solid colour, or a frozen, dimmed copy of the grid
/// while the save dialog is open.
pub enum Background {
    Solid(Color32),
    Frozen(Vec<u32>),
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
    pub background: Background,
}

fn pack(c: [u32; 3]) -> u32 {
    (c[0] << 16) | (c[1] << 8) | c[2]
}

fn unpack(p: u32) -> [u32; 3] {
    [(p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff]
}

/// Premultiplied `src` over opaque `dst`.
fn blend(dst: u32, src: [u32; 4]) -> u32 {
    if src[3] == 255 {
        return pack([src[0], src[1], src[2]]);
    }
    let d = unpack(dst);
    let inv = 255 - src[3];
    pack([
        (src[0] + (d[0] * inv + 127) / 255).min(255),
        (src[1] + (d[1] * inv + 127) / 255).min(255),
        (src[2] + (d[2] * inv + 127) / 255).min(255),
    ])
}

fn rgba(c: Color32) -> [u32; 4] {
    [c.r() as u32, c.g() as u32, c.b() as u32, c.a() as u32]
}

fn is_white_uv(v: &Vertex) -> bool {
    (v.uv.x - WHITE_UV.x).abs() < 1e-3 && (v.uv.y - WHITE_UV.y).abs() < 1e-3
}

impl Canvas {
    pub fn new(width: usize, height: usize, background: Color32) -> Canvas {
        let mut c = Canvas {
            width,
            height,
            pixels: vec![0; width * height],
            background: Background::Solid(background),
        };
        c.restore_background(c.bounds());
        c
    }

    pub fn bounds(&self) -> IRect {
        IRect {
            x0: 0,
            y0: 0,
            x1: self.width as i32,
            y1: self.height as i32,
        }
    }

    /// Copy what lies under egui back into `rect`.
    pub fn restore_background(&mut self, rect: IRect) {
        let r = rect.intersect(&self.bounds());
        if r.is_empty() {
            return;
        }
        for y in r.y0 as usize..r.y1 as usize {
            let row = y * self.width;
            let span = row + r.x0 as usize..row + r.x1 as usize;
            match &self.background {
                Background::Solid(c) => {
                    let p = pack([c.r() as u32, c.g() as u32, c.b() as u32]);
                    self.pixels[span].fill(p);
                }
                Background::Frozen(f) => self.pixels[span.clone()].copy_from_slice(&f[span]),
            }
        }
    }

    /// Freeze the current picture, darkened, as the background. Done once
    /// when the dialog opens; blending a full-window scrim every frame while
    /// a tar runs is the most expensive thing this program could do.
    pub fn freeze_dimmed(&mut self, keep: u32) {
        let frozen = self
            .pixels
            .iter()
            .map(|&p| {
                let c = unpack(p);
                pack([c[0] * keep / 255, c[1] * keep / 255, c[2] * keep / 255])
            })
            .collect();
        self.background = Background::Frozen(frozen);
    }

    /// Rasterise `prims` into the canvas, touching only pixels inside `damage`.
    pub fn draw(&mut self, prims: &[ClippedPrimitive], textures: &Textures, ppp: f32, damage: IRect) {
        let damage = damage.intersect(&self.bounds());
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let clip = IRect::from_points(p.clip_rect, ppp).intersect(&damage);
            if clip.is_empty() {
                continue;
            }
            let tex = textures.map.get(&mesh.texture_id);
            if self.try_fill_rect(&mesh.vertices, &mesh.indices, ppp, clip) {
                continue;
            }
            for tri in mesh.indices.chunks_exact(3) {
                let v = [
                    &mesh.vertices[tri[0] as usize],
                    &mesh.vertices[tri[1] as usize],
                    &mesh.vertices[tri[2] as usize],
                ];
                self.triangle(v, tex, ppp, clip);
            }
        }
    }

    /// The fast path for opaque axis-aligned solid rectangles, which is most
    /// of the pixels on screen: fill rows instead of testing each pixel.
    fn try_fill_rect(&mut self, verts: &[Vertex], idx: &[u32], ppp: f32, clip: IRect) -> bool {
        if verts.len() != 4 || idx.len() != 6 {
            return false;
        }
        let c = verts[0].color;
        if c.a() != 255 || !verts.iter().all(|v| v.color == c && is_white_uv(v)) {
            return false;
        }
        let xs: Vec<f32> = verts.iter().map(|v| v.pos.x).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos.y).collect();
        let (minx, maxx) = (
            xs.iter().cloned().fold(f32::MAX, f32::min),
            xs.iter().cloned().fold(f32::MIN, f32::max),
        );
        let (miny, maxy) = (
            ys.iter().cloned().fold(f32::MAX, f32::min),
            ys.iter().cloned().fold(f32::MIN, f32::max),
        );
        let axis_aligned = verts
            .iter()
            .all(|v| (v.pos.x == minx || v.pos.x == maxx) && (v.pos.y == miny || v.pos.y == maxy));
        if !axis_aligned {
            return false;
        }
        let r = IRect {
            x0: (minx * ppp).round() as i32,
            y0: (miny * ppp).round() as i32,
            x1: (maxx * ppp).round() as i32,
            y1: (maxy * ppp).round() as i32,
        }
        .intersect(&clip);
        let p = pack([c.r() as u32, c.g() as u32, c.b() as u32]);
        if !r.is_empty() {
            for y in r.y0 as usize..r.y1 as usize {
                let row = y * self.width;
                self.pixels[row + r.x0 as usize..row + r.x1 as usize].fill(p);
            }
        }
        true
    }

    fn triangle(&mut self, v: [&Vertex; 3], tex: Option<&Texture>, ppp: f32, clip: IRect) {
        let p: [(f32, f32); 3] = [
            (v[0].pos.x * ppp, v[0].pos.y * ppp),
            (v[1].pos.x * ppp, v[1].pos.y * ppp),
            (v[2].pos.x * ppp, v[2].pos.y * ppp),
        ];
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
        if area.abs() < 1e-6 {
            return;
        }
        let bb = IRect {
            x0: p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor() as i32,
            y0: p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor() as i32,
            x1: p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as i32,
            y1: p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as i32,
        }
        .intersect(&clip);
        if bb.is_empty() {
            return;
        }
        let solid = v.iter().all(|x| is_white_uv(x)) && v[0].color == v[1].color && v[1].color == v[2].color;
        // Edge function of edge a->b at point (x, y), positive inside for a
        // triangle with positive area.
        let sign = area.signum();
        let edge =
            |a: (f32, f32), b: (f32, f32), x: f32, y: f32| sign * ((b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0));
        // Top-left rule: a pixel centre exactly on a shared edge belongs to
        // one of the two triangles only, so translucent fills are not blended
        // twice along the diagonal of a quad.
        let owns = |a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (sign * (b.0 - a.0), sign * (b.1 - a.1));
            (dy == 0.0 && dx < 0.0) || dy > 0.0
        };
        let tl = [owns(p[1], p[2]), owns(p[2], p[0]), owns(p[0], p[1])];
        let inv_area = 1.0 / (area * sign);
        for y in bb.y0..bb.y1 {
            let cy = y as f32 + 0.5;
            let row = y as usize * self.width;
            for x in bb.x0..bb.x1 {
                let cx = x as f32 + 0.5;
                let w = [
                    edge(p[1], p[2], cx, cy),
                    edge(p[2], p[0], cx, cy),
                    edge(p[0], p[1], cx, cy),
                ];
                if (0..3).any(|i| w[i] < 0.0 || (w[i] == 0.0 && !tl[i])) {
                    continue;
                }
                let src = if solid {
                    rgba(v[0].color)
                } else {
                    let b = [w[0] * inv_area, w[1] * inv_area, w[2] * inv_area];
                    let col: [f32; 4] = std::array::from_fn(|k| {
                        b[0] * rgba(v[0].color)[k] as f32
                            + b[1] * rgba(v[1].color)[k] as f32
                            + b[2] * rgba(v[2].color)[k] as f32
                    });
                    let texel = match tex {
                        Some(t) => {
                            let u = b[0] * v[0].uv.x + b[1] * v[1].uv.x + b[2] * v[2].uv.x;
                            let vv = b[0] * v[0].uv.y + b[1] * v[1].uv.y + b[2] * v[2].uv.y;
                            let tx = ((u * t.width as f32) as usize).min(t.width - 1);
                            let ty = ((vv * t.height as f32) as usize).min(t.height - 1);
                            rgba(t.pixels[ty * t.width + tx])
                        }
                        None => [255; 4],
                    };
                    std::array::from_fn(|k| ((col[k] * texel[k] as f32 / 255.0).round() as u32).min(255))
                };
                if src[3] == 0 && src[0] == 0 && src[1] == 0 && src[2] == 0 {
                    continue;
                }
                let i = row + x as usize;
                self.pixels[i] = blend(self.pixels[i], src);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::Mesh;
    use egui::{Rect, pos2};

    fn prim(mesh: Mesh, clip: Rect) -> ClippedPrimitive {
        ClippedPrimitive {
            clip_rect: clip,
            primitive: Primitive::Mesh(mesh),
        }
    }

    fn rect_mesh(r: Rect, c: Color32) -> Mesh {
        let mut m = Mesh::default();
        m.add_colored_rect(r, c);
        m
    }

    const FULL: Rect = Rect {
        min: pos2(0.0, 0.0),
        max: pos2(100.0, 100.0),
    };

    #[test]
    fn opaque_rect_fills_exactly() {
        let mut c = Canvas::new(10, 10, Color32::BLACK);
        let m = rect_mesh(
            Rect::from_min_max(pos2(2.0, 3.0), pos2(5.0, 6.0)),
            Color32::from_rgb(255, 0, 0),
        );
        c.draw(&[prim(m, FULL)], &Textures::default(), 1.0, c.bounds());
        let red = (0..10)
            .flat_map(|y| (0..10).map(move |x| (x, y)))
            .filter(|&(x, y)| c.pixels[y * 10 + x] == 0xff0000)
            .count();
        assert_eq!(red, 9);
        assert_eq!(c.pixels[3 * 10 + 2], 0xff0000);
        assert_eq!(c.pixels[6 * 10 + 5], 0);
    }

    #[test]
    fn translucent_quad_is_not_blended_twice_on_its_diagonal() {
        let mut c = Canvas::new(8, 8, Color32::BLACK);
        let half = Color32::from_rgba_premultiplied(100, 100, 100, 128);
        let m = rect_mesh(Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 8.0)), half);
        c.draw(&[prim(m, FULL)], &Textures::default(), 1.0, c.bounds());
        assert!(c.pixels.iter().all(|&p| p == 0x646464), "{:x?}", c.pixels);
    }

    #[test]
    fn clip_and_damage_are_respected() {
        let mut c = Canvas::new(10, 10, Color32::BLACK);
        let m = rect_mesh(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)), Color32::WHITE);
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(5.0, 10.0));
        c.draw(
            &[prim(m, clip)],
            &Textures::default(),
            1.0,
            IRect {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 2,
            },
        );
        assert_eq!(c.pixels[4], 0xffffff);
        assert_eq!(c.pixels[5], 0, "outside clip");
        assert_eq!(c.pixels[2 * 10], 0, "outside damage");
    }

    #[test]
    fn pixels_per_point_scales_geometry() {
        let mut c = Canvas::new(10, 10, Color32::BLACK);
        let m = rect_mesh(Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0)), Color32::WHITE);
        c.draw(&[prim(m, FULL)], &Textures::default(), 2.0, c.bounds());
        assert_eq!(c.pixels.iter().filter(|&&p| p == 0xffffff).count(), 16);
    }

    #[test]
    fn texture_is_sampled_and_tinted() {
        let mut tex = Textures::default();
        let id = TextureId::Managed(7);
        tex.map.insert(
            id,
            Texture {
                width: 1,
                height: 1,
                pixels: vec![Color32::from_rgb(255, 255, 255)],
            },
        );
        let mut m = Mesh::with_texture(id);
        m.add_rect_with_uv(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::from_rgb(0, 128, 0),
        );
        let mut c = Canvas::new(4, 4, Color32::BLACK);
        c.draw(&[prim(m, FULL)], &tex, 1.0, c.bounds());
        assert!(c.pixels.iter().all(|&p| p == 0x008000));
    }

    #[test]
    fn frozen_background_is_dimmed_once() {
        let mut c = Canvas::new(2, 1, Color32::from_rgb(200, 100, 50));
        c.freeze_dimmed(128);
        c.pixels.fill(0);
        c.restore_background(c.bounds());
        assert_eq!(c.pixels[0], pack([200 * 128 / 255, 100 * 128 / 255, 50 * 128 / 255]));
    }
}
