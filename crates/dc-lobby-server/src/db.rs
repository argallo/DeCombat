//! SQLite schema and row access.
//!
//! The table and column names the harness reads directly are fixed by SPEC: LOBBY_HARNESS.md §6 (accounts,
//! characters, items, inbox, friends, clans, match_log, reports, recommends); every other column is our own.
//! The harness opens the file with its own connection while the server runs, so the DB uses WAL journaling and
//! every handler commits before it replies (SPEC: LOBBY_HARNESS.md §1 "consistent after a kill").

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub type DbResult<T> = rusqlite::Result<T>;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts (
    user_no     INTEGER PRIMARY KEY AUTOINCREMENT,
    login       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    pw_hash     TEXT,
    pw_salt     TEXT,
    gm          INTEGER NOT NULL DEFAULT 0,
    created     INTEGER NOT NULL DEFAULT 0,
    last_login  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS characters (
    user_no        INTEGER PRIMARY KEY,
    nick           TEXT NOT NULL,
    nick_key       TEXT NOT NULL UNIQUE,
    exp            INTEGER NOT NULL DEFAULT 0,
    gp             INTEGER NOT NULL DEFAULT 0,
    nx             INTEGER NOT NULL DEFAULT 0,
    nx_cr          INTEGER NOT NULL DEFAULT 0,
    kills          INTEGER NOT NULL DEFAULT 0,
    deaths         INTEGER NOT NULL DEFAULT 0,
    wins           INTEGER NOT NULL DEFAULT 0,
    losses         INTEGER NOT NULL DEFAULT 0,
    headshots      INTEGER NOT NULL DEFAULT 0,
    matches        INTEGER NOT NULL DEFAULT 0,
    nick_color     INTEGER NOT NULL DEFAULT 0,
    options        INTEGER NOT NULL DEFAULT 0,
    clan_uid       INTEGER NOT NULL DEFAULT 0,
    clan_name      TEXT NOT NULL DEFAULT '',
    clan_type      INTEGER NOT NULL DEFAULT 0,
    face           INTEGER NOT NULL DEFAULT 0,
    nation         INTEGER NOT NULL DEFAULT 0,
    grade          INTEGER NOT NULL DEFAULT 0,
    mute_until     INTEGER NOT NULL DEFAULT 0,
    notice_pending INTEGER NOT NULL DEFAULT 0,
    created        INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS items (
    uid          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no      INTEGER NOT NULL,
    item_no      INTEGER NOT NULL,
    start_time   INTEGER NOT NULL DEFAULT 0,
    end_time     INTEGER NOT NULL DEFAULT 0,
    equip_slot   INTEGER NOT NULL DEFAULT -1,
    default_type INTEGER NOT NULL DEFAULT 0,
    parent_uid   INTEGER NOT NULL DEFAULT 0,
    use_type     INTEGER NOT NULL DEFAULT 0,
    remain       INTEGER NOT NULL DEFAULT 0,
    used         INTEGER NOT NULL DEFAULT 0,
    func_value   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS items_by_user ON items(user_no);
CREATE TABLE IF NOT EXISTS expiry_notes (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no  INTEGER NOT NULL,
    item_no  INTEGER NOT NULL,
    created  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS expiry_notes_by_user ON expiry_notes(user_no);
CREATE TABLE IF NOT EXISTS inbox (
    uproduct    INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no     INTEGER NOT NULL,
    product_id  INTEGER NOT NULL,
    created     INTEGER NOT NULL DEFAULT 0,
    gift_kind   INTEGER NOT NULL DEFAULT 0,
    sender      TEXT NOT NULL DEFAULT '',
    message     TEXT NOT NULL DEFAULT '',
    opened      INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS friends (
    user_no    INTEGER NOT NULL,
    friend_no  INTEGER NOT NULL,
    PRIMARY KEY (user_no, friend_no)
);
CREATE TABLE IF NOT EXISTS clans (
    clan_uid    INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    master      INTEGER NOT NULL,
    emblem      INTEGER NOT NULL DEFAULT 0,
    background  INTEGER NOT NULL DEFAULT 0,
    notice      TEXT NOT NULL DEFAULT '',
    created     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS match_log (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    ended    INTEGER NOT NULL DEFAULT 0,
    channel  INTEGER NOT NULL DEFAULT 0,
    room     INTEGER NOT NULL DEFAULT 0,
    map      INTEGER NOT NULL DEFAULT 0,
    mode     INTEGER NOT NULL DEFAULT 0,
    mode_key TEXT NOT NULL DEFAULT '',
    winner   INTEGER NOT NULL DEFAULT 0,
    alpha    INTEGER NOT NULL DEFAULT 0,
    bravo    INTEGER NOT NULL DEFAULT 0,
    players  INTEGER NOT NULL DEFAULT 0,
    detail   TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS reports (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    kind         INTEGER NOT NULL DEFAULT 0,
    target_nick  TEXT NOT NULL DEFAULT '',
    text         TEXT NOT NULL DEFAULT '',
    extra        TEXT NOT NULL DEFAULT '',
    reporter     INTEGER NOT NULL DEFAULT 0,
    created      INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS recommends (
    user_no      INTEGER PRIMARY KEY,
    recommender  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS shop_lists (
    list     INTEGER PRIMARY KEY,
    hash     INTEGER NOT NULL,
    version  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS shop_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no     INTEGER NOT NULL,
    product_id  INTEGER NOT NULL,
    price       INTEGER NOT NULL DEFAULT 0,
    currency    INTEGER NOT NULL DEFAULT 0,
    receiver    INTEGER NOT NULL DEFAULT 0,
    created     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS notices (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no  INTEGER NOT NULL,
    text     TEXT NOT NULL,
    created  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS coupons (
    code        TEXT PRIMARY KEY,
    product_id  INTEGER NOT NULL,
    uses        INTEGER NOT NULL DEFAULT -1,
    created     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS coupon_uses (
    code     TEXT NOT NULL,
    user_no  INTEGER NOT NULL,
    used     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (code, user_no)
);
CREATE TABLE IF NOT EXISTS megaphone (
    serial   INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no  INTEGER NOT NULL,
    name     TEXT NOT NULL DEFAULT '',
    colour   INTEGER NOT NULL DEFAULT 0,
    text     TEXT NOT NULL DEFAULT '',
    runs     BLOB,
    queued   INTEGER NOT NULL DEFAULT 0,
    aired    INTEGER NOT NULL DEFAULT 0,
    done     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS function_slots (
    uid      INTEGER PRIMARY KEY,
    user_no  INTEGER NOT NULL,
    slot     INTEGER NOT NULL
);
-- LG-407: a Weapon Free Pass's weapon in an equipment slot (no item row of its own; the pass's row is pass_uid)
CREATE TABLE IF NOT EXISTS free_pass_slots (
    user_no  INTEGER NOT NULL,
    slot     INTEGER NOT NULL,
    item_no  INTEGER NOT NULL,
    pass_uid INTEGER NOT NULL,
    PRIMARY KEY (user_no, slot)
);
CREATE TABLE IF NOT EXISTS clan_colours (
    clan_uid  INTEGER PRIMARY KEY,
    colour    INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS op_bans (
    user_no  INTEGER PRIMARY KEY,
    until    INTEGER NOT NULL DEFAULT 0,
    reason   TEXT NOT NULL DEFAULT '',
    by_no    INTEGER NOT NULL DEFAULT 0,
    created  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS op_actions (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    by_no    INTEGER NOT NULL,
    action   TEXT NOT NULL,
    target   INTEGER NOT NULL DEFAULT 0,
    period   INTEGER NOT NULL DEFAULT 0,
    reason   TEXT NOT NULL DEFAULT '',
    created  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS friend_requests (
    from_no  INTEGER NOT NULL,
    to_no    INTEGER NOT NULL,
    created  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (from_no, to_no)
);
-- LG-420: open clan applications ('apply', the player asked) and invitations ('invite', the leader asked); one row
-- per player and clan. A new table: an older DB gets it empty at the next start (nothing to migrate); a join, a
-- create, a disband or an account delete removes the rows.
CREATE TABLE IF NOT EXISTS clan_requests (
    clan_uid  INTEGER NOT NULL,
    user_no   INTEGER NOT NULL,
    kind      TEXT NOT NULL,
    created   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (clan_uid, user_no)
);
"#;

/// Opens (creating if needed) the DB file with its schema.
pub fn open(path: &Path) -> DbResult<Connection> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let conn = Connection::open(path)?;
    prepare(&conn)?;
    Ok(conn)
}

/// Pragmas and schema (also used for in-memory test DBs).
pub fn prepare(conn: &Connection) -> DbResult<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // WAL: readers (the harness) never block the server's writer and vice versa.
    let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(crate::clan::SCHEMA)?;
    // reports gained the picked category (0x312 +0xa / +0xe) after the first schema
    let mut has = false;
    conn.pragma(None, "table_info", "reports", |r| {
        let name: String = r.get(1)?;
        has |= name == "category";
        Ok(())
    })?;
    if !has {
        conn.execute_batch(
            "ALTER TABLE reports ADD COLUMN category INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE reports ADD COLUMN sub INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // LG-316: reports gained the mail state and what the mail says about the reporter. Older rows are not mailed
    // (`mailed` 2): turning the mail on must not send the operator a year of old reports.
    let mut has = false;
    conn.pragma(None, "table_info", "reports", |r| {
        let name: String = r.get(1)?;
        has |= name == "mailed";
        Ok(())
    })?;
    if !has {
        conn.execute_batch(
            "ALTER TABLE reports ADD COLUMN mailed INTEGER NOT NULL DEFAULT 2;
             ALTER TABLE reports ADD COLUMN mailed_at INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE reports ADD COLUMN reporter_login TEXT NOT NULL DEFAULT '';
             ALTER TABLE reports ADD COLUMN reporter_nick TEXT NOT NULL DEFAULT '';
             ALTER TABLE reports ADD COLUMN place TEXT NOT NULL DEFAULT '';",
        )?;
    }
    // LG-317: an account a stock dc-botclient logged into
    let mut has = false;
    conn.pragma(None, "table_info", "accounts", |r| {
        let name: String = r.get(1)?;
        has |= name == "self_bot";
        Ok(())
    })?;
    if !has {
        conn.execute_batch("ALTER TABLE accounts ADD COLUMN self_bot INTEGER NOT NULL DEFAULT 0;")?;
    }
    // LG-318: an account of the bot pool
    let mut has = false;
    conn.pragma(None, "table_info", "accounts", |r| {
        let name: String = r.get(1)?;
        has |= name == "pool_bot";
        Ok(())
    })?;
    if !has {
        conn.execute_batch("ALTER TABLE accounts ADD COLUMN pool_bot INTEGER NOT NULL DEFAULT 0;")?;
    }
    // accounts gained the disabled flag (LG-142) after the first schema; invite codes (LG-143)
    let mut has = false;
    conn.pragma(None, "table_info", "accounts", |r| {
        let name: String = r.get(1)?;
        has |= name == "disabled";
        Ok(())
    })?;
    if !has {
        conn.execute_batch("ALTER TABLE accounts ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0;")?;
    }
    conn.execute_batch(INVITES)?;
    // accounts gained the recovery address (LG-240) after the first schema: existing rows get an empty one
    let mut has = false;
    conn.pragma(None, "table_info", "accounts", |r| {
        let name: String = r.get(1)?;
        has |= name == "email";
        Ok(())
    })?;
    if !has {
        conn.execute_batch(
            "ALTER TABLE accounts ADD COLUMN email TEXT NOT NULL DEFAULT '' COLLATE NOCASE;",
        )?;
    }
    conn.execute_batch(RECOVERY)?;
    // LG-370: the beta signups and the mails sent to them
    conn.execute_batch(BETA)?;
    // LG-384: the admin web's security keys and its one-time enrolment links
    conn.execute_batch(ADMIN_WEB)?;
    // LG-303: characters gained the highest rank whose promotion was rewarded; existing rows start at 0 (a rank-up
    // pays the ranks above the rank the match started from, so no older rank is paid late)
    let mut has = false;
    conn.pragma(None, "table_info", "characters", |r| {
        let name: String = r.get(1)?;
        has |= name == "rank_rewarded";
        Ok(())
    })?;
    if !has {
        conn.execute_batch(
            "ALTER TABLE characters ADD COLUMN rank_rewarded INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // LG-396: a ban names who set it when that was not a GM account (the dashboard: the admin port or the key)
    let mut has = false;
    conn.pragma(None, "table_info", "op_bans", |r| {
        let name: String = r.get(1)?;
        has |= name == "by_name";
        Ok(())
    })?;
    if !has {
        conn.execute_batch("ALTER TABLE op_bans ADD COLUMN by_name TEXT NOT NULL DEFAULT '';")?;
    }
    // LG-425 (security audit M5): identity tokens issued before an account's last password write are refused;
    // existing rows start at 0 (every token stays valid until the next password change)
    let mut has = false;
    conn.pragma(None, "table_info", "accounts", |r| {
        let name: String = r.get(1)?;
        has |= name == "tokens_valid_after";
        Ok(())
    })?;
    if !has {
        conn.execute_batch(
            "ALTER TABLE accounts ADD COLUMN tokens_valid_after INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // LG-69: the local accounts of identity tokens (a new table: older DBs get it empty)
    conn.execute_batch(IDENTITY_LINKS)?;
    // LG-310..LG-312: the NX reward claims (new tables: older DBs get them empty)
    conn.execute_batch(NX_REWARDS)?;
    // MON-2 / MON-3: the operator monitor's tables (new tables: older DBs get them empty)
    conn.execute_batch(crate::opsmon::SCHEMA)?;
    // LG-389: the dashboard's bot groups (new tables: older DBs get them empty)
    conn.execute_batch(crate::botgroups::SCHEMA)?;
    Ok(())
}

/// LG-310: the last paid day per account; LG-311: the newest day's paid clan wins per account; LG-312: one row per
/// event and account paid.
const NX_REWARDS: &str = r#"
CREATE TABLE IF NOT EXISTS nx_daily (
    user_no   INTEGER PRIMARY KEY,
    last_day  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS nx_clan_wins (
    user_no  INTEGER PRIMARY KEY,
    day      INTEGER NOT NULL,
    wins     INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS nx_events (
    event_id  TEXT NOT NULL,
    user_no   INTEGER NOT NULL,
    amount    INTEGER NOT NULL,
    label     TEXT NOT NULL,
    created   INTEGER NOT NULL,
    PRIMARY KEY (event_id, user_no)
);
"#;

/// LG-69: which local account belongs to (issuer id, the issuer's account number).
const IDENTITY_LINKS: &str = r#"
CREATE TABLE IF NOT EXISTS identity_links (
    issuer     TEXT NOT NULL,
    remote_no  INTEGER NOT NULL,
    user_no    INTEGER NOT NULL UNIQUE,
    linked     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (issuer, remote_no)
);
"#;

/// The local account linked to (issuer, remote account), if any.
pub fn identity_link(c: &Connection, issuer: &str, remote_no: u32) -> DbResult<Option<u32>> {
    c.query_row(
        "SELECT user_no FROM identity_links WHERE issuer = ?1 AND remote_no = ?2",
        params![issuer, remote_no as i64],
        |r| Ok(r.get::<_, i64>(0)? as u32),
    )
    .optional()
}

/// True when the local account was made for an identity token (it has no password login).
pub fn is_identity_linked(c: &Connection, user_no: u32) -> DbResult<bool> {
    c.query_row(
        "SELECT 1 FROM identity_links WHERE user_no = ?1",
        params![user_no as i64],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
}

/// Links a local account to (issuer, remote account).
pub fn link_identity(
    c: &Connection,
    issuer: &str,
    remote_no: u32,
    user_no: u32,
    now: i64,
) -> DbResult<()> {
    c.execute(
        "INSERT INTO identity_links (issuer, remote_no, user_no, linked) VALUES (?1, ?2, ?3, ?4)",
        params![issuer, remote_no as i64, user_no as i64, now],
    )?;
    Ok(())
}

/// Password-recovery codes (LG-242): the SHA-256 of the code, never the code.
const RECOVERY: &str = r#"
CREATE TABLE IF NOT EXISTS recovery_codes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_no    INTEGER NOT NULL,
    code_hash  TEXT NOT NULL,
    created    INTEGER NOT NULL,
    expires    INTEGER NOT NULL,
    tries      INTEGER NOT NULL DEFAULT 0,
    used       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS recovery_codes_user ON recovery_codes (user_no, created);
"#;

/// The admin web sign-in (LG-382..LG-387): one row per enrolled hardware security key (`cred_id` is the WebAuthn
/// credential id, `public_key` its COSE_Key, `counter` the newest signature counter, `aaguid` the model as hex, `root`
/// the attestation root's name, `transports` what the browser reported, comma separated), and the one-time enrolment
/// links of `dc-lobby-server admin-web enroll` (only the SHA-256 of the link's token is stored).
const ADMIN_WEB: &str = r#"
CREATE TABLE IF NOT EXISTS admin_web_keys (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    cred_id    BLOB NOT NULL UNIQUE,
    public_key BLOB NOT NULL,
    alg        INTEGER NOT NULL,
    counter    INTEGER NOT NULL DEFAULT 0,
    aaguid     TEXT NOT NULL DEFAULT '',
    root       TEXT NOT NULL DEFAULT '',
    transports TEXT NOT NULL DEFAULT '',
    label      TEXT NOT NULL DEFAULT '',
    created    INTEGER NOT NULL DEFAULT 0,
    last_used  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS admin_web_enroll (
    token_hash TEXT PRIMARY KEY,
    label      TEXT NOT NULL DEFAULT '',
    created    INTEGER NOT NULL DEFAULT 0,
    expires    INTEGER NOT NULL DEFAULT 0,
    used       INTEGER NOT NULL DEFAULT 0
);
"#;

/// The beta signup list and its mail log (LG-370..LG-373). `email` is unique without case; the tokens are random
/// 128-bit values; `status` is pending, confirmed, invited, registered or unsubscribed; `state` of a mail is 0 queued,
/// 1 sent, 2 failed.
const BETA: &str = r#"
CREATE TABLE IF NOT EXISTS beta_signups (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    email         TEXT NOT NULL UNIQUE COLLATE NOCASE,
    platform      TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'pending',
    created       INTEGER NOT NULL DEFAULT 0,
    confirmed     INTEGER NOT NULL DEFAULT 0,
    invited       INTEGER NOT NULL DEFAULT 0,
    invite_code   TEXT NOT NULL DEFAULT '',
    user_no       INTEGER NOT NULL DEFAULT 0,
    confirm_token TEXT NOT NULL DEFAULT '',
    unsub_token   TEXT NOT NULL DEFAULT '',
    confirm_sent  INTEGER NOT NULL DEFAULT 0,
    ip            TEXT NOT NULL DEFAULT '',
    note          TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS beta_signups_status ON beta_signups (status, created);
CREATE INDEX IF NOT EXISTS beta_signups_confirm ON beta_signups (confirm_token);
CREATE INDEX IF NOT EXISTS beta_signups_unsub ON beta_signups (unsub_token);
CREATE INDEX IF NOT EXISTS beta_signups_code ON beta_signups (invite_code);
CREATE TABLE IF NOT EXISTS beta_mail_log (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    signup_id INTEGER NOT NULL,
    kind      TEXT NOT NULL,
    at        INTEGER NOT NULL,
    state     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS beta_mail_log_at ON beta_mail_log (at);
CREATE INDEX IF NOT EXISTS beta_mail_log_signup ON beta_mail_log (signup_id, id);
"#;

/// The recovery address of an account (empty when none was given).
pub fn account_email(c: &Connection, user_no: u32) -> DbResult<String> {
    c.query_row(
        "SELECT email FROM accounts WHERE user_no = ?1",
        params![user_no as i64],
        |r| r.get(0),
    )
    .optional()
    .map(Option::unwrap_or_default)
}

/// Sets the recovery address of an account (stored as given; compared without case).
pub fn set_email(c: &Connection, user_no: u32, email: &str) -> DbResult<()> {
    c.execute(
        "UPDATE accounts SET email = ?2 WHERE user_no = ?1",
        params![user_no as i64, email],
    )?;
    Ok(())
}

/// Login names registered with `email` (case-insensitive), in account order.
pub fn logins_by_email(c: &Connection, email: &str) -> DbResult<Vec<String>> {
    if email.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = c.prepare("SELECT login FROM accounts WHERE email = ?1 ORDER BY user_no")?;
    let rows = q.query_map(params![email], |r| r.get(0))?;
    rows.collect()
}

const INVITES: &str = r#"
CREATE TABLE IF NOT EXISTS invites (
    code     TEXT PRIMARY KEY COLLATE NOCASE,
    login    TEXT COLLATE NOCASE,
    created  INTEGER NOT NULL DEFAULT 0,
    expires  INTEGER NOT NULL DEFAULT 0,
    used_by  INTEGER NOT NULL DEFAULT 0,
    used_at  INTEGER NOT NULL DEFAULT 0
);
"#;

#[derive(Clone, Debug, Default)]
pub struct Account {
    pub user_no: u32,
    pub login: String,
    pub pw_hash: Option<String>,
    pub pw_salt: Option<String>,
    pub gm: i64,
    /// Refused at login (LG-142; `dc-lobby-server account disable`).
    pub disabled: bool,
    /// LG-425: unix seconds of the last password write; an identity token issued before it is refused.
    pub tokens_valid_after: i64,
}

/// Row mirror of `characters` (plus clan mark and GM flag); not every column is used by the core server yet.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Character {
    pub user_no: u32,
    pub nick: String,
    pub exp: i64,
    pub gp: i64,
    pub nx: i64,
    pub nx_cr: i64,
    pub kills: i64,
    pub deaths: i64,
    pub wins: i64,
    pub losses: i64,
    pub headshots: i64,
    pub matches: i64,
    pub nick_color: i64,
    pub options: i64,
    pub clan_uid: i64,
    pub clan_title: String,
    pub clan_role: i64,
    pub face: i64,
    pub nation: i64,
    pub grade: i64,
    pub mute_until: i64,
    pub notice_pending: i64,
    pub emblem: i64,
    pub background: i64,
    pub gm: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub uid: i64,
    pub user_no: u32,
    pub item_no: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub slot: i64,
    pub default_type: i64,
    pub parent_uid: i64,
    pub use_type: i64,
    pub remain: i64,
    pub used: i64,
    pub func_value: i64,
}

pub fn account_by_login(c: &Connection, login: &str) -> DbResult<Option<Account>> {
    c.query_row(
        "SELECT user_no, login, pw_hash, pw_salt, gm, disabled, tokens_valid_after FROM accounts WHERE login = ?1",
        params![login],
        |r| {
            Ok(Account {
                user_no: r.get::<_, i64>(0)? as u32,
                login: r.get(1)?,
                pw_hash: r.get(2)?,
                pw_salt: r.get(3)?,
                gm: r.get(4)?,
                disabled: r.get::<_, i64>(5)? != 0,
                tokens_valid_after: r.get(6)?,
            })
        },
    )
    .optional()
}

pub fn account_by_no(c: &Connection, user_no: u32) -> DbResult<Option<Account>> {
    c.query_row(
        "SELECT user_no, login, pw_hash, pw_salt, gm, disabled, tokens_valid_after FROM accounts WHERE user_no = ?1",
        params![user_no as i64],
        |r| {
            Ok(Account {
                user_no: r.get::<_, i64>(0)? as u32,
                login: r.get(1)?,
                pw_hash: r.get(2)?,
                pw_salt: r.get(3)?,
                gm: r.get(4)?,
                disabled: r.get::<_, i64>(5)? != 0,
                tokens_valid_after: r.get(6)?,
            })
        },
    )
    .optional()
}

pub fn create_account(c: &Connection, login: &str, now: i64) -> DbResult<u32> {
    c.execute(
        "INSERT INTO accounts (login, created, last_login) VALUES (?1, ?2, ?2)",
        params![login, now],
    )?;
    Ok(c.last_insert_rowid() as u32)
}

/// Like [`create_account`], but the very first account of an empty, never-used table gets the number `first`
/// (when it is above 1); a database that already holds or once held an account continues its own numbering.
// DC-DEFAULT: AUTH-1 (proposed) auth.first_account_no, default 1001 (numbering parity with the capture reference)
pub fn create_account_from(c: &Connection, login: &str, now: i64, first: u32) -> DbResult<u32> {
    let used: i64 = c.query_row(
        "SELECT (SELECT COUNT(*) FROM accounts) + (SELECT COUNT(*) FROM sqlite_sequence WHERE name = 'accounts')",
        [],
        |r| r.get(0),
    )?;
    if used == 0 && first > 1 {
        c.execute(
            "INSERT INTO accounts (user_no, login, created, last_login) VALUES (?1, ?2, ?3, ?3)",
            params![first as i64, login, now],
        )?;
        return Ok(first);
    }
    create_account(c, login, now)
}

pub fn touch_login(c: &Connection, user_no: u32, now: i64) -> DbResult<()> {
    c.execute(
        "UPDATE accounts SET last_login = ?2 WHERE user_no = ?1",
        params![user_no as i64, now],
    )?;
    Ok(())
}

/// Old salted SHA-256 store (tests make such rows to check the LG-140 upgrade).
#[cfg(test)]
pub fn set_password(c: &Connection, user_no: u32, hash: &str, salt: &str) -> DbResult<()> {
    c.execute(
        "UPDATE accounts SET pw_hash = ?2, pw_salt = ?3 WHERE user_no = ?1",
        params![user_no as i64, hash, salt],
    )?;
    Ok(())
}

/// Stores a new password as an Argon2id PHC string; the salt column is cleared (it marks the old SHA-256 rows,
/// LG-140). Every identity token of the account issued before now stops logging in (LG-425).
pub fn set_password_phc(c: &Connection, user_no: u32, phc: &str) -> DbResult<()> {
    set_password_phc_at(c, user_no, phc, crate::clock::unix_now())
}

/// [`set_password_phc`] at the unix time `now` (tests move the clock). The mark never moves back.
// DC-DEFAULT: LG-425 (proposed) tokens_valid_after = the time of the last password write
pub fn set_password_phc_at(c: &Connection, user_no: u32, phc: &str, now: i64) -> DbResult<()> {
    c.execute(
        "UPDATE accounts SET pw_hash = ?2, pw_salt = NULL, tokens_valid_after = MAX(tokens_valid_after, ?3)          WHERE user_no = ?1",
        params![user_no as i64, phc, now],
    )?;
    Ok(())
}

/// LG-140: rewrites the SAME password in the Argon2id form (an old salted SHA-256 row after a good login). The
/// password did not change, so the account's identity tokens stay valid (LG-425).
pub fn rehash_password_phc(c: &Connection, user_no: u32, phc: &str) -> DbResult<()> {
    c.execute(
        "UPDATE accounts SET pw_hash = ?2, pw_salt = NULL WHERE user_no = ?1",
        params![user_no as i64, phc],
    )?;
    Ok(())
}

const CHAR_COLS: &str = "c.user_no, c.nick, c.exp, c.gp, c.nx, c.nx_cr, c.kills, c.deaths, c.wins, c.losses, \
    c.headshots, c.matches, c.nick_color, c.options, c.clan_uid, c.clan_name, c.clan_type, c.face, c.nation, \
    c.grade, c.mute_until, c.notice_pending, COALESCE(k.emblem, 0), COALESCE(k.background, 0), \
    COALESCE(a.gm, 0)";

fn char_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Character> {
    Ok(Character {
        user_no: r.get::<_, i64>(0)? as u32,
        nick: r.get(1)?,
        exp: r.get(2)?,
        gp: r.get(3)?,
        nx: r.get(4)?,
        nx_cr: r.get(5)?,
        kills: r.get(6)?,
        deaths: r.get(7)?,
        wins: r.get(8)?,
        losses: r.get(9)?,
        headshots: r.get(10)?,
        matches: r.get(11)?,
        nick_color: r.get(12)?,
        options: r.get(13)?,
        clan_uid: r.get(14)?,
        clan_title: r.get(15)?,
        clan_role: r.get(16)?,
        face: r.get(17)?,
        nation: r.get(18)?,
        grade: r.get(19)?,
        mute_until: r.get(20)?,
        notice_pending: r.get(21)?,
        emblem: r.get(22)?,
        background: r.get(23)?,
        gm: r.get(24)?,
    })
}

pub fn character(c: &Connection, user_no: u32) -> DbResult<Option<Character>> {
    let sql = format!(
        "SELECT {CHAR_COLS} FROM characters c LEFT JOIN clans k ON k.clan_uid = c.clan_uid AND c.clan_uid != 0 \
         LEFT JOIN accounts a ON a.user_no = c.user_no WHERE c.user_no = ?1"
    );
    c.query_row(&sql, params![user_no as i64], char_row)
        .optional()
}

/// Key under which nick uniqueness is enforced.
// DC-DEFAULT: CD-16 names unique per server, case-insensitive (config names.case_insensitive)
pub fn nick_key(nick: &str, case_insensitive: bool) -> String {
    if case_insensitive {
        nick.to_lowercase()
    } else {
        nick.to_string()
    }
}

pub fn character_by_nick(
    c: &Connection,
    nick: &str,
    case_insensitive: bool,
) -> DbResult<Option<Character>> {
    let sql = format!(
        "SELECT {CHAR_COLS} FROM characters c LEFT JOIN clans k ON k.clan_uid = c.clan_uid AND c.clan_uid != 0 \
         LEFT JOIN accounts a ON a.user_no = c.user_no WHERE c.nick_key = ?1"
    );
    c.query_row(&sql, params![nick_key(nick, case_insensitive)], char_row)
        .optional()
}

pub struct NewCharacter<'a> {
    pub user_no: u32,
    pub nick: &'a str,
    pub face: i64,
    pub exp: i64,
    pub gp: i64,
    pub nx: i64,
    pub now: i64,
}

pub fn create_character(
    c: &Connection,
    n: &NewCharacter<'_>,
    case_insensitive: bool,
) -> DbResult<()> {
    c.execute(
        "INSERT INTO characters (user_no, nick, nick_key, face, exp, gp, nx, nx_cr, created) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
        params![
            n.user_no as i64,
            n.nick,
            nick_key(n.nick, case_insensitive),
            n.face,
            n.exp,
            n.gp,
            n.nx,
            n.now
        ],
    )?;
    Ok(())
}

const ITEM_COLS: &str =
    "uid, user_no, item_no, start_time, end_time, equip_slot, default_type, parent_uid, \
    use_type, remain, used, func_value";

fn item_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        uid: r.get(0)?,
        user_no: r.get::<_, i64>(1)? as u32,
        item_no: r.get(2)?,
        start_time: r.get(3)?,
        end_time: r.get(4)?,
        slot: r.get(5)?,
        default_type: r.get(6)?,
        parent_uid: r.get(7)?,
        use_type: r.get(8)?,
        remain: r.get(9)?,
        used: r.get(10)?,
        func_value: r.get(11)?,
    })
}

pub fn items(c: &Connection, user_no: u32) -> DbResult<Vec<Item>> {
    let mut st = c.prepare(&format!(
        "SELECT {ITEM_COLS} FROM items WHERE user_no = ?1 ORDER BY uid"
    ))?;
    let rows = st.query_map(params![user_no as i64], item_row)?;
    rows.collect()
}

pub fn item(c: &Connection, uid: i64) -> DbResult<Option<Item>> {
    c.query_row(
        &format!("SELECT {ITEM_COLS} FROM items WHERE uid = ?1"),
        params![uid],
        item_row,
    )
    .optional()
}

pub struct NewItem {
    pub user_no: u32,
    pub item_no: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub slot: i64,
    pub default_type: i64,
}

pub fn add_item(c: &Connection, n: &NewItem) -> DbResult<i64> {
    c.execute(
        "INSERT INTO items (user_no, item_no, start_time, end_time, equip_slot, default_type) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            n.user_no as i64,
            n.item_no,
            n.start_time,
            n.end_time,
            n.slot,
            n.default_type
        ],
    )?;
    Ok(c.last_insert_rowid())
}

pub fn set_slot(c: &Connection, uid: i64, slot: i64) -> DbResult<()> {
    c.execute(
        "UPDATE items SET equip_slot = ?2 WHERE uid = ?1",
        params![uid, slot],
    )?;
    Ok(())
}

/// Items whose end time (local FILETIME, 0 = permanent) has passed; `user` limits the scan to one account.
pub fn expired_items(c: &Connection, now_ticks: i64, user: Option<u32>) -> DbResult<Vec<Item>> {
    let (sql, uno) = match user {
        Some(u) => (
            format!("SELECT {ITEM_COLS} FROM items WHERE end_time > 0 AND end_time <= ?1 AND user_no = ?2 ORDER BY uid"),
            u as i64,
        ),
        None => (format!("SELECT {ITEM_COLS} FROM items WHERE end_time > 0 AND end_time <= ?1 AND ?2 = ?2 ORDER BY uid"), 0),
    };
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![now_ticks, uno], item_row)?;
    rows.collect()
}

/// CD-87: remembers the item numbers of expired items the owner was not online to be told about (one row each);
/// the next lobby login takes them.
pub fn add_expiry_notes(
    c: &Connection,
    user: u32,
    item_nos: &[u32],
    now_ticks: i64,
) -> DbResult<()> {
    let mut st =
        c.prepare("INSERT INTO expiry_notes (user_no, item_no, created) VALUES (?1, ?2, ?3)")?;
    for n in item_nos {
        st.execute(params![user as i64, *n as i64, now_ticks])?;
    }
    Ok(())
}

/// CD-87: the item numbers noted for `user` since the last login, oldest first, removed as they are read.
pub fn take_expiry_notes(c: &Connection, user: u32) -> DbResult<Vec<u32>> {
    let mut st = c.prepare("SELECT item_no FROM expiry_notes WHERE user_no = ?1 ORDER BY id")?;
    let rows = st.query_map(params![user as i64], |r| r.get::<_, i64>(0))?;
    let nos: Vec<u32> = rows
        .collect::<DbResult<Vec<i64>>>()?
        .into_iter()
        .map(|n| n.clamp(0, u32::MAX as i64) as u32)
        .collect();
    c.execute(
        "DELETE FROM expiry_notes WHERE user_no = ?1",
        params![user as i64],
    )?;
    Ok(nos)
}

pub fn delete_item(c: &Connection, uid: i64) -> DbResult<()> {
    c.execute("DELETE FROM items WHERE uid = ?1", params![uid])?;
    // an attachment whose parent is gone is detached
    c.execute(
        "UPDATE items SET parent_uid = 0 WHERE parent_uid = ?1",
        params![uid],
    )?;
    Ok(())
}

/// Adds match statistics and rewards to one character.
pub struct MatchDelta {
    pub exp: i64,
    pub gp: i64,
    pub kills: i64,
    pub deaths: i64,
    pub headshots: i64,
    pub win: bool,
    pub loss: bool,
}

pub fn apply_match(c: &Connection, user_no: u32, d: &MatchDelta) -> DbResult<usize> {
    c.execute(
        "UPDATE characters SET exp = MAX(0, exp + ?2), gp = MAX(0, gp + ?3), kills = kills + ?4, deaths = deaths + ?5, \
         headshots = headshots + ?6, matches = matches + 1, wins = wins + ?7, losses = losses + ?8 WHERE user_no = ?1",
        params![
            user_no as i64,
            d.exp,
            d.gp,
            d.kills,
            d.deaths,
            d.headshots,
            d.win as i64,
            d.loss as i64
        ],
    )
}

/// LG-303: the highest rank whose promotion the character was rewarded for (0 = none yet).
pub fn rank_rewarded(c: &Connection, user_no: u32) -> DbResult<u32> {
    let rank = c
        .query_row(
            "SELECT rank_rewarded FROM characters WHERE user_no = ?1",
            params![user_no as i64],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(rank.unwrap_or(0).clamp(0, u32::MAX as i64) as u32)
}

/// LG-303: records `rank` as rewarded (the stored value only grows).
pub fn set_rank_rewarded(c: &Connection, user_no: u32, rank: u32) -> DbResult<()> {
    c.execute(
        "UPDATE characters SET rank_rewarded = MAX(rank_rewarded, ?2) WHERE user_no = ?1",
        params![user_no as i64, rank as i64],
    )?;
    Ok(())
}

/// LG-250: adds `nx`, `nx_cr`, `gp` and `exp` (any sign) to a character's balances, each floored at 0; returns the
/// number of rows (0 = no character for `user_no`).
pub fn grant(
    c: &Connection,
    user_no: u32,
    nx: i64,
    nx_cr: i64,
    gp: i64,
    exp: i64,
) -> DbResult<usize> {
    c.execute(
        "UPDATE characters SET nx = MAX(0, nx + ?2), nx_cr = MAX(0, nx_cr + ?3), gp = MAX(0, gp + ?4), \
         exp = MAX(0, exp + ?5) WHERE user_no = ?1",
        params![user_no as i64, nx, nx_cr, gp, exp],
    )
}

pub struct MatchRow<'a> {
    pub ended: i64,
    pub channel: u32,
    pub room: u32,
    pub map: u32,
    pub mode: u32,
    pub mode_key: &'a str,
    pub winner: i64,
    pub alpha: i64,
    pub bravo: i64,
    pub players: i64,
    pub detail: &'a str,
}

pub fn log_match(c: &Connection, m: &MatchRow<'_>) -> DbResult<i64> {
    c.execute(
        "INSERT INTO match_log (ended, channel, room, map, mode, mode_key, winner, alpha, bravo, players, detail) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            m.ended,
            m.channel as i64,
            m.room as i64,
            m.map as i64,
            m.mode as i64,
            m.mode_key,
            m.winner,
            m.alpha,
            m.bravo,
            m.players,
            m.detail
        ],
    )?;
    Ok(c.last_insert_rowid())
}

#[cfg(test)]
pub fn memory() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    prepare(&c).unwrap();
    c
}

#[cfg(test)]
mod tests {
    #[test]
    fn first_account_number_applies_to_a_fresh_table_only() {
        // DC-DEFAULT: AUTH-1 (proposed)
        let c = memory();
        assert_eq!(create_account_from(&c, "x1", 0, 1001).unwrap(), 1001);
        assert_eq!(create_account_from(&c, "x2", 0, 1001).unwrap(), 1002);
        // a database that already has numbers below the configured start keeps counting from its own max
        let c = memory();
        assert_eq!(create_account(&c, "old1", 0).unwrap(), 1);
        assert_eq!(create_account(&c, "old2", 0).unwrap(), 2);
        assert_eq!(create_account_from(&c, "new", 0, 1001).unwrap(), 3);
        // first = 1 is the plain autoincrement; a deleted-out table does not restart at `first`
        let c = memory();
        assert_eq!(create_account_from(&c, "a", 0, 1).unwrap(), 1);
        let c = memory();
        assert_eq!(create_account_from(&c, "a", 0, 50).unwrap(), 50);
        c.execute("DELETE FROM accounts", []).unwrap();
        assert_eq!(create_account_from(&c, "b", 0, 50).unwrap(), 51);
    }

    use super::*;

    #[test]
    fn harness_columns_exist_with_harness_defaults() {
        let c = memory();
        let a = create_account(&c, "dave", 1).unwrap();
        create_character(
            &c,
            &NewCharacter {
                user_no: a,
                nick: "Dave",
                face: 2,
                exp: 0,
                gp: 10000,
                nx: 5000,
                now: 1,
            },
            true,
        )
        .unwrap();
        // the harness inserts items with only some columns (LOBBY_HARNESS.md section 6)
        c.execute(
            "INSERT INTO items (user_no, item_no) VALUES (?1, 3)",
            params![a as i64],
        )
        .unwrap();
        let it = items(&c, a).unwrap();
        assert_eq!(it.len(), 1);
        let i = &it[0];
        assert_eq!(
            (
                i.slot,
                i.start_time,
                i.end_time,
                i.default_type,
                i.parent_uid
            ),
            (-1, 0, 0, 0, 0)
        );
        assert_eq!((i.use_type, i.remain, i.used), (0, 0, 0));
        c.execute(
            "UPDATE items SET end_time = ?1 WHERE uid = ?2",
            params![1i64 << 60, i.uid],
        )
        .unwrap();
        assert_eq!(item(&c, i.uid).unwrap().unwrap().end_time, 1i64 << 60);
        for q in [
            "SELECT user_no, login, gm FROM accounts",
            "SELECT user_no, nick, exp, gp, nx, nx_cr, kills, deaths, wins, losses, headshots, matches, nick_color, \
             options, clan_uid, clan_name, clan_type, face, nation FROM characters",
            "SELECT user_no, product_id, created, uproduct FROM inbox",
            "SELECT user_no, friend_no FROM friends",
            "SELECT clan_uid, name, master, emblem, background FROM clans",
            "SELECT COUNT(*) FROM match_log",
            "SELECT kind, target_nick, text, extra, reporter FROM reports",
            "SELECT user_no, recommender FROM recommends",
        ] {
            c.prepare(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        }
        let ch = character(&c, a).unwrap().unwrap();
        assert_eq!(
            (ch.nick.as_str(), ch.gp, ch.nx, ch.face),
            ("Dave", 10000, 5000, 2)
        );
    }

    #[test]
    fn nick_uniqueness_is_case_insensitive() {
        let c = memory();
        let a = create_account(&c, "a", 0).unwrap();
        let b = create_account(&c, "b", 0).unwrap();
        create_character(
            &c,
            &NewCharacter {
                user_no: a,
                nick: "Erin",
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        assert!(create_character(
            &c,
            &NewCharacter {
                user_no: b,
                nick: "ERIN",
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0
            },
            true
        )
        .is_err());
        assert_eq!(
            character_by_nick(&c, "erin", true)
                .unwrap()
                .unwrap()
                .user_no,
            a
        );
    }

    #[test]
    fn login_lookup_ignores_case() {
        let c = memory();
        let a = create_account(&c, "Gina", 0).unwrap();
        assert_eq!(account_by_login(&c, "gina").unwrap().unwrap().user_no, a);
    }
}
