//! `dc-net-engine`: the delivery state machine of the 2009 game-connection UDP transport.
//!
//! Written only from `rust/specs/UDP_TRANSPORT_2009.md` (names from `GLOSSARY_UDP.md`) on top of the wire
//! primitives in `dc_wire::udp` (bit stream, datagram codecs, constants). Everything is sans-IO and deterministic:
//! callers inject the time (`u32` milliseconds) and move datagrams in and out themselves.
//!
//! - [`Link`]: one peer. Reliable delivery with 13-bit sequences, cumulative ack + 8-bit selective mask, 8
//!   out-of-order slots, split messages, resend with `RTO = clamp(L_ms, 17, 1000)`, pause above 8 outstanding
//!   blocks, trickle mode, keep-alive every `max(200, L_ms)` ms, unreliable block with its lifetime, the 8800-bit
//!   body budget, the flow token bucket, the 90 s idle drop with bye code 3 and the bye sent 4 times 100 ms apart.
//!   DeCombat bounds on top (NET-10..12): datagrams of at most 548 bytes, unreliable data while reliable blocks
//!   wait, an ack heartbeat at least every second.
//! - [`Host`]: the out-of-band side of a listening socket (info probe, join, echo) and the link table.
//! - [`Joiner`]: the client side of the join (ask every 300 ms, 10 s timeout).
//! - [`sim`]: a seeded lossy / reordering channel for tests.
//! - [`impair`] and [`shim`]: the network lab (docs/NETLAB.md): named bad-path profiles (loss bursts, latency,
//!   jitter, reordering, size limits, policers, blackouts, NAT rebinding, firewall flow kills) around a real client
//!   socket.
//! - [`socket_reader`]: the one piece of I/O here, a receive thread for a UDP socket (a receive timeout on the
//!   socket loses datagrams on Windows under load).
//!
//! Spec items marked UNPROVEN are named constants (`TODO_*` here or in `dc_wire::udp`) with a `// SPEC` cite.

pub mod body;
pub mod flow;
pub mod host;
pub mod impair;
pub mod join;
pub mod latency;
pub mod link;
pub mod shim;
pub mod sim;
pub mod socket_reader;
pub mod time;

pub use host::{Host, HostConfig, HostEvent, HostStats, APP_ID_BIG_ENDIAN_CANDIDATE};
pub use join::{JoinStatus, Joiner};
pub use link::{Bye, CloseCause, Link, LinkConfig, LinkEvent, LinkState, LinkStats};
pub use time::Millis;
