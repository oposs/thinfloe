//! Edge geometry of the Deskflow server: the capture strip, the entry point
//! on the device, pointer tracking on the device and edge resistance.

use crate::kanshi::Side;
use std::time::{Duration, Instant};

/// A rectangle in logical pixels of the compositor's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Pause without outward push after which the resistance sum starts again.
const RESISTANCE_RESET: Duration = Duration::from_millis(300);

/// The 1 px deep strip on the `side` edge of the output (logical pixels, so
/// it covers the whole logical edge of a scaled or rotated output).
pub fn strip_rect(out: Rect, side: Side) -> Rect {
    let w = out.w.max(0);
    let h = out.h.max(0);
    match side {
        Side::RightOf => Rect {
            x: out.x.saturating_add(w).saturating_sub(1),
            y: out.y,
            w: w.min(1),
            h,
        },
        Side::LeftOf => Rect {
            x: out.x,
            y: out.y,
            w: w.min(1),
            h,
        },
        Side::Above => Rect {
            x: out.x,
            y: out.y,
            w,
            h: h.min(1),
        },
        Side::Below => Rect {
            x: out.x,
            y: out.y.saturating_add(h).saturating_sub(1),
            w,
            h: h.min(1),
        },
    }
}

/// True when no other output touches the `side` edge of `out` along a
/// shared segment.
pub fn edge_free(out: Rect, side: Side, others: &[Rect]) -> bool {
    let end = |a: i32, b: i32| i64::from(a) + i64::from(b);
    let overlap = |a0: i32, al: i32, b0: i32, bl: i32| i64::from(a0.max(b0)) < end(a0, al).min(end(b0, bl));
    !others.iter().any(|o| match side {
        Side::RightOf => end(out.x, out.w) == i64::from(o.x) && overlap(out.y, out.h, o.y, o.h),
        Side::LeftOf => end(o.x, o.w) == i64::from(out.x) && overlap(out.y, out.h, o.y, o.h),
        Side::Below => end(out.y, out.h) == i64::from(o.y) && overlap(out.x, out.w, o.x, o.w),
        Side::Above => end(o.y, o.h) == i64::from(out.y) && overlap(out.x, out.w, o.x, o.w),
    })
}

fn clamp_i16(v: f64, max: i16) -> i16 {
    // `as` saturates and maps NaN to 0.
    let v = v.round().clamp(0.0, f64::from(max.max(0)));
    v as i16
}

/// Where the pointer enters the device when it leaves the output at `at`
/// (absolute logical position): proportional along the edge, on the device's
/// edge opposite the attached one.
pub fn entry_point(out: Rect, side: Side, at: (f64, f64), dev: (i16, i16)) -> (i16, i16) {
    let (mx, my) = (dev.0.saturating_sub(1), dev.1.saturating_sub(1));
    let frac = |pos: f64, start: i32, len: i32| {
        if len <= 0 {
            0.0
        } else {
            ((pos - f64::from(start)) / f64::from(len)).clamp(0.0, 1.0)
        }
    };
    let ax = frac(at.0, out.x, out.w);
    let ay = frac(at.1, out.y, out.h);
    match side {
        Side::RightOf => (0, clamp_i16(ay * f64::from(dev.1), my)),
        Side::LeftOf => (mx.max(0), clamp_i16(ay * f64::from(dev.1), my)),
        Side::Below => (clamp_i16(ax * f64::from(dev.0), mx), 0),
        Side::Above => (clamp_i16(ax * f64::from(dev.0), mx), my.max(0)),
    }
}

/// Where the pointer re-enters ThinFloe on the capture strip when crossing
/// back from the device at fraction `along` (`0.0..=1.0`), returning
/// `((surface_x, surface_y), (layout_x, layout_y))`.
pub fn reentry_point(out: Rect, side: Side, along: f64) -> ((f64, f64), (f64, f64)) {
    if out.w <= 0 || out.h <= 0 {
        return ((0.0, 0.0), (f64::from(out.x), f64::from(out.y)));
    }
    let f = if along.is_finite() { along.clamp(0.0, 1.0) } else { 0.5 };
    let strip = strip_rect(out, side);
    let (sx, sy) = match side {
        Side::RightOf | Side::LeftOf => (
            0.0,
            (f * f64::from(out.h)).clamp(0.0, f64::from(out.h.saturating_sub(1))),
        ),
        Side::Above | Side::Below => (
            (f * f64::from(out.w)).clamp(0.0, f64::from(out.w.saturating_sub(1))),
            0.0,
        ),
    };
    ((sx, sy), (f64::from(strip.x) + sx, f64::from(strip.y) + sy))
}

