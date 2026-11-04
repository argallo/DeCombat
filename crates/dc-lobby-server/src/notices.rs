//! Notices, pushes and player reports: the operator ticker 0x3052, item expiry boxes 0x3011, the grade push
//! 0x3003, supply case announcements 0x3013, the shop version pushes of `/shopver` (built by `shop.rs`), the
//! operator login texts (0x3052 at login, 0x366 / 0x367 notice box through the inbox notice queue) and the player
//! report 0x312. Also the test chat commands `/notice`, `/expire`, `/shopver`, `/grade`, `!hisec`.
// SPEC: PROTOCOL_2009.md §3.6 0x3052, §4.2 0x3011, §3.3 0x3003, §4.3 0x3013, §3.4 0x312, §3.5 version pushes
//
// Already sent elsewhere and only reused here: 0x3004 (game.rs, after a match), 0x3014 and the notice queue
// (inbox.rs), 0x33f (inventory.rs), 0x3054..0x305b (shop.rs builds them, `/shopver` raises the versions).

use crate::clock;
use crate::db;
use crate::delivery::Delivered;
use crate::opscfg::TestCommands;
use crate::shop;
use crate::state::{Core, Fault, HResult, Sid};
use crate::wire;
use dc_wire::{PktReader, PktWriter};
use rusqlite::params;
use std::collections::BTreeMap;

pub const GRADE_PUSH: u16 = 0x3003;
pub const EXPIRY_NOTICE_PUSH: u16 = 0x3011;
pub const CASE_ANNOUNCE_PUSH: u16 = 0x3013;
pub const TICKER_PUSH: u16 = 0x3052;
pub const PLAYER_REPORT: u16 = 0x312;

/// Longest ticker text sent (UTF-16 units); the 0x8000 body limit is far away.
const TICKER_MAX_UNITS: usize = 512;
/// Item numbers per 0x3011 (each one opens a box on the client).
pub const EXPIRY_BOX_MAX: usize = 50;

// ---------------------------------------------------------------------------------------------------------------
// layouts
// ---------------------------------------------------------------------------------------------------------------

/// 0x3052: +6 u32 period (s); +0xa u32 repeat count; +0xe u32 text length; +0x12 wstr text (NUL-terminated).
// SPEC: PROTOCOL_2009.md §3.6 0x3052 (EVIDENCE: cshell+0x34270)
pub fn ticker_frame(period_secs: u32, repeat: u32, text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().take(TICKER_MAX_UNITS).collect();
    let mut w = PktWriter::new(TICKER_PUSH);
    w.u32(period_secs).u32(repeat).u32(units.len() as u32);
    for u in &units {
        w.u16(*u);
    }
    w.u16(0);
    w.into_frame()
}

/// 0x3011: +6 u32 count; +0xa u32[count] item numbers (one box `res 0x1786` per item, no inventory change).
// SPEC: PROTOCOL_2009.md §4.2 0x3011 (EVIDENCE: cshell+0x39280)
pub fn expiry_box_frame(item_nos: &[u32]) -> Vec<u8> {
    let n = item_nos.len().min(EXPIRY_BOX_MAX);
    let mut w = PktWriter::new(EXPIRY_NOTICE_PUSH);
    w.u32(n as u32);
    for i in &item_nos[..n] {
        w.u32(*i);
    }
    w.into_frame()
}

/// 0x3003 (0xa): +6 u32 grade.
// SPEC: PROTOCOL_2009.md §3.3 0x3003 (EVIDENCE: cshell+0x36690)
pub fn grade_frame(grade: u32) -> Vec<u8> {
    wire::u32_push(GRADE_PUSH, grade)
}

/// 0x3013 (0x38): +6 u32 0; +0x12 wstr[17] player name; +0x34 u32 item number.
// SPEC: PROTOCOL_2009.md §4.3 0x3013 (EVIDENCE: cshell+0x41470)
pub fn case_announce_frame(name: &str, item_no: u32) -> Vec<u8> {
    let mut w = PktWriter::new(CASE_ANNOUNCE_PUSH);
    w.u32(0);
    w.at(0x12).wstr(name, 17).u32(item_no);
    w.into_frame()
}

