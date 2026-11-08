//! Shooter mode, sans-IO: after a spawn the bot selects its primary weapon the way the 2009 client did, turns its
//! aim toward the nearest enemy-team player object of the object stream, and every `shot_period_ms` reports one shot
//! (`UP_FIRE` 0x82, one message per pellet) that hits that object, until a kill message names the victim; it then
//! waits for the victim to come back and repeats. Hold-fire is the default (no [`Shooter`]).
//!
//! Every message form is the real 2009 client's, decoded from capture `20261025_002236_tdm4` (the client's link,
//! conn 6): the select pair `76 03 02` + `7f [slot][weapon][ammo]`, then per shot one 0x82 with the hit trace
//! (`has_extra` block with the hit object, a hit node byte, the hit point and a constant vector), the 0x64 that
//! follows a shot carrying `pu_flags` 0x2000 and control bit 0x400. Weapon, ammunition, pellets per round, clip and
//! range come from the server's weapon grants (0xb2 / 0xb3) or, without them, from the lobby loadout (0x323
//! slot 0) and the ROM weapon table, read at run time.
//!
//! Every pellet leaves along the 2009 cone (`dc-cone`, WEAPON_FIRE_2009_addendum_cone.md): the forward axis of the
//! sent aim rotation tilted by two reads of the ROM random table at the bot's shot counter (set by the server's 0x7d
//! `own_value`, advanced by 0xda kind 1, two reads per pellet, sent as `stat`), with the half-width of the weapon
//! row's perturb bounds, the row's shot scale raised by the bot's movement state (sent as `shot_scale`) and the
//! spread byte; the zoom byte is 0. The hit claim is the tilted pellet's own trace: a pellet that misses the box is
//! sent in the miss form.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (0x82), §4.2 (0x83), §4.4 (0x7f), §4.5 (0x76 category 3), §2.1 (0x64 flags)
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (0x6d), §4.4 (0xb2 / 0xb3); PROTOCOL_2009.md §3.3 (0x323 +0x12)
// OBSERVED: capture 20261025_002236_tdm4/capture.jsonl (conn 6 c2s 0x7f / 0x76 / 0x82 / 0x64)
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1 .. AC-3 (table, counter, cone, scale, pellets, range)

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use dc_bute::ButeDoc;
use dc_cone::{ConeInputs, ConeRow, MoveState, ShotCounter, ShotTable, NO_MODS};
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;
use dc_rez::RezVfs;
use serde_json::{json, Value};

use crate::skill::{self, AimRng, Skill, SpreadParams};
use crate::weapon::{FireExtra, FireReport, KillNote, WeaponChange, WeaponGrant};

/// BOT-172: the knife run (a child module: it reads the shooter's state).
#[path = "knife.rs"]
pub mod knife;

/// Default time between two shots.
// DC-DEFAULT: BOT-11 (proposed) 150 ms between shots (the real client fired every 295-345 ms with its rifle)
pub const DEFAULT_FIRE_INTERVAL_MS: u32 = 150;
/// The shot origin above the body centre, as a fraction of the box half height.
// OBSERVED: tdm4 conn 6: 0x82 origin - 0x64 trailer position = (0, 32.98, 0) with the 0xa0 box half height 50
pub const EYE_FRACTION: f32 = 0.66;
/// `weapon_pos` relative to the origin: right, up, forward (world units, turned by the yaw).
// OBSERVED: tdm4 conn 6: 0x82 weapon_pos - origin = (10.01, -4.27, 0.36) at yaw 1.0465 = 4.69 right, 8.85 forward
pub const WEAPON_OFFSET: [f32; 3] = [4.69, -4.27, 8.85];
/// The aim point above the target's box centre, as a fraction of its half height (a body hit, not the head). The
/// box centre: with the skill's vertical error at 30..65 (aim error, wander, recoil climb: the shots cross the
/// target a median 0.04 of its height above the aim point, spread 0.35) about 15 % of the hits land in the head
/// band (BOT-165), an average player's share; at 0.5 of the half height (0.75 of the box) it was about 30 %.
// DC-DEFAULT: BOT-12 (proposed; amended 2026-11-08 by the head share of BOT-165) aim at the box centre
pub const AIM_HEIGHT_FRACTION: f32 = 0.0;
/// The point of the target the sight test traces to, above its box centre as a fraction of its half height (BOT-12's
/// aim point before its amendment; the test is unchanged).
// DC-DEFAULT: BOT-12 (proposed) 0.5
pub const SIGHT_HEIGHT_FRACTION: f32 = 0.5;
/// BOT-23: the per-shot aim error of a non-scoped, non-melee weapon (the skill's error, at least BOT-135's floor) is
/// widened by this factor, so that skilled bots (30..65) hit about one shot in seven at the usual ranges, an average
/// player's rate. A scoped sniper rifle (BOT-114 / BOT-123), melee and throws keep their error.
// DC-DEFAULT: BOT-23 (proposed; amended 2026-11-08, coordinator decision "an average player hits with roughly one
// shot in seven at 300-400 u") 1.7
pub const AIM_ERROR_SCALE: f32 = 1.7;
/// BOT-161: a rusher (an infected of a Quarantine human, `View::rushers`) within this distance is fired at full-auto
/// (the clip in one burst, no pause); one crossing the line of sight slower than [`HEAD_ON_CROSSING`] units/s is
/// aimed at [`HEAD_AIM_FRACTION`] of its half height above the centre (the head band: the top [`HEAD_BAND`] of the
/// box height, as dc-dserver's geometric head test); its hits name the node of the zone they land in (BOT-165).
// DC-DEFAULT: BOT-161 (proposed) 600 units, 120 units/s, 0.8, the top 0.2
pub const RUSH_RANGE: f32 = 600.0;
pub const HEAD_ON_CROSSING: f32 = 120.0;
pub const HEAD_AIM_FRACTION: f32 = 0.8;
pub const HEAD_BAND: f32 = 0.2;

/// BOT-165: a hit's body zone, by the ROM skeleton `Location` code it stands for (1 head, 2 torso, 3 arms, 4 legs).
// OBSERVED: ROM table key MODELBUTES.TXT [Skeleton<s>] Node<k>Location (codes 1..4 read at run time)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Head = 1,
    Torso = 2,
    Arms = 3,
    Legs = 4,
}

/// BOT-165: the legs are the bottom [`LEG_BAND`] of the target's box height; in the band between the legs and the head
/// band ([`HEAD_BAND`]) a shot passing the box's vertical axis farther than [`ARM_OFFSET`] of its half width is an
/// arm hit, else a torso hit.
// DC-DEFAULT: BOT-165 (proposed) the bottom 0.45, 0.6 of the half width
pub const LEG_BAND: f32 = 0.45;
pub const ARM_OFFSET: f32 = 0.6;

/// BOT-165: the zone the shot ray `origin + t dir` (unit `dir`) strikes on the box `centre ± half`, judged where the
/// ray passes nearest the box's vertical axis within the box (`None`: the ray misses the box). The height there
/// against [`HEAD_BAND`] / [`LEG_BAND`], the distance from the axis against [`ARM_OFFSET`].
pub fn zone_of_ray(
    origin: [f32; 3],
    dir: [f32; 3],
    centre: [f32; 3],
    half: [f32; 3],
) -> Option<Zone> {
    let (t0, t1) = ray_box_span(origin, dir, centre, half)?;
    let o = sub(origin, centre);
    let flat = dir[0] * dir[0] + dir[2] * dir[2];
    let t = if flat > 1e-9 {
        (-(o[0] * dir[0] + o[2] * dir[2]) / flat).clamp(t0, t1)
    } else {
        t0
    };
    let q = [o[0] + t * dir[0], o[1] + t * dir[1], o[2] + t * dir[2]];
    let h = 2.0 * half[1].max(1e-3);
    let up = ((q[1] + half[1]) / h).clamp(0.0, 1.0);
    let off = (q[0] * q[0] + q[2] * q[2]).sqrt() / half[0].min(half[2]).max(1e-3);
    Some(if up >= 1.0 - HEAD_BAND {
        Zone::Head
    } else if up < LEG_BAND {
        Zone::Legs
    } else if off > ARM_OFFSET {
        Zone::Arms
    } else {
        Zone::Torso
    })
}

/// BOT-165: the skeleton node a hit in each [`Zone`] names (`UP_FIRE` extra block `extra_kind`). A client cannot see
/// which body row a remote player wears, so per zone the node is one whose `Location` is that zone in every skeleton
/// the ROM's body rows use (the server then scores it as that zone whatever the victim wears), the one with the
/// largest hit `Radius` in the default body's (row 0) skeleton; when the skeletons share none, row 0's own.
// OBSERVED: ROM table keys MODELBUTES.TXT [Model<n>] Skeleton, [Skeleton<s>] Node<k>Location, Node<k>Radius (read at
// run time, no value copied)
// DC-DEFAULT: BOT-165 (proposed)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZoneNodes {
    /// By zone code - 1 (head, torso, arms, legs).
    pub nodes: [Option<u8>; 4],
}

impl ZoneNodes {
    /// The table from the ROM's `MODELBUTES.TXT` (`None`: no such table or no row 0 skeleton).
    pub fn of_rom(rom: &std::path::Path) -> Option<Self> {
        let vfs = dc_rez::RezVfs::new(rom);
        let doc = ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, "ATTRIBUTES\\MODELBUTES.TXT").ok()??;
        Self::from_doc(&doc)
    }

    /// [`ZoneNodes::of_rom`] on a parsed table.
    pub fn from_doc(doc: &ButeDoc) -> Option<Self> {
        let num = |v: Option<&String>| -> Option<f32> {
            let t = v?.trim().trim_matches('"').trim();
            t.strip_suffix(['f', 'F']).unwrap_or(t).parse().ok()
        };
        let skel_of = |row: &dc_bute::SectionMap| num(row.get("Skeleton")).map(|s| s as i64);
        let row0 = skel_of(doc.get("Model0")?)?;
        let mut used: Vec<i64> = doc
            .iter()
            .filter(|(n, _)| n.strip_prefix("Model").is_some_and(|k| k.parse::<u32>().is_ok()))
            .filter_map(|(_, row)| skel_of(row))
            .collect();
        used.sort_unstable();
        used.dedup();
        let skels: Vec<&dc_bute::SectionMap> = used
            .iter()
            .filter_map(|s| doc.get(&format!("Skeleton{s}")))
            .collect();
        let base = doc.get(&format!("Skeleton{row0}"))?;
        let loc = |sec: &dc_bute::SectionMap, k: u8| num(sec.get(&format!("Node{k}Location")));
        let mut nodes = [None; 4];
        for (i, slot) in nodes.iter_mut().enumerate() {
            let code = (i + 1) as f32;
            let own: Vec<u8> = (0..=255u8).filter(|k| loc(base, *k) == Some(code)).collect();
            let shared: Vec<u8> = own
                .iter()
                .copied()
                .filter(|k| skels.iter().all(|s| loc(s, *k) == Some(code)))
                .collect();
            let pick = if shared.is_empty() { own } else { shared };
            let radius = |k: u8| num(base.get(&format!("Node{k}Radius"))).unwrap_or(0.0);
            // the largest radius, the lowest index among equals
            *slot = pick
                .into_iter()
                .fold(None::<u8>, |b, k| match b {
                    Some(b) if radius(b) >= radius(k) => Some(b),
                    _ => Some(k),
                });
        }
        Some(Self { nodes })
    }

    /// The node a hit in `zone` names.
    pub fn node(&self, zone: Zone) -> Option<u8> {
        self.nodes[zone as usize - 1]
    }
}
/// `extra_kind` of a hit on a player (meaning UNPROVEN; the real client sent 24 for its first two hits, 11 later).
// OBSERVED: tdm4 conn 6 0x82 extra block kind 24, 24, 11, 11
pub const DEFAULT_HIT_NODE: u8 = 24;
/// `extra_b` of a hit on a player (the same vector in every observed hit; meaning UNPROVEN).
// OBSERVED: tdm4 conn 6 0x82 extra_b = (-1, 0, 0) in all four hits
pub const HIT_VECTOR: [f32; 3] = [-1.0, 0.0, 0.0];
/// `shot_no` as the real client sent it with a row scale of 1.0 (the 32 bits of the float 1.0 in every message):
/// the shot scale (`dc_cone::shot_scale`).
// OBSERVED: tdm4 conn 6 0x82 shot_no = 0x3f800000 in all five messages; SPEC: addendum AC-0 item 4, AC-3.2
pub const SHOT_WORD: u32 = 0x3f80_0000;
/// `stat` grows by this per pellet while the cone draws (two table reads per pellet; 2, 4, 6, 8, 10 in the capture).
// OBSERVED: tdm4 conn 6 0x82 stat 2, 4, 6, 8, 10; SPEC: addendum AC-1.4, AC-1.5
pub const STAT_STEP: u32 = 2;
/// `pu_flags` bit and control bit the real client's update carried right after each shot.
// OBSERVED: tdm4 conn 6 0x64 flags 0x3321 / control 0x600 at the shots, 0x1321 / 0x200 between them
pub const FIRE_PU_FLAG: u16 = 0x2000;
pub const FIRE_CONTROL_BIT: u32 = 0x400;
/// The weapon slot a test throw takes its grenade from (`--throw-every`).
// OBSERVED: tdm4 / tdm6 (the reference stack granted the grenade in slot 3: 0xb2 [7][9][1.0] slot 3) and rust4 (item 143 in
// slot 3)
pub const THROW_SLOT: u8 = 3;
/// Pitch of a test throw (negative = up): about 20 degrees above the horizon along the bot's facing.
// DC-DEFAULT: BOT-26 (proposed) a test-tool choice, not client behaviour
pub const THROW_PITCH: f32 = -0.35;

/// BOT-114: the per-shot aim error of a scoped shot, as a share of the skill's (BOT-23).
// DC-DEFAULT: BOT-114 (proposed) 0.4
pub const SCOPED_SIGMA_FACTOR: f32 = 0.4;
/// A victim counts as back (respawned) when its object moved this far from where it died.
// DC-DEFAULT: BOT-13 (proposed) 16 units, or the re-arm time below
pub const RESPAWN_MOVE: f32 = 16.0;
/// The move counts only this long after the kill: before it the dead body falls or slides (GrayHammer bot match: 21 to
/// 105 ms after the kill, then the bot fired at the dead player).
// DC-DEFAULT: BOT-13 (amended, proposed) 2000 ms
pub const RESPAWN_MOVE_AFTER_MS: u32 = 2000;
/// A victim counts as back this long after its kill even if its object did not move.
// DC-DEFAULT: BOT-13 (proposed) 9000 ms
pub const DEFAULT_REARM_MS: u32 = 9000;
/// Time a reload takes before the next shot.
// DC-DEFAULT: BOT-14 (proposed) 2500 ms
pub const DEFAULT_RELOAD_MS: u32 = 2500;
/// How long after a spawn the bot waits for the server's weapon grants before it uses the lobby loadout.
// DC-DEFAULT: BOT-15 (proposed) 500 ms
pub const GRANT_WAIT_MS: u32 = 500;
/// Weapon numbers are item numbers below this (larger item numbers are not weapons).
// SPEC: PROTOCOL_2009.md §3.3 (0x323 item numbers); OBSERVED: tdm4 0x323 slots 0..3 = the 0xb2 weapon numbers
pub const WEAPON_ITEM_LIMIT: i32 = 1000;
/// BOT-126: a bot opens fire with a rifle, a pistol or a shotgun only within this distance of its target (a
/// person does not spray a rifle across a valley); a farther one in clear sight is walked toward (the target stays
/// the walk goal) and, kept farther for [`FAR_GIVE_UP_MS`], is let go for [`GIVE_UP_FOR_MS`] or until it comes
/// within the distance, so the bot goes back to its objective or roams. A sniper rifle keeps its own range
/// (BOT-114 / BOT-123).
// DC-DEFAULT: BOT-126 (proposed) 1200 units, given up after 8 s, for 15 s
pub const ENGAGE_RANGE: f32 = 1200.0;
pub const FAR_GIVE_UP_MS: u32 = 8000;
pub const GIVE_UP_FOR_MS: u32 = 15_000;
/// BOT-170: an enemy this bot hit within this long counts as weakened: a skilled bot (`Skill::weak_ratio` above 1)
/// takes it over a nearer one in sight. A client does not see another player's health; its own hits stand for it.
// DC-DEFAULT: BOT-170 (proposed) 5000 ms; a kill (the enemy down) forgets it
pub const WEAK_MS: u32 = 5000;
/// BOT-170: the aim points a skilled bot tries, in order, when the line to its own aim point is blocked: share of
/// the half height above the box centre (the chest, the head band).
// DC-DEFAULT: BOT-170 (proposed) 0.5, 0.8
pub const PEEK_FRACTIONS: [f32; 2] = [0.5, HEAD_AIM_FRACTION];
/// BOT-126: a weapon whose clip and stock are both empty is put away: the bot draws the next slot that has
/// ammunition (secondary, then melee; never the throwable of [`THROW_SLOT`]) and fires it after this draw time;
/// with no weapon left it fights no one (it plays its objective or roams). With a melee weapon it takes only an
/// enemy within [`MELEE_ENGAGE`] as a target and follows no unseen one.
// DC-DEFAULT: BOT-126 (proposed) draw 700 ms, melee 400 units
pub const SWITCH_DRAW_MS: u32 = 700;
/// BOT-146: an engaged target unseen for less than this is kept (occlusion flicker: a post, a rail, a body edge);
/// a switch from a target in clear sight this recently counts its reaction from when the new one came into sight.
// DC-DEFAULT: BOT-146 (proposed) 500 ms
pub const FLICKER_MS: u32 = 500;
pub const MELEE_ENGAGE: f32 = 400.0;

// --------------------------------------------------------------------------------------------- ROM weapon table

/// Archive and entry of the weapon table (the file the client loads, cshell+0x17f270).
// OBSERVED: ROM table key WEAPONS.TXT (entry of ATTRIBUTES.rez)
pub const ATTRIBUTES_REZ: &str = "ATTRIBUTES.rez";
pub const WEAPONS_ENTRY: &str = "ATTRIBUTES\\WEAPONS.TXT";
/// Keys of `[Weapon<n>]` and `[Ammo<n>]` rows.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] VectorsPerRound, ShotsPerClip, AmmoName0, Range;
// [Ammo<n>] Name, MaxAmount
pub const KEY_PELLETS: &str = "VectorsPerRound";
pub const KEY_CLIP: &str = "ShotsPerClip";
pub const KEY_AMMO_NAME0: &str = "AmmoName0";
pub const KEY_RANGE: &str = "Range";
// OBSERVED: ROM table keys WEAPONS.TXT [PVModelIdentifiers] Name<k>, [Weapon<n>] PVModel<ident>, RapidFireCount (same
// reading as dc_client::arms: first-person model per hand identifier, count >= 1 = one shot per press)
pub const KEY_PV_IDENTS: &str = "PVModelIdentifiers";
pub const KEY_PV_MODEL: &str = "PVModel";
pub const KEY_SINGLE_SHOT: &str = "RapidFireCount";
pub const KEY_NAME: &str = "Name";
pub const KEY_MAX_AMOUNT: &str = "MaxAmount";
/// Spread keys of `[Weapon<n>]` (CLI-31 stand-in law, `crate::skill::spread`).
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] MinPerturb, MaxPerturb, PerturbIncSpeed, PerturbDecSpeed
pub const KEY_MIN_SPREAD: &str = "MinPerturb";
pub const KEY_MAX_SPREAD: &str = "MaxPerturb";
pub const KEY_SPREAD_UP: &str = "PerturbIncSpeed";
pub const KEY_SPREAD_DOWN: &str = "PerturbDecSpeed";

/// The values of one weapon the shooter needs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RomWeapon {
    pub pellets: Option<u32>,
    pub clip: Option<u32>,
    /// The `[Ammo<n>]` row named by the first ammunition name.
    pub ammo: Option<u16>,
    pub range: Option<f32>,
    /// That ammunition row's largest carried amount.
    pub max_amount: Option<u32>,
    /// Spread values (min, max, grow, fall) of the row.
    pub spread: [Option<f32>; 4],
    /// The row's cone keys (perturb bounds, shot scale and its movement factors).
    pub cone: ConeRow,
    /// The least gap between two shots (the weapon's own clips, [`crate::shotcycle`]); `None` = unknown.
    pub cycle_ms: Option<u32>,
}

impl RomWeapon {
    /// The row's spread parameters, each missing value replaced by the CLI-31 fallback.
    pub fn spread_params(&self) -> SpreadParams {
        let d = SpreadParams::default();
        SpreadParams {
            min: self.spread[0].unwrap_or(d.min),
            max: self.spread[1].unwrap_or(d.max),
            up: self.spread[2].unwrap_or(d.up),
            down: self.spread[3].unwrap_or(d.down),
        }
    }
}

/// The ROM weapon table as far as the shooter uses it (numbers only; names only to join the rows).
#[derive(Debug, Clone, Default)]
pub struct RomWeapons {
    pub rows: BTreeMap<u16, RomWeapon>,
    /// BOT-97: the item table's `ItemType` of each weapon number (the 0xb2 weapon numbers are the weapon item
    /// numbers); empty without the item table.
    // OBSERVED: ROM table key ITEMS.TXT [WeaponItem<n>] ItemID, ItemType (read by `dc_bute::ItemsTable`)
    pub kinds: BTreeMap<u16, i32>,
    /// BOT-147: the thrown weapons (first ammunition row of `Type` 2) with their flight and blast
    /// ([`crate::grenade::lob_rows`]; the FX table's flight when the ROM folder is opened).
    pub lobs: BTreeMap<u16, crate::grenade::LobRow>,
}

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

fn num(v: Option<&String>) -> Option<f32> {
    let t = v?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

/// A table text value without its quotes, case kept (identifiers and ROM paths).
fn text_of(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_ascii_lowercase()
}

impl RomWeapons {
    pub fn from_doc(doc: &ButeDoc) -> Self {
        Self::from_doc_with(doc, None)
    }

    /// The table, with each row's shot cycle read from the first-person clips of `vfs`.
    pub fn from_doc_with(doc: &ButeDoc, vfs: Option<&RezVfs>) -> Self {
        let idents: Vec<String> = (0..16)
            .map_while(|i| {
                doc.get(KEY_PV_IDENTS)
                    .and_then(|s| s.get(&format!("{KEY_NAME}{i}")))
                    .map(|v| text_of(v))
            })
            .collect();
        let mut ammo: BTreeMap<String, (u16, Option<u32>)> = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if let (Some(n), Some(nm)) = (index_of(name, "Ammo"), sec.get(KEY_NAME)) {
                let max = num(sec.get(KEY_MAX_AMOUNT)).map(|x| x.max(0.0) as u32);
                ammo.entry(unquote(nm)).or_insert((n, max));
            }
        }
        let mut rows = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, "Weapon") {
                let a = sec.get(KEY_AMMO_NAME0).and_then(|a| ammo.get(&unquote(a)));
                rows.insert(
                    n,
                    RomWeapon {
                        pellets: num(sec.get(KEY_PELLETS)).map(|x| x.max(0.0) as u32),
                        clip: num(sec.get(KEY_CLIP)).map(|x| x.max(0.0) as u32),
                        ammo: a.map(|x| x.0),
                        range: num(sec.get(KEY_RANGE)),
                        max_amount: a.and_then(|x| x.1),
                        spread: [
                            num(sec.get(KEY_MIN_SPREAD)),
                            num(sec.get(KEY_MAX_SPREAD)),
                            num(sec.get(KEY_SPREAD_UP)),
                            num(sec.get(KEY_SPREAD_DOWN)),
                        ],
                        cone: ConeRow::of_section(sec),
                        cycle_ms: vfs.and_then(|v| {
                            let paths: Vec<String> = idents
                                .iter()
                                .filter_map(|i| {
                                    sec.get(&format!("{KEY_PV_MODEL}{i}")).map(|m| text_of(m))
                                })
                                .collect();
                            let (fire, post) = crate::shotcycle::model_clip_lengths(v, &paths);
                            let single = num(sec.get(KEY_SINGLE_SHOT)).is_some_and(|n| n >= 1.0);
                            crate::shotcycle::cycle_ms(single, fire, post)
                        }),
                    },
                );
            }
        }
        RomWeapons {
            rows,
            kinds: BTreeMap::new(),
            lobs: crate::grenade::lob_rows(doc, None),
        }
    }

    /// Reads the table from the user's ROM folder.
    pub fn open(rom: &Path) -> Result<Self, String> {
        let vfs = RezVfs::new(rom);
        match ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, WEAPONS_ENTRY) {
            Ok(Some(d)) => {
                let mut t = Self::from_doc_with(&d, Some(&vfs));
                t.lobs = crate::grenade::lob_rows(&d, crate::grenade::open_fx(&vfs).as_ref());
                if let Ok(items) = dc_bute::ItemsTable::from_vfs(&vfs) {
                    t.kinds = t
                        .rows
                        .keys()
                        .map(|&w| (w, items.weapon_class_code(i32::from(w))))
                        .filter(|(_, k)| *k >= 0)
                        .collect();
                }
                Ok(t)
            }
            Ok(None) => Err("weapon table entry missing".into()),
            Err(e) => Err(format!("weapon table: {e}")),
        }
    }

    pub fn get(&self, weapon: u16) -> Option<&RomWeapon> {
        self.rows.get(&weapon)
    }

    /// Whether weapon `weapon` is a sniper rifle (item type 2); false when its type is unknown.
    pub fn is_sniper(&self, weapon: u16) -> bool {
        self.kinds.get(&weapon) == Some(&dc_bute::arms::TYPE_SNIPER)
    }
}

// --------------------------------------------------------------------------------------------- geometry

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Yaw and pitch that look from `from` to `to`: forward = (sin yaw cos pitch, -sin pitch, cos yaw cos pitch).
// SPEC: MOVEMENT_2009.md §6.3 (forward axis of the yaw-then-pitch rotation); OBSERVED: tdm4 conn 6 0x82 (aim
// quaternion (0.0059, 0.4997, -0.0034, 0.8662) = yaw 1.0466 about +Y then pitch 0.0136, dir.y = -0.0132)
pub fn aim_angles(from: [f32; 3], to: [f32; 3]) -> (f32, f32) {
    let d = sub(to, from);
    let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
    (d[0].atan2(d[2]), (-d[1]).atan2(flat))
}

