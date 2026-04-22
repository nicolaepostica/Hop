//! Self-signed mTLS setup with fingerprint-only peer verification.
//!
//! Input Leap does not use a PKI. Each host holds its own long-lived
//! self-signed certificate; peers trust each other by listing SHA-256
//! certificate fingerprints in a shared TOML database. This module
//! builds the [`rustls`] configs that implement that model.

use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{self, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};
use thiserror::Error;
use tracing::info;

use crate::fingerprint::{Fingerprint, FingerprintDb};

/// Default subject alternative name baked into generated certificates.
///
/// Peers never look at the name (trust is based on fingerprints), but
/// rustls still requires a valid SAN during the handshake.
pub const DEFAULT_CERT_SAN: &str = "input-leap";

/// Errors from the TLS setup helpers.
#[derive(Debug, Error)]
pub enum TlsError {
    /// I/O error reading or writing cert/key files.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Certificate generation via rcgen failed.
    #[error("failed to generate self-signed cert: {0}")]
    CertGen(String),
    /// The PEM bundle on disk was malformed.
    #[error("cert bundle missing or malformed at {path}")]
    MalformedPem {
        /// Path that was being parsed.
        path: PathBuf,
    },
    /// Couldn't build a rustls config (bad cert/key combination).
    #[error("rustls config error: {0}")]
    Rustls(#[from] rustls::Error),
}

/// Ensure the default ring-backed crypto provider is installed.
///
/// Idempotent and safe to call from multiple places / multiple threads.
/// Must run once before any rustls config is built.
pub fn install_default_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Err => another provider is already installed; that's fine.
        let _ = crypto::ring::default_provider().install_default();
    });
}

fn supported_verify_algs() -> &'static WebPkiSupportedAlgorithms {
    static ALGS: OnceLock<WebPkiSupportedAlgorithms> = OnceLock::new();
    ALGS.get_or_init(|| crypto::ring::default_provider().signature_verification_algorithms)
}

/// Load the local identity from `dir` (files `cert.pem` and `key.pem`),
/// generating a fresh self-signed pair if either is missing.
///
/// Returns the decoded cert chain + private key plus the file paths so
/// the caller can log and chmod them.
pub fn load_or_generate_cert(dir: &Path) -> Result<LoadedIdentity, TlsError> {
    fs::create_dir_all(dir)?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    if !cert_path.exists() || !key_path.exists() {
        info!(dir = %dir.display(), "generating self-signed certificate");
        let CertifiedKey { cert, key_pair } =
            generate_simple_self_signed(vec![DEFAULT_CERT_SAN.to_string()])
                .map_err(|e| TlsError::CertGen(e.to_string()))?;
        fs::write(&cert_path, cert.pem())?;
        fs::write(&key_path, key_pair.serialize_pem())?;
        restrict_permissions(&key_path)?;
    }

    let chain = load_cert_chain(&cert_path)?;
    let key = load_private_key(&key_path)?;
    let fingerprint = chain
        .first()
        .map(|der| Fingerprint::from_cert_der(der.as_ref()))
        .ok_or_else(|| TlsError::MalformedPem {
            path: cert_path.clone(),
        })?;

    Ok(LoadedIdentity {
        chain,
        key,
        fingerprint,
        cert_path,
        key_path,
    })
}

/// Parsed local TLS identity with accompanying metadata.
#[derive(Debug)]
pub struct LoadedIdentity {
    /// DER-encoded certificate chain (just the leaf for self-signed).
    pub chain: Vec<CertificateDer<'static>>,
    /// DER-encoded private key.
    pub key: PrivateKeyDer<'static>,
    /// SHA-256 fingerprint of the leaf certificate — what peers must trust.
    pub fingerprint: Fingerprint,
    /// Path where the cert was loaded from (or written to).
    pub cert_path: PathBuf,
    /// Path where the key was loaded from (or written to).
    pub key_path: PathBuf,
}

impl Clone for LoadedIdentity {
    fn clone(&self) -> Self {
        // `PrivateKeyDer` intentionally does not derive `Clone` because
        // copying key material should be a deliberate act; `clone_key`
        // is the sanctioned way.
        Self {
            chain: self.chain.clone(),
            key: self.key.clone_key(),
            fingerprint: self.fingerprint,
            cert_path: self.cert_path.clone(),
            key_path: self.key_path.clone(),
        }
    }
}

fn load_cert_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let chain: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
    if chain.is_empty() {
        return Err(TlsError::MalformedPem { path: path.into() });
    }
    Ok(chain)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| TlsError::MalformedPem { path: path.into() })
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    // On Windows the user profile directory already restricts access to
    // the owning user; finer-grained ACLs are deferred.
    Ok(())
}

// ----- verifier ---------------------------------------------------------

/// rustls verifier that ignores CA chains and accepts any self-signed
/// peer cert whose SHA-256 fingerprint is listed in the attached
/// [`FingerprintDb`].
#[derive(Debug)]
pub struct FingerprintVerifier {
    trusted: Arc<FingerprintDb>,
}

impl FingerprintVerifier {
    /// Build a verifier backed by an immutable snapshot of a DB.
    #[must_use]
    pub fn new(trusted: Arc<FingerprintDb>) -> Self {
        Self { trusted }
    }

    fn check_fingerprint(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let fp = Fingerprint::from_cert_der(cert.as_ref());
        if self.trusted.lookup(&fp).is_some() {
            Ok(())
        } else {
            Err(rustls::Error::General(format!(
                "peer fingerprint {fp} not in trust store"
            )))
        }
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check_fingerprint(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls12_signature(message, cert, dss, supported_verify_algs())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature(message, cert, dss, supported_verify_algs())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_verify_algs().supported_schemes()
    }
}

impl ClientCertVerifier for FingerprintVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.check_fingerprint(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls12_signature(message, cert, dss, supported_verify_algs())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature(message, cert, dss, supported_verify_algs())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_verify_algs().supported_schemes()
    }
}

// ----- config builders --------------------------------------------------

/// Build a rustls server config that requires the client to present a
/// certificate whose fingerprint is in `trusted`.
pub fn build_server_config(
    identity: &LoadedIdentity,
    trusted: Arc<FingerprintDb>,
) -> Result<ServerConfig, TlsError> {
    install_default_crypto_provider();
    let verifier = Arc::new(FingerprintVerifier::new(trusted));
    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(identity.chain.clone(), identity.key.clone_key())?;
    Ok(config)
}

/// Build a rustls client config that presents our certificate and
/// verifies the server's fingerprint against `trusted`.
pub fn build_client_config(
    identity: &LoadedIdentity,
    trusted: Arc<FingerprintDb>,
) -> Result<ClientConfig, TlsError> {
    install_default_crypto_provider();
    let verifier = Arc::new(FingerprintVerifier::new(trusted));
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(identity.chain.clone(), identity.key.clone_key())?;
    Ok(config)
}
