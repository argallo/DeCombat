//! Quarantine (mode class 11, the infection mode): every player starts the round as a human (team 1); after the
//! outbreak countdown one random human (two above ten players) becomes infected (team 2); an infected player's
//! melee hit infects a human; an infected player who dies stays dead until the next round (DSV-213 amended); the
//! humans win the round when one of them lives at the time limit or when every infected is dead, the infected when no
//! human is left.
//!
//! The 2009 client only displays: 0xd3 sub 1 starts its outbreak countdown and round counters, 0xd3 sub 0 moves a
//! player to a team with his look (the infected look and flag for team 2), and the round-win banner of class 11 picks
//! the human or infected picture (GAME_MODES_2009.md §5.7). The countdown, the number of first infected, the infected
//! health, the melee-only rule, the infection by hit and the respawn are DeCombat rules (GMD-12, DSV-210..DSV-213).
// SPEC: GAME_MODES_2009.md §5.7 (teams, 0xd3 subs 0 / 1 / 4, round end); GAME_MESSAGES_2009_S2C.md §3.5 row 0xd3

use super::{Game, Life, RoundPhase};
use crate::gs::{self, Look, LOOK_OVERRIDE_BYTE};
use dc_net_engine::Millis;
use std::collections::{BTreeMap, BTreeSet};

/// The humans' and the infected team.
// SPEC: GAME_MODES_2009.md §5.7 (the class-11 win banner: human picture for team 1, infected for the other)
pub const HUMANS: u8 = 1;
pub const INFECTED: u8 = 2;
/// The outbreak countdown after the round start (0xd3 sub 1 `secs`).
// DC-DEFAULT: GMD-12 (proposed) 20 s (the 2009 client plays its intro sound for a countdown of just under 20 s and
// up to 20 s, cshell+0xb1190)
pub const COUNTDOWN_SECS: u32 = 20;
/// Above this many players two humans become the first infected.
// DC-DEFAULT: GMD-12 (proposed) 10
pub const TWO_HOSTS_ABOVE: usize = 10;
/// Health of an infected player whose override row names no `MaxHitPoints`.
// DC-DEFAULT: GMD-12 (proposed) 1000
pub const INFECTED_HEALTH: f32 = 1000.0;
/// The factor on the override row's `MaxHitPoints` (the infected's health; the row's value alone was too little
/// once the infected stopped coming back, user playtest 2026-11-03 "infected need more health").
// DC-DEFAULT: DSV-510 (proposed) 2.0
pub const INFECTED_HEALTH_SCALE: f32 = 2.0;
/// The look byte of the infected look: override row 0 of the character's overridable model.
// OBSERVED: capture 20261025_125432_rust6 (a look with override row 0 made the real client draw
// the infected face with its looping eye FX on the bot; ae-triage-reddot2-blast §1.3)
// DC-DEFAULT: DSV-211 (proposed) the infected look = the player's look with override row 0
pub const INFECTED_OVERRIDE: i8 = 0;
/// The look byte of the first infected (the outbreak's hosts): override row 2, the infected host.
// OBSERVED: ROM table key MODELBUTES.TXT [Overidable_Model<n>] (row 0 the infected man, row 2 the infected host man,
// each with its own face row and render styles)
// DC-DEFAULT: DSV-214 (proposed) hosts row 2, the others row 0 (the man's rows for every body)
pub const HOST_OVERRIDE: i8 = 2;
/// The claws: the host's and the infected's melee rows of the ROM weapon table (each with its own first-person hands
/// model and skin, its fire sound and the infection kill image).
// OBSERVED: ROM weapon table rows (melee order positions 11 / 12; the rows' names have these keys)
// DC-DEFAULT: DSV-215 (proposed)
pub const CLAWS_HOST: crate::chars::NameKey =
    ([0x88, 0x53, 0x5b, 0x02, 0x0a, 0xcf, 0x84, 0x44], 20);
pub const CLAWS_VIRUS: crate::chars::NameKey =
    ([0x7a, 0xfe, 0x93, 0x2c, 0xed, 0x03, 0x75, 0x26], 21);
