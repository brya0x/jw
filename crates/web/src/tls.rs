//! The wifi listener's TLS (mobile spec, REQ-56 to REQ-60): a self-signed
//! certificate made once and kept, the fingerprint a phone pins, the check
//! that the listener sits on a private address, and the connection type that
//! lets the bridge serve a plain or a TLS socket the same way.
//!
//! The certificate carries no address: the pin is the identity, so the same
//! certificate keeps working when the PC's address changes (REQ-57, REQ-69).

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream, UdpSocket};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use sha2::{Digest, Sha256};

/// What a TLS listener serves with.
pub type TlsConfig = Arc<ServerConfig>;

/// The listener's identity: what it serves, and what a phone pins.
pub struct Identity {
    pub config: TlsConfig,
    /// SHA-256 of the DER certificate, lower-case hex.
    pub fp: String,
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// Where the certificate is kept inside `dir` (next to `mobile.json`).
pub fn cert_path(dir: &Path) -> PathBuf {
    dir.join("mobile-cert.pem")
}

fn port_path(dir: &Path) -> PathBuf {
    dir.join("mobile-port")
}

/// The identity kept in `dir`, made on the first call. The file is 0600: it
/// holds the private key. A file that cannot be read is an error, not a new
/// certificate: replacing it would silently unpair every phone.
pub fn load_or_create(dir: &Path) -> io::Result<Identity> {
    let path = cert_path(dir);
    let pem = match std::fs::read(&path) {
        Ok(pem) => pem,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // The config dir may not exist yet: nothing else has written there.
            std::fs::create_dir_all(dir)?;
            let pem = make()?;
            write_private(&path, pem.as_bytes())?;
            pem.into_bytes()
        }
        Err(e) => return Err(e),
    };
    from_pem(&pem)
}

/// A new self-signed P-256 certificate, valid for ten years, and its key, as
/// one PEM text.
fn make() -> io::Result<String> {
    let key = rcgen::KeyPair::generate().map_err(other)?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(other)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "jw mobile");
    let now = rcgen::date_time_ymd(2026, 1, 1);
    params.not_before = now;
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    let cert = params.self_signed(&key).map_err(other)?;
    Ok(format!("{}{}", cert.pem(), key.serialize_pem()))
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    // Written whole and moved into place, so a crash leaves no half file.
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

fn from_pem(pem: &[u8]) -> io::Result<Identity> {
    let cert = CertificateDer::from_pem_slice(pem).map_err(other)?;
    let key = PrivateKeyDer::from_pem_slice(pem).map_err(other)?;
    let fp = fingerprint(cert.as_ref());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    // TLS 1.2 stays: a phone with minSdk 26 may not have 1.3 everywhere.
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(other)?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(other)?;
    Ok(Identity {
        config: Arc::new(config),
        fp,
    })
}

/// SHA-256 of a DER certificate, as lower-case hex: what the QR carries and a
/// phone compares.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The port the listener had last time, so a stored address stays good across
/// a stop and a start.
pub fn port_hint(dir: &Path) -> Option<u16> {
    std::fs::read_to_string(port_path(dir))
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|p| *p != 0)
}

pub fn remember_port(dir: &Path, port: u16) {
    let _ = write_private(&port_path(dir), port.to_string().as_bytes());
}

/// Whether the listener may sit on `addr`: a private IPv4 address (10/8,
/// 172.16/12, 192.168/16), or loopback, which is never more exposed than the
/// bridge's own listener. Never `0.0.0.0`, never a public address.
pub fn check_addr(addr: &SocketAddr) -> io::Result<()> {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_private() || ip.is_loopback() => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the wifi listener sits on a private address only",
        )),
    }
}

/// The PC's private IPv4 address on the interface the default route uses, or
/// `None` (no network, or a public address). Nothing is sent: connecting a UDP
/// socket only makes the system pick the route. Behind a VPN that routes
/// everything, that interface is the VPN's.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("192.0.2.1", 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if ip.is_private() => Some(ip),
        _ => None,
    }
}

