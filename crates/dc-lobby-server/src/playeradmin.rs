//! The Players tab's account panel and its writes (LG-395..LG-398): `GET /ops/player?account=N` (what the panel shows)
//! and `POST /ops/player/<action>` with a JSON body `{"account": N, ...}`:
//!
//! - `grant`: `nx`, `gp`, `exp` (signed; at least one), `reason`. Each at most `gm.admin_max_*` in one change; a change
//!   that would take a balance below 0 is refused. NX is the prepaid balance the NX rewards pay into (nxrewards.rs).
//! - `ban`: `hours` and / or `days`, or `permanent: true`, and `reason`; drops the connections like the GM ban. `unban`.
//! - `kick` (`reason`), `mute` (`minutes`), `unmute` (gm.rs, the GM tools' own functions).
//! - `disable`, `enable` (LG-142), `reset_mail` (LG-242's code mail to the account's address).
//! - `loadout`: `spec` = the items as `account loadout` takes them (`0:27,1:3`, or `random`); gives and equips them
//!   (accounts.rs `equip_loadout`), counting from the account's next login (OPS-14).
//! - `delete`: `confirm` = the login; refused for a GM account and while the account is connected. Irreversible.
//!
//! Every write goes through the dashboard's write guard (a JSON body, the page's own `Origin`, the page's token in
//! `X-Admin-CSRF`), runs in one transaction and is logged as an ops event of kind `admin` on the account and as an
//! `op_actions` row (by_no 0), naming who did it: the admin port, or on the admin web the signed-in key.
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-395, LG-396, LG-397, LG-398 (proposed, USER REQUEST 2026-11-03), see DC_DEFAULTS.md

use crate::betaweb::{header, WriteGuard};
use crate::clanweb::Request;
use crate::clock;
use crate::db;
use crate::gm;
use crate::opsmon::kind;
use crate::room::RoomPhase;
use crate::state::{Core, Fault, Shared};
use crate::wire;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

pub type Reply = (u16, &'static str, String);

/// The writes under `/ops/player/`.
pub const ACTIONS: &[&str] = &[
    "grant",
    "ban",
    "unban",
    "kick",
    "mute",
    "unmute",
    "disable",
    "enable",
    "reset_mail",
    "loadout",
    "delete",
];

/// Longest reason kept (characters).
pub const MAX_REASON: usize = 200;
/// Longest temporary ban: ten years.
pub const MAX_BAN_HOURS: i64 = 24 * 3650;

/// True for the panel's path and its writes (not `/ops/players`, the list).
pub fn handles(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    p == "/ops/player" || p.starts_with("/ops/player/")
}

fn action_of(path: &str) -> Option<&'static str> {
    let a = path.trim_end_matches('/').strip_prefix("/ops/player/")?;
    ACTIONS.iter().copied().find(|x| *x == a)
}

fn jerr(status: u16, text: &str) -> Reply {
    (
        status,
        crate::opsweb::JSON,
        json!({"ok": false, "message": text}).to_string(),
    )
}

fn jok(v: Value) -> Reply {
    (200, crate::opsweb::JSON, v.to_string())
}

/// `1234567` -> `1,234,567`.
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn signed(n: i64) -> String {
    if n >= 0 {
        format!("+{}", grouped(n))
    } else {
        grouped(n)
    }
}

// ------------------------------------------------------------------------------------------------- reading

/// Whether the account has a live connection (any listener) or a session key between sign-in and the lobby, and the
/// words for where it is.
pub fn whereabouts(st: &Core, account: u32) -> (bool, String) {
    let mine = || {
        st.sessions
            .values()
            .filter(move |s| !s.closing && s.account == Some(account))
    };
    if let Some(s) = mine().find(|s| s.in_lobby) {
        let text = match (s.room, s.channel) {
            (Some(k), _) => {
                let playing = st
                    .rooms
                    .get(&k)
                    .map(|r| r.state == RoomPhase::Playing)
                    .unwrap_or(false);
                format!(
                    "room {}-{}, {}",
                    k.channel,
                    k.no,
                    if playing { "in a match" } else { "waiting" }
                )
            }
            (None, Some(c)) => format!("channel {c} lobby"),
            (None, None) => "the server list".to_string(),
        };
        return (true, text);
    }
    if mine().next().is_some() {
        return (true, "signing in".to_string());
    }
    if st.tickets.values().any(|t| t.account == account) {
        return (true, "between sign-in and the lobby".to_string());
    }
    (false, String::new())
}

