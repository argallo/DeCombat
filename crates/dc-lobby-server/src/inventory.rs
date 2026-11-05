//! What login needs from the inventory: the starter kit, the equipment / inventory lists, the default-weapon
//! repair, item expiry (CD-06) and the activation of a counted period item. Shop, inbox and the special item
//! windows are later tasks.

use crate::clock;
use crate::db::{self, Item, NewItem};
use crate::rom::{
    StarterItem, DEFAULT_KIND_BACKPACK, DEFAULT_KIND_HEAD, DEFAULT_KIND_UNIFORM, DEFAULT_KIND_VEST,
    SLOT_BACKPACK, SLOT_THROW,
};
use crate::state::{Core, HResult};
use crate::wire::{self, InventoryRecord};
use rusqlite::params;

/// The starter kit of a new character: the ROM pick (CD-18 / CD-48) or the operator's `inventory.starter` list.
pub fn starter_kit(st: &Core) -> Vec<StarterItem> {
    if st.rules.starter_items.is_empty() {
        let mut kit = st.rom.starter.clone();
        apply_default_weapons(st, &mut kit);
        if !st.rules.starter_backpack {
            kit.retain(|s| s.slot != SLOT_BACKPACK);
        }
        return kit;
    }
    let mut out = Vec::new();
    for (i, &no) in st.rules.starter_items.iter().enumerate() {
        let no = no as i64;
        let slot = st
            .rom
            .slot_of_item(no)
            .unwrap_or((i as i64).min(SLOT_THROW));
        let kind = if slot == SLOT_BACKPACK {
            DEFAULT_KIND_BACKPACK
        } else {
            slot + 1
        };
        out.push(StarterItem {
            item_no: no,
            slot,
            default_kind: kind,
        });
    }
    out
}

/// Puts the configured default primary / secondary item numbers into weapon slots 0 / 1 of the ROM pick.
/// Order of preference for a slot: the configured number, else the ROM balanced pick (lobby-slots rule), else
/// the first marked row (CD-18). The numbers apply when an operator set them or when a ROM is loaded (`CA_ROM`);
/// a number the ROM table lists as a weapon of another slot kind is refused (noted) and the fallback stays.
// DC-DEFAULT: CD-18 refinement (project decision shop.default_primary_item / default_secondary_item
fn apply_default_weapons(st: &Core, kit: &mut Vec<StarterItem>) {
    let sh = &st.rules.shop;
    if !sh.default_items_explicit && st.rom.dir.is_none() {
        return;
    }
    for (slot, no) in [
        (0i64, sh.default_primary_item),
        (1, sh.default_secondary_item),
    ] {
        if no == 0 {
            continue;
        }
        let no = no as i64;
        if let Some(real) = st.rom.slot_of_item(no) {
            if real != slot {
                st.note(&format!(
                    "starter: item {no} belongs to slot {real}, not {slot}; the ROM pick stays"
                ));
                continue;
            }
        } else if !st.rom.weapon_kinds.is_empty() {
            st.note(&format!(
                "starter: item {no} is not a ROM weapon; the ROM pick stays"
            ));
            continue;
        }
        let item = StarterItem {
            item_no: no,
            slot,
            default_kind: slot + 1,
        };
        match kit.iter_mut().find(|k| k.slot == slot) {
            Some(k) => *k = item,
            None => kit.push(item),
        }
    }
    kit.sort_by_key(|k| k.slot);
}

/// Grants the starter kit, permanent and equipped.
pub fn grant_starter(st: &Core, account: u32) -> db::DbResult<()> {
    let now = clock::now_ticks();
    for s in starter_kit(st) {
        db::add_item(
            &st.db,
            &NewItem {
                user_no: account,
                item_no: s.item_no,
                start_time: now,
                end_time: 0,
                slot: s.slot,
                default_type: s.default_kind,
            },
        )?;
    }
    Ok(())
}

/// Equip slot that a default item of `kind` returns to.
fn default_slot(kind: i64) -> Option<i64> {
    match kind {
        1..=4 => Some(kind - 1),
        DEFAULT_KIND_BACKPACK => Some(SLOT_BACKPACK),
        DEFAULT_KIND_UNIFORM => Some(7),
        DEFAULT_KIND_VEST => Some(8),
        DEFAULT_KIND_HEAD => Some(4),
        _ => None,
    }
}

