//! The world camera shot: while the server keeps a world camera object live, the frame is drawn from that object
//! (its position and rotation as the object records place it, its field of view) instead of the own view, the
//! first-person weapon is hidden, the camera is the sound listener when its blob says so, and the body does not move
//! when the blob forbids it. The server moves the camera from point to point with change records (cuts), so the
//! camera follows the object's path exactly; a camera that is switched off returns the own view at once.
//!
//! The 2009 rule (CLI-240, proposed; DISASM, CShell):
//! - The camera objects are the objects created from type-0x0d blobs (`[0x0d][movement:u8][listener:u8][third:u8]
//!   [fov_x:f32][fov_y:f32]`, reader cshell+0x217b4a..0x217be7, init cshell+0x216870).
//! - Every frame the local player's slot +0x104 (cshell+0x1f83d0) takes the first of them whose server object has
//!   bit 0x20 in flag word 2 (the user flags, cshell+0x1f8439..0x1f8451) and draws from it: position and rotation of
//!   the object (cshell+0x1f8547..0x1f89d1; the smoothing branch at cshell+0x1f8539 needs two player flags the shot
//!   does not set and is not modelled), field of view from the blob in degrees (cshell+0x1f8aab..0x1f8b22,
//!   cshell+0x1f50d5), weapon hidden and listener set (cshell+0x1f50b0, cshell+0x1f8a9a). Its +0x80 := the movement
//!   byte (cshell+0x1f8502); the movement law zeroes the body's velocity while it is 0 (cshell+0x2001c0).
//! - With no live camera the own view returns (slot +0x3c, cshell+0x1f2ca0). The shot runs before the own camera
//!   update in every frame (cshell+0x1f4fe0 / cshell+0x1f4fea), also while dead: it takes precedence over the death
//!   camera.
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2 items 3, 6, 7 (the camera object's create and change records)
// DC-DEFAULT: CLI-240 (proposed) the shot; DSV-112 (proposed) the server side

use crate::look::ViewAngles;
use dc_botclient::objects::ClientObject;
use dc_sim::V3;
use std::collections::BTreeMap;

/// Client object type of a world camera.
// DISASM: cshell+0x222480 case 0xd, cshell+0x2167f0 (type getter 0xd)
pub const OBJ_CAMERA_TYPE: u8 = 0x0d;
/// The live bit in the camera object's user flags.
// DISASM: cshell+0x1f8445..0x1f8451
pub const UFLAG_CAMERA_LIVE: u32 = 0x20;

/// A live world camera as the frame loop needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamShotView {
    /// The camera object's id.
    pub obj: u16,
    pub pos: [f32; 3],
    /// The object's rotation (x, y, z, w); `None` when no rotation was received.
    pub rot: Option<[f32; 4]>,
    /// The players may move while the camera is live (blob byte 1).
    pub allow_move: bool,
    /// The camera is the sound listener (blob byte 2). The 2009 client reads the same byte as the camera's kind:
    /// 1 = letterbox ([`CamShotView::letterbox`]).
    pub listener: bool,
    /// Horizontal and vertical field of view in degrees.
    pub fov: [f32; 2],
}

impl CamShotView {
    /// The camera draws the letterbox bars: its blob's second byte is 1 (the only values sent are 0 / 1).
    // DISASM: cshell+0x216887 (byte -> camera +0x3c), cshell+0x1f8aa2 / 0x1f8b22 (+0x3c -> cshell+0x1f50b0),
    // cshell+0x1f50b8 (letterbox flag = byte == 1)
    // DC-DEFAULT: CLI-610 (proposed)
    pub fn letterbox(&self) -> bool {
        self.listener
    }
}

/// The blob fields of a camera object: (movement, listener, fov).
pub fn camera_blob(o: &ClientObject) -> Option<(bool, bool, [f32; 2])> {
    if o.blob_type != Some(OBJ_CAMERA_TYPE) {
        return None;
    }
    let b = o.blob.as_ref()?;
    if b.bit_len() < 8 * 4 + 64 {
        return None;
    }
    let mut r = b.reader();
    r.read_u8();
    let allow = r.read_u8() != 0;
    let listener = r.read_u8() != 0;
    r.read_u8();
    let fov = [r.read_f32(), r.read_f32()];
    Some((allow, listener, fov))
}

/// The live camera of the session's objects: the first camera object (lowest id) with the live bit and a position.
pub fn live_camera(objects: &BTreeMap<u16, ClientObject>) -> Option<CamShotView> {
    objects.iter().find_map(|(&obj, o)| {
        if o.uflags & UFLAG_CAMERA_LIVE == 0 {
            return None;
        }
        let (allow_move, listener, fov) = camera_blob(o)?;
        let pos = o.pos.filter(|p| p.iter().all(|v| v.is_finite()))?;
        Some(CamShotView {
            obj,
            pos,
            rot: o.rot.filter(|q| q.iter().all(|v| v.is_finite())),
            allow_move,
            listener,
            fov,
        })
    })
}

