//! The drive list `thinfloe-upgrade drives` prints, and what each drive
//! allows. The tool's JSON is untrusted input as far as the menu cares:
//! rows are checked, text is cut, and nothing here trusts a device name.

use serde_json::Value;

/// More rows than this are refused as a whole.
pub const DRIVES_MAX: usize = 64;
/// Longest text kept from a field.
const TEXT_MAX: usize = 64;
/// Longest `id` accepted. It carries the serial, so it is not cut: a cut id
/// would no longer match `--expect-id`.
const ID_MAX: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    pub device: String,
    pub model: String,
    pub size_bytes: u64,
    pub serial: Option<String>,
    pub transport: Option<String>,
    pub removable: bool,
    pub mounted: bool,
    pub is_boot: bool,
    pub is_thinfloe: bool,
    pub has_tl_old: bool,
    pub has_tl_new: bool,
    /// False: the tool did not look at the stick's ESP because a write runs.
    /// `is_thinfloe`, `has_tl_old` and `has_tl_new` then say nothing.
    pub examined: bool,
    pub id: String,
    /// Null with `--no-size` (and when the image size is unknown).
    pub fits: Option<bool>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fresh,
    Upgrade,
}

fn device_ok(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("/dev/") else { return false };
    (1..=32).contains(&rest.len())
        && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= ID_MAX
        && !s.starts_with('-')
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn cut(s: &str) -> String {
    s.chars().take(TEXT_MAX).collect()
}

fn text(row: &Value, key: &str) -> Option<String> {
    match row.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(cut(s)),
        _ => None,
    }
}

fn flag(row: &Value, key: &str, default: bool) -> bool {
    row.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn row(v: &Value) -> Option<Drive> {
    let device = v.get("device")?.as_str()?;
    let id = v.get("id")?.as_str()?;
    if !device_ok(device) || !id_ok(id) {
        return None;
    }
    Some(Drive {
        device: device.to_string(),
        model: text(v, "model").unwrap_or_default(),
        size_bytes: v.get("size_bytes").and_then(Value::as_u64).unwrap_or(0),
        serial: text(v, "serial"),
        transport: text(v, "transport"),
        removable: flag(v, "removable", false),
        mounted: flag(v, "mounted", false),
        is_boot: flag(v, "is_boot", false),
        is_thinfloe: flag(v, "is_thinfloe", false),
        has_tl_old: flag(v, "has_tl_old", false),
        has_tl_new: flag(v, "has_tl_new", false),
        examined: flag(v, "examined", true),
        id: id.to_string(),
        fits: v.get("fits").and_then(Value::as_bool),
        version: text(v, "version"),
    })
}

/// Parse the tool's JSON array. Bad rows are dropped; a text that is not an
/// array, or has more than `DRIVES_MAX` rows, is an error.
pub fn parse(json: &str) -> Result<Vec<Drive>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("drive list: {e}"))?;
    let Value::Array(rows) = v else {
        return Err("drive list: not an array".to_string());
    };
    if rows.len() > DRIVES_MAX {
        return Err(format!("drive list: more than {DRIVES_MAX} drives"));
    }
    Ok(rows.iter().filter_map(row).collect())
}

/// A drive that is neither removable, nor on USB, nor the boot drive.
pub fn internal(d: &Drive) -> bool {
    !d.removable && d.transport.as_deref() != Some("usb") && !d.is_boot
}

pub fn tag(d: &Drive) -> String {
    if d.is_boot {
        return "this machine's stick".to_string();
    }
    if internal(d) {
        return "internal drive".to_string();
    }
    if !d.is_thinfloe {
        return "other drive".to_string();
    }
    let mut s = "ThinFloe stick".to_string();
    if let Some(v) = &d.version {
        s.push_str(", version ");
        s.push_str(v);
    }
    if d.has_tl_old {
        s.push_str(", has fallback");
    }
    if d.has_tl_new {
        s.push_str(", upgrade pending");
    }
    s
}

/// Decimal size: whole MB below 1 GB, one decimal below 10 GB, whole GB
/// above. `up` rounds up (for what a stick needs), else to the nearest.
fn size_text(bytes: u64, up: bool) -> String {
    let div = |unit: u64| if up { bytes.div_ceil(unit) } else { (bytes + unit / 2) / unit };
    if bytes < 1_000_000_000 {
        format!("{} MB", div(1_000_000))
    } else if bytes < 10_000_000_000 {
        let t = div(100_000_000);
        format!("{}.{} GB", t / 10, t % 10)
    } else {
        format!("{} GB", div(1_000_000_000))
    }
}

