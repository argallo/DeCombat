//! Item windows: 0x330 `item_use` and its 0x331 reply, per function-item kind (ROM `ITEMS.TXT` `[UseItem<n>]`
//! `SubType`). The counted-period activation (CD-53) is one of the kinds here.
//!
//! 0x331 layout (PROVEN here; PROTOCOL_2009 Appendix B lists it as unproven): the client routes 0x331 to the
//! open use window (cshell+0x33790 -> cshell+0x1db910, window vfunc +0x40). The window classes and their
//! readers (vtables read from CShell.dll, DISASM where decompiler made no function):
//! - generic window (kinds 10, 12..16, 18..22; vtable cshell+0x3ede4c -> cshell+0x2452f0): +6 ok, else box
//!   by +0xa reason; +0xe unique id; +0x12 unique id of a second item the client deletes (0 = none, weapon
//!   change: the replaced default); +0x16 FILETIME end; +0x1e remaining count; the item is marked in use.
//! - colour window (kinds 8, 9, 11; DISASM: cshell+0x24a3a0): +6, +0xe, +0x12, +0x16 as above; the chosen
//!   colour becomes the item's function word.
//! - clan mark window (kind 2; cshell+0x2ab220): +6, +0xe, +0x12, +0x16 as above.
//! - name window (kinds 1, 4; cshell+0x249d80), record window (kinds 3, 5; cshell+0x244cf0), face window
//!   (kinds 6, 7; DISASM: cshell+0x24a1d0), country window (kind 17; cshell+0x2a94b0): only +6 ok (+0xa reason);
//!   on success the client applies the change itself and deletes the item.
// SPEC: PROTOCOL_2009.md §4.3 0x330 (EVIDENCE: cshell+0x18e1d0)

use crate::delivery;
use crate::shop::account_of;
use crate::shopmsg::*;
use crate::state::{Core, HResult, Sid};
use crate::wire::{self, ItemUse};
use crate::{clock, db, inventory, msg, views};
use dc_wire::PktWriter;
use rusqlite::params;

/// Function-item kinds (`SubType` of the ROM `[UseItem<n>]` sections).
pub const SUB_CLAN_NAME: i64 = 1;
pub const SUB_CLAN_MARK: i64 = 2;
pub const SUB_CLAN_RECORD: i64 = 3;
pub const SUB_NAME: i64 = 4;
pub const SUB_RECORD: i64 = 5;
pub const SUB_FACE: i64 = 6;
pub const SUB_FACE_OTHER: i64 = 7;
pub const SUB_NAME_COLOUR: i64 = 8;
pub const SUB_CLAN_COLOUR: i64 = 9;
pub const SUB_ROOM_BADGE: i64 = 10;
pub const SUB_RETICLE: i64 = 11;
pub const SUB_WEAPON_CHANGE: i64 = 12;
pub const SUB_NATION: i64 = 17;
pub const SUB_MEGAPHONE: i64 = 22;

/// Refusal reasons the client's reason table turns into boxes.
// EVIDENCE: cshell+0x393f0 (0x4a res 0x17f2 no clan, 0x4b res 0x17f3 not clan leader, 0x4c res 0x17f4 mark in
// use, 0x4e res 0x17f5 clan name in use, 0x53 res 0x17f7 name in use, 0x57 res 0x17fc too many characters)
pub mod reason {
    pub const GENERIC: u32 = 0;
    pub const NO_CLAN: u32 = 0x4a;
    pub const NOT_LEADER: u32 = 0x4b;
    pub const MARK_IN_USE: u32 = 0x4c;
    pub const CLAN_NAME_TAKEN: u32 = 0x4e;
    pub const NAME_TAKEN: u32 = 0x53;
    pub const TOO_LONG: u32 = 0x57;
}

/// `SubType` of a function item, from the ROM item table.
pub fn subtype(st: &Core, item_no: i64) -> Option<i64> {
    if !(9000..10000).contains(&item_no) {
        return None;
    }
    st.rom
        .items
        .as_ref()?
        .use_item_info(item_no as i32)?
        .get("SubType")?
        .trim()
        .parse()
        .ok()
}

/// A key of the item's ROM `[UseItem<n>]` section.
pub fn use_key(st: &Core, item_no: i64, key: &str) -> Option<String> {
    st.rom
        .items
        .as_ref()?
        .use_item_info(item_no as i32)?
        .get(key)
        .map(|s| s.trim().to_string())
}

