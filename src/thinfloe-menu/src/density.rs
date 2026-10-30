//! How large the menu draws on a given screen: the scale follows the pixel
//! density of the output, so a tile is about the same size on a 24" 1080p
//! panel and a 13" 8K one.

use std::io;
use std::path::{Path, PathBuf};

/// Width of one row of four tiles with their gaps, in points.
pub const GROUP_W: f32 = 4.0 * 220.0 + 3.0 * 3.0;

/// What the compositor tells us about one output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    pub name: Option<String>,
    pub mode_px: (u32, u32),
    pub phys_mm: (u32, u32),
}

/// The scale to use when the reported size cannot be trusted: 2 on a screen
/// whose shorter side has 1800 px or more, else 1.
pub fn fallback_scale(mode_px: (u32, u32)) -> f32 {
    if mode_px.0.min(mode_px.1) >= 1800 { 2.0 } else { 1.0 }
}

/// The scale that gives a tile its size on paper, from the pixel density of
/// the output, in steps of 0.25 and never below 1. A size the screen reports
/// wrongly (0, an aspect or a width no real screen has, a density out of
/// range) is not used; the fallback applies then.
pub fn density_scale(mode_px: (u32, u32), phys_mm: (u32, u32)) -> f32 {
    let (px_w, px_h) = (mode_px.0 as f32, mode_px.1 as f32);
    let (mm_w, mm_h) = (phys_mm.0 as f32, phys_mm.1 as f32);
    if px_w <= 0.0 || px_h <= 0.0 || mm_w <= 0.0 || mm_h <= 0.0 || mm_w < 200.0 {
        return fallback_scale(mode_px);
    }
    if ((mm_w / mm_h) / (px_w / px_h) - 1.0).abs() > 0.10 {
        return fallback_scale(mode_px);
    }
    let ppi = px_w / (mm_w / 25.4);
    if !(50.0..=800.0).contains(&ppi) {
        return fallback_scale(mode_px);
    }
    ((ppi / 92.0).max(1.0) * 4.0).round() / 4.0
}

/// Shrinks `s` to the largest step of 0.25 at which a row of tiles fills at
/// most 90 % of the window width, but not below 0.5.
pub fn fit_scale(s: f32, window_w_px: u32) -> f32 {
    let max = 0.9 * window_w_px as f32 / GROUP_W;
    if s <= max {
        s
    } else {
        ((max * 4.0).floor() / 4.0).max(0.5)
    }
}

/// The output the menu window is on: by name, else the only one, else the
/// first whose mode has the monitor's size.
pub fn choose_output<'a>(
    outputs: &'a [OutputInfo],
    monitor_name: Option<&str>,
    monitor_px: Option<(u32, u32)>,
) -> Option<&'a OutputInfo> {
    if let Some(o) = monitor_name.and_then(|n| outputs.iter().find(|o| o.name.as_deref() == Some(n))) {
        return Some(o);
    }
    if let [only] = outputs {
        return Some(only);
    }
    let px = monitor_px?;
    outputs.iter().find(|o| o.mode_px == px)
}

/// The scale for the menu window: density of the output (or the fallback for
/// `fallback_px`), then the fit to the window width.
pub fn menu_scale(output: Option<&OutputInfo>, fallback_px: (u32, u32), window_w_px: u32) -> f32 {
    let d = match output {
        Some(o) => density_scale(o.mode_px, o.phys_mm),
        None => fallback_scale(fallback_px),
    };
    fit_scale(d, window_w_px)
}

/// Writes the scale as one number and a newline, atomically.
pub fn write_scale_file(path: &Path, s: f32) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{s}\n"))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Where the scale is kept: `TF_SCALE_FILE`, else `$XDG_RUNTIME_DIR/thinfloe-scale`.
/// An empty variable counts as unset, as in the session scripts.
pub fn scale_file_path() -> Option<PathBuf> {
    scale_file_from(std::env::var_os("TF_SCALE_FILE"), std::env::var_os("XDG_RUNTIME_DIR"))
}