/// Who set a ban: a GM account by its name, else the stored name (the dashboard's writer).
fn ban_setter(st: &Core, by_no: u32, by_name: &str) -> String {
    if by_no != 0 {
        let name = st
            .character(by_no)
            .map(|c| c.nick)
            .or_else(|| {
                db::account_by_no(&st.db, by_no)
                    .ok()
                    .flatten()
                    .map(|a| a.login)
            })
            .unwrap_or_else(|| format!("account {by_no}"));
        return format!("GM {name}");
    }
    if by_name.is_empty() {
        "the dashboard".to_string()
    } else {
        by_name.to_string()
    }
}

/// The ban in force at `now`: (until, 0 = permanent; reason; by; since).
fn ban_of(
    st: &Core,
    account: u32,
    now: i64,
) -> rusqlite::Result<Option<(i64, String, String, i64)>> {
    let row: Option<(i64, String, i64, String, i64)> = st
        .db
        .query_row(
            "SELECT until, reason, by_no, by_name, created FROM op_bans WHERE user_no = ?1",
            [account as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    Ok(row.filter(|(until, ..)| *until == 0 || *until > now).map(
        |(until, reason, by_no, by_name, since)| {
            (until, reason, ban_setter(st, by_no as u32, &by_name), since)
        },
    ))
}

/// Minutes left of a mute ending at local FILETIME `until` (0 when not muted).
fn mute_minutes_left(until: i64, now_ticks: i64) -> i64 {
    if until <= now_ticks {
        return 0;
    }
    let per = 60 * clock::TICKS_PER_SEC;
    (until - now_ticks + per - 1) / per
}

/// `GET /ops/player?account=N`: the account panel. `None` when there is no such account.
pub fn account_json(st: &Core, account: u32, now: i64) -> rusqlite::Result<Option<Value>> {
    let Some(acc) = db::account_by_no(&st.db, account)? else {
        return Ok(None);
    };
    let (created, last_login, self_bot, pool_bot, email): (i64, i64, i64, i64, String) = st
        .db
        .query_row(
        "SELECT created, last_login, self_bot, pool_bot, email FROM accounts WHERE user_no = ?1",
        [account as i64],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let c = db::character(&st.db, account)?;
    let (online, place) = whereabouts(st, account);
    let ban = ban_of(st, account, now)?.map(|(until, reason, by, since)| {
        json!({"until": until, "permanent": until == 0, "reason": reason, "by": by, "since": since})
    });
    let ticks = clock::now_ticks();
    let mute = c
        .as_ref()
        .map(|c| mute_minutes_left(c.mute_until, ticks))
        .filter(|m| *m > 0)
        .map(|m| {
            let secs = c
                .as_ref()
                .map_or(0, |c| (c.mute_until - ticks) / clock::TICKS_PER_SEC);
            json!({"minutes_left": m, "until": now + secs})
        });
    let r = &st.rules.ops;
    let bot = self_bot != 0 || pool_bot != 0 || crate::grant::is_bot_name(&acc.login);
    Ok(Some(json!({
        "ok": true,
        "account": account,
        "login": acc.login,
        "nick": c.as_ref().map_or(String::new(), |c| c.nick.clone()),
        "has_character": c.is_some(),
        "created": created,
        "last_login": last_login,
        "gm": acc.gm != 0,
        "bot": bot,
        "pool_bot": pool_bot != 0,
        "disabled": acc.disabled,
        "has_email": !email.is_empty(),
        "email": if email.is_empty() { String::new() } else { crate::accounts::mask_email(&email) },
        "mail_on": st.mail.enabled(),
        "exp": c.as_ref().map_or(0, |c| c.exp),
        "rank": c.as_ref().map_or(0, |c| st.rank_of(c.exp)),
        "gp": c.as_ref().map_or(0, |c| c.gp),
        "nx": c.as_ref().map_or(0, |c| c.nx),
        "nx_credit": c.as_ref().map_or(0, |c| c.nx_cr),
        "online": online,
        "where": place,
        "ban": ban,
        "mute": mute,
        "caps": {"nx": r.admin_max_nx, "gp": r.admin_max_gp, "exp": r.admin_max_exp},
        "max_mute_minutes": r.max_mute_minutes,
    })))
}

// ------------------------------------------------------------------------------------------------- writes

/// A refused write: the HTTP status and the words.
pub type Refused = (u16, String);
/// What a write did: the words for the page, or why it was refused (nothing changed).
pub type Outcome = Result<Result<String, Refused>, Fault>;

fn refuse(status: u16, text: impl Into<String>) -> Outcome {
    Ok(Err((status, text.into())))
}

/// An integer field: absent = `None`, a number or a numeric string = its value, anything else an error.
fn int(m: &Map<String, Value>, k: &str) -> Result<Option<i64>, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{k}: a whole number")),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .trim()
            .replace(',', "")
            .parse()
            .map(Some)
            .map_err(|_| format!("{k}: a whole number")),
        _ => Err(format!("{k}: a whole number")),
    }
}

/// The reason, trimmed, control characters out, at most [`MAX_REASON`] characters.
fn reason_of(m: &Map<String, Value>) -> String {
    let r = m.get("reason").and_then(Value::as_str).unwrap_or("");
    r.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_REASON)
        .collect()
}

