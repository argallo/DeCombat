//! The room's weapon limit (the waiting room's "Arms" setting, 0x38c / 0x3035 type 0xe): the check a member's
//! equipped weapons must pass before Ready / Start, and the weapons a match grants under the limit.
//!
//! Limit values are the rows of the waiting room's weapon list, in order: 0 all permitted, 1 pistols only, 2 melee
//! only, 3 no sniper rifles, 4 no explosives, 5 no backpack, 6 sniper only, 7 shotguns only, 8 no shotguns.
//! Weapon types are the item table's `ItemType` of a `WeaponItem` row.
// SPEC: PROTOCOL_2009.md §3.7 (0x3035 / 0x38c type 0xe weapon limit)
// EVIDENCE: cshell+0x160080 (the weapon list's strings in their order), cshell+0x160f70 (the check before Ready /
// Start / joining a running match: on failure a box with the limit's two strings, nothing sent)

/// Weapon list row "Pistols Only".
pub const PISTOLS_ONLY: u32 = 1;
/// "Melee Only".
pub const MELEE_ONLY: u32 = 2;
/// "No Sniper Rifles".
pub const NO_SNIPERS: u32 = 3;
/// "No Explosives".
pub const NO_EXPLOSIVES: u32 = 4;
/// "No Backpack".
pub const NO_BACKPACK: u32 = 5;
/// "Sniper Only".
pub const SNIPER_ONLY: u32 = 6;
/// "Shotguns Only".
pub const SHOTGUNS_ONLY: u32 = 7;
/// "No Shotguns".
pub const NO_SHOTGUNS: u32 = 8;

/// Weapon `ItemType` codes the limits name.
// EVIDENCE: cshell+0x226dd0 (the codes each limit compares), cshell+0x259f00 (type -> weapon slot class)
pub const TYPE_SNIPER: i32 = 2;
pub const TYPE_PISTOL: i32 = 4;
pub const TYPE_MELEE: i32 = 7;
pub const TYPE_LAUNCHER: i32 = 8;
pub const TYPE_SHOTGUN: i32 = 0x1a;
/// The second type the "Shotguns Only" check accepts.
pub const TYPE_SHOTGUN_ALT: i32 = 5;

/// The weapon slot class of a weapon type: 0 primary, 1 secondary, 2 melee, 3 thrown; `None` for the other codes.
// EVIDENCE: cshell+0x259f00 (types 0..3, 8..10, 0x1a, 0x1d -> 0; 4, 5 -> 1; 7 -> 2; 6, 0x1b -> 3)
pub fn slot_class(t: i32) -> Option<u8> {
    match t {
        0..=3 | 8..=10 | 0x1a | 0x1d => Some(0),
        4 | 5 => Some(1),
        7 => Some(2),
        6 | 0x1b => Some(3),
        _ => None,
    }
}

/// Whether a weapon of `ItemType` `t` may stand on a Gun Game ladder: the shop's primary guns (assault rifle 0, SMG 1,
/// sniper rifle 2, machine gun 3, shotgun 0x1a) and secondaries (pistol 4, the second shotgun code 5). Left out: the
/// launchers (8) and the other primary codes 9, 10, the specialist slot (0x1d), melee and thrown weapons.
// DC-DEFAULT: LG-305 (proposed, project decision "no specialist weapons")
pub fn is_ladder_gun(t: i32) -> bool {
    matches!(t, 0..=3 | TYPE_SHOTGUN | TYPE_PISTOL | TYPE_SHOTGUN_ALT)
}

fn primary(t: i32) -> bool {
    slot_class(t) == Some(0)
}

/// The text ids of the box a failed check shows (what is wrong, what to do), `None` for a limit without one.
// EVIDENCE: cshell+0x160f70 (limit 1 -> 0x1778 / 0x1779, 3 -> 0x177a / 0x177b, 4 -> 0x177c / 0x177d, 6 -> 0x1795 /
// 0x1796, 7 -> 0x18ab / 0x18ac, 8 -> 0x18b3 / 0x18b4; the others show nothing)
pub fn refusal_texts(limit: u32) -> Option<(i32, i32)> {
    Some(match limit {
        PISTOLS_ONLY => (0x1778, 0x1779),
        NO_SNIPERS => (0x177a, 0x177b),
        NO_EXPLOSIVES => (0x177c, 0x177d),
        SNIPER_ONLY => (0x1795, 0x1796),
        SHOTGUNS_ONLY => (0x18ab, 0x18ac),
        NO_SHOTGUNS => (0x18b3, 0x18b4),
        _ => return None,
    })
}

