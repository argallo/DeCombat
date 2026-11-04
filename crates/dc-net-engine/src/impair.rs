//! Network impairment of one UDP path (the network lab, docs/NETLAB.md): a deterministic, sans-IO model of what a bad
//! real-world connection does to datagrams, per direction, plus the path-wide events of home routers, carrier NATs and
//! campus firewalls. [`crate::shim::NetShim`] puts it around a real socket; tests can drive it directly.
//!
//! Per direction ([`DirSpec`]): random loss, bursty loss (Gilbert-Elliott: a good and a bad state with their own loss
//! rates), a datagram size limit, a packets-per-second policer, a bandwidth cap with a drop-tail queue, one-way latency
//! with jitter (first in, first out unless a datagram is picked for reordering), reordering and duplication.
//! Path-wide ([`PathSpec`]): scheduled blackouts (both directions dead for a while), NAT rebinding (the client's source
//! port changes mid-session), a NAT mapping that expires when the client sends nothing for a while (the next send
//! goes out from a new port), and a middlebox that kills the flow after a stretch of server silence.
//!
//! Profiles ([`PathSpec::profile`]) name typical paths; [`PathSpec::parse`] takes `profile[,key=value...]`.
//! Our own test tooling (not 2009 behaviour, nothing on the wire changes); the random source is SplitMix64 (public
//! algorithm, Steele / Lea / Flood 2014), the burst model is the textbook two-state Gilbert-Elliott chain.
// DC-DEFAULT: NET-20 (proposed) network lab impairment model and profiles, see DC_DEFAULTS.md

use crate::sim::SimRng;
use crate::time::{later, since, Millis};

/// Gilbert-Elliott burst loss: per datagram the path moves from good to bad with `enter_permille`, from bad back to
/// good with `leave_permille`; in the bad state a datagram is lost with `loss_bad_permille` (the good state uses
/// the direction's plain `loss_permille`). Mean burst length = 1000 / `leave_permille` datagrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Burst {
    pub enter_permille: u32,
    pub leave_permille: u32,
    pub loss_bad_permille: u32,
}

/// What one direction of the path does to a datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirSpec {
    /// Random loss per thousand (the good state's loss when `burst` is set).
    pub loss_permille: u32,
    pub burst: Option<Burst>,
    /// One-way base delay.
    pub latency_ms: u32,
    /// Extra delay per datagram, uniform in `0..=jitter_ms`; delivery stays first in, first out.
    pub jitter_ms: u32,
    /// Datagrams per thousand taken out of the queue order: delivered `reorder_gap_ms` later than their turn.
    pub reorder_permille: u32,
    pub reorder_gap_ms: u32,
    pub dup_permille: u32,
    /// Datagrams longer than this many bytes are lost (a small MTU with fragments dropped).
    pub max_datagram: Option<u32>,
    /// Bandwidth cap in kbit/s: a datagram waits for the link to be free; past `queue_ms` of waiting it is dropped.
    pub rate_kbps: Option<u32>,
    pub queue_ms: u32,
    /// Packets per second policer (token bucket, bucket of a quarter second's worth, at least 4): over it, dropped.
    pub pps_cap: Option<u32>,
}

/// Scheduled blackouts: every `every_ms` (the first at a seeded point in the first period) the path drops everything
/// for a time uniform in `min_ms..=max_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Blackouts {
    pub every_ms: u32,
    pub min_ms: u32,
    pub max_ms: u32,
}

/// A whole path: client to server (`up`), server to client (`down`) and the path-wide events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PathSpec {
    /// Profile name the spec started from (`custom` for none), for reports.
    pub name: String,
    pub up: DirSpec,
    pub down: DirSpec,
    pub blackouts: Option<Blackouts>,
    /// NAT rebinding: the client's source port changes every this many ms (a carrier NAT reassigning the mapping).
    pub rebind_every_ms: Option<u32>,
    /// The NAT mapping expires when the client sent nothing for this long: datagrams from the server are dropped
    /// until the client sends again, and that send goes out from a new port.
    pub nat_idle_ms: Option<u32>,
    /// Middlebox: after this long without a datagram from the server, the flow is killed: both directions drop
    /// everything for `kill_hold_ms` (0 = for the rest of the session), then a new flow starts from a new port.
    pub silence_kill_ms: Option<u32>,
    pub kill_hold_ms: u32,
    pub seed: u64,
}

