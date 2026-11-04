//! Spy Hunt (mode class 9, the room table's bounty row): intel cases lie at the mode's item spawn points; a player who
//! picks one up becomes a spy (team 2) and carries it until he dies, when his intel drops where he fell; dropped intel
//! goes back to a spawn point after a while; everybody hurts everybody, spies other spies too (DSV-450). The spy who
//! seizes the last intel becomes the agent (the Super Spy, with the agent health of DSV-451): the second stage
//! (sudden death) starts, he gets the agent weapons, the world's upload station comes, and every other player hunts
//! him. His upload at the station wins the match for him, his death wins it for the hunters at once (DSV-452); when
//! the stage time runs out stage 1 starts again and the match runs to its time limit, where the most points win
//! (DSV-392).
//!
//! The 2009 client only displays: 0xcb `GS_BOUNTY_MODE` moves a player between the hunters (team 1) and the spies
//! (team 2) with his seized-intel count, sets the three counters of the top bar (total, dropped, spies), orders the
//! "dropped" and "back at a spawn point" notices, switches the stage (sub 5, with a banner), resets the agent's
//! weapon slots (sub 2) and sets the stage clock (sub 6) (GAME_MODES_2009.md §5.6). The intel count, its places, the
//! uplink, the agent's weapons and the stage time are read from the user's ROM at run time (the mode's GMS file and
//! the world file); the touch radius, the timers' defaults, the transmission rule and the points are DeCombat rules
//! (GMD-11 as revised, DSV-230..DSV-236).
// SPEC: GAME_MODES_2009.md §5.6 (0xcb subs 0..6, phase 1 effect, kind-3 pickups); GAME_MESSAGES_2009_S2C.md §3.5 row
// 0xcb
// EVIDENCE: cshell+0x2bd80 (0xcb handler: sub 0 team 1 removes the record from the spy list and clears the own spy
// flag, any other team adds it with `value` and sets the flag; sub 1 sets the total / dropped / spy counters; sub 3 /
// sub 4 the "dropped" / "back" notices outside phase 1; sub 5 the phase and its banner; sub 6 the stage clock),
// cshell+0x70880 (the element's keys), cshell+0x71a90 (stage messages: phase 0 = stage 1, phase 1 = stage 2)

use super::{Game, GameOut, Life, RoundPhase};
use crate::gs;
use crate::world::SpyWorld;
use dc_net_engine::time::since;
use dc_net_engine::Millis;
use dc_wire::udp::BitWriter;
use std::collections::{BTreeMap, BTreeSet};

/// The hunters' and the spies' team.
// EVIDENCE: cshell+0x2bd80 (sub 0: team 1 leaves the spy list, any other team joins it with the intel count)
pub const HUNTERS: u8 = 1;
pub const SPIES: u8 = 2;
/// `mode_rules`+0xb4: stage 1 (intel hunt) and stage 2 (the agent).
// EVIDENCE: cshell+0x71a90 (0 -> the stage-1 message, 1 -> the stage-2 message)
pub const PHASE_INTEL: u8 = 0;
pub const PHASE_AGENT: u8 = 1;
/// The 0xcb sub-messages.
// SPEC: GAME_MODES_2009.md §5.6
pub const SUB_TEAM: u8 = 0;
pub const SUB_COUNTS: u8 = 1;
pub const SUB_LOADOUT: u8 = 2;
pub const SUB_DROPPED: u8 = 3;
pub const SUB_RESPAWNED: u8 = 4;
pub const SUB_PHASE: u8 = 5;
pub const SUB_TIMER: u8 = 6;
/// The banner text fields of sub 5 are read up to this size.
// SPEC: GAME_MODES_2009.md §5.6 (`zstr8(260)`)
pub const BANNER_TEXT_MAX: usize = 259;
/// `pickup_kind` of an intel case.
// SPEC: GAME_MODES_2009.md §4.4 (kind 3 `[flag:bool1]`, class 9 only)
pub const PICKUP_KIND_INTEL: u32 = 3;

