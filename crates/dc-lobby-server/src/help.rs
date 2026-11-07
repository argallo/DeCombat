//! `/help`: one private chat line per chat command the sender may use right where they are.
//!
//! The table [`ROWS`] is the only list of the lobby's chat commands; a command added to a handler gets one row here
//! (the test `help_tests` reads the handlers' match arms and refuses a command that has no row, or a row the handler
//! does not take). Which rows a sender sees follows the condition each handler applies itself: the GM test commands
//! need `may_use_test_commands`, `/k` needs the operator flag and a waiting room, `!clan` lines follow the player's
//! clan and `clan.chat_commands`, `!nm invite` needs a room. Commands a mod of the sender's server entry declares in
//! its manifest follow the table (an x-ray server lists `/wallhack` and the rest, any other server none).
// DC-DEFAULT: LG-427 (proposed, OWNER REQUEST 2026-11-06 "A /help command should list all the other / commands that
// are available in that server"), see DC_DEFAULTS.md

use crate::state::{Core, Fault, Sid};

/// The longest line, in characters: the 2009 chat box shows it whole.
pub const MAX_LINE: usize = 70;

/// Where and for whom a command works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Every lobby session.
    Anyone,
    /// The sender is in a room (waiting or playing).
    InRoom,
    /// The sender is in a clan (any member).
    InClan,
    /// `!clan` line for a player without a clan (needs `clan.chat_commands`).
    NoClanCmd,
    /// `!clan` line for a clan member (needs `clan.chat_commands`).
    MemberCmd,
    /// `!clan` line for the clan leader (needs `clan.chat_commands`).
    LeaderCmd,
    /// The GM test commands: operators, or everyone under `gm.test_commands` (`notices::may_use_test_commands`).
    TestCommands,
    /// An operator in a waiting room (the 2009 client offers `/k` to operators only).
    OperatorInWaitingRoom,
}

/// One command: its first word (or `!clan verb`), its arguments and what it does.
pub struct Row {
    pub cmd: &'static str,
    pub args: &'static str,
    pub about: &'static str,
    pub need: Need,
}

const fn row(cmd: &'static str, args: &'static str, about: &'static str, need: Need) -> Row {
    Row {
        cmd,
        args,
        about,
        need,
    }
}

/// Every chat command of the lobby, in the order `/help` prints them.
pub const ROWS: &[Row] = &[
    // the client's own lobby commands (menu/diagbox.rs); listed here because they are typed in the same box
    row(
        "/help",
        "",
        "lists the commands you can use here",
        Need::Anyone,
    ),
    row(
        "/report",
        "[text]",
        "send the server's owner a diagnostic report",
        Need::Anyone,
    ),
    row(
        "/diag",
        "[always|ask|never]",
        "show or set when reports are sent",
        Need::Anyone,
    ),
    row(
        "/nettest",
        "[send]",
        "test your connection to the servers",
        Need::Anyone,
    ),
    // lobby.rs nation_command
    row(
        "!nation",
        "<0-255>",
        "set the country code of your profile",
        Need::Anyone,
    ),
    // messenger.rs on_chat
    row(
        "!nm add",
        "<nick>",
        "ask a player to be your friend",
        Need::Anyone,
    ),
    row(
        "!nm accept",
        "<nick>",
        "accept a friend request",
        Need::Anyone,
    ),
    row(
        "!nm decline",
        "<nick>",
        "turn a friend request down",
        Need::Anyone,
    ),
    row("!nm remove", "<nick>", "drop a friend", Need::Anyone),
    row(
        "!nm list",
        "",
        "show your friends and open requests",
        Need::Anyone,
    ),
    row(
        "!nm w",
        "<nick> <text>",
        "whisper to a player who is online",
        Need::Anyone,
    ),
    row(
        "!nm invite",
        "<nick>",
        "ask a friend to join your room",
        Need::InRoom,
    ),
    row(
        "!nm clan",
        "<text>",
        "say something to your clan",
        Need::InClan,
    ),
    // clan.rs bang_command
    row(
        "!clan create",
        "<name> [emblem bg]",
        "found a clan",
        Need::NoClanCmd,
    ),
    row(
        "!clan join",
        "<name>",
        "apply to a clan (in at once if it invited you)",
        Need::NoClanCmd,
    ),
    row("!clan leave", "", "leave your clan", Need::MemberCmd),
    row(
        "!clan invite",
        "<nick>",
        "invite a player to your clan",
        Need::LeaderCmd,
    ),
    row(
        "!clan accept",
        "<nick>",
        "accept a player's application",
        Need::LeaderCmd,
    ),
    row(
        "!clan decline",
        "<nick>",
        "turn a player's application down",
        Need::LeaderCmd,
    ),
    row(
        "!clan kick",
        "<nick>",
        "remove a member from your clan",
        Need::LeaderCmd,
    ),
    // menu/gm.rs kick prompt, room.rs on_kick
    row(
        "/k",
        "<nick>",
        "remove a player from this waiting room",
        Need::OperatorInWaitingRoom,
    ),
    // notices.rs on_chat_command
    row(
        "/notice",
        "<text>",
        "show a ticker line to everyone online",
        Need::TestCommands,
    ),
    row(
        "/expire",
        "",
        "run the item expiry scan for yourself now",
        Need::TestCommands,
    ),
    row(
        "/shopver",
        "[list]",
        "raise shop list versions (all, or the named ones)",
        Need::TestCommands,
    ),
    row(
        "/grade",
        "<n>",
        "set your own grade (test)",
        Need::TestCommands,
    ),
    row(
        "!hisec",
        "<item>",
        "announce a HiSec case award (test)",
        Need::TestCommands,
    ),
];

