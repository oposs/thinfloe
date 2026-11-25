//! `generated/share`, which the FloeDrop daemon `thinfloe-share` reads
//! instead of `config.toml`: the switch and the name phones show.

use crate::config::ShareCfg;

/// The longest name the file carries, in bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// `name` as phones get it: no control characters, no blanks at either
/// end, at most `MAX_NAME_BYTES`, cut between characters.
pub fn clean_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > MAX_NAME_BYTES {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
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
