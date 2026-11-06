//! One link: the per-peer delivery state machine of the 2009 transport (both sides run the same code).
//!
//! Sans-IO: datagrams go in through [`Link::handle_datagram`], everything the link wants to do comes out of
//! [`Link::poll`] as [`LinkEvent`]s (datagrams to transmit, delivered messages, the close notice). Time is always
//! the caller's `now`, so two links driven by the same inputs behave identically.

use std::collections::VecDeque;

use crate::body::{
    body_bits, ctl_bits, entry_bits, move_trailer, parse_body, rel_block_bits, seal_with_trailer,
    slice_bits, OpenEnd, TRAILER_EXTRA_BYTES,
};
use crate::flow::FlowBucket;
use crate::latency::Latency;
use crate::time::{earliest, later, since, Millis};
use dc_wire::linkmove::{same_secret, Secret};
use dc_wire::udp::{
    check8, classify_rel_seq, datagram_cost_bits, heartbeat_due, next_seq, round8, rto_ms,
    sack_seq, seal_link_body, unrel_ttl_ms, vlen_bits, BitWriter, CtlBlock, LinkBody, RelArrival,
    RelBlock, ACK_TEST_COST_BITS, BODY_BUDGET_BITS, BYE_GAP_MS, BYE_REPEAT,
    DATAGRAM_HEADER_COST_BITS, DROP_PLAIN, DROP_STALE, IDLE_DROP_MS, INITIAL_LINK_RATE_BPS,
    MIN_SEND_BODY_BITS, OUT_OF_ORDER_SLOTS, PAUSE_RESEND_LIMIT, PAUSE_SENT_RECORDS, PUMP_ROOM_BITS,
    RECV_BUFFER_BYTES, RESEND_CAP_AFTER_ACK, RESEND_TEST_EXTRA_BITS, RTO_MIN_MS, SEQ_MASK,
    SPLIT_THRESHOLD_BITS, TRICKLE_RESEND_LIMIT, TRICKLE_RTO_MIN_MS,
};

/// Header cost of one bit (the `more` separator between reliable entries).
const MORE_BIT: u32 = 1;
/// `rel_seq` + `tail_cont`.
const REL_HEAD_BITS: u32 = 14;
/// Bits of the check byte that `link_tx` prefixes (used inside the budget formulas).
const CHECK_BITS: u32 = 8;
/// Body of a plain bye datagram (`[1][CTL_BYE:4]`).
const BYE_BODY_BITS: u32 = 1 + 4;
/// Body of a coded bye datagram (`[1][CTL_BYE_CODE:4][u32]`).
const BYE_CODE_BODY_BITS: u32 = 1 + 4 + 32;
/// Safety bound on datagrams one pump builds (the 2009 loop is bounded by flow control only).
const MAX_DATAGRAMS_PER_PUMP: usize = 256;
/// Largest unreliable message a link accepts: one datagram must still fit the peer's receive buffer
/// (8192 bytes) with the check byte and the block framing.
// SPEC: UDP_TRANSPORT_2009.md §4, §11.5 (8192-byte receive buffer of the peer)
pub const MAX_UNRELIABLE_BITS: usize = (RECV_BUFFER_BYTES - 16) * 8;

/// DeCombat deviation: an old, duplicate or too-far-ahead reliable block makes an ack due. The 2009 receiver sets
/// no ack-due for them and relies on the next periodic ack (spec §8.1), but that ack waits for `max(200, L_ms)` and
/// `L_ms` grows while the receiver itself waits for an ack (spec §12.1, `L_eff`). The property test
/// `reliable_delivery_is_exactly_once_and_in_order` found the resulting stall: when both sides lose the ack of their
/// last block, neither acknowledges the other's resends again. An extra ack is idempotent for a 2009 peer
/// (a stale cumulative ack only uses its selective part, spec §10.2), and our acks clear the 2009 peer's wait.
// SPEC: UDP_TRANSPORT_2009.md §8.1 (old / duplicate -> no ack-due), §11.1 (hb_due), §12.1 (L_eff), §10.2
// DC-DEFAULT: DSV-4 (staged proposal, dserver-slice1_DC_DEFAULTS.md)
pub const ACK_DISCARDED_BLOCKS: bool = true;

/// DeCombat deviation: no link datagram is larger than this many UDP payload bytes (check byte included), so the IP
/// datagram stays within the 576 bytes every IPv4 host must accept and is never fragmented on any path. The 2009
/// sender fills a reliable block up to the 8800-bit body budget (spec §8, §11.2: about 1100 bytes) and packs queued
/// unreliable messages up to the same budget; a path that drops datagrams of that size (a small tunnel MTU, a
/// firewall dropping fragments or a path-MTU black hole) then loses every resend of such a block, the in-order
/// stream stops, and both sides end up sending only full-size datagrams. Here a reliable block, the unreliable
/// messages packed with it and the resend of a block (with the largest ack in front) all stay within the cap; the
/// 2009 split (`tail_cont`) carries longer messages, so the framing is unchanged and a 2009 peer reads it as usual.
/// Only an unreliable message that is itself longer than the cap still goes out alone (the 2009 framing cannot
/// split it; counted in [`LinkStats::oversize_out`]).
// SPEC: UDP_TRANSPORT_2009.md §8, §8.2, §11.2, §11.4 (budgets the cap tightens)
// DC-DEFAULT: NET-10 (proposed; project decision) datagram cap 548 payload bytes (576-byte IPv4 datagram)
pub const MAX_DATAGRAM_BYTES: usize = 548;

/// Body bits of a datagram at the cap (the check byte is the payload's first byte).
const CAP_BODY_BITS: u32 = (MAX_DATAGRAM_BYTES as u32 - 1) * 8;

/// Largest control block a resend can put in front of a stored block (`CTL_ACK` with the selective mask).
const MAX_ACK_CTL_BITS: u32 = 4 + 13 + 1 + 8;

/// DeCombat deviation: unreliable messages keep going out while new reliable blocks are paused (more than 8
/// outstanding, a block resent more than twice, trickle mode). The 2009 pump sends no queued data at all while
/// paused (spec §11.1), so a link whose reliable stream waits for an ack also stops the player updates (client) and
/// the object stream (server); the peer then hears only resends, the waiting side's queued updates pile up behind
/// them, and a lossy path can look dead for seconds. A 2009 peer reads an unreliable-only datagram as any other.
// SPEC: UDP_TRANSPORT_2009.md §11.1 (engine+0xb3b20: `!paused && queued`)
// DC-DEFAULT: NET-11 (proposed; project decision)
pub const UNRELIABLE_WHILE_PAUSED: bool = true;

/// DeCombat deviation: the ack heartbeat comes at least every this many ms. The 2009 interval is `max(200, L_ms)`
/// (spec §11.1) and `L_ms` grows with the time a sender has waited for an ack (§12.1, `L_eff`), so a side whose own
/// reliable block goes unacknowledged stops its heartbeat altogether. An extra ack is idempotent for a 2009 peer
/// (see [`ACK_DISCARDED_BLOCKS`]).
// SPEC: UDP_TRANSPORT_2009.md §11.1 (hb_due), §12.1 (L_eff)
// DC-DEFAULT: NET-12 (proposed; project decision) 1000 ms (the largest RTO, spec §11.3)
pub const HEARTBEAT_MAX_MS: u32 = 1_000;

/// Link construction parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkConfig {
    /// This side is the host: a plain bye from the peer is then reported with code 0, otherwise with 3.
    // SPEC: UDP_TRANSPORT_2009.md §7 (engine+0xb5d70: `hosting ? 0 : 3`)
    pub hosting: bool,
    /// Link rate in bits per second (flow control). Until set it is `0x7FFFFFFF`.
    // SPEC: UDP_TRANSPORT_2009.md §12.2 (engine+0xb48b0)
    pub rate_bps: u32,
}

impl Default for LinkConfig {
    fn default() -> Self {
        LinkConfig {
            hosting: false,
            rate_bps: INITIAL_LINK_RATE_BPS,
        }
    }
}

/// What a local close sends to the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bye {
    /// Nothing (the link just disappears).
    Silent,
    /// `CTL_BYE`, 4 copies 100 ms apart.
    Plain,
    /// `CTL_BYE_CODE(code)`, 4 copies 100 ms apart.
    Code(u32),
}