/// Unit forward vector of yaw / pitch.
pub fn forward(yaw: f32, pitch: f32) -> [f32; 3] {
    [
        yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ]
}

/// The aim rotation as `quat128` components (x, y, z, w): yaw about +Y, then pitch about +X.
pub fn aim_quat(yaw: f32, pitch: f32) -> [f32; 4] {
    let q = dc_sim::Quat::from_yaw_pitch(yaw, pitch);
    [q.x, q.y, q.z, q.w]
}

/// Distance along the unit ray `origin + t dir` at which it enters the box `centre ± half` (0 when the origin is
/// inside), or `None` when it misses.
pub fn ray_box_entry(
    origin: [f32; 3],
    dir: [f32; 3],
    centre: [f32; 3],
    half: [f32; 3],
) -> Option<f32> {
    ray_box_span(origin, dir, centre, half).map(|s| s.0)
}

/// [`ray_box_entry`] with the exit distance too: the part `(entry, exit)` of the ray inside the box.
pub fn ray_box_span(
    origin: [f32; 3],
    dir: [f32; 3],
    centre: [f32; 3],
    half: [f32; 3],
) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
    for i in 0..3 {
        let (lo, hi) = (centre[i] - half[i], centre[i] + half[i]);
        if dir[i].abs() < 1e-9 {
            if origin[i] < lo || origin[i] > hi {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo - origin[i]) / dir[i], (hi - origin[i]) / dir[i]);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// The unreliable 0x64 body with `extra` OR-ed into its `pu_flags` (bits that announce no field: 0x0800, 0x1000,
/// 0x2000), everything after the flag word unchanged.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (flag table: 0x0800, 0x1000, 0x2000 have no field)
pub fn with_pu_flags(body: &dc_wire::udp::BitWriter, extra: u16) -> dc_wire::udp::BitWriter {
    let mut r = body.reader();
    let flags = r.read_u16();
    let rest = r.read_bit_string(r.remaining_bits());
    let mut w = dc_wire::udp::BitWriter::new();
    w.write_u16(flags | (extra & 0x3800)).append(&rest);
    w
}

// --------------------------------------------------------------------------------------------- the shooter

/// Shooter settings.
#[derive(Debug, Clone)]
pub struct ShooterConfig {
    pub shot_period_ms: u32,
    /// 0x323 +0x12 item numbers per equipment slot (-1 = empty).
    pub loadout: Vec<i32>,
    pub rom: Option<Arc<RomWeapons>>,
    pub hit_node: u8,
    /// BOT-165: the node a hit names by the zone its ray strikes (ROM `MODELBUTES.TXT`); `None` without a ROM: every
    /// hit names `hit_node`.
    pub zone_nodes: Option<ZoneNodes>,
    pub rearm_ms: u32,
    pub reload_ms: u32,
    pub seed: u64,
    /// Human-like aiming (`--skill`); `None` = the instant aim of the first shooter (tests and wire checks).
    pub skill: Option<Skill>,
    /// Test throws (`--throw-every <ms>`): every this long while alive, one throw of the [`THROW_SLOT`] grant
    /// (`UP_THROW_HOLD` then the release `UP_FIRE`), whatever the target; `None` = never.
    // DC-DEFAULT: BOT-26 (proposed) off
    pub throw_every_ms: Option<u32>,
    /// BOT-147: the chance a sighting of an enemy in grenade range is answered with the slot-3 grenade (skill mode
    /// only, `--grenades <chance>`); 0 = never.
    // DC-DEFAULT: BOT-147 (proposed) 0 here (the unit tests), [`crate::grenade::CHANCE`] for a skill bot of a run
    pub grenade_chance: f32,
    /// The cone's random table (the ROM entry); `None` = rebuilt by its generator.
    pub table: Option<Arc<ShotTable>>,
    /// The room's weapon limit (0x389 +0x84 / 0x3035 type 0xe, `dc_bute::arms`); 0 = all weapons.
    pub weapon_limit: u32,
}

impl Default for ShooterConfig {
    fn default() -> Self {
        ShooterConfig {
            shot_period_ms: DEFAULT_FIRE_INTERVAL_MS,
            loadout: Vec::new(),
            rom: None,
            hit_node: DEFAULT_HIT_NODE,
            zone_nodes: None,
            rearm_ms: DEFAULT_REARM_MS,
            reload_ms: DEFAULT_RELOAD_MS,
            seed: 0x005e_ed0f_f1e5,
            skill: None,
            throw_every_ms: None,
            grenade_chance: 0.0,
            table: None,
            weapon_limit: 0,
        }
    }
}

/// The weapon the shooter holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldWeapon {
    pub slot: u8,
    pub weapon: u16,
    pub ammo: u16,
    pub pellets: u32,
    pub clip_size: Option<u32>,
    pub clip: u32,
    pub stock: u32,
    pub range: Option<f32>,
    /// "grant" (0xb2 / 0xb3) or "loadout" (0x323 + ROM).
    pub source: &'static str,
    /// The weapon's spread law (ROM row, CLI-31 fallbacks): the spread byte of skill mode.
    pub spread: SpreadParams,
    /// The row's cone keys (no row: no cone, CLI-120).
    pub cone: ConeRow,
    /// The weapon's own least shot gap (`None` = unknown); the bot never fires faster.
    pub cycle_ms: Option<u32>,
}

/// A player the shooter can see: roster record + object stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    pub pid: u8,
    pub team: u8,
    pub obj: u16,
    pub pos: Option<[f32; 3]>,
    pub dims: Option<[f32; 3]>,
    /// Milliseconds since the object's last stream entry (0 = streaming now).
    pub silent_ms: u32,
    /// The object sits on or below the bottom face of the world box (the server's park point).
    pub parked: bool,
    /// `pos` is the object's current position: false while the stream's compressed positions cannot be decoded
    /// (no world box loaded), when `pos` is only where the object was created.
    pub pos_known: bool,
}

/// Stream silence after which an enemy counts as unseen.
// DC-DEFAULT: VIS-13 (proposed) a parked enemy (position on the bottom face of the world box) or one whose stream
// entries stopped for more than 1500 ms is "unseen": never targeted, aimed at or walked toward
pub const UNSEEN_SILENT_MS: u32 = 1500;
/// A patrol point counts as reached within this flat distance.
pub const PATROL_REACHED: f32 = 150.0;

/// Why `p` is unseen, if it is.
// DC-DEFAULT: BOT-25 (proposed) an enemy whose stream position cannot be decoded (no world box) is unseen
// OBSERVED: capture 20261025_065016_tdm6/logs/Bot1.report.json (no ROM: world_resolved null,
// rom_weapons null; the human's character stayed at its create position (0, 0, 0) for 5,386 stream entries and the
// bot reported 810 hits there)
pub fn unseen_reason(p: &Seen) -> Option<&'static str> {
    if !p.pos_known {
        Some("no_position")
    } else if p.parked {
        Some("parked")
    } else if p.silent_ms > UNSEEN_SILENT_MS {
        Some("silent")
    } else {
        None
    }
}

/// True when the loaded world blocks the line from the bot's eye to `p`'s aim point (its centre + half its
/// half-height, BOT-12). No world or no position: not occluded.
// DC-DEFAULT: BOT-53 (proposed) an enemy behind world collision is "occluded": unseen (never chosen, aimed at or shot
// at); a hunting bot walks toward the nearest occluded enemy's current position until the line clears
pub fn occluded(view: &View, p: &Seen) -> bool {
    let (Some(world), Some(pos)) = (view.world.as_deref(), p.pos) else {
        return false;
    };
    let half = p.dims.unwrap_or(view.own_dims);
    let eye = [
        view.own_pos[0],
        view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
        view.own_pos[2],
    ];
    let aim = [pos[0], pos[1] + SIGHT_HEIGHT_FRACTION * half[1], pos[2]];
    !crate::hunt::line_clear(world, eye, aim)
}

/// Why `p` is unseen from this view: [`unseen_reason`], else "occluded" (behind world collision).
pub fn sight_reason(view: &View, p: &Seen) -> Option<&'static str> {
    unseen_reason(p).or_else(|| occluded(view, p).then_some("occluded"))
}

/// What the session tells the shooter each poll.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub own_pid: Option<u8>,
    pub own_team: Option<u8>,
    pub own_pos: [f32; 3],
    pub own_dims: [f32; 3],
    pub players: Vec<Seen>,
    /// Session time in ms (the `time_ms` of 0x82, the same clock as the 0x64 trailer time).
    pub time_ms: u32,
    /// The loaded world's collision (line of sight); `None` = every line counts as clear (instant aim only; with
    /// a skill the bot never claims a hit without a world).
    pub world: Option<Arc<dc_sim::CollisionWorld>>,
    /// The (yaw, pitch) the bot's last update showed (its body and view), the start of every turn.
    pub facing: Option<(f32, f32)>,
    /// The bot's own movement state (control word, airborne after a jump): the shot scale's movement factor.
    pub moving: MoveState,
    /// BOT-142: an enemy the bot takes as its target whenever it has him in sight (an attacker: a defender seen
    /// defusing the planted bomb).
    pub prefer: Option<u8>,
    /// BOT-148: the bot's living team mates (player id, place) in the modes whose bots share the enemies out
    /// (`spread::spreads`); empty elsewhere (the nearest enemy, as before).
    pub mates: Vec<(u8, [f32; 3])>,
    /// BOT-161: the enemies fired at full-auto and, coming head-on, at the head (a Quarantine human's infected).
    pub rushers: Vec<u8>,
}

/// What the session should do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send `76 [3][2]` then `7f` (the real client's select pair).
    Select(WeaponChange),
    /// Send one 0x82 per entry, in order.
    Fire(Vec<FireReport>),
    /// Send `UP_RELOAD` 0x83 `[ammo_id]`.
    Reload(u16),
    /// Send `UP_THROW_HOLD` 0x4d, then this release `UP_FIRE` 0x82 (a test throw).
    Throw(FireReport),
    /// BOT-147: send one `UP_THROW_HOLD` 0x4d (a pass of the hold clip).
    Hold,
    /// BOT-147: send this release `UP_FIRE` 0x82 (after the hold passes).
    Release(FireReport),
    /// An event-log line.
    Event(&'static str, Value),
}

/// Counters for the run report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShooterStats {
    pub selects: u32,
    pub shots: u32,
    pub fire_msgs: u32,
    pub reloads: u32,
    pub kills_by_me: u32,
    pub target_downs: u32,
    pub target_ups: u32,
    pub target_unseen: u32,
    pub target_seen: u32,
    /// Pellets whose tilted ray met the target's box along a clear line (887-bit hit form).
    pub hit_claims: u32,
    /// Pellets sent in the 623-bit form (the tilted ray missed the box or met the world first).
    pub miss_reports: u32,
    /// Skill mode: sightings that started a reaction delay.
    pub reactions: u32,
    /// Skill mode: bursts begun.
    pub bursts: u32,
    /// Test throws sent (`--throw-every`).
    pub throws: u32,
    /// BOT-147: grenades thrown, and sequences begun.
    pub grenades: u32,
    pub grenade_starts: u32,
}

/// The shooter state machine.
#[derive(Debug, Clone)]
pub struct Shooter {
    cfg: ShooterConfig,
    grants: BTreeMap<u8, WeaponGrant>,
    held: Option<HeldWeapon>,
    alive: bool,
    spawned_at: Option<Millis>,
    selected: bool,
    next_fire: Millis,
    reload_until: Option<Millis>,
    /// Victims: pid -> (time of the kill, last position seen then).
    down: BTreeMap<u8, (Millis, Option<[f32; 3]>)>,
    look: Option<(f32, f32)>,
    target: Option<u8>,
    fired: bool,
    /// Where to walk to get a clear shot (the target, while the line is blocked or too long).
    goal: Option<[f32; 3]>,
    /// The last line-of-sight verdict (for change events).
    sight: Option<bool>,
    /// Enemies currently unseen (pid -> reason).
    unseen: BTreeMap<u8, &'static str>,
    /// Last position each enemy was seen at, and where it was first seen (the spawn area).
    seen_at: BTreeMap<u8, ([f32; 3], [f32; 3])>,
    /// Where to patrol while every known enemy is unseen.
    patrol: Option<[f32; 3]>,
    rng: u64,
    /// Skill mode state.
    aim_rng: AimRng,
    last_poll: Option<Millis>,
    /// The first shot at the current sighting may leave at this time (reaction delay).
    ready_at: Option<(u8, Millis)>,
    /// BOT-146: when the line to each recent target was last lost (re-acquire delay; any target, not only the
    /// last one: a switch away and back is no new sighting).
    lost: BTreeMap<u8, Millis>,
    /// BOT-146: each enemy's current run in clear sight (first, last time seen; a flicker shorter than
    /// [`FLICKER_MS`] does not end the run).
    vis: BTreeMap<u8, (Millis, Millis)>,
    /// BOT-170: when this bot last hit each enemy (a weakened one, [`WEAK_MS`]).
    hit_on: BTreeMap<u8, Millis>,
    /// BOT-170: the target whose line has been blocked since then while a skilled bot keeps its reaction.
    blocked_at: Option<(u8, Millis)>,
    /// BOT-146: the last poll with a clear line to the engaged target.
    engaged_at: Option<Millis>,
    /// BOT-146: the reaction delays' random stream (per bot, apart from the aim's).
    react_rng: AimRng,
    /// Shots left in the current burst (0 = a new burst starts after the pause).
    burst_left: u32,
    /// BOT-135: shots already fired in the current burst (the spray growth of the close-range floor).
    burst_fired: u32,
    /// BOT-161: this poll's target is a rusher fired at full-auto (`Some`), coming head-on (`Some(true)`).
    rush: Option<bool>,
    /// BOT-90: the engaged target's last streamed position and when (its velocity for the lagged aim point).
    track: Option<(u8, [f32; 3], Millis)>,
    /// BOT-90: the slow aim wander (yaw, pitch offsets, radians) and when it was last advanced.
    wander: (f32, f32, Option<Millis>),
    /// BOT-90: the overshoot still to settle (yaw, pitch, radians) after a turn onto a new target.
    over: (f32, f32),
    /// BOT-90: the recoil climb of the current burst (radians up), falling back between shots.
    recoil: f32,
    /// BOT-92: the distance to the engaged target while it is in sight.
    target_dist: Option<f32>,
    /// The spread fraction and the time of the last shot.
    spread_f: f32,
    last_shot: Option<Millis>,
    /// BOT-123: no shot has left since the sighting began (the first shot of a sighting).
    first_of_sighting: bool,
    /// BOT-123: the extra settle of a sniper's sighting (from, to): the overshoot does not fall in it, so the longer
    /// wait is a delay only and the first shot is no steadier for it.
    settle_hold: Option<(Millis, Millis)>,
    no_world_said: bool,
    /// BOT-126: the slots emptied this life; the target farther than [`ENGAGE_RANGE`] and since when; the enemies
    /// let go (until when).
    empty: std::collections::BTreeSet<u8>,
    far_since: Option<(u8, Millis)>,
    given_up: BTreeMap<u8, Millis>,
    /// BOT-115: a round class without respawns in the round: a victim stays down until the round is over.
    no_respawn: bool,
    /// The next test throw (`--throw-every`).
    next_throw: Option<Millis>,
    /// BOT-147: the grenade throw under way; thrown this life; the last release (and its weapon, for the kills it
    /// earns); the sighting weighed last (pid, start) and its verdict; the next plan attempt; the target's last
    /// place in clear sight (pid, feet, time); the throw error's random stream.
    nade: Option<NadeRun>,
    nade_spent: bool,
    nade_last: Option<(Millis, u16)>,
    nade_roll: Option<((u8, Millis), bool)>,
    nade_try_at: Option<Millis>,
    nade_mark: Option<(u8, [f32; 3], Millis)>,
    nade_rng: AimRng,
    /// BOT-147: the predicted blast of the bot's grenade in flight, its radius and the fuse's end.
    nade_blast: Option<([f32; 3], f32, Millis)>,
    /// The cone's random table and the shot counter (`local_player`+0x1fc of the 2009 client).
    table: Arc<ShotTable>,
    counter: ShotCounter,
    pub stats: ShooterStats,
    /// BOT-172: the gun put away for a knife run (drawn again with its clip as it was).
    knife_run: Option<HeldWeapon>,
}

impl Shooter {
    pub fn new(cfg: ShooterConfig) -> Self {
        Shooter {
            rng: cfg.seed | 1,
            aim_rng: AimRng::new(cfg.seed ^ 0x00a1_3e77),
            last_poll: None,
            ready_at: None,
            lost: BTreeMap::new(),
            vis: BTreeMap::new(),
            hit_on: BTreeMap::new(),
            blocked_at: None,
            engaged_at: None,
            react_rng: AimRng::new(cfg.seed ^ 0x0e4c_7146),
            burst_left: 0,
            burst_fired: 0,
            rush: None,
            track: None,
            wander: (0.0, 0.0, None),
            over: (0.0, 0.0),
            recoil: 0.0,
            target_dist: None,
            spread_f: 0.0,
            last_shot: None,
            first_of_sighting: false,
            settle_hold: None,
            no_world_said: false,
            empty: Default::default(),
            far_since: None,
            given_up: BTreeMap::new(),
            no_respawn: false,
            next_throw: None,
            nade: None,
            nade_spent: false,
            nade_last: None,
            nade_roll: None,
            nade_try_at: None,
            nade_mark: None,
            nade_rng: AimRng::new(cfg.seed ^ 0x6e61_6465),
            nade_blast: None,
            table: cfg
                .table
                .clone()
                .unwrap_or_else(|| Arc::new(ShotTable::generated())),
            counter: ShotCounter::default(),
            knife_run: None,
            cfg,
            grants: BTreeMap::new(),
            held: None,
            alive: false,
            spawned_at: None,
            selected: false,
            next_fire: 0,
            reload_until: None,
            down: BTreeMap::new(),
            look: None,
            target: None,
            fired: false,
            goal: None,
            sight: None,
            unseen: BTreeMap::new(),
            seen_at: BTreeMap::new(),
            patrol: None,
            stats: ShooterStats::default(),
        }
    }

    pub fn config(&self) -> &ShooterConfig {
        &self.cfg
    }

    pub fn held(&self) -> Option<&HeldWeapon> {
        self.held.as_ref()
    }

    /// Where a hunting bot should walk: the target's position while no clear shot exists.
    pub fn hunt_goal(&self) -> Option<[f32; 3]> {
        let g = self.target.and(self.goal).or(self.patrol);
        // BOT-147: no hunting walk while the grenade is drawn and thrown (the throw is planned from where the bot
        // stands)
        // DC-DEFAULT: BOT-147 (proposed)
        if self.nade.is_some() {
            return None;
        }
        g
    }

    /// BOT-147: the predicted blast point and radius of the bot's grenade until it goes off.
    pub fn live_blast(&self) -> Option<([f32; 3], f32)> {
        self.nade_blast.map(|(b, r, _)| (b, r))
    }

    /// The current aim (yaw, pitch), while there is a target.
    pub fn look(&self) -> Option<(f32, f32)> {
        self.look
    }

    pub fn target(&self) -> Option<u8> {
        self.target
    }

    /// BOT-92: the distance to the engaged target at the last poll, when it was in sight.
    pub fn target_dist(&self) -> Option<f32> {
        self.target_dist
    }

    /// True once after each shot: the next 0x64 carries [`FIRE_PU_FLAG`] and [`FIRE_CONTROL_BIT`].
    pub fn take_fired(&mut self) -> bool {
        std::mem::take(&mut self.fired)
    }

    /// Server 0x7d: the shot counter := `own_value`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0x7d row; WEAPON_FIRE_2009_addendum_cone.md AC-1.5
    pub fn on_shot_seed(&mut self, own_value: u32) {
        self.counter.set(own_value);
    }

    /// Server 0xda kind 1: the shot counter += `delta`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0xda row; WEAPON_FIRE_2009_addendum_cone.md AC-1.5
    pub fn on_shot_seed_add(&mut self, delta: u32) {
        self.counter.add(delta);
    }

    /// The shot counter now (the `stat` of the last pellet).
    pub fn shot_counter(&self) -> u32 {
        self.counter.value
    }

    /// The cone's table ("rom" or "generator" in its `source`).
    pub fn shot_table(&self) -> &ShotTable {
        &self.table
    }

    /// The cone of one shot of `cone` from the aim rotation `aim` with the spread byte `frac`: the shot scale of the
    /// bot's movement state, the zoom byte (`zoom`: a scoped sniper rifle, BOT-114) and triple-shot off.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-3.2 (scale), AC-3.3 (zoom byte, triple byte, both fractions := spread)
    // DC-DEFAULT: BOT-30 (proposed, amended by BOT-114) no triple-shot, no mods; the spread is the byte's own value
    // frac / 255
    fn cone_inputs(cone: &ConeRow, aim: [f32; 4], frac: u8, view: &View, zoom: bool) -> ConeInputs {
        ConeInputs {
            aim,
            spread: f32::from(frac) / 255.0,
            scale: dc_cone::shot_scale(cone, &NO_MODS, view.moving),
            zoom,
            triple: false,
        }
    }

    /// Whether the bot fires `w` through its scope: a sniper rifle (item table `ItemType` 2) whose row has zoomed
    /// bounds. A person scopes a sniper rifle before the shot; unscoped its cone is the row's widest (the L96A1:
    /// half-width 100 either way, about 0.1 rad, against 4..60 zoomed), so an unscoped bot hit almost nothing far
    /// away.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-0 item 4 (the 0x82 `moving` bit is the zoom flag, PROVEN-REPLAY), AC-2.1
    // (zoomed bounds when the zoom byte is set and the row's ZoomedMinPerturb is non-zero)
    // DC-DEFAULT: BOT-114 (proposed) the bot scopes its sniper rifle for every shot
    fn scoped(&self, w: &HeldWeapon) -> bool {
        self.is_sniper(w.weapon) && (w.cone.zoomed.0 != 0 || w.cone.zoomed.1 != 0)
    }

    /// One pellet's direction from the bot's counter (two reads when the row's gate passes).
    fn pellet(&mut self, cone: &ConeRow, c: &ConeInputs) -> [f32; 3] {
        dc_cone::pellet(cone, c, &mut self.counter, &self.table)
    }

    /// Where a pellet along `dir` enters the target's box within `range` along a clear line (`world`; `None` = every
    /// line is clear): `(distance, point)`.
    /// BOT-165: the node a hit by the ray `origin + t dir` on the box `tpos ± half` names: the node of the zone the
    /// ray strikes ([`zone_of_ray`], [`ZoneNodes`]), else `hit_node`.
    // DC-DEFAULT: BOT-165 (proposed)
    fn claim_node(&self, origin: [f32; 3], dir: [f32; 3], tpos: [f32; 3], half: [f32; 3]) -> u8 {
        self.cfg
            .zone_nodes
            .zip(zone_of_ray(origin, dir, tpos, half))
            .and_then(|(z, zone)| z.node(zone))
            .unwrap_or(self.cfg.hit_node)
    }

    fn pellet_hit(
        origin: [f32; 3],
        dir: [f32; 3],
        tpos: [f32; 3],
        half: [f32; 3],
        range: Option<f32>,
        world: Option<&dc_sim::CollisionWorld>,
    ) -> Option<(f32, [f32; 3])> {
        ray_box_entry(origin, dir, tpos, half)
            .filter(|d| !range.is_some_and(|r| r > 0.0 && *d > r))
            .map(|d| {
                (
                    d,
                    [
                        origin[0] + d * dir[0],
                        origin[1] + d * dir[1],
                        origin[2] + d * dir[2],
                    ],
                )
            })
            .filter(|(_, p)| world.is_none_or(|w| crate::hunt::line_clear(w, origin, *p)))
    }

