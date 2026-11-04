//! Weapon list for the capture tooling (`exp_sweep.py`, the experiment-A sweep over every weapon). Reads
//! the weapon table, the item table and each weapon's first-person clips from the user's own ROM at run time and
//! prints one tab-separated row per `[Weapon<n>]` row. Nothing of the ROM is stored; the rows go to stdout only.
//!
//! ```text
//! cargo run -p dc-client --no-default-features --example weapons -- [<rom>] [--keys <n>]
//! ```
//! Without `<rom>` the `CA_ROM` path is used. `--keys <n>` prints the key names (no values) of `[Weapon<n>]`.
//!
//! Columns (empty = not known):
//! `id name class class_name item_code slot item_label rank clip pellets ammo single_shot client_kind hud_mode
//! reload_per_bullet fire_ms reload_ms select_ms throwable param_sig`.
//! - `id`: the weapon number the client sends in `UP_FIRE` and the lobby stores as the inventory item number
//!   (weapons are numbered by their weapon row below 1000).
//! - `class`: the block of the `[WeaponOrder]` command table that holds the weapon (`class_name` its `Class<k>`).
//! - `slot`: the equipment slot of the item row's type code (0 primary, 1 secondary, 2 melee, 3 throw); a weapon
//!   row without an item row has no slot and cannot be equipped through the lobby.
//! - `fire_ms`, `reload_ms`, `select_ms`: lengths of the first-person fire / reload / select clips (the fire clip is
//!   the cadence of an automatic weapon, GAME_MESSAGES_2009_C2S.md §4.1).
//! - `param_sig`: a 64-bit FNV-1a digest of the spread / recoil keys of the row and the fire clip length (the key
//!   values themselves are not printed). Two rows with the same digest carry the same spread and recoil numbers;
//!   whether the client then behaves the same is the sweep's question (UNPROVEN).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`weapon_id` of UP_FIRE; cadence = the fire clip)
// EVIDENCE: cshell+0x25c010 (inventory number below 1000 = weapon row); cshell+0x259f00 (item type -> slot)
// EVIDENCE: cshell+0x180590 (WeaponOrder classes, the command table is built class by class)
// OBSERVED: ROM table keys ITEMS.TXT [WeaponItem<n>] ItemID, ItemType, ItemName, RankPoint; WEAPONS.TXT
// [WeaponOrder] Class<k>, <class><i>; [Weapon<n>] Name, HudFireMode, ReloadPerBullet and the spread / recoil keys of
// SIG_KEYS (key names listed with --keys on the user's ROM, 2026-10-25; HudFireMode / ReloadPerBullet meanings
// UNPROVEN, printed raw)

use dc_bute::ButeDoc;
use dc_client::arms::{WeaponClips, WeaponTable, ATTRIBUTES_REZ, WEAPONS_ENTRY};
use dc_rez::RezVfs;
use std::collections::BTreeMap;
use std::path::PathBuf;

// OBSERVED: ROM table key ITEMS.TXT (entry of ATTRIBUTES.rez), sections [WeaponItem<n>]
const ITEMS_ENTRY: &str = "ATTRIBUTES\\ITEMS.TXT";
const WEAPON_ITEM_SECTION: &str = "WeaponItem";
const WEAPON_SECTION: &str = "Weapon";
const WEAPON_ORDER_SECTION: &str = "WeaponOrder";
const KEY_CLASS: &str = "Class";

/// Spread / recoil keys of a weapon row that go into `param_sig` (compared case-insensitively: the ROM spells
/// some of them in two cases).
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] (names only, 2026-10-25 --keys 145 / 164)
const SIG_KEYS: &[&str] = &[
    "minperturb",
    "maxperturb",
    "duckperturb",
    "moveduckperturb",
    "perturbincspeed",
    "perturbdecspeed",
    "maxcamrecoilwag",
    "basecamrecoilwagl",
    "basecamrecoilwagr",
    "maxrandomwag",
    "basecamrecoilpitch",
    "basecamrecoilaccelpitch",
    "maxcamrecoilpitch",
    "maxrandompitch",
    "dynamicperturbfactor",
    "perturbpathfactor",
    "maxcamrecoilyaw",
    "basecamrecoilyaw",
    "basecambackjitter",
    "backjitterrecover",
    "rapidfirecount",
    "movepenalty",
    "firedelay",
    "hudfiremode",
    "shotsperclip",
    "vectorsperround",
    "recoil",
    "zoomedminperturb",
    "zoomedmaxperturb",
];

