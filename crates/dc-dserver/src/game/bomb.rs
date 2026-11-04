//! Search and Destroy (mode classes 2 / 3): team 1 attacks with one bomb, team 2 defends the world's bomb sites.
//! At each round start one living attacker carries the bomb; he plants it by holding the use key inside a site, a
//! defender defuses it by holding the use key at the bomb; the bomb blows up after its timer.
//!
//! The 2009 client only displays: it asks for a hold with `UP_USE_HOLD` 0xd8 on a plant or defuse point, counts the
//! server's 0xad hold time down and shows the bomb timer, notices and sounds of 0x51 / 0x50 / 0x4f
//! (GAME_MODES_2009.md §4.5, §5.2). The carrier choice, the hold times, the bomb timer, the site extent, the blast
//! and the round-end rules are DeCombat rules (GMD-6, GMD-8, GMD-9; DSV-200..DSV-204).
// SPEC: GAME_MODES_2009.md §4.3 (kind-4 bomb site, kind-5 plant / defuse point), §4.5 (hold to use), §5.2 (teams,
// message order, round end); GAME_MESSAGES_2009_S2C.md §2.2 rows 0x4f / 0x50 / 0x51 / 0x56, §4.4 row 0xad;
// GAME_MESSAGES_2009_C2S.md §7.1, §7.2

use super::combat::DamageSource;
use super::projectile::blast_factor;
use super::{Game, GameOut, Life, RoundPhase};
use crate::gs;
use crate::world::{WorldInfo, ZoneKind};
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;

/// The attacking team (carries the bomb) and the defending team.
// SPEC: GAME_MODES_2009.md §5.2 (the carried-item notice: the bomb item for team 1, the defuse item for team 2,
// cshell+0x82860)
pub const ATTACKERS: u8 = 1;
pub const DEFENDERS: u8 = 2;
/// Hold times of a plant and a defuse.
// DC-DEFAULT: GMD-6 (proposed) plant 4 s, defuse 7 s (no defuse item in DeCombat)
pub const PLANT_MS: u32 = 4000;
pub const DEFUSE_MS: u32 = 7000;
/// The bomb timer after the plant (the 0x51 `bomb_secs`).
// DC-DEFAULT: GMD-9 (proposed) 40 s
pub const BOMB_SECS: u32 = 40;
/// The blast: radius and damage at the centre (falls off with the thrown-blast shape).
// DC-DEFAULT: GMD-9 (proposed) 500 units; DSV-203 (proposed) 1000 damage at the centre, the projectile falloff
pub const BLAST_RADIUS: f32 = 500.0;
pub const BLAST_DAMAGE: f32 = 1000.0;
/// A bomb site is at least this large in each half extent (the zone object's extent property is its half extent).
// DC-DEFAULT: DSV-200 (proposed) 96 units
pub const SITE_MIN_HALF: f32 = 96.0;
/// A defender defuses within this distance of the planted bomb.
// DC-DEFAULT: DSV-202 (proposed) 96 units
pub const DEFUSE_REACH: f32 = 96.0;
/// A hold breaks when the holder moved this far from where it started.
// DC-DEFAULT: GMD-6 (proposed) leaving the zone breaks the hold; DSV-202 (proposed) 48 units; DSV-461 (proposed) a walk past it during the plant
// cancels the plant (cshell+0x275ad0: the 2009 hold ends when the player leaves the target's zone)
pub const HOLD_MOVE: f32 = 48.0;
/// `kind` of 0xad for a plant and a defuse (non-zero, so the client takes it in every state).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 row 0xad (`kind` selects a 32-byte table entry, meaning UNPROVEN)
// DC-DEFAULT: DSV-202 (proposed) 1 plant, 2 defuse (the `z_sub` of the point)
pub const TRACK_PLANT: u8 = 1;
pub const TRACK_DEFUSE: u8 = 2;
/// The weapon id of the bomb's blast in the kill feed (no ROM bomb row is read).
// DC-DEFAULT: DSV-203 (proposed) the fall-death id (no weapon)
pub const BOMB_WEAPON: u16 = super::fall::FALL_DEATH_WEAPON;
/// How long the blast effect object stays on the clients (its group runs 3 s; the object outlives it).
// DC-DEFAULT: DSV-221 (proposed) 5 s
pub const BLAST_FX_MS: u32 = 5000;
/// The inventory slot of the carried bomb item: the 2009 client turns the weapon of inventory entry 9 on when the
/// player enters a plant / defuse point in classes 2 / 3 (entry 8 in the other mode classes).
// EVIDENCE: cshell+0x5d230 (mode-zone enter / leave) -> cshell+0x22d2b0 (classes 2 / 3: the weapon object of
// inventory entry 9, its slot +0x4c(9, 1); classes 1, 4, 6, 7, 8, 10: entry 8), cshell+0x22f2f0 (leave: every entry
// +0x4c(i, 0))
// DC-DEFAULT: DSV-240 (proposed) the bomb is granted in slot 9
pub const BOMB_SLOT: u8 = 9;
/// The key of the spawn-point mode filter of the Search and Destroy sets (lower case).
// OBSERVED: ROM world spawn points (see `Game::bomb_spawn_set`)
pub const SND_SPAWN_FILTER: crate::chars::NameKey = ([64, 40, 185, 148, 151, 1, 17, 130], 6);
/// The GMS keys that name the attackers' mode item: the offence team's name, then `<team>Weapon` of that team.
// OBSERVED: ROM table key GMS (S&D rows, e.g. SnowValley_M) [Object0] OffenceTeam, <team>Weapon (the weapon row
// name of the attackers' item, a Mission-class row with the time-bomb hand model and effects)
pub const GMS_OBJECT_SECTION: &str = "Object0";
pub const GMS_OFFENCE_KEY: &str = "OffenceTeam";
pub const GMS_TEAM_WEAPON_SUFFIX: &str = "Weapon";

