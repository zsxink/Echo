//! Trust the paired host key rather than the discovered network address.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};

#[derive(Debug)]
struct PinnedHostVerifier {
    public_key: SubjectPublicKeyInfoDer<'static>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedHostVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let public_key = ParsedCertificate::try_from(end_entity)?.subject_public_key_info();
        if public_key != self.public_key {
            return Err(Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        // Pairing provisions this key directly. CA chains, expiry, and SANs are
        // not its identity: TLS CertificateVerify proves possession below.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("LAN sync requires TLS 1.3".to_owned()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(super) fn pinned_config(certificate_der: &[u8]) -> Result<ClientConfig, Error> {
    let certificate = CertificateDer::from(certificate_der);
    let public_key = ParsedCertificate::try_from(&certificate)?.subject_public_key_info();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinnedHostVerifier {
        public_key,
        provider: Arc::clone(&provider),
    };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}
