//! The release notes `thinfloe-upgrade notes` prints, as headings and
//! bullets for the What's new dialog. Not a Markdown renderer: `## X.Y.Z -
//! date` and `### Group` become one heading per group, `- ` items become
//! bullets, and backticks go.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Heading(String),
    Bullet(String),
}

/// One heading per `###` group that has items (`4.4.0 · New`), or the
/// version alone for a `##` without groups. An indented line, or a line
/// right under an item, continues that item; other text is an item of its
/// own. Blank lines and `#` lines go.
pub fn parse(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut p = Parser::default();
    for raw in text.lines() {
        let line = raw.replace('`', "");
        let trimmed = line.trim();
        if let Some(h) = line.strip_prefix("### ") {
            p.group = Some(h.trim().to_string());
            p.pending = true;
            p.grouped = true;
            p.in_item = false;
            p.joinable = false;
        } else if let Some(h) = line.strip_prefix("## ") {
            p.end_version(&mut out);
            // `4.4.0 - 2026-11-14`: the version alone.
            let v = h.split(" - ").next().unwrap_or(h).trim();
            p.version = Some(v.to_string());
            p.group = None;
            p.pending = true;
            p.grouped = false;
            p.wrote = false;
            p.in_item = false;
            p.joinable = false;
        } else if trimmed.is_empty() || line.starts_with('#') {
            p.in_item = false;
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            // A nested item is an item of its own, never part of its parent.
            p.heading(&mut out);
            out.push(Line::Bullet(squash(item)));
            p.in_item = true;
            p.joinable = true;
        } else if let (true, Some(Line::Bullet(last))) = (
            p.in_item || line.starts_with(' '),
            out.last_mut().filter(|_| p.joinable),
        ) {
            last.push(' ');
            last.push_str(&squash(trimmed));
            p.in_item = true;
            p.joinable = true;
        } else {
            p.heading(&mut out);
            out.push(Line::Bullet(squash(trimmed)));
            p.in_item = true;
            p.joinable = true;
        }
    }
    p.end_version(&mut out);
    out
}

#[derive(Default)]
struct Parser {
    version: Option<String>,
    group: Option<String>,
    /// The heading for the current group is not written yet.
    pending: bool,
    /// The current `##` has a `###`.
    grouped: bool,
    /// Something was written under the current `##`.
    wrote: bool,
    /// The last line was part of an item.
    in_item: bool,
    /// The last line written is an item of the current group.
    joinable: bool,
}

impl Parser {
    fn heading(&mut self, out: &mut Vec<Line>) {
        if self.pending {
            let text = match (&self.version, &self.group) {
                (Some(v), Some(g)) => format!("{v} · {g}"),
                (Some(v), None) => v.clone(),
                (None, Some(g)) => g.clone(),
                (None, None) => String::new(),
            };
            if !text.is_empty() {
                out.push(Line::Heading(text));
            }
            self.pending = false;
        }
        self.wrote = true;
    }

    /// A `##` without groups and without items still names its version.
    fn end_version(&mut self, out: &mut Vec<Line>) {
        if self.version.is_some() && !self.grouped && !self.wrote {
            self.heading(out);
        }
    }
}

/// Runs of white space as one space.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The start of the 4.4.0 section of `CHANGES.md`.
    const V440: &str = "## 4.4.0 - 2026-11-14
### New
- The Settings screen has a Keyboard tab: pick the layout and its variant from the full list, CapsLock as Ctrl on or off, and the Compose key (Right Alt by default). A change applies at once to the running session, with a line to try it right there, and is kept with the home directory.

### Changed
- All settings the menu manages are now kept in `~/.config/thinfloe/config.toml`, and the first start after the upgrade moves the old files there by itself, WiFi included.
- `XKB_DEFAULT_*` lines in `~/.config/thinfloe/environment` are ignored, and the Settings screen names each one.

### Fixed
- A keyboard layout and variant XKB cannot combine no longer leaves tty1 at \"graphical session exited with status 139\" after login.
";

    fn b(s: &str) -> Line {
        Line::Bullet(s.into())
    }

    fn h(s: &str) -> Line {
        Line::Heading(s.into())
    }

    #[test]
    fn parse_headings_and_bullets() {
        let l = parse(V440);
        assert_eq!(l[0], h("4.4.0 · New"));
        assert!(
            matches!(&l[1], Line::Bullet(s) if s.starts_with("The Settings screen has a Keyboard tab")),
            "{:?}",
            l[1]
        );
        assert_eq!(l[2], h("4.4.0 · Changed"));
        assert_eq!(
            l[3],
            b(
                "All settings the menu manages are now kept in ~/.config/thinfloe/config.toml, and the first start after the upgrade moves the old files there by itself, WiFi included."
            )
        );
        assert_eq!(l[5], h("4.4.0 · Fixed"));
        assert_eq!(l.len(), 7);
    }

    #[test]
    fn parse_strips_backticks() {
        assert_eq!(
            parse("## 4.5.0 - 2026-12-01\n### New\n- `a` b\n"),
            [h("4.5.0 · New"), b("a b")]
        );
    }

    #[test]
    fn parse_joins_continuation_lines() {
        let text = "## 4.5.0 - 2026-12-01\n### Fixed\n- one\n  two   three\n- four\nfive\n";
        assert_eq!(parse(text), [h("4.5.0 · Fixed"), b("one two three"), b("four five")]);
    }

    #[test]
    fn parse_keeps_a_nested_item_apart() {
        let text = "## 4.5.0\n### New\n- one\n  - two\n    more\n- three\n";
        assert_eq!(parse(text), [h("4.5.0 · New"), b("one"), b("two more"), b("three")]);
    }

    #[test]
    fn parse_skips_empty_subsection() {
        let text = "## 4.5.0 - 2026-12-01\n### New\n\n### Fixed\n- x\n## 4.4.1 - 2026-11-20\n### New\n";
        assert_eq!(parse(text), [h("4.5.0 · Fixed"), b("x")]);
    }

    #[test]
    fn parse_version_without_subsections() {
        assert_eq!(parse("## 4.5.0 - 2026-12-01\n- x\n"), [h("4.5.0"), b("x")]);
        assert_eq!(parse("## 4.5.0\n"), [h("4.5.0")]);
    }
}
