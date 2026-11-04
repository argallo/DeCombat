//! NX rewards: the daily login bonus, the clan match win and operator events. NX is earned on this server, never
//! bought. Every payment goes to the prepaid balance of the instance's own DB (each instance has its own ledger) and
//! is claimed in the DB in the same transaction as the balance change, so a reconnect, a restart or a second login
//! finds the claim and pays nothing.
// DC-DEFAULT: LG-310, LG-311, LG-312 (proposed, USER REQUEST 2026-11-01), see DC_DEFAULTS.md

use crate::clock;
use crate::config::Rules;
use crate::db::{self, DbResult};
use crate::game::{MatchResult, PlayerResult};
use crate::state::{Core, HResult, Sid};
use crate::wire;
use rusqlite::{params, Connection};

const SECS_PER_DAY: i64 = 86_400;

/// The UTC day number of a unix time.
pub fn utc_day(unix: i64) -> i64 {
    unix.div_euclid(SECS_PER_DAY)
}

/// Runs `f` in a savepoint: its writes all stay or all go, inside or outside a surrounding transaction.
fn atomic<T>(c: &Connection, f: impl FnOnce() -> DbResult<T>) -> DbResult<T> {
    c.execute_batch("SAVEPOINT nx_reward")?;
    match f() {
        Ok(v) => {
            c.execute_batch("RELEASE nx_reward")?;
            Ok(v)
        }
        Err(e) => {
            let _ = c.execute_batch("ROLLBACK TO nx_reward; RELEASE nx_reward");
            Err(e)
        }
    }
}

/// LG-310: pays `amount` when `day` is later than the account's last paid day; true = paid. A clock set back finds
/// a later day stored and pays nothing until that day has passed.
pub fn claim_daily(c: &Connection, account: u32, amount: i64, day: i64) -> DbResult<bool> {
    atomic(c, || {
        let won = c.execute(
            "INSERT INTO nx_daily (user_no, last_day) VALUES (?1, ?2) \
             ON CONFLICT(user_no) DO UPDATE SET last_day = excluded.last_day WHERE excluded.last_day > last_day",
            params![account as i64, day],
        )? == 1;
        if won {
            db::grant(c, account, amount, 0, 0, 0)?;
        }
        Ok(won)
    })
}

/// LG-311: pays one clan win while the account has fewer than `cap` paid wins on `day`; true = paid. The row holds
/// the newest day, so a clock set back counts against that day.
pub fn claim_clan_win(
    c: &Connection,
    account: u32,
    amount: i64,
    cap: u32,
    day: i64,
) -> DbResult<bool> {
    atomic(c, || {
        let won = c.execute(
            "INSERT INTO nx_clan_wins (user_no, day, wins) VALUES (?1, ?2, 1) \
             ON CONFLICT(user_no) DO UPDATE SET \
               wins = CASE WHEN excluded.day > day THEN 1 ELSE wins + 1 END, \
               day = MAX(day, excluded.day) \
             WHERE excluded.day > day OR wins < ?3",
            params![account as i64, day, cap],
        )? == 1;
        if won {
            db::grant(c, account, amount, 0, 0, 0)?;
        }
        Ok(won)
    })
}

/// LG-310: the first lobby login of a UTC day pays the bonus and queues its box, which the client asks for after
/// 0x303 +0x104; the 0x344 / 0x345 of the same login carries the new balance. Called before the login's replies.
pub fn on_lobby_login(st: &mut Core, account: u32) -> HResult {
    let amount = st.rules.nx_daily;
    if !st.rules.nx_enabled || amount <= 0 || !crate::lifecycle::is_human(st, account) {
        return Ok(());
    }
    if claim_daily(&st.db, account, amount, utc_day(clock::unix_now()))? {
        crate::inbox::queue_notice(st, account, &format!("Daily bonus: +{amount} NX"))?;
        st.note(&format!("nx: account {account} daily bonus +{amount}"));
    }
    Ok(())
}