fn with_reason(text: String, reason: &str) -> String {
    if reason.is_empty() {
        text
    } else {
        format!("{text}: {reason}")
    }
}

/// The ops event of a write (kind `admin`, on the account) and the log line.
fn record(st: &Core, account: u32, detail: &str) {
    crate::opsmon::event(st, account, kind::ADMIN, detail);
    st.note(&format!("ops player admin: account {account}: {detail}"));
}

fn name_of(st: &Core, account: u32, login: &str) -> String {
    st.character(account)
        .map(|c| c.nick)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| login.to_string())
}

/// Runs one write in the caller's scope.
pub fn act(
    st: &mut Core,
    action: &str,
    account: u32,
    m: &Map<String, Value>,
    by: &str,
    now: i64,
) -> Outcome {
    let Some(acc) = db::account_by_no(&st.db, account)? else {
        return refuse(404, format!("no account {account}"));
    };
    let who = name_of(st, account, &acc.login);
    match action {
        "grant" => grant(st, account, &who, m, by),
        "ban" => ban(st, account, &who, m, by, now),
        "unban" => {
            if !gm::lift_ban(st, account)? {
                return refuse(409, format!("{who} is not banned"));
            }
            let reason = reason_of(m);
            gm::log_action(st, 0, "unban", account, 0, &reason)?;
            record(
                st,
                account,
                &with_reason(format!("ban lifted by {by}"), &reason),
            );
            Ok(Ok(format!("The ban of {who} is lifted.")))
        }
        "kick" => {
            let reason = reason_of(m);
            if reason.is_empty() {
                return refuse(400, "give a reason");
            }
            if !whereabouts(st, account).0 {
                return refuse(409, format!("{who} is not online"));
            }
            let n = gm::drop_account(st, account)?;
            gm::log_action(st, 0, "kick", account, 0, &reason)?;
            record(
                st,
                account,
                &with_reason(
                    format!("kicked ({n} connection(s) dropped) by {by}"),
                    &reason,
                ),
            );
            Ok(Ok(format!("{who} was kicked ({n} connection(s) dropped).")))
        }
        "mute" => {
            let minutes = match int(m, "minutes") {
                Ok(Some(n)) if n > 0 => n,
                Ok(_) => return refuse(400, "minutes: more than 0"),
                Err(e) => return refuse(400, e),
            };
            if st.character(account).is_none() {
                return refuse(
                    409,
                    format!("{who} has no character yet: there is no chat to mute"),
                );
            }
            let minutes = gm::mute_minutes(&st.rules.ops, minutes);
            gm::set_mute(st, account, minutes)?;
            let reason = reason_of(m);
            gm::log_action(st, 0, "mute", account, minutes, &reason)?;
            record(
                st,
                account,
                &with_reason(format!("muted for {minutes} min by {by}"), &reason),
            );
            Ok(Ok(format!("{who} is muted for {minutes} minutes.")))
        }
        "unmute" => {
            if gm::mute_end(st, account) == 0 {
                return refuse(409, format!("{who} is not muted"));
            }
            gm::set_mute(st, account, 0)?;
            gm::log_action(st, 0, "unmute", account, 0, "")?;
            record(st, account, &format!("mute lifted by {by}"));
            Ok(Ok(format!("{who} may chat again.")))
        }
        "disable" | "enable" => {
            let on = action == "disable";
            if acc.disabled == on {
                return refuse(
                    409,
                    format!(
                        "{who}'s login is already {}",
                        if on { "disabled" } else { "enabled" }
                    ),
                );
            }
            crate::accounts::set_disabled(&st.db, &acc.login, on)?;
            let reason = reason_of(m);
            gm::log_action(st, 0, action, account, 0, &reason)?;
            record(
                st,
                account,
                &with_reason(
                    format!("login {} by {by}", if on { "disabled" } else { "enabled" }),
                    &reason,
                ),
            );
            Ok(Ok(if on {
                format!("{who} cannot log in any more. A session already open stays: kick them to end it.")
            } else {
                format!("{who} may log in again.")
            }))
        }
        "reset_mail" => {
            if !st.mail.enabled() {
                return refuse(
                    409,
                    "mail is not set up on this lobby ([mail] in server.toml)",
                );
            }
            let email = db::account_email(&st.db, account)?;
            if email.is_empty() {
                return refuse(409, format!("{who} gave no recovery address"));
            }
            if !crate::recover::send_code(st, &acc.login, &email, by, now)? {
                return refuse(
                    409,
                    "no mail sent: the account already got 3 codes this hour, or the mail sender refused it (see the log)",
                );
            }
            gm::log_action(st, 0, "reset_mail", account, 0, "")?;
            record(st, account, &format!("password reset code mailed by {by}"));
            Ok(Ok(format!(
                "A reset code went to {}. It works for 15 minutes.",
                crate::accounts::mask_email(&email)
            )))
        }
        "loadout" => loadout(st, account, &acc.login, &who, m, by),
        "delete" => delete(st, account, &acc, &who, m, by),
        other => refuse(404, format!("no action {other}")),
    }
}

