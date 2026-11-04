//! TLS on the auth port and the web port: our own deviation from 2009. The 2009 auth protocol carries the login
//! password in clear text and the registration form posts over plain HTTP; DeCombat wraps both in TLS 1.3.
//!
//! - The lobby keeps one certificate + key in its data folder (`--tls-dir`, default the DB's folder). The first start
//!   makes a self-signed pair; an operator may drop in a CA-issued pair under the same file names, or the Let's Encrypt
//!   names `fullchain.pem` + `privkey.pem` (DC-DEFAULT: LG-221).
//! - Every start logs the SHA-256 fingerprint of the certificate; friends pin it (`--server-fingerprint`) (LG-222).
//! - One port serves both: the first byte of a connection tells a TLS handshake (0x16 0x03) from a 2009 frame, whose
//!   first auth frame is 0x206 or 0x200 (LG-223).
//! - No password crosses a non-loopback socket in clear text: a plain connection from a non-loopback peer is refused
//!   on a public lobby unless `--plain-auth-public`; `--tls loopback` refuses plain connections from every peer
//!   (LG-224, LG-225).
//! - The lobby port and the clan port carry no password and stay plain (LG-226).
// DC-DEFAULT: LG-220..226 (our own deviation from 2009)

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The file names of the lobby's own pair (a self-signed one is made when neither file exists).
pub const CERT_FILE: &str = "tls_cert.pem";
pub const KEY_FILE: &str = "tls_key.pem";
/// The Let's Encrypt file names, used first when both exist in the TLS folder.
pub const LE_CERT_FILE: &str = "fullchain.pem";
pub const LE_KEY_FILE: &str = "privkey.pem";

/// How long a new connection may take to send its first bytes and finish the handshake.
// DC-DEFAULT: LG-223 10 s
const HANDSHAKE_SECS: u64 = 10;

/// How many connections of one listener may be between accept and the end of the handshake at once; more are
/// closed at once so a flood of silent sockets cannot take every thread.
// DC-DEFAULT: LG-223 64
pub const MAX_HANDSHAKES: usize = 64;

/// The count of handshakes in progress on one listener.
#[derive(Clone, Default)]
pub struct Slots {
    open: Arc<std::sync::atomic::AtomicUsize>,
    refused: Arc<std::sync::atomic::AtomicU64>,
}

/// One handshake in progress; it frees its place when dropped.
pub struct Slot(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Slots {
    /// A place for one more handshake, or None when `max` are in progress.
    pub fn take(&self, max: usize) -> Option<Slot> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.open.fetch_add(1, SeqCst) >= max {
            self.open.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Slot(Arc::clone(&self.open)))
    }

    /// True for the 1st, 65th, ... refusal: the log line is not repeated for every closed socket.
    pub fn first_of_burst(&self) -> bool {
        self.refused
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .is_multiple_of(64)
    }
}

/// `--tls`: `auto` (TLS required from non-loopback peers of a public lobby) or `loopback` (required from every peer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    Auto,
    Loopback,
}

impl TlsMode {
    pub fn parse(v: &str) -> Result<TlsMode, String> {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(TlsMode::Auto),
            "loopback" | "always" | "required" => Ok(TlsMode::Loopback),
            o => Err(format!("--tls {o:?}: give auto or loopback")),
        }
    }
}

