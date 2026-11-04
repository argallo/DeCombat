//! Attempt limits: registration attempts per source address (LG-145) and failed logins per name and per address
//! (LG-146). In memory only; a lobby restart forgets them.
// DC-DEFAULT: LG-145 registration rate limit; LG-146 login lockout

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Registration attempts per source address within [`WINDOW`].
pub const REGISTER_ATTEMPTS: usize = 5;
/// Failed logins per name or per address within [`WINDOW`] that start a lockout.
pub const LOGIN_FAILURES: usize = 10;
pub const WINDOW: Duration = Duration::from_secs(10 * 60);
pub const LOCKOUT: Duration = Duration::from_secs(5 * 60);
/// Keys kept before empty ones are swept.
const SWEEP_AT: usize = 4096;

/// Events per key within a sliding window.
#[derive(Debug)]
pub struct Window {
    span: Duration,
    hits: HashMap<String, VecDeque<Instant>>,
}

impl Window {
    pub fn new(span: Duration) -> Window {
        Window {
            span,
            hits: HashMap::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        let span = self.span;
        let fresh = |t: &Instant| now.saturating_duration_since(*t) < span;
        for q in self.hits.values_mut() {
            while q.front().is_some_and(|t| !fresh(t)) {
                q.pop_front();
            }
        }
        self.hits.retain(|_, q| !q.is_empty());
    }

    /// Records one event for `key`; returns the events of `key` within the window, this one included.
    pub fn hit(&mut self, key: &str, now: Instant) -> usize {
        if self.hits.len() >= SWEEP_AT {
            self.prune(now);
        }
        let span = self.span;
        let q = self.hits.entry(key.to_string()).or_default();
        while q
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= span)
        {
            q.pop_front();
        }
        q.push_back(now);
        q.len()
    }

    pub fn clear(&mut self, key: &str) {
        self.hits.remove(key);
    }

    /// The events of `key` within the window.
    pub fn count(&self, key: &str, now: Instant) -> usize {
        self.hits.get(key).map_or(0, |q| {
            q.iter()
                .filter(|t| now.saturating_duration_since(**t) < self.span)
                .count()
        })
    }
}

/// LG-146: failed logins per name and per source address; ten within ten minutes lock that key for five minutes (never for good).
#[derive(Debug)]
pub struct LoginGuard {
    fails: Window,
    locked: HashMap<String, Instant>,
    /// LG-328: password checks under way per key; they count as failures until their outcome is known.
    running: HashMap<String, usize>,
}

impl Default for LoginGuard {
    fn default() -> Self {
        LoginGuard {
            fails: Window::new(WINDOW),
            locked: HashMap::new(),
            running: HashMap::new(),
        }
    }
}

/// The two keys of a login: the name (any case) and the source address without its port.
pub fn login_keys(login: &str, peer: &str) -> [String; 2] {
    [
        format!("name:{}", login.to_lowercase()),
        format!("addr:{}", peer_ip(peer)),
    ]
}

/// `1.2.3.4:5678` / `[::1]:5678` -> the address part; anything else unchanged.
pub fn peer_ip(peer: &str) -> String {
    match peer.parse::<std::net::SocketAddr>() {
        Ok(a) => a.ip().to_string(),
        Err(_) => peer.to_string(),
    }
}

impl LoginGuard {
    /// The first of `keys` that is locked at `now`.
    pub fn locked<'k>(&mut self, keys: &'k [String], now: Instant) -> Option<&'k str> {
        self.locked.retain(|_, until| *until > now);
        keys.iter()
            .find(|k| self.locked.contains_key(k.as_str()))
            .map(|k| k.as_str())
    }

    /// Records a failed login for each key; returns the keys that became locked by it.
    pub fn fail(&mut self, keys: &[String], now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        for k in keys {
            if self.fails.hit(k, now) >= LOGIN_FAILURES && !self.locked.contains_key(k) {
                self.locked.insert(k.clone(), now + LOCKOUT);
                out.push(k.clone());
            }
        }
        out
    }

    /// LG-328: takes a place for a password check that runs off the lock. With the checks counted as failures until
    /// they end, concurrent guesses cannot overrun the limit of a name or an address. False: no place, nothing changed.
    // DC-DEFAULT: LG-328 (proposed, project decision)
    pub fn begin_attempt(&mut self, keys: &[String], now: Instant) -> bool {
        let full = keys.iter().any(|k| {
            self.fails.count(k, now) + self.running.get(k).copied().unwrap_or(0) >= LOGIN_FAILURES
        });
        if full {
            return false;
        }
        for k in keys {
            *self.running.entry(k.clone()).or_insert(0) += 1;
        }
        true
    }

    /// Gives back the place of [`LoginGuard::begin_attempt`].
    pub fn end_attempt(&mut self, keys: &[String]) {
        for k in keys {
            if let Some(n) = self.running.get_mut(k) {
                *n -= 1;
                if *n == 0 {
                    self.running.remove(k);
                }
            }
        }
    }

    /// A good login clears the failures of its name (not of its address).
    pub fn success(&mut self, name_key: &str) {
        self.fails.clear(name_key);
    }
}

