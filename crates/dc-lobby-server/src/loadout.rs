//! Loadout: equip / unequip (0x324..0x327), backpack slots, function slots (0x368 / 0x36a, framed as 0x324),
//! discard (0x334), attachments (0x336 / 0x338) and the tuning sync (0x33a). Extends the default-slot repair of
//! `inventory.rs` (CD-52) with the backpack capacity, function-slot and colour checks (`repair`).
// SPEC: PROTOCOL_2009.md §4.2

use crate::delivery::own_slot;
use crate::shop::account_of;
use crate::shopmsg::*;
use crate::state::{Core, HResult, Sid};
use crate::{clock, db, inventory, itemuse};
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, Connection, OptionalExtension};

pub const SLOT_BACKPACK: i64 = 9;
pub const BACKPACK_SLOTS: std::ops::RangeInclusive<i64> = 10..=13;
pub const FUNCTION_SLOTS: i64 = 7;

/// Option kinds (ROM `[Option<n>] ItemType`) in the order of 0x33a +0xa / +0xe / +0x12.
// SPEC: PROTOCOL_2009.md §4.2 0x33a (kinds 0x11 / 0x12 / 0x13)
pub const OPTION_KINDS: [i32; 3] = [0x11, 0x12, 0x13];

/// `{+6 u32 ok}` reply padded to `min` bytes.
fn ok_reply(id: u16, ok: bool, min: usize) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32);
    if w.len() < min {
        w.at(min);
    }
    w.into_frame()
}

/// 0x369 / 0x36b (>= 0x12): +6 ok; +0xe unique id.
// SPEC: PROTOCOL_2009.md §4.2 0x369 / 0x36b (EVIDENCE: cshell+0x36710 / cshell+0x36740)
pub fn function_slot_reply(id: u16, ok: bool, uid: u32) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32).at(0xe).u32(uid);
    w.into_frame()
}

/// Extra weapon slots of the equipped backpack (ROM `MultyInvenSlot`).
pub fn capacity(st: &Core, account: u32) -> rusqlite::Result<i64> {
    let bp: Option<i64> = st
        .db
        .query_row(
            "SELECT item_no FROM items WHERE user_no = ?1 AND equip_slot = ?2 ORDER BY uid DESC LIMIT 1",
            params![account as i64, SLOT_BACKPACK],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match (bp, st.rom.items.as_ref()) {
        (Some(no), Some(t)) => t.backpack_slot_count(no as i32).max(0) as i64,
        _ => 0,
    })
}

fn clear_backpack_slots(st: &Core, account: u32) -> rusqlite::Result<()> {
    st.db.execute(
        "UPDATE items SET equip_slot = -1 WHERE user_no = ?1 AND equip_slot BETWEEN ?2 AND ?3",
        params![
            account as i64,
            *BACKPACK_SLOTS.start(),
            *BACKPACK_SLOTS.end()
        ],
    )?;
    Ok(())
}