/// OPS-14: gives and equips a loadout (`spec`, the CLI's `account loadout` syntax). Nothing changes on a refusal.
// DC-DEFAULT: OPS-14 (proposed) the Players tab equips a loadout like `account loadout` does, from the next login
fn loadout(
    st: &mut Core,
    account: u32,
    login: &str,
    who: &str,
    m: &Map<String, Value>,
    by: &str,
) -> Outcome {
    let spec = m.get("spec").and_then(Value::as_str).unwrap_or("").trim();
    let spec = match crate::botgroups::clean_loadout(spec) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => return refuse(400, "spec: random, or <slot>:<item>,... (for example 0:27)"),
        Err(e) => return refuse(400, e.replacen("loadout", "spec", 1)),
    };
    if st.character(account).is_none() {
        return refuse(409, format!("{who} has no character yet"));
    }
    st.db.execute_batch("SAVEPOINT ops_loadout")?;
    match crate::accounts::equip_loadout(&st.db, &st.rom, login, &spec, None) {
        Ok(text) => {
            st.db.execute_batch("RELEASE ops_loadout")?;
            gm::log_action(st, 0, "loadout", account, 0, &spec)?;
            record(st, account, &format!("loadout {spec} by {by}"));
            Ok(Ok(text))
        }
        Err(e) => {
            st.db
                .execute_batch("ROLLBACK TO ops_loadout; RELEASE ops_loadout")?;
            refuse(400, e)
        }
    }
}

