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

/// Make `name` usable as one path component; `fallback` replaces an empty or dots-only name.
/// Slashes, control and format characters become `_`.
pub fn safe(name: &str, fallback: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c.is_control() || is_format(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.chars().all(|c| c == '.') {
        return fallback.to_string();
    }
    if s.starts_with('.') {
        s.replace_range(0..1, "_");
    }
    let mut end = s.len().min(MAX_NAME);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}

/// Like `unique` for a folder name: a dot is part of the name, so " (N)" is always appended.
pub fn unique_dir(dir_entries: &[String], wanted: &str) -> String {
    if !dir_entries.iter().any(|e| e == wanted) {
        return wanted.to_string();
    }
    (2u32..)
        .map(|n| format!("{wanted} ({n})"))
        .find(|c| !dir_entries.iter().any(|e| e == c))
        .expect("an unused suffix exists")
}

/// `wanted`, or `wanted` with " (N)" before the last extension, so it is not in `dir_entries`.
pub fn unique(dir_entries: &[String], wanted: &str) -> String {
    if !dir_entries.iter().any(|e| e == wanted) {
        return wanted.to_string();
    }
    let (stem, ext) = match wanted.rfind('.') {
        Some(i) if i > 0 && i + 1 < wanted.len() => wanted.split_at(i),
        _ => (wanted, ""),
    };
    (2u32..)
        .map(|n| format!("{stem} ({n}){ext}"))
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
}
