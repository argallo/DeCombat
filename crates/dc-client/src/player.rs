//! The local player: dc-sim's state stepped by dc-sim's fixed-step clock, the last two states kept for render
//! interpolation, and the first-person eye derived from them.
//!
//! CLI-MOVE-1 (frame-rate independence, project decision: no frame-rate cap): the mover runs at a fixed
//! step of 1/60 s whatever the display rate. Real time is accumulated exactly ([`FixedStep`], integer
//! microseconds); each rendered frame runs zero, one or more steps and draws between the last two states. Every
//! per-frame rule of the 2009 mover (the 0.1-unit displacement drop, the `dt^2 / 4` term, the ramp and hop timers,
//! the cap-then-accelerate order) is applied once per fixed step, so the 2009 numbers that were tuned for about 60
//! frames per second hold at 30, 144 or 1000 frames per second. What the server sets is followed from the frame
//! state ([`LocalPlayer::follow`]): the player state, the step height and the sprint permission.
// SPEC: MOVEMENT_2009.md §4 (time step), §13.4 (eye position, step-up smoothing, crouch dip);
// MOVEMENT_2009_addendum_servervars.md B (per-frame rules, fixed-step recipe)
// DC-DEFAULT: CLI-MOVE-1 (proposed) fixed 1/60 s mover step with render interpolation, no frame-rate cap

use crate::input::InputSource;
use crate::look::ViewAngles;
use crate::todo;
use crate::world::{FrameState, SpawnPoint};
use dc_sim::{
    drop_to_floor, Collider, Container, ContainerVolumes, Correction, Cylinder, FixedStep,
    MovementParams, PlayerState, StepEvents, TickInput, V3,
};

/// What one simulation tick consumed and produced (for checks and logs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickRecord {
    pub tick: u64,
    pub before: V3,
    pub after: V3,
    pub events: StepEvents,
}

/// The head-gear, backpack and goggle rows of the speed table for an equipment list (item number per slot): the shop
/// look's mapping (head item -> `Hair<n>`, starting at row 0 without one; backpack item -> `Backpack<n>`; face
/// attachment -> `Goggle<n>`; none without an item).
// EVIDENCE: cshell+0x5c270 (the penalty adds the head-gear row of the own record +0x150 and the backpack row of
// +0x170, index -1 = 0), cshell+0x1fd390 (the dash adds head gear, backpack and goggles), cshell+0x14e410 /
// cshell+0x12cc60 (the look record's item ids per type and its start values: head 0, no backpack, no goggles)
// DC-DEFAULT: CLI-1137 (proposed, playtest report "not sure if gear worn is properly changing speed") the in-match record's
// head / backpack / goggle rows are the equipped items' ids by the shop look's mapping (the writer of the in-match
// record was not read); before, the client counted 0 for all three (CLI-340)
pub fn gear_rows(equipment: &[i32]) -> (Option<u32>, Option<u32>, Option<u32>) {
    let mut eq = [-1i32; 15];
    for (d, s) in eq.iter_mut().zip(equipment) {
        *d = *s;
    }
    let ids = crate::menu::figure::LookIds::from_equipped(&eq);
    let row = |v: i32| u32::try_from(v).ok();
    (
        row(ids.head),
        ids.backpack.and_then(row),
        ids.goggle.and_then(row),
    )
}

/// The local player.
#[derive(Debug, Clone)]
pub struct LocalPlayer {
    pub params: MovementParams,
    /// State after the last tick.
    pub state: PlayerState,
    /// State before the last tick (render interpolation runs from here to `state`).
    pub prev: PlayerState,
    pub clock: FixedStep,
    /// Events of every tick so far, accumulated.
    pub events: StepEvents,
    pub jumps: u64,
    pub landings: u64,
    /// Controller state for the simulation (the server's player state of shell 0x67; 0 offline): 2, 3, 5, 6 block
    /// jump and crouch, 3 zeroes motion.
    // SPEC: MOVEMENT_2009.md §12.5 (shell id 0x67 -> controller+0x30)
    pub ctrl_state: u8,
    /// Movement allowed (state 2 disables the own character's movement).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (state 2: +0xa8 := 0; state 1: +0xa8 := 1)
    pub movement_allowed: bool,
    /// The server's sprint permission (true offline).
    // SPEC: MOVEMENT_2009.md §7 step 2; GAME_MESSAGES_2009_S2C.md §3.2; DC-DEFAULT: CLI-58 (proposed)
    pub sprint_allowed: bool,
    /// The scope is up (zoom level >= 1 or a zoom change running; set by the frame loop from the weapon,
    /// [`crate::zoom`]): it blocks the sprint speed and the run speed, both walk.
    // SPEC: MOVEMENT_2009.md §7 step 2 (controller +0xf0 < 1 and +0xf4 == 0 for the sprint speed), step 3 (RUN walks
    // when cshell+0x5c450, the same zoom test, is non-zero)
    pub sprint_blocked: bool,
    /// The world's container volumes (ladders); the touched ones go into every tick.
    // SPEC: MOVEMENT_2009.md §11.1; DC-DEFAULT: CLI-59 (proposed)
    pub volumes: ContainerVolumes,
    containers: Vec<Container>,
    /// A live world camera forbids movement: the body's velocity is zeroed (controller +0x80 = 0).
    // DISASM: cshell+0x1f8502 (+0x80 := the camera's movement byte), cshell+0x2001c0 (`+0x80 == 0`: velocity zero)
    // DC-DEFAULT: CLI-240 (proposed)
    pub cam_lock: bool,
    /// The speed rows of the user's ROM (held weapon, body, figure and gear `MovePenalty` / `DashSpeed`); `None`
    /// (no ROM): the neutral modifiers, every stance at its code speed.
    // SPEC: MOVEMENT_2009.md §3, §7 step 5; EVIDENCE: cshell+0x5c270, cshell+0x1fd390 (dc_sim::loadout)
    pub speed_rows: Option<std::sync::Arc<dc_sim::SpeedTable>>,
    /// What the own body holds and wears, as rows of [`LocalPlayer::speed_rows`].
    pub loadout: dc_sim::Loadout,
    /// The remote characters' bodies the mover collides with (rebuilt by [`LocalPlayer::follow`] every frame from
    /// the drawn remotes, at their drawn positions; bounded, no allocation).
    pub bodies: dc_sim::collide::BodySet,
    /// Whether remote bodies block the own body (DC-DEFAULT SIM-13, on; `DC_SOLID_BODIES=0` turns it off).
    pub solid_bodies: bool,
    /// The step-up smoothing of the eye, run once per tick on the tick's eye height ([`LocalPlayer::eye`]).
    pub eye_smooth: EyeSmoother,
    /// The smoothing's lag below the raw eye (<= 0) after the previous tick and after the last tick.
    eye_lag: (f32, f32),
    /// The stop of the own plant / defuse hold (CLI-1250): set at the hold start, ended by the client's own release,
    /// by the server's freeze 0, by the answer wait running out, or handed to the server at the round result.
    pub own_hold: Option<OwnHold>,
    /// The running Possession move (CLI-1642): the server's closing teleport ends it.
    pub glide: Option<Glide>,
    /// Mover ticks left of the shot slow of an infected own body (CLI-1760).
    pub shot_slow_ticks: u32,
}

/// One Possession move of the own body (CLI-1642).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    from: V3,
    to: V3,
    ticks: u32,
    /// Ticks of the move.
    total: u32,
    /// Ticks after which a lost closing teleport no longer holds the body.
    give_up: u32,
}

/// How long the body waits at the end point for the server's closing teleport.
const GLIDE_WAIT_MS: u64 = 1500;

/// The own hold's stop (CLI-1250).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OwnHold {
    /// The server's freeze 1 came while it ran (the server took the hold).
    pub confirmed: bool,
    /// Ticks run since the hold start.
    pub ticks: u32,
}

/// How long the own hold's stop waits for the server's freeze before it ends by itself (a refused hold gets no
/// answer, DSV-202).
// DC-DEFAULT: CLI-1250 (proposed) 1000 ms
pub const OWN_HOLD_WAIT_MS: u32 = 1000;