/// Connections one source address may hold on the lobby's TCP ports together (a player holds at most three plus a web
/// request; a house or a campus behind one address has room).
// DC-DEFAULT: LG-326 (proposed) per-address and total connection caps, a time limit before the login
pub const MAX_CONNS_PER_ADDR: usize = 64;
/// Connections of all addresses together. Each takes up to three file descriptors and two threads: this stays below
/// the units' `LimitNOFILE`, so the accept loops keep working instead of running out of descriptors.
pub const MAX_CONNS_TOTAL: usize = 4000;
/// A connection that has not logged in within this time is closed. Logged in, it may sit idle for as long as it likes.
pub const PRE_LOGIN_TIMEOUT: Duration = Duration::from_secs(120);

/// Open connections per source address. Loopback peers are not counted (the host's own bots and tools).
#[derive(Clone, Debug)]
pub struct ConnCap {
    per_addr: usize,
    total: usize,
    open: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

/// One counted connection; dropping it gives the place back.
#[derive(Debug)]
pub struct ConnSlot {
    ip: IpAddr,
    open: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        let mut m = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

/// Why a connection was not let in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnRefused {
    Addr,
    Total,
}

impl ConnCap {
    pub fn new(per_addr: usize, total: usize) -> ConnCap {
        ConnCap {
            per_addr,
            total,
            open: Arc::default(),
        }
    }

    /// A place for a connection from `ip`; `Ok(None)` for a loopback peer, which is not counted.
    pub fn admit(&self, ip: IpAddr) -> Result<Option<ConnSlot>, ConnRefused> {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        if ip.is_loopback() {
            return Ok(None);
        }
        let mut m = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if m.values().sum::<usize>() >= self.total {
            return Err(ConnRefused::Total);
        }
        let n = m.entry(ip).or_insert(0);
        if *n >= self.per_addr {
            return Err(ConnRefused::Addr);
        }
        *n += 1;
        Ok(Some(ConnSlot {
            ip,
            open: Arc::clone(&self.open),
        }))
    }
}

/// LG-329: messages of one connection: a burst, then a steady rate. Far above what a client's screens send.
// DC-DEFAULT: LG-329 (proposed, project decision)
pub const REQUEST_BURST: f64 = 200.0;
pub const REQUESTS_PER_SEC: f64 = 50.0;
/// LG-329: lobby and room chat lines of one connection: a burst, then a steady rate.
pub const CHAT_BURST: f64 = 8.0;
pub const CHATS_PER_SEC: f64 = 1.0;

/// LG-329: one token bucket.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
    burst: f64,
    rate: f64,
}