fn grant(st: &mut Core, account: u32, who: &str, m: &Map<String, Value>, by: &str) -> Outcome {
    let mut d = [0i64; 3];
    for (i, k) in ["nx", "gp", "exp"].iter().enumerate() {
        match int(m, k) {
            Ok(v) => d[i] = v.unwrap_or(0),
            Err(e) => return refuse(400, e),
        }
    }
    let [dn, dg, de] = d;
    if d.iter().all(|v| *v == 0) {
        return refuse(400, "give an amount of NX, GP or EXP");
    }
    let reason = reason_of(m);
    if reason.is_empty() {
        return refuse(400, "give a reason");
    }
    let r = &st.rules.ops;
    for (v, cap, what, key) in [
        (dn, r.admin_max_nx, "NX", "nx"),
        (dg, r.admin_max_gp, "GP", "gp"),
        (de, r.admin_max_exp, "EXP", "exp"),
    ] {
        if v.unsigned_abs() > cap.max(0) as u64 {
            return refuse(
                400,
                format!(
                    "at most {} {what} in one change (gm.admin_max_{key})",
                    grouped(cap)
                ),
            );
        }
    }
    let Some(c) = st.character(account) else {
        return refuse(
            409,
            format!("{who} has no character yet: there is no balance to change"),
        );
    };
    for (have, v, what) in [(c.nx, dn, "NX"), (c.gp, dg, "GP"), (c.exp, de, "EXP")] {
        if have + v < 0 {
            return refuse(
                409,
                format!(
                    "that would take {what} below 0 ({who} has {})",
                    grouped(have)
                ),
            );
        }
    }
    db::grant(&st.db, account, dn, 0, dg, de)?;
    let after = st.character(account).unwrap_or(c);
    // LG-397: an online player's game shows the new values at once: 0x345 is stored whatever screen is up, 0x3004
    // without its popup (both pushes the lobby already sends after a match)
    // SPEC: PROTOCOL_2009.md §3.3 0x345, 0x3004
    let sids = st.sids_of(account);
    if dn != 0 {
        let f = wire::balance_reply(
            after.nx_cr.clamp(0, u32::MAX as i64) as u32,
            after.nx.clamp(0, u32::MAX as i64) as u32,
        );
        for &s in &sids {
            st.send(s, f.clone());
        }
    }
    if dg != 0 || de != 0 {
        let f = wire::progress_push(
            after.exp.clamp(0, u32::MAX as i64) as u32,
            after.gp.clamp(0, u32::MAX as i64) as u32,
            false,
        );
        for &s in &sids {
            st.send(s, f.clone());
        }
    }
    let parts: Vec<String> = [
        (dn, "NX", after.nx),
        (dg, "GP", after.gp),
        (de, "EXP", after.exp),
    ]
    .iter()
    .filter(|(v, ..)| *v != 0)
    .map(|(v, what, now)| format!("{what} {} (now {})", signed(*v), grouped(*now)))
    .collect();
    let text = parts.join(", ");
    gm::log_action(st, 0, "grant", account, 0, &format!("{text}: {reason}"))?;
    record(st, account, &format!("{text} by {by}: {reason}"));
    let seen = if sids.is_empty() {
        "They see it at their next login."
    } else {
        "They are online: their game shows it now."
    };
    Ok(Ok(format!("{who}: {text}. {seen}")))
}

fn ban(
    st: &mut Core,
    account: u32,
    who: &str,
    m: &Map<String, Value>,
    by: &str,
    now: i64,
) -> Outcome {
    let permanent = m.get("permanent").and_then(Value::as_bool).unwrap_or(false);
    let (hours, days) = match (int(m, "hours"), int(m, "days")) {
        (Ok(h), Ok(d)) => (h.unwrap_or(0), d.unwrap_or(0)),
        (Err(e), _) | (_, Err(e)) => return refuse(400, e),
    };
    let total = hours.saturating_add(days.saturating_mul(24));
    if !permanent && (hours < 0 || days < 0 || total <= 0) {
        return refuse(
            400,
            "give the hours or days of the ban, or make it permanent",
        );
    }
    if !permanent && total > MAX_BAN_HOURS {
        return refuse(400, "at most ten years: make it permanent instead");
    }
    let reason = reason_of(m);
    if reason.is_empty() {
        return refuse(400, "give a reason");
    }
    let until = if permanent { 0 } else { now + total * 3600 };
    let n = gm::put_ban(st, account, until, &reason, 0, by)?;
    gm::log_action(
        st,
        0,
        "ban_hours",
        account,
        if permanent { 0 } else { total },
        &reason,
    )?;
    let length = if permanent {
        "permanently".to_string()
    } else {
        format!(
            "for {} (until {})",
            hours_text(total),
            crate::opsmon::when(until)
        )
    };
    record(
        st,
        account,
        &format!("banned {length}, {n} connection(s) dropped, by {by}: {reason}"),
    );
    Ok(Ok(format!(
        "{who} is banned {length}; {n} connection(s) dropped."
    )))
}

fn hours_text(h: i64) -> String {
    if h % 24 == 0 {
        let d = h / 24;
        format!("{d} day{}", if d == 1 { "" } else { "s" })
    } else {
        format!("{h} hour{}", if h == 1 { "" } else { "s" })
    }
}