/// Why a link closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    /// The peer sent a bye.
    PeerBye,
    /// Nothing received for more than 90 s (a `CTL_BYE_CODE(3)` goes out).
    IdleTimeout,
    /// [`Link::close`] was called.
    Local,
    /// Host only: the peer asked to join again more than 9999 ms after the link's last traffic; the old link is
    /// dropped with code 3 and the ask is not answered (the client's next retry creates a fresh link).
    // SPEC: UDP_TRANSPORT_2009.md §5.3 step 1 (engine+0xb5120)
    StaleRejoin,
}

/// Output of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// A datagram to send to the peer (check byte included).
    Transmit(Vec<u8>),
    /// A delivered session message. Reliable messages arrive exactly once and in send order.
    Message { reliable: bool, bits: BitWriter },
    /// The link is closed; no message is delivered after this. `code` is the drop event code.
    Closed { code: u32, cause: CloseCause },
}

/// Life cycle of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Open,
    /// Closed, bye copies still to send.
    Closing,
    /// Closed, nothing left to send.
    Closed,
}

/// Counters for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkStats {
    pub datagrams_out: u64,
    pub datagrams_in: u64,
    pub bad_check: u64,
    pub rel_blocks_out: u64,
    pub resends: u64,
    pub keepalive_blocks: u64,
    pub acks_out: u64,
    pub flow_drops: u64,
    pub rel_in_order: u64,
    pub rel_stored: u64,
    pub rel_discarded: u64,
    pub rel_slots_full: u64,
    pub rel_abandoned: u64,
    pub delivered_rel: u64,
    pub delivered_unrel: u64,
    pub unrel_expired: u64,
    pub split_parts_out: u64,
    pub max_body_bits: u32,
    /// Largest datagram sent (UDP payload bytes, check byte included).
    // DC-DEFAULT: NET-10 (proposed)
    pub max_datagram_bytes: u32,
    /// Datagrams sent over [`MAX_DATAGRAM_BYTES`] (only an unreliable message longer than the cap makes one).
    pub oversize_out: u64,
    /// Receive-side sequence statistics (ours, for the client's network overlay): how far the highest reliable
    /// block number seen advanced, and how many numbers it skipped on the way (blocks missing when a later one
    /// arrived: lost, or reordered).
    // DC-DEFAULT: CLI-1572 (proposed) loss in from reliable sequence gaps (statistics only, no wire change)
    pub rel_seq_span: u64,
    pub rel_seq_gaps: u64,
    /// Transport byes ignored because the link moved less than [`BYE_AFTER_MOVE_MS`] ago (NET-25).
    // DC-DEFAULT: NET-25 (proposed)
    pub byes_after_move: u64,
}

#[derive(Debug, Clone)]
struct QueuedRel {
    msg: BitWriter,
    /// Bits already sent as earlier parts of a split message.
    offset: usize,
}

#[derive(Debug, Clone)]
struct QueuedUnrel {
    queued_at: Millis,
    msg: BitWriter,
}

/// How far (in block numbers, either way) a sequence number of a datagram from a new address may be from the link's
/// own for [`Link::claims`]: at most 8 blocks are outstanding, so a real peer is always within a few; a stranger
/// matches by chance about 1 in 60.
// DC-DEFAULT: NET-23 (proposed)
pub const CLAIM_SEQ_WINDOW: u16 = 64;

/// A link moves at most this many times within [`MOVE_WINDOW_MS`] (NET-25): a rebinding NAT moves a player a few
/// times an hour at most, so a burst of moves is a fault or an attack, and the link stays where it is.
// DC-DEFAULT: NET-25 (proposed, project decision) 3 moves a minute
pub const MOVE_LIMIT: usize = 3;
/// See [`MOVE_LIMIT`].
// DC-DEFAULT: NET-25 (proposed, project decision)
pub const MOVE_WINDOW_MS: u32 = 60_000;
/// A transport bye (`CTL_BYE` / `CTL_BYE_CODE`) closes a link only once the link has been on its address this long
/// (NET-25): the datagram that moves a link, and the ones right after it, never close it. A real client that leaves
/// right after a rebinding still ends its session with `C2S_BYE` (a reliable message), or at worst by the silence
/// drop.
// DC-DEFAULT: NET-25 (proposed, project decision) 1000 ms
pub const BYE_AFTER_MOVE_MS: u32 = 1_000;

/// NET-27: the largest reliable message the receiver joins from split entries, in bits (8 MiB). A peer that keeps a
/// message open past it is closed: the assembly buffer would otherwise grow with everything it sends.
// DC-DEFAULT: NET-27 (proposed) a bound on the reliable assembly buffer
pub const MAX_ASSEMBLY_BITS: usize = 8 * 1024 * 1024 * 8;

/// NET-28: the most reliable messages a link keeps queued. A peer that makes this side queue more (it never acks
/// while its requests keep asking for answers) is closed instead of growing the queue without end.
// DC-DEFAULT: NET-28 (proposed) a bound on the reliable send queue
pub const MAX_QUEUED_RELIABLE: usize = 65_536;

/// The sender's copy of one reliable block.
// SPEC: UDP_TRANSPORT_2009.md §11.2 (sent record: seq, block bits, entries, first_send, next_check, acked, count)
#[derive(Debug, Clone)]
struct SentRecord {
    seq: u16,
    tail_cont: bool,
    entries: Vec<BitWriter>,
    bits: u32,
    first_send: Millis,
    next_check: Millis,
    acked: bool,
    resends: u32,
}

#[derive(Debug, Clone, Copy)]
struct Closing {
    bye: Bye,
    copies_left: u32,
    next_at: Millis,
}

/// One peer connection.
#[derive(Debug, Clone)]
pub struct Link {
    hosting: bool,
    state: LinkState,
    closing: Option<Closing>,
    /// Quiet state (after the session layer's `C2S_BYE`): receive only, nothing leaves.
    quiet: bool,
    /// [`Link::quiesce_when_flushed`] was called and the reliable queue has not drained yet.
    quiesce_pending: bool,
    /// The plain bye burst was started (it is sent at most once per link).
    plain_bye_started: bool,
    // send side
    send_seq: u16,
    rel_queue: VecDeque<QueuedRel>,
    unrel_queue: VecDeque<QueuedUnrel>,
    sent: VecDeque<SentRecord>,
    paused: bool,
    stalled: bool,
    stall_since: Millis,
    // receive side
    last_in_order: u16,
    /// Highest reliable block number seen (statistics only, [`LinkStats::rel_seq_span`]).
    rel_highest: u16,
    stored: Vec<RelBlock>,
    assembly: Option<BitWriter>,
    /// NET-27: the assembly went past [`MAX_ASSEMBLY_BITS`]; the link closes after this datagram.
    assembly_overflow: bool,
    /// NET-28: a reliable message found the queue full; the link closes at its next update.
    queue_overflow: bool,
    ack_due: bool,
    last_ack_sent: Millis,
    // timing
    latency: Latency,
    flow: FlowBucket,
    last_recv: Millis,
    traffic_stamp: Millis,
    events: VecDeque<LinkEvent>,
    stats: LinkStats,
    // link moves (NET-24, NET-25)
    /// Host side: the secret a datagram from a new address must carry to move this link.
    move_secret: Option<Secret>,
    /// Peer side: the secret every datagram this link sends carries as its trailer.
    move_trailer: Option<Secret>,
    /// Times of the moves within the last [`MOVE_WINDOW_MS`] (at most [`MOVE_LIMIT`]).
    moves: VecDeque<Millis>,
    /// Time of the last move.
    moved_at: Option<Millis>,
}

impl Link {
    /// A new open link at `now`. Both sequence counters start at 0 and are pre-incremented, so the first
    /// reliable block carries `rel_seq = 1`.
    // SPEC: UDP_TRANSPORT_2009.md §8 (engine+0xb48b0), §12.1 (initial latency 200 ms)
    pub fn new(now: Millis, cfg: LinkConfig) -> Link {
        Link {
            hosting: cfg.hosting,
            state: LinkState::Open,
            closing: None,
            quiet: false,
            quiesce_pending: false,
            plain_bye_started: false,
            send_seq: 0,
            rel_queue: VecDeque::new(),
            unrel_queue: VecDeque::new(),
            sent: VecDeque::new(),
            paused: false,
            stalled: false,
            stall_since: now,
            last_in_order: 0,
            rel_highest: 0,
            stored: Vec::new(),
            assembly: None,
            assembly_overflow: false,
            queue_overflow: false,
            ack_due: false,
            last_ack_sent: now,
            latency: Latency::default(),
            flow: FlowBucket::new(cfg.rate_bps, now),
            last_recv: now,
            traffic_stamp: now,
            events: VecDeque::new(),
            stats: LinkStats::default(),
            move_secret: None,
            move_trailer: None,
            moves: VecDeque::new(),
            moved_at: None,
        }
    }