/// The shot slow of an infected own body (Quarantine): every damaging weapon or blast hit (0x74 `dmg_kind` not 0,
/// the fall kind) slows the body to this share of its speed for this long. The 2009 client has no such rule of its
/// own: the ROM damage-type rows carry a `SlowMovement` key and the client reads it into its damage-type table, but
/// nothing in CShell.dll calls that column's accessor; so the numbers are ours.
// OBSERVED: ROM table key DAMAGETYPES.TXT [<type>] SlowMovement (names only)
// DISASM: cshell+0x282ae4 (the row filler stores `SlowMovement` at row +0x20), cshell+0x282cb0 (its lookup; no call
// or pointer to it in the module, scan 2026-11-03)
// DC-DEFAULT: CLI-1760 (proposed, user playtest 2026-11-03 "infected are slowed when shot") 400 ms at 0.5
pub const SHOT_SLOW_MS: u32 = 400;
pub const SHOT_SLOW_FACTOR: f32 = 0.5;
/// The fall damage kind of 0x74 (no shot slow).
// OBSERVED: the three captured falls carried `dmg_kind` 0 (dc-dserver fall.rs `FALL_DAMAGE_KIND`; REFERENCE)
const FALL_DAMAGE_KIND: u8 = 0;

/// The default of [`LocalPlayer::solid_bodies`]: on, unless the environment variable `DC_SOLID_BODIES` is `0`,
/// `off` or `false`.
// DC-DEFAULT: SIM-13 (proposed) solid character bodies on, overridable
pub fn solid_bodies_default() -> bool {
    match std::env::var("DC_SOLID_BODIES") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false"
        ),
        Err(_) => true,
    }
}

/// Whether a drawn remote takes part in the object stage: its visible form (the hidden pre-spawn / dead form of a
/// character, flag word 0x0802, has no solid bit) and not the own character object.
// EVIDENCE: engine+0x70650 (an object is a candidate when its flag word has 0x2000 and not 0x8000); OBSERVED:
// CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.4 (characters 0x2802 before the spawn, 0x3803 spawned), the dead
// victim's character 0x0802 (dc-dserver modelobj, tdm6)
// DC-DEFAULT: SIM-16 (proposed) the client's visible flag stands for the solid bit (the hidden 0x2802 form
// is treated as not solid; RemoteBody carries no flag word)
pub fn remote_is_solid(r: &crate::world::RemoteBody, own_object: Option<u16>) -> bool {
    Some(r.obj) != own_object && r.look.as_ref().is_none_or(|l| l.visible)
}

fn lerp(a: V3, b: V3, t: f32) -> V3 {
    a + (b - a) * t
}

impl LocalPlayer {
    /// A standing body at `spawn`, placed on the floor under the point (lift by one step, sweep down at most
    /// 1000 units). PROPOSED default SIM-2 (staged, not approved): the 2009 client does not place itself, the
    /// server sends the position; until a server does, the client uses the same placement a DeCombat server
    /// would (`dc_sim::drop_to_floor`).
    // DC-DEFAULT: SIM-2 (proposed); SPEC: MOVEMENT_2009.md §12.4 flag 0x0020
    pub fn spawn<W: Collider + ?Sized>(
        params: MovementParams,
        world: &W,
        spawn: &SpawnPoint,
        stand_dims: V3,
        tick_hz: u32,
    ) -> LocalPlayer {
        let at = drop_to_floor(
            world,
            &Cylinder::from_dims(stand_dims),
            spawn.pos,
            params.step(),
            1000.0,
        );
        let state = PlayerState::new(at, stand_dims, &params);
        LocalPlayer {
            params,
            prev: state.clone(),
            state,
            clock: FixedStep::new(tick_hz),
            events: StepEvents::default(),
            jumps: 0,
            landings: 0,
            ctrl_state: 0,
            movement_allowed: true,
            sprint_allowed: true,
            sprint_blocked: false,
            volumes: ContainerVolumes::default(),
            containers: Vec::new(),
            cam_lock: false,
            speed_rows: None,
            loadout: dc_sim::Loadout::default(),
            bodies: dc_sim::collide::BodySet::default(),
            solid_bodies: solid_bodies_default(),
            eye_smooth: EyeSmoother::default(),
            eye_lag: (0.0, 0.0),
            glide: None,
            own_hold: None,
            shot_slow_ticks: 0,
        }
    }

    /// Rebuilds the body set from the drawn remotes: each solid remote's box at its drawn centre with the half
    /// extents the frame gives it (the server's box for that character).
    // SPEC: MOVEMENT_2009.md §9.2 (the box comes from the server, 0xa0 / 0x20), §9.6 stage 2 (other objects by
    // their box)
    pub fn set_bodies(&mut self, remotes: &[crate::world::RemoteBody], own_object: Option<u16>) {
        self.bodies.clear();
        for r in remotes.iter().filter(|r| remote_is_solid(r, own_object)) {
            self.bodies.push(dc_sim::collide::Body {
                id: r.obj as u32,
                centre: V3::from_array(r.pos),
                half: V3::from_array(r.half),
            });
        }
    }

    /// Reads the speed rows from the user's ROM (`None` or an unreadable table: the neutral modifiers) and starts
    /// with the default body row.
    // DC-DEFAULT: CLI-340 (proposed) the own body row is the row of the own look's body model file (the server's
    // look of the own object); until the look names one, body row 0 (the row both the reference stack and our dserver
    // default to, DSV-24 / CLI-105). Head gear, backpack, goggles and the override body are unknown to the
    // client (no loadout source read): their terms count 0.
    pub fn load_speed_rows(&mut self, rom: Option<&std::path::Path>) {
        let Some(rom) = rom else {
            return;
        };
        match dc_sim::SpeedTable::from_vfs(&dc_rez::RezVfs::new(rom)) {
            Ok(t) => {
                self.speed_rows = Some(std::sync::Arc::new(t));
                self.loadout.body = Some(0);
            }
            Err(e) => eprintln!("dc-client: speed rows not read ({e}): neutral speeds"),
        }
    }

    /// The speed modifiers of the own loadout now (neutral without the ROM rows).
    // SPEC: MOVEMENT_2009.md §7 step 2 (sprint terms), step 5 (penalty)
    pub fn speed_mods(&self) -> dc_sim::SpeedMods {
        match &self.speed_rows {
            Some(t) => t.mods(&self.loadout),
            None => dc_sim::SpeedMods::default(),
        }
    }

    /// [`LocalPlayer::speed_mods`] of one mover tick: while the shot slow runs (CLI-1760) the target speed
    /// `(1 - p) * s` is scaled by [`SHOT_SLOW_FACTOR`]; each tick uses one tick of it.
    fn tick_speed_mods(&mut self) -> dc_sim::SpeedMods {
        let mut m = self.speed_mods();
        if self.shot_slow_ticks > 0 {
            self.shot_slow_ticks -= 1;
            m.penalty = 1.0 - (1.0 - m.penalty.clamp(-1.0, 1.0)) * SHOT_SLOW_FACTOR;
        }
        m
    }

    /// The sprint gauge now as `(value, max)` (`None`: no gauge rule, e.g. without the ROM rows): the input of the
    /// HUD's stamina gauge.
    // EVIDENCE: cshell+0x95850 (the gauge element reads the value and the inventory's max)
    pub fn stamina_level(&self) -> Option<(f32, i32)> {
        let rates = self.speed_mods().stamina?;
        Some((self.state.stamina.level(Some(rates))?, rates.max))
    }

    /// The pair `[sprinting][stopped]` of the sprint notice 0xca for the current control word and gauge: `(1, 0)`
    /// sprinting, `(0, 0)` the sprint keys held on an empty gauge (the stamina end), `(0, 1)` otherwise.
    // EVIDENCE: cshell+0x1ff980 (cshell+0x1ff7f0 called with (1, 0) at a sprint start, (0, 0) beside gauge mode 2,
    // (0, 1) on the other paths); SPEC: GAME_MESSAGES_2009_C2S.md §2.3; OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md
    // (7 stamina ends (0, 0), each followed later by (0, 1))
    pub fn sprint_pair(&self) -> (bool, bool) {
        if self.state.controls & dc_sim::bits::SPRINT != 0 {
            (true, false)
        } else if self.state.stamina.mode == dc_sim::GaugeMode::Spent {
            (false, false)
        } else {
            (false, true)
        }
    }

