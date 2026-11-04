//! The connection test (CLI-1843): small UDP probes to the lobby's echo (its web port number over UDP, dc-lobby-server
//! LG-361) for round trip, jitter and loss, then growing sizes for the largest datagram that comes back, and a verdict
//! in plain words. Wire format and thresholds: docs/DIAGNOSTICS.md §4. The lobby's reply is never longer than the
//! request; a longer one is not taken as a reply.
// DC-DEFAULT: CLI-1843 (proposed, USER REQUEST 2026-11-02, placement by project decision) probes, sizes, verdicts

use serde_json::{json, Value};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const MAGIC_REQUEST: &[u8; 4] = b"DCPR";
pub const MAGIC_REPLY: &[u8; 4] = b"DCPA";
pub const VERSION: u8 = 1;
pub const HEADER: usize = 16;
/// The small probes: count, gap and their size.
pub const SMALL_COUNT: u32 = 20;
pub const SMALL_GAP: Duration = Duration::from_millis(50);
pub const SMALL_SIZE: usize = 64;
/// How long a probe's answer may take.
pub const WAIT: Duration = Duration::from_millis(800);
/// The size ladder (the larger ones are IP-fragmented on the way) and the tries per size.
pub const SIZES: &[usize] = &[512, 1024, 1200, 1300, 1400, 1472, 2000, 4000, 8000];
pub const TRIES: u32 = 3;
/// Verdict thresholds.
pub const LOSS_PCT: f32 = 5.0;
pub const FAR_MS: f32 = 150.0;
pub const JITTER_MS: f32 = 30.0;
pub const GOOD_SIZE: usize = 1400;

/// A probe datagram of `len` bytes (at least the header) with sequence `seq`.
pub fn request(seq: u64, len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len.max(HEADER)];
    v[..4].copy_from_slice(MAGIC_REQUEST);
    v[4] = VERSION;
    v[8..16].copy_from_slice(&seq.to_le_bytes());
    // filler that does not compress, so a link that compresses cannot make a large probe small
    let mut x = seq ^ 0x9e37_79b9_7f4a_7c15;
    for b in v[HEADER..].iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    v
}

/// The sequence of a reply, when `d` is one: the reply magic, the version, and never longer than the largest
/// request.
pub fn reply_seq(d: &[u8], max_request: usize) -> Option<u64> {
    if d.len() < HEADER || d.len() > max_request || &d[..4] != MAGIC_REPLY || d[4] != VERSION {
        return None;
    }
    Some(u64::from_le_bytes(d[8..16].try_into().ok()?))
}

/// What to test: a name for the player and the echo's address; `lobby` = the lobby itself (its echo surely runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub addr: SocketAddr,
    pub lobby: bool,
}

/// The measured numbers of one target.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measured {
    pub sent: u32,
    pub rtts: Vec<f32>,
    /// Sizes tried and whether one try came back.
    pub sizes: Vec<(usize, bool)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Blocked,
    NoService,
    Loss,
    Far,
    Unstable,
    SmallPackets,
    Failed,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Good => "good",
            Verdict::Blocked => "udp_blocked",
            Verdict::NoService => "no_answer",
            Verdict::Loss => "loss",
            Verdict::Far => "slow",
            Verdict::Unstable => "unstable",
            Verdict::SmallPackets => "large_packets_dropped",
            Verdict::Failed => "failed",
        }
    }

    /// Short word for a list row.
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Good => "Good",
            Verdict::Blocked => "UDP blocked",
            Verdict::NoService => "No answer",
            Verdict::Loss => "Packet loss",
            Verdict::Far => "Slow",
            Verdict::Unstable => "Unstable",
            Verdict::SmallPackets => "Large packets lost",
            Verdict::Failed => "Test failed",
        }
    }
}

impl Measured {
    pub fn got(&self) -> u32 {
        self.rtts.len() as u32
    }

    pub fn loss_pct(&self) -> f32 {
        if self.sent == 0 {
            return 0.0;
        }
        100.0 * (self.sent - self.got().min(self.sent)) as f32 / self.sent as f32
    }