    // ------------------------------------------------------------------------------------------- accessors

    pub fn state(&self) -> LinkState {
        self.state
    }

    pub fn is_open(&self) -> bool {
        self.state == LinkState::Open
    }

    /// Closed and every bye copy sent: the owner may forget the link.
    pub fn is_finished(&self) -> bool {
        self.state == LinkState::Closed && self.events.is_empty()
    }

    pub fn stats(&self) -> LinkStats {
        self.stats
    }

    /// `L_ms` as the timers use it.
    pub fn l_ms(&self) -> u32 {
        self.latency.l_ms()
    }

    pub fn latency(&self) -> &Latency {
        &self.latency
    }

    pub fn rate_bps(&self) -> u32 {
        self.flow.rate_bps()
    }

    pub fn flow(&self) -> &FlowBucket {
        &self.flow
    }

    /// Changes the link rate (resets the flow window).
    // SPEC: UDP_TRANSPORT_2009.md §12.2 (rate method VA 0x50ea50 not decompiled; mapping to link+0x150 UNPROVEN)
    pub fn change_rate(&mut self, now: Millis, rate_bps: u32) {
        self.flow.change_rate(rate_bps.max(1), now);
    }

    /// Sent reliable blocks not yet removed by an ack.
    pub fn outstanding(&self) -> usize {
        self.sent.len()
    }

    /// New reliable blocks are paused (more than 8 outstanding, or a record resent more than twice).
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Trickle mode (a record was resent more than 10 times).
    pub fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// Reliable messages (or remaining parts) not yet put into a block.
    pub fn queued_reliable(&self) -> usize {
        self.rel_queue.len()
    }

    pub fn queued_unreliable(&self) -> usize {
        self.unrel_queue.len()
    }

    /// Nothing queued and nothing outstanding.
    pub fn is_idle(&self) -> bool {
        self.rel_queue.is_empty() && self.unrel_queue.is_empty() && self.sent.is_empty()
    }

    /// Time of the last received datagram with a good check byte (NET-26: corrupt ones no longer count).
    pub fn last_recv(&self) -> Millis {
        self.last_recv
    }

    /// Traffic stamp used by the host's stale re-join test.
    pub fn traffic_stamp(&self) -> Millis {
        self.traffic_stamp
    }

    /// `rel_seq` of the last block received in order (the cumulative ack value).
    pub fn last_in_order(&self) -> u16 {
        self.last_in_order
    }

    /// `rel_seq` of the last block this side created.
    pub fn newest_block_seq(&self) -> u16 {
        self.send_seq
    }

    /// True when `datagram` proves it comes from this link's peer: it carries this link's move secret as its trailer
    /// (compared in constant time), has a good check byte, and the sequence numbers it carries fit this link (its
    /// cumulative ack names a block this side sent lately, its reliable block is near the next one this side expects;
    /// a datagram with neither, unreliable data only, fits). No secret on this link, or none in the datagram: false.
    /// The test behind a NAT rebinding's address move ([`crate::Host::migrate`]).
    // DC-DEFAULT: NET-23 (proposed) address move of a link after a NAT rebinding, see DC_DEFAULTS.md
    // DC-DEFAULT: NET-24 (proposed, project decision) only with the link's move secret
    pub fn claims(&self, datagram: &[u8]) -> bool {
        let (Some(want), Some(got)) = (self.move_secret.as_ref(), move_trailer(datagram)) else {
            return false;
        };
        if !same_secret(want, &got) {
            return false;
        }
        let rx = parse_body(&datagram[1..]);
        let near = |a: u16, b: u16| {
            let d = a.wrapping_sub(b) & SEQ_MASK;
            d <= CLAIM_SEQ_WINDOW || d >= SEQ_MASK + 1 - CLAIM_SEQ_WINDOW
        };
        if let Some(CtlBlock::Ack { ack_seq, .. }) = rx.ctl {
            if !near(ack_seq & SEQ_MASK, self.send_seq) {
                return false;
            }
        }
        if let Some(b) = &rx.rel {
            if !near(b.rel_seq & SEQ_MASK, self.last_in_order) {
                return false;
            }
        }
        true
    }

    /// Host side: datagrams from a new address move this link only when they carry `secret` (NET-24).
    // DC-DEFAULT: NET-24 (proposed, project decision)
    pub fn set_move_secret(&mut self, secret: Secret) {
        self.move_secret = Some(secret);
    }

    /// The host side's move secret, if one was set.
    pub fn move_secret(&self) -> Option<&Secret> {
        self.move_secret.as_ref()
    }

    /// Peer side: every datagram from now on carries `secret` as its trailer, so the host can follow this side to a
    /// new address (NET-24). The cap of [`MAX_DATAGRAM_BYTES`] still holds.
    // DC-DEFAULT: NET-24 (proposed, project decision)
    pub fn set_move_trailer(&mut self, secret: Secret) {
        self.move_trailer = Some(secret);
    }

    /// The secret this side's datagrams carry, if any.
    pub fn move_trailer(&self) -> Option<&Secret> {
        self.move_trailer.as_ref()
    }

    /// True when the link may move once more at `now` ([`MOVE_LIMIT`] within [`MOVE_WINDOW_MS`]).
    // DC-DEFAULT: NET-25 (proposed, project decision)
    pub fn may_move(&self, now: Millis) -> bool {
        self.moves
            .iter()
            .filter(|&&t| since(now, t) < MOVE_WINDOW_MS)
            .count()
            < MOVE_LIMIT
    }

    /// The host moved this link to a new address at `now`: counts the move, holds transport byes off for
    /// [`BYE_AFTER_MOVE_MS`], and makes every unacknowledged reliable block and an ack due at once, so what went to
    /// the old address (lost in the NAT) goes to the new one now instead of after its timeout.
    // DC-DEFAULT: NET-25 (proposed, project decision)
    pub fn on_moved(&mut self, now: Millis) {
        while self
            .moves
            .front()
            .is_some_and(|&t| since(now, t) >= MOVE_WINDOW_MS)
        {
            self.moves.pop_front();
        }
        self.moves.push_back(now);
        self.moved_at = Some(now);
        // `since(now, earliest(next_check, now)) >= rto` holds for any rto up to 10 s
        let due = now.wrapping_sub(10_000);
        for r in self.sent.iter_mut().filter(|r| !r.acked) {
            r.next_check = due;
        }
        if self.is_open() && !self.quiet {
            self.ack_due = true;
        }
    }

    // ------------------------------------------------------------------------------------------- input

    /// Queues a reliable message (delivered once, in order). Zero-length messages deliver nothing on the
    /// 2009 receiver and are dropped here. Returns false when the link is closed.
    // SPEC: UDP_TRANSPORT_2009.md §8 (an entry with L = 0 delivers nothing)
    pub fn send_reliable(&mut self, msg: BitWriter) -> bool {
        if !self.is_open() {
            return false;
        }
        if self.quiet {
            // quiet: accepted and discarded (nothing may leave)
            return true;
        }
        if self.rel_queue.len() >= MAX_QUEUED_RELIABLE {
            self.queue_overflow = true;
            return false;
        }
        if !msg.is_empty() {
            self.rel_queue.push_back(QueuedRel { msg, offset: 0 });
        }
        true
    }

    /// Queues an unreliable message; it is dropped if it waits longer than `2 * max(L_ms, 200)` ms. Returns
    /// false when the link is closed or the message cannot fit one datagram of the peer's receive buffer.
    // SPEC: UDP_TRANSPORT_2009.md §9, §11.4
    pub fn send_unreliable(&mut self, now: Millis, msg: BitWriter) -> bool {
        if !self.is_open() || msg.bit_len() > MAX_UNRELIABLE_BITS {
            return false;
        }
        if self.quiet {
            return true;
        }
        if !msg.is_empty() {
            self.unrel_queue.push_back(QueuedUnrel {
                queued_at: now,
                msg,
            });
        }
        true
    }

