//! The player body and its per-step update, in the order of the 2009 mover's frame function: sample the
//! commands into the control word, run the velocity law (or the frozen branch), enforce a freeze, then the
//! physics step (integrate and move with collision).
// SPEC: MOVEMENT_2009.md §1 (mover_frame cshell+0x201990 step order), §5, §6, §7, §8.1, §10.2, §11, §12.4

use crate::collide::Collider;
use crate::fmath::abs;
use crate::integrate::{integrate, Branch, IntegrateEnv};
use crate::motion::{sweep_move, Contact};
use crate::params::{MovementParams, SpeedMods};
use crate::shape::Cylinder;
use crate::stamina::{GaugeMode, Stamina};
use crate::todo;
use crate::vec3::{Quat, V3};

/// Control-word bits (the mover's command word).
// SPEC: MOVEMENT_2009.md §5.1 (bits, set in cshell+0x1ff980 / cshell+0x2013f0)
pub mod bits {
    pub const FWD: u32 = 0x0000_0001;
    pub const BACK: u32 = 0x0000_0002;
    pub const TURN_A: u32 = 0x0000_0004;
    pub const TURN_B: u32 = 0x0000_0008;
    pub const JUMP: u32 = 0x0000_0010;
    pub const CROUCH: u32 = 0x0000_0020;
    pub const STRAFE_MOD: u32 = 0x0000_0040;
    pub const LEFT: u32 = 0x0000_0080;
    pub const RIGHT: u32 = 0x0000_0100;
    pub const RUN: u32 = 0x0000_0200;
    pub const ANY_MOVE: u32 = 0x0000_0800;
    pub const SPRINT: u32 = 0x0040_0000;
    /// Bits that make `ANY_MOVE`: FWD, BACK, TURN_A, TURN_B, JUMP, LEFT, RIGHT.
    // SPEC: MOVEMENT_2009.md §5.1 (mask 0x19f)
    pub const ANY_MOVE_MASK: u32 = 0x19f;
    /// Removed while the crouch eye offset is non-zero (mask 0xffbfffef clears JUMP and SPRINT).
    // SPEC: MOVEMENT_2009.md §5.3
    pub const EYE_DIP_MASK: u32 = 0xffbf_ffef;
}

/// Commands held this step (the engine's numbered input commands, already mapped from keys or analog axes
/// by the caller).
// SPEC: MOVEMENT_2009.md §5.1 (commands 0, 1, 5/0x5c, 6, 7, 9, 10, 11, 12, 0x77), §5.2 (analog mapping)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Commands {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub crouch: bool,
    pub run: bool,
    /// Run-lock toggle: flips the lock on every step it is set (the client toggles on every frame the
    /// command reads as on), so pass it for one step per press.
    pub run_lock_toggle: bool,
    pub sprint: bool,
    pub strafe_mod: bool,
}

/// Movement mode.
// SPEC: MOVEMENT_2009.md §6.1 (fly = mode 1, 2, 4, 11; spectating = controller+0xac)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoveMode {
    #[default]
    Walk,
    /// Controller modes 1, 2, 4, 11: 3D acceleration along the view, jump / crouch move up / down.
    Fly,
    /// Free-fly spectator.
    Spectate,
}

/// A container (volume) the body is in this step: ladder, liquid, gravity volume, current.
// SPEC: MOVEMENT_2009.md §11.1 (entry fields), §12.4 flag 0x0004 (wire block)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Container {
    pub kind: u8,
    /// Current, units/s (accumulated as `dt * current` into the velocity).
    pub current: V3,
    /// Gravity of a gravity volume (kind 19); summed into the override.
    pub gravity: f32,
    pub viscosity: f32,
    pub hidden: bool,
    /// Friction factor (record +0x88); the body's `k` is `friction_base * min(1, min factor)`.
    pub friction: f32,
}

impl Container {
    /// Ladder container kind.
    // SPEC: MOVEMENT_2009.md §11.2 (kind 11 = ladder)
    pub const LADDER: u8 = 11;
    /// Gravity-volume container kind.
    // SPEC: MOVEMENT_2009.md §11.2 (kind 19 = 0x13 sets the gravity override)
    pub const GRAVITY: u8 = 19;

    pub fn ladder() -> Container {
        Container {
            kind: Container::LADDER,
            current: V3::ZERO,
            gravity: 0.0,
            viscosity: 0.0,
            hidden: false,
            friction: 1.0,
        }
    }
}

/// Knockback after damage: the last damage direction and the time since the knockback window started.
// SPEC: MOVEMENT_2009.md §6.9 (d = controller+0x1ec..+0x1f4 from shell id 0x74; mag = (1 - (now - start)) k^2)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knockback {
    pub dir: V3,
    pub elapsed: f32,
}

/// Everything the step needs besides the state, the parameters and the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickInput<'a> {
    pub commands: Commands,
    /// View angles in radians (yaw about +Y, pitch about +X; positive pitch looks down).
    // SPEC: MOVEMENT_2009.md §13.2 (controller+0x48 pitch, +0x4c yaw)
    pub yaw: f32,
    pub pitch: f32,
    pub mode: MoveMode,
    /// Controller state from shell id 0x67: 2, 3, 5, 6 block jump and crouch; 3 also zeroes motion.
    // SPEC: MOVEMENT_2009.md §12.5 (shell id 0x67)
    pub ctrl_state: u8,
    /// mover+0xa8 (movement allowed) and the shell flag; when false the frozen branch runs.
    // SPEC: MOVEMENT_2009.md §6 (gate in cshell+0x201470)
    pub movement_allowed: bool,
    /// controller+0x80; when false velocity and acceleration are zeroed at the end of the law.
    // SPEC: MOVEMENT_2009.md §6.10
    pub body_active: bool,
    /// The loadout's modifiers, including the sprint gauge rates (`mods.stamina`; `None`: no gauge rule).
    pub mods: SpeedMods,
    /// An outside veto on starting a sprint (a server or mod rule), on top of the own gauge
    /// ([`PlayerState::stamina`]); true = no veto. A vetoed sprint does not sprint and refills the gauge.
    // SPEC: MOVEMENT_2009.md §5.5 (stamina above 0); EVIDENCE: cshell+0x1ff980 (the gauge test; the mover veto
    // goes to gauge mode 0)
    pub stamina: bool,
    /// Sprint blocked (zoom level >= 1 or the second counter): a blocked sprint walks. The caller sets it from the
    /// zoom level (controller+0xf0) of the held weapon.
    // SPEC: MOVEMENT_2009.md §7 step 2 (controller+0xf0 < 1 and +0xf4 == 0)
    pub sprint_blocked: bool,
    /// The server's sprint permission (own player record +0x1f0, written by shell 0xca `[allowed]`): without it
    /// the sprint test of §7 step 2 is false and a sprinting body runs at run speed although the control word
    /// carries `SPRINT`. True offline (no server to ask).
    // SPEC: MOVEMENT_2009.md §7 step 2 (sprint test = SPRINT and the record byte +0x1f0); GAME_MESSAGES_2009_S2C.md
    // §3.2 (0xca `[allowed]`, PROVEN-CAPTURE by the client's reaction, tdm1 / tdm2)
    // DC-DEFAULT: CLI-58 (proposed) true when no server is connected
    pub sprint_allowed: bool,
    /// Run blocked (cshell+0x5c450 non-zero): running walks.
    // SPEC: MOVEMENT_2009.md §7 step 3
    pub run_blocked: bool,
    pub containers: &'a [Container],
    pub knockback: Option<Knockback>,
}

