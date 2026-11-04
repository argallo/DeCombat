//! ROM- and capture-gated replay of the upper-body table (DSV-125 / DSV-126): the real 2009 client's own messages
//! of the reference stack captures (control word and ground state of every `UP_PLAYER_UPDATE`, weapon changes, shots,
//! reloads, throw holds) drive a [`Pose`] with the server's own rules, and the trackers it gives are compared with
//! the trackers the reference stack streamed for the same character on the same link (the real client accepted and
//! played them). Clip indices are compared as numbers, the clips' names are never read here.
//!
//! Skipped without `CA_ROM` or the captures; captures are read in place and never copied (REFERENCE_TESTING.md §5).
// OBSERVED: capture 20261025_000014_tdm1 (the real client alone, its character 408 streamed on
// its own link), 20261025_065016_tdm6 (the real client on link 6, its character 140; REFERENCE)
// DC-DEFAULT: DSV-125 (proposed), DSV-126 (proposed)

use std::collections::BTreeMap;

use dc_botclient::capdecode::{decode_file, unbundle};
use dc_botclient::objects::ObjectTable;
use dc_dserver::anims::{Act, Pose};
use dc_dserver::game::{AnimSet, Motion, THROW_FIRE_WINDOW_MS};
use dc_dserver::up::{parse_up, Up};

/// (capture folder, link of the real client, its character object).
const CASES: [(&str, u32, u16); 2] = [
    ("20261025_000014_tdm1", 5, 408),
    ("20261025_065016_tdm6", 6, 140),
];