/// Defaults when the ROM has no GMS row (tests, a world without the mode's file).
// DC-DEFAULT: DSV-230 (proposed) 5 intel (the value of every ROM Spy Hunt GMS), 15 s transmission (the ROM uplinks'
// time amount), 180 s stage 2 (the ROM GMS stage-2 play time)
pub const DEFAULT_TOTAL: u32 = 5;
pub const DEFAULT_TRANSMIT_SECS: f32 = 15.0;
pub const DEFAULT_STAGE2_SECS: u32 = 180;
/// Touch: a living player this close across to an intel case picks it up.
// DC-DEFAULT: DSV-231 (proposed) 56 units across
pub const TOUCH_RADIUS: f32 = 56.0;
/// Touch height: the case's point is a standing player's centre on the case's floor, so a player's centre may be at
/// most this far above it (his feet at most a case's height over the floor it lies on: walking or landing on it, not
/// standing on a ledge or jumping over it; a jump apex is about 50 units) or this far below it (the case at most at
/// his head height). The pickup is a server rule: the 2009 client sends no pickup and only shows the 0xcb result.
// DC-DEFAULT: DSV-391 (proposed, replaces DSV-231's 96 up or down; user live report "high above an intel it
// auto-picked it up") 32 units above, 53 below (a standing player's half height, the configured box)
// EVIDENCE: cshell+0x2bd80 (the client learns a seizure only from 0xcb sub 0); MOVEMENT_2009.md V-M3 (jump apex 50)
pub const TOUCH_ABOVE: f32 = 32.0;
pub const TOUCH_BELOW: f32 = 53.0;
/// Dropped intel nobody picked up goes back to a free spawn point after this time.
// DC-DEFAULT: DSV-232 (proposed) 30 s
pub const RETURN_MS: u32 = 30_000;
/// Dropped cases lie this far apart round the place of the death.
// DC-DEFAULT: DSV-232 (proposed)
pub const DROP_SPREAD: f32 = 24.0;
/// Points.
// DC-DEFAULT: DSV-235 (proposed) a kill 1, an intel case 1, the transmission 3, killing the agent 5, the agent
// surviving his stage 5
pub const POINTS_KILL: u32 = 1;
pub const POINTS_INTEL: u32 = 1;
pub const POINTS_AGENT: u32 = 3;
pub const POINTS_AGENT_KILL: u32 = 5;
pub const POINTS_AGENT_SURVIVES: u32 = 5;
/// The sub-6 clock colour (the second field: the digits' colour, cshell+0xbe0c0 `param_2`).
// EVIDENCE: cshell+0xbe0c0 (first field = the clock's seconds, second = the colour set on the digit items)
// DC-DEFAULT: DSV-234 (proposed) red for stage 2, white for the match clock again
pub const CLOCK_STAGE2: u32 = 0xffff_3030;
pub const CLOCK_PLAIN: u32 = 0xffff_ffff;
/// Round time of a Spy Hunt room whose time is 0 (DSV-393): the GMD-2 class-9 fallback, 10 minutes.
pub const FALLBACK_SECS: u32 = 600;
/// Points of the upload at the station (the mission done; DSV-323).
// DC-DEFAULT: DSV-323 (proposed) 5
pub const POINTS_UPLOAD: u32 = 5;
/// The agent's health (and its maximum) while he is the agent, the default of `spy.agent_health`. The mode's GMS file
/// has no health key; the agent body row it names carries an armour maximum five times the normal rows' (`MaxArmor`
/// 500 against 100); the user found 5 x the normal health not enough and asked for more.
// DC-DEFAULT: DSV-451 (proposed, project decision "becoming the super spy raises the health a lot", amended the
// same day "the super spy did not have enough extra hp": 500 was not enough) 1000
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] MaxArmor of the GMS `[AgentModelSetting] Model` row and of the
// plain rows; GMS keys of the Spy Hunt files (no health key)
pub const AGENT_HEALTH: f32 = 1000.0;
/// The agent's armour (and its maximum), the default of `spy.agent_armor`: the agent body row's ROM `MaxArmor` of the
/// EU-test ROM.
// DC-DEFAULT: DSV-451 (proposed, amended 2026-10-30) 500
pub const AGENT_ARMOR: f32 = 500.0;
/// Multiplier of the damage the agent's own rounds do while he is the agent (his flame's burn and his rocket's
/// blast), the default of `spy.agent_damage_scale`. The ROM rows (the bounty flame: 30 per second for 1 s within
/// 75 units; the bounty AT4: 50 within 550 units) are the hunters' values; the user found the agent's weapons too
/// weak against a lobby of hunters.
// DC-DEFAULT: DSV-456 (proposed, USER REQUEST 2026-10-31 "the super spy flame thrower and rocket launch should do more
// damage") 2.0
pub const AGENT_DAMAGE_SCALE: f32 = 2.0;

/// What a Spy Hunt room takes from its GMS file.
// OBSERVED: ROM table key GMS [IntelObject] TotalNum, FileName, Skin0, RenderStyle0; [AgentModelSetting]
// PrimaryWeapon<k>; [BNT_Stage2_Comment] Stage2PlayTime (the seven Spy Hunt GMS files; scratch run 2026-10-28)
#[derive(Debug, Clone, PartialEq)]
pub struct SpyGms {
    pub total: u32,
    pub intel_model: Option<String>,
    pub intel_skins: Vec<String>,
    pub intel_styles: Vec<String>,
    pub agent_weapons: Vec<u16>,
    pub stage2_secs: u32,
    /// The body rows of a spy and of the agent (`[SpyModelSetting]` / `[AgentModelSetting]` `Model`).
    pub spy_model: Option<u32>,
    pub agent_model: Option<u32>,
    /// The face model file the mode preloads (`[PreLoadModels]` `Model1`).
    pub face_model: Option<String>,
    /// The world trigger that brings the stage-2 station (`[AgentModelSetting] AgentTrigger`).
    pub agent_trigger: Option<String>,
    /// The stage-2 banner images (`[BNT_Stage2_Comment]`): the operation title, the hunters' and the agent's
    /// comment.
    pub stage2_title: Option<String>,
    pub hunter_comment: Option<String>,
    pub bounty_comment: Option<String>,
}

impl Default for SpyGms {
    fn default() -> Self {
        SpyGms {
            total: DEFAULT_TOTAL,
            intel_model: None,
            intel_skins: Vec::new(),
            intel_styles: Vec::new(),
            agent_weapons: Vec::new(),
            stage2_secs: DEFAULT_STAGE2_SECS,
            spy_model: None,
            agent_model: None,
            face_model: None,
            agent_trigger: None,
            stage2_title: None,
            hunter_comment: None,
            bounty_comment: None,
        }
    }
}

pub const INTEL_SECTION: &str = "IntelObject";
pub const AGENT_SECTION: &str = "AgentModelSetting";
pub const STAGE2_SECTION: &str = "BNT_Stage2_Comment";
pub const SPY_MODEL_SECTION: &str = "SpyModelSetting";
pub const PRELOAD_SECTION: &str = "PreLoadModels";

