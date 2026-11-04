//! Combat: the weapon mirror (clip and stock), client-reported hits (`UP_FIRE`), damage, kills, the death
//! timeline and respawn requests.
// SPEC: GAME_MESSAGES_2009_C2S.md §3.3, §3.4, §4.1, §4.2, Appendix A steps A8-A11; GAME_MESSAGES_2009_S2C.md §3.3,
// §4.3, §10 steps 14-18

use super::hits::{self, HitVerdict};
use super::{
    finite3, len3, sub3, Absorb, DeathStates, Game, HitInfo, Life, RoundPhase, TODO_DAMAGE_KIND,
    TODO_DAMAGE_MODE, TODO_DAMAGE_PART, TODO_KILL_KIND, TODO_KILL_RELATION,
};
use crate::gs;
use crate::up::UpFire;
use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// `kill_kind` of a team kill: the level the 2009 client announces nowhere (no banner, no voice line, no note).
// EVIDENCE: cshell+0xa628b / cshell+0xa676b (level 0 falls through the level switch to the end without a form)
// DC-DEFAULT: DSV-106 (staged)
pub const KILL_KIND_NO_ANNOUNCE: u8 = 0;
/// The highest multi-kill level sent: the client shows one last form for 7 and more.
// DISASM: cshell+0xa676b..0xa6794 (levels above 6 share the last form)
// DC-DEFAULT: DSV-105 (staged) cap 7
pub const KILL_LEVEL_MAX: u8 = 7;

/// `cone_off` lines logged per link before the rest are only counted.
// DC-DEFAULT: DSV-102 (proposed) 32
pub const CONE_LOG_PER_LINK: u32 = 32;

/// How far past the weapon row's `Range` a melee claim's hit distance may lie (float and position rounding of the
/// client; the row range is measured from the same eye point the claim's `origin` carries).
// DC-DEFAULT: DSV-161 (proposed) 8 units
pub const MELEE_RANGE_SLACK: f32 = 8.0;

/// What caused a damage: the weapon (kill feed) and the damage type (`zone` of `GS_DAMAGE_TAKEN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DamageSource {
    pub weapon: u16,
    pub dtype: u8,
}

/// One hit split between the armour and the health: `(health damage, armour taken)`. `dmg` is the hit's damage
/// after the zone multiplier, `base` the ammunition's damage before it, `share` the armour's share of this hit
/// (damage type and location already applied) and `armour` the points left. While the armour lasts it takes `share`
/// of the cost (`share x base`, or `share x dmg` with `cost_from_base` false) and the health takes `dmg x (1 -
/// share)`; armour that cannot pay the whole cost protects in proportion to what it can pay.
// project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1 / §3 T1, T2; tdm4
// §3.1): the 2009 server's rifle hits took health = InstDamage x node factor x (1 - share) and armour = InstDamage x
// share (the same armour cost on the arm and on the torso), nothing from the armour on a head shot (share 0) or a
// blast (an unprotected damage type); residual 0 on all 30 captured weapon hits
// DC-DEFAULT: DSV-30 (calibration staged) the proportional rule when the armour cannot pay the whole cost (never
// observed: the captured armour never fell below 79)
pub(crate) fn armour_split(
    dmg: f32,
    base: f32,
    share: f32,
    armour: f32,
    cost_from_base: bool,
) -> (f32, f32) {
    let dmg = dmg.max(0.0);
    let share = share.clamp(0.0, 1.0);
    if share <= 0.0 || armour <= 0.0 || dmg <= 0.0 {
        return (dmg, 0.0);
    }
    let cost = share * if cost_from_base { base.max(0.0) } else { dmg };
    let cover = if cost > armour { armour / cost } else { 1.0 };
    (dmg * (1.0 - share * cover), (cost * cover).min(armour))
}

impl Game {
    /// Index of the player's weapon `weapon_id`, adopted from the table when the server did not give it.
    // DC-DEFAULT: DC-C2S-4 (proposed); adoption of unknown weapons DSV-15 (staged proposal)
    pub(super) fn carried_slot(&mut self, pid: u8, weapon_id: u16, ammo: Option<u16>) -> usize {
        let new = self.new_weapon(weapon_id, ammo);
        let p = self.players.get_mut(&pid).expect("player");
        if let Some(i) = p.weapons.iter().position(|w| w.weapon == weapon_id) {
            return i;
        }
        p.weapons.push(new);
        p.weapons.len() - 1
    }

    pub(super) fn on_reload(&mut self, pid: u8, ammo_id: u16) {
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        let cur = p
            .current
            .filter(|&i| p.weapons.get(i).is_some_and(|w| w.ammo == ammo_id));
        let Some(i) = cur.or_else(|| p.weapons.iter().position(|w| w.ammo == ammo_id)) else {
            return;
        };
        // SPEC: GAME_MESSAGES_2009_C2S.md §4.2 (the client books the transfer from its stock into the clip)
        let w = &mut p.weapons[i];
        let take = w.clip_size.saturating_sub(w.clip).min(w.stock);
        w.clip += take;
        w.stock -= take;
    }