/// The 2009 entry check: true when the equipped weapons (`types`, every equipped weapon's `ItemType`) pass the
/// room's weapon limit. A limit is met by ONE weapon that fits it: "Sniper Only" asks for an equipped sniper rifle,
/// "Pistols Only" for a pistol, "No Sniper Rifles" for a primary weapon that is not a sniper rifle, and so on.
// EVIDENCE: cshell+0x226dd0 (0 -> pass; per equipped weapon of both weapon lists: 1 type 4, 2 type 7, 3 a primary
// not type 2, 4 a primary not type 8, 6 a primary of type 2, 7 type 5 or 0x1a, 8 a primary not type 0x1a -> pass;
// any other limit passes; no weapon fits -> fail)
// DC-DEFAULT: LG-66 (proposed) "No Backpack" and unknown limits pass even with no weapon equipped
pub fn entry_ok(limit: u32, types: impl IntoIterator<Item = i32>) -> bool {
    let fits = |t: i32| match limit {
        PISTOLS_ONLY => t == TYPE_PISTOL,
        MELEE_ONLY => t == TYPE_MELEE,
        NO_SNIPERS => primary(t) && t != TYPE_SNIPER,
        NO_EXPLOSIVES => primary(t) && t != TYPE_LAUNCHER,
        SNIPER_ONLY => primary(t) && t == TYPE_SNIPER,
        SHOTGUNS_ONLY => t == TYPE_SHOTGUN_ALT || t == TYPE_SHOTGUN,
        NO_SHOTGUNS => primary(t) && t != TYPE_SHOTGUN,
        _ => true,
    };
    if !matches!(limit, PISTOLS_ONLY..=NO_EXPLOSIVES | SNIPER_ONLY..=NO_SHOTGUNS) {
        return true;
    }
    types.into_iter().any(fits)
}

/// Whether a match under `limit` grants an equipped weapon of type `t` (`backpack`: it sits in a backpack slot).
/// The melee weapon is always granted, so a player never spawns unarmed; the "only" limits drop the primary,
/// secondary and thrown weapons of other kinds ("Sniper Only" keeps the sniper rifles and the melee weapon alone),
/// the "no" limits drop the kind they name.
// DC-DEFAULT: DSV-180 (proposed) the 2009 server's grant rule under a weapon limit is not in the allowed sources; this
// is DeCombat's: 1 pistols + melee, 2 melee, 3 no type 2, 4 no type 8 and no thrown weapon (the box names rockets and
// grenades), 5 no backpack slot, 7 shotguns (types 5, 0x1a) + melee, 8 no type 0x1a
// DC-DEFAULT: DSV-476 (proposed, USER REPORT 2026-10-31 "pistols are available to use in a sniper-only match";
// replaces the DSV-180 row of limit 6) 6 sniper rifles (type 2) + melee: no secondary, no thrown weapon
pub fn match_grants(limit: u32, t: i32, backpack: bool) -> bool {
    let class = slot_class(t);
    if class == Some(2) {
        return true;
    }
    match limit {
        PISTOLS_ONLY => t == TYPE_PISTOL || class == Some(1),
        MELEE_ONLY => false,
        NO_SNIPERS => t != TYPE_SNIPER,
        NO_EXPLOSIVES => t != TYPE_LAUNCHER && class != Some(3),
        NO_BACKPACK => !backpack,
        SNIPER_ONLY => t == TYPE_SNIPER,
        SHOTGUNS_ONLY => t == TYPE_SHOTGUN || t == TYPE_SHOTGUN_ALT,
        NO_SHOTGUNS => t != TYPE_SHOTGUN,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniper_only_needs_an_equipped_sniper_rifle() {
        // rifle (0), pistol (4), knife (7), grenade (6)
        assert!(!entry_ok(SNIPER_ONLY, [0, 4, 7, 6]));
        assert!(entry_ok(SNIPER_ONLY, [2, 4, 7]));
        assert!(entry_ok(0, [0]));
        assert!(!entry_ok(SNIPER_ONLY, []));
    }

    #[test]
    fn each_limit_follows_the_2009_check() {
        assert!(entry_ok(PISTOLS_ONLY, [0, 4]) && !entry_ok(PISTOLS_ONLY, [0, 7]));
        assert!(entry_ok(MELEE_ONLY, [7]) && !entry_ok(MELEE_ONLY, [0]));
        assert!(entry_ok(NO_SNIPERS, [2, 0]) && !entry_ok(NO_SNIPERS, [2, 4, 7]));
        assert!(entry_ok(NO_EXPLOSIVES, [0]) && !entry_ok(NO_EXPLOSIVES, [8, 4]));
        assert!(entry_ok(SHOTGUNS_ONLY, [0x1a]) && !entry_ok(SHOTGUNS_ONLY, [0, 4]));
        assert!(entry_ok(NO_SHOTGUNS, [0]) && !entry_ok(NO_SHOTGUNS, [0x1a, 4]));
        assert!(entry_ok(NO_BACKPACK, []));
        assert_eq!(refusal_texts(SNIPER_ONLY), Some((0x1795, 0x1796)));
        assert_eq!(refusal_texts(NO_BACKPACK), None);
    }

    #[test]
    fn a_sniper_only_match_grants_sniper_rifles_and_the_knife_only() {
        assert!(match_grants(SNIPER_ONLY, 2, false));
        assert!(match_grants(SNIPER_ONLY, 2, true));
        assert!(!match_grants(SNIPER_ONLY, 4, false), "no pistol (DSV-476)");
        assert!(!match_grants(SNIPER_ONLY, 5, false), "no other secondary");
        assert!(match_grants(SNIPER_ONLY, 7, false));
        assert!(!match_grants(SNIPER_ONLY, 0, false));
        assert!(!match_grants(SNIPER_ONLY, 6, false));
        assert!(!match_grants(NO_BACKPACK, 0, true) && match_grants(NO_BACKPACK, 0, false));
        assert!(match_grants(0, 8, true));
    }
}