impl SpyGms {
    pub fn from_gms(doc: &dc_bute::ButeDoc) -> SpyGms {
        let text = |s: &str, k: &str| {
            doc.get_str(s, k)
                .map(|v| v.trim().trim_matches('"').trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let num = |s: &str, k: &str| {
            text(s, k).and_then(|v| v.trim_end_matches(['f', 'F']).trim().parse::<f64>().ok())
        };
        let mut g = SpyGms::default();
        if let Some(n) = num(INTEL_SECTION, "TotalNum").filter(|n| *n >= 1.0 && *n <= 64.0) {
            g.total = n as u32;
        }
        g.intel_model = text(INTEL_SECTION, "FileName");
        g.intel_skins = (0..4)
            .map_while(|i| text(INTEL_SECTION, &format!("Skin{i}")))
            .collect();
        g.intel_styles = (0..4)
            .map_while(|i| text(INTEL_SECTION, &format!("RenderStyle{i}")))
            .collect();
        g.agent_weapons = (0..8)
            .map_while(|i| num(AGENT_SECTION, &format!("PrimaryWeapon{i}")))
            .filter(|w| *w >= 0.0 && *w < 65535.0)
            .map(|w| w as u16)
            .collect();
        if let Some(s) = num(STAGE2_SECTION, "Stage2PlayTime").filter(|s| *s >= 1.0) {
            g.stage2_secs = s as u32;
        }
        let row = |s: &str| {
            num(s, "Model")
                .filter(|v| *v >= 0.0 && *v < 4096.0)
                .map(|v| v as u32)
        };
        g.spy_model = row(SPY_MODEL_SECTION);
        g.agent_model = row(AGENT_SECTION);
        g.face_model = text(PRELOAD_SECTION, "Model1");
        // OBSERVED: ROM table key GMS [AgentModelSetting] AgentTrigger; [BNT_Stage2_Comment] OperationName,
        // HunterComment, BountyComment (the seven Spy Hunt GMS files; modes-r13)
        g.agent_trigger = text(AGENT_SECTION, "AgentTrigger");
        g.stage2_title = text(STAGE2_SECTION, "OperationName");
        g.hunter_comment = text(STAGE2_SECTION, "HunterComment");
        g.bounty_comment = text(STAGE2_SECTION, "BountyComment");
        g
    }
}

/// An intel case the session creates on every client in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct IntelOut {
    /// The case key (the session maps it to an object id).
    pub key: u32,
    pub model: String,
    pub textures: Vec<String>,
    pub styles: Vec<String>,
    pub pos: [f32; 3],
}

/// One intel case on the ground.
#[derive(Debug, Clone, PartialEq)]
pub struct Intel {
    pub key: u32,
    pub pos: [f32; 3],
    /// Dropped by a dead spy at this time (`None` = at its spawn point).
    pub dropped_at: Option<Millis>,
}

/// The Spy Hunt state of a room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpyState {
    pub world: SpyWorld,
    pub gms: SpyGms,
    pub setup: bool,
    pub intel: Vec<Intel>,
    /// Intel each spy carries.
    pub carried: BTreeMap<u8, u32>,
    pub points: BTreeMap<u8, u32>,
    /// Kills of each player in this match (the board's kill column; DSV-322).
    pub kills: BTreeMap<u8, u32>,
    pub deaths: BTreeMap<u8, u32>,
    pub phase: u8,
    pub agent: Option<u8>,
    pub stage_ends: Option<Millis>,
    /// The stage-2 upload station (DSV-323).
    pub station: super::station::Station,
    /// The agent who uploaded the intel: the match's winner (DSV-323).
    pub winner: Option<u8>,
    /// The agent fell (died or left): the match ended with the hunters' result (DSV-452).
    pub hunters_won: bool,
    /// Players who got the mode's state after their introduction.
    pub told: BTreeSet<u8>,
    /// (player, case) pairs already logged as across from a case but out of its height window.
    pub near_noted: BTreeSet<(u8, u32)>,
    /// Each agent's own weapons (slot, weapon), given back at the end of the stage.
    pub own_weapons: BTreeMap<u8, Vec<(u8, u16)>>,
    /// Each spy's or agent's own character, given back when he is a hunter again (DSV-281).
    pub own_character: BTreeMap<u8, super::CharacterLook>,
    next_key: u32,
}

impl SpyState {
    pub fn from_world(world: &crate::world::WorldInfo) -> Self {
        SpyState {
            world: world.spy.clone(),
            next_key: 1,
            ..SpyState::default()
        }
    }

    pub fn points_of(&self, pid: u8) -> u32 {
        self.points.get(&pid).copied().unwrap_or(0)
    }

    pub fn spies(&self) -> u32 {
        self.carried.values().filter(|n| **n > 0).count() as u32
    }

    pub fn dropped(&self) -> u32 {
        self.intel.iter().filter(|i| i.dropped_at.is_some()).count() as u32
    }
}

/// `GS_BOUNTY_MODE` with the sub-message `body` (sub byte first).
pub fn bounty_mode(body: &BitWriter) -> BitWriter {
    gs::bounty_mode(body)
}

/// Sub 0: `[value][team][pid]` + look.
// SPEC: GAME_MODES_2009.md §5.6 (C-GM-1)
pub fn team_message(value: u32, team: u8, pid: u8, look: &gs::Look) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(SUB_TEAM)
        .write_u32(value)
        .write_u8(team)
        .write_u32(u32::from(pid));
    gs::write_look(&mut b, look);
    bounty_mode(&b)
}

/// Sub 1: `[total][dropped][spies]`.
// EVIDENCE: cshell+0x2bd80 (a -> cshell+0x71940 total, b -> cshell+0x71670 dropped, c -> cshell+0x71760 spies)
pub fn counts_message(total: u32, dropped: u32, spies: u32) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(SUB_COUNTS)
        .write_u32(total)
        .write_u32(dropped)
        .write_u32(spies);
    bounty_mode(&b)
}

/// Subs 2, 3, 4 (no fields).
pub fn bare_message(sub: u8) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(sub);
    bounty_mode(&b)
}

/// Sub 5: `[phase]`, with `[title][text]` for phase 1.
pub fn phase_message(phase: u8, title: &[u8], text: &[u8]) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(SUB_PHASE).write_u8(phase);
    if phase == PHASE_AGENT {
        b.write_zstr8(&title[..title.len().min(BANNER_TEXT_MAX)])
            .write_zstr8(&text[..text.len().min(BANNER_TEXT_MAX)]);
    }
    bounty_mode(&b)
}

/// Sub 6: `[secs][colour]`.
// EVIDENCE: cshell+0x2bd80 (sub 6 logs the second field in hex), cshell+0xbe0c0 (first = seconds, second = colour)
pub fn timer_message(secs: u32, colour: u32) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(SUB_TIMER).write_u32(secs).write_u32(colour);
    bounty_mode(&b)
}

/// Whether a player whose centre is at `p` touches the intel case at `f` (DSV-391): within the radius across and
/// inside the height window (more than [`TOUCH_ABOVE`] over the case or [`TOUCH_BELOW`] under it is out of reach).
pub fn touching(p: [f32; 3], f: [f32; 3]) -> bool {
    let dx = p[0] - f[0];
    let dz = p[2] - f[2];
    let dy = p[1] - f[1];
    (dx * dx + dz * dz).sqrt() <= TOUCH_RADIUS && dy <= TOUCH_ABOVE && dy >= -TOUCH_BELOW
}

impl Game {
    /// A Spy Hunt room (class 9).
    pub fn is_spy(&self) -> bool {
        self.rules.mode_class == gs::MODE_CLASS_9
    }