    /// `rand()` in 2..=255 like the client's seed byte (xorshift; deterministic per seed).
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`seed` = cshell+0x19eb40(2, 255))
    fn seed_byte(&mut self) -> u8 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        2 + (x % 254) as u8
    }

    /// Own player state (server 0x67). A spawn (1 after anything else) restarts the select step.
    pub fn on_life(&mut self, now: Millis, state: u8) {
        let alive = state == 1;
        if alive && !self.alive {
            self.spawned_at = Some(now);
            self.next_throw = self.cfg.throw_every_ms.map(|e| now.wrapping_add(e));
            self.selected = false;
            self.reload_until = None;
            self.next_fire = now;
            self.empty.clear();
            self.far_since = None;
            self.given_up.clear();
            self.nade_spent = false;
            self.nade_mark = None;
        }
        if !alive {
            self.nade = None;
            self.look = None;
            self.target = None;
            self.patrol = None;
            self.ready_at = None;
            self.lost.clear();
            self.vis.clear();
            self.engaged_at = None;
            self.burst_left = 0;
        }
        self.alive = alive;
    }

    /// Server 0xb2 / 0xb3. A grant after the select step re-selects (the server replaced the weapons).
    pub fn on_grant(&mut self, g: WeaponGrant) {
        self.grants.insert(g.slot, g);
        // BOT-126: a new grant fills the slot again; a better slot than the one drawn (or than none) is drawn
        if self.empty.remove(&g.slot)
            && g.slot < THROW_SLOT
            && self.held.is_none_or(|h| h.slot > g.slot)
        {
            self.selected = false;
        }
        if self
            .held
            .is_some_and(|h| h.slot == g.slot && h.weapon != g.weapon)
        {
            self.selected = false;
        }
        // BOT-97: a sniper rifle granted after the select in a sniper-only room is taken up
        if self.sniper_only()
            && self.is_sniper(g.weapon)
            && self.held.is_some_and(|h| !self.is_sniper(h.weapon))
        {
            self.selected = false;
        }
    }

    /// The room allows sniper rifles only (BOT-97).
    fn sniper_only(&self) -> bool {
        self.cfg.weapon_limit == dc_bute::arms::SNIPER_ONLY
    }

    fn is_sniper(&self, weapon: u16) -> bool {
        self.cfg.rom.as_ref().is_some_and(|r| r.is_sniper(weapon))
    }

    /// Server 0xb4: the weapon leaves its slot.
    pub fn on_remove(&mut self, slot: u8) {
        self.grants.remove(&slot);
        if self.held.is_some_and(|h| h.slot == slot) {
            self.held = None;
            self.selected = false;
        }
    }

    /// The room's mode class (server 0x9e game endpoint): in a round class without in-round respawns the
    /// victims stay down until the round result (BOT-115).
    // DC-DEFAULT: BOT-115 (proposed; amended 2026-11-07 for class 11 by DSV-213's amendment) classes 2, 3, 11, 12,
    // 13 (the bomb, infected and elimination rows and their clan rows; dc-dserver `RespawnRule::NextRound`): a dead
    // infected player stays dead until the next round
    pub fn set_mode_class(&mut self, class: u8) {
        self.no_respawn = matches!(class, 2 | 3 | 11 | 12 | 13);
    }

    /// Whether the player `pid` counts as down (killed and not back).
    pub fn is_down(&self, pid: u8) -> bool {
        self.down.contains_key(&pid)
    }

    /// Server 0x46 (round result): every victim comes back with the next round.
    pub fn on_round_result(&mut self) {
        self.down.clear();
    }

    /// Server 0x6c: a player left.
    pub fn on_left(&mut self, pid: u8) {
        self.down.remove(&pid);
        self.unseen.remove(&pid);
        self.seen_at.remove(&pid);
        if self.target == Some(pid) {
            self.target = None;
            self.look = None;
        }
    }

    /// Server 0x6d: the victim is down until it comes back.
    pub fn on_kill(&mut self, now: Millis, k: &KillNote, view: &View) -> Vec<Action> {
        let pos = view
            .players
            .iter()
            .find(|p| p.pid == k.victim)
            .and_then(|p| p.pos);
        self.down.insert(k.victim, (now, pos));
        let mine = view.own_pid == Some(k.killer) && k.killer != k.victim;
        if mine {
            self.stats.kills_by_me += 1;
        }
        let mut out = Vec::new();
        self.grenade_outcome(now, k, view, &mut out);
        if self.target == Some(k.victim) {
            self.stats.target_downs += 1;
            self.target = None;
            self.look = None;
            out.push(Action::Event(
                "target_down",
                json!({"pid": k.victim, "killer": k.killer, "mine": mine, "weapon": k.weapon,
                       "hit_zone": k.hit_zone, "shots": self.stats.shots}),
            ));
        }
        out
    }

    /// The primary weapon: the server's grant in the lowest slot (the drawn 0xb3 one first), else the lobby
    /// loadout's slot 0 with the ROM row.
    // OBSERVED: tdm4 conn 6: after the spawn grants (0xb2 slots 0..3) the client selected slot 0
    fn pick(&self) -> Option<HeldWeapon> {
        let rom = |w: u16| self.cfg.rom.as_ref().and_then(|r| r.get(w).copied());
        // 0xb3 carries the clip and the stock; 0xb2 carries the whole amount, of which a clip is loaded
        let from_grant = |g: &WeaponGrant| {
            let r = rom(g.weapon).unwrap_or_default();
            let total = if g.ammo.is_finite() && g.ammo > 0.0 {
                g.ammo as u32
            } else {
                0
            };
            let (clip, stock) = match (g.clip, r.clip) {
                (Some(c), _) => (c, total),
                (None, Some(size)) => (size.min(total), total - size.min(total)),
                (None, None) => (total, 0),
            };
            HeldWeapon {
                slot: g.slot,
                weapon: g.weapon,
                ammo: g.ammo_kind,
                pellets: dc_cone::pellets(r.pellets, &NO_MODS).max(1),
                clip_size: r.clip.or(g.clip),
                clip,
                stock,
                range: Some(dc_cone::range(r.range, &NO_MODS)),
                source: "grant",
                spread: r.spread_params(),
                cone: r.cone,
                cycle_ms: r.cycle_ms,
            }
        };
        // DC-DEFAULT: BOT-97 (proposed) in a sniper-only room the bot draws a granted sniper rifle (the drawn 0xb3 one
        // first, else the lowest slot); without one it keeps the usual pick
        if self.sniper_only() {
            let snipers = || {
                self.grants
                    .values()
                    .filter(|g| self.is_sniper(g.weapon) && !self.empty.contains(&g.slot))
            };
            if let Some(g) = snipers().find(|g| g.equip).or_else(|| snipers().next()) {
                return Some(from_grant(g));
            }
        }
        // BOT-126: no emptied slot, and once one is empty no throwable either
        let usable = |g: &&WeaponGrant| {
            !self.empty.contains(&g.slot) && (self.empty.is_empty() || g.slot < THROW_SLOT)
        };
        if let Some(g) = self.grants.values().filter(usable).find(|g| g.equip) {
            return Some(from_grant(g));
        }
        if let Some(g) = self.grants.values().find(usable) {
            return Some(from_grant(g));
        }
        if !self.empty.is_empty() {
            return None;
        }
        let item = *self.cfg.loadout.first()?;
        if !(0..WEAPON_ITEM_LIMIT).contains(&item) {
            return None;
        }
        let w = item as u16;
        let r = rom(w)?;
        let size = r.clip.unwrap_or(1).max(1);
        let total = r.max_amount.unwrap_or(size);
        Some(HeldWeapon {
            slot: 0,
            weapon: w,
            ammo: r.ammo.unwrap_or(w),
            pellets: dc_cone::pellets(r.pellets, &NO_MODS).max(1),
            clip_size: r.clip,
            clip: size.min(total),
            stock: total.saturating_sub(size.min(total)),
            range: Some(dc_cone::range(r.range, &NO_MODS)),
            source: "loadout",
            spread: r.spread_params(),
            cone: r.cone,
            cycle_ms: r.cycle_ms,
        })
    }

    /// The nearest visible enemy that is not down; also re-arms victims that came back.
    fn choose(&mut self, now: Millis, view: &View, out: &mut Vec<Action>) -> Option<Seen> {
        let mut back = Vec::new();
        // BOT-115: no respawn in the round: no move or timer brings a victim back (only the round result)
        let keep_down = self.no_respawn;
        for (pid, (at, dpos)) in self.down.iter().filter(|_| !keep_down) {
            let p = view.players.iter().find(|p| p.pid == *pid);
            // BOT-13 (amended): a move counts only once the least respawn time has passed (the body falls, slides
            // or is parked right after the kill)
            let moved = since(now, *at) >= RESPAWN_MOVE_AFTER_MS
                && match (p.and_then(|p| p.pos), dpos) {
                    (Some(a), Some(b)) => len(sub(a, *b)) > RESPAWN_MOVE,
                    _ => false,
                };
            let timed = since(now, *at) >= self.cfg.rearm_ms;
            if moved || timed {
                back.push((*pid, if moved { "moved" } else { "timer" }, since(now, *at)));
            }
        }
        for (pid, why, after) in back {
            self.down.remove(&pid);
            self.stats.target_ups += 1;
            out.push(Action::Event(
                "target_up",
                json!({"pid": pid, "why": why, "after_ms": after}),
            ));
        }
        let own = view.own_pos;
        let enemies: Vec<Seen> = view
            .players
            .iter()
            .filter(|p| Some(p.pid) != view.own_pid)
            .filter(|p| view.own_team.is_none_or(|t| p.team != t))
            .copied()
            .collect();
        let why: Vec<Option<&'static str>> =
            enemies.iter().map(|p| sight_reason(view, p)).collect();
        for (p, reason) in enemies.iter().zip(&why) {
            match (*reason, self.unseen.get(&p.pid).copied()) {
                (Some(why), None) => {
                    self.unseen.insert(p.pid, why);
                    self.stats.target_unseen += 1;
                    out.push(Action::Event(
                        "target_unseen",
                        json!({"pid": p.pid, "obj": p.obj, "why": why, "pos": p.pos,
                               "silent_ms": p.silent_ms,
                               "last_seen": self.seen_at.get(&p.pid).map(|s| s.0)}),
                    ));
                }
                (None, Some(_)) => {
                    self.unseen.remove(&p.pid);
                    self.stats.target_seen += 1;
                    out.push(Action::Event(
                        "target_seen",
                        json!({"pid": p.pid, "obj": p.obj, "pos": p.pos}),
                    ));
                }
                _ => {}
            }
            // BOT-146: the run in clear sight (a short flicker keeps its start)
            if reason.is_none() && !self.down.contains_key(&p.pid) {
                let e = self.vis.entry(p.pid).or_insert((now, now));
                if since(now, e.1) > FLICKER_MS {
                    e.0 = now;
                }
                e.1 = now;
            }
            // the last known position: in sight, or streamed from behind a wall (the patrol goal once the server
            // stops showing it)
            if matches!(reason, None | Some("occluded")) && !self.down.contains_key(&p.pid) {
                if let Some(pos) = p.pos {
                    self.seen_at.entry(p.pid).or_insert((pos, pos)).0 = pos;
                }
            }
        }
        let nearest = |want: Option<&'static str>| {
            enemies
                .iter()
                .zip(&why)
                .filter(|(_, r)| **r == want)
                .filter(|(p, _)| !self.down.contains_key(&p.pid))
                .filter_map(|(p, _)| p.pos.map(|pos| (len(sub(pos, own)), *p)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|x| x.1)
        };
        // BOT-148: with team mates the enemies are shared out (each member in player id order takes the nearest one
        // not yet taken), so a team does not all go for one enemy; without mates the nearest
        // DC-DEFAULT: BOT-148 (proposed)
        let own_pid = view.own_pid.unwrap_or(0);
        let spread = |want: Option<&'static str>| {
            let c: Vec<Seen> = enemies
                .iter()
                .zip(&why)
                .filter(|(_, r)| **r == want)
                .filter(|(p, _)| !self.down.contains_key(&p.pid) && p.pos.is_some())
                .map(|(p, _)| *p)
                .collect();
            let at: Vec<[f32; 3]> = c.iter().filter_map(|p| p.pos).collect();
            crate::spread::pick(own_pid, own, &view.mates, &at).map(|i| c[i])
        };
        let mut best = if view.mates.is_empty() {
            nearest(None)
        } else {
            spread(None)
        };
        // BOT-90: the engaged target is kept while it stays in sight unless another is much nearer (no snapping
        // between two visible enemies)
        if let (Some(cur), Some(b), Some(sk)) = (self.target, best, self.cfg.skill) {
            if b.pid != cur {
                let kept = enemies
                    .iter()
                    .zip(&why)
                    .find(|(p, r)| p.pid == cur && r.is_none() && !self.down.contains_key(&cur))
                    .and_then(|(p, _)| p.pos.map(|pos| (len(sub(pos, own)), *p)));
                if let (Some((dk, kp)), Some(bp)) = (kept, b.pos) {
                    if len(sub(bp, own)) > sk.stick_ratio * dk {
                        best = Some(kp);
                    }
                }
            }
        }
        // BOT-146: occlusion flicker keeps the engaged target: unseen behind world collision for less than
        // FLICKER_MS it stays the target (a nearer enemy in sight still takes over by the stick ratio above)
        // DC-DEFAULT: BOT-146 (proposed)
        if let (Some(cur), Some(sk)) = (self.target, self.cfg.skill) {
            let flicker = self
                .vis
                .get(&cur)
                .is_some_and(|v| since(now, v.1) < FLICKER_MS);
            let kept = enemies
                .iter()
                .zip(&why)
                .find(|(p, r)| {
                    p.pid == cur && **r == Some("occluded") && !self.down.contains_key(&cur)
                })
                .and_then(|(p, _)| p.pos.map(|pos| (len(sub(pos, own)), *p)));
            if let (true, Some((dk, kp))) = (flicker, kept) {
                let much_nearer = best
                    .and_then(|b| b.pos)
                    .is_some_and(|bp| len(sub(bp, own)) <= sk.stick_ratio * dk);
                if !much_nearer {
                    best = Some(kp);
                }
            }
        }
        self.vis
            .retain(|_, v| since(now, v.1) <= skill::REACQUIRE_WINDOW_MS);
        // BOT-170: a skilled bot finishes an enemy it hit lately (in sight) before it takes a nearer, unhurt one
        let down = &self.down;
        self.hit_on
            .retain(|p, t| since(now, *t) <= WEAK_MS && !down.contains_key(p));
        if let (Some(sk), Some(b)) = (self.cfg.skill, best) {
            let near = b.pos.map_or(f32::MAX, |p| len(sub(p, own)));
            let weak = enemies
                .iter()
                .zip(&why)
                .filter(|(p, r)| r.is_none() && p.pid != b.pid && self.hit_on.contains_key(&p.pid))
                .filter_map(|(p, _)| p.pos.map(|pos| (len(sub(pos, own)), *p)))
                .filter(|(d, _)| sk.weak_ratio > 1.0 && *d <= sk.weak_ratio * near)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if !self.hit_on.contains_key(&b.pid) {
                if let Some((_, w)) = weak {
                    best = Some(w);
                }
            }
        }
        // BOT-142: the preferred enemy (a defuser) in sight is the target, nearer ones or not
        // DC-DEFAULT: BOT-142 (proposed)
        if let Some(pp) = view.prefer.filter(|p| !self.down.contains_key(p)) {
            if let Some((p, _)) = enemies
                .iter()
                .zip(&why)
                .find(|(p, r)| p.pid == pp && r.is_none() && p.pos.is_some())
            {
                best = Some(*p);
            }
        }
        // nobody in sight: go find the nearest enemy behind a wall (its current position); else patrol toward
        // where an unseen enemy was last seen, then toward where it first appeared (its spawn area)
        self.patrol = None;
        if best.is_none() {
            self.patrol = if view.mates.is_empty() {
                nearest(Some("occluded"))
            } else {
                spread(Some("occluded"))
            }
            .and_then(|p| p.pos);
        }
        if best.is_none() && self.patrol.is_none() {
            let flat =
                |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
            let mut cands = Vec::new();
            // BOT-19 (amended by BOT-117): the last-seen place, then the first-seen place, each once; an enemy whose
            // places were both visited is forgotten until it is seen again (the bot roams meanwhile; walking
            // between the two forever kept a team circling the other team's spawn after both teams had swapped
            // sides)
            let mut visited = Vec::new();
            for p in enemies.iter().filter(|p| self.unseen.contains_key(&p.pid)) {
                if self.down.contains_key(&p.pid) {
                    continue;
                }
                if let Some((last, first)) = self.seen_at.get(&p.pid).copied() {
                    let g = if flat(last, own) > PATROL_REACHED {
                        last
                    } else {
                        visited.push((p.pid, first));
                        first
                    };
                    if flat(g, own) > PATROL_REACHED {
                        cands.push((flat(g, own), g));
                    }
                }
            }
            for (pid, first) in visited {
                if flat(first, own) <= PATROL_REACHED {
                    self.seen_at.remove(&pid);
                } else {
                    self.seen_at.insert(pid, (first, first));
                }
            }
            // BOT-148: the places to patrol to are shared out like the enemies
            self.patrol = if view.mates.is_empty() {
                cands
                    .into_iter()
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|c| c.1)
            } else {
                let at: Vec<[f32; 3]> = cands.iter().map(|c| c.1).collect();
                crate::spread::pick(own_pid, own, &view.mates, &at).map(|i| at[i])
            };
        }
        best
    }

    /// One shot at `t`: one report per pellet, or why there is no clear shot.
    // OBSERVED: botduel1 (the reference stack traces every report against the world: a report through a wall ended at the
    // wall and did no damage); SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (the report is the client's own trace)
    fn shot(
        &mut self,
        view: &View,
        t: &Seen,
        w: &HeldWeapon,
    ) -> Result<Vec<FireReport>, &'static str> {
        let half_t = t.dims.unwrap_or(view.own_dims);
        let tpos = t.pos.ok_or("no position")?;
        let aim_at = [tpos[0], tpos[1] + AIM_HEIGHT_FRACTION * half_t[1], tpos[2]];
        let origin = [
            view.own_pos[0],
            view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
            view.own_pos[2],
        ];
        let (yaw, pitch) = aim_angles(origin, aim_at);
        let dir = forward(yaw, pitch);
        let dist = ray_box_entry(origin, dir, tpos, half_t).ok_or("ray misses the box")?;
        if w.range.is_some_and(|r| r > 0.0 && dist > r) {
            return Err("out of range");
        }
        // the aim line decides whether the bot shoots at all (sight); each pellet then traces on its own
        let aim_point = [
            origin[0] + dist * dir[0],
            origin[1] + dist * dir[1],
            origin[2] + dist * dir[2],
        ];
        if let Some(world) = view.world.as_deref() {
            if !crate::hunt::line_clear(world, origin, aim_point) {
                return Err("blocked");
            }
        }
        let (s, c) = yaw.sin_cos();
        let [r, u, f] = WEAPON_OFFSET;
        let weapon_pos = [
            origin[0] + r * c + f * s,
            origin[1] + u,
            origin[2] - r * s + f * c,
        ];
        let q = aim_quat(yaw, pitch);
        // instant aim: the spread at its floor (the byte 0, the row's lowest half-width)
        // DC-DEFAULT: BOT-31 (proposed) the instant-aim shooter fires with spread byte 0
        let zoom = self.scoped(w);
        let c = Self::cone_inputs(&w.cone, q, 0, view, zoom);
        let n = w.pellets.max(1);
        let mut v = Vec::with_capacity(n as usize);
        for i in 0..n {
            // cone, then the pellet's own trace, then the report (AC-3.4): a tilted pellet may miss the box
            let pdir = self.pellet(&w.cone, &c);
            let hit = Self::pellet_hit(origin, pdir, tpos, half_t, w.range, view.world.as_deref());
            if hit.is_some() {
                self.stats.hit_claims += 1;
            } else {
                self.stats.miss_reports += 1;
            }
            self.stats.fire_msgs += 1;
            let seed = self.seed_byte();
            v.push(FireReport {
                wkind: 0,
                weapon_id: w.weapon,
                ammo_id: w.ammo,
                weapon_pos,
                origin,
                aim: q,
                yaw,
                dir: pdir,
                seed,
                // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-0 item 4 (the `moving` bit is the zoom flag)
                moving: zoom,
                wflag: false,
                mode1: false,
                frac: 0,
                shot_no: c.scale.to_bits(),
                time_ms: view.time_ms,
                extra: hit.map(|(_, p)| FireExtra {
                    obj: t.obj,
                    // BOT-165: the node of the zone this pellet's ray strikes
                    kind: self.claim_node(origin, pdir, tpos, half_t),
                    a: p,
                    b: HIT_VECTOR,
                }),
                flag_c: false,
                last: i + 1 == n,
                hit: hit.map(|(d, _)| (d, t.obj)),
                stat: self.counter.value,
            });
        }
        Ok(v)
    }

    /// The skill's turn rate in radians per second (`None` = instant aim).
    pub fn turn_rate(&self) -> Option<f32> {
        self.cfg.skill.map(|s| s.turn_rad_s())
    }

    /// One reported shot of skill mode along the facing `aim`: a per-shot aim error, then per pellet the weapon
    /// spread; a pellet whose ray enters the target's box along a clear line is the hit form, every other one the
    /// world-hit form (no extra block, no hit block). Returns the reports and how many claim a hit.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1.1 (623-bit miss / world hit, 887-bit object hit)
    // DC-DEFAULT: BOT-23 (proposed, amended) aim error N(0, sigma) on two axes across the aim, once per shot; it turns
    // the sent aim rotation, and the pellets are the 2009 cone from that rotation
    #[allow(clippy::too_many_arguments)]
    fn skill_shot(
        &mut self,
        view: &View,
        t: &Seen,
        w: &HeldWeapon,
        world: &dc_sim::CollisionWorld,
        aim: (f32, f32),
        sk: &Skill,
        frac: f32,
    ) -> (Vec<FireReport>, u32) {
        let half_t = t.dims.unwrap_or(view.own_dims);
        let tpos = t.pos.unwrap_or(view.own_pos);
        let origin = [
            view.own_pos[0],
            view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
            view.own_pos[2],
        ];
        let (yaw, pitch) = aim;
        let sigma = sk.aim_sigma_deg.to_radians();
        let (ea, eb) = (self.aim_rng.normal() * sigma, self.aim_rng.normal() * sigma);
        let mut shot_dir = skill::tilt(forward(yaw, pitch), ea, eb);
        // DC-DEFAULT: BOT-123 (proposed) the first shot of a sighting through a sniper scope, beyond SNIPER_FAR_RANGE,
        // goes wide with the skill's chance: the aim is pulled off by 1.6..3.2 target half heights (a clean miss)
        let first = std::mem::take(&mut self.first_of_sighting);
        if first && self.scoped(w) {
            let dist = len(sub(tpos, origin));
            if dist > skill::SNIPER_FAR_RANGE && self.aim_rng.unit() < sk.sniper_far_miss {
                let off = (1.6 + 1.6 * self.aim_rng.unit()) * half_t[1] / dist;
                let phi = std::f32::consts::TAU * self.aim_rng.unit();
                shot_dir =
                    skill::tilt(shot_dir, (off * phi.cos()).atan(), (off * phi.sin()).atan());
            }
        }
        // the aim error turns the view itself: the sent rotation looks along the erred direction, and the cone tilts
        // every pellet from that rotation's forward axis
        let (ayaw, apitch) = aim_angles([0.0; 3], shot_dir);
        let q = aim_quat(ayaw, apitch);
        let frac = (frac.clamp(0.0, 1.0) * 255.0) as u8;
        let zoom = self.scoped(w);
        let cone = Self::cone_inputs(&w.cone, q, frac, view, zoom);
        let (s, c) = yaw.sin_cos();
        let [r, u, f] = WEAPON_OFFSET;
        let weapon_pos = [
            origin[0] + r * c + f * s,
            origin[1] + u,
            origin[2] - r * s + f * c,
        ];
        let n = w.pellets.max(1);
        let mut v = Vec::with_capacity(n as usize);
        let mut hits = 0;
        for i in 0..n {
            let dir = self.pellet(&w.cone, &cone);
            let hit = Self::pellet_hit(origin, dir, tpos, half_t, w.range, Some(world));
            if hit.is_some() {
                hits += 1;
                self.stats.hit_claims += 1;
            } else {
                self.stats.miss_reports += 1;
            }
            self.stats.fire_msgs += 1;
            let seed = self.seed_byte();
            v.push(FireReport {
                wkind: 0,
                weapon_id: w.weapon,
                ammo_id: w.ammo,
                weapon_pos,
                origin,
                aim: q,
                yaw,
                dir,
                seed,
                // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-0 item 4 (the `moving` bit is the zoom flag)
                moving: zoom,
                wflag: false,
                mode1: false,
                frac,
                shot_no: cone.scale.to_bits(),
                time_ms: view.time_ms,
                extra: hit.map(|(_, p)| FireExtra {
                    obj: t.obj,
                    // BOT-165: the node of the zone this pellet's ray strikes
                    kind: self.claim_node(origin, dir, tpos, half_t),
                    a: p,
                    b: HIT_VECTOR,
                }),
                flag_c: false,
                last: i + 1 == n,
                hit: hit.map(|(d, _)| (d, t.obj)),
                stat: self.counter.value,
            });
        }
        (v, hits)
    }

    /// Skill mode: turn toward the target at the skill's rate, wait the reaction delay once the line is clear,
    /// fire bursts only while the aim is within the tolerance, each shot with aim error and spread.
    // DC-DEFAULT: BOT-21..BOT-24 (proposed); OBSERVED: tdm6 playtest report (the instant-aim bot killed
    // within a second without turning toward the player)
    fn poll_skill(&mut self, now: Millis, view: &View, sk: Skill, out: &mut Vec<Action>) {
        let dt = self.last_poll.map_or(0, |t| since(now, t)).min(250);
        self.last_poll = Some(now);
        self.rush = None;
        self.given_up.retain(|_, until| later(*until, now));
        let chosen = self.choose(now, view, out);
        let melee = self.held.is_some_and(|h| self.is_melee(&h));
        // BOT-126: with a melee weapon only a near enemy is fought and no unseen one followed (BOT-172: a knife run
        // keeps its patrol: the walk goes on)
        if melee && self.knife_run.is_none() {
            self.patrol = None;
        }
        self.lost
            .retain(|_, t| since(now, *t) <= skill::REACQUIRE_WINDOW_MS);
        let Some(target) = chosen.filter(|t| self.engages(now, t, view)) else {
            self.far_since = None;
            if let Some(pid) = self.target.take() {
                self.lost.insert(pid, now);
            }
            self.ready_at = None;
            self.goal = None;
            self.target_dist = None;
            // the body keeps facing where it last looked
            return;
        };
        if self.target != Some(target.pid) {
            // BOT-146: the target switched away from counts as lost now (switching back is a re-acquire)
            if let Some((p, _)) = self.ready_at {
                self.lost.insert(p, now);
            }
            self.target = Some(target.pid);
            self.ready_at = None;
            out.push(Action::Event(
                "target",
                json!({"pid": target.pid, "obj": target.obj, "team": target.team, "pos": target.pos}),
            ));
        }
        let Some(tpos) = target.pos else {
            return;
        };
        let half = target.dims.unwrap_or(view.own_dims);
        let eye = [
            view.own_pos[0],
            view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
            view.own_pos[2],
        ];
        // BOT-90: the aim point is where the target was `lag_ms` ago (its streamed velocity run backwards), so a
        // moving target is led late, as a person's is; the hit test below still uses its real position
        let vel = match self.track {
            Some((pid, p0, t0)) if pid == target.pid && since(now, t0) > 0 => {
                let dt_s = since(now, t0).min(1000) as f32 / 1000.0;
                Some([
                    (tpos[0] - p0[0]) / dt_s,
                    (tpos[1] - p0[1]) / dt_s,
                    (tpos[2] - p0[2]) / dt_s,
                ])
            }
            _ => None,
        };
        if self
            .track
            .is_none_or(|(pid, p0, _)| pid != target.pid || p0 != tpos)
        {
            self.track = Some((target.pid, tpos, now));
        }
        let lag = sk.lag_ms as f32 / 1000.0;
        let lagged = vel.map_or(tpos, |v| {
            [
                tpos[0] - v[0] * lag,
                tpos[1] - v[1] * lag,
                tpos[2] - v[2] * lag,
            ]
        });
        // BOT-161: a rusher (a Quarantine human's infected) within RUSH_RANGE is fired at full-auto; one coming nearly
        // head-on (crossing slower than HEAD_ON_CROSSING) is aimed at the head band
        let rush = view.rushers.contains(&target.pid)
            && len(sub(tpos, eye)) <= RUSH_RANGE
            && !self.held.is_some_and(|h| self.is_sniper(h.weapon) || self.is_melee(&h));
        let head_on = rush
            && vel.is_none_or(|v| {
                let (dx, dz) = (tpos[0] - eye[0], tpos[2] - eye[2]);
                let l = (dx * dx + dz * dz).sqrt().max(1.0);
                ((v[0] * dz - v[2] * dx) / l).abs() < HEAD_ON_CROSSING
            });
        self.rush = rush.then_some(head_on);
        // BOT-170: a good player's aim point sits higher (toward the head band)
        let mut lift = if head_on {
            HEAD_AIM_FRACTION
        } else {
            AIM_HEIGHT_FRACTION + sk.aim_lift
        };
        // BOT-170: a skilled bot shoots at what it sees: with the line to its aim point blocked (a target behind a
        // waist-high wall, a crate, a corner) it aims at the chest or the head when that line is clear
        if let (true, Some(w)) = (sk.keep_sight && !head_on, view.world.as_deref()) {
            let at = |f: f32| [tpos[0], tpos[1] + f * half[1], tpos[2]];
            if !crate::hunt::line_clear(w, eye, at(lift)) {
                if let Some(f) = PEEK_FRACTIONS
                    .into_iter()
                    .find(|f| *f > lift && crate::hunt::line_clear(w, eye, at(*f)))
                {
                    lift = f;
                }
            }
        }
        let aim_at = [tpos[0], tpos[1] + lift * half[1], tpos[2]];
        let aim_lag = [lagged[0], lagged[1] + lift * half[1], lagged[2]];
        // BOT-90: the wander drifts slowly (Ornstein-Uhlenbeck: the shots of a burst share it), the overshoot and
        // the recoil climb settle with their own time constants
        let dtf = dt as f32 / 1000.0;
        let (ws, wt) = (
            sk.wander_sigma_deg.to_radians(),
            skill::WANDER_TAU_MS as f32 / 1000.0,
        );
        let k = (-dtf / wt).exp();
        let kick = ws * (1.0 - k * k).max(0.0).sqrt();
        self.wander.0 = self.wander.0 * k + kick * self.aim_rng.normal();
        self.wander.1 = self.wander.1 * k + kick * self.aim_rng.normal();
        self.wander.2 = Some(now);
        let ko = (-dtf / (skill::OVERSHOOT_TAU_MS as f32 / 1000.0)).exp();
        let held = self
            .settle_hold
            .is_some_and(|(from, to)| !later(from, now) && later(to, now));
        if !held {
            self.over = (self.over.0 * ko, self.over.1 * ko);
        }
        self.recoil *= (-dtf / (skill::RECOIL_TAU_MS as f32 / 1000.0)).exp();
        let true_want = aim_angles(eye, aim_at);
        let base = aim_angles(eye, aim_lag);
        let want = (
            skill::wrap_diff(base.0 + self.wander.0 + self.over.0, 0.0),
            base.1 + self.wander.1 + self.over.1 + self.recoil,
        );
        let cur = view.facing.or(self.look).unwrap_or(want);
        let aim = skill::turn_toward(cur, want, sk.turn_rad_s() * dt as f32 / 1000.0);
        self.look = Some(aim);
        // the line from the eye to the aim point must be clear in the loaded world
        let Some(world) = view.world.clone() else {
            if !self.no_world_said {
                self.no_world_said = true;
                out.push(Action::Event(
                    "sight",
                    json!({"clear": false, "why": "no_world", "pid": target.pid,
                           "note": "no collision world loaded (--rom, CA_ROM or --world-file): no hit claims"}),
                ));
            }
            self.goal = None;
            return;
        };
        let range = self.held.and_then(|h| h.range).filter(|r| *r > 0.0);
        let far = len(sub(aim_at, eye));
        let limited = self
            .held
            .is_some_and(|h| !melee && !self.is_sniper(h.weapon));
        let sight = if range.is_some_and(|r| far > r) {
            Err("out of range")
        } else if limited && far > ENGAGE_RANGE {
            Err("beyond engage range")
        } else if !crate::hunt::line_clear(&*world, eye, aim_at) {
            Err("blocked")
        } else {
            Ok(())
        };
        // BOT-126: a target kept beyond the engage range is let go after a while (the bot moves on)
        if sight.is_ok() || sight == Err("blocked") {
            self.far_since = None;
        } else {
            match self.far_since {
                Some((pid, at)) if pid == target.pid => {
                    if since(now, at) >= FAR_GIVE_UP_MS {
                        self.far_since = None;
                        self.given_up
                            .insert(target.pid, now.wrapping_add(GIVE_UP_FOR_MS));
                        self.target = None;
                        self.goal = None;
                        self.target_dist = None;
                        self.ready_at = None;
                        out.push(Action::Event(
                            "target_given_up",
                            json!({"pid": target.pid, "dist": far, "after_ms": since(now, at)}),
                        ));
                        return;
                    }
                }
                _ => self.far_since = Some((target.pid, now)),
            }
        }
        if let Err(why) = sight {
            self.goal = target.pos;
            self.target_dist = None;
            // BOT-170: a skilled bot keeps its reaction through a line blocked for less than FLICKER_MS (a target
            // strafing past a post, peeking a corner): the delay is not drawn afresh when the line comes back
            if sk.keep_sight && self.ready_at.is_some_and(|r| r.0 == target.pid) {
                if self.blocked_at.is_none_or(|b| b.0 != target.pid) {
                    self.blocked_at = Some((target.pid, now));
                }
            } else if self.ready_at.take().is_some() {
                self.lost.insert(target.pid, now);
            }
            if self.sight != Some(false) {
                self.sight = Some(false);
                out.push(Action::Event(
                    "sight",
                    json!({"clear": false, "why": why, "pid": target.pid, "own_pos": view.own_pos,
                           "target_pos": target.pos}),
                ));
            }
            return;
        }
        self.goal = None;
        self.target_dist = Some(len(sub(tpos, view.own_pos)));
        // BOT-170: a block that lasted longer than a flicker ends the kept reaction (lost then: a re-acquire)
        if let Some((pid, at)) = self.blocked_at.take() {
            if pid == target.pid
                && since(now, at) > FLICKER_MS
                && self.ready_at.take().is_some()
            {
                self.lost.insert(pid, at);
            }
        }
        if self.sight != Some(true) {
            self.sight = Some(true);
            out.push(Action::Event(
                "sight",
                json!({"clear": true, "pid": target.pid, "own_pos": view.own_pos, "target_pos": target.pos}),
            ));
        }
        let was_engaged = self.engaged_at;
        self.engaged_at = Some(now);
        let ready = match self.ready_at {
            Some((pid, at)) if pid == target.pid => at,
            _ => {
                // BOT-146: a target in clear sight as the bot's target within the window (this one or any earlier
                // one) is re-acquired; a switch from a target in clear sight a moment ago counts the reaction from
                // when the new one came into sight; otherwise a fresh sighting, each delay drawn per sighting
                // DC-DEFAULT: BOT-146 (proposed)
                let again = self
                    .lost
                    .get(&target.pid)
                    .is_some_and(|t| since(now, *t) <= skill::REACQUIRE_WINDOW_MS);
                let full = sk.draw_reaction_ms(&mut self.react_rng);
                let (mut delay, kind) = if again {
                    (sk.draw_reacquire_ms(&mut self.react_rng), "reacquire")
                } else {
                    let switching = was_engaged.is_some_and(|t| since(now, t) <= FLICKER_MS);
                    let in_sight = self.vis.get(&target.pid).map_or(0, |v| since(now, v.0));
                    let re = sk.draw_reacquire_ms(&mut self.react_rng);
                    let rest = full.saturating_sub(in_sight).max(re);
                    if switching && rest < full {
                        (rest, "switch")
                    } else {
                        (full, "new")
                    }
                };
                let again = kind != "new";
                // DC-DEFAULT: BOT-123 (proposed) a scoped sniper rifle settles longer before the first shot
                let settle = self
                    .held
                    .filter(|h| self.scoped(h))
                    .map_or(0, |_| sk.sniper_settle_ms);
                self.settle_hold = (settle > 0).then(|| {
                    let from = now.wrapping_add(delay);
                    (from, from.wrapping_add(settle))
                });
                delay += settle;
                self.first_of_sighting = true;
                let at = now.wrapping_add(delay);
                self.ready_at = Some((target.pid, at));
                self.lost.remove(&target.pid);
                self.burst_left = 0;
                self.stats.reactions += 1;
                // BOT-90: a fresh sighting (or a switch) overshoots by a fraction of the turn still to make
                if kind != "reacquire" {
                    let dy = skill::wrap_diff(true_want.0, aim.0);
                    let dp = true_want.1 - aim.1;
                    let cap = sk.overshoot_cap_deg.to_radians();
                    self.over = (
                        (dy * sk.overshoot).clamp(-cap, cap),
                        (dp * sk.overshoot).clamp(-cap, cap),
                    );
                }
                out.push(Action::Event(
                    "reaction",
                    json!({"pid": target.pid, "delay_ms": delay, "reacquire": again, "kind": kind,
                           "full_ms": full,
                           "off_deg": skill::angle_between(aim, want).to_degrees()}),
                ));
                at
            }
        };
        if let Some(r) = self.reload_until {
            if later(r, now) {
                return;
            }
            self.reload_until = None;
            if let Some(h) = self.held.as_mut() {
                let size = h.clip_size.unwrap_or(h.clip).max(1);
                let take = size.min(h.stock);
                h.clip = take;
                h.stock -= take;
            }
        }
        // BOT-161: at a rusher a burst pause already running is cut to the shot cycle
        if let (Some(_), Some(t), Some(h)) = (self.rush, self.last_shot, self.held) {
            let cycle = self.cfg.shot_period_ms.max(h.cycle_ms.unwrap_or(0)).max(1);
            let earliest = t.wrapping_add(cycle);
            if later(self.next_fire, earliest) {
                self.next_fire = earliest;
            }
        }
        if later(ready, now) || later(self.next_fire, now) {
            return;
        }
        // turn before fire: no shot until the aim is within the tolerance of the aim point (the lag, the wander
        // and the recoil are part of the miss, not a reason to wait: a person fires at what they think they see)
        if skill::angle_between(aim, true_want)
            > sk.fire_tolerance_deg.to_radians() + skill::angle_between(want, true_want)
        {
            return;
        }
        let Some(mut h) = self.held else {
            return;
        };
        if h.clip == 0 {
            self.next_fire = now.wrapping_add(self.cfg.shot_period_ms.max(1));
            self.burst_left = 0;
            if h.stock == 0 {
                self.put_away_empty(now, out);
                return;
            }
            self.reload_until = Some(now.wrapping_add(self.cfg.reload_ms));
            self.stats.reloads += 1;
            out.push(Action::Event(
                "reload",
                json!({"ammo": h.ammo, "stock": h.stock}),
            ));
            out.push(Action::Reload(h.ammo));
            return;
        }
        if self.burst_left == 0 {
            // DC-DEFAULT: BOT-97 (proposed) a sniper rifle fires one aimed shot per burst (then the pause, at least the
            // rifle's own shot cycle); other weapons burst as BOT-24; BOT-161: at a rusher the clip in one burst
            self.burst_left = if self.rush.is_some() {
                h.clip.max(1)
            } else if self.is_sniper(h.weapon) {
                1
            } else {
                self.aim_rng.range(sk.burst_min.max(1), sk.burst_max.max(1))
            };
            self.burst_fired = 0;
            self.stats.bursts += 1;
        }
        let f = self
            .last_shot
            .map_or(0.0, |t| h.spread.decay(self.spread_f, since(now, t)));
        // BOT-90: a moving shooter errs more; every shot of a burst climbs the aim a little
        let moving = view.moving.any_move || view.moving.airborne;
        let mut sk_shot = sk;
        if moving {
            sk_shot.aim_sigma_deg *= sk.moving_factor;
        }
        // BOT-114: a shot through the scope is aimed with more care (a person holds the crosshair on the target)
        // BOT-123: but not as much care as a person: the sniper multiplier of the skill (1.6 at 50) widens it again
        if self.scoped(&h) {
            sk_shot.aim_sigma_deg *= SCOPED_SIGMA_FACTOR * sk.sniper_sigma_mult;
        } else if !melee {
            // DC-DEFAULT: BOT-135 (proposed) near the target the error has a floor in units at the target, grown by
            // the bot's own motion, the target's crossing speed and the spray of the burst; a scoped sniper rifle
            // (BOT-123) and a melee weapon keep their error as it was
            let crossing = vel.map_or(0.0, |v| {
                let (dx, dz) = (aim_at[0] - eye[0], aim_at[2] - eye[2]);
                let l = (dx * dx + dz * dz).sqrt();
                if l > 0.0 {
                    ((v[0] * dz - v[2] * dx) / l).abs()
                } else {
                    0.0
                }
            });
            let floor = sk.close_floor_deg(far, moving, crossing, self.burst_fired);
            // BOT-23: the rifle error widened by AIM_ERROR_SCALE (after the floor; skill scaling unchanged); a
            // Quarantine rusher (BOT-161) keeps its error
            let scale = if self.rush.is_some() { 1.0 } else { AIM_ERROR_SCALE };
            sk_shot.aim_sigma_deg = sk_shot.aim_sigma_deg.max(floor) * scale;
        }
        let (reports, hits) = self.skill_shot(view, &target, &h, &world, aim, &sk_shot, f);
        if hits > 0 {
            self.hit_on.insert(target.pid, now);
        }
        self.recoil -= sk.recoil_deg.to_radians();
        self.spread_f = h.spread.grow(f);
        self.last_shot = Some(now);
        self.burst_left -= 1;
        self.burst_fired += 1;
        let gap = if self.burst_left == 0 && self.rush.is_none() {
            // DC-DEFAULT: BOT-24 (proposed) the pause varies by +-25 % (BOT-161: none at a rusher)
            (sk.burst_pause_ms as f32 * (0.75 + 0.5 * self.aim_rng.unit())) as u32
        } else {
            self.cfg.shot_period_ms
        }
        .max(h.cycle_ms.unwrap_or(0));
        self.next_fire = now.wrapping_add(gap.max(1));
        if h.clip_size.is_some() {
            h.clip -= 1;
        }
        self.held = Some(h);
        self.stats.shots += 1;
        self.fired = true;
        if hits == 0 {
            out.push(Action::Event(
                "miss",
                json!({"pid": target.pid, "shot": self.stats.shots, "pellets": reports.len()}),
            ));
        }
        out.push(Action::Fire(reports));
    }

    /// Whether the held weapon is a melee weapon: its item type, or slot 2 when the type is unknown.
    fn is_melee(&self, h: &HeldWeapon) -> bool {
        match self.cfg.rom.as_ref().and_then(|r| r.kinds.get(&h.weapon)) {
            Some(k) => *k == dc_bute::arms::TYPE_MELEE,
            None => h.slot == 2,
        }
    }

    /// BOT-126: whether the bot fights `t` with what it holds: a melee weapon only a near enemy; an enemy let go
    /// only once it comes within [`ENGAGE_RANGE`]; with nothing drawn no one.
    fn engages(&self, now: Millis, t: &Seen, view: &View) -> bool {
        let Some(h) = self.held else {
            return false;
        };
        let d = t.pos.map(|p| len(sub(p, view.own_pos)));
        if self.is_melee(&h) {
            return d.is_some_and(|d| d <= MELEE_ENGAGE);
        }
        !(self.given_up.get(&t.pid).is_some_and(|u| later(*u, now))
            && d.is_none_or(|d| d > ENGAGE_RANGE))
    }

    /// BOT-126: the held weapon is empty (clip and stock): it is put away and the next slot with ammunition drawn;
    /// with none left the bot drops its target and fights no one until a grant arms it again.
    fn put_away_empty(&mut self, now: Millis, out: &mut Vec<Action>) {
        let Some(h) = self.held else {
            return;
        };
        self.empty.insert(h.slot);
        self.reload_until = None;
        match self.pick() {
            Some(n) => {
                self.held = Some(n);
                self.stats.selects += 1;
                self.next_fire = now.wrapping_add(SWITCH_DRAW_MS);
                self.ready_at = None;
                out.push(Action::Event(
                    "weapon_select",
                    json!({"slot": n.slot, "weapon": n.weapon, "ammo": n.ammo, "clip": n.clip,
                           "clip_size": n.clip_size, "stock": n.stock, "range": n.range, "source": n.source,
                           "empty_slot": h.slot, "melee": self.is_melee(&n)}),
                ));
                out.push(Action::Select(WeaponChange {
                    slot: n.slot,
                    weapon_id: n.weapon,
                    aux: n.ammo,
                }));
            }
            None => {
                self.held = None;
                self.target = None;
                self.goal = None;
                self.patrol = None;
                self.look = None;
                self.target_dist = None;
                self.ready_at = None;
                out.push(Action::Event(
                    "out_of_ammo",
                    json!({"empty_slots": self.empty.iter().collect::<Vec<_>>()}),
                ));
            }
        }
    }

    /// A test throw when one is due (`--throw-every`): the [`THROW_SLOT`] grant's ids, from the eye point along the
    /// facing, [`THROW_PITCH`] up; no hit, no extra block, `last` 1. Nothing without that grant.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (the release: the throwable's ids, `origin` = the eye point, `dir` = the
    // throw direction, no extra block, no hit, `last` 1)
    // DC-DEFAULT: BOT-26 (proposed)
    fn throw_due(&mut self, now: Millis, view: &View) -> Option<FireReport> {
        let every = self.cfg.throw_every_ms?;
        if later(self.next_throw?, now) {
            return None;
        }
        self.next_throw = Some(now.wrapping_add(every.max(1)));
        let g = *self.grants.get(&THROW_SLOT)?;
        let origin = [
            view.own_pos[0],
            view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
            view.own_pos[2],
        ];
        let yaw = view.facing.map_or(0.0, |f| f.0);
        // the release goes through the fire routine's cone like a pellet (CLI-124), with the throwable's row
        let row = self
            .cfg
            .rom
            .as_ref()
            .and_then(|r| r.get(g.weapon))
            .map_or_else(ConeRow::default, |r| r.cone);
        let aim = aim_quat(yaw, THROW_PITCH);
        let c = Self::cone_inputs(&row, aim, 0, view, false);
        let dir = self.pellet(&row, &c);
        self.stats.throws += 1;
        self.stats.fire_msgs += 1;
        let seed = self.seed_byte();
        Some(FireReport {
            wkind: 0,
            weapon_id: g.weapon,
            ammo_id: g.ammo_kind,
            weapon_pos: origin,
            origin,
            aim,
            yaw,
            dir,
            seed,
            moving: false,
            wflag: false,
            mode1: false,
            frac: 0,
            shot_no: c.scale.to_bits(),
            time_ms: view.time_ms,
            extra: None,
            flag_c: false,
            last: true,
            hit: None,
            stat: self.counter.value,
        })
    }

    /// Runs the shooter at `now`.
    pub fn poll(&mut self, now: Millis, view: &View) -> Vec<Action> {
        let mut out = Vec::new();
        if !self.alive {
            return out;
        }
        if self
            .nade_blast
            .is_some_and(|(_, _, until)| !later(until, now))
        {
            self.nade_blast = None;
        }
        // select once per life, after the grants had a chance to arrive
        if !self.selected {
            let waited = self
                .spawned_at
                .is_none_or(|s| since(now, s) >= GRANT_WAIT_MS);
            if self.grants.is_empty() && !waited {
                return out;
            }
            match self.pick() {
                Some(h) => {
                    self.selected = true;
                    self.held = Some(h);
                    self.stats.selects += 1;
                    let lobby_slot0 = self.cfg.loadout.first().copied();
                    out.push(Action::Event(
                        "weapon_select",
                        json!({"slot": h.slot, "weapon": h.weapon, "ammo": h.ammo, "pellets": h.pellets,
                               "clip": h.clip, "clip_size": h.clip_size, "stock": h.stock, "range": h.range,
                               "source": h.source, "lobby_slot0": lobby_slot0,
                               "weapon_limit": self.cfg.weapon_limit, "sniper": self.is_sniper(h.weapon),
                               "lobby_agrees": lobby_slot0 == Some(h.weapon as i32)}),
                    ));
                    // OBSERVED: tdm4 conn 6 `7f [slot][weapon][aux]` with aux = the grant's ammunition id
                    out.push(Action::Select(WeaponChange {
                        slot: h.slot,
                        weapon_id: h.weapon,
                        aux: h.ammo,
                    }));
                }
                None => {
                    if waited {
                        self.selected = true;
                        out.push(Action::Event(
                            "weapon_select",
                            json!({"error": "no weapon: no grant and no usable loadout slot 0 / ROM row"}),
                        ));
                    }
                    return out;
                }
            }
        }
        if let Some(f) = self.throw_due(now, view) {
            out.push(Action::Event(
                "throw",
                json!({"weapon": f.weapon_id, "ammo": f.ammo_id, "origin": f.origin, "dir": f.dir,
                       "n": self.stats.throws}),
            ));
            out.push(Action::Throw(f));
        }
        if self.held.is_none() {
            return out;
        }
        if let Some(sk) = self.cfg.skill {
            if self.nade.is_some() {
                self.nade_step(now, view, sk, &mut out);
                return out;
            }
            self.poll_skill(now, view, sk, &mut out);
            self.consider_grenade(now, view, &mut out);
            return out;
        }
        let Some(target) = self.choose(now, view, &mut out) else {
            if self.target.take().is_some() {
                self.look = None;
            }
            self.goal = None;
            return out;
        };
        if self.target != Some(target.pid) {
            self.target = Some(target.pid);
            out.push(Action::Event(
                "target",
                json!({"pid": target.pid, "obj": target.obj, "team": target.team, "pos": target.pos}),
            ));
        }
        if let Some(p) = target.pos {
            let half = target.dims.unwrap_or(view.own_dims);
            let eye = [
                view.own_pos[0],
                view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
                view.own_pos[2],
            ];
            self.look = Some(aim_angles(
                eye,
                [p[0], p[1] + AIM_HEIGHT_FRACTION * half[1], p[2]],
            ));
        }
        if let Some(r) = self.reload_until {
            if later(r, now) {
                return out;
            }
            self.reload_until = None;
            if let Some(h) = self.held.as_mut() {
                let size = h.clip_size.unwrap_or(h.clip).max(1);
                let take = size.min(h.stock);
                h.clip = take;
                h.stock -= take;
            }
        }
        if later(self.next_fire, now) {
            return out;
        }
        // one decision per interval: a shot, or the reason there is none (and a goal to walk to)
        self.next_fire = now.wrapping_add(self.cfg.shot_period_ms.max(1));
        let Some(mut h) = self.held else {
            return out;
        };
        if h.clip == 0 {
            if h.stock == 0 {
                self.put_away_empty(now, &mut out);
                return out;
            }
            self.reload_until = Some(now.wrapping_add(self.cfg.reload_ms));
            self.stats.reloads += 1;
            out.push(Action::Event(
                "reload",
                json!({"ammo": h.ammo, "stock": h.stock}),
            ));
            out.push(Action::Reload(h.ammo));
            return out;
        }
        let pellets = match self.shot(view, &target, &h) {
            Ok(p) => {
                self.goal = None;
                if self.sight != Some(true) {
                    self.sight = Some(true);
                    out.push(Action::Event(
                        "sight",
                        json!({"clear": true, "pid": target.pid, "own_pos": view.own_pos, "target_pos": target.pos}),
                    ));
                }
                p
            }
            Err(why) => {
                self.goal = target.pos;
                if self.sight != Some(false) {
                    self.sight = Some(false);
                    out.push(Action::Event(
                        "sight",
                        json!({"clear": false, "why": why, "pid": target.pid, "own_pos": view.own_pos,
                               "target_pos": target.pos}),
                    ));
                }
                return out;
            }
        };
        if h.clip_size.is_some() {
            h.clip -= 1;
        }
        self.held = Some(h);
        // BOT-60: the weapon's own cycle is the least gap (the flag period is the floor)
        self.next_fire =
            now.wrapping_add(self.cfg.shot_period_ms.max(h.cycle_ms.unwrap_or(0)).max(1));
        self.stats.shots += 1;
        self.fired = true;
        out.push(Action::Fire(pellets));
        out
    }
}