    /// Enters the quiet state: the session layer has sent `C2S_BYE`, so the link sends nothing more (no ack, not
    /// even for a block that arrives in the same tick, no keep-alive, no resend, no queued data). Incoming
    /// datagrams are still parsed and delivered, so the peer's bye code is observed. Queued outgoing data is
    /// discarded. The idle check still runs, but a quiet idle drop is silent (no bye code 3 goes out). Only
    /// [`Link::send_plain_bye`] (or [`Link::close`]) makes the link transmit again.
    // SPEC: UDP_TRANSPORT_2009.md §9.1 (PROVEN-CAPTURE tdm1: silence after C2S_BYE); CAPTURE_tdm1_TRANSPORT_VALIDATION C-T12
    pub fn quiesce(&mut self) {
        if self.state != LinkState::Open || self.quiet {
            return;
        }
        self.quiet = true;
        self.ack_due = false;
        self.rel_queue.clear();
        self.unrel_queue.clear();
        self.sent.clear();
    }

    /// Like [`Link::quiesce`], but only once everything queued (typically the block carrying `C2S_BYE`) has left in
    /// a datagram: the quiet state starts at the end of the update that drains the reliable queue. Use it right
    /// after queuing `C2S_BYE`; the real client sends that block and then nothing.
    // SPEC: CAPTURE_tdm1_TRANSPORT_VALIDATION.md C-T12 (C2S_BYE leaves, then silence)
    pub fn quiesce_when_flushed(&mut self) {
        if self.state == LinkState::Open && !self.quiet {
            self.quiesce_pending = true;
        }
    }

    /// True while the link is in the quiet state.
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// The explicit plain `CTL_BYE` burst (4 copies, 100 ms apart, first now) the real client sends when its shell
    /// leaves the result screen. Works from the quiet state and also after the peer's bye closed the link (the
    /// capture shows it 8.7 s after the host's code 5). Emits [`LinkEvent::Closed`] (cause `Local`) only when the
    /// link was still open. A second call, or a call while another bye is in progress, does nothing.
    // SPEC: UDP_TRANSPORT_2009.md §9.1 (PROVEN-CAPTURE tdm1: plain CTL_BYE x4 only when leaving the result screen)
    pub fn send_plain_bye(&mut self, now: Millis) {
        if self.plain_bye_started || self.closing.is_some() {
            return;
        }
        match self.state {
            LinkState::Open => self.drop_link(now, Bye::Plain, DROP_PLAIN, CloseCause::Local),
            LinkState::Closed => {
                self.plain_bye_started = true;
                self.state = LinkState::Closing;
                self.closing = Some(Closing {
                    bye: Bye::Plain,
                    copies_left: BYE_REPEAT,
                    next_at: now,
                });
                self.flow.tick(now);
                self.pump_bye(now);
            }
            LinkState::Closing => {}
        }
    }

    /// Closes the link locally. With a bye, 4 copies go out 100 ms apart (the first now) through the normal
    /// send path; a [`LinkEvent::Closed`] with cause `Local` is emitted at once.
    // SPEC: UDP_TRANSPORT_2009.md §9.1 (engine+0xb3920, engine+0xb3a10), §13 (local drop engine+0xb4ff0)
    pub fn close(&mut self, now: Millis, bye: Bye) {
        if !self.is_open() {
            return;
        }
        let code = match bye {
            Bye::Code(c) => c,
            Bye::Silent | Bye::Plain => DROP_PLAIN,
        };
        self.drop_link(now, bye, code, CloseCause::Local);
    }

    /// Handles one link datagram from the peer.
    ///
    /// A check mismatch drops the datagram silently. The 2009 receiver sets the traffic stamp and the last-receive
    /// time before it tests the check byte, so a corrupt datagram still counts as liveness; ours sets them only for
    /// a good one (NET-26: junk sent to a link's address must not keep it alive). A valid datagram runs the link
    /// update at once so an ack can leave immediately.
    // SPEC: UDP_TRANSPORT_2009.md §6.1, §6.3 (engine+0xb4a70, engine+0xb5d70 -> engine+0xb3e40)
    // DC-DEFAULT: NET-26 (proposed, project decision) liveness from good datagrams only
    pub fn handle_datagram(&mut self, now: Millis, datagram: &[u8]) {
        if datagram.is_empty() || !self.is_open() {
            return;
        }
        self.stats.datagrams_in += 1;
        let (&got, body) = datagram.split_first().expect("non-empty");
        if check8(body) != got {
            self.stats.bad_check += 1;
            return;
        }
        self.traffic_stamp = now;
        self.last_recv = now;
        let rx = parse_body(body);
        // NET-25: no transport bye right after a move
        let bye_held = self
            .moved_at
            .is_some_and(|t| since(now, t) < BYE_AFTER_MOVE_MS);
        if let Some(ctl) = rx.ctl {
            match ctl {
                CtlBlock::Bye | CtlBlock::ByeCode(_) if bye_held => {
                    self.stats.byes_after_move += 1;
                }
                CtlBlock::Ack { ack_seq, sack_mask } => self.on_ack(now, ack_seq, sack_mask),
                // SPEC: UDP_TRANSPORT_2009.md §7 (CTL_BYE -> 0 if hosting else 3; code 0 ignored; code 1 = plain bye)
                CtlBlock::Bye | CtlBlock::ByeCode(1) => {
                    let code = if self.hosting { DROP_PLAIN } else { DROP_STALE };
                    self.peer_closed(code);
                }
                CtlBlock::ByeCode(0) | CtlBlock::Unknown(_) => {}
                CtlBlock::ByeCode(code) => self.peer_closed(code),
            }
            if !self.is_open() {
                return;
            }
        }
        if rx.rel_abandoned {
            self.stats.rel_abandoned += 1;
        }
        if let Some(block) = rx.rel {
            self.on_rel_block(block);
        }
        if self.assembly_overflow {
            self.close(now, Bye::Plain);
            return;
        }
        for bits in rx.unrel {
            self.stats.delivered_unrel += 1;
            self.events.push_back(LinkEvent::Message {
                reliable: false,
                bits,
            });
        }
        self.update(now);
    }

    /// Runs the link update at `now` (flow tick, latency refresh, idle check, resend scan, send pump, bye copies)
    /// and returns everything that happened since the last poll.
    // SPEC: UDP_TRANSPORT_2009.md §11.1 (link update engine+0xb3e40), §13 (engine+0xb5c30)
    pub fn poll(&mut self, now: Millis) -> Vec<LinkEvent> {
        self.update(now);
        self.events.drain(..).collect()
    }

    // ------------------------------------------------------------------------------------------- receive

    /// Cumulative + selective ack.
    // SPEC: UDP_TRANSPORT_2009.md §10.2 (engine+0xb2500, engine+0xb23d0, engine+0xaf2b0)
    fn on_ack(&mut self, now: Millis, ack_seq: u16, sack_mask: Option<u8>) {
        let ack_seq = ack_seq & SEQ_MASK;
        if let Some(idx) = self.sent.iter().position(|r| r.seq == ack_seq) {
            let first_send = self.sent[idx].first_send;
            self.latency.sample(since(now, first_send));
            self.sent.drain(..=idx);
            for r in &mut self.sent {
                r.resends = r.resends.min(RESEND_CAP_AFTER_ACK);
            }
        }
        let mut newly = false;
        if let Some(mask) = sack_mask {
            for bit in 0..8u8 {
                if mask & (1 << bit) == 0 {
                    continue;
                }
                // the 2009 receiver does not mask the sum to 13 bits; the spec advises masking
                let seq = sack_seq(ack_seq, bit);
                if let Some(r) = self.sent.iter_mut().find(|r| r.seq == seq) {
                    if !r.acked {
                        r.acked = true;
                        newly = true;
                        let first_send = r.first_send;
                        self.latency.sample(since(now, first_send));
                    }
                }
            }
        }
        if newly {
            // fast retransmit of the gap: pull the first unacknowledged record's stamp back by max(L_ms, 17)
            let pull = self.latency.l_ms().max(RTO_MIN_MS);
            if let Some(r) = self.sent.iter_mut().find(|r| !r.acked) {
                r.next_check = r.next_check.wrapping_sub(pull);
            }
            self.ack_due = true;
        }
    }