/// The slot the claws are given in (the melee slot).
// SPEC: PROTOCOL_2009.md §3.3 (equipment slots 0..3: primary, secondary, melee, throwable)
pub const CLAWS_SLOT: u8 = 2;
/// The weapon class of the claws (the melee class of the ROM weapon order).
// OBSERVED: ROM weapon order classes (0 primary, 1 secondary, 2 melee, 3 throw)
pub const MELEE_CLASS: usize = 2;
/// 0xd3 `infector` of nobody (the outbreak).
pub const NO_INFECTOR: u32 = u32::MAX;

/// The Quarantine state of a room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InfectState {
    /// The round this state belongs to.
    pub round: u32,
    /// When the outbreak comes (`None` once it came).
    pub outbreak_at: Option<Millis>,
    pub infected: BTreeSet<u8>,
    /// Each infected player's own look, given back at the next round.
    pub own_look: BTreeMap<u8, Look>,
    /// Infections by each player in this match (its own counter on the client).
    pub infections: BTreeMap<u8, u32>,
    /// The infected of this round who came with the outbreak (the hosts).
    pub hosts: BTreeSet<u8>,
    /// Each infected player's own character look, given back at the next round (DSV-214).
    pub own_character: BTreeMap<u8, super::CharacterLook>,
    /// Each infected player's override row (DSV-320).
    pub rows: BTreeMap<u8, i8>,
}

/// `GS_INFECTION_MODE` sub 3: `[3][row:i8]`, the own player's override body row (-1: none). The 2009 client keeps it
/// as the own character's override slot, which its speed penalty, sprint factor and sprint gauge read first.
// EVIDENCE: cshell+0x2a1e0 (sub 3: an 8-bit signed value into the own player's info +0x2ac), cshell+0x5c270 /
// cshell+0x1e2bc0 (the penalty reads the override row of slot +0x2ac first)
pub fn override_message(row: i8) -> dc_wire::udp::BitWriter {
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_u8(3).write_u8(row as u8);
    gs::infection_mode(&b)
}

/// `GS_INFECTION_MODE` sub 0: `[0][team]` (`[infector]` when team 2) `[pid]` + look.
// SPEC: GAME_MODES_2009.md §5.7 (C-GM-2, DISASM cshell+0x2a26a..0x2a2a6)
pub fn team_message(team: u8, infector: u32, pid: u8, look: &Look) -> dc_wire::udp::BitWriter {
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_u8(0).write_u8(team);
    if team == INFECTED {
        b.write_u32(infector);
    }
    b.write_u32(u32::from(pid));
    gs::write_look(&mut b, look);
    gs::infection_mode(&b)
}

/// `GS_INFECTION_MODE` sub 1: `[1][a][b][flag][secs:f32]`, the countdown and the two round counters.
// SPEC: GAME_MODES_2009.md §5.7 (C-GM-3: `secs` read as a float, cshell+0xb1190; `a` / `b` to the two counters)
pub fn countdown_message(round_no: u32, round_limit: u32, secs: f32) -> dc_wire::udp::BitWriter {
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_u8(1)
        .write_u8(round_no.min(255) as u8)
        .write_u8(round_limit.min(255) as u8)
        .write_bit(false)
        .write_f32(secs);
    gs::infection_mode(&b)
}

impl Game {
    /// A Quarantine room (class 11).
    pub fn is_infect(&self) -> bool {
        self.rules.mode_class == gs::MODE_CLASS_11
    }

    pub fn infect(&self) -> &InfectState {
        &self.infect
    }

    /// The round start: the infected of the last round become humans again (their own look back, 0xd3 sub 0 team
    /// 1 to everyone, before the spawn set), the countdown starts (0xd3 sub 1 to everyone).
    /// `announce` false: the caller sends the countdown itself after its round-start messages (`infect_countdown_to`).
    // DC-DEFAULT: DSV-210 (proposed) every round starts with humans only; the countdown at the round start
    pub(super) fn infect_round_start(&mut self, now: Millis, announce: bool) {
        if !self.is_infect() {
            return;
        }
        let back: Vec<u8> = std::mem::take(&mut self.infect.infected)
            .into_iter()
            .collect();
        for pid in back {
            let look = self.infect.own_look.remove(&pid);
            let Some(p) = self.players.get_mut(&pid) else {
                continue;
            };
            p.team = HUMANS;
            if let Some(l) = look {
                p.look = l;
            }
            if let Some(c) = self.infect.own_character.remove(&pid) {
                p.character = Some(c);
                self.out.push(super::GameOut::CharacterLook { pid });
            }
            let m = team_message(HUMANS, NO_INFECTOR, pid, &p.look.clone());
            self.broadcast(&m);
            self.refresh_record(pid);
            if self.infect.rows.remove(&pid).is_some() {
                self.send(pid, override_message(-1));
            }
        }
        self.infect.rows.clear();
        self.infect.hosts.clear();
        self.infect.own_character.clear();
        self.infect.round = self.round_no.max(1);
        self.infect.outbreak_at = Some(now.wrapping_add(COUNTDOWN_SECS * 1000));
        if announce {
            for pid in self.introduced() {
                self.infect_countdown_to(pid, now);
            }
        }
        self.log(format!(
            "infect: round {}: outbreak in {COUNTDOWN_SECS} s",
            self.infect.round
        ));
    }

