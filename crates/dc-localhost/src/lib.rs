//! `dc-localhost`: local matches. One match description ([`LocalMatch`]: world, mode, the player's seat, bots,
//! practice options, how the match is reached) and the pure builders that turn it into what a game server needs: its
//! launch line ([`LocalMatch::server_args`]) and the control lines it reads on stdin ([`LocalMatch::pipe_lines`]: one
//! `room` line, then one `user` line per seat). Nothing here starts a process or opens a socket.
//!
//! The lines follow the pipe contract of the dedicated server (`specs/DSERVER_PIPE.md` §2 launch line, §3.1 `user`,
//! §3.2 `room`); every seat carries its own seat ticket (LG-327), so nobody else can take the player's or a bot's seat.
//! The crate needs no 2009 game file: it is an engine crate (`tools/engine_crates.txt`).
// SPEC: DSERVER_PIPE.md §2, §3.1, §3.2
// DC-DEFAULT: LOC-1 (proposed) the match description and its seat limits

use std::fmt;
use std::path::PathBuf;

use dc_wire::seatticket::{self, Ticket};
use serde::{Deserialize, Serialize};

/// Most players a match hosted on a player's own PC takes (the host included): a home connection's upload is the limit.
// DC-DEFAULT: LOC-1 (proposed; owner decision) 8
pub const HOME_HOST_MAX_PLAYERS: u8 = 8;
/// Most seats of a match nobody else can join (the player and the bots on one PC).
// DC-DEFAULT: LOC-1 (proposed) 16
pub const SOLO_MAX_PLAYERS: u8 = 16;
/// Bot skill levels as the bot client reads them (`--skill 0..100`); the standard level.
pub const BOT_SKILL_MAX: u8 = 100;
pub const BOT_SKILL_STANDARD: u8 = 50;
/// Longest player name: the hello carries at most 63 UTF-16 units of it.
// SPEC: UDP_SESSION_2009_A.md §5.2 (the hello blob's name fields)
pub const NAME_MAX_CHARS: usize = 63;
/// Longest match time and the tick rates the server takes (`-tick 1..240`).
pub const TIME_MAX_SECS: u32 = 24 * 3600;
pub const TICK_MAX_HZ: u32 = 240;
/// Most entries of one loadout.
pub const LOADOUT_MAX: usize = 16;
/// The game server's variable for the join secret (the server's `cli::JOIN_SECRET_ENV`).
// DC-DEFAULT: LOC-11 (proposed)
pub const JOIN_SECRET_ENV: &str = "DC_JOIN_SECRET";
/// The user ids of the seats: the player is 1, the bots follow.
pub const PLAYER_UID: u32 = 1;

/// The match description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalMatch {
    pub world: World,
    /// `-rom`: the ROM folder (the server's default is `CA_ROM`).
    #[serde(default)]
    pub rom: Option<PathBuf>,
    /// `-content`: a weapon content document applied at the start.
    #[serde(default)]
    pub content: Option<PathBuf>,
    pub mode: Mode,
    pub player: Seat,
    #[serde(default)]
    pub bots: Bots,
    #[serde(default)]
    pub practice: Practice,
    /// `-loadout`: the weapons (weapon table indices) of a seat whose line names none.
    #[serde(default)]
    pub server_loadout: Vec<u16>,
    /// `-tick`: world ticks per second (the server's default without it).
    #[serde(default)]
    pub tick_hz: Option<u32>,
    #[serde(default)]
    pub hosting: Hosting,
    /// `-idle`: the server stops after this long with no client.
    #[serde(default)]
    pub idle_secs: Option<u32>,
    /// `-teamempty`: how long a team may stay empty before the round ends.
    #[serde(default)]
    pub team_empty_secs: Option<u32>,
    /// `-log`: the server's log file.
    #[serde(default)]
    pub log: Option<PathBuf>,
    /// `-verbose`: every refused hit claim in the log.
    #[serde(default)]
    pub verbose: bool,
}

/// Where the world comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum World {
    /// `-world <name>`: a world of the ROM, in the lobby's form (`Worlds\<name>`).
    Rom { name: String },
    /// `-worldfile <path>`: a world file on disk; `announce` is the path the clients are told (`-announce`).
    File {
        path: PathBuf,
        #[serde(default)]
        announce: Option<String>,
    },
}

