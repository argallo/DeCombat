//! Weapon Free Pass (function item kind 16): while a pass is in use its weapons (ROM `FreePassItemID<n>` of the
//! pass's `[UseItem<n>]` row) can be equipped from the client's Free Pass storage page without owning them. The
//! client sends 0x35e {weapon item number, pass item number, slot} to equip one and 0x360 with the same fields to
//! take it off; the replies 0x35f / 0x361 carry +6 ok. The equipped free weapon shows in the 0x323 view with the
//! slot's free-pass flag (+0x4e) set and reaches the game server as a normal equipped slot.
// SPEC: PROTOCOL_2009.md §4.2 (0x35e / 0x360 / 0x35f / 0x361, 0x323 +0x4e)
// EVIDENCE: cshell+0x18ee60 (0x35e / 0x360 sent as their own frame, 0x12 bytes), cshell+0x147e40 / cshell+0x14baa0
// (equip of a record with the free-pass flag +0x5d: +6 the record's item number (+8), +0xa the pass's item number
// (the first word of the pass entry cshell+0x287eb0 finds for the weapon), +0xe the slot: own slot or backpack row + 10),
// cshell+0x145ed0 (0x360 with the same fields for a flagged record), cshell+0x339d0 / 0x14eac0 / 0x14e8d0 (0x35f ok:
// the slot gets the weapon with flag 1 (cshell+0x225170 / 0x225140); 0x361 ok: the slot is cleared; a failed reply
// shows res 0x506 / 0x4dd), cshell+0x229ca0 (0x323 +0x4e: one i32 flag per slot -> slot +4 byte), cshell+0x289ba0
// (a pass is a record of SubType 16; its weapon list is the item row's list at +0x3f0), cshell+0x25dd60 / 0xe5280
// (the row's list is `FreePassItemID0`, `FreePassItemID1`, ... read until a key is missing)

use crate::shop::account_of;
use crate::state::{Core, HResult, Sid};
use crate::{clock, db, delivery, itemuse, loadout};
use dc_wire::{PktReader, PktWriter};
use rusqlite::params;

/// `SubType` of the Weapon Free Pass items.
pub const SUB_FREE_PASS: i64 = 16;

pub const FREE_PASS_EQUIP: u16 = dc_wire::FUNCTION_ITEM_A;
pub const FREE_PASS_EQUIP_REPLY: u16 = dc_wire::FUNCTION_ITEM_A_REPLY;
pub const FREE_PASS_UNEQUIP: u16 = dc_wire::FUNCTION_ITEM_B;
pub const FREE_PASS_UNEQUIP_REPLY: u16 = dc_wire::FUNCTION_ITEM_B_REPLY;

/// One free weapon in an equipment slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeSlot {
    pub slot: i64,
    pub item_no: i64,
    pub pass_uid: i64,
}

/// The weapons pass item `pass_no` grants (`FreePassItemID<n>`; -1 entries skipped; empty without the ROM table).
pub fn granted(st: &Core, pass_no: i64) -> Vec<i64> {
    (0..)
        .map_while(|k| itemuse::use_key(st, pass_no, &format!("FreePassItemID{k}")))
        .filter_map(|v| v.trim_matches('"').trim().parse::<i64>().ok())
        .filter(|n| *n >= 0)
        .collect()
}

fn live(it: &db::Item, now: i64) -> bool {
    it.end_time == 0 || it.end_time > now
}

/// `it` is a pass of the account that grants weapon `item_no` now: a kind 16 item, in use, not expired.
fn grants(st: &Core, account: u32, it: &db::Item, item_no: i64, now: i64) -> bool {
    it.user_no == account
        && it.used != 0
        && live(it, now)
        && itemuse::subtype(st, it.item_no) == Some(SUB_FREE_PASS)
        && granted(st, it.item_no).contains(&item_no)
}

fn rows(st: &Core, account: u32) -> rusqlite::Result<Vec<FreeSlot>> {
    let mut q = st.db.prepare(
        "SELECT slot, item_no, pass_uid FROM free_pass_slots WHERE user_no = ?1 ORDER BY slot",
    )?;
    let r = q.query_map([account as i64], |r| {
        Ok(FreeSlot {
            slot: r.get(0)?,
            item_no: r.get(1)?,
            pass_uid: r.get(2)?,
        })
    })?;
    r.collect()
}

/// The slot `slot` takes weapon `item_no`: its own slot, or a backpack weapon slot within the equipped backpack's
/// capacity for a weapon kind the backpack takes (the rule of 0x324, CD-77).
fn fits(st: &Core, account: u32, item_no: i64, slot: i64) -> rusqlite::Result<bool> {
    let Some(own) = delivery::own_slot(st, item_no) else {
        return Ok(false);
    };
    Ok(if slot == own {
        (0..4).contains(&own) && loadout::own_slot_ok(st, item_no)
    } else {
        loadout::pack_fits(st, account, item_no, own, slot)?
    })
}

/// The account's free weapons that hold now: their pass is still the account's, in use and not expired, and still
/// grants them, and the slot still takes them.
// DC-DEFAULT: LG-407 (proposed) a free weapon lasts as long as its pass: it is checked on every read, so it leaves its
// slot when the pass expires, is discarded or is used up
pub fn held(st: &Core, account: u32) -> rusqlite::Result<Vec<FreeSlot>> {
    let now = clock::now_ticks();
    let mut out = Vec::new();
    for r in rows(st, account)? {
        let ok = match db::item(&st.db, r.pass_uid)? {
            Some(p) => {
                grants(st, account, &p, r.item_no, now) && fits(st, account, r.item_no, r.slot)?
            }
            None => false,
        };
        if ok {
            out.push(r);
        }
    }
    Ok(out)
}

