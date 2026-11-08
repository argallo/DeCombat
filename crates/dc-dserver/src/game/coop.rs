//! Co-op (Fireteam, mode class 5) slice 1: AI characters as kind-1 character objects with the NPC tail, a pool of
//! them hidden and reused (FT-R1), waves from the world's spawners and the GMS spawn tables, a first behaviour
//! (walk to the nearest living human along the line of sight or the world's AI nodes, shoot within range), hits on
//! AI by the client's report and by a server-side ray test, the NPC health bar, kill credit and points through
//! `GS_COOP`, the class-5 lives rule and the co-op result block.
//!
//! The client-side facts are `rust/specs/FIRETEAM_2009.md` (cited `FIRETEAM §n`); the AI's behaviour is ours
//! (FIRETEAM covers nothing of it). Values the ROM has are read at run time (`coop_rom`, `world::CoopWorld`); every
//! other value is a DeCombat default: FT-n rows of `DC_DEFAULTS.md` are PROPOSED and read with the corrections
//! the overturned points of `docs/FIRETEAM_AI_DESIGN.md` section 10; FT-S.. rows are staged by this slice
//! (`DC_DEFAULTS.md`).

use std::collections::{BTreeMap, VecDeque};

use super::combat::DamageSource;
use super::{finite3, len3, sub3, AnimSet, CharacterLook, Game, GameOut, Life, Motion, RoundPhase};
use crate::chars::{self, CharacterAssets, Rig};
use crate::coop_rom::{self, AiBodyRow, AiTables, CoopPlan};
use crate::gs;
use crate::stream::StreamEntry;
use crate::up::UpFire;
use crate::world::CoopWorld;
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;

mod nav;
mod recovery;
mod specials;
use nav::{NavGoal, NavMove};
pub use nav::{NavSlot, NavStats};
pub use recovery::heal_share;
pub use specials::{Cloud, DeathSpecial, Growth};

/// Team of the humans and of the AI characters: the NPC bar is drawn only for entries of another team than the
/// own (FIRETEAM §1.4 draw rule).
// SPEC: FIRETEAM_2009.md §1.4 (cshell+0x8a420: class 5 and entry team != own team)
// DC-DEFAULT: GMD-15 (proposed) humans team 1, AI team 2
pub const HUMAN_TEAM: u8 = 1;
pub const AI_TEAM: u8 = 2;
/// NPC kind bytes: 1 = ordinary AI with a health bar, 2 = boss (its own radar and map icon kinds, bar shown).
// SPEC: FIRETEAM_2009.md §1.3 step 4, §1.5 (only kinds 1 and 2 show the bar)
// DC-DEFAULT: FT-K1 (proposed; corrects FT-14 / F4 of the design, D1)
pub const NPC_KIND_NORMAL: u8 = 1;
pub const NPC_KIND_BOSS: u8 = 2;
/// Result banners of the co-op block: complete, failed, timed out.
// SPEC: FIRETEAM_2009.md §2.6 (cshell+0x76ee0: 2 -> complete, 3 / 4 -> failed, 5 -> timed out; PROVEN by key order)
pub const STAGE_COMPLETE: u8 = 2;
pub const STAGE_FAILED: u8 = 3;
pub const STAGE_TIMEOUT: u8 = 5;
/// States of a mission goal in the goal list.
// DC-DEFAULT: DSV-574 (proposed; the client's meaning of the state byte is UNPROVEN, V-FT-8)
pub const GOAL_OPEN: u8 = 0;
pub const GOAL_DONE: u8 = 1;
pub const GOAL_FAILED: u8 = 2;
/// What `healinpart` gives when the difficulty row has no `RecoverRoundTotalHP`.
// DC-DEFAULT: DSV-578 (proposed) 100
pub const RECOVER_HP_DEFAULT: f32 = 100.0;

/// `healinpart` for one human: `pool` points, health first up to `max_health`, the rest as armour up to `max_armor`.
/// Neither value goes down.
// DC-DEFAULT: DSV-578 (proposed)
pub fn heal_in_part(
    health: f32,
    armor: f32,
    pool: f32,
    max_health: f32,
    max_armor: f32,
) -> (f32, f32) {
    let pool = pool.max(0.0);
    let to_health = (max_health - health).clamp(0.0, pool);
    let to_armor = (max_armor - armor).clamp(0.0, pool - to_health);
    (health + to_health, armor + to_armor)
}

/// The caption of an AI whose ROM row has no display name.
// DC-DEFAULT: FT-S3 (proposed)
pub const FALLBACK_CAPTION: &[u8] = b"AI";

/// A dead AI's drop rows (item word, chance in percent), its position and its box (DSV-591).
pub type AiDropRows = (Vec<(String, f32)>, [f32; 3], [f32; 3]);
/// Reach of a melee AI whose brain row and weapon row give none, and the share of its reach it closes to before it
/// stops (it strikes from there while the target stays within reach).
// DC-DEFAULT: DSV-589 (proposed) 100 units (the ROM brains' common melee range), two thirds
pub const MELEE_REACH_DEFAULT: f32 = 100.0;
pub const MELEE_CLOSE: f32 = 2.0 / 3.0;

/// Co-op rule values (`-coop key=value,...`). Every value is a DeCombat default.
#[derive(Debug, Clone, PartialEq)]
pub struct CoopTuning {
    /// AI pool size = the most AIs alive at once (each AI keeps two object ids for the match).
    // DC-DEFAULT: FT-2 (proposed: 32); FT-S4 (proposed) 12 for slice 1
    pub max_alive: u32,
    /// Lives of a human counting the first one; `None` = the difficulty row's `LifeCount`, else `default_lives`.
    // DC-DEFAULT: FT-5 (proposed) ROM `LifeCount`, 1 when absent; operator override
    pub lives: Option<u32>,
    pub default_lives: u32,
    /// Waves of the synthetic plan (worlds without spawn tables): wave n (1-based) has `4 + 2 n` AIs, x 1.5 on the
    /// highest difficulty.
    // DC-DEFAULT: GMD-17 (proposed) 5 waves; FT-S1 (proposed) the synthetic plan uses the world's spawners
    pub waves: u32,
    /// The first wave starts this long after the round started; later waves `wave_break_ms` after the last one was
    /// cleared.
    // DC-DEFAULT: FT-S1 (proposed) 5000 ms; FT-10 (proposed) break 10 s
    pub first_wave_ms: u32,
    pub wave_break_ms: u32,
    /// One AI per this interval in the synthetic plan (the ROM tables carry their own `SpawnDelay`).
    // DC-DEFAULT: FT-S1 (proposed) 1000 ms
    pub spawn_interval_ms: u32,
    /// A spawner within this distance of a living human is skipped.
    // DC-DEFAULT: FT-10 (proposed) 256 units
    pub spawn_clear: f32,
    /// A dead AI's corpse lies this long; then the corpse is removed and the AI's slot (its hidden objects) may be
    /// reused (FT-R1, DSV-585).
    // DC-DEFAULT: FT-4 (proposed) 3000 ms
    pub corpse_ms: u32,
    /// Walk speed (units per second), approach stop distance, firing range and interval, hit chance per difficulty
    /// (0, 1, 2) at short range (half of it at the end of the range), shot damage without a ROM weapon row.
    // DC-DEFAULT: FT-S5 (proposed) 200 u/s, stop at 400, range 1500, 700 ms, 0.2 / 0.3 / 0.4, 8 damage
    pub speed: f32,
    pub hold_range: f32,
    pub shot_range: f32,
    pub fire_ms: u32,
    pub hit_chance: [f32; 3],
    pub default_damage: f32,
    /// Hit chance of a melee strike of an AI whose brain or weapon fights in melee only (DSV-589), every difficulty.
    // DC-DEFAULT: DSV-589 (proposed) 1.0
    pub melee_hit: f32,
    /// AI health without a ROM value.
    // DC-DEFAULT: FT-12 (proposed) template HitPoints, else the placed object's MaxHitPoints, else 100
    pub default_health: f32,
    /// Where an AI's health maximum comes from: its kind's attribute row first (`rows`), or the placed object's
    /// `MaxHitPoints` first (`world`).
    // DC-DEFAULT: DSV-603 (proposed) rows
    pub ai_health: AiHealthSource,
    /// AI step period.
    // DC-DEFAULT: FT-S5 (proposed) 50 ms
    pub step_ms: u32,
    /// Accept hits on AI found by a server-side ray test of the shot as well as by the client's `hit_obj`.
    // SPEC: FIRETEAM_2009.md §1.8 (V-FT-7: that the client reports an AI as `hit_obj` is UNPROVEN)
    // DC-DEFAULT: FT-S6 (proposed) on
    pub ray_hits: bool,
    /// `npc_rank` of every AI (a rank-icon index, FIRETEAM §1.4).
    // DC-DEFAULT: FT-1 (proposed) 0
    pub npc_rank: u8,
    /// The AI plan their way over the navigation grid and walk with their body (off: FT-S5's steering only).
    // DC-DEFAULT: FT-S11 (proposed) on
    pub nav: bool,
    /// `healinpart` splits the difficulty row's `RecoverRoundTotalHP` among the living humans (off: each gets all).
    // DC-DEFAULT: DSV-593 (proposed) on
    pub heal_split: bool,
    /// The kinds' specials: the walk-then-rush (DSV-597) and what a dead AI's brain row sets off (DSV-598..DSV-600).
    // DC-DEFAULT: DSV-597..DSV-600 (proposed) on
    pub specials: bool,
    /// A dead AI's dropped item goes off when a living human comes this close to it, once it has lain this long
    /// (else at its fuse).
    // DC-DEFAULT: DSV-599 (proposed) 1000 ms, 96 units
    pub growth_arm_ms: u32,
    pub growth_trigger: f32,
    /// An AI fires in its weapon row's bursts: a burst's shots `fire_ms` apart, then the row's pause (DSV-640; off,
    /// or a row without the burst keys: a shot every `fire_ms`).
    // DC-DEFAULT: DSV-640 (proposed) on
    pub bursts: bool,
    /// A ranged AI's shot always hits a human within its attribute row's `FullAccuracyRadius` (DSV-641).
    // DC-DEFAULT: DSV-641 (proposed) on
    pub full_accuracy: bool,
    /// On a mission world a spawn that waits for a free slot takes the slot of a living AI left behind: farther than
    /// `retire_range` from every human, out of their sight, alive at least `retire_age_ms`, met by a human before
    /// (or unarmed), its template without a death command (DSV-642; `retire_range` 0: off).
    // DC-DEFAULT: DSV-642 (proposed) 2000 units (beyond the AI's 1500-unit shot range), 20 s
    pub retire_range: f32,
    pub retire_age_ms: u32,
}

impl Default for CoopTuning {
    fn default() -> Self {
        CoopTuning {
            max_alive: 12,
            lives: None,
            default_lives: 1,
            waves: 5,
            first_wave_ms: 5_000,
            wave_break_ms: 10_000,
            spawn_interval_ms: 1_000,
            spawn_clear: 256.0,
            corpse_ms: 3_000,
            speed: 200.0,
            hold_range: 400.0,
            shot_range: 1500.0,
            fire_ms: 700,
            hit_chance: [0.2, 0.3, 0.4],
            default_damage: 8.0,
            melee_hit: 1.0,
            default_health: 100.0,
            ai_health: AiHealthSource::Rows,
            step_ms: 50,
            ray_hits: true,
            npc_rank: 0,
            nav: true,
            heal_split: true,
            specials: true,
            growth_arm_ms: 1_000,
            growth_trigger: 96.0,
            bursts: true,
            full_accuracy: true,
            retire_range: 2_000.0,
            retire_age_ms: 20_000,
        }
    }
}

/// Where an AI's health maximum comes from (`coop.ai_health`, DSV-603).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiHealthSource {
    /// The `HitPoints` of the attribute row named as its body row, else the placed object's `MaxHitPoints`.
    Rows,
    /// The placed object's `MaxHitPoints` (every ROM AI: the same value), else the attribute row's.
    World,
}

/// The `-coop` keys.
pub const COOP_KEYS: [&str; 23] = [
    "max_alive",
    "lives",
    "waves",
    "first_wave_ms",
    "wave_break_ms",
    "corpse_ms",
    "speed",
    "shot_range",
    "fire_ms",
    "hit_chance",
    "default_damage",
    "melee_hit",
    "ray_hits",
    "nav",
    "heal_split",
    "specials",
    "growth_arm_ms",
    "growth_trigger",
    "ai_health",
    "bursts",
    "full_accuracy",
    "retire_range",
    "retire_age_ms",
];

impl CoopTuning {
    /// Sets one `coop.*` key (the prefix is optional). `hit_chance` takes one value for every difficulty.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let k = key.trim();
        let k = k.strip_prefix("coop.").unwrap_or(k);
        let v = value.trim();
        let bad = || format!("coop.{k}: bad value {v:?}");
        let num = || {
            v.parse::<f32>()
                .ok()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .ok_or_else(bad)
        };
        let int = || v.parse::<u32>().map_err(|_| bad());
        match k {
            "max_alive" => self.max_alive = int()?.clamp(1, 64),
            "lives" => self.lives = Some(int()?.clamp(1, 255)),
            "waves" => self.waves = int()?.clamp(1, 100),
            "first_wave_ms" => self.first_wave_ms = int()?,
            "wave_break_ms" => self.wave_break_ms = int()?,
            "corpse_ms" => self.corpse_ms = int()?,
            "speed" => self.speed = num()?,
            "shot_range" => self.shot_range = num()?,
            "fire_ms" => self.fire_ms = int()?.max(50),
            "hit_chance" => self.hit_chance = [num()?.min(1.0); 3],
            "default_damage" => self.default_damage = num()?,
            "melee_hit" => self.melee_hit = num()?.min(1.0),
            "ray_hits" => {
                self.ray_hits = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad()),
                }
            }
            "nav" => {
                self.nav = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad()),
                }
            }
            "heal_split" => {
                self.heal_split = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad()),
                }
            }
            "specials" => {
                self.specials = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad()),
                }
            }
            "growth_arm_ms" => self.growth_arm_ms = int()?,
            "growth_trigger" => self.growth_trigger = num()?,
            "bursts" => {
                self.bursts = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad()),
                }
            }
            "full_accuracy" => {
                self.full_accuracy = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad()),
                }
            }
            "retire_range" => self.retire_range = num()?,
            "retire_age_ms" => self.retire_age_ms = int()?,
            "ai_health" => {
                self.ai_health = match v {
                    "rows" => AiHealthSource::Rows,
                    "world" => AiHealthSource::World,
                    _ => return Err(bad()),
                }
            }
            _ => {
                return Err(format!(
                    "unknown key coop.{k} (known: {})",
                    COOP_KEYS.join(", ")
                ))
            }
        }
        Ok(())
    }
}

/// Life of one AI slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiState {
    Alive,
    /// Killed at `at`: its objects are hidden at once and a corpse lies in its place (DSV-585); the slot is kept
    /// until the corpse time is over.
    Dead {
        at: Millis,
    },
    /// Hidden at the park point, its objects kept for the next spawn (FT-R1).
    Hidden,
}