/// True when the ROM marks the function item as one that sits in a function slot (`IsCanUnEquip=1`).
pub fn slot_item(st: &Core, item_no: i64) -> bool {
    use_key(st, item_no, "IsCanUnEquip")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 0x331 (0x22): +6 ok; +0xa reason; +0xe unique id; +0x12 unique id of a second item to delete (0 = none);
/// +0x16 FILETIME end; +0x1e remaining count.
// EVIDENCE: cshell+0x2452f0 (generic window), DISASM: cshell+0x24a3a0 (colour window), cshell+0x2ab220 (mark)
pub fn use_reply(ok: bool, reason: u32, uid: u32, removed: u32, end: u64, count: u32) -> Vec<u8> {
    let mut w = PktWriter::new(msg::ITEM_USE_REPLY);
    w.u32(ok as u32)
        .u32(reason)
        .u32(uid)
        .u32(removed)
        .u64(end)
        .u32(count);
    w.into_frame()
}

/// What a use did.
#[derive(Debug, Default)]
struct Outcome {
    ok: bool,
    reason: u32,
    /// The client deletes the item itself (kinds whose window does so); the server drops the row too.
    consumed: bool,
    removed: i64,
    end: i64,
    count: i64,
    clan_changed: Option<i64>,
}

fn refuse(reason: u32) -> Outcome {
    Outcome {
        ok: false,
        reason,
        ..Default::default()
    }
}

/// Marks an item in use; a counted period item starts its period now (CD-53).
// DC-DEFAULT: CD-53 a counted period item (use kind 1, N days remaining) starts at the use
fn activate(st: &Core, it: &db::Item) -> rusqlite::Result<i64> {
    if it.use_type == 1 && it.remain > 0 && it.used == 0 && st.rules.period_activation {
        let start = clock::now_ticks();
        // DC-DEFAULT: LG-196 (proposed) a 999-day period starts permanent (end 0)
        let end = delivery::new_end(start, it.remain);
        st.db.execute(
            "UPDATE items SET used = 1, remain = 0, start_time = ?2, end_time = ?3 WHERE uid = ?1",
            params![it.uid, start, end],
        )?;
        return Ok(end);
    }
    st.db
        .execute("UPDATE items SET used = 1 WHERE uid = ?1", [it.uid])?;
    Ok(it.end_time)
}

fn kept(st: &Core, it: &db::Item) -> rusqlite::Result<Outcome> {
    let end = activate(st, it)?;
    Ok(Outcome {
        ok: true,
        end,
        count: it.remain.max(0),
        ..Default::default()
    })
}

fn consumed() -> Outcome {
    Outcome {
        ok: true,
        consumed: true,
        ..Default::default()
    }
}

/// The user's clan if the user leads it.
fn led_clan(c: &db::Character) -> Result<i64, Outcome> {
    if c.clan_uid == 0 {
        return Err(refuse(reason::NO_CLAN));
    }
    if c.clan_role != 5 {
        return Err(refuse(reason::NOT_LEADER));
    }
    Ok(c.clan_uid)
}

const NAME_MAX: usize = 16;

// DC-DEFAULT: CD-80 (proposed) item windows per kind; clan items need the clan leader (CD-32); names follow
// CD-16 (unique, case-insensitive) and the 16-unit wire field
fn use_kind(
    st: &mut Core,
    account: u32,
    it: &db::Item,
    kind: Option<i64>,
    req: &ItemUse,
) -> rusqlite::Result<Outcome> {
    let me = st.character(account).unwrap_or_default();
    let colour_ok = |v: u32, max: u32| v >= 1 && v <= if max == 0 { 16 } else { max };
    Ok(match kind {
        Some(SUB_CLAN_NAME) => {
            let clan = match led_clan(&me) {
                Ok(c) => c,
                Err(o) => return Ok(o),
            };
            let name = req.new_name.trim().to_string();
            if name.is_empty() || name.encode_utf16().count() > NAME_MAX {
                return Ok(refuse(reason::TOO_LONG));
            }
            let taken: i64 = st.db.query_row(
                "SELECT COUNT(*) FROM clans WHERE name = ?1 COLLATE NOCASE AND clan_uid != ?2",
                params![name, clan],
                |r| r.get(0),
            )?;
            if taken > 0 {
                return Ok(refuse(reason::CLAN_NAME_TAKEN));
            }
            st.db.execute(
                "UPDATE clans SET name = ?2 WHERE clan_uid = ?1",
                params![clan, name],
            )?;
            st.db.execute(
                "UPDATE characters SET clan_name = ?2 WHERE clan_uid = ?1",
                params![clan, name],
            )?;
            Outcome {
                clan_changed: Some(clan),
                ..consumed()
            }
        }
        Some(SUB_CLAN_MARK) => {
            let clan = match led_clan(&me) {
                Ok(c) => c,
                Err(o) => return Ok(o),
            };
            // DC-DEFAULT: CD-32 one emblem + background pair per clan
            let used: i64 = st.db.query_row(
                "SELECT COUNT(*) FROM clans WHERE emblem = ?1 AND background = ?2 AND clan_uid != ?3",
                params![req.emblem as i64, req.background as i64, clan],
                |r| r.get(0),
            )?;
            if used > 0 {
                return Ok(refuse(reason::MARK_IN_USE));
            }
            st.db.execute(
                "UPDATE clans SET emblem = ?2, background = ?3 WHERE clan_uid = ?1",
                params![clan, req.emblem as i64, req.background as i64],
            )?;
            Outcome {
                clan_changed: Some(clan),
                ..kept(st, it)?
            }
        }
        Some(SUB_CLAN_RECORD) => match led_clan(&me) {
            Ok(clan) => Outcome {
                clan_changed: Some(clan),
                ..consumed()
            },
            Err(o) => o,
        },
        Some(SUB_NAME) => {
            let name = req.new_name.trim().to_string();
            if name.is_empty() || name.encode_utf16().count() > NAME_MAX {
                return Ok(refuse(reason::TOO_LONG));
            }
            let ci = st.rules.names_case_insensitive;
            // DC-DEFAULT: LG-172 a "GM" name prefix is refused for a non-GM account as a taken name
            if crate::gm::name_reserved_for(st, Some(account), &name)
                || db::character_by_nick(&st.db, &name, ci)?
                    .map(|c| c.user_no != account)
                    .unwrap_or(false)
            {
                return Ok(refuse(reason::NAME_TAKEN));
            }
            st.db.execute(
                "UPDATE characters SET nick = ?2, nick_key = ?3 WHERE user_no = ?1",
                params![account as i64, name, db::nick_key(&name, ci)],
            )?;
            consumed()
        }
        Some(SUB_RECORD) => {
            // the client clears exactly these three counters (cshell+0x244cf0: shell +0xfc / +0x100 / +0xd4)
            st.db.execute(
                "UPDATE characters SET kills = 0, deaths = 0, headshots = 0 WHERE user_no = ?1",
                [account as i64],
            )?;
            consumed()
        }
        Some(SUB_FACE) | Some(SUB_FACE_OTHER) => {
            let v = req.face as i32;
            if !(6000..7000).contains(&v) {
                return Ok(refuse(reason::GENERIC));
            }
            st.db.execute(
                "UPDATE characters SET face = ?2 WHERE user_no = ?1",
                params![account as i64, views::face_index(v)],
            )?;
            consumed()
        }
        Some(SUB_NAME_COLOUR) => {
            if !colour_ok(req.colour, st.rom.shop.name_colours) {
                return Ok(refuse(reason::GENERIC));
            }
            st.db.execute(
                "UPDATE characters SET nick_color = ?2 WHERE user_no = ?1",
                params![account as i64, req.colour as i64],
            )?;
            st.db.execute(
                "UPDATE items SET func_value = ?2 WHERE uid = ?1",
                params![it.uid, req.colour as i64],
            )?;
            kept(st, it)?
        }
        Some(SUB_CLAN_COLOUR) => {
            let clan = match led_clan(&me) {
                Ok(c) => c,
                Err(o) => return Ok(o),
            };
            if !colour_ok(req.colour, st.rom.shop.name_colours) {
                return Ok(refuse(reason::GENERIC));
            }
            st.db.execute(
                "INSERT INTO clan_colours (clan_uid, colour) VALUES (?1, ?2) ON CONFLICT(clan_uid) DO UPDATE SET colour = excluded.colour",
                params![clan, req.colour as i64],
            )?;
            st.db.execute(
                "UPDATE items SET func_value = ?2 WHERE uid = ?1",
                params![it.uid, req.colour as i64],
            )?;
            Outcome {
                clan_changed: Some(clan),
                ..kept(st, it)?
            }
        }
        Some(SUB_RETICLE) => {
            if !colour_ok(req.colour, st.rom.shop.reticle_colours) {
                return Ok(refuse(reason::GENERIC));
            }
            st.db.execute(
                "UPDATE items SET func_value = ?2 WHERE uid = ?1",
                params![it.uid, req.colour as i64],
            )?;
            let o = kept(st, it)?;
            crate::loadout::assign_function_slot(st, account, it.uid)?;
            o
        }
        Some(SUB_WEAPON_CHANGE) => return weapon_change(st, account, it, req),
        Some(SUB_NATION) => {
            if req.nation > 0xff {
                return Ok(refuse(reason::GENERIC));
            }
            st.db.execute(
                "UPDATE characters SET nation = ?2 WHERE user_no = ?1",
                params![account as i64, req.nation as i64],
            )?;
            consumed()
        }
        Some(SUB_ROOM_BADGE) => kept(st, it)?,
        Some(SUB_MEGAPHONE) => Outcome {
            ok: true,
            end: it.end_time,
            count: it.remain.max(0),
            ..Default::default()
        },
        Some(_) => {
            let o = kept(st, it)?;
            if slot_item(st, it.item_no) {
                crate::loadout::assign_function_slot(st, account, it.uid)?;
            }
            o
        }
        // no ROM kind: only the counted period activation is known
        None if it.use_type == 1 && it.remain > 0 && it.used == 0 => kept(st, it)?,
        None => refuse(reason::GENERIC),
    })
}

/// use-items-sweep: a colour item's use names the item of the same kind it replaces (0x330 +0x3c): that item, when it
/// is the account's, in use and of the same kind, is deleted and reported in 0x331 +0x12 (the client deletes it); with
/// the same item number the new item's period runs on from the old one's end.
// DC-DEFAULT: CD-100 (proposed) the replaced colour item goes; the same item number extends (the 2009 question
// 0x1865 / 0x183b: "extend the usage of the current color"); 0x330 +0x3c is the colour window's in-use item
// (cshell+0x244f30, cshell+0x242b80)
fn replace_used(
    st: &Core,
    account: u32,
    it: &db::Item,
    kind: i64,
    used_uid: u32,
    o: &mut Outcome,
) -> rusqlite::Result<()> {
    if !o.ok || used_uid == 0 || i64::from(used_uid) == it.uid {
        return Ok(());
    }
    let Some(old) = db::item(&st.db, i64::from(used_uid))?
        .filter(|u| u.user_no == account && u.used != 0 && subtype(st, u.item_no) == Some(kind))
    else {
        return Ok(());
    };
    if old.item_no == it.item_no && old.end_time != 0 && o.end != 0 {
        let now = clock::now_ticks();
        let end = old
            .end_time
            .max(now)
            .saturating_add(o.end.saturating_sub(now).max(0));
        st.db.execute(
            "UPDATE items SET end_time = ?2 WHERE uid = ?1",
            params![it.uid, end],
        )?;
        o.end = end;
    }
    db::delete_item(&st.db, old.uid)?;
    st.db
        .execute("DELETE FROM function_slots WHERE uid = ?1", [old.uid])?;
    o.removed = old.uid;
    Ok(())
}

/// Weapon change: the item becomes the account's default weapon of its slot kind, permanent; the old default of
/// that kind is deleted (its unique id goes in 0x331 +0x12, the client deletes it: cshell+0x2452f0 kind 12).
// DC-DEFAULT: CD-80 (proposed) the target is the ROM ChangeItemID (the request's +0x40 must match when the ROM
// names one); CD-52 repairs the slot afterwards
fn weapon_change(
    st: &mut Core,
    account: u32,
    it: &db::Item,
    req: &ItemUse,
) -> rusqlite::Result<Outcome> {
    let rom_target = use_key(st, it.item_no, "ChangeItemID").and_then(|v| v.parse::<i64>().ok());
    let target = match (rom_target, req.change_item as i64) {
        (Some(t), 0) => t,
        (Some(t), c) if c == t => t,
        (Some(_), _) => return Ok(refuse(reason::GENERIC)),
        (None, c) if c > 0 => c,
        _ => return Ok(refuse(reason::GENERIC)),
    };
    let Some(slot) = delivery::own_slot(st, target).filter(|s| (0..4).contains(s)) else {
        return Ok(refuse(reason::GENERIC));
    };
    let kind = slot + 1;
    let old: Vec<db::Item> = db::items(&st.db, account)?
        .into_iter()
        .filter(|i| i.default_type == kind && i.uid != it.uid)
        .collect();
    let was_slot = old.iter().map(|o| o.slot).find(|&s| s >= 0).unwrap_or(-1);
    let removed = old.first().map(|o| o.uid).unwrap_or(0);
    for o in &old {
        db::delete_item(&st.db, o.uid)?;
    }
    st.db.execute(
        "UPDATE items SET item_no = ?2, default_type = ?3, use_type = 0, remain = 0, used = 0, start_time = ?4, \
         end_time = 0, equip_slot = ?5, func_value = 0 WHERE uid = ?1",
        params![it.uid, target, kind, clock::now_ticks(), was_slot],
    )?;
    inventory::repair_defaults(st, account)?;
    Ok(Outcome {
        ok: true,
        removed,
        end: 0,
        count: 0,
        ..Default::default()
    })
}

/// 0x330.
pub fn on_use(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = wire::parse_item_use(p);
    let Some(account) = account_of(st, sid) else {
        st.send(sid, use_reply(false, reason::GENERIC, req.uid, 0, 0, 0));
        return Ok(());
    };
    let now = clock::now_ticks();
    let it = db::item(&st.db, req.uid as i64)?
        .filter(|i| i.user_no == account && (i.end_time == 0 || i.end_time > now));
    let Some(it) = it else {
        st.send(sid, use_reply(false, reason::GENERIC, req.uid, 0, 0, 0));
        return Ok(());
    };
    let kind = subtype(st, it.item_no);
    let mut o = use_kind(st, account, &it, kind, &req)?;
    if let Some(k @ (SUB_NAME_COLOUR | SUB_CLAN_COLOUR | SUB_RETICLE)) = kind {
        replace_used(st, account, &it, k, req.used_uid, &mut o)?;
    }
    if o.ok && o.consumed {
        db::delete_item(&st.db, it.uid)?;
        st.db
            .execute("DELETE FROM function_slots WHERE uid = ?1", [it.uid])?;
    }
    st.send(
        sid,
        use_reply(
            o.ok,
            o.reason,
            it.uid as u32,
            o.removed as u32,
            o.end.max(0) as u64,
            o.count.max(0) as u32,
        ),
    );
    // DC-DEFAULT: CD-31 the item-use reply goes first; the user who used the item gets no 0x3061, other members do
    if let Some(clan) = o.clan_changed.filter(|_| o.ok) {
        let members: Vec<i64> = {
            let mut q = st
                .db
                .prepare("SELECT user_no FROM characters WHERE clan_uid = ?1 AND user_no != ?2")?;
            let r = q.query_map(params![clan, account as i64], |r| r.get(0))?;
            r.collect::<rusqlite::Result<_>>()?
        };
        let f = PktWriter::new(CLAN_INFO_STALE_PUSH).into_frame();
        for m in members {
            for s in st.sids_of(m as u32) {
                st.send(s, f.clone());
            }
        }
    }
    st.note(&format!(
        "item use: account {} item {} (uid {}, kind {:?}) -> ok {} reason 0x{:x}{}",
        account,
        it.item_no,
        it.uid,
        kind,
        o.ok,
        o.reason,
        if o.consumed && o.ok {
            " (consumed)"
        } else {
            ""
        }
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    #[test]
    fn reply_layout_matches_the_window_readers() {
        let (frames, _) = dc_wire::parse_tcp_frames(&use_reply(true, 0, 5, 9, 1 << 60, 3)).unwrap();
        let (id, p) = &frames[0];
        assert_eq!((*id, p.len()), (0x331, 0x22));
        assert_eq!(
            (
                u32_at(p, 6),
                u32_at(p, 0xa),
                u32_at(p, 0xe),
                u32_at(p, 0x12),
                u32_at(p, 0x1e)
            ),
            (1, 0, 5, 9, 3)
        );
        assert_eq!(
            u64::from_le_bytes(p[0x16..0x1e].try_into().unwrap()),
            1 << 60
        );
        // the request it answers: 0x48 bytes (SPEC: PROTOCOL_2009.md §4.3 0x330)
        let mut w = PktWriter::new(msg::ITEM_USE);
        w.u32(5)
            .wstr("Alicia", 17)
            .u32(1)
            .u32(2)
            .u32(6002)
            .u32(10)
            .u32(0)
            .u32(144)
            .u32(16);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x48);
        let r = wire::parse_item_use(&p);
        assert_eq!(
            (
                r.uid,
                r.new_name.as_str(),
                r.face,
                r.colour,
                r.change_item,
                r.nation
            ),
            (5, "Alicia", 6002, 10, 144, 16)
        );
    }
}
