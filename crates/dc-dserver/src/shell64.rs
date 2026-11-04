//! The client's player update (shell id 0x64 inside `C2S_SHELL_MSG`), parsed with the live layout of the derived
//! player class (cshell+0x5f7f0), whose two rotation blocks are `quat128`. Slice 2 takes the movement trailer
//! (sequence, position, velocity, time, ground state) as the player's reported state; meanings of the other fields
//! stay PLAUSIBLE until the capture V-B6.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1; UDP_SESSION_2009_B_addendum_disasm.md B15, B16, B18
// DC-DEFAULT: DC-C2S-2 (proposed) parse the derived (live) layout only

use dc_wire::udp::{read_quat128, BitReader, BitWriter};

/// `pu_flags` bit: view block `[yaw_c8:u8][v: 3 x f32]`.
// SPEC: UDP_SESSION_2009_B.md §10.3 (cshell+0x5f7f0)
pub const PU_VIEW8: u16 = 0x0020;
/// `pu_flags` bit: view block `[yaw_c16:u16]` (only when 0x0020 is clear).
// SPEC: UDP_SESSION_2009_B.md §10.3
pub const PU_VIEW16: u16 = 0x0040;
/// `pu_flags` bit: view block `[view_q:quat128]` (writer slot +0x4c = `quat128`).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1; UDP_SESSION_2009_B_addendum_disasm.md B16
pub const PU_VIEWQ: u16 = 0x0080;
/// `pu_flags` bit: `[offset: 3 x f32]`.
// SPEC: UDP_SESSION_2009_B.md §10.3
pub const PU_OFFSET: u16 = 0x0001;
/// `pu_flags` bit of the reliable 4-byte state form `[0x64][0x0010][state:u8]`.
// SPEC: UDP_SESSION_2009_B.md §10.3 (reliable form)
pub const PU_STATE_FORM: u16 = 0x0010;
/// `pu_flags` bit: `[aim_q:quat128]` (rotation built from the controller's pitch, yaw, roll).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1; UDP_SESSION_2009_B_addendum_disasm.md B16
pub const PU_ROT2: u16 = 0x0100;
/// `pu_flags` bit: `[value:f32]` (a weapon-related float).
// SPEC: UDP_SESSION_2009_B.md §10.3
pub const PU_WEAPON: u16 = 0x0200;
/// `pu_flags` bit: `[vec: 3 x f32]` (a tracked vector).
// SPEC: UDP_SESSION_2009_B.md §10.3
pub const PU_TRACKED: u16 = 0x0400;

/// Which of the two CShell builders the parser follows. Both exist in the 2009 shell and lay out the same flags
/// differently. The live player object is of the derived class, whose builder is cshell+0x5f7f0 (variant A); the
/// base-class form B is kept for experiments only.
// SPEC: UDP_SESSION_2009_B.md §10.3 (cshell+0x5f7f0, cshell+0x1fa060); UDP_SESSION_2009_B_addendum_disasm.md B15, B17
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builder {
    /// cshell+0x5f7f0: raw floats.
    A,
    /// cshell+0x1fa060: compressed view / offset fields.
    B,
}

/// The builder the server parses (the derived class's, DISASM grade; a capture V-B6 confirms it).
// SPEC: UDP_SESSION_2009_B_addendum_disasm.md B15 (DISASM)
// DC-DEFAULT: DC-C2S-2 (proposed)
pub const TODO_SHELL64_BUILDER: Builder = Builder::A;