/// An empty weapon or backpack slot gets the account's default item for it back; a slot claimed twice keeps
/// the newest claim.
// DC-DEFAULT: CD-52 (proposed) a weapon / backpack slot never stays empty while a default item for it exists
// DC-DEFAULT: LG-180 an account without a default weapon item of a kind gets the starter kit's
pub fn repair_defaults(st: &Core, account: u32) -> db::DbResult<()> {
    crate::loadout::repair(st, account)?;
    // LG-90 / LG-120: the default vest and head older accounts were given (LG-51) are no defaults any more: removed,
    // which empties the slot each held
    for it in db::items(&st.db, account)?
        .iter()
        .filter(|i| i.default_type == DEFAULT_KIND_VEST || i.default_type == DEFAULT_KIND_HEAD)
    {
        db::delete_item(&st.db, it.uid)?;
    }
    if !st.rules.default_repair {
        return Ok(());
    }
    let mut items = db::items(&st.db, account)?;
    let mut added = false;
    // LG-51: an account without a default body item of a kind gets the ROM's (see `rom::body_defaults`)
    for s in st
        .rom
        .starter
        .iter()
        .filter(|s| s.default_kind >= DEFAULT_KIND_UNIFORM)
    {
        if !items.iter().any(|i| i.default_type == s.default_kind) {
            db::add_item(
                &st.db,
                &NewItem {
                    user_no: account,
                    item_no: s.item_no,
                    start_time: clock::now_ticks(),
                    end_time: 0,
                    slot: -1,
                    default_type: s.default_kind,
                },
            )?;
            added = true;
        }
    }
    // LG-180: an account without a default weapon item of a kind (1 primary .. 4 throwable; 6 backpack when the
    // backpack repair is on) gets the starter kit's, permanent and unequipped here; the slot pass below equips it
    // where the slot is free
    let mut weapon_kinds: Vec<i64> = Vec::new();
    for s in starter_kit(st).iter().filter(|s| {
        (1..=4).contains(&s.default_kind)
            || (s.default_kind == DEFAULT_KIND_BACKPACK && st.rules.default_repair_backpack)
    }) {
        if !items.iter().any(|i| i.default_type == s.default_kind) {
            db::add_item(
                &st.db,
                &NewItem {
                    user_no: account,
                    item_no: s.item_no,
                    start_time: clock::now_ticks(),
                    end_time: 0,
                    slot: -1,
                    default_type: s.default_kind,
                },
            )?;
            weapon_kinds.push(s.default_kind);
            added = true;
        }
    }
    if !weapon_kinds.is_empty() {
        st.note(&format!(
            "starter repair: account {account} had no default item of kind(s) {weapon_kinds:?}; the starter kit's were added"
        ));
    }
    if added {
        items = db::items(&st.db, account)?;
    }
    let mut owner: [Option<i64>; 15] = [None; 15];
    for it in &items {
        if (0..15).contains(&it.slot) {
            let s = it.slot as usize;
            if let Some(prev) = owner[s] {
                db::set_slot(&st.db, prev, -1)?;
            }
            owner[s] = Some(it.uid);
        }
    }
    for it in &items {
        // DC-DEFAULT: CD-52 (user change 2026-10-24) the backpack slot is only refilled with "all"
        if let Some(slot) = default_slot(it.default_type)
            .filter(|&s| s != SLOT_BACKPACK || st.rules.default_repair_backpack)
        {
            let s = slot as usize;
            // DC-DEFAULT: LG-403 (proposed, USER DECISION 2026-11-04) an empty weapon slot takes its default weapon
            // from a backpack slot too, not only from storage
            let in_pack = in_backpack_slot(it) && owner[it.slot as usize] == Some(it.uid);
            if owner[s].is_none() && (it.slot < 0 || in_pack) {
                if in_pack {
                    owner[it.slot as usize] = None;
                }
                db::set_slot(&st.db, it.uid, slot)?;
                owner[s] = Some(it.uid);
            }
        }
    }
    Ok(())
}

/// LG-403: a default weapon (kinds 1..4) sitting in a backpack weapon slot 10..13.
fn in_backpack_slot(it: &Item) -> bool {
    is_default_weapon(it) && crate::loadout::BACKPACK_SLOTS.contains(&it.slot)
}

/// A default weapon item: default kind 1 primary .. 4 throwable (its own slot is kind - 1).
pub fn is_default_weapon(it: &Item) -> bool {
    (1..=4).contains(&it.default_type)
}

/// LG-403: the slot item `uid` sits in as this session's entry shows it ([`visible_items`]); None when it is not shown.
pub fn seen_slot(st: &Core, account: u32, uid: i64) -> db::DbResult<Option<i64>> {
    Ok(visible_items(st, account, db::items(&st.db, account)?)
        .into_iter()
        .find(|i| i.uid == uid)
        .map(|i| i.slot))
}