/// The named profiles, in the order the docs list them.
pub const PROFILES: [&str; 9] = [
    "good",
    "wifi",
    "mobile_hotspot",
    "australia",
    "campus_firewall",
    "cgnat_rebind",
    "lossy_burst",
    "blackout",
    "clean",
];

impl PathSpec {
    /// A named profile (one-way numbers; a round trip is about twice the latency). `None` for an unknown name.
    // DC-DEFAULT: NET-21 (proposed) the profile numbers, see DC_DEFAULTS.md
    pub fn profile(name: &str) -> Option<PathSpec> {
        let both = |d: DirSpec| (d, d);
        let base = |latency_ms, jitter_ms| DirSpec {
            latency_ms,
            jitter_ms,
            queue_ms: 200,
            reorder_gap_ms: 30,
            ..DirSpec::default()
        };
        let mut p = PathSpec {
            name: name.to_string(),
            kill_hold_ms: 0,
            ..PathSpec::default()
        };
        match name {
            // no impairment at all (the shim only measures)
            "clean" => {}
            // a wired home line to a nearby server
            "good" => (p.up, p.down) = both(base(10, 2)),
            // a busy home Wi-Fi: jitter spikes, short loss bursts (retransmissions given up), a little reordering
            "wifi" => {
                let mut d = base(8, 25);
                d.loss_permille = 5;
                d.burst = Some(Burst {
                    enter_permille: 10,
                    leave_permille: 250,
                    loss_bad_permille: 500,
                });
                d.reorder_permille = 2;
                d.dup_permille = 1;
                (p.up, p.down) = both(d);
            }
            // a phone's 4G hotspot: higher latency and jitter, bursts, a thin uplink with a deep queue, a short
            // outage at each cell handover
            "mobile_hotspot" => {
                let mut d = base(35, 30);
                d.loss_permille = 10;
                d.burst = Some(Burst {
                    enter_permille: 10,
                    leave_permille: 150,
                    loss_bad_permille: 700,
                });
                d.reorder_permille = 5;
                let mut up = d;
                up.rate_kbps = Some(1500);
                up.queue_ms = 400;
                let mut down = d;
                down.rate_kbps = Some(6000);
                down.queue_ms = 250;
                p.up = up;
                p.down = down;
                p.blackouts = Some(Blackouts {
                    every_ms: 90_000,
                    min_ms: 1000,
                    max_ms: 3000,
                });
            }
            // a player in Australia on a European / US server: about 300 ms round trip
            "australia" => {
                let mut d = base(150, 8);
                d.loss_permille = 3;
                (p.up, p.down) = both(d);
            }
            // a campus network: a per-flow packet policer and bandwidth cap with a short queue, a firewall that tears
            // a UDP flow down after 4 s without a datagram from the server (and lets a new one start 30 s later),
            // datagrams over 1200 bytes lost (a tunnel)
            "campus_firewall" => {
                let mut d = base(12, 4);
                d.pps_cap = Some(100);
                d.rate_kbps = Some(2000);
                d.queue_ms = 50;
                d.max_datagram = Some(1200);
                (p.up, p.down) = both(d);
                p.silence_kill_ms = Some(4000);
                p.kill_hold_ms = 30_000;
                p.nat_idle_ms = Some(30_000);
            }
            // a carrier-grade NAT that moves the client to a new source port every 60 s and expires an idle
            // mapping after 20 s
            "cgnat_rebind" => {
                (p.up, p.down) = both(base(25, 5));
                p.rebind_every_ms = Some(60_000);
                p.nat_idle_ms = Some(20_000);
            }
            // long loss bursts (a saturated or flaky line)
            "lossy_burst" => {
                let mut d = base(30, 10);
                d.loss_permille = 10;
                d.burst = Some(Burst {
                    enter_permille: 20,
                    leave_permille: 100,
                    loss_bad_permille: 800,
                });
                (p.up, p.down) = both(d);
            }
            // a line that goes dead for 2 to 10 s every 45 s
            "blackout" => {
                (p.up, p.down) = both(base(20, 5));
                p.blackouts = Some(Blackouts {
                    every_ms: 45_000,
                    min_ms: 2000,
                    max_ms: 10_000,
                });
            }
            _ => return None,
        }
        Some(p)
    }

