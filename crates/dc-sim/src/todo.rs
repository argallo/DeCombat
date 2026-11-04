//! Named stand-ins for the items MOVEMENT_2009.md marks UNPROVEN (§16.1) or leaves unread. Each constant is
//! the choice this crate makes until a capture or a further read settles it; grep for `todo::` to find every
//! use. Changing one changes behaviour on both server and client (they share this crate).
// SPEC: MOVEMENT_2009.md §16.1 (open items), §16.2 (validation plan)

// (Retired by CLI-1607: mover+0x84, the jump-latch enable, is `PlayerState::jump_enable` in `player.rs`, set when
// the player state turns alive and cleared at death; EVIDENCE cshell+0x1f97d0, DISASM cshell+0x61870.)

// (Retired: the "equipment-state check" of the jump request is the jump-key-held flag, now implemented as
// `PlayerState::jump_held` in `player.rs`; SPEC: MOVEMENT_2009.md §5.3, DISASM cshell+0x1ffad3..0x1ffb75.)

/// mover+0x324 (the "already tall" height of the stand-up test) has no writer in the code read. We use
/// `stand_dims.y - STANDUP_TALL_MARGIN`, the same margin the jump request uses for "not in a crouched box".
// SPEC: MOVEMENT_2009.md §5.4, §16.1 item 2 (UNPROVEN source)
pub const STANDUP_TALL_MARGIN: f32 = 0.5;

/// The caller of slide-to-stop (cshell+0x1fe940) was not found; we run it once per step after the
/// velocity law, before the physics step.
// SPEC: MOVEMENT_2009.md §6.8, §16.1 item 1 (UNPROVEN that it runs every frame)
pub const SLIDE_TO_STOP_EVERY_STEP: bool = true;

/// Keyboard turn bits 0x04 / 0x08 have no writer in the mover; keyboard turning stays inactive.
// SPEC: MOVEMENT_2009.md §5.1, §13.2 step 4, §16.1 item 2 (UNPROVEN)
pub const KEYBOARD_TURN_BITS_WRITTEN: bool = false;

/// The crouched collision box is not set by any client code found; PLAUSIBLY the server sends it with the
/// 0xa0 / 0x20 correction. Without a crouch box (`PlayerState::crouch_dims` unset) this crate never shrinks the
/// box on its own: dims change only through [`crate::PlayerState::apply_correction`] or
/// [`crate::PlayerState::set_box`]. With one, the stance switches between the two boxes (SIM-25).
// SPEC: MOVEMENT_2009.md §9.2, §16.1 item 3 (UNPROVEN, capture item V-M7)
pub const CROUCH_BOX_FROM_SERVER: bool = true;

/// The blocker-polygon test (engine+0xc5660 -> engine+0xc4bf0: a capsule against the player-clip soup, an
/// iterated distance solver of at most 10 steps per polygon) is not ported. This crate tests blocker polygons
/// with the same 2009 polygon sweep as BSP polygons (engine+0x77530 / engine+0x76120). Known differences: the
/// capsule test also takes moves parallel to the face, and a blocker hit carries no polygon in the engine, so
/// its ground probe (engine+0x79210, object 0) does not stand the body on a blocker; here it does.
// SPEC: MOVEMENT_2009.md §9.6 stage 1, §16.1 item 4
// DC-DEFAULT: SIM-7 (proposed) blocker polygons are swept as cylinder faces under the 2009 polygon rule
pub const BLOCKERS_AS_CYLINDER_FACES: bool = true;

/// The polygon bounding-sphere pre-filter's line parameter is written in the client as
/// `dot(c, d) / (|d|^2 |c|)`; this crate uses the exact closest point of the segment instead, which never
/// rejects a polygon the sweep can reach (the as-written form can, for long moves).
// SPEC: MOVEMENT_2009.md §9.9 step 1 (PROVEN as written; deliberate deviation)
pub const SPHERE_PREFILTER_EXACT: bool = true;

/// The Euler convention of the client's rotation builder (cshell+0x1b700) is not fully decoded; this crate
/// builds the camera rotation as yaw about +Y applied after pitch about +X, positive pitch looking down.
// SPEC: MOVEMENT_2009.md §13.2 step 7, §16.1 item 10 (PLAUSIBLE)
pub const EULER_YAW_THEN_PITCH: bool = true;

/// mover+0x358 (added to the spectator acceleration magnitude) has no known writer; 0 here.
// SPEC: MOVEMENT_2009.md §6.6 (spectating: normalize(acc) * (mover+0x358 + 300000))
pub const SPECTATE_EXTRA_ACCEL: f32 = 0.0;

/// The ground probe of cshell+0x5e3f0 used by slide-to-stop was not read; this crate uses `on_ground`.
// SPEC: MOVEMENT_2009.md §6.8 (probe cshell+0x5e3f0)
pub const SLIDE_TO_STOP_PROBE_IS_ON_GROUND: bool = true;

/// Pushers (radial impulses, §11.5) are not implemented: the strength operand is partly lost and their
/// source was not traced.
// SPEC: MOVEMENT_2009.md §11.5 (strength operand partly lost; who adds pushes not traced)
pub const PUSHERS_IMPLEMENTED: bool = false;

/// The stuck rule (§10.3) depends on a special standing-object kind (cshell+0x5f480) that was not read; it
/// is not implemented.
// SPEC: MOVEMENT_2009.md §10.3 (PLAUSIBLE purpose)
pub const STUCK_RULE_IMPLEMENTED: bool = false;

/// The standing-info flag 0x1000 that clears `on_ground` has no known meaning (it is not the surface flag
/// word: most ROM floors carry surface bit 0x1000); this crate never sets it.
// SPEC: MOVEMENT_2009.md §10.2 (standing flags 0x1000)
pub const STANDING_FLAG_1000_KNOWN: bool = false;

/// Riders and the touch pass (engine+0x71020, engine+0x79450) are not modelled: world geometry is static.
// SPEC: MOVEMENT_2009.md §9.3 step 4-5, §9.4 touch pass, §16.1 item 4
pub const RIDERS_AND_TOUCH_PASS: bool = false;