    pub fn spy(&self) -> &SpyState {
        &self.spy
    }

    /// Replaces the world's Spy Hunt places (tests).
    pub fn set_spy_world(&mut self, w: SpyWorld) {
        self.spy.world = w;
    }

    /// The GMS row of the room (once): the intel count and look, the agent's weapons, the stage time.
    fn spy_setup(&mut self) {
        self.spy.setup = true;
        let doc = self
            .vfs
            .as_ref()
            .and_then(|v| crate::coop_rom::find_gms(v, self.room.mode_key(), &self.world_announce));
        if let Some(d) = &doc {
            self.spy.gms = SpyGms::from_gms(d);
        }
        if let Some(n) = self.cfg.spy_intel.filter(|n| *n >= 1) {
            self.spy.gms.total = n.min(64);
        }
        let g = &self.spy.gms;
        let line =
            format!(
            "spy: {} ({} intel, {} intel point(s), {} uplink(s), {} agent weapon(s), stage 2 {} s)",
            if doc.is_some() { "the world's Spy Hunt GMS" } else { "no GMS: defaults" },
            g.total,
            self.spy.world.intel_points.len(),
            self.spy.world.uplinks.len(),
            g.agent_weapons.len(),
            g.stage2_secs
        );
        self.log(line);
    }

    /// A free intel place: a random spawn point no case lies at (any one when all are taken), else a player spawn;
    /// dropped to the floor.
    // DC-DEFAULT: DSV-230 (proposed) random free points; the player spawns when the world has no intel point
    fn intel_place(&mut self) -> [f32; 3] {
        let pts: Vec<[f32; 3]> = if self.spy.world.intel_points.is_empty() {
            self.spawns.iter().map(|s| s.0).collect()
        } else {
            self.spy.world.intel_points.clone()
        };
        if pts.is_empty() {
            return self.centre;
        }
        let free: Vec<[f32; 3]> = pts
            .iter()
            .copied()
            .filter(|p| {
                !self
                    .spy
                    .intel
                    .iter()
                    .any(|i| i.pos[0] == p[0] && i.pos[2] == p[2])
            })
            .collect();
        let from = if free.is_empty() { &pts } else { &free };
        let i = (self.next_rand() % from.len() as u64) as usize;
        // the case lies on the floor below the point (the points sit up to 150 units above it in the ROM worlds),
        // at a standing player's centre height, so the touch test compares like with like
        // DC-DEFAULT: DSV-230 (proposed)
        let dims = self.cfg.player_dims;
        self.floor_point(from[i], dims)
    }

    fn intel_create(&mut self, pos: [f32; 3], dropped_at: Option<Millis>) {
        let key = self.spy.next_key;
        self.spy.next_key = self.spy.next_key.wrapping_add(1).max(1);
        self.spy.intel.push(Intel {
            key,
            pos,
            dropped_at,
        });
        self.log(format!(
            "spy: intel {key} at ({:.0}, {:.0}, {:.0})",
            pos[0], pos[1], pos[2]
        ));
        if let Some(model) = self.spy.gms.intel_model.clone() {
            self.out.push(GameOut::IntelCreate(Box::new(IntelOut {
                key,
                model,
                textures: self.spy.gms.intel_skins.clone(),
                styles: self.spy.gms.intel_styles.clone(),
                pos,
            })));
        }
    }

    fn intel_remove(&mut self, i: usize) -> Intel {
        let it = self.spy.intel.remove(i);
        self.out.push(GameOut::IntelGone { key: it.key });
        it
    }

    fn spy_counts(&mut self) {
        let m = counts_message(self.spy.gms.total, self.spy.dropped(), self.spy.spies());
        self.broadcast(&m);
    }

    /// The face row of a mode body row: among the face rows naming the mode's preloaded face model, the one whose
    /// first render style is the body row's first (the spy and agent faces differ only in their styles); the
    /// player's own face when none matches.
    // DC-DEFAULT: DSV-281 (proposed)
    fn spy_face_row(&self, body_row: u32) -> Option<u32> {
        let want = self.spy.gms.face_model.as_deref()?;
        let first = self
            .tables
            .chars
            .bodies
            .get(&body_row)
            .and_then(|b| b.styles.first())?;
        self.tables
            .chars
            .faces
            .iter()
            .filter(|(_, f)| f.model.eq_ignore_ascii_case(want))
            .find(|(_, f)| {
                f.styles
                    .first()
                    .is_some_and(|s| s.eq_ignore_ascii_case(first))
            })
            .map(|(i, _)| *i)
    }

    /// The body of a spy (`row` = the GMS spy row) or the agent (the agent row) in place of the player's own; `None`
    /// gives his own character back. Every client gets the model block again; a row whose body does not load in the
    /// user's ROM leaves the character as it is.
    // OBSERVED: ROM table key GMS [SpyModelSetting] / [AgentModelSetting] Model, [PreLoadModels] Model0 / Model1 (the
    // body rows name the spy body file; the agent row has its own styles and MaxArmor; no CShell reader of these
    // sections, so the server sets the body)
    // DC-DEFAULT: DSV-281 (proposed) the spy / agent body by model block, own skin 0, face by `spy_face_row`; Goggle /
    // BackPack not applied
    fn spy_wear(&mut self, pid: u8, row: Option<u32>) {
        let Some(row) = row else {
            if let Some(c) = self.spy.own_character.remove(&pid) {
                if let Some(p) = self.players.get_mut(&pid) {
                    p.character = Some(c);
                    self.out.push(GameOut::CharacterLook { pid });
                }
            }
            return;
        };
        let Some(cur) = self.players.get(&pid).and_then(|p| p.character.clone()) else {
            return;
        };
        let own = self.spy.own_character.get(&pid).unwrap_or(&cur).clone();
        let face_row = self
            .spy_face_row(row)
            .unwrap_or(self.cfg.character.face_row);
        let choice = crate::chars::CharacterChoice {
            model_row: row,
            skin: 0,
            face_row,
        };
        let Ok(assets) = self.assets_for(choice, None) else {
            return;
        };
        if cur.assets.body == assets.body
            && cur.assets.body_styles == assets.body_styles
            && cur.assets.face == assets.face
        {
            return;
        }
        let anims = super::AnimSet::from_rig(&assets.rig);
        self.spy.own_character.entry(pid).or_insert(own);
        if let Some(p) = self.players.get_mut(&pid) {
            p.character = Some(super::CharacterLook {
                assets,
                weapon: cur.weapon,
                anims,
            });
        }
        self.out.push(GameOut::CharacterLook { pid });
    }

