//! `--grant`: raise (or with `--grant-force` set) one account's exp / GP / NX at start-up and after its
//! first-login character creation.
// SPEC: LOBBY_HARNESS.md §1 (--grant* flags, "only raises, never lowers" without --grant-force)

use crate::cli::Options;
use crate::db;
use crate::rom::{RankTable, RomData};
use rusqlite::{params, Connection};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub target: String,
    pub exp: Option<i64>,
    pub gp: Option<i64>,
    pub nx: Option<i64>,
    /// The credit balance's own value (`--grant-nx-credit`); without it the credit follows `nx`.
    pub credit_nx: Option<i64>,
    pub force: bool,
}

/// Resolves the grant flags; a bad `--grant-rank` is a usage error (Err).
pub fn resolve(o: &Options, ranks: &RankTable) -> Result<Option<Grant>, String> {
    let mut exp = o.grant_exp;
    if !o.grant_rank.trim().is_empty() {
        let r = ranks
            .parse_rank(&o.grant_rank)
            .ok_or_else(|| format!("--grant-rank: unknown rank '{}'", o.grant_rank))?;
        exp = ranks.exp_for(r);
    }
    if o.grant.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(Grant {
        target: o.grant.trim().to_string(),
        exp,
        gp: o.grant_gp,
        nx: o.grant_nx,
        credit_nx: o.grant_nx_credit.or(o.grant_nx),
        force: o.grant_force,
    }))
}

/// Starting experience of a new character: `--start-rank` through the rank table, else `--start-exp`.
// DC-DEFAULT: capture tooling seeds new accounts by rank (capture-seed round); a bad rank is a usage error
pub fn new_character_exp(o: &Options, ranks: &RankTable) -> Result<i64, String> {
    if o.start_rank.trim().is_empty() {
        return Ok(o.new_exp);
    }
    let r = ranks
        .parse_rank(&o.start_rank)
        .ok_or_else(|| format!("--start-rank: unknown rank '{}'", o.start_rank))?;
    ranks
        .exp_for(r)
        .ok_or_else(|| format!("--start-rank: no experience for rank '{}'", o.start_rank))
}

/// Highest equipment slot a seeded item may name (the 0x323 slot view holds slots 0..14).
// SPEC: DSERVER_PIPE.md §12.A (slots 0..3 weapons, 4..9 and 14 gear and backpack, 10..13 backpack weapons)
pub const LOADOUT_SLOT_MAX: i64 = 14;

/// One `--start-loadout` entry as written: an optional slot, an item number and the option items (1000..1999)
/// attached to it (`<slot>:<item>+<option>+...`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SeedEntry {
    pub slot: Option<i64>,
    pub item_no: i64,
    pub options: Vec<i64>,
}

/// Parses `--start-loadout`: a comma (or space) list of `<slot>:<item>` or bare `<item>`, each optionally followed
/// by `+<option>` attachments (option item numbers 1000..1999); empty = no seed.
pub fn parse_start_loadout(text: &str) -> Result<Vec<SeedEntry>, String> {
    let mut out = Vec::new();
    for part in text
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let bad = || {
            format!(
                "--start-loadout: bad entry '{part}' (want <slot>:<item>[+<option>...] or <item>)"
            )
        };
        let mut pieces = part.split('+');
        let head = pieces.next().unwrap_or_default();
        let mut options = Vec::new();
        for o in pieces {
            let n = o.trim().parse::<i64>().map_err(|_| bad())?;
            if !(1000..2000).contains(&n) {
                return Err(format!(
                    "--start-loadout: attachment {n} in '{part}' is not an option item (1000..1999)"
                ));
            }
            options.push(n);
        }
        let (slot, item) = match head.split_once(':') {
            Some((a, b)) => (Some(a.trim().parse::<i64>().map_err(|_| bad())?), b.trim()),
            None => (None, head.trim()),
        };
        let item_no = item.parse::<i64>().map_err(|_| bad())?;
        if item_no < 0 {
            return Err(bad());
        }
        if let Some(sl) = slot {
            if !(0..=LOADOUT_SLOT_MAX).contains(&sl) {
                return Err(format!(
                    "--start-loadout: slot {sl} outside 0..{LOADOUT_SLOT_MAX} in '{part}'"
                ));
            }
        }
        if !options.is_empty() && (1000..2000).contains(&item_no) {
            return Err(format!(
                "--start-loadout: an option cannot carry attachments in '{part}'"
            ));
        }
        out.push(SeedEntry {
            slot,
            item_no,
            options,
        });
    }
    Ok(out)
}

