//! ThinFloe's own TLS identity for the Deskflow server (spec, Identity
//! and pairing): a self-signed ECDSA P-256 certificate and its key in one
//! PEM file, and the fingerprint forms the user compares with what the
//! Deskflow client shows.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The largest identity file read: a P-256 key and certificate take about
/// 1 KiB.
const IDENTITY_MAX: u64 = 64 * 1024;
/// How long the certificate is valid: ten years.
const VALIDITY: Duration = Duration::from_secs(3652 * 24 * 3600);
/// The prefix of a fingerprint, as Deskflow writes it in its trust files.
const FP_PREFIX: &str = "v2:sha256:";

/// ThinFloe's certificate and its PKCS#8 private key, both DER.
pub struct Identity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

/// The identity in `path`, made and stored there (directory 0700 when it is
/// created, file 0600) when there is none. A file that does not hold a
/// certificate and its matching key is an error and is left as it is: a
/// new identity would make every client distrust ThinFloe.
pub fn load_or_create(path: &Path, hostname: &str) -> Result<Identity, String> {
    let shown = crate::atomic::home_relative(path);
    if let Some(bytes) = crate::atomic::read(path, IDENTITY_MAX).map_err(|e| format!("{shown}: {e}"))? {
        return parse(&bytes).map_err(|e| format!("{shown}: {e}"));
    }
    let (id, pem) = generate(hostname)?;
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("{}: {e}", crate::atomic::home_relative(dir)))?;
    }
    crate::atomic::write(path, pem.as_bytes(), 0o600).map_err(|e| format!("{shown}: {e}"))?;
    Ok(id)
}

/// A new identity and its PEM text (certificate, then key).
fn generate(hostname: &str) -> Result<(Identity, String), String> {
    let err = |e: rcgen::Error| format!("making the certificate: {e}");
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(err)?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(err)?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, format!("ThinFloe {hostname}"));
    params.distinguished_name = dn;
    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    params.not_before = rcgen::date_time_ymd(today.year().into(), today.month() as u8, today.day() as u8);
    params.not_after = params.not_before + VALIDITY;
    let cert = params.self_signed(&key).map_err(err)?;
    let pem = format!("{}{}", cert.pem(), key.serialize_pem());
    let id = Identity {
        cert_der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    };
    Ok((id, pem))
}

