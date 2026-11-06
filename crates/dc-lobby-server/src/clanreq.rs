//! Clan applications and invitations (LG-420): a player's join is an application the clan leader accepts or
//! declines; the leader invites a player by name and the player answers a question box.
//!
//! The 2009 join was a web application on the publisher's site that the leader approved there (CD-47: the clan
//! screen only opened the page URLs); the pages, the table and the relay lines here are our own.
//!
//! * `clan_requests(clan_uid, user_no, kind, created)`: one open row per player and clan, kind `apply` (the player
//!   asked) or `invite` (the leader asked). A join (any path), a create, a disband or an account delete removes them.
//! * An application from a player the clan already invited, or an invitation of a player who already applied, joins
//!   at once (both sides asked).
//! * The invited player hears the relay line `claninvite<TAB>clan name<TAB>clan number` (0x3f01, the messenger
//!   relay's convention, PROTOCOL_2009.md §6.3) at once when online and again at each login while it is open; the
//!   answers are the relay commands `!nm clanyes<TAB>clan number` / `!nm clanno<TAB>clan number` over the player's
//!   own lobby connection (which carries the identity).
//! * Results and notes to the players are 0x3051 lines from the server (CLN-9's answer channel).
// SPEC: PROTOCOL_2009.md §5.2 (clan web pages: ours), §6.3 (the 0x3f01 relay: our extension)
// DC-DEFAULT: LG-420 (proposed) clan applications and invitations

use crate::clan::{self, Act, ClanRow, Refusal};
use crate::clock;
use crate::db;
use crate::messenger::push_frame;
use crate::state::{Core, HResult, Sid};
use crate::wire;
use rusqlite::{params, OptionalExtension};

pub const KIND_APPLY: &str = "apply";
pub const KIND_INVITE: &str = "invite";
/// The relay line kind of an invitation.
pub const INVITE_LINE: &str = "claninvite";

/// A player with an open application (the leader's manage page rows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applicant {
    pub user_no: u32,
    pub nick: String,
    pub exp: i64,
}

/// What an application or an invitation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The row is open (the other side answers later).
    Sent(ClanRow),
    /// The other side had asked already: the player is a member now.
    Joined(ClanRow),
}

/// The answer line of an application (web page `msg`, chat reply).
pub fn asked_text(a: &Asked) -> String {
    match a {
        Asked::Sent(k) => format!("Application sent to {}.", k.name),
        Asked::Joined(k) => format!("You joined {}.", k.name),
    }
}

/// The answer line of an invitation of `nick`.
pub fn invited_text(a: &Asked, nick: &str) -> String {
    match a {
        Asked::Sent(_) => format!("Invitation sent to {}.", nick),
        Asked::Joined(k) => format!("{} joined {}.", nick, k.name),
    }
}

/// The answer line of an accept (`yes`) or a decline of `nick`'s application.
pub fn answered_text(yes: bool, nick: &str) -> String {
    if yes {
        format!("{} joined the clan.", nick)
    } else {
        format!("The application of {} was declined.", nick)
    }
}

/// A 0x3051 line from the server to every lobby session of `account` (none when it is offline).
pub fn tell_account(st: &mut Core, account: u32, text: &str) {
    let f = wire::chat_push(0, "", 0, text);
    for s in st.sids_of(account) {
        st.send(s, f.clone());
    }
}

fn open_kind(st: &Core, uid: i64, account: u32) -> rusqlite::Result<Option<String>> {
    st.db
        .query_row(
            "SELECT kind FROM clan_requests WHERE clan_uid = ?1 AND user_no = ?2",
            params![uid, account as i64],
            |r| r.get(0),
        )
        .optional()
}

fn put_row(st: &Core, uid: i64, account: u32, kind: &str) -> rusqlite::Result<()> {
    st.db.execute(
        "INSERT INTO clan_requests (clan_uid, user_no, kind, created) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(clan_uid, user_no) DO UPDATE SET kind = excluded.kind, created = excluded.created",
        params![uid, account as i64, kind, clock::unix_now()],
    )?;
    Ok(())
}

fn drop_row(st: &Core, uid: i64, account: u32) -> rusqlite::Result<usize> {
    st.db.execute(
        "DELETE FROM clan_requests WHERE clan_uid = ?1 AND user_no = ?2",
        params![uid, account as i64],
    )
}

/// A member (or a new leader) has no open application or invitation left.
pub fn forget_account(st: &Core, account: u32) -> rusqlite::Result<()> {
    st.db.execute(
        "DELETE FROM clan_requests WHERE user_no = ?1",
        [account as i64],
    )?;
    Ok(())
}

/// The open applications of clan `uid`, oldest first.
pub fn applicants(st: &Core, uid: i64) -> rusqlite::Result<Vec<Applicant>> {
    let mut q = st.db.prepare(
        "SELECT r.user_no, c.nick, c.exp FROM clan_requests r JOIN characters c ON c.user_no = r.user_no \
         WHERE r.clan_uid = ?1 AND r.kind = 'apply' ORDER BY r.created, r.user_no",
    )?;
    let rows = q.query_map([uid], |r| {
        Ok(Applicant {
            user_no: r.get::<_, i64>(0)? as u32,
            nick: r.get(1)?,
            exp: r.get(2)?,
        })
    })?;
    rows.collect()
}

