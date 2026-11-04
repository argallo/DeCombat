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
/// The caption of an AI whose ROM row has no display name.
// DC-DEFAULT: FT-S3 (proposed)
pub const FALLBACK_CAPTION: &[u8] = b"AI";

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
    /// A dead AI lies still this long, then it is hidden (and its object reused, FT-R1).
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
    /// AI health without a ROM value.
    // DC-DEFAULT: FT-12 (proposed) template HitPoints, else the placed object's MaxHitPoints, else 100
    pub default_health: f32,
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
            default_health: 100.0,
            step_ms: 50,
            ray_hits: true,
            npc_rank: 0,
        }
    }
}

/// The `-coop` keys.
pub const COOP_KEYS: [&str; 12] = [
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
    "ray_hits",
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
            "ray_hits" => {
                self.ray_hits = match v {
                    "on" => true,
                    "off" => false,
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
    /// Killed at `at`; the body lies still until the corpse time is over.
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
    pub target: Option<u8>,
    next_fire: Millis,
    waypoint: Option<[f32; 3]>,
    repath_at: Millis,
    /// Shots fired and hits scored on humans (tests, logs).
    pub shots: u32,
    pub hits: u32,
}

impl AiChar {
    pub fn visible(&self) -> bool {
        matches!(self.state, AiState::Alive | AiState::Dead { .. })
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
    pub waves_cleared: u32,
    pub stage: Option<u8>,
    /// Accounts whose refused respawn request was logged since their last death.
    refusals_logged: std::collections::BTreeSet<u32>,
    started: Option<Millis>,
    last_step: Option<Millis>,
    /// Log of what the slice did (tests): spawns, kills, hides.
    pub events: Vec<String>,
}

/// The account key of a player for the per-account co-op counters (the account id, else the client id).
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
            started: None,
            last_step: None,
            events: Vec::new(),
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
        self.points.values().sum()
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
        let (plan, tables, note) = match self.vfs.as_ref() {
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
                (plan, AiTables::from_vfs(v), note)
            }
            None => (CoopPlan::default(), AiTables::default(), "no ROM".into()),
        };
        let c = Coop::new(plan, goal, tables, &self.cfg.coop);
        let line = format!(
            "co-op room: difficulty {goal} ({note}), {} wave(s){}, {} spawner(s), {} AI template(s), {} AI node(s), {} AI body row(s)",
            c.waves_total(),
            if c.plan.rounds.is_empty() { " (synthetic plan)" } else { " (spawn tables)" },
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
    /// when that row shares the player rig, else the first such row, else the default player character; its
    /// right-hand weapon, else the first default-loadout weapon; its health maximum x the difficulty's rate of the
    /// wave; the caption (boss: the GMS boss name) and the kind.
    // DC-DEFAULT: FT-S2 (proposed) body fallback; FT-S3 (proposed) caption; FT-12 (proposed) health; FT-14 / FT-K1
    // (proposed) boss kind 2
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
        if let (Some(player), Some(c)) = (player.as_ref(), self.coop.as_ref()) {
            let rows: Vec<AiBodyRow> = c.tables.bodies.values().cloned().collect();
            let wanted = t.as_ref().and_then(|t| t.body_row.clone());
            let order = rows
                .iter()
                .filter(|r| {
                    wanted
                        .as_deref()
                        .is_some_and(|w| r.name.eq_ignore_ascii_case(w))
                })
                .chain(rows.iter());
            for r in order {
                if let Some(a) = self.ai_row_look(r, player) {
                    assets = Some(a);
                    row_name = Some(r.ai_name.clone().unwrap_or_default());
                    break;
                }
            }
        }
        let weapon = t
            .as_ref()
            .and_then(|t| t.right_hand.as_deref())
            .and_then(|n| self.weapon_named(n))
            .or_else(|| self.loadout_weapons(None).first().map(|w| w.1));
        let look = match assets {
            Some(a) => CharacterLook {
                anims: AnimSet::from_rig(&a.rig),
                weapon: weapon.and_then(|w| self.weapon_look(w)),
                assets: a,
            },
            None => match self.character_for(None) {
                Ok(Some(mut c)) => {
                    c.weapon = weapon.and_then(|w| self.weapon_look(w)).or(c.weapon);
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
        let max_health = t
            .as_ref()
            .and_then(|t| t.max_health)
            .unwrap_or(self.cfg.coop.default_health);
        let spec = AiSpec {
            key: key.clone(),
            weapon: weapon.map(|w| (w, self.tables.weapons.ammo_of(w).unwrap_or(0))),
            look,
            max_health,
            caption,
            kind: if boss { NPC_KIND_BOSS } else { NPC_KIND_NORMAL },
        };
        if let Some(c) = self.coop.as_mut() {
            c.specs.insert(key, Some(spec.clone()));
        }
        Some(spec)
    }

    // ------------------------------------------------------------------------------------------- waves

    /// The round started: the first wave is due.
    pub(super) fn coop_round_started(&mut self, now: Millis) {
        let at = now.wrapping_add(self.cfg.coop.first_wave_ms);
        if let Some(c) = self.coop.as_mut() {
            if c.phase == WavePhase::Idle {
                c.phase = WavePhase::Next { at };
                c.started = Some(now);
            }
        }
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
        let total = c.waves.len();
        let goal = c.goal;
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
    fn coop_spawn_ai(&mut self, template: Option<Vec<u8>>, spot: &SpawnSpot, now: Millis) -> bool {
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
            target: None,
            next_fire: now.wrapping_add(fire_ms),
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
                    // a pool slot must be free (a corpse keeps its slot until it is hidden)
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
                    if !self.coop_spawn_ai(template, &spot, now) {
                        break;
                    }
                    if let Some(c) = self.coop.as_mut() {
                        c.queue.pop_front();
                    }
                }
                let Some(c) = self.coop.as_ref() else {
                    return;
                };
                if c.queue.is_empty() && c.alive() == 0 {
                    let last = c.wave + 1 >= c.waves.len();
                    let n = c.wave + 1;
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
                c.deaths.values().sum(),
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

    /// One AI step: corpses are hidden after the corpse time; living AIs pick the nearest living human, walk to it
    /// and shoot it.
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
        self.coop_tick_waves(now);
        let n = self.coop.as_ref().map_or(0, |c| c.ais.len());
        for slot in 0..n {
            self.ai_step(slot, dt, now);
        }
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
                if since(now, at) >= self.cfg.coop.corpse_ms {
                    if let Some(c) = self.coop.as_mut() {
                        c.ais[slot].state = AiState::Hidden;
                        c.events.push(format!("hide slot {slot}"));
                    }
                    self.out.push(GameOut::AiHide { slot });
                }
                return;
            }
            // the spawn tick: the create record goes out with the spawn position
            AiState::Alive if a.spawned_at == now => return,
            AiState::Alive => {}
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
            if let Some(d) = norm2(sub3(tpos, a.pos)) {
                new.yaw = d[0].atan2(d[2]);
            }
            if los && dist <= cfg.shot_range && !later(a.next_fire, now) {
                new.next_fire = now.wrapping_add(cfg.fire_ms);
                self.ai_fire(slot, &a, tpid, eye, teye, dist, now);
                if let Some(c) = self.coop.as_ref() {
                    new.shots = c.ais[slot].shots;
                    new.hits = c.ais[slot].hits;
                }
            }
            if !(los && dist <= cfg.hold_range) {
                // the goal point: the human in sight, else a waypoint toward it
                let goal = if los {
                    new.waypoint = None;
                    tpos
                } else {
                    if a.waypoint.is_none_or(|w| len3(sub3(w, a.pos)) < 48.0)
                        || !later(a.repath_at, now)
                    {
                        new.waypoint = self.pick_waypoint(eye, a.pos, tpos);
                        new.repath_at = now.wrapping_add(1000);
                    }
                    new.waypoint.unwrap_or(tpos)
                };
                if let Some(dir) = norm2(sub3(goal, a.pos)) {
                    let stride = cfg.speed * dt;
                    let reach = stride + dims[0].max(dims[2]) + 4.0;
                    let chest = add3(a.pos, [0.0, dims[1] * 0.3, 0.0]);
                    let knee = add3(a.pos, [0.0, -dims[1] * 0.5, 0.0]);
                    let free = |d: [f32; 3]| {
                        self.sight_clear(chest, add3(chest, scale3(d, reach)))
                            && self.sight_clear(knee, add3(knee, scale3(d, reach)))
                    };
                    let tries = [0.0f32, 0.785, -0.785, 1.571, -1.571];
                    if let Some(d) = tries.iter().map(|t| rotate_y(dir, *t)).find(|d| free(*d)) {
                        let mut p = add3(a.pos, scale3(d, stride));
                        p = self.follow_floor(p, dims[1]);
                        new.vel = scale3(d, cfg.speed);
                        new.pos = p;
                        new.yaw = d[0].atan2(d[2]);
                        moving = true;
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
        let chance = cfg.hit_chance[goal] * falloff;
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
    fn ai_on_ray(&self, origin: [f32; 3], dir: [f32; 3], limit: f32) -> Option<(usize, f32)> {
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
        if by_ray || !self.cfg.zone_factors_from_rom {
            return geometry;
        }
        let hit_obj = f.hit.and_then(|h| h.1);
        f.extra
            .filter(|x| x.obj.is_some() && x.obj == hit_obj)
            .and_then(|x| a.spec.look.assets.hit_nodes.get(x.kind as usize).copied())
            .map_or(geometry, |n| {
                (
                    n.factor,
                    if n.location == 0 {
                        geometry.1
                    } else {
                        n.location
                    },
                )
            })
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

    fn coop_kill_points(&self, boss: bool, head: bool) -> u32 {
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
        ((base + extra) * f).round().max(0.0) as u32
    }

    /// Damage to an AI: its health, the bar of every client (0xa6 sub 0x24), the shooter's points (0xd7 sub 3 for
    /// a hit, sub 1 with the record total for a kill), the kill credit; a dead AI lies still, then is hidden.
    // SPEC: FIRETEAM_2009.md §1.5 (sub 0x24), §2.2 rows 1 and 3 (points), §1.7 (no death message for an AI, no kill
    // feed in class 5)
    // DC-DEFAULT: FT-3 (proposed; per hit, no coalescing in slice 1); FT-13 (proposed); FT-R1 (proposed)
    fn ai_damage(&mut self, slot: usize, pid: u8, dmg: f32, zone: u8, now: Millis) {
        let key = self.player_key(pid);
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
        }
        if let Some(o) = obj {
            self.broadcast(&gs::npc_health_note(o, frac));
        }
        let gained = if lethal {
            self.coop_kill_points(boss, zone == gs::KILL_ZONE_HEAD)
        } else {
            self.coop_hit_points(zone)
        };
        let total = {
            let Some(c) = self.coop.as_mut() else {
                return;
            };
            let t = c.points.entry(key).or_default();
            *t = t.saturating_add(gained);
            let total = *t;
            if lethal {
                c.ai_killed += 1;
                *c.kills.entry(key).or_default() += 1;
                c.events.push(format!("kill slot {slot} by client {pid}"));
            }
            total
        };
        if lethal {
            if let Some(u) = self.players.get(&pid).and_then(|p| p.uid) {
                let s = self.stats.entry(u).or_default();
                s.kills += 1;
                if zone == gs::KILL_ZONE_HEAD {
                    s.headshots += 1;
                }
            }
            self.broadcast(&gs::coop_points(u32::from(pid), obj, gained, total));
            self.log(format!(
                "client {pid}: killed AI slot {slot} (+{gained}, {total} points)"
            ));
        } else {
            self.broadcast(&gs::coop_points_note(
                u32::from(pid),
                gained.min(255) as u8,
                total,
            ));
        }
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
                Some(StreamEntry {
                    obj: a.obj?,
                    pos: a.pos,
                    vel: a.vel,
                    rot: None,
                    yaw8: Some(a.yaw),
                    anim: Some(a.spec.look.anims.trackers(
                        a.motion,
                        since(now, a.spawned_at),
                        since(now, a.motion_since),
                    )),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(t.lives, Some(3));
        assert_eq!(t.hit_chance, [0.5; 3]);
        assert!(!t.ray_hits);
        assert!(t.set("nope", "1").is_err());
        assert!(t.set("speed", "-1").is_err());
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
