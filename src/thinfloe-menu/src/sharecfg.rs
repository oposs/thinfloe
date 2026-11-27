//! `generated/share`, which the FloeDrop daemon `thinfloe-share` reads
//! instead of `config.toml`: the switch and the name phones show.

use crate::config::ShareCfg;

/// The longest name the file carries, in bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// `name` as phones get it: no control characters, no blanks at either
/// end, at most `MAX_NAME_BYTES`, cut between characters. Blanks at the start do not
/// count against the limit: the name is trimmed before the cut, and again after it.
pub fn clean_name(name: &str) -> String {
    let kept: String = name.chars().filter(|c| !c.is_control()).collect();
    let mut out = String::new();
    for c in kept.trim().chars() {
        if out.len() + c.len_utf8() > MAX_NAME_BYTES {
            break;
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

/// The `generated/share` body (header line allowed) as `thinfloe-share` reads it:
/// `enabled` is true for `true`, `1` or `yes`, `name` is trimmed, unknown keys and lines
/// without `=` are ignored.
pub fn parse(body: &str) -> ShareCfg {
    let mut cfg = ShareCfg::default();
    for line in body.lines() {
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

/// The name phones show for an empty one, as `thinfloe-share` makes it:
/// `FloeDrop <id>`, the host name without its `thinfloe-` prefix, or the
/// whole host name when it has none.
pub fn default_name(host: &str) -> String {
    let id = host.strip_prefix("thinfloe-").filter(|s| !s.is_empty()).unwrap_or(host);
    format!("FloeDrop {id}")
}

/// The body of `generated/share`, without the header line.
pub fn render(cfg: &ShareCfg) -> String {
    format!("enabled={}\nname={}\n", cfg.enabled, clean_name(&cfg.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_name_is_floedrop_and_the_host_id() {
        assert_eq!(default_name("thinfloe-56db68"), "FloeDrop 56db68");
        assert_eq!(default_name("desk-7"), "FloeDrop desk-7");
        assert_eq!(default_name("thinfloe-"), "FloeDrop thinfloe-");
    }

    #[test]
    fn default_name_matches_the_daemon() {
        let table = include_str!("../../../test/unit/floedrop-names.tsv");
        let mut rows = 0;
        for line in table.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let (host, want) = line.split_once('\t').expect("host<TAB>name");
            assert_eq!(default_name(host), want, "{host}");
            rows += 1;
        }
        assert!(rows >= 5);
    }

    #[test]
    fn parse_reads_what_render_writes() {
        let c = ShareCfg {
            enabled: true,
            name: "Front desk".into(),
        };
        assert_eq!(parse(&render(&c)), c);
        assert!(parse("enabled=yes\n").enabled);
        assert!(parse("enabled=1\n").enabled);
        assert!(!parse("enabled=on\n").enabled);
        assert_eq!(parse("# header\nnoise\nfuture=1\nname= x \n").name, "x");
    }

    #[test]
    fn blanks_do_not_count_against_the_limit() {
        assert_eq!(
            clean_name(&format!("{}{}", " ".repeat(10), "a".repeat(60))),
            "a".repeat(60)
        );
        assert_eq!(clean_name(&format!("{} b", "a".repeat(63))), "a".repeat(63));
    }

    #[test]
    fn render_share_file() {
        let mut c = ShareCfg {
            enabled: true,
            name: "Front desk".into(),
        };
        assert_eq!(render(&c), "enabled=true\nname=Front desk\n");
        // Control characters would end the line early or hide in it.
        c.name = "Front\ndesk\t\u{7}2\r".into();
        assert_eq!(render(&c), "enabled=true\nname=Frontdesk2\n");
        // At most 64 bytes, never inside a character.
        c.name = "ä".repeat(40);
        let r = render(&c);
        let name = r
            .strip_prefix("enabled=true\nname=")
            .unwrap()
            .strip_suffix('\n')
            .unwrap();
        assert_eq!(name, "ä".repeat(32));
        c.name = format!("{}é", "a".repeat(63));
        assert_eq!(clean_name(&c.name), "a".repeat(63));
        c.enabled = false;
        c.name.clear();
        assert_eq!(render(&c), "enabled=false\nname=\n");
    }
}
