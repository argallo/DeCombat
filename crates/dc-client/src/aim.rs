//! The upper-body aim of a remote character: where its player points the view (pitch, and the free-look yaw
//! offset) bends the torso, neck, head and shoulders of the drawn body.
//!
//! The object stream (id 10) carries only the body yaw (`yaw8`) and the trackers; the 2009 client takes the aim of
//! another player from the object notes shell 0xa6 `[0x18][obj]` sub 0x80 / 0xc0 (six f32: pitch, yaw offset, their
//! rates and accelerations; 0xc0 adds a u32 flag word, sub 0x40 is the flag word alone). The player character's
//! client object (built for type 0x18 by the client-object factory of cshell+0x66c10, constructor cshell+0x28a30)
//! stores them, eases its drawn angles toward them every frame and turns seven nodes of the body by a share of the
//! angles each.
// EVIDENCE: cshell+0x66c4b (factory: type 0x18 builds the 0x7f0-byte object of constructor cshell+0x28a30, whose
// message slot +0x30 is cshell+0x26f60); DISASM: cshell+0x26f60 (reads the sub byte after the base reader
// cshell+0x206260: 0x80 -> cshell+0x259b0 and the "update now" byte +0x7dc, 0xc0 -> cshell+0x259b0 then a u32 to
// +0x7b0, 0x40 -> a u32 to +0x7b0); EVIDENCE: cshell+0x259b0 (six f32 to +0x78..+0x8c)
// SPEC correction: GAME_MESSAGES_2009_S2C.md §8.3 lists 0x40 / 0x80 / 0xc0 as not handled; that is true of the
// base class (cshell+0x206260) only, the class the 2009 client builds for a player character handles them.

use dc_render::Skeleton;
use glam::{Mat4, Quat, Vec3};

/// Note type of a character object (shell 0xa6 `note_kind`).
pub const NOTE_CHARACTER: u8 = 0x18;
/// Aim note: six f32, and draw the change at once.
pub const SUB_AIM: u8 = 0x80;
/// Aim note with the movement flag word.
pub const SUB_AIM_FLAGS: u8 = 0xc0;

/// The aim the server last sent for a character (the 2009 object's +0x78..+0x8c).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AimNote {
    /// View pitch, radians, positive looks down.
    pub pitch: f32,
    /// Free-look yaw offset from the body yaw, radians.
    pub yaw: f32,
    pub pitch_rate: f32,
    pub yaw_rate: f32,
    pub pitch_accel: f32,
    pub yaw_accel: f32,
}

/// Reads shell 0xa6 `[0x18][obj:u16][sub:u8]` with sub 0x80 or 0xc0: the character object and its aim.
// EVIDENCE: cshell+0x259b0 (order pitch, yaw, pitch rate, yaw rate, pitch accel, yaw accel; 32 bits each)
pub fn read_aim_note(body: &[u8], bits: usize) -> Option<(u16, AimNote)> {
    let mut r = dc_wire::udp::BitReader::with_bit_len(body, bits);
    if r.remaining_bits() < 32 + 6 * 32 || r.read_u8() != NOTE_CHARACTER {
        return None;
    }
    let obj = r.read_u16();
    let sub = r.read_u8();
    if sub != SUB_AIM && sub != SUB_AIM_FLAGS {
        return None;
    }
    let mut f = [0f32; 6];
    for v in &mut f {
        *v = r.read_f32();
    }
    if !f.iter().all(|v| v.is_finite()) {
        return None;
    }
    Some((
        obj,
        AimNote {
            pitch: f[0],
            yaw: f[1],
            pitch_rate: f[2],
            yaw_rate: f[3],
            pitch_accel: f[4],
            yaw_accel: f[5],
        },
    ))
}

/// The 2009 frame the easing step was written for.
// DC-DEFAULT: CLI-569 (proposed) the 2009 easing runs once per drawn frame (it halves the distance to the target
// each frame); DeCombat runs it in fixed 1/60 s steps, so the drawn aim does not depend on the frame rate
pub const STEP_S: f32 = 1.0 / 60.0;
/// Steps run at most per update (a long stall snaps to the target instead of replaying every step).
const MAX_STEPS: u32 = 120;