/// Result of one motion on the device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    /// New position, clamped inside the device.
    Move(i16, i16),
    /// The position crossed the attached edge; `along` is 0..1 along it.
    Back { along: f64 },
}

/// The pointer's position on the device.
#[derive(Clone, Debug)]
pub struct Tracker {
    side: Side,
    x: f64,
    y: f64,
    max: (f64, f64),
    dev: (f64, f64),
}

impl Tracker {
    pub fn new(side: Side, start: (i16, i16), dev: (i16, i16)) -> Tracker {
        let max = (f64::from(dev.0.max(1) - 1), f64::from(dev.1.max(1) - 1));
        Tracker {
            side,
            x: f64::from(start.0).clamp(0.0, max.0),
            y: f64::from(start.1).clamp(0.0, max.1),
            max,
            dev: (f64::from(dev.0.max(0)), f64::from(dev.1.max(0))),
        }
    }

    pub fn motion(&mut self, dx: f64, dy: f64) -> Track {
        let (nx, ny) = (self.x + dx, self.y + dy);
        let crossed = match self.side {
            Side::RightOf => nx < 0.0,
            Side::LeftOf => nx > self.max.0,
            Side::Below => ny < 0.0,
            Side::Above => ny > self.max.1,
        };
        if crossed {
            let (pos, len) = match self.side {
                Side::RightOf | Side::LeftOf => (self.y + dy, self.dev.1),
                Side::Below | Side::Above => (self.x + dx, self.dev.0),
            };
            let along = if len > 0.0 { (pos / len).clamp(0.0, 1.0) } else { 0.0 };
            return Track::Back { along };
        }
        self.x = nx.clamp(0.0, self.max.0);
        self.y = ny.clamp(0.0, self.max.1);
        Track::Move(clamp_i16(self.x, i16::MAX), clamp_i16(self.y, i16::MAX))
    }
}

/// Edge resistance: sums the outward push on the strip until it reaches the
/// threshold; the sum starts again after 300 ms without outward push.
#[derive(Clone, Debug)]
pub struct Resistance {
    threshold: f64,
    sum: f64,
    last_outward: Option<Instant>,
}

impl Resistance {
    pub fn new(threshold_px: f64) -> Resistance {
        Resistance {
            threshold: threshold_px,
            sum: 0.0,
            last_outward: None,
        }
    }

    /// True when this motion pushes outward and the outward sum has reached
    /// the threshold; motion inward, sideways or none never fires, so a
    /// pointer that came back from the device stays home.
    pub fn push(&mut self, side: Side, dx: f64, dy: f64, now: Instant) -> bool {
        if let Some(last) = self.last_outward
            && now.saturating_duration_since(last) >= RESISTANCE_RESET
        {
            self.reset();
        }
        let outward = match side {
            Side::RightOf => dx,
            Side::LeftOf => -dx,
            Side::Below => dy,
            Side::Above => -dy,
        };
        if outward <= 0.0 {
            return false;
        }
        self.sum += outward;
        self.last_outward = Some(now);
        self.sum >= self.threshold
    }