impl Default for TickInput<'_> {
    fn default() -> Self {
        TickInput {
            commands: Commands::default(),
            yaw: 0.0,
            pitch: 0.0,
            mode: MoveMode::Walk,
            ctrl_state: 0,
            movement_allowed: true,
            body_active: true,
            mods: SpeedMods::default(),
            stamina: true,
            sprint_blocked: false,
            sprint_allowed: true,
            run_blocked: false,
            containers: &[],
            knockback: None,
        }
    }
}

/// Movement events of one step; they map to the reliable notice `[0x76][1][e]`.
// SPEC: MOVEMENT_2009.md §12.3 (e = 1 jump take-off, 2 started falling, 3 landed)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StepEvents {
    pub jumped: bool,
    pub fall_started: bool,
    /// Landed after a fall without a jump.
    pub landed_from_fall: bool,
    /// Landed after a jump (the jump notification reports 3).
    pub landed_from_jump: bool,
    /// The physics step moved the body (displacement above the threshold).
    pub moved: bool,
}

impl StepEvents {
    /// The `e` codes of the notices this step sends, in send order.
    pub fn notice_codes(&self) -> impl Iterator<Item = u8> {
        [
            (self.landed_from_fall, 3u8),
            (self.landed_from_jump, 3),
            (self.fall_started, 2),
            (self.jumped, 1),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, e)| e)
    }
}

/// Server correction: shell id 0xa0 with flag 0x0020 (teleport and new box).
// SPEC: MOVEMENT_2009.md §12.4 (flag 0x0020: [seq:u8][pos:3 x f32][dims:3 x f32], cshell+0x1feb90)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correction {
    pub seq: u8,
    pub pos: V3,
    pub dims: V3,
    /// The saved velocity the client restores when its flag mover+0x38 is set; `None` zeroes the velocity
    /// and resets the fall height.
    pub velocity: Option<V3>,
}

/// The simulated player body.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub pos: V3,
    pub vel: V3,
    /// Stored acceleration (the engine writes the integrator's acceleration back, so it can include gravity).
    pub acc: V3,
    /// Current half extents (collision box).
    pub dims: V3,
    /// Standing half extents (from the player model's stand animation, MOVEMENT_2009.md §9.2).
    pub stand_dims: V3,
    /// Crouched half extents (the user dims of the player model's crouch clips); `None` keeps the standing box
    /// while crouched (SIM-6). With a crouch box the body shrinks when `CROUCH` is set and grows back when it
    /// clears, the feet kept in place.
    // OBSERVED: ROM player models (every crouch clip carries a lower box than the stand clips); CAPTURE tdm1
    // analysis/updates.csv t 257.3 / 258.4 (the 2009 body centre drops 5.0 when crouching and rises 5.0 when
    // standing, on the ground throughout)
    // EVIDENCE: engine+0x25b20 (set_box(dims x object scale, must_fit 0) from a per-object record, PLAUSIBLY the
    // dims of the playing animation)
    // DC-DEFAULT: SIM-25 (proposed) crouch box from the crouch clip, feet kept on the floor
    pub crouch_dims: Option<V3>,
    pub contact: Option<Contact>,
    pub gravity_on: bool,
    /// Friction coefficient `k`.
    pub friction: f32,
    pub controls: u32,
    pub prev_controls: u32,
    /// mover+0x60
    pub on_ground: bool,
    /// In the air since a take-off, until the next real ground contact. Not a 2009 field: for presentation and
    /// for callers that want the whole flight (the fire cone's airborne input, the eye's step-up smoothing). The
    /// movement law uses [`PlayerState::jump_flag`].
    // DC-DEFAULT: SIM-20 (proposed) the whole-flight state is kept for the callers
    pub jumping: bool,
    /// mover+0x80: airborne after a jump. Set at take-off and cleared by the next step's ground test (see
    /// [`PlayerState::takeoff_contact`]), so it lasts one step, not the whole flight. It gates the jump request and
    /// the latch, scales the acceleration by `AIR_CONTROL`, skips container drag, the fall probe and slide-to-stop.
    // EVIDENCE: cshell+0x1ffec0 (a standing contact clears +0x80 and sends the land notice); OBSERVED: MOVEMENT_2009.md
    // §12.3 / M12 (the land notice 3 follows the take-off 1 one frame later, 26 of 27 jumps of tdm1 / tdm4)
    pub jump_flag: bool,
    /// The standing contact at the last take-off, kept for the next step's ground test only: the 2009 engine still
    /// reports the floor the frame after a take-off, which ends the airborne-after-jump flag (MOVEMENT M12).
    // DC-DEFAULT: SIM-20 (proposed) the kept contact ends the flag and sends the land notice but does not make the
    // body count as on the ground for that step (2009: one 1-2 ms frame; at a fixed 1/60 s step the on-ground cap,
    // slide-to-stop and jump test would act about ten times longer)
    pub takeoff_contact: Option<Contact>,
    /// mover+0x68: falling without a jump.
    pub falling: bool,
    /// The jump latch (global VA 0x3759c21c in the client).
    pub jump_latch: bool,
    /// mover+0x84, the latch enable: a JUMP edge latches a jump only while it is set. The player state sets it
    /// when the body turns alive and clears it at death, so a jump key pressed while dead (the corpse still
    /// stands on the ground and the sampling still runs) latches nothing that the spawn would then play.
    // EVIDENCE: cshell+0x1ff980 (the latch needs mover+0x84 != 0), cshell+0x1f97d0 (state 1: +0x84 := 1),
    // cshell+0x1f4110 (the respawn path: +0x84 := 1); DISASM: cshell+0x61870 (state 2: +0x84 := 0, with +0xa8),
    // GAME_MESSAGES_2009_S2C.md §4.1 table
    // DC-DEFAULT: CLI-1607 (a new body starts enabled: offline play has no player state)
    pub jump_enable: bool,
    /// The jump key was already down when the last jump request was taken: a held key requests one jump only,
    /// and a new request needs the key seen up while the body stands on the ground (the 2009 client clears the
    /// flag only in its on-ground sampling branch, so letting go in the air does not count).
    // DISASM: cshell+0x1ffad3..0x1ffb75 (on ground, not on a ladder, not airborne after a jump, not crouched:
    // command 6 off -> flag := 0 through slot +0x11c of the object at VA 0x3759e110; on -> read the flag through
    // slot +0x120, compare with mover+0x468 (always 0: written only by the mover's constructor cshell+0x1fdba0);
    // equal -> flag := 1 and, with the standing box, the JUMP bit)
    // DISASM: cshell+0x64180 / cshell+0x64190 (slots +0x11c / +0x120: write / read the byte flag at +0x188 of that
    // object; no other writer: zeroed by its constructor cshell+0x63e60)
    pub jump_held: bool,
    pub run_lock: bool,
    /// Hop penalty (mover+0x35c).
    pub hop: f32,
    /// Ramp accumulators (mover+0x350 forward / back, +0x354 strafe).
    pub ramp_fwd: f32,
    pub ramp_side: f32,
    /// Last run speed (mover+0x364).
    pub last_run_speed: f32,
    /// The §6.3 ground cap of this step (`Some(target speed)` when the grounded branch applied: on the ground, not
    /// spectating, no jump request, not on a ladder); the physics step keeps the moved speed under it.
    // DC-DEFAULT: SIM-22 (grounded displacement capped at the target speed)
    pub ground_cap: Option<f32>,
    /// Fall start height (mover+0x7c).
    pub fall_start_y: f32,
    pub on_ladder: bool,
    /// Container accumulators of this step.
    pub current_acc: V3,
    pub drag_sum: f32,
    pub gravity_override: Option<f32>,
    /// Crouch command state and press time (s).
    pub crouch_held: bool,
    pub crouch_pressed_at: f64,
    /// Crouch eye offset (controller+0x108) and its timers.
    pub eye_dip: f32,
    pub eye_crouched: bool,
    pub eye_t0: f64,
    pub eye_t1: f64,
    /// Sequence byte of the last applied correction, echoed in every player update.
    // SPEC: MOVEMENT_2009.md §12.2 ([move_seq:u8] mover+0x04)
    pub move_seq: u8,
    /// Freeze position (0xa0 flag 0x0080) and release flag (0x0100).
    pub freeze: Option<V3>,
    pub freeze_released: bool,
    /// Simulation clock in seconds (sum of the step times).
    pub clock: f64,
    /// The sprint gauge (advanced every step by the step's time with the rates of `TickInput::mods.stamina`).
    // EVIDENCE: cshell+0x95850 (see `crate::stamina`)
    pub stamina: Stamina,
}

