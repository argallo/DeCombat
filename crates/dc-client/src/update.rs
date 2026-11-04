//! The player update the client sends to a game server: shell message 0x64 (`UP_PLAYER_UPDATE`) in the live
//! (derived) layout, gathered from the local simulation and the camera every frame, packed by dc-botclient's
//! writer and sent by its session under the 2009 gate (player state 1, at most `SEND_RATE` per second).
//!
//! Layout written (GAME_MESSAGES_2009_C2S.md §2.1), in the form the 2009 client sent (capture tdm1): `[pu_flags:u16]`
//! with 0x1000 in every update, the view block of flag 0x0020 (`[yaw8c:u8][body_angles:vec96]`, then the six aux
//! words: view pitch, view yaw offset, four zeros), the eye offset of flag 0x0001 (every update), the aim rotation
//! of flag 0x0100 (`quat128`, only when it changed), the camera point of flag 0x0400 (only when it moved),
//! `[control:u32]`, and the movement trailer of MOVEMENT_2009.md §12.2.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (layout, flag sources, send rule); MOVEMENT_2009.md §12.2 (trailer)
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (d) flags, (f) aux and aim, (g) eye offset and camera point

use crate::look::ViewAngles;
use crate::todo;
use dc_botclient::motion::STANDING_NONE;
use dc_botclient::shell::{pu, MoveReport, MoveTrailer, UpdateExtras};
use dc_sim::PlayerState;

/// Unreliable updates per second towards a remote server. The 2009 client variable default is 15; DeCombat
/// sends 30 (CLI-1578): the game server streams at 30 Hz (CD-63 on the VPS) and a 15 Hz reporter made every other
/// streamed position a repeat of the last, a stop-go motion on every other client (half of a running player's
/// samples repeated, 2026-10-31). Costs 15 more small datagrams per second per player; the server has no
/// per-update speed check (DSV-140).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (R = 15.0 when the variable is missing, PE-DATA VA 0x374ce220);
// MOVEMENT_2009.md §13.1 SEND_RATE (15.0)
// DC-DEFAULT: CLI-1578 (proposed) 30 updates per second (the 2009 default is 15)
pub const SEND_RATE_HZ: f32 = 30.0;

/// Surface type the trailer reports on a ladder.
// SPEC: MOVEMENT_2009.md §12.2 ([surface:u8] mover+0x50, 200 on ladders)
pub const SURFACE_LADDER: u8 = 200;

/// The fields of one 0x64 update, unpacked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalUpdate {
    /// Body angles (pitch, yaw, roll) of the view block; pitch is 0 for the body.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (body_angles = local_player+0x68..+0x70); MOVEMENT_2009.md §13.2 step 7
    pub body_angles: [f32; 3],
    /// The camera's rotation (x, y, z, w) for the `aim_q` block.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (aim_q: rotation built from local_player+0x48, +0x4c, +0x50)
    pub aim: [f32; 4],
    /// View pitch (radians, positive looks down) for `aux[0]`.
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (f) (aux[0] = view pitch, aux[1] = view yaw offset, rest 0)
    pub pitch: f32,
    /// View yaw offset (the view yaw minus the body yaw) for `aux[1]`: 0 from [`LocalUpdate::from_state`], the own
    /// twist with [`LocalUpdate::with_twist`] (CLI-636).
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (f); DISASM: cshell+0x5ecc0 (offset +0x410)
    pub yaw_offset: f32,
    /// Eye point minus body position for the `eye_offset` block (flag 0x0001); the camera point is the body
    /// position plus this.
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (g) ((0, 33, 0) standing, (0, 20, 0) crouched)
    pub eye_offset: [f32; 3],
    /// The tick's control word (mover+0x24).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 ([control:u32]); MOVEMENT_2009.md §5.1
    pub control: u32,
    // Movement trailer, MOVEMENT_2009.md §12.2:
    /// The sequence byte of the last 0xa0 / 0x0020 teleport, echoed.
    pub move_seq: u8,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Client time in seconds.
    pub time: f32,
    pub on_ground: bool,
    /// Surface type under the feet (200 on ladders): [`LocalUpdate::from_state`] leaves 0, the frame loops set the
    /// standing polygon's type ([`crate::world::LoadedWorld::surface_under`], held while airborne; CLI-14 amended,
    /// proposed).
    pub surface: u8,
    /// The standing ids (low, high 32 bits); `-1 / -1` (nothing). Which engine ids they are is not specified, so
    /// the client sends "nothing" (CLI-14, proposed; the bot does the same, BOT-3).
    pub standing: u64,
}