    /// Reliable block arrival: in order -> deliver and replay stored blocks; 1..8 ahead -> store in one of 8 slots;
    /// otherwise discard (ack-due only with [`ACK_DISCARDED_BLOCKS`]).
    // SPEC: UDP_TRANSPORT_2009.md §8.1 (engine+0xb2690, engine+0xaf450)
    fn on_rel_block(&mut self, block: RelBlock) {
        let arrival = classify_rel_seq(block.rel_seq, self.last_in_order);
        if !matches!(arrival, RelArrival::Discard) {
            self.note_rel_seq(block.rel_seq);
        }
        match arrival {
            RelArrival::InOrder => {
                self.stats.rel_in_order += 1;
                self.last_in_order = block.rel_seq & SEQ_MASK;
                self.ack_due = true;
                self.deliver_block(&block);
                self.replay_stored();
            }
            RelArrival::Ahead(_) => {
                if !self.stored.iter().any(|b| b.rel_seq == block.rel_seq) {
                    if self.stored.len() < OUT_OF_ORDER_SLOTS {
                        self.stats.rel_stored += 1;
                        self.stored.push(block);
                    } else {
                        self.stats.rel_slots_full += 1;
                    }
                }
                self.ack_due = true;
            }
            RelArrival::Discard => {
                self.stats.rel_discarded += 1;
                if ACK_DISCARDED_BLOCKS {
                    self.ack_due = true;
                }
            }
        }
    }

    /// Statistics only: a block number past the highest seen advances the span; the numbers it skipped count as
    /// gaps. A block that fills an older gap changes nothing.
    // DC-DEFAULT: CLI-1572 (proposed)
    fn note_rel_seq(&mut self, seq: u16) {
        let d = seq.wrapping_sub(self.rel_highest) & SEQ_MASK;
        if d == 0 || d > SEQ_MASK / 2 {
            return;
        }
        self.stats.rel_seq_span += d as u64;
        self.stats.rel_seq_gaps += (d - 1) as u64;
        self.rel_highest = seq & SEQ_MASK;
    }

    fn replay_stored(&mut self) {
        loop {
            let expected = next_seq(self.last_in_order);
            let Some(i) = self.stored.iter().position(|b| b.rel_seq == expected) else {
                break;
            };
            let b = self.stored.swap_remove(i);
            self.last_in_order = expected;
            self.deliver_block(&b);
        }
    }

    /// Delivers a block's entries in order, joining split messages through the assembly buffer.
    // SPEC: UDP_TRANSPORT_2009.md §8.2 (receiver PLAUSIBLE)
    fn deliver_block(&mut self, block: &RelBlock) {
        if self.assembly_overflow {
            return;
        }
        let n = block.entries.len();
        for (i, e) in block.entries.iter().enumerate() {
            let last = i + 1 == n;
            if i == 0 {
                if let Some(acc) = self.assembly.as_mut() {
                    acc.append(e);
                    if acc.bit_len() > MAX_ASSEMBLY_BITS {
                        self.assembly = None;
                        self.assembly_overflow = true;
                        return;
                    }
                    if last && block.tail_cont {
                        continue;
                    }
                    let whole = self.assembly.take().expect("assembly");
                    self.deliver_reliable(whole);
                    continue;
                }
            }
            if last && block.tail_cont {
                self.assembly = Some(e.clone());
                continue;
            }
            self.deliver_reliable(e.clone());
        }
    }

    fn deliver_reliable(&mut self, bits: BitWriter) {
        if bits.is_empty() {
            return;
        }
        self.stats.delivered_rel += 1;
        self.events.push_back(LinkEvent::Message {
            reliable: true,
            bits,
        });
    }

    fn peer_closed(&mut self, code: u32) {
        self.state = LinkState::Closed;
        self.closing = None;
        self.clear_queues();
        self.events.push_back(LinkEvent::Closed {
            code,
            cause: CloseCause::PeerBye,
        });
    }

    fn clear_queues(&mut self) {
        self.rel_queue.clear();
        self.unrel_queue.clear();
        self.sent.clear();
        self.stored.clear();
        self.assembly = None;
    }

    // ------------------------------------------------------------------------------------------- update

    fn update(&mut self, now: Millis) {
        match self.state {
            LinkState::Closed => return,
            LinkState::Closing => {
                self.flow.tick(now);
                self.pump_bye(now);
                return;
            }
            LinkState::Open => {}
        }
        if self.queue_overflow {
            self.close(now, Bye::Plain);
            return;
        }
        self.flow.tick(now);
        self.latency.refresh(now);
        if self.quiet {
            // Quiet: only the idle check runs, and its drop is silent.
            if since(now, self.last_recv) > IDLE_DROP_MS {
                self.drop_link(now, Bye::Silent, DROP_STALE, CloseCause::IdleTimeout);
            }
            return;
        }
        // SPEC: UDP_TRANSPORT_2009.md §13 (`now - last_recv >= 90001` -> drop with code 3 and CTL_BYE_CODE(3))
        if since(now, self.last_recv) > IDLE_DROP_MS {
            self.drop_link(
                now,
                Bye::Code(DROP_STALE),
                DROP_STALE,
                CloseCause::IdleTimeout,
            );
            return;
        }
        self.pump(now);
        if self.quiesce_pending && self.rel_queue.is_empty() {
            self.quiesce_pending = false;
            self.quiesce();
        }
    }

    fn drop_link(&mut self, now: Millis, bye: Bye, code: u32, cause: CloseCause) {
        self.clear_queues();
        self.events.push_back(LinkEvent::Closed { code, cause });
        if bye == Bye::Silent {
            self.state = LinkState::Closed;
            return;
        }
        self.state = LinkState::Closing;
        self.plain_bye_started = bye == Bye::Plain;
        self.closing = Some(Closing {
            bye,
            copies_left: BYE_REPEAT,
            next_at: now,
        });
        self.pump_bye(now);
    }

    /// Bye copies: 4, 100 ms apart, each through the normal link send (check byte, flow control).
    // SPEC: UDP_TRANSPORT_2009.md §9.1 (Sleep(100) between copies in the 2009 code; timers here)
    fn pump_bye(&mut self, now: Millis) {
        while let Some(c) = self.closing {
            if c.copies_left == 0 {
                self.closing = None;
                self.state = LinkState::Closed;
                break;
            }
            if later(c.next_at, now) {
                break;
            }
            let mut w = BitWriter::new();
            w.write_bit(true);
            let body_len = match c.bye {
                Bye::Code(code) => {
                    write_ctl(&mut w, &CtlBlock::ByeCode(code));
                    BYE_CODE_BODY_BITS
                }
                _ => {
                    write_ctl(&mut w, &CtlBlock::Bye);
                    BYE_BODY_BITS
                }
            };
            debug_assert_eq!(w.bit_len() as u32, body_len);
            self.link_send(&w, OpenEnd::AfterCtl);
            self.closing = Some(Closing {
                copies_left: c.copies_left - 1,
                next_at: c.next_at.wrapping_add(BYE_GAP_MS),
                ..c
            });
        }
    }

    fn hb_due(&self, now: Millis) -> bool {
        heartbeat_due(
            now,
            self.last_ack_sent,
            self.latency.l_ms().min(HEARTBEAT_MAX_MS),
        )
    }

    /// Send pump: resend scan, then datagrams while there is something to say and flow control leaves room.
    // SPEC: UDP_TRANSPORT_2009.md §11.1 (engine+0xb3b20)
    fn pump(&mut self, now: Millis) {
        self.resend_scan(now);
        for _ in 0..MAX_DATAGRAMS_PER_PUMP {
            let hb = self.hb_due(now);
            let ack_now = !self.flow.blocked(ACK_TEST_COST_BITS) && (self.ack_due || hb);
            let keepalive = hb && !self.paused;
            if !(self.has_sendable() || ack_now || keepalive) {
                break;
            }
            let ack_was_due = self.ack_due;
            let mut body = LinkBody::default();
            if ack_now {
                body.ctl = Some(self.write_ack(now));
            }
            let mut bits = 1 + body.ctl.as_ref().map_or(0, ctl_bits) + 1;
            let want_rel = !self.paused && (!self.rel_queue.is_empty() || keepalive);
            if want_rel && !self.flow.blocked(datagram_cost_bits(bits)) {
                if let Some(block) = self.build_rel(now, bits, keepalive) {
                    if block.entries.is_empty() {
                        self.stats.keepalive_blocks += 1;
                    }
                    bits += rel_block_bits(&block.entries);
                    body.rel = Some(block);
                }
            }
            body.unrel = self.build_unrel(now, bits);
            let total = body_bits(&body);
            if (total as usize) < MIN_SEND_BODY_BITS {
                break;
            }
            let encoded = body.encode_bits();
            debug_assert_eq!(encoded.bit_len() as u32, total);
            if self
                .flow
                .blocked(datagram_cost_bits(total + self.trailer_bits()))
            {
                if ack_was_due && body.ctl.is_some() {
                    self.ack_due = true;
                }
                self.stats.flow_drops += 1;
                break;
            }
            self.link_send(&encoded, open_end(&body));
            if !(self.has_sendable() && !self.flow.blocked(PUMP_ROOM_BITS)) {
                break;
            }
        }
    }