    /// `profile[,key=value...]` or only `key=value,...` (starting from a clean path). Keys (both directions, or one
    /// with an `up.` / `down.` prefix): `loss` (permille, or percent with `%`), `burst=enter/leave/bad` (permille),
    /// `latency`, `jitter`, `reorder`, `reorder_gap`, `dup`, `mtu` (bytes), `kbps`, `queue` (ms), `pps`; path-wide:
    /// `blackout=every_s:min_s-max_s`, `rebind=s`, `nat_idle=s`, `silence_kill=s[:hold_s]`, `seed=n`.
    pub fn parse(text: &str) -> Result<PathSpec, String> {
        let mut parts = text.split(',').map(str::trim).filter(|s| !s.is_empty());
        let mut spec = PathSpec::profile("clean").expect("clean profile");
        spec.name = "custom".into();
        let mut first = parts.next();
        if let Some(f) = first {
            if !f.contains('=') {
                spec = PathSpec::profile(f).ok_or_else(|| {
                    format!(
                        "unknown network profile {f:?} (known: {})",
                        PROFILES.join(", ")
                    )
                })?;
                first = None;
            }
        }
        let mut changed = false;
        for kv in first.into_iter().chain(parts) {
            spec.set(kv)?;
            changed = true;
        }
        if changed && spec.name != "custom" {
            spec.name.push('+');
        }
        Ok(spec)
    }

    fn set(&mut self, kv: &str) -> Result<(), String> {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("{kv:?}: expected key=value"))?;
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        let bad = || format!("{k}: bad value {v:?}");
        let n = |s: &str| s.trim().parse::<f64>().map_err(|_| bad());
        let ms_of_s = |s: &str| n(s).map(|x| (x.max(0.0) * 1000.0).round() as u32);
        match k.as_str() {
            "seed" => self.seed = v.parse().map_err(|_| bad())?,
            "rebind" => self.rebind_every_ms = Some(ms_of_s(v)?).filter(|&x| x > 0),
            "nat_idle" => self.nat_idle_ms = Some(ms_of_s(v)?).filter(|&x| x > 0),
            "silence_kill" => {
                let (a, b) = v.split_once(':').unwrap_or((v, "0"));
                self.silence_kill_ms = Some(ms_of_s(a)?).filter(|&x| x > 0);
                self.kill_hold_ms = ms_of_s(b)?;
            }
            "blackout" => {
                if v == "off" || v == "0" {
                    self.blackouts = None;
                } else {
                    let (every, dur) = v.split_once(':').ok_or_else(bad)?;
                    let (lo, hi) = dur.split_once('-').unwrap_or((dur, dur));
                    let b = Blackouts {
                        every_ms: ms_of_s(every)?,
                        min_ms: ms_of_s(lo)?,
                        max_ms: ms_of_s(hi)?,
                    };
                    if b.every_ms == 0 || b.max_ms < b.min_ms || b.max_ms >= b.every_ms {
                        return Err(bad());
                    }
                    self.blackouts = Some(b);
                }
            }
            _ => {
                let (dirs, key): (Vec<&mut DirSpec>, &str) =
                    if let Some(key) = k.strip_prefix("up.") {
                        (vec![&mut self.up], key)
                    } else if let Some(key) = k.strip_prefix("down.") {
                        (vec![&mut self.down], key)
                    } else {
                        (vec![&mut self.up, &mut self.down], k.as_str())
                    };
                let permille = |s: &str| -> Result<u32, String> {
                    match s.strip_suffix('%') {
                        Some(p) => Ok((n(p)? * 10.0).round().clamp(0.0, 1000.0) as u32),
                        None => Ok(n(s)?.round().clamp(0.0, 1000.0) as u32),
                    }
                };
                let whole = |s: &str| -> Result<u32, String> {
                    Ok(n(s)?.round().clamp(0.0, u32::MAX as f64) as u32)
                };
                let opt = |s: &str| -> Result<Option<u32>, String> {
                    if s == "off" {
                        Ok(None)
                    } else {
                        Ok(Some(whole(s)?).filter(|&x| x > 0))
                    }
                };
                for d in dirs {
                    match key {
                        "loss" => d.loss_permille = permille(v)?,
                        "burst" => {
                            if v == "off" {
                                d.burst = None;
                            } else {
                                let p: Vec<&str> = v.split('/').collect();
                                if p.len() != 3 {
                                    return Err(bad());
                                }
                                d.burst = Some(Burst {
                                    enter_permille: permille(p[0])?,
                                    leave_permille: permille(p[1])?.max(1),
                                    loss_bad_permille: permille(p[2])?,
                                });
                            }
                        }
                        "latency" => d.latency_ms = whole(v)?,
                        "jitter" => d.jitter_ms = whole(v)?,
                        "reorder" => d.reorder_permille = permille(v)?,
                        "reorder_gap" => d.reorder_gap_ms = whole(v)?,
                        "dup" => d.dup_permille = permille(v)?,
                        "mtu" => d.max_datagram = opt(v)?,
                        "kbps" => d.rate_kbps = opt(v)?,
                        "queue" => d.queue_ms = whole(v)?,
                        "pps" => d.pps_cap = opt(v)?,
                        _ => return Err(format!("unknown network key {k:?}")),
                    }
                }
            }
        }
        Ok(())
    }

    /// True when the spec changes nothing (the shim then passes datagrams straight through).
    pub fn is_clean(&self) -> bool {
        let p = PathSpec::profile("clean").expect("clean profile");
        self.up == p.up
            && self.down == p.down
            && self.blackouts.is_none()
            && self.rebind_every_ms.is_none()
            && self.nat_idle_ms.is_none()
            && self.silence_kill_ms.is_none()
    }
}