/// The game mode and its limits (the `room` line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    /// `key=`: the mode's key (`TDM`, `FFA`, ...).
    pub key: String,
    /// `type=`: the mode's variant number.
    #[serde(rename = "type", default)]
    pub kind: u32,
    /// `goal=`: the score that ends the round.
    pub goal: u32,
    /// `time=` and `-roundsecs`: the match time in seconds.
    pub time_secs: u32,
    /// `ff=`: friendly fire.
    #[serde(default)]
    pub friendly_fire: bool,
    /// Two teams (the seats alternate between them); `false` = everyone on team 0 (free for all).
    #[serde(default = "yes")]
    pub teams: bool,
}

fn yes() -> bool {
    true
}

/// The player's seat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seat {
    pub name: String,
    /// `loadout=`: what the seat carries; empty = the server's loadout.
    #[serde(default)]
    pub loadout: Vec<LoadoutItem>,
}

/// One loadout entry: a ROM item number in an equipment slot, or a bare weapon table index without a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadoutItem {
    #[serde(default)]
    pub slot: Option<u8>,
    pub item: u32,
}

/// The bots of the match. Their names are `<prefix><n>`, n from 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bots {
    #[serde(default)]
    pub count: u8,
    /// Skill level 0..=100, as the bot client's `--skill` reads it.
    #[serde(default = "standard_skill")]
    pub skill: u8,
    #[serde(default = "bot_prefix")]
    pub name_prefix: String,
}

fn standard_skill() -> u8 {
    BOT_SKILL_STANDARD
}

fn bot_prefix() -> String {
    "Bot".into()
}

impl Default for Bots {
    fn default() -> Self {
        Bots {
            count: 0,
            skill: BOT_SKILL_STANDARD,
            name_prefix: bot_prefix(),
        }
    }
}

/// Practice options the game server has (`-combat` keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Practice {
    /// `-combat cone_check=off`: no shot-direction check.
    #[serde(default)]
    pub cone_check_off: bool,
}

/// How the match is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Hosting {
    /// Nothing listens outside this PC: any free port on the loopback address.
    #[default]
    Solo,
    /// Others may join: a fixed port on every address, at most `max_players` seats (the host included).
    Host { port: u16, max_players: u8 },
}

/// The seat tickets of one match: the player's and one per bot, each 128 random bits the caller made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tickets {
    pub player: Ticket,
    pub bots: Vec<Ticket>,
}

/// Why a description cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError(pub String);

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SpecError {}

fn bad<T>(s: impl Into<String>) -> Result<T, SpecError> {
    Err(SpecError(s.into()))
}

/// A name for a `user` line and a hello: non-empty, at most [`NAME_MAX_CHARS`], no whitespace or control character.
fn check_name(what: &str, n: &str) -> Result<(), SpecError> {
    if n.is_empty() {
        return bad(format!("{what}: empty"));
    }
    if n.chars().count() > NAME_MAX_CHARS {
        return bad(format!("{what}: longer than {NAME_MAX_CHARS} characters"));
    }
    if n.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return bad(format!("{what}: {n:?} has a space or a control character"));
    }
    Ok(())
}

fn check_path(what: &str, p: &std::path::Path) -> Result<(), SpecError> {
    if p.as_os_str().is_empty() {
        return bad(format!("{what}: empty path"));
    }
    Ok(())
}

impl LocalMatch {
    /// Most seats this match may have: the home-host cap or the solo cap.
    pub fn seat_cap(&self) -> u8 {
        match self.hosting {
            Hosting::Solo => SOLO_MAX_PLAYERS,
            Hosting::Host { max_players, .. } => max_players,
        }
    }

