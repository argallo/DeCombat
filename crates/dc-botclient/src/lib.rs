//! `dc-botclient`: a headless client for the 2009 Combat Arms protocol (no rendering, no window).
//!
//! It stands in for a player in tests and is the black-box transport reference: it speaks the 2009 lobby TCP
//! protocol (`rust/specs/PROTOCOL_2009.md`) and the UDP game session (`UDP_TRANSPORT_2009.md`,
//! `UDP_SESSION_2009_A.md` / `_B.md` with their disasm addenda, `GAME_MESSAGES_2009_C2S.md` / `_S2C.md`) the way
//! the 2009 client does, from the client side, and records everything it sends and receives.
//!
//! - [`lobby`]: auth login with the `LOCAL:<id>` token the harness uses, nickname creation, server pick, lobby
//!   login, channel, room create / enter, ready / start, and the match address push 0x3043.
//! - [`session`]: [`session::BotSession`], the sans-IO UDP game client on `dc_net_engine::Joiner` / `Link`: the
//!   out-of-band join, the start-up steps S1-S12 (client side), the player-info answer 0x72, the world-ready
//!   report 0x87, player updates 0x64, and an observation record of every message.
//! - [`objects`]: readers for the object group (id 8) and the object stream (id 10).
//! - [`shell`]: shell-message readers (server -> client) and writers (client -> server).
//! - [`motion`]: the body the bot moves with `dc-sim` (stand still, or walk a small circle).
//! - [`romworld`]: optional ROM access: does the announced world name resolve (the client's 0x29 check), the
//!   world box for `pos48`, the collision world.
//! - [`record`]: the captap-format capture (format 1) and the JSONL event log.
//! - [`run`]: the socket loop around [`session::BotSession`] and the whole lobby-to-game run of one bot.
//!
//! Every value the 2009 client derives from data it does not have here (its profile, its look record) is a
//! DeCombat choice for the bot, documented where it is made (proposal rows `BOT-n`, staged by the botclient task).

// the session report is one large `json!` literal
#![recursion_limit = "512"]

pub mod bombrun;
pub mod capdecode;
pub mod clock;
pub mod control;
pub mod doors;
pub mod evade;
pub mod flagrun;
// DC-DEFAULT: BOT-75 (proposed) Spy Hunt: the nearest intel case, then the uplink
pub mod fx;
pub mod hostroom;
pub mod hunt;
pub mod lifecycle;
pub mod lobby;
pub mod logring;
pub mod motion;
pub mod nav;
pub mod objects;
pub mod record;
pub mod roles;
pub mod romworld;
pub mod run;
pub mod session;
pub mod shell;
pub mod shooter;
pub mod shotcycle;
pub mod skill;
pub mod spyrun;
pub mod squad;
pub mod swarm;
pub mod timeline;
pub mod tls;
pub mod weapon;

use std::fmt;

/// Error of a bot run: what failed, in words (the run report keeps the detail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotError(pub String);

impl fmt::Display for BotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BotError {}

impl From<std::io::Error> for BotError {
    fn from(e: std::io::Error) -> Self {
        BotError(format!("io: {e}"))
    }
}

/// Shorthand for results of this crate.
pub type BotResult<T> = Result<T, BotError>;

/// Builds a [`BotError`] from format arguments.
#[macro_export]
macro_rules! bot_err {
    ($($t:tt)*) => { $crate::BotError(format!($($t)*)) };
}