    /// Queued data the pump may send now: reliable messages unless paused, unreliable messages always
    /// ([`UNRELIABLE_WHILE_PAUSED`]; the 2009 pump: neither while paused).
    fn has_sendable(&self) -> bool {
        let unrel = !self.unrel_queue.is_empty() && (UNRELIABLE_WHILE_PAUSED || !self.paused);
        (!self.paused && !self.rel_queue.is_empty()) || unrel
    }

    /// Ack writer: `ack_seq = last_in_order`, selective bits for stored blocks (`d < 8`; slots with `d >= 9`
    /// are freed). Clears ack-due and stamps `last_ack_sent`.
    // SPEC: UDP_TRANSPORT_2009.md §10.2 writer (engine+0xabc10)
    fn write_ack(&mut self, now: Millis) -> CtlBlock {
        let last = self.last_in_order;
        let d_of = |seq: u16| seq.wrapping_sub(last).wrapping_sub(2) & SEQ_MASK;
        self.stored.retain(|b| d_of(b.rel_seq) < 9);
        let sack_mask = if self.stored.is_empty() {
            None
        } else {
            let mut m = 0u8;
            for b in &self.stored {
                let d = d_of(b.rel_seq);
                if d < 8 {
                    m |= 1 << d;
                }
            }
            Some(m)
        };
        self.ack_due = false;
        self.last_ack_sent = now;
        self.stats.acks_out += 1;
        CtlBlock::Ack {
            ack_seq: last,
            sack_mask,
        }
    }

    /// Builds the reliable block of a new datagram (and its sent record).
    ///
    /// Budget `B_rel = min(8800, W / 2)`; entries are taken while
    /// `round8(datagram_bits + block_bits + header + L + 8) + 224 <= B_rel`. Only a message (or remaining part)
    /// longer than 255 bits is split: the part that fits goes out as the last entry with `tail_cont = 1`, the
    /// rest stays at the head of the queue for the next block. An empty block (keep-alive) is only built when
    /// `forced`, and never while the queue head is the continuation of a split message (it would end the
    /// receiver's assembly early).
    // SPEC: UDP_TRANSPORT_2009.md §8, §8.2, §11.2 (engine+0xb2e20)
    fn build_rel(&mut self, now: Millis, datagram_bits: u32, forced: bool) -> Option<RelBlock> {
        let b_rel = (BODY_BUDGET_BITS as u32).min(self.flow.window_bits() / 2);
        // NET-10: the block must also fit a datagram at the cap behind the largest ack (`[1][ack][1][block]`),
        // so its resend fits whatever ack it carries; the closing `more` bit is counted too.
        let cap_block = self.cap_body_bits() - 2 - MAX_ACK_CTL_BITS - 1;
        let fits = |used: u32, block: u32, len: u32| {
            round8(used + vlen_bits(len) + len + CHECK_BITS) + DATAGRAM_HEADER_COST_BITS <= b_rel
                && block + entry_bits(len) <= cap_block
        };
        let mut block_bits = REL_HEAD_BITS;
        let mut entries: Vec<BitWriter> = Vec::new();
        let mut tail_cont = false;
        while let Some(q) = self.rel_queue.front_mut() {
            let rem = (q.msg.bit_len() - q.offset) as u32;
            let sep = if entries.is_empty() { 0 } else { MORE_BIT };
            let used = datagram_bits + block_bits + sep;
            if fits(used, block_bits + sep, rem) {
                entries.push(slice_bits(&q.msg, q.offset, rem as usize));
                block_bits += sep + entry_bits(rem);
                self.rel_queue.pop_front();
                continue;
            }
            if rem as usize > SPLIT_THRESHOLD_BITS {
                let cap_room = cap_block.saturating_sub(block_bits + sep);
                if let Some(part) = largest_fit(used, rem, b_rel, cap_room) {
                    // the split part is always the last entry of the block
                    entries.push(slice_bits(&q.msg, q.offset, part as usize));
                    q.offset += part as usize;
                    tail_cont = true;
                    self.stats.split_parts_out += 1;
                }
            }
            break;
        }
        if entries.is_empty() {
            let head_is_continuation = self.rel_queue.front().is_some_and(|q| q.offset > 0);
            if !forced || head_is_continuation {
                return None;
            }
        }
        let seq = next_seq(self.send_seq);
        self.send_seq = seq;
        if self.sent.is_empty() {
            self.latency.start_waiting(now);
        }
        let rto = rto_ms(self.latency.l_ms(), false);
        self.sent.push_back(SentRecord {
            seq,
            tail_cont,
            entries: entries.clone(),
            bits: rel_block_bits(&entries),
            first_send: now,
            next_check: now.wrapping_add(rto),
            acked: false,
            resends: 0,
        });
        self.stats.rel_blocks_out += 1;
        Some(RelBlock {
            rel_seq: seq,
            tail_cont,
            entries,
        })
    }

    /// Builds the unreliable block: drops messages older than `2 * max(L_ms, 200)` ms, then takes messages
    /// while `2W >= used + debt + round8(datagram_bits + 1.5 * (header + L) + 8) + 224` and the datagram stays
    /// within 8800 bits (a datagram holding 2 bits or less always takes its first message).
    // SPEC: UDP_TRANSPORT_2009.md §11.4 (engine+0xaf560)
    fn build_unrel(&mut self, now: Millis, datagram_bits: u32) -> Vec<BitWriter> {
        let ttl = unrel_ttl_ms(self.latency.l_ms());
        while let Some(f) = self.unrel_queue.front() {
            if since(now, f.queued_at) > ttl {
                self.unrel_queue.pop_front();
                self.stats.unrel_expired += 1;
            } else {
                break;
            }
        }
        let mut out = Vec::new();
        if self.unrel_queue.is_empty() || self.flow.blocked(datagram_cost_bits(datagram_bits)) {
            return out;
        }
        let mut bits = datagram_bits + 1;
        while let Some(f) = self.unrel_queue.front() {
            let len = f.msg.bit_len() as u32;
            let head = vlen_bits(len);
            let estimate =
                round8(bits + (3 * (head + len)) / 2 + CHECK_BITS) + DATAGRAM_HEADER_COST_BITS;
            let flow_ok = !self.flow.blocked(estimate);
            let lone = datagram_bits <= 2 && out.is_empty();
            // NET-10: within the datagram cap as well; a message longer than the cap still goes out alone
            let size_ok =
                bits + head + len <= (BODY_BUDGET_BITS as u32).min(self.cap_body_bits()) || lone;
            if !(flow_ok && size_ok) {
                break;
            }
            bits += head + len;
            out.push(self.unrel_queue.pop_front().expect("front").msg);
        }
        out
    }