/// The SHA-256 fingerprint of a DER certificate, upper-case hex pairs joined by ':' (the form browsers show).
pub fn fingerprint(der: &[u8]) -> String {
    dc_rom::sha256(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The lobby's certificate, its fingerprint and the rustls server set-up.
pub struct Identity {
    pub config: Arc<ServerConfig>,
    pub fingerprint: String,
    pub cert_path: PathBuf,
    /// True when this start made the pair.
    pub created: bool,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the key
        write!(
            f,
            "Identity({}, {})",
            self.cert_path.display(),
            self.fingerprint
        )
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The pair's paths in `dir`: the Let's Encrypt names when both exist, else ours.
pub fn pair_paths(dir: &Path) -> (PathBuf, PathBuf) {
    let (lc, lk) = (dir.join(LE_CERT_FILE), dir.join(LE_KEY_FILE));
    if lc.is_file() && lk.is_file() {
        return (lc, lk);
    }
    (dir.join(CERT_FILE), dir.join(KEY_FILE))
}

/// Writes the key file readable by its owner only where the OS allows it (0600 on unix; on Windows the file
/// inherits the data folder's access list: no ACL is set here, so on Windows the host must secure the data folder
/// itself, e.g. grant it to the service account only; see docs/VPS.md).
pub(crate) fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Makes a self-signed pair in `dir` for the names `names` (plus `localhost`).
// DC-DEFAULT: LG-221 ECDSA P-256 (rcgen's default), names = localhost + the bind / advertised addresses
fn create_pair(dir: &Path, cert: &Path, key: &Path, names: &[String]) -> Result<(), String> {
    let mut sans = vec!["localhost".to_string()];
    for n in names {
        let n = n.trim();
        let unspecified = n
            .parse::<IpAddr>()
            .map(|ip| ip.is_unspecified())
            .unwrap_or(false);
        if !n.is_empty() && !unspecified && !sans.iter().any(|s| s == n) {
            sans.push(n.to_string());
        }
    }
    let ck = rcgen::generate_simple_self_signed(sans).map_err(|e| format!("certificate: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // both go to temporary names first and the cert is renamed first: a crash never leaves a key without a cert,
    // and `load_or_create` never sees a half-written file (a cert without a key is an error it reports)
    let (tc, tk) = (tmp_name(cert), tmp_name(key));
    let _ = (std::fs::remove_file(&tc), std::fs::remove_file(&tk));
    let r = write_private(&tk, ck.key_pair.serialize_pem().as_bytes())
        .map_err(|e| format!("{}: {e}", key.display()))
        .and_then(|_| {
            std::fs::write(&tc, ck.cert.pem()).map_err(|e| format!("{}: {e}", cert.display()))
        })
        .and_then(|_| std::fs::rename(&tc, cert).map_err(|e| format!("{}: {e}", cert.display())))
        .and_then(|_| std::fs::rename(&tk, key).map_err(|e| format!("{}: {e}", key.display())));
    if r.is_err() {
        let _ = (std::fs::remove_file(&tc), std::fs::remove_file(&tk));
    }
    r
}

fn tmp_name(p: &Path) -> PathBuf {
    let mut n = p.file_name().unwrap_or_default().to_os_string();
    n.push(".tmp");
    p.with_file_name(n)
}

/// Loads the pair of `dir`, making a self-signed one first when neither file exists. One file without the other is
/// an error (nothing is overwritten).
pub fn load_or_create(dir: &Path, names: &[String]) -> Result<Identity, String> {
    let (cert, key) = pair_paths(dir);
    let created = match (cert.exists(), key.exists()) {
        (true, true) => false,
        (false, false) => {
            create_pair(dir, &cert, &key, names)?;
            true
        }
        (true, false) => {
            return Err(format!(
                "{} exists but {} does not",
                cert.display(),
                key.display()
            ))
        }
        (false, true) => {
            return Err(format!(
                "{} exists but {} does not",
                key.display(),
                cert.display()
            ))
        }
    };
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert)
        .map_err(|e| format!("{}: {e}", cert.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", cert.display()))?;
    let first = chain
        .first()
        .ok_or_else(|| format!("{}: no certificate in the file", cert.display()))?;
    let fp = fingerprint(first.as_ref());
    // the key's own error text is not shown (it could quote key material); only the file name
    let pk = PrivateKeyDer::from_pem_file(&key)
        .map_err(|_| format!("{}: not a readable PEM private key", key.display()))?;
    let config = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, pk)
        .map_err(|e| format!("{}: {e}", cert.display()))?;
    Ok(Identity {
        config: Arc::new(config),
        fingerprint: fp,
        cert_path: cert,
        created,
    })
}

/// True for a loopback listen address (`127.x`, `::1`, `localhost`).
pub fn is_loopback_bind(bind: &str) -> bool {
    let b = bind.trim().trim_start_matches('[').trim_end_matches(']');
    b.eq_ignore_ascii_case("localhost")
        || b.parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// What one listener accepts.
#[derive(Clone)]
pub struct Gate {
    /// None: no certificate (a loopback lobby whose pair could not be made): plain only.
    pub identity: Option<Arc<Identity>>,
    /// `--tls loopback`: plain refused from every peer.
    pub tls_everywhere: bool,
    /// Plain refused from non-loopback peers (a public lobby without `--plain-auth-public`).
    pub tls_for_remote: bool,
}

impl Gate {
    /// The gate of the auth and web ports.
    // DC-DEFAULT: LG-224 public (or non-loopback bind) -> TLS from remote peers; LG-225 --tls loopback -> TLS from all
    pub fn new(
        identity: Option<Arc<Identity>>,
        mode: TlsMode,
        public: bool,
        plain_public: bool,
    ) -> Gate {
        Gate {
            identity,
            tls_everywhere: mode == TlsMode::Loopback,
            tls_for_remote: public && !plain_public,
        }
    }

    /// True when a plain connection from `peer` may go on.
    pub fn plain_ok(&self, peer: Option<SocketAddr>) -> bool {
        let loopback = peer
            .map(|p| unmapped(p.ip()).is_loopback())
            .unwrap_or(false);
        !self.tls_everywhere && (loopback || !self.tls_for_remote)
    }
}

fn unmapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// A first record that starts a TLS handshake: content type 22, protocol major 3.
pub fn looks_like_tls(head: &[u8]) -> bool {
    head.len() >= 2 && head[0] == 0x16 && head[1] == 0x03
}

/// One accepted connection.
pub enum Conn {
    Plain(TcpStream),
    Tls(Box<ServerConnection>, TcpStream),
}

/// What [`admit`] made of a new connection.
pub enum Admitted {
    Conn(Conn),
    /// A plain connection the gate refuses (the web port answers it with a redirect; the auth port closes it).
    PlainRefused(TcpStream),
    /// Closed before the first byte, or a failed handshake (the text is for the log).
    Failed(String),
}

/// Looks at the first bytes of `s` and runs the TLS handshake when they start one.
pub fn admit(s: TcpStream, gate: &Gate, slot: Option<Slot>) -> Admitted {
    admit_within(s, gate, Duration::from_secs(HANDSHAKE_SECS), slot)
}

/// [`admit`] with the overall time `limit` for the first bytes and the whole handshake together (a client that
/// trickles bytes is dropped at the deadline); the slot is freed when this returns.
pub fn admit_within(s: TcpStream, gate: &Gate, limit: Duration, slot: Option<Slot>) -> Admitted {
    let _slot = slot;
    let peer = s.peer_addr().ok();
    let end = std::time::Instant::now() + limit;
    // the time left, as a socket timeout (zero is not allowed)
    let left = || {
        end.checked_duration_since(std::time::Instant::now())
            .filter(|d| !d.is_zero())
            .map(|d| d.max(Duration::from_millis(1)))
    };
    let mut head = [0u8; 2];
    let n = loop {
        let Some(l) = left() else {
            return Admitted::Failed("no complete first bytes before the deadline".into());
        };
        let _ = s.set_read_timeout(Some(l));
        match s.peek(&mut head) {
            Ok(n) if n >= 2 || n == 0 => break n,
            // one byte so far: peek again once more arrives (a 2009 frame header has eight)
            Ok(_) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Admitted::Failed(format!("first bytes: {e}")),
        }
    };
    if n == 0 {
        return Admitted::Failed("closed before the first byte".into());
    }
    if !looks_like_tls(&head) {
        let _ = s.set_read_timeout(None);
        return if gate.plain_ok(peer) {
            Admitted::Conn(Conn::Plain(s))
        } else {
            Admitted::PlainRefused(s)
        };
    }
    let Some(id) = &gate.identity else {
        return Admitted::Failed("a TLS handshake, but the lobby has no certificate".into());
    };
    let mut c = match ServerConnection::new(Arc::clone(&id.config)) {
        Ok(c) => c,
        Err(e) => return Admitted::Failed(format!("TLS: {e}")),
    };
    let mut sock = s;
    // one socket operation at a time, each with the time left (rustls' own `complete_io` would loop on reads
    // that each get a fresh timeout, which a client sending one byte at a time keeps alive)
    while c.is_handshaking() || c.wants_write() {
        let Some(l) = left() else {
            return Admitted::Failed("TLS handshake: deadline passed".into());
        };
        let _ = sock.set_read_timeout(Some(l));
        let _ = sock.set_write_timeout(Some(l));
        let step = if c.wants_write() {
            c.write_tls(&mut sock).map(|_| ())
        } else {
            match c.read_tls(&mut sock) {
                Ok(0) => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                Ok(_) => c
                    .process_new_packets()
                    .map(|_| ())
                    .map_err(io::Error::other),
                Err(e) => Err(e),
            }
        };
        if let Err(e) = step {
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Admitted::Failed(format!("TLS handshake: {e}"));
        }
    }
    let _ = sock.set_read_timeout(None);
    let _ = sock.set_write_timeout(None);
    Admitted::Conn(Conn::Tls(Box::new(c), sock))
}

impl Conn {
    pub fn tcp(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) | Conn::Tls(_, s) => s,
        }
    }

    pub fn is_tls(&self) -> bool {
        matches!(self, Conn::Tls(..))
    }

    /// One reader and one writer for two threads. The TLS state sits behind a lock; socket reads happen outside it,
    /// socket writes keep the record order under a second lock.
    pub fn split(self) -> io::Result<(ConnReader, ConnWriter)> {
        match self {
            Conn::Plain(s) => {
                let w = s.try_clone()?;
                Ok((ConnReader::Plain(s), ConnWriter::Plain(w)))
            }
            Conn::Tls(c, s) => {
                let shared = Arc::new(Mutex::new(*c));
                let out = Arc::new(Mutex::new(s.try_clone()?));
                Ok((
                    ConnReader::Tls {
                        conn: Arc::clone(&shared),
                        out: Arc::clone(&out),
                        sock: s,
                        pending: Vec::new(),
                    },
                    ConnWriter::Tls { conn: shared, out },
                ))
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Encrypted bytes waiting in `c`, written in order while the connection lock is still held.
fn flush_tls(c: &mut ServerConnection, out: &Mutex<TcpStream>) -> io::Result<()> {
    if !c.wants_write() {
        return Ok(());
    }
    let mut buf = Vec::new();
    while c.wants_write() {
        c.write_tls(&mut buf)?;
    }
    lock(out).write_all(&buf)
}

pub enum ConnReader {
    Plain(TcpStream),
    Tls {
        conn: Arc<Mutex<ServerConnection>>,
        out: Arc<Mutex<TcpStream>>,
        sock: TcpStream,
        pending: Vec<u8>,
    },
}

impl ConnReader {
    /// The read timeout of the socket this reader reads from.
    pub fn set_read_timeout(&self, t: Option<Duration>) {
        let _ = match self {
            ConnReader::Plain(s) => s.set_read_timeout(t),
            ConnReader::Tls { sock, .. } => sock.set_read_timeout(t),
        };
    }
}

impl Read for ConnReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (conn, out, sock, pending) = match self {
            ConnReader::Plain(s) => return s.read(buf),
            ConnReader::Tls {
                conn,
                out,
                sock,
                pending,
            } => (conn, out, sock, pending),
        };
        let mut tmp = [0u8; 16 * 1024];
        loop {
            {
                let mut c = lock(conn);
                match c.reader().read(buf) {
                    Ok(n) => return Ok(n),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
                if !pending.is_empty() {
                    let mut rest: &[u8] = pending;
                    let r = c.read_tls(&mut rest);
                    let used = pending.len() - rest.len();
                    pending.drain(..used);
                    r?;
                    let st = c.process_new_packets();
                    let _ = flush_tls(&mut c, out);
                    st.map_err(io::Error::other)?;
                    continue;
                }
            }
            let n = sock.read(&mut tmp)?;
            if n == 0 {
                return Ok(0);
            }
            pending.extend_from_slice(&tmp[..n]);
        }
    }
}

pub enum ConnWriter {
    Plain(TcpStream),
    Tls {
        conn: Arc<Mutex<ServerConnection>>,
        out: Arc<Mutex<TcpStream>>,
    },
}

impl ConnWriter {
    pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        match self {
            ConnWriter::Plain(s) => s.write_all(data),
            ConnWriter::Tls { conn, out } => {
                let mut c = lock(conn);
                c.writer().write_all(data)?;
                flush_tls(&mut c, out)
            }
        }
    }

    /// Ends the connection (a TLS close notice first).
    pub fn close(&mut self) {
        match self {
            ConnWriter::Plain(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
            ConnWriter::Tls { conn, out } => {
                let mut c = lock(conn);
                c.send_close_notify();
                let _ = flush_tls(&mut c, out);
                let _ = lock(out).shutdown(Shutdown::Both);
            }
        }
    }
}

/// A connection served by one thread (the web port): plain or a TLS stream.
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<ServerConnection, TcpStream>>),
}

impl From<Conn> for Stream {
    fn from(c: Conn) -> Stream {
        match c {
            Conn::Plain(s) => Stream::Plain(s),
            Conn::Tls(c, s) => Stream::Tls(Box::new(rustls::StreamOwned::new(*c, s))),
        }
    }
}

impl Stream {
    pub fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(t) => &t.sock,
        }
    }

    pub fn close(&mut self) {
        if let Stream::Tls(t) = self {
            t.conn.send_close_notify();
            let _ = t.flush();
        }
        let _ = self.tcp().shutdown(Shutdown::Both);
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(t) => t.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(t) => t.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(t) => t.flush(),
        }
    }
}

/// The same-host proxy note of LG-224: `plain_ok` judges a peer by its socket address.
pub const PROXY_NOTE: &str = "TLS: the lobby judges 'loopback' by the socket peer, so a reverse proxy, tunnel or port forwarder on this host makes remote players look like loopback and their passwords would cross the public leg in clear text. If one runs here, start the lobby with --tls loopback.";

/// The start-up lines about TLS (the fingerprint in a copyable form).
pub fn startup_lines(id: &Identity, gate: &Gate) -> Vec<String> {
    let mut v = vec![
        format!(
            "TLS: certificate {} ({}); SHA-256 fingerprint {}",
            id.cert_path.display(),
            if id.created { "made now" } else { "loaded" },
            id.fingerprint
        ),
        format!(
            "TLS: players pin this server with: --server-fingerprint {}",
            id.fingerprint
        ),
    ];
    v.push(if gate.tls_everywhere {
        "TLS: the auth and web ports require TLS from every peer (--tls loopback)".into()
    } else if gate.tls_for_remote {
        "TLS: the auth and web ports require TLS from non-loopback peers; the lobby and clan ports stay plain (they carry no password)".into()
    } else {
        "TLS: the auth and web ports accept TLS and plain connections (a loopback lobby)".into()
    });
    if gate.tls_for_remote && !gate.tls_everywhere {
        v.push(PROXY_NOTE.into());
    }
    v
}

/// `dc-lobby-server tls [--db PATH] [--tls-dir DIR]`: prints the fingerprint (the pair is made when missing).
pub fn run_subcommand(args: &[String]) -> i32 {
    let mut db: Option<PathBuf> = None;
    let mut dir: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (k, inline) = match a.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let mut val = || inline.clone().or_else(|| it.next().cloned());
        match k {
            "--db" => db = val().map(PathBuf::from),
            "--tls-dir" => dir = val().map(PathBuf::from),
            "-h" | "--help" => {
                println!("usage: dc-lobby-server tls [--db PATH] [--tls-dir DIR]\n  prints the SHA-256 fingerprint of the lobby's certificate (made when missing)");
                return 0;
            }
            o => {
                eprintln!("dc-lobby-server tls: unknown argument {o:?}");
                return 2;
            }
        }
    }
    let dir = dir.unwrap_or_else(|| default_dir(&db.unwrap_or_else(crate::cli::default_db)));
    match load_or_create(&dir, &[]) {
        Ok(id) => {
            if id.created {
                eprintln!(
                    "made a new self-signed certificate {}",
                    id.cert_path.display()
                );
            }
            println!("{}", id.fingerprint);
            0
        }
        Err(e) => {
            eprintln!("dc-lobby-server tls: {e}");
            1
        }
    }
}

/// The default TLS folder: the DB's folder.
pub fn default_dir(db: &Path) -> PathBuf {
    match db.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
