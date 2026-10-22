//! A desktop entry reader that knows exactly as much of the freedesktop
//! format as a tile needs: the `[Desktop Entry]` group, `key=value` lines,
//! comments, value escapes, and the `Exec` quoting rules.

use std::collections::BTreeMap;

/// The keys of one `[Desktop Entry]` group. Localised keys (`Name[de]`) and
/// other groups are skipped, not rejected.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Entry {
    pub keys: BTreeMap<String, String>,
}

impl Entry {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(String::as_str)
    }
}

/// Parse a desktop entry. The error names the line, because the grid shows
/// it on the tile and that is the only place a person at the machine sees it.
pub fn parse(text: &str) -> Result<Entry, String> {
    let mut entry = Entry::default();
    let mut in_main = false;
    let mut seen_main = false;
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(format!("line {line_no} is not a group header"));
            }
            in_main = line == "[Desktop Entry]";
            seen_main |= in_main;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {line_no} is not a desktop entry line"));
        };
        if !seen_main {
            return Err(format!("line {line_no} comes before [Desktop Entry]"));
        }
        if !in_main {
            continue;
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("line {line_no} has no key"));
        }
        if key.contains('[') {
            continue;
        }
        entry.keys.insert(key.to_string(), unescape(value.trim()));
    }
    if !seen_main {
        return Err("no [Desktop Entry] group".to_string());
    }
    Ok(entry)
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into argv. Double quotes group, a backslash inside
/// quotes escapes the next character, field codes (`%f`, `%U`, ...) are
/// dropped because a tile is never started with files, and `%%` is a `%`.
pub fn split_exec(exec: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                have = true;
            }
            '\\' if quoted => match chars.next() {
                Some(n) => cur.push(n),
                None => return Err("Exec ends in a backslash".to_string()),
            },
            '%' => match chars.next() {
                Some('%') => {
                    cur.push('%');
                    have = true;
                }
                Some(_) => {}
                None => return Err("Exec ends in a lone %".to_string()),
            },
            c if c.is_whitespace() && !quoted => {
                if have || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
                cur.push(c);
                have = true;
            }
        }
    }
    if quoted {
        return Err("Exec has an unterminated quote".to_string());
    }
    if have || !cur.is_empty() {
        args.push(cur);
    }
    if args.is_empty() {
        return Err("Exec is empty".to_string());
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_main_group_and_skips_others() {
        let e = parse(
            "# c\n[Desktop Entry]\nName=ThinLinc\nName[de]=Dünn\nExec=/opt/thinlinc/bin/tlclient\n\
             [Desktop Action x]\nName=other\n",
        )
        .unwrap();
        assert_eq!(e.get("Name"), Some("ThinLinc"));
        assert_eq!(e.get("Exec"), Some("/opt/thinlinc/bin/tlclient"));
        assert_eq!(e.keys.len(), 2);
    }

    #[test]
    fn names_the_bad_line() {
        assert_eq!(
            parse("[Desktop Entry]\nName=x\nthis is junk\n").unwrap_err(),
            "line 3 is not a desktop entry line"
        );
        assert_eq!(parse("Name=x\n").unwrap_err(), "line 1 comes before [Desktop Entry]");
        assert_eq!(parse("").unwrap_err(), "no [Desktop Entry] group");
    }

    #[test]
    fn unescapes_values() {
        let e = parse("[Desktop Entry]\nComment=a\\sb\\\\c\n").unwrap();
        assert_eq!(e.get("Comment"), Some("a b\\c"));
    }

    #[test]
    fn splits_exec() {
        assert_eq!(
            split_exec("sudo thinfloe-upgrade").unwrap(),
            ["sudo", "thinfloe-upgrade"]
        );
        assert_eq!(split_exec("foo %U --x").unwrap(), ["foo", "--x"]);
        assert_eq!(
            split_exec(r#""/a b/c" "q\"x" 100%%"#).unwrap(),
            ["/a b/c", "q\"x", "100%"]
        );
        assert_eq!(split_exec(r#"x """#).unwrap(), ["x", ""]);
        assert!(split_exec("\"open").is_err());
        assert!(split_exec("   ").is_err());
        assert!(split_exec("%U").is_err());
    }
}