    /// Follows the held weapon, the own look and the own equipment for the speed rows.
    fn follow_loadout(&mut self, fs: &FrameState) {
        // CLI-1211: the Quarantine override row (0xd3 sub 3) is the body override of the own loadout: the infected
        // rows' `MovePenalty` / `DashSpeed` / `MaxStamina` replace the body row's
        // EVIDENCE: cshell+0x2a1e0 (sub 3 -> own info +0x2ac), cshell+0x5c270 (penalty: cshell+0x1e2bc0(body,
        // info +0x2ac))
        for (_, e) in &fs.hud_events {
            match e {
                crate::hud::state::HudEvent::Qrt(crate::hud::qrt::QrtEvent::Override(row)) => {
                    self.loadout.body_override = u32::try_from(*row).ok();
                    if self.loadout.body_override.is_none() {
                        self.shot_slow_ticks = 0;
                    }
                }
                // CLI-1760: a damaging hit on the infected own body slows it
                crate::hud::state::HudEvent::Wound { damage, kind }
                    if *damage > 0
                        && *kind != FALL_DAMAGE_KIND
                        && self.loadout.body_override.is_some() =>
                {
                    let ticks = (SHOT_SLOW_MS as u64 * self.clock.hz() as u64).div_ceil(1000);
                    self.shot_slow_ticks = self.shot_slow_ticks.max(ticks as u32);
                }
                _ => {}
            }
        }
        if fs.held_weapon.is_some() {
            self.loadout.weapon = fs.held_weapon;
        }
        if !fs.equipment.is_empty() {
            let g = gear_rows(&fs.equipment);
            self.loadout.head = g.0;
            self.loadout.backpack = g.1;
            self.loadout.goggles = g.2;
        }
        if let (Some(t), Some(body)) = (
            &self.speed_rows,
            fs.own_look.as_ref().and_then(|l| l.models.first()),
        ) {
            if let Some(row) = t.body_row_of_file(&body.model) {
                self.loadout.body = Some(row);
            }
        }
    }

    /// Follows what the server set for the own body this frame: the player state (0x67), the step height
    /// (`S2C_EVAR`; when the server never sent it the code default stays) and the sprint permission (0xca).
    // SPEC: MOVEMENT_2009.md §2 (`STEP_HEIGHT`), §8.1 step 2, §7 step 2, §12.5
    // DC-DEFAULT: CLI-57 (proposed) a step height the server did not send keeps the code default 30 (the 2009
    // client reads 0 then, MOVEMENT_2009.md §2 correction)
    pub fn follow(&mut self, fs: &FrameState) {
        if let Some(ls) = fs.life_state {
            self.set_life_state(ls);
        }
        if let Some(h) = fs.step_height {
            self.params.step_height = h;
        }
        if let Some(a) = fs.sprint_allowed {
            self.sprint_allowed = a;
        }
        for m in &fs.mover {
            self.on_mover(m);
        }
        // DC-DEFAULT: CLI-240 (proposed) a live world camera with the movement byte 0 holds the body
        self.cam_lock = fs.cam_shot.is_some_and(|c| !c.allow_move);
        self.follow_loadout(fs);
        self.set_bodies(&fs.remotes, fs.own_object);
    }

    /// One freeze / release block of the server's mover update: freeze 1 holds the body where it is now (the
    /// position is re-applied every step), 0 ends the hold; release 1 suspends the hold, 0 resumes it.
    // SPEC: MOVEMENT_2009.md §12.4 (0x0080 freeze: mover+0x338 := bit, the current position stored at +0x33c and
    // re-applied every frame by cshell+0x1ff3f0; 0x0100 release: mover+0x348, the freeze is not applied)
    pub fn on_mover(&mut self, m: &crate::world::MoverNote) {
        if let Some(f) = m.freeze {
            // CLI-1250: the server's freeze 1 confirms a running own hold and keeps its stored point (storing the
            // position again would move the hold by up to one step); its freeze 0 ends any stop
            match (f, self.own_hold.as_mut()) {
                (true, Some(h)) if self.state.freeze.is_some() => h.confirmed = true,
                _ => {
                    self.state.set_freeze(f);
                    if !f {
                        self.own_hold = None;
                    }
                }
            }
        }
        if let Some(r) = m.release {
            self.state.set_freeze_release(r);
        }
        if let Some(h) = m.hold {
            self.on_own_hold(h);
        }
        if let Some((to, ms)) = m.glide {
            let hz = self.clock.hz() as u64;
            let total = (u64::from(ms) * hz).div_ceil(1000).max(1) as u32;
            self.glide = Some(Glide {
                from: self.state.pos,
                to: V3::from_array(to),
                ticks: 0,
                total,
                give_up: total + (GLIDE_WAIT_MS * hz / 1000) as u32,
            });
        }
    }

    /// The own plant / defuse hold (CLI-1250): its start stops the body at once with the server's freeze mechanism
    /// (the position stored and re-applied every step) instead of one round trip later when the server's freeze
    /// arrives; the client's own release ends that stop again. A body the server holds already is left to the server.
    /// At the round result the stop is handed to the server (its round-end stop keeps the body until the next spawn,
    /// DSV-77 / DSV-300), so a release after that no longer frees the body.
    // SPEC: MOVEMENT_2009.md §12.4 (the freeze: position stored, re-applied every frame); GAME_MODES_2009.md §4.5
    // EVIDENCE: cshell+0x1ff430 (flag 0x0080 stores the position and the bit), cshell+0x1ff3f0 (re-applied every
    // frame)
    // DC-DEFAULT: CLI-1250 (proposed, playtest report "I can still move my character a little bit during planting but
    // it rubberbands back")
    pub fn on_own_hold(&mut self, h: crate::world::HoldNote) {
        use crate::world::HoldNote;
        match h {
            HoldNote::Start => {
                if self.state.freeze.is_none() && self.own_hold.is_none() {
                    self.state.set_freeze(true);
                    self.own_hold = Some(OwnHold::default());
                }
            }
            HoldNote::End => {
                if self.own_hold.take().is_some() {
                    self.state.set_freeze(false);
                }
            }
            HoldNote::HandOver => self.own_hold = None,
        }
    }

    /// Follows the server's player state: before the first state (0) and while waiting (3) the body is held still,
    /// dead (2) it cannot move, alive (1) it moves.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (states 1 / 2 / 3); MOVEMENT_2009.md §12.5
    // DC-DEFAULT: CLI-9 (proposed) state 0 (no state yet) is simulated as state 3
    pub fn set_life_state(&mut self, state: u8) {
        // a body that turns alive starts with a full sprint gauge
        // DISASM: cshell+0x63d50 (gauge refill, called on shell 0x45, cshell+0x49a40)
        // DC-DEFAULT: SIM-21 (proposed) the refill runs when the server's player state turns 1 (alive)
        if state == 1 && self.ctrl_state != 1 {
            self.state.stamina.refill();
        }
        self.ctrl_state = if state == 0 { 3 } else { state };
        self.movement_allowed = state != 2;
        // CLI-1607: the jump-latch enable follows the player state (alive sets it, dead clears it), so a jump key
        // pressed during the death camera is not played as a hop on the respawn (user 2026-10-31)
        // EVIDENCE: cshell+0x1f97d0 (state 1: mover+0x84 := 1); DISASM: cshell+0x61870 (state 2: mover+0x84 := 0)
        match state {
            1 => self.state.jump_enable = true,
            2 => self.state.jump_enable = false,
            _ => {}
        }
    }

    pub fn tick_hz(&self) -> u32 {
        self.clock.hz()
    }

    /// Ticks run so far.
    pub fn ticks(&self) -> u64 {
        self.clock.ticks()
    }

