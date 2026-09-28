//! A machine's identity on the split-editor wire: a self-signed certificate (per machine,
//! kept in the user config — a paired device is a per-machine credential, Ch.37 §37.6) and
//! the QUIC/TLS configurations that use it.
//!
//! There is no certificate authority. A peer is known by the **SHA-256 of its certificate**
//! (its fingerprint): pairing exchanges fingerprints under a key both sides derived from the
//! one-time code ([`crate::pairing`]); afterwards the device pins the host's fingerprint and
//! the host admits only paired fingerprints. TLS 1.3 only; the crypto provider is `ring`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::{ALPN, RemoteError, hex, unhex};

/// The host's certificate subject (the verifier pins fingerprints, not names).
pub const HOST_NAME: &str = "forge-host";

/// A certificate and its private key (see the module docs).
#[derive(Clone)]
pub struct Identity {
    cert: Vec<u8>,
    key: Vec<u8>,
    fingerprint: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    cert: String,
    key: String,
}

/// The SHA-256 of `cert`, lowercase hex.
#[must_use]
pub fn fingerprint(cert: &[u8]) -> String {
    hex(&sha2::Sha256::digest(cert))
}

/// A short, human-comparable form of a fingerprint (`3f9a 1c07 …`), for the UI.
#[must_use]
pub fn short(fp: &str) -> String {
    fp.as_bytes()
        .chunks(4)
        .take(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

impl Identity {
    /// A fresh identity whose certificate names `subject`.
    pub fn generate(subject: &str) -> Result<Self, RemoteError> {
        let c = rcgen::generate_simple_self_signed(vec![subject.to_string()])
            .map_err(|e| RemoteError::Tls(e.to_string()))?;
        let cert = c.cert.der().to_vec();
        let key = c.signing_key.serialize_der();
        Ok(Self {
            fingerprint: fingerprint(&cert),
            cert,
            key,
        })
    }

    /// The identity kept in `<dir>/<file>`, made (and kept) on first use. `None`: a fresh
    /// identity kept nowhere (tests).
    pub fn load_or_create(
        dir: Option<&Path>,
        file: &str,
        subject: &str,
    ) -> Result<Self, RemoteError> {
        let Some(dir) = dir else {
            return Self::generate(subject);
        };
        let path = dir.join(file);
        let cfg = |e: String| RemoteError::Config(format!("{}: {e}", path.display()));
        match forge_editor::user_config::read_optional(&path).map_err(|e| cfg(e.to_string()))? {
            Some(text) => {
                let f: IdentityFile = ron::from_str(&text).map_err(|e| cfg(e.to_string()))?;
                let cert =
                    unhex(&f.cert).ok_or_else(|| cfg("the certificate is not hex".into()))?;
                let key = unhex(&f.key).ok_or_else(|| cfg("the key is not hex".into()))?;
                Ok(Self {
                    fingerprint: fingerprint(&cert),
                    cert,
                    key,
                })
            }
            None => {
                let id = Self::generate(subject)?;
                let text = ron::to_string(&IdentityFile {
                    cert: hex(&id.cert),
                    key: hex(&id.key),
                })
                .map_err(|e| cfg(e.to_string()))?;
                forge_editor::user_config::write_atomic(&path, text.as_bytes())
                    .map_err(|e| cfg(e.to_string()))?;
                Ok(id)
            }
        }
    }

    /// The SHA-256 of the certificate, lowercase hex.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    fn cert_der(&self) -> CertificateDer<'static> {
        CertificateDer::from(self.cert.clone())
    }

    fn key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    // An idle split editor stays connected (keep-alives are the QUIC stack's, on its own
    // thread: they never wake the UI), and a dead peer is noticed within a minute.
    t.keep_alive_interval(Some(Duration::from_secs(10)));
    if let Ok(idle) = quinn::IdleTimeout::try_from(Duration::from_secs(60)) {
        t.max_idle_timeout(Some(idle));
    }
    Arc::new(t)
}

/// The host's QUIC server configuration: its certificate, and client certificates required
/// (any self-signed one is accepted at the handshake; the host then admits paired
/// fingerprints only).
pub fn server_config(id: &Identity) -> Result<quinn::ServerConfig, RemoteError> {
    let p = provider();
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::clone(&p))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| RemoteError::Tls(e.to_string()))?
        .with_client_cert_verifier(Arc::new(AnyClientCert { provider: p }))
        .with_single_cert(vec![id.cert_der()], id.key_der())
        .map_err(|e| RemoteError::Tls(e.to_string()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| RemoteError::Tls(e.to_string()))?;
    let mut c = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    c.transport_config(transport());
    Ok(c)
}

/// What the device saw of the host's certificate.
pub type SeenHost = Arc<Mutex<Option<String>>>;

/// A device's QUIC client configuration: its own certificate, and the host's pinned
/// (`expect`), or — only while pairing — whatever the host presents, recorded in the
/// returned cell for the key confirmation to bind.
pub fn client_config(
    id: &Identity,
    expect: Option<&str>,
) -> Result<(quinn::ClientConfig, SeenHost), RemoteError> {
    let p = provider();
    let seen: SeenHost = Arc::new(Mutex::new(None));
    let verifier = PinnedHost {
        expect: expect.map(str::to_string),
        seen: Arc::clone(&seen),
        provider: Arc::clone(&p),
    };
    let mut tls = rustls::ClientConfig::builder_with_provider(p)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| RemoteError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(vec![id.cert_der()], id.key_der())
        .map_err(|e| RemoteError::Tls(e.to_string()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| RemoteError::Tls(e.to_string()))?;
    let mut c = quinn::ClientConfig::new(Arc::new(crypto));
    c.transport_config(transport());
    Ok((c, seen))
}

/// The fingerprint of a connection's peer certificate.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Option<String> {
    let any = conn.peer_identity()?;
    let certs = any.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certs.first().map(|c| fingerprint(c.as_ref()))
}

#[derive(Debug)]
struct AnyClientCert {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClientCert {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        // Who the device is is decided by its fingerprint, after the handshake proved it
        // holds the key (the signature checks below).
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct PinnedHost {
    expect: Option<String>,
    seen: SeenHost,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedHost {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(fp.clone());
        }
        match &self.expect {
            Some(want) if *want != fp => Err(rustls::Error::General(format!(
                "the host presented certificate {} but the paired host is {}",
                short(&fp),
                short(want)
            ))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_is_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("forge-remote-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Identity::load_or_create(Some(&dir), "id.ron", HOST_NAME).expect("made");
        let b = Identity::load_or_create(Some(&dir), "id.ron", HOST_NAME).expect("read");
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint().len(), 64);
        let c = Identity::generate(HOST_NAME).expect("fresh");
        assert_ne!(a.fingerprint(), c.fingerprint());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
