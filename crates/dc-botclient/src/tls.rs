//! The TLS client of the auth and web connections: our own deviation from 2009 (the 2009 auth protocol carries the
//! password in clear text). The lobby's certificate is trusted in this order (DC-DEFAULT: CLI-1470..1475):
//!
//! 1. a pinned fingerprint (`--server-fingerprint` / `DC_SERVER_FINGERPRINT`): the certificate must have it;
//! 2. a loopback server: any certificate (the traffic never leaves the machine; test lobbies make a new pair per run);
//! 3. a certificate chain the operating system's roots validate for the host name (a CA-issued certificate on a VPS);
//! 4. trust on first use: the fingerprint seen at the first connection to `host:port` is kept in the profile
//!    (`known_servers.txt`) and a different one later is refused with a box until the player pins the new one.
//!
//! A server that does not speak TLS is used in plain text only on loopback (the reference client, old test servers); a
//! non-loopback server without TLS is refused: no password crosses the network in clear text.
// DC-DEFAULT: CLI-1470..1475 (our own deviation from 2009)

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The file of the trust-on-first-use fingerprints in the client's profile folder.
pub const KNOWN_SERVERS_FILE: &str = "known_servers.txt";

/// The first words of the texts a trust failure shows. The first line of such a text is the box the login screen
/// shows (CLI-1471); the lines after it (server, fingerprints) go to the console.
pub const T_CHANGED: &str = "The server's identity has changed";
pub const T_MISMATCH: &str = "The server's identity does not match";
pub const T_NO_TLS: &str = "This server does not offer an encrypted login";
pub const T_DAMAGED: &str = "The server's identity cannot be checked";

/// True when `text` is one of this module's refusals.
pub fn is_trust_message(text: &str) -> bool {
    [T_CHANGED, T_MISMATCH, T_NO_TLS, T_DAMAGED]
        .iter()
        .any(|p| text.starts_with(p))
}

/// `DC_CLIENT_TLS`: `auto` (TLS first, plain fallback on loopback only), `off` (plain on loopback; refused elsewhere).
// DC-DEFAULT: CLI-1474
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Off,
}

/// What the client trusts.
#[derive(Clone, Debug)]
pub struct Trust {
    /// The pinned fingerprint, normalised (`AB:CD:..`).
    pub pin: Option<String>,
    /// The trust-on-first-use file; None = remembered for this process only.
    pub store: Option<PathBuf>,
    /// Host names typed for an address (`--lobby name:port`), for the certificate's name check.
    pub names: BTreeMap<IpAddr, String>,
    pub mode: Mode,
    /// How long the TLS attempt on a loopback server may take before the plain fallback.
    pub loopback_handshake: Duration,
}

impl Default for Trust {
    fn default() -> Trust {
        Trust {
            pin: None,
            store: None,
            names: BTreeMap::new(),
            mode: Mode::Auto,
            loopback_handshake: Duration::from_secs(2),
        }
    }
}

impl Trust {
    /// The defaults with `DC_SERVER_FINGERPRINT` and `DC_CLIENT_TLS` applied (a bad value is ignored with a line on
    /// stderr).
    pub fn from_env() -> Trust {
        let mut t = Trust::default();
        if let Ok(v) = std::env::var("DC_SERVER_FINGERPRINT") {
            if !v.trim().is_empty() {
                match normalize_fingerprint(&v) {
                    Ok(p) => t.pin = Some(p),
                    Err(e) => eprintln!("DC_SERVER_FINGERPRINT: {e}"),
                }
            }
        }
        if let Ok(v) = std::env::var("DC_CLIENT_TLS") {
            if v.trim().eq_ignore_ascii_case("off") {
                t.mode = Mode::Off;
            }
        }
        t
    }
}

fn global() -> &'static Mutex<Trust> {
    static G: OnceLock<Mutex<Trust>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Trust::from_env()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The process-wide trust (what [`connect`] uses without an explicit one).
pub fn current() -> Trust {
    lock(global()).clone()
}