/// A capture folder's event file when present (under `DC_CAPTURES`).
fn capture_named(folder: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(folder)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// The character's flag word while it is placed and alive (the reference stack's 0x3803).
const ALIVE_FLAGS: u16 = 0x3803;

#[test]
fn rom_upper_body_table_follows_the_reference_streams() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let chars = dc_dserver::chars::CharTables::from_vfs(&vfs).expect("character tables");
    let weapons = dc_dserver::rom_tables::WeaponTable::from_vfs(&vfs).expect("weapon table");
    // the default body (row 0), the one the reference stack gave both characters
    let body = &chars.bodies[&0].model;
    let rig = dc_dserver::chars::RigCache::default()
        .rig(&vfs, body)
        .expect("rig");
    let anims = AnimSet::from_rig(&rig);
    assert!(!anims.table.is_empty(), "the body's clips classify");
    let mut checked = 0;
    for (folder, link, obj) in CASES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {folder}");
            continue;
        };
        let d = decode_file(&cap).expect("capture");
        let mut msgs: Vec<_> = d.msgs.iter().filter(|m| m.conn == link).collect();
        msgs.sort_by(|a, b| a.t.total_cmp(&b.t));
        let mut t = ObjectTable::default();
        let mut pose = Pose::default();
        let (mut motion, mut motion_since) = (Motion::Stand, 0u32);
        let mut last_throw: Option<u32> = None;
        let mut lmiss: BTreeMap<(u16, u16), u32> = BTreeMap::new();
        let (mut n, mut up_ok, mut low_ok) = (0u32, 0u32, 0u32);
        let mut misses: BTreeMap<(u16, u16), u32> = BTreeMap::new();
        let table = &anims.table;
        for m in msgs {
            let now = (m.t * 1000.0) as u32;
            if m.c2s {
                let mut r = m.bits.reader();
                if r.read_u8() != 11 {
                    continue;
                }
                let sh = r.read_u8();
                let n_bits = r.remaining_bits();
                let body = r.read_bit_string(n_bits);
                if sh == 0x64 {
                    let u = dc_dserver::shell64::parse_shell64(&body);
                    if let Some(c) = u.control() {
                        let mo = Motion::from_control(c, true);
                        if mo != motion {
                            motion = mo;
                            motion_since = now;
                        }
                    }
                    if let Some(tr) = u.trailer {
                        pose.set_ground(tr.on_ground(), now);
                    }
                    continue;
                }
                match parse_up(sh, &body) {
                    Up::WeaponChange { weapon_id, .. } => {
                        let ani = weapons.weapons.get(&weapon_id).and_then(|w| w.ani_type);
                        if pose.ani != ani {
                            pose.stop();
                        }
                        pose.ani = ani;
                        pose.start(Act::Select, table, motion, now);
                    }
                    Up::Fire(_) => {
                        let throw = last_throw.is_some_and(|l| now - l <= THROW_FIRE_WINDOW_MS);
                        let act = if throw { Act::Throw } else { Act::Fire };
                        pose.start(act, table, motion, now);
                    }
                    Up::Reload { .. } => {
                        pose.start(Act::Reload, table, motion, now);
                    }
                    Up::Event {
                        category: 1,
                        event: 1,
                    } => pose.jumped(now),
                    Up::ThrowHold => {
                        last_throw = Some(now);
                        pose.start(Act::ThrowHold, table, motion, now);
                    }
                    _ => {}
                }
                continue;
            }
            for part in unbundle(&m.bits) {
                let mut r = part.reader();
                let id = r.read_u8();
                let n_bits = r.remaining_bits();
                let body = r.read_bit_string(n_bits);
                if id == 8 {
                    t.apply_group(&body);
                    continue;
                }
                if id != 10 {
                    continue;
                }
                let before = t.objects.get(&obj).map_or(0, |o| o.anim_updates);
                t.apply_stream(&body);
                let Some(o) = t.objects.get(&obj) else {
                    continue;
                };
                if o.anim_updates == before || o.flags != ALIVE_FLAGS || o.anim.len() != 3 {
                    continue;
                }
                let ours = anims.trackers_posed(motion, &pose, now, 0, now - motion_since, true);
                n += 1;
                if ours[1].anim == o.anim[1].anim {
                    up_ok += 1;
                } else {
                    *misses.entry((o.anim[1].anim, ours[1].anim)).or_default() += 1;
                }
                if ours[2].anim == o.anim[2].anim {
                    low_ok += 1;
                } else {
                    *lmiss.entry((o.anim[2].anim, ours[2].anim)).or_default() += 1;
                }
            }
        }
        let share = |k: u32| f64::from(k) / f64::from(n.max(1));
        let mut top: Vec<_> = misses.into_iter().collect();
        top.sort_by_key(|x| std::cmp::Reverse(x.1));
        top.truncate(8);
        println!(
            "{folder}: {n} stream entries of character {obj}: upper clip equal {:.1} %, lower {:.1} %; top \
             differences (reference, ours): {top:?}",
            100.0 * share(up_ok),
            100.0 * share(low_ok)
        );
        let mut lt: Vec<_> = lmiss.into_iter().collect();
        lt.sort_by_key(|x| std::cmp::Reverse(x.1));
        lt.truncate(12);
        println!("{folder}: lower differences (reference, ours): {lt:?}");
        assert!(n >= 100, "{folder}: {n} entries");
        // the rest: the tap sees an input before the server's tick, the reference stack's still clips while the control word
        // moves (about 4 %), its rare jump clips (ours play from every jump event, DSV-126) and its still / moving
        // select and reload variants
        assert!(share(up_ok) >= 0.85, "{folder}: upper clip agreement");
        assert!(share(low_ok) >= 0.85, "{folder}: lower clip agreement");
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// DSV-125 / DSV-126 on every player body of the ROM: the clips classify into the families the TDM weapons use
/// (rifle 0, pistol 10, knife 14, throwable 11 by `AniType`), each with its still and four-direction run aim
/// clips, a fire clip and a select clip; the throwable also its hold and release phases; the two hit clips and the
/// nine jump clips resolve. Output: counts only.
#[test]
fn rom_every_body_has_the_upper_body_families() {
    use dc_dserver::anims::{Heading as H, Slot, Stance as S};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let chars = dc_dserver::chars::CharTables::from_vfs(&vfs).expect("character tables");
    let mut cache = dc_dserver::chars::RigCache::default();
    let mut bodies = 0;
    let mut classified = 0;
    for (n, row) in &chars.bodies {
        let Ok(rig) = cache.rig(&vfs, &row.model) else {
            continue;
        };
        let a = AnimSet::from_rig(&rig);
        let t = &a.table;
        if t.is_empty() {
            // a body outside the player scheme (an NPC model row) keeps the plain trackers
            continue;
        }
        bodies += 1;
        classified += t.len();
        for ani in [0u8, 10, 14, 11] {
            let f = t.family(Some(ani)).expect("a family");
            for h in [H::Still, H::Forward, H::Back, H::Left, H::Right] {
                assert!(
                    f.get(Slot::Aim(S::Run, h)).is_some(),
                    "body row {n}: AniType {ani} aim {h:?}"
                );
            }
            for act in [Act::Fire, Act::Select] {
                assert!(
                    t.action_clip(Some(ani), act, Motion::Stand).is_some(),
                    "body row {n}: AniType {ani} {act:?}"
                );
            }
        }
        let throw = t.family(Some(11)).unwrap();
        assert!(throw.get(Slot::ThrowHold(S::Run, H::Still)).is_some());
        assert!(throw.get(Slot::Throw(S::Run, H::Still)).is_some());
        assert!(t.hit.iter().all(|c| c.is_some()), "body row {n}: hit clips");
        assert!(
            t.jump.iter().all(|c| c.is_some()),
            "body row {n}: jump clips"
        );
    }
    assert!(bodies > 0, "no player body");
    println!("{bodies} player bodies, {classified} classified upper-body clips");
}