    /// The client's respawn request (0x89, 0xc5 or 0xd2 by mode class): a dead player whose death is at least the
    /// class wait minus the guard old is respawned at once with the full spawn set. Ignored (and logged): a request
    /// from a living player, before the guard, after the round end, or in a class without respawn inside a round.
    // SPEC: GAME_MESSAGES_2009_C2S.md §3.4; CAPTURE_tdm4_KILL_VALIDATION.md §4, §8.1 S1; REFERENCE_FINDINGS_botduel.md F2
    // (the reference stack answers the request 30-80 ms later), F4 (requests after the round end are not answered)
    // DC-DEFAULT: DSV-32 (proposed, refines DC-KILL-1 / DC-C2S-3)
    pub(super) fn on_respawn_request(&mut self, pid: u8, id: u8, now: Millis) {
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        let died = match p.life {
            Life::Dying { died } | Life::Waiting { died } => died,
            _ => {
                self.log(format!(
                    "client {pid}: respawn request {id:#04x} ignored (not dead)"
                ));
                return;
            }
        };
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            self.log(format!(
                "client {pid}: respawn request {id:#04x} ignored (the round is over)"
            ));
            return;
        }
        if let Some(want) = super::respawn_request_id(self.rules.mode_class).filter(|w| *w != id) {
            self.log(format!(
                "client {pid}: respawn request {id:#04x} where the mode class asks with {want:#04x} (accepted)"
            ));
        }
        let wait = match self.respawn_rule() {
            super::RespawnRule::ClientRequest { wait_ms, .. } => wait_ms,
            super::RespawnRule::ServerTimer { after_ms } => after_ms,
            super::RespawnRule::NextRound => {
                self.log(format!(
                    "client {pid}: respawn request {id:#04x} ignored (no respawn inside a round of mode class {})",
                    self.rules.mode_class
                ));
                return;
            }
        };
        // SPEC: FIRETEAM_2009.md §2.3 (the class-5 client asks only while its record +0x1f8 > 0)
        // DC-DEFAULT: FT-5 (proposed) no respawn without a life left
        if !self.coop_may_respawn(pid) {
            if self.coop_log_refusal(pid) {
                self.log(format!(
                    "client {pid}: respawn request {id:#04x} ignored (no co-op life left; repeats not logged)"
                ));
            }
            return;
        }
        let age = since(now, died);
        if age.saturating_add(self.cfg.respawn_guard_ms) < wait {
            self.log(format!(
                "client {pid}: respawn request {id:#04x} ignored (too early: {age} ms after the death, wait {wait} ms)"
            ));
            return;
        }
        self.log(format!(
            "client {pid}: respawn request {id:#04x} answered {age} ms after the death"
        ));
        self.spawn(pid, now);
        self.respawn_match_begin(pid, now);
    }

    /// With `respawn_match_begin` (on by default): `GS_MATCH_BEGIN` without the intro after a respawn's spawn set,
    /// so the 2009 client, now in player state 1, plays its round-start voice again (its intro banner stays latched
    /// for the round). Head as at the round start: the seconds left, the waiting flag of the configuration, no
    /// countdown, round number 0.
    // DISASM: cshell+0x49f00..0x49f7b (the voice for every 0x45 taken in player state 1); cshell+0x4a0ff (banner
    // latch)
    // DC-DEFAULT: DSV-473 (USER REPORT 2026-10-31; settles DSV-98) on. Only the in-round respawns of the respawn
    // classes come here (request or server timer); the round classes' next-round spawns have their own 0x45 rule
    // (DSV-243, DSV-321) and the infected respawn of class 11 sends none
    pub(super) fn respawn_match_begin(&mut self, pid: u8, now: Millis) {
        if !self.cfg.respawn_match_begin
            || !self
                .players
                .get(&pid)
                .is_some_and(|p| p.life == Life::Alive)
        {
            return;
        }
        let mb = gs::MatchBegin {
            round_secs: self.round_secs_left(now),
            waiting_banner: self.cfg.waiting_banner || self.in_warmup(),
            countdown_secs: 0,
            intro: None,
            round_no: 0,
        };
        self.send(pid, gs::match_begin(&mb, self.rules.mode_class));
    }

    /// `UP_FIRE` (one pellet): ammunition check, hit validation, damage, kill. Every pellet that reports an object
    /// hit gets one verdict in the hit counters (DSV-42); refusals are logged with `-verbose`.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 server answer, Appendix A steps A8, A9
    // DC-DEFAULT: DC-C2S-4 (proposed) clip mirror; DC-C2S-11 (proposed) validation and damage; DSV-42 (proposed)
    // rewind and verdicts
    pub(super) fn on_fire(&mut self, pid: u8, f: &UpFire, now: Millis) {
        let claim = f.hit.and_then(|h| h.1);
        self.cone_refused = self.cone_check(pid, f);
        let verdict = self.fire_verdict(pid, f, now);
        self.cone_refused = false;
        // misses and co-op AI hits (judged by the co-op layer) are not counted
        let (Some(obj), Some(v)) = (claim, verdict) else {
            return;
        };
        // an accepted claim is counted before its damage (a lethal hit may end the round and log the counters)
        if !v.accepted() {
            self.hit_counts.add(v);
        }
        if self.cfg.verbose_hits && !v.accepted() {
            self.log(format!(
                "client {pid}: hit claim on object {obj} refused ({})",
                v.name()
            ));
        }
    }

    /// The shot-direction check of one pellet (DSV-102): the direction rebuilt from the 2009 cone with the player's
    /// counter, compared with the claimed one; counted, and logged when it is off (the first
    /// [`CONE_LOG_PER_LINK`] per link). Returns true when the pellet is refused (`cone_check=refuse` and
    /// `cone_off`).
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-6
    // DC-DEFAULT: DSV-102 (proposed) log by default; `cone_na` for a link that never drew from the table
    pub(super) fn cone_check(&mut self, pid: u8, f: &UpFire) -> bool {
        let mode = self.cfg.cone_check;
        if mode == crate::cone::ConeMode::Off {
            return false;
        }
        let tol = self.cfg.cone_tolerance;
        let table = self.tables.cone_table();
        let bounds = self.tables.weapons.cone_bounds(f.weapon_id);
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        let c = crate::cone::check(table, bounds, &mut p.cone, f, tol);
        let refuse =
            mode == crate::cone::ConeMode::Refuse && c.verdict == crate::cone::ConeVerdict::Off;
        self.cone_counts.add(&c, f, refuse);
        let mut lines = Vec::new();
        match c.verdict {
            crate::cone::ConeVerdict::Off => {
                p.cone.logged += 1;
                if p.cone.logged <= CONE_LOG_PER_LINK {
                    let bounds = match c.bounds {
                        Some((lo, hi)) => format!("{lo}..{hi}"),
                        None => "none".to_string(),
                    };
                    lines.push(format!(
                        "client {pid}: cone_off weapon {} stat {}: {:.1e} rad from the 2009 cone (h {}, row bounds {bounds},                          frac {}, scale {}, zoom {}){}",
                        f.weapon_id,
                        f.stat,
                        c.error,
                        c.h.map_or("-".to_string(), |h| h.to_string()),
                        f.frac,
                        f32::from_bits(f.shot_no),
                        u8::from(f.moving),
                        if refuse { ", refused" } else { "" }
                    ));
                }
                if p.cone.logged == CONE_LOG_PER_LINK + 1 {
                    lines.push(format!(
                        "client {pid}: further cone_off pellets are counted, not logged"
                    ));
                }
            }
            crate::cone::ConeVerdict::Na if !p.cone.na_noted => {
                p.cone.na_noted = true;
                lines.push(format!(
                    "client {pid}: shot directions do not follow the 2009 cone table (cone_na: a client that does not                      draw from the table, such as a DeCombat bot or dc-client; counted, not logged)"
                ));
            }
            _ => {}
        }
        for l in lines {
            self.log(l);
        }
        refuse
    }

    /// Whether player `pid` is inside its spawn window (start note 0xa6 sub 0x1f sent, end note 0x25 not yet) while
    /// the room's spawn shield is on: such a player takes no bullet, knife or blast damage.
    // EVIDENCE: cshell+0x238eb0 (no hit claimed on a character with +0x53a set); DC-DEFAULT: DSV-156, DSV-166
    pub(super) fn spawn_shielded(&self, pid: u8) -> bool {
        self.cfg.spawn_shield && self.spawn_notes.iter().any(|n| n.pid == pid)
    }

    /// Whether a player of team `by` may hurt another player of team `victim`: other teams always, the same team
    /// only with the room's friendly fire; in Spy Hunt the hunters are a team (never each other) and a spy or the
    /// agent hurts anyone (DSV-453 amended 2026-10-31). The 2009 mode row has team kills on (ROM `EnableTeamKillSetting` TRUE for the class-9 row); the
    /// client's drawing of a fellow spy as a team mate (cshell+0xb60f0 / cshell+0x5ba60) is a display rule only.
    // DC-DEFAULT: DSV-450 (proposed, project decision "spies need to be able to kill other spies"; replaces
    // DSV-390) everyone against everyone in class 9
    // OBSERVED: ROM table key GAMEROOM.TXT class-9 mode row EnableTeamKillSetting
    pub(super) fn team_may_hurt(&self, by: u8, victim: u8) -> bool {
        if self.is_spy() {
            // DC-DEFAULT: DSV-453 (amended by project decision "regular players ... can't attack regular
            // players ... can only attack spies and super spy; spies / super spy ... can attack everyone"): the
            // hunters are a team, a spy or the agent (team 2) hurts anyone
            return !(by == super::spy::HUNTERS && victim == super::spy::HUNTERS);
        }
        if by != victim {
            return true;
        }
        self.rules.team_damage
    }

    /// The refusal a hit claim of `pid` on the object `obj` gets from its victim alone: `VictimDead` for a character
    /// that is not alive, `Friendly` for a team mate's while the room has no friendly fire; `None` for a living
    /// enemy and for an object that is no other player's character. The shot effect of such a claim takes the miss
    /// form (DSV-101) before the claim is judged, and [`Game::fire_verdict`] gives the same verdict.
    // DC-DEFAULT: DC-C2S-11 (proposed) the two refusals; DSV-101 (proposed) shared with the shot effect
    pub(super) fn target_refusal(&self, pid: u8, obj: u16) -> Option<HitVerdict> {
        let team = self.players.get(&pid)?.team;
        let v = self
            .players
            .values()
            .find(|v| v.obj == obj && v.pid != pid)?;
        if v.life != Life::Alive {
            Some(HitVerdict::VictimDead)
        } else if !self.team_may_hurt(team, v.team) {
            Some(HitVerdict::Friendly)
        } else if self.spawn_shielded(v.pid) {
            // inside the spawn window: from the start note (0xa6 sub 0x1f) to the end note (sub 0x25)
            // EVIDENCE: cshell+0x238eb0 (no hit claimed on a character with +0x53a set); DC-DEFAULT: DSV-156
            Some(HitVerdict::SpawnShield)
        } else {
            None
        }
    }

    /// The work of [`Game::on_fire`] and the verdict of a hit claim (`None` for a co-op AI hit or a miss that got
    /// past the common checks).
    fn fire_verdict(&mut self, pid: u8, f: &UpFire, now: Millis) -> Option<HitVerdict> {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return Some(HitVerdict::NotRunning);
        }
        if !f.complete {
            return Some(HitVerdict::Incomplete);
        }
        if self.players.get(&pid).map(|p| p.life) != Some(Life::Alive) {
            return Some(HitVerdict::ShooterDead);
        }
        // DC-DEFAULT: DSV-29 (proposed) the fire within the window after the last hold is the throw's release
        let throw = self.take_throw(pid, now);
        let launched = !throw && self.tables.weapons.fires_projectile(f.weapon_id, f.ammo_id);
        let wall = if throw {
            self.throw_sound(pid, f);
            None
        } else if launched {
            // DC-DEFAULT: DSV-181 (amended, proposed) a launcher's shot record carries no hit (its round is flown)
            self.launch_effect(pid, f);
            None
        } else {
            self.shot_effect(pid, f)
        };
        let wi = self.carried_slot(pid, f.weapon_id, Some(f.ammo_id));
        let shooter = self.players.get_mut(&pid).expect("player");
        let w = &mut shooter.weapons[wi];
        if !w.shot_open {
            w.shot_open = true;
            w.shot_ok = w.clip_size == 0 || w.clip > 0;
            if w.shot_ok && w.clip_size > 0 {
                w.clip -= 1;
            }
        }
        let ok = w.shot_ok;
        if f.last {
            w.shot_open = false;
        }
        let shooter_pos = shooter.pos;
        if !ok {
            self.log(format!("client {pid}: shot beyond the clip, no damage"));
            return Some(HitVerdict::ClipEmpty);
        }
        // DC-DEFAULT: DSV-102 (proposed) `cone_check=refuse`: after the clip mirror (the client spent the round)
        if self.cone_refused {
            return Some(HitVerdict::ConeOff);
        }
        if throw {
            if self.throw_release(pid, f, now) {
                self.throw_count(pid, wi);
            }
            return Some(HitVerdict::Throw);
        }
        // DC-DEFAULT: DSV-181 (proposed) a launcher's shot (no hit from the client) is flown by the server
        if launched {
            self.rocket_release(pid, f, now);
            return Some(HitVerdict::Throw);
        }
        // co-op: a hit on an AI character (FIRETEAM_2009.md §1.8)
        if self.is_coop() && self.coop_on_fire(pid, f, wall, now) {
            return None;
        }
        // DC-DEFAULT: DSV-122 (proposed) a pellet without a hit object is traced against the breakables here
        if matches!(f.hit, None | Some((_, None))) && self.ray_breakables(pid, f, now) {
            return None;
        }
        let Some((dist, Some(obj))) = f.hit else {
            return None;
        };
        let Some(victim) = self
            .players
            .values()
            .find(|v| v.obj == obj && v.pid != pid)
            .map(|v| (v.pid, v.pos))
        else {
            // DC-DEFAULT: DSV-115 (proposed) a hit on a breakable world model is the destructibles' (not counted)
            if self.hit_breakable(pid, obj, f, dist, wall.is_some(), now) {
                return None;
            }
            return Some(HitVerdict::UnknownVictim);
        };
        let (vpid, vpos) = victim;
        if let Some(v) = self.target_refusal(pid, obj) {
            return Some(v);
        }
        // geometry: the reconstructed hit point near the victim's box, the origin near the shooter
        let dl = len3(f.dir);
        if !(dl.is_finite() && dl > 1e-6 && dist.is_finite() && dist >= 0.0) || !finite3(f.origin) {
            return Some(HitVerdict::BadGeometry);
        }
        let dir = [f.dir[0] / dl, f.dir[1] / dl, f.dir[2] / dl];
        let hit = [
            f.origin[0] + dist * dir[0],
            f.origin[1] + dist * dir[1],
            f.origin[2] + dist * dir[2],
        ];
        let from_shooter = len3(sub3(f.origin, shooter_pos));
        if from_shooter > self.cfg.origin_tolerance {
            self.log(format!(
                "client {pid}: hit on client {vpid} refused (origin {from_shooter:.0} from the shooter)"
            ));
            return Some(HitVerdict::OriginFar);
        }
        // a melee swing reaches the weapon row's `Range` from the eye: the 2009 client traces no farther
        // (WEAPON_FIRE_2009.md §5.2, row Range; the knife row 105), so a farther melee claim is refused
        // DC-DEFAULT: DSV-161 (proposed) melee only (ammunition damage types of MELEE_DAMAGE_TYPES), slack
        // MELEE_RANGE_SLACK
        let melee = self
            .tables
            .weapons
            .damage_type(f.weapon_id, f.ammo_id)
            .is_some_and(|t| super::destruct::MELEE_DAMAGE_TYPES.contains(&t));
        if let Some(range) = self.tables.weapons.range(f.weapon_id).filter(|_| melee) {
            if dist > range + MELEE_RANGE_SLACK {
                self.log(format!(
                    "client {pid}: melee hit on client {vpid} refused ({dist:.0} units, row range {range:.0})"
                ));
                return Some(HitVerdict::OutOfRange);
            }
        }
        let d = self.hit_dims_of(vpid);
        let tol = self.cfg.hit_tolerance;
        let ext = [d[0] + tol, d[1] + tol, d[2] + tol];
        // DC-DEFAULT: DSV-42 (proposed) the latest position first, then the victim's own path of the last rewind_ms
        let path = self
            .players
            .get(&vpid)
            .map(|v| v.track.path(now, self.cfg.rewind_ms, vpos))
            .unwrap_or_else(|| vec![vpos]);
        let Some(centre) = hits::match_path(&path, hit, ext) else {
            self.log(format!(
                "client {pid}: hit on client {vpid} refused (point {:.0} from the box centre)",
                len3(sub3(hit, vpos))
            ));
            return Some(HitVerdict::OutsideBox);
        };
        let rewound = centre != vpos;
        // SPEC: REFERENCE_FINDINGS_botduel.md F1 (the reference stack's effect record of a shot stops at the first wall; REFERENCE);
        // CAPTURE_tdm6_VALIDATION.md §5.3 (its damage decision does not trace)
        // DC-DEFAULT: DSV-34 (proposed) the shot is traced from its origin to the reported hit point, the point that
        // lies in the box at the matched position (the one the zone test below uses)
        if let Some(w) = wall {
            self.log(format!(
                "client {pid}: hit on client {vpid} refused (a wall {w:.0} units along a {dist:.0}-unit shot)"
            ));
            return Some(HitVerdict::Wall);
        }
        let head = hit[1] >= centre[1] + d[1] - 2.0 * d[1] * self.cfg.head_fraction;
        // DC-DEFAULT: DSV-30 (proposed) health damage = ROM InstDamage x the zone multiplier (ROM skeleton node of the
        // reported model part, else head / body factor)
        let (zone_factor, zone) = self.hit_zone(vpid, f, head);
        let base = self.hit_damage(pid, f);
        let info = HitInfo {
            shooter: pid,
            victim: vpid,
            weapon_id: f.weapon_id,
            ammo_id: f.ammo_id,
            base_damage: base,
            head: zone == gs::KILL_ZONE_HEAD,
            distance: dist,
            zone_factor,
            zone,
        };
        let dmg = match self.cfg.hit_hook {
            Some(h) => (h.0)(&info),
            None => Some(base * zone_factor),
        };
        let Some(dmg) = dmg.filter(|d| d.is_finite() && *d > 0.0) else {
            return Some(HitVerdict::NoDamage);
        };
        // SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §3.1 item 3 (`zone` of 0x74 = the ammunition's InstDamageType)
        let dtype = self
            .tables
            .weapons
            .damage_type(f.weapon_id, f.ammo_id)
            .unwrap_or(super::TODO_DAMAGE_ZONE);
        let src = DamageSource {
            weapon: f.weapon_id,
            dtype,
        };
        let v = if rewound {
            HitVerdict::AcceptedRewound
        } else {
            HitVerdict::Accepted
        };
        // DC-DEFAULT: DSV-42 (proposed) `-verbose` also names each accepted hit: the node the client reported (the
        // extra block's part byte, 255 = none), the zone and factor it gave and the health damage before armour
        if self.cfg.verbose_hits {
            let node = f
                .extra
                .filter(|x| x.obj == Some(obj))
                .map_or(-1, |x| i32::from(x.kind));
            self.log(format!(
                "client {pid}: hit on client {vpid} accepted (node {node}, zone {zone}, factor {zone_factor:.2}, damage {dmg:.1})"
            ));
        }
        self.hit_counts.add(v);
        self.apply_hit(vpid, Some(pid), dmg, base, dir, src, zone, now);
        Some(v)
    }

    /// Applies one damage without a zone multiplier (blasts, co-op AI shots): [`Game::apply_hit`] with the damage as
    /// its own base.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_damage(
        &mut self,
        vpid: u8,
        killer: Option<u8>,
        dmg: f32,
        dir: [f32; 3],
        src: DamageSource,
        zone: u8,
        now: Millis,
    ) {
        self.apply_hit(vpid, killer, dmg, dmg, dir, src, zone, now);
    }

    /// Applies one accepted hit: the worn armour's share (by the damage type and the hit location `zone`) first,
    /// then health ([`armour_split`]). A lethal hit sends the kill messages to everyone before the victim's own
    /// damage, status and death-state messages (the reference stack's order in one block).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3, §10 steps 14-17; CAPTURE_tdm4_KILL_VALIDATION.md §3.1, §3.2 item 3 (0x6d /
    // 0xb5 before the victim's 0x74 / 0x66 / 0x67)
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_hit(
        &mut self,
        vpid: u8,
        killer: Option<u8>,
        dmg: f32,
        base: f32,
        dir: [f32; 3],
        src: DamageSource,
        zone: u8,
        now: Millis,
    ) {
        self.apply_hit_as(
            vpid,
            killer,
            dmg,
            base,
            dir,
            src,
            zone,
            TODO_DAMAGE_KIND,
            now,
        );
    }

    /// [`Game::apply_hit`] with the `dmg_kind` byte of the victim's `GS_DAMAGE_TAKEN` (weapon hits and blasts 3,
    /// falls 0).
    // OBSERVED: the three captured falls carried `dmg_kind` 0 (fall.rs `FALL_DAMAGE_KIND`; REFERENCE)
    // DC-DEFAULT: DSV-76 (proposed)
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_hit_as(
        &mut self,
        vpid: u8,
        killer: Option<u8>,
        dmg: f32,
        base: f32,
        dir: [f32; 3],
        src: DamageSource,
        zone: u8,
        dmg_kind: u8,
        now: Millis,
    ) {
        // DC-DEFAULT: DSV-483 (proposed) no damage during a Possession move
        if self.in_flight(vpid) {
            return;
        }
        // DC-DEFAULT: DSV-212 (proposed) Quarantine: an infected attacker's claws infect, his other weapons do nothing
        if self.infect_on_hit(vpid, killer, src.weapon, now) {
            return;
        }
        // DC-DEFAULT: DSV-482 (proposed, project decision "apply the .2 multiplier to the number so that it
        // scales in all game modes") Hardcore: every damage a player takes passes here (weapons, knife, blasts, the
        // bomb, AI shots, falls); health and armour stay on their shown scale, so one multiplier serves every mode
        let scale = self.cfg.health_scale;
        let (dmg, base) = (dmg / scale, base / scale);
        let Some(v) = self.players.get(&vpid) else {
            return;
        };
        let share = match v.armor_absorb {
            Absorb::Flat(x) => x,
            Absorb::Gear(n) => self
                .tables
                .weapons
                .gear
                .get(&n)
                .map_or(0.0, |g| g.share(src.dtype, zone)),
            // DC-DEFAULT: DSV-430 (proposed) the worn rows' shares add up, at most the whole hit
            Absorb::Worn(rows) => rows
                .iter()
                .flatten()
                .filter_map(|n| self.tables.weapons.gear.get(n))
                .map(|g| g.share(src.dtype, zone))
                .sum::<f32>()
                .min(1.0),
        };
        let (to_health, to_armor) =
            armour_split(dmg, base, share, v.armor, self.cfg.armor_cost_from_base);
        let armor_before = v.armor;
        let armor = (v.armor - to_armor).max(0.0);
        let health = (v.health - to_health).max(0.0);
        if let Some(v) = self.players.get_mut(&vpid) {
            v.armor = armor;
            v.health = health;
        }
        let lethal = health <= 0.0;
        if lethal {
            self.kill_feed(vpid, killer, src.weapon, zone, now);
        }
        // SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §3.1 items 1-4 (`from_dir` = shot direction x health loss / 100,
        // `zone` = the ammunition's ROM damage type, `damage` = the rounded health loss; REFERENCE)
        // OBSERVED: CAPTURE_tdm6_VALIDATION.md §3 item 3 (`damage` is the health damage before the clamp to the
        // remaining health: 210 on a victim with 70.25 left, 27 on the lethal arm hits with 19.675 left; `|from_dir|`
        // = min(1, that damage / 100): 1.0, 0.2975, 0.4232; the real client received the unclamped 27 as a victim)
        // DC-DEFAULT: DC-KILL-2 (proposed); DSV-33 (proposed, 0x74 correction staged by capval-tdm6)
        let dtype = src.dtype;
        let scale = (to_health / 100.0).min(1.0);
        self.send(
            vpid,
            gs::damage_taken(&gs::Damage {
                from_dir: [dir[0] * scale, dir[1] * scale, dir[2] * scale],
                unused: dtype,
                mode: TODO_DAMAGE_MODE,
                ammo_kind: super::DAMAGE_NO_AMMO_ROW,
                dmg_kind,
                zone: dtype,
                part: TODO_DAMAGE_PART,
                damage: to_health.round().max(0.0) as u32,
            }),
        );
        self.send(vpid, gs::player_status(gs::STATUS_HEALTH, health));
        if armor != armor_before {
            self.send(vpid, gs::player_status(gs::STATUS_ARMOR, armor));
        }
        // DC-DEFAULT: DSV-64 (proposed) the pain (and head-shot) sound of a damaging hit
        if to_health > 0.0 {
            self.pain_sound(vpid, zone == gs::KILL_ZONE_HEAD, now);
            // DC-DEFAULT: DSV-127 (proposed) the hit reaction (on by default, project decision)
            if !lethal {
                self.pose_on_hit(vpid, dir, now);
            }
        }
        if lethal {
            // co-op: the new lives count before the death states (FIRETEAM_2009.md §2.3)
            if self.is_coop() {
                self.coop_on_human_death(vpid);
            }
            self.death_states(vpid);
            self.death_presentation(vpid, zone == gs::KILL_ZONE_HEAD, dir);
            self.death_note(vpid);
            self.check_goal(now);
            // DC-DEFAULT: DSV-93 (proposed) the victim's weapon drops (not on the kill that ends the round)
            self.drop_on_death(vpid, now);
        }
    }

    /// The death note (0xa6 [0x18] sub 0x1d) of the victim's character to every introduced player, unreliable, after
    /// the lethal shot effect: the clients drop the client FX attached to the dead character.
    // OBSERVED: capture 20261025_065016_tdm6 seq 10134 / 10136 (after the shot effects of the
    // lethal shot, unreliable, on both links; one per death, 7 of 7) and 20261025_002236_tdm4 seq 3133 / 3135 (REFERENCE,
    // accepted by the real client); 20261025_125432_rust6 (0 of 24 deaths: the bot's eye FX stayed at its death spot)
    // DC-DEFAULT: DSV-91 (proposed)
    fn death_note(&mut self, vpid: u8) {
        let Some(obj) = self.players.get(&vpid).map(|p| p.obj) else {
            return;
        };
        let m = gs::death_note(obj);
        for to in self.introduced() {
            self.out.push(super::GameOut::Unreliable(to, m.clone()));
        }
    }

    /// A death outside a hit (tests): kill feed, death states, round-end test.
    #[cfg(test)]
    pub(super) fn kill(
        &mut self,
        vpid: u8,
        killer: Option<u8>,
        weapon: u16,
        head: bool,
        now: Millis,
    ) {
        let zone = if head {
            gs::KILL_ZONE_HEAD
        } else {
            super::TODO_KILL_ZONE_BODY
        };
        if let Some(v) = self.players.get_mut(&vpid) {
            v.health = 0.0;
        }
        self.kill_feed(vpid, killer, weapon, zone, now);
        self.death_states(vpid);
        self.death_presentation(vpid, head, [0.0, 0.0, 1.0]);
        self.death_note(vpid);
        self.check_goal(now);
        self.drop_on_death(vpid, now);
    }

    /// What the other clients see of a death: the character hidden, its weapon model gone and a corpse playing the
    /// death clip at the victim's position (the session writes the records).
    // OBSERVED: capture 20261025_065016_tdm6 (at each kill the reference stack sent, to both links in the
    // tick of the kill: a change record of the victim's character with the hidden flag words and an empty attachment
    // list, the removal of its weapon model, and the create of a kind-1 corpse object with a type-0x26 blob playing a
    // death clip; REFERENCE, accepted by the real client); capture 20261025_081704_rust3 (without
    // them the real client kept the dead bot standing and hittable)
    // DC-DEFAULT: DSV-44 (proposed)
    fn death_presentation(&mut self, vpid: u8, head: bool, dir: [f32; 3]) {
        let Some(p) = self.players.get(&vpid) else {
            return;
        };
        let Some(c) = p.character.as_ref() else {
            return;
        };
        let clip = c.anims.death_clip(head, dir, p.yaw);
        // the dead character's own stream carries the corpse's clip from here on (relay.rs `stream_for`)
        // DC-DEFAULT: DSV-44 (amendment, proposed)
        if let Some(v) = self.players.get_mut(&vpid) {
            v.death_clip = Some(clip);
        }
        let Some(p) = self.players.get(&vpid) else {
            return;
        };
        self.out.push(super::GameOut::Died {
            pid: vpid,
            pos: p.pos,
            yaw: p.yaw,
            clip,
        });
    }

    /// Death: counters, the kill feed (0x6d) to everyone and the team kill totals (0xb5) to everyone. The victim is
    /// dead from here on. The 0x6d `kill_kind` is the multi-kill level (the client draws the killer's banner and
    /// plays its voice line from it); a team kill carries level 0, which the client announces nowhere.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (the clients count kills themselves from 0x6d), §10 steps 15-17;
    // CAPTURE_tdm4_KILL_VALIDATION.md §3.2 (0x6d fields, then 0xb5 [team 1 kills][team 2 kills]; never 0x4a)
    // DC-DEFAULT: DC-KILL-3 (proposed) 0xb5 after each kill; DSV-33 (proposed) 0x6d values; DC-C2S-13 (amended,
    // staged DSV-105) multi-kill chain; DSV-106 (staged) team kill form
    fn kill_feed(&mut self, vpid: u8, killer: Option<u8>, weapon: u16, zone: u8, now: Millis) {
        let Some(vteam) = self.players.get(&vpid).map(|v| v.team) else {
            return;
        };
        if let Some(v) = self.players.get_mut(&vpid) {
            v.life = Life::Dying { died: now };
            v.health = 0.0;
            v.vel = [0.0; 3];
            // DC-DEFAULT: DSV-76 (proposed) a death drops the fall in progress
            v.fall = None;
            // DC-DEFAULT: DSV-105 (staged) a death ends the victim's multi-kill chain
            v.kill_chain = None;
            // DC-DEFAULT: VIS-14 (proposed) the killer stays shown to the dead viewer
            v.killed_by = killer.filter(|&k| k != vpid);
        }
        let head = zone == gs::KILL_ZONE_HEAD;
        let kinfo = killer
            .filter(|&k| k != vpid)
            .and_then(|k| self.players.get(&k).map(|p| (k, p.team, p.uid)));
        // A team kill counts nothing on the 2009 client: no kill for the killer, no death for the victim, no streak
        // (the client compares the two records' teams before its counters); a death without a killer or by the own
        // hand counts the victim's death only. The server's own counters follow the client's.
        // EVIDENCE: cshell+0x2dd9a..0x2dda0 (same team: the counting calls are skipped), cshell+0x2df08..0x2df3a (no
        // killer: the victim's death only)
        // DC-DEFAULT: DSV-106 (staged)
        // DC-DEFAULT: DSV-236 (proposed) Spy Hunt: every other player is a foe
        let spy = self.is_spy();
        let mate_kill = !spy && kinfo.is_some_and(|(_, kteam, _)| kteam == vteam);
        let vuid = self.players.get(&vpid).and_then(|v| v.uid);
        if let (Some(u), false) = (vuid, mate_kill) {
            self.stats.entry(u).or_default().deaths += 1;
        }
        let mut foe_kill = false;
        let mut streak_flag = 0;
        let mut kill_kind = TODO_KILL_KIND;
        if let Some((k, kteam, kuid)) = kinfo {
            foe_kill = kteam != vteam || spy;
            if let Some(u) = kuid {
                let s = self.stats.entry(u).or_default();
                if foe_kill {
                    s.kills += 1;
                    if head {
                        s.headshots += 1;
                    }
                } else {
                    s.team_kills += 1;
                }
            }
            if foe_kill {
                // DC-DEFAULT: GMD-10 (proposed) FFA counts player kills (below), no team totals; a team value
                // outside 1 / 2 never indexes the two team counters
                if !self.is_ffa() && !spy {
                    if let Some(s) = (kteam as usize)
                        .checked_sub(1)
                        .and_then(|i| self.scores.get_mut(i))
                    {
                        *s += 1;
                    }
                }
                kill_kind = self.chain_level(k, now);
                if let Some(p) = self.players.get_mut(&k) {
                    p.kills_this_life += 1;
                }
                // `streak_flag` 1 on the first kill of the match only (the client plays its first-kill
                // presentation for it), not on each killer's first kill since its spawn; a team kill never takes it
                // OBSERVED: capture 20261025_065016_tdm6 (7 kills, the flag 1 on the first only;
                // the user's own first kill and the bot's first kill after its respawn carried 0; REFERENCE, accepted by
                // the real client); capture 20261025_081704_rust3 (the flag on each killer's
                // first kill since its spawn: the real client showed the first-kill presentation on every kill)
                // DC-DEFAULT: DSV-33 (corrected, staged) the first kill of the match
                if !self.first_kill_done {
                    self.first_kill_done = true;
                    streak_flag = 1;
                }
            } else {
                // The client announces every 0x6d with a known killer whatever the teams (cshell+0xa5fe0 is
                // called for any killer record); level 0 is the one value that shows no banner, plays no voice line
                // and adds no note, also on a head hit (the head-shot banner is a level-1 form). The killer's chain
                // is neither extended nor broken.
                // EVIDENCE: cshell+0x2e122..0x2e1d6 (no team test before the announcement), cshell+0xa628b /
                // cshell+0xa676b (level 0: no form), cshell+0xa629e (head shot: level 1 only)
                // DC-DEFAULT: DSV-106 (staged)
                kill_kind = KILL_KIND_NO_ANNOUNCE;
            }
        }
        let k = gs::Kill {
            victim: vpid,
            killer: kinfo.map_or(gs::NO_KILLER, |x| x.0),
            kill_kind,
            weapon,
            hit_zone: zone,
            relation: TODO_KILL_RELATION,
            keep_hud: false,
            streak_flag,
            feed_value: super::TODO_KILL_FEED_VALUE,
            no_count: false,
        };
        self.broadcast(&gs::kill(&k));
        // No 0xc8: its multi-count form is the client's "N COMBO" banner without a voice line, not the multi-kill
        // banner (that one comes from the 0x6d level above).
        // EVIDENCE: cshell+0x2d8ef..0x2d902 (0xc8 count > 1 -> cshell+0xa52a0), cshell+0xa52a0 (banner slot 10 with
        // the count's digits; a sound only for the head-shot flag)
        // DC-DEFAULT: DSV-105 (staged) never 0xc8 in the kill modes
        // DC-DEFAULT: DC-KILL-3 (proposed) the team kills after each kill; GMD-21 (staged) the round wins (unchanged by
        // a kill) in a round mode, so the scoreboard keeps showing rounds
        self.broadcast(&gs::round_wins(self.team_counters()));
        if let (Some((k, _, _)), true, true) = (kinfo, foe_kill, self.is_ffa()) {
            self.ffa_on_kill(k, vpid, weapon, now);
        }
        self.log(format!(
            "client {vpid} killed by {:?} (zone {zone}, level {kill_kind}{}); score {}:{}",
            kinfo.map(|x| x.0),
            if mate_kill { ", team kill" } else { "" },
            self.scores[0],
            self.scores[1]
        ));
        // DC-DEFAULT: DSV-235 (proposed) Spy Hunt points, the dropped intel, the agent's end
        self.spy_on_kill(vpid, kinfo.map(|x| x.0), now);
        // DC-DEFAULT: HS-1 (project decision) a foe kill by a player rolls a case drop; DSV-512 (proposed) none in the
        // warm-up
        if !self.in_warmup() {
            self.hisec_on_kill(vpid, kinfo.filter(|_| foe_kill).map(|x| x.0), now);
        }
        // DC-DEFAULT: DSV-483 (proposed) Possession: a foe kill moves the killer
        if let (Some((k, _, _)), true) = (kinfo, foe_kill) {
            self.possession_on_kill(k, vpid, now);
        }
    }

    /// The multi-kill level of a foe kill by `killer` at `now`: one more than the level of its previous foe kill when
    /// that came at most the window earlier in the same life, else 1; levels below `multi_kill_min` are sent as 1,
    /// and the level is capped at [`KILL_LEVEL_MAX`].
    // DISASM: cshell+0xa628b..0xa6297 (levels 1..6 by a jump table), cshell+0xa676b (7 and more: one last form);
    // cshell+0x2ac56 (the client's per-level counter is record +0x44 + 4 x level, so a level above 7 would write
    // the record's later fields)
    // DC-DEFAULT: DC-C2S-13 (amended, staged DSV-105) window 4 s, first multi-kill form at level 2, cap 7
    fn chain_level(&mut self, killer: u8, now: Millis) -> u8 {
        let window = self.cfg.multi_kill_window_ms;
        let min = self.cfg.multi_kill_min;
        let Some(p) = self.players.get_mut(&killer) else {
            return TODO_KILL_KIND;
        };
        let level = match p.kill_chain {
            Some((t, l)) if since(now, t) <= window => l.saturating_add(1),
            _ => 1,
        };
        p.kill_chain = Some((now, level));
        if u32::from(level) < min {
            TODO_KILL_KIND
        } else {
            level.min(KILL_LEVEL_MAX)
        }
    }

    /// The victim's death states.
    fn death_states(&mut self, vpid: u8) {
        match self.cfg.death_states {
            // SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §3.3 (0x67 [3] then [2] in one block; the client arms its wait on
            // the 2 and state 3 does not clear the death time)
            // DC-DEFAULT: DSV-32 (proposed)
            // DC-DEFAULT: DC-C2S-10' (proposed, GAME_MESSAGES_2009_C2S.md §12.3): state 3 and state 2 in one block
            DeathStates::Reference => {
                self.send(vpid, gs::player_state(gs::STATE_WAITING));
                self.send(vpid, gs::player_state(gs::STATE_DYING));
            }
            // DC-DEFAULT: DC-C2S-10 (superseded by GAME_MESSAGES_2009_C2S.md §12.3 DC-C2S-10'; kept behind
            // `-deathstates spec`): state 2 at once, 3 after the dying time
            DeathStates::Spec => self.send(vpid, gs::player_state(gs::STATE_DYING)),
        }
    }

    /// The goal test after a kill. In a round mode the goal counts round wins, so a kill only ends the round when it
    /// leaves a team without a living player (GMD-22).
    // DC-DEFAULT: DC-C2S-13 (proposed) the round ends when a team reaches the goal in team kills; GMD-1 (proposed)
    // the goal unit per class (rounds for 2, 3, 11, 12, 13); GMD-22 (staged)
    fn check_goal(&mut self, now: Millis) {
        // co-op: `goal` is the difficulty and the mission ends by waves, lives or the timer
        if self.is_coop() {
            return;
        }
        if self.round_loop() {
            self.check_elimination(now);
            return;
        }
        // DC-DEFAULT: GMD-1 (proposed) player kills in class 6; DSV-148 (proposed)
        if self.is_ffa() {
            self.ffa_check_goal(now);
            return;
        }
        // DC-DEFAULT: DSV-192 (proposed) the flag modes' goal counts captures, not kills
        if self.is_flag() {
            return;
        }
        // DC-DEFAULT: DSV-392 (proposed, replaces DSV-235's goal) Spy Hunt ends at the agent's upload or the time
        // limit, never at a kill or points goal
        if self.is_spy() {
            return;
        }
        let goal = self.rules.goal;
        if goal > 0 && self.scores.iter().any(|&s| s >= goal) {
            let win = if self.scores[0] >= goal { 1 } else { 2 };
            self.end_round(now, win);
            return;
        }
        // DC-DEFAULT: GMD-7 (proposed) class 4 switches sides at half the goal; DSV-147 (proposed) two halves
        self.clan_half_on_kill(now);
    }

    /// The death timeline: with the old death states, state 3 after the dying time; the server's own respawn only
    /// where the mode class has one (a client that never asks, or a fallback for a client that may not ask).
    // DC-DEFAULT: DC-C2S-10 (superseded by GAME_MESSAGES_2009_C2S.md §12.3 DC-C2S-10'; kept behind
    // `-deathstates spec`): state 3 after 2000 ms; DC-C2S-10' / DSV-32 (proposed) no server timer where the 2009
    // client asks itself
    pub(super) fn tick_lives(&mut self, now: Millis) {
        let running = matches!(self.phase, RoundPhase::Running { .. });
        let timer = match self.respawn_rule() {
            super::RespawnRule::ServerTimer { after_ms } => Some(after_ms),
            super::RespawnRule::ClientRequest { fallback_ms, .. } => fallback_ms,
            super::RespawnRule::NextRound => None,
        };
        let pids: Vec<u8> = self.players.keys().copied().collect();
        for pid in pids {
            let Some(life) = self.players.get(&pid).map(|p| p.life) else {
                continue;
            };
            match life {
                Life::Dying { died }
                    if self.cfg.death_states == DeathStates::Spec
                        && since(now, died) >= self.cfg.dying_ms =>
                {
                    if let Some(p) = self.players.get_mut(&pid) {
                        p.life = Life::Waiting { died };
                    }
                    self.send(pid, gs::player_state(gs::STATE_WAITING));
                }
                Life::Dying { died } | Life::Waiting { died }
                    if running
                        && timer.is_some_and(|t| since(now, died) >= t)
                        && self.coop_may_respawn(pid) =>
                {
                    self.log(format!(
                        "client {pid}: respawned by the server {} ms after the death (no request)",
                        since(now, died)
                    ));
                    self.spawn(pid, now);
                    self.respawn_match_begin(pid, now);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::armour_split;

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    /// The split rule with made-up numbers: share of the base while the armour lasts, proportional protection when
    /// it cannot pay, nothing without armour or share, the cost from the zoned damage on request.
    #[test]
    fn armour_split_rule() {
        // base 40, zone factor 0.5 -> 20 damage, share 0.25: health 15, armour pays 0.25 x 40 = 10
        assert!(near(
            armour_split(20.0, 40.0, 0.25, 50.0, true),
            (15.0, 10.0)
        ));
        // the same hit with the cost from the zoned damage: armour pays 0.25 x 20 = 5
        assert!(near(
            armour_split(20.0, 40.0, 0.25, 50.0, false),
            (15.0, 5.0)
        ));
        // a head node (factor 3) with share 0: full damage, armour untouched
        assert!(near(
            armour_split(120.0, 40.0, 0.0, 50.0, true),
            (120.0, 0.0)
        ));
        // 4 points of armour left for a cost of 10: 40 % of the protection
        assert!(near(armour_split(20.0, 40.0, 0.25, 4.0, true), (18.0, 4.0)));
        // no armour: no protection
        assert!(near(armour_split(20.0, 40.0, 0.25, 0.0, true), (20.0, 0.0)));
        // shares outside 0..1 are clamped; negative damage is none
        assert!(near(
            armour_split(20.0, 40.0, 2.0, 100.0, true),
            (0.0, 40.0)
        ));
        assert!(near(
            armour_split(-5.0, 40.0, 0.25, 100.0, true),
            (0.0, 0.0)
        ));
    }
}
