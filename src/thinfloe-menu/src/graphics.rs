//! The Screen tab's "Graphics" band: each GPU with its kernel driver, read
//! from sysfs, and the video codecs VA-API offers, from `vainfo`. Read in
//! the `Job::Graphics` job thread, once, when the Screen tab first opens:
//! `vainfo` loads the GPU's VA driver, which takes a moment and must never
//! stall the UI thread or a sampler.

use crate::netdetail;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// How long `vainfo` may take in the session before it counts as not
/// available.
pub const VAINFO_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of `vainfo`'s stdout that is read. Its list is a few KiB; an
/// output this long is not that list.
const VAINFO_CAP: usize = 64 * 1024;
/// Most DRM cards looked at; a machine has one or two.
const CARDS_MAX: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    /// The DRM card it was found as, `card0`.
    pub card: String,
    /// PCI vendor and device id.
    pub ids: (u16, u16),
    /// Vendor and device name from `pci.ids`.
    pub name: Option<String>,
    /// The kernel driver bound to it.
    pub driver: Option<String>,
}

impl Gpu {
    /// The name, or the ids as `8086:5916` when `pci.ids` lacks them.
    pub fn shown_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{:04x}:{:04x}", self.ids.0, self.ids.1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codec {
    pub name: &'static str,
    pub decode: bool,
    pub encode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vaapi {
    Codecs(Vec<Codec>),
    /// Why not, for the menu's log: the run's error, its exit, or no codec.
    NotAvailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub gpus: Vec<Gpu>,
    pub vaapi: Vaapi,
}

/// The codecs shown, in this order, by the prefix of their VA profiles.
const CODECS: [(&str, &str); 4] = [
    ("VAProfileH264", "H.264"),
    ("VAProfileHEVC", "HEVC"),
    ("VAProfileVP9", "VP9"),
    ("VAProfileAV1", "AV1"),
];

/// The GPUs and the VA-API codecs, as the job reads them.
pub fn read(sys: &Path, pci_ids: &Path, vainfo_cmd: &[String]) -> Info {
    Info {
        gpus: gpus(sys, pci_ids),
        // `vainfo --display drm` opens libva's default render node. On a
        // machine with two GPUs the codec list belongs to that node's GPU
        // and is not tied to any one GPU row of the band.
        vaapi: vaapi(vainfo_cmd, VAINFO_TIMEOUT),
    }
}

/// One entry per graphics device under `sys/class/drm`: only `card<N>`
/// counts, not its connectors (`card0-HDMI-A-1`) or render nodes. A card
/// without PCI ids (simpledrm's framebuffer) is no GPU to name.
pub fn gpus(sys: &Path, pci_ids: &Path) -> Vec<Gpu> {
    let Ok(dir) = fs::read_dir(sys.join("class/drm")) else {
        return Vec::new();
    };
    let mut cards: Vec<(u32, String)> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let n = name.strip_prefix("card")?;
            let digits = !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
            Some((n.parse().ok().filter(|_| digits)?, name))
        })
        .collect();
    cards.sort();
    cards.truncate(CARDS_MAX);
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (_, card) in cards {
        let dev = sys.join("class/drm").join(&card).join("device");
        let (Some(v), Some(d)) = (
            netdetail::hex_id(&dev.join("vendor")),
            netdetail::hex_id(&dev.join("device")),
        ) else {
            continue;
        };
        // Two cards of one device would show the GPU twice.
        if let Ok(real) = fs::canonicalize(&dev) {
            if seen.contains(&real) {
                continue;
            }
            seen.push(real);
        }
        out.push(Gpu {
            card,
            ids: (v, d),
            name: crate::pciids::name(pci_ids, v, d),
            driver: netdetail::driver(&dev),
        });
    }
    out
}

/// The codecs of a `vainfo` listing: a profile line counts for decoding
/// with `VAEntrypointVLD` and for encoding with `VAEntrypointEncSlice` (or
/// its low-power `EncSliceLP`). Codecs without a line are left out.
pub fn parse_vainfo(text: &str) -> Vec<Codec> {
    let mut found: Vec<Codec> = CODECS
        .iter()
        .map(|&(_, name)| Codec {
            name,
            decode: false,
            encode: false,
        })
        .collect();
    for line in text.lines() {
        let Some((profile, entry)) = line.split_once(':') else {
            continue;
        };
        let (profile, entry) = (profile.trim(), entry.trim());
        let Some(i) = CODECS.iter().position(|(prefix, _)| profile.starts_with(prefix)) else {
            continue;
        };
        if entry == "VAEntrypointVLD" {
            found[i].decode = true;
        } else if entry.starts_with("VAEntrypointEncSlice") {
            found[i].encode = true;
        }
    }
    found.retain(|c| c.decode || c.encode);
    found
}