/// The identity in a PEM file's bytes; the key must belong to the
/// certificate, so the server config built from it cannot fail.
fn parse(pem: &[u8]) -> Result<Identity, String> {
    let cert = CertificateDer::from_pem_slice(pem).map_err(|e| format!("no certificate: {e}"))?;
    let key = PrivatePkcs8KeyDer::from_pem_slice(pem).map_err(|e| format!("no PKCS#8 key: {e}"))?;
    let id = Identity {
        cert_der: cert.as_ref().to_vec(),
        key_der: key.secret_pkcs8_der().to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::sign::CertifiedKey::from_der(vec![cert], PrivateKeyDer::Pkcs8(key), &provider)
        .map_err(|e| format!("unusable key: {e}"))?;
    Ok(id)
}

/// The fingerprint of a DER certificate as Deskflow writes it:
/// `v2:sha256:` and the SHA-256 in lowercase hex.
pub fn fingerprint(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    let mut s = String::with_capacity(FP_PREFIX.len() + 64);
    s.push_str(FP_PREFIX);
    for b in digest.as_ref() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The hex digits of a fingerprint, with or without its prefix.
fn fp_hex(fp: &str) -> &str {
    fp.strip_prefix(FP_PREFIX).unwrap_or(fp)
}

/// The fingerprint as Deskflow's dialogs show it: uppercase hex, the bytes
/// joined by colons (`BA:78:16:...`).
pub fn colon_hex(fp: &str) -> String {
    let hex = fp_hex(fp).to_ascii_uppercase();
    let pairs: Vec<&str> = hex
        .as_bytes()
        .chunks(2)
        .filter_map(|p| std::str::from_utf8(p).ok())
        .collect();
    pairs.join(":")
}

/// The fingerprint's digest bytes; `None` for anything but pairs of hex
/// digits.
fn digest_bytes(fp: &str) -> Option<Vec<u8>> {
    let hex = fp_hex(fp).as_bytes();
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}

/// Columns and rows of the random-art field.
const ART_W: usize = 17;
const ART_H: usize = 9;
/// A cell's symbol by how often the walk visited it; the last two mark
/// the start and the end.
const ART_SYMBOLS: [char; 17] = [
    ' ', '.', 'o', '+', '=', '*', 'B', 'O', 'X', '@', '%', '&', '#', '/', '^', 'S', 'E',
];

/// The fingerprint as Deskflow's random-art picture: 11 lines of 19
/// characters, a box around a 17 x 9 field. An algorithm port of
/// `generateFingerprintArt` in Deskflow's `src/lib/net/SecureUtils.cpp`
/// (commit 80221f0), the OpenSSH "drunken bishop": from the middle of the
/// field each byte of the digest, low bits first, gives four diagonal
/// steps (bit 0 right or left, bit 1 down or up), clamped at the border;
/// each visit counts up in its cell, to at most 14. A fingerprint that is
/// not hex draws an empty field.
pub fn random_art(fp: &str) -> Vec<String> {
    let start = ART_SYMBOLS.len() - 2;
    let end = ART_SYMBOLS.len() - 1;
    let mut field = [[0usize; ART_H]; ART_W];
    let (mut x, mut y) = (ART_W / 2, ART_H / 2);
    for byte in digest_bytes(fp).unwrap_or_default() {
        let mut bits = byte;
        for _ in 0..4 {
            x = if bits & 1 != 0 {
                (x + 1).min(ART_W - 1)
            } else {
                x.saturating_sub(1)
            };
            y = if bits & 2 != 0 {
                (y + 1).min(ART_H - 1)
            } else {
                y.saturating_sub(1)
            };
            if field[x][y] < start - 1 {
                field[x][y] += 1;
            }
            bits >>= 2;
        }
    }
    field[ART_W / 2][ART_H / 2] = start;
    field[x][y] = end;

    let bar = "═".repeat(ART_W);
    let mut lines = Vec::with_capacity(ART_H + 2);
    lines.push(format!("╔{bar}╗"));
    for row in 0..ART_H {
        let cells: String = (0..ART_W).map(|col| ART_SYMBOLS[field[col][row]]).collect();
        lines.push(format!("║{cells}║"));
    }
    lines.push(format!("╚{bar}╝"));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn create_then_load_same_fingerprint() {
        let d = TestDir::new("deskflow-ident", "reload");
        let path = d.join("deskflow/identity.pem");
        let made = load_or_create(&path, "host-a").unwrap();
        let loaded = load_or_create(&path, "host-b").unwrap();
        assert_eq!(fingerprint(&made.cert_der), fingerprint(&loaded.cert_der));
        assert_eq!(made.cert_der, loaded.cert_der);
        assert_eq!(made.key_der, loaded.key_der);
    }

    #[test]
    fn the_certificate_names_thinfloe_and_the_host() {
        let d = TestDir::new("deskflow-ident", "cn");
        let id = load_or_create(&d.join("identity.pem"), "thinfloe-0a1b2c").unwrap();
        let cn = b"ThinFloe thinfloe-0a1b2c";
        assert!(id.cert_der.windows(cn.len()).any(|w| w == cn));
    }

    #[test]
    fn identity_file_mode_0600() {
        let d = TestDir::new("deskflow-ident", "mode");
        let dir = d.join("deskflow");
        let path = dir.join("identity.pem");
        load_or_create(&path, "host").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn a_damaged_identity_is_an_error_and_stays() {
        let d = TestDir::new("deskflow-ident", "damaged");
        let path = d.join("identity.pem");
        fs::write(&path, "not a pem\n").unwrap();
        assert!(load_or_create(&path, "host").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not a pem\n");
    }

    #[test]
    fn a_key_of_another_certificate_is_an_error() {
        let d = TestDir::new("deskflow-ident", "mismatch");
        let a = d.join("a.pem");
        let b = d.join("b.pem");
        load_or_create(&a, "a").unwrap();
        load_or_create(&b, "b").unwrap();
        let a = fs::read_to_string(&a).unwrap();
        let b = fs::read_to_string(&b).unwrap();
        let cert_a = &a[..a.find("-----BEGIN PRIVATE KEY").unwrap()];
        let key_b = &b[b.find("-----BEGIN PRIVATE KEY").unwrap()..];
        let mixed = d.join("mixed.pem");
        fs::write(&mixed, format!("{cert_a}{key_b}")).unwrap();
        assert!(load_or_create(&mixed, "host").is_err());
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint(b"any bytes");
        let hex = fp.strip_prefix("v2:sha256:").unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    }

    #[test]
    fn fingerprint_known_vector() {
        assert_eq!(
            fingerprint(b"abc"),
            "v2:sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn colon_hex_shape() {
        let c = colon_hex(&fingerprint(b"abc"));
        assert_eq!(c.len(), 95);
        assert!(c.starts_with("BA:78:16:BF:"));
        assert!(c.ends_with(":00:15:AD"));
        assert!(!c.bytes().any(|b| b.is_ascii_lowercase()));
    }

    #[test]
    fn random_art_shape() {
        let art = random_art(&fingerprint(b"abc"));
        assert_eq!(art.len(), 11);
        for line in &art {
            assert_eq!(line.chars().count(), 19, "{line:?}");
        }
        assert_eq!(art[0], "╔═════════════════╗");
        assert_eq!(art[10], "╚═════════════════╝");
        assert!(art[1..10].iter().all(|l| l.starts_with('║') && l.ends_with('║')));
        // The walk starts in the middle of the field.
        assert_eq!(art[5].chars().nth(9), Some('S'));
        assert_eq!(art.concat().matches('E').count(), 1);
        assert_eq!(random_art(&fingerprint(b"abc")), art);
    }

    #[test]
    fn random_art_follows_deskflows_walk() {
        // One byte 0b11_11_11_11 walks down-right four times from the
        // middle (8, 4): it visits (9, 5), (10, 6), (11, 7) and ends on
        // (12, 8).
        let art = random_art("v2:sha256:ff");
        let row = |y: usize| art[y + 1].chars().skip(1).take(17).collect::<String>();
        assert_eq!(row(4), "        S        ");
        assert_eq!(row(5), "         .       ");
        assert_eq!(row(6), "          .      ");
        assert_eq!(row(7), "           .     ");
        assert_eq!(row(8), "            E    ");
        // A walk into the corner stays there and stacks up.
        let art = random_art(&format!("v2:sha256:{}", "00".repeat(8)));
        assert_eq!(art[1].chars().nth(1), Some('E'));
    }
}