/// `--server-fingerprint`: pins the server (beats `DC_SERVER_FINGERPRINT`).
pub fn set_pin(fp: &str) -> Result<(), String> {
    let p = normalize_fingerprint(fp)?;
    lock(global()).pin = Some(p);
    Ok(())
}

/// CLI-1616: drops the pin (the in-client browser picked a server whose entry names none).
pub fn clear_pin() {
    lock(global()).pin = None;
}

/// The trust-on-first-use file (the client's profile folder).
pub fn set_store(path: PathBuf) {
    lock(global()).store = Some(path);
}

/// Remembers the host name typed for `ip` (the certificate name check of a CA-issued certificate).
pub fn note_host(ip: IpAddr, name: &str) {
    if name.parse::<IpAddr>().is_err() && !name.is_empty() {
        lock(global()).names.insert(ip, name.to_string());
    }
}

/// A fingerprint in any common spelling (`AB:CD:..`, `abcd..`, `SHA256:..`, spaces) -> `AB:CD:..` (32 pairs).
pub fn normalize_fingerprint(s: &str) -> Result<String, String> {
    let t = s.trim();
    let t = t
        .strip_prefix("sha256:")
        .or_else(|| t.strip_prefix("SHA256:"))
        .or_else(|| t.strip_prefix("sha256 Fingerprint="))
        .or_else(|| t.strip_prefix("SHA256 Fingerprint="))
        .unwrap_or(t);
    let hex: String = t
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect::<String>()
        .to_ascii_uppercase();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "{s:?} is not a SHA-256 fingerprint (64 hex digits, colons allowed)"
        ));
    }
    Ok(hex
        .as_bytes()
        .chunks(2)
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect::<Vec<_>>()
        .join(":"))
}

/// The SHA-256 fingerprint of a DER certificate (`AB:CD:..`).
pub fn fingerprint(der: &[u8]) -> String {
    dc_rom::sha256(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn unmapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

// ------------------------------------------------------------------------------------------------ known servers

fn memory_store() -> &'static Mutex<BTreeMap<String, String>> {
    static M: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The `host:port FINGERPRINT` lines of the file. A missing file is empty; a file that cannot be read, or a
/// non-comment line that does not parse (a truncated or hand-edited file), is damage: the store fails closed
/// (CLI-1470) instead of treating the host as a first use.
pub fn read_known(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8(b).map_err(|_| damaged(path))?,
        // a parent folder that is a file reads as NotADirectory on Unix and NotFound on Windows: no store either way
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {
            return Ok(BTreeMap::new())
        }
        Err(_) => return Err(damaged(path)),
    };
    let mut m = BTreeMap::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let parsed = l
            .split_once(char::is_whitespace)
            .and_then(|(k, v)| Some((k.to_string(), normalize_fingerprint(v).ok()?)));
        match parsed {
            Some((k, v)) => {
                m.insert(k, v);
            }
            None => return Err(damaged(path)),
        }
    }
    Ok(m)
}

fn damaged(path: &Path) -> String {
    format!(
        "{T_DAMAGED}: the saved server list is damaged: delete {} to trust the server again.",
        path.display()
    )
}

/// The temporary file a write of `path` goes through (same folder, so the rename stays on one volume).
pub fn temp_path(path: &Path) -> PathBuf {
    let mut n = path.file_name().unwrap_or_default().to_os_string();
    n.push(".tmp");
    path.with_file_name(n)
}

fn write_known(path: &Path, m: &BTreeMap<String, String>) -> io::Result<()> {
    let mut s = String::from(
        "# DeCombat: the lobby certificates this player trusts (host:port SHA-256); delete a line to trust again
",
    );
    for (k, v) in m {
        s.push_str(&format!("{k} {v}
"));
    }
    if let Some(d) = path.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d)?;
        }
    }
    let tmp = temp_path(path);
    let r = std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// The warning of the last failed save of the trust file; shown once in the login box.
fn warning_slot() -> &'static Mutex<Option<String>> {
    static W: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(None))
}