    /// Resend scan: pop acknowledged heads; trickle mode after more than 10 resends; pause decision; resend every
    /// due record. A record is due when `now - min(next_check, now) >= RTO`.
    // SPEC: UDP_TRANSPORT_2009.md §11.3 (engine+0xb3680, resend writer engine+0xb3490)
    fn resend_scan(&mut self, now: Millis) {
        let mut rto = rto_ms(self.latency.l_ms(), false);
        while self.sent.front().is_some_and(|r| r.acked) {
            self.sent.pop_front();
        }
        let was_stalled = self.stalled;
        let trickle = self
            .sent
            .iter()
            .any(|r| !r.acked && r.resends > TRICKLE_RESEND_LIMIT);
        if trickle {
            if !self.stalled {
                self.stalled = true;
                self.stall_since = now;
            }
            self.paused = true;
            rto = rto.max(TRICKLE_RTO_MIN_MS);
            let due = self.sent.front().map(|r| (r.next_check, r.bits));
            if let Some(head) = self.sent.front_mut() {
                head.resends = TRICKLE_RESEND_LIMIT + 1;
            }
            if let Some((next_check, bits)) = due {
                if since(now, earliest(next_check, now)) >= rto
                    && !self.flow.blocked(bits + RESEND_TEST_EXTRA_BITS)
                {
                    self.resend(now, 0);
                }
            }
            return;
        }
        self.stalled = false;
        self.paused = self.sent.len() > PAUSE_SENT_RECORDS
            || self
                .sent
                .iter()
                .any(|r| !r.acked && r.resends > PAUSE_RESEND_LIMIT)
            || was_stalled;
        let due: Vec<usize> = self
            .sent
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.acked && since(now, earliest(r.next_check, now)) >= rto)
            .map(|(i, _)| i)
            .collect();
        for i in due {
            let bits = self.sent[i].bits;
            if self.flow.blocked(bits + RESEND_TEST_EXTRA_BITS) {
                continue;
            }
            self.resend(now, i);
        }
    }

    /// Resend datagram: `[has_ctl][ack if due][1][same rel_seq][same tail_cont]` + the same entries, then the
    /// current unreliable block.
    // SPEC: UDP_TRANSPORT_2009.md §11.3 (engine+0xb3490)
    fn resend(&mut self, now: Millis, i: usize) {
        let mut body = LinkBody::default();
        if !self.flow.blocked(ACK_TEST_COST_BITS) && (self.ack_due || self.hb_due(now)) {
            body.ctl = Some(self.write_ack(now));
        }
        let r = &mut self.sent[i];
        r.next_check = now;
        r.resends += 1;
        let block = RelBlock {
            rel_seq: r.seq,
            tail_cont: r.tail_cont,
            entries: r.entries.clone(),
        };
        let bits = 1 + body.ctl.as_ref().map_or(0, ctl_bits) + 1 + rel_block_bits(&block.entries);
        body.rel = Some(block);
        body.unrel = self.build_unrel(now, bits);
        self.stats.resends += 1;
        let encoded = body.encode_bits();
        self.link_send(&encoded, open_end(&body));
    }

    /// Link send: `cost = round8(body_bits + 8) + 224`; blocked -> silently dropped (reliable data is recovered by
    /// resend, unreliable data is lost); else the cost is added and the datagram goes out with its check byte.
    // SPEC: UDP_TRANSPORT_2009.md §11.6 (engine+0xb1440)
    /// With a move trailer (NET-24) the body's framing is closed as `end` says and the trailer follows it.
    fn link_send(&mut self, body: &BitWriter, end: OpenEnd) -> bool {
        let bits = body.bit_len() as u32;
        let cost = datagram_cost_bits(bits + self.trailer_bits());
        if self.flow.blocked(cost) {
            self.stats.flow_drops += 1;
            return false;
        }
        self.flow.add(cost);
        self.stats.datagrams_out += 1;
        self.stats.max_body_bits = self.stats.max_body_bits.max(bits);
        let datagram = match &self.move_trailer {
            Some(s) => seal_with_trailer(body, end, s),
            None => seal_link_body(body),
        };
        self.stats.max_datagram_bytes = self.stats.max_datagram_bytes.max(datagram.len() as u32);
        if datagram.len() > MAX_DATAGRAM_BYTES {
            self.stats.oversize_out += 1;
        }
        self.events.push_back(LinkEvent::Transmit(datagram));
        true
    }
}

impl Link {
    /// Body bits of a datagram at the cap: less the trailer's room when this side sends one (NET-24).
    fn cap_body_bits(&self) -> u32 {
        CAP_BODY_BITS - self.trailer_bits()
    }

    /// Room a move trailer takes in a datagram (closed framing, padding and the trailer); 0 without one.
    fn trailer_bits(&self) -> u32 {
        if self.move_trailer.is_some() {
            TRAILER_EXTRA_BYTES as u32 * 8
        } else {
            0
        }
    }
}

/// Where the framing of a pump or resend body stops (NET-24).
fn open_end(body: &LinkBody) -> OpenEnd {
    if body.unrel.iter().any(|e| !e.is_empty()) {
        OpenEnd::InUnrel
    } else {
        OpenEnd::AfterRel
    }
}

/// Largest part length `p` (1 <= p < rem) with `round8(used + vlen(p) + p + 8) + 224 <= b_rel` and
/// `vlen(p) + p <= cap_room` (the room left under the datagram cap, NET-10).
fn largest_fit(used: u32, rem: u32, b_rel: u32, cap_room: u32) -> Option<u32> {
    let room = b_rel.checked_sub(DATAGRAM_HEADER_COST_BITS)? & !7;
    let cap = room.checked_sub(used + CHECK_BITS)?.min(cap_room);
    let mut p = cap.min(rem.saturating_sub(1));
    while p > 0 && p + vlen_bits(p) > cap {
        p -= 1;
    }
    (p > 0).then_some(p)
}