impl PlayerState {
    /// A standing body at `pos` with box `stand_dims`.
    pub fn new(pos: V3, stand_dims: V3, p: &MovementParams) -> PlayerState {
        PlayerState {
            pos,
            vel: V3::ZERO,
            acc: V3::ZERO,
            dims: stand_dims,
            stand_dims,
            crouch_dims: None,
            contact: None,
            gravity_on: true,
            friction: p.friction_base,
            controls: 0,
            prev_controls: 0,
            on_ground: false,
            jumping: false,
            jump_flag: false,
            falling: false,
            jump_latch: false,
            jump_enable: true,
            jump_held: false,
            run_lock: false,
            hop: 1.0,
            ramp_fwd: p.start_accel,
            ramp_side: p.start_accel,
            last_run_speed: 0.0,
            ground_cap: None,
            fall_start_y: pos.y,
            on_ladder: false,
            current_acc: V3::ZERO,
            drag_sum: 0.0,
            gravity_override: None,
            crouch_held: false,
            crouch_pressed_at: 0.0,
            eye_dip: 0.0,
            eye_crouched: false,
            eye_t0: 0.0,
            eye_t1: 0.0,
            move_seq: 0,
            freeze: None,
            freeze_released: false,
            clock: 0.0,
            stamina: Stamina::default(),
            takeoff_contact: None,
        }
    }

    /// The collision cylinder of the current box.
    pub fn shape(&self) -> Cylinder {
        Cylinder::from_dims(self.dims)
    }

    fn has(&self, b: u32) -> bool {
        self.controls & b != 0
    }

    fn had(&self, b: u32) -> bool {
        self.prev_controls & b != 0
    }

    /// One simulation step of `frame_dt` seconds. The physics part uses `min(frame_dt, dt_clamp)`; the
    /// acceleration ramp and the hop timers use the unclamped value, as in the client.
    // SPEC: MOVEMENT_2009.md §1 (mover_frame order), §4 (who uses dt / frame_dt)
    pub fn step<W: Collider + ?Sized>(
        &mut self,
        p: &MovementParams,
        w: &W,
        input: &TickInput<'_>,
        frame_dt: f32,
    ) -> StepEvents {
        let dt = if frame_dt > p.dt_clamp {
            p.dt_clamp
        } else {
            frame_dt
        };
        let clock_before = self.clock;
        self.clock += frame_dt as f64;
        let mut ev = StepEvents::default();
        // step 3: sample commands into the control word (this also sets the gauge mode)
        self.sample(p, w, input);
        self.stance_box(w);
        // the gauge advances by the real time of the step in the mode the sampling chose (2009: the health HUD
        // draw after the mover, by GetTickCount time)
        // EVIDENCE: cshell+0x95850, cshell+0x96b30
        self.stamina.advance(input.mods.stamina, frame_dt);
        // step 4: movement gate, then the velocity law; the gravity override is cleared first
        self.gravity_override = None;
        self.ground_cap = None;
        if input.movement_allowed {
            self.move_law(p, w, input, frame_dt, dt, &mut ev);
            if todo::SLIDE_TO_STOP_EVERY_STEP {
                self.slide_to_stop(p, dt);
            }
        } else {
            self.frozen_branch(p, input);
        }
        // step 5: freeze enforcement
        if let Some(fp) = self.freeze {
            if !self.freeze_released {
                self.place(p, w, fp);
            }
        }
        // step 8: physics step
        self.phys_step(p, w, dt, &mut ev);
        self.update_eye(p, input, clock_before);
        ev
    }