/// A phone's connection: a plain socket (the tunnel's loopback listener) or
/// one inside TLS (the wifi listener). It also holds the limits the bridge
/// puts on a connection that has not authenticated yet.
pub struct Conn {
    inner: Inner,
    /// Every read waits at most this (`None`: as long as it takes).
    read_to: Option<Duration>,
    /// The moment before which the connection must be authenticated, however
    /// slowly its bytes come: the timeout of each read starts over, so on its
    /// own it would let a trickle of bytes hold a seat for ever.
    deadline: Option<Instant>,
}

enum Inner {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
}

impl Conn {
    pub fn plain(stream: TcpStream) -> Self {
        Self {
            inner: Inner::Plain(stream),
            read_to: None,
            deadline: None,
        }
    }

    pub fn tls(stream: TcpStream, config: Arc<ServerConfig>) -> io::Result<Self> {
        let conn = ServerConnection::new(config).map_err(other)?;
        Ok(Self {
            inner: Inner::Tls(Box::new(StreamOwned::new(conn, stream))),
            read_to: None,
            deadline: None,
        })
    }

    fn sock(&self) -> &TcpStream {
        match &self.inner {
            Inner::Plain(s) => s,
            Inner::Tls(t) => &t.sock,
        }
    }

    pub fn peer_ip(&self) -> io::Result<IpAddr> {
        Ok(self.sock().peer_addr()?.ip())
    }

    pub fn set_read_timeout(&mut self, to: Option<Duration>) -> io::Result<()> {
        self.read_to = to;
        self.sock().set_read_timeout(to)
    }

    pub fn set_write_timeout(&self, to: Option<Duration>) -> io::Result<()> {
        self.sock().set_write_timeout(to)
    }

    /// Reads fail with `TimedOut` from `at` on, until [`Conn::no_deadline`].
    pub fn deadline(&mut self, at: Instant) {
        self.deadline = Some(at);
    }

    pub fn no_deadline(&mut self) {
        self.deadline = None;
    }

    pub fn deadline_at(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn shutdown(&self) -> io::Result<()> {
        self.sock().shutdown(Shutdown::Both)
    }

    /// The timeout the next read gets: the one set, cut to what is left of
    /// the deadline.
    fn arm(&self) -> io::Result<()> {
        let Some(at) = self.deadline else {
            return Ok(());
        };
        let left = at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let to = self.read_to.map_or(left, |t| t.min(left));
        self.sock().set_read_timeout(Some(to))
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.arm()?;
        match &mut self.inner {
            Inner::Plain(s) => s.read(buf),
            Inner::Tls(t) => t.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.inner {
            Inner::Plain(s) => s.write(buf),
            Inner::Tls(t) => t.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.inner {
            Inner::Plain(s) => s.flush(),
            Inner::Tls(t) => t.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_made_once_kept_private_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create(dir.path()).unwrap();
        let path = cert_path(dir.path());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the file holds the private key");
        let b = load_or_create(dir.path()).unwrap();
        assert_eq!(a.fp, b.fp, "a new certificate would unpair every phone");
        assert_eq!(a.fp.len(), 64);
        assert!(
            a.fp.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn a_file_that_is_not_a_certificate_is_an_error_not_a_new_certificate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(cert_path(dir.path()), "garbage").unwrap();
        assert!(load_or_create(dir.path()).is_err());
        assert_eq!(std::fs::read(cert_path(dir.path())).unwrap(), b"garbage");
    }

    #[test]
    fn the_listener_sits_on_a_private_or_loopback_address_only() {
        let ok = |s: &str| check_addr(&s.parse().unwrap()).is_ok();
        assert!(ok("192.168.1.23:1"));
        assert!(ok("10.1.2.3:1"));
        assert!(ok("172.16.0.9:1"));
        assert!(ok("172.31.255.1:1"));
        assert!(ok("127.0.0.1:1"));
        assert!(!ok("0.0.0.0:1"), "never every interface");
        assert!(!ok("8.8.8.8:1"));
        assert!(!ok("172.32.0.1:1"), "172.32 is public");
        assert!(!ok("169.254.1.1:1"), "link-local is not a home network");
        assert!(!ok("[::]:1"));
        assert!(!ok("[::1]:1"));
    }

    #[test]
    fn the_port_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(port_hint(dir.path()), None);
        remember_port(dir.path(), 48211);
        assert_eq!(port_hint(dir.path()), Some(48211));
        std::fs::write(port_path(dir.path()), "0").unwrap();
        assert_eq!(port_hint(dir.path()), None);
    }
}