/// `vainfo` on the DRM display, which needs no Wayland or X environment in
/// the job thread. A failed, slow or flooding run, or one that lists no
/// codec, is `NotAvailable` with the cause. libva picks the render node
/// (its default, the first one), not the caller.
pub fn vaapi(cmd: &[String], timeout: Duration) -> Vaapi {
    let args = ["--display".to_string(), "drm".to_string()];
    let q = match crate::quiet::run(cmd, &args, None, VAINFO_CAP, timeout) {
        Ok(q) => q,
        Err(e) => return Vaapi::NotAvailable(e),
    };
    match q.status {
        Some(0) => {}
        Some(n) => return Vaapi::NotAvailable(format!("vainfo exited with status {n}")),
        None => return Vaapi::NotAvailable("vainfo was killed by a signal".to_string()),
    }
    // An output that fills the cap may be cut mid-list.
    if q.stdout.len() >= VAINFO_CAP {
        return Vaapi::NotAvailable(format!("vainfo wrote {VAINFO_CAP} bytes or more"));
    }
    let codecs = parse_vainfo(&String::from_utf8_lossy(&q.stdout));
    if codecs.is_empty() {
        Vaapi::NotAvailable("vainfo lists no codec".to_string())
    } else {
        Vaapi::Codecs(codecs)
    }
}