/// One bomb site (axis-aligned box round the zone object).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BombSite {
    /// The zone number (A = the lower number).
    pub number: i32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl BombSite {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    pub fn centre(&self) -> [f32; 3] {
        [0, 1, 2].map(|i| (self.min[i] + self.max[i]) / 2.0)
    }
}

/// A running hold to use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub pid: u8,
    pub defuse: bool,
    pub target: u16,
    pub start: Millis,
    pub at: [f32; 3],
}

/// The planted bomb.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Planted {
    pub site: usize,
    /// The planter's body centre at the plant: the defuse reach and the blast are measured from here.
    pub pos: [f32; 3],
    /// Where the bomb lies: on the floor below the planter (DSV-360); the bomb's objects and the blast effect.
    pub floor: [f32; 3],
    pub at: Millis,
    pub by: u8,
}

/// How a bomb round ended (the server log names it; 0x46 has no reason field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BombEnd {
    Exploded,
    Defused,
    /// The round ended by another rule while the bomb was planted (DSV-361).
    Cancelled,
}

/// The Search and Destroy state of a room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SndState {
    pub sites: Vec<BombSite>,
    /// The round this state belongs to (a new round number resets it).
    pub round: u32,
    /// The attackers who hold the bomb item (every living attacker until the plant; nothing is dropped or taken).
    pub carriers: Vec<u8>,
    pub hold: Option<Hold>,
    pub planted: Option<Planted>,
    pub ended: Option<BombEnd>,
    /// The blast is being applied (the elimination rule waits).
    exploding: bool,
    /// The planted bomb's objects are on the clients (DSV-220; kept over the round reset so they are removed).
    pub shown: bool,
    /// The blast effect object is on the clients until then (DSV-221; kept over the round reset).
    pub blast_until: Option<Millis>,
    /// The bomb item's weapon row (DSV-240; read once from the room's GMS, kept over the round reset).
    pub item: Option<u16>,
    pub item_setup: bool,
    /// Per site number: the world effects its planted command switches (name, on), in text order (CLI-1340; kept
    /// over the round reset).
    pub plant_fx: Vec<(i32, Vec<(Vec<u8>, bool)>)>,
    /// A plant switched effects that the next round restores.
    pub fx_switched: bool,
}

impl SndState {
    /// The world file's bomb sites, ordered by zone number.
    // SPEC: GAME_MODES_2009.md §4.3 (kind-4 bomb site zones; `z_value` the zone number)
    // DC-DEFAULT: DSV-200 (proposed) the extent property as half extents, at least SITE_MIN_HALF
    pub fn from_world(world: &WorldInfo) -> Self {
        let mut sites: Vec<BombSite> = world
            .zones
            .iter()
            .filter(|z| z.kind == ZoneKind::BombSite)
            .map(|z| {
                let h = z.dims.map(|d| d.abs().max(SITE_MIN_HALF));
                BombSite {
                    number: z.number,
                    min: [z.pos[0] - h[0], z.pos[1] - h[1], z.pos[2] - h[2]],
                    max: [z.pos[0] + h[0], z.pos[1] + h[1], z.pos[2] + h[2]],
                }
            })
            .collect();
        sites.sort_by_key(|s| s.number);
        // CLI-1340: the effect switches of each site's planted command (statements on other objects are not run)
        let plant_fx = world
            .zones
            .iter()
            .filter(|z| z.kind == ZoneKind::BombSite)
            .map(|z| {
                let fx = crate::doors::parse_statements(&z.planted_command)
                    .iter()
                    .filter(|st| st.delay_ms == 0)
                    .filter(|st| {
                        world
                            .effects
                            .iter()
                            .any(|e| e.name.eq_ignore_ascii_case(&st.target))
                    })
                    .filter_map(|st| {
                        crate::doors::switch_word(st).map(|on| (st.target.clone(), on))
                    })
                    .collect();
                (z.number, fx)
            })
            .collect();
        SndState {
            sites,
            plant_fx,
            ..SndState::default()
        }
    }