    pub fn rtt_mean(&self) -> Option<f32> {
        (!self.rtts.is_empty()).then(|| self.rtts.iter().sum::<f32>() / self.rtts.len() as f32)
    }

    pub fn rtt_min(&self) -> Option<f32> {
        self.rtts.iter().copied().reduce(f32::min)
    }

    pub fn rtt_max(&self) -> Option<f32> {
        self.rtts.iter().copied().reduce(f32::max)
    }

    /// Mean absolute difference of successive round trips.
    pub fn jitter(&self) -> Option<f32> {
        (self.rtts.len() >= 2).then(|| {
            self.rtts.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / (self.rtts.len() - 1) as f32
        })
    }

    /// The largest size that came back (the small probes count).
    pub fn largest(&self) -> Option<usize> {
        let small = (!self.rtts.is_empty()).then_some(SMALL_SIZE);
        self.sizes.iter().filter(|(_, ok)| *ok).map(|(s, _)| *s).max().or(small)
    }

    /// The verdict, in the order of docs/DIAGNOSTICS.md §4.
    pub fn verdict(&self, lobby: bool) -> Verdict {
        if self.error.is_some() && self.sent == 0 {
            return Verdict::Failed;
        }
        if self.rtts.is_empty() {
            return if lobby { Verdict::Blocked } else { Verdict::NoService };
        }
        if self.loss_pct() > LOSS_PCT {
            return Verdict::Loss;
        }
        if self.rtt_mean().unwrap_or(0.0) > FAR_MS {
            return Verdict::Far;
        }
        if self.jitter().unwrap_or(0.0) > JITTER_MS {
            return Verdict::Unstable;
        }
        if self.largest().unwrap_or(0) < GOOD_SIZE {
            return Verdict::SmallPackets;
        }
        Verdict::Good
    }

    /// A list row's few words: the verdict and its numbers.
    pub fn short(&self, lobby: bool) -> String {
        let rtt = self.rtt_mean().map_or(0, |r| r.round() as i64);
        let v = self.verdict(lobby);
        match v {
            Verdict::Good | Verdict::Far | Verdict::Unstable => {
                format!("{}: {rtt} ms, {:.0}% loss", v.word(), self.loss_pct())
            }
            Verdict::Loss => format!("{}: {:.0}%, {rtt} ms", v.word(), self.loss_pct()),
            Verdict::SmallPackets => format!("{}: max {} bytes", v.word(), self.largest().unwrap_or(0)),
            _ => v.word().to_string(),
        }
    }

    /// The verdict in plain words for the player.
    pub fn words(&self, lobby: bool) -> String {
        let rtt = self.rtt_mean().map_or(0, |r| r.round() as i64);
        let loss = self.loss_pct();
        let loss_words = if loss <= 0.0 {
            "no loss".to_string()
        } else {
            format!("{loss:.0}% loss")
        };
        match self.verdict(lobby) {
            Verdict::Good => format!("Good: {rtt} ms, {loss_words}, large packets pass."),
            Verdict::Blocked => "No answer: UDP is blocked on this network (school, office, some public Wi-Fi) or \
                                 the server is down. The game cannot run like this."
                .into(),
            Verdict::NoService => "No answer: this server may not run the connection test, or UDP is blocked.".into(),
            Verdict::Loss => format!(
                "Packet loss {loss:.0}% ({rtt} ms): matches will stutter or drop. Use a cable instead of Wi-Fi if you \
                 can, and pause downloads."
            ),
            Verdict::Far => format!("Slow: {rtt} ms. Playable, but shots will feel late; a closer server is better."),
            Verdict::Unstable => format!(
                "Unstable: {rtt} ms with {:.0} ms jitter. Usually Wi-Fi or a busy line.",
                self.jitter().unwrap_or(0.0)
            ),
            Verdict::SmallPackets => format!(
                "Large packets get lost (largest {} bytes, {rtt} ms): a router or VPN drops them, so matches may \
                 not load. Try without the VPN.",
                self.largest().unwrap_or(0)
            ),
            Verdict::Failed => format!(
                "The test could not run: {}.",
                self.error.as_deref().unwrap_or("unknown error")
            ),
        }
    }
}

