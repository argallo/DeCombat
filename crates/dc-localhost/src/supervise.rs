//! The local match supervisor: runs one game server and its bots as child processes of the caller (the game client)
//! from a [`LocalMatch`].
//!
//! Order: the server starts with the description's launch line, the supervisor waits for its `@ready port=` line,
//! writes the `room` line and one `user` line per seat (each with a ticket of its own, made here from the OS random
//! source), and then the caller joins with the player's ticket and starts the bots with [`LocalHost::start_bots`].
//! Each bot gets `--udp 127.0.0.1:<port> --name <nick> --skill <n> --parent-pipe` and its ticket in the environment
//! (`DC_SEAT_TICKET`), never on its command line, so no ticket shows in a process list.
//!
//! Lifetime: every child's stdin is a pipe the supervisor holds. The server stops on stdin EOF and a bot with
//! `--parent-pipe` exits when its stdin closes, so a parent that dies in any way takes its children with it; on
//! Windows the children are also in a job object that kills them when the parent's last handle to it closes.
//! [`LocalHost::shutdown`] (also run on drop) stops the bots first (stdin closed, then killed after a grace), then the
//! server (`quit`, stdin closed, killed after 5 s). A server that exits on its own ends the match: [`LocalHost::poll`]
//! reports it and stops the bots.
//!
//! Logs: each child's stdout and stderr go to `<log_dir>/<name>.log`; the previous run's file becomes
//! `<name>.1.log`, and a file that passes the size limit is moved there too and started again. The server's event
//! lines also come back through [`LocalHost::poll`]. The tickets and the join secret are never written to a log.
// SPEC: DSERVER_PIPE.md §1 (process model), §3 (control lines), §4.1 (`@ready`), §4.3 (`@join`), §6 (30 s to ready,
// 5 s quit grace)
// DC-DEFAULT: LOC-9 (proposed) the supervisor's order, lifetime, logs and timeouts

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_wire::seatticket::{self, Hidden, Ticket};

use crate::{Hosting, LocalMatch, SpecError, Tickets};

/// The environment variable a bot reads its ticket from (the bot client's own name for it).
pub const TICKET_ENV: &str = "DC_SEAT_TICKET";
/// How long the server may take to report `@ready`.
// SPEC: DSERVER_PIPE.md §6 (spawn -> @ready 30 s)
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the server gets after `quit` before it is killed.
// SPEC: DSERVER_PIPE.md §3.4 (force-kill after a 5 s grace)
pub const SERVER_QUIT_GRACE: Duration = Duration::from_secs(5);
/// How long a bot gets after its stdin closed before it is killed.
// DC-DEFAULT: LOC-9 (proposed) 2 s
pub const BOT_QUIT_GRACE: Duration = Duration::from_secs(2);
/// Size at which a child's log file is moved to `<name>.1.log` and started again.
// DC-DEFAULT: LOC-9 (proposed) 16 MB
pub const LOG_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The programs the supervisor runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    pub server: PathBuf,
    pub bot: PathBuf,
}

impl Programs {
    /// `dc-dserver` and `dc-botclient` in `dir` (the folder of the game client's own program).
    pub fn beside(dir: &Path) -> Programs {
        let exe = |stem: &str| dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
        Programs {
            server: exe("dc-dserver"),
            bot: exe("dc-botclient"),
        }
    }
}

/// Settings of one supervised match beyond the description.
#[derive(Debug, Clone)]
pub struct Options {
    /// Folder of the children's log files; `None` discards their output (the server's events still arrive).
    pub log_dir: Option<PathBuf>,
    pub log_max_bytes: u64,
    pub ready_timeout: Duration,
    /// Arguments every bot gets after the supervisor's own (`--rom <dir>`, `--hunt`, ...).
    pub bot_args: Vec<String>,
    /// Environment variables set for every child (on top of the caller's own environment).
    pub env: Vec<(String, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            log_dir: None,
            log_max_bytes: LOG_MAX_BYTES,
            ready_timeout: READY_TIMEOUT,
            bot_args: Vec::new(),
            env: Vec::new(),
        }
    }
}