/// What an AI spawns as (resolved once per template).
#[derive(Debug, Clone, PartialEq)]
pub struct AiSpec {
    /// Cache key: the template name (or the fallback marker).
    pub key: String,
    pub look: CharacterLook,
    /// The weapon in its right hand and that weapon's ammunition row.
    pub weapon: Option<(u16, u16)>,
    pub max_health: f32,
    pub caption: Vec<u8>,
    pub kind: u8,
    /// Walk speed in units per second (DSV-588).
    pub speed: f32,
    /// The reach of an AI that fights in melee only (DSV-589); `None`: it shoots within `shot_range`.
    pub melee: Option<f32>,
    /// The held weapon's `AniType` (the upper-body family a melee AI is posed with, DSV-589).
    pub ani: Option<u8>,
    /// Its template's hand slot is empty: it holds nothing and does not attack (DSV-590).
    pub unarmed: bool,
    /// A melee AI walks at `walk` until a human comes within `rush`, then at `speed` (DSV-597); `None`: always at
    /// `speed`.
    pub walk: Option<f32>,
    pub rush: Option<f32>,
    /// What its death sets off (DSV-598..DSV-600).
    pub death: DeathSpecial,
    /// The hit nodes of its own body row's skeleton when it wears another row's body (DSV-601): a hit's factor is
    /// its own row's for the hit's location.
    pub own_nodes: Option<Vec<chars::HitNode>>,
    /// The weapon table's gear row its own body row names (DSV-604): the damage types a blast cannot hurt it with.
    pub gear: Option<u16>,
    /// Its weapon row's AI burst (DSV-640; `None` for a melee AI or a row without the keys).
    pub burst: Option<crate::rom_tables::AiBurst>,
    /// Its attribute row's `FullAccuracyRadius` (DSV-641).
    pub full_accuracy: Option<f32>,
}

/// A blast as the AI take it ([`Game::blast_ais`]): its point, the line-of-sight ray's start, its radius, the damage at
/// its centre with every scale applied, its damage type and the player it is credited to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::game) struct AiBlast {
    pub at: [f32; 3],
    pub lifted: [f32; 3],
    pub radius: f32,
    pub full: f32,
    pub dtype: u8,
    pub by: Option<u8>,
}

/// One AI character slot (the objects stay for the match).
#[derive(Debug, Clone, PartialEq)]
pub struct AiChar {
    pub slot: usize,
    /// Character object and attached weapon object (bound by the session once it has created them).
    pub obj: Option<u16>,
    pub weapon_obj: Option<u16>,
    pub spec: AiSpec,
    /// The caption and kind the create record carried (the client keeps the create's caption for the match:
    /// FIRETEAM §1.5, sub 0x11 does not re-run the init).
    pub caption: Vec<u8>,
    pub kind: u8,
    pub health: f32,
    pub max_health: f32,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub motion: Motion,
    motion_since: Millis,
    spawned_at: Millis,
    pub state: AiState,
    pub wave: usize,
    /// The name the statement that spawned it gave (`spawn <name>`, DSV-571) and its template (the spawner's
    /// target): its death runs the template's death command.
    pub name: Vec<u8>,
    pub template: Option<Vec<u8>>,
    pub target: Option<u8>,
    next_fire: Millis,
    /// Shots left in its current burst (DSV-640; 0: the next shot starts a new burst).
    burst_left: u32,
    /// A living human came within the shot range of it (DSV-642: an AI the players met and left).
    met: bool,
    /// The last melee strike (its play-once clip runs from here, DSV-589).
    struck_at: Option<Millis>,
    /// It saw a human within its rush distance and runs from then on (DSV-597).
    pub rushing: bool,
    waypoint: Option<[f32; 3]>,
    repath_at: Millis,
    /// Shots fired and hits scored on humans (tests, logs).
    pub shots: u32,
    pub hits: u32,
}

impl AiChar {
    /// The AI's own objects stand in the world (and are streamed): only while it lives; a dead AI's body is its
    /// corpse object (DSV-585).
    // DC-DEFAULT: DSV-585 (proposed)
    pub fn visible(&self) -> bool {
        self.state == AiState::Alive
    }

    /// Half extents of the AI's box (the body's stand box, else the default player box).
    pub fn dims(&self, fallback: [f32; 3]) -> [f32; 3] {
        self.spec.look.assets.rig.stand_extent.unwrap_or(fallback)
    }
}

/// One spawn point of a wave: position, yaw and the AI template it names.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnSpot {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub template: Option<Vec<u8>>,
}

/// One group of a wave: `count` AIs of `template` (`None` = the spot's template), one per `every_ms`.
#[derive(Debug, Clone, PartialEq)]
struct WaveGroup {
    template: Option<Vec<u8>>,
    count: u32,
    every_ms: u32,
}

/// One wave of the plan.
#[derive(Debug, Clone, PartialEq)]
struct Wave {
    groups: Vec<WaveGroup>,
    /// Fixed spawn spots (ROM spawn tables); empty = the spawners nearest to the humans at the wave's start.
    spots: Vec<SpawnSpot>,
    max_alive: u32,
}

/// Wave progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavePhase {
    /// The round has not started.
    Idle,
    /// Wave `wave` starts at `at`.
    Next {
        at: Millis,
    },
    Spawning,
    Done,
}

/// A queued spawn of the current wave.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    template: Option<Vec<u8>>,
    due: Millis,
}

/// The co-op state of a room.
#[derive(Debug, Clone, PartialEq)]
pub struct Coop {
    pub plan: CoopPlan,
    /// The difficulty index (the room's goal, 0..2).
    pub goal: u32,
    tables: AiTables,
    waves: Vec<Wave>,
    pub ais: Vec<AiChar>,
    /// Index of the current (or next) wave.
    pub wave: usize,
    pub phase: WavePhase,
    queue: VecDeque<Pending>,
    spots: Vec<SpawnSpot>,
    next_spot: usize,
    specs: BTreeMap<String, Option<AiSpec>>,
    /// Lives left counting the current one, points, AI kills, deaths, per account key.
    pub lives: BTreeMap<u32, u32>,
    pub points: BTreeMap<u32, u32>,
    pub kills: BTreeMap<u32, u32>,
    pub deaths: BTreeMap<u32, u32>,
    pub ai_killed: u32,
    /// Points and deaths of earlier stays of rejoined accounts: still the team's (DSV-621).
    pub banked_points: u32,
    pub banked_deaths: u32,
    pub waves_cleared: u32,
    pub stage: Option<u8>,
    /// Accounts whose refused respawn request was logged since their last death.
    refusals_logged: std::collections::BTreeSet<u32>,
    started: Option<Millis>,
    last_step: Option<Millis>,
    /// Log of what the slice did (tests): spawns, kills, hides.
    pub events: Vec<String>,
    /// A mission world: the AI come from the world's statements, not from a wave plan (DSV-571).
    pub mission: bool,
    /// Spawns the statements asked for that wait for a free AI slot: the spot (the spawner's place and target) and
    /// the new AI's name.
    mission_queue: VecDeque<(SpawnSpot, Vec<u8>)>,
    /// The statements' spawns so far (tests, logs) and the ones dropped (unknown spawner, no body).
    pub mission_spawns: u32,
    pub mission_dropped: u32,
    /// The AI stand still and hold fire (`aifreeze 1`, DSV-572).
    pub frozen: bool,
    /// When the last AI left behind gave up its slot (DSV-642: at most one per tick).
    retired_at: Option<Millis>,
    /// The mission goals shown: goal number and state (DSV-574).
    pub goals: Vec<(u32, u8)>,
    /// The world's statements run in this room (a mission world or a round world, DSV-570 / DSV-576): AI deaths run
    /// their templates' death commands and the statements' spawns are made.
    pub network: bool,
    /// A round world (DSV-576): the spawn-table rounds start when the world's round counter reaches them and a
    /// cleared round runs the world's round-end event instead of starting the next round itself.
    pub round_world: bool,
    /// The round the world's round counter reached last (0: none yet), and the `healinpart` statements run.
    pub round: u32,
    pub heals: u32,
    /// A round world's cleared round still owes its recovery (DSV-593): `healinpart` or, without one, the next
    /// round's start pays it.
    pub recovery_owed: bool,
    /// The last rounds of the easier difficulties (a chapter ends there, DSV-594), and the recoveries made.
    pub chapter_ends: Vec<u32>,
    pub recoveries: u32,
    pub chapter_recoveries: u32,
    /// What the statements showed (DSV-579..DSV-583): the music trigger playing (its index in the world's list),
    /// the characters hidden by `hidecharacter`, and counts for logs and tests.
    pub show: super::coop_show::Shown,
    /// Dead AIs' dropped items waiting for a human (DSV-599) and their clouds (DSV-600).
    pub growths: Vec<Growth>,
    pub clouds: Vec<Cloud>,
    /// Death specials set off (tests, logs): blasts, dropped items, clouds.
    pub death_blasts: u32,
    pub death_throws: u32,
    pub death_clouds: u32,
}

/// The account key of a player for the per-account co-op counters (the account id, else the client id).
/// The upper-body clip of a melee AI and the ms into it: the strike clip of its weapon's family while the last strike
/// plays, else the family's aim clip of its motion (the player's stand / walk clip is the rifle pose); `None` for a
/// shooting AI (its upper body stays on the player's clips) or a rig without that family.
// SPEC: FIRETEAM_2009.md §1.8 (trackers travel in the object stream as for players)
// DC-DEFAULT: DSV-589 (proposed); DSV-125 (proposed) the families by `AniType`
fn ai_melee_upper(a: &AiChar, now: Millis) -> Option<(crate::anims::ClipRef, u32)> {
    a.spec.melee?;
    let table = &a.spec.look.anims.table;
    let strike = a.struck_at.and_then(|at| {
        let c = table.action_clip(a.spec.ani, crate::anims::Act::Fire, a.motion)?;
        let t = since(now, at);
        (t < c.1).then_some((c, t))
    });
    strike.or_else(|| {
        table
            .aim_clip(a.spec.ani, a.motion)
            .map(|c| (c, since(now, a.motion_since)))
    })
}

fn key_of(uid: Option<u32>, pid: u8) -> u32 {
    uid.unwrap_or(0x8000_0000 | u32::from(pid))
}