/// Checks run wherever the default-slot repair runs (login, 0x320 / 0x322, expiry, every loadout change).
// DC-DEFAULT: CD-77 (proposed) items in backpack slots beyond the equipped backpack's capacity are unequipped;
// CD-82 (proposed) function slots of items that are gone are freed; CD-81 (proposed) the name colour returns to 0
// when no colour item is in use (only checked when the ROM item table is known)
pub fn repair(st: &Core, account: u32) -> rusqlite::Result<()> {
    let cap = capacity(st, account)?;
    st.db.execute(
        "UPDATE items SET equip_slot = -1 WHERE user_no = ?1 AND equip_slot BETWEEN ?2 AND ?3 AND equip_slot - ?2 >= ?4",
        params![account as i64, *BACKPACK_SLOTS.start(), *BACKPACK_SLOTS.end(), cap],
    )?;
    st.db.execute(
        "DELETE FROM function_slots WHERE user_no = ?1 AND uid NOT IN (SELECT uid FROM items WHERE user_no = ?1)",
        [account as i64],
    )?;
    if st.rom.items.is_some() {
        let colour: i64 = st
            .db
            .query_row(
                "SELECT nick_color FROM characters WHERE user_no = ?1",
                [account as i64],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if colour != 0 {
            let items = db::items(&st.db, account)?;
            let active = items.iter().any(|i| {
                i.used != 0 && itemuse::subtype(st, i.item_no) == Some(itemuse::SUB_NAME_COLOUR)
            });
            if !active {
                st.db.execute(
                    "UPDATE characters SET nick_color = 0 WHERE user_no = ?1",
                    [account as i64],
                )?;
            }
        }
        // DC-DEFAULT: LG-198 (proposed) the clan colour goes when no member of the clan has a clan colour item
        // (kind 9) in use any more, as CD-81 for the name colour
        let clan: i64 = st
            .db
            .query_row(
                "SELECT c.clan_uid FROM characters c JOIN clan_colours k ON k.clan_uid = c.clan_uid WHERE c.user_no = ?1",
                [account as i64],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if clan != 0 {
            let used: Vec<i64> = {
                let mut q = st.db.prepare(
                    "SELECT i.item_no FROM items i JOIN characters c ON c.user_no = i.user_no WHERE c.clan_uid = ?1 AND i.used != 0",
                )?;
                let r = q.query_map([clan], |r| r.get(0))?;
                r.collect::<rusqlite::Result<_>>()?
            };
            if !used
                .iter()
                .any(|&n| itemuse::subtype(st, n) == Some(itemuse::SUB_CLAN_COLOUR))
            {
                st.db
                    .execute("DELETE FROM clan_colours WHERE clan_uid = ?1", [clan])?;
            }
        }
    }
    Ok(())
}

/// The seven function-slot item numbers of 0x323 +0x8a (-1 = empty; the client keeps n % 9000, so 0 would be a real item number).
// EVIDENCE: cshell+0x229ca0; OBSERVED: capture 20261025_065016_tdm6 (CAPTURE_tdm6_VALIDATION.md L7)
pub fn function_slots(c: &Connection, account: u32) -> rusqlite::Result<[i32; 7]> {
    let mut out = [-1i32; 7];
    let mut q = c.prepare(
        "SELECT f.slot, i.item_no FROM function_slots f JOIN items i ON i.uid = f.uid WHERE f.user_no = ?1 ORDER BY f.slot",
    )?;
    let rows = q.query_map([account as i64], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (slot, no) = row?;
        if (0..FUNCTION_SLOTS).contains(&slot) {
            out[slot as usize] = no as i32;
        }
    }
    Ok(out)
}

/// A clan mark item (kind 2) that was in use has left the inventory (expired or discarded): when no member of the
/// owner's clan keeps another one in use, the clan's emblem and background go back to none. Returns the clan cleared.
// DC-DEFAULT: LG-202 (proposed) the clan mark goes with its item, as LG-198 for the clan colour
// EVIDENCE: cshell+0x22ca10 (kind 2: taking the item out / removing it switches the mark off)
pub fn item_gone(st: &Core, it: &db::Item) -> rusqlite::Result<Option<i64>> {
    if it.used == 0 || itemuse::subtype(st, it.item_no) != Some(itemuse::SUB_CLAN_MARK) {
        return Ok(None);
    }
    let clan: i64 = st
        .db
        .query_row(
            "SELECT clan_uid FROM characters WHERE user_no = ?1",
            [it.user_no as i64],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if clan == 0 {
        return Ok(None);
    }
    let used: Vec<i64> = {
        let mut q = st.db.prepare(
            "SELECT i.item_no FROM items i JOIN characters c ON c.user_no = i.user_no WHERE c.clan_uid = ?1 AND i.used != 0",
        )?;
        let r = q.query_map([clan], |r| r.get(0))?;
        r.collect::<rusqlite::Result<_>>()?
    };
    if used
        .iter()
        .any(|&n| itemuse::subtype(st, n) == Some(itemuse::SUB_CLAN_MARK))
    {
        return Ok(None);
    }
    st.db.execute(
        "UPDATE clans SET emblem = 0, background = 0 WHERE clan_uid = ?1",
        [clan],
    )?;
    Ok(Some(clan))
}

/// The function slot of a function item kind.
// EVIDENCE: cshell+0x2254e0 (SubType 11 -> 0, 13 -> 1, 14 -> 2, 15 -> 3, 18 -> 4, 20 -> 5, 21 -> 6)
pub fn slot_of_subtype(kind: i64) -> Option<i64> {
    match kind {
        11 => Some(0),
        13 => Some(1),
        14 => Some(2),
        15 => Some(3),
        18 => Some(4),
        20 => Some(5),
        21 => Some(6),
        _ => None,
    }
}

/// Puts an item in the function slot of its SubType, else the lowest free one; false when all seven are taken.
// DC-DEFAULT: CD-82 (proposed) up to seven function items sit in function slots
pub fn assign_function_slot(st: &Core, account: u32, uid: i64) -> rusqlite::Result<bool> {
    let have: Option<i64> = st
        .db
        .query_row(
            "SELECT slot FROM function_slots WHERE uid = ?1",
            [uid],
            |r| r.get(0),
        )
        .optional()?;
    if have.is_some() {
        return Ok(true);
    }
    let used: Vec<i64> = {
        let mut q = st
            .db
            .prepare("SELECT slot FROM function_slots WHERE user_no = ?1")?;
        let r = q.query_map([account as i64], |r| r.get(0))?;
        r.collect::<rusqlite::Result<_>>()?
    };
    // DC-DEFAULT: LG-199 (proposed) the slot follows the item's SubType when it is free
    let want = db::item(&st.db, uid)?
        .and_then(|i| itemuse::subtype(st, i.item_no))
        .and_then(slot_of_subtype)
        .filter(|s| !used.contains(s));
    match want.or_else(|| (0..FUNCTION_SLOTS).find(|s| !used.contains(s))) {
        Some(s) => {
            st.db.execute(
                "INSERT INTO function_slots (uid, user_no, slot) VALUES (?1, ?2, ?3)",
                params![uid, account as i64, s],
            )?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn owned(st: &Core, account: u32, uid: u32, item_no: u32) -> rusqlite::Result<Option<db::Item>> {
    Ok(db::item(&st.db, uid as i64)?
        .filter(|i| i.user_no == account && i.item_no == item_no as i64)
        // LG-314: off a featured entry a pack item is not there to equip, discard or use
        .filter(|i| {
            !dc_bute::modpatch::is_pack_item(i.item_no)
                || crate::inventory::pack_items_allowed(st, account)
        }))
}

fn live(it: &db::Item) -> bool {
    it.end_time == 0 || it.end_time > clock::now_ticks()
}

/// Parsed 0x324 / 0x326: unique id, item number, slot.
// SPEC: PROTOCOL_2009.md §4.2 0x324 (EVIDENCE: cshell+0x18b0e0), 0x326 (EVIDENCE: cshell+0x18af30)
pub fn parse_slot_request(p: &[u8]) -> (u32, u32, i64) {
    let mut r = PktReader::new(p);
    let uid = r.at(6).u32();
    let no = r.u32();
    let slot = r.i32() as i64;
    (uid, no, slot)
}

// DC-DEFAULT: CD-77 (proposed) the slot is the item's own slot by type (cshell+0x259f00), or a backpack slot
// 10..13 within the equipped backpack's MultyInvenSlot for items whose own slot is in inventory.backpack_kinds;
// the occupant of the target slot is unequipped; any change of slot 9 empties slots 10..13; CD-52 runs after
pub fn equip(st: &Core, account: u32, uid: u32, item_no: u32, slot: i64) -> rusqlite::Result<bool> {
    let Some(it) = owned(st, account, uid, item_no)?.filter(|i| live(i) && i.parent_uid == 0)
    else {
        return Ok(false);
    };
    let Some(own) = own_slot(st, it.item_no) else {
        return Ok(false);
    };
    let fits = if slot == own {
        true
    } else if BACKPACK_SLOTS.contains(&slot) && st.rules.shop.backpack_kinds.contains(&own) {
        slot - BACKPACK_SLOTS.start() < capacity(st, account)?
    } else {
        false
    };
    if !fits {
        return Ok(false);
    }
    // DC-DEFAULT: LG-403 (proposed, USER DECISION 2026-11-04) a default weapon that holds its own slot (as this
    // entry shows it) does not go into the backpack: that would leave the slot empty; another gun goes there first
    if let Some(own) =
        inventory::default_weapon_slot(&it).filter(|_| BACKPACK_SLOTS.contains(&slot))
    {
        if inventory::seen_slot(st, account, it.uid)? == Some(own) {
            return Ok(false);
        }
    }
    st.db.execute(
        "UPDATE items SET equip_slot = -1 WHERE user_no = ?1 AND equip_slot = ?2 AND uid != ?3",
        params![account as i64, slot, it.uid],
    )?;
    db::set_slot(&st.db, it.uid, slot)?;
    if slot == SLOT_BACKPACK {
        clear_backpack_slots(st, account)?;
    }
    inventory::repair_defaults(st, account)?;
    Ok(true)
}

// DC-DEFAULT: CD-77 (proposed) a default item is never unequipped (0x327 ok = 0); unequipping the backpack
// empties slots 10..13; CD-52 then returns a default weapon (the backpack slot may stay empty)
// DC-DEFAULT: LG-403 (proposed, USER DECISION 2026-11-04) except a default weapon in a backpack slot 10..13 (as this
// entry shows it): it leaves the backpack, back to its own slot when that is empty (CD-52), else to storage
pub fn unequip(st: &Core, account: u32, uid: u32, item_no: u32) -> rusqlite::Result<bool> {
    let Some(it) = owned(st, account, uid, item_no)?.filter(|i| i.slot >= 0) else {
        return Ok(false);
    };
    if it.default_type != 0
        && !(inventory::is_default_weapon(&it)
            && inventory::seen_slot(st, account, it.uid)?
                .is_some_and(|s| BACKPACK_SLOTS.contains(&s)))
    {
        return Ok(false);
    }
    db::set_slot(&st.db, it.uid, -1)?;
    if it.slot == SLOT_BACKPACK {
        clear_backpack_slots(st, account)?;
    }
    inventory::repair_defaults(st, account)?;
    Ok(true)
}

pub fn on_equip(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (uid, no, slot) = parse_slot_request(p);
    let ok = match account_of(st, sid) {
        Some(a) => equip(st, a, uid, no, slot)?,
        None => false,
    };
    st.note(&format!(
        "equip: uid {} item {} slot {} -> {}",
        uid, no, slot, ok
    ));
    // SPEC: PROTOCOL_2009.md §4.2 0x325 (EVIDENCE: cshell+0x34ab0; 0 -> res 0x506)
    st.send(sid, ok_reply(EQUIP_REPLY, ok, 0xa));
    Ok(())
}

pub fn on_unequip(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (uid, no, _slot) = parse_slot_request(p);
    let ok = match account_of(st, sid) {
        Some(a) => unequip(st, a, uid, no)?,
        None => false,
    };
    st.note(&format!("unequip: uid {} item {} -> {}", uid, no, ok));
    // SPEC: PROTOCOL_2009.md §4.2 0x327 (EVIDENCE: cshell+0x34a40; 0 -> res 0x4dd)
    st.send(sid, ok_reply(UNEQUIP_REPLY, ok, 0xa));
    Ok(())
}

/// 0x368 / 0x36a (framed as 0x324): +6 item number; +0xa unique id. The sender cshell+0x18f7b0 writes its second
/// argument at +6 and its first at +0xa; the caller cshell+0x1413c0 passes the unique id first (inventory
/// lookup) and the item's number second, and sends only for items in use whose ROM row allows unequip.
// DC-DEFAULT: CD-82 (proposed) only owned function items that are in use and marked IsCanUnEquip=1
pub fn on_function_slot(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let no = r.at(6).u32();
    let uid = r.u32();
    let reply_id = if id == FUNCTION_EQUIP {
        FUNCTION_EQUIP_REPLY
    } else {
        FUNCTION_UNEQUIP_REPLY
    };
    let ok = match account_of(st, sid) {
        Some(a) => {
            match owned(st, a, uid, no)?.filter(|i| (9000..10000).contains(&i.item_no) && live(i)) {
                Some(it) if id == FUNCTION_EQUIP => {
                    it.used != 0
                        && itemuse::slot_item(st, it.item_no)
                        && assign_function_slot(st, a, it.uid)?
                }
                Some(it) => {
                    st.db
                        .execute("DELETE FROM function_slots WHERE uid = ?1", [it.uid])?
                        > 0
                }
                None => false,
            }
        }
        None => false,
    };
    st.send(sid, function_slot_reply(reply_id, ok, uid));
    Ok(())
}

// DC-DEFAULT: CD-78 (proposed) an equipped or default item cannot be discarded; the attachments of a discarded
// weapon are detached; no GP back (0x335 has no GP field)
pub fn discard(st: &Core, account: u32, uid: u32, item_no: u32) -> rusqlite::Result<bool> {
    let Some(it) = owned(st, account, uid, item_no)?.filter(|i| i.slot < 0 && i.default_type == 0)
    else {
        return Ok(false);
    };
    db::delete_item(&st.db, it.uid)?;
    st.db
        .execute("DELETE FROM function_slots WHERE uid = ?1", [it.uid])?;
    item_gone(st, &it)?;
    Ok(true)
}

pub fn on_discard(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let uid = r.at(6).u32();
    let no = r.u32();
    let ok = match account_of(st, sid) {
        Some(a) => discard(st, a, uid, no)?,
        None => false,
    };
    st.note(&format!("discard: uid {} item {} -> {}", uid, no, ok));
    // SPEC: PROTOCOL_2009.md §4.2 0x335 (EVIDENCE: cshell+0x32ee0)
    st.send(sid, ok_reply(ITEM_DISCARD_REPLY, ok, 0xa));
    Ok(())
}

fn option_kind(st: &Core, item_no: i64) -> i32 {
    st.rom
        .items
        .as_ref()
        .map(|t| t.option_kind(item_no as i32))
        .unwrap_or(-1)
}

/// Attaches an option to a parent item, replacing an option of the same kind on that parent.
// DC-DEFAULT: CD-79 (proposed) option (1000..1999) and parent are owned; one option per kind and parent
fn attach_option(
    st: &Core,
    account: u32,
    opt: &db::Item,
    parent: &db::Item,
) -> rusqlite::Result<()> {
    let kind = option_kind(st, opt.item_no);
    for other in db::items(&st.db, account)?
        .into_iter()
        .filter(|i| i.parent_uid == parent.uid && i.uid != opt.uid)
    {
        if option_kind(st, other.item_no) == kind {
            st.db.execute(
                "UPDATE items SET parent_uid = 0 WHERE uid = ?1",
                [other.uid],
            )?;
        }
    }
    st.db.execute(
        "UPDATE items SET parent_uid = ?2 WHERE uid = ?1",
        params![opt.uid, parent.uid],
    )?;
    Ok(())
}

/// CLI-1606: `parent`'s socket of option `opt_no`'s kind is closed (weapon-table state 0 none or 2 the part is on
/// the model as standard, such as a sniper's built-in suppressor), so the option cannot go on it. A parent without
/// a weapon-table row, or an option of an unknown kind, is not refused.
// EVIDENCE: cshell+0x152d90 (the 2009 client attaches only into a socket whose state, record +0x42c / +0x430 /
// +0x434 for kinds 0x11 / 0x12 / 0x13, is 1), cshell+0x17bd20 (the states' weapon-table keys)
fn socket_closed(st: &Core, opt_no: i64, parent_no: i64) -> bool {
    let Some(k) = OPTION_KINDS
        .iter()
        .position(|x| *x == option_kind(st, opt_no))
    else {
        return false;
    };
    st.rom
        .socket_states
        .get(&crate::inventory::base_weapon(st, parent_no))
        .is_some_and(|s| s[k] != 1)
}

fn parent_of(st: &Core, account: u32, uid: u32) -> rusqlite::Result<Option<db::Item>> {
    Ok(db::item(&st.db, uid as i64)?
        .filter(|i| i.user_no == account && !(1000..2000).contains(&i.item_no))
        .filter(|i| {
            !dc_bute::modpatch::is_pack_item(i.item_no)
                || crate::inventory::pack_items_allowed(st, account)
        }))
}

pub fn on_attach(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let (opt_uid, opt_no, parent_uid) = (r.at(6).u32(), r.u32(), r.u32());
    let ok = match account_of(st, sid) {
        Some(a) => match (
            owned(st, a, opt_uid, opt_no)?.filter(|i| (1000..2000).contains(&i.item_no)),
            parent_of(st, a, parent_uid)?,
        ) {
            (Some(o), Some(par)) if !socket_closed(st, o.item_no, par.item_no) => {
                attach_option(st, a, &o, &par)?;
                true
            }
            _ => false,
        },
        None => false,
    };
    // SPEC: PROTOCOL_2009.md §4.2 0x337 (EVIDENCE: cshell+0x34b10)
    st.send(sid, ok_reply(ATTACH_REPLY, ok, 0xa));
    Ok(())
}

pub fn on_detach(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let (opt_uid, opt_no) = (r.at(6).u32(), r.u32());
    let ok = match account_of(st, sid) {
        Some(a) => match owned(st, a, opt_uid, opt_no)? {
            Some(o) => {
                st.db
                    .execute("UPDATE items SET parent_uid = 0 WHERE uid = ?1", [o.uid])?
                    > 0
            }
            None => false,
        },
        None => false,
    };
    // SPEC: PROTOCOL_2009.md §4.2 0x339 (EVIDENCE: cshell+0x34b70)
    st.send(sid, ok_reply(DETACH_REPLY, ok, 0xa));
    Ok(())
}

/// 0x33a: the weapon's three option kinds at once (-1 / 0 = none). No reply.
// DC-DEFAULT: CD-79 (proposed) tuning sets each kind: the named option is attached, others of that kind detached
pub fn on_tuning(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let weapon = r.at(6).u32();
    let opts = [r.u32(), r.u32(), r.u32()];
    let Some(account) = account_of(st, sid) else {
        return Ok(());
    };
    let Some(par) = parent_of(st, account, weapon)? else {
        return Ok(());
    };
    for (k, &u) in opts.iter().enumerate() {
        let want = OPTION_KINDS[k];
        let chosen = if u == 0 || u == u32::MAX {
            None
        } else {
            db::item(&st.db, u as i64)?.filter(|i| {
                i.user_no == account
                    && (1000..2000).contains(&i.item_no)
                    && {
                        let kind = option_kind(st, i.item_no);
                        kind < 0 || kind == want
                    }
                    && !socket_closed(st, i.item_no, par.item_no)
            })
        };
        match chosen {
            Some(o) => attach_option(st, account, &o, &par)?,
            None => {
                for other in db::items(&st.db, account)?
                    .into_iter()
                    .filter(|i| i.parent_uid == par.uid)
                {
                    if option_kind(st, other.item_no) == want {
                        st.db.execute(
                            "UPDATE items SET parent_uid = 0 WHERE uid = ?1",
                            [other.uid],
                        )?;
                    }
                }
            }
        }
    }
    st.note(&format!(
        "tuning: account {} weapon {} options {:?}",
        account, weapon, opts
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    #[test]
    fn reply_layouts() {
        let (frames, _) =
            dc_wire::parse_tcp_frames(&function_slot_reply(FUNCTION_EQUIP_REPLY, true, 44))
                .unwrap();
        let (id, p) = &frames[0];
        assert_eq!(
            (*id, p.len(), u32_at(p, 6), u32_at(p, 0xe)),
            (0x369, 0x12, 1, 44)
        );
        let (frames, _) = dc_wire::parse_tcp_frames(&ok_reply(EQUIP_REPLY, false, 0xa)).unwrap();
        assert_eq!(
            (frames[0].0, frames[0].1.len(), u32_at(&frames[0].1, 6)),
            (0x325, 0xa, 0)
        );
        let mut w = PktWriter::new(EQUIP);
        w.u32(5).u32(3).u32(11);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x12);
        assert_eq!(parse_slot_request(&p), (5, 3, 11));
    }
}