    /// Commands to control word.
    // SPEC: MOVEMENT_2009.md §5.1, §5.3, §5.4, §5.5
    fn sample<W: Collider + ?Sized>(&mut self, p: &MovementParams, w: &W, input: &TickInput<'_>) {
        use bits::*;
        self.prev_controls = self.controls;
        let c = &input.commands;
        let allowed = input.movement_allowed;
        let blocked = matches!(input.ctrl_state, 2 | 3 | 5 | 6);
        let mut b = 0u32;
        if c.forward && allowed {
            b |= FWD;
        }
        if c.backward && allowed {
            b |= BACK;
        }
        if c.left && allowed {
            b |= LEFT;
        }
        if c.right && allowed {
            b |= RIGHT;
        }
        if c.strafe_mod {
            b |= STRAFE_MOD;
        }
        if c.run_lock_toggle {
            self.run_lock = !self.run_lock;
        }
        // 5.4: crouch, with the 100 ms + stand-up rule on release; not sampled in states 2, 3, 5, 6
        let crouch_cmd = c.crouch && !blocked;
        if crouch_cmd && !self.crouch_held {
            self.crouch_pressed_at = self.clock;
        }
        let mut crouch = crouch_cmd;
        if !crouch_cmd && !blocked && self.had(CROUCH) {
            let waited = self.clock - self.crouch_pressed_at >= p.crouch_release_delay as f64;
            if !(waited && self.stand_up_test(w)) {
                crouch = true;
            }
        }
        self.crouch_held = crouch_cmd;
        if crouch {
            b |= CROUCH;
        }
        // 5.3: RUN and JUMP are sampled only when CROUCH is clear
        if !crouch {
            if c.run != self.run_lock {
                b |= RUN;
            }
            let tall = self.stand_dims.y - p.jump_box_margin <= self.dims.y;
            // the jump key counts once per press: a held key (or one pressed in the air and still down at the
            // landing) requests nothing until it is seen up on the ground (no bunny hopping on a held key)
            // DISASM: cshell+0x1ffad3..0x1ffb75 (see `jump_held`)
            if self.on_ground && !self.on_ladder && !self.jump_flag {
                if !c.jump {
                    self.jump_held = false;
                } else {
                    let fresh = !self.jump_held;
                    self.jump_held = true;
                    if fresh && tall {
                        b |= JUMP;
                    }
                }
            }
        }
        // JUMP and SPRINT are removed while the crouch eye offset is non-zero; this runs before the sprint block,
        // which sets SPRINT again (the eye return sprints and drains)
        // EVIDENCE: cshell+0x1ff980 (controller+0x108 != 0: bits &= 0xffbfffef ahead of the FWD branch)
        if self.eye_dip != 0.0 {
            b &= EYE_DIP_MASK;
        }
        // 5.5: sprint, and the gauge mode: forward with the sprint command, RUN and a gauge above 0 (not on a
        // ladder, no veto) sprints and drains; the same keys with an empty gauge keep it at 0; anything else,
        // a vetoed sprint included, refills
        // EVIDENCE: cshell+0x1ff980 (FWD branch: slots +0xec / +0xcc / +0xd8 / +0xf4 of the gauge object; the
        // mover's veto (method +8) and the ladder go to mode 0 with the pair (0, 1) before the mode 2 test)
        self.stamina.mode = GaugeMode::Rest;
        if b & FWD != 0 && c.sprint && b & RUN != 0 && !self.on_ladder && input.stamina {
            if self.stamina.can_sprint(input.mods.stamina) {
                b |= SPRINT;
                self.stamina.mode = GaugeMode::Sprint;
            } else {
                self.stamina.mode = GaugeMode::Spent;
            }
        }
        // BACK clears SPRINT but not the gauge mode (cshell+0x1ffd0f..: the BACK block runs after the mode is set)
        if b & FWD == 0 || b & BACK != 0 {
            b &= !SPRINT;
        }
        if b & ANY_MOVE_MASK != 0 {
            b |= ANY_MOVE;
        }
        // the latch: JUMP went 0 -> 1 while not airborne after a jump and with the latch enable set (alive)
        // EVIDENCE: cshell+0x1ff980 (mover+0x80 == 0 && mover+0x84 != 0 -> VA 0x3759c21c := 1); CLI-1607
        if b & JUMP != 0 && !self.had(JUMP) && !self.jump_flag && self.jump_enable {
            self.jump_latch = true;
        }
        self.controls = b;
    }

    /// The crouch box follows the stance: `CROUCH` set shrinks the standing box to [`PlayerState::crouch_dims`],
    /// cleared grows it back when the standing box fits; the feet stay where they are.
    // DC-DEFAULT: SIM-25 (proposed)
    fn stance_box<W: Collider + ?Sized>(&mut self, w: &W) {
        let Some(crouch) = self.crouch_dims else {
            return;
        };
        if self.has(bits::CROUCH) {
            if self.dims == self.stand_dims && crouch != self.stand_dims {
                self.pos.y -= self.stand_dims.y - crouch.y;
                self.dims = crouch;
            }
        } else if self.dims == crouch && crouch != self.stand_dims {
            let at = self.pos + V3::new(0.0, self.stand_dims.y - crouch.y, 0.0);
            if !w.overlaps(&Cylinder::from_dims(self.stand_dims), at) {
                self.pos = at;
                self.dims = self.stand_dims;
            }
        }
    }

    /// How far the body centre sits below its standing height because the crouch box is in force (0 standing):
    /// the camera adds it so the eye keeps its height above the feet.
    // DC-DEFAULT: SIM-25 (proposed)
    pub fn crouch_drop(&self) -> f32 {
        match self.crouch_dims {
            Some(c) if self.dims == c && c != self.stand_dims => self.stand_dims.y - c.y,
            _ => 0.0,
        }
    }

    /// Stand-up test: already tall, or the standing box fits with the feet kept in place.
    // SPEC: MOVEMENT_2009.md §5.4 (cshell+0x1fcfd0; trial set_box(stand_dims, must_fit 1))
    fn stand_up_test<W: Collider + ?Sized>(&self, w: &W) -> bool {
        if self.dims.y > self.stand_dims.y - todo::STANDUP_TALL_MARGIN {
            return true;
        }
        let raise = self.stand_dims.y - self.dims.y;
        let at = self.pos + V3::new(0.0, raise, 0.0);
        !w.overlaps(&Cylinder::from_dims(self.stand_dims), at)
    }

