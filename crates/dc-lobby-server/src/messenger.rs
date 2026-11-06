//! The messenger relay: friends, friend requests, whispers and the friend-menu room invite.
//!
//! The 2009 client runs these through an external messenger component, not the lobby protocol (SPEC:
//! PROTOCOL_2009.md §6.3). DeCombat carries them over the lobby connection with its own convention (our
//! extension, not a 2009 message; a client without our launcher hook simply never sends or sees it):
//!
//! * client -> server: a lobby chat line (0x3a0) `!nm <verb>` followed by tab-separated arguments
//!   (`!nm add\tErin`, `!nm accept\tDave`, `!nm list`; a space also separates the first argument);
//! * server -> client: the custom push **0x3f01** `{+6 wstr text}`, a NUL-terminated line of tab-separated fields
//!   whose first field names the kind. The 2009 dispatcher ignores the id (SPEC: PROTOCOL_2009.md §1.2: "id 0x3f01 is
//!   ignored", LIVE).
//!
//! Lines the server sends (field 1 = kind):
//!
//! | kind | fields | when |
//! |---|---|---|
//! | `status` | text | the result of the sender's own command |
//! | `request` | from-name | a friend request arrived (also re-sent at login while it is open) |
//! | `friends` | own name, friend names... | the full friend list (on `list`, after accept / remove, at login) |
//! | `online` | name, 1 or 0 | a friend logged in or out |
//! | `whisper` | from-name, to-name, text | to both sides of a whisper |
//! | `invite` | from-name, room number | a friend-menu invite (the room invite itself is 0x3023) |
//! | `clan` | from-name, text | w3-clan: a clan chat line, to every online member of the sender's clan |
//! | `claninvite` | clan name, clan number | LG-420: a clan leader's invitation (also re-sent at login while open); the
//! answers are `!nm clanyes<TAB>clan number` / `!nm clanno<TAB>clan number` (`clanreq.rs`) |
// SPEC: PROTOCOL_2009.md §6.3; HARNESS: lobby_harness::test_persist_restart (`!nm add` / `!nm accept` /
// `!nm list` and the 0x3f01 `status` / `friends` lines)
// DC-DEFAULT: OPS-8 (proposed) a friendship needs the other side's acceptance; friends are symmetric

use crate::clock;
use crate::db;
use crate::msg::CHAT_SEND;
use crate::state::{Core, Fault, HResult, Sid};
use crate::wire;
use dc_wire::PktWriter;
use rusqlite::{params, OptionalExtension};

/// Our messenger push (not a 2009 id).
pub const MESSENGER_PUSH: u16 = 0x3f01;
/// Chat prefix of a messenger command.
pub const PREFIX: &str = "!nm";
/// Longest relayed line (UTF-16 units).
const MAX_UNITS: usize = 1024;

/// 0x3f01: +6 NUL-terminated wstr of tab-separated fields.
pub fn push_frame(fields: &[&str]) -> Vec<u8> {
    let line = fields
        .iter()
        .map(|f| f.replace(['\t', '\0'], " "))
        .collect::<Vec<_>>()
        .join("\t");
    let units: Vec<u16> = line.encode_utf16().take(MAX_UNITS).collect();
    let mut w = PktWriter::new(MESSENGER_PUSH);
    for u in &units {
        w.u16(*u);
    }
    w.u16(0);
    w.into_frame()
}

/// Parsed `!nm` command: the verb (lower case) and its arguments; None when the line is not a messenger command.
pub fn parse_command(text: &str) -> Option<(String, Vec<String>)> {
    let t = text.trim_start();
    let rest = t.strip_prefix(PREFIX)?;
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let rest = rest.trim_start_matches([' ', '\t']);
    let mut parts: Vec<String> = rest.split('\t').map(|s| s.to_string()).collect();
    if parts.len() == 1 {
        // no tab: `!nm add Erin` -> verb and one argument split at the first space
        let one = parts.remove(0);
        let (v, a) = one.split_once(' ').unwrap_or((one.as_str(), ""));
        parts = vec![v.to_string()];
        if !a.trim().is_empty() {
            parts.push(a.trim().to_string());
        }
    }
    let verb = parts
        .first()
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let args = parts.into_iter().skip(1).collect();
    Some((verb, args))
}