    /// Runs one tick with `commands` and `view` (exactly what dc-sim's `Simulation::tick` does).
    pub fn tick<W: Collider + ?Sized>(
        &mut self,
        world: &W,
        commands: dc_sim::Commands,
        view: &ViewAngles,
    ) -> StepEvents {
        // DC-DEFAULT: CLI-1642 (proposed) a Possession move: the mover is suspended, the body eases along the straight
        // line, through the world; the view stays the player's; the server's teleport at the same point closes it
        if let Some(g) = self.glide.as_mut() {
            g.ticks += 1;
            if g.ticks <= g.give_up {
                let t = (g.ticks as f32 / g.total as f32).min(1.0);
                let eased = t * t * (3.0 - 2.0 * t);
                let pos = g.from + (g.to - g.from) * eased;
                self.prev = self.state.clone();
                self.state.pos = pos;
                self.state.vel = V3::new(0.0, 0.0, 0.0);
                self.state.acc = V3::new(0.0, 0.0, 0.0);
                self.eye_smooth.reset();
                self.eye_lag = (0.0, 0.0);
                return StepEvents::default();
            }
            self.glide = None;
        }
        // SPEC: MOVEMENT_2009.md §11.1 (the containers the body touches, refreshed before the law runs)
        self.volumes
            .touched(&self.state.shape(), self.state.pos, &mut self.containers);
        // CLI-1532: a held body (the plant / defuse hold, the round-end stop) takes no movement command and keeps no
        // ground speed, so it stands exactly at the freeze point instead of stepping up to one tick's travel away and
        // back every tick (the user saw the planter still move)
        // DC-DEFAULT: CLI-1532 (proposed, user 2026-10-30 "it's still possible to move while planting the bomb")
        let commands = if self.state.freeze.is_some() && !self.state.freeze_released {
            self.state.vel.x = 0.0;
            self.state.vel.z = 0.0;
            self.state.acc.x = 0.0;
            self.state.acc.z = 0.0;
            dc_sim::Commands {
                crouch: commands.crouch,
                ..dc_sim::Commands::default()
            }
        } else {
            commands
        };
        let mods = self.tick_speed_mods();
        let input = TickInput {
            commands,
            yaw: view.yaw,
            pitch: view.pitch,
            ctrl_state: self.ctrl_state,
            movement_allowed: self.movement_allowed,
            sprint_allowed: self.sprint_allowed,
            sprint_blocked: self.sprint_blocked,
            // SIM-23: the zoomed run walks too (the client never set it before)
            // SPEC: MOVEMENT_2009.md §7 step 3; DISASM cshell+0x1fd390 (RUN, not CROUCH, cshell+0x5c450 != 0 -> the
            // walk speed variable)
            run_blocked: self.sprint_blocked,
            containers: &self.containers,
            body_active: !self.cam_lock,
            mods,
            ..TickInput::default()
        };
        // CLI-1250: an own hold the server did not take within the wait ends by itself
        if let Some(h) = self.own_hold.as_mut() {
            h.ticks = h.ticks.saturating_add(1);
            let wait = (OWN_HOLD_WAIT_MS as u64 * self.clock.hz() as u64).div_ceil(1000) as u32;
            if !h.confirmed && h.ticks > wait {
                self.own_hold = None;
                self.state.set_freeze(false);
            }
        }
        self.prev = self.state.clone();
        let dt = self.clock.dt();
        // the object stage: the remote bodies after the world (MOVEMENT_2009 §9.6 stage 2)
        let ev = if self.solid_bodies && !self.bodies.is_empty() {
            let w = dc_sim::collide::WithBodies {
                world,
                bodies: &self.bodies,
            };
            self.state.step(&self.params, &w, &input, dt)
        } else {
            self.state.step(&self.params, world, &input, dt)
        };
        self.events.jumped |= ev.jumped;
        self.events.fall_started |= ev.fall_started;
        self.events.landed_from_fall |= ev.landed_from_fall;
        self.events.landed_from_jump |= ev.landed_from_jump;
        self.events.moved |= ev.moved;
        self.jumps += ev.jumped as u64;
        self.landings += (ev.landed_from_fall || ev.landed_from_jump) as u64;
        // the step-up smoothing on the tick's eye height, before the crouch dip (§13.4 order: base, smoothing,
        // bob, effects, crouch dip)
        // SPEC: MOVEMENT_2009.md §13.4 second bullet (one mover step per frame in 2009, §4)
        // DC-DEFAULT: CLI-650 (proposed) the smoothing runs per simulation tick and its lag is interpolated like
        // the body: run per rendered frame on the interpolated eye, a frame rate above about 1000 / s rises less
        // than the 1-unit threshold per frame and the step is never smoothed
        let raw =
            self.state.pos.y + crate::update::eye_offset_of(&self.state)[1] - self.state.eye_dip;
        let smoothed = self.eye_smooth.smooth(raw, !self.state.jumping, dt);
        self.eye_lag = (self.eye_lag.1, smoothed - raw);
        ev
    }

    /// Advances the fixed-step clock by `elapsed_us` of wall time and runs the due ticks, each with the
    /// commands `input` gives for it. `on_tick` sees every tick. Returns the number of ticks run.
    // SPEC: MOVEMENT_2009.md §4 (dc-sim FixedStep); DC-DEFAULT: SIM-1 (proposed, 60 Hz, at most 8 catch-up ticks)
    pub fn advance<W: Collider + ?Sized>(
        &mut self,
        world: &W,
        elapsed_us: u64,
        input: &mut dyn InputSource,
        view: &mut ViewAngles,
        on_tick: &mut dyn FnMut(&TickRecord),
    ) -> u32 {
        let first = self.clock.ticks();
        let n = self.clock.advance(elapsed_us);
        let hz = self.clock.hz();
        for i in 0..n as u64 {
            let tick = first + i;
            let commands = input.tick(tick, hz, view);
            let before = self.state.pos;
            let events = self.tick(world, commands, view);
            on_tick(&TickRecord {
                tick,
                before,
                after: self.state.pos,
                events,
            });
        }
        n
    }

    /// Applies a server correction (teleport and new box); interpolation restarts from the new state.
    // SPEC: MOVEMENT_2009.md §12.4 flag 0x0020 (cshell+0x1feb90)
    pub fn apply_correction<W: Collider + ?Sized>(&mut self, world: &W, c: &Correction) -> bool {
        self.glide = None;
        let fitted = self.state.apply_correction(&self.params, world, c);
        // a teleport while frozen moves the hold with it (else the next step would pull the body back)
        // DC-DEFAULT: CLI-89 (proposed) the freeze point follows a teleport (the 2009 teleport cshell+0x1feb90 is
        // not read for mover+0x33c; the reference stack's spawn set sends freeze 1 / 0 around its teleport, MOVEMENT_2009 §12.4)
        if self.state.freeze.is_some() {
            self.state.freeze = Some(self.state.pos);
        }
        self.prev = self.state.clone();
        self.eye_smooth.reset();
        self.eye_lag = (0.0, 0.0);
        fitted
    }

    /// The world models in `gone` left the collision set: a body standing on one loses its standing contact and
    /// falls on the next ticks without input (dc-sim [`PlayerState::detach_from_models`]).
    // DISASM: engine+0x70c70; DC-DEFAULT: CLI-1369 (proposed)
    pub fn detach_from_models(&mut self, gone: &[u32]) -> bool {
        !gone.is_empty() && self.state.detach_from_models(gone)
    }

    /// Fraction of the next tick already elapsed (0..1).
    pub fn alpha(&self) -> f32 {
        self.clock.alpha()
    }

    /// Body centre for rendering: interpolated between the last two states.
    pub fn render_pos(&self) -> V3 {
        lerp(self.prev.pos, self.state.pos, self.alpha())
    }

    /// Eye point for rendering, before step-up smoothing: the interpolated body centre, plus the eye height
    /// above the centre (UNPROVEN, [`todo::EYE_ABOVE_CENTRE_FRACTION`]), plus the crouch dip of the
    /// simulation (MOVEMENT_2009 §13.4, dc-sim `eye_dip`), both interpolated.
    // SPEC: MOVEMENT_2009.md §13.4 (target position + (fwd_off * dir.x, eye_h, fwd_off * dir.z); crouch dip
    // controller+0x108)
    pub fn eye_target(&self, view: &ViewAngles) -> V3 {
        let a = self.alpha();
        let pos = self.render_pos();
        let e0 = crate::update::eye_offset_of(&self.prev)[1];
        let e1 = crate::update::eye_offset_of(&self.state)[1];
        let (s, c) = view.yaw.sin_cos();
        pos + V3::new(
            todo::EYE_FORWARD_OFFSET * s,
            e0 + (e1 - e0) * a,
            todo::EYE_FORWARD_OFFSET * c,
        )
    }

    /// Eye point for rendering: [`LocalPlayer::eye_target`] with the step-up smoothing's lag (interpolated
    /// between the last two ticks like the body).
    // SPEC: MOVEMENT_2009.md §13.4 second bullet; DC-DEFAULT: CLI-650 (proposed)
    pub fn eye(&self, view: &ViewAngles) -> V3 {
        let (a0, a1) = self.eye_lag;
        let lag = a0 + (a1 - a0) * self.alpha();
        self.eye_target(view) + V3::new(0.0, lag, 0.0)
    }

    /// Short support-state label for the overlay.
    pub fn support_label(&self) -> String {
        let s = &self.state;
        let what = if s.on_ladder {
            "ladder"
        } else if s.on_ground {
            "ground"
        } else if s.jumping {
            "air (jump)"
        } else if s.falling {
            "air (fall)"
        } else if s.contact.is_some() {
            "steep contact"
        } else {
            "air"
        };
        match s.contact {
            Some(c) => format!("{what}, n.y {:.2}", c.normal.y),
            None => what.to_string(),
        }
    }
}

/// The step-up smoothing of the first-person eye: when the eye rises by more than 1 unit in one frame while
/// walking (a stair step), it follows at `max(diff / UP_TIME, UP_MIN_VEL)` units per second instead of jumping.
// SPEC: MOVEMENT_2009.md §13.1 (CAM_MOVE_UP_TIME 0.1, CAM_MOVE_UP_MIN_VEL 140.0, CAM_MOVE_MAX_YDIFF 30.0),
// §13.4 second bullet
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeSmoother {
    pub up_time: f32,
    pub up_min_vel: f32,
    pub max_ydiff: f32,
    last: Option<f32>,
}