    /// The player's roster record again (kind 2): his team, look and points.
    fn spy_refresh(&mut self, pid: u8) {
        let Some(info) = self
            .players
            .get(&pid)
            .map(|p| self.info_of(p, gs::INFO_UPDATE))
        else {
            return;
        };
        self.broadcast(&gs::player_info(&info));
    }

    pub(super) fn spy_team(&mut self, pid: u8, team: u8, value: u32) {
        let row = match team {
            SPIES if self.spy.agent == Some(pid) => self.spy.gms.agent_model,
            SPIES => self.spy.gms.spy_model,
            _ => None,
        };
        self.spy_wear(pid, row);
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        p.team = team;
        let look = p.look.clone();
        self.broadcast(&team_message(value, team, pid, &look));
        self.spy_refresh(pid);
    }

    /// The board values of a player: (kills, deaths, the mission column's `v64`).
    // DC-DEFAULT: DSV-322 (proposed, amends DSV-235; playtest report "in spy hunt capture intel is counting as kills")
    // the kill column counts kills only; the points (intel, transmission, the agent, kills: the goal's count) go to
    // the mission column, which the 2009 board shows as `v64` outside the flag classes
    // EVIDENCE: cshell+0xc0ac0 / cshell+0xbe7c0 (the mission text `%d` of record +0x64 shown in every class but 7 / 8)
    pub(super) fn spy_board(&self, pid: u8) -> (u32, u32, [u8; 3]) {
        (
            self.spy.kills.get(&pid).copied().unwrap_or(0),
            self.spy.deaths.get(&pid).copied().unwrap_or(0),
            [self.spy.points_of(pid).min(255) as u8, 0, 0],
        )
    }

    fn add_points(&mut self, pid: u8, n: u32) {
        *self.spy.points.entry(pid).or_insert(0) += n;
    }

    /// Places every intel case at a spawn point (the match start and the end of stage 2).
    fn spy_place_all(&mut self) {
        for i in (0..self.spy.intel.len()).rev() {
            self.intel_remove(i);
        }
        for _ in 0..self.spy.gms.total {
            let p = self.intel_place();
            self.intel_create(p, None);
        }
    }

    /// A kill in class 9: points to the killer (more for the agent), the victim's death, the end of stage 2 when the
    /// agent fell, both records refreshed (no points goal: DSV-392).
    // DC-DEFAULT: DSV-235 (proposed)
    pub(super) fn spy_on_kill(&mut self, victim: u8, killer: Option<u8>, now: Millis) {
        if !self.is_spy() {
            return;
        }
        *self.spy.deaths.entry(victim).or_insert(0) += 1;
        let killer = killer.filter(|k| *k != victim);
        if let Some(k) = killer {
            *self.spy.kills.entry(k).or_insert(0) += 1;
            let n = if self.spy.agent == Some(victim) {
                POINTS_AGENT_KILL
            } else {
                POINTS_KILL
            };
            self.add_points(k, n);
            self.spy_refresh(k);
        }
        self.spy_refresh(victim);
        if self.spy.agent == Some(victim) {
            self.log(format!(
                "spy: the agent (client {victim}) killed by {killer:?}"
            ));
            self.spy_agent_fell(now);
        } else {
            self.spy_drop(victim, now);
        }
    }

    /// A spy who died or left: his intel lies where he was, he is a hunter again.
    // DC-DEFAULT: DSV-232 (proposed)
    fn spy_drop(&mut self, pid: u8, now: Millis) {
        let Some(n) = self.spy.carried.remove(&pid).filter(|n| *n > 0) else {
            return;
        };
        let at = self.players.get(&pid).map_or(self.centre, |p| p.pos);
        // the cases lie on the floor below the body (a spy who fell in the air drops them to the ground), at a
        // standing player's centre height like the placed ones, so the touch window of DSV-391 holds for them too
        // DC-DEFAULT: DSV-391 (proposed)
        let dims = self.cfg.player_dims;
        let at = self.floor_point(at, dims);
        for i in 0..n {
            let a = i as f32 * std::f32::consts::TAU / n as f32;
            let off = if n > 1 { DROP_SPREAD } else { 0.0 };
            self.intel_create(
                [at[0] + off * a.cos(), at[1], at[2] + off * a.sin()],
                Some(now),
            );
        }
        if self.players.contains_key(&pid) {
            self.spy_team(pid, HUNTERS, 0);
        }
        self.broadcast(&bare_message(SUB_DROPPED));
        self.spy_counts();
        self.log(format!("spy: client {pid} dropped {n} intel"));
    }

    /// Weapons of `pid` become `weapons` (slot, weapon): each held one leaves its slot (0xb4), each new one is given
    /// (0xb2) and the first is drawn (0x7f).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 rows 0xb2 / 0xb4 / 0x7f
    // DC-DEFAULT: DSV-233 (proposed)
    fn spy_rearm(&mut self, pid: u8, weapons: &[(u8, u16)]) {
        let states: Vec<super::WeaponState> = weapons
            .iter()
            .map(|&(slot, w)| super::WeaponState {
                slot,
                ..self.new_weapon(w, None)
            })
            .collect();
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        let old = std::mem::replace(&mut p.weapons, states.clone());
        p.current = (!states.is_empty()).then_some(0);
        for w in &old {
            self.send(pid, gs::weapon_remove(w.slot, w.weapon, w.ammo));
        }
        for w in &states {
            let class = self.tables.weapons.class_of(w.weapon);
            let class_pos = states
                .iter()
                .take_while(|x| x.weapon != w.weapon)
                .filter(|x| self.tables.weapons.class_of(x.weapon) == class)
                .count() as u8;
            let spec = gs::WeaponSpec {
                weapon: w.weapon,
                ammo_kind: w.ammo,
                ammo: w.told_amount(),
                clip: w.clip,
                class_pos,
                slot: w.slot,
                parts: gs::NO_PARTS,
            };
            self.send(pid, gs::weapon_give(&spec));
        }
        if let Some(w) = states.first() {
            match self.tables.weapons.command_of(w.weapon) {
                Some(c) => self.send(pid, gs::weapon_switch(c, true, -1.0, 0)),
                None => {
                    let spec = gs::WeaponSpec {
                        weapon: w.weapon,
                        ammo_kind: w.ammo,
                        ammo: w.told_amount(),
                        clip: w.clip,
                        class_pos: 0,
                        slot: w.slot,
                        parts: gs::NO_PARTS,
                    };
                    self.send(pid, gs::drawn_weapon(&spec));
                }
            }
            self.hand_weapon_changed(pid, w.weapon);
        }
    }