    pub fn reset(&mut self) {
        self.sum = 0.0;
        self.last_outward = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn strip_rects_each_side() {
        let o = r(100, 50, 800, 600);
        assert_eq!(strip_rect(o, Side::RightOf), r(899, 50, 1, 600));
        assert_eq!(strip_rect(o, Side::LeftOf), r(100, 50, 1, 600));
        assert_eq!(strip_rect(o, Side::Above), r(100, 50, 800, 1));
        assert_eq!(strip_rect(o, Side::Below), r(100, 649, 800, 1));
    }

    #[test]
    fn strip_on_scaled_and_rotated_output() {
        // 2560x1440 at scale 1.5 is 1707x960 logical.
        let s = r(1920, 0, 1707, 960);
        assert_eq!(strip_rect(s, Side::RightOf), r(3626, 0, 1, 960));
        assert_eq!(strip_rect(s, Side::Below), r(1920, 959, 1707, 1));
        // Rotated: 1920x1080 turned to 1080x1920.
        let p = r(0, 0, 1080, 1920);
        assert_eq!(strip_rect(p, Side::RightOf), r(1079, 0, 1, 1920));
        assert_eq!(strip_rect(p, Side::Above), r(0, 0, 1080, 1));
    }

    #[test]
    fn edge_between_two_monitors_not_free() {
        let a = r(0, 0, 1920, 1080);
        let b = r(1920, 0, 1920, 1080);
        assert!(!edge_free(a, Side::RightOf, &[b]));
        assert!(!edge_free(b, Side::LeftOf, &[a]));
        assert!(edge_free(a, Side::LeftOf, &[b]));
        assert!(edge_free(b, Side::RightOf, &[a]));
        assert!(edge_free(a, Side::Below, &[b]));
        // Touching only at a corner shares no segment.
        let c = r(1920, 1080, 800, 600);
        assert!(edge_free(a, Side::RightOf, &[c]));
        // A shorter neighbour still shares part of the edge.
        let d = r(1920, 900, 800, 600);
        assert!(!edge_free(a, Side::RightOf, &[d]));
        let e = r(0, 1080, 100, 100);
        assert!(!edge_free(a, Side::Below, &[e]));
        assert!(!edge_free(e, Side::Above, &[a]));
    }

    #[test]
    fn entry_maps_proportionally() {
        let o = r(0, 0, 1280, 720);
        assert_eq!(entry_point(o, Side::RightOf, (1279.0, 360.0), (1920, 1080)), (0, 540));
        assert_eq!(entry_point(o, Side::LeftOf, (0.0, 360.0), (1920, 1080)), (1919, 540));
        assert_eq!(entry_point(o, Side::Below, (640.0, 719.0), (1920, 1080)), (960, 0));
        assert_eq!(entry_point(o, Side::Above, (640.0, 0.0), (1920, 1080)), (960, 1079));
        // Offset output, position at the far end stays inside the device.
        let s = r(1920, 0, 1707, 960);
        assert_eq!(entry_point(s, Side::Below, (3627.0, 959.0), (1000, 800)), (999, 0));
        // Degenerate device does not panic.
        assert_eq!(entry_point(o, Side::RightOf, (1.0, 1.0), (0, 0)), (0, 0));
    }

    #[test]
    fn reentry_maps_proportionally_and_clamps_inside_strip() {
        let o = r(1280, 100, 1920, 1080);
        assert_eq!(reentry_point(o, Side::RightOf, 0.5), ((0.0, 540.0), (3199.0, 640.0)));
        assert_eq!(reentry_point(o, Side::LeftOf, 0.25), ((0.0, 270.0), (1280.0, 370.0)));
        assert_eq!(reentry_point(o, Side::Below, 0.5), ((960.0, 0.0), (2240.0, 1179.0)));
        assert_eq!(reentry_point(o, Side::Above, 1.0), ((1919.0, 0.0), (3199.0, 100.0)));
        // Clamping, non-finite and degenerate output never leave strip bounds.
        assert_eq!(reentry_point(o, Side::RightOf, -0.5), ((0.0, 0.0), (3199.0, 100.0)));
        assert_eq!(reentry_point(o, Side::RightOf, 1.5), ((0.0, 1079.0), (3199.0, 1179.0)));
        assert_eq!(
            reentry_point(o, Side::RightOf, f64::NAN),
            ((0.0, 540.0), (3199.0, 640.0))
        );
        assert_eq!(
            reentry_point(r(0, 0, 0, 0), Side::RightOf, 0.5),
            ((0.0, 0.0), (0.0, 0.0))
        );
    }

    #[test]
    fn tracker_clamps_and_comes_back() {
        let mut t = Tracker::new(Side::RightOf, (0, 540), (1920, 1080));
        assert_eq!(t.motion(10.0, 0.0), Track::Move(10, 540));
        assert_eq!(t.motion(0.0, 1000.0), Track::Move(10, 1079));
        assert_eq!(t.motion(100000.0, 0.0), Track::Move(1919, 1079));
        assert_eq!(t.motion(-1919.0, -539.0), Track::Move(0, 540));
        match t.motion(-1.0, 0.0) {
            Track::Back { along } => assert!((along - 0.5).abs() < 1e-9),
            o => panic!("{o:?}"),
        }
        let mut a = Tracker::new(Side::Above, (100, 1079), (1000, 1080));
        assert_eq!(a.motion(0.0, -5.0), Track::Move(100, 1074));
        assert!(matches!(a.motion(0.0, 10.0), Track::Back { .. }));
        let mut z = Tracker::new(Side::LeftOf, (0, 0), (0, 0));
        assert!(matches!(z.motion(1.0, 1.0), Track::Back { .. }));
    }

    #[test]
    fn resistance_levels() {
        for th in [15.0, 40.0, 100.0] {
            let mut r = Resistance::new(th);
            let t = Instant::now();
            let mut sum = 0.0;
            while sum + 5.0 < th {
                sum += 5.0;
                assert!(!r.push(Side::RightOf, 5.0, 0.0, t));
            }
            assert!(!r.push(Side::RightOf, th - sum - 0.5, 0.0, t));
            assert!(r.push(Side::RightOf, 0.5, 0.0, t));
        }
    }

    #[test]
    fn resistance_ignores_inward_and_sideways() {
        let t = Instant::now();
        let mut r = Resistance::new(15.0);
        assert!(!r.push(Side::RightOf, 10.0, 0.0, t));
        assert!(!r.push(Side::RightOf, -50.0, 0.0, t));
        assert!(!r.push(Side::RightOf, 0.0, 500.0, t));
        assert!(r.push(Side::RightOf, 5.0, 0.0, t));
        let mut u = Resistance::new(10.0);
        assert!(!u.push(Side::Above, 0.0, 9.0, t));
        assert!(u.push(Side::Above, 0.0, -10.0, t));
        let mut l = Resistance::new(10.0);
        assert!(l.push(Side::LeftOf, -10.0, 0.0, t));
        let mut b = Resistance::new(10.0);
        assert!(b.push(Side::Below, 0.0, 10.0, t));
    }

    #[test]
    fn resistance_resets_after_300ms() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        let mut r = Resistance::new(15.0);
        assert!(!r.push(Side::RightOf, 10.0, 0.0, t));
        assert!(r.push(Side::RightOf, 5.0, 0.0, t + ms(299)));
        let mut r = Resistance::new(15.0);
        assert!(!r.push(Side::RightOf, 10.0, 0.0, t));
        assert!(!r.push(Side::RightOf, 5.0, 0.0, t + ms(300)));
        assert!(r.push(Side::RightOf, 10.0, 0.0, t + ms(400)));
        // Inward motion does not keep the sum alive.
        let mut r = Resistance::new(15.0);
        assert!(!r.push(Side::RightOf, 10.0, 0.0, t));
        assert!(!r.push(Side::RightOf, -1.0, 0.0, t + ms(200)));
        assert!(!r.push(Side::RightOf, 5.0, 0.0, t + ms(350)));
        r.reset();
        assert!(!r.push(Side::RightOf, 14.0, 0.0, t));
    }

