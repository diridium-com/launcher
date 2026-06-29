// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

//! Per-connection TLS certificate pinning (trust-on-first-use).
//!
//! Mirth/OIE servers almost always present self-signed certs, so chain
//! validation is the wrong model. Instead we pin the server's leaf certificate
//! per connection: on first connect we capture and show its SHA-256 for the
//! operator to approve, then on every later connect the TLS handshake must
//! present a cert whose leaf SHA-256 matches the stored pin or it is rejected.
//!
//! The verifiers still validate the handshake signature (possession of the
//! private key) via the ring provider — pinning the public cert bytes alone
//! would let anyone replay them. Hostname/SAN is intentionally NOT checked; the
//! pin replaces it, which is what we want for self-signed certs that rarely
//! carry a SAN matching the configured address.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Error, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};

/// Details of a server leaf certificate, shown to the user in the trust prompt.
/// `sha256` (hex) is the security-relevant value to verify out-of-band; the rest
/// is self-asserted context.
#[derive(Debug, Clone, Serialize)]
pub struct CertInfo {
    pub sha256: String,
    pub subject: String,
    pub issuer: String,
    pub not_after: String,
}

/// The process-wide ring crypto provider. Built explicitly (not via crate
/// feature autodetection) so a future transitive dep enabling aws-lc-rs can't
/// cause the "no/!1 process default provider" panic.
fn ring_provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// Verifier that accepts ONLY a leaf whose SHA-256 equals the stored pin.
#[derive(Debug)]
struct PinnedVerifier {
    expected: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if got == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            // Surfaces as a distinguishable TLS error -> "certificate changed".
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Verifier used only for the first-use probe: records the presented leaf DER
/// and accepts it (TOFU). The captured cert is shown to the user for approval.
#[derive(Debug)]
struct CaptureVerifier {
    seen: Arc<Mutex<Option<Vec<u8>>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for CaptureVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().expect("capture mutex poisoned") = Some(end_entity.as_ref().to_vec());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Build a blocking reqwest client whose TLS uses the given custom verifier.
fn build_client(
    verifier: Arc<dyn ServerCertVerifier>,
    timeout_secs: u64,
) -> Result<reqwest::blocking::Client> {
    let config = rustls::ClientConfig::builder_with_provider(ring_provider())
        .with_safe_default_protocol_versions()
        .expect("ring provider supports default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    // Pass the BARE config: reqwest wraps it in Some() internally and downcasts
    // to Option<rustls::ClientConfig>. Passing Some(config) double-wraps it and
    // fails as "unknown TLS backend".
    reqwest::blocking::ClientBuilder::new()
        .use_preconfigured_tls(config)
        // Never chase a 3xx to another host: the pin must correspond to the
        // exact configured host:port, not wherever a redirect points.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(Error::from)
}

/// A client that rejects any server whose leaf SHA-256 != `pin_hex`. Used for
/// the actual JNLP + JAR downloads once a pin is established/approved.
pub fn pinned_client(pin_hex: &str) -> Result<reqwest::blocking::Client> {
    let expected = hex_to_32(pin_hex)?;
    let verifier = Arc::new(PinnedVerifier {
        expected,
        provider: ring_provider(),
    });
    build_client(verifier, 60)
}

/// Capture the leaf cert presented by the server (first-use / cert-change
/// probe). Accepts whatever is presented and returns its fingerprint + details.
/// Only the TLS handshake matters; the HTTP response is discarded.
pub fn capture_cert(base_url: &str) -> Result<CertInfo> {
    let base = crate::webstart::normalize_url(base_url)?;
    let url = format!("{}/webstart.jnlp", base);

    let seen: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let verifier = Arc::new(CaptureVerifier {
        seen: Arc::clone(&seen),
        provider: ring_provider(),
    });
    let client = build_client(verifier, 15)?;
    let _ = client.get(&url).send()?;

    let der = seen
        .lock()
        .expect("capture mutex poisoned")
        .take()
        .ok_or_else(|| Error::msg("TLS handshake produced no server certificate"))?;
    Ok(cert_info_from_der(&der))
}

/// Compute the fingerprint and parse the human-readable fields. Parsing failures
/// degrade gracefully: the fingerprint (the part that matters) is always set.
fn cert_info_from_der(der: &[u8]) -> CertInfo {
    let sha256 = to_hex(&Sha256::digest(der));
    let (subject, issuer, not_after) = match x509_parser::parse_x509_certificate(der) {
        Ok((_, cert)) => (
            cert.subject().to_string(),
            cert.issuer().to_string(),
            cert.validity().not_after.to_string(),
        ),
        Err(_) => (String::new(), String::new(), String::new()),
    };
    CertInfo { sha256, subject, issuer, not_after }
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

fn hex_to_32(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return Err(Error::msg("pinned fingerprint must be 64 hex characters"));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| Error::msg("pinned fingerprint contains invalid hex"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_hex_is_lowercase_padded() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff, 0xab]), "000fffab");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn hex_to_32_roundtrips_and_trims() {
        let hex = "ab".repeat(32);
        assert_eq!(hex_to_32(&hex).unwrap(), [0xab; 32]);
        // surrounding whitespace is tolerated
        assert_eq!(hex_to_32(&format!("  {hex}\n")).unwrap(), [0xab; 32]);
    }

    #[test]
    fn hex_to_32_rejects_bad_input() {
        assert!(hex_to_32("").is_err());
        assert!(hex_to_32("abcd").is_err()); // wrong length
        assert!(hex_to_32(&"zz".repeat(32)).is_err()); // non-hex
        assert!(hex_to_32(&"ab".repeat(33)).is_err()); // too long
    }
}
