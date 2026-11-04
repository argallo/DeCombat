//! The lobby connection between the menu and the game session: the menu's connection thread hands it over when
//! the game server's address arrives; the network session ([`crate::net`]) keeps it read during the match
//! (its lobby keeper) and gives it back when the session ends, and the menu's waiting room opens again on it.
// SPEC: PROTOCOL_2009.md §1.1 (the lobby connection stays up during a match), §7 (0x3043, then the game join)
// OBSERVED: PROTOCOL_2009.md §7.1 (after the match the client is in the waiting room on the same lobby connection)

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use dc_botclient::hostroom;
use dc_botclient::lobby::Lobby;
use dc_botclient::record::Recorder;
use dc_botclient::romworld::RomWorlds;
use dc_botclient::run::player_info_from_lobby;
use dc_botclient::session::GameConfig;

use crate::matchexit::MatchResult;
use crate::net::{session_config, LobbyKeeper, NetOptions};

enum Slot {
    Empty,
    Ready {
        server: SocketAddr,
        lobby: Box<Lobby>,
        mode: i32,
    },
    Back(Box<Lobby>),
}

/// The handed-over lobby connection (shared between the menu and one network session), and the match's result
/// that comes back with it ([`crate::matchexit`]).
#[derive(Clone)]
pub struct LobbyHandoff(Arc<Mutex<Slot>>, Arc<Mutex<Option<MatchResult>>>);

impl std::fmt::Debug for LobbyHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match &*self.lock() {
            Slot::Empty => "empty",
            Slot::Ready { .. } => "ready",
            Slot::Back(_) => "back",
        };
        write!(f, "LobbyHandoff({what})")
    }
}

impl LobbyHandoff {
    /// The connection for the game server at `server`; `mode` is the room's mode row.
    pub fn new(server: SocketAddr, lobby: Lobby, mode: i32) -> LobbyHandoff {
        LobbyHandoff(
            Arc::new(Mutex::new(Slot::Ready {
                server,
                lobby: Box::new(lobby),
                mode,
            })),
            Arc::new(Mutex::new(None)),
        )
    }

    /// The network session leaves the match's result (`None`: no result screen came).
    pub fn set_result(&self, r: Option<MatchResult>) {
        *self.1.lock().unwrap_or_else(|p| p.into_inner()) = r;
    }

    /// The menu takes the match's result (once).
    pub fn take_result(&self) -> Option<MatchResult> {
        self.1.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The network session takes the connection (once).
    pub fn take_ready(&self) -> Option<(SocketAddr, Lobby, i32)> {
        let mut g = self.lock();
        match std::mem::replace(&mut *g, Slot::Empty) {
            Slot::Ready {
                server,
                lobby,
                mode,
            } => Some((server, *lobby, mode)),
            other => {
                *g = other;
                None
            }
        }
    }

    /// The network session gives the connection back when it ends.
    pub fn give_back(&self, lobby: Lobby) {
        *self.lock() = Slot::Back(Box::new(lobby));
    }

    /// The menu takes the connection back after the session (`None` while the session still has it, or when it
    /// never started).
    pub fn take_back(&self) -> Option<Lobby> {
        let mut g = self.lock();
        match std::mem::replace(&mut *g, Slot::Empty) {
            Slot::Back(l) => Some(*l),
            Slot::Ready { lobby, .. } => Some(*lobby),
            Slot::Empty => None,
        }
    }
}

/// The game session's start from a handed-over connection: its configuration (the room's mode class from the
/// ROM mode row, the player info of the lobby login), the lobby keeper and the equipment slots.
// SPEC: GAME_MESSAGES_2009_C2S.md Appendix A step A0 (mode class from the room's mode row); PROTOCOL_2009.md §3.3
// (0x323 slots), §7 (player info: room slot, character number, session index)
pub fn session_from(
    opts: &NetOptions,
    rec: &Recorder,
    rom: Option<&RomWorlds>,
    rom_dir: Option<&Path>,
    lobby: Lobby,
    mode: i32,
) -> (GameConfig, LobbyKeeper, Vec<i32>) {
    let mode_class = rom_dir.and_then(|d| hostroom::rom_mode_class(d, mode.max(0) as usize));
    rec.event(
        "mode_class_hint",
        serde_json::json!({"mode_row": mode, "mode_class": mode_class, "from": "menu"}),
    );
    let mut gc = session_config(opts, &lobby.profile.nick, rom, mode_class);
    gc.info = player_info_from_lobby(&lobby);
    let mut lobby = lobby;
    // DC-DEFAULT: LG-327 (proposed, project decision) one match, one ticket
    gc.seat_ticket = lobby.seat_ticket.take();
    let slots = lobby.profile.equipped.clone();
    (gc, LobbyKeeper::start(lobby), slots)
}

/// The options of the game session for a handed-over connection: `base` (log, capture, ROM, waits) with the
/// handoff, the character's nickname and the login ID typed on the login screen (the game hello carries it).
// SPEC: PROTOCOL_2009.md §7 (the hello blob carries the login name)
pub fn game_options(
    base: &NetOptions,
    handoff: LobbyHandoff,
    nick: &str,
    login: &str,
) -> NetOptions {
    let mut o = base.clone();
    if !nick.is_empty() {
        o.name = nick.to_string();
    }
    if !login.is_empty() {
        o.user = Some(login.to_string());
    }
    o.handoff = Some(handoff);
    o
}