    /// The agent's weapons: the GMS rows, each in the slot of its weapon class (primary 0, secondary 1, melee 2,
    /// throw 3).
    // OBSERVED: ROM table key WEAPONS.TXT [WeaponOrder] (the three agent weapons sit in the primary, secondary and melee
    // lists)
    fn agent_weapons(&self) -> Vec<(u8, u16)> {
        let mut out: Vec<(u8, u16)> = Vec::new();
        for &w in &self.spy.gms.agent_weapons {
            if !self.tables.weapons.weapons.contains_key(&w) {
                continue;
            }
            let slot = self.tables.weapons.class_of(w).unwrap_or(0).min(3) as u8;
            if out.iter().any(|(s, _)| *s == slot) {
                continue;
            }
            out.push((slot, w));
        }
        out
    }

    /// The agent's weapons while he is the agent (his spawns included).
    pub(super) fn agent_weapons_of(&self, pid: u8) -> Option<Vec<(u8, u16)>> {
        if !self.is_spy() || self.spy.agent != Some(pid) {
            return None;
        }
        Some(self.agent_weapons()).filter(|w| !w.is_empty())
    }

    /// The stage-2 banner of one receiver (sub 5 phase 1): the operation title and the agent's or the hunters'
    /// comment. The 2009 client loads both strings as images (the 0x45 intro banner's code).
    // EVIDENCE: cshell+0x2bd80 (sub 5 phase 1: cshell+0xbbfc0(first, second, 4000, 1.0)), cshell+0xbbfc0 (each string
    // a texture path, cshell+0x2c1570)
    // DC-DEFAULT: DSV-323 (proposed, amends DSV-234) the GMS `[BNT_Stage2_Comment]` images: the agent gets
    // `BountyComment`, everyone else `HunterComment`
    fn stage2_banner(&self, pid: u8) -> BitWriter {
        let g = &self.spy.gms;
        let title = g.stage2_title.clone().unwrap_or_default();
        let text = if self.spy.agent == Some(pid) {
            g.bounty_comment.clone()
        } else {
            g.hunter_comment.clone()
        }
        .unwrap_or_default();
        phase_message(PHASE_AGENT, title.as_bytes(), text.as_bytes())
    }

    /// The spy who holds every intel becomes the agent: stage 2 (his body and weapons, the banner, the stage clock)
    /// and the world's station comes.
    // DC-DEFAULT: DSV-323 (proposed, amends DSV-233, playtest report "captured all 5 intel but did not become the super
    // spy or trigger the upload station") the agent at the last seized case; the upload is stage 2's goal
    fn spy_become_agent(&mut self, pid: u8, now: Millis) {
        self.spy.phase = PHASE_AGENT;
        self.spy.agent = Some(pid);
        let secs = self.spy.gms.stage2_secs;
        self.spy.stage_ends = Some(now.wrapping_add(secs.saturating_mul(1000)));
        self.add_points(pid, POINTS_AGENT);
        for to in self.introduced() {
            let m = self.stage2_banner(to);
            self.send(to, m);
        }
        let total = self.spy.gms.total;
        self.spy_team(pid, SPIES, total);
        self.broadcast(&timer_message(secs, CLOCK_STAGE2));
        let weapons = self.agent_weapons();
        if !weapons.is_empty() {
            let own: Vec<(u8, u16)> = self
                .players
                .get(&pid)
                .map(|p| p.weapons.iter().map(|w| (w.slot, w.weapon)).collect())
                .unwrap_or_default();
            self.spy.own_weapons.insert(pid, own);
            self.send(pid, bare_message(SUB_LOADOUT));
            self.spy_rearm(pid, &weapons);
        }
        // DC-DEFAULT: DSV-451 (proposed, amended 2026-10-30) the agent's health and armour (`spy.agent_*`), both full
        let (health, armor) = (self.cfg.agent_health, self.cfg.agent_armor);
        self.spy_vitals(pid, health, Some(armor), true);
        self.spy_counts();
        self.log(format!(
            "spy: client {pid} holds every intel: the agent, stage 2 for {secs} s"
        ));
        self.station_start(now);
    }

    /// The agent uploaded the intel at the station: his points, and the match ends with him the winner (the world's
    /// activate command tells the mode controller that the defence won).
    // DC-DEFAULT: DSV-323 (proposed)
    pub(super) fn spy_upload_done(&mut self, now: Millis) {
        if self.spy.station.done || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        self.spy.station.done = true;
        let Some(a) = self.spy.agent else {
            return;
        };
        self.add_points(a, POINTS_UPLOAD);
        self.spy_refresh(a);
        self.spy.winner = Some(a);
        let team = self.players.get(&a).map_or(SPIES, |p| p.team);
        self.log(format!("spy: the agent (client {a}) completed the mission"));
        self.end_round(now, team);
    }