fn nick_of(st: &Core, account: u32) -> String {
    st.character(account).map(|c| c.nick).unwrap_or_default()
}

/// The clan `leader` leads (refusals: no character, no clan, not the leader).
fn led_clan(st: &Core, leader: u32) -> Act<ClanRow> {
    if st.character(leader).is_none() {
        return Ok(Err(Refusal::NoCharacter));
    }
    let Some(k) = clan::clan_of(st, leader)? else {
        return Ok(Err(Refusal::NoClan));
    };
    if k.master != leader {
        return Ok(Err(Refusal::NotLeader));
    }
    Ok(Ok(k))
}

fn full(st: &Core, k: &ClanRow) -> rusqlite::Result<bool> {
    let rec = clan::record(st, k)?;
    Ok(rec.members >= rec.member_limit)
}

/// The join of a request: [`clan::join`] (in-clan and member limit checked again) and the joined player's line.
fn join_now(st: &mut Core, account: u32, uid: i64) -> Act<ClanRow> {
    let r = clan::join(st, account, uid)?;
    if let Ok(k) = &r {
        tell_account(st, account, &format!("You joined {}.", k.name));
    }
    Ok(r)
}

/// `account` asks to join clan `uid` (the find page's Apply, `!clan join`). Refused in a clan, for an unknown clan
/// and a full one (CLN-6); joins at once when the clan invited the player already. The leader hears of it.
pub fn apply(st: &mut Core, account: u32, uid: i64) -> Act<Asked> {
    if !st.rules.clan.join_approval {
        // `clan.join_approval` off: the join of before LG-420
        return Ok(clan::join(st, account, uid)?.map(Asked::Joined));
    }
    let Some(c) = st.character(account) else {
        return Ok(Err(Refusal::NoCharacter));
    };
    if c.clan_uid != 0 {
        if let Some(k) = clan::clan_by_uid(&st.db, c.clan_uid)? {
            return Ok(Err(Refusal::InClan(k.name)));
        }
    }
    let Some(k) = clan::clan_by_uid(&st.db, uid)? else {
        return Ok(Err(Refusal::NoSuchClan));
    };
    if full(st, &k)? {
        return Ok(Err(Refusal::Full));
    }
    if open_kind(st, uid, account)?.as_deref() == Some(KIND_INVITE) {
        let r = join_now(st, account, uid)?;
        if let Ok(k) = &r {
            let n = nick_of(st, account);
            tell_account(st, k.master, &format!("{} joined {}.", n, k.name));
            st.note(&format!(
                "clan {}: account {} applied after an invitation: joined",
                k.uid, account
            ));
        }
        return Ok(r.map(Asked::Joined));
    }
    let again = open_kind(st, uid, account)?.is_some();
    put_row(st, uid, account, KIND_APPLY)?;
    if !again {
        let n = nick_of(st, account);
        tell_account(
            st,
            k.master,
            &format!(
                "{} applied to join {}. Open Clan Setting to accept or decline.",
                n, k.name
            ),
        );
        st.note(&format!(
            "clan {}: account {} applied to join",
            k.uid, account
        ));
    }
    Ok(Ok(Asked::Sent(k)))
}

/// The leader accepts the application of `account`: the player joins (member limit checked again).
pub fn accept(st: &mut Core, leader: u32, account: u32) -> Act<(ClanRow, String)> {
    let k = match led_clan(st, leader)? {
        Ok(k) => k,
        Err(r) => return Ok(Err(r)),
    };
    if open_kind(st, k.uid, account)?.as_deref() != Some(KIND_APPLY) {
        return Ok(Err(Refusal::NoApplication));
    }
    match join_now(st, account, k.uid)? {
        Ok(k) => {
            st.note(&format!(
                "clan {}: the leader {} accepted account {}",
                k.uid, leader, account
            ));
            Ok(Ok((k, nick_of(st, account))))
        }
        // the player went into another clan: the application is void
        Err(Refusal::InClan(_)) => {
            drop_row(st, k.uid, account)?;
            Ok(Err(Refusal::TheyAreInClan))
        }
        Err(r) => Ok(Err(r)),
    }
}

/// The leader declines the application of `account`; the player hears of it.
pub fn decline(st: &mut Core, leader: u32, account: u32) -> Act<(ClanRow, String)> {
    let k = match led_clan(st, leader)? {
        Ok(k) => k,
        Err(r) => return Ok(Err(r)),
    };
    if open_kind(st, k.uid, account)?.as_deref() != Some(KIND_APPLY) {
        return Ok(Err(Refusal::NoApplication));
    }
    drop_row(st, k.uid, account)?;
    tell_account(
        st,
        account,
        &format!("Your application to {} was declined.", k.name),
    );
    st.note(&format!(
        "clan {}: the leader {} declined account {}",
        k.uid, leader, account
    ));
    Ok(Ok((k, nick_of(st, account))))
}