/// Drops the free weapons that no longer hold (run with the loadout repair; only with the ROM item table, which
/// tells the passes).
pub fn prune(st: &Core, account: u32) -> rusqlite::Result<()> {
    if st.rom.items.is_none() {
        return Ok(());
    }
    let keep = held(st, account)?;
    for r in rows(st, account)? {
        if !keep.contains(&r) {
            st.db.execute(
                "DELETE FROM free_pass_slots WHERE user_no = ?1 AND slot = ?2",
                params![account as i64, r.slot],
            )?;
        }
    }
    Ok(())
}

/// An owned item went into `slot` (0x324): a free weapon there leaves it.
pub fn slot_taken(st: &Core, account: u32, slot: i64) -> rusqlite::Result<()> {
    st.db.execute(
        "DELETE FROM free_pass_slots WHERE user_no = ?1 AND slot = ?2",
        params![account as i64, slot],
    )?;
    Ok(())
}

/// The account's items as an equipped view (0x323, the game server's loadout, the room's weapon check): each free
/// weapon that holds sits in its slot as an item of its own (unique id below 0: not a row, no options), the owned item
/// of that slot is shown unequipped (its row keeps the slot, so it is back when the free weapon goes).
// DC-DEFAULT: LG-407 (proposed) the free weapon covers the slot's owned item instead of moving it (2009 server rule
// not known; the 2009 client puts the weapon in its own slot copy with the flag, cshell+0x225170)
pub fn equipped_view(st: &Core, account: u32, items: Vec<db::Item>) -> Vec<db::Item> {
    let free = held(st, account).unwrap_or_default();
    if free.is_empty() {
        return items;
    }
    let mut out = items;
    for f in &free {
        for it in out.iter_mut().filter(|i| i.slot == f.slot) {
            it.slot = -1;
        }
        let pass = db::item(&st.db, f.pass_uid)
            .ok()
            .flatten()
            .unwrap_or_default();
        out.push(db::Item {
            uid: -1 - f.slot,
            user_no: account,
            item_no: f.item_no,
            start_time: pass.start_time,
            end_time: pass.end_time,
            slot: f.slot,
            ..Default::default()
        });
    }
    out
}

/// The 0x323 free-pass flags (+0x4e): 1 for each slot a free weapon holds.
pub fn flags(st: &Core, account: u32) -> [i32; 15] {
    let mut out = [0; 15];
    for f in held(st, account).unwrap_or_default() {
        if let Some(v) = usize::try_from(f.slot).ok().and_then(|s| out.get_mut(s)) {
            *v = 1;
        }
    }
    out
}

/// Parsed 0x35e / 0x360: weapon item number, pass item number, slot.
pub fn parse(p: &[u8]) -> (i64, i64, i64) {
    let mut r = PktReader::new(p);
    let item = r.at(6).u32() as i64;
    let pass = r.u32() as i64;
    let slot = r.i32() as i64;
    (item, pass, slot)
}

/// 0x35e: puts weapon `item_no` of pass `pass_no` (an item number) into `slot`. The pass is the account's first
/// (lowest unique id) item of that number that grants the weapon now.
// DC-DEFAULT: LG-407 (proposed) one slot per free weapon (a second equip moves it); the slot's free weapon before
// is replaced
pub fn equip(
    st: &Core,
    account: u32,
    item_no: i64,
    pass_no: i64,
    slot: i64,
) -> rusqlite::Result<bool> {
    let now = clock::now_ticks();
    let Some(pass) = db::items(&st.db, account)?
        .into_iter()
        .find(|i| i.item_no == pass_no && grants(st, account, i, item_no, now))
    else {
        return Ok(false);
    };
    if !fits(st, account, item_no, slot)? {
        return Ok(false);
    }
    st.db.execute(
        "DELETE FROM free_pass_slots WHERE user_no = ?1 AND (slot = ?2 OR item_no = ?3)",
        params![account as i64, slot, item_no],
    )?;
    st.db.execute(
        "INSERT INTO free_pass_slots (user_no, slot, item_no, pass_uid) VALUES (?1, ?2, ?3, ?4)",
        params![account as i64, slot, item_no, pass.uid],
    )?;
    Ok(true)
}

/// 0x360: takes free weapon `item_no` out of `slot`.
pub fn unequip(st: &Core, account: u32, item_no: i64, slot: i64) -> rusqlite::Result<bool> {
    Ok(st.db.execute(
        "DELETE FROM free_pass_slots WHERE user_no = ?1 AND slot = ?2 AND item_no = ?3",
        params![account as i64, slot, item_no],
    )? > 0)
}

/// 0x35f / 0x361 (0xa): +6 ok.
pub fn reply(id: u16, ok: bool) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32);
    w.into_frame()
}

/// 0x35e / 0x360.
pub fn on_message(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> HResult {
    let (item, pass, slot) = parse(p);
    let put = id == FREE_PASS_EQUIP;
    let ok = match account_of(st, sid) {
        Some(a) if put => equip(st, a, item, pass, slot)?,
        Some(a) => unequip(st, a, item, slot)?,
        None => false,
    };
    st.note(&format!(
        "free pass {}: item {} pass {} slot {} -> {}",
        if put { "equip" } else { "unequip" },
        item,
        pass,
        slot,
        ok
    ));
    let reply_id = if put {
        FREE_PASS_EQUIP_REPLY
    } else {
        FREE_PASS_UNEQUIP_REPLY
    };
    st.send(sid, reply(reply_id, ok));
    Ok(())
}