    #[test]
    fn resistance_off_fires_at_once_on_outward_motion() {
        let t = Instant::now();
        let mut r = Resistance::new(0.0);
        assert!(r.push(Side::RightOf, 0.1, 0.0, t));
        let mut r = Resistance::new(0.0);
        assert!(r.push(Side::Above, 0.0, -1.0, t));
    }

    #[test]
    fn inward_sideways_and_still_never_fire_at_any_level() {
        // After a release by moving back the pointer is still on the strip:
        // only a push outward may begin the next capture.
        let t = Instant::now();
        for th in [0.0, 15.0, 40.0, 100.0] {
            for side in [Side::RightOf, Side::LeftOf, Side::Above, Side::Below] {
                let mut r = Resistance::new(th);
                let (ox, oy) = match side {
                    Side::RightOf => (1.0, 0.0),
                    Side::LeftOf => (-1.0, 0.0),
                    Side::Below => (0.0, 1.0),
                    Side::Above => (0.0, -1.0),
                };
                for _ in 0..50 {
                    assert!(!r.push(side, 0.0, 0.0, t), "still, {th} {side:?}");
                    assert!(!r.push(side, -ox * 200.0, -oy * 200.0, t), "inward, {th} {side:?}");
                    assert!(!r.push(side, oy * 200.0, ox * 200.0, t), "sideways, {th} {side:?}");
                }
            }
        }
    }
}
