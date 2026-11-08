//! BOT-172: the knife run. A player in the 2009 game moves fastest with the knife in hand (every stance and the sprint
//! scale by `1 - p`, `p` taking the held weapon's `MovePenalty`: BOT-149), so on a long quiet walk he puts the rifle
//! away and runs with the knife, and draws the gun again the moment anything is on. The bot does the same: it draws
//! its melee weapon when the rest of the way to its walk goal is long and nothing is in contention, and draws the
//! weapon it put away (with its clip as it was) at once on any sighting, enemy near, hit, team mate killed near it, or
//! when the way left to a contested goal gets short.
//!
//! The switch is an ordinary weapon change (`76 03 02` then `7f`, BOT-126's 700 ms draw before the next shot); the
//! server takes a weapon change at once (no switch delay of its own). A flag carrier keeps his weapons in the 2009
//! game (the 0xc6 flag events set only the carry flag), so a carrier may knife-run home; the bomb carrier does not.
//! This module is a child of [`crate::shooter`] (it reads the shooter's own state).
// DC-DEFAULT: BOT-172 (proposed) start beyond 600 units of way left (skill 100; up to 1000 at skill 0), stop within 400
// of a contested goal, quiet 5 s (no enemy in sight, none streamed within 1000 units, no hit, no team mate killed
// within 1200 units), 2 s between two runs
// SPEC: MOVEMENT_2009.md §7 (the held weapon's speed modifier); GAME_MODES_2009.md §5.5 (0xc6 flag events: the carry
// flag only, no weapon change); GAME_MESSAGES_2009_C2S.md §4.4, §4.5 (the weapon change pair)

use dc_cone::NO_MODS;
use dc_net_engine::time::since;
use dc_net_engine::Millis;
use serde_json::json;

use super::{unseen_reason, Action, HeldWeapon, Shooter, View, FLICKER_MS, SWITCH_DRAW_MS, THROW_SLOT};
use crate::weapon::{WeaponChange, WeaponGrant};

/// A run starts only with at least this much way left to the walk goal (a skill-100 bot; less skilled bots want a
/// longer way, [`start_leg`]).
pub const LEG_START: f32 = 600.0;
/// Extra way a bot wants per skill point below 100 before it starts a run (skill 0: 1000 units).
pub const LEG_PER_SKILL: f32 = 4.0;
/// The gun is drawn again when the way left to a contested goal drops under this.
pub const LEG_STOP: f32 = 400.0;
/// A run starts only this long after the last contact, hit or team mate killed near the bot.
pub const QUIET_MS: u32 = 5000;
/// An enemy streamed (live, in sight or behind a wall) within this distance counts as contact.
pub const HEAR_RANGE: f32 = 1000.0;
/// A team mate killed within this distance counts as contact.
pub const MATE_DOWN_RANGE: f32 = 1200.0;
/// The least time between the end of one run and the start of the next (no flapping at a threshold).
pub const REARM_MS: u32 = 2000;

/// Goal kinds a run may go on toward: the uncontested walks (any way left keeps the run) ...
pub const FREE_KINDS: [&str; 2] = ["roam", "lost"];
/// ... and the contested ones (the run ends within [`LEG_STOP`] of the goal).
pub const CONTESTED_KINDS: [&str; 4] = ["hunt", "objective", "flag", "idle"];

/// The way a skill `level` bot wants left before it starts a run.
pub fn start_leg(level: u8) -> f32 {
    LEG_START + f32::from(100u8.saturating_sub(level)) * LEG_PER_SKILL
}

/// What the session knows about the bot's walk and its surroundings this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Situation {
    /// The walk goal's kind (`None`: no walk).
    pub kind: Option<&'static str>,
    /// The way left to the walk goal (the planned way, else the straight line).
    pub left: Option<f32>,
    /// The last contact (its kind) and how long ago.
    pub contact: Option<(&'static str, u32)>,
    /// How long ago the bot was last hurt.
    pub hurt_ago: Option<u32>,
    /// How long ago a team mate was killed within [`MATE_DOWN_RANGE`].
    pub mate_down_ago: Option<u32>,
    /// Why no run is allowed at all (the bomb carrier, a mode without runs).
    pub barred: Option<&'static str>,
    /// The bot's skill level.
    pub level: u8,
    /// The shooter may draw the knife now (a gun drawn, a melee weapon granted, no target, no reload, no grenade).
    pub ready: bool,
    /// How long ago the last run ended.
    pub since_stop: Option<u32>,
}

/// The run's next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Start(&'static str),
    Stop(&'static str),
    Stay,
}

