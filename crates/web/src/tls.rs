//! TLS for the private-address listener (REQ-81): one self-signed certificate, kept and
//! reused, that a phone trusts only by the fingerprint the QR carried.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpStream, UdpSocket};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, ServerConnection, StreamOwned};

use crate::auth::{same, sha256_bytes_hex};
use crate::bridge::Link;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// The certificate and its key.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    /// The certificate kept at `path` (cert and key in one PEM file, mode 0600), made first
    /// if there is none. It stays the same across starts so a paired phone keeps trusting it.
    pub fn load_or_create(path: &Path) -> io::Result<Identity> {
        match fs::read(path) {
            Ok(pem) => {
                return Ok(Identity {
                    cert: CertificateDer::from_pem_slice(&pem).map_err(invalid)?,
                    key: PrivateKeyDer::from_pem_slice(&pem).map_err(invalid)?,
                });
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            Err(_) => {}
        }
        let made =
            rcgen::generate_simple_self_signed(vec![format!("{}.local", identity::get().name)])
                .map_err(invalid)?;
        let pem = format!("{}{}", made.cert.pem(), made.signing_key.serialize_pem());
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(pem.as_bytes())?;
        Ok(Identity {
            cert: CertificateDer::from_pem_slice(pem.as_bytes()).map_err(invalid)?,
            key: PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(invalid)?,
        })
    }

    /// SHA-256 of the certificate, in hex: what the QR carries as `fp`.
    pub fn fingerprint(&self) -> String {
        sha256_bytes_hex(&self.cert)
    }

    pub fn server_config(&self) -> io::Result<Arc<ServerConfig>> {
        let config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(invalid)?
            .with_no_client_auth()
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(invalid)?;
        Ok(Arc::new(config))
    }
}

/// A TLS connection from a phone.
pub struct TlsLink(pub StreamOwned<ServerConnection, TcpStream>);

impl TlsLink {
    pub fn new(config: &Arc<ServerConfig>, sock: TcpStream) -> io::Result<TlsLink> {
        let conn = ServerConnection::new(config.clone()).map_err(invalid)?;
        Ok(TlsLink(StreamOwned::new(conn, sock)))
    }
}

impl Read for TlsLink {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for TlsLink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Link for TlsLink {
    fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        self.0.sock.set_read_timeout(d)
    }
}

/// Whether `ip` is one the listener may bind: loopback or a private range (10/8,
/// 172.16/12, 192.168/16), never `0.0.0.0`.
pub fn bindable(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private()
}

/// The private address the PC reaches the network with, if it has one. Nothing is sent: a
/// UDP socket is only asked which interface it would use.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("192.0.2.1:9").ok()?;
    match probe.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if ip.is_private() => Some(ip),
        _ => None,
    }
}

/// A client's verdict on the bridge's certificate: its SHA-256 must be `fingerprint`, and
/// nothing else about it (name, chain, dates) matters. This is what a phone does; it is
/// here so tests and tools can speak to the listener.
#[derive(Debug)]
pub struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl Pinned {
    pub fn client_config(fingerprint: &str) -> io::Result<Arc<ClientConfig>> {
        let provider = provider();
        let verifier = Arc::new(Pinned {
            fingerprint: fingerprint.to_owned(),
            provider: provider.clone(),
        });
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(invalid)?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        Ok(Arc::new(config))
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if same(&sha256_bytes_hex(end_entity), &self.fingerprint) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "certificate does not match the pinned fingerprint".into(),
            ))
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
    fn the_certificate_is_made_once_kept_private_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link-cert.pem");
        let first = Identity::load_or_create(&path).unwrap();
        let again = Identity::load_or_create(&path).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.fingerprint().len(), 64);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(first.server_config().is_ok());
    }

    #[test]
    fn a_broken_certificate_file_is_an_error_not_a_new_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link-cert.pem");
        fs::write(&path, "not a pem").unwrap();
        assert!(
            Identity::load_or_create(&path).is_err(),
            "a pinned phone would stop trusting a silently replaced one"
        );
    }

    #[test]
    fn only_loopback_and_private_addresses_can_be_bound() {
        assert!(bindable("127.0.0.1".parse().unwrap()));
        assert!(bindable("10.1.2.3".parse().unwrap()));
        assert!(bindable("172.16.0.9".parse().unwrap()));
        assert!(bindable("192.168.1.20".parse().unwrap()));
        assert!(!bindable("0.0.0.0".parse().unwrap()));
        assert!(!bindable("8.8.8.8".parse().unwrap()));
        assert!(!bindable("172.32.0.1".parse().unwrap()));
    }
}