/// The drawn aim of one character, eased toward the last note.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AimState {
    pub target: AimNote,
    /// Drawn pitch and yaw offset (the 2009 object's +0x5e0 / +0x5e4).
    pub pitch: f32,
    pub yaw: f32,
    /// Their eased rates (+0x5e8 / +0x5ec).
    pub pitch_vel: f32,
    pub yaw_vel: f32,
    /// Seconds not yet stepped.
    carry: f32,
}

impl AimState {
    /// A new note: the target changes, the drawn angles ease toward it from where they are.
    pub fn on_note(&mut self, n: AimNote) {
        self.target = n;
    }

    /// One 2009 easing step of `dt` seconds: the rate moves halfway to `rate + dt * accel`, the angle halfway to
    /// `target + dt * rate`.
    // EVIDENCE: cshell+0x256f0 (vel = (dt * accel + vel + rate) / 2; angle = (vel * dt + angle + target) / 2, for
    // pitch and yaw alike)
    pub fn step_once(&mut self, dt: f32) {
        let t = self.target;
        self.pitch_vel = (dt * t.pitch_accel + self.pitch_vel + t.pitch_rate) * 0.5;
        self.pitch = (self.pitch_vel * dt + self.pitch + t.pitch) * 0.5;
        self.yaw_vel = (dt * t.yaw_accel + self.yaw_vel + t.yaw_rate) * 0.5;
        self.yaw = (self.yaw_vel * dt + self.yaw + t.yaw) * 0.5;
    }

    /// Advances by `dt_s` seconds in [`STEP_S`] steps (CLI-569).
    pub fn advance(&mut self, dt_s: f32) {
        if !dt_s.is_finite() || dt_s <= 0.0 {
            return;
        }
        self.carry += dt_s;
        let mut n = 0;
        while self.carry >= STEP_S {
            self.carry -= STEP_S;
            n += 1;
            if n > MAX_STEPS {
                self.carry = 0.0;
                self.pitch = self.target.pitch;
                self.yaw = self.target.yaw;
                return;
            }
            self.step_once(STEP_S);
        }
    }

    /// The drawn (pitch, yaw offset), the pitch held within the node limits and the yaw offset within
    /// [`YAW_LIMIT`] (the rest turns the body, [`AimState::body_follow`]).
    pub fn drawn(&self) -> [f32; 2] {
        [clamp_pitch(self.pitch), split_yaw(self.yaw).0]
    }

    /// The turn added to the stream's body yaw: the part of the drawn yaw offset beyond [`YAW_LIMIT`].
    pub fn body_follow(&self) -> f32 {
        split_yaw(self.yaw).1
    }
}

/// The largest yaw offset between the view and the body: past it the body turns with the view.
// DISASM: cshell+0x5ecc0 / cshell+0x5cd70 (the 2009 player: offset +0x410 += the view's yaw change, held within half
// of two console angles in degrees (x pi/180 x 0.5); body yaw +0x6c = view yaw +0x408 - offset; while moving
// (flags 0x80 / 0x100 / 1 / 2) the offset is 0)
// DISASM: cshell+0x5e6f2..0x5e720 (the two console angles "PlayerTwistYawMax" / "PlayerTwistYawMin" registered
// with the defaults 90.0 / -90.0; no ROM config file sets them), so the limit is 90 x pi/180 x 0.5 = pi/4
// OBSERVED: remote-aim part 2: the 2009-like client's 0x64 aux[1] stays within +-0.79
pub const YAW_LIMIT: f32 = TWIST_YAW_MAX_DEG * std::f32::consts::PI / 180.0 * 0.5;

/// The smallest yaw offset (negative): half the "PlayerTwistYawMin" angle.
// DISASM: cshell+0x5ecc0 (the min angle x pi/180 x 0.5 is the clamp of a negative change, cshell+0x5cd70)
pub const YAW_LIMIT_MIN: f32 = TWIST_YAW_MIN_DEG * std::f32::consts::PI / 180.0 * 0.5;