/// The match's secrets: one ticket per seat and, for a hosted match, the join secret.
#[derive(Debug, Clone)]
pub struct Secrets {
    pub player: Hidden,
    pub bots: Vec<Hidden>,
    pub join_secret: Option<Hidden>,
}

/// 128 random bits from the operating system's random source (never all zeros).
pub fn random_ticket() -> io::Result<Ticket> {
    loop {
        let mut t = [0u8; seatticket::LEN];
        getrandom::getrandom(&mut t).map_err(|e| io::Error::other(e.to_string()))?;
        if t != [0; seatticket::LEN] {
            return Ok(t);
        }
    }
}

impl Secrets {
    /// Fresh secrets for `m`: a ticket per seat, a join secret when the match is hosted.
    pub fn generate(m: &LocalMatch) -> io::Result<Secrets> {
        Ok(Secrets {
            player: Hidden(random_ticket()?),
            bots: (0..m.bots.count)
                .map(|_| random_ticket().map(Hidden))
                .collect::<io::Result<_>>()?,
            join_secret: match m.hosting {
                Hosting::Solo => None,
                Hosting::Host { .. } => Some(Hidden(random_ticket()?)),
            },
        })
    }

    pub fn tickets(&self) -> Tickets {
        Tickets {
            player: self.player.0,
            bots: self.bots.iter().map(|t| t.0).collect(),
        }
    }
}

