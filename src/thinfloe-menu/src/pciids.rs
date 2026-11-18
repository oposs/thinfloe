//! Names for PCI vendor and device ids, from the `pci.ids` list that
//! pciutils ships (`/usr/share/misc/pci.ids` in the image). The list is
//! about 1.4 MB, so it is read line by line and never kept.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The most of the list that is read. The real list is about 1.4 MB; a
/// bigger file is not the list, and its rest is not looked at.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Longest vendor or device name kept.
const NAME_MAX: usize = 80;

/// `"<vendor name> <device name>"` for the ids, the vendor name alone when
/// the list has the vendor but not the device, `None` when the list lacks
/// the vendor or cannot be read.
pub fn name(path: &Path, vendor: u16, device: u16) -> Option<String> {
    let file = open(path)?;
    let vendor_hex = format!("{vendor:04x}");
    let device_hex = format!("{device:04x}");
    let mut vendor_name: Option<String> = None;
    for line in BufReader::new(file.take(MAX_BYTES)).split(b'\n') {
        let line = line.ok()?;
        let line = String::from_utf8_lossy(&line);
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        match &vendor_name {
            None => {
                // The class list after the vendors starts with `C `; no
                // vendor id begins with `C`, so a vendor not found by then
                // is not listed.
                if line.starts_with("C ") {
                    return None;
                }
                if let Some(n) = entry(&line, &vendor_hex) {
                    vendor_name = Some(n);
                }
            }
            Some(v) => {
                if !line.starts_with('\t') {
                    // The next vendor: this one has no such device.
                    break;
                }
                if line.starts_with("\t\t") {
                    // A subsystem line.
                    continue;
                }
                if let Some(d) = entry(&line[1..], &device_hex) {
                    return Some(format!("{v} {d}"));
                }
            }
        }
    }
    vendor_name
}

/// The name of an `id  name` line when its id is `id` (lower-case hex, as
/// the list writes ids).
fn entry(line: &str, id: &str) -> Option<String> {
    let rest = line.strip_prefix(id)?;
    let name = rest.strip_prefix("  ")?.trim();
    (!name.is_empty()).then(|| name.chars().take(NAME_MAX).collect())
}

/// The list opened for reading, refused unless it is a regular file:
/// `O_NONBLOCK` keeps a FIFO in its place from blocking the open.
fn open(path: &Path) -> Option<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    file.metadata().ok()?.file_type().is_file().then_some(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const LIST: &str = "\
# pci.ids fixture
#\tcomment with a tab

1022  Advanced Micro Devices, Inc. [AMD]
\t1480  Starship/Matisse Root Complex
8086  Intel Corporation
\t1533  I210 Gigabit Network Connection
\t\t8086 0001  I210 Gigabit Network Connection (subsystem)
\ta0f0  Wi-Fi 6 AX201
\t\t8086 0244  Wi-Fi 6 AX201 160MHz
abcd  Last Vendor

C 00  Unclassified device
\t00  Non-VGA unclassified device
";

    #[test]
    fn names_come_from_the_list_else_none() {
        let d = TestDir::new("pciids", "names");
        let path = d.join("pci.ids");
        fs::write(&path, LIST).unwrap();
        assert_eq!(
            name(&path, 0x8086, 0xa0f0).as_deref(),
            Some("Intel Corporation Wi-Fi 6 AX201")
        );
        assert_eq!(
            name(&path, 0x8086, 0x1533).as_deref(),
            Some("Intel Corporation I210 Gigabit Network Connection")
        );
        // A subsystem id of the same vendor is not a device.
        assert_eq!(name(&path, 0x8086, 0x0244).as_deref(), Some("Intel Corporation"));
        // The device of another vendor does not count.
        assert_eq!(name(&path, 0x8086, 0x1480).as_deref(), Some("Intel Corporation"));
        assert_eq!(
            name(&path, 0x1022, 0xa0f0).as_deref(),
            Some("Advanced Micro Devices, Inc. [AMD]")
        );
        assert_eq!(name(&path, 0xabcd, 0x0001).as_deref(), Some("Last Vendor"));
        assert_eq!(name(&path, 0x10ec, 0x8168), None);
        // A class line is not a vendor.
        assert_eq!(name(&path, 0x0000, 0x0000), None);
        assert_eq!(name(&d.join("missing"), 0x8086, 0xa0f0), None);
        assert_eq!(name(&d, 0x8086, 0xa0f0), None, "a directory is not the list");
    }
}
