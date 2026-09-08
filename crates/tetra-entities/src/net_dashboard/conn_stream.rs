//! Dashboard connection stream: plain TCP or rustls, plus unread-prefix for peek replacement.
//!
//! HTTPS follows the bluestation-telemetry pattern (`rustls::StreamOwned` after handshake).

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConnection;

/// Fixed HTTPS sidecar port (HTTP stays on the configured dashboard port).
pub const DASHBOARD_HTTPS_PORT: u16 = 8443;

const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

/// Live TLS listener status for `GET /api/dashboard/tls`.
#[derive(Debug, Clone)]
pub struct DashboardTlsStatus {
    pub https_port: u16,
    pub enabled: bool,
    pub cert_fingerprint: String,
}

static TLS_STATUS: OnceLock<DashboardTlsStatus> = OnceLock::new();

pub fn tls_status() -> DashboardTlsStatus {
    TLS_STATUS.get().cloned().unwrap_or(DashboardTlsStatus {
        https_port: DASHBOARD_HTTPS_PORT,
        enabled: false,
        cert_fingerprint: String::new(),
    })
}

fn set_tls_status(status: DashboardTlsStatus) {
    let _ = TLS_STATUS.set(status);
}

/// Plain TCP or rustls server stream (bluestation-telemetry style).
pub enum ConnStream {
    Plain(TcpStream),
    Tls(rustls::StreamOwned<ServerConnection, TcpStream>),
}

impl ConnStream {
    pub fn plain(tcp: TcpStream) -> Self {
        ConnStream::Plain(tcp)
    }

    pub fn from_tls_handshake(
        tcp: TcpStream,
        config: Arc<rustls::ServerConfig>,
    ) -> Result<Self, rustls::Error> {
        let conn = ServerConnection::new(config)?;
        Ok(ConnStream::Tls(rustls::StreamOwned::new(conn, tcp)))
    }

    pub fn is_tls(&self) -> bool {
        matches!(self, ConnStream::Tls(_))
    }

    pub fn tcp_ref(&self) -> &TcpStream {
        match self {
            ConnStream::Plain(s) => s,
            ConnStream::Tls(s) => s.get_ref(),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_ref().set_read_timeout(timeout)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.tcp_ref().peer_addr()
    }
}

impl Read for ConnStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ConnStream::Plain(s) => s.read(buf),
            ConnStream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for ConnStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            ConnStream::Plain(s) => s.write(buf),
            ConnStream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            ConnStream::Plain(s) => s.flush(),
            ConnStream::Tls(s) => s.flush(),
        }
    }
}

/// `ConnStream` plus bytes already read from the socket (replaces `TcpStream::peek`).
///
/// Handlers use this type everywhere after the initial routing read in `handle_connection`.
pub struct PrefixedConn {
    prefix: Vec<u8>,
    pos: usize,
    inner: ConnStream,
}

impl PrefixedConn {
    pub fn new(inner: ConnStream) -> Self {
        Self {
            prefix: Vec::new(),
            pos: 0,
            inner,
        }
    }

    pub fn with_prefix(prefix: Vec<u8>, inner: ConnStream) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }

    pub fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    pub fn tcp_ref(&self) -> &TcpStream {
        self.inner.tcp_ref()
    }
}