/// Writes a control block (dc-wire keeps its writer private; the layout is §7).
// SPEC: UDP_TRANSPORT_2009.md §7
fn write_ctl(w: &mut BitWriter, c: &CtlBlock) {
    let body = LinkBody {
        ctl: Some(c.clone()),
        rel: None,
        unrel: Vec::new(),
    }
    .encode_bits();
    // LinkBody writes [has_ctl=1][ctl][has_rel=0]; copy the ctl bits only.
    let n = ctl_bits(c) as usize;
    w.append(&slice_bits(&body, 1, n));
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::decode_link_datagram;

    fn msg(n: usize, seed: u8) -> BitWriter {
        let bytes: Vec<u8> = (0..n.div_ceil(8))
            .map(|i| seed.wrapping_add((i as u8).wrapping_mul(37)))
            .collect();
        BitWriter::from_bits(&bytes, n)
    }

    fn transmits(ev: &[LinkEvent]) -> Vec<Vec<u8>> {
        ev.iter()
            .filter_map(|e| match e {
                LinkEvent::Transmit(d) => Some(d.clone()),
                _ => None,
            })
            .collect()
    }

    fn messages(ev: &[LinkEvent]) -> Vec<(bool, BitWriter)> {
        ev.iter()
            .filter_map(|e| match e {
                LinkEvent::Message { reliable, bits } => Some((*reliable, bits.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn first_block_is_seq_one_and_increments() {
        let mut a = Link::new(0, LinkConfig::default());
        a.send_reliable(msg(40, 1));
        let d = transmits(&a.poll(0));
        assert_eq!(d.len(), 1);
        let body = decode_link_datagram(&d[0]).unwrap();
        let rel = body.rel.unwrap();
        assert_eq!(rel.rel_seq, 1);
        assert_eq!(rel.entries, vec![msg(40, 1)]);
        a.send_reliable(msg(41, 2));
        let d = transmits(&a.poll(1));
        assert_eq!(decode_link_datagram(&d[0]).unwrap().rel.unwrap().rel_seq, 2);
    }

    /// NET-27: a peer that keeps one reliable message open forever is closed once it passes the cap; the buffer
    /// never holds more than the cap.
    #[test]
    fn an_endless_split_message_closes_the_link() {
        let mut a = Link::new(0, LinkConfig::default());
        let mut b = Link::new(0, LinkConfig::default());
        let part = MAX_ASSEMBLY_BITS / 4;
        for seq in 1..=6u16 {
            b.on_rel_block(RelBlock {
                rel_seq: seq,
                tail_cont: true,
                entries: vec![msg(part, seq as u8)],
            });
            assert!(b.assembly.as_ref().map_or(0, |x| x.bit_len()) <= MAX_ASSEMBLY_BITS);
        }
        assert!(b.assembly_overflow && b.assembly.is_none());
        assert!(
            messages(&b.poll(10)).is_empty(),
            "nothing half-joined is delivered"
        );
        // the next good datagram from the peer closes the link
        a.send_reliable(msg(40, 1));
        let d = transmits(&a.poll(20));
        b.handle_datagram(30, &d[0]);
        assert!(!b.is_open());
        assert!(b.poll(40).iter().any(|e| matches!(
            e,
            LinkEvent::Closed {
                cause: CloseCause::Local,
                ..
            }
        )));
    }

    /// NET-28: a peer that never acks while it keeps asking cannot grow this side's reliable queue without end.
    #[test]
    fn a_full_reliable_queue_closes_the_link() {
        let mut a = Link::new(0, LinkConfig::default());
        // nothing is acked and nothing is pumped: every message stays queued
        for i in 0..MAX_QUEUED_RELIABLE {
            assert!(a.send_reliable(msg(8, i as u8)));
        }
        assert!(!a.send_reliable(msg(8, 0)), "refused once full");
        assert_eq!(a.queued_reliable(), MAX_QUEUED_RELIABLE);
        let ev = a.poll(10);
        assert!(!a.is_open());
        assert!(ev.iter().any(|e| matches!(
            e,
            LinkEvent::Closed {
                cause: CloseCause::Local,
                ..
            }
        )));
        assert_eq!(a.queued_reliable(), 0, "the queue is let go");
    }

    #[test]
    fn sequence_gaps_count_skipped_block_numbers() {
        let mut a = Link::new(0, LinkConfig::default());
        let mut b = Link::new(0, LinkConfig::default());
        let mut sent = Vec::new();
        for i in 0..5u8 {
            a.send_reliable(msg(40, i));
            sent.extend(transmits(&a.poll(i as u32 * 20)));
        }
        assert_eq!(sent.len(), 5);
        // blocks 1, 3, 5 arrive, then 2 late; 4 never
        for i in [0usize, 2, 4, 1] {
            b.handle_datagram(200, &sent[i]);
        }
        let st = b.stats();
        assert_eq!(st.rel_seq_span, 5);
        assert_eq!(st.rel_seq_gaps, 2);
    }

    #[test]
    fn bye_codes_on_receive() {
        for (hosting, ctl, code) in [
            (true, CtlBlock::Bye, Some(0)),
            (false, CtlBlock::Bye, Some(3)),
            (false, CtlBlock::ByeCode(1), Some(3)),
            (true, CtlBlock::ByeCode(6), Some(6)),
            (true, CtlBlock::ByeCode(0), None),
        ] {
            let mut l = Link::new(
                0,
                LinkConfig {
                    hosting,
                    ..LinkConfig::default()
                },
            );
            let d = dc_wire::udp::encode_link_datagram(&LinkBody {
                ctl: Some(ctl.clone()),
                rel: None,
                unrel: vec![],
            });
            l.handle_datagram(5, &d);
            let ev = l.poll(5);
            let closed = ev.iter().find_map(|e| match e {
                LinkEvent::Closed { code, cause } => Some((*code, *cause)),
                _ => None,
            });
            assert_eq!(closed.map(|c| c.0), code, "{hosting} {ctl:?}");
            if code.is_some() {
                assert_eq!(closed.unwrap().1, CloseCause::PeerBye);
                assert!(l.is_finished());
            }
        }
    }

    #[test]
    fn corrupt_datagram_is_dropped_and_is_no_liveness() {
        // NET-26: the 2009 receiver stamps liveness before the check byte; ours only for a good datagram
        let mut a = Link::new(0, LinkConfig::default());
        a.send_reliable(msg(64, 3));
        let mut d = transmits(&a.poll(0)).remove(0);
        d[0] ^= 0x5a;
        let mut b = Link::new(0, LinkConfig::default());
        b.handle_datagram(50_000, &d);
        assert_eq!(b.stats().bad_check, 1);
        assert_eq!(
            b.last_recv(),
            0,
            "junk does not refresh the last receive time"
        );
        assert_eq!(b.traffic_stamp(), 0);
        assert!(messages(&b.poll(50_000)).is_empty());
        d[0] ^= 0x5a;
        b.handle_datagram(50_010, &d);
        assert_eq!(b.last_recv(), 50_010);
    }

    #[test]
    fn idle_drop_sends_four_coded_byes_100ms_apart() {
        let mut a = Link::new(0, LinkConfig::default());
        let mut byes = Vec::new();
        let mut closed = None;
        let mut t = 0;
        while t < 91_000 {
            for e in a.poll(t) {
                match e {
                    LinkEvent::Transmit(d) => {
                        let b = decode_link_datagram(&d).unwrap();
                        if let Some(CtlBlock::ByeCode(c)) = b.ctl {
                            byes.push((t, c));
                        }
                    }
                    LinkEvent::Closed { code, cause } => closed = Some((t, code, cause)),
                    _ => {}
                }
            }
            t += 10;
        }
        let (t0, code, cause) = closed.expect("closed");
        assert_eq!((code, cause), (3, CloseCause::IdleTimeout));
        assert_eq!(t0, 90_010, "drop when now - last_recv >= 90001");
        assert_eq!(byes.len(), 4);
        for (i, (t, c)) in byes.iter().enumerate() {
            assert_eq!(*c, 3);
            assert_eq!(*t, t0 + 100 * i as u32);
        }
        assert!(a.is_finished());
    }

    #[test]
    fn keepalive_is_ack_plus_empty_reliable_block_every_200ms() {
        let mut a = Link::new(0, LinkConfig::default());
        let mut b = Link::new(0, LinkConfig::default());
        let mut a_sends = Vec::new();
        for t in 0..3000u32 {
            for e in a.poll(t) {
                if let LinkEvent::Transmit(d) = e {
                    let body = decode_link_datagram(&d).unwrap();
                    a_sends.push((t, body));
                    b.handle_datagram(t, &d);
                }
            }
            for e in b.poll(t) {
                if let LinkEvent::Transmit(d) = e {
                    a.handle_datagram(t, &d);
                }
            }
        }
        // ignore the immediate acks a sends in answer to b's keep-alive blocks; look at a's own keep-alives
        let keepalives: Vec<u32> = a_sends
            .iter()
            .filter(|(_, b)| {
                b.rel
                    .as_ref()
                    .is_some_and(|r| r.entries.iter().all(|e| e.is_empty()))
            })
            .map(|(t, _)| *t)
            .collect();
        assert!(keepalives.len() >= 10, "{keepalives:?}");
        for w in keepalives.windows(2) {
            assert!(w[1] - w[0] >= 200, "{keepalives:?}");
            assert!(w[1] - w[0] <= 210, "{keepalives:?}");
        }
        assert!(a_sends
            .iter()
            .all(|(_, b)| matches!(b.ctl, Some(CtlBlock::Ack { .. }))));
        assert!(a.latency().sample_count() >= 10);
        assert!(a.is_idle());
    }

    #[test]
    fn resend_keeps_seq_and_follows_rto() {
        let mut a = Link::new(0, LinkConfig::default());
        a.send_reliable(msg(100, 7));
        let mut times = Vec::new();
        for t in 0..6000u32 {
            for d in transmits(&a.poll(t)) {
                let body = decode_link_datagram(&d).unwrap();
                if let Some(r) = body.rel {
                    if r.rel_seq == 1 {
                        times.push(t);
                    }
                }
            }
        }
        assert!(times.len() >= 4, "{times:?}");
        assert_eq!(times[0], 0);
        // first resend not before 2 * RTO(200) = 400 ms; L_eff grows while waiting, RTO clamps at 1000
        assert!(times[1] >= 400, "{times:?}");
        for w in times.windows(2).skip(1) {
            assert!(
                w[1] - w[0] >= RTO_MIN_MS && w[1] - w[0] <= 1000,
                "{times:?}"
            );
        }
    }

    #[test]
    fn pause_above_eight_outstanding() {
        let mut a = Link::new(0, LinkConfig::default());
        // nine messages that each need a block of their own (8000 bits, split threshold irrelevant here)
        for i in 0..12 {
            a.send_reliable(msg(8000, i));
        }
        let first = transmits(&a.poll(0));
        assert!(a.outstanding() > 8, "{}", a.outstanding());
        let max_first = a.newest_block_seq();
        for i in 0..3 {
            a.send_reliable(msg(50, 100 + i));
        }
        // the first resend comes after about 1200 ms (L_eff grows while waiting; RTO clamps at 1000)
        let seqs_after: Vec<u16> = (1..2500u32)
            .flat_map(|t| transmits(&a.poll(t)))
            .filter_map(|d| decode_link_datagram(&d).unwrap().rel.map(|r| r.rel_seq))
            .collect();
        assert!(a.is_paused());
        assert_eq!(a.queued_reliable(), 3);
        assert!(!seqs_after.is_empty(), "resends of the outstanding blocks");
        assert!(seqs_after.iter().all(|&s| s <= max_first), "{seqs_after:?}");
        assert!(!first.is_empty());
    }

    #[test]
    fn ctl_writer_matches_layout() {
        let mut w = BitWriter::new();
        write_ctl(&mut w, &CtlBlock::ByeCode(0xdeadbeef));
        let mut r = w.reader();
        assert_eq!(r.read_bits(4), 3);
        assert_eq!(r.read_u32(), 0xdeadbeef);
        assert!(r.at_end());
    }

    #[test]
    fn largest_fit_respects_the_budget() {
        for b_rel in [600u32, 3584, 8800] {
            for used in [16u32, 100, 300] {
                if let Some(p) = largest_fit(used, 1_000_000, b_rel, u32::MAX) {
                    assert!(round8(used + vlen_bits(p) + p + 8) + 224 <= b_rel);
                    assert!(round8(used + vlen_bits(p + 1) + p + 1 + 8) + 224 > b_rel);
                }
            }
        }
    }
}
