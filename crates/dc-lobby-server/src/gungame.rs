//! Gun Game on a server entry with `features = ["gun_game"]` (LG-304, LG-305; project decision): the room
//! offer, the ladder draw at each start and the game server's flag. The match rules are the game server's (DSV-481);
//! the room stays a One Man Army room on every 2009 message, so rewards, statistics and the anti-abuse rules are that
//! mode's. No other entry sends or accepts anything of this.
// DC-DEFAULT: LG-304 (proposed) the entry's feature; LG-305 (proposed) the room setting and the draw

use crate::room::RoomKey;
use crate::rom::RomData;
use crate::state::{Core, Sid};
use crate::wire;
use dc_wire::gungame;

/// The value of `lobby.servers[].features`.
pub const FEATURE: &str = "gun_game";
/// The mode class of the match engine under a Gun Game room (One Man Army's free-for-all).
// SPEC: GAME_MODES_2009.md §5.4 (`FFA` = class 6)
pub const FFA_CLASS: u32 = 6;

/// Whether rooms of channel `ch` may play Gun Game.
pub fn offered(st: &Core, ch: u32) -> bool {
    crate::servers::channel_has_feature(st, ch, FEATURE)
}

/// The weapon rows on the ladder's list: every row a product shown in the ROM shop sells in a listed GP or credit
/// cell, whose `ItemType` is a primary or secondary gun (`dc_bute::arms::is_ladder_gun`), ascending.
pub fn pool(rom: &RomData) -> Vec<u16> {
    let Some(cat) = rom.shop.catalog.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<u16> = cat
        .view_list()
        .into_iter()
        .filter_map(|pid| cat.product(pid))
        .flat_map(|p| p.items.iter())
        .filter_map(|it| cat.info(it.0))
        .filter(|row| row.gp_display != 0 || row.nx_display != 0)
        .filter(|row| {
            rom.weapon_kinds
                .get(&i64::from(row.itemid))
                .is_some_and(|t| dc_bute::arms::is_ladder_gun(*t))
        })
        .filter_map(|row| u16::try_from(row.itemid).ok())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A seed for a match: the clock mixed with the game counter (logged by both servers; not a secret).
fn new_seed(game: u64) -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    t ^ game.rotate_left(32)
}

/// At the start of a Gun Game room: draws the ladder, fits the goal (above the list: the list's length, said in the
/// room) and tells the members just before they get the match address. A list with no gun fails the start.
pub fn prepare_start(st: &mut Core, key: RoomKey) -> Result<(), String> {
    let (on, goal, mode) = match st.rooms.get(&key) {
        Some(r) => (r.gun_game, r.goal, r.mode),
        None => return Ok(()),
    };
    if !on {
        return Ok(());
    }
    let list = pool(&st.rom);
    let want = if goal == 0 {
        st.rom.mode_defaults(mode).1
    } else {
        goal
    };
    let seed = new_seed(st.next_game);
    let ladder = gungame::draw_ladder(&list, seed, want);
    if ladder.is_empty() {
        return Err("gun game: the ROM shop lists no gun".into());
    }
    let effective = ladder.len() as u32;
    st.note(&format!(
        "room {}-{}: gun game seed {} goal {} of {} guns ladder {:?}",
        key.channel,
        key.no,
        seed,
        effective,
        list.len(),
        ladder
    ));
    if let Some(r) = st.rooms.get_mut(&key) {
        r.goal = effective;
        r.gun_seed = seed;
        r.ladder = ladder.clone();
    }
    if effective != goal {
        st.push_room_update(key, crate::msg::field::GOAL, effective, "", true);
        // our text; the ROM has no such string
        let text = format!(
            "{}: the goal is {effective}, the guns in the ladder.",
            gungame::MODE_NAME
        );
        st.push_room(key, &wire::chat_push(0, "", 0, &text));
    }
    st.push_room(key, &gungame::room_frame(key.no, true, &ladder));
    Ok(())
}