    fn site_at(&self, p: [f32; 3]) -> Option<usize> {
        self.sites.iter().position(|s| s.contains(p))
    }
}

/// The bomb item of a GMS row: the weapon row named by `[Object0] <OffenceTeam>Weapon`.
// DC-DEFAULT: DSV-240 (proposed)
pub fn bomb_item_of(
    doc: &dc_bute::ButeDoc,
    weapons: &crate::rom_tables::WeaponTable,
) -> Option<u16> {
    let clean = |v: &str| v.trim().trim_matches('"').trim().to_string();
    let team = clean(doc.get_str(GMS_OBJECT_SECTION, GMS_OFFENCE_KEY)?);
    let w = clean(doc.get_str(
        GMS_OBJECT_SECTION,
        &format!("{team}{GMS_TEAM_WEAPON_SUFFIX}"),
    )?);
    if w.is_empty() {
        return None;
    }
    weapons.by_name_key(crate::chars::name_key(w.as_bytes()))
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether a spawn point's mode filter is the Search and Destroy set (its pinned key, lower case, trimmed).
fn is_snd_filter(filter: Option<&[u8]>) -> bool {
    filter.is_some_and(|f| {
        crate::chars::name_key(f.trim_ascii().to_ascii_lowercase().as_slice()) == SND_SPAWN_FILTER
    })
}

/// Whether a mode filter's capital letters equal `key` (the DSV-50 initials rule for a fixed key).
fn filter_initials_are(filter: Option<&[u8]>, key: &str) -> bool {
    filter.is_some_and(|f| {
        f.iter()
            .copied()
            .filter(u8::is_ascii_uppercase)
            .eq(key.bytes())
    })
}

/// The mode key whose spawn set classes 12 / 13 use on a world without the Search and Destroy set (team deathmatch).
// DC-DEFAULT: DSV-470 (project decision)
pub const ELIM_FALLBACK_SET_KEY: &str = "TDM";

fn site_letter(i: usize) -> char {
    (b'A' + (i.min(25) as u8)) as char
}

impl Game {
    /// A Search and Destroy room (classes 2 / 3).
    pub fn is_bomb(&self) -> bool {
        matches!(self.rules.mode_class, 2 | 3)
    }

    /// Whether a spawn point's mode filter is the Search and Destroy spawn set (classes 2 / 3): the ROM worlds name
    /// it with a text whose initials are not the mode key, so the mode-key rule found none and every team point of
    /// every mode was drawn from (round starts on the far side of the map).
    // OBSERVED: ROM world spawn points: every Search and Destroy world (SnowValley, GrayHammer, SandHog, WarHead,
    // RattleSnake, RapidStream) has a 12 + 12 team set with this filter (key below) beside the TDM and CTF sets
    // DC-DEFAULT: DSV-241 (proposed) classes 2 / 3 spawn on that set
    pub(super) fn bomb_spawn_set(&self, filter: Option<&[u8]>) -> bool {
        self.is_bomb() && is_snd_filter(filter)
    }

    /// An Elimination Pro room (classes 12 / 13).
    pub fn is_elimination_pro(&self) -> bool {
        matches!(self.rules.mode_class, 12 | 13)
    }

    /// Which spawn points (index-aligned with the world's spawn points) are the room's spawn set: the points whose
    /// mode filter names the room's mode key (DSV-59), in classes 2 / 3 also the Search and Destroy set (DSV-241), and
    /// in classes 12 / 13 the Search and Destroy set of the world when it has one, else its team deathmatch set
    /// (DSV-470). The ROM worlds tag no spawn point for Elimination Pro, so the mode-key rule alone found none and
    /// every team point of every mode was drawn from.
    // OBSERVED: ROM world spawn points: no mode filter of any world has the Elimination Pro key as its initials; the
    // six Search and Destroy worlds carry the S&D 12 + 12 team set, the eleven other Elimination Pro worlds a
    // 12 + 12 (Gravedigger 11 + 12) team deathmatch set
    // DC-DEFAULT: DSV-470 (project decision "Elimination Pro needs to have the same spawns as Search and
    // Destroy")
    //
    // A DeCombat `modes` filter (a comma list or one DeCombat mode name, or an empty filter on a `dc.player_spawn`)
    // is decided by the DeCombat rule instead (`Game::dc_mode_serves`): the point serves the room when one of its
    // names serves the room's DeCombat mode name (`BOMB` serves `ELIM` too); in classes 12 / 13 with no TAGGED point
    // serving the room (a point with an empty filter serves every mode but is no set of its own), the points tagged
    // for team deathmatch join the set (the DSV-470 fallback in DeCombat names).
    // DC-DEFAULT: MAP-21
    pub fn spawn_set(&self) -> Vec<bool> {
        let elim = self.is_elimination_pro();
        let has_snd = elim && self.spawn_modes.iter().any(|m| is_snd_filter(m.as_deref()));
        let room = self.dc_mode_name();
        let dc_of = |i: usize| self.spawn_dc.get(i).copied().unwrap_or(false);
        let dc_rule = |i: usize, m: Option<&[u8]>, room: Option<&str>| {
            Self::dc_mode_serves(m, dc_of(i), room)
        };
        let tagged = |m: Option<&[u8]>| m.is_some_and(|t| !t.trim_ascii().is_empty());
        let dc_any = self
            .spawn_modes
            .iter()
            .enumerate()
            .any(|(i, m)| tagged(m.as_deref()) && dc_rule(i, m.as_deref(), room) == Some(true));
        self.spawn_modes
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let m = m.as_deref();
                match dc_rule(i, m, room) {
                    Some(serves) => {
                        serves
                            || (elim
                                && !dc_any
                                && dc_rule(i, m, Some(ELIM_FALLBACK_SET_KEY)) == Some(true))
                    }
                    None => {
                        self.mode_matches(m)
                            || self.bomb_spawn_set(m)
                            || (elim && has_snd && is_snd_filter(m))
                            || (elim && !has_snd && filter_initials_are(m, ELIM_FALLBACK_SET_KEY))
                    }
                }
            })
            .collect()
    }

    pub fn bomb(&self) -> &SndState {
        &self.bomb
    }

    /// A planted bomb holds the round: the clock does not end it and an attacker side without living players does
    /// not lose it (the defenders still have to defuse).
    // DC-DEFAULT: GMD-9 (proposed) a planted bomb still has to be defused; DSV-203 (proposed) no time-out after the
    // plant (the 2009 client hides the round timer at 0x51, GAME_MESSAGES_2009_S2C.md §2.2)
    pub(super) fn bomb_holds_round(&self) -> bool {
        self.is_bomb() && (self.bomb.planted.is_some() || self.bomb.exploding)
    }

    /// The held-bomb mark for one attacker: 0x56 with his own character object (what the top-left bomb picture keys
    /// on, CLI-1190), or the cleared mark.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.2 row 0x56 (the radar marks the object as the own team's objective item)
    // DC-DEFAULT: DSV-350 (proposed, project decision every attacker holds the bomb, so each gets 0x56 with
    // his own object
    fn send_carrier_to(&mut self, pid: u8, held: bool) {
        let item = if held {
            self.players.get(&pid).map(|p| p.obj)
        } else {
            None
        };
        self.send(pid, gs::carried_mark(item));
    }

    /// The mark of every attacker is cleared (the plant, the round reset).
    fn clear_carriers_mark(&mut self) {
        let to: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.introduced && p.team == ATTACKERS)
            .map(|p| p.pid)
            .collect();
        for pid in to {
            self.send_carrier_to(pid, false);
        }
    }

    /// Resets the state for a new round and gives the bomb to a random living attacker.
    // DC-DEFAULT: GMD-8 (proposed) one random living attacker per round
    fn bomb_new_round(&mut self) {
        let sites = std::mem::take(&mut self.bomb.sites);
        let plant_fx = std::mem::take(&mut self.bomb.plant_fx);
        // CLI-1340: the site indicators of the last plant go back to the world's start state
        if self.bomb.fx_switched {
            self.out.push(GameOut::WorldFxRestore);
        }
        if !self.bomb.item_setup {
            self.bomb_item_setup();
        }
        // a bomb item still in an inventory (the round ended before the plant) leaves it
        for c in std::mem::take(&mut self.bomb.carriers) {
            self.take_bomb_item(c);
        }
        self.bomb = SndState {
            sites,
            shown: self.bomb.shown,
            blast_until: self.bomb.blast_until,
            item: self.bomb.item,
            item_setup: true,
            plant_fx,
            round: self.round_no.max(1),
            ..SndState::default()
        };
        self.log(format!(
            "bomb: round {}: every attacker holds the bomb ({} site(s))",
            self.bomb.round,
            self.bomb.sites.len()
        ));
        self.grant_bomb_round();
    }

    /// Every living attacker who does not hold the bomb item yet gets it (round start, a late join); a holder who
    /// died, left or changed side loses it. Nothing is granted once the bomb is planted or the round decided.
    // DC-DEFAULT: DSV-350 (proposed) in 2009 Search and Destroy every attacker carries the bomb (user decision
    // 2026-10-28; the map's S&D GMS lists `JSFWeapon = XK-2` as the attackers' team weapon next to JSFSpawn); no
    // bomb lies in the world and nobody takes one
    fn grant_bomb_round(&mut self) {
        let gone: Vec<u8> = self
            .bomb
            .carriers
            .iter()
            .copied()
            .filter(|c| {
                !self
                    .players
                    .get(c)
                    .is_some_and(|p| p.life == Life::Alive && p.team == ATTACKERS)
            })
            .collect();
        for c in gone {
            self.take_bomb_item(c);
            self.bomb.carriers.retain(|x| *x != c);
        }
        if self.bomb.planted.is_some() || self.bomb.ended.is_some() {
            return;
        }
        let new: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.introduced && p.life == Life::Alive && p.team == ATTACKERS)
            .map(|p| p.pid)
            .filter(|p| !self.bomb.carriers.contains(p))
            .collect();
        for pid in new {
            self.bomb.carriers.push(pid);
            self.give_bomb_item(pid);
            self.send_carrier_to(pid, true);
        }
    }

    /// The bomb item of the room (once): the GMS row of the room's mode and world names the offence team and that
    /// team's weapon; the weapon row of that name is the item. Without a GMS or a row of that name there is no item
    /// (the plant still works with the use key alone).
    // DC-DEFAULT: DSV-240 (proposed) the attackers' GMS weapon is the carried bomb
    fn bomb_item_setup(&mut self) {
        self.bomb.item_setup = true;
        let doc = self
            .vfs
            .as_ref()
            .and_then(|v| crate::coop_rom::find_gms(v, self.room.mode_key(), &self.world_announce));
        self.bomb.item = doc
            .as_ref()
            .and_then(|d| bomb_item_of(d, &self.tables.weapons));
        self.log(match self.bomb.item {
            Some(w) => format!("bomb: the bomb item is weapon {w} (slot {BOMB_SLOT})"),
            None => "bomb: no bomb item (no GMS weapon row): the use key plants".to_string(),
        });
    }

    /// The carrier gets the bomb item: `GS_WEAPON_GIVE` in [`BOMB_SLOT`]; his class key of the item's order class
    /// draws it.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 row 0xb2; GAME_MODES_2009.md §5.2 step 1 (0xb2 bomb item to the carrier)
    // DC-DEFAULT: DSV-240 (proposed)
    fn give_bomb_item(&mut self, pid: u8) {
        let Some(item) = self.bomb.item else {
            return;
        };
        let state = super::WeaponState {
            slot: BOMB_SLOT,
            ..self.new_weapon(item, None)
        };
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        if p.weapons.iter().any(|w| w.weapon == item) {
            return;
        }
        p.weapons.push(state);
        let spec = gs::WeaponSpec {
            weapon: state.weapon,
            ammo_kind: state.ammo,
            ammo: state.told_amount(),
            clip: state.clip,
            class_pos: 0,
            slot: BOMB_SLOT,
            parts: gs::NO_PARTS,
        };
        self.send(pid, gs::weapon_give(&spec));
    }

    /// The bomb item leaves the player's inventory (`GS_WEAPON_REMOVE`); when it was drawn, his first weapon is drawn
    /// again (`GS_WEAPON_SWITCH` with `auto`).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 rows 0xb4, 0x7f
    // DC-DEFAULT: DSV-240 (proposed)
    fn take_bomb_item(&mut self, pid: u8) {
        let Some(item) = self.bomb.item else {
            return;
        };
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        // a spawn set since the grant replaced the server's list: the client's slot still holds the item
        let (ammo, was_drawn) = match p.weapons.iter().position(|w| w.weapon == item) {
            Some(i) => {
                let w = p.weapons.remove(i);
                let was_drawn = p.current == Some(i);
                p.current = match p.current {
                    Some(c) if c == i => (!p.weapons.is_empty()).then_some(0),
                    Some(c) if c > i => Some(c - 1),
                    c => c,
                };
                (w.ammo, was_drawn)
            }
            None => (self.tables.weapons.ammo_of(item).unwrap_or(0), false),
        };
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        let alive = p.life == Life::Alive;
        let first = p.weapons.first().map(|w| w.weapon);
        self.send(pid, gs::weapon_remove(BOMB_SLOT, item, ammo));
        if was_drawn && alive {
            if let Some(f) = first {
                if let Some(c) = self.tables.weapons.command_of(f) {
                    self.send(pid, gs::weapon_switch(c, true, -1.0, 0));
                }
                self.hand_weapon_changed(pid, f);
            }
        }
    }

    /// The bomb rules of one tick (classes 2 / 3, round running).
    // DC-DEFAULT: DSV-201 (proposed) carrier, drop and take; DSV-202 (proposed) hold checks; DSV-203 (proposed) the
    // timer and the blast
    pub(super) fn tick_bomb(&mut self, now: Millis) {
        self.tick_bomb_rules(now);
        self.sync_bomb_objects(now);
    }

    /// The planted bomb's objects follow the state: shown while a bomb is planted (the bomb model, its indicator
    /// effect and the defuse point at it), removed when it is defused, blew up or a new round began; the blast effect
    /// object leaves after [`BLAST_FX_MS`].
    // SPEC: GAME_MODES_2009.md §5.2 step 3 (the defuse zone at the bomb and PLAUSIBLY a world object of the bomb)
    // DC-DEFAULT: DSV-220 (proposed) the bomb objects; DSV-221 (proposed) the blast effect's lifetime
    fn sync_bomb_objects(&mut self, now: Millis) {
        match (self.bomb.planted, self.bomb.shown) {
            (Some(pl), false) => {
                self.bomb.shown = true;
                let site = self.bomb.sites.get(pl.site).map_or(0, |s| s.number);
                self.out.push(GameOut::BombShown {
                    site,
                    pos: pl.floor,
                });
            }
            (None, true) => {
                self.bomb.shown = false;
                self.out.push(GameOut::BombHidden);
            }
            _ => {}
        }
        if self.bomb.blast_until.is_some_and(|u| !later(u, now)) {
            self.bomb.blast_until = None;
            self.out.push(GameOut::BombBlastGone);
        }
    }

    fn tick_bomb_rules(&mut self, now: Millis) {
        // DC-DEFAULT: DSV-512 (proposed) no bomb in the warm-up (the carrier is chosen at round 1)
        if !self.is_bomb() || !matches!(self.phase, RoundPhase::Running { .. }) || self.in_warmup() {
            self.bomb.hold = None;
            return;
        }
        if self.bomb.round != self.round_no.max(1) {
            self.bomb_new_round();
        }
        self.grant_bomb_round();
        self.tick_hold(now);
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        if let Some(pl) = self.bomb.planted {
            if since(now, pl.at) >= BOMB_SECS * 1000 {
                self.bomb_explode(pl, now);
            }
        }
    }

    /// The running hold: broken by death, leaving, moving away or the bomb state; completed after its time.
    fn tick_hold(&mut self, now: Millis) {
        let Some(h) = self.bomb.hold else {
            return;
        };
        let ok = match self.players.get(&h.pid) {
            Some(p) => p.life == Life::Alive && dist(p.pos, h.at) <= HOLD_MOVE,
            None => false,
        } && (h.defuse == self.bomb.planted.is_some());
        if !ok {
            self.bomb.hold = None;
            self.release_hold_freeze(h.pid);
            self.log(format!(
                "bomb: client {} {} broken off",
                h.pid,
                if h.defuse { "defuse" } else { "plant" }
            ));
            return;
        }
        let need = if h.defuse { DEFUSE_MS } else { PLANT_MS };
        if since(now, h.start) < need {
            return;
        }
        self.bomb.hold = None;
        self.release_hold_freeze(h.pid);
        if h.defuse {
            self.bomb.ended = Some(BombEnd::Defused);
            self.bomb.planted = None;
            self.log(format!(
                "bomb: defused by client {} (round won by the defenders)",
                h.pid
            ));
            self.broadcast(&gs::defused_notice());
            self.end_round(now, DEFENDERS);
        } else {
            let pos = self.players.get(&h.pid).map_or(h.at, |p| p.pos);
            let site = self.bomb.site_at(pos).unwrap_or(0);
            let floor = self.bomb_floor(h.pid, pos);
            self.bomb.planted = Some(Planted {
                site,
                pos,
                floor,
                at: now,
                by: h.pid,
            });
            for c in std::mem::take(&mut self.bomb.carriers) {
                self.take_bomb_item(c);
            }
            self.log(format!(
                "bomb: planted at site {} by client {} ({} s)",
                site_letter(site),
                h.pid,
                BOMB_SECS
            ));
            self.broadcast(&gs::planted_notice(BOMB_SECS));
            self.clear_carriers_mark();
            self.switch_plant_fx(site);
        }
    }

    /// The planted site's command switches the world's site indicators: the green ones of every site off, the red one
    /// of this site on; they stay so until the next round starts.
    // OBSERVED: the ROM worlds' bomb-site trigger objects carry a planted command (property `PROP_PLANTED_COMMAND`)
    // whose `msg <effect> on|off` statements switch the green indicator effects of both sites off and the red one of
    // the planted site on (the red ones start off; the round-start triggers switch the green ones on, the round-end
    // triggers every indicator off)
    // DC-DEFAULT: CLI-1340 (proposed, playtest report "the planted area colour should turn from green to red while the
    // bomb counts down") the server runs the effect statements of the planted command; the next round restores
    fn switch_plant_fx(&mut self, site: usize) {
        let Some(number) = self.bomb.sites.get(site).map(|s| s.number) else {
            return;
        };
        let fx: Vec<(Vec<u8>, bool)> = self
            .bomb
            .plant_fx
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, f)| f.clone())
            .unwrap_or_default();
        for (name, on) in fx {
            self.bomb.fx_switched = true;
            self.out.push(GameOut::WorldFxSwitch { name, on });
        }
    }

    /// Where the planted bomb lies: on the floor below the planter's centre (the world trace), else at his feet
    /// (his centre less his half height). The bomb model's origin is its underside, so it lies on that point with no
    /// lift.
    // OBSERVED: ROM bomb model of DSV-220 (bind-pose LOD 0 vertices from -0.01 to 8.0 units above its origin on the
    // up axis, 16 x 20 units across)
    // DC-DEFAULT: DSV-360 (proposed, playtest report "the planted bomb is floating above the ground": it was drawn at the
    // planter's body centre, about half a body above the floor)
    fn bomb_floor(&self, pid: u8, centre: [f32; 3]) -> [f32; 3] {
        let y = self
            .floor_below(centre)
            .unwrap_or(centre[1] - self.dims_of(pid)[1]);
        [centre[0], y, centre[2]]
    }

    /// `UP_USE_HOLD`: phase 1 starts a plant (the carrier inside a site, no bomb planted) or a defuse (a defender at
    /// the planted bomb) and answers with 0xad and the hold time; any other phase ends the player's hold.
    // SPEC: GAME_MODES_2009.md §4.5 steps 2, 3, 5 (phase 1 / 4, the server's 0xad with the hold time)
    // DC-DEFAULT: GMD-6 (proposed); DSV-202 (proposed) the checks, a refused hold gets no answer
    pub(super) fn on_use_hold(&mut self, pid: u8, phase: u8, target: u16, now: Millis) {
        if !self.is_bomb() {
            return;
        }
        if phase != 1 {
            if self.bomb.hold.is_some_and(|h| h.pid == pid) {
                self.bomb.hold = None;
                self.release_hold_freeze(pid);
                self.log(format!("bomb: client {pid} let go (phase {phase})"));
            }
            return;
        }
        if !matches!(self.phase, RoundPhase::Running { .. }) || self.bomb.hold.is_some() {
            return;
        }
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.life != Life::Alive {
            return;
        }
        let (team, pos) = (p.team, p.pos);
        let defuse = match (team, self.bomb.planted) {
            (ATTACKERS, None) if self.bomb.carriers.contains(&pid) => {
                if self.bomb.site_at(pos).is_none() {
                    self.log(format!("bomb: client {pid} tried to plant outside a site"));
                    return;
                }
                false
            }
            (DEFENDERS, Some(pl)) if dist(pos, pl.pos) <= DEFUSE_REACH => true,
            _ => {
                self.log(format!(
                    "bomb: client {pid} (team {team}) may not use target {target}"
                ));
                return;
            }
        };
        self.bomb.hold = Some(Hold {
            pid,
            defuse,
            target,
            start: now,
            at: pos,
        });
        let (kind, ms) = if defuse {
            (TRACK_DEFUSE, DEFUSE_MS)
        } else {
            (TRACK_PLANT, PLANT_MS)
        };
        self.log(format!(
            "bomb: client {pid} starts to {} ({} ms)",
            if defuse { "defuse" } else { "plant" },
            ms
        ));
        self.send(pid, gs::track_object(target, kind, ms as f32 / 1000.0));
        self.freeze_for_hold(pid);
    }

    /// `UP_USE_PROGRESS_END`: a stop with time left ends the hold; a run-out leaves the decision to the server clock.
    // SPEC: GAME_MESSAGES_2009_C2S.md §7.2 (`remaining` 0 = ran out); DC-DEFAULT: DSV-202 (proposed)
    pub(super) fn on_use_progress_end(
        &mut self,
        pid: u8,
        _target: u16,
        remaining: f32,
        now: Millis,
    ) {
        if remaining > 0.05 {
            self.on_use_hold(pid, 4, 0, now);
        }
    }

    /// The round ended by any other rule (the defenders out, a team gone, the match goal, the clock) while the bomb was
    /// planted or a hold ran: the bomb is cancelled at once. No blast, no 0x4f and no win follow, its objects leave the
    /// clients now (not at the next round start) and the next round starts without a bomb. A hold that ran keeps the
    /// round-end stop (DSV-300).
    // DC-DEFAULT: DSV-361 (proposed, playtest report "if the round ends before the bomb goes off it should stop counting
    // down on the round end screen, or at least make sure it doesn't affect the round"); GMD-9 unchanged: dead
    // attackers alone never end a planted round
    pub(super) fn bomb_cancel_at_round_end(&mut self, now: Millis) {
        if !self.is_bomb() {
            return;
        }
        self.bomb.hold = None;
        if let Some(pl) = self.bomb.planted.take() {
            self.bomb.ended = Some(BombEnd::Cancelled);
            self.log(format!(
                "bomb: the round ended {} s after the plant at site {}: bomb cancelled",
                since(now, pl.at) / 1000,
                site_letter(pl.site)
            ));
        }
        self.sync_bomb_objects(now);
    }

    /// The timer ran out: 0x4f to everyone, the blast, the round to the attackers.
    // SPEC: GAME_MODES_2009.md §5.2 step 4 (0x4f and damage to players in range)
    // DC-DEFAULT: GMD-9 (proposed); DSV-203 (proposed)
    fn bomb_explode(&mut self, pl: Planted, now: Millis) {
        self.bomb.planted = None;
        self.bomb.ended = Some(BombEnd::Exploded);
        self.bomb.exploding = true;
        self.broadcast(&gs::exploded_notice());
        // DC-DEFAULT: DSV-221 (proposed) the blast effect at the bomb, on every client
        self.out.push(GameOut::BombBlast { pos: pl.floor });
        self.bomb.blast_until = Some(now.wrapping_add(BLAST_FX_MS));
        let inner = self.cfg.projectile.inner_fraction;
        let victims: Vec<(u8, [f32; 3])> = self
            .players
            .values()
            .filter(|v| v.life == Life::Alive && v.introduced)
            .map(|v| (v.pid, v.pos))
            .collect();
        let mut hit = 0;
        for (vpid, vpos) in victims {
            let d = dist(vpos, pl.pos);
            if !(d.is_finite() && d < BLAST_RADIUS) {
                continue;
            }
            let dmg = BLAST_DAMAGE * blast_factor(d, BLAST_RADIUS, inner);
            if !(dmg.is_finite() && dmg > 0.0) {
                continue;
            }
            let dir = {
                let v = [
                    vpos[0] - pl.pos[0],
                    vpos[1] - pl.pos[1],
                    vpos[2] - pl.pos[2],
                ];
                let l = dist(vpos, pl.pos);
                if l > 0.001 {
                    v.map(|x| x / l)
                } else {
                    [0.0, 1.0, 0.0]
                }
            };
            let src = DamageSource {
                weapon: BOMB_WEAPON,
                dtype: super::TODO_DAMAGE_ZONE,
            };
            self.apply_damage(vpid, None, dmg, dir, src, super::TODO_KILL_ZONE_BODY, now);
            hit += 1;
        }
        self.bomb.exploding = false;
        self.log(format!(
            "bomb: exploded at site {}: {hit} player(s) in {BLAST_RADIUS:.0} (round won by the attackers)",
            site_letter(pl.site)
        ));
        if matches!(self.phase, RoundPhase::Running { .. }) {
            self.end_round(now, ATTACKERS);
        }
    }
}