    /// `run_speed()`: forward, backward, sideways, or the last value; stored back.
    // SPEC: MOVEMENT_2009.md §7 (cshell+0x1fd2e0)
    fn run_speed(&mut self, p: &MovementParams) -> f32 {
        use bits::*;
        let s = if self.has(FWD) {
            p.run_forward
        } else if self.has(BACK) {
            p.run_backward
        } else if self.has(LEFT) || self.has(RIGHT) {
            p.run_sideways
        } else {
            self.last_run_speed
        };
        self.last_run_speed = s;
        s
    }

    /// `target_speed()`.
    // SPEC: MOVEMENT_2009.md §7 (cshell+0x1fd390), steps 1-5
    fn target_speed(&mut self, p: &MovementParams, input: &TickInput<'_>) -> f32 {
        use bits::*;
        let crouch = self.has(CROUCH);
        let s = if self.on_ladder {
            p.ladder_move_speed
        } else if self.has(SPRINT) && input.stamina && input.sprint_allowed {
            if crouch {
                p.crouch_speed
            } else if !input.sprint_blocked {
                let mul = input.mods.sprint_mul.unwrap_or(p.sprint_mul_default);
                (input.mods.sprint_add + 1.0) * self.run_speed(p) * mul
            } else {
                p.walk_speed
            }
        } else if self.has(RUN) {
            if crouch {
                p.crouch_speed
            } else if !input.run_blocked {
                self.run_speed(p)
            } else {
                p.walk_speed
            }
        } else if crouch {
            p.crouch_speed
        } else {
            p.walk_speed
        };
        let pen = input.mods.penalty.clamp(-1.0, 1.0);
        (1.0 - pen) * s
    }

    /// The speed cap the velocity law would use with the current control word and `input` (the state is not
    /// changed): the `(1 - p)` scaled stance speed of §7. For tests, logs and server-side plausibility checks.
    // SPEC: MOVEMENT_2009.md §7 (target_speed, cshell+0x1fd390)
    pub fn speed_cap(&self, p: &MovementParams, input: &TickInput<'_>) -> f32 {
        self.clone().target_speed(p, input)
    }

    /// Containers refresh, drag and friction coefficient.
    // SPEC: MOVEMENT_2009.md §11.2 (cshell+0x1fe300), §11.3 (cshell+0x1fe060), §8.4 (cshell+0x1fce10)
    fn refresh_containers(&mut self, p: &MovementParams, input: &TickInput<'_>, dt: f32) {
        self.current_acc = V3::ZERO;
        self.drag_sum = 0.0;
        self.on_ladder = false;
        self.gravity_on = input.mode != MoveMode::Spectate;
        for c in input
            .containers
            .iter()
            .filter(|c| c.kind == Container::LADDER && !c.hidden)
        {
            if !self.on_ladder {
                self.on_ladder = true;
                self.gravity_on = false;
            }
            self.drag(c, dt);
            self.current_acc += c.current * dt;
        }
        for c in input
            .containers
            .iter()
            .filter(|c| c.kind != Container::LADDER && !c.hidden)
        {
            self.drag(c, dt);
            if c.kind == Container::GRAVITY {
                self.gravity_override = Some(self.gravity_override.unwrap_or(0.0) + c.gravity);
            }
            self.current_acc += c.current * dt;
        }
        let mut factor = 1.0f32;
        for c in input.containers {
            if c.friction < factor {
                factor = c.friction;
            }
        }
        self.friction = p.friction_base * factor;
    }

    // SPEC: MOVEMENT_2009.md §11.3 (drag_sum += viscosity; v -= normalize(v) viscosity 5000 dt; no reversal)
    fn drag(&mut self, c: &Container, dt: f32) {
        if self.on_ladder || !self.jump_flag {
            self.drag_sum += c.viscosity;
            let l = self.vel.len();
            if c.viscosity > 0.0 && l > 1.0 {
                let dv = c.viscosity * 5000.0 * dt;
                if dv >= l {
                    self.vel = V3::ZERO;
                } else {
                    self.vel -= self.vel.normalized() * dv;
                }
            }
        }
    }

    /// Ground state, landing and fall start.
    // SPEC: MOVEMENT_2009.md §10.2 (cshell+0x1ffec0), fall probe cshell+0x1fe1f0
    fn ground_state<W: Collider + ?Sized>(
        &mut self,
        p: &MovementParams,
        w: &W,
        input: &TickInput<'_>,
        ev: &mut StepEvents,
    ) {
        let spectate = input.mode == MoveMode::Spectate;
        self.on_ground = self.contact.is_some();
        // the floor of the take-off, reported once more by the step after it (MOVEMENT M12)
        // DC-DEFAULT: SIM-20 (proposed) it ends the flag and the fall height, not the airborne state
        if self.takeoff_contact.take().is_some() && self.contact.is_none() && self.jump_flag {
            self.fall_start_y = self.pos.y;
            self.jump_flag = false;
            ev.landed_from_jump = true;
        }
        if let Some(c) = self.contact {
            self.jumping = false;
            if !self.jump_flag && self.falling {
                ev.landed_from_fall = true;
            }
            self.falling = false;
            self.fall_start_y = self.pos.y;
            if self.jump_flag {
                self.jump_flag = false;
                ev.landed_from_jump = true;
            }
            if c.normal.y < p.ground_cos {
                self.on_ground = false;
            }
        }
        if !self.on_ground {
            self.fall_start_y = if spectate {
                -1e7
            } else if self.pos.y > self.fall_start_y {
                self.pos.y
            } else {
                self.fall_start_y
            };
        }
        if input.mode == MoveMode::Walk
            && !self.on_ladder
            && !self.jump_flag
            && !self.falling
            && !self.on_ground
        {
            let below = self.pos - V3::new(0.0, p.fall_probe_depth, 0.0);
            if w.sweep(&Cylinder::POINT, self.pos, below).is_none() {
                self.falling = true;
                ev.fall_started = true;
            }
        }
    }

