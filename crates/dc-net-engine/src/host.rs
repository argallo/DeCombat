//! The host side of the transport: out-of-band datagrams (info probe, join, echo) and the link table keyed by the
//! peer address. Generic over the address type so tests can use plain integers; deterministic (a `BTreeMap` keeps
//! the per-link order stable).

use std::collections::{BTreeMap, VecDeque};

use crate::link::{Bye, CloseCause, Link, LinkConfig, LinkEvent};
use crate::time::{since, Millis};
use dc_wire::udp::{
    is_oob_datagram, BitWriter, InfoReply, OobDatagram, STALE_LINK_MS,
    TODO_GAME_APP_ID_GUID_LAYOUT, TODO_HOST_RATE_BPS,
};

/// The application id as the plain big-endian byte order of the same GUID value (Data1..Data3 not swapped).
///
/// The value is PROVEN (PROTOCOL §4.4); which in-memory layout the client copies is UNPROVEN, so by default the
/// host accepts both candidate layouts and reports which one matched ([`HostEvent::LinkOpened`]), which closes
/// validation V3 from the first real join.
// SPEC: UDP_TRANSPORT_2009.md §5.1 (layout UNPROVEN, V3)
// DC-DEFAULT: DSV-5 (staged proposal) accept both candidate layouts until V3
pub const APP_ID_BIG_ENDIAN_CANDIDATE: [u8; 16] = [
    0x62, 0x58, 0x33, 0xCB, 0x3B, 0xC8, 0x44, 0xA1, 0x91, 0xD8, 0xEC, 0xB5, 0xCE, 0x89, 0x6B, 0x72,
];

/// The rate the host raises a joining client's requested rate to (`max(requested, local_default)`). The 2009
/// host's local default is not visible; DeCombat uses the host rate announced in `S2C_VERSION`.
// SPEC: UDP_TRANSPORT_2009.md §5.3 step 3 (local default UNPROVEN), §14 (host rate PE-DATA 5120000)
// DC-DEFAULT: DSV-6 (staged proposal)
pub const TODO_HOST_LOCAL_DEFAULT_RATE_BPS: u32 = TODO_HOST_RATE_BPS;

/// Host parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// Accepted application ids, in preference order.
    pub app_ids: Vec<[u8; 16]>,
    /// `session_name` of the info reply.
    pub session_name: Vec<u8>,
    /// `host_flag` of the info reply (meaning UNPROVEN, spec §5.4).
    pub host_flag: bool,
    /// `host_byte` of the info reply (meaning UNPROVEN, spec §5.4).
    pub host_byte: u8,
    /// See [`TODO_HOST_LOCAL_DEFAULT_RATE_BPS`].
    pub local_rate_bps: u32,
    /// DeCombat limit; the 2009 host has none. When reached the join is refused with `accepted = 0, same_app = 1`.
    // SPEC: UDP_TRANSPORT_2009.md §5.3 (a Rust host may add a limit and must then send same_app = 1)
    pub max_links: Option<usize>,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            app_ids: vec![TODO_GAME_APP_ID_GUID_LAYOUT, APP_ID_BIG_ENDIAN_CANDIDATE],
            session_name: b"DeCombat".to_vec(),
            host_flag: false,
            host_byte: 0,
            local_rate_bps: TODO_HOST_LOCAL_DEFAULT_RATE_BPS,
            max_links: None,
        }
    }
}

/// Output of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent<A> {
    /// A datagram to send (out-of-band reply or link datagram).
    Transmit { to: A, bytes: Vec<u8> },
    /// A new link: the join was accepted. `app_id_index` is the index in [`HostConfig::app_ids`] that matched.
    LinkOpened {
        peer: A,
        rate_bps: u32,
        app_id_index: usize,
    },
    /// A session message from a peer.
    Message {
        peer: A,
        reliable: bool,
        bits: BitWriter,
    },
    /// A link is closed (by the peer, idle timeout, a local disconnect or a stale re-join).
    LinkClosed {
        peer: A,
        code: u32,
        cause: CloseCause,
    },
}

/// Counters for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostStats {
    pub info_asks: u64,
    pub join_asks: u64,
    pub joins_accepted: u64,
    pub joins_wrong_app: u64,
    pub joins_refused: u64,
    pub joins_repeated: u64,
    pub stale_rejoins: u64,
    pub echo_asks: u64,
    pub oob_ignored: u64,
    pub unknown_peer: u64,
    /// Links moved to a new address of the same host (NAT rebinding, [`Host::migrate`]).
    pub migrations: u64,
    /// Datagrams from a new address that more than one silent link of that host could claim (not moved).
    pub migrations_ambiguous: u64,
    /// Datagrams from a new address of a host with a silent link that did not prove they belong to it (no move
    /// secret, a wrong one, or sequence numbers that do not fit): not moved (NET-24).
    pub migrations_refused: u64,
    /// Proven moves refused because the link already moved [`crate::link::MOVE_LIMIT`] times within
    /// [`crate::link::MOVE_WINDOW_MS`] (NET-25).
    pub migrations_limited: u64,
}