    /// Checks the description; the builders call it first.
    // DC-DEFAULT: LOC-1 (proposed)
    pub fn validate(&self) -> Result<(), SpecError> {
        match &self.world {
            World::Rom { name } => {
                if name.trim().is_empty() {
                    return bad("world: empty name");
                }
            }
            World::File { path, announce } => {
                check_path("world file", path)?;
                if announce.as_deref().is_some_and(|a| a.trim().is_empty()) {
                    return bad("world: empty announce path");
                }
            }
        }
        if let Some(p) = &self.rom {
            check_path("rom", p)?;
        }
        if let Some(p) = &self.content {
            check_path("content", p)?;
        }
        if let Some(p) = &self.log {
            check_path("log", p)?;
        }
        let m = &self.mode;
        if m.key.is_empty() || !m.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return bad(format!("mode key {:?}: letters, digits and _ only", m.key));
        }
        if m.goal == 0 {
            return bad("mode goal: 0");
        }
        if m.time_secs == 0 || m.time_secs > TIME_MAX_SECS {
            return bad(format!("mode time: 1..={TIME_MAX_SECS} seconds"));
        }
        check_name("player name", &self.player.name)?;
        if self.player.loadout.len() > LOADOUT_MAX || self.server_loadout.len() > LOADOUT_MAX {
            return bad(format!("loadout: more than {LOADOUT_MAX} entries"));
        }
        if self.bots.skill > BOT_SKILL_MAX {
            return bad(format!("bot skill: 0..={BOT_SKILL_MAX}"));
        }
        if self.bots.count > 0 {
            check_name("bot name prefix", &self.bots.name_prefix)?;
            for n in self.bot_names() {
                check_name("bot name", &n)?;
                if n.eq_ignore_ascii_case(&self.player.name) {
                    return bad(format!("bot name {n:?} is the player's name"));
                }
            }
        }
        if let Some(hz) = self.tick_hz {
            if hz == 0 || hz > TICK_MAX_HZ {
                return bad(format!("tick: 1..={TICK_MAX_HZ}"));
            }
        }
        if let Hosting::Host { port, max_players } = self.hosting {
            if port == 0 {
                return bad("host port: 0 (a hosted match needs a fixed port to forward)");
            }
            if !(2..=HOME_HOST_MAX_PLAYERS).contains(&max_players) {
                return bad(format!(
                    "max players: 2..={HOME_HOST_MAX_PLAYERS} on a home host"
                ));
            }
        }
        let seats = 1 + u32::from(self.bots.count);
        if seats > u32::from(self.seat_cap()) {
            return bad(format!(
                "{seats} seats (you and {} bots): more than {}",
                self.bots.count,
                self.seat_cap()
            ));
        }
        Ok(())
    }

    /// The bots' names, in seat order.
    pub fn bot_names(&self) -> Vec<String> {
        (1..=self.bots.count)
            .map(|n| format!("{}{n}", self.bots.name_prefix))
            .collect()
    }

    /// The game server's environment beyond the caller's: a hosted match needs the join secret the caller made for it
    /// (LOC-2), passed as [`JOIN_SECRET_ENV`] so it never shows in a process list (LOC-11); a solo match takes none.
    // DC-DEFAULT: LOC-11 (proposed)
    pub fn server_env(
        &self,
        join_secret: Option<&Ticket>,
    ) -> Result<Vec<(String, String)>, SpecError> {
        self.validate()?;
        match (self.hosting, join_secret) {
            (Hosting::Solo, Some(_)) => bad("a solo match takes no join secret"),
            (Hosting::Host { .. }, None) => bad("a hosted match needs a join secret"),
            (_, Some(t)) => Ok(vec![(JOIN_SECRET_ENV.to_string(), seatticket::to_hex(t))]),
            (_, None) => Ok(Vec::new()),
        }
    }

    /// The game server's launch arguments (after the program name). They carry no secret: the join secret goes in
    /// the environment ([`LocalMatch::server_env`]).
    // SPEC: DSERVER_PIPE.md §2 (launch line; `-port 0` = any free port, reported in `@ready port=`)
    // DC-DEFAULT: LOC-1 (proposed) solo = loopback only; a hosted match binds every address on its fixed port
    pub fn server_args(&self) -> Result<Vec<String>, SpecError> {
        self.validate()?;
        let mut a: Vec<String> = Vec::new();
        let push = |a: &mut Vec<String>, k: &str, v: String| {
            a.push(k.to_string());
            a.push(v);
        };
        match self.hosting {
            Hosting::Solo => {
                push(&mut a, "-port", "0".into());
                push(&mut a, "-bindip", "127.0.0.1".into());
            }
            Hosting::Host { port, .. } => {
                push(&mut a, "-port", port.to_string());
                push(&mut a, "-bindip", "0.0.0.0".into());
            }
        }
        match &self.world {
            World::Rom { name } => push(&mut a, "-world", name.clone()),
            World::File { path, announce } => {
                push(&mut a, "-worldfile", path.display().to_string());
                if let Some(x) = announce {
                    push(&mut a, "-announce", x.clone());
                }
            }
        }
        if let Some(p) = &self.rom {
            push(&mut a, "-rom", p.display().to_string());
        }
        if let Some(s) = self.idle_secs {
            push(&mut a, "-idle", s.to_string());
        }
        push(&mut a, "-roundsecs", self.mode.time_secs.to_string());
        if !self.server_loadout.is_empty() {
            push(&mut a, "-loadout", join(self.server_loadout.iter()));
        }
        if let Some(p) = &self.content {
            push(&mut a, "-content", p.display().to_string());
        }
        if let Some(hz) = self.tick_hz {
            push(&mut a, "-tick", hz.to_string());
        }
        if let Hosting::Host { max_players, .. } = self.hosting {
            push(&mut a, "-maxplayers", max_players.to_string());
        }
        if let Some(p) = &self.log {
            push(&mut a, "-log", p.display().to_string());
        }
        if self.verbose {
            a.push("-verbose".into());
        }
        if let Some(s) = self.team_empty_secs {
            push(&mut a, "-teamempty", s.to_string());
        }
        if self.practice.cone_check_off {
            push(&mut a, "-combat", "cone_check=off".into());
        }
        Ok(a)
    }

    /// The `room` line.
    // SPEC: DSERVER_PIPE.md §3.2
    pub fn room_line(&self) -> String {
        let m = &self.mode;
        format!(
            "room key={} type={} goal={} ff={} time={}",
            m.key,
            m.kind,
            m.goal,
            u8::from(m.friendly_fire),
            m.time_secs
        )
    }

    /// The team (0-based) of seat `i` (0 = the player): alternating in a team mode, all 0 otherwise.
    pub fn team_of(&self, i: usize) -> u8 {
        if self.mode.teams {
            (i % 2) as u8
        } else {
            0
        }
    }

    /// The control lines in the order the server reads them: `room`, the player's `user` line, one per bot. Each
    /// seat's line carries its ticket.
    // SPEC: DSERVER_PIPE.md §3.1 (fields 1-6, then `k=v` tokens), §3.1 seat ticket (LG-327)
    pub fn pipe_lines(&self, tickets: &Tickets) -> Result<Vec<String>, SpecError> {
        self.validate()?;
        if tickets.bots.len() != usize::from(self.bots.count) {
            return bad(format!(
                "{} bot tickets for {} bots",
                tickets.bots.len(),
                self.bots.count
            ));
        }
        let mut seats: Vec<String> = Vec::with_capacity(tickets.bots.len() + 2);
        seats.push(self.room_line());
        let loadout = (!self.player.loadout.is_empty()).then(|| {
            join(self.player.loadout.iter().map(|x| match x.slot {
                Some(s) => format!("{s}:{}", x.item),
                None => x.item.to_string(),
            }))
        });
        seats.push(user_line(
            PLAYER_UID,
            self.team_of(0),
            &self.player.name,
            loadout.as_deref(),
            &tickets.player,
        ));
        for (i, (name, t)) in self.bot_names().iter().zip(&tickets.bots).enumerate() {
            seats.push(user_line(
                PLAYER_UID + 1 + i as u32,
                self.team_of(i + 1),
                name,
                None,
                t,
            ));
        }
        Ok(seats)
    }
}

/// `user <uid> <sid> <key> <team> <exp> <nick> [loadout=..] ticket=<hex>`: sid and key are the uid (the server reads
/// neither), the experience 0.
fn user_line(uid: u32, team: u8, nick: &str, loadout: Option<&str>, t: &Ticket) -> String {
    let mut l = format!("user {uid} {uid} {uid} {team} 0 {nick}");
    if let Some(x) = loadout {
        l.push_str(" loadout=");
        l.push_str(x);
    }
    l.push(' ');
    l.push_str(seatticket::TOKEN);
    l.push('=');
    l.push_str(&seatticket::to_hex(t));
    l
}

fn join<T: ToString>(it: impl Iterator<Item = T>) -> String {
    it.map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

pub mod supervise;

#[cfg(test)]
mod tests;