/// LG-311: the guards of a finished match, from what the lobby knows of it.
pub fn clan_win_counts(
    rules: &Rules,
    winner: i64,
    elapsed_secs: Option<u64>,
    side_counts: [u32; 2],
) -> bool {
    rules.nx_enabled
        && rules.nx_clan_win > 0
        && (1..=2).contains(&winner)
        && elapsed_secs.is_some_and(|s| s >= u64::from(rules.nx_clan_min_seconds))
        && side_counts.iter().all(|n| *n >= rules.nx_clan_min_players)
}

/// LG-311: pays the winning clan's members after a stored clan match result. A player counts when still there at the
/// end (no `@leave`), a member of the side's own clan, no bot, and with a kill or a death.
pub fn on_match_result(
    st: &mut Core,
    game: u64,
    r: &MatchResult,
    players: &[PlayerResult],
) -> HResult {
    if !st.rules.nx_enabled {
        return Ok(());
    }
    let Some((room, left)) = st.games.get(&game).map(|g| (g.room, g.left.clone())) else {
        return Ok(());
    };
    let Some(s) = crate::clan::sides(st, room)? else {
        return Ok(());
    };
    if s.home == 0 || s.away == 0 {
        return Ok(());
    }
    let mut counted: [Vec<u32>; 2] = [Vec::new(), Vec::new()];
    for p in players {
        if !(1..=2).contains(&p.team) || left.contains(&p.uid) || p.kills + p.deaths <= 0 {
            continue;
        }
        let side = (p.team - 1) as usize;
        let own = if side == 0 { s.home } else { s.away };
        let member = crate::clan::clan_of(st, p.uid)?.is_some_and(|k| k.uid == own);
        if member && crate::lifecycle::is_human(st, p.uid) {
            counted[side].push(p.uid);
        }
    }
    let elapsed = st
        .rooms
        .get(&room)
        .and_then(|x| x.played_since)
        .map(|t| t.elapsed().as_secs());
    let sizes = [counted[0].len() as u32, counted[1].len() as u32];
    if !clan_win_counts(&st.rules, r.winner, elapsed, sizes) {
        return Ok(());
    }
    let amount = st.rules.nx_clan_win;
    let cap = st.rules.nx_clan_win_daily_cap;
    let day = utc_day(clock::unix_now());
    for &uid in &counted[(r.winner - 1) as usize] {
        if !claim_clan_win(&st.db, uid, amount, cap, day)? {
            st.note(&format!("nx: account {uid} clan win over the daily cap"));
            continue;
        }
        crate::inbox::queue_notice(st, uid, &format!("Clan match won: +{amount} NX"))?;
        push_balance(st, uid);
        st.note(&format!("nx: account {uid} clan win +{amount}"));
    }
    Ok(())
}

/// 0x345 to the account's sessions, as after a match in the capture (PROTOCOL_2009.md §7.1); a client keeps it while
/// its connection belongs to the game session.
// SPEC: PROTOCOL_2009.md §3.3 0x345
fn push_balance(st: &mut Core, account: u32) {
    let Some(c) = st.character(account) else {
        return;
    };
    let f = wire::balance_reply(
        c.nx_cr.clamp(0, u32::MAX as i64) as u32,
        c.nx.clamp(0, u32::MAX as i64) as u32,
    );
    for sid in st.sids_of(account) {
        st.send(sid, f.clone());
    }
}

/// LG-311: the client tells it is back in the waiting room (0x306 {1}, also when the game screen closes): a waiting
/// notice opens now rather than at the next login (0x367 is accepted unprompted; each notice is read once).
// SPEC: PROTOCOL_2009.md §3.3 0x367 ("Also accepted unprompted")
pub fn on_back_in_room(st: &mut Core, sid: Sid) -> HResult {
    let Some(account) = st.sessions.get(&sid).and_then(|s| s.account) else {
        return Ok(());
    };
    let waiting: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM notices WHERE user_no = ?1",
        [account as i64],
        |r| r.get(0),
    )?;
    if waiting > 0 {
        crate::inbox::on_notice(st, sid)?;
    }
    Ok(())
}