/// Why the bot must hold its gun now (contact, a hit, a team mate down, a bar); `None`: quiet.
fn unrest(s: &Situation) -> Option<&'static str> {
    let recent = |ago: Option<u32>| ago.is_some_and(|a| a < QUIET_MS);
    if let Some((kind, ago)) = s.contact {
        if ago < QUIET_MS {
            return Some(kind);
        }
    }
    if recent(s.hurt_ago) {
        return Some("hurt");
    }
    if recent(s.mate_down_ago) {
        return Some("mate_down");
    }
    s.barred
}

/// BOT-172: start, stop or keep the knife run.
pub fn verdict(knifing: bool, s: &Situation) -> Verdict {
    let free = s.kind.is_some_and(|k| FREE_KINDS.contains(&k));
    let contested = s.kind.is_some_and(|k| CONTESTED_KINDS.contains(&k));
    if knifing {
        if let Some(why) = unrest(s) {
            return Verdict::Stop(why);
        }
        if !free && !contested {
            return Verdict::Stop("goal");
        }
        if contested && s.left.is_none_or(|l| l < LEG_STOP) {
            return Verdict::Stop("near_goal");
        }
        return Verdict::Stay;
    }
    let rested = s.since_stop.is_none_or(|t| t >= REARM_MS);
    if s.ready
        && rested
        && unrest(s).is_none()
        && (free || contested)
        && s.left.is_some_and(|l| l > start_leg(s.level))
    {
        return Verdict::Start("long_leg");
    }
    Verdict::Stay
}

/// The session's side of the run: when it started and ended, the last contact, the last team mate killed near.
#[derive(Debug, Clone, Copy, Default)]
pub struct KnifeState {
    pub started: Option<Millis>,
    pub stopped: Option<Millis>,
    pub contact: Option<(Millis, &'static str)>,
    pub mate_down: Option<(Millis, [f32; 3])>,
    pub runs: u32,
    pub run_ms: u64,
}

impl Shooter {
    /// BOT-172: whether a knife run is on (a gun put away for the melee weapon).
    pub fn knifing(&self) -> bool {
        self.knife_run.is_some()
    }

    /// A granted melee weapon (item type 7, else slot 2) the bot can draw.
    fn melee_grant(&self) -> Option<&WeaponGrant> {
        let melee = |g: &&WeaponGrant| match self.cfg.rom.as_ref().and_then(|r| r.kinds.get(&g.weapon)) {
            Some(k) => *k == dc_bute::arms::TYPE_MELEE,
            None => g.slot == 2,
        };
        self.grants
            .values()
            .filter(|g| g.slot < THROW_SLOT && !self.empty.contains(&g.slot))
            .find(melee)
    }

    /// Whether a run may start now: alive, a gun (not a melee weapon) drawn, a melee weapon granted, no target, no
    /// reload, no grenade under way.
    pub fn knife_ready(&self) -> bool {
        self.alive
            && self.selected
            && self.knife_run.is_none()
            && self.held.is_some_and(|h| !self.is_melee(&h))
            && self.melee_grant().is_some()
            && self.target.is_none()
            && self.reload_until.is_none()
            && self.nade.is_none()
    }

    /// The contact that ends or forbids a run: an enemy in clear sight now (`sighting`), a target (`target`), an enemy
    /// streamed live within `hear` (`near`).
    pub fn contact(&self, now: Millis, view: &View, hear: f32) -> Option<&'static str> {
        if self.vis.values().any(|v| since(now, v.1) <= FLICKER_MS) {
            return Some("sighting");
        }
        if self.target.is_some() {
            return Some("target");
        }
        let own = view.own_pos;
        let near = view
            .players
            .iter()
            .filter(|p| Some(p.pid) != view.own_pid)
            .filter(|p| view.own_team.is_none_or(|t| p.team != t))
            .filter(|p| !self.down.contains_key(&p.pid) && unseen_reason(p).is_none())
            .filter_map(|p| p.pos)
            .any(|p| (p[0] - own[0]).hypot(p[2] - own[2]) <= hear);
        near.then_some("near")
    }

    fn draw(&mut self, now: Millis, h: HeldWeapon, why: &'static str, out: &mut Vec<Action>) {
        self.held = Some(h);
        self.stats.selects += 1;
        self.next_fire = now.wrapping_add(SWITCH_DRAW_MS);
        self.ready_at = None;
        out.push(Action::Event(
            "weapon_select",
            json!({"slot": h.slot, "weapon": h.weapon, "ammo": h.ammo, "clip": h.clip, "clip_size": h.clip_size,
                   "stock": h.stock, "range": h.range, "source": h.source, "knife_run": why,
                   "melee": self.is_melee(&h)}),
        ));
        out.push(Action::Select(WeaponChange {
            slot: h.slot,
            weapon_id: h.weapon,
            aux: h.ammo,
        }));
    }