/// What happened to the datagrams of one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirStats {
    pub offered: u64,
    pub delivered: u64,
    pub lost_random: u64,
    pub lost_burst: u64,
    pub oversize: u64,
    pub policed: u64,
    pub queue_drops: u64,
    pub blackout_drops: u64,
    /// Dropped by the NAT / middlebox (an expired mapping, a killed flow, an old port after a rebind).
    pub flow_drops: u64,
    pub duplicated: u64,
    pub reordered: u64,
    /// Largest wait in the bandwidth queue.
    pub max_queue_ms: u32,
}

impl DirStats {
    /// Datagrams the path dropped, all causes.
    pub fn dropped(&self) -> u64 {
        self.lost_random
            + self.lost_burst
            + self.oversize
            + self.policed
            + self.queue_drops
            + self.blackout_drops
            + self.flow_drops
    }

    /// `(name, value)` pairs for a report.
    pub fn fields(&self) -> [(&'static str, u64); 12] {
        [
            ("offered", self.offered),
            ("delivered", self.delivered),
            ("lost_random", self.lost_random),
            ("lost_burst", self.lost_burst),
            ("oversize", self.oversize),
            ("policed", self.policed),
            ("queue_drops", self.queue_drops),
            ("blackout_drops", self.blackout_drops),
            ("flow_drops", self.flow_drops),
            ("duplicated", self.duplicated),
            ("reordered", self.reordered),
            ("max_queue_ms", self.max_queue_ms as u64),
        ]
    }
}

/// Path-wide counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PathStats {
    pub blackouts: u32,
    pub blackout_ms: u64,
    /// Source port changes (scheduled rebinds, expired mappings, restarted flows).
    pub rebinds: u32,
    pub nat_expiries: u32,
    pub flow_kills: u32,
}

#[derive(Debug, Clone, Default)]
struct DirState {
    bad: bool,
    link_free_at: Option<Millis>,
    fifo_last: Option<Millis>,
    tokens_milli: u64,
    tokens_at: Option<Millis>,
    stats: DirStats,
}

