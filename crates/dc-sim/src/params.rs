//! Movement tunables. Every field has the 2009 client's code default (all are code constants of the 2009
//! client, none is a ROM table, MOVEMENT_2009.md §2 note) and can be overridden per server or per match:
//! by name through [`MovementParams::set`], or with the `serde` feature from a config table
//! (`#[serde(default)]`, unknown keys refused).
// SPEC: MOVEMENT_2009.md §2 (console variables and hard-coded defaults, cshell+0x1fdcb0)

/// The movement parameter block shared by server and client.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct MovementParams {
    /// Gravity along Y during the player's physics step (units/s^2).
    // SPEC: MOVEMENT_2009.md §2 GRAVITY (-1100.0), §8.1 step 1
    pub gravity: f32,
    /// Acceleration factor while airborne after a jump.
    // SPEC: MOVEMENT_2009.md §2 AIR_CONTROL (0.1), §6.4
    pub air_control: f32,
    /// Slide-to-stop time (s); 0.1 is used when the value is <= 0.
    // SPEC: MOVEMENT_2009.md §2 STOP_TIME (0.1), §6.8
    pub stop_time: f32,
    /// Speed below which slide-to-stop zeroes the velocity at once.
    // SPEC: MOVEMENT_2009.md §6.8 (h <= 5 -> velocity := 0)
    pub stop_snap_speed: f32,
    /// Acceleration magnitude per input axis (units/s^2).
    // SPEC: MOVEMENT_2009.md §2 BASE_ACCEL (3000.0), §6.4
    pub base_accel: f32,
    /// Acceleration at the start of a move.
    // SPEC: MOVEMENT_2009.md §2 START_ACCEL (500.0), §6.5
    pub start_accel: f32,
    /// Ramp ceiling and total-acceleration cap.
    // SPEC: MOVEMENT_2009.md §2 MAX_ACCEL (3000.0), §6.5, §6.6
    pub max_accel: f32,
    /// Ramp rate (units/s^2 per second).
    // SPEC: MOVEMENT_2009.md §2 ACCEL_RATE (6000.0), §6.5
    pub accel_rate: f32,
    /// Factor on the per-axis acceleration while crouched (not on a ladder).
    // SPEC: MOVEMENT_2009.md §6.4 (CROUCH and not on a ladder: a0 *= 0.5)
    pub crouch_accel_mul: f32,
    /// Walking speed cap.
    // SPEC: MOVEMENT_2009.md §2 WALK_SPEED (70.0), §7
    pub walk_speed: f32,
    /// Running speed when moving forward.
    // SPEC: MOVEMENT_2009.md §2 RUN_FWD (285.0), §7
    pub run_forward: f32,
    /// Running speed when moving backward.
    // SPEC: MOVEMENT_2009.md §2 RUN_BACK (285.0), §7
    pub run_backward: f32,
    /// Running speed when only strafing.
    // SPEC: MOVEMENT_2009.md §2 RUN_SIDE (285.0), §7
    pub run_sideways: f32,
    /// Crouched speed cap.
    // SPEC: MOVEMENT_2009.md §2 CROUCH_SPEED (50.0), §7
    pub crouch_speed: f32,
    /// Speed on a ladder (hard-coded in the client).
    // SPEC: MOVEMENT_2009.md §7 step 1 (s = 200, hard-coded)
    pub ladder_move_speed: f32,
    /// Upward speed cap on a ladder (hard-coded in the client).
    // SPEC: MOVEMENT_2009.md §6.3 (on a ladder: v.y capped at 200)
    pub ladder_max_up: f32,
    /// Jump take-off vertical speed, before the figure bonus and the hop penalty.
    // SPEC: MOVEMENT_2009.md §2 JUMP_SPEED (330.0), §6.4 (jump_v = JUMP_SPEED * (1 + fig_jump_bonus) * hop)
    pub jump_speed: f32,
    /// Acceleration factor while the hop penalty is active.
    // SPEC: MOVEMENT_2009.md §2 HOP_ACCEL_MUL (1.0), §6.7
    pub hop_accel_mul: f32,
    /// Hop-penalty recovery per second.
    // SPEC: MOVEMENT_2009.md §2 HOP_RECOVER (1.2), §6.7
    pub hop_recover: f32,
    /// Hop-penalty value after a jump.
    // SPEC: MOVEMENT_2009.md §2 HOP_MIN (0.3), §6.7
    pub hop_min: f32,
    /// Further hop decrease per second for chained jumps.
    // SPEC: MOVEMENT_2009.md §2 HOP_DECAY (0.05), §6.7
    pub hop_decay: f32,
    /// Knockback strength (an integer console variable in the client; 0 disables knockback).
    // SPEC: MOVEMENT_2009.md §2 KNOCKBACK (0, integer), §6.9
    pub knockback: i32,
    /// Free-fly acceleration factor while spectating.
    // SPEC: MOVEMENT_2009.md §2 SPECTATE_MUL (1.0), §6.4
    pub spectate_mul: f32,
    /// Spectator per-axis acceleration while walking / running (hard-coded).
    // SPEC: MOVEMENT_2009.md §6.4 (a0 = (RUN ? 600000 : 300000) * SPECTATE_MUL)
    pub spectate_accel: f32,
    pub spectate_accel_run: f32,
    /// Spectator velocity damping per step and the speed below which it snaps to zero.
    // SPEC: MOVEMENT_2009.md §6.10 (v *= 0.9; v := 0 if |v| < 0.1)
    pub spectate_damping: f32,
    pub spectate_snap: f32,
    /// Step height (the engine adds `step_margin` when stair stepping).
    // SPEC: MOVEMENT_2009.md §2 STEP_HEIGHT (default 30.0, server-settable), §9.2 (step = get_step() + 0.5)
    pub step_height: f32,
    // SPEC: MOVEMENT_2009.md §9.2 (+ 0.5, engine+0x75110)
    pub step_margin: f32,
    /// Friction coefficient before the container factor: `k = friction_base * min(1, factor)`.
    // SPEC: MOVEMENT_2009.md §8.4 (k = 5.0 * min(1.0, ...), cshell+0x1fce10)
    pub friction_base: f32,
    /// Cosine of the steepest standable slope. The engine stores it negated in its gravity record
    /// (-0.7071, bits 0xbf350481) and the stair / probe code compares `n.y` with 0.7071.
    // SPEC: MOVEMENT_2009.md §8.2 (standable = -0.7071, bits 0xbf350481), §9.7, §10.1 (0.7071)
    pub standable_cos: f32,
    /// The mover's own on-ground threshold for `n.y` (slightly different constant from the engine's).
    // SPEC: MOVEMENT_2009.md §10.2 (on_ground := 0 when the standing plane normal y is < 0.707)
    pub ground_cos: f32,
    /// Physics time step clamp (s).
    // SPEC: MOVEMENT_2009.md §4 (shell clamp to 0.1, cshell+0x1aa810)
    pub dt_clamp: f32,
    /// Integrator dead zones on `|v|^2` and `|a|^2`.
    // SPEC: MOVEMENT_2009.md §8.3 (if |v|^2 < 0.1: v = 0; if |a|^2 < 0.1: a = 0)
    pub dead_zone_sq: f32,
    /// Displacements with `|out|^2` at or below this are dropped (the body does not move).
    // SPEC: MOVEMENT_2009.md §8.1 step 5 (|out|^2 > 0.01)
    pub min_move_sq: f32,
    /// Vertical acceleration floor in the frozen branch (hard-coded -1100).
    // SPEC: MOVEMENT_2009.md §6 (frozen branch: a.y' = max(a.y, -1100), VA 0x374efb54)
    pub frozen_fall_cap: f32,
    /// Fall-start probe depth: no ground within this many units below starts a fall.
    // SPEC: MOVEMENT_2009.md §10.2 (probe cshell+0x1fe1f0, 150 units)
    pub fall_probe_depth: f32,
    /// Crouch release delay before the stand-up test may succeed (s).
    // SPEC: MOVEMENT_2009.md §5.4 (at least 100 ms since the press)
    pub crouch_release_delay: f32,
    /// Stand-box tolerance for the jump request: jump needs `stand_dims.y - jump_box_margin <= dims.y`.
    // SPEC: MOVEMENT_2009.md §5.3 (stand_dims.y - 0.5 <= dims.y)
    pub jump_box_margin: f32,
    /// Crouch camera dip floor and the dip / return rates (units, units/s^2-like rates as in the client).
    // SPEC: MOVEMENT_2009.md §13.1 CROUCH_CAM_DIP (-13.0), §13.4 (rates -75 / +75, controller+0x10c / +0x110)
    pub crouch_eye_dip: f32,
    pub crouch_eye_rate: f32,
    /// Sprint multiplier of the figure record when the ROM key is missing.
    // SPEC: MOVEMENT_2009.md §3.1 (fig_sprint_mul, code default 1.4)
    pub sprint_mul_default: f32,
}