    /// The velocity law.
    // SPEC: MOVEMENT_2009.md §6.2-§6.10 (move_law, cshell+0x2001c0)
    fn move_law<W: Collider + ?Sized>(
        &mut self,
        p: &MovementParams,
        w: &W,
        input: &TickInput<'_>,
        frame_dt: f32,
        dt: f32,
        ev: &mut StepEvents,
    ) {
        use bits::*;
        let spectate = input.mode == MoveMode::Spectate;
        let fly_mode = input.mode == MoveMode::Fly;
        let blocked = matches!(input.ctrl_state, 2 | 3 | 5 | 6);
        // 6.2: acceleration := 0, containers and ground, then velocity and target speed
        self.acc = V3::ZERO;
        self.refresh_containers(p, input, dt);
        self.ground_state(p, w, input, ev);
        let mut v = self.vel;
        let speed = self.target_speed(p, input);
        let ladder = self.on_ladder;
        let fly = fly_mode || ladder || spectate;
        let jr = if fly {
            self.jump_latch = false;
            self.has(JUMP)
        } else {
            self.jump_latch && !self.jump_flag
        };
        // 6.3: speed cap before acceleration
        if ladder {
            if v.y > p.ladder_max_up {
                v.y = p.ladder_max_up;
                self.vel = v;
            }
        } else if self.on_ground && !spectate && !jr {
            let h = v.len_xz();
            if h > speed {
                v *= speed / h; // the whole vector, y included
                self.vel = v;
            }
            self.ground_cap = Some(speed);
        } else {
            let h = v.len_xz();
            if h > speed {
                let s = speed / h;
                v.x *= s;
                v.z *= s;
                self.vel = v;
            }
        }
        // basis: body rotation (yaw only) when walking, camera rotation otherwise
        let q = if !spectate && !ladder && !fly_mode {
            Quat::from_yaw(input.yaw)
        } else {
            Quat::from_yaw_pitch(input.yaw, input.pitch)
        };
        let f = q.forward();
        let r = q.right_flat();
        // 6.4: per-axis acceleration
        let mut a0 = p.base_accel;
        let fdir = if !fly {
            V3::new(f.x, 0.0, f.z).normalized()
        } else if !ladder {
            let mut g = f;
            if g.y > 0.0 {
                g.y = 0.0;
            }
            g.normalized()
        } else {
            f
        };
        if spectate {
            self.vel = V3::ZERO;
            a0 = if self.has(RUN) {
                p.spectate_accel_run
            } else {
                p.spectate_accel
            } * p.spectate_mul;
        }
        let jump_v = p.jump_speed * (1.0 + input.mods.jump_bonus) * self.hop;
        if self.has(CROUCH) && !ladder {
            a0 *= p.crouch_accel_mul;
        }
        if self.jump_flag && !fly {
            a0 *= p.air_control;
        }
        let mut acc = V3::ZERO;
        if input.ctrl_state != 3 {
            if spectate {
                // 6.6 spectator sum; a zero sum stays zero (the client would divide by zero)
                let r3 = q.right();
                if self.has(FWD) {
                    acc += f * a0;
                }
                if self.has(BACK) {
                    acc -= f * a0;
                }
                if self.has(RIGHT) {
                    acc += r3 * a0;
                }
                if self.has(LEFT) {
                    acc -= r3 * a0;
                }
                acc = if acc.len_sq() > 0.0 {
                    acc.with_len(todo::SPECTATE_EXTRA_ACCEL + p.spectate_accel)
                } else {
                    V3::ZERO
                };
            } else {
                let mut af = fdir * a0;
                let mut ar = r * a0;
                let mut boost = 0.0f32;
                // 6.5: acceleration ramp (not when jumping this step or crouched)
                if !jr && !self.has(CROUCH) {
                    let (s, m, k) = (p.start_accel, p.max_accel, p.accel_rate);
                    let mut reset = false;
                    let mut skip = false;
                    if self.has(ANY_MOVE) && !self.had(ANY_MOVE) {
                        boost = s;
                        self.ramp_fwd = s;
                        self.ramp_side = s;
                        reset = true;
                    }
                    if (self.has(FWD) && self.had(BACK)) || (self.has(BACK) && self.had(FWD)) {
                        af = af.normalized() * s;
                        self.ramp_fwd = s;
                        reset = true;
                    }
                    if self.has(LEFT) && self.had(RIGHT) {
                        ar = ar.normalized() * s;
                        self.ramp_side = s;
                        reset = true;
                    }
                    if self.has(RIGHT) && self.had(LEFT) {
                        ar = ar.normalized() * s;
                        self.ramp_side = s;
                        skip = true;
                    }
                    if !reset && !skip {
                        if self.has(FWD) || self.has(BACK) {
                            let nf = self.ramp_fwd + frame_dt * k;
                            if nf < m {
                                af = af.normalized() * nf;
                                self.ramp_fwd = nf;
                            } else {
                                af = af.normalized() * m;
                                self.ramp_fwd = m;
                            }
                        }
                        if self.has(LEFT) || self.has(RIGHT) {
                            let ns = self.ramp_side + frame_dt * k;
                            if ns < m {
                                ar = ar.normalized() * ns;
                                self.ramp_side = ns;
                            } else {
                                ar = ar.normalized() * m;
                                self.ramp_side = m;
                            }
                        }
                    }
                    if !(self.has(FWD) || self.has(BACK)) {
                        self.ramp_fwd = s;
                    }
                    if !(self.has(LEFT) || self.has(RIGHT)) {
                        self.ramp_side = s;
                    }
                }
                // 6.6: sum and caps
                if self.has(FWD) {
                    acc += af;
                }
                if self.has(BACK) {
                    acc -= af;
                }
                if fly {
                    if jr {
                        acc.y += a0;
                    }
                    if self.has(CROUCH) {
                        acc.y = -a0;
                    }
                }
                if self.has(LEFT) {
                    acc -= ar;
                    if acc.len() > p.max_accel {
                        acc = acc.with_len(p.max_accel);
                    }
                }
                if self.has(RIGHT) {
                    acc += ar;
                    if acc.len() > p.max_accel {
                        acc = acc.with_len(p.max_accel);
                    }
                }
                if boost > 0.0 && acc.len_sq() > 0.0 {
                    acc = acc.with_len(boost);
                }
            }
        }
        // 6.7: hop penalty on the acceleration
        if self.hop < 1.0 {
            acc *= p.hop_accel_mul * self.hop;
        }
        // 6.9: knockback replaces the acceleration
        if let Some(kb) = input.knockback {
            if p.knockback != 0 {
                let k = p.knockback as f32;
                let mut mag = (1.0 - kb.elapsed) * k * k;
                if self.has(CROUCH) {
                    if self.has(ANY_MOVE) {
                        mag *= 0.25;
                    } else {
                        self.vel = V3::ZERO;
                        mag = 0.0;
                    }
                }
                if kb.dir.x * kb.dir.x + kb.dir.z * kb.dir.z != 0.0 {
                    acc = V3::new(kb.dir.x, 0.0, kb.dir.z).normalized() * mag;
                }
            }
        }
        // 6.10: write the acceleration, re-read the velocity, jump, current, write the velocity
        self.acc = acc;
        let mut v = self.vel;
        if jr && !blocked && self.on_ground && !ladder {
            self.jumping = true;
            self.jump_flag = true;
            self.takeoff_contact = self.contact;
            self.jump_latch = false;
            v.y = jump_v;
            if self.hop >= p.hop_min {
                self.hop = p.hop_min;
            } else {
                self.hop -= frame_dt * p.hop_decay;
            }
            ev.jumped = true;
        } else if self.hop >= 1.0 {
            self.hop = 1.0;
        } else {
            self.hop += frame_dt * p.hop_recover;
        }
        if spectate {
            v *= p.spectate_damping;
            if v.len() < p.spectate_snap {
                v = V3::ZERO;
            }
        }
        v += self.current_acc;
        self.vel = v;
        if input.ctrl_state == 3 || !input.body_active {
            self.vel = V3::ZERO;
            self.acc = V3::ZERO;
        }
    }