fn clean(s: &str) -> String {
    s.trim()
        .trim_matches('"')
        .trim()
        .replace(['\t', '\r', '\n'], " ")
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut keys_of: Option<u16> = None;
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--keys" => keys_of = a.next().and_then(|v| v.parse().ok()),
            "-h" | "--help" => {
                println!("usage: weapons [<rom>] [--keys <n>]");
                return;
            }
            o => rom = Some(PathBuf::from(o)),
        }
    }
    let Some(rom) = rom.or_else(dc_rom::test_rom) else {
        eprintln!("weapons: no ROM (pass a path or set CA_ROM)");
        std::process::exit(2);
    };
    let vfs = RezVfs::new(&rom);
    let wdoc = match ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, WEAPONS_ENTRY) {
        Ok(Some(d)) => d,
        other => {
            eprintln!("weapons: weapon table unavailable: {other:?}");
            std::process::exit(1);
        }
    };
    if let Some(n) = keys_of {
        match wdoc.get(&format!("{WEAPON_SECTION}{n}")) {
            Some(sec) => sec.iter().for_each(|(k, _)| println!("{k}")),
            None => {
                eprintln!("weapons: no row {n}");
                std::process::exit(1);
            }
        }
        return;
    }
    let table = WeaponTable::from_doc(&wdoc);
    // weapon names (folded) -> number, then the class blocks of the order section (the client's construction)
    let mut by_name: BTreeMap<String, u16> = BTreeMap::new();
    for (tag, sec) in wdoc.iter() {
        let Some(n) = tag
            .strip_prefix(WEAPON_SECTION)
            .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|r| r.parse::<u16>().ok())
        else {
            continue;
        };
        if let Some(nm) = sec.get("Name") {
            by_name.entry(dc_bute::fold_latin1(&clean(nm))).or_insert(n);
        }
    }
    let mut class_of: BTreeMap<u16, (usize, String)> = BTreeMap::new();
    if let Some(order) = wdoc.get(WEAPON_ORDER_SECTION) {
        for k in 0..64usize {
            let Some(class) = order.get(&format!("{KEY_CLASS}{k}")).map(|v| clean(v)) else {
                break;
            };
            for i in 0..4096u32 {
                let Some(entry) = order.get(&format!("{class}{i}")) else {
                    break;
                };
                if let Some(w) = by_name.get(&dc_bute::fold_latin1(&clean(entry))) {
                    class_of.entry(*w).or_insert((k, class.clone()));
                }
            }
        }
    }
    // item rows by weapon number: (item type code, item name, rank requirement)
    let mut items: BTreeMap<u16, (i32, String, String)> = BTreeMap::new();
    if let Ok(Some(idoc)) = ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, ITEMS_ENTRY) {
        for (tag, sec) in idoc.iter() {
            if !tag.starts_with(WEAPON_ITEM_SECTION) {
                continue;
            }
            let id = sec.get("ItemID").and_then(|v| v.trim().parse::<u16>().ok());
            let ty = sec
                .get("ItemType")
                .and_then(|v| v.trim().parse::<i32>().ok());
            if let (Some(id), Some(ty)) = (id, ty) {
                items.entry(id).or_insert((
                    ty,
                    sec.get("ItemName").map(|v| clean(v)).unwrap_or_default(),
                    sec.get("RankPoint").map(|v| clean(v)).unwrap_or_default(),
                ));
            }
        }
    } else {
        eprintln!("weapons: item table unavailable (no slots)");
    }
    println!(
        "id\tname\tclass\tclass_name\titem_code\tslot\titem_label\trank\tclip\tpellets\tammo\tsingle_shot\t\
         client_kind\thud_mode\treload_per_bullet\tfire_ms\treload_ms\tselect_ms\tthrowable\tparam_sig"
    );
    for (id, w) in &table.rows {
        let sec = wdoc.get(&format!("{WEAPON_SECTION}{id}"));
        let raw = |k: &str| {
            sec.and_then(|s| s.iter().find(|(kk, _)| kk.eq_ignore_ascii_case(k)))
                .map(|(_, v)| clean(v))
                .unwrap_or_default()
        };
        let clips: Option<WeaponClips> = w
            .pv_model
            .as_ref()
            .and_then(|p| WeaponClips::load(&vfs, p).ok());
        let len = |f: fn(&WeaponClips) -> Option<&dc_client::arms::ClipFact>| {
            clips.as_ref().and_then(f).map(|c| c.len_ms)
        };
        let fire_ms = len(|c| c.fire.as_ref());
        let mut sig = 0xcbf2_9ce4_8422_2325u64;
        for k in SIG_KEYS.iter().copied() {
            sig = fnv1a(format!("{k}={};", raw(k)).as_bytes(), sig);
        }
        sig = fnv1a(format!("fire_ms={};", opt(fire_ms)).as_bytes(), sig);
        let item = items.get(id);
        let slot = item
            .and_then(|(ty, _, _)| dc_bute::equip_slot_by_class_code(*ty))
            .filter(|s| (0..4).contains(s));
        let class = class_of.get(id);
        let cols = [
            id.to_string(),
            raw("Name"),
            opt(class.map(|c| c.0)),
            class.map(|c| c.1.clone()).unwrap_or_default(),
            opt(item.map(|i| i.0)),
            opt(slot),
            item.map(|i| i.1.clone()).unwrap_or_default(),
            item.map(|i| i.2.clone()).unwrap_or_default(),
            opt(w.clip),
            opt(w.pellets),
            opt(w.ammo),
            opt(w.single_shot),
            opt(w.client_kind),
            raw("HudFireMode"),
            raw("ReloadPerBullet"),
            opt(fire_ms),
            opt(len(|c| c.reload.as_ref())),
            opt(len(|c| c.select.as_ref())),
            opt(clips.as_ref().map(|c| u8::from(c.throwable()))),
            format!("{sig:016x}"),
        ];
        println!("{}", cols.join("\t"));
    }
}