impl Shooter {
    /// One throw of the slot-3 grant now, whatever `--throw-every` says (a timeline `throw` step); the periodic
    /// schedule is kept. `None` while dead or without the grant.
    pub fn throw_now(&mut self, now: Millis, view: &View) -> Option<FireReport> {
        if !self.alive {
            return None;
        }
        let keep = (self.cfg.throw_every_ms, self.next_throw);
        self.cfg.throw_every_ms = Some(keep.0.unwrap_or(1));
        self.next_throw = Some(now);
        let f = self.throw_due(now, view);
        self.cfg.throw_every_ms = keep.0;
        self.next_throw = keep.1;
        f
    }
}

/// BOT-147: a grenade throw under way: begun at `start` with the select of the [`THROW_SLOT`] grant, then the draw,
/// the wind-up, [`crate::grenade::HOLD_PASSES`] hold passes and the release.
#[derive(Debug, Clone, Copy)]
struct NadeRun {
    start: Millis,
    grant: WeaponGrant,
    row: crate::grenade::LobRow,
    plan: crate::grenade::Plan,
    target: u8,
    mark: [f32; 3],
    dist: f32,
    from_sight: bool,
    holds: u32,
}

// --------------------------------------------------------------------------------------------- BOT-147 grenades

impl Shooter {
    /// The living team mates' places the bot knows (streamed, not down).
    fn mates_now(&self, view: &View) -> Vec<[f32; 3]> {
        let Some(team) = view.own_team else {
            return Vec::new();
        };
        view.players
            .iter()
            .filter(|p| Some(p.pid) != view.own_pid && p.team == team)
            .filter(|p| unseen_reason(p).is_none() && !self.down.contains_key(&p.pid))
            .filter_map(|p| p.pos)
            .collect()
    }

    /// Whether a grenade throw is under way (BOT-147).
    pub fn throwing(&self) -> bool {
        self.nade.is_some()
    }

    fn eye(view: &View) -> [f32; 3] {
        [
            view.own_pos[0],
            view.own_pos[1] + EYE_FRACTION * view.own_dims[1],
            view.own_pos[2],
        ]
    }