impl DirState {
    /// Admits one datagram of `len` bytes at `now`: the delivery times of its copies (empty = lost).
    fn admit(&mut self, d: &DirSpec, rng: &mut SimRng, now: Millis, len: usize) -> Vec<Millis> {
        self.stats.offered += 1;
        if d.max_datagram.is_some_and(|m| len as u64 > m as u64) {
            self.stats.oversize += 1;
            return Vec::new();
        }
        if let Some(pps) = d.pps_cap {
            // token bucket in thousandths of a token: `pps` tokens a second, a quarter second's worth deep
            let cap = (pps as u64 / 4).max(4) * 1000;
            let at = *self.tokens_at.get_or_insert_with(|| {
                self.tokens_milli = cap;
                now
            });
            self.tokens_milli = (self.tokens_milli + since(now, at) as u64 * pps as u64).min(cap);
            self.tokens_at = Some(now);
            if self.tokens_milli < 1000 {
                self.stats.policed += 1;
                return Vec::new();
            }
            self.tokens_milli -= 1000;
        }
        // the burst chain moves once per datagram; the state decides this datagram's loss rate
        let lost = match d.burst {
            Some(b) => {
                self.bad = if self.bad {
                    !rng.chance(b.leave_permille)
                } else {
                    rng.chance(b.enter_permille)
                };
                if self.bad {
                    rng.chance(b.loss_bad_permille).then_some(true)
                } else {
                    rng.chance(d.loss_permille).then_some(false)
                }
            }
            None => rng.chance(d.loss_permille).then_some(false),
        };
        match lost {
            Some(true) => {
                self.stats.lost_burst += 1;
                return Vec::new();
            }
            Some(false) => {
                self.stats.lost_random += 1;
                return Vec::new();
            }
            None => {}
        }
        let mut depart = now;
        if let Some(kbps) = d.rate_kbps {
            let tx_ms = ((len as u64 * 8).div_ceil(kbps.max(1) as u64)) as u32;
            let free = self.link_free_at.filter(|&f| later(f, now)).unwrap_or(now);
            let wait = since(free, now);
            if wait > d.queue_ms {
                self.stats.queue_drops += 1;
                return Vec::new();
            }
            self.stats.max_queue_ms = self.stats.max_queue_ms.max(wait);
            depart = free.wrapping_add(tx_ms);
            self.link_free_at = Some(depart);
        }
        let mut at = depart
            .wrapping_add(d.latency_ms)
            .wrapping_add(rng.range(0, d.jitter_ms));
        if d.reorder_permille > 0 && rng.chance(d.reorder_permille) {
            self.stats.reordered += 1;
            at = at.wrapping_add(d.reorder_gap_ms.max(1));
        } else {
            if let Some(last) = self.fifo_last {
                if later(last, at) {
                    at = last;
                }
            }
            self.fifo_last = Some(at);
        }
        let mut out = vec![at];
        if rng.chance(d.dup_permille) {
            self.stats.duplicated += 1;
            out.push(at.wrapping_add(rng.range(0, 5)));
        }
        self.stats.delivered += out.len() as u64;
        out
    }
}

/// The impairment state of one path. Feed it every datagram with the time; it says when (if ever) each arrives and
/// when the client's socket must move to a new port.
#[derive(Debug, Clone)]
pub struct Impairer {
    spec: PathSpec,
    rng: SimRng,
    up: DirState,
    down: DirState,
    start: Option<Millis>,
    next_blackout: Option<Millis>,
    blackout_until: Option<Millis>,
    next_rebind: Option<Millis>,
    last_out: Option<Millis>,
    last_in: Option<Millis>,
    nat_expired: bool,
    killed_until: Option<Option<Millis>>,
    rebind_due: bool,
    stats: PathStats,
}

impl Impairer {
    pub fn new(spec: PathSpec) -> Self {
        let rng = SimRng::new(spec.seed ^ 0x6e65_746c_6162);
        Impairer {
            spec,
            rng,
            up: DirState::default(),
            down: DirState::default(),
            start: None,
            next_blackout: None,
            blackout_until: None,
            next_rebind: None,
            last_out: None,
            last_in: None,
            nat_expired: false,
            killed_until: None,
            rebind_due: false,
            stats: PathStats::default(),
        }
    }

    pub fn spec(&self) -> &PathSpec {
        &self.spec
    }

    pub fn up_stats(&self) -> DirStats {
        self.up.stats
    }

    pub fn down_stats(&self) -> DirStats {
        self.down.stats
    }

    pub fn path_stats(&self) -> PathStats {
        self.stats
    }