    /// Movement not allowed: keep only downward motion, halve the horizontal velocity.
    // SPEC: MOVEMENT_2009.md §6 (frozen branch of cshell+0x201470, DISASM)
    fn frozen_branch(&mut self, p: &MovementParams, input: &TickInput<'_>) {
        let blocked = matches!(input.ctrl_state, 2 | 3 | 5 | 6);
        let a = self.acc;
        let ay = if a.y > 0.0 {
            0.0
        } else if a.y < 0.0 {
            if blocked && !self.falling {
                0.0
            } else if a.y > p.frozen_fall_cap {
                a.y
            } else {
                p.frozen_fall_cap
            }
        } else {
            0.0
        };
        self.acc = V3::new(0.0, ay, 0.0);
        let v = self.vel;
        self.vel = V3::new(v.x * 0.5, if v.y < 0.0 { v.y } else { 0.0 }, v.z * 0.5);
    }

    /// Slide to stop when no key is held on the ground.
    // SPEC: MOVEMENT_2009.md §6.8 (cshell+0x1fe940)
    fn slide_to_stop(&mut self, p: &MovementParams, dt: f32) {
        let probe = todo::SLIDE_TO_STOP_PROBE_IS_ON_GROUND && self.on_ground;
        if self.has(bits::ANY_MOVE)
            || self.jump_flag
            || !self.on_ground
            || !probe
            || self.drag_sum != 0.0
        {
            return;
        }
        let v = self.vel;
        let h = v.len_xz();
        if h <= p.stop_snap_speed {
            self.vel = V3::ZERO;
            return;
        }
        let t = if p.stop_time <= 0.0 { 0.1 } else { p.stop_time };
        let d = v.xz().normalized() * (self.run_speed(p) / t * dt);
        if d.len() >= h {
            self.vel = V3::new(0.0, v.y, 0.0);
        } else {
            self.vel = V3::new(v.x - d.x, v.y, v.z - d.z);
        }
    }

    /// The physics step: gravity record, integrate, move with collision, floor/ceiling stop.
    // SPEC: MOVEMENT_2009.md §8.1 (phys_step, cshell+0x1fe6c0)
    fn phys_step<W: Collider + ?Sized>(
        &mut self,
        p: &MovementParams,
        w: &W,
        dt: f32,
        ev: &mut StepEvents,
    ) {
        let g = self.gravity_override.unwrap_or(p.gravity);
        let env = IntegrateEnv {
            force: V3::new(0.0, g, 0.0),
            gravity: self.gravity_on,
            standing: self.contact.map(|c| c.normal),
            standable: p.standable_record(),
            friction: self.friction,
            dead_zone_sq: p.dead_zone_sq,
        };
        let mut r = integrate(&mut self.vel, &mut self.acc, &env, dt);
        if r.branch == Branch::Rest {
            return;
        }
        // The 2009 client applied the §6.3 cap once per drawn frame (1-2 ms on the real client), so a grounded body
        // moved at the target speed: OBSERVED weapon 27 run 187.0, knife 255.4 (rom_speed.rs). At our fixed step the
        // acceleration after the cap adds up to MAX_ACCEL x dt (about 9 % at 60 Hz for a sniper), so the same cap is
        // applied to the step's displacement and velocity (the limit of the 2009 law as the frame time goes to 0).
        // DC-DEFAULT: SIM-22 (grounded displacement capped at the target speed)
        if let Some(cap) = self.ground_cap {
            if matches!(r.branch, Branch::Supported | Branch::SupportedFriction) {
                let h = r.out.len_xz();
                if h > cap * dt {
                    r.out *= cap * dt / h; // the whole vector, as §6.3
                }
                let hv = self.vel.len_xz();
                if hv > cap {
                    self.vel *= cap / hv;
                }
            }
        }
        if r.out.len_sq() > p.min_move_sq {
            let old = self.pos;
            let shape = self.shape();
            self.pos = sweep_move(
                w,
                &shape,
                old,
                old + r.out,
                false,
                &mut self.contact,
                p.step(),
                p.standable_cos,
            );
            ev.moved = true;
            if abs(r.out.y) > 0.01 && abs(self.pos.y - old.y) < 0.01 {
                self.vel.y = 0.0;
            }
        }
    }

    /// Crouch eye offset: dips at -75 per second down to `crouch_eye_dip` while crouched, returns with an
    /// accelerating rate after standing up; zero on ladders, in fly modes and in states 2, 3, 5, 6.
    // SPEC: MOVEMENT_2009.md §13.4 (crouch dip, cshell+0x1f2650, rates cshell+0x1f20b0), §5.3 (gates jumping)
    fn update_eye(&mut self, p: &MovementParams, input: &TickInput<'_>, clock_before: f64) {
        let blocked = matches!(input.ctrl_state, 2 | 3 | 5 | 6);
        if self.on_ladder || input.mode != MoveMode::Walk || blocked {
            self.eye_dip = 0.0;
            self.eye_crouched = false;
            return;
        }
        if self.has(bits::CROUCH) {
            if !self.eye_crouched {
                self.eye_crouched = true;
                self.eye_t0 = clock_before; // the first frame counts one frame
            }
            let d = -p.crouch_eye_rate * (self.clock - self.eye_t0) as f32;
            self.eye_dip = if d < p.crouch_eye_dip {
                p.crouch_eye_dip
            } else {
                d
            };
        } else {
            if self.eye_crouched {
                self.eye_crouched = false;
                self.eye_t1 = clock_before;
            }
            if self.eye_dip < 0.0 {
                self.eye_dip += p.crouch_eye_rate * (self.clock - self.eye_t1) as f32;
                if self.eye_dip > 0.0 {
                    self.eye_dip = 0.0;
                }
            }
        }
    }

