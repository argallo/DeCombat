//! HiSec cases on the lobby side: the dedicated server's `@hisec` pickup line becomes one inbox row of the HiSec
//! case product and an inbox-stale push 0x3014 (HS-6); opening such a case (0x32e) needs one owned matching key,
//! which the server consumes itself (HS-7); the award is drawn from the case's ROM pool with the HiSec duration
//! table (HS-8); the opening is announced with 0x3013 (HS-9). The harness trigger `!hisec <item>` (HS-11) lives
//! in `notices.rs`.
//!
//! Which items are HiSec cases and keys is read from the user's ROM item table at run time (never a number in
//! this tree): a case is a supply item (8000..) whose `[Supply<n>]` section has a first `FunctionValue` that is
//! not -1; its keys are the function items (9000..) of `SubType` 19 with the same first `FunctionValue`.
// SPEC: HISEC_2009.md §4.1 (case and key tests, EVIDENCE: cshell+0x14cc00, cshell+0x25c760), §4.2 (the request
// names no key: the server finds it), §4.3 (0x32f +0x1e = the key's unique id), §2 (0x3013), §6.1 (wire duties)
// SPEC: DSERVER_PIPE.md §4 (event lines; `@hisec` is the DeCombat addition HS-13 of dc-dserver)
// OBSERVED: ROM table keys ITEMS.TXT `[Supply<n>]` / `[UseItem<n>]` `FunctionValue<k>`, `SubType`, `ItemID`;
// HISECS.TXT `[Hisec<n>]` `Type`, `SubType` (key names only; values read at run time)

use crate::delivery::{self, Delivered};
use crate::inbox;
use crate::shop::{self, reason};
use crate::state::{Core, HResult, Sid};
use crate::{clock, db};
use dc_bute::SectionMap;
use dc_mod_lua::JsonValue;
use rusqlite::params;
use std::collections::BTreeMap;

/// `SubType` of a HiSec key in the ROM function-item table.
// EVIDENCE: cshell+0x14cc00 (the key lookup cshell+0x25c760 is called with type 0x15 and subtype 0x13)
pub const SUB_HISEC_KEY: i64 = 0x13;

/// Who hears a HiSec opening (0x3013).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Announce {
    /// Every connected lobby session (including the opener and players inside a match).
    All,
    /// Every lobby session in the opener's channel.
    Channel,
    Off,
}

/// Operator rules of the HiSec lobby path (`lobby.json` / `server.toml` keys below).
#[derive(Clone, Debug)]
pub struct HisecRules {
    // DC-DEFAULT: HS-6 (proposed) the product a picked-up case becomes; 0 = resolved from the ROM tables
    pub case_product: u32,
    // DC-DEFAULT: HS-9 (proposed) 0x3013 to every lobby session after a HiSec opening
    pub announce: Announce,
    // DC-DEFAULT: HS-8 (proposed) award duration table of HiSec cases, separate from CD-01: [days, weight]
    pub odds: Vec<(u32, u32)>,
    // DC-DEFAULT: HS-7 (proposed) 0x32f reason when no usable key is owned (0x99: outside the client's table)
    pub no_key_reason: u32,
    // DC-DEFAULT: CRATE-2 (amended 2026-11-07, OWNER DIRECTION) percent of the drops that are Camo Crates; `None` =
    // the game server's own default (75), `Some(n)` goes to every game server as `-hisec camo_share=<n>`
    pub camo_share: Option<u32>,
}

impl Default for HisecRules {
    fn default() -> Self {
        Self {
            case_product: 0,
            announce: Announce::All,
            // DC-DEFAULT: HS-8 (proposed) 7 days 50 %, 30 days 35 %, permanent 15 %
            odds: vec![(7, 50), (30, 35), (0, 15)],
            no_key_reason: 0x99,
            camo_share: None,
        }
    }
}