/// Takes the pending warning about the trust file (None when the last save worked or it was already shown).
pub fn take_store_warning() -> Option<String> {
    lock(warning_slot()).take()
}

fn warn_store(text: String) {
    let mut w = lock(warning_slot());
    if w.is_none() {
        eprintln!("{text}");
        *w = Some(text);
    }
}

fn known(t: &Trust, key: &str) -> Result<Option<String>, String> {
    match &t.store {
        Some(p) => Ok(read_known(p)?.get(key).cloned()),
        None => Ok(lock(memory_store()).get(key).cloned()),
    }
}

fn remember(t: &Trust, key: &str, fp: &str) {
    match &t.store {
        Some(p) => {
            let mut m = match read_known(p) {
                Ok(m) => m,
                Err(e) => return warn_store(e),
            };
            if m.get(key).map(String::as_str) != Some(fp) {
                m.insert(key.to_string(), fp.to_string());
                if let Err(e) = write_known(p, &m) {
                    warn_store(format!(
                        "Warning: DeCombat could not save the server's identity to {}: {e}. The server will be trusted again as new next time.",
                        p.display()
                    ));
                }
            }
        }
        None => {
            lock(memory_store()).insert(key.to_string(), fp.to_string());
        }
    }
}

// ------------------------------------------------------------------------------------------------ verifier

fn native_roots() -> Option<Arc<WebPkiServerVerifier>> {
    static V: OnceLock<Option<Arc<WebPkiServerVerifier>>> = OnceLock::new();
    V.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        let (_added, _bad) = roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        if roots.is_empty() {
            return None;
        }
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .ok()
    })
    .clone()
}

#[derive(Debug)]
struct Verifier {
    trust: Trust,
    key: String,
    loopback: bool,
    failure: Arc<Mutex<Option<String>>>,
    /// The fingerprint to remember once the handshake (the server's proof of its key) has passed.
    keep: Arc<Mutex<Option<String>>>,
}

impl Verifier {
    fn refuse(&self, text: String) -> Result<ServerCertVerified, rustls::Error> {
        *lock(&self.failure) = Some(text);
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::ApplicationVerificationFailure,
        ))
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        // CLI-1471: a pin decides alone; a matching pin also replaces a remembered fingerprint
        if let Some(pin) = &self.trust.pin {
            if *pin == fp {
                if !self.loopback {
                    *lock(&self.keep) = Some(fp);
                }
                return Ok(ServerCertVerified::assertion());
            }
            return self.refuse(format!(
                "{T_MISMATCH} the fingerprint you were given. Ask the host for the right one.\nserver {}\npinned     {pin}\nshown now  {fp}",
                self.key
            ));
        }
        // CLI-1472: loopback traffic never leaves the machine
        if self.loopback {
            return Ok(ServerCertVerified::assertion());
        }
        // CLI-1473: a CA-issued certificate for the host name
        if let Some(v) = native_roots() {
            if v
                .verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
                .is_ok()
            {
                return Ok(ServerCertVerified::assertion());
            }
        }
        // CLI-1470: trust on first use
        match known(&self.trust, &self.key) {
            Err(text) => self.refuse(text),
            Ok(None) => {
                *lock(&self.keep) = Some(fp);
                Ok(ServerCertVerified::assertion())
            }
            Ok(Some(old)) if old == fp => Ok(ServerCertVerified::assertion()),
            Ok(Some(old)) => self.refuse(format!(
                "{T_CHANGED}. Ask the host for the new fingerprint and start the game with --server-fingerprint.\nserver {} (this can mean someone is intercepting the connection)\nremembered {old}\nshown now  {fp}",
                self.key
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // TLS 1.3 only (CLI-1475)
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

// ------------------------------------------------------------------------------------------------ stream

/// A client connection: plain (a loopback server without TLS) or TLS.
pub enum ClientStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl ClientStream {
    pub fn tcp(&self) -> &TcpStream {
        match self {
            ClientStream::Plain(s) => s,
            ClientStream::Tls(t) => &t.sock,
        }
    }

    pub fn is_tls(&self) -> bool {
        matches!(self, ClientStream::Tls(_))
    }

    /// Ends the connection (a TLS close notice first).
    pub fn close(&mut self) {
        if let ClientStream::Tls(t) = self {
            t.conn.send_close_notify();
            let _ = t.flush();
        }
        let _ = self.tcp().shutdown(Shutdown::Both);
    }
}

impl Read for ClientStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ClientStream::Plain(s) => s.read(buf),
            ClientStream::Tls(t) => t.read(buf),
        }
    }
}