/// The text of one row: `<command> <args> - <what it does>`.
pub fn line(r: &Row) -> String {
    if r.args.is_empty() {
        format!("{} - {}", r.cmd, r.about)
    } else {
        format!("{} {} - {}", r.cmd, r.args, r.about)
    }
}

/// A mod command's line, cut to [`MAX_LINE`] characters (a manifest description may run to 120).
pub fn mod_line(command: &str, description: &str) -> String {
    let text = format!("{} - {}", command, description.trim());
    if text.chars().count() <= MAX_LINE {
        return text;
    }
    let mut cut: String = text.chars().take(MAX_LINE - 3).collect();
    cut.push_str("...");
    cut
}

/// What decides which rows a sender sees.
#[derive(Debug, Default, Clone, Copy)]
struct Facts {
    in_room: bool,
    waiting_room: bool,
    clan: bool,
    leader: bool,
    clan_commands: bool,
    test_commands: bool,
    operator: bool,
}

impl Facts {
    fn allows(&self, need: Need) -> bool {
        match need {
            Need::Anyone => true,
            Need::InRoom => self.in_room,
            Need::InClan => self.clan,
            Need::NoClanCmd => self.clan_commands && !self.clan,
            Need::MemberCmd => self.clan_commands && self.clan,
            Need::LeaderCmd => self.clan_commands && self.leader,
            Need::TestCommands => self.test_commands,
            Need::OperatorInWaitingRoom => self.operator && self.waiting_room,
        }
    }
}

fn facts(st: &Core, sid: Sid, account: u32) -> Result<Facts, Fault> {
    let room = st.sessions.get(&sid).and_then(|s| s.room);
    let clan = crate::clan::clan_of(st, account)?;
    Ok(Facts {
        in_room: room.is_some(),
        waiting_room: room
            .and_then(|k| st.rooms.get(&k))
            .is_some_and(|r| r.state == crate::room::RoomPhase::Waiting),
        clan: clan.is_some(),
        leader: clan.as_ref().is_some_and(|k| k.master == account),
        clan_commands: st.rules.clan.chat_commands,
        test_commands: crate::notices::may_use_test_commands(st, account),
        operator: crate::gm::is_gm(st, account),
    })
}

/// The lines of the table that a sender with these facts sees.
fn table_lines(f: &Facts) -> Vec<String> {
    ROWS.iter().filter(|r| f.allows(r.need)).map(line).collect()
}

/// The lines of the commands the mods of the sender's server entry declare (LG-313 sets, or the lobby's own set).
fn mod_lines(st: &Core, sid: Sid) -> Vec<String> {
    let entry = crate::servers::map_entry_of_session(st, sid);
    let declared: Vec<&dc_mod_lua::ModCommand> = match entry.and_then(|e| st.entry_mods.get(&e)) {
        Some(set) => set
            .set
            .mods
            .iter()
            .flat_map(|m| m.commands.iter())
            .collect(),
        None => st.mod_commands.iter().collect(),
    };
    let mut out: Vec<String> = Vec::new();
    for c in declared {
        let l = mod_line(&c.command, &c.description);
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// Whether the chat line is `/help` (any case, arguments ignored).
pub fn is_help(text: &str) -> bool {
    text.split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("/help"))
}

/// The `/help` answer for a session: the lines, in order.
pub fn lines_for(st: &Core, sid: Sid, account: u32) -> Result<Vec<String>, Fault> {
    let mut out = table_lines(&facts(st, sid, account)?);
    out.extend(mod_lines(st, sid));
    Ok(out)
}

/// `/help` from the lobby chat: false when the line is anything else. The answer goes to the sender alone, in the
/// same kind of line the other chat commands answer with.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, Fault> {
    if id != crate::msg::CHAT_SEND || !is_help(&crate::wire::parse_chat(p)) {
        return Ok(false);
    }
    let Some(account) = st
        .sessions
        .get(&sid)
        .filter(|s| s.in_lobby && !s.closing)
        .and_then(|s| s.account)
    else {
        return Ok(false);
    };
    for l in lines_for(st, sid, account)? {
        crate::clan::tell(st, sid, &l);
    }
    Ok(true)
}
