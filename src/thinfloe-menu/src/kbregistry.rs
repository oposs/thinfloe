//! The XKB layout list (xkeyboard-config's `evdev.xml`), for the Layout and
//! Variant pickers of the Keyboard tab.

use crate::{bounded, config};
use std::path::Path;

/// Where the image keeps the registry; `TF_XKB_REGISTRY` overrides it
/// (see `registry_path` in `main.rs`).
pub const REGISTRY_PATH: &str = "/usr/share/X11/xkb/rules/evdev.xml";
/// The real file is about 1.5 MB; anything past this is not it.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub name: String,
    pub description: String,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub layouts: Vec<Layout>,
}

/// The `name` and `description` of the `configItem` child of `node`.
/// `None` when the name is empty or not an XKB name; a missing description
/// falls back to the name.
fn item(node: roxmltree::Node) -> Option<(String, String)> {
    let ci = node.children().find(|c| c.has_tag_name("configItem"))?;
    let text = |tag: &str| {
        ci.children()
            .find(|c| c.has_tag_name(tag))
            .and_then(|c| c.text())
            .map(str::trim)
            .unwrap_or("")
    };
    let name = text("name");
    if name.is_empty() || !config::xkb_name_ok(name) {
        return None;
    }
    let description = match text("description") {
        "" => name,
        d => d,
    };
    Some((name.to_string(), description.to_string()))
}

pub fn parse(xml: &str) -> Result<Registry, String> {
    // The real file starts with a DOCTYPE; roxmltree refuses it by default.
    // It never fetches the DTD and caps entity expansion.
    let opt = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, opt).map_err(|e| e.to_string())?;
    let mut layouts: Vec<Layout> = Vec::new();
    let lists = doc.descendants().filter(|n| n.has_tag_name("layoutList"));
    for node in lists.flat_map(|l| l.children().filter(|c| c.has_tag_name("layout"))) {
        let Some((name, description)) = item(node) else {
            continue;
        };
        if layouts.iter().any(|l| l.name == name) {
            continue;
        }
        let mut variants: Vec<Variant> = node
            .children()
            .filter(|c| c.has_tag_name("variantList"))
            .flat_map(|l| l.children().filter(|c| c.has_tag_name("variant")))
            .filter_map(item)
            .map(|(name, description)| Variant { name, description })
            .collect();
        variants.sort_by(|a, b| a.description.cmp(&b.description));
        layouts.push(Layout {
            name,
            description,
            variants,
        });
    }
    layouts.sort_by(|a, b| a.description.cmp(&b.description));
    Ok(Registry { layouts })
}

pub fn read(path: &Path) -> Result<Registry, String> {
    let bytes = bounded::read_bounded(path, MAX_BYTES, true).map_err(|e| format!("{}: {e}", path.display()))?;
    let xml = String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&xml).map_err(|e| format!("{}: {e}", path.display()))
}

impl Registry {
    pub fn layout(&self, name: &str) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.name == name)
    }

    pub fn description(&self, name: &str) -> Option<&str> {
        self.layout(name).map(|l| l.description.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::xkb_name_ok;
    use crate::testdir::TestDir;

    const FIXTURE: &str = include_str!("testdata/evdev-small.xml");

    #[test]
    fn the_fixture_parses_sorted() {
        let r = parse(FIXTURE).unwrap();
        let d: Vec<&str> = r.layouts.iter().map(|l| l.description.as_str()).collect();
        assert_eq!(d, ["English (US)", "German", "German (Switzerland)"]);
        let ch = r.layout("ch").unwrap();
        let v: Vec<&str> = ch.variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(v, ["fr", "de_nodeadkeys"]); // French before German
        assert_eq!(r.description("de"), Some("German"));
        assert_eq!(r.description("nope"), None);
    }

    #[test]
    fn a_bad_name_is_skipped() {
        let r = parse(FIXTURE).unwrap();
        assert!(r.layout("x y").is_none());
        assert!(r.layout("ch").unwrap().variants.iter().all(|v| v.name != "bad name"));
    }

    #[test]
    fn a_duplicate_layout_keeps_the_first() {
        let r = parse(FIXTURE).unwrap();
        assert_eq!(r.description("us"), Some("English (US)"));
        assert_eq!(r.layouts.iter().filter(|l| l.name == "us").count(), 1);
    }

    #[test]
    fn an_empty_name_is_skipped() {
        let xml = "<xkbConfigRegistry><layoutList><layout><configItem>\
                   <name></name><description>Nothing</description>\
                   </configItem></layout></layoutList></xkbConfigRegistry>";
        assert!(parse(xml).unwrap().layouts.is_empty());
    }

    #[test]
    fn broken_xml_is_an_error() {
        assert!(parse("<xkbConfigRegistry><layoutList>").is_err());
    }

    #[test]
    fn an_oversized_file_is_refused() {
        let dir = TestDir::new("kbregistry", "big");
        let p = dir.join("evdev.xml");
        std::fs::write(&p, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        let e = read(&p).unwrap_err();
        assert!(e.contains(p.to_str().unwrap()), "{e}");
    }

    #[test]
    fn the_host_registry_parses_when_present() {
        if !Path::new(REGISTRY_PATH).exists() {
            return;
        }
        let r = read(Path::new(REGISTRY_PATH)).unwrap();
        assert!(r.layout("us").is_some());
        assert!(r.layout("ch").is_some());
        for l in &r.layouts {
            assert!(!l.name.is_empty() && xkb_name_ok(&l.name));
            assert!(l.variants.iter().all(|v| !v.name.is_empty() && xkb_name_ok(&v.name)));
        }
    }
}