/// Why a match could not start or went wrong.
#[derive(Debug)]
pub enum Error {
    Spec(SpecError),
    /// A program could not be started (which one, why).
    Start(String, io::Error),
    /// The server ended before `@ready` (its exit status when known).
    ServerEnded(Option<ExitStatus>),
    /// No `@ready` within the timeout.
    NotReady(Duration),
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Spec(e) => write!(f, "match description: {e}"),
            Error::Start(what, e) => write!(f, "cannot start {what}: {e}"),
            Error::ServerEnded(Some(s)) => {
                write!(f, "the game server ended before it was ready ({s})")
            }
            Error::ServerEnded(None) => write!(f, "the game server ended before it was ready"),
            Error::NotReady(t) => {
                write!(f, "the game server was not ready within {} s", t.as_secs())
            }
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<SpecError> for Error {
    fn from(e: SpecError) -> Self {
        Error::Spec(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Something that happened in the match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An event line of the server (`@join`, `@leave`, `@player`, `@result`, `@end`, ...), as printed.
    Server(String),
    /// `@join <client> <uid> <key> <team> <name>`.
    Joined { client: u16, uid: u32, nick: String },
    /// The server process ended; the match is over (the bots were stopped).
    ServerExited(Option<i32>),
    /// Bot `index` (0-based, seat order) ended.
    BotExited { index: usize, code: Option<i32> },
    /// Something the caller should show or log: a child outside the kill-on-close group, bots that did not start.
    Warning(String),
}

/// The warning for a child Windows did not put into the kill-on-close job.
// DC-DEFAULT: LOC-9 (proposed) a child outside the job is reported, not stopped
fn ungrouped(what: &str, pid: u32) -> String {
    format!(
        "{what} (process {pid}) is not in the kill-on-close group: if this program is killed, it ends only when it \
         notices its input pipe closed"
    )
}

/// Reads `@join <client> <uid> <key> <team> <name>`.
// SPEC: DSERVER_PIPE.md §4.3
pub fn parse_join(line: &str) -> Option<Event> {
    let t: Vec<&str> = line.split_whitespace().collect();
    if t.first() != Some(&"@join") || t.len() < 6 {
        return None;
    }
    Some(Event::Joined {
        client: t[1].parse().ok()?,
        uid: t[2].parse().ok()?,
        nick: t[5].to_string(),
    })
}

/// Reads the port of `@ready port=<p> ...`.
// SPEC: DSERVER_PIPE.md §4.1
pub fn parse_ready(line: &str) -> Option<u16> {
    let mut t = line.split_whitespace();
    if t.next() != Some("@ready") {
        return None;
    }
    t.find_map(|x| x.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
}

/// A child's log file: the previous run's file becomes `<name>.1.log`, and so does a file that passes the limit.
struct RotatingLog {
    path: PathBuf,
    old: PathBuf,
    max: u64,
    file: Option<File>,
    written: u64,
}

impl RotatingLog {
    fn open(dir: &Path, name: &str, max: u64) -> RotatingLog {
        let path = dir.join(format!("{name}.log"));
        let old = dir.join(format!("{name}.1.log"));
        let mut l = RotatingLog {
            path,
            old,
            max,
            file: None,
            written: 0,
        };
        l.rotate();
        l
    }

    fn rotate(&mut self) {
        self.file = None;
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.old);
            let _ = std::fs::rename(&self.path, &self.old);
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .ok();
        self.written = 0;
    }

    fn line(&mut self, l: &str) {
        let n = l.len() as u64 + 1;
        if self.written + n > self.max && self.written > 0 {
            self.rotate();
        }
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{l}");
            self.written += n;
        }
    }
}

type SharedLog = Option<Arc<Mutex<RotatingLog>>>;

fn log_of(o: &Options, name: &str) -> io::Result<SharedLog> {
    match &o.log_dir {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            Ok(Some(Arc::new(Mutex::new(RotatingLog::open(
                d,
                name,
                o.log_max_bytes.max(1),
            )))))
        }
        None => Ok(None),
    }
}

/// Copies a child's output to its log, line by line; `tx` also gets each line. A line that is not UTF-8 is passed on
/// with replacement characters, and the pipe is drained to its end whatever happens (a gone receiver, a full log), so
/// the child never blocks on a full pipe. A line longer than [`LINE_MAX`] is cut there and the rest of it skipped.
// DC-DEFAULT: LOC-9 (proposed) a child's output is always drained
fn pump(r: impl Read + Send + 'static, log: SharedLog, tx: Option<Sender<String>>) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match (&mut r).take(LINE_MAX as u64).read_until(b'\n', &mut buf) {
                Ok(0) => return,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
            if buf.len() == LINE_MAX && buf.last() != Some(&b'\n') && !skip_line(&mut r) {
                return;
            }
            while matches!(buf.last(), Some(b'\n' | b'\r')) {
                buf.pop();
            }
            let l = String::from_utf8_lossy(&buf);
            if let Some(log) = &log {
                if let Ok(mut g) = log.lock() {
                    g.line(&l);
                }
            }
            if let Some(tx) = &tx {
                let _ = tx.send(l.into_owned());
            }
        }
    });
}

/// Longest line passed on from a child's output; a longer one is cut there.
// DC-DEFAULT: LOC-9 (proposed) 64 KB
pub const LINE_MAX: usize = 64 * 1024;

/// Skips the rest of an overlong line; false at the end of the output.
fn skip_line(r: &mut impl BufRead) -> bool {
    let mut skip = Vec::new();
    loop {
        skip.clear();
        match r.take(LINE_MAX as u64).read_until(b'\n', &mut skip) {
            Ok(0) => return false,
            Ok(_) if skip.last() == Some(&b'\n') => return true,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

/// The kill-on-close group of the children (Windows: a job object; elsewhere the stdin pipes do the job).
mod group {
    #[cfg(windows)]
    pub struct Group(windows_sys::Win32::Foundation::HANDLE);

    #[cfg(windows)]
    // SAFETY: the job handle is owned by this value and only passed to the Win32 job calls
    unsafe impl Send for Group {}

    #[cfg(windows)]
    impl Group {
        /// A job object whose children die when its last handle closes (this process's, at the latest at its end).
        pub fn new() -> Option<Group> {
            use windows_sys::Win32::System::JobObjects::{
                CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };
            // SAFETY: plain Win32 calls with a zeroed, correctly sized information block
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    windows_sys::Win32::Foundation::CloseHandle(h);
                    return None;
                }
                Some(Group(h))
            }
        }

        /// Puts a started child into the job (false when Windows refused).
        pub fn add(&self, c: &std::process::Child) -> bool {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: both handles are valid for the duration of the call
            unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                    self.0,
                    c.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                ) != 0
            }
        }
    }

    #[cfg(windows)]
    impl Drop for Group {
        fn drop(&mut self) {
            // SAFETY: the handle was made by CreateJobObjectW and is closed once
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }

    #[cfg(not(windows))]
    pub struct Group;

    #[cfg(not(windows))]
    impl Group {
        pub fn new() -> Option<Group> {
            Some(Group)
        }
        pub fn add(&self, _c: &std::process::Child) -> bool {
            true
        }
    }
}

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    exited: Option<Option<i32>>,
}

