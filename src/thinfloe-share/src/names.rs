//! Safe folder and file names for FloeDrop: whatever a sender calls itself or its files,
//! the result is one plain path component of bounded length.

/// Longest name in bytes.
pub const MAX_NAME: usize = 64;

/// Make `name` usable as one path component; `fallback` replaces an empty or dots-only name.
pub fn safe(name: &str, fallback: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
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