/// The view angles of a rotation: its forward axis (+Z turned by the quaternion) as yaw (0 along +Z, positive toward
/// +X) and pitch (positive down), the convention of [`crate::deathcam::look_at`].
pub fn view_of(q: [f32; 4]) -> ViewAngles {
    let [x, y, z, w] = q;
    let n = (x * x + y * y + z * z + w * w).sqrt();
    let (x, y, z, w) = if n > 1e-6 {
        (x / n, y / n, z / n, w / n)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    // the third column of the rotation matrix
    let f = V3::new(
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    );
    let flat = (f.x * f.x + f.z * f.z).sqrt();
    ViewAngles {
        yaw: f.x.atan2(f.z),
        pitch: (-f.y).atan2(flat),
    }
}

/// The camera shot of a frame loop: what it drew last and how long.
#[derive(Debug, Clone, Default)]
pub struct CamShot {
    /// The live camera of the last frame.
    pub current: Option<CamShotView>,
    /// Frames drawn from a world camera so far.
    pub frames: u32,
    /// Shots seen (a new shot = the first live frame after a frame without one).
    pub shots: u32,
    /// The view of the last live frame (kept when a camera has no rotation).
    view: ViewAngles,
}

impl CamShot {
    /// The camera of this frame: `None` without a live camera; else the eye, the view angles and the horizontal
    /// field of view to draw with.
    pub fn frame(&mut self, shot: Option<&CamShotView>) -> Option<(V3, ViewAngles, f32)> {
        let Some(s) = shot else {
            self.current = None;
            return None;
        };
        if self.current.is_none() {
            self.shots += 1;
        }
        self.current = Some(*s);
        self.frames += 1;
        if let Some(q) = s.rot {
            self.view = view_of(q);
        }
        // the blob's vertical field (the client-fov rule derives the horizontal one from the target aspect)
        let fov = if s.fov[1].is_finite() && s.fov[1] > 1.0 {
            s.fov[1]
        } else {
            dc_render::FOV_Y_DEFAULT_DEG
        };
        Some((V3::new(s.pos[0], s.pos[1], s.pos[2]), self.view, fov))
    }

    /// The shot is live and holds the body (the blob's movement byte is 0).
    pub fn locks_movement(&self) -> bool {
        self.current.is_some_and(|s| !s.allow_move)
    }
}

/// The view settings of a frame: the shot's vertical field of view replaces the client's while a camera is live
/// (the horizontal one follows the target aspect as the client-fov rule does; the blob's own horizontal value is
/// not used).
// DISASM: cshell+0x1f8aab..0x1f8b22 (fov from the camera blob); DC-DEFAULT: CLI-240 (proposed) the horizontal from
// the aspect (CLI-212)
pub fn view_settings(
    v: crate::frame::ViewSettings,
    fov_y: Option<f32>,
) -> crate::frame::ViewSettings {
    match fov_y {
        Some(f) => crate::frame::ViewSettings { fov_y_deg: f, ..v },
        None => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::BitWriter;

    // the table's object has a private field: no struct literal outside its crate
    #[allow(clippy::field_reassign_with_default)]
    fn camera(live: bool, pos: [f32; 3], q: [f32; 4]) -> ClientObject {
        let mut b = BitWriter::new();
        b.write_u8(OBJ_CAMERA_TYPE)
            .write_u8(0)
            .write_u8(1)
            .write_u8(0)
            .write_f32(80.0)
            .write_f32(60.0);
        let mut o = ClientObject::default();
        o.pos = Some(pos);
        o.rot = Some(q);
        o.blob_type = Some(OBJ_CAMERA_TYPE);
        o.blob = Some(b);
        o.uflags = if live { UFLAG_CAMERA_LIVE } else { 0 };
        o
    }

    #[test]
    fn view_of_matches_the_look_convention() {
        // identity looks along +Z
        let v = view_of([0.0, 0.0, 0.0, 1.0]);
        assert!(v.yaw.abs() < 1e-6 && v.pitch.abs() < 1e-6);
        // a yaw of 90 degrees about +Y looks along +X
        let h = std::f32::consts::FRAC_PI_4;
        let v = view_of([0.0, h.sin(), 0.0, h.cos()]);
        assert!((v.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        // a positive pitch about +X looks down (positive pitch)
        let p = 0.3f32;
        let v = view_of([(p / 2.0).sin(), 0.0, 0.0, (p / 2.0).cos()]);
        assert!((v.pitch - p).abs() < 1e-5, "{v:?}");
    }

    #[test]
    fn only_a_live_camera_object_is_a_shot() {
        let mut objs = BTreeMap::new();
        objs.insert(9, camera(false, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0]));
        assert!(live_camera(&objs).is_none());
        objs.insert(12, camera(true, [4.0, 5.0, 6.0], [0.0, 0.0, 0.0, 1.0]));
        let s = live_camera(&objs).unwrap();
        assert_eq!(
            (s.obj, s.pos, s.allow_move, s.listener),
            (12, [4.0, 5.0, 6.0], false, true)
        );
        assert_eq!(s.fov, [80.0, 60.0]);
        // another object type with the bit is not a camera
        let mut other = camera(true, [0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        other.blob_type = Some(0x37);
        let mut objs = BTreeMap::new();
        objs.insert(3, other);
        assert!(live_camera(&objs).is_none());
        let mut c = CamShot::default();
        assert!(c.frame(None).is_none());
        assert!(!c.locks_movement());
    }
}