impl Bucket {
    fn new(burst: f64, rate: f64, now: Instant) -> Bucket {
        Bucket {
            tokens: burst,
            at: now,
            burst,
            rate,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// LG-329: the message rates of one connection, held by its reader thread (so a flood is dropped without the lock).
#[derive(Debug)]
pub struct SessionRate {
    any: Bucket,
    chat: Bucket,
}

impl SessionRate {
    pub fn new(now: Instant) -> SessionRate {
        SessionRate {
            any: Bucket::new(REQUEST_BURST, REQUESTS_PER_SEC, now),
            chat: Bucket::new(CHAT_BURST, CHATS_PER_SEC, now),
        }
    }

    /// True when message `id` may go on; a dropped message costs nothing further.
    pub fn admit(&mut self, id: u16, now: Instant) -> bool {
        self.any.take(now) && (id != crate::msg::CHAT_SEND || self.chat.take(now))
    }
}

/// The limits the lobby keeps (one per `Core`).
#[derive(Debug)]
pub struct Limits {
    pub register: Window,
    pub login: LoginGuard,
    /// LG-241: recovery requests per address, as `register`.
    pub recover: Window,
    /// LG-241: login-ID mails per recipient in an hour.
    pub id_mail: Window,
    /// LG-326: the connections of the TCP listeners, shared by all of them.
    pub conns: ConnCap,
    /// LG-326: [`PRE_LOGIN_TIMEOUT`].
    pub pre_login: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            register: Window::new(WINDOW),
            login: LoginGuard::default(),
            recover: Window::new(WINDOW),
            id_mail: Window::new(Duration::from_secs(3600)),
            conns: ConnCap::new(MAX_CONNS_PER_ADDR, MAX_CONNS_TOTAL),
            pre_login: PRE_LOGIN_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_are_capped_per_address_and_in_total_and_loopback_is_free() {
        let cap = ConnCap::new(2, 3);
        let a: IpAddr = "198.51.100.4".parse().unwrap();
        let b: IpAddr = "198.51.100.5".parse().unwrap();
        let (a1, a2) = (cap.admit(a).unwrap(), cap.admit(a).unwrap());
        assert_eq!(cap.admit(a).unwrap_err(), ConnRefused::Addr);
        let b1 = cap.admit(b).unwrap();
        assert_eq!(cap.admit(b).unwrap_err(), ConnRefused::Total);
        // the host's own tools are never refused, however many
        for _ in 0..10 {
            assert!(cap.admit("127.0.0.1".parse().unwrap()).unwrap().is_none());
            assert!(cap.admit("::ffff:127.0.0.1".parse().unwrap()).unwrap().is_none());
        }
        // a closed connection gives its place back; a mapped address counts as the same peer
        drop(a1);
        drop(b1);
        let mapped = cap.admit("::ffff:198.51.100.4".parse().unwrap()).unwrap();
        assert_eq!(cap.admit(a).unwrap_err(), ConnRefused::Addr);
        drop((a2, mapped));
        assert!(cap.open.lock().unwrap().is_empty());
    }

    #[test]
    fn running_checks_count_against_the_limit_until_they_end() {
        let t0 = Instant::now();
        let mut g = LoginGuard::default();
        let keys = login_keys("Quinn", "198.51.100.4:6000");
        for _ in 0..LOGIN_FAILURES {
            assert!(g.begin_attempt(&keys, t0));
        }
        assert!(!g.begin_attempt(&keys, t0), "ten at once are the limit");
        let other = login_keys("Erin", "198.51.100.5:6000");
        assert!(g.begin_attempt(&other, t0), "another name and address go on");
        g.end_attempt(&other);
        for _ in 0..LOGIN_FAILURES {
            g.end_attempt(&keys);
        }
        assert!(g.running.is_empty());
        assert!(g.begin_attempt(&keys, t0));
        g.end_attempt(&keys);
        // nine failures leave one place
        for _ in 0..LOGIN_FAILURES - 1 {
            g.fail(&keys, t0);
        }
        assert!(g.begin_attempt(&keys, t0));
        assert!(!g.begin_attempt(&keys, t0));
    }

    #[test]
    fn session_rates_leave_normal_play_alone_and_stop_a_flood() {
        let t0 = Instant::now();
        let mut r = SessionRate::new(t0);
        // a person chatting one line every two seconds for ten minutes
        for i in 0..300u64 {
            let t = t0 + Duration::from_secs(2 * i);
            assert!(r.admit(crate::msg::CHAT_SEND, t), "line {i}");
        }
        let mut r = SessionRate::new(t0);
        let chats = (0..100)
            .filter(|_| r.admit(crate::msg::CHAT_SEND, t0))
            .count();
        assert_eq!(chats, CHAT_BURST as usize);
        let mut r = SessionRate::new(t0);
        let any = (0..1000).filter(|_| r.admit(0x370, t0)).count();
        assert_eq!(any, REQUEST_BURST as usize);
        assert!(r.admit(0x370, t0 + Duration::from_secs(1)), "it refills");
    }

    #[test]
    fn window_counts_within_its_span() {
        let t0 = Instant::now();
        let mut w = Window::new(Duration::from_secs(600));
        for i in 1..=5 {
            assert_eq!(w.hit("a", t0 + Duration::from_secs(i)), i as usize);
        }
        assert_eq!(w.hit("b", t0), 1);
        assert_eq!(
            w.hit("a", t0 + Duration::from_secs(602)),
            4,
            "the first two left the window"
        );
    }

    #[test]
    fn ten_failures_lock_for_five_minutes_then_each_failure_relocks() {
        let t0 = Instant::now();
        let mut g = LoginGuard::default();
        let keys = login_keys("Erika", "198.51.100.4:6000");
        assert_eq!(keys[1], "addr:198.51.100.4");
        for i in 0..(LOGIN_FAILURES as u64 - 1) {
            assert!(g.fail(&keys, t0 + Duration::from_secs(i)).is_empty());
            assert!(g.locked(&keys, t0 + Duration::from_secs(i)).is_none());
        }
        assert_eq!(
            g.fail(&keys, t0 + Duration::from_secs(LOGIN_FAILURES as u64 - 1))
                .len(),
            2
        );
        assert_eq!(
            g.locked(&keys, t0 + Duration::from_secs(LOGIN_FAILURES as u64)),
            Some("name:erika")
        );
        assert!(g
            .locked(
                &keys,
                t0 + Duration::from_secs(LOGIN_FAILURES as u64 + LOCKOUT.as_secs())
            )
            .is_none());
        // still ten failures in the window: the next failure locks again
        let relock = t0 + Duration::from_secs(LOGIN_FAILURES as u64 + LOCKOUT.as_secs() + 1);
        assert_eq!(g.fail(&keys, relock).len(), 2);
        // a good login clears the name, not the address
        g.success(&keys[0]);
        let t = relock + LOCKOUT + Duration::from_secs(1);
        assert!(g.locked(&keys, t).is_none());
        // the old failures left the ten-minute window meanwhile: no permanent lock, a fresh count starts
        assert!(g.fail(&keys, t).is_empty());
    }
}