/// The relay line of an open invitation to clan `k`.
fn invite_line(k: &ClanRow) -> Vec<u8> {
    push_frame(&[INVITE_LINE, &k.name, &k.uid.to_string()])
}

/// The leader invites the player named `name`. Refused for an unknown name, a player in a clan and a full clan;
/// joins at once when the player applied already. The player's online sessions get the question line.
pub fn invite(st: &mut Core, leader: u32, name: &str) -> Act<(Asked, String)> {
    let k = match led_clan(st, leader)? {
        Ok(k) => k,
        Err(r) => return Ok(Err(r)),
    };
    let Some(t) = db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?
    else {
        return Ok(Err(Refusal::NoSuchPlayer));
    };
    if t.clan_uid != 0 && clan::clan_by_uid(&st.db, t.clan_uid)?.is_some() {
        return Ok(Err(Refusal::TheyAreInClan));
    }
    if full(st, &k)? {
        return Ok(Err(Refusal::Full));
    }
    if open_kind(st, k.uid, t.user_no)?.as_deref() == Some(KIND_APPLY) {
        let r = join_now(st, t.user_no, k.uid)?;
        if r.is_ok() {
            st.note(&format!(
                "clan {}: the leader {} invited account {} who had applied: joined",
                k.uid, leader, t.user_no
            ));
        }
        return Ok(r.map(|k| (Asked::Joined(k), t.nick)));
    }
    put_row(st, k.uid, t.user_no, KIND_INVITE)?;
    let line = invite_line(&k);
    for s in st.sids_of(t.user_no) {
        st.send(s, line.clone());
    }
    st.note(&format!(
        "clan {}: the leader {} invited account {}",
        k.uid, leader, t.user_no
    ));
    Ok(Ok((Asked::Sent(k), t.nick)))
}

/// The invited player's answer: yes joins (member limit checked again; a full clan keeps the invitation open),
/// no removes the invitation. The leader hears the answer.
pub fn answer_invite(st: &mut Core, account: u32, uid: i64, yes: bool) -> Act<ClanRow> {
    if open_kind(st, uid, account)?.as_deref() != Some(KIND_INVITE) {
        return Ok(Err(Refusal::NoInvitation));
    }
    let Some(k) = clan::clan_by_uid(&st.db, uid)? else {
        drop_row(st, uid, account)?;
        return Ok(Err(Refusal::NoSuchClan));
    };
    let n = nick_of(st, account);
    if !yes {
        drop_row(st, uid, account)?;
        tell_account(
            st,
            k.master,
            &format!("{} declined the invitation to {}.", n, k.name),
        );
        st.note(&format!(
            "clan {}: account {} declined the invitation",
            uid, account
        ));
        return Ok(Ok(k));
    }
    match join_now(st, account, uid)? {
        Ok(k) => {
            tell_account(st, k.master, &format!("{} joined {}.", n, k.name));
            st.note(&format!(
                "clan {}: account {} accepted the invitation",
                uid, account
            ));
            Ok(Ok(k))
        }
        Err(Refusal::Full) => Ok(Err(Refusal::Full)),
        Err(r) => {
            drop_row(st, uid, account)?;
            Ok(Err(r))
        }
    }
}

/// `!nm clanyes<TAB>uid` / `!nm clanno<TAB>uid` (the invitation box's buttons); the result is a 0x3051 line.
pub fn on_answer(st: &mut Core, sid: Sid, account: u32, arg: &str, yes: bool) -> HResult {
    let uid = arg.trim().parse::<i64>().unwrap_or(0);
    let r = answer_invite(st, account, uid, yes)?;
    let text = match (&r, yes) {
        (Ok(k), false) => format!("You declined the invitation to {}.", k.name),
        // the join already told the player (`You joined ...`)
        (Ok(_), true) => return Ok(()),
        (Err(e), _) => format!("Cannot answer the invitation: {}.", e.text()),
    };
    clan::tell(st, sid, &text);
    Ok(())
}

/// Lobby login: the open invitations ask again; a leader hears how many applications wait.
pub fn on_lobby_login(st: &mut Core, sid: Sid, account: u32) -> HResult {
    let mut q = st.db.prepare(
        "SELECT r.clan_uid FROM clan_requests r WHERE r.user_no = ?1 AND r.kind = 'invite' ORDER BY r.created",
    )?;
    let uids: Vec<i64> = q
        .query_map([account as i64], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    drop(q);
    for uid in uids {
        match clan::clan_by_uid(&st.db, uid)? {
            Some(k) => st.send(sid, invite_line(&k)),
            None => {
                drop_row(st, uid, account)?;
            }
        }
    }
    if let Some(k) = clan::clan_of(st, account)?.filter(|k| k.master == account) {
        let n = applicants(st, k.uid)?.len();
        if n > 0 {
            let s = if n == 1 { "" } else { "s" };
            clan::tell(
                st,
                sid,
                &format!(
                    "{} open application{} to {}: open Clan Setting to answer.",
                    n, s, k.name
                ),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "clanreq_tests.rs"]
mod tests;