    /// Runs the path's clock: blackouts, rebinds, mapping expiry, the silence kill. Called by `outbound` / `inbound`;
    /// a caller with nothing to send may call it to see `take_rebind`.
    pub fn tick(&mut self, now: Millis) {
        let start = *self.start.get_or_insert(now);
        if let Some(b) = self.spec.blackouts {
            if self.next_blackout.is_none() {
                // the first blackout at a seeded point of the second half of the first period
                let first = b.every_ms / 2 + self.rng.range(0, b.every_ms / 2);
                self.next_blackout = Some(start.wrapping_add(first));
            }
            if let Some(until) = self.blackout_until {
                if !later(until, now) {
                    self.blackout_until = None;
                }
            }
            if let Some(nb) = self.next_blackout {
                if !later(nb, now) {
                    let dur = self.rng.range(b.min_ms, b.max_ms);
                    self.blackout_until = Some(nb.wrapping_add(dur));
                    self.next_blackout = Some(nb.wrapping_add(b.every_ms));
                    self.stats.blackouts += 1;
                    self.stats.blackout_ms += dur as u64;
                }
            }
        }
        if let Some(every) = self.spec.rebind_every_ms {
            let nr = *self.next_rebind.get_or_insert(start.wrapping_add(every));
            if !later(nr, now) {
                self.next_rebind = Some(nr.wrapping_add(every));
                self.rebind_due = true;
            }
        }
        if let (Some(idle), Some(last)) = (self.spec.nat_idle_ms, self.last_out) {
            if !self.nat_expired && since(now, last) > idle {
                self.nat_expired = true;
                self.stats.nat_expiries += 1;
            }
        }
        if let Some(silence) = self.spec.silence_kill_ms {
            match self.killed_until {
                None => {
                    // silence counts from the last datagram from the server, or from the first send
                    let ref_t = self.last_in.or(self.last_out);
                    if ref_t.is_some_and(|t| since(now, t) > silence) {
                        self.stats.flow_kills += 1;
                        self.killed_until = Some(
                            (self.spec.kill_hold_ms > 0)
                                .then(|| now.wrapping_add(self.spec.kill_hold_ms)),
                        );
                    }
                }
                Some(Some(until)) if !later(until, now) => {
                    // the firewall lets a new flow start: a new mapping, a new port
                    self.killed_until = None;
                    self.last_in = None;
                    self.last_out = Some(now);
                    self.rebind_due = true;
                }
                Some(_) => {}
            }
        }
    }

    fn blacked_out(&self, now: Millis) -> bool {
        self.blackout_until.is_some_and(|u| later(u, now))
    }

    fn killed(&self) -> bool {
        self.killed_until.is_some()
    }

    /// The client sends `len` bytes at `now`: the times the server gets the copies (empty = lost).
    pub fn outbound(&mut self, now: Millis, len: usize) -> Vec<Millis> {
        self.tick(now);
        if self.killed() {
            self.up.stats.offered += 1;
            self.up.stats.flow_drops += 1;
            return Vec::new();
        }
        if self.nat_expired {
            // the send opens a new mapping from a new port
            self.nat_expired = false;
            self.rebind_due = true;
        }
        self.last_out = Some(now);
        if self.blacked_out(now) {
            self.up.stats.offered += 1;
            self.up.stats.blackout_drops += 1;
            return Vec::new();
        }
        let d = self.spec.up;
        self.up.admit(&d, &mut self.rng, now, len)
    }

    /// A datagram of `len` bytes from the server reaches the path at `now`: the times the client gets the copies.
    pub fn inbound(&mut self, now: Millis, len: usize) -> Vec<Millis> {
        self.tick(now);
        if self.killed() || self.nat_expired {
            self.down.stats.offered += 1;
            self.down.stats.flow_drops += 1;
            return Vec::new();
        }
        self.last_in = Some(now);
        if self.blacked_out(now) {
            self.down.stats.offered += 1;
            self.down.stats.blackout_drops += 1;
            return Vec::new();
        }
        let d = self.spec.down;
        self.down.admit(&d, &mut self.rng, now, len)
    }

    /// Counts a datagram that reached the client's old port after a rebind (lost: the mapping is gone).
    pub fn note_stale_port(&mut self) {
        self.down.stats.offered += 1;
        self.down.stats.flow_drops += 1;
    }

