//! The `device_sessions` table and its row operations (SES-1..SES-3).
//!
//! Secrets never reach this table: a device row holds the SHA-256 of the secret the server accepts next and of the
//! secret spent last. Hashes are compared in constant time; device ids are not secret (they are looked up by primary
//! key). Every time is the caller's server clock (`now`, unix seconds); nothing here reads a client time.
//!
//! The operations run inside the caller's transaction when there is one (a handler's `Core::scope`) and use a SQLite
//! savepoint for their own multi-statement steps, so they nest under `BEGIN IMMEDIATE` and stand alone as well.
// DC-DEFAULT: SES-1 hashed storage, SES-2 rotation and reuse revoke, SES-3 idle expiry and the device cap
#![allow(dead_code)] // the row API of the device endpoints, which build on it

use crate::db::DbResult;
use rusqlite::{params, Connection, OptionalExtension};

/// The table (a new one: an older DB gets it empty at the next start; no existing table changes).
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS device_sessions (
    device     TEXT PRIMARY KEY,
    user_no    INTEGER NOT NULL,
    cur_hash   BLOB NOT NULL,
    prev_hash  BLOB,
    spent      BLOB NOT NULL DEFAULT x'',
    generation INTEGER NOT NULL DEFAULT 0,
    name       TEXT NOT NULL DEFAULT '',
    platform   TEXT NOT NULL DEFAULT '',
    created    INTEGER NOT NULL,
    last_used  INTEGER NOT NULL,
    last_place TEXT NOT NULL DEFAULT '',
    revoked    INTEGER NOT NULL DEFAULT 0,
    revoked_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS device_sessions_user ON device_sessions (user_no, revoked);
"#;

/// Bytes of a secret's hash (SHA-256).
pub const HASH_LEN: usize = 32;
/// Spent hashes kept per device for reuse detection (newest first, HASH_LEN bytes each in spent).
// DC-DEFAULT: SES-2 the last 8 spent secrets count as reuse
pub const SPENT_KEPT: usize = 8;

/// Creates the table, and adds the spent-hash history to a table made before it (its last spent hash seeds it).
pub fn prepare(c: &Connection) -> DbResult<()> {
    c.execute_batch(SCHEMA)?;
    let mut has = false;
    c.pragma(None, "table_info", "device_sessions", |r| {
        let name: String = r.get(1)?;
        has |= name == "spent";
        Ok(())
    })?;
    if !has {
        c.execute_batch(
            "ALTER TABLE device_sessions ADD COLUMN spent BLOB NOT NULL DEFAULT x'';
             UPDATE device_sessions SET spent = prev_hash WHERE prev_hash IS NOT NULL;",
        )?;
    }
    Ok(())
}

/// True when h equals one of the hashes in spent; every entry is compared, in constant time.
fn in_history(h: &[u8], spent: &[u8]) -> bool {
    spent
        .chunks(HASH_LEN)
        .fold(false, |hit, s| hit | ct_eq(h, s))
}
/// Random bytes of a device id.
pub const DEVICE_ID_BYTES: usize = 16;
/// The device id's prefix.
pub const DEVICE_PREFIX: &str = "dv_";
/// Characters of a device id: the prefix and 16 bytes in unpadded base64url.
pub const DEVICE_ID_LEN: usize = 3 + 22;
/// Longest device name and platform text kept (longer is cut at a character boundary).
pub const MAX_LABEL: usize = 64;

/// Why a device row is no longer live (`revoked` column; 0 = live).
pub mod reason {
    pub const LIVE: i64 = 0;
    /// The device signed itself out.
    pub const SIGN_OUT: i64 = 1;
    /// The account owner revoked it from another device.
    pub const USER: i64 = 2;
    /// "Sign out everywhere".
    pub const ALL: i64 = 3;
    /// A password change or recovery.
    pub const PASSWORD: i64 = 4;
    /// A spent secret was presented again.
    pub const REUSE: i64 = 5;
    /// An operator.
    pub const ADMIN: i64 = 6;
    /// Not used for the idle limit.
    pub const IDLE: i64 = 7;
    /// The longest-idle device made room for a new one under the per-account cap.
    pub const CAP: i64 = 8;
}

/// SHA-256 of a secret as presented (the client's base64url text, as bytes).
pub fn secret_hash(secret: &[u8]) -> [u8; HASH_LEN] {
    let d = ring::digest::digest(&ring::digest::SHA256, secret);
    let mut h = [0u8; HASH_LEN];
    h.copy_from_slice(d.as_ref());
    h
}

/// Constant-time equality of two byte strings (the length is not secret: both are hashes of fixed size).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh device id from the OS random source; None when the source fails (no fallback to weaker material).
pub fn new_device_id() -> Option<String> {
    use ring::rand::SecureRandom;
    let mut b = [0u8; DEVICE_ID_BYTES];
    ring::rand::SystemRandom::new().fill(&mut b).ok()?;
    Some(format!("{DEVICE_PREFIX}{}", crate::identity::b64url(&b)))
}

/// True for the shape of a device id (`dv_` + 22 base64url characters encoding 16 bytes).
pub fn valid_device_id(s: &str) -> bool {
    s.len() == DEVICE_ID_LEN
        && s.starts_with(DEVICE_PREFIX)
        && s[DEVICE_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        // the last digit of 16 bytes carries 2 bits: only A, Q, g, w decode without spare bits
        && matches!(s.as_bytes()[DEVICE_ID_LEN - 1], b'A' | b'Q' | b'g' | b'w')
}

fn label(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    let mut end = s.len().min(MAX_LABEL);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim().to_string()
}

/// Runs `f` atomically: every statement of it lands, or none. Inside an open transaction (a handler's
/// `BEGIN IMMEDIATE` scope) it nests as a savepoint; on its own it opens a `BEGIN IMMEDIATE` transaction, so the write
/// lock is taken before the first read and a check (the cap, the current hash) cannot race another connection.
fn atomically<T>(c: &Connection, f: impl FnOnce(&Connection) -> DbResult<T>) -> DbResult<T> {
    if c.is_autocommit() {
        c.execute_batch("BEGIN IMMEDIATE")?;
        return match f(c) {
            Ok(v) => {
                c.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = c.execute_batch("ROLLBACK");
                Err(e)
            }
        };
    }
    c.execute_batch("SAVEPOINT device_sessions_op")?;
    match f(c) {
        Ok(v) => {
            c.execute_batch("RELEASE device_sessions_op")?;
            Ok(v)
        }
        Err(e) => {
            let _ = c.execute_batch("ROLLBACK TO device_sessions_op; RELEASE device_sessions_op");
            Err(e)
        }
    }
}

/// A device row as shown to its owner and the operator. The hashes are not part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    pub device: String,
    pub user_no: u32,
    pub generation: i64,
    pub name: String,
    pub platform: String,
    pub created: i64,
    pub last_used: i64,
    pub last_place: String,
    /// [`reason`]; 0 = live.
    pub revoked: i64,
    pub revoked_at: i64,
}

impl DeviceRow {
    pub fn live(&self) -> bool {
        self.revoked == reason::LIVE
    }
}

const ROW_COLS: &str =
    "device, user_no, generation, name, platform, created, last_used, last_place, revoked, revoked_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceRow> {
    Ok(DeviceRow {
        device: r.get(0)?,
        user_no: r.get::<_, i64>(1)? as u32,
        generation: r.get(2)?,
        name: r.get(3)?,
        platform: r.get(4)?,
        created: r.get(5)?,
        last_used: r.get(6)?,
        last_place: r.get(7)?,
        revoked: r.get(8)?,
        revoked_at: r.get(9)?,
    })
}

