//! The `[deskflow]` table of config.toml and the body of
//! `generated/deskflow`, which the Deskflow daemon reads instead of
//! config.toml. The generated file holds the same TOML table and
//! `mic_mute_local`, which the Keyboard tab's volume key switch decides.

use std::collections::HashSet;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::DeskflowCfg;
use crate::kanshi::Side;

/// The default port of Deskflow servers.
pub const DEFAULT_PORT: u16 = 24800;

/// How far the pointer has to push against an edge before it crosses.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Resistance {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl Resistance {
    /// The push in pixels.
    pub fn px(self) -> f64 {
        match self {
            Resistance::Off => 0.0,
            Resistance::Low => 15.0,
            Resistance::Medium => 40.0,
            Resistance::High => 100.0,
        }
    }
}

// A side is written with the words of the monitor profiles.
impl Serialize for Side {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.word())
    }
}

impl<'de> Deserialize<'de> for Side {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Side, D::Error> {
        let w = String::deserialize(d)?;
        Side::from_word(&w).ok_or_else(|| D::Error::custom(format!("not a side: {w}")))
    }
}

const MAX_NAME_BYTES: usize = 64;
const FINGERPRINT_PREFIX: &str = "v2:sha256:";

fn fingerprint_ok(f: &str) -> bool {
    f.strip_prefix(FINGERPRINT_PREFIX)
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Refuses what the daemon could not use: a fingerprint that is not
/// `v2:sha256:` and 64 lower-case hex digits, a name of 0 or more than 64
/// bytes, a name twice, two devices at one edge of one output, port 0.
pub fn validate(cfg: &DeskflowCfg) -> Result<(), String> {
    if cfg.port == 0 {
        return Err("Deskflow: the port must not be 0".into());
    }
    let mut names = HashSet::new();
    let mut edges = HashSet::new();
    for d in &cfg.devices {
        if d.name.is_empty() || d.name.len() > MAX_NAME_BYTES {
            return Err(format!("Deskflow: a device name must have 1 to {MAX_NAME_BYTES} bytes"));
        }
        if !fingerprint_ok(&d.fingerprint) {
            return Err(format!(
                "Deskflow: the fingerprint of {} is not v2:sha256:<64 hex digits>",
                d.name
            ));
        }
        if !names.insert(d.name.as_str()) {
            return Err(format!("Deskflow: the name {} is used twice", d.name));
        }
        if !edges.insert((d.output.as_str(), d.side.word())) {
            return Err(format!("Deskflow: two devices are {} {}", d.side.word(), d.output));
        }
    }
    Ok(())
}

/// What generated/deskflow holds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Generated {
    /// The microphone mute key mutes ThinFloe while a device has the
    /// keyboard: the volume key switch is on, and labwc's binding cannot
    /// act then. Not part of `[deskflow]`.
    pub mic_mute_local: bool,
    pub deskflow: DeskflowCfg,
}

/// The TOML text of the file (no generated header).
pub fn render(cfg: &DeskflowCfg, mic_mute_local: bool) -> String {
    #[derive(Serialize)]
    struct Wrap<'a> {
        mic_mute_local: bool,
        deskflow: &'a DeskflowCfg,
    }
    toml::to_string(&Wrap {
        mic_mute_local,
        deskflow: cfg,
    })
    .expect("the Deskflow table serialises to TOML")
}