    /// The outbreak countdown of the running round to `pid` with the seconds left (0xd3 sub 1), while the outbreak is
    /// still to come: every player who enters the world after the round start (a slow loader of round 1, whose round
    /// the first ready player started; a late joiner) and every player after his round-start messages (the 2009 0x44
    /// resets the infection HUD).
    // SPEC: GAME_MODES_2009.md §5.7 (sub 1 `secs` read as a float), §4.1 row round start (0x44 resets the infection
    // HUD, PLAUSIBLE)
    // DC-DEFAULT: DSV-511 (proposed, playtest report "some rounds never run the initial countdown")
    pub(super) fn infect_countdown_to(&mut self, pid: u8, now: Millis) {
        if !self.is_infect() || self.infect.round != self.round_no.max(1) {
            return;
        }
        let Some(at) = self.infect.outbreak_at else {
            return;
        };
        let left_ms = at.wrapping_sub(now);
        if left_ms >= u32::MAX / 2 || left_ms == 0 {
            return;
        }
        let m = countdown_message(
            self.round_no.max(1),
            self.round_limit(),
            left_ms as f32 / 1000.0,
        );
        self.send(pid, m);
    }

    /// The player's roster record again (kind 2) with the new team and look, for clients that keep both from the
    /// roster.
    // DC-DEFAULT: DSV-211 (proposed) a 0x6b refresh follows every 0xd3 sub 0
    fn refresh_record(&mut self, pid: u8) {
        let Some(info) = self
            .players
            .get(&pid)
            .map(|p| self.info_of(p, gs::INFO_UPDATE))
        else {
            return;
        };
        self.broadcast(&gs::player_info(&info));
    }

    /// Turns `pid` infected (team 2, the infected look, the infected health) and tells everyone (0xd3 sub 0).
    // DC-DEFAULT: DSV-211 (proposed)
    fn make_infected(&mut self, pid: u8, by: Option<u8>, now: Millis) {
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.team == INFECTED && self.infect.infected.contains(&pid) {
            return;
        }
        let own = p.look.clone();
        self.infect.own_look.entry(pid).or_insert(own);
        let row = self.override_row_of(pid, by.is_none());
        let health = self.infected_health(row);
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        p.team = INFECTED;
        p.look.bytes[LOOK_OVERRIDE_BYTE] = row;
        if p.life == Life::Alive {
            p.health = health;
        }
        self.infect.rows.insert(pid, row);
        let look = p.look.clone();
        let alive = p.life == Life::Alive;
        self.infect.infected.insert(pid);
        if by.is_none() {
            self.infect.hosts.insert(pid);
        }
        self.wear_infected_look(pid, row);
        // DC-DEFAULT: DSV-216 (proposed) the infection voice at the player, for everyone near
        if self.cfg.event_sounds {
            let family = self.voice_family(pid);
            let pos = self.players.get(&pid).map_or([0.0; 3], |p| p.pos);
            if let Some(f) = self
                .tables
                .event_sounds
                .infection_voice(family, by.is_none())
                .cloned()
            {
                self.event_sound(f, pos, crate::sounds::PAIN_FORM);
            }
        }
        if let Some(b) = by {
            *self.infect.infections.entry(b).or_insert(0) += 1;
        }
        let infector = by.map_or(NO_INFECTOR, u32::from);
        self.broadcast(&team_message(INFECTED, infector, pid, &look));
        self.refresh_record(pid);
        self.send(pid, override_message(row));
        if alive {
            self.send(pid, gs::player_status(gs::STATUS_MAX_HEALTH, health));
            self.send(pid, gs::player_status(gs::STATUS_HEALTH, health));
            self.arm_claws(pid);
        }
        self.log(format!(
            "infect: client {pid} infected{}",
            by.map_or(" by the outbreak".to_string(), |b| format!(
                " by client {b}"
            ))
        ));
        let _ = now;
    }