/// One target's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub target: Target,
    pub measured: Measured,
}

impl Report {
    pub fn verdict(&self) -> Verdict {
        self.measured.verdict(self.target.lobby)
    }

    pub fn words(&self) -> String {
        self.measured.words(self.target.lobby)
    }

    /// The result for a diagnostic report (the address is the server's, which a report keeps).
    pub fn to_json(&self) -> Value {
        let m = &self.measured;
        json!({
            "name": self.target.name,
            "addr": self.target.addr.to_string(),
            "verdict": self.verdict().name(),
            "words": self.words(),
            "sent": m.sent,
            "received": m.got(),
            "loss_pct": m.loss_pct(),
            "rtt_mean": m.rtt_mean(),
            "rtt_min": m.rtt_min(),
            "rtt_max": m.rtt_max(),
            "jitter": m.jitter(),
            "largest": m.largest(),
            "sizes": m.sizes.iter().map(|(s, ok)| json!([s, ok])).collect::<Vec<_>>(),
            "error": m.error,
        })
    }
}

/// Measures `target` (blocks for a few seconds).
pub fn run(target: &Target) -> Report {
    Report {
        target: target.clone(),
        measured: measure(target.addr),
    }
}

fn measure(addr: SocketAddr) -> Measured {
    let mut m = Measured::default();
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse().expect("literal")
    } else {
        "0.0.0.0:0".parse().expect("literal")
    };
    let sock = match UdpSocket::bind(bind).and_then(|s| s.connect(addr).map(|_| s)) {
        Ok(s) => s,
        Err(e) => {
            m.error = Some(format!("UDP socket: {e}"));
            return m;
        }
    };
    let max = *SIZES.last().expect("sizes");
    let mut buf = vec![0u8; max + 64];
    // 1. the small probes, SMALL_GAP apart, answers read in between
    let mut sent_at: Vec<Instant> = Vec::new();
    let mut seen = vec![false; SMALL_COUNT as usize];
    let mut rtts = vec![None; SMALL_COUNT as usize];
    let mut take = |sock: &UdpSocket, until: Instant, sent_at: &[Instant]| loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = sock.set_read_timeout(Some(left.max(Duration::from_millis(1))));
        match sock.recv(&mut buf) {
            Ok(n) => {
                if let Some(seq) = reply_seq(&buf[..n], SMALL_SIZE) {
                    let i = seq as usize;
                    if i < sent_at.len() && !seen[i] {
                        seen[i] = true;
                        rtts[i] = Some(sent_at[i].elapsed().as_secs_f32() * 1000.0);
                    }
                }
            }
            Err(_) => break,
        }
    };
    for seq in 0..SMALL_COUNT as u64 {
        sent_at.push(Instant::now());
        if let Err(e) = sock.send(&request(seq, SMALL_SIZE)) {
            m.error = Some(format!("send: {e}"));
            if seq == 0 {
                return m;
            }
            break;
        }
        m.sent += 1;
        take(&sock, Instant::now() + SMALL_GAP, &sent_at);
    }
    take(&sock, Instant::now() + WAIT, &sent_at);
    m.rtts = rtts.into_iter().flatten().collect();
    if m.rtts.is_empty() {
        return m;
    }
    // 2. the size ladder; the first size that never comes back ends it
    let mut seq = 1000u64;
    for &size in SIZES {
        let mut ok = false;
        for _ in 0..TRIES {
            seq += 1;
            if sock.send(&request(seq, size)).is_err() {
                break;
            }
            let until = Instant::now() + WAIT;
            loop {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let _ = sock.set_read_timeout(Some(left.max(Duration::from_millis(1))));
                match sock.recv(&mut buf) {
                    Ok(n) if n == size && reply_seq(&buf[..n], size) == Some(seq) => {
                        ok = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if ok {
                break;
            }
        }
        m.sizes.push((size, ok));
        if !ok {
            break;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The echo of docs/DIAGNOSTICS.md §4 (the lobby's own is tested in dc-lobby-server): same length, `DCPA`, the
    /// header copied, the rest zero; `drop_over` drops datagrams above a size (a path that loses large packets).
    fn echo(drop_over: usize) -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = s.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 9000];
            let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
            while let Ok((n, from)) = s.recv_from(&mut buf) {
                if n < HEADER || &buf[..4] != MAGIC_REQUEST || n > drop_over {
                    continue;
                }
                let mut r = vec![0u8; n];
                r[..4].copy_from_slice(MAGIC_REPLY);
                r[4..HEADER].copy_from_slice(&buf[4..HEADER]);
                let _ = s.send_to(&r, from);
            }
        });
        addr
    }

    #[test]
    fn requests_and_replies_have_the_documented_shape() {
        let r = request(7, 100);
        assert_eq!(r.len(), 100);
        assert_eq!(&r[..4], b"DCPR");
        assert_eq!(r[4], 1);
        assert_eq!(&r[5..8], &[0, 0, 0]);
        assert_eq!(u64::from_le_bytes(r[8..16].try_into().unwrap()), 7);
        assert_eq!(request(1, 3).len(), HEADER);
        let mut reply = r.clone();
        reply[..4].copy_from_slice(b"DCPA");
        assert_eq!(reply_seq(&reply, 100), Some(7));
        // a reply longer than the request is not one (no amplification is taken for an answer)
        assert_eq!(reply_seq(&reply, 99), None);
        assert_eq!(reply_seq(&r, 100), None);
        assert_eq!(reply_seq(&reply[..10], 100), None);
    }

    #[test]
    fn verdicts_follow_the_documented_order() {
        let good = Measured {
            sent: 20,
            rtts: vec![30.0; 20],
            sizes: SIZES.iter().map(|s| (*s, true)).collect(),
            error: None,
        };
        assert_eq!(good.verdict(true), Verdict::Good);
        assert!(good.words(true).starts_with("Good: 30 ms, no loss"));
        let none = Measured { sent: 20, ..Measured::default() };
        assert_eq!(none.verdict(true), Verdict::Blocked);
        assert_eq!(none.verdict(false), Verdict::NoService);
        let lossy = Measured { rtts: vec![30.0; 18], ..good.clone() };
        assert_eq!(lossy.verdict(true), Verdict::Loss);
        assert!(lossy.words(true).contains("10%"));
        let far = Measured { rtts: vec![200.0; 20], ..good.clone() };
        assert_eq!(far.verdict(true), Verdict::Far);
        let shaky = Measured {
            rtts: (0..20).map(|i| if i % 2 == 0 { 20.0 } else { 90.0 }).collect(),
            ..good.clone()
        };
        assert_eq!(shaky.verdict(true), Verdict::Unstable);
        let small = Measured { sizes: vec![(512, true), (1024, true), (1200, true), (1300, false)], ..good };
        assert_eq!(small.verdict(true), Verdict::SmallPackets);
        assert!(small.words(true).contains("largest 1200 bytes"));
    }

    #[test]
    fn a_loopback_echo_measures_good_and_a_path_that_drops_large_packets_shows() {
        let r = run(&Target { name: "local".into(), addr: echo(usize::MAX), lobby: true });
        assert_eq!(r.measured.sent, SMALL_COUNT);
        assert_eq!(r.measured.got(), SMALL_COUNT, "{:?}", r.measured);
        assert_eq!(r.measured.largest(), Some(8000));
        assert_eq!(r.verdict(), Verdict::Good, "{}", r.words());
        let r = run(&Target { name: "mtu".into(), addr: echo(1300), lobby: true });
        assert_eq!(r.measured.largest(), Some(1300));
        assert_eq!(r.verdict(), Verdict::SmallPackets);
        assert_eq!(r.to_json()["verdict"], "large_packets_dropped");
    }

    #[test]
    fn nobody_answering_is_udp_blocked() {
        // a bound socket that never answers
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let r = run(&Target { name: "x".into(), addr: s.local_addr().unwrap(), lobby: true });
        assert_eq!(r.verdict(), Verdict::Blocked);
        assert!(r.measured.sizes.is_empty());
    }
}