    /// BOT-172: puts the drawn gun away (kept with its clip) and draws the melee weapon.
    pub fn draw_knife(&mut self, now: Millis) -> Vec<Action> {
        let mut out = Vec::new();
        let (Some(gun), Some(g)) = (self.held, self.melee_grant().copied()) else {
            return out;
        };
        let r = self
            .cfg
            .rom
            .as_ref()
            .and_then(|r| r.get(g.weapon).copied())
            .unwrap_or_default();
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
        let knife = HeldWeapon {
            slot: g.slot,
            weapon: g.weapon,
            ammo: g.ammo_kind,
            pellets: dc_cone::pellets(r.pellets, &NO_MODS).max(1),
            clip_size: r.clip.or(g.clip),
            clip,
            stock,
            range: Some(dc_cone::range(r.range, &NO_MODS)),
            source: "knife_run",
            spread: r.spread_params(),
            cone: r.cone,
            cycle_ms: r.cycle_ms,
        };
        self.knife_run = Some(gun);
        self.draw(now, knife, "start", &mut out);
        out
    }

    /// BOT-172: ends the run: the gun put away is drawn again (a sniper rifle too: BOT-114 / BOT-123 go on with it).
    pub fn end_knife(&mut self, now: Millis) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(gun) = self.knife_run.take() {
            self.draw(now, gun, "stop", &mut out);
        }
        out
    }

    /// BOT-172: the run is over without a draw of the bot's own (a death, a spawn, a new grant made the shooter draw
    /// something else): `true` when one was on.
    pub fn knife_lapsed(&mut self, alive: bool) -> bool {
        let lapsed = self.knife_run.is_some()
            && (!alive || !self.alive || self.held.is_none_or(|h| !self.is_melee(&h)));
        if lapsed {
            self.knife_run = None;
        }
        lapsed
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::shooter::{RomWeapon, RomWeapons, Seen, ShooterConfig};
    use crate::skill::Skill;

    fn quiet(kind: &'static str, left: f32) -> Situation {
        Situation {
            kind: Some(kind),
            left: Some(left),
            level: 100,
            ready: true,
            ..Situation::default()
        }
    }

    /// A long quiet walk starts a run; a short one, a recent contact or hit, a team mate down, a bar, a reload or
    /// target (not ready) or a run that just ended does not. A less skilled bot wants a longer way.
    #[test]
    fn a_run_starts_only_on_a_long_quiet_walk() {
        assert_eq!(verdict(false, &quiet("roam", 900.0)), Verdict::Start("long_leg"));
        assert_eq!(verdict(false, &quiet("objective", 700.0)), Verdict::Start("long_leg"));
        assert_eq!(verdict(false, &quiet("roam", 500.0)), Verdict::Stay);
        let no = |s: Situation| assert_eq!(verdict(false, &s), Verdict::Stay, "{s:?}");
        no(Situation { contact: Some(("sighting", 4000)), ..quiet("roam", 900.0) });
        no(Situation { hurt_ago: Some(100), ..quiet("roam", 900.0) });
        no(Situation { mate_down_ago: Some(2000), ..quiet("roam", 900.0) });
        no(Situation { barred: Some("bomb_carrier"), ..quiet("roam", 900.0) });
        no(Situation { ready: false, ..quiet("roam", 900.0) });
        no(Situation { since_stop: Some(1000), ..quiet("roam", 900.0) });
        no(quiet("bomb", 2000.0));
        no(quiet("stand", 2000.0));
        no(Situation { kind: None, left: None, ..quiet("roam", 0.0) });
        // a contact long gone does not count
        let old = Situation { contact: Some(("near", 6000)), hurt_ago: Some(9000), ..quiet("roam", 900.0) };
        assert_eq!(verdict(false, &old), Verdict::Start("long_leg"));
        // skill: 600 at 100, 800 at 50, 1000 at 0
        assert_eq!((start_leg(100), start_leg(50), start_leg(0)), (600.0, 800.0, 1000.0));
        no(Situation { level: 50, ..quiet("roam", 700.0) });
        assert_eq!(
            verdict(false, &Situation { level: 50, ..quiet("roam", 850.0) }),
            Verdict::Start("long_leg")
        );
    }

    /// A run ends at once on contact (its kind is the reason), a hit, a team mate down, a bar, a goal kind no run goes
    /// toward, or near a contested goal; an uncontested walk keeps it to the end.
    #[test]
    fn a_run_stops_on_contact_or_near_a_contested_goal() {
        let stop = |s: Situation| verdict(true, &s);
        assert_eq!(stop(quiet("roam", 900.0)), Verdict::Stay);
        assert_eq!(stop(quiet("roam", 100.0)), Verdict::Stay, "free walk: kept");
        assert_eq!(stop(quiet("hunt", 500.0)), Verdict::Stay, "hysteresis 400..600");
        assert_eq!(stop(quiet("hunt", 350.0)), Verdict::Stop("near_goal"));
        assert_eq!(stop(quiet("flag", 100.0)), Verdict::Stop("near_goal"));
        assert_eq!(
            stop(Situation { contact: Some(("sighting", 0)), ..quiet("roam", 900.0) }),
            Verdict::Stop("sighting")
        );
        assert_eq!(
            stop(Situation { contact: Some(("near", 0)), ..quiet("roam", 900.0) }),
            Verdict::Stop("near")
        );
        assert_eq!(stop(Situation { hurt_ago: Some(0), ..quiet("roam", 900.0) }), Verdict::Stop("hurt"));
        assert_eq!(
            stop(Situation { mate_down_ago: Some(10), ..quiet("roam", 900.0) }),
            Verdict::Stop("mate_down")
        );
        assert_eq!(
            stop(Situation { barred: Some("bomb_carrier"), ..quiet("roam", 900.0) }),
            Verdict::Stop("bomb_carrier")
        );
        assert_eq!(stop(quiet("stand", 900.0)), Verdict::Stop("goal"));
        assert_eq!(stop(Situation { kind: None, ..quiet("roam", 900.0) }), Verdict::Stop("goal"));
    }

    fn armed_with_knife() -> Shooter {
        let mut r = RomWeapons::default();
        r.rows.insert(
            5,
            RomWeapon { pellets: Some(1), clip: Some(30), ammo: Some(6), range: Some(5000.0), ..RomWeapon::default() },
        );
        r.rows.insert(0, RomWeapon { pellets: Some(1), range: Some(100.0), ..RomWeapon::default() });
        r.kinds.insert(0, dc_bute::arms::TYPE_MELEE);
        r.kinds.insert(5, 0);
        let mut s = Shooter::new(ShooterConfig {
            rom: Some(Arc::new(r)),
            skill: Some(Skill::new(100)),
            ..ShooterConfig::default()
        });
        s.on_life(0, 1);
        let grant = |slot, weapon, ammo_kind, ammo| WeaponGrant {
            weapon,
            ammo_kind,
            ammo,
            clip: None,
            class_pos: 0,
            slot,
            equip: false,
            parts: [-1; 4],
        };
        s.on_grant(grant(0, 5, 6, 90.0));
        s.on_grant(grant(2, 0, 0, 1000.0));
        s
    }

    fn view(players: Vec<Seen>) -> View {
        View {
            own_pid: Some(0),
            own_team: Some(2),
            own_pos: [0.0, 50.0, 0.0],
            own_dims: [15.0, 50.0, 15.0],
            players,
            time_ms: 0,
            world: None,
            facing: None,
            moving: Default::default(),
            prefer: None,
            mates: Vec::new(),
            rushers: Vec::new(),
        }
    }

    fn enemy(pos: [f32; 3]) -> Seen {
        Seen {
            pid: 1,
            team: 1,
            obj: 40,
            pos: Some(pos),
            dims: Some([15.0, 50.0, 15.0]),
            silent_ms: 0,
            parked: false,
            pos_known: true,
        }
    }

    /// The knife is drawn with the select pair, the gun comes back with its clip as it was; a near enemy is contact
    /// (and a far one is not); a respawn's own select ends the run without a draw of its own.
    #[test]
    fn the_knife_is_drawn_and_the_gun_comes_back_as_it_was() {
        let mut s = armed_with_knife();
        let v = view(Vec::new());
        s.poll(600, &v);
        assert_eq!(s.held().map(|h| h.slot), Some(0));
        s.held.as_mut().unwrap().clip = 17;
        assert!(s.knife_ready());
        let a = s.draw_knife(1000);
        assert!(a.iter().any(|x| matches!(x, Action::Select(c) if c.slot == 2 && c.weapon_id == 0)));
        assert!(s.knifing() && !s.knife_ready());
        assert_eq!(s.held().map(|h| h.weapon), Some(0));
        assert!(!s.knife_lapsed(true), "the knife is in hand: still on");
        // contact: none far away, a streamed enemy within the hearing range
        assert_eq!(s.contact(1000, &view(vec![enemy([3000.0, 50.0, 0.0])]), HEAR_RANGE), None);
        assert_eq!(s.contact(1000, &view(vec![enemy([800.0, 50.0, 0.0])]), HEAR_RANGE), Some("near"));
        let a = s.end_knife(1500);
        assert!(a.iter().any(|x| matches!(x, Action::Select(c) if c.slot == 0 && c.weapon_id == 5)));
        assert_eq!(s.held().map(|h| (h.weapon, h.clip)), Some((5, 17)));
        assert!(!s.knifing());
        // a death ends the run without a draw
        s.draw_knife(2000);
        s.on_life(2100, 0);
        assert!(s.knife_lapsed(true) && !s.knifing());
        assert!(s.end_knife(2200).is_empty());
    }
}