/// Parsed 0x312 player report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub kind: u32,
    pub key_a: u32,
    pub key_b: u32,
    pub name: String,
    pub text: String,
    /// Kind 2: the free text; kind 1: the in-match statistics block as space-separated words.
    pub extra: String,
}

/// 0x312 (0xe4c): +6 kind (0 plain, 1 with statistics, 2 with free text); +0xa / +0xe the picked general kind and detail index (`key_a` / `key_b`); +0x12 wstr[17]
/// name; +0x34 wstr[601] report text; from +0x4e6 the kind body (kind 2: u32 length + wstr[1200] text; kind 1:
/// thirteen u32 statistics words).
// EVIDENCE: cshell+0x273790 (the builder copies the dialog's text into a 601-unit field at +0x34 and, for kind 2,
// the length to +0x4e6 and at most 0x4b0 units to +0x4ea), sender cshell+0x18e9d0
pub fn parse_report(p: &[u8]) -> Report {
    let mut r = PktReader::new(p);
    let kind = r.at(6).u32();
    let key_a = r.u32();
    let key_b = r.u32();
    let name = r.at(0x12).wstr(17);
    let text = r.at(0x34).wstr(601);
    let extra = match kind {
        2 => {
            let len = r.at(0x4e6).u32() as usize;
            let units: Vec<u16> = r
                .at(0x4ea)
                .wstr(0x4b0)
                .encode_utf16()
                .take(len.min(0x4b0))
                .collect();
            String::from_utf16_lossy(&units)
        }
        1 => {
            r.at(0x4e6);
            (0..13)
                .map(|_| r.u32().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => String::new(),
    };
    Report {
        kind,
        key_a,
        key_b,
        name,
        text,
        extra,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// sends
// ---------------------------------------------------------------------------------------------------------------

/// Every lobby session (server-wide pushes).
pub(crate) fn everyone(st: &Core) -> Vec<Sid> {
    st.sessions
        .values()
        .filter(|s| s.in_lobby && !s.closing)
        .map(|s| s.sid)
        .collect()
}

/// The operator ticker to every lobby session.
// DC-DEFAULT: OPS-2 (proposed) notice.ticker_period_secs / notice.ticker_repeat
pub fn broadcast_ticker(st: &mut Core, text: &str) -> usize {
    let f = ticker_frame(
        st.rules.ops.ticker_period_secs,
        st.rules.ops.ticker_repeat,
        text,
    );
    let all = everyone(st);
    st.send_all(&all, &f);
    all.len()
}

/// Removes expired items like the CD-06 scan (0x33f to the owners) and, when `boxes` is set, also shows the
/// owners one box per expired item number with 0x3011. `None` = every account.
// DC-DEFAULT: CD-42 removal pushes use 0x33f; plain notices use 0x3011; OPS-12 (proposed) both on every scan
pub fn expire_and_tell(st: &mut Core, account: Option<u32>, boxes: bool) -> HResult {
    let gone = db::expired_items(&st.db, clock::now_ticks(), account)?;
    crate::inventory::expire(st, account, true)?;
    if !boxes {
        return Ok(());
    }
    let mut by_user: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for it in &gone {
        by_user
            .entry(it.user_no)
            .or_default()
            .push(it.item_no.clamp(0, u32::MAX as i64) as u32);
    }
    for (user, nos) in by_user {
        for sid in st.sids_of(user) {
            for chunk in nos.chunks(EXPIRY_BOX_MAX) {
                st.send(sid, expiry_box_frame(chunk));
            }
        }
    }
    Ok(())
}

/// The periodic expiry scan (net.rs): boxes per `notice.expiry_boxes`.
pub fn expire_with_boxes(st: &mut Core, account: Option<u32>) -> HResult {
    let boxes = st.rules.ops.expiry_boxes;
    expire_and_tell(st, account, boxes)
}

/// A supply case award: 0x3013 to every lobby session for the awards `notice.case_announce` selects.
// DC-DEFAULT: OPS-9 (proposed) permanent awards are announced server-wide
pub fn case_award(st: &mut Core, account: u32, award: &Delivered) -> HResult {
    use crate::opscfg::CaseAnnounce;
    let tell = match st.rules.ops.case_announce {
        CaseAnnounce::Off => false,
        CaseAnnounce::Permanent => award.days == 0,
        CaseAnnounce::All => true,
    };
    if !tell {
        return Ok(());
    }
    let name = st.character(account).map(|c| c.nick).unwrap_or_default();
    let f = case_announce_frame(&name, award.item_no.clamp(0, u32::MAX as i64) as u32);
    let all = everyone(st);
    st.send_all(&all, &f);
    Ok(())
}

/// Lobby login: the operator's login ticker (0x3052 to this session) and login box (queued 0x367 notice, read
/// once through 0x303 +0x104 -> 0x366; not queued twice while one is unread).
// DC-DEFAULT: OPS-10 (proposed) notice.login_ticker / notice.login_box
pub fn on_lobby_login(st: &mut Core, sid: Sid, account: u32) -> HResult {
    let ticker = st.rules.ops.login_ticker.clone();
    if !ticker.is_empty() {
        let f = ticker_frame(
            st.rules.ops.ticker_period_secs,
            st.rules.ops.ticker_repeat,
            &ticker,
        );
        st.send(sid, f);
    }
    let boxed = st.rules.ops.login_box.clone();
    if !boxed.is_empty() {
        let pending: i64 = st.db.query_row(
            "SELECT COUNT(*) FROM notices WHERE user_no = ?1 AND text = ?2",
            params![account as i64, boxed],
            |r| r.get(0),
        )?;
        if pending == 0 {
            crate::inbox::queue_notice(st, account, &boxed)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------------------------------------------

/// 0x312: stored for the operators; there is no reply (SPEC: PROTOCOL_2009.md §3.4 0x312).
// DC-DEFAULT: OPS-7 (proposed) at most reports.per_hour stored reports per reporter
fn on_report(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let Some(reporter) = st
        .sessions
        .get(&sid)
        .filter(|s| s.in_lobby)
        .and_then(|s| s.account)
    else {
        return Ok(());
    };
    let now = clock::unix_now();
    let cap = st.rules.ops.reports_per_hour as usize;
    let times = st.ops.report_times.entry(reporter).or_default();
    times.retain(|&t| now - t < 3600);
    if times.len() >= cap {
        st.note(&format!(
            "report from account {} dropped (hourly cap {})",
            reporter, cap
        ));
        return Ok(());
    }
    times.push(now);
    let r = parse_report(p);
    let login = db::account_by_no(&st.db, reporter)?
        .map(|a| a.login)
        .unwrap_or_default();
    let nick = st.character(reporter).map(|c| c.nick).unwrap_or_default();
    let place = crate::reportmail::place_of(st, sid);
    // LG-316: a row stored with the mail off is never mailed later
    let mailed = if crate::reportmail::enabled(st) { 0 } else { 2 };
    st.db.execute(
        "INSERT INTO reports (kind, target_nick, text, extra, reporter, created, category, sub, mailed, reporter_login, reporter_nick, place) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            r.kind as i64,
            r.name,
            r.text,
            r.extra,
            reporter as i64,
            now,
            r.key_a as i64,
            r.key_b as i64,
            mailed,
            login,
            nick,
            place
        ],
    )?;
    st.note(&format!(
        "report (kind {}, category {}) from account {} about {}: {}",
        r.kind,
        r.key_a,
        reporter,
        crate::reportmail::one_line(&r.name),
        crate::reportmail::one_line(&r.text)
    ));
    // MON-3
    crate::opsmon::event(
        st,
        reporter,
        crate::opsmon::kind::REPORT,
        &format!(
            "report (kind {}, category {}) about {}: {}",
            r.kind,
            r.key_a,
            crate::reportmail::one_line(&r.name),
            crate::reportmail::one_line(&r.text)
        ),
    );
    crate::reportmail::pump(st)?;
    Ok(())
}

/// Whether a lobby session may use the test chat commands.
// DC-DEFAULT: OPS-1 (proposed) GM accounts always; everyone with gm.test_commands = "all" or, by default, while
// the server listens on a loopback address only (one machine: every user is the operator)
pub fn may_use_test_commands(st: &Core, account: u32) -> bool {
    if crate::gm::is_gm(st, account) {
        return true;
    }
    match st.rules.ops.test_commands {
        TestCommands::All => true,
        TestCommands::Gm => false,
        TestCommands::Loopback => bind_is_loopback(&st.opts.bind),
    }
}

pub fn bind_is_loopback(bind: &str) -> bool {
    let b = bind.trim();
    b.eq_ignore_ascii_case("localhost")
        || b.parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Shop list names of `/shopver` (none = all of them).
fn shop_keys(args: &str) -> Vec<i64> {
    let all = vec![
        shop::KEY_VIEW,
        shop::KEY_HOT,
        shop::KEY_NEW,
        shop::KEY_SALE,
        shop::KEY_LIST3,
        shop::KEY_LIST4,
        shop::KEY_PRICES,
    ];
    let mut out = Vec::new();
    for w in args.split_whitespace() {
        let k = match w.to_ascii_lowercase().as_str() {
            "view" => shop::KEY_VIEW,
            "hot" | "0" => shop::KEY_HOT,
            "new" | "1" => shop::KEY_NEW,
            "sale" | "2" => shop::KEY_SALE,
            "list3" | "3" => shop::KEY_LIST3,
            "list4" | "4" => shop::KEY_LIST4,
            "price" | "prices" => shop::KEY_PRICES,
            "all" => return all,
            _ => continue,
        };
        if !out.contains(&k) {
            out.push(k);
        }
    }
    if out.is_empty() {
        all
    } else {
        out
    }
}

/// `/notice`, `/expire`, `/shopver`, `/grade` from the lobby chat; false = not one of them (or not allowed), so
/// the line goes on as ordinary chat.
fn on_chat_command(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    let Some(account) = st
        .sessions
        .get(&sid)
        .filter(|s| s.in_lobby && !s.closing)
        .and_then(|s| s.account)
    else {
        return Ok(false);
    };
    let text = wire::parse_chat(p);
    let t = text.trim();
    // `!hisec <item>` is the HiSec-case award announcement test command (a HiSec case is a supply case whose
    // award is announced server-wide: 0x3013 with the opener's name). HARNESS: lobby_harness::test_lb5cash
    let Some(rest) = t
        .strip_prefix('/')
        .or_else(|| t.strip_prefix("!hisec").map(|_| t.trim_start_matches('!')))
    else {
        return Ok(false);
    };
    let (word, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let args = args.trim();
    let word = word.to_ascii_lowercase();
    if !matches!(
        word.as_str(),
        "notice" | "expire" | "shopver" | "grade" | "hisec"
    ) || !may_use_test_commands(st, account)
    {
        return Ok(false);
    }
    match word.as_str() {
        "hisec" => {
            let Ok(item) = args.split_whitespace().next().unwrap_or("").parse::<u32>() else {
                return Ok(false);
            };
            let name = st.character(account).map(|c| c.nick).unwrap_or_default();
            let f = case_announce_frame(&name, item);
            let all = everyone(st);
            st.send_all(&all, &f);
            st.note(&format!(
                "HiSec award announcement by account {}: item {}",
                account, item
            ));
        }
        "notice" => {
            if args.is_empty() {
                return Ok(false);
            }
            let n = broadcast_ticker(st, args);
            st.note(&format!(
                "notice by account {} to {} session(s): {}",
                account, n, args
            ));
        }
        "expire" => {
            expire_and_tell(st, Some(account), true)?;
            st.note(&format!("expiry scan for account {} on request", account));
        }
        "shopver" => {
            let all = everyone(st);
            for key in shop_keys(args) {
                let v = shop::raise_version(st, key)?;
                if let Some(f) = shop::version_push(key, v) {
                    st.send_all(&all, &f);
                }
            }
            st.note(&format!(
                "shop versions raised by account {} ({})",
                account,
                if args.is_empty() { "all" } else { args }
            ));
        }
        "grade" => {
            let Ok(g) = args.split_whitespace().next().unwrap_or("").parse::<u32>() else {
                return Ok(false);
            };
            st.db.execute(
                "UPDATE characters SET grade = ?2 WHERE user_no = ?1",
                params![account as i64, g as i64],
            )?;
            let f = grade_frame(g);
            for s in st.sids_of(account) {
                st.send(s, f.clone());
            }
            st.note(&format!("grade of account {} set to {}", account, g));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Report and chat-command messages; false when not handled here.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, Fault> {
    match id {
        PLAYER_REPORT => on_report(st, sid, p)?,
        crate::msg::CHAT_SEND => return on_chat_command(st, sid, p),
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    fn split(f: &[u8]) -> (i32, Vec<u8>) {
        let (frames, used) = dc_wire::parse_tcp_frames(f).unwrap();
        assert_eq!(used, f.len());
        frames.into_iter().next().unwrap()
    }

    #[test]
    fn ticker_layout() {
        let (id, p) = split(&ticker_frame(30, 3, "Server maintenance"));
        assert_eq!(
            (id, u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0xe)),
            (0x3052, 30, 3, 18)
        );
        assert_eq!(PktReader::new(&p).at(0x12).wstr(19), "Server maintenance");
        assert_eq!(p.len(), 0x12 + 2 * 19, "text plus its NUL");
    }

    #[test]
    fn expiry_grade_case_layouts() {
        let (id, p) = split(&expiry_box_frame(&[12, 7]));
        assert_eq!(
            (id, p.len(), u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0xe)),
            (0x3011, 0xa + 8, 2, 12, 7)
        );
        let many: Vec<u32> = (0..80).collect();
        let (_, p) = split(&expiry_box_frame(&many));
        assert_eq!(u32_at(&p, 6), EXPIRY_BOX_MAX as u32);
        let (id, p) = split(&grade_frame(3));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3003, 0xa, 3));
        let (id, p) = split(&case_announce_frame("Dave", 54));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3013, 0x38, 0));
        assert_eq!(
            (PktReader::new(&p).at(0x12).wstr(17), u32_at(&p, 0x34)),
            ("Dave".into(), 54)
        );
    }

    #[test]
    fn report_layout_kind_2_and_1() {
        // HARNESS: lobby_harness::test_lb5gm builds the same kind-2 report
        let mut w = PktWriter::new(PLAYER_REPORT);
        w.u32(2)
            .u32(1)
            .u32(9)
            .wstr("Alice", 17)
            .wstr("bad words", 601)
            .u32(3)
            .wstr("abc", 0x4b0);
        w.at(0xe4c);
        let p = w.into_payload();
        assert_eq!(p.len(), 0xe4c);
        let r = parse_report(&p);
        assert_eq!(
            r,
            Report {
                kind: 2,
                key_a: 1,
                key_b: 9,
                name: "Alice".into(),
                text: "bad words".into(),
                extra: "abc".into()
            }
        );
        // the length word caps the free text
        let mut w = PktWriter::new(PLAYER_REPORT);
        w.u32(2)
            .u32(0)
            .u32(0)
            .wstr("A", 17)
            .wstr("", 601)
            .u32(2)
            .wstr("abcdef", 0x4b0);
        assert_eq!(parse_report(&w.into_payload()).extra, "ab");
        let mut w = PktWriter::new(PLAYER_REPORT);
        w.u32(1).u32(0).u32(0).wstr("A", 17).wstr("x", 601);
        for i in 0..13u32 {
            w.u32(i);
        }
        assert_eq!(
            parse_report(&w.into_payload()).extra,
            "0 1 2 3 4 5 6 7 8 9 10 11 12"
        );
    }

    #[test]
    fn shopver_names() {
        assert_eq!(
            shop_keys("hot sale price"),
            vec![shop::KEY_HOT, shop::KEY_SALE, shop::KEY_PRICES]
        );
        assert_eq!(shop_keys("").len(), 7);
        assert_eq!(shop_keys("bogus").len(), 7);
    }

    #[test]
    fn loopback_binds() {
        assert!(
            bind_is_loopback("127.0.0.1")
                && bind_is_loopback("::1")
                && bind_is_loopback("localhost")
        );
        assert!(!bind_is_loopback("0.0.0.0") && !bind_is_loopback("192.168.1.2"));
    }
}
