//! The TLS client of the control connection (HOST_AGENT.md §5): rustls with the ring provider, TLS 1.3 only, and a
//! certificate verifier that accepts exactly one certificate: the one whose SHA-256 fingerprint (of its DER bytes,
//! the form the lobby prints, LG-222) is pinned in `host.toml`. No root store, no name check, no trust on first use:
//! a CA-issued certificate is refused too unless it is the pinned one. The handshake signature is checked as usual,
//! so only the holder of the pinned certificate's key passes.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};
use std::net::{IpAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The SHA-256 fingerprint of a DER certificate (`AB:CD:..`), as the lobby's `tls::fingerprint`.
pub fn fingerprint(der: &[u8]) -> String {
    dc_rom::sha256(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Accepts the pinned certificate only.
#[derive(Debug)]
pub struct PinVerifier {
    /// Normalised `AB:CD:..`.
    pin: String,
    /// The fingerprint of the last refused certificate, for the log line.
    seen: Mutex<Option<String>>,
}

impl PinVerifier {
    pub fn new(pin: &str) -> PinVerifier {
        PinVerifier {
            pin: pin.to_string(),
            seen: Mutex::new(None),
        }
    }

    /// The fingerprint of the certificate the last refused handshake showed.
    pub fn refused(&self) -> Option<String> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        if fp == self.pin {
            return Ok(ServerCertVerified::assertion());
        }
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::ApplicationVerificationFailure,
        ))
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // TLS 1.3 only
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
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
            &provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The server name sent in the handshake (SNI): the host of `host:port` (the verifier does not check it).
fn server_name(host: &str) -> Result<ServerName<'static>, String> {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = h.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }
    ServerName::try_from(h.to_string()).map_err(|e| format!("{h}: {e}"))
}

/// A finished handshake: the TLS state and its socket (driven by [`crate::link`]).
pub struct Tls {
    pub conn: ClientConnection,
    pub sock: TcpStream,
}

/// Connects to `lobby` (`host:port`) and completes a TLS 1.3 handshake that accepts only the certificate `pin`.
pub fn connect(lobby: &str, pin: &str, timeout: Duration) -> Result<Tls, String> {
    use std::net::ToSocketAddrs;
    let host = lobby.rsplit_once(':').map(|(h, _)| h).unwrap_or(lobby);
    let addrs: Vec<_> = lobby
        .to_socket_addrs()
        .map_err(|e| format!("resolve {lobby}: {e}"))?
        .collect();
    let mut last = format!("resolve {lobby}: no address");
    let mut sock = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                sock = Some(s);
                break;
            }
            Err(e) => last = format!("connect {a}: {e}"),
        }
    }
    let mut sock = sock.ok_or(last)?;
    let verifier = Arc::new(PinVerifier::new(pin));
    let cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let mut conn =
        ClientConnection::new(Arc::new(cfg), server_name(host)?).map_err(|e| e.to_string())?;
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));
    let _ = sock.set_nodelay(true);
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut sock) {
            let _ = sock.shutdown(std::net::Shutdown::Both);
            return Err(match verifier.refused() {
                Some(fp) => format!(
                    "the lobby's certificate does not match the pinned fingerprint (pinned {pin}, shown {fp})"
                ),
                None => format!("TLS handshake with {lobby}: {e}"),
            });
        }
    }
    Ok(Tls { conn, sock })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verifier_accepts_the_pin_only() {
        let a = CertificateDer::from(vec![1u8, 2, 3]);
        let b = CertificateDer::from(vec![1u8, 2, 4]);
        let v = PinVerifier::new(&fingerprint(a.as_ref()));
        let name = ServerName::try_from("lobby.example").unwrap();
        assert!(v
            .verify_server_cert(&a, &[], &name, &[], UnixTime::now())
            .is_ok());
        assert!(v
            .verify_server_cert(&b, &[], &name, &[], UnixTime::now())
            .is_err());
        assert_eq!(v.refused(), Some(fingerprint(b.as_ref())));
    }

    #[test]
    fn the_fingerprint_is_upper_hex_pairs() {
        let f = fingerprint(b"");
        assert_eq!(f.len(), 32 * 3 - 1);
        assert!(f.starts_with("E3:B0:C4:42"));
    }
}