fn scale_file_from(file: Option<std::ffi::OsString>, runtime_dir: Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(p) = file.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-scale"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(px: (u32, u32), mm: (u32, u32)) -> f32 {
        density_scale(px, mm)
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let o = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(scale_file_from(o("/a/f"), o("/run")), Some(PathBuf::from("/a/f")));
        assert_eq!(
            scale_file_from(o(""), o("/run")),
            Some(PathBuf::from("/run/thinfloe-scale"))
        );
        assert_eq!(
            scale_file_from(None, o("/run")),
            Some(PathBuf::from("/run/thinfloe-scale"))
        );
        assert_eq!(scale_file_from(o(""), o("")), None);
        assert_eq!(scale_file_from(None, None), None);
    }

    #[test]
    fn a_24_inch_1080p_is_one() {
        assert_eq!(s((1920, 1080), (531, 299)), 1.0);
    }
    #[test]
    fn a_21_inch_4k_is_2_25() {
        assert_eq!(s((3840, 2160), (465, 261)), 2.25);
    }
    #[test]
    fn a_43_inch_4k_is_one() {
        assert_eq!(s((3840, 2160), (952, 535)), 1.0);
    }
    #[test]
    fn a_13_inch_8k_is_7_25() {
        assert_eq!(s((7680, 4320), (288, 162)), 7.25);
    }
    #[test]
    fn a_32_inch_1080p_is_not_below_one() {
        assert_eq!(s((1920, 1080), (708, 398)), 1.0);
    }
    #[test]
    fn zero_mm_falls_back() {
        assert_eq!(s((3840, 2160), (0, 0)), 2.0);
        assert_eq!(s((1920, 1080), (531, 0)), 1.0);
    }
    #[test]
    fn an_aspect_in_cm_falls_back() {
        // EDID 16 x 9 cm
        assert_eq!(s((3840, 2160), (160, 90)), 2.0);
        assert_eq!(s((1920, 1080), (160, 90)), 1.0);
    }
    #[test]
    fn a_wrong_aspect_falls_back() {
        assert_eq!(s((1920, 1080), (600, 200)), 1.0);
    }
    #[test]
    fn ppi_out_of_range_falls_back() {
        assert_eq!(s((1920, 1080), (1000, 563)), 1.0); // 48.8 ppi
        assert_eq!(s((7680, 4320), (210, 118)), 2.0); // 929 ppi
    }
    #[test]
    fn the_fit_shrinks_a_small_dense_screen() {
        let d = s((1920, 1080), (256, 144)); // 11.6", 2.0 by density
        assert_eq!(d, 2.0);
        assert_eq!(fit_scale(d, 1920), 1.75);
    }
    #[test]
    fn the_fit_leaves_a_13_inch_8k_alone() {
        assert_eq!(fit_scale(7.25, 7680), 7.25);
    }
    #[test]
    fn the_fit_stops_at_one_half() {
        assert_eq!(fit_scale(1.0, 400), 0.5);
    }
    #[test]
    fn a_rotated_monitor_takes_density_from_its_native_size_and_fits_its_width() {
        let out = OutputInfo {
            name: Some("DP-1".into()),
            mode_px: (3840, 2160),
            phys_mm: (465, 261),
        };
        assert_eq!(menu_scale(Some(&out), (3840, 2160), 2160), 2.0);
    }
    #[test]
    fn the_group_with_its_side_gaps_fits_every_result() {
        for w in [400u32, 800, 1024, 1366, 1920, 2560, 3840, 7680] {
            for d in [1.0f32, 1.75, 2.25, 4.0, 7.25] {
                let f = fit_scale(d, w);
                assert!(f == 0.5 || GROUP_W * f <= 0.9 * w as f32, "{w} {d} {f}");
                assert_eq!((f * 4.0).fract(), 0.0);
            }
        }
    }
    #[test]
    fn the_output_is_found_by_name_else_alone_else_by_size() {
        let a = OutputInfo {
            name: Some("DP-1".into()),
            mode_px: (1920, 1080),
            phys_mm: (531, 299),
        };
        let b = OutputInfo {
            name: Some("HDMI-A-1".into()),
            mode_px: (3840, 2160),
            phys_mm: (465, 261),
        };
        let both = [a.clone(), b.clone()];
        assert_eq!(choose_output(&both, Some("HDMI-A-1"), None), Some(&b));
        assert_eq!(choose_output(&both, None, Some((3840, 2160))), Some(&b));
        assert_eq!(choose_output(&both, None, None), None);
        assert_eq!(choose_output(std::slice::from_ref(&a), None, None), Some(&a));
    }
    #[test]
    fn the_scale_file_holds_one_number() {
        let dir = crate::testdir::TestDir::new("density", "scale");
        let p = dir.join("thinfloe-scale");
        write_scale_file(&p, 2.25).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "2.25\n");
        write_scale_file(&p, 1.0).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "1\n");
        assert!(!p.with_extension("tmp").exists());
    }
    #[test]
    fn an_unwritable_scale_file_is_an_error_not_a_panic() {
        assert!(write_scale_file(Path::new("/nonexistent-dir/thinfloe-scale"), 1.0).is_err());
    }
    #[test]
    fn the_fallback_edge_is_1800_px_on_the_shorter_side() {
        assert_eq!(fallback_scale((3200, 1799)), 1.0);
        assert_eq!(fallback_scale((3200, 1800)), 2.0);
        assert_eq!(fallback_scale((1800, 3200)), 2.0);
    }
    #[test]
    fn a_width_under_200_mm_falls_back() {
        // 199 mm is refused, 200 mm is used: 1920 px / 7.87 in = 244 ppi, 2.75 by density
        assert_eq!(s((1920, 1080), (199, 112)), 1.0);
        assert_eq!(s((1920, 1080), (200, 113)), 2.75);
    }
    #[test]
    fn the_aspect_may_differ_by_ten_percent() {
        // 8K is 1.7778; 10 % wider is 1.9556, so 500 mm needs a height of 255.7 mm or more
        assert_eq!(s((7680, 4320), (500, 255)), 2.0); // 10.3 % wider: falls back
        assert_eq!(s((7680, 4320), (500, 256)), 4.25); // 9.9 % wider: used
        // 10 % narrower is 1.6: 500 mm allows a height up to 312.5 mm
        assert_eq!(s((7680, 4320), (500, 312)), 4.25);
        assert_eq!(s((7680, 4320), (500, 313)), 2.0);
    }
    #[test]
    fn the_density_range_is_50_to_800_ppi() {
        // 3840 px over 1950 mm is 50.02 ppi (used, the floor of 1); over 1951 mm
        // it is 49.99 and the fallback 2 applies
        assert_eq!(s((3840, 2160), (1950, 1097)), 1.0);
        assert_eq!(s((3840, 2160), (1951, 1097)), 2.0);
        // 7680 px over 244 mm is 799.5 ppi (used); over 243 mm it is 802.8
        assert_eq!(s((7680, 4320), (244, 137)), 8.75);
        assert_eq!(s((7680, 4320), (243, 137)), 2.0);
    }
    #[test]
    fn a_tie_at_an_eighth_rounds_up() {
        // 1955 px over 254 mm is 195.5 ppi, 2.125 * 92: the step 2.125 is a tie
        // between 2.0 and 2.25 and rounds up (f32::round, away from zero)
        assert_eq!(s((1955, 1100), (254, 143)), 2.25);
    }
}