impl Read for PrefixedConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.prefix.len() {
            let available = &self.prefix[self.pos..];
            let n = available.len().min(buf.len());
            buf[..n].copy_from_slice(&available[..n]);
            self.pos += n;
            if self.pos >= self.prefix.len() {
                self.prefix.clear();
                self.pos = 0;
            }
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

impl Write for PrefixedConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Prefer `/etc/flowstation/dashboard-tls/` when that tree exists; else `<config_dir>/dashboard-tls/`.
pub fn tls_dir_for_config(config_path: &str) -> PathBuf {
    let system = PathBuf::from("/etc/flowstation/dashboard-tls");
    if system.is_dir() {
        return system;
    }
    let system_parent = Path::new("/etc/flowstation");
    if system_parent.is_dir() && fs::create_dir_all(&system).is_ok() {
        return system;
    }
    Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("dashboard-tls")
}

/// Ensure cert+key exist (openssl CLI). Returns `(ServerConfig, sha256 fingerprint hex)` when usable.
pub fn ensure_dashboard_tls(tls_dir: &Path) -> Option<(Arc<rustls::ServerConfig>, String)> {
    if let Err(e) = fs::create_dir_all(tls_dir) {
        tracing::warn!("Dashboard TLS: cannot create {}: {}", tls_dir.display(), e);
        return None;
    }

    let cert_path = tls_dir.join(CERT_FILE);
    let key_path = tls_dir.join(KEY_FILE);

    if !cert_path.is_file() || !key_path.is_file() {
        if let Err(e) = generate_self_signed_openssl(&cert_path, &key_path) {
            tracing::warn!(
                "Dashboard TLS: openssl failed to generate certs in {}: {} — HTTPS disabled",
                tls_dir.display(),
                e
            );
            return None;
        }
        tracing::info!(
            "Dashboard TLS: generated self-signed cert at {}",
            cert_path.display()
        );
    }

    match load_tls_config(&cert_path, &key_path) {
        Ok((cfg, fp)) => {
            set_tls_status(DashboardTlsStatus {
                https_port: DASHBOARD_HTTPS_PORT,
                enabled: true,
                cert_fingerprint: fp.clone(),
            });
            Some((cfg, fp))
        }
        Err(e) => {
            tracing::warn!("Dashboard TLS: failed to load certs: {} — HTTPS disabled", e);
            set_tls_status(DashboardTlsStatus {
                https_port: DASHBOARD_HTTPS_PORT,
                enabled: false,
                cert_fingerprint: String::new(),
            });
            None
        }
    }
}

fn generate_self_signed_openssl(cert_path: &Path, key_path: &Path) -> Result<(), String> {
    // Prefer SAN-capable openssl (1.1.1+). Fall back to a CN-only cert on older builds.
    let with_san = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-sha256",
            "-nodes",
            "-days",
            "825",
            "-subj",
            "/CN=Bost FlowStation Dashboard",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
            "-keyout",
        ])
        .arg(key_path)
        .arg("-out")
        .arg(cert_path)
        .status();

    let ok = match with_san {
        Ok(st) if st.success() => true,
        _ => {
            let _ = fs::remove_file(cert_path);
            let _ = fs::remove_file(key_path);
            let status = std::process::Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-sha256",
                    "-nodes",
                    "-days",
                    "825",
                    "-subj",
                    "/CN=Bost FlowStation Dashboard",
                    "-keyout",
                ])
                .arg(key_path)
                .arg("-out")
                .arg(cert_path)
                .status()
                .map_err(|e| format!("failed to spawn openssl: {e}"))?;
            status.success()
        }
    };

    if !ok {
        return Err("openssl failed to generate self-signed cert".into());
    }
    if !cert_path.is_file() || !key_path.is_file() {
        return Err("openssl reported success but cert/key missing".into());
    }
    Ok(())
}

fn load_tls_config(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Arc<rustls::ServerConfig>, String), String> {
    // rustls 0.23 requires an installed crypto provider; ring is already a crate feature.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert_file = File::open(cert_path).map_err(|e| format!("open cert: {e}"))?;
    let key_file = File::open(key_path).map_err(|e| format!("open key: {e}"))?;

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("parse cert PEM: {e}"))?;

    if certs.is_empty() {
        return Err("no certificates in PEM".into());
    }

    let fingerprint = openssl_cert_fingerprint(cert_path)
        .unwrap_or_else(|| format!("{:x}", md5::compute(certs[0].as_ref())));

    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .map_err(|e| format!("parse key PEM: {e}"))?
        .ok_or_else(|| "no private key in PEM".to_string())?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("invalid TLS config: {e}"))?;
    // Dashboard speaks raw HTTP/1.1 (and WS upgrade), not ALPN-negotiated h2.
    config.alpn_protocols.clear();

    Ok((Arc::new(config), fingerprint))
}

/// Prefer `openssl x509 -fingerprint -sha256` (colon-separated → bare lowercase hex).
fn openssl_cert_fingerprint(cert_path: &Path) -> Option<String> {
    let out = std::process::Command::new("openssl")
        .args(["x509", "-noout", "-fingerprint", "-sha256", "-in"])
        .arg(cert_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // e.g. "sha256 Fingerprint=AB:CD:..."
    let hex = text.split('=').nth(1)?.trim();
    let cleaned: String = hex
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if cleaned.len() == 64 {
        Some(cleaned)
    } else {
        None
    }
}