pub fn format_size(bytes: u64) -> String {
    size_text(bytes, false)
}

/// What the drive offers, each with the reason when it is refused.
pub fn modes(d: &Drive, image_bytes: Option<u64>) -> Vec<(Mode, Result<(), String>)> {
    let wanted: &[Mode] = if d.is_boot {
        &[Mode::Upgrade]
    } else if d.is_thinfloe {
        &[Mode::Upgrade, Mode::Fresh]
    } else {
        &[Mode::Fresh]
    };
    wanted.iter().map(|&m| (m, refusal(d, m, image_bytes))).collect()
}

fn refusal(d: &Drive, mode: Mode, image_bytes: Option<u64>) -> Result<(), String> {
    if !d.examined {
        return Err("another write is running; wait for it".to_string());
    }
    if d.mounted && !d.is_boot {
        return Err("in use: a partition is mounted".to_string());
    }
    if mode == Mode::Fresh {
        let small = d.fits == Some(false) || image_bytes.is_some_and(|b| d.size_bytes < b);
        if small {
            return Err(match image_bytes {
                Some(b) => format!("too small (needs {})", size_text(b, true)),
                None => "too small".to_string(),
            });
        }
    }
    Ok(())
}

/// What the person types back before an internal drive is erased.
pub fn confirm_text(d: &Drive) -> String {
    format!("{} {}", d.model, format_size(d.size_bytes))
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

pub fn typed_matches(typed: &str, d: &Drive) -> bool {
    norm(typed) == norm(&confirm_text(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Printed by `drives_json` of the tool for two rows.
    const TOOL_JSON: &str = r#"[{"device": "/dev/sdb", "model": "SanDisk Cruzer", "size_bytes": 16000000000, "serial": "SER1", "transport": "usb", "removable": true, "mounted": false, "is_boot": false, "is_thinfloe": true, "has_tl_old": true, "has_tl_new": false, "efi_partition": "/dev/sdb1", "examined": true, "id": "sn-SER1:16000000000:abababababababab", "fits": true}, {"device": "/dev/nvme0n1", "model": "Samsung SSD", "size_bytes": 500107862016, "serial": "", "transport": "nvme", "removable": false, "mounted": true, "is_boot": false, "is_thinfloe": false, "has_tl_old": false, "has_tl_new": false, "efi_partition": null, "examined": true, "id": "nosn:500107862016:-", "fits": true}]"#;

    fn stick() -> Drive {
        Drive {
            device: "/dev/sdb".into(),
            model: "SanDisk Cruzer".into(),
            size_bytes: 16_000_000_000,
            serial: Some("SER1".into()),
            transport: Some("usb".into()),
            removable: true,
            mounted: false,
            is_boot: false,
            is_thinfloe: false,
            has_tl_old: false,
            has_tl_new: false,
            examined: true,
            id: "sn-SER1:16000000000:ab".into(),
            fits: None,
            version: None,
        }
    }

    fn row_json(device: &str, id: &str) -> String {
        format!(r#"{{"device":"{device}","id":"{id}","model":"M","size_bytes":1}}"#)
    }

    #[test]
    fn parses_the_tools_json() {
        let v = parse(TOOL_JSON).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].device, "/dev/sdb");
        assert_eq!(v[0].model, "SanDisk Cruzer");
        assert_eq!(v[0].size_bytes, 16_000_000_000);
        assert_eq!(v[0].serial.as_deref(), Some("SER1"));
        assert_eq!(v[0].transport.as_deref(), Some("usb"));
        assert!(v[0].removable && v[0].is_thinfloe && v[0].has_tl_old && v[0].examined);
        assert!(!v[0].mounted && !v[0].is_boot && !v[0].has_tl_new);
        assert_eq!(v[0].id, "sn-SER1:16000000000:abababababababab");
        assert_eq!(v[0].fits, Some(true));
        assert_eq!(v[0].version, None);
        assert_eq!(v[1].device, "/dev/nvme0n1");
        assert_eq!(v[1].serial, None);
        assert!(v[1].mounted && internal(&v[1]));
    }

    #[test]
    fn a_row_with_a_hostile_device_or_id_is_dropped() {
        let rows = [
            row_json("/dev/sdb;x", "a"),
            row_json("../sda", "a"),
            row_json("/dev/sdb", "-x"),
            row_json("/dev/sdb", "a b"),
            row_json("/dev/sdb", ""),
            row_json("/dev/sdb", "a\\u0007b"),
            row_json("/dev/sdc", "ok"),
        ];
        let v = parse(&format!("[{}]", rows.join(","))).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].device, "/dev/sdc");
    }

    #[test]
    fn null_fields_are_none() {
        let v = parse(
            r#"[{"device":"/dev/sdb","id":"x","model":null,"serial":null,"transport":null,"fits":null,"size_bytes":null}]"#,
        )
        .unwrap();
        assert_eq!(v[0].serial, None);
        assert_eq!(v[0].transport, None);
        assert_eq!(v[0].fits, None);
        assert_eq!(v[0].model, "");
        assert_eq!(v[0].size_bytes, 0);
    }

    #[test]
    fn long_text_is_cut() {
        let long = "x".repeat(200);
        let v = parse(&format!(r#"[{{"device":"/dev/sdb","id":"i","model":"{long}"}}]"#)).unwrap();
        assert_eq!(v[0].model.chars().count(), 64);
    }

    #[test]
    fn sixty_five_rows_are_refused() {
        let one = row_json("/dev/sdb", "a");
        let many = |n: usize| format!("[{}]", vec![one.as_str(); n].join(","));
        assert_eq!(parse(&many(64)).unwrap().len(), 64);
        assert!(parse(&many(65)).is_err());
        assert!(parse("{}").is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn the_boot_stick_offers_only_upgrade() {
        let mut d = stick();
        d.is_boot = true;
        d.is_thinfloe = true;
        d.mounted = true;
        let m = modes(&d, None);
        assert_eq!(m, vec![(Mode::Upgrade, Ok(()))]);
        assert_eq!(tag(&d), "this machine's stick");
    }

    #[test]
    fn another_thinfloe_stick_offers_both() {
        let mut d = stick();
        d.is_thinfloe = true;
        d.has_tl_old = true;
        d.has_tl_new = true;
        d.version = Some("4.0.0".into());
        let m = modes(&d, None);
        assert_eq!(m, vec![(Mode::Upgrade, Ok(())), (Mode::Fresh, Ok(()))]);
        assert_eq!(
            tag(&d),
            "ThinFloe stick, version 4.0.0, has fallback, upgrade pending"
        );
        assert_eq!(modes(&stick(), None), vec![(Mode::Fresh, Ok(()))]);
        assert_eq!(tag(&stick()), "other drive");
        let mut i = stick();
        i.removable = false;
        i.transport = Some("nvme".into());
        assert_eq!(tag(&i), "internal drive");
    }

    #[test]
    fn a_mounted_stick_is_refused_with_the_reason() {
        let mut d = stick();
        d.is_thinfloe = true;
        d.mounted = true;
        for (_, r) in modes(&d, None) {
            assert_eq!(r, Err("in use: a partition is mounted".to_string()));
        }
    }

    #[test]
    fn a_small_stick_says_what_it_needs() {
        let mut d = stick();
        d.size_bytes = 2_000_000_000;
        let m = modes(&d, Some(3_148_873_728));
        assert_eq!(m[0].1, Err("too small (needs 3.2 GB)".to_string()));
        d.size_bytes = 16_000_000_000;
        d.fits = Some(false);
        assert_eq!(modes(&d, Some(3_148_873_728))[0].1, Err("too small (needs 3.2 GB)".to_string()));
        assert_eq!(modes(&d, None)[0].1, Err("too small".to_string()));
        d.fits = None;
        assert_eq!(modes(&d, None)[0].1, Ok(()));
    }

    #[test]
    fn an_unexamined_row_offers_nothing() {
        let mut d = stick();
        d.examined = false;
        d.is_thinfloe = true;
        for (_, r) in modes(&d, None) {
            assert_eq!(r, Err("another write is running; wait for it".to_string()));
        }
    }

    #[test]
    fn sizes_are_decimal() {
        assert_eq!(format_size(15_623_782_400), "16 GB");
        assert_eq!(format_size(7_800_000_000), "7.8 GB");
        assert_eq!(format_size(512_000_000), "512 MB");
    }

    #[test]
    fn the_typed_text_ignores_case_and_spaces() {
        let d = stick();
        assert_eq!(confirm_text(&d), "SanDisk Cruzer 16 GB");
        assert!(typed_matches(" sandisk   cruzer 16 gb ", &d));
        assert!(!typed_matches("SanDisk Cruzer", &d));
    }
}