    /// BOT-147: after the skill poll, whether to begin a grenade throw at the engaged target (in clear sight early
    /// in the sighting) or at the place it was last seen (shortly after it went out of sight): the rules of
    /// [`crate::grenade`] (range, one roll per sighting, the predicted blast near the mark and safe for the thrower
    /// and its mates, one grenade per life, a cooldown). The throw begins with the select of the grenade.
    // DC-DEFAULT: BOT-147 (proposed)
    fn consider_grenade(&mut self, now: Millis, view: &View, out: &mut Vec<Action>) {
        use crate::grenade as gr;
        let chance = self.cfg.grenade_chance;
        if chance <= 0.0 || !self.alive || self.nade_spent || !self.empty.is_empty() {
            return;
        }
        let Some(h) = self.held else {
            return;
        };
        let Some(pid) = self.target.filter(|_| h.slot != THROW_SLOT) else {
            return;
        };
        let in_sight = self.engaged_at == Some(now);
        if in_sight {
            let t = view.players.iter().find(|p| p.pid == pid);
            if let Some(p) = t.and_then(|t| t.pos) {
                let half = t.and_then(|t| t.dims).unwrap_or(view.own_dims);
                self.nade_mark = Some((pid, [p[0], p[1] - half[1], p[2]], now));
            }
        }
        let Some(g) = self.grants.get(&THROW_SLOT).copied() else {
            return;
        };
        if !(g.ammo.is_finite() && g.ammo >= 1.0) {
            return;
        }
        let Some(row) = self
            .cfg
            .rom
            .as_ref()
            .and_then(|r| r.lobs.get(&g.weapon))
            .copied()
        else {
            return;
        };
        if self
            .nade_last
            .is_some_and(|(t, _)| since(now, t) < gr::COOLDOWN_MS)
        {
            return;
        }
        let Some(world) = view.world.clone() else {
            return;
        };
        let Some((_, mark, seen)) = self.nade_mark.filter(|m| m.0 == pid) else {
            return;
        };
        let run_start = self.vis.get(&pid).map(|v| v.0);
        let (early, key) = if in_sight {
            let st = run_start.unwrap_or(now);
            (since(now, st) <= gr::EARLY_MS, (pid, st))
        } else {
            (
                since(now, seen) <= gr::LOST_MS,
                (pid, run_start.unwrap_or(seen)),
            )
        };
        if !early || self.nade_try_at.is_some_and(|t| later(t, now)) {
            return;
        }
        let eye = Self::eye(view);
        let flat = ((mark[0] - eye[0]).powi(2) + (mark[2] - eye[2]).powi(2)).sqrt();
        if !(gr::MIN_RANGE..=gr::MAX_RANGE).contains(&flat) {
            return;
        }
        let ok = match self.nade_roll {
            Some((k, v)) if k == key => v,
            _ => {
                let v = self.nade_rng.unit() < chance;
                self.nade_roll = Some((key, v));
                v
            }
        };
        if !ok {
            return;
        }
        self.nade_try_at = Some(now.wrapping_add(gr::REPLAN_MS));
        let plan = gr::plan(&world, eye, mark, &row);
        if plan.miss > gr::MISS_SHARE * row.radius {
            return;
        }
        let mates = self.mates_now(view);
        if gr::unsafe_blast(plan.blast, view.own_pos, &mates, &row).is_some() {
            return;
        }
        self.nade = Some(NadeRun {
            start: now,
            grant: g,
            row,
            plan,
            target: pid,
            mark,
            dist: flat,
            from_sight: in_sight,
            holds: 0,
        });
        self.stats.grenade_starts += 1;
        self.burst_left = 0;
        // SPEC: GAME_MESSAGES_2009_C2S.md §4.4, §4.5 (the select pair for the grenade's slot)
        out.push(Action::Select(WeaponChange {
            slot: THROW_SLOT,
            weapon_id: g.weapon,
            aux: g.ammo_kind,
        }));
    }

    /// BOT-147: one poll of a throw under way: the aim turns to the planned direction; the hold passes' 0x4d go at
    /// the end of the draw and the wind-up, the release at its time into the release clip.
    // SPEC: CLI-29 (draw, wind-up, hold pass, release timings); GAME_MESSAGES_2009_C2S.md §4.6
    // DC-DEFAULT: BOT-147 (proposed)
    fn nade_step(&mut self, now: Millis, view: &View, sk: Skill, out: &mut Vec<Action>) {
        use crate::grenade as gr;
        let Some(mut r) = self.nade else {
            return;
        };
        let dt = self.last_poll.map_or(0, |t| since(now, t)).min(250);
        self.last_poll = Some(now);
        let want = (r.plan.yaw, r.plan.pitch);
        let cur = view.facing.or(self.look).unwrap_or(want);
        self.look = Some(skill::turn_toward(
            cur,
            want,
            sk.turn_rad_s() * dt as f32 / 1000.0,
        ));
        let hold0 = r.start.wrapping_add(gr::DRAW_MS + gr::WINDUP_MS);
        while r.holds < gr::HOLD_PASSES
            && !later(hold0.wrapping_add(r.holds * gr::HOLD_PASS_MS), now)
        {
            r.holds += 1;
            out.push(Action::Hold);
        }
        let release = hold0.wrapping_add(gr::HOLD_PASSES * gr::HOLD_PASS_MS + gr::RELEASE_AT_MS);
        if r.holds < gr::HOLD_PASSES || later(release, now) {
            self.nade = Some(r);
            return;
        }
        self.nade = None;
        self.release_grenade(now, view, sk, &r, out);
    }

    /// BOT-147: the release: the planned direction with the skill's throw error (a draw whose predicted blast would
    /// be unsafe is drawn again, at most three times, then the plan itself; unsafe too: no release, the gun comes
    /// back after the server's throw window), through the throwable row's cone (CLI-124); then the gun is drawn again.
    // DC-DEFAULT: BOT-147 (proposed)
    fn release_grenade(
        &mut self,
        now: Millis,
        view: &View,
        sk: Skill,
        r: &NadeRun,
        out: &mut Vec<Action>,
    ) {
        use crate::grenade as gr;
        let eye = Self::eye(view);
        // the error is the bot's misjudged landing point: across and along the line to the mark, each with the
        // skill's throw error as an angle seen from the eye; the throw is planned to that point
        let sig = (sk.aim_sigma_deg * gr::ERROR_FACTOR).to_radians();
        let mates = self.mates_now(view);
        let safe = |b: [f32; 3]| gr::unsafe_blast(b, view.own_pos, &mates, &r.row).is_none();
        let (ux, uz) = {
            let (dx, dz) = (r.mark[0] - eye[0], r.mark[2] - eye[2]);
            let l = (dx * dx + dz * dz).sqrt().max(1.0);
            (dx / l, dz / l)
        };
        let mut pick = None;
        if let Some(w) = view.world.clone() {
            for _ in 0..3 {
                let (across, along) = (
                    r.dist * sig * self.nade_rng.normal(),
                    r.dist * sig * self.nade_rng.normal(),
                );
                let aim_at = [
                    r.mark[0] + ux * along - uz * across,
                    r.mark[1],
                    r.mark[2] + uz * along + ux * across,
                ];
                let p = gr::plan(&w, eye, aim_at, &r.row);
                if safe(p.blast) {
                    pick = Some((p.yaw, p.pitch, p.blast));
                    break;
                }
            }
            if pick.is_none() {
                let b = gr::blast_point(&w, eye, forward(r.plan.yaw, r.plan.pitch), &r.row);
                pick = safe(b).then_some((r.plan.yaw, r.plan.pitch, b));
            }
        }
        let held = self.held;
        let reselect = |out: &mut Vec<Action>| {
            if let Some(h) = held {
                out.push(Action::Select(WeaponChange {
                    slot: h.slot,
                    weapon_id: h.weapon,
                    aux: h.ammo,
                }));
            }
        };
        let Some((yaw, pitch, blast)) = pick else {
            // no 0x82 within the server's window after the last hold (it would be taken for the release)
            self.next_fire = now.wrapping_add(gr::CANCEL_HOLD_OFF_MS);
            self.nade_try_at = Some(now.wrapping_add(gr::REPLAN_MS));
            out.push(Action::Event(
                "grenade",
                json!({"phase": "cancel", "target": r.target, "dist": r.dist,
                       "why": gr::unsafe_blast(r.plan.blast, view.own_pos, &mates, &r.row).unwrap_or("unsafe")}),
            ));
            reselect(out);
            return;
        };
        let g = r.grant;
        let cone = self
            .cfg
            .rom
            .as_ref()
            .and_then(|x| x.get(g.weapon))
            .map_or_else(ConeRow::default, |x| x.cone);
        let aim = aim_quat(yaw, pitch);
        let c = Self::cone_inputs(&cone, aim, 0, view, false);
        let dir = self.pellet(&cone, &c);
        self.stats.grenades += 1;
        self.stats.fire_msgs += 1;
        let seed = self.seed_byte();
        let f = FireReport {
            wkind: 0,
            weapon_id: g.weapon,
            ammo_id: g.ammo_kind,
            weapon_pos: eye,
            origin: eye,
            aim,
            yaw,
            dir,
            seed,
            moving: false,
            wflag: false,
            mode1: false,
            frac: 0,
            shot_no: c.scale.to_bits(),
            time_ms: view.time_ms,
            extra: None,
            flag_c: false,
            last: true,
            hit: None,
            stat: self.counter.value,
        };
        let pred = blast;
        out.push(Action::Event(
            "grenade",
            json!({"phase": "throw", "target": r.target, "dist": r.dist, "mark": r.mark,
                   "from": if r.from_sight { "sight" } else { "lost" }, "weapon": g.weapon,
                   "pred_blast": pred, "pred_miss": len(sub(pred, r.mark)), "plan_miss": r.plan.miss,
                   "self_dist": len(sub(pred, view.own_pos)), "pitch": pitch, "yaw": yaw, "dir": dir,
                   "want_dir": forward(yaw, pitch), "eye": eye,
                   "err_deg": [(yaw - r.plan.yaw).to_degrees(), (pitch - r.plan.pitch).to_degrees()],
                   "fuse_ms": r.row.fuse_ms, "radius": r.row.radius, "n": self.stats.grenades}),
        ));
        out.push(Action::Release(f));
        self.nade_spent = true;
        self.nade_last = Some((now, g.weapon));
        self.nade_blast = Some((pred, r.row.radius, now.wrapping_add(r.row.fuse_ms)));
        if let Some(x) = self.grants.get_mut(&THROW_SLOT) {
            x.ammo = 0.0;
        }
        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §1 (after the last grenade the client drew slot 0 at once)
        reselect(out);
        self.next_fire = now.wrapping_add(SWITCH_DRAW_MS);
    }

    /// BOT-147: a kill by the bot's last grenade (within its fuse and a moment after): a `grenade` event with the
    /// outcome (`kill`, `team_kill`, `self_kill`).
    fn grenade_outcome(&mut self, now: Millis, k: &KillNote, view: &View, out: &mut Vec<Action>) {
        let Some((t, w)) = self.nade_last else {
            return;
        };
        let fuse = self
            .cfg
            .rom
            .as_ref()
            .and_then(|r| r.lobs.get(&w))
            .map_or(crate::grenade::DEFAULT_FUSE_MS, |r| r.fuse_ms);
        if k.weapon != w || since(now, t) > fuse + crate::grenade::OUTCOME_SLACK_MS {
            return;
        }
        let own = view.own_pid;
        let selfish = own == Some(k.victim);
        // a self-kill names no killer on the server (DSV-39): the note's killer is the bot or no other player
        let by_other = Some(k.killer) != own && view.players.iter().any(|p| p.pid == k.killer);
        if own != Some(k.killer) && !(selfish && !by_other) {
            return;
        }
        let vteam = view
            .players
            .iter()
            .find(|p| p.pid == k.victim)
            .map(|p| p.team);
        let phase = if selfish {
            "self_kill"
        } else if vteam.is_some() && vteam == view.own_team {
            "team_kill"
        } else {
            "kill"
        };
        out.push(Action::Event(
            "grenade",
            json!({"phase": phase, "victim": k.victim, "killer": k.killer, "after_ms": since(now, t)}),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(players: Vec<Seen>) -> View {
        View {
            own_pid: Some(0),
            own_team: Some(2),
            own_pos: [0.0, 50.0, 0.0],
            own_dims: [15.0, 50.0, 15.0],
            players,
            time_ms: 1234,
            world: None,
            facing: None,
            moving: MoveState::default(),
            prefer: None,
            mates: Vec::new(),
            rushers: Vec::new(),
        }
    }

    /// BOT-148: a team mate on the same spot with a lower player id takes the nearer enemy in sight; the bot takes
    /// the other one (and without mates the nearest, as before).
    #[test]
    fn a_mate_on_the_same_spot_takes_the_nearer_enemy() {
        let near = enemy(3, 30, [0.0, 50.0, 200.0]);
        let far = enemy(4, 40, [0.0, 50.0, 400.0]);
        let mut out = Vec::new();
        let v = View {
            own_pid: Some(5),
            mates: vec![(1, [0.0, 50.0, 0.0])],
            ..view(vec![near, far])
        };
        assert_eq!(armed().choose(0, &v, &mut out).map(|t| t.pid), Some(4));
        let v = View {
            own_pid: Some(0),
            ..v
        };
        assert_eq!(armed().choose(0, &v, &mut out).map(|t| t.pid), Some(3));
        let v = View {
            own_pid: Some(5),
            mates: Vec::new(),
            ..v
        };
        assert_eq!(armed().choose(0, &v, &mut out).map(|t| t.pid), Some(3));
    }

    /// BOT-142: a preferred enemy (a defuser) in sight is the target although another one is nearer.
    #[test]
    fn a_preferred_enemy_in_sight_is_the_target() {
        let near = enemy(3, 30, [0.0, 50.0, 200.0]);
        let far = enemy(4, 40, [0.0, 50.0, 600.0]);
        let mut s = armed();
        let mut out = Vec::new();
        let v = view(vec![near, far]);
        assert_eq!(s.choose(0, &v, &mut out).map(|t| t.pid), Some(3));
        let v = View {
            prefer: Some(4),
            ..v
        };
        assert_eq!(s.choose(100, &v, &mut out).map(|t| t.pid), Some(4));
        // out of sight (parked): the nearest again
        let mut hidden = far;
        hidden.parked = true;
        let v = View {
            players: vec![near, hidden],
            ..v
        };
        assert_eq!(s.choose(200, &v, &mut out).map(|t| t.pid), Some(3));
    }

    fn enemy(pid: u8, obj: u16, pos: [f32; 3]) -> Seen {
        Seen {
            pid,
            team: 1,
            obj,
            pos: Some(pos),
            dims: Some([15.0, 50.0, 15.0]),
            silent_ms: 0,
            parked: false,
            pos_known: true,
        }
    }

    fn grant(slot: u8, weapon: u16, ammo_kind: u16, ammo: f32) -> WeaponGrant {
        WeaponGrant {
            weapon,
            ammo_kind,
            ammo,
            clip: None,
            class_pos: 0,
            slot,
            equip: false,
            parts: [-1; 4],
        }
    }

    fn rom() -> Arc<RomWeapons> {
        let mut r = RomWeapons::default();
        r.rows.insert(
            5,
            RomWeapon {
                pellets: Some(1),
                clip: Some(3),
                ammo: Some(6),
                range: Some(5000.0),
                max_amount: Some(9),
                ..RomWeapon::default()
            },
        );
        r.rows.insert(
            8,
            RomWeapon {
                pellets: Some(4),
                clip: Some(2),
                ammo: Some(8),
                range: Some(800.0),
                max_amount: Some(10),
                cone: ConeRow {
                    min: 10,
                    max: 40,
                    ..ConeRow::default()
                },
                ..RomWeapon::default()
            },
        );
        Arc::new(r)
    }

    fn fires(a: &[Action]) -> Vec<&Vec<FireReport>> {
        a.iter()
            .filter_map(|x| match x {
                Action::Fire(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn aim_math_matches_the_observed_convention() {
        // OBSERVED: tdm4 aim (0.0059, 0.4997, -0.0034, 0.8662) <-> yaw 1.0466, pitch 0.0136
        let q = aim_quat(1.0466, 0.0136);
        assert!((q[0] - 0.0059).abs() < 2e-4 && (q[1] - 0.4997).abs() < 2e-4);
        assert!((q[2] + 0.0034).abs() < 2e-4 && (q[3] - 0.8662).abs() < 2e-4);
        let (y, p) = aim_angles([0.0, 0.0, 0.0], [866.0, -13.6, 500.0]);
        assert!(
            (y - std::f32::consts::FRAC_PI_3).abs() < 1e-3 && (p - 0.0136).abs() < 1e-3,
            "{y} {p}"
        );
        let f = forward(y, p);
        assert!((len(f) - 1.0).abs() < 1e-5);
        // the forward axis of the quaternion is the same vector
        let qf = dc_sim::Quat::from_yaw_pitch(y, p).forward();
        assert!(
            (qf.x - f[0]).abs() < 1e-5 && (qf.y - f[1]).abs() < 1e-5 && (qf.z - f[2]).abs() < 1e-5
        );
    }

    #[test]
    fn fire_flags_keep_the_rest_of_the_update() {
        let mut b = dc_wire::udp::BitWriter::new();
        b.write_u16(0x0120)
            .write_u8(7)
            .write_bit(true)
            .write_u32(0xdead_beef);
        let f = with_pu_flags(&b, FIRE_PU_FLAG);
        assert_eq!(f.bit_len(), b.bit_len());
        let mut r = f.reader();
        assert_eq!(r.read_u16(), 0x2120);
        assert_eq!(
            (r.read_u8(), r.read_bit(), r.read_u32()),
            (7, true, 0xdead_beef)
        );
        // a bit that announces a field is never added
        assert_eq!(with_pu_flags(&b, 0x0200).reader().read_u16(), 0x0120);
    }

    #[test]
    fn ray_box_entry_distances() {
        let d = ray_box_entry([0.0; 3], [0.0, 0.0, 1.0], [0.0, 0.0, 100.0], [10.0; 3]);
        assert_eq!(d, Some(90.0));
        assert_eq!(
            ray_box_entry([0.0; 3], [0.0, 0.0, 1.0], [50.0, 0.0, 100.0], [10.0; 3]),
            None
        );
        assert_eq!(
            ray_box_entry([0.0; 3], [0.0, 0.0, 1.0], [0.0, 0.0, 5.0], [10.0; 3]),
            Some(0.0)
        );
        assert_eq!(
            ray_box_entry([0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, 100.0], [10.0; 3]),
            None
        );
    }

    fn armed() -> Shooter {
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![5, 8, -1],
            rom: Some(rom()),
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 30.0));
        s
    }

    fn names(a: &[Action]) -> Vec<&'static str> {
        a.iter()
            .filter_map(|x| match x {
                Action::Event(n, _) => Some(*n),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_throws_take_the_slot_3_grant_on_their_interval() {
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![5, 8, -1],
            rom: Some(rom()),
            throw_every_ms: Some(1000),
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 30.0));
        s.on_grant(grant(THROW_SLOT, 40, 41, 2.0));
        let throws = |a: &[Action]| -> Vec<FireReport> {
            a.iter()
                .filter_map(|x| match x {
                    Action::Throw(f) => Some(*f),
                    _ => None,
                })
                .collect()
        };
        assert!(
            throws(&s.poll(500, &view(Vec::new()))).is_empty(),
            "not yet"
        );
        let t = throws(&s.poll(1000, &view(Vec::new())));
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].weapon_id, t[0].ammo_id), (40, 41));
        assert!(t[0].hit.is_none() && t[0].extra.is_none() && t[0].last);
        assert!(t[0].dir[1] > 0.0, "thrown upward: {:?}", t[0].dir);
        assert!(throws(&s.poll(1500, &view(Vec::new()))).is_empty());
        assert_eq!(throws(&s.poll(2000, &view(Vec::new()))).len(), 1);
        assert_eq!(s.stats.throws, 2);
        // without the option: never
        let mut q = armed();
        q.on_grant(grant(THROW_SLOT, 40, 41, 2.0));
        assert!(throws(&q.poll(5000, &view(Vec::new()))).is_empty());
    }

    #[test]
    fn a_parked_or_silent_enemy_is_never_targeted_and_the_bot_patrols() {
        let mut s = armed();
        // seen first at (0, 50, 1000)
        let a = s.poll(1000, &view(vec![enemy(1, 40, [0.0, 50.0, 1000.0])]));
        assert_eq!(s.target(), Some(1));
        assert!(!names(&a).contains(&"target_unseen"));
        // parked at the bottom face of the world box: unseen, no target, no look, patrol to the last seen position
        let mut parked = enemy(1, 40, [-512.0, -64.0, -512.0]);
        parked.parked = true;
        let a = s.poll(1500, &view(vec![parked]));
        assert_eq!(s.target(), None);
        assert_eq!(s.look(), None);
        assert!(a.iter().all(|x| !matches!(x, Action::Fire(_))));
        assert_eq!(names(&a), vec!["target_unseen"]);
        assert_eq!(s.hunt_goal(), Some([0.0, 50.0, 1000.0]));
        // the event is logged once
        let a = s.poll(2000, &view(vec![parked]));
        assert!(names(&a).is_empty());
        assert_eq!(s.stats.target_unseen, 1);
        // visible again: targeted at once, patrol over
        let a = s.poll(2500, &view(vec![enemy(1, 40, [0.0, 50.0, 800.0])]));
        assert!(names(&a).contains(&"target_seen"));
        assert_eq!(s.target(), Some(1));
        assert_eq!(s.stats.target_seen, 1);
        assert_eq!(s.hunt_goal(), None);
    }

    #[test]
    fn a_silent_stream_makes_the_enemy_unseen_and_the_nearer_visible_one_wins() {
        let mut s = armed();
        let mut silent = enemy(1, 40, [0.0, 50.0, 100.0]);
        silent.silent_ms = UNSEEN_SILENT_MS + 1;
        let near_edge = Seen {
            silent_ms: UNSEEN_SILENT_MS,
            ..enemy(2, 41, [0.0, 50.0, 900.0])
        };
        let a = s.poll(1000, &view(vec![silent, near_edge]));
        // pid 1 is nearer but silent; pid 2 is exactly at the threshold and still counts as seen
        assert_eq!(s.target(), Some(2));
        assert!(names(&a).contains(&"target_unseen"));
        assert_eq!(unseen_reason(&silent), Some("silent"));
        assert_eq!(unseen_reason(&near_edge), None);
    }

    #[test]
    fn with_only_unseen_enemies_the_patrol_visits_last_seen_then_spawn_area_once() {
        let mut s = armed();
        s.poll(1000, &view(vec![enemy(1, 40, [0.0, 50.0, 1000.0])]));
        s.poll(1100, &view(vec![enemy(1, 40, [0.0, 50.0, 100.0])]));
        let mut gone = enemy(1, 40, [-512.0, -64.0, -512.0]);
        gone.parked = true;
        // the last seen position is within reach of the bot: go to where it first appeared
        s.poll(1500, &view(vec![gone]));
        assert_eq!(s.hunt_goal(), Some([0.0, 50.0, 1000.0]));
        // BOT-117: at the spawn area both places were visited: the enemy is forgotten until it is seen again (the
        // old patrol walked back to the last seen position, between the two forever)
        let mut v = view(vec![gone]);
        v.own_pos = [0.0, 50.0, 1000.0];
        s.poll(1600, &v);
        assert_eq!(s.hunt_goal(), None);
        // seen again: hunted again
        v.players[0] = enemy(1, 40, [300.0, 50.0, 1200.0]);
        s.poll(1700, &v);
        assert_eq!(s.target(), Some(1));
    }

    #[test]
    fn a_sniper_only_room_draws_the_sniper_slot() {
        // synthetic fixture: weapon 5 a rifle (type 0) in slot 0, weapon 27 a sniper rifle (type 2) in slot 2
        let mut r = (*rom()).clone();
        r.rows.insert(
            27,
            RomWeapon {
                pellets: Some(1),
                clip: Some(5),
                ammo: Some(28),
                range: Some(9000.0),
                max_amount: Some(20),
                cycle_ms: Some(1500),
                ..RomWeapon::default()
            },
        );
        r.kinds.insert(5, 0);
        r.kinds.insert(27, dc_bute::arms::TYPE_SNIPER);
        let r = Arc::new(r);
        let selected = |limit: u32| {
            let mut s = Shooter::new(ShooterConfig {
                loadout: vec![5, 8, -1],
                rom: Some(r.clone()),
                weapon_limit: limit,
                ..ShooterConfig::default()
            });
            s.on_life(100, 1);
            s.on_grant(grant(0, 5, 6, 9.0));
            s.on_grant(grant(1, 8, 8, 10.0));
            s.on_grant(grant(2, 27, 28, 20.0));
            let a = s.poll(160, &view(vec![]));
            let sel = a.iter().find_map(|x| match x {
                Action::Select(c) => Some(*c),
                _ => None,
            });
            (s, sel)
        };
        // no limit: the lowest slot, as before
        assert_eq!(selected(0).1.map(|c| c.slot), Some(0));
        let (s, sel) = selected(dc_bute::arms::SNIPER_ONLY);
        assert_eq!(
            sel,
            Some(WeaponChange {
                slot: 2,
                weapon_id: 27,
                aux: 28
            })
        );
        assert_eq!(s.held().map(|h| h.cycle_ms), Some(Some(1500)));
        // a sniper rifle granted after the select is taken up
        let mut late = Shooter::new(ShooterConfig {
            loadout: vec![5],
            rom: Some(r.clone()),
            weapon_limit: dc_bute::arms::SNIPER_ONLY,
            ..ShooterConfig::default()
        });
        late.on_life(100, 1);
        late.on_grant(grant(0, 5, 6, 9.0));
        assert!(late
            .poll(160, &view(vec![]))
            .iter()
            .any(|x| matches!(x, Action::Select(c) if c.slot == 0)));
        late.on_grant(grant(2, 27, 28, 20.0));
        assert!(late
            .poll(170, &view(vec![]))
            .iter()
            .any(|x| matches!(x, Action::Select(c) if c.slot == 2)));
        // without a sniper rifle the usual pick stays
        let mut none = Shooter::new(ShooterConfig {
            loadout: vec![5],
            rom: Some(r.clone()),
            weapon_limit: dc_bute::arms::SNIPER_ONLY,
            ..ShooterConfig::default()
        });
        none.on_life(100, 1);
        none.on_grant(grant(1, 8, 8, 10.0));
        assert!(none
            .poll(160, &view(vec![]))
            .iter()
            .any(|x| matches!(x, Action::Select(c) if c.slot == 1)));
        assert!(s.held().is_some_and(|h| r.is_sniper(h.weapon)));
    }

    /// BOT-114: a sniper rifle fires scoped (the 0x82 zoom bit, the row's zoomed bounds); unscoped its row's cone is
    /// the widest (synthetic row in the L96A1's shape: plain bounds 100 / 100, zoomed 4 / 60).
    #[test]
    fn a_sniper_rifle_fires_scoped_with_the_zoomed_cone() {
        let row = |sniper: bool| {
            let mut r = (*rom()).clone();
            r.rows.insert(
                27,
                RomWeapon {
                    pellets: Some(1),
                    clip: Some(50),
                    ammo: Some(28),
                    range: Some(9000.0),
                    max_amount: Some(100),
                    cone: ConeRow {
                        min: 100,
                        max: 100,
                        zoomed: (4, 60),
                        ..ConeRow::default()
                    },
                    ..RomWeapon::default()
                },
            );
            r.kinds.insert(
                27,
                if sniper {
                    dc_bute::arms::TYPE_SNIPER
                } else {
                    0
                },
            );
            Arc::new(r)
        };
        let shots = |sniper: bool| {
            let mut s = Shooter::new(ShooterConfig {
                loadout: vec![27],
                rom: Some(row(sniper)),
                shot_period_ms: 100,
                ..ShooterConfig::default()
            });
            s.on_shot_seed(12345);
            s.on_life(100, 1);
            s.on_grant(grant(0, 27, 28, 100.0));
            let v = view(vec![enemy(1, 40, [0.0, 50.0, 1500.0])]);
            let mut out = Vec::new();
            for k in 0..40 {
                for f in fires(&s.poll(160 + k * 100, &v)) {
                    out.extend(f.iter().copied());
                }
            }
            out
        };
        let off = |p: &FireReport| {
            let f = dc_cone::axes(p.aim).2;
            let d = p.dir[0] * f[0] + p.dir[1] * f[1] + p.dir[2] * f[2];
            d.clamp(-1.0, 1.0).acos()
        };
        let scoped = shots(true);
        assert!(scoped.len() >= 20, "{}", scoped.len());
        assert!(
            scoped.iter().all(|p| p.moving),
            "every sniper shot carries the zoom bit"
        );
        // spread byte 0: the zoomed low bound 4, at most 0.004 on each axis
        assert!(
            scoped.iter().all(|p| off(p) <= 0.0058),
            "{:?}",
            scoped.iter().map(off).collect::<Vec<_>>()
        );
        assert!(scoped.iter().filter(|p| p.hit.is_some()).count() * 10 >= scoped.len() * 9);
        // the same row on a rifle that is no sniper rifle: no zoom bit, the plain bounds (100: up to 0.1 per axis)
        let plain = shots(false);
        assert!(plain.iter().all(|p| !p.moving));
        assert!(plain.iter().any(|p| off(p) > 0.02));
    }

    /// BOT-123: one engagement of a stationary target `dist` ahead, polled every 50 ms for 8 s with the look fed back as
    /// the facing; returns (first shot hit, time of the first shot, shots, hit claims). `sniper`: the synthetic row in
    /// the L96A1's shape (plain bounds 100 / 100, zoomed 4 / 60, cycle 1500 ms) typed a sniper rifle, else a rifle row.
    fn engagement(sniper: bool, sk: Skill, dist: f32, seed: u64) -> (bool, u32, u32, u32) {
        let mut r = (*rom()).clone();
        r.rows.insert(
            27,
            RomWeapon {
                pellets: Some(1),
                clip: Some(50),
                ammo: Some(28),
                range: Some(9000.0),
                max_amount: Some(100),
                cone: ConeRow {
                    min: if sniper { 100 } else { 3 },
                    max: 100,
                    zoomed: if sniper { (4, 60) } else { (0, 0) },
                    ..ConeRow::default()
                },
                cycle_ms: Some(1500),
                ..RomWeapon::default()
            },
        );
        r.kinds.insert(
            27,
            if sniper {
                dc_bute::arms::TYPE_SNIPER
            } else {
                0
            },
        );
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![27],
            rom: Some(Arc::new(r)),
            skill: Some(sk),
            seed,
            ..ShooterConfig::default()
        });
        s.on_shot_seed(12345);
        s.on_life(0, 1);
        s.on_grant(grant(0, 27, 28, 100.0));
        let mut v = walled_view(vec![enemy(1, 40, [dist, 50.0, 0.0])], (0.0, 0.0));
        let (mut first, mut first_t, mut shots, mut hits) = (false, 0, 0, 0);
        for t in (0..8000u32).step_by(50) {
            let a = s.poll(t, &v);
            v.facing = s.look().or(v.facing);
            for f in fires(&a) {
                if shots == 0 {
                    first = f[0].hit.is_some();
                    first_t = t;
                }
                shots += 1;
                hits += f.iter().filter(|p| p.hit.is_some()).count() as u32;
            }
        }
        (first, first_t, shots, hits)
    }

