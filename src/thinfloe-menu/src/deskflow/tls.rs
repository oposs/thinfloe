//! The Deskflow server's TLS: rustls with ring, TLS 1.2 and 1.3, and a
//! client certificate always required (spec, Identity and pairing). The
//! client certificate is not chain-checked, since Deskflow's are
//! self-signed; the daemon decides trust by its fingerprint. Only its public
//! key is read: the Deskflow GUI makes X.509 v1 certificates, which webpki
//! (and so rustls's own signature check) refuses.

use std::sync::Arc;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::SubjectPublicKeyInfoDer;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, PeerMisbehaved, ServerConfig, ServerConnection,
    SignatureScheme,
};

use super::ident::{self, Identity};

/// The server config for ThinFloe's identity. `ident::load_or_create` has
/// checked that the key belongs to the certificate, so building it cannot
/// fail.
pub fn server_config(id: &Identity) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der.clone()));
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .expect("ring supports TLS 1.2 and 1.3")
        .with_client_cert_verifier(Arc::new(AnyClient::new()))
        .with_single_cert(vec![CertificateDer::from(id.cert_der.clone())], key)
        .expect("the identity was checked when it was loaded");
    Arc::new(cfg)
}

/// The fingerprint of the certificate the client presented, once the
/// handshake has received it.
pub fn peer_fingerprint(conn: &ServerConnection) -> Option<String> {
    conn.peer_certificates()?
        .first()
        .map(|c| ident::fingerprint(c.as_ref()))
}

/// Takes any client certificate without a chain or name check, but still
/// verifies the client's handshake signature with its public key, so the
/// client holds the key of the certificate whose fingerprint the daemon
/// trusts. The certificate's version is not checked.
#[derive(Debug)]
struct AnyClient(Arc<CryptoProvider>);

impl AnyClient {
    fn new() -> AnyClient {
        AnyClient(Arc::new(rustls::crypto::ring::default_provider()))
    }
}

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.verify_tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        let spki = cert_spki(cert.as_ref())
            .map(SubjectPublicKeyInfoDer::from)
            .ok_or(Error::InvalidCertificate(CertificateError::BadEncoding))?;
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &spki,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl AnyClient {
    /// Checks a TLS 1.2 `dss` over `message` with the public key of `cert`.
    /// rustls has no raw-key check for TLS 1.2: a TLS 1.2 scheme can name
    /// several algorithms (ECDSA does not name the curve), and those that do
    /// not fit the key are skipped, as rustls's own check does.
    fn verify_tls12(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        let bad = |e| Error::InvalidCertificate(e);
        let spki = cert_spki(cert.as_ref())
            .map(SubjectPublicKeyInfoDer::from)
            .ok_or(bad(CertificateError::BadEncoding))?;
        let key = webpki::RawPublicKeyEntity::try_from(&spki).map_err(|_| bad(CertificateError::BadEncoding))?;
        let algs = self
            .0
            .signature_verification_algorithms
            .mapping
            .iter()
            .find(|(s, _)| *s == dss.scheme)
            .map(|(_, algs)| *algs)
            .ok_or(Error::PeerMisbehaved(
                PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme,
            ))?;
        for alg in algs {
            #[allow(deprecated)]
            match key.verify_signature(*alg, message, dss.signature()) {
                Ok(()) => return Ok(HandshakeSignatureValid::assertion()),
                Err(
                    webpki::Error::UnsupportedSignatureAlgorithmForPublicKeyContext(_)
                    | webpki::Error::UnsupportedSignatureAlgorithmForPublicKey,
                ) => continue,
                Err(webpki::Error::InvalidSignatureForPublicKey) => return Err(bad(CertificateError::BadSignature)),
                Err(_) => return Err(bad(CertificateError::BadEncoding)),
            }
        }
        Err(bad(CertificateError::BadSignature))
    }
}

/// The SubjectPublicKeyInfo of an X.509 certificate of any version, as DER
/// with its own header: the seventh element of the TBSCertificate, after the
/// optional `[0]` version, serial, signature, issuer, validity and subject.
fn cert_spki(cert: &[u8]) -> Option<&[u8]> {
    let (0x30, body, []) = der_element(cert)? else {
        return None;
    };
    let (0x30, tbs, _) = der_element(body)? else {
        return None;
    };
    let mut rest = tbs;
    if let (0xa0, _, after) = der_element(rest)? {
        rest = after;
    }
    for want in [0x02, 0x30, 0x30, 0x30, 0x30] {
        let (tag, _, after) = der_element(rest)?;
        if tag != want {
            return None;
        }
        rest = after;
    }
    let (0x30, _, after) = der_element(rest)? else {
        return None;
    };
    Some(&rest[..rest.len() - after.len()])
}