impl MovementParams {
    /// The 2009 client's defaults.
    pub const DEFAULT: MovementParams = MovementParams {
        gravity: -1100.0,
        air_control: 0.1,
        stop_time: 0.1,
        stop_snap_speed: 5.0,
        base_accel: 3000.0,
        start_accel: 500.0,
        max_accel: 3000.0,
        accel_rate: 6000.0,
        crouch_accel_mul: 0.5,
        walk_speed: 70.0,
        run_forward: 285.0,
        run_backward: 285.0,
        run_sideways: 285.0,
        crouch_speed: 50.0,
        ladder_move_speed: 200.0,
        ladder_max_up: 200.0,
        jump_speed: 330.0,
        hop_accel_mul: 1.0,
        hop_recover: 1.2,
        hop_min: 0.3,
        hop_decay: 0.05,
        knockback: 0,
        spectate_mul: 1.0,
        spectate_accel: 300_000.0,
        spectate_accel_run: 600_000.0,
        spectate_damping: 0.9,
        spectate_snap: 0.1,
        step_height: 30.0,
        step_margin: 0.5,
        friction_base: 5.0,
        // exactly the client's 0.7071 (bits 0x3f350481), which is not 1/sqrt(2) (0x3f3504f3)
        standable_cos: f32::from_bits(0x3f35_0481),
        ground_cos: 0.707,
        dt_clamp: 0.1,
        dead_zone_sq: 0.1,
        min_move_sq: 0.01,
        frozen_fall_cap: -1100.0,
        fall_probe_depth: 150.0,
        crouch_release_delay: 0.1,
        jump_box_margin: 0.5,
        crouch_eye_dip: -13.0,
        crouch_eye_rate: 75.0,
        sprint_mul_default: 1.4,
    };