/// Default of the console angle holding the largest twist (degrees); the offset limit is half of it.
// DISASM: cshell+0x5e6f2 (push 0x42b40000 = 90.0 for "PlayerTwistYawMax")
pub const TWIST_YAW_MAX_DEG: f32 = 90.0;
/// Default of the console angle holding the smallest twist (degrees).
// DISASM: cshell+0x5e70e (push 0xc2b40000 = -90.0 for "PlayerTwistYawMin")
pub const TWIST_YAW_MIN_DEG: f32 = -90.0;

/// Splits a yaw offset into the part the upper body twists (within [`YAW_LIMIT`]) and the part the whole body
/// turns: the 2009 body is dragged by the view once the offset reaches the limit, and stays put within it.
// DISASM: cshell+0x5cd70 (the offset clamped at the limit; the body yaw is the view yaw minus the offset)
// DC-DEFAULT: CLI-634 (proposed) the remote body applies the sender's rule to the offset of the aim note (a sender
// that keeps the rule never sends more than the limit; one that does not gets the 2009 look)
pub fn split_yaw(offset: f32) -> (f32, f32) {
    let twist = offset.clamp(-YAW_LIMIT, YAW_LIMIT);
    (twist, offset - twist)
}

/// The own player's view yaw offset (the sender side of the aim note): while standing the view turns alone and the
/// offset collects its yaw change, held within the twist limits; past them the body is dragged along; a movement key
/// returns it to 0. The body yaw sent and drawn is the view yaw minus the offset; the offset travels as 0x64 aux[1].
// DISASM: cshell+0x5ecc0 (per frame: the mouse yaw change goes to cshell+0x5cd70 with the console angles x pi/180 x
// 0.5; then control bits 0x80 / 0x100 / 0x01 / 0x02 set the offset +0x410 to 0; body yaw +0x6c = view yaw +0x408 -
// offset), cshell+0x5cd70 (with none of the control bits 0xc0 / 0x100: offset += change; a positive change clamps
// at the max, a negative one at the min)
// DC-DEFAULT: CLI-636 (proposed) the change is the frame's view yaw difference (mouse, recoil and server sets alike,
// wrapped to +-pi); a server view set (0x6a) zeroes the offset as the 2009 handler does (cshell+0x5cca0)
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OwnTwist {
    offset: f32,
    last_yaw: Option<f32>,
}

impl OwnTwist {
    /// Advances by one frame with the view yaw after this frame's input and the frame's control word; returns the
    /// offset.
    pub fn step(&mut self, view_yaw: f32, controls: u32) -> f32 {
        use dc_sim::player::bits;
        let change = match self.last_yaw {
            Some(last) if view_yaw.is_finite() => crate::look::wrap_angle(view_yaw - last),
            _ => 0.0,
        };
        if view_yaw.is_finite() {
            self.last_yaw = Some(view_yaw);
        }
        let (max, min) = (YAW_LIMIT, YAW_LIMIT_MIN);
        if controls & (bits::STRAFE_MOD | bits::LEFT | bits::RIGHT) == 0 {
            let o = self.offset + change;
            self.offset = if change > 0.0 && o > max {
                max
            } else if change < 0.0 && o < min {
                min
            } else {
                o
            };
        }
        if controls & (bits::LEFT | bits::RIGHT | bits::FWD | bits::BACK) != 0 {
            self.offset = 0.0;
        }
        self.offset
    }

    /// The server set the view (0x6a): the offset is 0 and the next change counts from `view_yaw`.
    pub fn reset(&mut self, view_yaw: f32) {
        self.offset = 0.0;
        self.last_yaw = view_yaw.is_finite().then_some(view_yaw);
    }

    /// The current offset.
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// The body yaw for `view_yaw`: the view yaw minus the offset (not wrapped, as the 2009 player's +0x6c).
    pub fn body_yaw(&self, view_yaw: f32) -> f32 {
        view_yaw - self.offset
    }
}

/// Pitch limit of the controlled nodes: +-pi/2.
// EVIDENCE: cshell+0x24660 (each torso, pelvis and shoulder controller gets the limits 0x3fc90fdb / 0xbfc90fdb)
// DC-DEFAULT: CLI-570 (proposed) the 2009 code also clamps to half a console angle (cshell+0x24340, the variable
// is not identified); DeCombat uses the node limit alone
pub fn clamp_pitch(p: f32) -> f32 {
    p.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
}