/// Reads what `render` wrote (a generated header line is a comment); a
/// file without `mic_mute_local` reads as false.
pub fn parse(text: &str) -> Result<Generated, String> {
    toml::from_str::<Generated>(text).map_err(|e| format!("generated/deskflow: {}", e.message()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceCfg;

    const FP: &str = "v2:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn dev(name: &str, output: &str, side: Side) -> DeviceCfg {
        DeviceCfg {
            name: name.into(),
            fingerprint: FP.into(),
            output: output.into(),
            side,
            clipboard: true,
            compose: false,
            enabled: true,
        }
    }

    fn sample() -> DeskflowCfg {
        DeskflowCfg {
            listen: true,
            port: 24801,
            edge_resistance: Resistance::High,
            devices: vec![dev("laptop", "DP-1", Side::RightOf), dev("pad", "DP-1", Side::Below)],
        }
    }

    #[test]
    fn default_is_off() {
        let c = DeskflowCfg::default();
        assert!(!c.listen);
        assert_eq!(c.port, 24800);
        assert_eq!(c.edge_resistance, Resistance::Medium);
        assert!(c.devices.is_empty());
    }

    #[test]
    fn resistance_pixels() {
        let px: Vec<f64> = [Resistance::Off, Resistance::Low, Resistance::Medium, Resistance::High]
            .iter()
            .map(|r| r.px())
            .collect();
        assert_eq!(px, [0.0, 15.0, 40.0, 100.0]);
    }

    #[test]
    fn toml_roundtrip_matches_spec_block() {
        let text = "[deskflow]\nlisten = true\nport = 24800\n\n[[deskflow.device]]\nname = \"laptop\"\nfingerprint = \"v2:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\noutput = \"DP-1\"\nside = \"right-of\"\nclipboard = true\n";
        let c = crate::config::parse(text).unwrap();
        assert!(c.deskflow.listen);
        assert_eq!(c.deskflow.edge_resistance, Resistance::Medium);
        assert_eq!(c.deskflow.devices, vec![dev("laptop", "DP-1", Side::RightOf)]);
        let again = crate::config::parse(&crate::config::to_text(&c)).unwrap();
        assert_eq!(again, c);
        // A device without `clipboard` shares it.
        let c = crate::config::parse(
            "[[deskflow.device]]\nname = \"a\"\nfingerprint = \"x\"\noutput = \"o\"\nside = \"above\"\n",
        )
        .unwrap();
        assert!(c.deskflow.devices[0].clipboard);
        assert!(
            crate::config::parse("[[deskflow.device]]\nname=\"a\"\nfingerprint=\"x\"\noutput=\"o\"\nside=\"up\"\n")
                .is_err()
        );
    }

    #[test]
    fn render_parse_roundtrip() {
        let c = sample();
        let body = render(&c, true);
        assert!(
            body.starts_with("mic_mute_local = true\n")
                && body.contains("[[deskflow.device]]")
                && body.contains("side = \"right-of\""),
            "{body}"
        );
        let g = Generated {
            mic_mute_local: true,
            deskflow: c.clone(),
        };
        assert_eq!(parse(&body).unwrap(), g);
        let with_header = format!("{}\n{body}", crate::generated::HEADER);
        assert_eq!(parse(&with_header).unwrap(), g);
        assert_eq!(
            parse(&render(&DeskflowCfg::default(), false)).unwrap(),
            Generated::default()
        );
        // A file written before the key: the microphone key is not local.
        let old = body.replacen("mic_mute_local = true\n", "", 1);
        assert_eq!(parse(&old).unwrap().deskflow, c);
        assert!(!parse(&old).unwrap().mic_mute_local);
    }

    #[test]
    fn validate_accepts_a_good_config() {
        assert_eq!(validate(&sample()), Ok(()));
        assert_eq!(validate(&DeskflowCfg::default()), Ok(()));
    }

    #[test]
    fn validate_refuses_bad_fingerprint() {
        for bad in [
            "",
            "v2:sha256:abc",
            &FP.to_uppercase(),
            &FP.replace("v2:", "v1:"),
            &format!("{FP}0"),
        ] {
            let mut c = sample();
            c.devices[0].fingerprint = bad.into();
            assert!(validate(&c).unwrap_err().contains("fingerprint"), "{bad}");
        }
    }

    #[test]
    fn validate_refuses_duplicate_edge() {
        let mut c = sample();
        c.devices[1] = dev("pad", "DP-1", Side::RightOf);
        assert!(validate(&c).is_err());
        c.devices[1] = dev("pad", "DP-2", Side::RightOf);
        assert!(validate(&c).is_ok());
    }

    #[test]
    fn validate_refuses_duplicate_name() {
        let mut c = sample();
        c.devices[1].name = "laptop".into();
        assert!(validate(&c).unwrap_err().contains("twice"));
    }

    #[test]
    fn validate_refuses_bad_name_and_port() {
        let mut c = sample();
        c.devices[0].name = String::new();
        assert!(validate(&c).is_err());
        c.devices[0].name = "x".repeat(65);
        assert!(validate(&c).is_err());
        c.devices[0].name = "x".repeat(64);
        assert!(validate(&c).is_ok());
        c.port = 0;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn generated_mode_0600() {
        assert_eq!(crate::generated::mode_of("deskflow"), 0o600);
    }

    #[test]
    fn compose_defaults_false_and_roundtrips() {
        let absent = "[[deskflow.device]]\nname = \"a\"\nfingerprint = \"x\"\noutput = \"o\"\nside = \"above\"\n";
        let c = crate::config::parse(absent).unwrap();
        assert!(!c.deskflow.devices[0].compose);

        let on = format!("{absent}compose = true\n");
        let c = crate::config::parse(&on).unwrap();
        assert!(c.deskflow.devices[0].compose);
        let again = crate::config::parse(&crate::config::to_text(&c)).unwrap();
        assert!(again.deskflow.devices[0].compose);

        let g = parse(&render(&c.deskflow, false)).unwrap();
        assert!(g.deskflow.devices[0].compose);
    }

    #[test]
    fn enabled_defaults_true_and_roundtrips() {
        let absent = "[[deskflow.device]]\nname = \"a\"\nfingerprint = \"x\"\noutput = \"o\"\nside = \"above\"\n";
        let c = crate::config::parse(absent).unwrap();
        assert!(c.deskflow.devices[0].enabled);

        let off = format!("{absent}enabled = false\n");
        let c = crate::config::parse(&off).unwrap();
        assert!(!c.deskflow.devices[0].enabled);
        let again = crate::config::parse(&crate::config::to_text(&c)).unwrap();
        assert!(!again.deskflow.devices[0].enabled);

        let g = parse(&render(&c.deskflow, false)).unwrap();
        assert!(!g.deskflow.devices[0].enabled);
    }
}
