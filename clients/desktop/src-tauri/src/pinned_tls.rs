//! Trust exactly one certificate: the server's own, named by its SHA-256 in the pairing string
//! (ADR 0034). Used while the server has no publicly trusted certificate, so the app can reach it by
//! address over HTTPS. The pin is the server's identity: the name and the issuer are not checked, the
//! handshake signature is.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// In the error when the server's certificate is not the pinned one; `is_pin_mismatch` looks for it.
const MISMATCH: &str = "askbar: the server's certificate is not the one this app was paired with";

/// An HTTP client builder that trusts only the certificate with this SHA-256 when `pin` is set, and
/// the usual public roots when it is not.
pub fn client_builder(pin: Option<&str>) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    let Some(pin) = pin else { return builder };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { sha256: pin.to_ascii_lowercase(), provider }))
        .with_no_client_auth();
    builder.use_preconfigured_tls(config)
}

/// Whether a failed request failed because the server showed a different certificate than the pinned one.
pub fn is_pin_mismatch(e: &reqwest::Error) -> bool {
    let mut source: Option<&dyn std::error::Error> = Some(e);
    while let Some(s) = source {
        if s.to_string().contains(MISMATCH) {
            return true;
        }
        source = s.source();
    }
    false
}

/// SHA-256 of a DER certificate, as lowercase hex.
pub fn sha256_hex(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug)]
struct Pinned {
    sha256: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if sha256_hex(end_entity) == self.sha256 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(MISMATCH.into()))
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rustls::pki_types::PrivateKeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-route HTTPS server with its own certificate: answers every request with 200. Returns its
    /// address and the certificate's SHA-256.
    pub async fn self_signed_server() -> (String, String) {
        let made = rcgen::generate_simple_self_signed(vec!["askbar.internal".to_string()]).unwrap();
        let fp = sha256_hex(made.cert.der());
        let key = PrivateKeyDer::Pkcs8(made.signing_key.serialize_der().into());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![made.cert.der().clone()], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                    let mut buf = [0u8; 4096];
                    let _ = tls.read(&mut buf).await;
                    let _ = tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok").await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        (format!("https://{addr}"), fp)
    }

    #[tokio::test]
    async fn the_pinned_certificate_is_trusted_by_address() {
        let (url, fp) = self_signed_server().await;
        let http = client_builder(Some(&fp.to_uppercase())).build().unwrap();
        let r = http.get(format!("{url}/api/client/v1/health")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn another_certificate_is_refused_and_recognised_as_a_changed_one() {
        let (url, _) = self_signed_server().await;
        let http = client_builder(Some(&"0".repeat(64))).build().unwrap();
        let e = http.get(&url).send().await.unwrap_err();
        assert!(is_pin_mismatch(&e), "{e:?}");
    }

    #[tokio::test]
    async fn without_a_pin_a_self_signed_certificate_is_refused() {
        let (url, _) = self_signed_server().await;
        let e = client_builder(None).build().unwrap().get(&url).send().await.unwrap_err();
        assert!(!is_pin_mismatch(&e), "an unknown issuer, not a changed pin: {e:?}");
    }
}