/// One controlled node: its name (compared without case; the second name when the first is missing) and its
/// shares of the yaw offset and of the pitch looking up / down.
#[derive(Debug, Clone, Copy)]
pub struct NodeShare {
    pub names: [&'static str; 2],
    pub yaw: f32,
    pub pitch_up: f32,
    pub pitch_down: f32,
}

/// The 2009 console defaults: yaw upper 0.588 / lower 0.412 / pelvis 0 (shares of their sum), pitch up upper 0.15,
/// lower 0.15, neck 0.25, shoulders 0.45 (divided by their sum, 1.0), down upper 0.45, lower 0.35, neck 0.1,
/// shoulders 0.1 (divided by the same up sum), head 0.5 up / 0.35 down (not divided).
// EVIDENCE: cshell+0x25a50 (the ratio variables and their defaults 0x3f169697, 0x3ed2d2d3, 0, 0x3e19999a,
// 0x3ee66666, 0x3e19999a, 0x3eb33333, 0x3e800000, 0x3dcccccd, 0x3f000000, 0x3eb33333, 0x3ee66666, 0x3dcccccd),
// cshell+0x24660 (the shares), cshell+0x266c0 / cshell+0x271c0 (the seven nodes by name)
pub const NODES: [NodeShare; 7] = [
    NodeShare {
        names: ["Right_pelvis", ""],
        yaw: 0.0,
        pitch_up: 0.0,
        pitch_down: 0.0,
    },
    NodeShare {
        names: ["Torso", ""],
        yaw: 0.411_764_7,
        pitch_up: 0.15,
        pitch_down: 0.35,
    },
    NodeShare {
        names: ["Upper_torso", ""],
        yaw: 0.588_235_3,
        pitch_up: 0.15,
        pitch_down: 0.45,
    },
    NodeShare {
        names: ["Neck", "Bip01 Neck"],
        yaw: 0.0,
        pitch_up: 0.25,
        pitch_down: 0.1,
    },
    NodeShare {
        names: ["Head", "Bip01 Head"],
        yaw: 0.0,
        pitch_up: 0.5,
        pitch_down: 0.35,
    },
    NodeShare {
        names: ["Left_shoulder", ""],
        yaw: 0.0,
        pitch_up: 0.45,
        pitch_down: 0.1,
    },
    NodeShare {
        names: ["Right_shoulder", "Bip01 R Clavicle"],
        yaw: 0.0,
        pitch_up: 0.45,
        pitch_down: 0.1,
    },
];

/// The node of `sk` for a share entry, if the model has it.
pub fn node_of(sk: &Skeleton, n: &NodeShare) -> Option<usize> {
    n.names.iter().filter(|s| !s.is_empty()).find_map(|s| {
        let up = s.as_bytes().to_ascii_uppercase();
        sk.names.iter().position(|m| *m == up)
    })
}

/// The model-space turn of one controlled node: its yaw share about +Y, then its pitch share about +X (positive
/// pitch tips the model's forward +Z down), the up or down share by the pitch's sign.
// DISASM: cshell+0x263f0 (the node callback: the pitch input times the share at +0x10 when the pitch is not above
// 0, else the share at +0x14; the yaw input times the share at +0x28)
// DC-DEFAULT: CLI-571 (proposed) each node turns about its own origin in model space by its own share and its
// children follow (the shares along torso, upper torso, neck and shoulder add up to the whole pitch); axes +Y / +X
// of the model (the body's facing is +Z, CLI-40's frame)
pub fn node_turn(share: &NodeShare, pitch: f32, yaw: f32) -> Quat {
    let p = pitch
        * if pitch > 0.0 {
            share.pitch_down
        } else {
            share.pitch_up
        };
    Quat::from_rotation_y(yaw * share.yaw) * Quat::from_rotation_x(p)
}

/// Turns the controlled nodes of a posed skeleton (`globals`, model space, parents first) by the aim; children
/// follow their parents. Nothing changes for a zero aim.
pub fn apply(sk: &Skeleton, globals: &mut [Mat4], aim: [f32; 2]) {
    let [pitch, yaw] = aim;
    if pitch == 0.0 && yaw == 0.0 {
        return;
    }
    let mut turn: Vec<Option<Quat>> = vec![None; globals.len()];
    for n in &NODES {
        if let Some(i) = node_of(sk, n) {
            if i < turn.len() {
                let q = node_turn(n, pitch, yaw);
                if q != Quat::IDENTITY {
                    turn[i] = Some(q);
                }
            }
        }
    }
    if turn.iter().all(Option::is_none) {
        return;
    }
    // delta[i] maps the node's old model-space transform to its new one (None: the node did not move)
    let mut delta: Vec<Option<Mat4>> = Vec::with_capacity(globals.len());
    for i in 0..globals.len() {
        let parent = sk.parents.get(i).copied().flatten();
        let inherited = parent.and_then(|p| delta.get(p).copied().flatten());
        if inherited.is_none() && turn[i].is_none() {
            delta.push(None);
            continue;
        }
        let old = globals[i];
        let mut g = inherited.map_or(old, |d| d * old);
        if let Some(q) = turn[i] {
            let pivot = g.w_axis.truncate();
            g = Mat4::from_translation(pivot)
                * Mat4::from_quat(q)
                * Mat4::from_translation(-pivot)
                * g;
        }
        let inv = old.inverse();
        delta.push(Some(if inv.is_finite() {
            g * inv
        } else {
            inherited.unwrap_or(Mat4::IDENTITY)
        }));
        globals[i] = g;
    }
}

/// The model-space forward (+Z) of a node after posing, for checks.
pub fn node_forward(globals: &[Mat4], i: usize) -> Option<Vec3> {
    globals.get(i).map(|g| g.transform_vector3(Vec3::Z))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(sub: u8, obj: u16, f: [f32; 6]) -> (Vec<u8>, usize) {
        let mut w = dc_wire::udp::BitWriter::new();
        w.write_u8(NOTE_CHARACTER).write_u16(obj).write_u8(sub);
        for v in f {
            w.write_f32(v);
        }
        if sub == SUB_AIM_FLAGS {
            w.write_u32(0);
        }
        (w.as_bytes().to_vec(), w.bit_len())
    }

    #[test]
    fn aim_notes_read_and_other_subs_do_not() {
        let (b, n) = note(SUB_AIM, 71, [0.3, -0.2, 0.0, 0.0, 0.0, 0.0]);
        let (obj, a) = read_aim_note(&b, n).unwrap();
        assert_eq!(obj, 71);
        assert_eq!((a.pitch, a.yaw), (0.3, -0.2));
        let (b, n) = note(SUB_AIM_FLAGS, 9, [0.1, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(read_aim_note(&b, n).unwrap().1.pitch, 0.1);
        let (b, n) = note(0x1f, 9, [0.1; 6]);
        assert!(read_aim_note(&b, n).is_none());
    }

    #[test]
    fn two_notes_in_a_row_are_followed() {
        // the 2009 step halves the distance per 2009 frame; 0.25 s of steps reach the target
        let mut s = AimState::default();
        s.on_note(AimNote {
            pitch: 0.5,
            yaw: 0.2,
            ..Default::default()
        });
        s.step_once(STEP_S);
        assert!((s.pitch - 0.25).abs() < 1e-6, "one step: halfway");
        s.advance(0.25);
        assert!((s.drawn()[0] - 0.5).abs() < 1e-3);
        assert!((s.drawn()[1] - 0.2).abs() < 1e-3);
        s.on_note(AimNote {
            pitch: -0.4,
            ..Default::default()
        });
        s.advance(0.25);
        assert!(
            (s.drawn()[0] + 0.4).abs() < 1e-3,
            "the second note is followed"
        );
        assert!(s.drawn()[1].abs() < 1e-3);
    }

    #[test]
    fn easing_does_not_depend_on_the_frame_rate() {
        let mut a = AimState::default();
        let mut b = AimState::default();
        let n = AimNote {
            pitch: 1.0,
            ..Default::default()
        };
        a.on_note(n);
        b.on_note(n);
        for _ in 0..20 {
            a.advance(1.0 / 30.0);
        }
        for _ in 0..80 {
            b.advance(1.0 / 120.0);
        }
        assert!((a.pitch - b.pitch).abs() < 1e-6);
    }

    /// A chain pelvis -> torso -> upper torso -> neck -> head, and a shoulder under the neck, each 10 units up.
    fn chain() -> Skeleton {
        let names = [
            "Right_pelvis",
            "Torso",
            "Upper_torso",
            "Neck",
            "Head",
            "Right_shoulder",
        ];
        let parents = vec![None, Some(0), Some(1), Some(2), Some(3), Some(3)];
        let bind_local: Vec<(Vec3, Quat)> = (0..names.len())
            .map(|i| {
                (
                    Vec3::new(0.0, if i == 0 { 40.0 } else { 10.0 }, 0.0),
                    Quat::IDENTITY,
                )
            })
            .collect();
        let mut sk = Skeleton {
            parents,
            names: names
                .iter()
                .map(|n| n.as_bytes().to_ascii_uppercase())
                .collect(),
            bind_global: Vec::new(),
            inverse_bind: Vec::new(),
            bind_local,
        };
        sk.bind_global = sk.globals(&sk.bind_local);
        sk.inverse_bind = sk.bind_global.iter().map(|g| g.inverse()).collect();
        sk
    }

    #[test]
    fn pitch_tips_the_upper_body_and_the_shoulder_follows_the_whole_angle() {
        let sk = chain();
        let mut g = sk.globals(&sk.bind_local);
        let before = g.clone();
        apply(&sk, &mut g, [0.0, 0.0]);
        assert_eq!(g, before, "no aim, no change");
        // looking down 0.6 rad: the shoulder's forward turns by the whole down share (0.35 + 0.45 + 0.1 + 0.1)
        apply(&sk, &mut g, [0.6, 0.0]);
        let f = node_forward(&g, 5).unwrap();
        let down = (-f.y).atan2(f.z);
        assert!((down - 0.6).abs() < 1e-4, "shoulder pitch {down}");
        assert!(f.y < 0.0, "positive pitch looks down");
        // the pelvis does not move, the head moves
        assert_eq!(g[0], before[0]);
        assert!(g[4].w_axis.distance(before[4].w_axis) > 1.0);
        // looking up turns the other way, by the up shares
        let mut up = sk.globals(&sk.bind_local);
        apply(&sk, &mut up, [-0.4, 0.0]);
        let f = node_forward(&up, 5).unwrap();
        assert!(f.y > 0.0);
        assert!(((f.y).atan2(f.z) - 0.4).abs() < 1e-4);
    }

    /// CLI-634: a look offset of 60 degrees turns the body by the 15 past the limit and twists the upper body by
    /// 45; a look offset of 30 degrees leaves the body yaw alone.
    #[test]
    fn past_the_limit_the_body_follows_the_view() {
        let deg = |d: f32| d.to_radians();
        let settled = |yaw: f32| {
            let mut a = AimState::default();
            a.on_note(AimNote {
                yaw,
                ..AimNote::default()
            });
            a.advance(2.0);
            a
        };
        let a = settled(deg(60.0));
        assert!(
            (a.body_follow() - deg(15.0)).abs() < 1e-4,
            "{}",
            a.body_follow()
        );
        assert!((a.drawn()[1] - deg(45.0)).abs() < 1e-4);
        let a = settled(deg(-60.0));
        assert!((a.body_follow() + deg(15.0)).abs() < 1e-4);
        let a = settled(deg(30.0));
        assert_eq!(a.body_follow(), 0.0);
        assert!((a.drawn()[1] - deg(30.0)).abs() < 1e-4);
        // back within the limit: the body stays where the stream puts it
        assert_eq!(split_yaw(deg(44.0)).1, 0.0);
    }

    #[test]
    fn yaw_offset_twists_torso_and_upper_torso() {
        let sk = chain();
        let mut g = sk.globals(&sk.bind_local);
        apply(&sk, &mut g, [0.0, 0.5]);
        let f = node_forward(&g, 2).unwrap();
        assert!(
            (f.x.atan2(f.z) - 0.5).abs() < 1e-4,
            "upper torso carries the whole twist"
        );
        let f = node_forward(&g, 1).unwrap();
        assert!((f.x.atan2(f.z) - 0.5 * 0.411_764_7).abs() < 1e-4);
    }
}