    /// The override row of an infected player (`host`: one of the outbreak's first): the player's body row's
    /// `HostInfectedOveridable` / `InfectedOveridable` when the ROM has that override row, else the man's rows 2 / 0.
    // DC-DEFAULT: DSV-320 (proposed, playtest report "the infected are supposed to have a faster default speed and
    // more stamina") the body row's own override keys pick the row (its speed, sprint, gauge and hit points)
    fn override_row_of(&self, pid: u8, host: bool) -> i8 {
        let fallback = if host {
            HOST_OVERRIDE
        } else {
            INFECTED_OVERRIDE
        };
        let model = self
            .infect
            .own_character
            .get(&pid)
            .or_else(|| self.players.get(&pid).and_then(|p| p.character.as_ref()))
            .and_then(|c| c.assets.body.first().cloned());
        let Some(model) = model else {
            return fallback;
        };
        let chars = &self.tables.chars;
        chars
            .bodies
            .values()
            .find(|b| dc_bute::eq_fold(&b.model, &model))
            .and_then(|b| {
                if host {
                    b.host_override
                } else {
                    b.infected_override
                }
            })
            .filter(|r| chars.overrides.contains_key(r))
            .and_then(|r| i8::try_from(r).ok())
            .unwrap_or(fallback)
    }

    /// Health of an infected player with override row `row`: the row's `MaxHitPoints` x [`INFECTED_HEALTH_SCALE`],
    /// else [`INFECTED_HEALTH`].
    // DC-DEFAULT: DSV-320 (proposed); DSV-510 (proposed) the factor
    pub(super) fn infected_health(&self, row: i8) -> f32 {
        u32::try_from(row)
            .ok()
            .and_then(|r| self.tables.chars.overrides.get(&r))
            .and_then(|o| o.max_hit_points)
            .map_or(INFECTED_HEALTH, |h| h as f32 * INFECTED_HEALTH_SCALE)
    }

    /// The infected character look (DSV-214): the override row's render styles over the body and its face row in
    /// place of the face, every client told with the model block; the own look is kept for the next round. Parts
    /// whose files the user's ROM lacks stay as they were.
    // EVIDENCE: cshell+0x1e3430 (the override row's face replaces the model row's face), cshell+0x206730 (the 0x6b /
    // 0xd3 look reaches the character object)
    // DC-DEFAULT: DSV-214 (proposed) the server sends the infected model block (a client that builds it from the look
    // gets the same picture)
    fn wear_infected_look(&mut self, pid: u8, row: i8) {
        let Ok(row) = u32::try_from(row) else {
            return;
        };
        let Some(o) = self.tables.chars.overrides.get(&row).cloned() else {
            return;
        };
        let face = o
            .face
            .and_then(|f| self.tables.chars.faces.get(&f))
            .filter(|f| {
                std::iter::once(&f.model)
                    .chain(f.skins.iter().take(1))
                    .chain(f.styles.iter())
                    .all(|x| self.has_resource(x))
            })
            .map(|f| {
                (
                    f.model.clone(),
                    f.skins.iter().take(1).cloned().collect::<Vec<_>>(),
                    f.styles.clone(),
                )
            });
        let styles: Vec<String> = o
            .styles
            .iter()
            .filter(|s| self.has_resource(s))
            .cloned()
            .collect();
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        let Some(c) = p.character.as_mut() else {
            return;
        };
        self.infect
            .own_character
            .entry(pid)
            .or_insert_with(|| c.clone());
        if !styles.is_empty() {
            c.assets.body_styles = styles;
        }
        if face.is_some() {
            c.assets.face = face;
        }
        self.out.push(super::GameOut::CharacterLook { pid });
    }

    /// The claws of an infected player (`None` for a human or without the ROM row): the host's for the outbreak's
    /// infected, the infected's for the others.
    // DC-DEFAULT: DSV-215 (proposed)
    pub(super) fn claws_of(&self, pid: u8) -> Option<u16> {
        if !self.is_infect() || !self.infect.infected.contains(&pid) {
            return None;
        }
        let key = if self.infect.hosts.contains(&pid) {
            CLAWS_HOST
        } else {
            CLAWS_VIRUS
        };
        self.tables.weapons.by_name_key(key)
    }

