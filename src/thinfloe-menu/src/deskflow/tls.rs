//! The Deskflow server's TLS: rustls with ring, TLS 1.2 and 1.3, and a
//! client certificate always required (spec, Identity and pairing). The
//! client certificate is not chain-checked, since Deskflow's are
//! self-signed; the daemon decides trust by its fingerprint.

use std::sync::Arc;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error, ServerConfig, ServerConnection, SignatureScheme};

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
/// verifies the client's handshake signature with it, so the client holds
/// the key of the certificate whose fingerprint the daemon trusts.
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
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
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