    /// Names accepted by [`MovementParams::set`] / [`MovementParams::get`] (the field names).
    pub const NAMES: [&'static str; 42] = [
        "gravity",
        "air_control",
        "stop_time",
        "stop_snap_speed",
        "base_accel",
        "start_accel",
        "max_accel",
        "accel_rate",
        "crouch_accel_mul",
        "walk_speed",
        "run_forward",
        "run_backward",
        "run_sideways",
        "crouch_speed",
        "ladder_move_speed",
        "ladder_max_up",
        "jump_speed",
        "hop_accel_mul",
        "hop_recover",
        "hop_min",
        "hop_decay",
        "knockback",
        "spectate_mul",
        "spectate_accel",
        "spectate_accel_run",
        "spectate_damping",
        "spectate_snap",
        "step_height",
        "step_margin",
        "friction_base",
        "standable_cos",
        "ground_cos",
        "dt_clamp",
        "dead_zone_sq",
        "min_move_sq",
        "frozen_fall_cap",
        "fall_probe_depth",
        "crouch_release_delay",
        "jump_box_margin",
        "crouch_eye_dip",
        "crouch_eye_rate",
        "sprint_mul_default",
    ];

    fn slot(&mut self, name: &str) -> Option<&mut f32> {
        Some(match name {
            "gravity" => &mut self.gravity,
            "air_control" => &mut self.air_control,
            "stop_time" => &mut self.stop_time,
            "stop_snap_speed" => &mut self.stop_snap_speed,
            "base_accel" => &mut self.base_accel,
            "start_accel" => &mut self.start_accel,
            "max_accel" => &mut self.max_accel,
            "accel_rate" => &mut self.accel_rate,
            "crouch_accel_mul" => &mut self.crouch_accel_mul,
            "walk_speed" => &mut self.walk_speed,
            "run_forward" => &mut self.run_forward,
            "run_backward" => &mut self.run_backward,
            "run_sideways" => &mut self.run_sideways,
            "crouch_speed" => &mut self.crouch_speed,
            "ladder_move_speed" => &mut self.ladder_move_speed,
            "ladder_max_up" => &mut self.ladder_max_up,
            "jump_speed" => &mut self.jump_speed,
            "hop_accel_mul" => &mut self.hop_accel_mul,
            "hop_recover" => &mut self.hop_recover,
            "hop_min" => &mut self.hop_min,
            "hop_decay" => &mut self.hop_decay,
            "spectate_mul" => &mut self.spectate_mul,
            "spectate_accel" => &mut self.spectate_accel,
            "spectate_accel_run" => &mut self.spectate_accel_run,
            "spectate_damping" => &mut self.spectate_damping,
            "spectate_snap" => &mut self.spectate_snap,
            "step_height" => &mut self.step_height,
            "step_margin" => &mut self.step_margin,
            "friction_base" => &mut self.friction_base,
            "standable_cos" => &mut self.standable_cos,
            "ground_cos" => &mut self.ground_cos,
            "dt_clamp" => &mut self.dt_clamp,
            "dead_zone_sq" => &mut self.dead_zone_sq,
            "min_move_sq" => &mut self.min_move_sq,
            "frozen_fall_cap" => &mut self.frozen_fall_cap,
            "fall_probe_depth" => &mut self.fall_probe_depth,
            "crouch_release_delay" => &mut self.crouch_release_delay,
            "jump_box_margin" => &mut self.jump_box_margin,
            "crouch_eye_dip" => &mut self.crouch_eye_dip,
            "crouch_eye_rate" => &mut self.crouch_eye_rate,
            "sprint_mul_default" => &mut self.sprint_mul_default,
            _ => return None,
        })
    }