/// A device to store after a password sign-in.
pub struct NewDevice<'a> {
    /// From [`new_device_id`].
    pub device: &'a str,
    pub user_no: u32,
    /// SHA-256 of the client's first secret (the client sent only this hash).
    pub first_hash: &'a [u8; HASH_LEN],
    pub name: &'a str,
    pub platform: &'a str,
    /// The masked address of the sign-in.
    pub place: &'a str,
    pub now: i64,
}

/// Stores a new live device. When the account already has `max_live` live devices, the longest-idle ones are
/// revoked ([`reason::CAP`]) to make room; their ids are returned. `max_live` 0 = no cap.
// DC-DEFAULT: SES-3 the cap evicts the live device with the oldest last use (the older one on a tie)
pub fn create(c: &Connection, d: &NewDevice<'_>, max_live: u32) -> DbResult<Vec<String>> {
    atomically(c, |c| {
        let mut evicted = Vec::new();
        if max_live > 0 {
            let live = count_live(c, d.user_no)?;
            let excess = (live + 1).saturating_sub(max_live);
            if excess > 0 {
                let mut q = c.prepare(
                    "SELECT device FROM device_sessions WHERE user_no = ?1 AND revoked = 0 \
                     ORDER BY last_used ASC, created ASC, device ASC LIMIT ?2",
                )?;
                let ids = q
                    .query_map(params![d.user_no as i64, excess as i64], |r| {
                        r.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for id in ids {
                    revoke(c, &id, reason::CAP, d.now)?;
                    evicted.push(id);
                }
            }
        }
        c.execute(
            "INSERT INTO device_sessions (device, user_no, cur_hash, prev_hash, generation, name, platform, created, \
             last_used, last_place) VALUES (?1, ?2, ?3, NULL, 0, ?4, ?5, ?6, ?6, ?7)",
            params![
                d.device,
                d.user_no as i64,
                &d.first_hash[..],
                label(d.name),
                label(d.platform),
                d.now,
                label(d.place)
            ],
        )?;
        Ok(evicted)
    })
}

/// The row of a device, live or not.
pub fn get(c: &Connection, device: &str) -> DbResult<Option<DeviceRow>> {
    c.query_row(
        &format!("SELECT {ROW_COLS} FROM device_sessions WHERE device = ?1"),
        params![device],
        row,
    )
    .optional()
}

/// The account's devices: the live ones first, each group most recently used first.
pub fn list(c: &Connection, user_no: u32) -> DbResult<Vec<DeviceRow>> {
    let mut q = c.prepare(&format!(
        "SELECT {ROW_COLS} FROM device_sessions WHERE user_no = ?1 \
         ORDER BY revoked != 0, last_used DESC, device ASC"
    ))?;
    let rows = q.query_map(params![user_no as i64], row)?;
    rows.collect()
}

/// Live devices of the account.
pub fn count_live(c: &Connection, user_no: u32) -> DbResult<u32> {
    c.query_row(
        "SELECT COUNT(*) FROM device_sessions WHERE user_no = ?1 AND revoked = 0",
        params![user_no as i64],
        |r| Ok(r.get::<_, i64>(0)? as u32),
    )
}

/// Devices of the account made at or after `since` (live or not), for the per-day creation limit.
pub fn created_since(c: &Connection, user_no: u32, since: i64) -> DbResult<u32> {
    c.query_row(
        "SELECT COUNT(*) FROM device_sessions WHERE user_no = ?1 AND created >= ?2",
        params![user_no as i64, since],
        |r| Ok(r.get::<_, i64>(0)? as u32),
    )
}

/// What a spend did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spend {
    /// The secret was the current one: the pair rotated, the device is used now.
    Rotated { user_no: u32, generation: i64 },
    /// The request repeats the rotation accepted last (the secret spent last, and the hash that rotation made
    /// current): a resent or concurrently doubled request. Nothing changed; answered like the rotation it repeats.
    Replayed { user_no: u32, generation: i64 },
    /// The secret is one of the last [`SPENT_KEPT`] spent ones: the device is now revoked ([`reason::REUSE`]).
    Reuse { user_no: u32 },
    /// The secret was current but the device had not been used within the idle limit: now revoked
    /// ([`reason::IDLE`]).
    Expired { user_no: u32 },
    /// The device is revoked already (nothing changed).
    Revoked { user_no: u32 },
    /// The proposed next hash equals the current or a spent one (nothing changed).
    BadNext,
    /// The secret matches neither hash (nothing changed; never revokes).
    Mismatch,
    /// No such device.
    Unknown,
}

/// Spends `secret` on `device`: when it is the current secret and the device is live and not idle past
/// `idle_secs` (0 = no limit), `next_hash` becomes the current hash, the spent one joins the history of the last
/// [`SPENT_KEPT`] spent hashes, and the use time `now` and `place` are recorded. Any of those spent secrets presented
/// again revokes the device. Anything else changes
/// nothing. The caller decides what the account's tokens and the answer become.
// DC-DEFAULT: SES-2 rotation and reuse revoke; SES-3 sliding idle expiry
pub fn rotate(
    c: &Connection,
    device: &str,
    secret: &[u8],
    next_hash: &[u8; HASH_LEN],
    place: &str,
    now: i64,
    idle_secs: i64,
) -> DbResult<Spend> {
    atomically(c, |c| {
        let found = c
            .query_row(
                "SELECT user_no, cur_hash, spent, generation, last_used, revoked FROM device_sessions \
                 WHERE device = ?1",
                params![device],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)? as u32,
                        r.get::<_, Vec<u8>>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((user_no, cur, spent, generation, last_used, revoked)) = found else {
            return Ok(Spend::Unknown);
        };
        let h = secret_hash(secret);
        // every comparison always runs, so the time does not tell which one matched
        let is_cur = ct_eq(&h, &cur);
        let is_spent = in_history(&h, &spent);
        // the newest spent hash with the hash its rotation made current: the same request again
        let last = &spent[..spent.len().min(HASH_LEN)];
        let is_replay = (last.len() == HASH_LEN) & ct_eq(&h, last) & ct_eq(next_hash, &cur);
        if revoked != reason::LIVE {
            return Ok(Spend::Revoked { user_no });
        }
        if is_cur {
            if idle_secs > 0 && now - last_used > idle_secs {
                revoke(c, device, reason::IDLE, now)?;
                return Ok(Spend::Expired { user_no });
            }
            // a next hash that was current or spent before would let a secret be spent twice
            if ct_eq(next_hash, &cur) | in_history(next_hash, &spent) {
                return Ok(Spend::BadNext);
            }
            let mut history = cur.clone();
            history.extend_from_slice(&spent[..spent.len().min((SPENT_KEPT - 1) * HASH_LEN)]);
            // compare-and-swap on the hash just read: a concurrent spend of the same secret rotates only once
            let n = c.execute(
                "UPDATE device_sessions SET prev_hash = cur_hash, spent = ?6, cur_hash = ?2, \
                 generation = generation + 1, last_used = ?3, last_place = ?4 \
                 WHERE device = ?1 AND revoked = 0 AND cur_hash = ?5",
                params![device, &next_hash[..], now, label(place), &cur[..], history],
            )?;
            if n != 1 {
                return Ok(Spend::Mismatch);
            }
            return Ok(Spend::Rotated {
                user_no,
                generation: generation + 1,
            });
        }
        // DC-DEFAULT: SES-2 a repeated request is answered as the rotation it repeats (idempotent), never as reuse:
        // it carries the hash of the current secret, which a stale copy of an older secret cannot know
        if is_replay {
            return Ok(Spend::Replayed {
                user_no,
                generation,
            });
        }
        if is_spent {
            revoke(c, device, reason::REUSE, now)?;
            return Ok(Spend::Reuse { user_no });
        }
        Ok(Spend::Mismatch)
    })
}

/// The device signs itself out: revoked ([`reason::SIGN_OUT`]) when `secret` is its current one. Anything else
/// changes nothing (a stale or wrong secret never revokes and is never read as reuse here). True when revoked.
pub fn sign_out(c: &Connection, device: &str, secret: &[u8], now: i64) -> DbResult<bool> {
    atomically(c, |c| {
        let cur: Option<Vec<u8>> = c
            .query_row(
                "SELECT cur_hash FROM device_sessions WHERE device = ?1 AND revoked = 0",
                params![device],
                |r| r.get(0),
            )
            .optional()?;
        match cur {
            Some(cur) if ct_eq(&secret_hash(secret), &cur) => {
                revoke(c, device, reason::SIGN_OUT, now)
            }
            _ => Ok(false),
        }
    })
}

/// Ends every identity token of the account issued before `after` (LG-425's `tokens_valid_after`, never lowered):
/// a token is accepted only when issued at or after it.
pub fn end_account_tokens(c: &Connection, user_no: u32, after: i64) -> DbResult<()> {
    c.execute(
        "UPDATE accounts SET tokens_valid_after = MAX(tokens_valid_after, ?2) WHERE user_no = ?1",
        params![user_no as i64, after],
    )?;
    Ok(())
}

/// Revokes one live device with `why` (a [`reason`] other than LIVE). True when a live row changed.
pub fn revoke(c: &Connection, device: &str, why: i64, now: i64) -> DbResult<bool> {
    debug_assert!(why != reason::LIVE);
    let n = c.execute(
        "UPDATE device_sessions SET revoked = ?2, revoked_at = ?3 WHERE device = ?1 AND revoked = 0",
        params![device, why, now],
    )?;
    Ok(n == 1)
}

/// Revokes every live device of the account except `keep` (the device asking, if any). Returns how many changed.
pub fn revoke_account(
    c: &Connection,
    user_no: u32,
    why: i64,
    keep: Option<&str>,
    now: i64,
) -> DbResult<usize> {
    debug_assert!(why != reason::LIVE);
    c.execute(
        "UPDATE device_sessions SET revoked = ?2, revoked_at = ?3 WHERE user_no = ?1 AND revoked = 0 \
         AND device IS NOT ?4",
        params![user_no as i64, why, now, keep],
    )
}

/// Revokes every live device not used within `idle_secs` of `now` ([`reason::IDLE`]). Returns how many.
// DC-DEFAULT: SES-3 sliding idle expiry
pub fn expire_idle(c: &Connection, now: i64, idle_secs: i64) -> DbResult<usize> {
    if idle_secs <= 0 {
        return Ok(0);
    }
    c.execute(
        "UPDATE device_sessions SET revoked = ?2, revoked_at = ?1 WHERE revoked = 0 AND last_used < ?1 - ?3",
        params![now, reason::IDLE, idle_secs],
    )
}

/// The periodic clean-up: expires idle devices, then deletes rows revoked more than `keep_revoked_secs` ago (kept
/// until then for the owner's device history). Returns (expired, deleted).
// DC-DEFAULT: SES-3 revoked rows kept 30 days (the caller passes the time)
pub fn sweep(
    c: &Connection,
    now: i64,
    idle_secs: i64,
    keep_revoked_secs: i64,
) -> DbResult<(usize, usize)> {
    atomically(c, |c| {
        let expired = expire_idle(c, now, idle_secs)?;
        let deleted = c.execute(
            "DELETE FROM device_sessions WHERE revoked != 0 AND revoked_at < ?1 - ?2",
            params![now, keep_revoked_secs.max(0)],
        )?;
        Ok((expired, deleted))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::memory;

    const DAY: i64 = 86_400;
    const IDLE: i64 = 180 * DAY;

    /// A client-made secret: 32 OS-random bytes as base64url text (what the client sends when it spends one).
    fn secret() -> String {
        use ring::rand::SecureRandom;
        let mut b = [0u8; 32];
        ring::rand::SystemRandom::new().fill(&mut b).unwrap();
        crate::identity::b64url(&b)
    }

    fn add(c: &Connection, user: u32, s: &str, now: i64, cap: u32) -> (String, Vec<String>) {
        let id = new_device_id().unwrap();
        let ev = create(
            c,
            &NewDevice {
                device: &id,
                user_no: user,
                first_hash: &secret_hash(s.as_bytes()),
                name: "Desk",
                platform: "windows",
                place: "203.0.113.0/24",
                now,
            },
            cap,
        )
        .unwrap();
        (id, ev)
    }

    fn spend(c: &Connection, id: &str, s: &str, next: &str, now: i64) -> Spend {
        rotate(
            c,
            id,
            s.as_bytes(),
            &secret_hash(next.as_bytes()),
            "198.51.100.0/24",
            now,
            IDLE,
        )
        .unwrap()
    }

    #[test]
    fn device_ids_are_random_and_well_formed() {
        let a = new_device_id().unwrap();
        let b = new_device_id().unwrap();
        assert_ne!(a, b);
        assert!(valid_device_id(&a), "{a}");
        assert_eq!(a.len(), DEVICE_ID_LEN);
        assert!(!valid_device_id("dv_short"));
        assert!(!valid_device_id(&a.replace("dv_", "xx_")));
        assert!(
            !valid_device_id(&format!("{}B", &a[..a.len() - 1])),
            "spare bits set"
        );
        assert!(!valid_device_id(&format!("dv_{}+", &a[3..a.len() - 1])));
    }

    #[test]
    fn the_table_appears_in_an_older_db_and_nothing_else_changes() {
        let c = memory();
        c.execute(
            "INSERT INTO accounts (login, pw_hash, created) VALUES ('old', 'x', 1)",
            [],
        )
        .unwrap();
        c.execute_batch("DROP TABLE device_sessions").unwrap();
        crate::db::prepare(&c).unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM device_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let login: String = c
            .query_row("SELECT login FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(login, "old");
        // opening twice is harmless
        crate::db::prepare(&c).unwrap();
    }

    #[test]
    fn only_hashes_of_the_secrets_are_stored() {
        let c = memory();
        let (s0, s1) = (secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        assert_eq!(
            spend(&c, &id, &s0, &s1, 1100),
            Spend::Rotated {
                user_no: 7,
                generation: 1
            }
        );
        let (cur, prev): (Vec<u8>, Vec<u8>) = c
            .query_row(
                "SELECT cur_hash, prev_hash FROM device_sessions WHERE device = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(cur, secret_hash(s1.as_bytes()));
        assert_eq!(prev, secret_hash(s0.as_bytes()));
        // no column of the table holds either secret, as text or as bytes
        let mut q = c.prepare("SELECT * FROM device_sessions").unwrap();
        let ncol = q.column_count();
        let mut rows = q.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            for i in 0..ncol {
                let v: rusqlite::types::Value = r.get(i).unwrap();
                let bytes = match v {
                    rusqlite::types::Value::Text(t) => t.into_bytes(),
                    rusqlite::types::Value::Blob(b) => b,
                    _ => continue,
                };
                for s in [&s0, &s1] {
                    assert!(
                        !bytes
                            .windows(8)
                            .any(|w| s.as_bytes().windows(8).any(|x| x == w)),
                        "column {i} holds part of a secret"
                    );
                }
            }
        }
    }

    #[test]
    fn each_spend_rotates_and_the_new_secret_works() {
        let c = memory();
        let (s0, s1, s2) = (secret(), secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        assert!(matches!(
            spend(&c, &id, &s0, &s1, 1100),
            Spend::Rotated { .. }
        ));
        assert_eq!(
            spend(&c, &id, &s1, &s2, 1200),
            Spend::Rotated {
                user_no: 7,
                generation: 2
            }
        );
        let r = get(&c, &id).unwrap().unwrap();
        assert_eq!((r.generation, r.last_used, r.created), (2, 1200, 1000));
        assert_eq!(r.last_place, "198.51.100.0/24");
        assert!(r.live());
    }

    #[test]
    fn a_spent_secret_presented_again_is_reuse_and_revokes() {
        let c = memory();
        let (s0, s1, s2) = (secret(), secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        spend(&c, &id, &s0, &s1, 1100);
        // the copy of s0 in other hands
        assert_eq!(spend(&c, &id, &s0, &s2, 1200), Spend::Reuse { user_no: 7 });
        let r = get(&c, &id).unwrap().unwrap();
        assert_eq!((r.revoked, r.revoked_at), (reason::REUSE, 1200));
        // the rightful holder of s1 is signed out too
        assert_eq!(
            spend(&c, &id, &s1, &s2, 1300),
            Spend::Revoked { user_no: 7 }
        );
        assert_eq!(count_live(&c, 7).unwrap(), 0);
    }

    /// A copy of a secret spent three rotations ago is still recognised (the history keeps the last 8).
    #[test]
    fn a_secret_spent_several_rotations_ago_is_reuse_and_revokes() {
        let c = memory();
        let s: Vec<String> = (0..12).map(|_| secret()).collect();
        let (id, _) = add(&c, 7, &s[0], 1000, 10);
        for i in 0..4 {
            assert!(matches!(
                spend(&c, &id, &s[i], &s[i + 1], 1100 + i as i64),
                Spend::Rotated { .. }
            ));
        }
        // s[4] is current; s[1] was spent three rotations before the last one
        assert_eq!(
            spend(&c, &id, &s[1], &s[11], 1200),
            Spend::Reuse { user_no: 7 }
        );
        assert_eq!(get(&c, &id).unwrap().unwrap().revoked, reason::REUSE);
        // the history is bounded: after 9 more rotations the oldest drops out and only refuses
        let (id2, _) = add(&c, 8, &s[0], 1000, 10);
        let mut cur = s[0].clone();
        let mut olds = Vec::new();
        for i in 0..9 {
            let next = secret();
            assert!(matches!(
                spend(&c, &id2, &cur, &next, 2000 + i),
                Spend::Rotated { .. }
            ));
            olds.push(std::mem::replace(&mut cur, next));
        }
        let spent: Vec<u8> = c
            .query_row(
                "SELECT spent FROM device_sessions WHERE device = ?1",
                params![id2],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spent.len(), SPENT_KEPT * HASH_LEN);
        assert_eq!(spend(&c, &id2, &olds[0], &s[11], 3000), Spend::Mismatch);
        assert_eq!(
            spend(&c, &id2, &olds[1], &s[11], 3000),
            Spend::Reuse { user_no: 8 }
        );
    }

    #[test]
    fn a_next_hash_equal_to_any_spent_hash_is_refused() {
        let c = memory();
        let (s0, s1, s2, s3) = (secret(), secret(), secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        spend(&c, &id, &s0, &s1, 1100);
        spend(&c, &id, &s1, &s2, 1200);
        assert_eq!(spend(&c, &id, &s2, &s0, 1300), Spend::BadNext);
        assert!(matches!(
            spend(&c, &id, &s2, &s3, 1300),
            Spend::Rotated { .. }
        ));
    }

    /// A table made before the spent-hash history gets the column, seeded with its last spent hash.
    #[test]
    fn an_older_device_table_gains_the_history() {
        let c = memory();
        c.execute_batch(
            "DROP TABLE device_sessions;
             CREATE TABLE device_sessions (device TEXT PRIMARY KEY, user_no INTEGER NOT NULL, cur_hash BLOB NOT NULL,
               prev_hash BLOB, generation INTEGER NOT NULL DEFAULT 0, name TEXT NOT NULL DEFAULT '',
               platform TEXT NOT NULL DEFAULT '', created INTEGER NOT NULL, last_used INTEGER NOT NULL,
               last_place TEXT NOT NULL DEFAULT '', revoked INTEGER NOT NULL DEFAULT 0,
               revoked_at INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        let (s0, s1, s2) = (secret(), secret(), secret());
        let id = new_device_id().unwrap();
        c.execute(
            "INSERT INTO device_sessions (device, user_no, cur_hash, prev_hash, created, last_used) \
             VALUES (?1, 7, ?2, ?3, 1, 1)",
            params![id, &secret_hash(s1.as_bytes())[..], &secret_hash(s0.as_bytes())[..]],
        )
        .unwrap();
        crate::db::prepare(&c).unwrap();
        assert_eq!(spend(&c, &id, &s0, &s2, 10), Spend::Reuse { user_no: 7 });
    }

    #[test]
    fn a_wrong_secret_never_revokes_and_an_unknown_device_is_unknown() {
        let c = memory();
        let (s0, s1) = (secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        assert_eq!(spend(&c, &id, &secret(), &s1, 1100), Spend::Mismatch);
        assert_eq!(spend(&c, &id, "", &s1, 1100), Spend::Mismatch);
        assert!(get(&c, &id).unwrap().unwrap().live());
        assert!(matches!(
            spend(&c, &id, &s0, &s1, 1200),
            Spend::Rotated { .. }
        ));
        let other = new_device_id().unwrap();
        assert_eq!(spend(&c, &other, &s1, &s0, 1300), Spend::Unknown);
    }

    #[test]
    fn a_next_hash_that_repeats_an_old_one_is_refused() {
        let c = memory();
        let (s0, s1, s2) = (secret(), secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        assert_eq!(spend(&c, &id, &s0, &s0, 1100), Spend::BadNext);
        spend(&c, &id, &s0, &s1, 1100);
        assert_eq!(spend(&c, &id, &s1, &s0, 1200), Spend::BadNext);
        assert_eq!(spend(&c, &id, &s1, &s1, 1200), Spend::BadNext);
        // nothing changed: s1 is still the current secret
        assert!(matches!(
            spend(&c, &id, &s1, &s2, 1300),
            Spend::Rotated { .. }
        ));
    }

    #[test]
    fn a_device_idle_past_the_limit_expires_and_a_use_restarts_the_clock() {
        let c = memory();
        let (s0, s1, s2) = (secret(), secret(), secret());
        let (id, _) = add(&c, 7, &s0, 0, 10);
        // used on day 179: the clock restarts
        assert!(matches!(
            spend(&c, &id, &s0, &s1, 179 * DAY),
            Spend::Rotated { .. }
        ));
        // 180 days after that use, to the second: still fine
        assert!(matches!(
            spend(&c, &id, &s1, &s2, 359 * DAY),
            Spend::Rotated { .. }
        ));
        // one second past 180 idle days: expired, revoked as idle
        let s3 = secret();
        assert_eq!(
            spend(&c, &id, &s2, &s3, 539 * DAY + 1),
            Spend::Expired { user_no: 7 }
        );
        assert_eq!(get(&c, &id).unwrap().unwrap().revoked, reason::IDLE);
    }

    #[test]
    fn expire_idle_and_sweep_follow_the_server_clock() {
        let c = memory();
        let (a, _) = add(&c, 7, &secret(), 0, 10);
        let (b, _) = add(&c, 7, &secret(), 100 * DAY, 10);
        assert_eq!(expire_idle(&c, 180 * DAY, IDLE).unwrap(), 0);
        assert_eq!(expire_idle(&c, 180 * DAY + 1, IDLE).unwrap(), 1);
        assert!(!get(&c, &a).unwrap().unwrap().live());
        assert!(get(&c, &b).unwrap().unwrap().live());
        // revoked rows stay 30 days for the history, then go
        assert_eq!(
            sweep(&c, 180 * DAY + 1 + 30 * DAY, IDLE, 30 * DAY).unwrap(),
            (0, 0)
        );
        assert_eq!(
            sweep(&c, 180 * DAY + 2 + 30 * DAY, IDLE, 30 * DAY).unwrap(),
            (0, 1)
        );
        assert!(get(&c, &a).unwrap().is_none());
        assert_eq!(list(&c, 7).unwrap().len(), 1);
        assert_eq!(
            expire_idle(&c, i64::MAX / 2, 0).unwrap(),
            0,
            "0 = no idle limit"
        );
    }

    #[test]
    fn the_cap_evicts_the_longest_idle_live_device() {
        let c = memory();
        let mut ids = Vec::new();
        for i in 0..10 {
            // device 3 is the longest idle (used at 10), the others at 100 + i
            let t = if i == 3 { 10 } else { 100 + i };
            ids.push(add(&c, 7, &secret(), t, 10).0);
        }
        // another account's devices do not count
        add(&c, 8, &secret(), 1, 10);
        assert_eq!(count_live(&c, 7).unwrap(), 10);
        let (eleventh, evicted) = add(&c, 7, &secret(), 500, 10);
        assert_eq!(evicted, vec![ids[3].clone()]);
        assert_eq!(count_live(&c, 7).unwrap(), 10);
        let r = get(&c, &ids[3]).unwrap().unwrap();
        assert_eq!((r.revoked, r.revoked_at), (reason::CAP, 500));
        assert!(get(&c, &eleventh).unwrap().unwrap().live());
        assert_eq!(count_live(&c, 8).unwrap(), 1);
        // a revoked device does not hold a place: after one signs out, the next sign-in evicts nobody
        revoke(&c, &ids[0], reason::SIGN_OUT, 600).unwrap();
        assert!(add(&c, 7, &secret(), 700, 10).1.is_empty());
        // a lowered cap evicts down to it
        let (_, ev) = add(&c, 7, &secret(), 800, 5);
        assert_eq!(ev.len(), 6);
        assert_eq!(count_live(&c, 7).unwrap(), 5);
        // 0 = no cap
        assert!(add(&c, 7, &secret(), 900, 0).1.is_empty());
        assert_eq!(created_since(&c, 7, 500).unwrap(), 4);
    }

    #[test]
    fn revoke_one_and_all_but_the_asking_device() {
        let c = memory();
        let (a, _) = add(&c, 7, &secret(), 1, 10);
        let (b, _) = add(&c, 7, &secret(), 2, 10);
        let (d, _) = add(&c, 7, &secret(), 3, 10);
        assert!(revoke(&c, &a, reason::USER, 10).unwrap());
        assert!(
            !revoke(&c, &a, reason::ADMIN, 11).unwrap(),
            "already revoked"
        );
        assert_eq!(get(&c, &a).unwrap().unwrap().revoked, reason::USER);
        assert_eq!(
            revoke_account(&c, 7, reason::PASSWORD, Some(&b), 20).unwrap(),
            1
        );
        assert!(get(&c, &b).unwrap().unwrap().live());
        assert_eq!(get(&c, &d).unwrap().unwrap().revoked, reason::PASSWORD);
        assert_eq!(revoke_account(&c, 7, reason::ALL, None, 30).unwrap(), 1);
        assert_eq!(count_live(&c, 7).unwrap(), 0);
        // the history lists all three, newest use first among the revoked
        let l = list(&c, 7).unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].device, d);
    }

    #[test]
    fn labels_are_cut_and_cleaned() {
        let c = memory();
        let id = new_device_id().unwrap();
        create(
            &c,
            &NewDevice {
                device: &id,
                user_no: 1,
                first_hash: &secret_hash(b"x"),
                name: &format!("{}\u{7}ä", "n".repeat(63)),
                platform: "linux\n",
                place: "",
                now: 5,
            },
            10,
        )
        .unwrap();
        let r = get(&c, &id).unwrap().unwrap();
        assert_eq!(
            r.name,
            "n".repeat(63),
            "the control character dropped, cut at 64 bytes"
        );
        assert_eq!(r.platform, "linux");
    }

    #[test]
    fn operations_nest_inside_a_handler_transaction() {
        let c = memory();
        let (s0, s1) = (secret(), secret());
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (id, _) = add(&c, 7, &s0, 1, 10);
        assert!(matches!(spend(&c, &id, &s0, &s1, 2), Spend::Rotated { .. }));
        c.execute_batch("ROLLBACK").unwrap();
        assert!(
            get(&c, &id).unwrap().is_none(),
            "the outer rollback undoes them"
        );
    }

    /// Two connections to one DB file (as the lobby and the harness have).
    fn file_pair(name: &str) -> (std::path::PathBuf, Connection, Connection) {
        let dir = std::env::temp_dir().join(format!("dc-devsession-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("ca.db");
        let a = crate::db::open(&path).unwrap();
        let b = crate::db::open(&path).unwrap();
        (dir, a, b)
    }

    /// On its own an operation holds the write lock from its first read: another writer cannot start meanwhile.
    #[test]
    fn a_standalone_operation_takes_the_write_lock_before_it_reads() {
        let (dir, a, b) = file_pair("immediate");
        b.busy_timeout(std::time::Duration::from_millis(0)).unwrap();
        atomically(&a, |a| {
            count_live(a, 7)?;
            assert!(!a.is_autocommit());
            assert!(
                b.execute_batch("BEGIN IMMEDIATE").is_err(),
                "the other connection must wait for the write lock"
            );
            Ok(())
        })
        .unwrap();
        assert!(a.is_autocommit());
        b.execute_batch("BEGIN IMMEDIATE; COMMIT").unwrap();
        // inside a transaction it nests and leaves the transaction open
        a.execute_batch("BEGIN IMMEDIATE").unwrap();
        atomically(&a, |a| count_live(a, 7)).unwrap();
        assert!(!a.is_autocommit());
        a.execute_batch("COMMIT").unwrap();
        // an error rolls the standalone transaction back
        let id = new_device_id().unwrap();
        let r: DbResult<()> = atomically(&a, |a| {
            a.execute(
                "INSERT INTO device_sessions (device, user_no, cur_hash, created, last_used) VALUES (?1, 1, x'00', 1, 1)",
                params![id],
            )?;
            Err(rusqlite::Error::InvalidQuery)
        });
        assert!(r.is_err());
        assert!(a.is_autocommit());
        assert!(get(&a, &id).unwrap().is_none());
        drop((a, b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same request sent twice (a retry after a lost answer) is answered as the rotation, not as reuse.
    #[test]
    fn a_resent_request_is_a_replay_and_does_not_revoke() {
        let c = memory();
        let (s0, s1, s2) = (secret(), secret(), secret());
        let (id, _) = add(&c, 7, &s0, 1000, 10);
        assert_eq!(
            spend(&c, &id, &s0, &s1, 1100),
            Spend::Rotated {
                user_no: 7,
                generation: 1
            }
        );
        assert_eq!(
            spend(&c, &id, &s0, &s1, 1101),
            Spend::Replayed {
                user_no: 7,
                generation: 1
            }
        );
        let r = get(&c, &id).unwrap().unwrap();
        assert!(r.live());
        assert_eq!(
            (r.generation, r.last_used),
            (1, 1100),
            "a replay changes nothing"
        );
        // the secret it made current still works; after that the old request is plain reuse
        assert!(matches!(
            spend(&c, &id, &s1, &s2, 1200),
            Spend::Rotated { .. }
        ));
        assert_eq!(spend(&c, &id, &s0, &s1, 1300), Spend::Reuse { user_no: 7 });
    }

    /// Two identical refreshes at once on two connections: one rotates, the other is its replay.
    #[test]
    fn two_concurrent_identical_refreshes_rotate_once_and_do_not_revoke() {
        let (dir, a, b) = file_pair("concurrent");
        let (s0, s1) = (secret(), secret());
        let (id, _) = add(&a, 7, &s0, 1000, 10);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let run = |conn: Connection| {
            let (id, s0, s1, barrier) = (id.clone(), s0.clone(), s1.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                spend(&conn, &id, &s0, &s1, 1100)
            })
        };
        let (ta, tb) = (run(a), run(b));
        let mut got = vec![ta.join().unwrap(), tb.join().unwrap()];
        got.sort_by_key(|s| matches!(s, Spend::Replayed { .. }));
        assert_eq!(
            got,
            vec![
                Spend::Rotated {
                    user_no: 7,
                    generation: 1
                },
                Spend::Replayed {
                    user_no: 7,
                    generation: 1
                }
            ]
        );
        let c = crate::db::open(&dir.join("ca.db")).unwrap();
        assert!(get(&c, &id).unwrap().unwrap().live());
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn constant_time_compare_is_exact() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }
}