/// The camera rotation of `view` as `quat128` components x, y, z, w: the rotation dc-render's camera uses
/// (yaw about +Y after pitch about +X, PLAUSIBLE for the 2009 Euler builder).
// SPEC: MOVEMENT_2009.md §13.2 step 7 (rotation order PLAUSIBLY yaw about +Y, pitch about +X); RENDER_2009.md §4.1
pub fn aim_quat_of_view(view: &ViewAngles) -> [f32; 4] {
    let c = dc_render::Camera::looking(glam::Vec3::ZERO, view.yaw, view.pitch, 1.0, 1.0);
    let q = c.rot.normalize();
    [q.x, q.y, q.z, q.w]
}

impl LocalUpdate {
    /// Gathers the fields from the current simulation state and view.
    pub fn from_state(state: &PlayerState, view: &ViewAngles, time: f32) -> LocalUpdate {
        LocalUpdate {
            body_angles: [0.0, view.yaw, 0.0],
            aim: aim_quat_of_view(view),
            pitch: view.pitch,
            yaw_offset: 0.0,
            eye_offset: eye_offset_of(state),
            control: state.controls,
            move_seq: state.move_seq,
            pos: state.pos.to_array(),
            vel: state.vel.to_array(),
            time,
            on_ground: state.on_ground,
            surface: if state.on_ladder { SURFACE_LADDER } else { 0 },
            standing: STANDING_NONE,
        }
    }

    /// Applies the own view yaw offset ([`crate::aim::OwnTwist`]): the body yaw becomes the view yaw minus the
    /// offset and `aux[1]` carries the offset, as the 2009 player sends them.
    // DISASM: cshell+0x5ecc0 (body yaw +0x6c = view yaw +0x408 - offset +0x410)
    // DC-DEFAULT: CLI-636 (proposed) the sender keeps the 2009 twist rule
    pub fn with_twist(mut self, view_yaw: f32, twist: &crate::aim::OwnTwist) -> LocalUpdate {
        self.yaw_offset = twist.offset();
        self.body_angles[1] = twist.body_yaw(view_yaw);
        self
    }

    /// The wire form for dc-botclient's 0x64 writer. The aim block is always offered; the session drops it when
    /// the rotation did not change since the last update (BOT-9 rule). `aux` = view pitch, then the view yaw
    /// offset, then four zero words, as the 2009 client sends them.
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (f); DC-DEFAULT: CLI-18 (proposed) the aim rotation is the
    // camera's, sent whenever it changed (BOT-9 rule for the client)
    pub fn to_wire(&self) -> MoveReport {
        MoveReport {
            angles: self.body_angles,
            aux: [self.pitch.to_bits(), self.yaw_offset.to_bits(), 0, 0, 0, 0],
            aim: Some(self.aim),
            control: self.control,
            trailer: MoveTrailer {
                move_seq: self.move_seq,
                pos: self.pos,
                vel: self.vel,
                time: self.time,
                on_ground: self.on_ground as u8,
                surface: self.surface,
                standing: self.standing,
            },
        }
    }