impl Write for ClientStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            ClientStream::Plain(s) => s.write(buf),
            ClientStream::Tls(t) => t.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            ClientStream::Plain(s) => s.flush(),
            ClientStream::Tls(t) => t.flush(),
        }
    }
}

fn host_key(t: &Trust, addr: SocketAddr) -> String {
    match t.names.get(&addr.ip()) {
        Some(n) => format!("{n}:{}", addr.port()),
        None => addr.to_string(),
    }
}

fn no_tls(addr: SocketAddr, why: &str) -> String {
    format!(
        "{T_NO_TLS}, so DeCombat will not send your password. Ask the host to update the server.\nserver {addr}: {why}"
    )
}

/// Connects to `addr` with TLS, trusting the server by `t`. A loopback server that does not speak TLS (or
/// `DC_CLIENT_TLS=off`) gets a plain connection; any other server without TLS is refused.
pub fn connect_with(addr: SocketAddr, timeout: Duration, t: &Trust) -> Result<ClientStream, String> {
    let loopback = unmapped(addr.ip()).is_loopback();
    let plain = || -> Result<ClientStream, String> {
        let s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
        Ok(ClientStream::Plain(s))
    };
    if t.mode == Mode::Off {
        return if loopback {
            plain()
        } else {
            Err(no_tls(addr, "DC_CLIENT_TLS=off"))
        };
    }
    let s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
    let failure = Arc::new(Mutex::new(None));
    let keep = Arc::new(Mutex::new(None));
    let key = host_key(t, addr);
    let verifier = Verifier {
        trust: t.clone(),
        key: key.clone(),
        loopback,
        failure: Arc::clone(&failure),
        keep: Arc::clone(&keep),
    };
    let cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = match t.names.get(&addr.ip()) {
        Some(n) => ServerName::try_from(n.clone()).map_err(|e| format!("{n}: {e}"))?,
        None => ServerName::IpAddress(addr.ip().into()),
    };
    let mut c = ClientConnection::new(Arc::new(cfg), name).map_err(|e| e.to_string())?;
    let hs = if loopback {
        t.loopback_handshake.min(timeout)
    } else {
        timeout
    };
    let _ = s.set_read_timeout(Some(hs.max(Duration::from_millis(50))));
    let _ = s.set_write_timeout(Some(timeout.max(Duration::from_millis(50))));
    let mut sock = s;
    let mut err = None;
    while c.is_handshaking() {
        if let Err(e) = c.complete_io(&mut sock) {
            err = Some(e);
            break;
        }
    }
    if let Some(text) = lock(&failure).take() {
        let _ = sock.shutdown(Shutdown::Both);
        return Err(text);
    }
    match err {
        None => {
            if let Some(fp) = lock(&keep).take() {
                remember(t, &key, &fp);
            }
            let _ = sock.set_read_timeout(None);
            let _ = sock.set_write_timeout(None);
            Ok(ClientStream::Tls(Box::new(StreamOwned::new(c, sock))))
        }
        // CLI-1474: a server without TLS: plain on loopback only
        Some(e) => {
            let _ = sock.shutdown(Shutdown::Both);
            if loopback {
                plain()
            } else {
                Err(no_tls(addr, &e.to_string()))
            }
        }
    }
}

/// [`connect_with`] with the process-wide trust.
pub fn connect(addr: SocketAddr, timeout: Duration) -> Result<ClientStream, String> {
    connect_with(addr, timeout, &current())
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
