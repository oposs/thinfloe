//! The daemon's configuration, `generated/share`: `key=value` lines written by the menu.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShareConfig {
    pub enabled: bool,
    /// Name shown on phones; empty means the machine's host name.
    pub name: String,
}

/// Parse the file's text. Unknown keys and lines without `=` are ignored.
pub fn parse(text: &str) -> ShareConfig {
    let mut cfg = ShareConfig::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "enabled" => cfg.enabled = matches!(v.trim(), "true" | "1" | "yes"),
            "name" => cfg.name = v.trim().to_string(),
            _ => {}
        }
    }
    cfg
}

/// Read the file; an absent or unreadable file means disabled.
pub fn load(path: &Path) -> ShareConfig {
    std::fs::read_to_string(path)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_absent_is_disabled() {
        assert_eq!(parse(""), ShareConfig::default());
        assert!(!parse("").enabled);
        assert_eq!(
            load(Path::new("/nonexistent/generated/share")),
            ShareConfig::default()
        );
    }

    #[test]
    fn keys_are_read_and_unknown_ones_ignored() {
        let c = parse("# header\nenabled=true\nfuture=1\nname=Front desk\nnoise\n");
        assert!(c.enabled);
        assert_eq!(c.name, "Front desk");
        assert!(!parse("enabled=false\n").enabled);
    }
}