/// The game server's flag of a Gun Game room; empty for every other room.
pub fn args(st: &Core, key: RoomKey) -> Vec<String> {
    let Some(r) = st.rooms.get(&key).filter(|r| r.gun_game) else {
        return Vec::new();
    };
    let list: Vec<String> = r.ladder.iter().map(|w| w.to_string()).collect();
    vec![
        "-gungame".to_string(),
        format!("{}:{}", r.gun_seed, list.join(",")),
    ]
}

/// The channel's Gun Game rooms to `sid` (after its 0x381 room list; nothing on an entry without the feature).
pub fn send_channel_rooms(st: &mut Core, sid: Sid, ch: u32) {
    if !offered(st, ch) {
        return;
    }
    let rooms: Vec<u32> = st
        .rooms
        .values()
        .filter(|r| r.key.channel == ch && r.gun_game)
        .map(|r| r.key.no)
        .collect();
    for no in rooms {
        st.send(sid, gungame::room_frame(no, true, &[]));
    }
}

/// The room's setting to a member after its 0x389 (the ladder while it starts or plays: a late joiner's HUD).
pub fn send_room(st: &mut Core, sid: Sid, key: RoomKey) {
    let Some(r) = st.rooms.get(&key) else {
        return;
    };
    if !offered(st, key.channel) || !r.gun_game {
        return;
    }
    let waiting = r.state == crate::room::RoomPhase::Waiting;
    let frame = gungame::room_frame(key.no, true, if waiting { &[] } else { &r.ladder });
    st.send(sid, frame);
}

/// A room's new setting to its channel's lobby and its members.
pub fn push_change(st: &mut Core, key: RoomKey, on: bool) {
    let frame = gungame::room_frame(key.no, on, &[]);
    st.push_lobby(key.channel, &frame);
    st.push_room(key, &frame);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exclusion list against the real shop: guns of the primary and secondary lists only.
    #[test]
    fn rom_the_pool_is_the_shop_guns_only() {
        let Some(dir) = dc_rom::test_rom() else {
            eprintln!("skipped: CA_ROM unset");
            return;
        };
        let rom = RomData::load(&dir);
        let list = pool(&rom);
        assert!(list.len() >= 30, "{} guns", list.len());
        let mut by_name = std::collections::BTreeMap::new();
        let cat = rom.shop.catalog.as_ref().unwrap();
        for pid in cat.view_list() {
            let p = cat.product(pid).unwrap();
            for it in &p.items {
                if let Some(r) = cat.info(it.0) {
                    by_name.insert(r.itemid, p.name.clone());
                }
            }
        }
        let mut types = std::collections::BTreeSet::new();
        for w in &list {
            let t = rom.weapon_kinds[&i64::from(*w)];
            types.insert(t);
            assert!(dc_bute::arms::is_ladder_gun(t), "weapon {w} type {t}");
        }
        // assault rifle, SMG, sniper, machine gun, shotgun, pistol: all present, nothing else
        assert_eq!(
            types.into_iter().collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 0x1a]
        );
        for (id, name) in &by_name {
            let t = rom.weapon_kinds.get(&i64::from(*id));
            // specialist (0x1d), launchers (8), melee (7), thrown (6, 0x1b) and non-weapons are never on the list
            if matches!(t, Some(0x1d | 8 | 7 | 6 | 0x1b) | None) {
                assert!(
                    u16::try_from(*id).map_or(true, |w| !list.contains(&w)),
                    "{name} ({id}) is on the list"
                );
            }
        }
        let a = gungame::draw_ladder(&list, 42, 20);
        assert_eq!(a, gungame::draw_ladder(&list, 42, 20));
        assert_eq!(a.len(), 20);
        let all = gungame::draw_ladder(&list, 42, 100_000);
        assert_eq!(all.len(), list.len(), "capped at the list");
        let mut sorted = all;
        sorted.sort_unstable();
        assert_eq!(sorted, list, "no repeats");
    }
}