fn norm2(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = (v[0] * v[0] + v[2] * v[2]).sqrt();
    (l.is_finite() && l > 1e-4).then(|| [v[0] / l, 0.0, v[2] / l])
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn rotate_y(d: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [d[0] * c + d[2] * s, 0.0, -d[0] * s + d[2] * c]
}

/// Distance along a unit ray to an axis-aligned box (slab test), when it lies within `max_t`.
pub fn ray_box(
    origin: [f32; 3],
    dir: [f32; 3],
    max_t: f32,
    centre: [f32; 3],
    half: [f32; 3],
) -> Option<f32> {
    let mut t0 = 0.0f32;
    let mut t1 = max_t;
    for i in 0..3 {
        let lo = centre[i] - half[i];
        let hi = centre[i] + half[i];
        if dir[i].abs() < 1e-8 {
            if origin[i] < lo || origin[i] > hi {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir[i];
        let (mut a, mut b) = ((lo - origin[i]) * inv, (hi - origin[i]) * inv);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

impl Coop {
    fn new(plan: CoopPlan, goal: u32, tables: AiTables, cfg: &CoopTuning) -> Coop {
        let waves = Self::waves_of(&plan, goal, cfg);
        Coop {
            plan,
            goal,
            tables,
            waves,
            ais: Vec::new(),
            wave: 0,
            phase: WavePhase::Idle,
            queue: VecDeque::new(),
            spots: Vec::new(),
            next_spot: 0,
            specs: BTreeMap::new(),
            lives: BTreeMap::new(),
            points: BTreeMap::new(),
            kills: BTreeMap::new(),
            deaths: BTreeMap::new(),
            ai_killed: 0,
            waves_cleared: 0,
            stage: None,
            refusals_logged: std::collections::BTreeSet::new(),
            banked_points: 0,
            banked_deaths: 0,
            started: None,
            last_step: None,
            events: Vec::new(),
            mission: false,
            mission_queue: VecDeque::new(),
            mission_spawns: 0,
            mission_dropped: 0,
            frozen: false,
            retired_at: None,
            goals: Vec::new(),
            network: false,
            round_world: false,
            round: 0,
            heals: 0,
            recovery_owed: false,
            chapter_ends: Vec::new(),
            recoveries: 0,
            chapter_recoveries: 0,
            show: Default::default(),
            growths: Vec::new(),
            clouds: Vec::new(),
            death_blasts: 0,
            death_throws: 0,
            death_clouds: 0,
        }
    }

    /// The waves: the GMS spawn tables (one wave per row, in `Round` order) or the synthetic plan.
    // DC-DEFAULT: FT-10 (proposed) spawn tables; GMD-17 (proposed) / FT-S1 (proposed) the synthetic plan
    fn waves_of(plan: &CoopPlan, goal: u32, cfg: &CoopTuning) -> Vec<Wave> {
        if !plan.rounds.is_empty() {
            return plan
                .rounds
                .iter()
                .map(|r| Wave {
                    groups: r
                        .groups
                        .iter()
                        .map(|g| WaveGroup {
                            template: Some(g.template.as_bytes().to_vec()),
                            count: g.count,
                            every_ms: (g.delay_s * 1000.0).round().max(0.0) as u32,
                        })
                        .collect(),
                    // filled from the spawner names when the wave starts
                    spots: Vec::new(),
                    max_alive: r
                        .max_alive
                        .unwrap_or(cfg.max_alive)
                        .min(cfg.max_alive)
                        .max(1),
                })
                .collect();
        }
        (1..=cfg.waves)
            .map(|n| {
                let base = 4 + 2 * n;
                let count = if goal >= 2 {
                    (base * 3).div_ceil(2)
                } else {
                    base
                };
                Wave {
                    groups: vec![WaveGroup {
                        template: None,
                        count,
                        every_ms: cfg.spawn_interval_ms,
                    }],
                    spots: Vec::new(),
                    max_alive: cfg.max_alive.max(1),
                }
            })
            .collect()
    }

    pub fn waves_total(&self) -> usize {
        self.waves.len()
    }

    pub fn alive(&self) -> usize {
        self.ais
            .iter()
            .filter(|a| a.state == AiState::Alive)
            .count()
    }

    fn total_points(&self) -> u32 {
        self.banked_points + self.points.values().sum::<u32>()
    }

    fn total_deaths(&self) -> u32 {
        self.banked_deaths + self.deaths.values().sum::<u32>()
    }
}

impl Game {
    /// Whether the room is a co-op room with its co-op state.
    pub fn is_coop(&self) -> bool {
        self.coop.is_some()
    }

    pub fn coop(&self) -> Option<&Coop> {
        self.coop.as_ref()
    }

    /// Replaces the world's co-op objects (tests with synthetic spawners and templates).
    pub fn set_coop_world(&mut self, w: CoopWorld) {
        self.coop_world = w;
    }

    /// Sets the co-op state up for the room line of a class-5 room: the GMS of this world for the room's mode key
    /// and difficulty (`goal`), the AI rows of the model table.
    // SPEC: FIRETEAM_2009.md §2.1 (0x7d `goal` PLAUSIBLY the difficulty), §3.1 (three difficulty rows)
    // DC-DEFAULT: FT-12 (proposed) the room goal selects `[Object<goal>]`
    pub(super) fn coop_setup(&mut self) {
        let goal = self.rules.goal.min(2);
        let (plan, tables, note, chapter_ends) = match self.vfs.as_ref() {
            Some(v) => {
                let key = self.room.mode_key().to_string();
                let doc = coop_rom::find_gms(v, &key, &self.world_announce);
                let note = match &doc {
                    Some(_) => "the world's co-op GMS".to_string(),
                    None => format!("no GMS of mode {key:?} names this world"),
                };
                let plan = doc
                    .as_ref()
                    .map(|d| CoopPlan::from_gms(d, goal))
                    .unwrap_or_default();
                // DC-DEFAULT: DSV-594 (proposed) a chapter ends at the last round of each easier difficulty
                let chapters = doc
                    .as_ref()
                    .map(|d| recovery::chapter_ends(d, goal))
                    .unwrap_or_default();
                (plan, AiTables::from_vfs(v), note, chapters)
            }
            None => (
                CoopPlan::default(),
                AiTables::default(),
                "no ROM".into(),
                Vec::new(),
            ),
        };
        // DC-DEFAULT: DSV-595 (proposed) the difficulty row's `TimeLimit` when the room line gives no time
        self.coop_mission_clock(&plan);
        let mut c = Coop::new(plan, goal, tables, &self.cfg.coop);
        // DC-DEFAULT: DSV-571 (proposed) a mission world runs its own trigger network: no wave plan
        // (a world whose GMS has spawn tables keeps its waves: its statements count the rounds, DSV-571)
        if self.coop_world.mission && c.plan.rounds.is_empty() {
            c.mission = true;
            c.waves.clear();
        }
        // DC-DEFAULT: DSV-576 (proposed) a world with spawn tables and a round loop of its own runs its statements; its
        // rounds follow the world's round counter
        c.round_world = !c.mission && !c.plan.rounds.is_empty() && self.coop_world.rounds.is_some();
        c.network = c.mission || c.round_world;
        if c.round_world {
            c.chapter_ends = chapter_ends;
        }
        let line = format!(
            "co-op room: difficulty {goal} ({note}), {} wave(s){}, {} spawner(s), {} AI template(s), {} AI node(s), {} AI body row(s)",
            c.waves_total(),
            if c.mission {
                " (mission: the world's trigger network)"
            } else if c.round_world {
                " (spawn tables; rounds by the world's trigger network)"
            } else if c.plan.rounds.is_empty() {
                " (synthetic plan)"
            } else {
                " (spawn tables)"
            },
            self.coop_world.spawners.len(),
            self.coop_world.templates.len(),
            self.coop_world.nodes.len(),
            c.tables.bodies.len()
        );
        self.coop = Some(c);
        self.log(line);
    }

    /// Lives a new human starts with.
    // DC-DEFAULT: FT-5 (proposed)
    fn coop_initial_lives(&self) -> u32 {
        let c = &self.cfg.coop;
        c.lives
            .or_else(|| self.coop.as_ref().and_then(|x| x.plan.difficulty.lives))
            .filter(|n| *n > 0)
            .unwrap_or(c.default_lives)
            .clamp(1, 255)
    }

    fn player_key(&self, pid: u8) -> u32 {
        key_of(self.players.get(&pid).and_then(|p| p.uid), pid)
    }

    /// The player's lives (counting the current one) for 0x6b `m1` and 0xd7 sub 2; 0 outside co-op.
    pub fn coop_lives_of(&self, pid: u8) -> u32 {
        let k = self.player_key(pid);
        self.coop
            .as_ref()
            .and_then(|c| c.lives.get(&k).copied())
            .unwrap_or(0)
    }

    pub fn coop_points_of(&self, pid: u8) -> u32 {
        let k = self.player_key(pid);
        self.coop
            .as_ref()
            .and_then(|c| c.points.get(&k).copied())
            .unwrap_or(0)
    }

    /// A rejoined account's new stay: its own points, AI kills and deaths start at 0 (the team totals keep them);
    /// its lives stay (FT-S9).
    // DC-DEFAULT: DSV-621 (proposed); FT-S9 (proposed) the lives
    pub(super) fn coop_new_stay(&mut self, uid: u32) {
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        c.banked_points += c.points.remove(&uid).unwrap_or(0);
        c.banked_deaths += c.deaths.remove(&uid).unwrap_or(0);
        c.kills.remove(&uid);
    }

    /// A human was placed: its lives are set once per account (a rejoin keeps what is left).
    pub(super) fn coop_on_player(&mut self, pid: u8) {
        let n = self.coop_initial_lives();
        let k = self.player_key(pid);
        if let Some(c) = self.coop.as_mut() {
            c.lives.entry(k).or_insert(n);
        }
    }

    /// After a human's spawn set: 0xd7 sub 2 `[n][n]` with the lives counting this one (the client's record +0x1f8).
    // SPEC: FIRETEAM_2009.md §2.2 row 2, §2.3
    // DC-DEFAULT: FT-L1 (proposed); FT-5 (proposed) sub 2 after every spawn
    pub(super) fn coop_after_spawn(&mut self, pid: u8) {
        if self.coop.is_none() {
            return;
        }
        let n = self.coop_lives_of(pid).min(255) as u8;
        self.send(pid, gs::coop_lives(n, n));
    }

    /// A human's lethal hit in class 5, before its death states: one life less, sent as 0xd7 sub 2 `[n-1][n-1]`
    /// **before** 0x67 (the client decides on the value it holds when it processes state 2; a sub 2 after state 3
    /// would re-arm on the old value).
    // SPEC: FIRETEAM_2009.md §2.3 (DISASM cshell+0x60ac0..0x60bcc), §2.2 re-arm (needs a dead state and a == b)
    // DC-DEFAULT: FT-L1 (proposed; D6 of the design diff)
    pub(super) fn coop_on_human_death(&mut self, vpid: u8) {
        let k = self.player_key(vpid);
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        let left = c.lives.get(&k).copied().unwrap_or(0).saturating_sub(1);
        c.lives.insert(k, left);
        c.refusals_logged.remove(&k);
        *c.deaths.entry(k).or_default() += 1;
        let n = left.min(255) as u8;
        self.send(vpid, gs::coop_lives(n, n));
        self.log(format!("client {vpid}: co-op death, {left} lives left"));
    }

    /// Whether a dead human may come back (class 5: only while lives are left).
    pub(super) fn coop_may_respawn(&self, pid: u8) -> bool {
        self.coop.is_none() || self.coop_lives_of(pid) > 0
    }

    /// Whether a refused respawn request of this player should be logged (the first one per death; a client may
    /// repeat the request every frame).
    pub(super) fn coop_log_refusal(&mut self, pid: u8) -> bool {
        let k = self.player_key(pid);
        self.coop
            .as_mut()
            .is_some_and(|c| c.refusals_logged.insert(k))
    }

    // ------------------------------------------------------------------------------------------- AI specs

    /// The rig of the default player body (what an AI body must share, FIRETEAM §1.8).
    fn player_rig(&mut self) -> Option<Rig> {
        let row = self.cfg.character.model_row;
        let model = self.tables.chars.bodies.get(&row)?.model.clone();
        match self.vfs.as_ref() {
            Some(v) => self.rigs.rig(v, &model).ok(),
            None => self.rigs.cached(&model),
        }
    }

    /// The look of an AI on a ROM `[AIModel<n>]` row: body (and its clip child models), skins, styles, the face
    /// row when attached, the skeleton row's hit nodes; `None` when the body does not share the player rig.
    // SPEC: FIRETEAM_2009.md §1.8 (model block from the AI row: ModelFile, Skin0/1, RenderStyle0/1, AttachFace /
    // AIFace; layout as for players)
    // DC-DEFAULT: FT-S2 (proposed) only the player-rig rows
    fn ai_row_look(&mut self, row: &AiBodyRow, player: &Rig) -> Option<CharacterAssets> {
        let vfs = self.vfs.as_ref()?;
        let rig = self.rigs.rig(vfs, &row.model).ok()?;
        let socket_ok = coop_rom::weapon_socket_present(vfs, &row.model, rig.grip_socket);
        if !coop_rom::shares_player_rig(&rig, player, socket_ok) {
            return None;
        }
        let mut body = vec![row.model.clone()];
        body.extend(rig.children.iter().cloned());
        let missing = |p: &String| chars::resource_size(vfs, p).is_none();
        if body.iter().chain(row.skins.iter()).any(missing) {
            return None;
        }
        let face = row
            .face
            .as_deref()
            .and_then(|f| self.coop.as_ref()?.tables.face_named(f).cloned())
            .map(|f| {
                (
                    f.model.clone(),
                    f.skins.iter().take(1).cloned().collect::<Vec<_>>(),
                    f.styles.clone(),
                )
            })
            .filter(|(m, sk, st)| {
                !std::iter::once(m)
                    .chain(sk.iter())
                    .chain(st.iter())
                    .any(missing)
            });
        let hit_nodes = row
            .skeleton
            .and_then(|s| self.tables.chars.skeletons.get(&s))
            .cloned()
            .unwrap_or_default();
        Some(CharacterAssets {
            body,
            body_textures: row.skins.clone(),
            body_styles: row.styles.iter().filter(|p| !missing(p)).cloned().collect(),
            face,
            rig,
            mover_skins: row.skins.clone(),
            hit_nodes,
            max_armor: None,
            gear: None,
        })
    }

    /// The weapon row of a ROM weapon `Name` (ASCII case-insensitive).
    fn weapon_named(&self, name: &str) -> Option<u16> {
        self.tables
            .weapons
            .weapons
            .iter()
            .find(|(_, w)| {
                w.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .map(|(id, _)| *id)
    }

    /// Resolves what an AI of `template` (an AI character object of the world, by name) spawns as: its body row
    /// when that row shares the player rig, else the first such row the world's templates name, else the first
    /// such row, else the default player character; its right-hand weapon (a template whose slot is empty holds
    /// none; a name the weapon table lacks, or no template, gets the first default-loadout weapon); its health
    /// maximum x the difficulty's rate of the wave; the caption (boss: the GMS boss name) and the kind; its speed
    /// (DSV-588) and whether it fights in melee (DSV-589).
    // DC-DEFAULT: FT-S2 (proposed) body fallback, DSV-590 (proposed) the world's rows first and the empty hand slot;
    // FT-S3 (proposed) caption; FT-12 (proposed) health; FT-14 / FT-K1 (proposed) boss kind 2
    fn ai_spec(&mut self, template: Option<&[u8]>) -> Option<AiSpec> {
        let key = template
            .map(|t| String::from_utf8_lossy(t).to_ascii_uppercase())
            .unwrap_or_else(|| "*".into());
        if let Some(s) = self.coop.as_ref().and_then(|c| c.specs.get(&key)) {
            return s.clone();
        }
        let t = template.and_then(|n| self.coop_world.template(n).cloned());
        let player = self.player_rig();
        let mut assets = None;
        let mut row_name = None;
        let mut worn: Option<String> = None;
        if let (Some(player), Some(c)) = (player.as_ref(), self.coop.as_ref()) {
            let rows: Vec<AiBodyRow> = c.tables.bodies.values().cloned().collect();
            let wanted = t.as_ref().and_then(|t| t.body_row.clone());
            let named =
                |r: &AiBodyRow, w: Option<&str>| w.is_some_and(|w| r.name.eq_ignore_ascii_case(w));
            let world_rows: Vec<&AiBodyRow> = rows
                .iter()
                .filter(|r| {
                    self.coop_world
                        .templates
                        .iter()
                        .any(|t| named(r, t.body_row.as_deref()))
                })
                .collect();
            let order = rows
                .iter()
                .filter(|r| named(r, wanted.as_deref()))
                .chain(world_rows)
                .chain(rows.iter());
            for r in order {
                if let Some(a) = self.ai_row_look(r, player) {
                    assets = Some(a);
                    row_name = Some(r.ai_name.clone().unwrap_or_default());
                    worn = Some(r.name.clone());
                    break;
                }
            }
        }
        let weapon = match t.as_ref().map(|t| t.right_hand.as_deref()) {
            Some(None) => None,
            Some(Some(n)) => self
                .weapon_named(n)
                .or_else(|| self.loadout_weapons(None).first().map(|w| w.1)),
            None => self.loadout_weapons(None).first().map(|w| w.1),
        };
        let weapon = weapon.map(|w| (w, self.tables.weapons.ammo_of(w).unwrap_or(0)));
        let (speed, melee) = self.ai_kind(t.as_ref(), weapon);
        let (walk, rush) = self.ai_rush(t.as_ref(), speed, melee.is_some());
        let death = self.ai_death_special(t.as_ref());
        let own_nodes = self.ai_own_nodes(t.as_ref(), worn.as_deref());
        // a melee AI's hand is not attached: the ROM's hand rows draw it with a transparent style only (DSV-589)
        let held = weapon.filter(|_| melee.is_none()).map(|w| w.0);
        let look = match assets {
            Some(a) => CharacterLook {
                anims: AnimSet::from_rig(&a.rig),
                weapon: held.and_then(|w| self.weapon_look(w)),
                assets: a,
            },
            None => match self.character_for(None) {
                Ok(Some(mut c)) => {
                    c.weapon = match melee {
                        Some(_) => None,
                        None => held.and_then(|w| self.weapon_look(w)).or(c.weapon),
                    };
                    c
                }
                _ => {
                    if let Some(c) = self.coop.as_mut() {
                        c.specs.insert(key.clone(), None);
                    }
                    self.log(format!("co-op: no character model for AI template {key}"));
                    return None;
                }
            },
        };
        let boss = self
            .coop
            .as_ref()
            .and_then(|c| c.plan.boss_name.as_deref())
            .zip(template)
            .is_some_and(|(b, t)| b.as_bytes().eq_ignore_ascii_case(t));
        let caption = match (&boss, &row_name) {
            (true, _) => template.map(<[u8]>::to_vec),
            (false, Some(n)) if !n.is_empty() => Some(n.as_bytes().to_vec()),
            _ => None,
        }
        .unwrap_or_else(|| FALLBACK_CAPTION.to_vec());
        let max_health = self.ai_health_max(t.as_ref());
        let gear = t
            .as_ref()
            .and_then(|t| t.body_row.as_deref())
            .zip(self.coop.as_ref())
            .and_then(|(n, c)| c.tables.body_named(n)?.gear);
        let spec = AiSpec {
            key: key.clone(),
            weapon,
            look,
            max_health,
            caption,
            kind: if boss { NPC_KIND_BOSS } else { NPC_KIND_NORMAL },
            speed,
            melee,
            ani: weapon.and_then(|w| self.tables.weapons.weapons.get(&w.0)?.ani_type),
            unarmed: t.as_ref().is_some_and(|t| t.right_hand.is_none()),
            walk,
            rush,
            death,
            own_nodes,
            gear,
            burst: weapon
                .filter(|_| melee.is_none())
                .and_then(|(w, _)| self.tables.weapons.ai_burst(w)),
            full_accuracy: t
                .as_ref()
                .and_then(|t| t.body_row.as_deref())
                .zip(self.coop.as_ref())
                .and_then(|(n, c)| c.tables.attributes_named(n)?.full_accuracy),
        };
        if let Some(c) = self.coop.as_mut() {
            c.specs.insert(key, Some(spec.clone()));
        }
        Some(spec)
    }

    /// The health maximum of an AI of template `t` before the round's rate (DSV-603): with `coop.ai_health=rows` the
    /// `HitPoints` of the attribute row named as its body row (the kind's own value), else the placed object's
    /// `MaxHitPoints`; with `world` the other way round; else `coop.default_health`.
    // OBSERVED: ROM co-op worlds (every AI character writes the same `MaxHitPoints` and `MaxArmor`, and empty
    // attribute-override texts; 2026-11-05 scratch run); ROM table key AIBUTES.TXT [AttributeTemplate<n>] HitPoints
    // DC-DEFAULT: DSV-603 (proposed; refines FT-12) the row first
    fn ai_health_max(&self, t: Option<&crate::world::AiTemplate>) -> f32 {
        let row = t
            .and_then(|t| t.body_row.as_deref())
            .zip(self.coop.as_ref())
            .and_then(|(n, c)| c.tables.attributes_named(n)?.hit_points);
        let placed = t.and_then(|t| t.max_health);
        match self.cfg.coop.ai_health {
            AiHealthSource::Rows => row.or(placed),
            AiHealthSource::World => placed.or(row),
        }
        .unwrap_or(self.cfg.coop.default_health)
    }

    /// The speed and the melee reach of an AI of template `t` holding `weapon` (weapon, ammunition).
    ///
    /// Speed (DSV-588): the template's own run speed, else the run speed of the attribute row named as its body
    /// row, at most `coop.speed` (a faster row, the soldiers' one, walks at `coop.speed` as before).
    /// Melee (DSV-589): an AI whose brain row attacks in melee only, or whose weapon's ammunition does melee or hands
    /// damage, strikes within its reach: the brain's melee range, else the weapon row's `Range`, else
    /// [`MELEE_REACH_DEFAULT`].
    // DC-DEFAULT: DSV-588 (proposed), DSV-589 (proposed)
    fn ai_kind(
        &self,
        t: Option<&crate::world::AiTemplate>,
        weapon: Option<(u16, u16)>,
    ) -> (f32, Option<f32>) {
        let cap = self.cfg.coop.speed;
        let tables = self.coop.as_ref().map(|c| &c.tables);
        let row_speed = t
            .and_then(|t| t.body_row.as_deref())
            .zip(tables)
            .and_then(|(n, tb)| tb.attributes_named(n)?.run_speed);
        let speed = t
            .and_then(|t| t.run_speed)
            .or(row_speed)
            .map_or(cap, |s| s.min(cap));
        let brain = t
            .and_then(|t| t.brain.as_deref())
            .zip(tables)
            .and_then(|(n, tb)| tb.brain_named(n));
        // EVIDENCE: cshell+0x27f320 (the melee and hands damage types, `destruct::MELEE_DAMAGE_TYPES`)
        let melee_weapon = weapon
            .and_then(|(w, am)| self.tables.weapons.damage_type(w, am))
            .is_some_and(|d| super::destruct::MELEE_DAMAGE_TYPES.contains(&d));
        let melee = (brain.is_some_and(|b| b.melee_only()) || melee_weapon).then(|| {
            brain
                .and_then(|b| b.melee_max)
                .filter(|r| *r > 0.0)
                .or_else(|| weapon.and_then(|(w, _)| self.tables.weapons.range(w)))
                .filter(|r| *r > 0.0)
                .unwrap_or(MELEE_REACH_DEFAULT)
        });
        (speed, melee)
    }

    // ------------------------------------------------------------------------------------------- waves

    /// The round started: the first wave is due.
    pub(super) fn coop_round_started(&mut self, now: Millis) {
        let at = now.wrapping_add(self.cfg.coop.first_wave_ms);
        if self.coop.as_ref().is_some_and(|c| c.mission) {
            self.coop_mission_started(now);
            return;
        }
        // DC-DEFAULT: DSV-576 (proposed) a round world's match starts with its start boxes (they count round 1)
        if self.coop.as_ref().is_some_and(|c| c.round_world) {
            let boxes = self
                .coop_world
                .rounds
                .as_ref()
                .map(|r| r.start_boxes.clone())
                .unwrap_or_default();
            let Some(c) = self.coop.as_mut() else {
                return;
            };
            if c.started.is_some() {
                return;
            }
            c.started = Some(now);
            for b in &boxes {
                let mut t = b"msg ".to_vec();
                t.extend_from_slice(b);
                t.extend_from_slice(b" trigger");
                self.out.push(GameOut::WorldScript(t));
            }
            self.log(format!(
                "co-op: round world starts ({} start box(es); the world's round counter starts the rounds)",
                boxes.len()
            ));
            return;
        }
        if let Some(c) = self.coop.as_mut() {
            if c.phase == WavePhase::Idle {
                c.phase = WavePhase::Next { at };
                c.started = Some(now);
            }
        }
    }

    /// A mission world's round started: the AI the file places live (not as templates) spawn where they stand, and
    /// the goal list (empty until the statements add goals) goes out with the round's seconds as the mission timer.
    // DC-DEFAULT: DSV-571 (proposed) the placed live AI; DSV-574 (proposed) the goal list
    fn coop_mission_started(&mut self, now: Millis) {
        let live: Vec<(SpawnSpot, Vec<u8>)> = self
            .coop_world
            .templates
            .iter()
            .filter(|t| !t.template)
            .map(|t| {
                (
                    SpawnSpot {
                        pos: t.pos,
                        yaw: t.rot[1],
                        template: Some(t.name.clone()),
                    },
                    t.name.clone(),
                )
            })
            .collect();
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        if c.started.is_some() {
            return;
        }
        c.started = Some(now);
        let n = live.len();
        c.mission_queue.extend(live);
        self.log(format!(
            "co-op: mission starts ({n} placed AI; the world's statements drive it)"
        ));
        self.coop_send_goals(now);
        self.coop_tick_mission(now);
    }

    /// What the world's statements ask of the co-op rules (DSV-571, DSV-572, DSV-574).
    pub fn coop_effects(&mut self, effects: Vec<crate::script::Effect>, now: Millis) {
        use crate::script::{Effect, GoalOp};
        if self.coop.is_none() {
            return;
        }
        for e in effects {
            match e {
                Effect::Spawn { spawner, name } => {
                    // `msg <spawner> spawn` without a name: the AI takes its template's name
                    let name = name.or_else(|| {
                        self.coop_world
                            .spawners
                            .iter()
                            .find(|s| s.name.eq_ignore_ascii_case(&spawner))
                            .and_then(|s| s.target.clone())
                    });
                    let name = name.unwrap_or_default();
                    let spot = self
                        .coop_world
                        .spawners
                        .iter()
                        .find(|s| s.name.eq_ignore_ascii_case(&spawner))
                        .map(|s| SpawnSpot {
                            pos: s.pos,
                            yaw: s.rot[1],
                            template: s.target.clone(),
                        });
                    let Some(c) = self.coop.as_mut() else {
                        return;
                    };
                    match spot {
                        // DC-DEFAULT: DSV-571 (proposed) at most 256 waiting spawns
                        Some(s) if c.mission_queue.len() < 256 => {
                            c.mission_queue.push_back((s, name))
                        }
                        _ => c.mission_dropped += 1,
                    }
                }
                Effect::Lives(n) => self.coop_add_lives(n),
                Effect::Round(n) => self.coop_round_counted(n, now),
                Effect::HealInPart => self.coop_heal_in_part(now),
                Effect::Complete => self.coop_finish(now, STAGE_COMPLETE),
                Effect::Freeze(f) => {
                    if let Some(c) = self.coop.as_mut() {
                        c.frozen = f;
                    }
                    self.log(format!("co-op: AI {}", if f { "frozen" } else { "go on" }));
                }
                Effect::MissionTimer(on) => self.log(format!(
                    "co-op: mission timer {} (not shown)",
                    if on { "on" } else { "off" }
                )),
                Effect::Goal(op, n) => {
                    let state = match op {
                        GoalOp::Add => GOAL_OPEN,
                        GoalOp::Check => GOAL_DONE,
                        GoalOp::Fail => GOAL_FAILED,
                    };
                    let Some(c) = self.coop.as_mut() else {
                        return;
                    };
                    let added = match c.goals.iter_mut().find(|g| g.0 == n) {
                        Some(g) => {
                            if op == GoalOp::Add {
                                continue;
                            }
                            g.1 = state;
                            false
                        }
                        None => {
                            c.goals.push((n, state));
                            true
                        }
                    };
                    if op == GoalOp::Check {
                        c.waves_cleared += 1;
                    }
                    let label = c
                        .plan
                        .mission_list
                        .iter()
                        .find(|m| m.0 == n)
                        .map(|m| m.1.clone())
                        .unwrap_or_default();
                    self.log(format!("co-op: goal {n} {op:?} ({label})"));
                    self.coop_send_goals(now);
                    // DC-DEFAULT: DSV-574 (amended by DSV-579, proposed) a world with message triggers shows its own
                    // banners (`play` of a message trigger): no banner of the goal text there
                    let own_texts = self.coop_world.texts.iter().any(|t| !t.caption);
                    if added && op == GoalOp::Add && !label.is_empty() && !own_texts {
                        self.broadcast(&gs::coop_banner(label.as_bytes(), 3.0));
                    }
                }
            }
        }
        self.coop_tick_mission(now);
    }

    /// Every human gets `n` lives more (`inclifecount`, a checkpoint): 0xd7 sub 2 with the new count to each, which
    /// also re-arms the respawn wait of a dead one (FIRETEAM §2.2).
    // SPEC: FIRETEAM_2009.md §2.2 (re-arm: dead and a == b), §2.3
    // DC-DEFAULT: DSV-572 (proposed) not capped by `MaxLifeCount` (the mission rows give 1 and 1: the checkpoints'
    // lives would be lost), at most 255
    fn coop_add_lives(&mut self, n: i32) {
        let pids: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.introduced)
            .map(|p| p.pid)
            .collect();
        let keys: Vec<(u8, u32)> = pids.iter().map(|p| (*p, self.player_key(*p))).collect();
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        for v in c.lives.values_mut() {
            *v = (*v as i64 + n as i64).clamp(0, 255) as u32;
        }
        for (pid, k) in keys {
            let l = c.lives.get(&k).copied().unwrap_or(0).min(255) as u8;
            self.out.push(GameOut::To(pid, gs::coop_lives(l, l)));
        }
        self.log(format!("co-op: {n:+} life for every human"));
    }

    /// A round world's round counter reached `n`: spawn-table round `n` (the `n`-th row in `Round` order) starts now.
    /// A count past the last row, or while a round is still spawning or fighting, only moves the number.
    // DC-DEFAULT: DSV-576 (proposed)
    fn coop_round_counted(&mut self, n: u32, now: Millis) {
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        if !c.round_world || n == 0 {
            return;
        }
        c.round = n;
        let idx = n as usize - 1;
        let busy = c.phase == WavePhase::Spawning;
        let fits = idx < c.waves.len();
        let starts = fits && !busy && c.stage.is_none();
        if starts {
            c.wave = idx;
            c.phase = WavePhase::Next { at: now };
        }
        let total = c.waves.len();
        // DC-DEFAULT: DSV-594 (proposed) a new chapter starts with the full recovery; DSV-593 (proposed) a round end
        // without `healinpart` pays its recovery at the next round's start
        let chapter = starts && c.chapter_ends.contains(&(n - 1));
        let owed = starts && c.recovery_owed;
        self.log(format!(
            "co-op: the world counts round {n} of {total}{}",
            if !fits {
                " (no spawn table: nothing spawns)"
            } else if busy {
                " (the last round still runs)"
            } else {
                ""
            }
        ));
        if chapter || owed {
            self.coop_recover(now, chapter);
        }
    }

    /// `healinpart`: the living humans share the difficulty row's round health (`RecoverRoundTotalHP`, 100 when
    /// absent; DSV-593: split evenly, whole points): health first up to its maximum, the rest as armour up to the
    /// armour's maximum; the new values go out as player status. A round world's owed recovery follows (DSV-593).
    // DC-DEFAULT: DSV-578 (proposed), amended by DSV-593 (proposed) the split
    fn coop_heal_in_part(&mut self, now: Millis) {
        let pids: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.introduced && p.life == Life::Alive)
            .map(|p| p.pid)
            .collect();
        let split = self.cfg.coop.heal_split;
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        c.heals += 1;
        let total = c.plan.difficulty.recover_hp.unwrap_or(RECOVER_HP_DEFAULT);
        let pool = if split {
            heal_share(total, pids.len())
        } else {
            total
        };
        let owed = c.round_world && c.recovery_owed;
        let max_health = self.cfg.max_health;
        for pid in pids {
            let max_armor = self.armour_of(pid).max;
            let Some(p) = self.players.get_mut(&pid) else {
                continue;
            };
            let (health, armor) = heal_in_part(p.health, p.armor, pool, max_health, max_armor);
            p.health = health;
            p.armor = armor;
            self.send(pid, gs::player_status(gs::STATUS_HEALTH, health));
            self.send(pid, gs::player_status(gs::STATUS_ARMOR, armor));
        }
        self.log(format!(
            "co-op: the living humans recover {pool} points each ({total} in all)"
        ));
        if owed {
            self.coop_recover(now, false);
        }
    }

    /// The goal list (0xc9 sub 1): the goals the statements added, with the difficulty row's texts and their
    /// states; the round's seconds as the timer; `a` = the goals done + 1, `b` = the difficulty.
    // SPEC: FIRETEAM_2009.md §2.4 (sub 1; `a` PLAUSIBLY the stage); GAME_MESSAGES_2009_S2C.md §6.2
    // DC-DEFAULT: DSV-574 (proposed) states 0 open, 1 done, 2 failed (UNPROVEN, V-FT-8)
    fn coop_send_goals(&mut self, now: Millis) {
        if let Some(m) = self.coop_goal_list(now) {
            self.broadcast(&m);
        }
    }

    /// The goal list message of [`Game::coop_send_goals`] (also a late joiner's, DSV-584).
    pub(super) fn coop_goal_list(&self, now: Millis) -> Option<dc_wire::udp::BitWriter> {
        let c = self.coop.as_ref()?;
        let goals: Vec<gs::MissionGoal> = c
            .goals
            .iter()
            .map(|(n, st)| gs::MissionGoal {
                id: *n,
                label: c
                    .plan
                    .mission_list
                    .iter()
                    .find(|m| m.0 == *n)
                    .map(|m| m.1.as_bytes().to_vec())
                    .unwrap_or_default(),
                state: *st,
            })
            .collect();
        let done = c.goals.iter().filter(|g| g.1 == GOAL_DONE).count();
        let goal = c.goal;
        let secs = self.round_secs_left(now);
        let timers: Vec<f32> = if secs > 0 {
            vec![secs as f32]
        } else {
            Vec::new()
        };
        Some(gs::coop_mission_list(
            &goals,
            &timers,
            (done + 1).min(255) as u8,
            goal as u8,
        ))
    }

    /// The waiting spawns of a mission world, while AI slots are free.
    fn coop_tick_mission(&mut self, now: Millis) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        loop {
            let pool = self.cfg.coop.max_alive as usize;
            let Some(c) = self.coop.as_ref() else {
                return;
            };
            let free = c.ais.len() < pool || c.ais.iter().any(|a| a.state == AiState::Hidden);
            let Some((spot, name)) = c.mission_queue.front().cloned() else {
                return;
            };
            // DC-DEFAULT: DSV-642 (proposed) a full pool gives the waiting spawn the slot of an AI left behind, once
            // the spawn's body is known to resolve; a spawn that still fails gives the slot back
            let mut retired = None;
            if !free {
                if self.ai_spec(spot.template.as_deref()).is_none() {
                    // the spawn cannot appear: dropped now, as a failed spawn is
                    if let Some(c) = self.coop.as_mut() {
                        c.mission_queue.pop_front();
                        c.mission_dropped += 1;
                    }
                    continue;
                }
                retired = self.coop_retire_left_behind(now);
                if retired.is_none() {
                    return;
                }
            }
            let ok = self.coop_spawn_ai(None, &spot, Some(name.clone()), now);
            if let (false, Some(slot)) = (ok, retired) {
                self.coop_unretire(slot);
            }
            let Some(c) = self.coop.as_mut() else {
                return;
            };
            c.mission_queue.pop_front();
            if ok {
                c.mission_spawns += 1;
            } else {
                c.mission_dropped += 1;
            }
        }
    }

    /// A mission world's pool is full while a spawn waits: the living AI left behind farthest from every human gives
    /// up its slot. It qualifies when it has lived `retire_age_ms`, a human came within the shot range of it before
    /// (or it is unarmed: it never fights), its template has no death command (its death may move the mission on),
    /// it is farther than `retire_range` from every living human (and, while a human waits to spawn again, from
    /// every open spawn point) and no living human's eye has a clear line to its feet, middle or eye. It leaves the
    /// world as a corpse's objects do (hidden at the park point, FT-R1) with no death: no death command, drops,
    /// points or blast. At most one per tick, none while a corpse is about to free a slot. Returns the slot freed.
    // DC-DEFAULT: DSV-642 (proposed)
    fn coop_retire_left_behind(&mut self, now: Millis) -> Option<usize> {
        let range = self.cfg.coop.retire_range;
        let age = self.cfg.coop.retire_age_ms;
        let c = self.coop.as_ref()?;
        if !c.mission
            || range <= 0.0
            || c.retired_at == Some(now)
            || c.ais
                .iter()
                .any(|a| matches!(a.state, AiState::Dead { .. }))
        {
            return None;
        }
        let eyes: Vec<[f32; 3]> = self
            .living_humans()
            .into_iter()
            .map(|(pid, p)| add3(p, [0.0, self.dims_of(pid)[1] * 0.8, 0.0]))
            .collect();
        if eyes.is_empty() {
            return None;
        }
        let mut near_points = eyes.clone();
        let waiting = self
            .players
            .values()
            .any(|p| p.introduced && p.spawns > 0 && p.life != Life::Alive);
        if waiting {
            let open = self.spawn_open();
            near_points.extend(
                self.spawns
                    .iter()
                    .zip(open)
                    .filter(|(_, o)| *o)
                    .map(|(s, _)| s.0),
            );
        }
        let fallback = self.cfg.player_dims;
        let pick = c
            .ais
            .iter()
            .filter(|a| a.state == AiState::Alive && since(now, a.spawned_at) >= age)
            .filter(|a| a.met || a.spec.unarmed)
            .filter(|a| {
                a.template
                    .as_deref()
                    .and_then(|t| self.coop_world.template(t))
                    .is_none_or(|t| t.death_cmd.iter().all(u8::is_ascii_whitespace))
            })
            .filter_map(|a| {
                let near = near_points
                    .iter()
                    .map(|e| len3(sub3(*e, a.pos)))
                    .fold(f32::INFINITY, f32::min);
                (near > range).then_some((a.slot, near, a.pos, a.dims(fallback)))
            })
            .filter(|(_, _, pos, dims)| {
                let marks = [-0.9f32, 0.0, 0.8].map(|k| add3(*pos, [0.0, dims[1] * k, 0.0]));
                !eyes
                    .iter()
                    .any(|e| marks.iter().any(|m| self.sight_clear(*e, *m)))
            })
            .max_by(|x, y| x.1.total_cmp(&y.1));
        let (slot, near, _, _) = pick?;
        let c = self.coop.as_mut()?;
        c.retired_at = Some(now);
        let a = &mut c.ais[slot];
        a.state = AiState::Hidden;
        a.vel = [0.0; 3];
        a.target = None;
        c.events.push(format!("retire slot {slot}"));
        self.out.push(GameOut::AiHidden { slot });
        self.log(format!(
            "co-op: AI slot {slot} left behind ({near:.0} u from the nearest human): its slot goes to a waiting spawn"
        ));
        Some(slot)
    }

    /// The waiting spawn did not take the slot an AI left behind gave up: that AI comes back where it stood.
    // DC-DEFAULT: DSV-642 (proposed)
    fn coop_unretire(&mut self, slot: usize) {
        let Some(a) = self
            .coop
            .as_mut()
            .and_then(|c| c.ais.get_mut(slot))
            .filter(|a| a.state == AiState::Hidden)
        else {
            return;
        };
        a.state = AiState::Alive;
        self.out.push(GameOut::AiShow { slot });
        self.log(format!(
            "co-op: AI slot {slot} kept its slot (the waiting spawn failed)"
        ));
    }

    /// Living, introduced humans: (pid, position).
    fn living_humans(&self) -> Vec<(u8, [f32; 3])> {
        self.players
            .values()
            .filter(|p| p.introduced && p.life == Life::Alive)
            .map(|p| (p.pid, p.pos))
            .collect()
    }

    /// Spawn spots of a wave without fixed spots: the spawners (else the AI templates, else the player spawn
    /// points) nearest to the living humans' centre, at least `spawn_clear` from every living human.
    // DC-DEFAULT: FT-S1 (proposed) the 8 nearest clear spawners; FT-10 (proposed) 256 units clear
    fn nearest_spots(&self) -> Vec<SpawnSpot> {
        let humans = self.living_humans();
        let mut all: Vec<SpawnSpot> = self
            .coop_world
            .spawners
            .iter()
            .map(|s| SpawnSpot {
                pos: s.pos,
                yaw: s.rot[1],
                template: s.target.clone(),
            })
            .collect();
        if all.is_empty() {
            all = self
                .coop_world
                .templates
                .iter()
                .map(|t| SpawnSpot {
                    pos: t.pos,
                    yaw: t.rot[1],
                    template: Some(t.name.clone()),
                })
                .collect();
        }
        if all.is_empty() {
            all = self
                .spawns
                .iter()
                .map(|s| SpawnSpot {
                    pos: s.0,
                    yaw: s.1[1],
                    template: None,
                })
                .collect();
        }
        let centre = if humans.is_empty() {
            self.centre
        } else {
            let n = humans.len() as f32;
            humans
                .iter()
                .fold([0.0; 3], |a, h| add3(a, scale3(h.1, 1.0 / n)))
        };
        let clear = self.cfg.coop.spawn_clear;
        let mut far: Vec<(f32, SpawnSpot)> = all
            .into_iter()
            .filter(|s| finite3(s.pos))
            .filter(|s| humans.iter().all(|h| len3(sub3(s.pos, h.1)) >= clear))
            .map(|s| (len3(sub3(s.pos, centre)), s))
            .collect();
        far.sort_by(|a, b| a.0.total_cmp(&b.0));
        far.into_iter().take(8).map(|x| x.1).collect()
    }

    fn coop_start_wave(&mut self, now: Millis) {
        let Some(c) = self.coop.as_ref() else {
            return;
        };
        let w = c.wave;
        let Some(wave) = c.waves.get(w).cloned() else {
            return;
        };
        // DC-DEFAULT: DSV-584 (proposed) a round world's banner counts to the difficulty's last round
        let total = if c.round_world {
            c.plan.last_round()
        } else {
            c.waves.len()
        };
        let goal = c.goal;
        let round_no = c.plan.rounds.get(w).map(|r| r.round);
        // fixed spots of a ROM row: its spawner names
        let mut spots: Vec<SpawnSpot> = Vec::new();
        if let Some(r) = c.plan.rounds.get(w) {
            for name in &r.spawners {
                if let Some(s) = self
                    .coop_world
                    .spawners
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(name.as_bytes()))
                {
                    spots.push(SpawnSpot {
                        pos: s.pos,
                        yaw: s.rot[1],
                        template: s.target.clone(),
                    });
                }
            }
        }
        if spots.is_empty() {
            spots = self.nearest_spots();
        }
        let mut queue: Vec<Pending> = Vec::new();
        for g in &wave.groups {
            for i in 0..g.count {
                queue.push(Pending {
                    template: g.template.clone(),
                    due: now.wrapping_add(i.saturating_mul(g.every_ms)),
                });
            }
        }
        // interleave the groups by due time (stable)
        queue.sort_by_key(|p| since(p.due, now));
        let n = queue.len();
        if let Some(c) = self.coop.as_mut() {
            c.queue = queue.into();
            c.spots = spots;
            c.next_spot = 0;
            c.phase = WavePhase::Spawning;
        }
        let secs_left = self.round_secs_left(now);
        // SPEC: FIRETEAM_2009.md §2.4 (0xc9 sub 1 `a` = stage / wave, `b` = goal; sub 2 banner)
        // DC-DEFAULT: FT-S7 (proposed) the wave HUD: an empty goal list, the round's seconds as the mission timer,
        // `a` = the wave number, `b` = the difficulty; a "Wave n / N" banner for 3 s
        let timers: Vec<f32> = if secs_left > 0 {
            vec![secs_left as f32]
        } else {
            Vec::new()
        };
        let list = gs::coop_mission_list(&[], &timers, (w + 1).min(255) as u8, goal as u8);
        self.broadcast(&list);
        let text = format!("Wave {} / {}", w + 1, total);
        self.broadcast(&gs::coop_banner(text.as_bytes(), 3.0));
        self.log(format!("co-op: wave {} of {total} starts ({n} AI)", w + 1));
        // DC-DEFAULT: DSV-583 (proposed) the GMS weapon rows of this round lie at their places
        if let Some(r) = round_no {
            self.coop_weapon_rows(r, now);
        }
    }

    /// The next free spawn spot of the wave (round-robin; a spot near a living human or a living AI is skipped).
    // DC-DEFAULT: FT-10 (proposed) skip within 256 units of a player or 48 units of an AI
    fn take_spot(&mut self) -> Option<SpawnSpot> {
        let humans = self.living_humans();
        let clear = self.cfg.coop.spawn_clear;
        let c = self.coop.as_mut()?;
        let n = c.spots.len();
        for _ in 0..n {
            let s = c.spots[c.next_spot % n].clone();
            c.next_spot = (c.next_spot + 1) % n;
            let near_ai = c
                .ais
                .iter()
                .any(|a| a.state == AiState::Alive && len3(sub3(a.pos, s.pos)) < 48.0);
            let near_human = humans.iter().any(|h| len3(sub3(h.1, s.pos)) < clear);
            if !near_ai && !near_human {
                return Some(s);
            }
        }
        None
    }

    /// Where an AI stands at a spot: on the floor below it when the trace world has one within reach.
    // DC-DEFAULT: FT-S5 (proposed) drop to the floor within 1024 units
    fn floor_at(&self, pos: [f32; 3], half_h: f32) -> [f32; 3] {
        let Some(w) = self.trace.as_ref() else {
            return pos;
        };
        let from = add3(pos, [0.0, 16.0, 0.0]);
        let to = add3(pos, [0.0, -1024.0, 0.0]);
        match w.ray(dc_sim::V3::from_array(from), dc_sim::V3::from_array(to)) {
            Some(h) => {
                let y = from[1] + (to[1] - from[1]) * h.t.clamp(0.0, 1.0);
                [pos[0], y + half_h + 1.0, pos[2]]
            }
            None => pos,
        }
    }

    /// Spawns one AI of the wave at a spot: a hidden slot is reused (new body by `CF_FILES` when the spec differs),
    /// else a new slot is created while the pool has room.
    // DC-DEFAULT: FT-R1 (proposed) hide and reuse; FT-2 (proposed) the pool
    fn coop_spawn_ai(
        &mut self,
        template: Option<Vec<u8>>,
        spot: &SpawnSpot,
        name: Option<Vec<u8>>,
        now: Millis,
    ) -> bool {
        let want = template.or_else(|| spot.template.clone());
        let Some(spec) = self.ai_spec(want.as_deref()) else {
            return false;
        };
        let pool = self.cfg.coop.max_alive as usize;
        let dims = spec
            .look
            .assets
            .rig
            .stand_extent
            .unwrap_or(self.cfg.player_dims);
        let pos = self.floor_at(spot.pos, dims[1]);
        let rate = self.coop.as_ref().map_or(1.0, |c| {
            let r = &c.plan.health_rates;
            r.get(c.wave).or(r.last()).copied().unwrap_or(1.0)
        });
        let max_health = (spec.max_health * rate).max(1.0);
        let fire_ms = self.cfg.coop.fire_ms;
        let Some(c) = self.coop.as_mut() else {
            return false;
        };
        let wave = c.wave;
        let reuse = c
            .ais
            .iter()
            .position(|a| a.state == AiState::Hidden && a.spec.key == spec.key)
            .or_else(|| c.ais.iter().position(|a| a.state == AiState::Hidden));
        let slot = match reuse {
            Some(i) => i,
            None if c.ais.len() < pool => c.ais.len(),
            None => return false,
        };
        let fresh = AiChar {
            slot,
            obj: None,
            weapon_obj: None,
            caption: spec.caption.clone(),
            kind: spec.kind,
            spec: spec.clone(),
            health: max_health,
            max_health,
            pos,
            vel: [0.0; 3],
            yaw: spot.yaw,
            motion: Motion::Stand,
            motion_since: now,
            spawned_at: now,
            state: AiState::Alive,
            wave,
            name: name.unwrap_or_else(|| want.clone().unwrap_or_default()),
            template: want.clone(),
            target: None,
            next_fire: now.wrapping_add(fire_ms),
            burst_left: 0,
            met: false,
            struck_at: None,
            rushing: false,
            waypoint: None,
            repath_at: now,
            shots: 0,
            hits: 0,
        };
        if slot == c.ais.len() {
            c.ais.push(fresh);
            c.events.push(format!("spawn new slot {slot}"));
            self.out.push(GameOut::AiCreate { slot });
        } else {
            let a = &mut c.ais[slot];
            let new_body = a.spec.key != spec.key;
            *a = AiChar {
                obj: a.obj,
                weapon_obj: a.weapon_obj,
                // the client keeps the create's caption and kind byte (FIRETEAM §1.5); the bar follows the kind
                caption: a.caption.clone(),
                kind: a.kind,
                ..fresh
            };
            let obj = a.obj;
            c.events.push(format!("spawn reused slot {slot}"));
            if new_body {
                self.out.push(GameOut::AiLook { slot });
            }
            self.out.push(GameOut::AiShow { slot });
            if let Some(o) = obj {
                self.broadcast(&gs::npc_health_note(o, gs::npc_health_fraction(1.0, 1.0)));
            }
        }
        true
    }

    /// Wave spawning, the end of a wave, the next wave and the end of the mission.
    fn coop_tick_waves(&mut self, now: Millis) {
        let Some(c) = self.coop.as_ref() else {
            return;
        };
        match c.phase {
            WavePhase::Idle | WavePhase::Done => {}
            WavePhase::Next { at } => {
                if !later(at, now) {
                    self.coop_start_wave(now);
                }
            }
            WavePhase::Spawning => {
                let cap = c
                    .waves
                    .get(c.wave)
                    .map_or(self.cfg.coop.max_alive, |w| w.max_alive)
                    as usize;
                loop {
                    let Some(c) = self.coop.as_ref() else {
                        return;
                    };
                    let due = c.queue.front().is_some_and(|p| !later(p.due, now));
                    if !due || c.alive() >= cap {
                        break;
                    }
                    // a pool slot must be free (a dead AI keeps its slot for the corpse time)
                    let pool = self.cfg.coop.max_alive as usize;
                    let free =
                        c.ais.len() < pool || c.ais.iter().any(|a| a.state == AiState::Hidden);
                    if !free {
                        break;
                    }
                    let template = c.queue.front().and_then(|p| p.template.clone());
                    let Some(spot) = self.take_spot() else {
                        break;
                    };
                    if !self.coop_spawn_ai(template, &spot, None, now) {
                        break;
                    }
                    if let Some(c) = self.coop.as_mut() {
                        c.queue.pop_front();
                    }
                }
                let Some(c) = self.coop.as_ref() else {
                    return;
                };
                if c.queue.is_empty() && c.alive() == 0 && c.mission_queue.is_empty() {
                    let last = c.wave + 1 >= c.waves.len();
                    let n = c.wave + 1;
                    // DC-DEFAULT: DSV-576 (proposed) a round world's cleared round runs the round-end event of its
                    // number (caption, heal, the next round's box, the difficulty's end); the world starts the next
                    let end = c
                        .round_world
                        .then(|| {
                            let r = self.coop_world.rounds.as_ref()?;
                            r.clear_events
                                .iter()
                                .find(|e| e.0 as usize == n)
                                .map(|e| e.1.clone())
                        })
                        .flatten();
                    if let Some(cmd) = end {
                        if let Some(c) = self.coop.as_mut() {
                            c.waves_cleared += 1;
                            c.phase = WavePhase::Idle;
                            // DC-DEFAULT: DSV-593 (proposed) the cleared round owes its recovery
                            c.recovery_owed = true;
                        }
                        self.out.push(GameOut::WorldScript(cmd));
                        self.log(format!(
                            "co-op: round {n} cleared (the world's round end runs)"
                        ));
                        return;
                    }
                    let at = now.wrapping_add(self.cfg.coop.wave_break_ms);
                    if let Some(c) = self.coop.as_mut() {
                        c.waves_cleared += 1;
                        if last {
                            c.phase = WavePhase::Done;
                        } else {
                            c.wave += 1;
                            c.phase = WavePhase::Next { at };
                        }
                    }
                    self.log(format!("co-op: wave {n} cleared"));
                    if last {
                        self.coop_finish(now, STAGE_COMPLETE);
                    }
                }
            }
        }
    }

    /// The mission is over: complete, failed or timed out (the end set of the round module follows).
    // DC-DEFAULT: FT-15 (proposed) end conditions; GMD-18 (proposed)
    pub(super) fn coop_finish(&mut self, now: Millis, stage: u8) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        if let Some(c) = self.coop.as_mut() {
            c.stage = Some(stage);
            c.phase = WavePhase::Done;
        }
        self.log(format!("co-op: mission over (stage {stage})"));
        let win = if stage == STAGE_COMPLETE { 1 } else { 2 };
        self.end_round(now, win);
    }

    /// Failure when no human is alive and none has a life left.
    // DC-DEFAULT: FT-15 (proposed)
    fn coop_check_failure(&mut self, now: Millis) {
        let humans: Vec<&super::Player> = self
            .players
            .values()
            .filter(|p| p.introduced && p.spawns > 0)
            .collect();
        if humans.is_empty() {
            return;
        }
        let out = humans
            .iter()
            .all(|p| p.life != Life::Alive && self.coop_lives_of(p.pid) == 0);
        if out {
            self.coop_finish(now, STAGE_FAILED);
        }
    }

    /// The co-op block of the round result: `c1` team points, `c2` waves cleared, `c3` AIs killed, `c4` human
    /// deaths, `c5` 0, `c6` mission seconds; for a completed mission the humans' names.
    // SPEC: FIRETEAM_2009.md §2.6 (stages, field targets; labels UNPROVEN, V-FT-11)
    // DC-DEFAULT: FT-13 (proposed) the six values; FT-S8 (proposed) names = the humans, panel 5000 ms
    pub(super) fn coop_result(&self, now: Millis) -> gs::CoopResult {
        let Some(c) = self.coop.as_ref() else {
            return gs::CoopResult::default();
        };
        let stage = c.stage.unwrap_or(STAGE_FAILED);
        let secs = c.started.map_or(0, |s| since(now, s) / 1000);
        gs::CoopResult {
            stage,
            values: [
                c.total_points(),
                c.waves_cleared,
                c.ai_killed,
                c.total_deaths(),
                0,
                secs,
            ],
            names: if stage == STAGE_COMPLETE {
                self.players
                    .values()
                    .filter(|p| p.introduced)
                    .map(|p| p.name8.clone())
                    .collect()
            } else {
                Vec::new()
            },
            title: Vec::new(),
            text: Vec::new(),
            panel_ms: 5_000,
        }
    }

    /// The co-op block of a player's summary: own points, own AI kills, own points, own deaths.
    // DC-DEFAULT: FT-13 (proposed)
    pub(super) fn coop_summary(&self, pid: u8) -> [u32; 4] {
        let k = self.player_key(pid);
        let Some(c) = self.coop.as_ref() else {
            return [0; 4];
        };
        let pts = c.points.get(&k).copied().unwrap_or(0);
        [
            pts,
            c.kills.get(&k).copied().unwrap_or(0),
            pts,
            c.deaths.get(&k).copied().unwrap_or(0),
        ]
    }

    // ------------------------------------------------------------------------------------------- AI steps

    /// One AI step: a dead AI's slot is freed after the corpse time; living AIs pick the nearest living human, walk
    /// to it and shoot it.
    pub(super) fn tick_coop(&mut self, now: Millis) {
        if self.coop.is_none() || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let step = self.cfg.coop.step_ms.max(1);
        let dt_ms = match self.coop.as_ref().and_then(|c| c.last_step) {
            Some(t) => since(now, t),
            None => step,
        };
        if dt_ms < step {
            return;
        }
        if let Some(c) = self.coop.as_mut() {
            c.last_step = Some(now);
        }
        let dt = (dt_ms.min(200) as f32) / 1000.0;
        // DC-DEFAULT: DSV-576 (proposed) a round world makes the statements' spawns beside its waves
        if self.coop.as_ref().is_some_and(|c| c.network) {
            self.coop_tick_mission(now);
        }
        if self.coop.as_ref().is_some_and(|c| !c.mission) {
            self.coop_tick_waves(now);
        }
        let n = self.coop.as_ref().map_or(0, |c| c.ais.len());
        // DC-DEFAULT: FT-S11 (proposed) the navigation grid grows and the step's plans open before the AI move
        self.ai_nav_begin(now);
        for slot in 0..n {
            self.ai_step(slot, dt, now);
        }
        self.ai_nav_end();
        // DC-DEFAULT: DSV-599 / DSV-600 (proposed) the dead AIs' dropped items and clouds
        self.coop_tick_hazards(now);
        if matches!(self.phase, RoundPhase::Running { .. }) {
            self.coop_check_failure(now);
        }
    }

    /// A clear segment of the trace world (no world: clear).
    fn sight_clear(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        match self.trace.as_ref() {
            Some(w) => w
                .ray(dc_sim::V3::from_array(a), dc_sim::V3::from_array(b))
                .is_none(),
            None => true,
        }
    }

    /// A waypoint toward `target` among the world's AI nodes: of the 24 nodes nearest to the AI, the one with a
    /// clear line from the AI that is closest to the target by the two-leg distance.
    // DC-DEFAULT: FT-S5 (proposed) node hop steering (no graph search in slice 1)
    fn pick_waypoint(&self, eye: [f32; 3], at: [f32; 3], target: [f32; 3]) -> Option<[f32; 3]> {
        let mut near: Vec<(f32, [f32; 3])> = self
            .coop_world
            .nodes
            .iter()
            .map(|n| (len3(sub3(*n, at)), *n))
            .filter(|(d, _)| *d > 48.0 && *d < 2000.0)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        near.truncate(24);
        let here = len3(sub3(target, at));
        near.into_iter()
            .filter(|(d, n)| *d + len3(sub3(target, *n)) < here * 1.6 + 64.0)
            .filter(|(_, n)| self.sight_clear(eye, add3(*n, [0.0, eye[1] - at[1], 0.0])))
            .min_by(|a, b| {
                let ka = a.0 + len3(sub3(target, a.1));
                let kb = b.0 + len3(sub3(target, b.1));
                ka.total_cmp(&kb)
            })
            .map(|x| x.1)
    }

    fn ai_step(&mut self, slot: usize, dt: f32, now: Millis) {
        let Some(a) = self.coop.as_ref().and_then(|c| c.ais.get(slot)).cloned() else {
            return;
        };
        match a.state {
            AiState::Hidden => return,
            AiState::Dead { at } => {
                // the objects were hidden at the death (DSV-585): the slot is free for the next spawn
                if since(now, at) >= self.cfg.coop.corpse_ms {
                    if let Some(c) = self.coop.as_mut() {
                        c.ais[slot].state = AiState::Hidden;
                        c.events.push(format!("hide slot {slot}"));
                    }
                }
                return;
            }
            // the spawn tick: the create record goes out with the spawn position
            AiState::Alive if a.spawned_at == now => return,
            AiState::Alive => {}
        }
        // DC-DEFAULT: DSV-572 (proposed) frozen AI stand and hold fire
        if self.coop.as_ref().is_some_and(|c| c.frozen) {
            if let Some(x) = self.coop.as_mut().and_then(|c| c.ais.get_mut(slot)) {
                x.vel = [0.0; 3];
                if x.motion != Motion::Stand {
                    x.motion = Motion::Stand;
                    x.motion_since = now;
                }
            }
            return;
        }
        let cfg = self.cfg.coop.clone();
        let dims = a.dims(self.cfg.player_dims);
        let eye = add3(a.pos, [0.0, dims[1] * 0.8, 0.0]);
        let target = self
            .living_humans()
            .into_iter()
            .min_by(|x, y| len3(sub3(x.1, a.pos)).total_cmp(&len3(sub3(y.1, a.pos))));
        let mut new = a.clone();
        new.vel = [0.0; 3];
        let mut moving = false;
        if let Some((tpid, tpos)) = target {
            new.target = Some(tpid);
            let dist = len3(sub3(tpos, a.pos));
            let teye = add3(tpos, [0.0, self.dims_of(tpid)[1] * 0.8, 0.0]);
            let los = self.sight_clear(eye, teye);
            // DC-DEFAULT: DSV-642 (proposed) a human within the shot range has met it
            new.met = a.met || dist <= cfg.shot_range;
            // DC-DEFAULT: DSV-597 (proposed) a human in sight within the rush distance starts the rush, for good
            new.rushing = a.rushing || a.spec.rush.is_some_and(|r| los && dist <= r);
            if let Some(d) = norm2(sub3(tpos, a.pos)) {
                new.yaw = d[0].atan2(d[2]);
            }
            // DC-DEFAULT: DSV-589 (proposed) a melee AI strikes within its reach and closes to `MELEE_CLOSE` of it;
            // DSV-590 (proposed) an AI whose template's hand slot is empty does not attack
            let (reach, hold) = match a.spec.melee {
                Some(r) => (r, r * MELEE_CLOSE),
                None => (cfg.shot_range, cfg.hold_range),
            };
            if !a.spec.unarmed && los && dist <= reach && !later(a.next_fire, now) {
                let (left, wait) = self.ai_fire_wait(&a, cfg.fire_ms, cfg.bursts);
                new.burst_left = left;
                new.next_fire = now.wrapping_add(wait);
                if a.spec.melee.is_some() {
                    new.struck_at = Some(now);
                }
                self.ai_fire(slot, &a, tpid, eye, teye, dist, now);
                if let Some(c) = self.coop.as_ref() {
                    new.shots = c.ais[slot].shots;
                    new.hits = c.ais[slot].hits;
                }
            }
            if !(los && dist <= hold) {
                // DC-DEFAULT: FT-S11 (proposed) the way over the navigation grid when it has one; else (FT-S5) the
                // human in sight, else a waypoint toward it
                let goal = match self.ai_nav_waypoint(&a, tpos, now) {
                    NavGoal::Way(w) => {
                        new.waypoint = None;
                        Some(w)
                    }
                    // no way leads from here to the human: the AI holds its place (and fights from there)
                    NavGoal::Hold => None,
                    NavGoal::Steer if los => {
                        new.waypoint = None;
                        Some(tpos)
                    }
                    NavGoal::Steer => {
                        if a.waypoint.is_none_or(|w| len3(sub3(w, a.pos)) < 48.0)
                            || !later(a.repath_at, now)
                        {
                            new.waypoint = self.pick_waypoint(eye, a.pos, tpos);
                            new.repath_at = now.wrapping_add(1000);
                        }
                        Some(new.waypoint.unwrap_or(tpos))
                    }
                };
                // DC-DEFAULT: DSV-588 (proposed) the AI's own speed; DSV-597 (proposed) its walk before the rush
                let speed = specials::step_speed(&a.spec, new.rushing);
                let stride = speed * dt;
                let walked = match goal {
                    Some(goal) => self.ai_nav_walk(&a, goal, stride, now),
                    None => NavMove::Blocked,
                };
                match walked {
                    // FT-S11: the body over the navigation world
                    NavMove::Moved(p, d) => {
                        new.vel = scale3(d, speed);
                        new.pos = p;
                        new.yaw = d[0].atan2(d[2]);
                        moving = true;
                    }
                    NavMove::Blocked => {}
                    NavMove::Off => {
                        if let Some(dir) = goal.and_then(|g| norm2(sub3(g, a.pos))) {
                            let reach = stride + dims[0].max(dims[2]) + 4.0;
                            let chest = add3(a.pos, [0.0, dims[1] * 0.3, 0.0]);
                            let knee = add3(a.pos, [0.0, -dims[1] * 0.5, 0.0]);
                            let free = |d: [f32; 3]| {
                                self.sight_clear(chest, add3(chest, scale3(d, reach)))
                                    && self.sight_clear(knee, add3(knee, scale3(d, reach)))
                            };
                            let tries = [0.0f32, 0.785, -0.785, 1.571, -1.571];
                            if let Some(d) =
                                tries.iter().map(|t| rotate_y(dir, *t)).find(|d| free(*d))
                            {
                                let mut p = add3(a.pos, scale3(d, stride));
                                p = self.follow_floor(p, dims[1]);
                                new.vel = scale3(d, speed);
                                new.pos = p;
                                new.yaw = d[0].atan2(d[2]);
                                moving = true;
                            }
                        }
                    }
                }
            }
        } else {
            new.target = None;
        }
        let m = if moving { Motion::Walk } else { Motion::Stand };
        if m != new.motion {
            new.motion = m;
            new.motion_since = now;
        }
        if let Some(c) = self.coop.as_mut() {
            if let Some(x) = c.ais.get_mut(slot) {
                if x.state == AiState::Alive {
                    *x = new;
                }
            }
        }
    }

    /// The wait after an AI's shot and the shots left in its burst (DSV-640): with its weapon row's burst a shot
    /// that starts a burst draws the burst's size; the burst's shots are `fire_ms` apart and its last one is followed
    /// by a pause drawn from the row's range (never shorter than `fire_ms`). Without a burst (or with `bursts` off):
    /// `fire_ms`, no burst.
    // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] AIMinBurstShots, AIMaxBurstShots, AIMinBurstInterval,
    // AIMaxBurstInterval (the launcher and bolt rifle rows: one shot, a pause of whole seconds)
    // DC-DEFAULT: DSV-640 (proposed) uniform draws, the pause at least `fire_ms`
    fn ai_fire_wait(&mut self, a: &AiChar, fire_ms: u32, bursts: bool) -> (u32, u32) {
        let Some(b) = a.spec.burst.filter(|_| bursts) else {
            return (0, fire_ms);
        };
        let left = match a.burst_left {
            0 => {
                let span = u64::from(b.shots.1 - b.shots.0) + 1;
                b.shots.0 + (self.next_rand() % span) as u32
            }
            n => n,
        }
        .saturating_sub(1);
        if left > 0 {
            return (left, fire_ms);
        }
        let u = (self.next_rand() % 10_001) as f32 / 10_000.0;
        let pause = b.pause_s.0 + (b.pause_s.1 - b.pause_s.0) * u;
        (0, ((pause * 1000.0).round() as u32).max(fire_ms))
    }

    /// The floor under a walking AI: a step up of 18 units, a drop of up to the box height (more: a fall of at most
    /// 600 u/s worth of the step is not modelled; the AI keeps its height when no floor is found).
    // DC-DEFAULT: FT-S5 (proposed)
    fn follow_floor(&self, p: [f32; 3], half_h: f32) -> [f32; 3] {
        let Some(w) = self.trace.as_ref() else {
            return p;
        };
        let from = add3(p, [0.0, -half_h + 18.0, 0.0]);
        let to = add3(p, [0.0, -half_h - half_h, 0.0]);
        match w.ray(dc_sim::V3::from_array(from), dc_sim::V3::from_array(to)) {
            Some(h) => {
                let y = from[1] + (to[1] - from[1]) * h.t.clamp(0.0, 1.0);
                [p[0], y + half_h + 1.0, p[2]]
            }
            None => p,
        }
    }

    /// One AI shot at a human: the shot effect to everyone (shooter = the AI's character object), a hit by the
    /// difficulty's chance (half of it at the end of the range), damage = the weapon row's damage x the difficulty's
    /// `AIDamageFactor`, through the players' damage path (no killer: 0x6d counts the victim's death only).
    // SPEC: FIRETEAM_2009.md §1.8 (shot effect type 0x06 with the AI as shooter: V-FT-8 UNPROVEN; 0x74 carries no
    // attacker); GAME_MESSAGES_2009_S2C.md §3.3.1 (no-killer form)
    // DC-DEFAULT: FT-S5 (proposed) hit chance; FT-9 (proposed) the damage
    #[allow(clippy::too_many_arguments)]
    fn ai_fire(
        &mut self,
        slot: usize,
        a: &AiChar,
        tpid: u8,
        eye: [f32; 3],
        teye: [f32; 3],
        dist: f32,
        now: Millis,
    ) {
        let Some(obj) = a.obj else {
            return;
        };
        let cfg = self.cfg.coop.clone();
        let goal = self.coop.as_ref().map_or(0, |c| c.goal.min(2) as usize);
        let falloff = 1.0 - 0.5 * (dist / cfg.shot_range.max(1.0)).clamp(0.0, 1.0);
        // DC-DEFAULT: DSV-589 (proposed) a melee strike hits by `melee_hit`, at any distance within the reach
        let chance = match a.spec.melee {
            Some(_) => cfg.melee_hit,
            // DC-DEFAULT: DSV-641 (proposed) a human within the attribute row's `FullAccuracyRadius` is always hit
            None if cfg.full_accuracy && a.spec.full_accuracy.is_some_and(|r| dist <= r) => 1.0,
            None => cfg.hit_chance[goal] * falloff,
        };
        let roll = (self.next_rand() % 10_000) as f32 / 10_000.0;
        let hit = roll < chance;
        let d = sub3(teye, eye);
        let l = len3(d).max(1e-3);
        let dir = scale3(d, 1.0 / l);
        let to = if hit {
            teye
        } else {
            // a miss passes the target's side
            let side = norm2([dir[2], 0.0, -dir[0]]).unwrap_or([1.0, 0.0, 0.0]);
            add3(add3(teye, scale3(side, 40.0)), scale3(dir, 200.0))
        };
        let tobj = self.players.get(&tpid).map(|p| p.obj);
        let (weapon, ammo) = a.spec.weapon.unwrap_or((0, 0));
        let m = gs::shot_effect(&gs::ShotEffect {
            first: if hit { tobj } else { None },
            shooter: obj,
            weapon,
            ammo,
            from: eye,
            to,
            normal: [-dir[0], -dir[1], -dir[2]],
            // DC-DEFAULT: DSV-29 (amendment, proposed) a hit on a player is a body hit (the AI aims at the eye
            // point, no zone is judged)
            surface: if hit {
                gs::SURFACE_BODY
            } else {
                gs::TODO_SURFACE_WORLD
            },
            // DC-DEFAULT: DSV-86 (proposed) the hit mask on a character hit; DSV-85 (proposed) an AI has no client id
            mask: if hit {
                self.cfg.hit_mask
            } else {
                gs::SHOT_MASK_MISS
            },
            client: gs::SHOT_CLIENT_NONE,
            mods: gs::NO_SHOT_MODS,
        });
        for to in self.introduced() {
            self.out.push(GameOut::Unreliable(to, m.clone()));
        }
        if let Some(c) = self.coop.as_mut() {
            c.ais[slot].shots += 1;
            if hit {
                c.ais[slot].hits += 1;
            }
        }
        if !hit {
            return;
        }
        let base = a
            .spec
            .weapon
            .and_then(|(w, am)| self.tables.weapons.damage(w, am))
            .unwrap_or(cfg.default_damage);
        let factor = self
            .coop
            .as_ref()
            .and_then(|c| c.plan.difficulty.ai_damage_factor)
            .unwrap_or(1.0);
        let dtype = a
            .spec
            .weapon
            .and_then(|(w, am)| self.tables.weapons.damage_type(w, am))
            .unwrap_or(super::TODO_DAMAGE_ZONE);
        let src = DamageSource { weapon, dtype };
        self.apply_damage(
            tpid,
            None,
            base * factor,
            dir,
            src,
            super::TODO_KILL_ZONE_BODY,
            now,
        );
    }

    // ------------------------------------------------------------------------------------------- hits on AI

    /// The AI slot whose character (or weapon) object is `obj`.
    fn ai_of_obj(&self, obj: u16) -> Option<usize> {
        self.coop.as_ref()?.ais.iter().position(|a| {
            a.state == AiState::Alive && (a.obj == Some(obj) || a.weapon_obj == Some(obj))
        })
    }

    /// The nearest living AI whose box the shot's ray crosses before `limit`.
    pub(super) fn ai_on_ray(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        limit: f32,
    ) -> Option<(usize, f32)> {
        let c = self.coop.as_ref()?;
        let fallback = self.cfg.player_dims;
        c.ais
            .iter()
            .filter(|a| a.state == AiState::Alive && a.obj.is_some())
            .filter_map(|a| {
                ray_box(origin, dir, limit, a.pos, a.dims(fallback)).map(|t| (a.slot, t))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))
    }

    /// `UP_FIRE` in a co-op room: a hit on an AI (by the client's `hit_obj`, or, with `ray_hits`, by the server's ray
    /// test of the shot when the client named no AI), validated like a player hit (box, origin, wall), damage by
    /// the DSV-30 rule with the AI body's hit nodes. Returns true when the shot was an AI hit (handled).
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1.1 (`hit_obj`, hit point = origin + dist x dir); FIRETEAM_2009.md §1.8
    // (V-FT-7 UNPROVEN)
    // DC-DEFAULT: DSV-30 (proposed) the damage rule; DSV-34 (proposed) the wall trace; FT-S6 (proposed) ray hits
    pub(super) fn coop_on_fire(
        &mut self,
        pid: u8,
        f: &UpFire,
        wall: Option<f32>,
        now: Millis,
    ) -> bool {
        if self.coop.is_none() {
            return false;
        }
        let dl = len3(f.dir);
        if !(dl.is_finite() && dl > 1e-6) || !finite3(f.origin) {
            return false;
        }
        let dir = [f.dir[0] / dl, f.dir[1] / dl, f.dir[2] / dl];
        let shooter_pos = match self.players.get(&pid) {
            Some(p) => p.pos,
            None => return false,
        };
        let named = f
            .hit
            .and_then(|(d, o)| o.and_then(|o| self.ai_of_obj(o)).map(|s| (s, d)));
        let (slot, dist, by_ray) = match named {
            Some((s, d)) => (s, d, false),
            None => {
                if !self.cfg.coop.ray_hits {
                    return false;
                }
                // a player hit stays a player hit
                if f.hit.is_some_and(|(_, o)| {
                    o.is_some_and(|o| self.players.values().any(|p| p.obj == o))
                }) {
                    return false;
                }
                let world = self
                    .wall_along(f.origin, dir, super::TODO_MISS_TRACE_UNITS)
                    .unwrap_or(super::TODO_MISS_TRACE_UNITS);
                let limit = f
                    .hit
                    .map_or(world, |(d, _)| d.min(world) + self.cfg.trace_slack);
                match self.ai_on_ray(f.origin, dir, limit) {
                    Some((s, t)) => (s, t, true),
                    None => return false,
                }
            }
        };
        let Some(a) = self.coop.as_ref().map(|c| c.ais[slot].clone()) else {
            return false;
        };
        let dims = a.dims(self.cfg.player_dims);
        let hit = add3(f.origin, scale3(dir, dist));
        let tol = self.cfg.hit_tolerance;
        let inside = (0..3).all(|i| (hit[i] - a.pos[i]).abs() <= dims[i] + tol);
        if !inside || len3(sub3(f.origin, shooter_pos)) > self.cfg.origin_tolerance {
            self.log(format!(
                "client {pid}: hit on AI slot {slot} refused (point {:.0} from the box centre)",
                len3(sub3(hit, a.pos))
            ));
            return true;
        }
        if !by_ray {
            if let Some(w) = wall.filter(|w| *w + self.cfg.trace_slack < dist) {
                self.log(format!(
                    "client {pid}: hit on AI slot {slot} refused (a wall {w:.0} units along a {dist:.0}-unit shot)"
                ));
                return true;
            }
        }
        let head = hit[1] >= a.pos[1] + dims[1] - 2.0 * dims[1] * self.cfg.head_fraction;
        let (factor, zone) = self.ai_hit_zone(&a, f, head, by_ray);
        let base = self.hit_damage(pid, f);
        let dmg = base * factor;
        if !(dmg.is_finite() && dmg > 0.0) {
            return true;
        }
        self.ai_damage(slot, pid, dmg, zone, now);
        true
    }

    /// The zone factor of a hit on an AI: the AI body's skeleton node the client named (not for ray hits), else the
    /// box geometry.
    // DC-DEFAULT: DSV-30 (proposed)
    fn ai_hit_zone(&self, a: &AiChar, f: &UpFire, head: bool, by_ray: bool) -> (f32, u8) {
        let geometry = if head {
            (self.cfg.head_factor, gs::KILL_ZONE_HEAD)
        } else {
            (self.cfg.body_factor, super::TODO_KILL_ZONE_BODY)
        };
        let node = (!by_ray && self.cfg.zone_factors_from_rom)
            .then(|| self.ai_hit_node(a, f, geometry.1))
            .flatten();
        let worn = node.unwrap_or(geometry);
        // DC-DEFAULT: DSV-601 (proposed) a substitute body's hit counts by the AI's own skeleton row
        match a.spec.own_nodes.as_deref() {
            Some(own) => (specials::own_factor(own, worn, node.is_some()), worn.1),
            None => worn,
        }
    }

    /// The factor and location of the skeleton node a client-reported hit named on the body the AI wears (a node
    /// without a location keeps `zone`).
    fn ai_hit_node(&self, a: &AiChar, f: &UpFire, zone: u8) -> Option<(f32, u8)> {
        let hit_obj = f.hit.and_then(|h| h.1);
        f.extra
            .filter(|x| x.obj.is_some() && x.obj == hit_obj)
            .and_then(|x| a.spec.look.assets.hit_nodes.get(x.kind as usize).copied())
            .map(|n| (n.factor, if n.location == 0 { zone } else { n.location }))
    }

    /// Points of a hit by zone (head, body, arms, legs) and of a kill, x the difficulty's `ScoreFactor`.
    // DC-DEFAULT: FT-13 (proposed) ROM `[CooperativeModeScoreTable]`, defaults 10 / 8 / 5 / 3 and kill 40 (boss 500),
    // head kill + 50
    fn coop_hit_points(&self, zone: u8) -> u32 {
        let Some(c) = self.coop.as_ref() else {
            return 0;
        };
        let i = match zone {
            1 => 0,
            3 => 2,
            4 => 3,
            _ => 1,
        };
        let def = [10.0, 8.0, 5.0, 3.0][i];
        let f = c.plan.difficulty.score_factor.unwrap_or(1.0);
        (c.plan.score.hit[i].unwrap_or(def) * f).round().max(0.0) as u32
    }

    /// The points of a killing blow: the kill (or boss death) and, for a head kill, its extra, all x the factor
    /// (the ROM's); plus the lethal hit's own zone points and, when the kill came inside the player's kill chain
    /// (`combo`, the 0xc8 combo above 1), the combo bonus x the factor.
    // OBSERVED: ROM table keys GMS [CooperativeModeScoreTable] DamagePlusByEtcDeath / ...BossDeath / ...HeadShotDeath /
    // DamagePlusByCombo and the difficulty row's ScoreFactor (the keys and the factor are the ROM's)
    // DC-DEFAULT: DSV-605 (proposed; amends FT-13, DSV-602) USER-PROVIDED OBSERVATION 2026-11-05: a December 2009
    // recording shows +33 and +42 per kill (24 + 9, 24 + 18); that the lethal hit also pays and that the bonus is paid
    // by every kill of a chain (not once per chain) is our reading of it
    fn coop_kill_points(&self, boss: bool, head: bool, zone: u8, combo: bool) -> u32 {
        let Some(c) = self.coop.as_ref() else {
            return 0;
        };
        let s = &c.plan.score;
        let base = if boss {
            s.boss_death.unwrap_or(500.0)
        } else {
            s.kill.unwrap_or(40.0)
        };
        let extra = if head {
            s.head_kill.unwrap_or(50.0)
        } else {
            0.0
        };
        let f = c.plan.difficulty.score_factor.unwrap_or(1.0);
        let bonus = if combo {
            (s.combo.unwrap_or(30.0) * f).round().max(0.0) as u32
        } else {
            0
        };
        let kill = ((base + extra) * f).round().max(0.0) as u32;
        kill.saturating_add(self.coop_hit_points(zone))
            .saturating_add(bonus)
    }

    /// Damage to an AI: its health, the bar of every client (0xa6 sub 0x24), the shooter's points (0xd7 sub 3 for
    /// a hit, sub 1 with the record total for a kill), the kill credit; a dead AI is hidden and its corpse lies in its
    /// place (DSV-585).
    // SPEC: FIRETEAM_2009.md §1.5 (sub 0x24), §2.2 rows 1 and 3 (points), §1.7 (no death message for an AI, no kill
    // feed in class 5)
    // DC-DEFAULT: FT-3 (proposed; per hit, no coalescing in slice 1); FT-13 (proposed); FT-R1 (proposed)
    fn ai_damage(&mut self, slot: usize, pid: u8, dmg: f32, zone: u8, now: Millis) {
        self.ai_hurt(slot, Some(pid), dmg, zone, now);
    }

    /// [`Game::ai_damage`] by a player (`by`) or by nothing (an AI's blast, a breakable no player broke): without a
    /// player no points, no kill credit and no points message.
    pub(in crate::game) fn ai_hurt(
        &mut self,
        slot: usize,
        by: Option<u8>,
        dmg: f32,
        zone: u8,
        now: Millis,
    ) {
        let key = by.map(|p| self.player_key(p));
        let Some(a) = self.coop.as_mut().and_then(|c| c.ais.get_mut(slot)) else {
            return;
        };
        if a.state != AiState::Alive {
            return;
        }
        a.health = (a.health - dmg).max(0.0);
        let lethal = a.health <= 0.0;
        let (obj, frac, boss) = (
            a.obj,
            gs::npc_health_fraction(a.health, a.max_health),
            a.kind == NPC_KIND_BOSS,
        );
        if lethal {
            a.state = AiState::Dead { at: now };
            a.vel = [0.0; 3];
            a.motion = Motion::Stand;
            a.motion_since = now;
            // the death shown as a player's (DSV-44): the AI's objects leave the world at once (a body left standing
            // is drawn and hit on the clients while the server takes no more damage on it: the owner's
            // "unlimited health", 2026-11-04) and a corpse plays the body's death clip in its place
            // DC-DEFAULT: DSV-585 (proposed; amends FT-4 and FT-S5's standing corpse)
            let clip =
                a.spec
                    .look
                    .anims
                    .death_clip(zone == gs::KILL_ZONE_HEAD, [0.0, 0.0, 1.0], a.yaw);
            let (pos, yaw) = (a.pos, a.yaw);
            self.out.push(GameOut::AiDied {
                slot,
                pos,
                yaw,
                clip,
            });
        }
        if let Some(o) = obj {
            self.broadcast(&gs::npc_health_note(o, frac));
        }
        // the killer's place in its kill chain (DSV-105 / DSV-602): the combo count of the banner and the bonus
        let combo = match (by, lethal) {
            (Some(pid), true) => self.chain_step(pid, now),
            _ => None,
        };
        let gained = match (key, lethal) {
            (None, _) => 0,
            (Some(_), true) => self.coop_kill_points(
                boss,
                zone == gs::KILL_ZONE_HEAD,
                zone,
                combo.is_some_and(|c| c > 1),
            ),
            (Some(_), false) => self.coop_hit_points(zone),
        };
        let total = {
            let Some(c) = self.coop.as_mut() else {
                return;
            };
            let total = key.map_or(0, |k| {
                let t = c.points.entry(k).or_default();
                *t = t.saturating_add(gained);
                *t
            });
            if lethal {
                c.ai_killed += 1;
                if let Some(k) = key {
                    *c.kills.entry(k).or_default() += 1;
                }
                c.events.push(match by {
                    Some(pid) => format!("kill slot {slot} by client {pid}"),
                    None => format!("kill slot {slot} by no player"),
                });
            }
            total
        };
        if lethal {
            // DC-DEFAULT: DSV-571 (proposed) a mission AI's death runs its template's death command
            // DC-DEFAULT: DSV-576 (proposed) also in a round world (the kill counter)
            let death = self.coop.as_ref().filter(|c| c.network).and_then(|c| {
                let t = c.ais.get(slot)?.template.clone()?;
                let cmd = self.coop_world.template(&t)?.death_cmd.clone();
                (!cmd.is_empty()).then_some(cmd)
            });
            if let Some(cmd) = death {
                self.out.push(GameOut::WorldScript(cmd));
            }
            // DC-DEFAULT: DSV-591 (proposed) the dead AI's brain row decides what falls at its body
            self.coop_ai_drops(slot, now);
            // DC-DEFAULT: DSV-598..DSV-600 (proposed) and what goes off there
            self.coop_death_specials(slot, now);
            let Some(pid) = by else {
                self.log(format!("co-op: AI slot {slot} killed by no player"));
                return;
            };
            if let Some(u) = self.players.get(&pid).and_then(|p| p.uid) {
                let s = self.stats.entry(u).or_default();
                s.kills += 1;
                if zone == gs::KILL_ZONE_HEAD {
                    s.headshots += 1;
                }
            }
            self.broadcast(&gs::coop_points(u32::from(pid), obj, gained, total));
            // the killer's own banner: its AI kill count, the head flag of the lethal hit, the combo count
            // DC-DEFAULT: DSV-602 (proposed) 0xc8 per AI kill to the killer; the kill chain of DSV-105 (window 4 s)
            let count = self
                .coop
                .as_ref()
                .zip(key)
                .map_or(0, |(c, k)| c.kills.get(&k).copied().unwrap_or(0));
            if let Some(combo) = combo {
                self.send(
                    pid,
                    gs::streak_banner(
                        count.min(u32::from(u16::MAX)) as u16,
                        zone == gs::KILL_ZONE_HEAD,
                        u32::from(combo),
                    ),
                );
            }
            self.log(format!(
                "client {pid}: killed AI slot {slot} (+{gained}, {total} points)"
            ));
        } else if let Some(pid) = by {
            self.broadcast(&gs::coop_points_note(
                u32::from(pid),
                gained.min(255) as u8,
                total,
            ));
        }
    }

    /// Kills the living AI of this slot as if `pid` had shot it (the harness entry of the mission tests; the
    /// points, the kill credit and a mission AI's death command follow as for a shot).
    pub fn coop_kill_ai(&mut self, slot: usize, pid: u8, now: Millis) {
        let hp = self
            .ai_chars()
            .get(slot)
            .filter(|a| a.state == AiState::Alive)
            .map(|a| a.health);
        if let Some(h) = hp {
            self.ai_damage(slot, pid, h + 1.0, gs::KILL_ZONE_HEAD, now);
        }
    }

    /// A blast's damage to the living AI (DSV-604): every AI within the radius whose centre the trace world does not
    /// hide (the line-of-sight rule of `-combat blast_los`) takes `full` x the falloff of DSV-39, less the share its
    /// own body's gear row protects of the blast's damage type (a full share: nothing), through the AI damage path at
    /// the body location, credited to `by` (a kill: 0xd7 sub 1 and the kill points as for a shot). Returns the AI hit.
    // OBSERVED: ROM table key MODELBUTES.TXT [AIModel<n>] GearId, WEAPONS.TXT [Gear<n>] protection slots (the infected
    // rows' gear covers their own death weapons' damage types in full, not the players' explosives'; 2026-11-05)
    // DC-DEFAULT: DSV-604 (proposed; amends DSV-598's "other AI take nothing"); DSV-39 / DSV-92 the falloff and the line
    // of sight
    pub(in crate::game) fn blast_ais(&mut self, b: &AiBlast, now: Millis) -> usize {
        let Some(c) = self.coop.as_ref() else {
            return 0;
        };
        let pt = self.cfg.projectile;
        let near: Vec<(usize, [f32; 3], Option<u16>)> = c
            .ais
            .iter()
            .enumerate()
            .filter(|(_, a)| a.state == AiState::Alive)
            .map(|(i, a)| (i, a.pos, a.spec.gear))
            .collect();
        let mut hit = 0;
        for (slot, pos, gear) in near {
            let d = len3(sub3(pos, b.at));
            if !(d.is_finite() && d < b.radius) {
                continue;
            }
            let guard = gear
                .and_then(|g| self.tables.weapons.gear.get(&g))
                .map_or(0.0, |r| r.share(b.dtype, super::TODO_KILL_ZONE_BODY));
            if guard >= 1.0 {
                continue;
            }
            if pt.blast_los && self.projectile_ray(b.lifted, pos).is_some() {
                self.log(format!("blast: AI slot {slot} behind a wall"));
                continue;
            }
            let dmg = b.full
                * super::projectile::blast_factor(d, b.radius, pt.inner_fraction)
                * (1.0 - guard);
            if !(dmg.is_finite() && dmg > 0.0) {
                continue;
            }
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                break;
            }
            self.log(format!(
                "blast: AI slot {slot} takes {dmg:.1} at {d:.0} units (type {})",
                b.dtype
            ));
            self.ai_hurt(slot, b.by, dmg, super::TODO_KILL_ZONE_BODY, now);
            hit += 1;
        }
        hit
    }

    /// The drop rows of the brain row the template of the AI in `slot` names (DSV-591; empty: no brain, no row), its
    /// position and its box.
    pub(super) fn coop_ai_drop_rows(&self, slot: usize) -> Option<AiDropRows> {
        let c = self.coop.as_ref()?;
        let a = c.ais.get(slot)?;
        let rows = a
            .template
            .as_deref()
            .and_then(|t| self.coop_world.template(t))
            .and_then(|t| t.brain.as_deref())
            .and_then(|b| c.tables.brain_named(b))
            .map(|b| b.drops.clone())
            .unwrap_or_default();
        Some((rows, a.pos, a.dims(self.cfg.player_dims)))
    }

    /// Spawns one AI of `template` at `pos` (unit tests of the AI's death).
    #[cfg(test)]
    pub(super) fn coop_spawn_test_ai(
        &mut self,
        template: &[u8],
        pos: [f32; 3],
        now: Millis,
    ) -> bool {
        let spot = SpawnSpot {
            pos,
            yaw: 0.0,
            template: Some(template.to_vec()),
        };
        self.coop_spawn_ai(None, &spot, None, now)
    }

    /// The AI tables of the co-op state (unit tests with synthetic brain rows).
    #[cfg(test)]
    pub(super) fn coop_tables_mut(&mut self) -> Option<&mut AiTables> {
        self.coop.as_mut().map(|c| &mut c.tables)
    }

    // ------------------------------------------------------------------------------------------- session view

    /// The session created the AI's objects.
    pub fn bind_ai(&mut self, slot: usize, obj: u16, weapon_obj: Option<u16>) {
        if let Some(a) = self.coop.as_mut().and_then(|c| c.ais.get_mut(slot)) {
            a.obj = Some(obj);
            a.weapon_obj = weapon_obj;
        }
    }

    pub fn ai_chars(&self) -> &[AiChar] {
        self.coop.as_ref().map_or(&[], |c| c.ais.as_slice())
    }

    /// The NPC tail of an AI's create record.
    // SPEC: FIRETEAM_2009.md §1.2, §1.9
    // DC-DEFAULT: FT-C1 (proposed), FT-K1 (proposed) kind, FT-W1 (proposed) health, GMD-15 (proposed) team 2
    pub fn ai_tail(&self, slot: usize) -> Option<crate::modelobj::NpcBlock> {
        let a = self.ai_chars().get(slot)?;
        let health = if a.state == AiState::Alive {
            gs::npc_health_fraction(a.health, a.max_health)
        } else {
            0.0
        };
        Some(crate::modelobj::NpcBlock {
            npc_rank: self.cfg.coop.npc_rank,
            team: AI_TEAM,
            kind: a.kind,
            health_bits: health.to_bits(),
            caption: a.caption.clone(),
        })
    }

    /// The box half extents of an AI.
    pub fn ai_dims(&self, slot: usize) -> [f32; 3] {
        self.ai_chars()
            .get(slot)
            .map_or(self.cfg.player_dims, |a| a.dims(self.cfg.player_dims))
    }

    /// Stream entries of the visible AIs (character entries with yaw8 and the three trackers, as for players).
    pub(super) fn ai_stream_entries(&self, now: Millis) -> Vec<StreamEntry> {
        self.ai_chars()
            .iter()
            .filter(|a| a.visible())
            .filter_map(|a| {
                let mut anim = a.spec.look.anims.trackers(
                    a.motion,
                    since(now, a.spawned_at),
                    since(now, a.motion_since),
                );
                if let Some(((clip, len), t)) = ai_melee_upper(a, now) {
                    anim[1].anim = clip;
                    anim[1].len_ms = len;
                    anim[1].time_ms = if len > 0 { t % len } else { 0 };
                }
                Some(StreamEntry {
                    obj: a.obj?,
                    pos: a.pos,
                    vel: a.vel,
                    rot: None,
                    yaw8: Some(a.yaw),
                    anim: Some(anim),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DSV-578: health first, the rest as armour, both capped; nothing goes down.
    #[test]
    fn heal_in_part_fills_health_then_armour() {
        assert_eq!(
            heal_in_part(30.0, 10.0, 180.0, 100.0, 100.0),
            (100.0, 100.0)
        );
        assert_eq!(heal_in_part(30.0, 10.0, 80.0, 100.0, 100.0), (100.0, 20.0));
        assert_eq!(heal_in_part(90.0, 0.0, 120.0, 100.0, 50.0), (100.0, 50.0));
        assert_eq!(heal_in_part(120.0, 60.0, 80.0, 100.0, 50.0), (120.0, 60.0));
        assert_eq!(heal_in_part(50.0, 0.0, -5.0, 100.0, 100.0), (50.0, 0.0));
    }

    #[test]
    fn slab_test() {
        let c = [100.0, 0.0, 0.0];
        let h = [10.0, 50.0, 10.0];
        assert_eq!(ray_box([0.0; 3], [1.0, 0.0, 0.0], 1000.0, c, h), Some(90.0));
        assert_eq!(ray_box([0.0; 3], [1.0, 0.0, 0.0], 50.0, c, h), None);
        assert_eq!(ray_box([0.0; 3], [0.0, 0.0, 1.0], 1000.0, c, h), None);
        assert_eq!(ray_box([0.0; 3], [-1.0, 0.0, 0.0], 1000.0, c, h), None);
        assert_eq!(
            ray_box([100.0, 0.0, 0.0], [1.0, 0.0, 0.0], 5.0, c, h),
            Some(0.0)
        );
    }

    #[test]
    fn tuning_keys() {
        let mut t = CoopTuning::default();
        t.set("coop.lives", "3").unwrap();
        t.set("hit_chance", "0.5").unwrap();
        t.set("ray_hits", "off").unwrap();
        t.set("coop.nav", "off").unwrap();
        assert!(!t.nav);
        assert!(t.set("nav", "maybe").is_err());
        assert_eq!(t.lives, Some(3));
        assert_eq!(t.hit_chance, [0.5; 3]);
        assert!(!t.ray_hits);
        assert!(t.set("nope", "1").is_err());
        assert!(t.set("speed", "-1").is_err());
        // DSV-589: one melee hit chance for every difficulty, at most 1
        assert_eq!(t.melee_hit, 1.0);
        t.set("coop.melee_hit", "3").unwrap();
        assert_eq!(t.melee_hit, 1.0);
        t.set("melee_hit", "0.25").unwrap();
        assert_eq!(t.melee_hit, 0.25);
        // DSV-597..DSV-601: the specials switch and the dropped items' numbers
        assert!(t.specials);
        t.set("specials", "off").unwrap();
        t.set("growth_arm_ms", "500").unwrap();
        t.set("coop.growth_trigger", "64").unwrap();
        assert_eq!(
            (t.specials, t.growth_arm_ms, t.growth_trigger),
            (false, 500, 64.0)
        );
        assert!(t.set("specials", "1").is_err());
    }

    #[test]
    fn synthetic_plan_sizes() {
        let cfg = CoopTuning::default();
        let w = Coop::waves_of(&CoopPlan::default(), 0, &cfg);
        assert_eq!(w.len(), 5);
        assert_eq!(
            w.iter().map(|w| w.groups[0].count).collect::<Vec<_>>(),
            [6, 8, 10, 12, 14]
        );
        let hard = Coop::waves_of(&CoopPlan::default(), 2, &cfg);
        assert_eq!(hard[0].groups[0].count, 9);
    }
}