fn num(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

impl HisecRules {
    /// Keys this struct owns.
    pub fn owns(key: &str) -> bool {
        matches!(
            key,
            "hisec.case_product"
                | "hisec.announce"
                | "hisec.no_key_reason"
                | "hisec.camo_share"
                | "economy.hisec_case_odds"
        )
    }

    /// Applies one override; false for an unknown key or a value of the wrong shape.
    pub fn apply(&mut self, key: &str, v: &JsonValue) -> bool {
        let n = num(v);
        match key {
            "hisec.case_product" => n
                .filter(|x| (0.0..=65535.0).contains(x))
                .map(|x| self.case_product = x as u32)
                .is_some(),
            "hisec.announce" => match v {
                JsonValue::String(s) => {
                    self.announce = match s.as_str() {
                        "all" => Announce::All,
                        "channel" => Announce::Channel,
                        "off" => Announce::Off,
                        _ => return false,
                    };
                    true
                }
                _ => false,
            },
            "hisec.no_key_reason" => n
                .filter(|x| (1.0..=65535.0).contains(x))
                .map(|x| self.no_key_reason = x as u32)
                .is_some(),
            "hisec.camo_share" => n
                .filter(|x| (0.0..=100.0).contains(x) && x.fract() == 0.0)
                .map(|x| self.camo_share = Some(x as u32))
                .is_some(),
            "economy.hisec_case_odds" => {
                let JsonValue::Array(a) = v else { return false };
                let mut out = Vec::new();
                for e in a {
                    match e {
                        JsonValue::Array(p) if p.len() == 2 => match (num(&p[0]), num(&p[1])) {
                            (Some(d), Some(w)) if d >= 0.0 && w >= 0.0 => {
                                out.push((d as u32, w as u32))
                            }
                            _ => return false,
                        },
                        _ => return false,
                    }
                }
                if out.iter().all(|&(_, w)| w == 0) {
                    return false;
                }
                self.odds = out;
                true
            }
            _ => false,
        }
    }
}

// ------------------------------------------------------------------------------------------- ROM lookups

/// The first `FunctionValue<k>` of a section (lowest numeric suffix `k`), if any.
// EVIDENCE: cshell+0xe5280 parses the `FunctionValue` list into the item record (count +0x418, values from +0x420);
// cshell+0x14cc00 tests the first value
fn first_function_value(sec: &SectionMap) -> Option<i64> {
    sec.iter()
        .filter_map(|(k, v)| {
            let suffix = k.strip_prefix("FunctionValue")?;
            let n: u64 = if suffix.is_empty() {
                0
            } else if suffix.bytes().all(|b| b.is_ascii_digit()) {
                suffix.parse().ok()?
            } else {
                return None;
            };
            Some((n, v.trim().trim_matches('"').parse::<i64>().ok()?))
        })
        .min_by_key(|&(n, _)| n)
        .map(|(_, v)| v)
}

/// The key class of a HiSec case (its first `FunctionValue`), or None when `item_no` is no HiSec case.
// SPEC: HISEC_2009.md §4.1 (a supply item whose first FunctionValue is not -1; EVIDENCE: cshell+0x14cc00)
pub fn case_value(st: &Core, item_no: i64) -> Option<i64> {
    if !(8000..9000).contains(&item_no) {
        return None;
    }
    let sec = st
        .rom
        .items
        .as_ref()?
        .doc()
        .get(&format!("Supply{}", item_no - 8000))?;
    first_function_value(sec).filter(|&v| v != -1)
}

/// Every HiSec case item of the ROM with its key class, by item number.
fn cases(st: &Core) -> Vec<(i64, i64)> {
    let Some(t) = st.rom.items.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<(i64, i64)> = t
        .doc()
        .iter()
        .filter_map(|(tag, sec)| {
            let n: i64 = tag.strip_prefix("Supply")?.parse().ok()?;
            let v = first_function_value(sec).filter(|&v| v != -1)?;
            Some((8000 + n, v))
        })
        .collect();
    out.sort_unstable();
    out
}

/// The key items (function items of `SubType` 19) whose first `FunctionValue` equals `value`, by item number.
// SPEC: HISEC_2009.md §4.1 (EVIDENCE: cshell+0x25c760 called with type 0x15, the case's value, subtype 0x13)
pub fn key_items(st: &Core, value: i64) -> Vec<i64> {
    let Some(t) = st.rom.items.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<i64> = t
        .doc()
        .iter()
        .filter(|(tag, _)| tag.starts_with("UseItem"))
        .filter(|(_, sec)| {
            sec.get("SubType")
                .and_then(|s| s.trim().parse::<i64>().ok())
                == Some(SUB_HISEC_KEY)
        })
        .filter(|(_, sec)| first_function_value(sec) == Some(value))
        .filter_map(|(_, sec)| sec.get("ItemID")?.trim().parse::<i64>().ok())
        .map(|id| 9000 + id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The case item a dserver case-table row stands for: the row's `Type` is the case's key class and its `SubType`
/// picks among the cases of that class in item-number order (first when out of range).
// DC-DEFAULT: HS-6 (amendment, proposed; PLAUSIBLE reading of the ROM case table: `Type` matches the cases' first
// FunctionValue, `SubType` the variant)
pub fn case_item_for_row(st: &Core, row: u32) -> Option<i64> {
    let (class, variant) = st.rom.hisec_rows.get(&row).copied()?;
    let same: Vec<i64> = cases(st)
        .into_iter()
        .filter(|&(_, v)| v == class)
        .map(|(no, _)| no)
        .collect();
    same.get(variant.max(0) as usize)
        .or_else(|| same.first())
        .copied()
}

/// The lowest catalogue product whose first item is `item_no`.
pub fn product_of_item(st: &Core, item_no: i64) -> Option<i32> {
    let cat = st.rom.shop.catalog.as_ref()?;
    let scan: Vec<i32> = if st.rom.shop.all_products.is_empty() {
        (0..0x10000).collect()
    } else {
        st.rom.shop.all_products.clone()
    };
    scan.into_iter().find(|&pid| {
        cat.product(pid)
            .map(|p| shop::product_items(cat, p))
            .and_then(|items| items.first().map(|i| i.item_no == item_no))
            .unwrap_or(false)
    })
}

/// The product a picked-up case of dserver row `row` becomes: the operator's `hisec.case_product`, else the
/// product of the row's case item, else the product of the first HiSec case of the ROM.
// DC-DEFAULT: HS-6 (proposed) product = the first ROM product whose item is a HiSec case; operators may name one
pub fn case_product(st: &Core, row: u32) -> Option<i32> {
    if st.rules.hisec.case_product != 0 {
        return Some(st.rules.hisec.case_product as i32);
    }
    if let Some(p) = case_item_for_row(st, row).and_then(|it| product_of_item(st, it)) {
        return Some(p);
    }
    cases(st)
        .into_iter()
        .find_map(|(it, _)| product_of_item(st, it))
}

/// The HiSec case table rows `(Type, SubType)` by row number, from the ROM's case table.
pub fn rows_from_doc(doc: &dc_bute::ButeDoc) -> BTreeMap<u32, (i64, i64)> {
    doc.iter()
        .filter_map(|(tag, sec)| {
            let n: u32 = tag.strip_prefix("Hisec")?.parse().ok()?;
            let int = |k: &str| sec.get(k).and_then(|s| s.trim().parse::<i64>().ok());
            Some((n, (int("Type")?, int("SubType").unwrap_or(0))))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------- HS-6 delivery

/// One `@hisec uid=<u32> case=<row> key=<case key> name=<nick>` line (`name=` last, to the end of the line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pickup {
    pub uid: u32,
    pub case_row: u32,
    pub key: u32,
    pub name: String,
}

/// Parses the line; None when `uid` or `key` is missing or not a number.
// SPEC: DSERVER_PIPE.md §4 (whitespace split, k=v tokens, unknown keys ignored); DC-DEFAULT: HS-13 (proposed)
pub fn pickup_from_line(line: &str) -> Option<Pickup> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (head, name) = match line.find(" name=") {
        Some(i) => (&line[..i], line[i + 6..].to_string()),
        None => (line, String::new()),
    };
    let mut toks = head.split_whitespace();
    if toks.next()? != "@hisec" {
        return None;
    }
    let kv: BTreeMap<&str, &str> = toks.filter_map(|t| t.split_once('=')).collect();
    let n = |k: &str| kv.get(k).and_then(|v| v.parse::<u32>().ok());
    Some(Pickup {
        uid: n("uid")?,
        case_row: n("case").unwrap_or(0),
        key: n("key")?,
        name,
    })
}

/// A pickup reported by game server `game`: one inbox row of the HiSec case product for the player and 0x3014 to
/// his sessions. Ignored (logged) when the account is not a player of that game, the same case key was already
/// delivered for this game, or no case product resolves.
// DC-DEFAULT: HS-6 (proposed) one inbox row per picked-up case at once (kept if the player leaves the match);
// 0x3014 to every session of the account now (the reference stack sent it while the player was in the match: tdm6 seq
// 10685, 31 ms before 0xd6 sub 1; OBSERVED form, our rule)
// DC-DEFAULT: HS-14 (proposed) a player of the game is an account of its `@join` table: the game server prints
// `@join` when a client connects (DSERVER_PIPE.md §5 step 4), so a late joiner has his row before he can pick
// anything up; room membership alone (a member still in the waiting room) is not enough. A case key counts as
// delivered only once a row was written: a line that resolves no product leaves the key for a later line.
pub fn on_pickup(st: &mut Core, game: u64, p: &Pickup) -> HResult {
    let Some(g) = st.games.get(&game) else {
        return Ok(());
    };
    let in_game = g.clients.values().any(|&u| u == p.uid);
    if !in_game || st.character(p.uid).is_none() {
        st.note(&format!(
            "game {}: @hisec for account {} who is not a player of this game: ignored",
            game, p.uid
        ));
        return Ok(());
    }
    if g.hisec_keys.contains(&p.key) {
        st.note(&format!(
            "game {}: @hisec case key {} already delivered: ignored",
            game, p.key
        ));
        return Ok(());
    }
    let Some(product) = case_product(st, p.case_row) else {
        st.note(&format!(
            "game {}: @hisec for account {}: no HiSec case product (no ROM case row {} and no hisec.case_product)",
            game, p.uid, p.case_row
        ));
        return Ok(());
    };
    let row = inbox::add_row(st, p.uid, product, 0, "", "")?;
    if let Some(g) = st.games.get_mut(&game) {
        g.hisec_keys.insert(p.key);
    }
    inbox::push_stale(st, p.uid);
    st.note(&format!(
        "game {}: HiSec case key {} picked up by account {}: inbox row {} (product {})",
        game, p.key, p.uid, row, product
    ));
    Ok(())
}

// ------------------------------------------------------------------------------------------- HS-7..HS-9 opening

/// The usable key the opening consumes: an owned, unexpired instance of one of `keys` with a count left; the one
/// that expires first (a permanent one last), then the oldest row.
// DC-DEFAULT: HS-7 (proposed) the oldest-expiring key first
fn pick_key(st: &Core, account: u32, keys: &[i64]) -> rusqlite::Result<Option<db::Item>> {
    let now = clock::now_ticks();
    let mut owned: Vec<db::Item> = db::items(&st.db, account)?
        .into_iter()
        .filter(|i| keys.contains(&i.item_no))
        .filter(|i| i.end_time == 0 || i.end_time > now)
        .filter(|i| i.use_type != 3 || i.remain > 0)
        .collect();
    owned.sort_by_key(|i| (i.end_time == 0, i.end_time, i.uid));
    Ok(owned.into_iter().next())
}

/// Takes one use of the key: a counted key loses one count (the row goes at 0), any other key row goes.
fn consume_key(st: &Core, key: &db::Item) -> rusqlite::Result<()> {
    if key.use_type == 3 && key.remain > 1 {
        st.db.execute(
            "UPDATE items SET remain = remain - 1, used = 1 WHERE uid = ?1",
            params![key.uid],
        )?;
    } else {
        db::delete_item(&st.db, key.uid)?;
    }
    Ok(())
}

/// 0x32e for an inbox row whose item is a HiSec case of key class `value` (the caller checked the row).
// SPEC: HISEC_2009.md §4.3 (case layout; +0x1e non-zero selects the HiSec pop-up and the client decrements that
// key), §6.1 (refuse without a key; announce after a successful opening)
// DC-DEFAULT: HS-7 (proposed) one key use and the case row go together (one handler transaction); without a usable
// key 0x32f ok 0 with hisec.no_key_reason and the case row stays
// DC-DEFAULT: HS-8 (proposed) award = uniform pick from the case's ROM pool (CD-76), duration from
// economy.hisec_case_odds
pub fn open(
    st: &mut Core,
    sid: Sid,
    account: u32,
    entry: i64,
    case_item: i64,
    value: i64,
) -> HResult {
    let keys = key_items(st, value);
    let Some(key) = pick_key(st, account, &keys)? else {
        let r = st.rules.hisec.no_key_reason;
        st.send(sid, inbox::open_reply(false, r, &[]));
        st.note(&format!(
            "inbox: account {} has no usable key for HiSec case {} (keys {:?}): refused",
            account, case_item, keys
        ));
        return Ok(());
    };
    if !delivery::room_for(st, account, 1)? {
        st.send(sid, inbox::open_reply(false, reason::INVENTORY_FULL, &[]));
        return Ok(());
    }
    let odds = st.rules.hisec.odds.clone();
    let Some(award) = delivery::open_case_with(st, account, case_item, &odds)? else {
        st.send(sid, inbox::open_reply(false, reason::SHOP_UNAVAILABLE, &[]));
        return Ok(());
    };
    consume_key(st, &key)?;
    st.db
        .execute("DELETE FROM inbox WHERE uproduct = ?1", [entry])?;
    st.send(
        sid,
        inbox::case_reply(&award, key.uid.clamp(0, u32::MAX as i64) as u32),
    );
    announce(st, sid, account, &award);
    st.note(&format!(
        "inbox: account {} opened HiSec case {} with key {} (uid {}) -> item {} ({} days)",
        account, case_item, key.item_no, key.uid, award.item_no, award.days
    ));
    Ok(())
}

/// 0x3013 for a HiSec opening, to the sessions `hisec.announce` selects. GP awards are not announced.
// SPEC: HISEC_2009.md §2 (+6 must be 0; the client also shows it inside a match)
// DC-DEFAULT: HS-9 (proposed) every connected lobby session, the opener included
pub fn announce(st: &mut Core, sid: Sid, account: u32, award: &Delivered) {
    if delivery::type_code(st, award.item_no) == Some(delivery::CLASS_GP_GRANT) {
        return;
    }
    let to: Vec<Sid> = match st.rules.hisec.announce {
        Announce::Off => return,
        Announce::All => crate::notices::everyone(st),
        Announce::Channel => {
            let ch = st.sessions.get(&sid).and_then(|s| s.channel);
            st.sessions
                .values()
                .filter(|s| s.in_lobby && !s.closing && ch.is_some() && s.channel == ch)
                .map(|s| s.sid)
                .collect()
        }
    };
    let name = st.character(account).map(|c| c.nick).unwrap_or_default();
    let f =
        crate::notices::case_announce_frame(&name, award.item_no.clamp(0, u32::MAX as i64) as u32);
    st.send_all(&to, &f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pickup_lines_parse() {
        assert_eq!(
            pickup_from_line("@hisec uid=1001 case=2 key=7 name=Dave the Great\r\n"),
            Some(Pickup {
                uid: 1001,
                case_row: 2,
                key: 7,
                name: "Dave the Great".into()
            })
        );
        assert_eq!(
            pickup_from_line("@hisec key=3 uid=5").map(|p| (p.uid, p.case_row, p.key)),
            Some((5, 0, 3))
        );
        assert!(pickup_from_line("@hisec uid=x key=1").is_none());
        assert!(pickup_from_line("@hisec uid=1").is_none());
        assert!(pickup_from_line("@player uid=1 key=1").is_none());
    }

    #[test]
    fn rules_apply_and_refuse() {
        let mut r = HisecRules::default();
        assert_eq!(r.odds, vec![(7, 50), (30, 35), (0, 15)]);
        let v = |t: &str| JsonValue::parse(t).unwrap();
        assert!(r.apply("hisec.case_product", &v("1234")));
        assert!(r.apply("hisec.announce", &v("\"channel\"")));
        assert!(r.apply("hisec.no_key_reason", &v("77")));
        assert!(r.apply("economy.hisec_case_odds", &v("[[1, 1], [0, 3]]")));
        assert_eq!(
            (r.case_product, r.announce, r.no_key_reason, r.odds.clone()),
            (1234, Announce::Channel, 77, vec![(1, 1), (0, 3)])
        );
        assert!(!r.apply("hisec.announce", &v("\"loud\"")));
        assert!(!r.apply("hisec.case_product", &v("-1")));
        assert!(!r.apply("hisec.no_key_reason", &v("0")));
        assert!(!r.apply("economy.hisec_case_odds", &v("[[1, 0]]")));
        assert!(!r.apply("economy.hisec_case_odds", &v("[1, 2]")));
        assert!(!r.apply("hisec.other", &v("1")));
        // CRATE-2 (amended): the crate share is a whole percent, unset until the operator gives it
        assert_eq!(r.camo_share, None);
        assert!(r.apply("hisec.camo_share", &v("60")));
        assert_eq!(r.camo_share, Some(60));
        assert!(r.apply("hisec.camo_share", &v("0")));
        assert_eq!(r.camo_share, Some(0));
        for bad in ["101", "-1", "12.5", "\"half\""] {
            assert!(!r.apply("hisec.camo_share", &v(bad)), "{bad}");
        }
        assert_eq!(r.camo_share, Some(0));
    }

    #[test]
    fn first_function_value_takes_the_lowest_suffix() {
        let d = dc_bute::ButeDoc::parse(
            r#"[A]
FunctionValue2=9
FunctionValue10=8
FunctionValue1=4
[B]
FunctionValueX=1
[C]
FunctionValue=3
"#,
        );
        assert_eq!(first_function_value(d.get("A").unwrap()), Some(4));
        assert_eq!(first_function_value(d.get("B").unwrap()), None);
        assert_eq!(first_function_value(d.get("C").unwrap()), Some(3));
    }
}