    /// True once when the client's socket must move to a new source port now.
    pub fn take_rebind(&mut self) -> bool {
        let due = std::mem::take(&mut self.rebind_due);
        if due {
            self.stats.rebinds += 1;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spec: &PathSpec, n: u32, gap_ms: u32, len: usize) -> (Impairer, Vec<Vec<Millis>>) {
        let mut imp = Impairer::new(spec.clone());
        let mut out = Vec::new();
        for i in 0..n {
            out.push(imp.outbound(i * gap_ms, len));
        }
        (imp, out)
    }

    #[test]
    fn every_profile_parses_and_clean_is_clean() {
        for p in PROFILES {
            let s = PathSpec::parse(p).unwrap();
            assert_eq!(s.name, p);
            assert_eq!(s.is_clean(), p == "clean", "{p}");
        }
        assert!(PathSpec::parse("nope").is_err());
        assert!(PathSpec::parse("wifi,frobnicate=3").is_err());
        assert!(PathSpec::parse("blackout=10:2-12").is_err());
    }

    #[test]
    fn overrides_apply_per_direction_and_in_percent() {
        let s = PathSpec::parse("australia,down.loss=2.5%,up.latency=40,mtu=548,blackout=30:2-10,rebind=45,silence_kill=5:20,seed=7").unwrap();
        assert_eq!(s.name, "australia+");
        assert_eq!(s.down.loss_permille, 25);
        assert_eq!(s.up.loss_permille, 3);
        assert_eq!(s.up.latency_ms, 40);
        assert_eq!(s.down.latency_ms, 150);
        assert_eq!(s.up.max_datagram, Some(548));
        assert_eq!(
            s.blackouts,
            Some(Blackouts {
                every_ms: 30_000,
                min_ms: 2000,
                max_ms: 10_000
            })
        );
        assert_eq!(s.rebind_every_ms, Some(45_000));
        assert_eq!((s.silence_kill_ms, s.kill_hold_ms), (Some(5000), 20_000));
        assert_eq!(s.seed, 7);
        let c = PathSpec::parse("loss=10,burst=20/100/800").unwrap();
        assert_eq!(c.name, "custom");
        assert_eq!(c.up.burst.unwrap().loss_bad_permille, 800);
    }

    #[test]
    fn latency_jitter_keeps_order_and_is_deterministic() {
        let s = PathSpec::parse("latency=50,jitter=30").unwrap();
        let (_, a) = run(&s, 500, 5, 100);
        let (_, b) = run(&s, 500, 5, 100);
        assert_eq!(a, b);
        let times: Vec<Millis> = a.iter().map(|v| v[0]).collect();
        assert!(
            times.windows(2).all(|w| w[1] >= w[0]),
            "first in, first out"
        );
        for (i, t) in times.iter().enumerate() {
            let sent = i as u32 * 5;
            assert!(*t >= sent + 50, "never earlier than the latency");
        }
    }

    #[test]
    fn reordering_and_duplication_happen_at_their_rates() {
        let s = PathSpec::parse("latency=20,reorder=100,reorder_gap=40,dup=50").unwrap();
        let (imp, a) = run(&s, 2000, 5, 100);
        let st = imp.up_stats();
        assert!((150..260).contains(&st.reordered), "{st:?}");
        assert!((60..150).contains(&st.duplicated), "{st:?}");
        let firsts: Vec<Millis> = a.iter().map(|v| v[0]).collect();
        assert!(firsts.windows(2).any(|w| w[1] < w[0]), "some overtaking");
    }

    #[test]
    fn burst_loss_comes_in_runs() {
        let s = PathSpec::parse("burst=20/100/1000").unwrap();
        let (imp, a) = run(&s, 20_000, 1, 100);
        let st = imp.up_stats();
        // stationary bad share = enter / (enter + leave) = 1/6; mean run length 10
        let lost = st.lost_burst as f64 / 20_000.0;
        assert!((0.10..0.24).contains(&lost), "{lost}");
        let mut runs = Vec::new();
        let mut cur = 0;
        for v in &a {
            if v.is_empty() {
                cur += 1;
            } else if cur > 0 {
                runs.push(cur);
                cur = 0;
            }
        }
        let mean = runs.iter().sum::<u32>() as f64 / runs.len() as f64;
        assert!((6.0..15.0).contains(&mean), "mean run {mean}");
    }

    #[test]
    fn size_limit_policer_and_bandwidth_queue() {
        let s = PathSpec::parse("mtu=548").unwrap();
        let mut imp = Impairer::new(s);
        assert!(imp.outbound(0, 549).is_empty());
        assert_eq!(imp.outbound(1, 548).len(), 1);
        assert_eq!(imp.up_stats().oversize, 1);

        // 100 pps for 2 s of 1 ms sends: about 200 + the 25-token bucket pass
        let s = PathSpec::parse("pps=100").unwrap();
        let (imp, _) = run(&s, 2000, 1, 100);
        let st = imp.up_stats();
        assert!((215..235).contains(&st.delivered), "{st:?}");

        // 80 kbit/s, 1000-byte datagrams (100 ms each) every 50 ms: the queue grows to 200 ms, then drops
        let s = PathSpec::parse("kbps=80,queue=200").unwrap();
        let (imp, a) = run(&s, 40, 50, 1000);
        let st = imp.up_stats();
        assert!(st.queue_drops > 10, "{st:?}");
        assert!(st.max_queue_ms <= 200);
        let firsts: Vec<Millis> = a.iter().filter(|v| !v.is_empty()).map(|v| v[0]).collect();
        assert!(firsts.windows(2).all(|w| w[1] >= w[0] + 100), "serialised");
    }

    #[test]
    fn blackouts_drop_both_directions_for_their_time() {
        let s = PathSpec::parse("blackout=10:2-2").unwrap();
        let mut imp = Impairer::new(s);
        let mut dead = 0;
        for t in 0..40_000u32 {
            let a = imp.outbound(t, 10).is_empty();
            let b = imp.inbound(t, 10).is_empty();
            assert_eq!(a, b);
            dead += a as u32;
        }
        let ps = imp.path_stats();
        assert!((3..=4).contains(&ps.blackouts), "{ps:?}");
        assert!((6000..=8000).contains(&dead), "{dead}");
    }

    #[test]
    fn rebinds_on_schedule_and_after_an_idle_mapping() {
        let s = PathSpec::parse("rebind=10").unwrap();
        let mut imp = Impairer::new(s);
        let mut rebinds = 0;
        for t in (0..35_000u32).step_by(10) {
            imp.outbound(t, 10);
            rebinds += imp.take_rebind() as u32;
        }
        assert_eq!(rebinds, 3);

        let s = PathSpec::parse("nat_idle=5").unwrap();
        let mut imp = Impairer::new(s);
        assert_eq!(imp.outbound(0, 10).len(), 1);
        assert_eq!(imp.inbound(4000, 10).len(), 1);
        // 6 s without a send: the mapping is gone, the server's datagram is lost
        assert!(imp.inbound(6000, 10).is_empty());
        assert!(!imp.take_rebind());
        // the next send opens a new mapping from a new port
        assert_eq!(imp.outbound(6100, 10).len(), 1);
        assert!(imp.take_rebind());
        assert_eq!(imp.inbound(6200, 10).len(), 1);
    }

    #[test]
    fn silence_kill_holds_the_flow_down_then_starts_a_new_one() {
        let s = PathSpec::parse("silence_kill=4:10").unwrap();
        let mut imp = Impairer::new(s);
        assert_eq!(imp.outbound(0, 10).len(), 1);
        assert_eq!(imp.inbound(100, 10).len(), 1);
        // the client keeps sending, the server is silent
        for t in (200..4100).step_by(100) {
            assert_eq!(imp.outbound(t, 10).len(), 1, "{t}");
        }
        assert!(imp.outbound(4200, 10).is_empty(), "killed");
        assert!(imp.inbound(4300, 10).is_empty());
        assert!(imp.outbound(14_000, 10).is_empty(), "still held");
        assert!(!imp.take_rebind());
        assert_eq!(imp.outbound(14_300, 10).len(), 1, "a new flow");
        assert!(imp.take_rebind());
        assert_eq!(imp.path_stats().flow_kills, 1);
        // a permanent kill
        let s = PathSpec::parse("silence_kill=1").unwrap();
        let mut imp = Impairer::new(s);
        imp.outbound(0, 10);
        assert!(imp.outbound(1500, 10).is_empty());
        assert!(imp.outbound(600_000, 10).is_empty());
    }
}