/// The attachments of the seed by resolved slot (the last claim of a slot wins, like `start_loadout`). With the ROM
/// item table loaded, an attachment the table does not list as an option is a usage error.
// DC-DEFAULT: LG-5 (proposed, endflow-fixes3) `--start-loadout` `+<option>` attachments on a seeded item
pub fn start_loadout_options(
    o: &Options,
    rom: &RomData,
) -> Result<std::collections::BTreeMap<i64, Vec<i64>>, String> {
    let slots = start_loadout(o, rom)?;
    let mut by_slot = std::collections::BTreeMap::new();
    for e in parse_start_loadout(&o.start_loadout)? {
        let slot = match e.slot.or_else(|| rom.slot_of_item(e.item_no)) {
            Some(s) => s,
            None => continue,
        };
        if let Some(t) = rom.items.as_ref() {
            if let Some(bad) = e.options.iter().find(|n| t.option_kind(**n as i32) < 0) {
                return Err(format!(
                    "--start-loadout: {bad} is not an option item of the ROM table"
                ));
            }
        }
        by_slot.insert(slot, e.options);
    }
    by_slot.retain(|s, v| !v.is_empty() && slots.iter().any(|(x, _)| x == s));
    Ok(by_slot)
}

/// Resolves the seed to `(slot, item)` pairs, slots ascending, the last claim of a slot wins. A bare item takes
/// its slot from the ROM item table; with the ROM loaded, a weapon the table lists for another weapon slot is
/// refused (a backpack weapon slot 10..13 takes any weapon), so the client never shows a rifle in the pistol slot.
/// Items the table does not list are allowed only with an explicit slot. The seed is written straight into the
/// inventory: no shop, catalogue (`shop.show_hidden_products`) or rank check applies. Errors are usage errors.
// DC-DEFAULT: LG-4 (proposed, weapon-sweep) `--start-loadout` equips chosen items on every new character
pub fn start_loadout(o: &Options, rom: &RomData) -> Result<Vec<(i64, i64)>, String> {
    let mut by_slot = std::collections::BTreeMap::new();
    for e in parse_start_loadout(&o.start_loadout)? {
        let known = rom.slot_of_item(e.item_no);
        let slot = match (e.slot, known) {
            (Some(s), Some(k)) if (0..=3).contains(&k) && s != k && !(10..=13).contains(&s) => {
                return Err(format!(
                    "--start-loadout: item {} belongs to slot {k}, not {s}",
                    e.item_no
                ))
            }
            (Some(s), _) => s,
            (None, Some(k)) => k,
            (None, None) => {
                return Err(format!(
                    "--start-loadout: no slot known for item {} (write <slot>:{})",
                    e.item_no, e.item_no
                ))
            }
        };
        by_slot.insert(slot, e.item_no);
    }
    Ok(by_slot.into_iter().collect())
}

/// `bot` (any ASCII case) followed by one or more digits: the names the launcher gives its bots. The launcher
/// passes `--name Bot1`, dc-botclient logs in with the lowercased login `bot1`, so the prefix ignores case (LG-9).
// DC-DEFAULT: LG-7 (proposed, bot-loadout) `--bot-start-loadout` for new accounts named Bot<digits>
// DC-DEFAULT: LG-9 (proposed, dsv-fixes8) the bot prefix ignores ASCII case (dc-botclient's login is lowercase)
pub fn is_bot_name(login: &str) -> bool {
    bot_number(login).is_some()
}

/// LG-260: the number of a bot name: `bot<digits>` with any prefix (`Bot1`, `VpsBot12`, `PcBot3`; any ASCII
/// case), `None` for other names.
pub fn bot_number(login: &str) -> Option<u32> {
    let head = login.trim_end_matches(|c: char| c.is_ascii_digit());
    let n: u32 = login[head.len()..].parse().ok()?;
    head.to_ascii_lowercase().ends_with("bot").then_some(n)
}

