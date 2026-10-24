//! OKLCH -> sRGB conversion (shadcn v4 stores tokens as OKLCH).
//! Converts oklch(L C H) to an egui Color32. Out-of-gamut channels are clamped.

use egui::Color32;

/// `l` in 0..=1, `c` chroma (~0..0.4), `h` hue in degrees.
pub fn oklch_to_srgb(l: f32, c: f32, h: f32) -> Color32 {
    oklch_to_srgb_a(l, c, h, 1.0)
}

/// Same as [`oklch_to_srgb`] with an explicit alpha in 0..=1.
pub fn oklch_to_srgb_a(l: f32, c: f32, h: f32, alpha: f32) -> Color32 {
    let h_rad = h.to_radians();
    let oklab_a = c * h_rad.cos();
    let oklab_b = c * h_rad.sin();

    // OKLab -> LMS' -> LMS
    let l_ = l + 0.396_337_78 * oklab_a + 0.215_803_76 * oklab_b;
    let m_ = l - 0.105_561_35 * oklab_a - 0.063_854_17 * oklab_b;
    let s_ = l - 0.089_484_18 * oklab_a - 1.291_485_5 * oklab_b;
    let (lc, mc, sc) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    // LMS -> linear sRGB
    let r = 4.076_741_7 * lc - 3.307_711_6 * mc + 0.230_969_93 * sc;
    let g = -1.268_438 * lc + 2.609_757_4 * mc - 0.341_319_4 * sc;
    let b = -0.004_196_09 * lc - 0.703_418_6 * mc + 1.707_614_7 * sc;

    let to_u8 = |lin: f32| -> u8 {
        let lin = lin.clamp(0.0, 1.0);
        let srgb = if lin <= 0.003_130_8 {
            12.92 * lin
        } else {
            1.055 * lin.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round().clamp(0.0, 255.0) as u8
    };

    Color32::from_rgba_unmultiplied(
        to_u8(r),
        to_u8(g),
        to_u8(b),
        (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}