/// Who an event pays.
pub enum EventTargets<'a> {
    /// Every account with a character, not disabled and no bot, seen since this unix time.
    ActiveSince(i64),
    /// These logins (unknown names and accounts without a character are counted as skipped).
    Names(&'a [String]),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EventOutcome {
    pub paid: u32,
    pub already: u32,
    pub skipped: u32,
}

/// LG-312: pays an event: `amount` NX and the box `<label>: +<amount> NX` at the next login, once per event id and
/// account, however often the command runs.
pub fn run_event(
    c: &Connection,
    id: &str,
    label: &str,
    amount: i64,
    targets: &EventTargets<'_>,
    now: i64,
) -> DbResult<EventOutcome> {
    atomic(c, || {
        let mut out = EventOutcome::default();
        let accounts: Vec<u32> = match targets {
            EventTargets::ActiveSince(since) => {
                let mut q = c.prepare(
                    "SELECT a.user_no, a.login FROM accounts a JOIN characters h ON h.user_no = a.user_no \
                     WHERE a.last_login >= ?1 AND a.disabled = 0 ORDER BY a.user_no",
                )?;
                let rows = q.query_map([since], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut v = Vec::new();
                for row in rows {
                    let (no, login) = row?;
                    if !crate::grant::is_bot_name(&login) {
                        v.push(no as u32);
                    }
                }
                v
            }
            EventTargets::Names(names) => {
                let mut v = Vec::new();
                for n in names.iter() {
                    match db::account_by_login(c, n)? {
                        Some(a) if db::character(c, a.user_no)?.is_some() => v.push(a.user_no),
                        _ => out.skipped += 1,
                    }
                }
                v
            }
        };
        for account in accounts {
            let fresh = c.execute(
                "INSERT OR IGNORE INTO nx_events (event_id, user_no, amount, label, created) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, account as i64, amount, label, now],
            )? == 1;
            if !fresh {
                out.already += 1;
                continue;
            }
            db::grant(c, account, amount, 0, 0, 0)?;
            crate::inbox::queue_notice_on(c, account, &format!("{label}: +{amount} NX"), now)?;
            out.paid += 1;
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_account(c: &Connection, login: &str) -> u32 {
        let no = db::create_account(c, login, 0).unwrap();
        db::create_character(
            c,
            &db::NewCharacter {
                user_no: no,
                nick: login,
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        no
    }

    fn nx(c: &Connection, a: u32) -> i64 {
        db::character(c, a).unwrap().unwrap().nx
    }

    fn remove(p: &std::path::Path) {
        for e in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{e}", p.display()));
        }
    }

    #[test]
    fn utc_day_turns_over_at_midnight() {
        assert_eq!(utc_day(86_399), 0);
        assert_eq!(utc_day(86_400), 1);
    }

    #[test]
    fn one_daily_bonus_per_day_also_after_a_restart_and_a_clock_replay() {
        let p = std::env::temp_dir().join(format!("dc_nx_daily_{}.db", std::process::id()));
        remove(&p);
        let c = db::open(&p).unwrap();
        let a = with_account(&c, "Kim");
        assert!(claim_daily(&c, a, 250, 100).unwrap());
        assert!(!claim_daily(&c, a, 250, 100).unwrap(), "same day");
        drop(c);
        let c = db::open(&p).unwrap();
        assert!(!claim_daily(&c, a, 250, 100).unwrap(), "after a restart");
        assert!(!claim_daily(&c, a, 250, 99).unwrap(), "clock set back");
        assert_eq!(nx(&c, a), 250);
        assert!(claim_daily(&c, a, 250, 101).unwrap(), "day boundary");
        assert_eq!(nx(&c, a), 500);
        assert!(!claim_daily(&c, a, 250, 100).unwrap(), "yesterday again");
        drop(c);
        remove(&p);
    }

    #[test]
    fn parallel_connections_pay_once() {
        let p = std::env::temp_dir().join(format!("dc_nx_par_{}.db", std::process::id()));
        remove(&p);
        let c = db::open(&p).unwrap();
        let a = with_account(&c, "Kim");
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let c = db::open(&p).unwrap();
                    claim_daily(&c, a, 250, 7).unwrap()
                })
            })
            .collect();
        let wins = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|w| *w)
            .count();
        assert_eq!((wins, nx(&c, a)), (1, 250));
        drop(c);
        remove(&p);
    }

    #[test]
    fn clan_wins_stop_at_the_cap_until_the_next_day() {
        let c = db::memory();
        let a = with_account(&c, "Kim");
        for _ in 0..3 {
            assert!(claim_clan_win(&c, a, 200, 3, 5).unwrap());
        }
        assert!(!claim_clan_win(&c, a, 200, 3, 5).unwrap());
        assert!(!claim_clan_win(&c, a, 200, 3, 4).unwrap(), "clock set back");
        assert_eq!(nx(&c, a), 600);
        assert!(claim_clan_win(&c, a, 200, 3, 6).unwrap());
        assert_eq!(nx(&c, a), 800);
    }

    #[test]
    fn clan_guards() {
        let r = Rules::default();
        let (secs, n) = (
            Some(u64::from(r.nx_clan_min_seconds)),
            r.nx_clan_min_players,
        );
        assert!(clan_win_counts(&r, 1, secs, [n, n]));
        assert!(clan_win_counts(&r, 2, secs, [n, n]));
        assert!(!clan_win_counts(&r, 0, secs, [n, n]), "draw");
        assert!(!clan_win_counts(&r, 1, secs, [n, n - 1]), "forfeit: too few losers");
        assert!(!clan_win_counts(&r, 1, secs, [n - 1, n]), "too few winners");
        assert!(
            !clan_win_counts(&r, 1, Some(secs.unwrap() - 1), [n, n]),
            "too short"
        );
        assert!(!clan_win_counts(&r, 1, None, [n, n]), "never played");
    }

    #[test]
    fn defaults_are_on_without_any_config() {
        let r = Rules::default();
        assert!(r.nx_enabled && r.nx_daily > 0 && r.nx_clan_win > 0 && r.nx_clan_win_daily_cap > 0);
    }

    #[test]
    fn an_event_pays_once_per_id_and_skips_bots_and_the_inactive() {
        let c = db::memory();
        let (a, b, bot) = (
            with_account(&c, "Kim"),
            with_account(&c, "Lee"),
            with_account(&c, "PcBot3"),
        );
        for (x, seen) in [(a, 1000), (b, 10), (bot, 1000)] {
            c.execute(
                "UPDATE accounts SET last_login = ?2 WHERE user_no = ?1",
                params![x as i64, seen],
            )
            .unwrap();
        }
        let t = EventTargets::ActiveSince(500);
        let o = run_event(&c, "xmas", "Holiday gift", 1000, &t, 1000).unwrap();
        assert_eq!(
            o,
            EventOutcome {
                paid: 1,
                already: 0,
                skipped: 0
            }
        );
        assert_eq!((nx(&c, a), nx(&c, b), nx(&c, bot)), (1000, 0, 0));
        let o = run_event(&c, "xmas", "Holiday gift", 1000, &t, 1001).unwrap();
        assert_eq!((o.paid, o.already), (0, 1));
        assert_eq!(nx(&c, a), 1000, "the same id pays once");
        let text: String = c
            .query_row(
                "SELECT text FROM notices WHERE user_no = ?1",
                [a as i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "Holiday gift: +1000 NX");
        assert_eq!(db::character(&c, a).unwrap().unwrap().notice_pending, 1);
        let names = vec!["lee".to_string(), "Kim".to_string(), "Nobody".to_string()];
        let o = run_event(
            &c,
            "xmas",
            "Holiday gift",
            1000,
            &EventTargets::Names(&names),
            1002,
        )
        .unwrap();
        assert_eq!((o.paid, o.already, o.skipped), (1, 1, 1));
        assert_eq!(nx(&c, b), 1000);
    }
}