/// One DER element at the start of `b`: its tag, its contents and what
/// follows it. Lengths up to four bytes; webpki checks the SPKI strictly.
fn der_element(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&first, mut b) = b.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || b.len() < n {
            return None;
        }
        let (bytes, after) = b.split_at(n);
        b = after;
        bytes.iter().fold(0usize, |a, &x| (a << 8) | usize::from(x))
    };
    if b.len() < len {
        return None;
    }
    let (contents, after) = b.split_at(len);
    Some((tag, contents, after))
}

/// A client for the tests that takes any server certificate, as a
/// Deskflow client that trusts the server by fingerprint does.
#[cfg(test)]
pub(crate) mod testclient {
    use std::sync::Arc;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::CryptoProvider;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
    use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};

    /// A Deskflow client trusts the server by fingerprint too; the test
    /// client takes any server certificate (signatures still checked).
    #[derive(Debug)]
    pub(crate) struct AnyServer(pub(crate) Arc<CryptoProvider>);

    impl ServerCertVerifier for AnyServer {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    /// TLS 1.3 and 1.2, with this client certificate.
    pub(crate) fn config(cert: &rcgen::CertifiedKey<rcgen::KeyPair>) -> Arc<ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyServer(provider)))
            .with_client_auth_cert(
                vec![cert.cert.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der())),
            )
            .unwrap();
        Arc::new(cfg)
    }

    pub(crate) fn server_name() -> ServerName<'static> {
        ServerName::try_from("thinfloe").unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deskflow::ident::{self, fingerprint};
    use crate::testdir::TestDir;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, Error, SignatureScheme};

    use super::testclient::AnyServer;

    fn server_identity(tag: &str) -> (TestDir, ident::Identity) {
        let d = TestDir::new("deskflow-tls", tag);
        let id = ident::load_or_create(&d.join("identity.pem"), "server").unwrap();
        (d, id)
    }

    fn client(
        cert: Option<&rcgen::CertifiedKey<rcgen::KeyPair>>,
        version: &'static rustls::SupportedProtocolVersion,
    ) -> ClientConnection {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let b = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[version])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyServer(provider)));
        let cfg = match cert {
            None => b.with_no_client_auth(),
            Some(c) => b
                .with_client_auth_cert(
                    vec![c.cert.der().clone()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(c.signing_key.serialize_der())),
                )
                .unwrap(),
        };
        ClientConnection::new(Arc::new(cfg), ServerName::try_from("thinfloe").unwrap()).unwrap()
    }

    /// Shuttle records between the two ends until both are done or one
    /// fails; the server's error, if any.
    fn handshake(c: &mut ClientConnection, s: &mut ServerConnection) -> Result<(), Error> {
        for _ in 0..20 {
            let mut buf = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut buf).unwrap();
            }
            if !buf.is_empty() {
                s.read_tls(&mut buf.as_slice()).unwrap();
                s.process_new_packets()?;
            }
            let mut buf = Vec::new();
            while s.wants_write() {
                s.write_tls(&mut buf).unwrap();
            }
            if !buf.is_empty() {
                c.read_tls(&mut buf.as_slice()).unwrap();
                // A client error shows on the server's next read.
                let _ = c.process_new_packets();
            }
            if !c.is_handshaking() && !s.is_handshaking() && !c.wants_write() {
                return Ok(());
            }
        }
        panic!("the handshake did not finish");
    }

    fn self_signed(name: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
        rcgen::generate_simple_self_signed(vec![name.to_string()]).unwrap()
    }

    #[test]
    fn tls_handshake_needs_client_cert() {
        let (_d, id) = server_identity("needs");
        let cfg = server_config(&id);
        for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
            let mut c = client(None, version);
            let mut s = ServerConnection::new(cfg.clone()).unwrap();
            let err = handshake(&mut c, &mut s).unwrap_err();
            assert!(matches!(err, Error::NoCertificatesPresented), "{version:?}: {err:?}");
            assert_eq!(peer_fingerprint(&s), None);

            let cert = self_signed("laptop");
            let mut c = client(Some(&cert), version);
            let mut s = ServerConnection::new(cfg.clone()).unwrap();
            handshake(&mut c, &mut s).unwrap();
            assert_eq!(peer_fingerprint(&s), Some(fingerprint(cert.cert.der())));
            assert_eq!(s.protocol_version(), Some(version.version));
        }
    }

    /// A certificate as the Deskflow GUI makes one (`generatePemSelfSignedCert`
    /// in Deskflow 1.26): RSA 2048, signed with SHA-256, and no version set,
    /// so X.509 v1 without extensions. Made with `openssl req -new` and
    /// `openssl x509 -req -signkey`, which write v1 the same way.
    const DESKFLOW_V1_CERT: &[u8] = include_bytes!("../../tests/fixtures/deskflow-v1-cert.der");
    const DESKFLOW_V1_KEY: &[u8] = include_bytes!("../../tests/fixtures/deskflow-v1-key.der");

    #[test]
    fn a_deskflow_gui_v1_certificate_is_taken() {
        let (_d, id) = server_identity("v1");
        let cfg = server_config(&id);
        let cert = CertificateDer::from(DESKFLOW_V1_CERT.to_vec());
        for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            // rustls's own client refuses to load a v1 certificate, so it
            // is handed over by a resolver that does not check it.
            let key = provider
                .key_provider
                .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(DESKFLOW_V1_KEY.to_vec())))
                .unwrap();
            let resolver = WrongKey(Arc::new(rustls::sign::CertifiedKey::new(vec![cert.clone()], key)));
            let ccfg = ClientConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&[version])
                .unwrap()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AnyServer(provider)))
                .with_client_cert_resolver(Arc::new(resolver));
            let mut c = ClientConnection::new(Arc::new(ccfg), ServerName::try_from("thinfloe").unwrap()).unwrap();
            let mut s = ServerConnection::new(cfg.clone()).unwrap();
            handshake(&mut c, &mut s).unwrap_or_else(|e| panic!("{version:?}: {e:?}"));
            assert_eq!(peer_fingerprint(&s), Some(fingerprint(DESKFLOW_V1_CERT)));
        }
    }

    #[test]
    fn the_public_key_is_found_in_v1_and_v3_certificates() {
        let v1_spki = include_bytes!("../../tests/fixtures/deskflow-v1-spki.der");
        assert_eq!(cert_spki(DESKFLOW_V1_CERT), Some(&v1_spki[..]));
        let v3 = self_signed("laptop");
        assert_eq!(
            cert_spki(v3.cert.der()),
            Some(&rcgen::PublicKeyData::subject_public_key_info(&v3.signing_key)[..])
        );
        for cut in [0, 1, 2, 10, DESKFLOW_V1_CERT.len() / 2, DESKFLOW_V1_CERT.len() - 1] {
            assert_eq!(cert_spki(&DESKFLOW_V1_CERT[..cut]), None, "cut at {cut}");
        }
        let mut trailing = DESKFLOW_V1_CERT.to_vec();
        trailing.push(0);
        assert_eq!(cert_spki(&trailing), None);
    }

    #[test]
    fn the_client_sees_the_identity_certificate() {
        let (_d, id) = server_identity("server-cert");
        let cert = self_signed("laptop");
        let mut c = client(Some(&cert), &rustls::version::TLS13);
        let mut s = ServerConnection::new(server_config(&id)).unwrap();
        handshake(&mut c, &mut s).unwrap();
        let seen = c.peer_certificates().unwrap()[0].as_ref().to_vec();
        assert_eq!(seen, id.cert_der);
    }

    /// Presents one certificate but signs with another key.
    #[derive(Debug)]
    struct WrongKey(Arc<rustls::sign::CertifiedKey>);

    impl rustls::client::ResolvesClientCert for WrongKey {
        fn resolve(&self, _: &[&[u8]], _: &[SignatureScheme]) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
        fn has_certs(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_client_signing_with_another_key_is_refused() {
        let (_d, id) = server_identity("wrong-key");
        let cfg = server_config(&id);
        let shown = self_signed("laptop");
        let other = self_signed("other");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = provider
            .key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                other.signing_key.serialize_der(),
            )))
            .unwrap();
        let resolver = WrongKey(Arc::new(rustls::sign::CertifiedKey::new(
            vec![shown.cert.der().clone()],
            key,
        )));
        let resolver = Arc::new(resolver);
        for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
            let ccfg = ClientConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&[version])
                .unwrap()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AnyServer(provider.clone())))
                .with_client_cert_resolver(resolver.clone());
            let mut c = ClientConnection::new(Arc::new(ccfg), ServerName::try_from("thinfloe").unwrap()).unwrap();
            let mut s = ServerConnection::new(cfg.clone()).unwrap();
            let err = handshake(&mut c, &mut s).unwrap_err();
            assert!(
                matches!(err, Error::InvalidCertificate(rustls::CertificateError::BadSignature)),
                "{version:?}: {err:?}"
            );
        }
    }
}