    /// BOT-123: a sniper rifle at skill 50 settles +250 ms, errs 1.6 times as much and goes wide on 30 % of
    /// its far first shots; a rifle's engagements are bit for bit what they were. Prints the hit rates (run with
    /// `--nocapture`) at 300 / 900 / 2000 units, before (the BOT-123 fields neutral) and after.
    #[test]
    fn a_sniper_bot_is_less_accurate_a_rifle_bot_is_unchanged() {
        let mut rows = Vec::new();
        for level in [50u8, 80] {
            let after = Skill::new(level);
            let mut before = after;
            before.sniper_sigma_mult = 1.0;
            before.sniper_settle_ms = 0;
            before.sniper_far_miss = 0.0;
            if level == 50 {
                assert_eq!((after.sniper_sigma_mult * 10.0).round(), 16.0);
                assert_eq!(
                    (
                        after.sniper_settle_ms,
                        (after.sniper_far_miss * 100.0).round()
                    ),
                    (250, 30.0)
                );
            }
            let n = 800u64;
            for dist in [300.0f32, 900.0, 2000.0] {
                let mut cells = [(0u32, 0u32, 0u32, 0u32, 0u64); 4];
                for seed in 0..n {
                    for (k, (sniper, sk)) in [
                        (true, before),
                        (true, after),
                        (false, before),
                        (false, after),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let (f, ft, sh, hi) = engagement(sniper, sk, dist, seed + 1);
                        let c = &mut cells[k];
                        c.0 += f as u32;
                        c.1 += sh;
                        c.2 += hi;
                        c.3 += (sh > 0) as u32;
                        c.4 += ft as u64;
                    }
                }
                // a rifle is not touched by the sniper rule
                assert_eq!(cells[2], cells[3], "rifle at {dist}");
                let pct = |c: (u32, u32, u32, u32, u64)| {
                    (
                        100.0 * c.0 as f32 / c.3.max(1) as f32,
                        100.0 * c.2 as f32 / c.1.max(1) as f32,
                        c.4 as f32 / c.3.max(1) as f32,
                    )
                };
                let (b, a) = (pct(cells[0]), pct(cells[1]));
                let r = pct(cells[3]);
                rows.push(format!(
                "skill {level}, {dist:>5} units: sniper first-shot hit {:.1}% -> {:.1}%, all shots in 8 s {:.1}% -> {:.1}%, first shot at {:.0} -> {:.0} ms; rifle (unchanged) first {:.1}% all {:.1}% at {:.0} ms",
                b.0, a.0, b.1, a.1, b.2, a.2, r.0, r.1, r.2
            ));
                assert!(a.0 <= b.0 + 4.0 && a.1 <= b.1 + 4.0, "{dist}: {b:?} {a:?}");
                assert!(a.2 >= b.2 + 100.0, "{dist}: settle {b:?} {a:?}");
                if dist > skill::SNIPER_FAR_RANGE {
                    assert!(a.0 < b.0 * 0.93, "far first shot: {b:?} {a:?}");
                }
            }
        }
        println!(
            "{}",
            rows.join(
                "
"
            )
        );
    }

    /// BOT-115: in a round class without respawns (Search and Destroy) a victim stays down for the rest of the round
    /// whatever its body does or how long it lies (the old timer re-armed it after 9 s and the bot fired at the dead
    /// player: dserver `victim_dead`); the round result brings everyone back.
    #[test]
    fn in_a_round_class_the_dead_stay_down_until_the_round_result() {
        // BOT-115 amended: Quarantine (class 11) too, a dead infected stays dead (DSV-213 amended)
        for class in [2u8, 11] {
            dead_stay_down(class);
        }
    }