/// LG-403: the own weapon slot of a default weapon (kind - 1).
pub fn default_weapon_slot(it: &Item) -> Option<i64> {
    is_default_weapon(it).then(|| it.default_type - 1)
}

/// Removes expired items (local FILETIME end <= now) of one account or of everyone, repairs the defaults and,
/// when `push`, sends 0x33f to the owner's lobby sessions. An owner with no lobby session to push to (offline, or
/// the login's own scan before its reply) gets the item numbers noted for the next login (CD-87).
// DC-DEFAULT: CD-06 expiry scan (periodic + at login); CD-42 removal pushes use 0x33f; CD-87 (proposed) items that
// expired while the owner was away are noted and told at the next lobby login
pub fn expire(st: &mut Core, account: Option<u32>, push: bool) -> HResult {
    let now = clock::now_ticks();
    let gone = db::expired_items(&st.db, now, account)?;
    if gone.is_empty() {
        return Ok(());
    }
    let mut by_user: std::collections::BTreeMap<u32, Vec<(u32, u32)>> =
        std::collections::BTreeMap::new();
    for it in &gone {
        db::delete_item(&st.db, it.uid)?;
        crate::loadout::item_gone(st, it)?;
        by_user
            .entry(it.user_no)
            .or_default()
            .push((it.uid as u32, it.item_no.clamp(0, u32::MAX as i64) as u32));
    }
    for (&user, items) in &by_user {
        repair_defaults(st, user)?;
        let uids: Vec<u32> = items.iter().map(|i| i.0).collect();
        st.note(&format!(
            "expiry: account {} lost {} item(s) {:?}",
            user,
            uids.len(),
            uids
        ));
        let sids = if push { st.sids_of(user) } else { Vec::new() };
        if sids.is_empty() {
            let nos: Vec<u32> = items.iter().map(|i| i.1).collect();
            db::add_expiry_notes(&st.db, user, &nos, now)?;
            continue;
        }
        for sid in sids {
            for chunk in uids.chunks(100) {
                st.send(sid, wire::expiry_push(chunk, true));
            }
        }
    }
    Ok(())
}

/// LG-314: true while the account's lobby session is on an entry with a curated set; pack items exist only there.
pub fn pack_items_allowed(st: &Core, account: u32) -> bool {
    st.sessions
        .values()
        .any(|s| s.in_lobby && s.account == Some(account) && st.entry_mods.contains_key(&s.server))
}

/// VAR-1: the gun whose type, slot and attachment sockets item `item_no` has: the base gun of a variant of a loaded curated
/// set, else the item itself.
// DC-DEFAULT: VAR-1 (proposed)
pub fn base_weapon(st: &Core, item_no: i64) -> i64 {
    if !dc_bute::modpatch::is_variant_item(item_no) {
        return item_no;
    }
    st.entry_mods
        .values()
        .flat_map(|e| e.client.client_content.variants())
        .find(|v| i64::from(v.row) == item_no)
        .map_or(item_no, |v| i64::from(v.base))
}

/// LG-314: the account's items as this session's entry shows and equips them. Off an entry with a curated set, a pack
/// item (and what is attached to it) is gone from the list, and an equipped one leaves its slot to the account's default
/// item of that slot. Only the copy changes: the stored row stays equipped, so it is back on the featured entry.
// DC-DEFAULT: LG-314 (proposed, project decision): decided here on the server, never by the client
pub fn visible_items(st: &Core, account: u32, items: Vec<Item>) -> Vec<Item> {
    if !items
        .iter()
        .any(|i| dc_bute::modpatch::is_pack_item(i.item_no))
        || pack_items_allowed(st, account)
    {
        return items;
    }
    let (pack, mut out): (Vec<Item>, Vec<Item>) = items
        .into_iter()
        .partition(|i| dc_bute::modpatch::is_pack_item(i.item_no));
    out.retain(|i| !pack.iter().any(|p| p.uid == i.parent_uid));
    for p in pack.iter().filter(|p| p.slot >= 0) {
        // DC-DEFAULT: LG-403 (proposed, USER DECISION 2026-11-04) the default of the slot comes from storage, else
        // (a default weapon) from a backpack slot, which then shows empty: the primary is never empty in the view
        let from_storage = out
            .iter()
            .position(|i| i.slot < 0 && default_slot(i.default_type) == Some(p.slot));
        let from_pack = || {
            out.iter()
                .position(|i| in_backpack_slot(i) && default_slot(i.default_type) == Some(p.slot))
        };
        if let Some(k) = from_storage.or_else(from_pack) {
            out[k].slot = p.slot;
        }
    }
    out
}