impl Proc {
    fn check(&mut self) -> Option<Option<i32>> {
        if self.exited.is_none() {
            if let Ok(Some(s)) = self.child.try_wait() {
                self.exited = Some(s.code());
                return self.exited;
            }
        }
        None
    }

    /// Closes stdin, waits up to `grace`, then kills.
    fn stop(&mut self, grace: Duration) {
        stop_all(std::slice::from_mut(self), grace);
    }
}

/// Closes every child's stdin at once, waits up to `grace` for all of them together, then kills the ones still
/// running: stopping N children takes at most one grace, not N.
// DC-DEFAULT: LOC-9 (proposed) one deadline for all bots
fn stop_all(procs: &mut [Proc], grace: Duration) {
    for p in procs.iter_mut() {
        p.stdin = None;
    }
    let until = Instant::now() + grace;
    loop {
        let mut running = false;
        for p in procs.iter_mut() {
            p.check();
            running |= p.exited.is_none();
        }
        if !running || Instant::now() >= until {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for p in procs.iter_mut().filter(|p| p.exited.is_none()) {
        let _ = p.child.kill();
    }
    for p in procs.iter_mut().filter(|p| p.exited.is_none()) {
        p.exited = Some(p.child.wait().ok().and_then(|s| s.code()));
    }
}

/// A running local match.
pub struct LocalHost {
    spec: LocalMatch,
    programs: Programs,
    options: Options,
    secrets: Secrets,
    port: u16,
    group: Option<group::Group>,
    server: Proc,
    lines: Receiver<String>,
    bots: Vec<Proc>,
    server_reported: bool,
    stopped: bool,
    /// Every child went into the kill-on-close group (Windows; always true elsewhere).
    grouped: bool,
    /// Warnings not yet returned by [`LocalHost::poll`].
    warnings: Vec<String>,
}

impl LocalHost {
    /// Starts the server for `spec`, waits for its `@ready`, and writes the `room` and `user` lines. The bots are
    /// not started yet ([`LocalHost::start_bots`]).
    pub fn start(
        spec: &LocalMatch,
        programs: &Programs,
        options: Options,
    ) -> Result<LocalHost, Error> {
        let secrets = Secrets::generate(spec)?;
        LocalHost::start_with(spec, programs, options, secrets)
    }

    /// [`LocalHost::start`] with the given secrets (tests).
    pub fn start_with(
        spec: &LocalMatch,
        programs: &Programs,
        options: Options,
        secrets: Secrets,
    ) -> Result<LocalHost, Error> {
        let args = spec.server_args()?;
        // LOC-11: the join secret goes in the server's environment, never on its command line
        let secret_env = spec.server_env(secrets.join_secret.as_ref().map(|s| &s.0))?;
        let lines = spec.pipe_lines(&secrets.tickets())?;
        let group = group::Group::new();
        let mut cmd = Command::new(&programs.server);
        cmd.args(&args)
            .envs(options.env.iter().map(|(k, v)| (k, v)))
            .env_remove(crate::JOIN_SECRET_ENV)
            .envs(secret_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // the log first: nothing can fail between the spawn and the moment the child is owned
        let log = log_of(&options, "dc-dserver")?;
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Start(programs.server.display().to_string(), e))?;
        let grouped = group.as_ref().is_some_and(|g| g.add(&child));
        let mut warnings = Vec::new();
        if !grouped {
            warnings.push(ungrouped("the game server", child.id()));
        }
        let (tx, rx) = mpsc::channel();
        if let Some(o) = child.stdout.take() {
            pump(o, log.clone(), Some(tx));
        }
        if let Some(e) = child.stderr.take() {
            pump(e, log, None);
        }
        let stdin = child.stdin.take();
        let mut host = LocalHost {
            spec: spec.clone(),
            programs: programs.clone(),
            options,
            secrets,
            port: 0,
            group,
            server: Proc {
                child,
                stdin,
                exited: None,
            },
            lines: rx,
            bots: Vec::new(),
            server_reported: false,
            stopped: false,
            grouped,
            warnings,
        };
        host.port = host.wait_ready()?;
        for l in &lines {
            host.send_line(l)?;
        }
        Ok(host)
    }

    fn wait_ready(&mut self) -> Result<u16, Error> {
        let until = Instant::now() + self.options.ready_timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::NotReady(self.options.ready_timeout));
            }
            match self
                .lines
                .recv_timeout(left.min(Duration::from_millis(100)))
            {
                Ok(l) => {
                    if let Some(p) = parse_ready(&l) {
                        return Ok(p);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Ok(Some(s)) = self.server.child.try_wait() {
                        return Err(Error::ServerEnded(Some(s)));
                    }
                }
                // the server closed its stdout: it is ending, or broken; never wait past the deadline for it
                Err(RecvTimeoutError::Disconnected) => {
                    if let Ok(Some(s)) = self.server.child.try_wait() {
                        return Err(Error::ServerEnded(Some(s)));
                    }
                    std::thread::sleep(left.min(Duration::from_millis(50)));
                }
            }
        }
    }

    /// Writes one control line to the server.
    pub fn send_line(&mut self, l: &str) -> io::Result<()> {
        let s = self.server.stdin.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "the server's stdin is closed")
        })?;
        writeln!(s, "{l}")?;
        s.flush()
    }

    /// The server's UDP port (on 127.0.0.1 for the player and the bots).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The player's own seat ticket (for the client's hello).
    pub fn player_ticket(&self) -> Hidden {
        self.secrets.player
    }

    /// The join secret of a hosted match (what joiners send as their ticket).
    pub fn join_secret(&self) -> Option<Hidden> {
        self.secrets.join_secret
    }

    /// The description the match runs.
    pub fn spec(&self) -> &LocalMatch {
        &self.spec
    }

    /// The arguments bot `i` is started with (no ticket in them).
    pub fn bot_args(&self, i: usize) -> Vec<String> {
        let names = self.spec.bot_names();
        let mut a = vec![
            "--udp".to_string(),
            format!("127.0.0.1:{}", self.port),
            "--name".into(),
            names.get(i).cloned().unwrap_or_default(),
            "--skill".into(),
            self.spec.bots.skill.to_string(),
            "--parent-pipe".into(),
        ];
        a.extend(self.options.bot_args.iter().cloned());
        a
    }

    /// Starts every bot of the description (each with its ticket in [`TICKET_ENV`]).
    pub fn start_bots(&mut self) -> Result<(), Error> {
        for i in self.bots.len()..self.secrets.bots.len() {
            let mut cmd = Command::new(&self.programs.bot);
            cmd.args(self.bot_args(i))
                .envs(self.options.env.iter().map(|(k, v)| (k, v)))
                // LOC-11: the match's join secret is the server's alone
                .env_remove(crate::JOIN_SECRET_ENV)
                .env(TICKET_ENV, seatticket::to_hex(&self.secrets.bots[i].0))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let log = log_of(&self.options, &format!("dc-botclient-{}", i + 1))?;
            let mut child = cmd
                .spawn()
                .map_err(|e| Error::Start(self.programs.bot.display().to_string(), e))?;
            if !self.group.as_ref().is_some_and(|g| g.add(&child)) {
                self.grouped = false;
                self.warnings
                    .push(ungrouped(&format!("bot {}", i + 1), child.id()));
            }
            if let Some(o) = child.stdout.take() {
                pump(o, log.clone(), None);
            }
            if let Some(e) = child.stderr.take() {
                pump(e, log, None);
            }
            let stdin = child.stdin.take();
            self.bots.push(Proc {
                child,
                stdin,
                exited: None,
            });
        }
        Ok(())
    }

    /// Process ids of the server and the bots started so far (diagnostics, tests).
    pub fn pids(&self) -> (u32, Vec<u32>) {
        (
            self.server.child.id(),
            self.bots.iter().map(|b| b.child.id()).collect(),
        )
    }

    /// What happened since the last call, without waiting. When the server has ended, the bots are stopped.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out: Vec<Event> = self.warnings.drain(..).map(Event::Warning).collect();
        while let Ok(l) = self.lines.try_recv() {
            if let Some(j) = parse_join(&l) {
                out.push(j);
            }
            out.push(Event::Server(l));
        }
        for (index, b) in self.bots.iter_mut().enumerate() {
            if let Some(code) = b.check() {
                out.push(Event::BotExited { index, code });
            }
        }
        if !self.server_reported {
            if let Some(code) = self.server.check() {
                // the remaining event lines first
                while let Ok(l) = self.lines.recv_timeout(Duration::from_millis(50)) {
                    if let Some(j) = parse_join(&l) {
                        out.push(j);
                    }
                    out.push(Event::Server(l));
                }
                self.server_reported = true;
                stop_all(&mut self.bots, BOT_QUIT_GRACE);
                out.push(Event::ServerExited(code));
            }
        }
        out
    }

    /// Waits up to `timeout` for an event `f` accepts; the events before it are returned with it.
    pub fn wait_for(
        &mut self,
        timeout: Duration,
        mut f: impl FnMut(&Event) -> bool,
    ) -> (bool, Vec<Event>) {
        let until = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let got = self.poll();
            let hit = got.iter().any(&mut f);
            seen.extend(got);
            if hit {
                return (true, seen);
            }
            if Instant::now() >= until {
                return (false, seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// True when every child is in the group that dies with this process (a Windows job object with kill on
    /// close); on other systems the children's stdin pipes do that and this is true.
    pub fn grouped(&self) -> bool {
        self.grouped
    }

    /// True while the server runs.
    pub fn running(&mut self) -> bool {
        self.server.exited.is_none() && !matches!(self.server.child.try_wait(), Ok(Some(_)))
    }

    /// Stops everything: the bots (stdin closed, killed after [`BOT_QUIT_GRACE`]), then the server (`quit`, stdin
    /// closed, killed after [`SERVER_QUIT_GRACE`]). Safe to call twice; also run on drop.
    pub fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        stop_all(&mut self.bots, BOT_QUIT_GRACE);
        let _ = self.send_line("quit");
        self.server.stop(SERVER_QUIT_GRACE);
    }
}

/// A [`LocalHost`] run by a thread of its own: it starts the bots once the seat `bots_after` (a user id) joined, passes
/// every event on, and shuts the match down when the server ends or the handle is stopped or dropped.
pub struct Background {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    events: Receiver<Event>,
}

impl LocalHost {
    /// Moves the match to a thread of its own; the bots start when user id `bots_after` joins (`None`: at once). When
    /// the thread cannot be started the match is stopped and the error returned.
    pub fn run_in_background(mut self, bots_after: Option<u32>) -> Result<Background, Error> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("local-match".into())
            .spawn(move || {
                let mut bots_started = false;
                let start_bots = |h: &mut LocalHost, started: &mut bool| {
                    if !*started {
                        *started = true;
                        if let Err(e) = h.start_bots() {
                            let _ = tx.send(Event::Warning(format!("bots not started: {e}")));
                        }
                    }
                };
                if bots_after.is_none() {
                    start_bots(&mut self, &mut bots_started);
                }
                while !flag.load(Ordering::Relaxed) {
                    let mut over = false;
                    for e in self.poll() {
                        match &e {
                            Event::Joined { uid, .. } if Some(*uid) == bots_after => {
                                start_bots(&mut self, &mut bots_started)
                            }
                            Event::ServerExited(_) => over = true,
                            _ => {}
                        }
                        let _ = tx.send(e);
                    }
                    if over {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                self.shutdown();
            })
            // a failed spawn drops the closure and with it the match, which shuts down on drop
            .map_err(Error::Io)?;
        Ok(Background {
            stop,
            thread: Some(thread),
            events: rx,
        })
    }
}

impl Background {
    /// The events since the last call, without waiting.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Waits up to `timeout` for an event `f` accepts.
    pub fn wait_for(&self, timeout: Duration, mut f: impl FnMut(&Event) -> bool) -> bool {
        let until = Instant::now() + timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(e) if f(&e) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    /// Stops the match (bots, then the server) and waits for it.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.halt();
    }
}

impl Drop for LocalHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_and_join_lines() {
        assert_eq!(parse_ready("@ready port=29950 world=synth"), Some(29950));
        assert_eq!(parse_ready("@ready world=synth"), None);
        assert_eq!(parse_ready("@join 0 1 1 0 Me"), None);
        assert_eq!(
            parse_join("@join 3 9 0 1 Guest"),
            Some(Event::Joined {
                client: 3,
                uid: 9,
                nick: "Guest".into()
            })
        );
        assert_eq!(parse_join("@join 3 9"), None);
        assert_eq!(parse_join("@leave 3"), None);
    }

    #[test]
    fn secrets_are_random_per_seat_and_hosted_matches_get_a_join_secret() {
        let mut m: LocalMatch = serde_json_free_spec();
        m.bots.count = 3;
        let s = Secrets::generate(&m).unwrap();
        assert_eq!(s.bots.len(), 3);
        assert!(s.join_secret.is_none(), "solo: no join secret");
        let mut all: Vec<Ticket> = s.bots.iter().map(|t| t.0).collect();
        all.push(s.player.0);
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 4, "every seat its own ticket");
        assert!(all.iter().all(|t| *t != [0; seatticket::LEN]));
        m.hosting = Hosting::Host {
            port: 29950,
            max_players: 8,
        };
        let h = Secrets::generate(&m).unwrap();
        assert!(h.join_secret.is_some());
        assert_eq!(format!("{:?}", h.join_secret), "Some(Ticket(..))");
    }

    fn serde_json_free_spec() -> LocalMatch {
        LocalMatch {
            world: crate::World::Rom { name: "w".into() },
            rom: None,
            content: None,
            mode: crate::Mode {
                key: "TDM".into(),
                kind: 0,
                goal: 1,
                time_secs: 60,
                friendly_fire: false,
                teams: true,
            },
            player: crate::Seat {
                name: "Me".into(),
                loadout: Vec::new(),
            },
            bots: crate::Bots::default(),
            practice: crate::Practice::default(),
            server_loadout: Vec::new(),
            tick_hz: None,
            hosting: Hosting::Solo,
            idle_secs: None,
            team_empty_secs: None,
            log: None,
            verbose: false,
        }
    }

    #[test]
    fn a_log_moves_the_last_run_aside_and_rotates_at_its_limit() {
        let dir = std::env::temp_dir().join(format!("dc-localhost-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("x.log"),
            "last run
",
        )
        .unwrap();
        let mut l = RotatingLog::open(&dir, "x", 20);
        assert_eq!(
            std::fs::read_to_string(dir.join("x.1.log")).unwrap(),
            "last run
"
        );
        l.line("0123456789");
        l.line("abcdefghij");
        drop(l);
        assert_eq!(
            std::fs::read_to_string(dir.join("x.1.log")).unwrap(),
            "0123456789
"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("x.log")).unwrap(),
            "abcdefghij
"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