impl Default for EyeSmoother {
    fn default() -> Self {
        EyeSmoother {
            up_time: 0.1,
            up_min_vel: 140.0,
            max_ydiff: 30.0,
            last: None,
        }
    }
}

impl EyeSmoother {
    /// Smoothed eye height for this frame (`dt` = frame time in seconds). `walking` = walking mode and not
    /// airborne after a jump.
    pub fn smooth(&mut self, target: f32, walking: bool, dt: f32) -> f32 {
        let y = match self.last {
            Some(mut last) if walking && target - last > 1.0 => {
                let mut diff = target - last;
                if diff > self.max_ydiff {
                    last += diff - self.max_ydiff;
                    diff = self.max_ydiff;
                }
                let vel = (diff / self.up_time).max(self.up_min_vel);
                let y = last + vel * dt;
                if y < target - 1.0 {
                    y
                } else {
                    target
                }
            }
            _ => target,
        };
        self.last = Some(y);
        y
    }

    /// Forget the history (teleport, respawn).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ScriptedInput;
    use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID};

    fn floor() -> CollisionWorld {
        let q = ConvexPoly::from_corners(
            vec![
                V3::new(-5000.0, 0.0, -5000.0),
                V3::new(-5000.0, 0.0, 5000.0),
                V3::new(5000.0, 0.0, 5000.0),
                V3::new(5000.0, 0.0, -5000.0),
            ],
            SURF_SOLID,
        );
        CollisionWorld::from_parts(vec![CollModel::from_polys(&[q])], &[])
    }

    const DIMS: V3 = V3::new(16.0, 40.0, 16.0); // made-up test box

    /// CLI-1642: a glide eases along the straight line through anything, without steering, stays at the end, and the
    /// closing teleport at the same point changes nothing.
    #[test]
    fn a_possession_glide_runs_in_a_line_and_the_teleport_closes_it() {
        let world = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &world, &sp, DIMS, 60);
        let from = p.state.pos;
        let to = from + V3::new(600.0, 200.0, 0.0);
        p.on_mover(&crate::world::MoverNote {
            glide: Some((to.to_array(), 500)),
            ..crate::world::MoverNote::default()
        });
        let cmd = dc_sim::Commands { forward: true, run: true, ..dc_sim::Commands::default() };
        let view = ViewAngles::default();
        let total = (500 * p.tick_hz() as usize).div_ceil(1000);
        let mut last = 0.0;
        for i in 0..total {
            p.tick(&world, cmd, &view);
            let d = p.state.pos - from;
            assert!((d.y - d.x / 3.0).abs() < 0.01, "on the line at {i}: {d:?}");
            assert!(d.x >= last, "forward only");
            last = d.x;
        }
        assert!((p.state.pos - to).dot(p.state.pos - to).sqrt() < 0.01, "{:?}", p.state.pos);
        p.tick(&world, cmd, &view);
        assert!((p.state.pos - to).dot(p.state.pos - to).sqrt() < 0.01, "it waits for the teleport");
        p.apply_correction(
            &world,
            &Correction { seq: 9, pos: to, dims: DIMS, velocity: None },
        );
        assert!(p.glide.is_none());
        assert!((p.state.pos - to).dot(p.state.pos - to).sqrt() < 1.0, "no jump: {:?}", p.state.pos);
    }

    #[test]
    fn spawn_lands_on_the_floor_and_interpolation_is_between_states() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        assert!((p.state.pos.y - 40.0).abs() < 0.5, "{:?}", p.state.pos);
        let mut input = ScriptedInput::new(ViewAngles::default());
        let mut view = ViewAngles::default();
        // 1.5 ticks of wall time: one tick runs, alpha = 0.5
        let n = p.advance(&w, 25_000, &mut input, &mut view, &mut |_| {});
        assert_eq!(n, 1);
        assert!((p.alpha() - 0.5).abs() < 1e-6);
        let r = p.render_pos();
        let lo = p.prev.pos.z.min(p.state.pos.z);
        let hi = p.prev.pos.z.max(p.state.pos.z);
        assert!(r.z >= lo && r.z <= hi);
        let eye = p.eye_target(&view);
        assert!((eye.y - r.y - todo::EYE_ABOVE_CENTRE_FRACTION * DIMS.y).abs() < 1e-3);
        // the 2009 stand box (half height 50) puts the eye 33 above the centre, as the 2009 client reports it
        assert_eq!(todo::EYE_ABOVE_CENTRE_FRACTION * 50.0, 33.0);
    }

    #[test]
    fn eye_smoother_follows_a_step() {
        let mut e = EyeSmoother::default();
        assert_eq!(e.smooth(100.0, true, 0.01), 100.0);
        // a 20-unit step: rises at max(20 / 0.1, 140) = 200 units/s
        let y = e.smooth(120.0, true, 0.01);
        assert!((y - 102.0).abs() < 1e-4, "{y}");
        let mut last = y;
        for _ in 0..20 {
            last = e.smooth(120.0, true, 0.01);
        }
        assert_eq!(last, 120.0);
        // airborne: no smoothing
        let mut f = EyeSmoother::default();
        f.smooth(0.0, false, 0.01);
        assert_eq!(f.smooth(50.0, false, 0.01), 50.0);
    }

    /// Playtest report (CLI-650): walking up a 16-unit step at a high frame rate (1000 frames/s, 1 ms each) lifts the
    /// eye over the smoothing time (16 / max(16 / 0.1, 140) = 0.1 s) instead of within one 60 Hz tick.
    #[test]
    fn a_step_up_raises_the_eye_over_the_smoothing_time_at_any_frame_rate() {
        let poly = |c: [V3; 4]| ConvexPoly::from_corners(c.to_vec(), SURF_SOLID);
        let floor = poly([
            V3::new(-5000.0, 0.0, -5000.0),
            V3::new(-5000.0, 0.0, 100.0),
            V3::new(5000.0, 0.0, 100.0),
            V3::new(5000.0, 0.0, -5000.0),
        ]);
        let riser = poly([
            V3::new(-5000.0, 0.0, 100.0),
            V3::new(-5000.0, 16.0, 100.0),
            V3::new(5000.0, 16.0, 100.0),
            V3::new(5000.0, 0.0, 100.0),
        ]);
        let tread = poly([
            V3::new(-5000.0, 16.0, 100.0),
            V3::new(-5000.0, 16.0, 5000.0),
            V3::new(5000.0, 16.0, 5000.0),
            V3::new(5000.0, 16.0, 100.0),
        ]);
        let w =
            CollisionWorld::from_parts(vec![CollModel::from_polys(&[floor, riser, tread])], &[]);
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        let mut input = ScriptedInput::new(ViewAngles::default());
        let mut view = ViewAngles::default();
        let base = p.eye(&view).y;
        let (mut started, mut done, mut last, mut max_rise) = (None, None, base, 0.0f32);
        for frame in 0..1500u32 {
            p.advance(&w, 1_000, &mut input, &mut view, &mut |_| {});
            let y = p.eye(&view).y;
            max_rise = max_rise.max(y - last);
            last = y;
            if started.is_none() && y > base + 0.01 {
                started = Some(frame);
            }
            if done.is_none() && y > base + 15.0 {
                done = Some(frame);
            }
        }
        assert!(p.state.pos.z > 200.0 && p.state.on_ground, "{:?}", p.state);
        assert!(
            (last - base - 16.0).abs() < 0.5,
            "on the step: {base} -> {last}"
        );
        let (s, d) = (started.unwrap(), done.unwrap());
        assert!(d - s >= 80, "the eye rose 15 units in {} ms", d - s);
        assert!(max_rise < 0.5, "largest rise in one 1 ms frame: {max_rise}");
    }

    #[test]
    fn a_server_freeze_holds_the_body_until_released_and_follows_a_teleport() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        let fwd = dc_sim::Commands {
            forward: true,
            run: true,
            ..dc_sim::Commands::default()
        };
        let view = ViewAngles::default();
        let start = p.state.pos;
        let frame = |freeze: Option<bool>, release: Option<bool>| crate::world::FrameState {
            mover: vec![crate::world::MoverNote {
                freeze,
                release,
                hold: None,
                glide: None,
            }],
            ..crate::world::FrameState::default()
        };
        p.follow(&frame(Some(true), None));
        for _ in 0..30 {
            p.tick(&w, fwd, &view);
        }
        // dc-sim re-places the body before each step's physics (MOVEMENT_2009 §1 order), so a running body stays
        // within one step's travel of the hold (run speed / 60 Hz) and does not get further away
        // CLI-1532: no movement command reaches a held body: it stands at the hold
        let held = (p.state.pos - start).len();
        assert!(held < 0.01, "frozen: {:?}", p.state.pos);
        for _ in 0..30 {
            p.tick(&w, fwd, &view);
        }
        assert!((p.state.pos - start).len() <= held + 0.01, "not drifting");
        // release suspends the hold
        p.follow(&frame(None, Some(true)));
        for _ in 0..30 {
            p.tick(&w, fwd, &view);
        }
        assert!(
            p.state.pos.z > start.z + 20.0,
            "released: {:?}",
            p.state.pos
        );
        // hold again, then a teleport while frozen: the hold moves with it
        p.follow(&frame(Some(true), Some(false)));
        let to = V3::new(300.0, 40.1, 300.0);
        p.apply_correction(
            &w,
            &Correction {
                seq: 2,
                pos: to,
                dims: DIMS,
                velocity: None,
            },
        );
        for _ in 0..30 {
            p.tick(&w, fwd, &view);
        }
        assert!(
            (p.state.pos - to).len() < 6.0,
            "held at the teleport: {:?}",
            p.state.pos
        );
        // freeze 0 ends it
        p.follow(&frame(Some(false), None));
        for _ in 0..30 {
            p.tick(&w, fwd, &view);
        }
        assert!(
            p.state.pos.z > to.z + 20.0,
            "moving again: {:?}",
            p.state.pos
        );
    }

    /// CLI-1532: while the plant hold runs, walking, running, sprinting and jumping move the body not at all (it
    /// stood up to one tick's travel away from the hold before); the release frees it again.
    #[test]
    fn a_planting_body_takes_no_movement_command() {
        use crate::world::{FrameState, HoldNote, MoverNote};
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let all = dc_sim::Commands {
            forward: true,
            left: true,
            run: true,
            sprint: true,
            jump: true,
            ..dc_sim::Commands::default()
        };
        let view = ViewAngles::default();
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        for _ in 0..120 {
            p.tick(&w, dc_sim::Commands::default(), &view);
        }
        // running (on the ground) when the plant starts
        let run = dc_sim::Commands {
            jump: false,
            ..all
        };
        for _ in 0..20 {
            p.tick(&w, run, &view);
        }
        assert!(p.state.on_ground);
        p.follow(&FrameState {
            mover: vec![MoverNote {
                hold: Some(HoldNote::Start),
                ..MoverNote::default()
            }],
            ..FrameState::default()
        });
        // the server takes the hold (its freeze 1)
        p.follow(&FrameState {
            mover: vec![MoverNote {
                freeze: Some(true),
                ..MoverNote::default()
            }],
            ..FrameState::default()
        });
        let start = p.state.pos;
        for i in 0..240 {
            p.tick(&w, all, &view);
            assert!(
                (p.state.pos - start).len() < 0.01,
                "tick {i}: {:?} from {start:?}",
                p.state.pos
            );
        }
        p.follow(&FrameState {
            mover: vec![MoverNote {
                hold: Some(HoldNote::End),
                ..MoverNote::default()
            }],
            ..FrameState::default()
        });
        for _ in 0..30 {
            p.tick(&w, all, &view);
        }
        assert!((p.state.pos - start).len() > 20.0, "free again: {:?}", p.state.pos);
    }

    /// CLI-1250: the own hold start stops the body before any server message; the server's freeze then confirms it
    /// and the own release frees the body at once; an unanswered hold ends by itself after the wait; the server's
    /// freeze 0 ends the own stop; after the hand-over at the round result the own release no longer frees the body.
    #[test]
    fn the_own_hold_start_holds_the_body_before_any_server_message() {
        use crate::world::{FrameState, HoldNote, MoverNote};
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let fwd = dc_sim::Commands {
            forward: true,
            run: true,
            ..dc_sim::Commands::default()
        };
        let view = ViewAngles::default();
        let note = |n: MoverNote| FrameState {
            mover: vec![n],
            ..FrameState::default()
        };
        let hold = |h: HoldNote| {
            note(MoverNote {
                hold: Some(h),
                ..MoverNote::default()
            })
        };
        let server = |f: bool| {
            note(MoverNote {
                freeze: Some(f),
                ..MoverNote::default()
            })
        };
        let run = |p: &mut LocalPlayer, n: usize| {
            for _ in 0..n {
                p.tick(&w, fwd, &view);
            }
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        run(&mut p, 5);
        // the hold starts: no server message yet, the body stays where it was
        p.follow(&hold(HoldNote::Start));
        let start = p.state.pos;
        run(&mut p, 20);
        // CLI-1532: exactly in place, not one step away and back
        assert!((p.state.pos - start).len() < 0.01, "held: {:?}", p.state.pos);
        // the server's freeze arrives (confirmed); still held past the answer wait
        p.follow(&server(true));
        run(&mut p, 90);
        assert!((p.state.pos - start).len() < 6.0, "still held: {:?}", p.state.pos);
        // the key is let go: free at once, before the server's release
        p.follow(&hold(HoldNote::End));
        run(&mut p, 30);
        assert!(p.state.pos.z > start.z + 20.0, "released: {:?}", p.state.pos);
        p.follow(&server(false));
        assert!(p.state.freeze.is_none() && p.own_hold.is_none());

        // a refused hold (no answer): free again after the wait
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        p.follow(&hold(HoldNote::Start));
        let start = p.state.pos;
        run(&mut p, 30);
        assert!((p.state.pos - start).len() < 6.0);
        run(&mut p, 60);
        assert!(p.own_hold.is_none(), "the wait ran out");
        assert!(p.state.pos.z > start.z + 20.0, "moving: {:?}", p.state.pos);

        // the server ends the hold (completed plant): its freeze 0 frees the body although the key is still down
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        p.follow(&hold(HoldNote::Start));
        p.follow(&server(true));
        let start = p.state.pos;
        run(&mut p, 10);
        p.follow(&server(false));
        run(&mut p, 30);
        assert!(p.state.pos.z > start.z + 20.0, "free: {:?}", p.state.pos);
        // a late own release changes nothing
        p.follow(&hold(HoldNote::End));
        assert!(p.state.freeze.is_none());

        // the round result hands the stop to the server: letting go no longer frees the body
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        p.follow(&hold(HoldNote::Start));
        p.follow(&server(true));
        p.follow(&hold(HoldNote::HandOver));
        p.follow(&hold(HoldNote::End));
        let start = p.state.pos;
        run(&mut p, 120);
        assert!((p.state.pos - start).len() < 6.0, "the round-end stop: {:?}", p.state.pos);
        // the spawn's release frees it
        p.follow(&server(false));
        run(&mut p, 30);
        assert!(p.state.pos.z > start.z + 20.0);
    }

    /// The round end as the 2009 client lives it (CLI-170): the result 0x46 changes no camera and locks no input
    /// for a living player; our server's freeze (DSV-77) holds the body, and the mouse still turns the view until
    /// the result screen (0x3f) takes the world away.
    // DISASM: cshell+0x4a608..0x4aa42 (0x46: camera calls only for the observer camera / the observer flag);
    // OBSERVED: capture 20261025_204341_realclient (the user's view turned in 70 of 82 updates
    // of the 6 s from 0x46 to 0x3f, the body held by the freeze)
    #[test]
    fn at_the_round_result_the_body_is_held_and_the_view_still_turns() {
        struct TurnAndRun;
        impl InputSource for TurnAndRun {
            fn frame(&mut self, _view: &mut ViewAngles) {}
            fn tick(&mut self, _tick: u64, _hz: u32, view: &mut ViewAngles) -> dc_sim::Commands {
                view.yaw += 0.01;
                dc_sim::Commands {
                    forward: true,
                    run: true,
                    ..dc_sim::Commands::default()
                }
            }
            fn label(&self) -> &'static str {
                "turn-and-run"
            }
        }
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        let start = p.state.pos;
        // the freeze that our server sends with the 0x46
        p.follow(&crate::world::FrameState {
            mover: vec![crate::world::MoverNote {
                freeze: Some(true),
                ..Default::default()
            }],
            ..crate::world::FrameState::default()
        });
        let mut view = ViewAngles::default();
        let mut input = TurnAndRun;
        let mut cam = crate::deathcam::DeathCam::default();
        for _ in 0..120 {
            p.advance(&w, 16_667, &mut input, &mut view, &mut |_| {});
            // alive: no observer camera, the own view is drawn
            assert!(cam
                .frame(
                    None,
                    &[],
                    p.state.pos,
                    p.state.pos,
                    &view,
                    Some(&w),
                    1.0 / 60.0
                )
                .is_none());
        }
        assert!((p.state.pos - start).len() < 6.0, "held: {:?}", p.state.pos);
        assert!(view.yaw > 1.0, "the view still turns: {view:?}");
        assert_eq!(cam.mode(), None);
    }

    /// A speed table with one body row and a figure row with a gauge (made-up values, not ROM rows).
    fn gauge_rows() -> std::sync::Arc<dc_sim::SpeedTable> {
        std::sync::Arc::new(dc_sim::SpeedTable {
            bodies: vec![dc_sim::BodySpeed {
                figure: Some(0),
                ..Default::default()
            }],
            override_base: 1,
            figures: vec![dc_sim::FigureSpeed {
                dash_mul: 1.4,
                max_stamina: Some(60),
                dash_gauge: Some(30),
                gauge_rec: Some(20),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    fn sprint_keys() -> dc_sim::Commands {
        dc_sim::Commands {
            forward: true,
            run: true,
            sprint: true,
            ..dc_sim::Commands::default()
        }
    }

    // EVIDENCE: cshell+0x95850, cshell+0x1ff980 (gauge and the 0xca pair)
    #[test]
    fn the_gauge_ends_the_sprint_reports_the_stamina_end_and_refills_on_spawn() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.speed_rows = Some(gauge_rows());
        p.loadout.body = Some(0);
        p.set_life_state(1);
        let view = ViewAngles::default();
        assert_eq!(p.stamina_level(), Some((60.0, 60)));
        p.tick(&w, sprint_keys(), &view);
        assert_eq!(p.sprint_pair(), (true, false));
        // 60 / 30 per second: about 2 s of sprint, then the stamina end while the keys are held
        let mut sprint_ticks = 1;
        for _ in 0..180 {
            p.tick(&w, sprint_keys(), &view);
            sprint_ticks += p.sprint_pair().0 as u32;
        }
        assert!((118..=120).contains(&sprint_ticks), "{sprint_ticks}");
        assert_eq!(p.sprint_pair(), (false, false), "stamina end");
        assert_eq!(p.stamina_level(), Some((0.0, 60)));
        // let go: (0, 1) and the gauge refills by time
        p.tick(&w, dc_sim::Commands::default(), &view);
        assert_eq!(p.sprint_pair(), (false, true));
        // dead, then alive again: full
        p.set_life_state(2);
        p.set_life_state(1);
        assert_eq!(p.stamina_level(), Some((60.0, 60)));
    }

    /// CLI-1607 (user 2026-10-31 "sometimes when the player respawns it makes them jump once"): a jump key pressed
    /// while dead (the corpse stands on the ground, so the sampling sets JUMP) latched a jump that the first alive
    /// step played after the spawn teleport. The latch enable follows the player state: dead latches nothing, and a
    /// key held through the spawn needs a release and a new press.
    // EVIDENCE: cshell+0x1ff980 (latch gate mover+0x84), cshell+0x1f97d0 (state 1 sets it); DISASM: cshell+0x61870
    #[test]
    fn a_jump_pressed_while_dead_does_not_jump_at_the_respawn() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let jump = dc_sim::Commands {
            jump: true,
            ..dc_sim::Commands::default()
        };
        let idle = dc_sim::Commands::default();
        let view = ViewAngles::default();
        let respawn = |p: &mut LocalPlayer, seq: u8| {
            p.apply_correction(
                &w,
                &Correction {
                    seq,
                    pos: V3::new(300.0, DIMS.y / 2.0, 300.0),
                    dims: DIMS,
                    velocity: None,
                },
            );
            p.set_life_state(1);
        };
        // case 1: tapped while dead, released before the spawn
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        for _ in 0..30 {
            p.tick(&w, idle, &view);
        }
        assert!(p.state.on_ground);
        p.set_life_state(2);
        p.tick(&w, idle, &view);
        p.tick(&w, jump, &view);
        p.tick(&w, idle, &view);
        assert!(!p.state.jump_latch, "a dead body latches no jump");
        respawn(&mut p, 2);
        for _ in 0..30 {
            p.tick(&w, idle, &view);
        }
        assert_eq!(p.jumps, 0, "no hop at the respawn");
        // case 2: held from the death through the spawn: nothing until a release and a new press
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.set_life_state(1);
        for _ in 0..30 {
            p.tick(&w, idle, &view);
        }
        p.set_life_state(2);
        for _ in 0..10 {
            p.tick(&w, jump, &view);
        }
        respawn(&mut p, 3);
        for _ in 0..30 {
            p.tick(&w, jump, &view);
        }
        assert_eq!(p.jumps, 0, "a key held through the spawn does not jump");
        p.tick(&w, idle, &view);
        p.tick(&w, jump, &view);
        assert_eq!(p.jumps, 1, "a new press after the spawn jumps");
    }

    /// CLI-1211 (playtest report "the infected are supposed to have a faster default speed and more stamina"): the
    /// Quarantine 0xd3 sub 3 row becomes the own body override; its negative move penalty speeds the body and its
    /// `MaxStamina` replaces the body's; row -1 gives the body row back. Before, the override was never set.
    #[test]
    fn the_quarantine_override_row_drives_the_own_speed() {
        use crate::hud::qrt::QrtEvent;
        use crate::hud::state::HudEvent;
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        let body = |penalty: f32, max_stamina: f32| dc_sim::loadout::BodySpeed {
            penalty,
            max_stamina,
            ..Default::default()
        };
        let mut t = dc_sim::SpeedTable::default();
        t.bodies = vec![body(0.1, 0.0), body(-0.18, 0.0), body(-0.2, 2.0)];
        t.override_base = 1;
        p.speed_rows = Some(std::sync::Arc::new(t));
        p.loadout.body = Some(0);
        let ev = |row: i8| FrameState {
            hud_events: vec![(0, HudEvent::Qrt(QrtEvent::Override(row)))],
            ..FrameState::default()
        };
        assert!((p.speed_mods().penalty - 0.1).abs() < 1e-6);
        p.follow(&ev(1));
        assert_eq!(p.loadout.body_override, Some(1));
        assert!((p.speed_mods().penalty + 0.2).abs() < 1e-6);
        assert_eq!(p.speed_rows.as_ref().unwrap().body_stamina(&p.loadout), 2.0);
        p.follow(&ev(0));
        assert!((p.speed_mods().penalty + 0.18).abs() < 1e-6);
        p.follow(&ev(-1));
        assert_eq!(p.loadout.body_override, None);
        assert!((p.speed_mods().penalty - 0.1).abs() < 1e-6);
    }

    /// CLI-1760 (user playtest 2026-11-03 "infected are slowed when shot"): a damaging hit on the infected own body
    /// halves its target speed for 400 ms; a human body and a fall are not slowed.
    #[test]
    fn an_infected_own_body_is_slowed_when_shot() {
        use crate::hud::qrt::QrtEvent;
        use crate::hud::state::HudEvent;
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        let mut t = dc_sim::SpeedTable::default();
        t.bodies = vec![
            dc_sim::loadout::BodySpeed::default(),
            dc_sim::loadout::BodySpeed {
                penalty: -0.2,
                ..Default::default()
            },
        ];
        t.override_base = 1;
        p.speed_rows = Some(std::sync::Arc::new(t));
        p.loadout.body = Some(0);
        let ev = |e: HudEvent| FrameState {
            hud_events: vec![(0, e)],
            ..FrameState::default()
        };
        let shot = HudEvent::Wound { damage: 20, kind: 3 };
        // a human is not slowed
        p.follow(&ev(shot.clone()));
        assert_eq!(p.shot_slow_ticks, 0);
        // infected: 400 ms at 60 Hz = 24 ticks of half speed
        p.follow(&ev(HudEvent::Qrt(QrtEvent::Override(0))));
        p.follow(&ev(HudEvent::Wound { damage: 30, kind: FALL_DAMAGE_KIND }));
        assert_eq!(p.shot_slow_ticks, 0, "a fall does not slow");
        p.follow(&ev(shot));
        assert_eq!(p.shot_slow_ticks, 24);
        let slow = p.tick_speed_mods().penalty;
        assert!((1.0 - slow - 1.2 * SHOT_SLOW_FACTOR).abs() < 1e-6, "{slow}");
        for _ in 0..23 {
            p.tick_speed_mods();
        }
        assert!((p.tick_speed_mods().penalty + 0.2).abs() < 1e-6, "slow over");
        // human again: no slow left
        p.shot_slow_ticks = 5;
        p.follow(&ev(HudEvent::Qrt(QrtEvent::Override(-1))));
        assert_eq!(p.shot_slow_ticks, 0);
    }

    /// CLI-1137 (playtest report "not sure if gear worn is properly changing speed"): the equipped head item, backpack
    /// and goggles pick their speed rows: a backpack row's move penalty slows the body, a head row's negative one
    /// speeds it; the empty slots count their 2009 start values (head row 0, no backpack, no goggles).
    #[test]
    fn the_equipped_gear_picks_its_speed_rows() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        let gear = |penalty: f32, dash: f32| dc_sim::loadout::GearSpeed {
            penalty,
            dash,
            ..Default::default()
        };
        let mut t = dc_sim::SpeedTable::default();
        t.heads = vec![gear(0.0, 0.0), gear(-0.05, 0.0)];
        t.backpacks = vec![gear(0.0, 0.0), gear(0.0, 0.0), gear(0.03, 0.0)];
        t.goggles = vec![gear(0.0, 0.2)];
        p.speed_rows = Some(std::sync::Arc::new(t));
        let base = p.speed_mods();
        let mut eq = vec![-1i32; 15];
        eq[9] = 7002; // backpack row 2
        p.follow(&FrameState {
            equipment: eq.clone(),
            ..FrameState::default()
        });
        assert_eq!(
            (p.loadout.head, p.loadout.backpack, p.loadout.goggles),
            (Some(0), Some(2), None)
        );
        assert!((p.speed_mods().penalty - (base.penalty + 0.03)).abs() < 1e-6);
        eq[4] = 4001; // head row 1
        eq[6] = 5000; // goggle row 0
        p.follow(&FrameState {
            equipment: eq,
            ..FrameState::default()
        });
        let m = p.speed_mods();
        assert!(
            (m.penalty - (base.penalty + 0.03 - 0.05)).abs() < 1e-6,
            "{}",
            m.penalty
        );
        assert!(
            (m.sprint_add - (base.sprint_add + 0.2)).abs() < 1e-6,
            "{}",
            m.sprint_add
        );
        assert_eq!(gear_rows(&[]), (Some(0), None, None));
    }

    /// CLI-589 (playtest report "the sprint bar stays full"): with the ROM's rows the gauge the HUD reads has a
    /// value, and sprinting lowers it. ROM-gated; no ROM value asserted.
    // EVIDENCE: cshell+0x95850 (the HUD gauge reads the sprint gauge)
    #[test]
    fn rom_rows_give_the_hud_a_draining_gauge() {
        let Some(rom) = dc_rom::test_rom() else {
            return;
        };
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        p.load_speed_rows(Some(rom.as_path()));
        p.set_life_state(1);
        let (full, max) = p.stamina_level().expect("a gauge with the ROM rows");
        assert!(max > 0 && full > 0.0);
        let view = ViewAngles::default();
        for _ in 0..60 {
            p.tick(&w, sprint_keys(), &view);
        }
        let (after, _) = p.stamina_level().unwrap();
        assert!(
            after < full,
            "sprinting drains the gauge: {full} -> {after}"
        );
    }

    // SPEC: MOVEMENT_2009.md §7 step 2 (zoomed or a zoom change running: a sprint walks)
    #[test]
    fn a_zoomed_sprint_walks() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        let view = ViewAngles::default();
        p.sprint_blocked = true;
        for _ in 0..120 {
            p.tick(&w, sprint_keys(), &view);
        }
        // the cap is the walk speed; the 60 Hz step adds at most one step of acceleration above it (m21)
        let walk = p.params.walk_speed + p.params.max_accel / 60.0;
        assert!(p.state.vel.len_xz() <= walk, "{:?}", p.state.vel);
        p.sprint_blocked = false;
        for _ in 0..120 {
            p.tick(&w, sprint_keys(), &view);
        }
        assert!(
            p.state.vel.len_xz() > p.params.run_forward,
            "{:?}",
            p.state.vel
        );
    }

    // SIM-23 (playtest report: "walking speed scoped in feels fast"): zoomed, the run key walks at the walk speed times
    // the held weapon's (1 - p); the L96 (27) unzoomed runs at its run cap
    #[test]
    fn rom_a_zoomed_sniper_runs_at_the_walk_speed() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let run_keys = dc_sim::Commands {
            forward: true,
            run: true,
            ..dc_sim::Commands::default()
        };
        let view = ViewAngles::default();
        let speed = |zoomed: bool| {
            let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
            p.load_speed_rows(Some(&rom));
            p.loadout.weapon = Some(27);
            p.sprint_blocked = zoomed;
            for _ in 0..180 {
                p.tick(&w, run_keys, &view);
            }
            let pen = p.speed_mods().penalty.clamp(-1.0, 1.0);
            (
                p.state.vel.len_xz(),
                pen,
                p.params.walk_speed,
                p.params.run_forward,
            )
        };
        let (zoomed, pen, walk, run) = speed(true);
        let (open, _, _, _) = speed(false);
        println!(
            "L96 p {pen}: zoomed {zoomed} (walk cap {}), unzoomed {open} (run cap {})",
            walk * (1.0 - pen),
            run * (1.0 - pen)
        );
        assert!(pen > 0.0, "the L96 has a move penalty");
        assert!(
            (zoomed / (walk * (1.0 - pen)) - 1.0).abs() < 0.005,
            "zoomed {zoomed}"
        );
        assert!(
            (open / (run * (1.0 - pen)) - 1.0).abs() < 0.005,
            "unzoomed {open}"
        );
    }

    fn remote(obj: u16, pos: [f32; 3], visible: Option<bool>) -> crate::world::RemoteBody {
        crate::world::RemoteBody {
            obj,
            pid: 2,
            name: "R".into(),
            team: 2,
            pos,
            half: [DIMS.x, DIMS.y, DIMS.z],
            yaw: None,
            look: visible.map(|v| crate::world::RemoteLook {
                styles: Vec::new(),
                models: Vec::new(),
                weapon: None,
                trackers: Vec::new(),
                visible: v,
                aim: None,
            }),
        }
    }

    /// The own body runs at a standing remote of the frame state: it stops at the remote's box (grown by the own
    /// radius, kept 0.1 away) and then, turned 25 degrees, slides along the face and past it. With the switch off,
    /// the same run goes through.
    // SPEC: MOVEMENT_2009.md §9.6 stage 2; DC-DEFAULT: SIM-13 (proposed)
    #[test]
    fn a_standing_remote_stops_the_own_body_and_it_slides_along() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let run = |solid: bool| {
            let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
            p.solid_bodies = solid;
            p.set_life_state(1);
            p.follow(&crate::world::FrameState {
                remotes: vec![remote(77, [10.0, 40.0, 200.0], Some(true))],
                ..crate::world::FrameState::default()
            });
            assert_eq!(p.bodies.len(), 1);
            let fwd = dc_sim::Commands {
                forward: true,
                run: true,
                ..dc_sim::Commands::default()
            };
            let mut max_z = f32::MIN;
            for _ in 0..90 {
                p.tick(&w, fwd, &ViewAngles::default());
                max_z = max_z.max(p.state.pos.z);
            }
            let stopped = p.state.pos;
            let turned = ViewAngles {
                yaw: 25.0f32.to_radians(),
                pitch: 0.0,
            };
            for _ in 0..120 {
                p.tick(&w, fwd, &turned);
                let inside = (p.state.pos.x - 10.0).abs() < DIMS.x
                    && (p.state.pos.z - 200.0).abs() < DIMS.z + DIMS.x;
                assert!(!(solid && inside), "inside the box: {:?}", p.state.pos);
            }
            (max_z, stopped, p.state.pos)
        };
        let face = 200.0 - DIMS.z - DIMS.x;
        let (max_z, stopped, end) = run(true);
        assert!(max_z < face, "never past the face: {max_z}");
        assert!((stopped.z - (face - 0.1)).abs() < 0.05, "{stopped:?}");
        assert!(stopped.x.abs() < 1e-3, "{stopped:?}");
        assert!(end.x > 10.0 + 2.0 * DIMS.x, "slid along +x: {end:?}");
        assert!(end.z > 200.0 + DIMS.z, "and past it: {end:?}");
        let (max_z, _, _) = run(false);
        assert!(max_z > 300.0, "switch off: through the remote ({max_z})");
    }

    #[test]
    fn hidden_remotes_and_the_own_object_are_not_bodies() {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &w, &sp, DIMS, 60);
        let fs = crate::world::FrameState {
            remotes: vec![
                remote(1, [0.0, 40.0, 100.0], Some(false)),
                remote(2, [0.0, 40.0, 200.0], None),
                remote(3, [0.0, 40.0, 300.0], Some(true)),
                remote(4, [0.0, 40.0, 400.0], Some(true)),
            ],
            own_object: Some(4),
            ..crate::world::FrameState::default()
        };
        p.follow(&fs);
        let ids: Vec<u32> = p.bodies.as_slice().iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![2, 3]);
        // the next frame without remotes empties the set (rebuilt, not accumulated)
        p.follow(&crate::world::FrameState::default());
        assert!(p.bodies.is_empty());
    }
}