/// The tables that hang off an account and are emptied for it (LG-396); `?1` is the account number.
pub const DELETED_ROWS: &[&str] = &[
    "DELETE FROM items WHERE user_no = ?1",
    "DELETE FROM function_slots WHERE user_no = ?1",
    "DELETE FROM expiry_notes WHERE user_no = ?1",
    "DELETE FROM inbox WHERE user_no = ?1",
    "DELETE FROM notices WHERE user_no = ?1",
    "DELETE FROM friends WHERE user_no = ?1 OR friend_no = ?1",
    "DELETE FROM friend_requests WHERE from_no = ?1 OR to_no = ?1",
    "DELETE FROM recommends WHERE user_no = ?1",
    "DELETE FROM coupon_uses WHERE user_no = ?1",
    "DELETE FROM megaphone WHERE user_no = ?1 AND aired = 0 AND done = 0",
    "DELETE FROM clan_joins WHERE user_no = ?1",
    "DELETE FROM clan_requests WHERE user_no = ?1",
    "DELETE FROM op_bans WHERE user_no = ?1",
    "DELETE FROM nx_daily WHERE user_no = ?1",
    "DELETE FROM nx_clan_wins WHERE user_no = ?1",
    "DELETE FROM nx_events WHERE user_no = ?1",
    "DELETE FROM identity_links WHERE user_no = ?1",
    "DELETE FROM recovery_codes WHERE user_no = ?1",
    "UPDATE beta_signups SET user_no = 0 WHERE user_no = ?1",
    "DELETE FROM characters WHERE user_no = ?1",
    "DELETE FROM accounts WHERE user_no = ?1",
];

fn delete(
    st: &mut Core,
    account: u32,
    acc: &db::Account,
    who: &str,
    m: &Map<String, Value>,
    by: &str,
) -> Outcome {
    let typed = m
        .get("confirm")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if typed.is_empty() || !typed.eq_ignore_ascii_case(&acc.login) {
        return refuse(400, format!("type the login {} to confirm", acc.login));
    }
    if acc.gm != 0 {
        return refuse(
            409,
            "a GM account cannot be deleted here: take its GM flag away first (account gm <LOGIN> --off)",
        );
    }
    let (online, place) = whereabouts(st, account);
    if online {
        return refuse(409, format!("{who} is online ({place}): kick them first"));
    }
    // the event first, while the name is still there; the ops rows keep the account number
    let nick = st.character(account).map(|c| c.nick).unwrap_or_default();
    record(
        st,
        account,
        &format!(
            "account deleted (login {}, character {}) by {by}",
            acc.login,
            if nick.is_empty() { "-" } else { &nick }
        ),
    );
    // the clan as at a leave: a leader hands it to the longest-standing member, a leader alone disbands it
    if st.character(account).is_some_and(|c| c.clan_uid != 0) {
        let _ = crate::clan::leave(st, account)?;
    }
    orphan_clans(st, account)?;
    let n = account as i64;
    for sql in DELETED_ROWS {
        st.db.execute(sql, [n])?;
    }
    st.db.execute(
        "DELETE FROM bot_loadouts WHERE login = ?1",
        params![acc.login],
    )?;
    // what the lobby keeps in memory for the account
    crate::botpool::forget(st, account);
    st.seat_notices.remove(&account);
    st.ops.report_times.remove(&account);
    st.tickets.retain(|_, t| t.account != account);
    gm::log_action(
        st,
        0,
        "delete",
        account,
        0,
        &format!("login {}, character {}", acc.login, nick),
    )?;
    Ok(Ok(format!(
        "Account {account} ({}) is deleted with its character, items and friends.",
        acc.login
    )))
}