fn send_to(st: &mut Core, account: u32, fields: &[&str]) {
    let f = push_frame(fields);
    for s in st.sids_of(account) {
        st.send(s, f.clone());
    }
}

fn status(st: &mut Core, sid: Sid, text: &str) {
    st.send(sid, push_frame(&["status", text]));
}

fn nick_of(st: &Core, account: u32) -> String {
    st.character(account).map(|c| c.nick).unwrap_or_default()
}

/// Friend accounts of `account`, ordered by name.
pub fn friends_of(st: &Core, account: u32) -> rusqlite::Result<Vec<(u32, String)>> {
    let mut q = st.db.prepare(
        "SELECT f.friend_no, c.nick FROM friends f JOIN characters c ON c.user_no = f.friend_no \
         WHERE f.user_no = ?1 ORDER BY c.nick_key",
    )?;
    let rows = q.query_map([account as i64], |r| {
        Ok((r.get::<_, i64>(0)? as u32, r.get::<_, String>(1)?))
    })?;
    rows.collect()
}

fn are_friends(st: &Core, a: u32, b: u32) -> rusqlite::Result<bool> {
    let n: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM friends WHERE user_no = ?1 AND friend_no = ?2",
        params![a as i64, b as i64],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn request_open(st: &Core, from: u32, to: u32) -> rusqlite::Result<bool> {
    Ok(st
        .db
        .query_row(
            "SELECT 1 FROM friend_requests WHERE from_no = ?1 AND to_no = ?2",
            params![from as i64, to as i64],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

/// Sends `friends\t<own name>\t<friend>...` to every lobby session of `account`.
fn push_friend_list(st: &mut Core, account: u32) -> HResult {
    let me = nick_of(st, account);
    let names: Vec<String> = friends_of(st, account)?
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    let mut fields: Vec<&str> = vec!["friends", &me];
    fields.extend(names.iter().map(|s| s.as_str()));
    send_to(st, account, &fields);
    Ok(())
}

/// `a` accepts the request of `b`.
fn make_friends(st: &mut Core, a: u32, b: u32) -> HResult {
    st.db.execute("DELETE FROM friend_requests WHERE (from_no = ?1 AND to_no = ?2) OR (from_no = ?2 AND to_no = ?1)", params![a as i64, b as i64])?;
    st.db.execute(
        "INSERT OR IGNORE INTO friends (user_no, friend_no) VALUES (?1, ?2)",
        params![a as i64, b as i64],
    )?;
    st.db.execute(
        "INSERT OR IGNORE INTO friends (user_no, friend_no) VALUES (?1, ?2)",
        params![b as i64, a as i64],
    )?;
    // the requester first: its list is the answer it waits for
    push_friend_list(st, b)?;
    push_friend_list(st, a)?;
    Ok(())
}

fn full(st: &Core, account: u32) -> rusqlite::Result<bool> {
    Ok(friends_of(st, account)?.len() as u32 >= st.rules.ops.max_friends)
}

fn on_add(st: &mut Core, sid: Sid, me: u32, name: &str) -> HResult {
    let Some(t) = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?
    else {
        status(st, sid, "No such player");
        return Ok(());
    };
    if t.user_no == me {
        status(st, sid, "You cannot add yourself");
    } else if are_friends(st, me, t.user_no)? {
        status(st, sid, "Already friends");
    } else if full(st, me)? || full(st, t.user_no)? {
        status(st, sid, "Friend list full");
    } else if request_open(st, t.user_no, me)? {
        // the other side asked first: adding back accepts
        make_friends(st, me, t.user_no)?;
        st.note(&format!(
            "messenger: {} added {} back: friends",
            me, t.user_no
        ));
    } else {
        st.db.execute(
            "INSERT OR IGNORE INTO friend_requests (from_no, to_no, created) VALUES (?1, ?2, ?3)",
            params![me as i64, t.user_no as i64, clock::unix_now()],
        )?;
        status(st, sid, "Friend request sent");
        let my = nick_of(st, me);
        send_to(st, t.user_no, &["request", &my]);
    }
    Ok(())
}

/// `accept <requester>`: `me` accepts the open request of `name`.
fn accept_request(st: &mut Core, sid: Sid, me: u32, name: &str) -> HResult {
    let from = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?;
    match from {
        Some(f) if request_open(st, f.user_no, me)? => {
            if full(st, me)? || full(st, f.user_no)? {
                status(st, sid, "Friend list full");
                return Ok(());
            }
            make_friends(st, me, f.user_no)?;
            st.note(&format!("messenger: {} and {} are friends", me, f.user_no));
        }
        _ => status(st, sid, "No friend request from that player"),
    }
    Ok(())
}

fn on_decline(st: &mut Core, sid: Sid, me: u32, name: &str) -> HResult {
    if let Some(f) = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)? {
        let n = st.db.execute(
            "DELETE FROM friend_requests WHERE from_no = ?1 AND to_no = ?2",
            params![f.user_no as i64, me as i64],
        )?;
        if n > 0 {
            status(st, sid, "Friend request declined");
            return Ok(());
        }
    }
    status(st, sid, "No friend request from that player");
    Ok(())
}

fn on_remove(st: &mut Core, sid: Sid, me: u32, name: &str) -> HResult {
    let Some(f) = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?
    else {
        status(st, sid, "No such player");
        return Ok(());
    };
    let n = st.db.execute(
        "DELETE FROM friends WHERE (user_no = ?1 AND friend_no = ?2) OR (user_no = ?2 AND friend_no = ?1)",
        params![me as i64, f.user_no as i64],
    )?;
    if n == 0 {
        status(st, sid, "Not on your friend list");
        return Ok(());
    }
    push_friend_list(st, me)?;
    push_friend_list(st, f.user_no)?;
    Ok(())
}

fn on_list(st: &mut Core, me: u32) -> HResult {
    push_friend_list(st, me)?;
    let mut q = st.db.prepare(
        "SELECT c.nick FROM friend_requests r JOIN characters c ON c.user_no = r.from_no WHERE r.to_no = ?1 ORDER BY r.created",
    )?;
    let open: Vec<String> = q
        .query_map([me as i64], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    drop(q);
    for n in open {
        send_to(st, me, &["request", &n]);
    }
    for (f, n) in friends_of(st, me)? {
        if !st.sids_of(f).is_empty() {
            send_to(st, me, &["online", &n, "1"]);
        }
    }
    Ok(())
}

// DC-DEFAULT: CD-33 a muted player cannot whisper; OPS-8 (proposed) whisper to any online player unless
// messenger.whisper_friends_only
fn on_whisper(st: &mut Core, sid: Sid, me: u32, args: &[String]) -> HResult {
    let (name, text) = match args {
        [n, rest @ ..] if !rest.is_empty() => (n.clone(), rest.join(" ")),
        [one] => match one.split_once(' ') {
            Some((n, t)) => (n.to_string(), t.to_string()),
            None => (one.clone(), String::new()),
        },
        _ => (String::new(), String::new()),
    };
    if text.trim().is_empty() {
        status(st, sid, "Nothing to whisper");
        return Ok(());
    }
    let end = crate::gm::mute_end(st, me);
    if end != 0 {
        st.send(sid, crate::gm::mute_until_push(end as u64));
        status(st, sid, "You are muted");
        return Ok(());
    }
    let target = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?;
    let Some(t) = target.filter(|t| !st.sids_of(t.user_no).is_empty()) else {
        status(st, sid, "That player is not online");
        return Ok(());
    };
    if st.rules.ops.whisper_friends_only && !are_friends(st, me, t.user_no)? {
        status(st, sid, "You can only whisper to friends");
        return Ok(());
    }
    let my = nick_of(st, me);
    let fields = ["whisper", my.as_str(), t.nick.as_str(), text.as_str()];
    send_to(st, t.user_no, &fields);
    if t.user_no != me {
        send_to(st, me, &fields);
    }
    st.note(&format!("whisper {} -> {}: {}", my, t.nick, text));
    Ok(())
}

/// w3-clan: `!nm clan<TAB>text`: the line goes to every online member of the sender's clan (the sender included) as
/// `clan<TAB>name<TAB>text`.
// DC-DEFAULT: CLI-563 (proposed) clan chat over the messenger relay (the 2009 client sends clan chat through its
// external messenger component, SPEC: PROTOCOL_2009.md §5.2 "Clan chat"); a muted player cannot send it (as CD-33)
fn on_clan_say(st: &mut Core, sid: Sid, me: u32, args: &[String]) -> HResult {
    let text = args.join(" ");
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let end = crate::gm::mute_end(st, me);
    if end != 0 {
        st.send(sid, crate::gm::mute_until_push(end as u64));
        status(st, sid, "You are muted");
        return Ok(());
    }
    let Some(clan) = crate::clan::clan_of(st, me)? else {
        status(st, sid, "You are not in a clan");
        return Ok(());
    };
    let my = nick_of(st, me);
    for m in crate::clan::members(&st.db, clan.uid)? {
        send_to(st, m.user_no, &["clan", my.as_str(), text]);
    }
    st.note(&format!("clan chat {} [{}]: {}", my, clan.name, text));
    Ok(())
}

/// Friend-menu invite: the friend gets the 2009 room invite 0x3023 (and an `invite` line) when it waits in a
/// channel lobby and the inviter is in a room.
// DC-DEFAULT: OPS-8 (proposed) invites go to friends only; CD-44 room invite layout
fn on_invite(st: &mut Core, sid: Sid, me: u32, name: &str) -> HResult {
    let Some(key) = st.sessions.get(&sid).and_then(|s| s.room) else {
        status(st, sid, "Enter a room first");
        return Ok(());
    };
    let target = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?;
    let Some(t) = target else {
        status(st, sid, "No such player");
        return Ok(());
    };
    if !are_friends(st, me, t.user_no)? {
        status(st, sid, "Only friends can be invited");
        return Ok(());
    }
    let waiting: Vec<Sid> = st
        .sessions
        .values()
        .filter(|s| {
            s.in_lobby
                && !s.closing
                && s.account == Some(t.user_no)
                && s.channel.is_some()
                && s.room.is_none()
        })
        .map(|s| s.sid)
        .collect();
    if waiting.is_empty() {
        status(st, sid, "That friend is not waiting in a lobby");
        return Ok(());
    }
    let my = nick_of(st, me);
    let pw = st
        .rooms
        .get(&key)
        .map(|r| r.password.clone())
        .unwrap_or_default();
    let invite = wire::room_invite_push(key.no, &my, &pw);
    let line = push_frame(&["invite", &my, &key.no.to_string()]);
    for s in waiting {
        st.send(s, invite.clone());
        st.send(s, line.clone());
    }
    status(st, sid, "Invite sent");
    Ok(())
}

fn on_chat(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    let text = wire::parse_chat(p);
    let Some((verb, args)) = parse_command(&text) else {
        return Ok(false);
    };
    let Some(me) = st
        .sessions
        .get(&sid)
        .filter(|s| s.in_lobby && !s.closing)
        .and_then(|s| s.account)
    else {
        return Ok(true);
    };
    let first = args.first().cloned().unwrap_or_default();
    match verb.as_str() {
        "add" => on_add(st, sid, me, &first)?,
        "accept" => accept_request(st, sid, me, &first)?,
        "decline" | "deny" => on_decline(st, sid, me, &first)?,
        "remove" | "del" => on_remove(st, sid, me, &first)?,
        "list" | "" => on_list(st, me)?,
        "w" | "whisper" | "msg" => on_whisper(st, sid, me, &args)?,
        "invite" => on_invite(st, sid, me, &first)?,
        "clan" => on_clan_say(st, sid, me, &args)?,
        // LG-420: the clan invitation box's answers (the clan number of the `claninvite` line)
        "clanyes" => crate::clanreq::on_answer(st, sid, me, &first, true)?,
        "clanno" => crate::clanreq::on_answer(st, sid, me, &first, false)?,
        // the reference client sends `!nm sync` when it comes back from a match; the 2009 client never sends a
        // messenger line (none in the captures, no `!nm` text in its CShell.dll): taken without a reply, so no
        // status line (the reference stack shows every status line as a notice box)
        // OBSERVED: capture 20261026_034121_arena-roundend-b033909 (reference: one `!nm sync`
        // chat line); 20261025_204341_realclient, 20261025_125432_rust6, 20261025_153705_rust7 (real client: none)
        // DC-DEFAULT: CLI-274 (proposed) the messenger `sync` is a tolerated no-op
        "sync" => {}
        _ => status(st, sid, "Unknown messenger command"),
    }
    Ok(true)
}

/// Lobby login: the friend list, open requests and presence of the new session; friends hear it come online.
pub fn on_lobby_login(st: &mut Core, _sid: Sid, account: u32) -> HResult {
    // already online elsewhere (a second lobby session replaces the first): no presence change
    let first = st.sids_of(account).len() <= 1;
    on_list(st, account)?;
    if first {
        let me = nick_of(st, account);
        for (f, _) in friends_of(st, account)? {
            send_to(st, f, &["online", &me, "1"]);
        }
    }
    Ok(())
}

/// Connection loss of a lobby session: friends hear it go offline when it was the account's last session.
pub fn on_disconnect(st: &mut Core, sid: Sid) -> HResult {
    let Some(account) = st
        .sessions
        .get(&sid)
        .filter(|s| s.in_lobby)
        .and_then(|s| s.account)
    else {
        return Ok(());
    };
    if st.sids_of(account).iter().any(|&s| s != sid) {
        return Ok(());
    }
    let me = nick_of(st, account);
    for (f, _) in friends_of(st, account)? {
        send_to(st, f, &["online", &me, "0"]);
    }
    Ok(())
}

/// Messenger commands from the lobby chat; false when the line is not one.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, Fault> {
    match id {
        CHAT_SEND => on_chat(st, sid, p),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(f: &[u8]) -> (i32, String) {
        let (frames, _) = dc_wire::parse_tcp_frames(f).unwrap();
        let (id, p) = frames.into_iter().next().unwrap();
        (
            id,
            dc_wire::PktReader::new(&p).at(6).wstr((p.len() - 6) / 2),
        )
    }

    #[test]
    fn push_layout_is_a_tab_line() {
        let (id, t) = text_of(&push_frame(&["friends", "Dave", "Erin"]));
        assert_eq!((id, t.as_str()), (0x3f01, "friends\tDave\tErin"));
        // separators inside a field cannot split it
        let (_, t) = text_of(&push_frame(&["whisper", "A", "B", "x\ty"]));
        assert_eq!(
            t.split('\t').collect::<Vec<_>>(),
            ["whisper", "A", "B", "x y"]
        );
    }

    #[test]
    fn command_parsing() {
        assert_eq!(
            parse_command("!nm add\tErin"),
            Some(("add".into(), vec!["Erin".into()]))
        );
        assert_eq!(
            parse_command("!nm accept\tDave"),
            Some(("accept".into(), vec!["Dave".into()]))
        );
        assert_eq!(parse_command("!nm list"), Some(("list".into(), vec![])));
        assert_eq!(
            parse_command("!nm add Erin"),
            Some(("add".into(), vec!["Erin".into()]))
        );
        assert_eq!(
            parse_command("!nm w\tErin\thello there"),
            Some(("w".into(), vec!["Erin".into(), "hello there".into()]))
        );
        assert_eq!(parse_command("!nm"), Some(("".into(), vec![])));
        assert_eq!(parse_command("!nmx add"), None);
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("!clan create X"), None);
    }
}