/// LG-260: the seed of a bot's random kit and face: its login name, so the same bot gets the same look after a
/// re-creation and different bots differ.
pub fn bot_seed(login: &str) -> u64 {
    login
        .to_ascii_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
        })
}

/// The options a new account's start loadout is resolved with: a bot name gets `--bot-start-loadout` when it is
/// set, else (LG-260, `--bot-kits random`) a random GP primary and secondary of the ROM shop picked by its name
/// (`rom` supplies the shop for the random kits).
pub fn options_for_account_with(o: &Options, login: &str, rom: Option<&RomData>) -> Options {
    let mut o = o.clone();
    if is_bot_name(login) {
        if !o.bot_start_loadout.trim().is_empty() {
            o.start_loadout = o.bot_start_loadout.clone();
        } else if o.bot_kits_random {
            if let Some(kit) = rom.and_then(|r| r.random_kit(bot_seed(login))) {
                o.start_loadout = kit;
            }
        }
    }
    o
}

/// Puts the `--start-items` in a new player character's inventory: owned for good, not equipped, for trying them out
/// (DC-DEFAULT: CD-99). Bots get none. Returns the items added.
pub fn apply_start_items(
    c: &Connection,
    o: &Options,
    login: &str,
    account: u32,
    now_ticks: i64,
) -> Result<Vec<i64>, String> {
    if is_bot_name(login) {
        return Ok(Vec::new());
    }
    let mut added = Vec::new();
    for part in o
        .start_items
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let item: i64 = part
            .parse()
            .map_err(|_| format!("--start-items: '{part}' is not an item number"))?;
        db::add_item(
            c,
            &db::NewItem {
                user_no: account,
                item_no: item,
                start_time: now_ticks,
                end_time: 0,
                slot: -1,
                default_type: 0,
            },
        )
        .map_err(|e| e.to_string())?;
        added.push(item);
    }
    Ok(added)
}

/// Equips the `--start-loadout` items on a new character: whatever the starter kit put in a seeded slot is
/// unequipped (it stays in the inventory with its default mark, so the slot repair can bring it back later), the
/// seeded item is granted permanent and equipped. Returns the pairs applied.
pub fn apply_start_loadout(
    c: &Connection,
    o: &Options,
    rom: &RomData,
    account: u32,
    now_ticks: i64,
) -> Result<Vec<(i64, i64)>, String> {
    apply_start_loadout_until(c, o, rom, account, now_ticks, 0)
}