    /// The optional blocks: the eye offset in every update, the camera point (body position + eye offset; the
    /// session sends it only when it moved) and flag 0x1000, which the 2009 client sets in every update.
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (d) (0x0001 and 0x1000 in 2,338 of 2,338), (g)
    // DC-DEFAULT: CLI-20 (proposed) the 2009 client's optional 0x64 blocks, weapon float absent (no weapons yet)
    pub fn extras(&self) -> UpdateExtras {
        let e = self.eye_offset;
        UpdateExtras {
            eye_offset: Some(e),
            weapon_f: None,
            cam_point: Some([self.pos[0] + e[0], self.pos[1] + e[1], self.pos[2] + e[2]]),
            flag_bits: pu::FLAG_1000,
        }
    }

    /// The packed 0x64 body (after the shell id) with the aim block and every optional block, as a server receives
    /// the first update.
    pub fn pack(&self) -> Vec<u8> {
        dc_botclient::shell::player_update_body_ext(&self.to_wire(), &self.extras())
            .as_bytes()
            .to_vec()
    }
}

/// The eye offset of the simulated body: the camera's height above the body centre (CLI-3, observed value) plus
/// the crouch dip, no forward offset. The height comes from the standing box plus the drop of the centre in the
/// crouch box (SIM-25), so the eye keeps its height above the feet whichever box is in force. The render camera uses the same height
/// ([`crate::player::LocalPlayer::eye_target`]).
// SPEC: MOVEMENT_2009.md §13.4 (eye_h, crouch dip); OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (g)
pub fn eye_offset_of(state: &PlayerState) -> [f32; 3] {
    [
        0.0,
        todo::EYE_ABOVE_CENTRE_FRACTION * state.stand_dims.y + state.crouch_drop() + state.eye_dip,
        0.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_sim::{MovementParams, V3};
    use dc_wire::udp::{read_quat128, BitWriter};

    #[test]
    fn fields_come_from_the_state_and_pack_in_the_live_layout() {
        let p = MovementParams::default();
        let mut s = PlayerState::new(V3::new(1.0, 2.0, 3.0), V3::new(16.0, 40.0, 16.0), &p);
        s.controls = 0x201;
        s.move_seq = 7;
        s.vel = V3::new(10.0, 0.0, -5.0);
        let view = ViewAngles {
            yaw: 0.5,
            pitch: 0.1,
        };
        let u = LocalUpdate::from_state(&s, &view, 1.5);
        assert_eq!(u.pos, [1.0, 2.0, 3.0]);
        assert_eq!(u.control, 0x201);
        assert_eq!(u.move_seq, 7);
        assert_eq!(u.body_angles, [0.0, 0.5, 0.0]);
        assert_eq!(u.standing, u64::MAX);
        // the camera rotation equals the shared simulation's yaw-after-pitch quaternion
        let q = dc_sim::Quat::from_yaw_pitch(view.yaw, view.pitch);
        for (a, b) in u.aim.iter().zip([q.x, q.y, q.z, q.w]) {
            assert!((a - b).abs() < 1e-5, "{:?} vs {q:?}", u.aim);
        }
        // the eye 0.66 x the half height above the centre: 26.4 for this made-up 40-unit half height
        assert_eq!(u.eye_offset, [0.0, 0.66 * 40.0, 0.0]);
        // packed in the 2009 client's form: flags 0x1521 (eye offset, view block, aim, camera point, 0x1000),
        // yaw8c, body angles, aux (pitch, 0, 0, 0, 0, 0), eye offset, quat128, camera point, control, trailer
        let bytes = u.pack();
        let w = BitWriter::from_bytes(&bytes);
        let mut r = w.reader();
        assert_eq!(r.read_u16(), 0x1521);
        assert_eq!(r.read_u8(), 20); // trunc(0.5 * 40.425354)
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [0.0, 0.5, 0.0]);
        assert_eq!(r.read_f32(), 0.1, "aux[0] = view pitch");
        for _ in 0..5 {
            assert_eq!(r.read_u32(), 0);
        }
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], u.eye_offset);
        let aim = read_quat128(&mut r);
        assert!(aim.iter().zip(u.aim).all(|(a, b)| (a - b).abs() < 1e-6));
        assert_eq!(
            [r.read_f32(), r.read_f32(), r.read_f32()],
            [1.0, 2.0 + 0.66 * 40.0, 3.0],
            "camera point = body + eye offset"
        );
        assert_eq!(r.read_u32(), 0x201);
        assert_eq!(r.read_u8(), 7);
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [1.0, 2.0, 3.0]);
        assert_eq!(
            [r.read_f32(), r.read_f32(), r.read_f32()],
            [10.0, 0.0, -5.0]
        );
        assert_eq!(r.read_f32(), 1.5);
        r.skip_bits(16);
        assert_eq!(r.read_u64(), u64::MAX);
        assert!(r.remaining_bits() < 8, "nothing follows the trailer");
    }

    /// CLI-636: a standing client turning 60 degrees sends the view yaw offset clamped at the twist limit (pi/4) as
    /// aux[1] and the body yaw view - offset; a move key returns the offset to 0 and the body yaw to the view yaw.
    #[test]
    fn standing_turn_sends_the_clamped_twist_and_a_move_key_clears_it() {
        use crate::aim::{OwnTwist, YAW_LIMIT};
        let p = MovementParams::default();
        let mut s = PlayerState::new(V3::new(0.0, 0.0, 0.0), V3::new(16.0, 40.0, 16.0), &p);
        let mut twist = OwnTwist::default();
        let mut view = ViewAngles {
            yaw: 0.2,
            pitch: 0.0,
        };
        twist.step(view.yaw, 0);
        // 60 degrees in 30 frames, standing
        for _ in 0..30 {
            view.yaw += 2f32.to_radians();
            twist.step(view.yaw, 0);
        }
        let u = LocalUpdate::from_state(&s, &view, 1.0).with_twist(view.yaw, &twist);
        assert!((u.yaw_offset - YAW_LIMIT).abs() < 1e-6, "{}", u.yaw_offset);
        let body = u.body_angles[1];
        assert!((body - (view.yaw - YAW_LIMIT)).abs() < 1e-5, "body {body}");
        assert!(
            (body - (0.2 + 15f32.to_radians())).abs() < 1e-4,
            "dragged 15 degrees past the limit"
        );
        let bytes = u.pack();
        let w = BitWriter::from_bytes(&bytes);
        let mut r = w.reader();
        r.skip_bits(16 + 8 + 32);
        assert!(
            (r.read_f32() - body).abs() < 1e-6,
            "body yaw in the view block"
        );
        r.skip_bits(32 + 32);
        assert_eq!(r.read_f32(), YAW_LIMIT, "aux[1] = the clamped offset");
        // turning back 10 degrees untwists by 10 without moving the body
        for _ in 0..5 {
            view.yaw -= 2f32.to_radians();
            twist.step(view.yaw, 0);
        }
        assert!((twist.offset() - (YAW_LIMIT - 10f32.to_radians())).abs() < 1e-5);
        assert!((twist.body_yaw(view.yaw) - body).abs() < 1e-5);
        // a move key: the offset is 0, body yaw = view yaw
        s.controls = dc_sim::player::bits::FWD;
        twist.step(view.yaw, s.controls);
        let u = LocalUpdate::from_state(&s, &view, 1.1).with_twist(view.yaw, &twist);
        assert_eq!(u.yaw_offset, 0.0);
        assert!((u.body_angles[1] - view.yaw).abs() < 1e-6);
        assert_eq!(u.to_wire().aux[1], 0);
        // moving while turning keeps it at 0
        view.yaw += 0.3;
        twist.step(view.yaw, s.controls);
        assert_eq!(twist.offset(), 0.0);
        // a server view set zeroes it
        twist.step(view.yaw + 0.4, 0);
        twist.reset(1.0);
        assert_eq!((twist.offset(), twist.body_yaw(1.0)), (0.0, 1.0));
    }
}