/// Trailing (movement) block of the update: `[a:u8][b: 96 bits][c: 96 bits][t:f32][d:u8][e:u8][f:u64]`, exactly 312
/// bits. MOVEMENT_2009 names the fields: `a` = the sequence of the last teleport (echoed), `b` = body position,
/// `c` = body velocity, `t` = client time in seconds, `d` = on ground, `e` = surface type, `f` = standing ids.
// SPEC: UDP_SESSION_2009_B_addendum_disasm.md B18 (cshell+0x1fed20, complete); MOVEMENT_2009.md §12.2
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trailer {
    pub a: u8,
    pub vec_b: [f32; 3],
    pub vec_c: [f32; 3],
    /// PLAUSIBLY the client time (engine client interface slot +0x6c).
    pub t: f32,
    pub d: u8,
    pub e: u8,
    pub f: u64,
}

impl Trailer {
    /// The `seq` of the last `GS_MOVER_UPDATE` teleport the client applied.
    // SPEC: MOVEMENT_2009.md §12.2 (`move_seq`, mover+0x04), §12.4
    pub fn move_seq(&self) -> u8 {
        self.a
    }

    /// Body position (raw floats).
    // SPEC: MOVEMENT_2009.md §12.2
    pub fn pos(&self) -> [f32; 3] {
        self.vec_b
    }

    /// Body velocity (raw floats; zero while the client shell is paused).
    // SPEC: MOVEMENT_2009.md §12.2
    pub fn vel(&self) -> [f32; 3] {
        self.vec_c
    }

    /// Client time in seconds.
    // SPEC: MOVEMENT_2009.md §12.2
    pub fn time(&self) -> f32 {
        self.t
    }

    /// Whether the body stands on the ground (mover+0x60).
    // SPEC: MOVEMENT_2009.md §12.2
    pub fn on_ground(&self) -> bool {
        self.d != 0
    }

    /// Surface type under the feet (mover+0x50): 0 in the air, 200 on a ladder.
    // SPEC: MOVEMENT_2009.md §10.2, §12.2
    pub fn surface(&self) -> u8 {
        self.e
    }
}

/// A parsed player update.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shell64Update {
    pub flags: u16,
    /// The 4-byte reliable state-change form.
    pub state_form: Option<u8>,
    pub yaw_c8: Option<u8>,
    pub view: Option<[f32; 3]>,
    pub yaw_c16: Option<u16>,
    /// View rotation of flag 0x0080.
    pub view_q: Option<[f32; 4]>,
    pub wide: Option<[u32; 6]>,
    pub offset: Option<[f32; 3]>,
    /// Aim rotation of flag 0x0100.
    pub aim_q: Option<[f32; 4]>,
    pub weapon: Option<f32>,
    pub tracked: Option<[f32; 3]>,
    /// The frame's mover control word (`move_mgr`+0x24: FWD 0x1, BACK 0x2, JUMP 0x10, CROUCH 0x20, ...).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (`control`); MOVEMENT_2009.md §5.1
    pub state: Option<u32>,
    pub trailer: Option<Trailer>,
    /// False when the message was short (or, for the base-class form, stopped at a field it lacks).
    pub complete: bool,
}

/// Scale of `yaw8c` (254 / 2π) and of `yaw16c` (32767 / 2π).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (cshell+0x19f390, cshell+0x19f3e0)
pub const YAW8C_SCALE: f32 = 40.425_354;
pub const YAW16C_SCALE: f32 = 5215.03;

/// Which field the server takes as the player's position until V-B6 decides it. The trailing block's first
/// vector is the only absolute 96-bit vector present in every update of builder A (`offset` is a difference of two
/// positions and only sent with flag 0x0001).
// SPEC: UDP_SESSION_2009_B.md §10.3, §10.4 (meanings UNPROVEN)
pub const TODO_POSITION_IS_TRAILER_VEC_B: bool = true;

impl Shell64Update {
    /// The mover control word (see [`Shell64Update::state`]).
    pub fn control(&self) -> Option<u32> {
        self.state
    }