    /// Overrides one tunable by name. `knockback` is truncated toward zero like the client's integer
    /// variable. Non-finite values are refused.
    pub fn set(&mut self, name: &str, value: f32) -> Result<(), ParamError> {
        if !value.is_finite() {
            return Err(ParamError::NotFinite);
        }
        if name == "knockback" {
            self.knockback = value as i32;
            return Ok(());
        }
        match self.slot(name) {
            Some(s) => {
                *s = value;
                Ok(())
            }
            None => Err(ParamError::Unknown),
        }
    }

    /// Reads one tunable by name.
    pub fn get(&self, name: &str) -> Option<f32> {
        if name == "knockback" {
            return Some(self.knockback as f32);
        }
        let mut copy = *self;
        copy.slot(name).map(|v| *v)
    }

    /// Applies `(name, value)` pairs in order; stops at the first error and reports its name.
    pub fn apply<'a, I>(&mut self, pairs: I) -> Result<(), (&'a str, ParamError)>
    where
        I: IntoIterator<Item = (&'a str, f32)>,
    {
        for (n, v) in pairs {
            self.set(n, v).map_err(|e| (n, e))?;
        }
        Ok(())
    }

    /// The engine's standable value as stored in its gravity record (negative).
    pub fn standable_record(&self) -> f32 {
        -self.standable_cos
    }

    /// Effective step for the stair move: `step_height + step_margin`.
    pub fn step(&self) -> f32 {
        self.step_height + self.step_margin
    }
}

impl Default for MovementParams {
    fn default() -> Self {
        MovementParams::DEFAULT
    }
}

/// Error of [`MovementParams::set`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamError {
    Unknown,
    NotFinite,
}

