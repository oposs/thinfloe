//! Safe folder and file names for FloeDrop: whatever a sender calls itself or its files,
//! the result is one plain path component of bounded length.

/// Longest name in bytes.
pub const MAX_NAME: usize = 64;

/// Unicode general category Cf (format characters): invisible marks such as the
/// bidi overrides (U+202E turns "invoice<RLO>fdp.exe" into "invoiceexe.pdf" on screen)
/// and zero-width spaces. Rust's std has no category table, so the assigned Cf code
/// points are listed here (Unicode 16).
pub fn is_format(c: char) -> bool {
    matches!(
        c as u32,
        0xAD | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x890..=0x891
            | 0x8E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// Longest extension, in bytes after the dot, that a cut keeps and a suffix goes before.
const MAX_EXT: usize = 16;

/// `s` cut to at most `max` bytes, at a char boundary.
fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `name` split into stem and extension (with its dot): the part after the last dot,
/// when the dot is not the first byte and the part is 1 to `MAX_EXT` bytes.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && name.len() - i - 1 <= MAX_EXT => {
            name.split_at(i)
        }
        _ => (name, ""),
    }
}

/// Slashes, control and format characters replaced by `_`, a leading dot too; `None`
/// when nothing usable is left (empty, dots only, or every character replaced).
fn clean(name: &str) -> Option<String> {
    let mut replaced = 0usize;
    let mut s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c.is_control() || is_format(c) {
                replaced += 1;
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.chars().all(|c| c == '.') || replaced == s.chars().count() {
        return None;
    }
    if s.starts_with('.') {
        s.replace_range(0..1, "_");
    }
    Some(s)
}

fn check_fallback(fallback: &str) {
    debug_assert!(
        !fallback.is_empty()
            && fallback.len() <= MAX_NAME
            && !fallback.contains('/')
            && !fallback.starts_with('.'),
        "unsafe fallback {fallback:?}"
    );
}

/// Make `name` usable as one path component of at most `MAX_NAME` bytes. Slashes, control
/// and format characters become `_`, a leading dot too. `fallback` replaces an empty or
/// dots-only name and one whose every character was replaced; it is returned as it is and
/// must itself be a safe name.
pub fn safe(name: &str, fallback: &str) -> String {
    check_fallback(fallback);
    match clean(name) {
        Some(s) => cut(&s, MAX_NAME).to_string(),
        None => fallback.to_string(),
    }
}

/// Like `safe` for a file name: a cut shortens the stem and keeps the extension
/// (up to `MAX_EXT` bytes after the last dot). `fallback` must itself be a safe name.
pub fn safe_file(name: &str, fallback: &str) -> String {
    check_fallback(fallback);
    let Some(s) = clean(name) else {
        return fallback.to_string();
    };
    if s.len() <= MAX_NAME {
        return s;
    }
    let (stem, ext) = split_ext(&s);
    format!("{}{ext}", cut(stem, MAX_NAME - ext.len()))
}

/// Like `unique` for a folder name: a dot is part of the name, so " (N)" is always appended;
/// the name is cut so the result stays within `MAX_NAME` bytes.
pub fn unique_dir(dir_entries: &[String], wanted: &str) -> String {
    if !dir_entries.iter().any(|e| e == wanted) {
        return wanted.to_string();
    }
    (2u32..)
        .map(|n| {
            let suffix = format!(" ({n})");
            format!("{}{suffix}", cut(wanted, MAX_NAME.saturating_sub(suffix.len())))
        })
        .find(|c| !dir_entries.iter().any(|e| e == c))
        .expect("an unused suffix exists")
}

/// `wanted`, or `wanted` with " (N)" before the last extension, so it is not in
/// `dir_entries`; the stem is cut so the result stays within `MAX_NAME` bytes.
pub fn unique(dir_entries: &[String], wanted: &str) -> String {
    if !dir_entries.iter().any(|e| e == wanted) {
        return wanted.to_string();
    }
    let (stem, ext) = split_ext(wanted);
    (2u32..)
        .map(|n| {
            let suffix = format!(" ({n})");
            let room = MAX_NAME.saturating_sub(suffix.len() + ext.len());
            format!("{}{suffix}{ext}", cut(stem, room))
        })
        .find(|c| !dir_entries.iter().any(|e| e == c))
        .expect("an unused suffix exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_and_controls_replaced() {
        assert_eq!(safe("a/b\nc", "iPhone"), "a_b_c");
        assert_eq!(safe("a\x7fb\0c", "iPhone"), "a_b_c");
    }

    #[test]
    fn format_characters_replaced() {
        let s = safe("invoice\u{202E}fdp.exe", "iPhone");
        assert!(!s.contains('\u{202E}'), "{s:?}");
        assert_eq!(s, "invoice_fdp.exe");
        assert_eq!(safe("a\u{200B}b", "iPhone"), "a_b");
        for c in [
            '\u{AD}',
            '\u{61C}',
            '\u{180E}',
            '\u{200F}',
            '\u{2066}',
            '\u{2069}',
            '\u{FEFF}',
            '\u{E0001}',
        ] {
            assert_eq!(safe(&format!("x{c}y"), "iPhone"), "x_y", "{:X}", c as u32);
        }
        // Ordinary letters and emoji stay.
        assert_eq!(safe("J\u{fc}rg \u{1F600}", "iPhone"), "J\u{fc}rg \u{1F600}");
    }

    #[test]
    fn leading_dots_replaced() {
        assert_eq!(safe("..", "iPhone"), "iPhone");
        assert_eq!(safe(".x", "iPhone"), "_x");
    }

    #[test]
    fn empty_is_fallback() {
        assert_eq!(safe("", "iPhone"), "iPhone");
    }

    #[test]
    fn hostile_names_stay_inside() {
        let long = "\u{1F600}".repeat(75);
        assert_eq!(long.len(), 300);
        let s = safe(&long, "iPhone");
        assert!(s.len() <= MAX_NAME && !s.is_empty());
        assert!(!s.contains('/'));
        assert!(std::str::from_utf8(s.as_bytes()).is_ok());
    }

    #[test]
    fn same_name_gets_a_suffix() {
        assert_eq!(unique(&["iPhone".into()], "iPhone"), "iPhone (2)");
        assert_eq!(
            unique(&["a.jpg".into(), "a (2).jpg".into()], "a.jpg"),
            "a (3).jpg"
        );
        assert_eq!(unique(&[], "a.jpg"), "a.jpg");
        assert_eq!(
            unique(&["IMG_0042.HEIC".into()], "IMG_0042.HEIC"),
            "IMG_0042 (2).HEIC"
        );
    }
    #[test]
    fn folder_suffix_ignores_dots() {
        assert_eq!(
            unique_dir(&["Tobias Q. iPhone".into()], "Tobias Q. iPhone"),
            "Tobias Q. iPhone (2)"
        );
        assert_eq!(
            unique_dir(
                &["iPhone 15.5".into(), "iPhone 15.5 (2)".into()],
                "iPhone 15.5"
            ),
            "iPhone 15.5 (3)"
        );
        assert_eq!(unique_dir(&[], "iPhone 15.5"), "iPhone 15.5");
    }
    #[test]
    fn a_long_name_keeps_its_extension() {
        let s = safe_file(&format!("{}.HEIC", "a".repeat(200)), "file");
        assert_eq!(s.len(), MAX_NAME);
        assert!(s.ends_with(".HEIC"), "{s}");
        // Not an extension: too long, or a leading dot.
        assert_eq!(
            safe_file(&format!("x.{}", "b".repeat(80)), "file").len(),
            MAX_NAME
        );
        assert_eq!(
            safe_file(&format!(".{}", "b".repeat(80)), "file"),
            format!("_{}", "b".repeat(63))
        );
        // A multi-byte stem is cut at a char boundary.
        let s = safe_file(&format!("{}.jpg", "\u{fc}".repeat(40)), "file");
        assert!(s.len() <= MAX_NAME && s.ends_with(".jpg"), "{s}");
    }

    #[test]
    fn a_suffix_never_passes_the_limit() {
        let long = format!("{}.jpg", "a".repeat(60)); // 64 bytes
        let u = unique(std::slice::from_ref(&long), &long);
        assert!(u.len() <= MAX_NAME && u.ends_with(" (2).jpg"), "{u}");
        let d = "d".repeat(64);
        let u = unique_dir(std::slice::from_ref(&d), &d);
        assert!(u.len() <= MAX_NAME && u.ends_with(" (2)"), "{u}");
        assert!(std::str::from_utf8(u.as_bytes()).is_ok());
        let m = "\u{fc}".repeat(32); // 64 bytes of two-byte chars
        let u = unique_dir(std::slice::from_ref(&m), &m);
        assert!(u.len() <= MAX_NAME && u.ends_with(" (2)"), "{u}");
        // A long extension is not one: the suffix goes at the end, within the limit.
        let x = format!("x.{}", "b".repeat(62));
        let u = unique(std::slice::from_ref(&x), &x);
        assert!(u.len() <= MAX_NAME && u.ends_with(" (2)"), "{u}");
    }

    #[test]
    fn a_name_of_controls_only_is_the_fallback() {
        assert_eq!(safe("\u{1}\u{2}\u{3}", "iPhone"), "iPhone");
        assert_eq!(safe("\u{202E}\u{200B}", "iPhone"), "iPhone");
        assert_eq!(safe("/", "iPhone"), "iPhone");
        assert_eq!(safe("a\u{1}", "iPhone"), "a_");
        assert_eq!(safe_file("\u{1}", "file"), "file");
    }

    #[test]
    fn the_cut_and_dot_edges() {
        assert_eq!(safe(&"a".repeat(64), "f"), "a".repeat(64));
        assert_eq!(safe(&"a".repeat(65), "f"), "a".repeat(64));
        assert_eq!(safe(".bashrc", "f"), "_bashrc");
        assert_eq!(safe(".", "f"), "f");
        assert_eq!(safe_file(".bashrc", "f"), "_bashrc");
    }
}