    /// Yaw in radians from whichever view block the update carries: the body angles `v` (pitch, yaw, roll), the
    /// 8- or 16-bit yaw (`round(yaw * 40.425354)` / `round(yaw * 5215.03)`) or the view quaternion.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (yaw8c / yaw16c scales; angle triples of MOVEMENT_2009.md §12.4)
    pub fn yaw(&self) -> Option<f32> {
        if let Some(v) = self.view {
            return Some(v[1]);
        }
        if let Some(y) = self.yaw_c8 {
            return Some(y as i8 as f32 / YAW8C_SCALE);
        }
        if let Some(y) = self.yaw_c16 {
            return Some(y as i16 as f32 / YAW16C_SCALE);
        }
        self.view_q.map(dc_wire::udp::quat_yaw)
    }

    /// The position candidate (see [`TODO_POSITION_IS_TRAILER_VEC_B`]).
    pub fn position(&self) -> Option<[f32; 3]> {
        if TODO_POSITION_IS_TRAILER_VEC_B {
            self.trailer.map(|t| t.vec_b)
        } else {
            self.offset
        }
    }
}

fn vec3(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

/// Parses the bits after the shell id 0x64. Never fails: fields that are missing or behind an UNPROVEN encoding
/// stay `None` and `complete` is false.
// SPEC: UDP_SESSION_2009_B.md §10.3 (builder A field order PROVEN in decomp_sig widths), §10.5 (never trusted)
pub fn parse_shell64(body: &BitWriter) -> Shell64Update {
    let mut r = body.reader();
    let mut u = Shell64Update::default();
    if r.remaining_bits() < 16 {
        return u;
    }
    u.flags = r.read_u16();
    if u.flags == PU_STATE_FORM && r.remaining_bits() == 8 {
        u.state_form = Some(r.read_u8());
        u.complete = true;
        return u;
    }
    let f = u.flags;
    let builder = TODO_SHELL64_BUILDER;
    let need = |r: &BitReader<'_>, n: usize| r.remaining_bits() >= n;
    if f & PU_VIEW8 != 0 {
        if !need(&r, 8) {
            return u;
        }
        u.yaw_c8 = Some(r.read_u8());
        if builder == Builder::A {
            if !need(&r, 96) {
                return u;
            }
            u.view = Some(vec3(&mut r));
        }
    } else if f & PU_VIEW16 != 0 {
        if !need(&r, 16) {
            return u;
        }
        u.yaw_c16 = Some(r.read_u16());
    } else if f & PU_VIEWQ != 0 {
        if !need(&r, 128) {
            return u;
        }
        u.view_q = Some(read_quat128(&mut r));
    }
    if f & (PU_VIEW8 | PU_VIEW16) != 0 {
        match builder {
            Builder::A => {
                if !need(&r, 192) {
                    return u;
                }
                let mut w = [0u32; 6];
                for x in &mut w {
                    *x = r.read_u32();
                }
                u.wide = Some(w);
            }
            Builder::B => {
                if !need(&r, 8) {
                    return u;
                }
                r.skip_bits(8);
            }
        }
    }
    if f & PU_OFFSET != 0 {
        match builder {
            Builder::A => {
                if !need(&r, 96) {
                    return u;
                }
                u.offset = Some(vec3(&mut r));
            }
            Builder::B => {
                if !need(&r, 48) {
                    return u;
                }
                let s = |r: &mut BitReader<'_>| r.read_u16() as i16 as f32 / 100.0;
                u.offset = Some([s(&mut r), s(&mut r), s(&mut r)]);
            }
        }
    }
    if builder == Builder::A {
        if f & PU_ROT2 != 0 {
            if !need(&r, 128) {
                return u;
            }
            u.aim_q = Some(read_quat128(&mut r));
        }
        if f & PU_WEAPON != 0 {
            if !need(&r, 32) {
                return u;
            }
            u.weapon = Some(r.read_f32());
        }
        if f & PU_TRACKED != 0 {
            if !need(&r, 96) {
                return u;
            }
            u.tracked = Some(vec3(&mut r));
        }
    }
    if !need(&r, 32) {
        return u;
    }
    u.state = Some(r.read_u32());
    if !need(&r, 8 + 96 + 96 + 32 + 8 + 8 + 64) {
        return u;
    }
    u.trailer = Some(Trailer {
        a: r.read_u8(),
        vec_b: vec3(&mut r),
        vec_c: vec3(&mut r),
        t: r.read_f32(),
        d: r.read_u8(),
        e: r.read_u8(),
        f: r.read_u64(),
    });
    u.complete = true;
    u
}

/// Builds a builder-A update with a view block, a position in the trailing block and the given state word (for
/// tests and the synthetic client; the trailing fields the spec does not show are not written).
pub fn build_shell64(yaw_c8: u8, view: [f32; 3], pos: [f32; 3], state: u32, t: f32) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u16(PU_VIEW8).write_u8(yaw_c8);
    for c in view {
        w.write_f32(c);
    }
    for c in view.iter().chain(view.iter()) {
        w.write_f32(*c);
    }
    w.write_u32(state);
    w.write_u8(0);
    for c in pos {
        w.write_f32(c);
    }
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    w.write_f32(t).write_u8(0).write_u8(0).write_u64(0);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_a_round_trip() {
        let b = build_shell64(12, [0.0, 1.5, 0.0], [10.0, 20.0, -30.0], 0x20, 4.25);
        let u = parse_shell64(&b);
        assert!(u.complete, "{u:?}");
        assert_eq!(u.flags, PU_VIEW8);
        assert_eq!(u.yaw_c8, Some(12));
        assert_eq!(u.view, Some([0.0, 1.5, 0.0]));
        assert!(u.wide.is_some());
        assert_eq!(u.state, Some(0x20));
        assert_eq!(u.position(), Some([10.0, 20.0, -30.0]));
        assert_eq!(u.trailer.unwrap().t, 4.25);
    }

    #[test]
    fn reliable_state_form() {
        let u = parse_shell64(&BitWriter::from_bytes(&[0x10, 0x00, 0x05]));
        assert_eq!(u.state_form, Some(5));
        assert!(u.complete);
    }

    #[test]
    fn quat_blocks_of_the_live_layout() {
        let mut w = BitWriter::new();
        w.write_u16(PU_VIEWQ | PU_ROT2);
        for c in [0.0, 0.5, 0.0, 0.5] {
            w.write_f32(c);
        }
        for c in [0.0, 0.0, 0.0, 1.0] {
            w.write_f32(c);
        }
        w.write_u32(0x21);
        w.write_u8(3);
        for c in [1.0, 2.0, 3.0, 0.0, 0.0, 0.0] {
            w.write_f32(c);
        }
        w.write_f32(9.0).write_u8(1).write_u8(0).write_u64(u64::MAX);
        let u = parse_shell64(&w);
        assert!(u.complete, "{u:?}");
        assert_eq!(u.view_q, Some([0.0, 0.5, 0.0, 0.5]));
        assert_eq!(u.aim_q, Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(u.control(), Some(0x21));
        let t = u.trailer.unwrap();
        assert_eq!(
            (t.move_seq(), t.pos(), t.time(), t.on_ground()),
            (3, [1.0, 2.0, 3.0], 9.0, true)
        );
        assert!((u.yaw().unwrap() - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
    }

    #[test]
    fn unproven_encodings_and_short_messages_stop_cleanly() {
        let u = parse_shell64(&BitWriter::from_bytes(&[0x80, 0x00, 1, 2, 3]));
        assert!(!u.complete && u.flags == PU_VIEWQ);
        let u = parse_shell64(&BitWriter::from_bytes(&[0x20]));
        assert!(!u.complete && u.flags == 0);
        let u = parse_shell64(&BitWriter::from_bytes(&[0x20, 0x00, 7]));
        assert_eq!(u.yaw_c8, Some(7));
        assert!(!u.complete && u.view.is_none());
    }
}