impl core::fmt::Display for ParamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParamError::Unknown => f.write_str("unknown movement parameter"),
            ParamError::NotFinite => f.write_str("movement parameter must be finite"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParamError {}

/// Per-player modifiers that come from the ROM's character, gear, weapon and item records
/// (MOVEMENT_2009.md §3). The caller computes them from the tables; defaults are the neutral values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedMods {
    /// Speed penalty fraction `p` (the sum of held-item, weapon, figure weight, item and gear terms minus the
    /// figure speed bonus); clamped to [-1, 1] at use. Target speed is `(1 - p) * s`.
    // SPEC: MOVEMENT_2009.md §7 step 5 (cshell+0x5c270)
    pub penalty: f32,
    /// Figure jump bonus (`fig_jump_bonus`).
    // SPEC: MOVEMENT_2009.md §3.1 (+0x28), §6.4
    pub jump_bonus: f32,
    /// Additive sprint terms `weapon_sprint_mod + gear_sprint_add + item_speed_mod_b + item_sprint_c`.
    // SPEC: MOVEMENT_2009.md §7 step 2 (sprint formula)
    pub sprint_add: f32,
    /// Figure sprint multiplier (`fig_sprint_mul`); `None` uses [`MovementParams::sprint_mul_default`].
    // SPEC: MOVEMENT_2009.md §3.1 (+0xe4, default 1.4 when missing)
    pub sprint_mul: Option<f32>,
    /// The sprint gauge rates of the character (`None`: no gauge rule, a sprint never runs out).
    // DISASM: cshell+0x1d33d0 (the inventory's max / drain / regen from the figure, head, goggle, body and backpack
    // rows); see `crate::stamina`
    pub stamina: Option<crate::stamina::StaminaRates>,
}

impl Default for SpeedMods {
    fn default() -> Self {
        SpeedMods {
            penalty: 0.0,
            jump_bonus: 0.0,
            sprint_add: 0.0,
            sprint_mul: None,
            stamina: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_spec_values() {
        let p = MovementParams::default();
        assert_eq!(p.gravity, -1100.0);
        assert_eq!(
            (
                p.walk_speed,
                p.run_forward,
                p.crouch_speed,
                p.ladder_move_speed
            ),
            (70.0, 285.0, 50.0, 200.0)
        );
        assert_eq!(p.jump_speed, 330.0);
        assert_eq!(
            (p.base_accel, p.start_accel, p.accel_rate),
            (3000.0, 500.0, 6000.0)
        );
        assert_eq!(p.air_control, 0.1);
        assert_eq!(p.step(), 30.5);
        assert_eq!(p.friction_base, 5.0);
        assert_eq!(p.dt_clamp, 0.1);
        // the engine's gravity-record value, bits 0xbf350481
        assert_eq!(p.standable_record().to_bits(), 0xbf35_0481);
    }

    #[test]
    fn every_name_round_trips() {
        let mut p = MovementParams::default();
        for (i, n) in MovementParams::NAMES.iter().enumerate() {
            let v = 1000.0 + i as f32;
            p.set(n, v).unwrap();
            assert_eq!(p.get(n), Some(v), "{n}");
        }
        assert_eq!(p.set("nope", 1.0), Err(ParamError::Unknown));
        assert_eq!(p.set("gravity", f32::NAN), Err(ParamError::NotFinite));
        assert_eq!(p.get("nope"), None);
    }

    #[test]
    fn apply_overrides() {
        let mut p = MovementParams::default();
        p.apply([("walk_speed", 90.0), ("knockback", 3.7)]).unwrap();
        assert_eq!(p.walk_speed, 90.0);
        assert_eq!(p.knockback, 3);
        assert_eq!(p.apply([("x", 1.0)]), Err(("x", ParamError::Unknown)));
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::*;

    #[test]
    fn partial_table_overrides_and_keeps_defaults() {
        let p: MovementParams =
            serde_json::from_str(r#"{"walk_speed": 90.0, "gravity": -800.0, "knockback": 2}"#)
                .unwrap();
        assert_eq!((p.walk_speed, p.gravity, p.knockback), (90.0, -800.0, 2));
        assert_eq!(
            p.run_forward, 285.0,
            "unnamed fields keep the 2009 defaults"
        );
        assert!(
            serde_json::from_str::<MovementParams>(r#"{"walkspeed": 1.0}"#).is_err(),
            "unknown keys refused"
        );
        let round: MovementParams =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(round, p);
    }
}