/// The host: one per listening socket.
#[derive(Debug)]
pub struct Host<A: Ord + Clone> {
    cfg: HostConfig,
    links: BTreeMap<A, Link>,
    events: VecDeque<HostEvent<A>>,
    stats: HostStats,
}

impl<A: Ord + Clone> Host<A> {
    pub fn new(cfg: HostConfig) -> Self {
        Host {
            cfg,
            links: BTreeMap::new(),
            events: VecDeque::new(),
            stats: HostStats::default(),
        }
    }

    pub fn config(&self) -> &HostConfig {
        &self.cfg
    }

    pub fn stats(&self) -> HostStats {
        self.stats
    }

    /// Number of open links (the info reply's `link_count`).
    // SPEC: UDP_TRANSPORT_2009.md §5.4 (driver+0x1b0 = number of live links)
    pub fn open_links(&self) -> usize {
        self.links.values().filter(|l| l.is_open()).count()
    }

    /// Links still held, open or closing (bye copies pending).
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    pub fn link(&self, peer: &A) -> Option<&Link> {
        self.links.get(peer)
    }

    pub fn link_mut(&mut self, peer: &A) -> Option<&mut Link> {
        self.links.get_mut(peer)
    }

    /// Peers with an open link.
    pub fn peers(&self) -> Vec<A> {
        self.links
            .iter()
            .filter(|(_, l)| l.is_open())
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// Queues a reliable message to a peer. False when the peer has no open link.
    pub fn send_reliable(&mut self, peer: &A, msg: BitWriter) -> bool {
        self.links
            .get_mut(peer)
            .is_some_and(|l| l.send_reliable(msg))
    }

    /// Queues an unreliable message to a peer. False when the peer has no open link.
    pub fn send_unreliable(&mut self, now: Millis, peer: &A, msg: BitWriter) -> bool {
        self.links
            .get_mut(peer)
            .is_some_and(|l| l.send_unreliable(now, msg))
    }

    /// Drops a peer's link, optionally with a bye (4 copies, 100 ms apart).
    // SPEC: UDP_TRANSPORT_2009.md §13 (local drop engine+0xb4ff0)
    pub fn disconnect(&mut self, now: Millis, peer: &A, bye: Bye) {
        if let Some(l) = self.links.get_mut(peer) {
            l.close(now, bye);
        }
    }

    /// Handles one received datagram: out-of-band datagrams are answered here; link datagrams go to the link of
    /// their source address; a link datagram from an unknown address is dropped.
    // SPEC: UDP_TRANSPORT_2009.md §4 (classification, engine+0xb5d70)
    pub fn handle_datagram(&mut self, now: Millis, from: A, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if is_oob_datagram(bytes) {
            self.handle_oob(now, from, bytes);
            return;
        }
        match self.links.get_mut(&from) {
            Some(l) => l.handle_datagram(now, bytes),
            None => self.stats.unknown_peer += 1,
        }
    }

    /// NAT rebinding: a link datagram from `from`, an address without a link, that proves it belongs to an open link
    /// of the same host (`same_host`, the caller's rule: the same IP for socket addresses) which has been silent for
    /// at least `min_silence_ms`: the link moves to `from` and the old address is returned. The proof is the link's
    /// move secret as the datagram's trailer ([`Link::claims`], NET-24); a link without a secret (a 2009 client)
    /// never moves. A link moves at most [`crate::link::MOVE_LIMIT`] times a minute; after a move its unacknowledged
    /// reliable blocks go to the new address at once and no transport bye closes it for a moment
    /// ([`Link::on_moved`]). The 2009 transport keys a link by the peer's address only, so a home router or a carrier
    /// NAT that gives the client a new source port mid-match would otherwise leave both sides talking past each other
    /// until the silence drop.
    // DC-DEFAULT: NET-23 (proposed) address move of a link after a NAT rebinding, see DC_DEFAULTS.md
    // DC-DEFAULT: NET-24 (proposed, project decision) the move needs the link's secret
    pub fn migrate(
        &mut self,
        now: Millis,
        from: &A,
        bytes: &[u8],
        same_host: impl Fn(&A, &A) -> bool,
        min_silence_ms: u32,
    ) -> Option<A> {
        if bytes.is_empty() || is_oob_datagram(bytes) || self.links.contains_key(from) {
            return None;
        }
        let mut candidates = 0usize;
        let mut found: Vec<A> = Vec::new();
        for (peer, link) in &self.links {
            if link.is_open()
                && same_host(peer, from)
                && since(now, link.last_recv()) >= min_silence_ms
            {
                candidates += 1;
                if link.claims(bytes) {
                    found.push(peer.clone());
                }
            }
        }
        let old = match found.len() {
            0 => {
                if candidates > 0 {
                    self.stats.migrations_refused += 1;
                }
                return None;
            }
            1 => found.pop().expect("one"),
            _ => {
                self.stats.migrations_ambiguous += 1;
                return None;
            }
        };
        if !self.links.get(&old).is_some_and(|l| l.may_move(now)) {
            self.stats.migrations_limited += 1;
            return None;
        }
        let mut link = self.links.remove(&old)?;
        link.on_moved(now);
        self.links.insert(from.clone(), link);
        self.stats.migrations += 1;
        Some(old)
    }

    /// Runs every link's update and returns what happened. Finished links are forgotten.
    pub fn poll(&mut self, now: Millis) -> Vec<HostEvent<A>> {
        for (peer, link) in self.links.iter_mut() {
            for e in link.poll(now) {
                self.events.push_back(match e {
                    LinkEvent::Transmit(bytes) => HostEvent::Transmit {
                        to: peer.clone(),
                        bytes,
                    },
                    LinkEvent::Message { reliable, bits } => HostEvent::Message {
                        peer: peer.clone(),
                        reliable,
                        bits,
                    },
                    LinkEvent::Closed { code, cause } => HostEvent::LinkClosed {
                        peer: peer.clone(),
                        code,
                        cause,
                    },
                });
            }
        }
        self.links.retain(|_, l| !l.is_finished());
        self.events.drain(..).collect()
    }

    fn reply(&mut self, to: A, d: OobDatagram) {
        self.events.push_back(HostEvent::Transmit {
            to,
            bytes: d.encode(),
        });
    }

    fn app_index(&self, app_id: &[u8; 16]) -> Option<usize> {
        self.cfg.app_ids.iter().position(|a| a == app_id)
    }

    // SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb5120)
    fn handle_oob(&mut self, now: Millis, from: A, bytes: &[u8]) {
        let Ok(d) = OobDatagram::decode(bytes) else {
            self.stats.oob_ignored += 1;
            return;
        };
        match d {
            // SPEC: UDP_TRANSPORT_2009.md §5.4 (id mismatch -> ok = 0; else name, echoed tag, flag, link count, byte)
            OobDatagram::InfoAsk { app_id, probe_tag } => {
                self.stats.info_asks += 1;
                let reply = self.app_index(&app_id).map(|_| InfoReply {
                    session_name: self.cfg.session_name.clone(),
                    probe_tag,
                    host_flag: self.cfg.host_flag,
                    link_count: self.open_links().min(255) as u8,
                    host_byte: self.cfg.host_byte,
                });
                self.reply(from, OobDatagram::InfoReply(reply));
            }
            OobDatagram::JoinAsk { app_id, rate_bps } => {
                self.handle_join(now, from, app_id, rate_bps)
            }
            // SPEC: UDP_TRANSPORT_2009.md §5.5 (copy the 24-bit id back with opcode 5; no host state)
            OobDatagram::EchoAsk { echo_id } => {
                self.stats.echo_asks += 1;
                self.reply(from, OobDatagram::EchoReply { echo_id });
            }
            _ => self.stats.oob_ignored += 1,
        }
    }

    /// Join, host side: a live link younger than 9999 ms gets the reply again; an older one is dropped with code 3
    /// and the ask is not answered; a wrong application id gets `accepted = 0, same_app = 0`; a new peer gets a
    /// link with rate `max(requested, local_default)` and `accepted = 1`.
    // SPEC: UDP_TRANSPORT_2009.md §5.3 (engine+0xb5120 case 2)
    fn handle_join(&mut self, now: Millis, from: A, app_id: [u8; 16], rate_bps: u32) {
        self.stats.join_asks += 1;
        if let Some(l) = self.links.get(&from) {
            if l.is_open() {
                if since(now, l.traffic_stamp()) > STALE_LINK_MS {
                    self.links.remove(&from);
                    self.stats.stale_rejoins += 1;
                    self.events.push_back(HostEvent::LinkClosed {
                        peer: from,
                        code: dc_wire::udp::DROP_STALE,
                        cause: CloseCause::StaleRejoin,
                    });
                    return;
                }
                self.stats.joins_repeated += 1;
                self.reply(
                    from,
                    OobDatagram::JoinReply {
                        accepted: true,
                        same_app: false,
                    },
                );
                return;
            }
            // a closing link (bye copies pending) must not send them into the peer's new session
            self.links.remove(&from);
        }
        let Some(app_id_index) = self.app_index(&app_id) else {
            self.stats.joins_wrong_app += 1;
            self.reply(
                from,
                OobDatagram::JoinReply {
                    accepted: false,
                    same_app: false,
                },
            );
            return;
        };
        if self.cfg.max_links.is_some_and(|m| self.open_links() >= m) {
            self.stats.joins_refused += 1;
            self.reply(
                from,
                OobDatagram::JoinReply {
                    accepted: false,
                    same_app: true,
                },
            );
            return;
        }
        let rate = rate_bps.max(self.cfg.local_rate_bps);
        self.links.insert(
            from.clone(),
            Link::new(
                now,
                LinkConfig {
                    hosting: true,
                    rate_bps: rate,
                },
            ),
        );
        self.stats.joins_accepted += 1;
        self.events.push_back(HostEvent::LinkOpened {
            peer: from.clone(),
            rate_bps: rate,
            app_id_index,
        });
        self.reply(
            from,
            OobDatagram::JoinReply {
                accepted: true,
                same_app: false,
            },
        );
    }
}