    pub(super) fn claws_state(&self, claws: u16) -> super::WeaponState {
        super::WeaponState {
            slot: CLAWS_SLOT,
            ..self.new_weapon(claws, None)
        }
    }

    /// The living infected player's weapons become the claws: each carried weapon leaves its slot (0xb4), the claws
    /// are drawn (0xb3) and every client sees them in his hand.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 rows 0xb3 / 0xb4
    // DC-DEFAULT: DSV-215 (proposed)
    fn arm_claws(&mut self, pid: u8) {
        let Some(claws) = self.claws_of(pid) else {
            return;
        };
        let state = self.claws_state(claws);
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        let old = std::mem::replace(&mut p.weapons, vec![state]);
        p.current = Some(0);
        for w in &old {
            self.send(pid, gs::weapon_remove(w.slot, w.weapon, w.ammo));
        }
        let spec = gs::WeaponSpec {
            weapon: state.weapon,
            ammo_kind: state.ammo,
            ammo: state.told_amount(),
            clip: state.clip,
            class_pos: 0,
            slot: state.slot,
            parts: gs::NO_PARTS,
        };
        self.send(pid, gs::drawn_weapon(&spec));
        self.hand_weapon_changed(pid, claws);
    }

    /// A hit of an infected attacker: a melee hit on a human infects him (no damage); any other weapon of an
    /// infected player does nothing. Returns whether the hit was taken here.
    // DC-DEFAULT: DSV-212 (proposed) the claws infect, the infected use no other weapon
    pub(super) fn infect_on_hit(
        &mut self,
        vpid: u8,
        killer: Option<u8>,
        weapon: u16,
        now: Millis,
    ) -> bool {
        if !self.is_infect() {
            return false;
        }
        let Some(k) = killer else {
            return false;
        };
        if !self.infect.infected.contains(&k) || vpid == k {
            return false;
        }
        let melee = self.tables.weapons.class_of(weapon) == Some(MELEE_CLASS);
        let human = self
            .players
            .get(&vpid)
            .is_some_and(|v| v.team == HUMANS && v.life == Life::Alive);
        if melee && human {
            self.make_infected(vpid, Some(k), now);
            // the last human gone ends the round (GMD-22 (a): team 1 without a living player)
            self.check_elimination(now);
        }
        true
    }

    /// The Quarantine rules of one tick (class 11, round running): the round start, the outbreak.
    // DC-DEFAULT: GMD-12 (proposed); DSV-210, DSV-213 (proposed)
    pub(super) fn tick_infect(&mut self, now: Millis) {
        if !self.is_infect() || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        if self.infect.round != self.round_no.max(1) {
            self.infect_round_start(now, true);
        }
        if let Some(at) = self.infect.outbreak_at {
            if now.wrapping_sub(at) < u32::MAX / 2 {
                self.infect.outbreak_at = None;
                let mut humans: Vec<u8> = self
                    .players
                    .values()
                    .filter(|p| p.introduced && p.life == Life::Alive && p.team == HUMANS)
                    .map(|p| p.pid)
                    .collect();
                let n = if self.players.len() > TWO_HOSTS_ABOVE {
                    2
                } else {
                    1
                };
                // one human stays a human: an outbreak never ends the round at once
                let n = n.min(humans.len().saturating_sub(1));
                for _ in 0..n {
                    let i = (self.next_rand() % humans.len() as u64) as usize;
                    let pid = humans.remove(i);
                    self.make_infected(pid, None, now);
                }
                if n == 0 {
                    self.log("infect: outbreak skipped (fewer than two humans)".into());
                }
            }
        }
        // DC-DEFAULT: DSV-213 (amended 2026-11-03, proposed; user playtest "infected players respawn, they should
        // not") a dead infected player stays dead until the next round: the 2009 client asks for no respawn in class
        // 11 (GAME_MODES_2009.md §5.7) and the server no longer brings him back; the round ends when every infected is
        // dead (GMD-22 (a), round.rs `check_elimination`)
        self.infect
            .infected
            .retain(|pid| self.players.contains_key(pid));
    }
}