/// 0x323 slot view of the equipped items.
pub fn equipment_slots(items: &[Item]) -> [i32; 15] {
    let mut slots = [-1i32; 15];
    for it in items {
        if (0..15).contains(&it.slot) {
            slots[it.slot as usize] = it.item_no as i32;
        }
    }
    slots
}

pub fn inventory_records(items: &[Item]) -> Vec<InventoryRecord> {
    items
        .iter()
        .map(|it| InventoryRecord {
            uid: it.uid as u32,
            item_no: it.item_no as u32,
            parent_uid: it.parent_uid as u32,
            in_use: (it.slot >= 0 || it.used != 0) as u32,
            start: it.start_time.max(0) as u64,
            end: it.end_time.max(0) as u64,
            use_kind: it.use_type as u32,
            remain: if it.use_type == 1 {
                0
            } else {
                it.remain as u32
            },
            default_kind: it.default_type as u32,
            function_word: it.func_value as u32,
            // EVIDENCE: cshell+0x229130 (+0x24 use count, +0x30 remaining period); lobby-shop: CD-53 items
            remain_period: if it.use_type == 1 {
                it.remain.max(0) as u32
            } else {
                0
            },
        })
        .collect()
}

/// 0x330: the core server activates a counted period item (use kind 1 with N days remaining): it starts now and
/// ends N days later. Every other item kind is refused until the item-window task implements it.
// DC-DEFAULT: CD-53 (proposed) activating a counted period item starts its period at the moment of use
pub fn handle_item_window(st: &mut Core, sid: u64, account: u32, p: &[u8]) -> HResult {
    let req = wire::parse_item_use(p);
    let it = db::item(&st.db, req.uid as i64)?.filter(|i| i.user_no == account);
    let Some(it) = it else {
        st.send(sid, wire::item_use_reply(false, 0, req.uid, 0, 0, 0));
        return Ok(());
    };
    if st.rules.period_activation && it.use_type == 1 && it.remain > 0 && it.used == 0 {
        let start = clock::now_ticks();
        let end = start.saturating_add(it.remain.saturating_mul(clock::TICKS_PER_DAY));
        st.db.execute(
            "UPDATE items SET used = 1, remain = 0, start_time = ?2, end_time = ?3 WHERE uid = ?1",
            params![it.uid, start, end],
        )?;
        st.send(
            sid,
            wire::item_use_reply(true, 0, it.uid as u32, it.item_no as u32, end as u64, 0),
        );
    } else {
        st.note(&format!(
            "item use: kind of item {} (uid {}) not handled by the core server",
            it.item_no, it.uid
        ));
        st.send(
            sid,
            wire::item_use_reply(
                false,
                0,
                it.uid as u32,
                it.item_no as u32,
                it.end_time.max(0) as u64,
                0,
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::*;

    fn account(st: &Core) -> u32 {
        let a = db::create_account(&st.db, "t", 0).unwrap();
        db::create_character(
            &st.db,
            &db::NewCharacter {
                user_no: a,
                nick: "T",
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        a
    }

    #[test]
    fn starter_kit_is_equipped_and_permanent() {
        let mut st = state();
        let a = account(&st);
        st.scope(|st| Ok(grant_starter(st, a)?)).unwrap();
        let items = db::items(&st.db, a).unwrap();
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|i| i.end_time == 0));
        let slots = equipment_slots(&items);
        assert_eq!(&slots[0..4], &[101, 102, 103, 104]);
        assert_eq!(slots[9], 7001);
        assert_eq!(
            items
                .iter()
                .find(|i| i.item_no == 7001)
                .unwrap()
                .default_type,
            6
        );
    }

    fn kit_rows(st: &Core, a: u32) -> usize {
        db::items(&st.db, a).unwrap().len()
    }

    /// LG-180: deleted default weapon rows come back equipped; a full kit is untouched.
    #[test]
    fn missing_default_weapons_are_restored_at_repair() {
        let mut st = state();
        let a = account(&st);
        st.scope(|st| Ok(grant_starter(st, a)?)).unwrap();
        st.scope(|st| Ok(repair_defaults(st, a)?)).unwrap();
        assert_eq!(kit_rows(&st, a), 5, "a full kit is untouched");
        st.db
            .execute(
                "DELETE FROM items WHERE user_no = ?1 AND default_type BETWEEN 1 AND 4",
                [a],
            )
            .unwrap();
        st.scope(|st| Ok(repair_defaults(st, a)?)).unwrap();
        let items = db::items(&st.db, a).unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(&equipment_slots(&items)[0..4], &[101, 102, 103, 104]);
        // only melee + throwable gone
        st.db
            .execute(
                "DELETE FROM items WHERE user_no = ?1 AND default_type IN (3,4)",
                [a],
            )
            .unwrap();
        st.scope(|st| Ok(repair_defaults(st, a)?)).unwrap();
        st.scope(|st| Ok(repair_defaults(st, a)?)).unwrap();
        let items = db::items(&st.db, a).unwrap();
        assert_eq!(items.len(), 5, "no duplicates on a second pass");
        assert_eq!(&equipment_slots(&items)[0..4], &[101, 102, 103, 104]);
    }

    /// LG-180: a bought item in slot 0 keeps the slot; the restored default primary stays unequipped.
    #[test]
    fn restored_default_stays_unequipped_when_the_slot_is_taken() {
        let mut st = state();
        let a = account(&st);
        let uid = db::add_item(
            &st.db,
            &NewItem {
                user_no: a,
                item_no: 27,
                start_time: 1,
                end_time: i64::MAX / 2,
                slot: 0,
                default_type: 0,
            },
        )
        .unwrap();
        st.scope(|st| Ok(repair_defaults(st, a)?)).unwrap();
        let items = db::items(&st.db, a).unwrap();
        assert_eq!(db::item(&st.db, uid).unwrap().unwrap().slot, 0);
        let prim = items.iter().find(|i| i.default_type == 1).unwrap();
        assert_eq!((prim.item_no, prim.slot, prim.end_time), (101, -1, 0));
        let slots = equipment_slots(&items);
        assert_eq!(&slots[1..4], &[102, 103, 104]);
        assert_eq!(slots[9], -1, "backpack only with the backpack repair");
    }

    #[test]
    fn expired_equipped_item_is_removed_and_the_default_returns() {
        let mut st = state();
        let a = account(&st);
        let rx = connect(&mut st, 1, crate::logs::Listener::Lobby);
        st.sessions.get_mut(&1).unwrap().account = Some(a);
        st.sessions.get_mut(&1).unwrap().in_lobby = true;
        st.scope(|st| Ok(grant_starter(st, a)?)).unwrap();
        // a bought weapon in slot 1 that ends in the past
        let uid = db::add_item(
            &st.db,
            &NewItem {
                user_no: a,
                item_no: 10,
                start_time: 1,
                end_time: 1000,
                slot: -1,
                default_type: 0,
            },
        )
        .unwrap();
        st.db
            .execute("UPDATE items SET equip_slot = -1 WHERE item_no = 102", [])
            .unwrap();
        db::set_slot(&st.db, uid, 1).unwrap();
        st.scope(|st| expire(st, Some(a), true)).unwrap();
        assert!(db::item(&st.db, uid).unwrap().is_none());
        let slots = equipment_slots(&db::items(&st.db, a).unwrap());
        assert_eq!(slots[1], 102, "default secondary back in slot 1");
        let got = drain(&rx);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 0x33f);
        assert_eq!(u32_at(&got[0].1, 0xe), 1);
        assert_eq!(u32_at(&got[0].1, 0x12), uid as u32);
    }

    #[test]
    fn counted_period_item_activation() {
        let mut st = state();
        let a = account(&st);
        let rx = connect(&mut st, 1, crate::logs::Listener::Lobby);
        st.db
            .execute(
                "INSERT INTO items(user_no,item_no,use_type,remain) VALUES (?1,9010,1,7)",
                [a as i64],
            )
            .unwrap();
        let uid = st.db.last_insert_rowid();
        let mut w = dc_wire::PktWriter::new(0x330);
        w.u32(uid as u32).wstr("", 17).at(0x48);
        let p = w.into_payload();
        st.scope(|st| handle_item_window(st, 1, a, &p)).unwrap();
        let it = db::item(&st.db, uid).unwrap().unwrap();
        assert_eq!((it.used, it.remain, it.end_time > 0), (1, 0, true));
        let got = drain(&rx);
        assert_eq!(
            (got[0].0, u32_at(&got[0].1, 6), u32_at(&got[0].1, 0xe)),
            (0x331, 1, uid as u32)
        );
        // a second use is refused
        st.scope(|st| handle_item_window(st, 1, a, &p)).unwrap();
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 0);
    }
}
