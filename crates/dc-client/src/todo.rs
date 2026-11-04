//! Named stand-ins for what the client needs but the specs mark UNPROVEN or do not cover yet. Each constant is
//! the choice this slice makes until a capture or a further read settles it; grep for `todo::` to find every
//! use. None of them is a ROM value.

/// Eye height above the body centre, as a fraction of the body's half height. The 2009 camera helper adds
/// `eye_h` (helper+0xc8) to its target's position, but no writer of that field was found. The 2009 client's own
/// 0x64 updates settle the value on the wire: `eye_offset` = (0, 33, 0) standing and (0, 20, 0) crouched above the
/// body position, with the stand box half height 50 in force, i.e. 0.66 of the half height (exactly 33.0 for the
/// ROM stand box) and the crouch dip of -13 (MOVEMENT_2009 §13.1) on top. Proposed revision CLI-3' (0.8 before),
/// staged; replace by the model's eye socket once dc-model exposes it.
// SPEC: MOVEMENT_2009.md §13.4 first bullet (eye_h = helper+0xc8, no writer found), §16.2 V-M9
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (g) (2,236 updates at +33 standing, 52 at +20 crouched)
pub const EYE_ABOVE_CENTRE_FRACTION: f32 = 0.66;

/// The forward eye offset `fwd_off` (helper+0xc4) has no writer either; kept at its initial value 0.
// SPEC: MOVEMENT_2009.md §13.4 first bullet (fwd_off = helper+0xc4, initialised to 0)
pub const EYE_FORWARD_OFFSET: f32 = 0.0;

/// How the engine applies the mouse axis scale that CShell computes (`sensitivity * mul + base`, see
/// [`crate::look`]) is not read: we take the scale as radians per raw mouse count (one DirectInput relative
/// unit = one winit raw-motion unit) and add it to the view angles unchanged, as MOVEMENT_2009 §13.2 step 2 says
/// CShell does with the axis value it receives.
// SPEC: MOVEMENT_2009.md §13.2 step 2 ("any user sensitivity factor is applied inside the engine's input layer
// (UNPROVEN where)"), §16.1 item 9
pub const MOUSE_SCALE_IS_RADIANS_PER_COUNT: bool = true;

/// The adaptive small-motion mouse curve passes an argument the decompiler did not show; PLAUSIBLY
/// `|yaw_d| / ADAPTIVE_MAX`. The curve is off by default, so this only matters when a user turns it on.
// SPEC: MOVEMENT_2009.md §13.2 step 3 (argument not visible, PLAUSIBLE)
pub const ADAPTIVE_ARG_IS_RATIO: bool = true;

/// Head bob (`y += bob * scale`, cshell+0x2b0780) is not drawn: its amplitude source (the console variable at
/// handle VA 0x375b47e4) is UNPROVEN.
// SPEC: MOVEMENT_2009.md §13.4 third bullet, §16.1 item 6
pub const HEAD_BOB: bool = false;

/// Camera wave effects other than the hit jar (MOVEMENT_2009 §13.4: the landing dip / roll / pitch waves and the
/// rest of the 30 wave slots) are not drawn. The recoil with its kick-back ([`crate::recoil`]) and the hit jar
/// ([`crate::camfx`]) are.
// SPEC: MOVEMENT_2009.md §13.3, §13.4 (camera effect sum, kick-back)
pub const CAMERA_EFFECTS: bool = false;