    /// The health and armour of a living player who becomes the agent or a hunter again: health `max` (`fill`: set
    /// to it, else kept up to it), armour up to `armor_max` (`None`: the maximum of the body he now wears; `fill`:
    /// full); the client gets both maxima and values.
    // DC-DEFAULT: DSV-451 (proposed) the player status forms the spawn sends (health, armour and their maxima)
    fn spy_vitals(&mut self, pid: u8, max: f32, armor_max: Option<f32>, fill: bool) {
        let armour_max = armor_max.unwrap_or_else(|| self.armour_of(pid).max);
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.life == Life::Alive) else {
            return;
        };
        p.health = if fill { max } else { p.health.min(max) };
        p.armor = if fill {
            armour_max
        } else {
            p.armor.min(armour_max)
        };
        let (health, armor) = (p.health, p.armor);
        self.send(pid, gs::player_status(gs::STATUS_MAX_HEALTH, max));
        self.send(pid, gs::player_status(gs::STATUS_MAX_ARMOR, armour_max));
        self.send(pid, gs::player_status(gs::STATUS_HEALTH, health));
        self.send(pid, gs::player_status(gs::STATUS_ARMOR, armor));
    }

    /// The agent died (killed, fell, by his own hand) or left: the match ends at once with the hunters' result. His
    /// intel stays with him (no drop, nobody seizes it), and the round end keeps his weapon from dropping.
    // DC-DEFAULT: DSV-452 (proposed, project decision "the agent dying, dropping the intel and the round going
    // on is a glitch"; replaces DSV-234's agent death clause) the hunters win: the 2009 client shows the win picture
    // to a receiver whose team equals the win value (cshell+0xc1a50), so the value 1 is a win for every hunter and a
    // loss for the agent (team 2)
    fn spy_agent_fell(&mut self, now: Millis) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        self.spy.hunters_won = true;
        self.log("spy: the agent fell, the hunters win".into());
        self.end_round(now, HUNTERS);
    }

    /// Stage 2 ends (the agent survived its time): the agent is a hunter again with his own weapons, every intel case
    /// goes back to a spawn point, stage 1 again, the match clock again. (His death or leaving ends the match:
    /// DSV-452.)
    // DC-DEFAULT: DSV-234 (proposed; amended by DSV-452)
    fn spy_end_stage(&mut self, now: Millis) {
        self.station_reset();
        let agent = self.spy.agent.take();
        self.spy.phase = PHASE_INTEL;
        self.spy.stage_ends = None;
        self.spy.carried.clear();
        self.broadcast(&phase_message(PHASE_INTEL, b"", b""));
        if let Some(a) = agent.filter(|a| self.players.contains_key(a)) {
            self.spy_team(a, HUNTERS, 0);
            // DC-DEFAULT: DSV-451 (proposed) a hunter again: the normal health maximum, his own armour maximum
            let max = self.cfg.max_health;
            self.spy_vitals(a, max, None, false);
            if let Some(own) = self.spy.own_weapons.remove(&a) {
                let alive = self.players.get(&a).is_some_and(|p| p.life == Life::Alive);
                if alive && !own.is_empty() {
                    self.send(a, bare_message(SUB_LOADOUT));
                    self.spy_rearm(a, &own);
                }
            }
        }
        self.spy.own_weapons.clear();
        self.spy_place_all();
        self.broadcast(&bare_message(SUB_RESPAWNED));
        self.spy_counts();
        if let RoundPhase::Running { start } = self.phase {
            let left = self
                .rules
                .round_secs
                .saturating_sub(since(now, start) / 1000);
            if self.rules.round_secs > 0 {
                self.broadcast(&timer_message(left, CLOCK_PLAIN));
            }
        }
        self.log("spy: stage 1 again, the intel back at the spawn points".into());
    }

    // DC-DEFAULT: DSV-392 (proposed, replaces DSV-235's points goal; user live report "when I got all 5 the game
    // ended rather than making me a super spy": the live room's goal 8 was reached at a seizure, log
    // `spy: client 0 reached the goal of 8 points`) a Spy Hunt match ends only at the agent's upload
    // (`spy_upload_done`) or at its time limit (`spy_leader`); the room goal (ROM mode row 5 GoalMin 8 .. GoalMax 16)
    // ends nothing. The 2009 client's own counters skip kills in stage 2 (cshell+0x2aad0 / 0x2ab70) and its 0xcb
    // handler has no end message (cshell+0x2bd80): the end is the server's.

    /// The time-out winner: the team value of the one player with the most points, 0 for a shared lead.
    // DC-DEFAULT: DSV-235 (proposed)
    pub(super) fn spy_leader(&self) -> u8 {
        self.spy_leader_pid()
            .and_then(|pid| self.players.get(&pid))
            .map_or(0, |p| p.team)
    }

    /// The accounts the match's result names as winners, and the agent's account when his upload won (DSV-472):
    /// the agent alone after his upload; everyone but the agent after his fall (DSV-452); the single points leader
    /// at the time limit (DSV-235; a shared lead names nobody).
    // DC-DEFAULT: DSV-472 (proposed, USER REQUEST 2026-10-31 "the super spy winning should pay out more xp than the
    // non-winners")
    pub(super) fn spy_winner_uids(&self) -> (std::collections::BTreeSet<u32>, Option<u32>) {
        let uid = |pid: u8| self.players.get(&pid).and_then(|p| p.uid);
        if let Some(w) = self.spy.winner {
            let u = uid(w);
            return (u.into_iter().collect(), u);
        }
        if self.spy.hunters_won {
            let agent = self.spy.agent.and_then(uid);
            let wins = self
                .players
                .values()
                .filter(|p| p.introduced)
                .filter_map(|p| p.uid)
                .filter(|u| Some(*u) != agent)
                .collect();
            return (wins, None);
        }
        (self.spy_leader_pid().and_then(uid).into_iter().collect(), None)
    }

    /// The one introduced player with the most points (`None` for a shared lead or nobody).
    pub(super) fn spy_leader_pid(&self) -> Option<u8> {
        if let Some(w) = self.spy.winner {
            return Some(w);
        }
        let mut best: Option<(u32, u8)> = None;
        let mut shared = false;
        for p in self.players.values().filter(|p| p.introduced) {
            let k = self.spy.points_of(p.pid);
            match best {
                Some((b, _)) if k < b => {}
                Some((b, _)) if k == b => shared = true,
                _ => {
                    best = Some((k, p.pid));
                    shared = false;
                }
            }
        }
        best.filter(|_| !shared).map(|(_, pid)| pid)
    }

    /// The `GS_ROUND_RESULT` win value for one receiver in class 9: his own team value when he leads on points, a
    /// value other than his team when another player leads, 0 for a shared lead. The 2009 client picks the result
    /// banner by comparing the win value with its own team value (equal: the win picture, other non-zero: the loss
    /// picture, 0: the draw / score picture) and has no class-9 branch for the picture (only the announcer line is
    /// skipped in class 9). Every hunter shares team 1, so one broadcast value showed every hunter the win picture
    /// whenever a hunter led.
    // EVIDENCE: cshell+0xc1a50 (param 2 = 0: draw / score picture; else own team +0xf8 == param 2: win picture, other:
    // loss picture; classes 5 / 6 / 9 skip only the round-win sound)
    // DC-DEFAULT: DSV-280 (proposed) the class-9 result value per receiver: the leader's own team for the leader, the
    // other team value for everybody else, 0 for a shared lead
    pub(super) fn spy_result_for(&self, pid: u8, leader: Option<u8>) -> i8 {
        // DC-DEFAULT: DSV-452 (proposed) the agent fell: the hunters' value to everyone (a win for each hunter, a
        // loss for the agent)
        if self.spy.hunters_won {
            return HUNTERS as i8;
        }
        let Some(leader) = leader else {
            return 0;
        };
        let own = self.players.get(&pid).map_or(0, |p| p.team);
        if pid == leader {
            own as i8
        } else if own == 1 {
            2
        } else {
            1
        }
    }

    /// The mode state to a player introduced after the match began: the counters, every spy, the stage.
    fn spy_tell(&mut self, pid: u8) {
        let m = counts_message(self.spy.gms.total, self.spy.dropped(), self.spy.spies());
        self.send(pid, m);
        if self.spy.phase == PHASE_AGENT {
            let m = self.stage2_banner(pid);
            self.send(pid, m);
        }
        let spies: Vec<(u8, u32, gs::Look)> = self
            .spy
            .carried
            .iter()
            .filter(|(_, n)| **n > 0)
            .filter_map(|(s, n)| self.players.get(s).map(|p| (*s, *n, p.look.clone())))
            .collect();
        for (s, n, look) in spies {
            self.send(pid, team_message(n, SPIES, s, &look));
        }
    }

    /// The Spy Hunt rules of one tick (class 9, round running): setup, late joiners, lost spies, dropped intel going
    /// back, pickups, the transmission, the end of stage 2.
    // DC-DEFAULT: DSV-230..DSV-235 (proposed)
    pub(super) fn tick_spy(&mut self, now: Millis) {
        if !self.is_spy() || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        if !self.spy.setup {
            self.spy_setup();
            self.spy_place_all();
            self.spy_counts();
            self.spy.told = self.introduced().into_iter().collect();
            self.log(format!(
                "spy: {} intel placed, stage 1",
                self.spy.intel.len()
            ));
        }
        for pid in self.introduced() {
            if self.spy.told.insert(pid) {
                self.spy_tell(pid);
            }
        }
        self.spy.told.retain(|p| self.players.contains_key(p));
        // a spy who died outside a kill or left
        let lost: Vec<u8> = self
            .spy
            .carried
            .keys()
            .copied()
            .filter(|p| self.players.get(p).is_none_or(|x| x.life != Life::Alive))
            .filter(|p| self.spy.agent != Some(*p))
            .collect();
        for pid in lost {
            self.spy_drop(pid, now);
        }
        if let Some(a) = self.spy.agent {
            if self.players.get(&a).is_none_or(|p| p.life != Life::Alive) {
                self.log(format!("spy: the agent (client {a}) is gone"));
                self.spy_agent_fell(now);
                return;
            }
        }
        // dropped intel nobody took goes back to a spawn point
        let back: Vec<usize> = self
            .spy
            .intel
            .iter()
            .enumerate()
            .filter(|(_, i)| i.dropped_at.is_some_and(|t| since(now, t) >= RETURN_MS))
            .map(|(n, _)| n)
            .collect();
        if !back.is_empty() {
            for &n in back.iter().rev() {
                self.intel_remove(n);
            }
            for _ in 0..back.len() {
                let p = self.intel_place();
                self.intel_create(p, None);
            }
            self.broadcast(&bare_message(SUB_RESPAWNED));
            self.spy_counts();
            self.log(format!(
                "spy: {} dropped intel back at the spawn points",
                back.len()
            ));
        }
        // pickups (stage 1 only: in stage 2 the agent has every case)
        if self.spy.phase == PHASE_INTEL {
            let alive: Vec<(u8, [f32; 3])> = self
                .players
                .values()
                .filter(|p| p.introduced && p.life == Life::Alive)
                .map(|p| (p.pid, p.pos))
                .collect();
            for (pid, pos) in alive {
                let Some(i) = self.spy.intel.iter().position(|it| touching(pos, it.pos)) else {
                    // a player across from a case but out of its height window: noted once per player and case
                    let near: Vec<(u32, f32)> = self
                        .spy
                        .intel
                        .iter()
                        .filter(|it| {
                            let (dx, dz) = (pos[0] - it.pos[0], pos[2] - it.pos[2]);
                            (dx * dx + dz * dz).sqrt() <= TOUCH_RADIUS
                        })
                        .map(|it| (it.key, pos[1] - it.pos[1]))
                        .collect();
                    for (key, dy) in near {
                        if self.spy.near_noted.insert((pid, key)) {
                            self.log(format!(
                                "spy: client {pid} across from intel {key} but {dy:.0} units off in height"
                            ));
                        }
                    }
                    continue;
                };
                self.intel_remove(i);
                let n = {
                    let c = self.spy.carried.entry(pid).or_insert(0);
                    *c += 1;
                    *c
                };
                self.add_points(pid, POINTS_INTEL);
                self.spy_team(pid, SPIES, n);
                self.spy_counts();
                self.log(format!(
                    "spy: client {pid} seized intel ({n} of {})",
                    self.spy.gms.total
                ));
                // DSV-392: no points goal here; the spy with every case is the agent (the Super Spy)
                if n >= self.spy.gms.total {
                    self.spy_become_agent(pid, now);
                    break;
                }
            }
        }
        if self.spy.phase == PHASE_AGENT {
            self.station_tick(now);
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                return;
            }
        }
        if self.spy.phase == PHASE_AGENT
            && self
                .spy
                .stage_ends
                .is_some_and(|t| now.wrapping_sub(t) < u32::MAX / 2)
        {
            if let Some(a) = self.spy.agent {
                self.add_points(a, POINTS_AGENT_SURVIVES);
                self.spy_refresh(a);
                self.log(format!("spy: the agent (client {a}) survived stage 2"));
            }
            self.spy_end_stage(now);
        }
    }
}