/// A clan whose master is `account` without the account pointing at it (a dangling row): the longest-standing
/// member leads it, or with no member it goes.
fn orphan_clans(st: &Core, account: u32) -> rusqlite::Result<()> {
    let uids: Vec<i64> = {
        let mut q = st
            .db
            .prepare("SELECT clan_uid FROM clans WHERE master = ?1")?;
        let rows = q.query_map([account as i64], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for uid in uids {
        let next: Option<i64> = st
            .db
            .query_row(
                "SELECT c.user_no FROM characters c LEFT JOIN clan_joins j ON j.user_no = c.user_no \
                 WHERE c.clan_uid = ?1 AND c.user_no != ?2 ORDER BY COALESCE(j.joined, 0), c.user_no LIMIT 1",
                params![uid, account as i64],
                |r| r.get(0),
            )
            .optional()?;
        match next {
            Some(no) => {
                st.db.execute(
                    "UPDATE clans SET master = ?2 WHERE clan_uid = ?1",
                    params![uid, no],
                )?;
                st.db.execute(
                    "UPDATE characters SET clan_type = ?2 WHERE user_no = ?1",
                    params![no, crate::clan::ROLE_LEADER],
                )?;
            }
            None => {
                for t in ["clan_records", "clan_colours", "clans"] {
                    st.db
                        .execute(&format!("DELETE FROM {t} WHERE clan_uid = ?1"), [uid])?;
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------- HTTP

/// LG-397: a JSON body, the page's own `Origin`, the page's token in `X-Admin-CSRF` (the Bots tab's guard).
fn post_guard(head: &str, origin: &str, csrf: &str) -> Result<(), Reply> {
    let ct = header(head, "content-type")
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ct.starts_with("application/json") {
        return Err(jerr(415, "send application/json"));
    }
    if !header(head, "origin").is_some_and(|o| !origin.is_empty() && o.eq_ignore_ascii_case(origin))
    {
        return Err(jerr(
            403,
            "the request does not come from the dashboard (Origin)",
        ));
    }
    let tok = header(head, "x-admin-csrf").unwrap_or("");
    if csrf.is_empty() || !crate::hosts::token_eq(tok, csrf) {
        return Err(jerr(403, "missing or wrong page token: reload the page"));
    }
    Ok(())
}

/// One request under `/ops/player` (the caller checked loopback, Host and the admin token, or the admin web session).
pub fn serve(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    guard: Option<&WriteGuard>,
) -> Reply {
    serve_at(shared, req, head, body, guard, clock::unix_now())
}

/// [`serve`] at unix time `now`.
pub fn serve_at(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    guard: Option<&WriteGuard>,
    now: i64,
) -> Reply {
    let path = req.path.trim_end_matches('/');
    match (req.method.as_str(), path) {
        ("GET", "/ops/player") => {
            let Some(a) = req
                .query
                .get("account")
                .and_then(|v| v.trim().parse::<u32>().ok())
            else {
                return jerr(400, "account=<account number>");
            };
            let g = crate::net::lock(shared);
            match account_json(&g, a, now) {
                Ok(Some(v)) => jok(v),
                Ok(None) => jerr(404, &format!("no account {a}")),
                Err(e) => jerr(500, &format!("db: {e}")),
            }
        }
        ("POST", p) if action_of(p).is_some() => {
            let action = action_of(p).unwrap_or_default();
            let mut g = crate::net::lock(shared);
            let st = &mut *g;
            let host = header(head, "host").unwrap_or("");
            let (origin, csrf, by) = match guard {
                Some(w) => (
                    w.origin.clone(),
                    w.csrf.clone(),
                    if w.by.is_empty() {
                        "the admin web".to_string()
                    } else {
                        w.by.clone()
                    },
                ),
                None if host.is_empty() => {
                    return jerr(403, "the request does not come from the dashboard (Origin)")
                }
                // LG-400: the default lobby's admin web forwarding to this port names its key and client address
                // (only a caller past this port's loopback, Host and token checks and this write guard gets here)
                None => (
                    format!("http://{host}"),
                    st.beta.csrf(&mut st.keys),
                    header(head, "x-admin-via")
                        .map(crate::adminfwd::clean_via)
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| "the admin port".to_string()),
                ),
            };
            if let Err(r) = post_guard(head, &origin, &csrf) {
                return r;
            }
            let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
                return jerr(400, "send a JSON object");
            };
            let Some(account) = m
                .get("account")
                .and_then(Value::as_u64)
                .filter(|n| (1..=u64::from(u32::MAX)).contains(n))
            else {
                return jerr(400, "account: the account number");
            };
            let mut out = None;
            let r = st.scope(|st| {
                out = Some(act(st, action, account as u32, &m, &by, now)?);
                Ok(())
            });
            crate::net::run_deferred(shared, st);
            match (r, out) {
                (Err(e), _) => jerr(500, &format!("{e}")),
                (Ok(()), Some(Ok(msg))) => jok(json!({"ok": true, "message": msg})),
                (Ok(()), Some(Err((status, msg)))) => jerr(status, &msg),
                (Ok(()), None) => jerr(500, "no answer"),
            }
        }
        (_, p) if p == "/ops/player" || action_of(p).is_some() => jerr(405, "method not allowed"),
        _ => jerr(404, "not found"),
    }
}

#[cfg(test)]
#[path = "playeradmin_tests.rs"]
mod tests;