/// [`apply_start_loadout`] with an end time for the seeded items (local FILETIME, 0 = permanent); the account CLI
/// uses it to equip an existing character.
pub fn apply_start_loadout_until(
    c: &Connection,
    o: &Options,
    rom: &RomData,
    account: u32,
    now_ticks: i64,
    end_time: i64,
) -> Result<Vec<(i64, i64)>, String> {
    let seed = start_loadout(o, rom)?;
    let options = start_loadout_options(o, rom)?;
    for &(slot, item_no) in &seed {
        c.execute(
            "UPDATE items SET equip_slot = -1 WHERE user_no = ?1 AND equip_slot = ?2",
            params![account as i64, slot],
        )
        .map_err(|e| e.to_string())?;
        let parent = db::add_item(
            c,
            &db::NewItem {
                user_no: account,
                item_no,
                start_time: now_ticks,
                end_time,
                slot,
                default_type: 0,
            },
        )
        .map_err(|e| e.to_string())?;
        // the attachments: unequipped option items whose parent is the seeded item (the 0x336 attach result)
        for &opt in options.get(&slot).into_iter().flatten() {
            let uid = db::add_item(
                c,
                &db::NewItem {
                    user_no: account,
                    item_no: opt,
                    start_time: now_ticks,
                    end_time: 0,
                    slot: -1,
                    default_type: 0,
                },
            )
            .map_err(|e| e.to_string())?;
            c.execute(
                "UPDATE items SET parent_uid = ?2 WHERE uid = ?1",
                params![uid, parent],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(seed)
}

/// True when `account` is the grant target (login, with or without the `LOCAL:` prefix, or nick).
/// `--start-nx-credit`: the credit balance of a character just created (the prepaid balance is in the insert).
// DC-DEFAULT: CLI-1023 (proposed) both NX balances can be seeded at account creation
pub fn set_new_credit(c: &Connection, account: u32, credit: i64) -> db::DbResult<()> {
    if credit > 0 {
        c.execute(
            "UPDATE characters SET nx_cr = ?2 WHERE user_no = ?1",
            params![account as i64, credit],
        )?;
    }
    Ok(())
}

pub fn is_target(c: &Connection, g: &Grant, account: u32) -> bool {
    let want = crate::auth::normalize_login(&g.target).to_lowercase();
    if let Ok(Some(a)) = db::account_by_no(c, account) {
        if a.login.to_lowercase() == want {
            return true;
        }
    }
    matches!(db::character(c, account), Ok(Some(ch)) if ch.nick.to_lowercase() == g.target.to_lowercase())
}

/// Finds the target account (login first, then nick).
pub fn grant_target(c: &Connection, g: &Grant) -> db::DbResult<Option<u32>> {
    if let Some(a) = db::account_by_login(c, &crate::auth::normalize_login(&g.target))? {
        return Ok(Some(a.user_no));
    }
    Ok(db::character_by_nick(c, &g.target, true)?.map(|ch| ch.user_no))
}

fn pick(old: i64, new: Option<i64>, force: bool) -> i64 {
    match new {
        Some(v) if force => v,
        Some(v) => old.max(v),
        None => old,
    }
}

/// Applies the grant to `account`'s character; returns false when it has no character yet.
pub fn apply(c: &Connection, g: &Grant, account: u32) -> db::DbResult<bool> {
    let Some(ch) = db::character(c, account)? else {
        return Ok(false);
    };
    let exp = pick(ch.exp, g.exp, g.force);
    let gp = pick(ch.gp, g.gp, g.force);
    let nx = pick(ch.nx, g.nx, g.force);
    let nx_cr = pick(ch.nx_cr, g.credit_nx, g.force);
    c.execute(
        "UPDATE characters SET exp = ?2, gp = ?3, nx = ?4, nx_cr = ?5 WHERE user_no = ?1",
        params![account as i64, exp, gp, nx, nx_cr],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, u32) {
        let c = db::memory();
        let a = db::create_account(&c, "hank", 0).unwrap();
        db::create_character(
            &c,
            &db::NewCharacter {
                user_no: a,
                nick: "Hank",
                face: 0,
                exp: 0,
                gp: 10000,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        (c, a)
    }

    #[test]
    fn raise_never_lowers_and_force_sets() {
        let (c, a) = setup();
        let g = Grant {
            target: "LOCAL:hank".into(),
            exp: Some(500),
            gp: Some(50),
            nx: Some(7),
            credit_nx: Some(9),
            force: false,
        };
        assert!(is_target(&c, &g, a));
        assert_eq!(grant_target(&c, &g).unwrap(), Some(a));
        apply(&c, &g, a).unwrap();
        let ch = db::character(&c, a).unwrap().unwrap();
        assert_eq!((ch.exp, ch.gp, ch.nx, ch.nx_cr), (500, 10000, 7, 9));
        let g = Grant { force: true, ..g };
        apply(&c, &g, a).unwrap();
        let ch = db::character(&c, a).unwrap().unwrap();
        assert_eq!((ch.exp, ch.gp, ch.nx, ch.nx_cr), (500, 50, 7, 9));
    }

    #[test]
    fn both_nx_balances_are_seeded_and_topped_up() {
        let (c, a) = setup();
        let o = crate::cli::parse_from([
            "x",
            "--start-nx",
            "100000",
            "--start-nx-credit",
            "80000",
            "--grant",
            "hank",
            "--grant-nx",
            "50",
            "--grant-nx-credit",
            "120000",
        ])
        .unwrap();
        assert_eq!((o.new_nx, o.new_nx_credit), (100000, 80000));
        set_new_credit(&c, a, o.new_nx_credit).unwrap();
        let ch = db::character(&c, a).unwrap().unwrap();
        assert_eq!(ch.nx_cr, 80000, "the credit balance of a new character");
        // an existing account is raised (never lowered): here it had 0 -> 50
        let g = resolve(&o, &RankTable::fallback()).unwrap().unwrap();
        apply(&c, &g, a).unwrap();
        let ch = db::character(&c, a).unwrap().unwrap();
        assert_eq!((ch.nx, ch.nx_cr), (50, 120000));
    }

    #[test]
    fn start_rank_replaces_new_character_exp() {
        let t = RankTable::fallback();
        let mut o = crate::cli::parse_from(["x", "--start-exp", "7"]).unwrap();
        assert_eq!(new_character_exp(&o, &t), Ok(7));
        o.start_rank = "3".into();
        assert_eq!(new_character_exp(&o, &t), Ok(t.exp_for(3).unwrap()));
        o.start_rank = "nope".into();
        assert!(new_character_exp(&o, &t).is_err());
    }

    #[test]
    fn grant_rank_resolves_through_the_rank_table() {
        let mut o = crate::cli::parse_from(["x", "--grant", "gina", "--grant-rank", "2"]).unwrap();
        let t = RankTable::fallback();
        let g = resolve(&o, &t).unwrap().unwrap();
        assert_eq!(g.exp, t.exp_for(2));
        o.grant_rank = "not a rank".into();
        assert!(resolve(&o, &t).is_err());
        o.grant_rank.clear();
        o.grant.clear();
        assert_eq!(resolve(&o, &t).unwrap(), None);
    }

    fn rom_with(kinds: &[(i64, i32)]) -> RomData {
        let mut r = RomData::empty();
        for &(n, k) in kinds {
            r.weapon_kinds.insert(n, k);
        }
        r
    }

    fn class_for_slot(slot: usize) -> i32 {
        (0..256)
            .find(|&k| dc_bute::equip_slot_by_class_code(k) == Some(slot as _))
            .expect("a class code for the slot")
    }

    #[test]
    fn start_loadout_parses_slot_pairs_and_bare_items() {
        assert_eq!(parse_start_loadout("").unwrap(), vec![]);
        assert_eq!(
            parse_start_loadout("0:12, 3:7 ,40").unwrap(),
            vec![
                SeedEntry {
                    slot: Some(0),
                    item_no: 12,
                    ..Default::default()
                },
                SeedEntry {
                    slot: Some(3),
                    item_no: 7,
                    ..Default::default()
                },
                SeedEntry {
                    slot: None,
                    item_no: 40,
                    ..Default::default()
                },
            ]
        );
        assert_eq!(
            parse_start_loadout("0:27+1033+1053").unwrap(),
            vec![SeedEntry {
                slot: Some(0),
                item_no: 27,
                options: vec![1033, 1053],
            }]
        );
        for bad in [
            "x",
            "0:",
            ":5",
            "1:-3",
            "15:4",
            "-1:4",
            "0:1:2",
            "0:27+",
            "0:27+33",
            "0:1033+1034",
        ] {
            assert!(parse_start_loadout(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn start_loadout_takes_bare_slots_from_the_rom_and_refuses_the_wrong_slot() {
        // synthetic class codes (no ROM data): the slot comes from dc_bute's class-code map
        let rom = rom_with(&[(20, class_for_slot(0)), (21, class_for_slot(1))]);
        let mut o = crate::cli::parse_from(["x"]).unwrap();
        assert_eq!(start_loadout(&o, &rom), Ok(vec![]));
        o.start_loadout = "21,20".into();
        assert_eq!(start_loadout(&o, &rom), Ok(vec![(0, 20), (1, 21)]));
        o.start_loadout = "1:20".into();
        assert!(start_loadout(&o, &rom).unwrap_err().contains("slot 0"));
        // a backpack weapon slot takes any weapon; the last claim of a slot wins
        o.start_loadout = "10:21,2:99,2:98".into();
        assert_eq!(start_loadout(&o, &rom), Ok(vec![(2, 98), (10, 21)]));
        // an item the table does not list needs an explicit slot (no shop or catalogue check)
        o.start_loadout = "99".into();
        assert!(start_loadout(&o, &rom).is_err());
        o.start_loadout = "2:99".into();
        assert_eq!(start_loadout(&o, &rom), Ok(vec![(2, 99)]));
    }

    #[test]
    fn apply_start_loadout_replaces_the_seeded_slots_only() {
        let (c, a) = setup();
        for (no, slot, kind) in [(101, 0, 1), (102, 1, 2), (103, 2, 3)] {
            db::add_item(
                &c,
                &db::NewItem {
                    user_no: a,
                    item_no: no,
                    start_time: 0,
                    end_time: 0,
                    slot,
                    default_type: kind,
                },
            )
            .unwrap();
        }
        let mut o = crate::cli::parse_from(["x"]).unwrap();
        o.start_loadout = "55".into();
        assert!(apply_start_loadout(&c, &o, &RomData::empty(), a, 9).is_err());
        o.start_loadout = "0:55".into();
        assert_eq!(
            apply_start_loadout(&c, &o, &RomData::empty(), a, 9),
            Ok(vec![(0, 55)])
        );
        let items = db::items(&c, a).unwrap();
        let at = |s: i64| {
            items
                .iter()
                .filter(|i| i.slot == s)
                .map(|i| i.item_no)
                .collect::<Vec<_>>()
        };
        assert_eq!((at(0), at(1), at(2)), (vec![55], vec![102], vec![103]));
        let old = items.iter().find(|i| i.item_no == 101).unwrap();
        assert_eq!(
            (old.slot, old.default_type),
            (-1, 1),
            "the starter stays in the inventory"
        );
        let new = items.iter().find(|i| i.item_no == 55).unwrap();
        assert_eq!(
            (new.default_type, new.end_time),
            (0, 0),
            "seeded = permanent, not a default item"
        );
    }

    #[test]
    fn apply_start_loadout_attaches_the_options_to_the_seeded_weapon() {
        let (c, a) = setup();
        let mut o = crate::cli::parse_from(["x"]).unwrap();
        o.start_loadout = "0:27+1033+1053,2:58".into();
        assert_eq!(
            apply_start_loadout(&c, &o, &RomData::empty(), a, 9),
            Ok(vec![(0, 27), (2, 58)])
        );
        let items = db::items(&c, a).unwrap();
        let wpn = items.iter().find(|i| i.item_no == 27).unwrap();
        let mut opts: Vec<(i64, i64)> = items
            .iter()
            .filter(|i| i.parent_uid == wpn.uid)
            .map(|i| (i.item_no, i.slot))
            .collect();
        opts.sort();
        assert_eq!(opts, vec![(1033, -1), (1053, -1)]);
    }

    /// The loadout handed to the user on 2026-10-27 (sniper primary, G36E in the backpack, pistol, knife, grenade,
    /// suppressors / sight / magazines) resolves against the real ROM tables.
    #[test]
    fn user_sniper_loadout_resolves_with_the_rom() {
        let Some(dir) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let rom = RomData::load(&dir);
        let mut o = crate::cli::parse_from(["x"]).unwrap();
        o.start_loadout = USER_SNIPER_LOADOUT.into();
        assert_eq!(
            start_loadout(&o, &rom),
            Ok(vec![
                (0, 27),
                (1, 164),
                (2, 58),
                (3, 7),
                (9, 7002),
                (10, 25)
            ])
        );
        let opts = start_loadout_options(&o, &rom).unwrap();
        assert_eq!(opts.get(&0), Some(&vec![1033, 1053]));
        assert_eq!(opts.get(&1), Some(&vec![1034, 1040, 1055]));
        assert_eq!(opts.get(&10), Some(&vec![1031, 1051]));
        let t = rom.items.as_ref().expect("ROM item table");
        assert_eq!(
            [1033, 1040, 1053].map(|n| t.option_kind(n)),
            [17, 18, 19],
            "muzzle / sight / magazine kinds"
        );
    }

    const USER_SNIPER_LOADOUT: &str =
        "0:27+1033+1053,1:164+1034+1040+1055,2:58,3:7,9:7002,10:25+1031+1051";

    // LG-260: any prefix before bot<digits> is a bot name; the seed follows the name, case-folded
    #[test]
    fn bot_names_take_any_prefix_and_seed_by_name() {
        assert_eq!(bot_number("Bot1"), Some(1));
        assert_eq!(bot_number("vpsbot12"), Some(12));
        assert_eq!(bot_number("PcBot3"), Some(3));
        assert!(
            is_bot_name("bot7")
                && !is_bot_name("bot")
                && !is_bot_name("robot")
                && !is_bot_name("Erika1")
        );
        assert_eq!(bot_seed("VpsBot1"), bot_seed("vpsbot1"));
        assert_ne!(bot_seed("VpsBot1"), bot_seed("VpsBot2"));
    }
}