    fn dead_stay_down(class: u8) {
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![5],
            rom: Some(rom()),
            shot_period_ms: 100,
            ..ShooterConfig::default()
        });
        s.set_mode_class(class);
        assert!(!s.is_down(1));
        s.on_life(100, 1);
        s.on_grant(grant(0, 5, 6, 9.0));
        let mut v = view(vec![enemy(1, 40, [0.0, 50.0, 300.0])]);
        assert_eq!(fires(&s.poll(200, &v)).len(), 1);
        let k = KillNote {
            victim: 1,
            killer: 0,
            kill_kind: 1,
            weapon: 5,
            hit_zone: 3,
            f_relation: false,
            f_feed2: false,
            keep_hud: false,
            f_feed6: false,
            streak_flag: 0,
            feed_value: 0,
            no_count: false,
            revenge: None,
        };
        s.on_kill(250, &k, &v);
        assert!(s.is_down(1));
        v.players[0].pos = Some([40.0, 20.0, 330.0]);
        for t in [3_000, 12_000, 30_000] {
            let a = s.poll(t, &v);
            assert!(
                !a.iter().any(|x| matches!(x, Action::Event("target_up", _))),
                "{t}: {a:?}"
            );
            assert!(fires(&a).is_empty(), "{t}");
        }
        s.on_round_result();
        assert_eq!(
            fires(&s.poll(30_500, &v)).len(),
            1,
            "back with the next round"
        );
    }

    #[test]
    fn selects_the_slot0_grant_then_fires_at_the_nearest_enemy() {
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![5, 8, -1],
            rom: Some(rom()),
            ..ShooterConfig::default()
        });
        let v = view(vec![
            enemy(1, 40, [0.0, 50.0, 1000.0]),
            enemy(2, 41, [0.0, 50.0, 400.0]),
            Seen {
                team: 2,
                ..enemy(3, 42, [0.0, 50.0, 100.0])
            },
        ]);
        // dead: nothing
        assert!(s.poll(0, &v).is_empty());
        s.on_life(100, 1);
        // waits for grants up to GRANT_WAIT_MS
        assert!(s.poll(150, &v).is_empty());
        s.on_grant(grant(1, 8, 8, 10.0));
        s.on_grant(grant(0, 5, 6, 9.0));
        let a = s.poll(160, &v);
        let sel = a.iter().find_map(|x| match x {
            Action::Select(c) => Some(*c),
            _ => None,
        });
        assert_eq!(
            sel,
            Some(WeaponChange {
                slot: 0,
                weapon_id: 5,
                aux: 6
            })
        );
        // the first shot goes out in the same poll, at the nearest enemy (pid 2; pid 3 is a team mate)
        let f = fires(&a);
        assert_eq!(f.len(), 1);
        let p = &f[0][0];
        assert_eq!(s.target(), Some(2));
        assert_eq!(p.hit.map(|h| h.1), Some(41));
        assert_eq!(
            (p.weapon_id, p.ammo_id, p.last, p.time_ms),
            (5, 6, true, 1234)
        );
        let (d, _) = p.hit.unwrap();
        // origin at eye height 50 + 33, aim point 25 above the target centre: enters the box near 385
        assert!((d - 385.0).abs() < 2.0, "{d}");
        let e = p.extra.unwrap();
        assert_eq!((e.obj, e.kind, e.b), (41, DEFAULT_HIT_NODE, HIT_VECTOR));
        assert!((e.a[2] - (400.0 - 15.0)).abs() < 1.0, "{:?}", e.a);
        // weapon 5's row has no perturb keys: the 2009 gate draws nothing (CLI-120), the counter stays, scale 1.0
        assert_eq!(p.stat, 0);
        assert_eq!(p.shot_no, SHOT_WORD);
        assert_eq!(p.dir, dc_cone::axes(p.aim).2);
        assert!((2..=255).contains(&p.seed));
        assert!(s.take_fired() && !s.take_fired());
        let (yaw, pitch) = s.look().unwrap();
        assert!(yaw.abs() < 1e-6 && pitch > 0.0);
        // cadence: nothing before the interval, then the next shot
        assert!(fires(&s.poll(200, &v)).is_empty());
        assert_eq!(fires(&s.poll(310, &v)).len(), 1);
    }

    #[test]
    fn an_occluded_enemy_is_not_aimed_at_and_becomes_the_goal() {
        // a wall across z = 580..620 between the shooter at the origin and the target at z = 1200
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(rom()),
            ..ShooterConfig::default()
        });
        let mut v = view(vec![enemy(1, 40, [0.0, 50.0, 1200.0])]);
        v.world = Some(crate::hunt::tests::walled());
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 9.0));
        assert_eq!(sight_reason(&v, &v.players[0]), Some("occluded"));
        let a = s.poll(0, &v);
        assert!(fires(&a).is_empty(), "{a:?}");
        assert_eq!(
            event(&a, "target_unseen").map(|e| e["why"].clone()),
            Some(json!("occluded"))
        );
        assert_eq!(
            (s.target(), s.look()),
            (None, None),
            "not chosen, not aimed at"
        );
        // go find it: its current position is the hunt goal
        assert_eq!(s.hunt_goal(), Some([0.0, 50.0, 1200.0]));
        // the server stops showing it (parked): the patrol goes to its last known position
        v.players[0].parked = true;
        s.poll(100, &v);
        assert_eq!(s.hunt_goal(), Some([0.0, 50.0, 1200.0]));
        v.players[0].parked = false;
        // beside the wall the line is clear: seen again, a report, no goal
        v.own_pos = [900.0, 50.0, 0.0];
        let a = s.poll(200, &v);
        assert!(event(&a, "target_seen").is_some(), "{a:?}");
        assert_eq!(fires(&a).len(), 1, "{a:?}");
        assert_eq!(s.target(), Some(1));
        assert_eq!(s.hunt_goal(), None);
    }

    #[test]
    fn a_visible_enemy_wins_over_a_nearer_one_behind_the_wall() {
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(rom()),
            ..ShooterConfig::default()
        });
        // pid 1 is nearer but behind the wall; pid 2 is farther, past the wall's end, in sight
        let mut v = view(vec![
            enemy(1, 40, [0.0, 50.0, 1000.0]),
            enemy(2, 41, [900.0, 50.0, 1200.0]),
        ]);
        v.world = Some(crate::hunt::tests::walled());
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 9.0));
        assert_eq!(sight_reason(&v, &v.players[0]), Some("occluded"));
        assert_eq!(sight_reason(&v, &v.players[1]), None);
        let a = s.poll(0, &v);
        assert_eq!(s.target(), Some(2), "{a:?}");
        let f = fires(&a);
        assert_eq!(f.len(), 1, "{a:?}");
        assert!(f[0].iter().all(|p| p.extra.is_none_or(|e| e.obj == 41)));
        assert_eq!(s.hunt_goal(), None);
    }

    #[test]
    fn pellets_reload_and_kill_rearm() {
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(rom()),
            shot_period_ms: 100,
            rearm_ms: 5000,
            reload_ms: 1000,
            ..ShooterConfig::default()
        });
        let mut v = view(vec![enemy(1, 40, [0.0, 50.0, 300.0])]);
        s.on_life(0, 1);
        s.on_grant(grant(0, 8, 8, 10.0));
        // clip 2 of 4 pellets each: two shots, then a reload
        let a = s.poll(0, &v);
        let f = fires(&a);
        assert_eq!(f[0].len(), 4);
        assert_eq!(f[0].iter().filter(|p| p.last).count(), 1);
        assert!(f[0][3].last);
        // weapon 8 draws its cone: two reads per pellet
        assert_eq!(f[0][3].stat, 4 * STAT_STEP);
        assert!(f[0]
            .iter()
            .all(|p| dc_cone::replay(s.shot_table(), p.aim, p.dir, p.stat) == Some(10)));
        assert_eq!(fires(&s.poll(100, &v)).len(), 1);
        let a = s.poll(200, &v);
        assert!(a.contains(&Action::Reload(8)), "{a:?}");
        assert!(fires(&s.poll(700, &v)).is_empty());
        assert_eq!(fires(&s.poll(1200, &v)).len(), 1);
        // the kill: no more shots at the victim until it moved or the re-arm time passed
        let k = KillNote {
            victim: 1,
            killer: 0,
            kill_kind: 1,
            weapon: 8,
            hit_zone: 3,
            f_relation: false,
            f_feed2: false,
            keep_hud: false,
            f_feed6: false,
            streak_flag: 0,
            feed_value: 0,
            no_count: false,
            revenge: None,
        };
        let a = s.on_kill(1250, &k, &v);
        assert!(matches!(&a[0], Action::Event("target_down", _)));
        assert_eq!(s.stats.kills_by_me, 1);
        assert!(fires(&s.poll(1400, &v)).is_empty());
        assert!(s.look().is_none());
        // BOT-13 (amended): the dead body falls or slides right after the kill: it stays down (the old rule took
        // a 16-unit move at any time for the respawn and fired at the dead body: dserver `victim_dead`)
        v.players[0].pos = Some([0.0, -50.0, 300.0]);
        let a = s.poll(1500, &v);
        assert!(
            !a.iter().any(|x| matches!(x, Action::Event("target_up", _))),
            "{a:?}"
        );
        assert!(fires(&a).is_empty());
        // it respawned elsewhere (after the least respawn time): back at once
        v.players[0].pos = Some([200.0, 50.0, 300.0]);
        let a = s.poll(1250 + RESPAWN_MOVE_AFTER_MS + 50, &v);
        assert!(a.iter().any(|x| matches!(x, Action::Event("target_up", _))));
        assert_eq!(fires(&a).len(), 1);
        // a second kill and no movement: the timer re-arms (the empty clip reloads first)
        let t = 1250 + RESPAWN_MOVE_AFTER_MS + 100;
        s.on_kill(t, &k, &v);
        assert!(fires(&s.poll(t + 4400, &v)).is_empty());
        let a = s.poll(t + 5000, &v);
        assert!(a.iter().any(|x| matches!(x, Action::Event("target_up", _))));
        assert!(a.contains(&Action::Reload(8)), "{a:?}");
        assert_eq!(fires(&s.poll(t + 6100, &v)).len(), 1);
        // out of range (800): no report
        v.players[0].pos = Some([0.0, 50.0, 5000.0]);
        assert!(fires(&s.poll(t + 7400, &v)).is_empty());
        // own death stops everything
        v.players[0].pos = Some([0.0, 50.0, 300.0]);
        s.on_life(t + 7500, 2);
        assert!(s.poll(t + 7900, &v).is_empty());
        assert!(s.look().is_none());
    }

    /// The times (ms) of every shot a shooter holding `weapon` fires while polled each 10 ms for 8 s.
    fn shot_times(rom: Arc<RomWeapons>, weapon: i32, period: u32) -> Vec<u32> {
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![weapon],
            rom: Some(rom),
            shot_period_ms: period,
            ..ShooterConfig::default()
        });
        let v = view(vec![enemy(1, 40, [0.0, 50.0, 300.0])]);
        s.on_life(0, 1);
        let mut out = Vec::new();
        let mut t = 0;
        while t <= 8000 {
            if !fires(&s.poll(t, &v)).is_empty() {
                out.push(t);
            }
            t += 10;
        }
        out
    }

    fn min_gap(times: &[u32]) -> u32 {
        times
            .windows(2)
            .map(|w| w[1] - w[0])
            .min()
            .unwrap_or(u32::MAX)
    }

    #[test]
    fn a_weapon_never_fires_faster_than_its_own_cycle() {
        // BOT-60: a bolt row (cycle 1500 ms) fires no faster than that, whatever the flag period (the floor)
        let mut r = RomWeapons::default();
        r.rows.insert(
            5,
            RomWeapon {
                pellets: Some(1),
                clip: Some(30),
                ammo: Some(6),
                range: Some(5000.0),
                max_amount: Some(90),
                cycle_ms: Some(1500),
                ..RomWeapon::default()
            },
        );
        let t = shot_times(Arc::new(r.clone()), 5, 150);
        assert!(t.len() >= 3, "{t:?}");
        assert!(min_gap(&t) >= 1500, "gap {} in {t:?}", min_gap(&t));
        // the flag period still floors a faster row
        r.rows.get_mut(&5).unwrap().cycle_ms = Some(100);
        let t = shot_times(Arc::new(r), 5, 400);
        assert!(min_gap(&t) >= 400, "{t:?}");
    }

    #[test]
    fn rom_a_server_rom_gives_the_same_weapon_rows() {
        // BOT-82: the bot on a server ROM (env DC_SERVER_ROM, made by dc-server-rom from CA_ROM) reads the same
        // weapon table and the same fire cycles as on the client folder
        let (Some(rom), Some(srom)) = (
            std::env::var_os("CA_ROM"),
            std::env::var_os("DC_SERVER_ROM"),
        ) else {
            println!("skipped: CA_ROM or DC_SERVER_ROM unset");
            return;
        };
        let full = RomWeapons::open(Path::new(&rom)).expect("weapon table");
        let server = RomWeapons::open(Path::new(&srom)).expect("weapon table on the server ROM");
        assert_eq!(full.rows.len(), server.rows.len());
        let mut cycles = 0;
        for (n, w) in &full.rows {
            let s = &server.rows[n];
            assert_eq!(
                (w.cycle_ms, w.clip, w.pellets, w.ammo),
                (s.cycle_ms, s.clip, s.pellets, s.ammo),
                "weapon {n}"
            );
            cycles += w.cycle_ms.is_some() as usize;
        }
        println!(
            "{} rows, {cycles} with a fire cycle, identical",
            full.rows.len()
        );
        assert!(cycles > 100, "{cycles}");
    }

    #[test]
    fn rom_the_sniper_cycles_slower_than_the_automatic_rifle() {
        // weapon 27 (L96A1, bolt) and 145 (automatic rifle): the cycle comes from the weapon's own clips
        let Some(rom) = std::env::var_os("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let table = Arc::new(RomWeapons::open(Path::new(&rom)).expect("weapon table"));
        // BOT-97: the item table marks 27 a sniper rifle and 145 not
        assert!(table.is_sniper(27) && !table.is_sniper(145));
        let sniper = table
            .get(27)
            .and_then(|w| w.cycle_ms)
            .expect("weapon 27 cycle");
        let rifle = table
            .get(145)
            .and_then(|w| w.cycle_ms)
            .expect("weapon 145 cycle");
        println!(
            "cycle ms: weapon 27 = {sniper}, weapon 145 = {rifle}, weapon 164 = {:?}",
            table.get(164).and_then(|w| w.cycle_ms)
        );
        assert!(sniper > 3 * rifle, "{sniper} vs {rifle}");
        let t = shot_times(table.clone(), 27, 150);
        assert!(
            t.len() >= 2 && min_gap(&t) >= sniper,
            "{t:?} cycle {sniper}"
        );
        let t = shot_times(table, 145, 20);
        assert!(min_gap(&t) >= rifle, "{t:?} cycle {rifle}");
    }

    #[test]
    fn loadout_fallback_uses_the_rom_row() {
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![5, -1],
            rom: Some(rom()),
            ..ShooterConfig::default()
        });
        let v = view(vec![enemy(1, 40, [0.0, 50.0, 300.0])]);
        s.on_life(0, 1);
        assert!(s.poll(100, &v).is_empty());
        let a = s.poll(GRANT_WAIT_MS, &v);
        assert!(a.contains(&Action::Select(WeaponChange {
            slot: 0,
            weapon_id: 5,
            aux: 6
        })));
        let h = s.held().unwrap();
        // clip 3 of the ROM row, one shot already out in the select poll; stock = MaxAmount 9 - 3
        assert_eq!((h.source, h.clip, h.stock, h.pellets), ("loadout", 2, 6, 1));
        // no ROM and no grant: no weapon, said once
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![5],
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        let a = s.poll(GRANT_WAIT_MS, &v);
        assert!(matches!(&a[0], Action::Event("weapon_select", _)));
        assert!(s.poll(GRANT_WAIT_MS + 10, &v).is_empty());
    }

    #[test]
    fn rom_rows_join_the_ammunition_by_name() {
        // synthetic rows (made-up names and numbers), one key per line
        let text = [
            "[Ammo0]",
            "Name = \"a\"",
            "MaxAmount = 40",
            "[Ammo1]",
            "Name = \"B\"",
            "MaxAmount = 9",
            "[Weapon3]",
            "AmmoName0 = \"b\"",
            "VectorsPerRound = 6",
            "ShotsPerClip = 8",
            "Range = 900.0",
        ]
        .join("\n");
        let doc = ButeDoc::parse(&text);
        let r = RomWeapons::from_doc(&doc);
        assert_eq!(
            r.get(3),
            Some(&RomWeapon {
                pellets: Some(6),
                clip: Some(8),
                ammo: Some(1),
                range: Some(900.0),
                max_amount: Some(9),
                spread: [None; 4],
                cone: ConeRow::default(),
                cycle_ms: None,
            })
        );
    }

    // ----------------------------------------------------------------------------------------- skill mode

    /// A shooter with `level`, a 100-round clip (weapon 9 of a made-up row) and the walled test world.
    fn skilled(level: u8) -> Shooter {
        let mut r = RomWeapons::default();
        r.rows.insert(
            9,
            RomWeapon {
                pellets: Some(1),
                clip: Some(100),
                ammo: Some(9),
                range: Some(10_000.0),
                max_amount: Some(300),
                spread: [None; 4],
                // made-up bounds: a half-width of 3 thousandths at the spread floor
                cone: ConeRow {
                    min: 3,
                    max: 30,
                    ..ConeRow::default()
                },
                cycle_ms: None,
            },
        );
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(Arc::new(r)),
            skill: Some(Skill::new(level)),
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        s.on_grant(grant(0, 9, 9, 300.0));
        s
    }

    fn walled_view(players: Vec<Seen>, facing: (f32, f32)) -> View {
        let mut v = view(players);
        v.world = Some(crate::hunt::tests::walled());
        v.facing = Some(facing);
        v
    }

    fn event<'a>(a: &'a [Action], name: &str) -> Option<&'a Value> {
        a.iter().find_map(|x| match x {
            Action::Event(n, e) if *n == name => Some(e),
            _ => None,
        })
    }

    /// Polls every `step` ms from `t0` to `t1`, feeding the look back as the facing (what the session does);
    /// returns the time of the first shot and the angle off the aim point then.
    fn run_until_fire(
        s: &mut Shooter,
        v: &mut View,
        t0: Millis,
        t1: Millis,
        step: u32,
    ) -> Option<(Millis, f32)> {
        let mut t = t0;
        while t <= t1 {
            let before = v.facing;
            let a = s.poll(t, v);
            if let (Some(b), Some(l)) = (before, s.look()) {
                // never faster than the turn rate (+ rounding)
                let max = s.turn_rate().unwrap() * step.min(250) as f32 / 1000.0;
                assert!(
                    skill::wrap_diff(l.0, b.0).abs() <= max + 1e-4,
                    "turned {b:?} -> {l:?}"
                );
            }
            v.facing = s.look().or(v.facing);
            if !fires(&a).is_empty() {
                let p = v.players[0].pos.unwrap();
                let eye = [v.own_pos[0], v.own_pos[1] + 33.0, v.own_pos[2]];
                let want = aim_angles(eye, [p[0], p[1] + 25.0, p[2]]);
                return Some((t, skill::angle_between(s.look().unwrap(), want)));
            }
            t += step;
        }
        None
    }

    // BOT-90: the aim point lags a crossing target, so the first shot leaves behind it; a burst climbs the aim
    #[test]
    fn feel_aim_lags_a_crossing_target_and_a_burst_climbs() {
        let mut s = skilled(30);
        s.on_life(0, 1);
        let mut v = walled_view(vec![enemy(1, 40, [-400.0, 50.0, -800.0])], (0.0, 0.0));
        // the target crosses from left to right at 300 units/s (a run), streamed every poll
        let mut t: Millis = 0;
        let mut first: Option<(f32, f32, [f32; 3])> = None;
        let mut pitches = Vec::new();
        while t <= 4000 {
            let x = -400.0 + 300.0 * t as f32 / 1000.0;
            v.players[0].pos = Some([x, 50.0, -800.0]);
            let a = s.poll(t, &v);
            v.facing = s.look().or(v.facing);
            if !fires(&a).is_empty() {
                let look = s.look().unwrap();
                if first.is_none() {
                    first = Some((look.0, look.1, [x, 50.0, -800.0]));
                }
                pitches.push(look.1);
            }
            t += 20;
        }
        let (yaw, _, at) = first.expect("a shot");
        let eye = [0.0, 50.0 + 33.0, 0.0];
        let want = aim_angles(eye, [at[0], at[1] + 25.0, at[2]]);
        // the target moves toward +x; from the eye that is a turn one way in yaw; the lagged aim sits behind it
        let ahead = aim_angles(eye, [at[0] + 50.0, at[1] + 25.0, at[2]]);
        let dir = skill::wrap_diff(ahead.0, want.0).signum();
        let behind = skill::wrap_diff(yaw, want.0) * dir;
        assert!(
            behind < 0.0,
            "the first shot's yaw {yaw} is not behind the target (want {})",
            want.0
        );
        assert!(pitches.len() >= 3, "{pitches:?}");
        // recoil: within a burst the pitch climbs (more negative pitch = up in this convention or positive; the
        // climb is monotone in sign, so the spread of pitches across the burst exceeds the wander alone)
        let span = pitches.iter().cloned().fold(f32::MIN, f32::max)
            - pitches.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            span > Skill::new(30).recoil_deg.to_radians(),
            "pitch span {span}"
        );
    }

    #[test]
    fn skill_turns_its_body_before_it_fires() {
        // skill 100: reaction 200 ms but a half turn at 540 deg/s takes 333 ms: the turn decides the first shot
        let mut s = skilled(100);
        let mut v = walled_view(vec![enemy(1, 40, [0.0, 50.0, -500.0])], (0.0, 0.0));
        let (t, off) = run_until_fire(&mut s, &mut v, 0, 3000, 10).expect("a shot");
        let sk = Skill::new(100);
        let need = ((180.0 - sk.fire_tolerance_deg) / sk.turn_deg_s * 1000.0) as Millis;
        assert!(t >= need, "fired at {t} ms, the turn needs {need} ms");
        assert!(t <= need + 20, "fired late: {t}");
        // BOT-90: the shot may leave off by the feel allowance (the settling overshoot, the wander), never more
        let allow = sk.fire_tolerance_deg + sk.overshoot_cap_deg + 3.0 * sk.wander_sigma_deg;
        assert!(off <= allow.to_radians() + 1e-4, "{off}");
        // the 0x64 yaw follows: the facing at the shot is the turned body, not the start
        assert!(v.facing.unwrap().0.abs() > 3.0, "{:?}", v.facing);
    }

    #[test]
    fn skill_waits_the_reaction_delay_and_a_shorter_reacquire_delay() {
        let sk = Skill::new(50);
        let mut s = skilled(50);
        // already facing the target: only the reaction delay holds the first shot
        let mut v = walled_view(vec![enemy(1, 40, [0.0, 50.0, 500.0])], (0.0, 0.0));
        let a = s.poll(0, &v);
        // BOT-146: the delay is drawn per sighting around the skill's 600 ms
        let first = event(&a, "reaction").unwrap()["delay_ms"].as_u64().unwrap() as u32;
        assert!((250..=1500).contains(&first), "{first}");
        let (t, _) = run_until_fire(&mut s, &mut v, 10, 3000, 10).expect("a shot");
        assert_eq!(t, first.div_ceil(10) * 10);
        // behind the wall for 500 ms: no shot, then the re-acquire delay
        v.players[0].pos = Some([0.0, 50.0, 1200.0]);
        for t in (1000..1500).step_by(10) {
            assert!(
                fires(&s.poll(t, &v)).is_empty(),
                "shot through the wall at {t}"
            );
            v.facing = s.look();
        }
        v.players[0].pos = Some([0.0, 50.0, 500.0]);
        let a = s.poll(1500, &v);
        let r = event(&a, "reaction").expect("a new sighting");
        assert_eq!(r["reacquire"].as_bool(), Some(true));
        let re = r["delay_ms"].as_u64().unwrap() as u32;
        assert!((150..=sk.reaction_ms).contains(&re), "{re}");
        let (t, _) = run_until_fire(&mut s, &mut v, 1510, 5000, 10).expect("a shot");
        assert!(t >= 1500 + re, "{t}");
        // lost for longer than the window: the full reaction again
        v.players[0].pos = Some([0.0, 50.0, 1200.0]);
        for t in (t + 10..t + 2600).step_by(50) {
            assert!(fires(&s.poll(t, &v)).is_empty());
        }
        v.players[0].pos = Some([0.0, 50.0, 500.0]);
        let a = s.poll(t + 2700, &v);
        let r = event(&a, "reaction").expect("a new sighting");
        assert_eq!(r["reacquire"].as_bool(), Some(false));
        assert!((250..=1500).contains(&r["delay_ms"].as_u64().unwrap()));
    }

    // BOT-146: switching away from a target and back within the window is a re-acquire, not a new sighting
    #[test]
    fn a_switch_away_and_back_is_a_reacquire() {
        let mut s = skilled(50);
        // A ahead at 500, B far to the side; both in sight
        let mut v = walled_view(
            vec![
                enemy(1, 40, [0.0, 50.0, 500.0]),
                enemy(2, 41, [800.0, 50.0, 800.0]),
            ],
            (0.0, 0.0),
        );
        let a = s.poll(0, &v);
        assert_eq!(event(&a, "target").unwrap()["pid"], 1);
        let (t0, _) = run_until_fire(&mut s, &mut v, 10, 3000, 10).expect("a shot at A");
        for t in (t0 + 10..3010).step_by(10) {
            s.poll(t, &v);
            v.facing = s.look();
        }
        // B comes much nearer: the bot switches to B
        v.players[1].pos = Some([150.0, 50.0, 100.0]);
        let a = s.poll(3010, &v);
        assert_eq!(event(&a, "target").unwrap()["pid"], 2, "{a:?}");
        let r = event(&a, "reaction").expect("B's reaction");
        // B was in sight for 3 s: the switch's reaction counts from then (a short delay)
        assert_eq!(r["reacquire"].as_bool(), Some(true), "{r}");
        assert_eq!(r["kind"], "switch");
        // B leaves again: back to A within a second, a re-acquire of A
        v.players[1].pos = Some([0.0, 50.0, 1200.0]);
        let mut back = None;
        for t in (3020..4200).step_by(10) {
            let a = s.poll(t, &v);
            v.facing = s.look();
            if let Some(r) = event(&a, "reaction") {
                back = Some(r.clone());
                break;
            }
        }
        let r = back.expect("a reaction to A");
        assert_eq!(r["pid"], 1);
        assert_eq!(
            (r["reacquire"].as_bool(), &r["kind"]),
            (Some(true), &json!("reacquire"))
        );
    }

    // BOT-146: a target behind cover for less than FLICKER_MS stays the target though another is in sight
    #[test]
    fn occlusion_flicker_keeps_the_target() {
        let mut s = skilled(50);
        let mut v = walled_view(
            vec![
                enemy(1, 40, [0.0, 50.0, 500.0]),
                enemy(2, 41, [800.0, 50.0, 800.0]),
            ],
            (0.0, 0.0),
        );
        s.poll(0, &v);
        let (t0, _) = run_until_fire(&mut s, &mut v, 10, 3000, 10).expect("a shot at A");
        for t in (t0 + 10..3010).step_by(10) {
            s.poll(t, &v);
            v.facing = s.look();
        }
        // A steps behind the wall for 300 ms: the target stays A, no switch to B
        v.players[0].pos = Some([0.0, 50.0, 1200.0]);
        for t in (3010..3300).step_by(10) {
            let a = s.poll(t, &v);
            v.facing = s.look();
            assert!(event(&a, "target").is_none(), "switched at {t}");
        }
        // back in sight: a re-acquire of A
        v.players[0].pos = Some([0.0, 50.0, 500.0]);
        let a = s.poll(3300, &v);
        let r = event(&a, "reaction").expect("a reaction");
        assert_eq!(
            (r["pid"].as_u64(), r["reacquire"].as_bool()),
            (Some(1), Some(true))
        );
        // behind the wall for longer than the flicker: the bot takes B
        v.players[0].pos = Some([0.0, 50.0, 1200.0]);
        let mut switched = None;
        for t in (3310..4500).step_by(10) {
            let a = s.poll(t, &v);
            v.facing = s.look();
            if let Some(e) = event(&a, "target") {
                switched = Some((t, e["pid"].clone()));
                break;
            }
        }
        let (t, pid) = switched.expect("a switch to B");
        assert_eq!(pid, 2);
        assert!(t >= 3300 + FLICKER_MS, "{t}");
    }

    /// Fires `n` shots at a target `dist` ahead with the aim exactly on the aim point; returns the RMS angle of
    /// the shot rays off the aim (degrees), the share of hit claims and the reports.
    fn sample(level: u8, dist: f32, n: usize) -> (f32, f32, Vec<FireReport>) {
        let sk = Skill::new(level);
        let mut s = skilled(level);
        let v = walled_view(vec![enemy(1, 40, [dist, 50.0, 0.0])], (0.0, 0.0));
        s.poll(0, &v);
        let h = *s.held().unwrap();
        let t = v.players[0];
        let eye = [0.0, 83.0, 0.0];
        let aim = aim_angles(eye, [dist, 75.0, 0.0]);
        let f0 = forward(aim.0, aim.1);
        let world = v.world.clone().unwrap();
        let (mut sq, mut hits, mut all) = (0.0f64, 0u32, Vec::new());
        for _ in 0..n {
            let (r, k) = s.skill_shot(&v, &t, &h, &world, aim, &sk, 0.0);
            let d = r[0].dir;
            let a = (d[0] * f0[0] + d[1] * f0[1] + d[2] * f0[2])
                .clamp(-1.0, 1.0)
                .acos();
            sq += (a.to_degrees() as f64).powi(2);
            hits += k;
            all.extend(r);
        }
        ((sq / n as f64).sqrt() as f32, hits as f32 / n as f32, all)
    }

    #[test]
    fn skill_aim_error_has_the_stated_spread_and_hits_are_probabilistic() {
        // two axes of N(0, 2.2 deg) plus the 0.17 deg spread floor: RMS about sqrt(2) x 2.2 = 3.11 deg
        let (rms, hit50, shots) = sample(50, 1000.0, 4000);
        assert!((2.85..3.40).contains(&rms), "rms {rms}");
        assert!((0.10..0.40).contains(&hit50), "hit ratio {hit50}");
        let (rms100, hit100, _) = sample(100, 1000.0, 2000);
        assert!((0.45..0.75).contains(&rms100), "rms {rms100}");
        assert!(hit100 > 0.85, "{hit100}");
        let (_, hit0, _) = sample(0, 1000.0, 2000);
        assert!(hit0 < hit50, "{hit0} {hit50}");
        println!("skill 0 / 50 / 100 at 1000 units: hit {hit0} / {hit50} / {hit100}, rms {rms} / {rms100} deg");
        // the two wire forms: a hit carries both blocks (871 bits after the shell id: 887 with the ids), a miss
        // neither (607: 623)
        for f in &shots {
            assert_eq!(f.extra.is_some(), f.hit.is_some());
            let bits = crate::weapon::fire_body(f).bit_len();
            assert_eq!(bits, if f.hit.is_some() { 871 } else { 607 });
            if let (Some(x), Some((d, o))) = (f.extra, f.hit) {
                assert_eq!(o, 40);
                let p = [d * f.dir[0], 83.0 + d * f.dir[1], d * f.dir[2]];
                assert!(len(sub(p, x.a)) < 1e-2);
                assert!(
                    (x.a[0] - 985.0).abs() < 1e-2,
                    "enters the near face {:?}",
                    x.a
                );
            }
        }
    }

    #[test]
    fn skill_never_claims_a_hit_through_a_wall_or_without_a_world() {
        // the aim line meets the wall: no report at all, whatever the aim
        let mut s = skilled(100);
        let mut v = walled_view(vec![enemy(1, 40, [0.0, 50.0, 1200.0])], (0.0, 0.0));
        for t in (0..3000).step_by(10) {
            let a = s.poll(t, &v);
            assert!(fires(&a).is_empty(), "{a:?}");
            v.facing = s.look();
        }
        assert_eq!(s.hunt_goal(), Some([0.0, 50.0, 1200.0]));
        // the aim line passes the wall's edge but perturbed rays may meet it: those are world-hit forms (skill 30: at
        // skill 0 the BOT-23 scaled error hits nothing in 20 s)
        let mut s = skilled(30);
        // BOT-126: within the engage range (1200 would be just beyond it)
        let mut v = walled_view(vec![enemy(1, 40, [420.0, 50.0, 1100.0])], (0.0, 0.0));
        v.own_pos = [420.0, 50.0, 0.0];
        let world = v.world.clone().unwrap();
        let mut claims = 0;
        for t in (0..20_000).step_by(10) {
            for r in fires(&s.poll(t, &v)).into_iter().flatten() {
                if let Some((d, _)) = r.hit {
                    claims += 1;
                    let p = [
                        r.origin[0] + d * r.dir[0],
                        r.origin[1] + d * r.dir[1],
                        r.origin[2] + d * r.dir[2],
                    ];
                    assert!(crate::hunt::line_clear(&*world, r.origin, p));
                }
            }
            v.facing = s.look();
        }
        assert!(claims > 0 && s.stats.miss_reports > 0, "{:?}", s.stats);
        // no world: no shot, said once
        let mut s = skilled(100);
        let mut v = view(vec![enemy(1, 40, [0.0, 50.0, 500.0])]);
        v.facing = Some((0.0, 0.0));
        let mut said = 0;
        for t in (0..3000).step_by(10) {
            let a = s.poll(t, &v);
            assert!(fires(&a).is_empty());
            said += a
                .iter()
                .filter(|x| matches!(x, Action::Event("sight", e) if e["why"] == "no_world"))
                .count();
        }
        assert_eq!(said, 1);
        // a position the stream could not decode is never a target (tdm6: the bot shot at (0, 0, 0))
        let mut s = skilled(100);
        let mut e = enemy(1, 40, [0.0, 50.0, 500.0]);
        e.pos_known = false;
        let v = walled_view(vec![e], (0.0, 0.0));
        let a = s.poll(0, &v);
        assert_eq!(s.target(), None);
        assert_eq!(event(&a, "target_unseen").unwrap()["why"], "no_position");
    }

    #[test]
    fn skill_fires_in_bursts_with_pauses() {
        let sk = Skill::new(50);
        let mut s = skilled(50);
        let mut v = walled_view(vec![enemy(1, 40, [0.0, 50.0, 500.0])], (0.0, 0.0));
        let mut times = Vec::new();
        for t in (0..20_000).step_by(10) {
            if !fires(&s.poll(t, &v)).is_empty() {
                times.push(t);
            }
            v.facing = s.look();
        }
        let gaps: Vec<u32> = times.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.iter().all(|g| *g >= DEFAULT_FIRE_INTERVAL_MS),
            "{gaps:?}"
        );
        let pauses = gaps
            .iter()
            .filter(|g| **g >= sk.burst_pause_ms * 3 / 4)
            .count();
        assert!(pauses >= 5, "{gaps:?}");
        let per_burst = times.len() as f32 / (pauses + 1) as f32;
        assert!(
            (sk.burst_min as f32 - 0.5..sk.burst_max as f32 + 0.5).contains(&per_burst),
            "{per_burst}"
        );
        assert_eq!(s.stats.bursts as usize, pauses + 1);
    }

    /// BOT-161: at a rusher within range the bot holds the trigger (no burst pause: every gap is the shot cycle) and,
    /// the rusher coming head-on, aims at the head band, so more of its hits name the head node (BOT-165); at anyone
    /// else it bursts at the body.
    #[test]
    fn a_rusher_is_fired_at_full_auto_and_head_hits_name_the_head_node() {
        let run = |rushers: Vec<u8>| {
            let mut s = skilled(50);
            s.cfg.zone_nodes = Some(TEST_ZONES);
            let mut v = walled_view(vec![enemy(1, 40, [0.0, 50.0, 400.0])], (0.0, 0.0));
            v.rushers = rushers;
            let (mut times, mut nodes) = (Vec::new(), Vec::new());
            for t in (0..6_000).step_by(10) {
                let a = s.poll(t, &v);
                for f in fires(&a) {
                    times.push(t);
                    nodes.extend(f.iter().filter_map(|r| r.extra.map(|e| e.kind)));
                }
                v.facing = s.look();
            }
            let gaps: Vec<u32> = times.windows(2).map(|w| w[1] - w[0]).collect();
            (gaps, nodes)
        };
        let (gaps, nodes) = run(vec![1]);
        assert!(gaps.len() > 20, "{gaps:?}");
        assert!(gaps.iter().all(|g| *g < 2 * DEFAULT_FIRE_INTERVAL_MS), "{gaps:?}");
        let share = |nodes: &[u8]| {
            nodes.iter().filter(|n| **n == 9).count() as f32 / nodes.len().max(1) as f32
        };
        let rush_head = share(&nodes);
        assert!(rush_head > 0.5, "head hits name the head node: {nodes:?}");
        let (gaps, nodes) = run(Vec::new());
        assert!(gaps.iter().any(|g| *g >= 2 * DEFAULT_FIRE_INTERVAL_MS), "{gaps:?}");
        // (body hits at the centre aim: the BOT-12 test; with BOT-23's error few land in 6 s here)
        assert!(share(&nodes) < rush_head, "{nodes:?}");
    }

    /// BOT-170: against a player strafing left and right at run speed 400 units away (the way a person dodges), the
    /// hit rate by skill on the BOT-170 curve and on the straight lines it replaced: a skill-85 bot tracks the
    /// strafe (lag 30 ms) and hits far more often than before; a beginner hits rarely; the average band as before.
    #[test]
    fn a_skilled_bot_tracks_a_strafing_player() {
        let t = |l: u8| l as f32 / 100.0;
        let lerp = |a: f32, b: f32, l: u8| a + (b - a) * t(l);
        // the straight lines before BOT-170 (BOT-21..24 / BOT-90 / BOT-135)
        let linear = |l: u8| Skill {
            turn_deg_s: lerp(90.0, 540.0, l),
            fire_tolerance_deg: lerp(6.0, 1.5, l),
            reaction_ms: lerp(1000.0, 200.0, l).round() as u32,
            reacquire_ms: lerp(1000.0, 200.0, l).round() as u32 / 2,
            aim_sigma_deg: lerp(4.0, 0.4, l),
            burst_min: lerp(2.0, 6.0, l).round() as u32,
            burst_max: lerp(3.0, 10.0, l).round() as u32,
            burst_pause_ms: lerp(900.0, 300.0, l).round() as u32,
            lag_ms: lerp(280.0, 70.0, l).round() as u32,
            wander_sigma_deg: lerp(2.0, 0.3, l),
            overshoot: lerp(0.30, 0.05, l),
            overshoot_cap_deg: lerp(10.0, 4.0, l),
            recoil_deg: lerp(0.9, 0.25, l),
            moving_factor: lerp(1.9, 1.25, l),
            close_floor_u: lerp(4.0, 0.4, l).to_radians() * skill::FLOOR_RANGE,
            spray_step: lerp(0.08, 0.03, l),
            aim_lift: 0.0,
            weak_ratio: 1.0,
            keep_sight: false,
            ..Skill::new(l)
        };
        let rate = |sk: Skill| {
            let (mut hits, mut shots) = (0usize, 0usize);
            for seed in 0..4u64 {
                let mut s = skilled(sk.level);
                s.cfg.skill = Some(sk);
                s.aim_rng = AimRng::new(seed * 77 + 5);
                let mut v = walled_view(vec![enemy(1, 40, [0.0, 50.0, 400.0])], (0.0, 0.0));
                let (mut x, mut dir) = (0.0f32, 1.0f32);
                for t in (0..20_000u32).step_by(10) {
                    // run speed, the strafe reverses every 450..750 ms
                    if t % (450 + 300 * ((t / 1000) % 2)) < 10 {
                        dir = -dir;
                    }
                    x = (x + dir * 285.0 * 0.01).clamp(-150.0, 150.0);
                    v.players[0].pos = Some([x, 50.0, 400.0]);
                    for f in fires(&s.poll(t, &v)) {
                        shots += f.len();
                        hits += f.iter().filter(|r| r.extra.is_some()).count();
                    }
                    v.facing = s.look();
                }
            }
            hits as f32 / shots.max(1) as f32
        };
        let mut rows = Vec::new();
        for l in [20u8, 50, 80, 85] {
            rows.push((l, rate(linear(l)), rate(Skill::new(l))));
        }
        eprintln!("strafing target at 400 u, hits/shot (level, before, BOT-170): {rows:?}");
        let get = |l: u8| rows.iter().find(|r| r.0 == l).unwrap();
        assert!((get(50).1 - get(50).2).abs() < 0.02, "the average band as before: {rows:?}");
        assert!(get(85).2 > get(85).1 * 1.3, "{rows:?}");
        assert!(get(20).2 < get(50).2 && get(50).2 < get(80).2 && get(80).2 <= get(85).2 + 0.02);
    }

    /// BOT-12: skilled bots (30..65) at a standing target 140..1000 units away aim at the box centre; their shots climb
    /// and scatter so that about 15 % of the hits land in the head band (10..20 %), most of them in the torso.
    #[test]
    fn the_aim_point_puts_a_player_like_share_of_hits_in_the_head() {
        let (mut head, mut hits, mut shots) = (0usize, 0usize, 0usize);
        let mut by_level = Vec::new();
        for level in (30..=65).step_by(5) {
            let (h0, s0) = (hits, shots);
            for d in [138.0f32, 237.0, 397.0, 639.0, 996.0] {
                let mut s = skilled(level);
                s.cfg.zone_nodes = Some(TEST_ZONES);
                let mut v = walled_view(vec![enemy(1, 40, [d, 50.0, 0.0])], (1.0, 0.0));
                for t in (0..20_000).step_by(10) {
                    for f in fires(&s.poll(t, &v)) {
                        shots += f.len();
                        for k in f.iter().filter_map(|r| r.extra.map(|e| e.kind)) {
                            hits += 1;
                            head += usize::from(k == 9);
                        }
                    }
                    v.facing = s.look();
                }
            }
            by_level.push((hits - h0) as f32 / (shots - s0).max(1) as f32);
        }
        let share = head as f32 / hits.max(1) as f32;
        let rate = hits as f32 / shots.max(1) as f32;
        eprintln!("head {head} of {hits} hits, {shots} shots, by level {by_level:?}");
        assert!(hits > 300 && (0.10..=0.22).contains(&share), "{head} of {hits}");
        // BOT-23: about one shot in seven hits (12..18 %), a skill-65 bot more often than a skill-30 one
        assert!((0.12..=0.18).contains(&rate), "{hits} of {shots}");
        assert!(by_level[0] < by_level[by_level.len() - 1], "{by_level:?}");
    }

    /// BOT-165 test table: head 9, torso 6, arms 24, legs 4.
    const TEST_ZONES: ZoneNodes = ZoneNodes {
        nodes: [Some(9), Some(6), Some(24), Some(4)],
    };

    /// BOT-165: a ray's zone is where it passes the box's vertical axis: the top fifth the head, the bottom 0.45 the
    /// legs, between them the torso near the axis and the arms off it; a ray missing the box has none.
    #[test]
    fn the_zone_of_a_ray_follows_the_height_and_offset_where_it_crosses_the_body() {
        let c = [0.0, 50.0, 400.0];
        let h = [16.0, 50.0, 16.0];
        let at = |y: f32, x: f32| zone_of_ray([x, y, 0.0], [0.0, 0.0, 1.0], c, h);
        assert_eq!(at(95.0, 0.0), Some(Zone::Head));
        assert_eq!(at(81.0, 0.0), Some(Zone::Head));
        assert_eq!(at(75.0, 0.0), Some(Zone::Torso));
        assert_eq!(at(50.0, 0.0), Some(Zone::Torso), "the BOT-12 aim point");
        assert_eq!(at(60.0, 5.0), Some(Zone::Torso));
        assert_eq!(at(60.0, 12.0), Some(Zone::Arms), "off the axis: an arm");
        assert_eq!(at(30.0, 0.0), Some(Zone::Legs));
        assert_eq!(at(5.0, 12.0), Some(Zone::Legs));
        assert_eq!(at(120.0, 0.0), None);
        assert_eq!(at(60.0, 20.0), None);
        // a ray from above (a sniper on a roof) judged where it passes the axis, not where it enters the top face
        let o = [0.0, 50.0 + 300.0, 100.0];
        let d = sub([0.0, 70.0, 400.0], o);
        let d = [d[0] / len(d), d[1] / len(d), d[2] / len(d)];
        assert_eq!(zone_of_ray(o, d, c, h), Some(Zone::Torso));
        // straight down onto the top face: the head
        assert_eq!(zone_of_ray([0.0, 300.0, 400.0], [0.0, -1.0, 0.0], c, h), Some(Zone::Head));
    }

    /// BOT-165: per zone the node whose location is that zone in every skeleton the body rows use, the largest radius
    /// first; a zone the skeletons share no node for falls back to row 0's own; no row 0 skeleton: no table.
    /// ROM-gated: the user's ROM gives a node for every zone, and it is that zone in every used skeleton.
    #[test]
    fn the_zone_nodes_are_shared_by_every_body_skeleton() {
        let doc = ButeDoc::parse(concat!(
            "[Model0]\nSkeleton=0\n[Model1]\nSkeleton=1\n[ModelSounds]\nSkeleton=7\n",
            "[Skeleton0]\nNode0Location=0\nNode1Location=1\nNode1Radius=1\nNode2Location=1\nNode2Radius=3\n",
            "Node3Location=2\nNode3Radius=9\nNode4Location=2\nNode4Radius=10\nNode5Location=3\nNode5Radius=5\n",
            "Node6Location=4\nNode6Radius=8\n",
            "[Skeleton1]\nNode0Location=0\nNode1Location=1\nNode2Location=1\nNode3Location=2\nNode4Location=3\n",
            "Node5Location=3\nNode6Location=3\n",
        ));
        let z = ZoneNodes::from_doc(&doc).unwrap();
        assert_eq!(z.node(Zone::Head), Some(2), "both head, the larger radius");
        assert_eq!(z.node(Zone::Torso), Some(3), "node 4 is an arm in skeleton 1");
        assert_eq!(z.node(Zone::Arms), Some(5));
        assert_eq!(z.node(Zone::Legs), Some(6), "no shared leg node: row 0's own");
        assert_eq!(ZoneNodes::from_doc(&ButeDoc::parse("[Model0]\n")), None);
        // without a table every hit names the default node
        let s = Shooter::new(ShooterConfig::default());
        let (o, d) = ([0.0, 95.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(s.claim_node(o, d, [0.0, 50.0, 400.0], [16.0, 50.0, 16.0]), DEFAULT_HIT_NODE);
        if let Some(rom) = std::env::var_os("CA_ROM") {
            let rom = Path::new(&rom);
            let z = ZoneNodes::of_rom(rom).expect("zone table");
            eprintln!("zone nodes {:?}", z.nodes);
            let vfs = RezVfs::new(rom);
            let doc = ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, "ATTRIBUTES\\MODELBUTES.TXT")
                .unwrap()
                .unwrap();
            for zone in [Zone::Head, Zone::Torso, Zone::Arms, Zone::Legs] {
                let k = z.node(zone).expect("a node per zone");
                for s in 0..2 {
                    let loc = doc.get_str(&format!("Skeleton{s}"), &format!("Node{k}Location"));
                    assert_eq!(loc.map(str::trim), Some((zone as u8).to_string().as_str()), "{zone:?} {s}");
                }
            }
        }
    }

    // ----------------------------------------------------------------------------------------- the 2009 cone

    /// The pellets the shooter sends replay through the cone's inverse with the counter the server seeded, a wide
    /// cone's pellet that misses the box goes out in the miss form, the shot scale follows the movement state, the
    /// counter wraps at the table size, and skill mode's pellets replay too.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.4, AC-1.5, AC-2, AC-3.2, AC-3.4, AC-5
    #[test]
    fn pellets_follow_the_2009_cone_from_the_seeded_counter() {
        let mut r = RomWeapons::default();
        r.rows.insert(
            12,
            RomWeapon {
                pellets: Some(8),
                clip: Some(200),
                ammo: Some(12),
                range: None,
                max_amount: Some(400),
                // made-up row: a wide cone and a shot scale below the walk factor's 1.0
                cone: ConeRow {
                    min: 150,
                    max: 300,
                    scale: 0.5,
                    ..ConeRow::default()
                },
                ..RomWeapon::default()
            },
        );
        let rom = Arc::new(r);
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(rom.clone()),
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        s.on_grant(grant(0, 12, 12, 400.0));
        // the server's 0x7d own_value near the table end, then a 0xda kind 1 delta
        let n = dc_cone::TABLE_LEN as u32;
        s.on_shot_seed(n - 9);
        s.on_shot_seed_add(2);
        let mut v = view(vec![enemy(1, 40, [0.0, 50.0, 400.0])]);
        let (mut expect, mut hits, mut misses, mut wrapped) = (n - 7, 0, 0, false);
        let mut t = 0;
        for k in 0..6 {
            // standing for three shots, then walking (ANY_MOVE raises the 0.5 scale to the walk factor 1.0)
            v.moving = MoveState::of_controls(
                if k < 3 {
                    0
                } else {
                    dc_sim::player::bits::ANY_MOVE
                },
                false,
            );
            let a = s.poll(t, &v);
            let f = fires(&a);
            assert_eq!(f.len(), 1, "{a:?}");
            assert_eq!(f[0].len(), 8);
            for p in f[0] {
                let (_, _, after) = dc_cone::draw_pair(s.shot_table(), expect);
                wrapped |= after < expect;
                expect = after;
                assert_eq!(
                    p.stat, expect,
                    "stat = the counter after the pellet's two reads"
                );
                let scale = f32::from_bits(p.shot_no);
                assert_eq!(scale, if k < 3 { 0.5 } else { 1.0 });
                assert_eq!((p.frac, p.moving, p.wflag), (0, false, false));
                let h = dc_cone::replay(s.shot_table(), p.aim, p.dir, p.stat).expect("replays");
                assert_eq!(
                    Some(h),
                    dc_cone::half_width(&rom.rows[&12].cone, false, false, scale, 0.0)
                );
                assert_eq!(p.extra.is_some(), p.hit.is_some());
                if let Some((d, obj)) = p.hit {
                    hits += 1;
                    assert_eq!(obj, 40);
                    let x = p.extra.unwrap().a;
                    let q = [d * p.dir[0], 83.0 + d * p.dir[1], d * p.dir[2]];
                    assert!(
                        len(sub(q, x)) < 1e-2,
                        "the claim is the tilted pellet's own entry point"
                    );
                } else {
                    misses += 1;
                }
            }
            t += DEFAULT_FIRE_INTERVAL_MS;
        }
        assert!(wrapped, "the counter wrapped at the table size");
        assert!(hits > 0 && misses > 0, "hits {hits} misses {misses}");
        assert_eq!((s.stats.hit_claims, s.stats.miss_reports), (hits, misses));
        // the range of a row without one is the binary default
        assert_eq!(s.held().unwrap().range, Some(dc_cone::row_default::RANGE));
        // skill mode: the erred aim is the sent rotation, the cone tilts from it with the spread byte
        let mut s = skilled(50);
        s.on_shot_seed(40);
        let v = walled_view(vec![enemy(1, 40, [800.0, 50.0, 0.0])], (0.0, 0.0));
        s.poll(0, &v);
        let h = *s.held().unwrap();
        let world = v.world.clone().unwrap();
        let sk = Skill::new(50);
        let aim = aim_angles([0.0, 83.0, 0.0], [800.0, 75.0, 0.0]);
        let mut stat = 40;
        for i in 0..50 {
            let f = i as f32 / 50.0;
            let (r, _) = s.skill_shot(&v, &v.players[0], &h, &world, aim, &sk, f);
            stat += 2;
            assert_eq!(r[0].stat, stat);
            assert_eq!(r[0].frac, (f * 255.0) as u8);
            let got = dc_cone::replay(s.shot_table(), r[0].aim, r[0].dir, r[0].stat);
            let want =
                dc_cone::half_width(&h.cone, false, false, 1.0, f32::from(r[0].frac) / 255.0);
            assert_eq!(got, want, "pellet {i}");
        }
    }

    #[test]
    fn a_test_throw_goes_through_the_throwable_row_cone() {
        let mut r = RomWeapons::default();
        r.rows.insert(
            7,
            RomWeapon {
                cone: ConeRow {
                    min: 20,
                    max: 20,
                    ..ConeRow::default()
                },
                ..RomWeapon::default()
            },
        );
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(Arc::new(r)),
            throw_every_ms: Some(1000),
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 9.0));
        s.on_grant(grant(THROW_SLOT, 7, 9, 1.0));
        let mut v = view(Vec::new());
        v.facing = Some((0.4, 0.0));
        s.poll(0, &v);
        let a = s.poll(1000, &v);
        let f = a
            .iter()
            .find_map(|x| match x {
                Action::Throw(f) => Some(*f),
                _ => None,
            })
            .expect("a throw");
        assert_eq!(f.stat, 2);
        assert_eq!(
            dc_cone::replay(s.shot_table(), f.aim, f.dir, f.stat),
            Some(20)
        );
    }

    #[test]
    fn throw_now_throws_once_without_the_option_and_keeps_the_schedule() {
        let mut s = armed();
        assert!(
            s.throw_now(100, &view(Vec::new())).is_none(),
            "no slot-3 grant yet"
        );
        s.on_grant(grant(THROW_SLOT, 40, 41, 2.0));
        let f = s.throw_now(200, &view(Vec::new())).expect("a throw");
        assert_eq!((f.weapon_id, f.ammo_id), (40, 41));
        assert_eq!(s.stats.throws, 1);
        // the periodic option stays off: no throw on later polls
        assert!(!s
            .poll(5000, &view(Vec::new()))
            .iter()
            .any(|a| matches!(a, Action::Throw(_))));
        s.on_life(6000, 3);
        assert!(
            s.throw_now(6100, &view(Vec::new())).is_none(),
            "dead: no throw"
        );
    }

    /// A skill shooter with weapon 5 (3 rounds) in slot 0, weapon 8 (2 rounds) in slot 1, optionally a melee
    /// weapon 0 in slot 2, and a grenade in slot 3.
    fn emptying(melee: bool) -> Shooter {
        let mut r = (*rom()).clone();
        r.rows.insert(
            0,
            RomWeapon {
                pellets: Some(1),
                range: Some(100.0),
                ..RomWeapon::default()
            },
        );
        r.kinds.insert(0, dc_bute::arms::TYPE_MELEE);
        r.kinds.insert(5, 0);
        r.kinds.insert(8, dc_bute::arms::TYPE_PISTOL);
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(Arc::new(r)),
            skill: Some(Skill::new(100)),
            ..ShooterConfig::default()
        });
        s.on_shot_seed(12345);
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 3.0));
        s.on_grant(grant(1, 8, 8, 2.0));
        if melee {
            s.on_grant(grant(2, 0, 0, 1000.0));
        }
        s.on_grant(grant(THROW_SLOT, 40, 41, 1.0));
        s
    }

    /// Polls every 50 ms over `t0..t1`, feeding the look back; returns every action.
    fn run(s: &mut Shooter, v: &mut View, t0: Millis, t1: Millis) -> Vec<Action> {
        let mut all = Vec::new();
        for t in (t0..t1).step_by(50) {
            let a = s.poll(t, v);
            v.facing = s.look().or(v.facing);
            all.extend(a);
        }
        all
    }

    /// BOT-147: a skill bot with a grenade granted in slot 3 and a ROM throw row.
    fn nade_bot(chance: f32) -> Shooter {
        let mut r = (*rom()).clone();
        r.lobs.insert(
            40,
            crate::grenade::LobRow {
                speed: 900.0,
                ..crate::grenade::LobRow::default()
            },
        );
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(Arc::new(r)),
            skill: Some(Skill::new(100)),
            grenade_chance: chance,
            ..ShooterConfig::default()
        });
        s.on_shot_seed(12345);
        s.on_life(0, 1);
        s.on_grant(grant(0, 5, 6, 90.0));
        s.on_grant(grant(THROW_SLOT, 40, 41, 1.0));
        s
    }

    /// Polls every 20 ms over `t0..t1`, feeding the look back; every action with its time.
    fn run_timed(s: &mut Shooter, v: &mut View, t0: Millis, t1: Millis) -> Vec<(Millis, Action)> {
        let mut all = Vec::new();
        for t in (t0..t1).step_by(20) {
            let a = s.poll(t, v);
            v.facing = s.look().or(v.facing);
            all.extend(a.into_iter().map(|x| (t, x)));
        }
        all
    }

    /// BOT-147: an enemy in grenade range is answered with one grenade per life, sent as the client sends a throw:
    /// the select of slot 3, two hold passes after the draw and the wind-up, the release 133 ms into the release clip,
    /// the gun drawn again at once; no rifle shot during the throw; the blast predicted near the enemy.
    #[test]
    fn a_bot_throws_its_grenade_once_per_life_in_the_client_sequence() {
        use crate::grenade as gr;
        let mut s = nade_bot(1.0);
        let mut v = walled_view(vec![enemy(1, 40, [650.0, 50.0, 0.0])], (0.0, 0.0));
        let a = run_timed(&mut s, &mut v, 0, 9000);
        let at = |f: &dyn Fn(&Action) -> bool| -> Vec<(usize, Millis)> {
            a.iter()
                .enumerate()
                .filter(|(_, (_, x))| f(x))
                .map(|(i, (t, _))| (i, *t))
                .collect()
        };
        let sel3 = at(&|x| matches!(x, Action::Select(c) if c.slot == THROW_SLOT));
        let holds = at(&|x| matches!(x, Action::Hold));
        let rel = at(&|x| matches!(x, Action::Release(_)));
        assert_eq!((sel3.len(), holds.len(), rel.len()), (1, 2, 1), "one throw");
        let (i0, t0) = sel3[0];
        assert!(i0 < holds[0].0 && holds[1].0 < rel[0].0);
        assert!(
            holds[0].1 - t0 >= gr::DRAW_MS + gr::WINDUP_MS
                && holds[0].1 - t0 < gr::DRAW_MS + gr::WINDUP_MS + 40
        );
        let rel_due =
            gr::DRAW_MS + gr::WINDUP_MS + gr::HOLD_PASSES * gr::HOLD_PASS_MS + gr::RELEASE_AT_MS;
        assert!(
            rel[0].1 - t0 >= rel_due && rel[0].1 - t0 < rel_due + 40,
            "release at {}",
            rel[0].1 - t0
        );
        // the gun again right after the release; no rifle shot during the throw
        let back = a[rel[0].0..].iter().find_map(|(_, x)| match x {
            Action::Select(c) => Some(c.slot),
            _ => None,
        });
        assert_eq!(back, Some(0));
        assert!(!a[i0..rel[0].0]
            .iter()
            .any(|(_, x)| matches!(x, Action::Fire(_))));
        let Action::Release(f) = &a[rel[0].0].1 else {
            unreachable!()
        };
        assert_eq!(
            (f.weapon_id, f.ammo_id, f.hit, f.last),
            (40, 41, None, true)
        );
        let ev = a
            .iter()
            .find_map(|(_, x)| match x {
                Action::Event("grenade", e) if e["phase"] == "throw" => Some(e.clone()),
                _ => None,
            })
            .expect("grenade event");
        assert!(
            ev["pred_miss"].as_f64().unwrap() < f64::from(gr::MISS_SHARE * 400.0),
            "{ev}"
        );
        assert!(
            ev["self_dist"].as_f64().unwrap() >= f64::from(gr::SELF_SHARE * 400.0),
            "{ev}"
        );
        assert_eq!(s.stats.grenades, 1);
        // the next life throws again only after the cooldown
        s.on_life(9000, 0);
        s.on_life(9500, 1);
        s.on_grant(grant(THROW_SLOT, 40, 41, 1.0));
        let a = run_timed(&mut s, &mut v, 9500, 14_000);
        assert!(
            !a.iter().any(|(_, x)| matches!(x, Action::Release(_))),
            "cooldown"
        );
    }

    /// BOT-147: no throw with a team mate near the enemy, at an enemy too near (the bot inside the blast), or with
    /// the chance 0.
    #[test]
    fn no_grenade_near_a_mate_too_near_or_when_off() {
        let released = |s: &mut Shooter, v: &mut View| {
            run_timed(s, v, 0, 6000)
                .iter()
                .any(|(_, x)| matches!(x, Action::Release(_) | Action::Hold))
        };
        let mut mate = enemy(2, 41, [620.0, 50.0, 60.0]);
        mate.team = 2;
        let mut v = walled_view(vec![enemy(1, 40, [650.0, 50.0, 0.0]), mate], (0.0, 0.0));
        assert!(!released(&mut nade_bot(1.0), &mut v), "mate in the blast");
        let mut v = walled_view(vec![enemy(1, 40, [350.0, 50.0, 0.0])], (0.0, 0.0));
        assert!(!released(&mut nade_bot(1.0), &mut v), "self in the blast");
        let mut v = walled_view(vec![enemy(1, 40, [650.0, 50.0, 0.0])], (0.0, 0.0));
        assert!(!released(&mut nade_bot(0.0), &mut v), "off");
    }

    /// BOT-126: an empty primary is put away for the secondary, an empty secondary for the melee weapon (never the
    /// grenade); with the knife only a near enemy is a target. With no weapon left the bot lets its target go.
    #[test]
    fn an_empty_gun_is_put_away_for_the_next_slot_and_an_unarmed_bot_drops_its_target() {
        let mut s = emptying(true);
        let mut v = walled_view(vec![enemy(1, 40, [300.0, 50.0, 0.0])], (0.0, 0.0));
        let a = run(&mut s, &mut v, 0, 12_000);
        let selects: Vec<u8> = a
            .iter()
            .filter_map(|x| match x {
                Action::Select(c) => Some(c.slot),
                _ => None,
            })
            .collect();
        assert_eq!(selects, [0, 1, 2], "rifle, then the pistol, then the knife");
        let by_weapon = |w: u16| fires(&a).iter().filter(|f| f[0].weapon_id == w).count();
        assert_eq!((by_weapon(5), by_weapon(8)), (3, 2));
        assert_eq!(s.held().map(|h| h.slot), Some(2));
        // the knife: an enemy 300 away is still the target (walked to), one 1000 away is none and no goal
        assert_eq!(s.target(), Some(1));
        assert!(s.hunt_goal().is_some());
        v.players = vec![enemy(1, 40, [1000.0, 50.0, 0.0])];
        run(&mut s, &mut v, 12_000, 12_500);
        assert_eq!((s.target(), s.hunt_goal()), (None, None));
        // a new grant in slot 0 arms the rifle again
        s.on_grant(grant(0, 5, 6, 3.0));
        let a = run(&mut s, &mut v, 12_500, 13_000);
        assert!(a
            .iter()
            .any(|x| matches!(x, Action::Select(c) if c.slot == 0)));

        // no melee weapon: once both guns are empty the bot has no target and no goal
        let mut s = emptying(false);
        let mut v = walled_view(vec![enemy(1, 40, [300.0, 50.0, 0.0])], (0.0, 0.0));
        let a = run(&mut s, &mut v, 0, 12_000);
        assert!(event(&a, "out_of_ammo").is_some());
        assert!(!a
            .iter()
            .any(|x| matches!(x, Action::Select(c) if c.slot == THROW_SLOT)));
        assert_eq!((s.target(), s.hunt_goal(), s.held()), (None, None, None));
    }

    /// BOT-126: a rifle does not fire beyond the engage range: the target is walked toward and, still that far
    /// after 8 s, let go (the bot moves on); within the range it is fought as before.
    #[test]
    fn a_rifle_holds_fire_beyond_the_engage_range_and_lets_a_far_target_go() {
        let mut s = emptying(false);
        s.on_grant(grant(0, 5, 6, 9.0));
        let far = [ENGAGE_RANGE + 600.0, 50.0, 0.0];
        let mut v = walled_view(vec![enemy(1, 40, far)], (0.0, 0.0));
        let a = run(&mut s, &mut v, 0, 3000);
        assert!(fires(&a).is_empty(), "no shot at {}", far[0]);
        assert_eq!(s.target(), Some(1));
        assert_eq!(s.hunt_goal(), Some(far), "walks toward it");
        assert_eq!(s.target_dist(), None);
        let a = run(&mut s, &mut v, 3000, 9000);
        assert!(event(&a, "target_given_up").is_some());
        assert_eq!((s.target(), s.hunt_goal()), (None, None));
        // let go: not taken again while it stays far
        run(&mut s, &mut v, 9000, 12_000);
        assert_eq!(s.target(), None);
        // it comes near: fought at once
        v.players = vec![enemy(1, 40, [600.0, 50.0, 0.0])];
        let a = run(&mut s, &mut v, 12_000, 15_000);
        assert_eq!(s.target(), Some(1));
        assert!(!fires(&a).is_empty());
    }

    /// BOT-135 harness: one engagement of a rifle-shaped row (one pellet, 30-round clip, 9000 range) at `dist` for
    /// `ms`: the target stands or strafes across the line at the run speed (285 u/s, reversing every 700 ms), the
    /// bot stands or moves. Returns (first shot hit, shots, hits).
    fn rifle_engagement(
        sk: Skill,
        dist: f32,
        moving: bool,
        strafe: bool,
        ms: u32,
        seed: u64,
    ) -> (bool, u32, u32) {
        let mut r = RomWeapons::default();
        r.rows.insert(
            21,
            RomWeapon {
                pellets: Some(1),
                clip: Some(30),
                ammo: Some(22),
                range: Some(9000.0),
                max_amount: Some(300),
                cone: ConeRow {
                    min: 3,
                    max: 30,
                    ..ConeRow::default()
                },
                cycle_ms: Some(100),
                ..RomWeapon::default()
            },
        );
        let mut s = Shooter::new(ShooterConfig {
            loadout: vec![21],
            rom: Some(Arc::new(r)),
            skill: Some(sk),
            seed,
            ..ShooterConfig::default()
        });
        s.on_shot_seed(12345);
        s.on_life(0, 1);
        s.on_grant(grant(0, 21, 22, 300.0));
        let mut v = walled_view(vec![enemy(1, 40, [dist, 50.0, 0.0])], (0.0, 0.0));
        v.players[0].dims = Some([24.0, 53.0, 24.0]);
        v.moving.any_move = moving;
        let (mut first, mut shots, mut hits) = (false, 0, 0);
        for t in (0..ms).step_by(50) {
            if strafe {
                // a triangle wave across the line: 285 u/s, turning every 700 ms
                let ph = (t % 1400) as f32 / 1000.0;
                let z = if ph < 0.7 { ph } else { 1.4 - ph } * 285.0 - 100.0;
                v.players[0].pos = Some([dist, 50.0, z]);
            }
            let a = s.poll(t, &v);
            v.facing = s.look().or(v.facing);
            for f in fires(&a) {
                if shots == 0 {
                    first = f[0].hit.is_some();
                }
                shots += 1;
                hits += f.iter().filter(|p| p.hit.is_some()).count() as u32;
            }
        }
        (first, shots, hits)
    }

    /// (first-shot hit %, all-shots hit %) over `n` engagements.
    fn rifle_cell(sk: Skill, dist: f32, moving: bool, strafe: bool, n: u64) -> (f32, f32) {
        let (mut f, mut e, mut sh, mut hi) = (0u32, 0u32, 0u32, 0u32);
        for seed in 1..=n {
            let (a, b, c) = rifle_engagement(sk, dist, moving, strafe, 4000, seed);
            f += a as u32;
            e += (b > 0) as u32;
            sh += b;
            hi += c;
        }
        (
            100.0 * f as f32 / e.max(1) as f32,
            100.0 * hi as f32 / sh.max(1) as f32,
        )
    }

    /// BOT-135: near the target a rifle bot's error has a floor in units, so close shots are no longer near-certain
    /// (stationary bot and target at 150..300 units, with BOT-23's 1.7 error scale: skill 30 14..26 %, skill 50
    /// 24..36 % of the shots, before that scale 30..45 % and 42..60 %; a live match,
    /// where both sides move, lands about half of that), the bot's motion, a strafing target and the spray of a
    /// burst cost more, skill 65 stays clearly better than 30, and beyond
    /// from skill::FLOOR_RANGE on the engagements are nearly what they were. Prints the table (`--nocapture`).
    #[test]
    fn close_range_rifle_aim_has_a_human_floor_and_far_shots_are_unchanged() {
        let n = 300u64;
        let mut rows = Vec::new();
        let mut checks = Vec::new();
        let mut close = std::collections::BTreeMap::new();
        for level in [30u8, 50, 65] {
            let after = Skill::new(level);
            let mut before = after;
            before.close_floor_u = 0.0;
            for dist in [150.0f32, 300.0, 600.0, 900.0, 1150.0] {
                let (bf, ba) = rifle_cell(before, dist, false, false, n);
                let (af, aa) = rifle_cell(after, dist, false, false, n);
                let (_, bmv) = rifle_cell(before, dist, true, false, n);
                let (_, mv) = rifle_cell(after, dist, true, false, n);
                let (_, bst) = rifle_cell(before, dist, true, true, n);
                let (_, st) = rifle_cell(after, dist, true, true, n);
                rows.push(format!(
                    "skill {level}, {dist:>5} u: stationary first {bf:.1} -> {af:.1} %, all {ba:.1} -> {aa:.1} %; \
                     bot moving {bmv:.1} -> {mv:.1} %; bot moving + target strafing {bst:.1} -> {st:.1} %"
                ));
                if dist >= 1100.0 {
                    checks.push((
                        (af - bf).abs() < 3.0 && (aa - ba).abs() < 3.0,
                        format!("far {level} {dist}"),
                    ));
                }
                if dist <= 400.0 {
                    checks.push((mv < aa && st < mv + 2.0, format!("motion {level} {dist}")));
                    close.entry(level).or_insert_with(Vec::new).push(aa);
                }
                if dist == 900.0 {
                    checks.push((
                        aa > ba * 0.75,
                        format!("{level} at 900 stays near: {ba} -> {aa}"),
                    ));
                }
            }
        }
        println!("{}", rows.join("\n"));
        let mean = |l: u8| close[&l].iter().sum::<f32>() / close[&l].len() as f32;
        let (m30, m50, m65) = (mean(30), mean(50), mean(65));
        println!("close (150..300 u) stationary: skill 30 {m30:.1}%, 50 {m50:.1}%, 65 {m65:.1}%");
        for (ok, what) in checks {
            assert!(ok, "{what}");
        }
        assert!((14.0..=26.0).contains(&m30), "skill 30 close {m30}");
        assert!((24.0..=36.0).contains(&m50), "skill 50 close {m50}");
        assert!(m65 > m30 + 15.0, "skill 65 {m65} vs 30 {m30}");
    }
}