/// `H.264 decode, encode`, `HEVC decode`, `AV1 encode`.
pub fn codec_line(c: &Codec) -> String {
    let ways: Vec<&str> = [(c.decode, "decode"), (c.encode, "encode")]
        .iter()
        .filter(|(on, _)| *on)
        .map(|&(_, w)| w)
        .collect();
    format!("{} {}", c.name, ways.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::os::unix::fs::symlink;
    use std::time::Instant;

    /// `vainfo --display drm` of the i965 driver on a Skylake laptop; the
    /// `libva info` lines are its stderr, kept to show they are skipped.
    const SKYLAKE: &str = "\
libva info: VA-API version 1.22.0
libva info: Trying to open /usr/lib/x86_64-linux-gnu/dri/i965_drv_video.so
libva info: Found init function __vaDriverInit_1_22
libva info: va_openDriver() returns 0
vainfo: VA-API version: 1.22 (libva 2.22.0)
vainfo: Driver version: Intel i965 driver for Intel(R) Skylake - 2.4.1
vainfo: Supported profile and entrypoints
      VAProfileMPEG2Simple            :\tVAEntrypointVLD
      VAProfileMPEG2Simple            :\tVAEntrypointEncSlice
      VAProfileMPEG2Main              :\tVAEntrypointVLD
      VAProfileMPEG2Main              :\tVAEntrypointEncSlice
      VAProfileH264ConstrainedBaseline:\tVAEntrypointVLD
      VAProfileH264ConstrainedBaseline:\tVAEntrypointEncSlice
      VAProfileH264ConstrainedBaseline:\tVAEntrypointEncSliceLP
      VAProfileH264Main               :\tVAEntrypointVLD
      VAProfileH264Main               :\tVAEntrypointEncSlice
      VAProfileH264Main               :\tVAEntrypointEncSliceLP
      VAProfileH264High               :\tVAEntrypointVLD
      VAProfileH264High               :\tVAEntrypointEncSlice
      VAProfileH264High               :\tVAEntrypointEncSliceLP
      VAProfileH264MultiviewHigh      :\tVAEntrypointVLD
      VAProfileH264StereoHigh         :\tVAEntrypointVLD
      VAProfileVC1Simple              :\tVAEntrypointVLD
      VAProfileVC1Main                :\tVAEntrypointVLD
      VAProfileVC1Advanced            :\tVAEntrypointVLD
      VAProfileNone                   :\tVAEntrypointVideoProc
      VAProfileJPEGBaseline           :\tVAEntrypointVLD
      VAProfileJPEGBaseline           :\tVAEntrypointEncPicture
      VAProfileVP8Version0_3          :\tVAEntrypointVLD
      VAProfileVP8Version0_3          :\tVAEntrypointEncSlice
      VAProfileHEVCMain               :\tVAEntrypointVLD
      VAProfileVP9Profile0            :\tVAEntrypointVLD
";

    const PCI_IDS: &str = "\
8086  Intel Corporation
\t1916  Skylake GT2 [HD Graphics 520]
";

    fn sh(script: &str) -> Vec<String> {
        ["/bin/sh", "-c", script, "sh"].map(String::from).to_vec()
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `sys/devices/<device>` with PCI ids and a driver link, and
    /// `sys/class/drm/<card>` linked to it as the kernel does.
    fn drm_card(sys: &Path, card: &str, device: &str, ids: (&str, &str), driver: &str) {
        let dev = sys.join("devices").join(device);
        write(&dev.join("vendor"), &format!("{}\n", ids.0));
        write(&dev.join("device"), &format!("{}\n", ids.1));
        let drv = sys.join("bus/pci/drivers").join(driver);
        fs::create_dir_all(&drv).unwrap();
        let _ = symlink(&drv, dev.join("driver"));
        let class = sys.join("class/drm").join(card);
        fs::create_dir_all(&class).unwrap();
        symlink(&dev, class.join("device")).unwrap();
    }

    #[test]
    fn vainfo_profiles_become_codec_names() {
        let h264 = Codec {
            name: "H.264",
            decode: true,
            encode: true,
        };
        let decode = |name| Codec {
            name,
            decode: true,
            encode: false,
        };
        assert_eq!(parse_vainfo(SKYLAKE), vec![h264, decode("HEVC"), decode("VP9")]);
        assert_eq!(codec_line(&h264), "H.264 decode, encode");
        assert_eq!(codec_line(&decode("HEVC")), "HEVC decode");
        // The order is fixed, not the listing's; an encoder alone counts.
        let av1 = "VAProfileAV1Profile0 : VAEntrypointEncSliceLP\nVAProfileVP9Profile2 : VAEntrypointVLD\n";
        let got = parse_vainfo(av1);
        assert_eq!(
            got,
            vec![
                decode("VP9"),
                Codec {
                    name: "AV1",
                    decode: false,
                    encode: true
                }
            ]
        );
        assert_eq!(codec_line(&got[1]), "AV1 encode");
        assert_eq!(parse_vainfo(""), Vec::new());
    }

    #[test]
    fn a_working_vainfo_lists_its_codecs_from_the_drm_display() {
        let d = TestDir::new("graphics", "vainfo-ok");
        let listing = d.join("listing");
        fs::write(&listing, SKYLAKE).unwrap();
        // The listing only when called with `--display drm`.
        let script = format!("[ \"$*\" = '--display drm' ] || exit 2; cat '{}'", listing.display());
        match vaapi(&sh(&script), Duration::from_secs(5)) {
            Vaapi::Codecs(c) => assert_eq!(c.len(), 3, "{c:?}"),
            other => panic!("{other:?}"),
        }
        // A run that lists no codec has none to offer.
        assert_eq!(
            vaapi(&sh("echo 'vainfo: no profiles'"), Duration::from_secs(5)),
            Vaapi::NotAvailable("vainfo lists no codec".into())
        );
    }

    #[test]
    fn a_slow_vainfo_is_not_available() {
        let started = Instant::now();
        assert_eq!(
            vaapi(&sh("sleep 5"), Duration::from_millis(200)),
            Vaapi::NotAvailable("no answer within 200 ms".into())
        );
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn a_failing_vainfo_is_not_available() {
        let d = TestDir::new("graphics", "vainfo-fail");
        let listing = d.join("listing");
        fs::write(&listing, SKYLAKE).unwrap();
        // A listing that came with a failure is not trusted.
        let script = format!("cat '{}'; exit 1", listing.display());
        assert_eq!(
            vaapi(&sh(&script), Duration::from_secs(5)),
            Vaapi::NotAvailable("vainfo exited with status 1".into())
        );
        assert!(matches!(
            vaapi(&["/nonexistent/vainfo".to_string()], Duration::from_secs(5)),
            Vaapi::NotAvailable(e) if e.starts_with("/nonexistent/vainfo: ")
        ));
        assert!(matches!(
            vaapi(&sh("kill -9 $$"), Duration::from_secs(5)),
            Vaapi::NotAvailable(e) if e == "vainfo was killed by a signal"
        ));
        // An endless output stops at the cap; one of exactly the cap may be
        // cut mid-list. In a shell: `yes` itself refuses `--display` and
        // exits 1 before writing anything.
        assert_eq!(
            vaapi(&sh("yes"), Duration::from_secs(5)),
            Vaapi::NotAvailable(format!("more than {VAINFO_CAP} bytes of output"))
        );
        let full = format!("head -c {VAINFO_CAP} /dev/zero");
        assert_eq!(
            vaapi(&sh(&full), Duration::from_secs(5)),
            Vaapi::NotAvailable(format!("vainfo wrote {VAINFO_CAP} bytes or more"))
        );
    }

    #[test]
    fn gpus_come_from_drm_cards() {
        let d = TestDir::new("graphics", "drm-cards");
        let sys = d.join("sys");
        drm_card(&sys, "card0", "pci0000:00/0000:00:02.0", ("0x8086", "0x1916"), "i915");
        // A connector of the same card is no second GPU.
        let conn = sys.join("class/drm/card0-HDMI-A-1");
        fs::create_dir_all(&conn).unwrap();
        symlink(sys.join("devices/pci0000:00/0000:00:02.0"), conn.join("device")).unwrap();
        // simpledrm's framebuffer has no PCI ids.
        let fb = sys.join("devices/platform/simple-framebuffer.0");
        fs::create_dir_all(&fb).unwrap();
        fs::create_dir_all(sys.join("class/drm/card1")).unwrap();
        symlink(&fb, sys.join("class/drm/card1/device")).unwrap();
        let pci_ids = d.join("pci.ids");
        fs::write(&pci_ids, PCI_IDS).unwrap();
        let got = gpus(&sys, &pci_ids);
        assert_eq!(
            got,
            vec![Gpu {
                card: "card0".into(),
                ids: (0x8086, 0x1916),
                name: Some("Intel Corporation Skylake GT2 [HD Graphics 520]".into()),
                driver: Some("i915".into()),
            }]
        );
        assert_eq!(got[0].shown_name(), "Intel Corporation Skylake GT2 [HD Graphics 520]");
        // A second card of the same device shows it once; another device
        // is a second GPU, after the first.
        let card2 = sys.join("class/drm/card2");
        fs::create_dir_all(&card2).unwrap();
        symlink(sys.join("devices/pci0000:00/0000:00:02.0"), card2.join("device")).unwrap();
        drm_card(
            &sys,
            "card10",
            "pci0000:00/0000:01:00.0",
            ("0x10de", "0x1c82"),
            "nouveau",
        );
        let cards: Vec<String> = gpus(&sys, &pci_ids).into_iter().map(|g| g.card).collect();
        assert_eq!(cards, ["card0", "card10"]);
    }

    /// The owner's thin client (4.6.0): `card1`, no `card0`, three
    /// connectors whose `device` is the card itself. Every attribute file
    /// there reports 4096 bytes; the files here are as small as they hold.
    #[test]
    fn gpus_of_the_thin_client() {
        let d = TestDir::new("graphics", "thinclient");
        let sys = d.join("sys");
        for line in include_str!("testdata/thinclient-drm/cards.tsv").lines() {
            if line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let dev = sys.join(f[1]);
            fs::create_dir_all(&dev).unwrap();
            if !f[2].is_empty() {
                write(&dev.join("vendor"), &format!("{}\n", f[2]));
                write(&dev.join("device"), &format!("{}\n", f[3]));
            }
            if !f[4].is_empty() {
                let drv = sys.join("bus/pci/drivers").join(f[4]);
                fs::create_dir_all(&drv).unwrap();
                symlink(&drv, dev.join("driver")).unwrap();
            }
            let class = sys.join("class/drm").join(f[0]);
            fs::create_dir_all(&class).unwrap();
            symlink(&dev, class.join("device")).unwrap();
        }
        let pci_ids = d.join("pci.ids");
        fs::write(&pci_ids, "8086  Intel Corporation\n\t1616  HD Graphics 5500\n").unwrap();
        assert_eq!(
            gpus(&sys, &pci_ids),
            vec![Gpu {
                card: "card1".into(),
                ids: (0x8086, 0x1616),
                name: Some("Intel Corporation HD Graphics 5500".into()),
                driver: Some("i915".into()),
            }]
        );
    }

    /// On a host with a PCI GPU the real `/sys` names it: its attribute
    /// files report 4096 bytes, which the reader must not trust.
    #[test]
    fn a_gpu_is_found_on_this_hosts_sysfs() {
        let Ok(dir) = fs::read_dir("/sys/class/drm") else {
            return;
        };
        let pci_card = dir.filter_map(|e| e.ok()).any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("card")
                .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
                && e.path().join("device/vendor").exists()
        });
        if !pci_card {
            return;
        }
        assert!(!gpus(Path::new("/sys"), Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn a_machine_without_drm_cards_has_no_gpu() {
        let d = TestDir::new("graphics", "no-drm");
        assert_eq!(gpus(&d.join("sys"), &d.join("pci.ids")), Vec::new());
        fs::create_dir_all(d.join("sys/class/drm")).unwrap();
        assert_eq!(gpus(&d.join("sys"), &d.join("pci.ids")), Vec::new());
    }

    #[test]
    fn without_pci_ids_the_gpu_shows_its_ids() {
        let d = TestDir::new("graphics", "no-pci-ids");
        let sys = d.join("sys");
        drm_card(&sys, "card0", "pci0000:00/0000:00:02.0", ("0x8086", "0x1916"), "i915");
        let got = gpus(&sys, &d.join("missing.ids"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, None);
        assert_eq!(got[0].shown_name(), "8086:1916");
        assert_eq!(got[0].driver.as_deref(), Some("i915"));
    }
}