    /// Teleport (no collision test) when `pos` differs from the current position.
    // SPEC: MOVEMENT_2009.md §9.1 (move_to only when pos differs), §9.3 (flag 9 = teleport)
    fn place<W: Collider + ?Sized>(&mut self, p: &MovementParams, w: &W, pos: V3) {
        if pos != self.pos {
            let shape = self.shape();
            self.pos = sweep_move(
                w,
                &shape,
                self.pos,
                pos,
                true,
                &mut self.contact,
                p.step(),
                p.standable_cos,
            );
        }
    }

    /// Sets the collision box; with `must_fit` it is refused (returns false) when the new box would
    /// intersect the world at the current position.
    // SPEC: MOVEMENT_2009.md §9.1 (set_box(obj, dims, must_fit), returns 0 on success)
    pub fn set_box<W: Collider + ?Sized>(&mut self, w: &W, dims: V3, must_fit: bool) -> bool {
        if must_fit && w.overlaps(&Cylinder::from_dims(dims), self.pos) {
            return false;
        }
        self.dims = dims;
        true
    }

    /// Applies a server correction (teleport and new box). Returns whether the box fitted (`false`: it was
    /// forced, as the client does on its fallback).
    // SPEC: MOVEMENT_2009.md §12.4 flag 0x0020 (cshell+0x1feb90)
    pub fn apply_correction<W: Collider + ?Sized>(
        &mut self,
        p: &MovementParams,
        w: &W,
        c: &Correction,
    ) -> bool {
        self.move_seq = c.seq;
        self.dims = V3::new(0.5, 0.5, 0.5);
        self.place(p, w, c.pos);
        let fitted = self.set_box(w, c.dims, true);
        if !fitted {
            self.set_box(w, c.dims, false);
        }
        self.acc = V3::ZERO;
        match c.velocity {
            Some(v) => self.vel = v,
            None => {
                self.vel = V3::ZERO;
                self.fall_start_y = self.pos.y;
            }
        }
        fitted
    }

    /// The world models in `gone` left the collision set (a destroyed breakable): a body standing on one of them
    /// loses its standing contact, so the next step integrates the fall. Without it a body at rest keeps the contact
    /// of the vanished model: the standing integrator rests on `v == 0 and a == 0` and moves nothing, so the ground
    /// probe that would clear the contact never runs until an input moves the body. Returns whether the contact
    /// was cleared.
    // DISASM: engine+0x70c70 (every rider of an object: standing object +0x164 and polygon +0x168 := 0, awake bit
    // 0x40 set), called by the client object removal engine+0x17cf0 and sm_RemoveObjectFromWorld engine+0x9f200
    // SPEC: MOVEMENT_2009.md §8.3 (REST in the standing case), §8.1 step 5 (no move below 0.1 units)
    // DC-DEFAULT: CLI-1369 (proposed) a model switched to non-solid detaches its riders like a removed object
    pub fn detach_from_models(&mut self, gone: &[u32]) -> bool {
        match self.contact {
            Some(c) if gone.contains(&c.model) => {
                self.contact = None;
                true
            }
            _ => false,
        }
    }

    /// Freeze flag (0xa0 flag 0x0080): on stores the current position, re-applied every step.
    // SPEC: MOVEMENT_2009.md §12.4 flag 0x0080 (mover+0x338, +0x33c, cshell+0x1ff3f0)
    pub fn set_freeze(&mut self, on: bool) {
        self.freeze = if on { Some(self.pos) } else { None };
    }

    /// Release flag (0xa0 flag 0x0100): when set the freeze is not applied.
    // SPEC: MOVEMENT_2009.md §12.4 flag 0x0100 (mover+0x348)
    pub fn set_freeze_release(&mut self, release: bool) {
        self.freeze_released = release;
    }

    /// FNV-1a over every field's bits: equal digests mean bit-identical states (desync checks).
    pub fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |v: u64| {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for v in [
            self.pos,
            self.vel,
            self.acc,
            self.dims,
            self.stand_dims,
            self.current_acc,
        ] {
            for b in v.bits() {
                eat(b as u64);
            }
        }
        match self.contact {
            Some(c) => {
                eat(1);
                for b in c.normal.bits() {
                    eat(b as u64);
                }
                eat(c.surface_flags as u64);
                eat(c.model as u64);
                eat(c.poly as u64);
            }
            None => eat(0),
        }
        for f in [
            self.friction,
            self.hop,
            self.ramp_fwd,
            self.ramp_side,
            self.last_run_speed,
            self.fall_start_y,
            self.drag_sum,
            self.eye_dip,
            self.gravity_override.unwrap_or(f32::NAN),
            self.stamina.value.unwrap_or(f32::NAN),
        ] {
            eat(f.to_bits() as u64);
        }
        let flags = [
            self.gravity_on,
            self.on_ground,
            self.jumping,
            self.jump_flag,
            self.falling,
            self.jump_latch,
            self.run_lock,
            self.on_ladder,
            self.crouch_held,
            self.eye_crouched,
            self.freeze_released,
            self.freeze.is_some(),
            self.jump_held,
            self.takeoff_contact.is_some(),
            // stored inverted so an enabled body (the default) keeps the digests of before CLI-1607
            !self.jump_enable,
        ];
        eat(flags
            .iter()
            .enumerate()
            .fold(0u64, |a, (i, &f)| a | (f as u64) << i));
        eat(self.controls as u64);
        eat(self.prev_controls as u64);
        eat(self.move_seq as u64);
        eat(self.stamina.mode as u64);
        if let Some(fp) = self.freeze {
            for b in fp.bits() {
                eat(b as u64);
            }
        }
        for t in [self.crouch_pressed_at, self.eye_t0, self.eye_t1, self.clock] {
            eat(t.to_bits());
        }
        h
    }
}
