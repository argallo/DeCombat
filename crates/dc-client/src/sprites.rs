//! Billboards: the server's kind-3 objects (the scaled sprites of a world file: light coronas, glows, flares; 110 of
//! them in the tdm1 capture) drawn the way the 2009 engine draws a sprite object, as one textured quad.
//!
//! The 2009 rules, read from Engine.exe (clean decompiler output):
//! - The object carries a sprite file id (create and `CF_FILES`), a two-component scale, a colour block and the flag
//!   words of every object (UDP_SESSION_2009_B §3.3, §4.2).
//! - The sprite draw (engine+0x1b1210) picks the builder by the object flag word (+0x84, the wire `flags`, the
//!   reader keeps only its own bits 0x0024): bit 0x08 builds the quad from the object's own rotation and scale
//!   (engine+0x1b0b80), otherwise it faces the camera (engine+0x1af730).
//! - The camera-facing quad (engine+0x1af730): the depth of the object along the camera's forward axis is taken first
//!   and an object nearer than 7.0 is not drawn; the half width is the current frame texture's width (u16 at
//!   texture +4) times the scale x (+0x9c), the half height its height (+6) times the scale y (+0xa0); the corners
//!   are the position plus or minus the camera's right axis times the half width and its up axis times the half
//!   height. Bit 0x40 (eye-attached) uses the identity axes in eye space. Bits 0x04 and 0x20 (a pull toward the
//!   camera and a distance size factor) are among the bits the wire never sets (reader mask 0xffff0024).
//! - Blend (engine+0x1abc80, the object blend helper): flag word 2 (+0x88) bit 0x2 additive, bit 0x4 multiply,
//!   otherwise alpha blend; the colour block is the vertex colour (TEXTURE x DIFFUSE, engine+0x1af4a0).
//! - The sprite file plays its frames from the object's creation, looping (FMT_SPR_LTC_FX §2.4, cursor flags 3).
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 3 `[file:u16]`), §3.5.1 (sprites `cf` 0x049b, flags 0x0081 / 0x0002 /
// 0x00000001 / 0, scale (0.5, 0.5): REFERENCE), §4.2 items 1, 3, 4, 5; RENDER_2009.md §7.1 (blend helper), §8.1
// (sprites); FMT_SPR_LTC_FX.md §2.4 (playback)
// EVIDENCE: engine+0x1b1210 (builder by flag 0x08), engine+0x1af730 (camera-facing quad, near cut 7.0, half extents =
// texture size x scale), engine+0x1b0b80 (quad from the object's rotation, position and scale matrix),
// engine+0xa32f0 (flag word stored at +0x84 keeping bits 0x0024)

use std::collections::BTreeMap;

use dc_botclient::objects::{kind, ClientObject};
use dc_botclient::session::BotSession;
use glam::{Quat, Vec3};

use crate::fx::{Blend, Quad, TextureStore};

/// Object flag bits the sprite draw reads (the `flags` word of the flag block).
// EVIDENCE: engine+0x1b1210 (0x08 rotatable builder, 0x40 eye space), engine+0x1af730 (0x40 identity axes)
pub const FLAG_VISIBLE: u16 = 0x0001;
pub const FLAG_ROTATABLE: u16 = 0x0008;
pub const FLAG_EYE: u16 = 0x0040;
/// Flag word 2 bits of the blend helper.
// EVIDENCE: engine+0x1abc80 (+0x88 bit 0x2 additive ONE / ONE, bit 0x4 multiply ZERO / SRCCOLOR)
pub const FLAG2_ADDITIVE: u16 = 0x0002;
pub const FLAG2_MULTIPLY: u16 = 0x0004;
/// A camera-facing sprite nearer than this along the camera's forward axis is not drawn.
// EVIDENCE: engine+0x1af730 (`7.0 <= depth` before the quad is built)
pub const NEAR_CUT: f32 = 7.0;

/// One billboard object as the frame loop draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Billboard {
    pub obj: u16,
    /// The sprite (or texture) file the object names (session file table).
    pub file: String,
    pub pos: [f32; 3],
    /// Rotation `(x, y, z, w)` (identity when the object has none).
    pub rot: [f32; 4],
    /// Scale x and y (1, 1 when the object has no scale block).
    pub scale: [f32; 2],
    /// Red, green, blue, alpha (white and opaque when the object has no colour block).
    pub colour: [u8; 4],
    pub flags: u16,
    pub flags2: u16,
    /// Session time the client first saw the object (its sprite plays from there).
    pub seen_ms: u32,
}

/// The billboards of a session's object table, with the time each was first seen.
#[derive(Debug, Clone, Default)]
pub struct BillboardTracker {
    /// Object -> (file id, first sight); a new file id restarts the sprite.
    seen: BTreeMap<u16, (u16, u32)>,
    /// Billboards of the last sync and the most seen at once.
    pub last: usize,
    pub most: usize,
}

impl BillboardTracker {
    /// The billboards of `s` at session time `now`.
    pub fn sync(&mut self, s: &BotSession, now: u32) -> Vec<Billboard> {
        let files = &s.obs.files;
        self.collect(
            &s.table.objects,
            &|f| files.get(&f).map(|x| x.1.clone()),
            now,
        )
    }

    /// The billboards of an object table whose file ids `name` resolves.
    // DC-DEFAULT: CLI-522 (proposed) only objects with the visible bit 0x0001 in their latest flag block are drawn
    pub fn collect(
        &mut self,
        objects: &BTreeMap<u16, ClientObject>,
        name: &dyn Fn(u16) -> Option<String>,
        now: u32,
    ) -> Vec<Billboard> {
        self.seen
            .retain(|k, _| objects.get(k).is_some_and(|o| o.kind == kind::BILLBOARD));
        let mut out = Vec::new();
        for (id, o) in objects {
            if o.kind != kind::BILLBOARD {
                continue;
            }
            let Some(fid) = o.sprite_file else {
                continue;
            };
            let e = self.seen.entry(*id).or_insert((fid, now));
            if e.0 != fid {
                *e = (fid, now);
            }
            let seen_ms = e.1;
            if o.flags & FLAG_VISIBLE == 0 {
                continue;
            }
            let Some(file) = name(fid).filter(|f| !f.trim().is_empty()) else {
                continue;
            };
            let flags2 = o.flag_words.map_or(0, |w| w.1);
            let scale = o.scale.map_or([1.0, 1.0], |s| [s[0], s[1]]);
            let colour = o.color.map_or([255; 4], |c| [c[0], c[1], c[2], c[3]]);
            out.push(Billboard {
                obj: *id,
                file,
                pos: o.pos.unwrap_or(o.create_pos),
                rot: o.rot.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                scale,
                colour,
                flags: o.flags,
                flags2,
                seen_ms,
            });
        }
        self.last = out.len();
        self.most = self.most.max(out.len());
        out
    }
}

/// The blend of a billboard's flag word 2.
// EVIDENCE: engine+0x1abc80
pub fn blend_of(flags2: u16) -> Blend {
    if flags2 & FLAG2_ADDITIVE != 0 {
        Blend::Additive
    } else if flags2 & FLAG2_MULTIPLY != 0 {
        Blend::Multiply
    } else {
        Blend::Alpha
    }
}

fn unit_quat([x, y, z, w]: [f32; 4]) -> Quat {
    let q = Quat::from_xyzw(x, y, z, w);
    if q.is_finite() && q.length_squared() > 1e-12 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}

/// The frame a billboard draws `ms` after its creation: the texture path and its width and height in texels;
/// `None` draws nothing (no frame, or the texture does not load: the 2009 sprite loader refuses a missing file).
pub type FrameLookup<'a> = dyn FnMut(&str, u32) -> Option<(String, f32, f32)> + 'a;

/// The quads of the billboards at `now_ms` for `camera`: world-space quads, and eye-space quads of eye-attached
/// sprites.
// EVIDENCE: engine+0x1af730, engine+0x1b0b80 (see the module notes)
// DC-DEFAULT: CLI-522 (proposed) depth test always on, a frame without a texture draws nothing, the sprite clock from
// the first sight
pub fn quads(
    list: &[Billboard],
    camera: &dc_render::Camera,
    now_ms: u32,
    frame: &mut FrameLookup<'_>,
) -> (Vec<Quad>, Vec<Quad>) {
    let mut world = Vec::new();
    let mut view = Vec::new();
    for b in list {
        let pos = Vec3::from(b.pos);
        let eye = b.flags & FLAG_EYE != 0;
        let rotatable = b.flags & FLAG_ROTATABLE != 0;
        let (right, up) = if rotatable {
            let q = unit_quat(b.rot);
            (q * Vec3::X, q * Vec3::Y)
        } else if eye {
            (Vec3::X, Vec3::Y)
        } else {
            (camera.rot * Vec3::X, camera.rot * Vec3::Y)
        };
        if !rotatable {
            let depth = if eye {
                pos.z
            } else {
                (pos - camera.pos).dot(camera.forward())
            };
            if !eye && depth < NEAR_CUT {
                continue;
            }
        }
        let Some((tex, w, h)) = frame(&b.file, now_ms.saturating_sub(b.seen_ms)) else {
            continue;
        };
        let [r, g, bl, a] = b.colour.map(|c| c as f32 / 255.0);
        let q = Quad {
            centre: pos,
            u: right * (w * b.scale[0]),
            v: up * (h * b.scale[1]),
            colour: [r, g, bl, a],
            texture: Some(tex),
            blend: blend_of(b.flags2),
            depth_test: true,
        };
        if eye {
            view.push(q);
        } else {
            world.push(q);
        }
    }
    (world, view)
}

/// [`quads`] with the effects' texture store (sprite frames from the ROM, texture sizes from the decoded images).
pub fn quads_with_store(
    list: &[Billboard],
    camera: &dc_render::Camera,
    now_ms: u32,
    store: &mut TextureStore,
) -> (Vec<Quad>, Vec<Quad>) {
    quads(list, camera, now_ms, &mut |file, ms| {
        let path = store.frame(file, ms)?;
        let img = store.image(&path)?;
        Some((path, img.width as f32, img.height as f32))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(file: u16, flags: u16, flags2: u16) -> ClientObject {
        let mut o = ClientObject::default();
        o.kind = kind::BILLBOARD;
        o.pos = Some([100.0, 50.0, 300.0]);
        o.flags = flags;
        o.flag_words = Some((flags, flags2, 1, 0));
        o.sprite_file = Some(file);
        o.scale = Some([0.5, 0.25, 1.0]);
        o.color = Some([255, 128, 0, 255, 0]);
        o
    }

    fn name(f: u16) -> Option<String> {
        (f == 7).then(|| "SPRITES\\GLOW.SPR".to_string())
    }

    /// A 32 x 16 texture for every file; frames advance every 100 ms (two frames).
    fn lookup(file: &str, ms: u32) -> Option<(String, f32, f32)> {
        Some((format!("{file}#{}", (ms / 100) % 2), 32.0, 16.0))
    }

    /// The server's sprite (the reference stack's flag words 0x0081 / 0x0002) becomes one billboard with its file, scale,
    /// colour and first sight; invisible ones and other kinds are not billboards.
    #[test]
    fn billboards_come_from_kind_3_objects() {
        let mut objects = BTreeMap::new();
        objects.insert(40u16, object(7, 0x0081, 0x0002));
        objects.insert(41u16, object(7, 0x0080, 0x0002));
        let mut model = object(7, 0x0081, 0);
        model.kind = kind::MODEL;
        objects.insert(42u16, model);
        let mut t = BillboardTracker::default();
        let list = t.collect(&objects, &name, 1000);
        assert_eq!(list.len(), 1);
        let b = &list[0];
        assert_eq!(
            (
                b.obj,
                b.file.as_str(),
                b.scale,
                b.colour,
                b.flags2,
                b.seen_ms
            ),
            (
                40,
                "SPRITES\\GLOW.SPR",
                [0.5, 0.25],
                [255, 128, 0, 255],
                2,
                1000
            )
        );
        // the first sight stays; a new file restarts it
        assert_eq!(t.collect(&objects, &name, 2000)[0].seen_ms, 1000);
        objects.get_mut(&40).unwrap().sprite_file = Some(8);
        assert!(
            t.collect(&objects, &name, 3000).is_empty(),
            "file 8 unnamed"
        );
        objects.get_mut(&40).unwrap().sprite_file = Some(7);
        assert_eq!(t.collect(&objects, &name, 4000)[0].seen_ms, 4000);
    }

    fn camera(yaw: f32, pitch: f32) -> dc_render::Camera {
        dc_render::Camera::looking(Vec3::new(100.0, 50.0, 0.0), yaw, pitch, 1.5, 1.1)
    }

    fn board(flags: u16, flags2: u16) -> Billboard {
        Billboard {
            obj: 40,
            file: "S.SPR".into(),
            pos: [100.0, 50.0, 300.0],
            rot: [0.0, 0.0, 0.0, 1.0],
            scale: [0.5, 0.25],
            colour: [255, 128, 0, 255],
            flags,
            flags2,
            seen_ms: 1000,
        }
    }

    /// The 2009 camera-facing quad: centred on the object, the half width along the camera's right axis = texture
    /// width x scale x, the half height along its up axis = texture height x scale y, for any camera rotation;
    /// additive with flag word 2 bit 0x2; the sprite frame from the object's first sight.
    #[test]
    fn a_billboard_faces_the_camera_at_its_place() {
        for (yaw, pitch) in [(0.0f32, 0.0f32), (0.3, -0.4), (-0.2, 0.5)] {
            let cam = camera(yaw, pitch);
            let (w, v) = quads(&[board(0x0081, 0x0002)], &cam, 1150, &mut lookup);
            assert!(v.is_empty());
            let q = &w[0];
            assert_eq!(q.centre, Vec3::new(100.0, 50.0, 300.0));
            let right = cam.rot * Vec3::X;
            let up = cam.rot * Vec3::Y;
            assert!((q.u - right * 16.0).length() < 1e-4, "{:?}", q.u);
            assert!((q.v - up * 4.0).length() < 1e-4, "{:?}", q.v);
            // the quad's plane faces the camera: its normal is the camera's forward axis
            let n = q.u.cross(q.v).normalize();
            assert!(n.dot(cam.forward()).abs() > 0.9999, "normal {n:?}");
            assert_eq!(q.blend, Blend::Additive);
            assert_eq!(
                q.texture.as_deref(),
                Some("S.SPR#1"),
                "150 ms after its sight"
            );
            assert_eq!(q.colour, [1.0, 128.0 / 255.0, 0.0, 1.0]);
        }
        assert_eq!(blend_of(0), Blend::Alpha);
        assert_eq!(blend_of(4), Blend::Multiply);
    }

    /// Nearer than 7 units along the camera's forward axis (or behind the camera) a camera-facing sprite is not
    /// drawn; a rotatable one (flag 0x08) keeps its own axes and has no near cut; an eye-attached one (0x40) goes to
    /// the eye-space list with the identity axes.
    #[test]
    fn near_cut_rotatable_and_eye_sprites() {
        let cam = camera(0.0, 0.0);
        let mut b = board(0x0081, 0);
        b.pos = [100.0, 50.0, 6.5];
        assert!(quads(&[b.clone()], &cam, 0, &mut lookup).0.is_empty());
        b.pos = [100.0, 50.0, 7.0];
        assert_eq!(quads(&[b.clone()], &cam, 0, &mut lookup).0.len(), 1);
        b.pos = [100.0, 50.0, -50.0];
        assert!(quads(&[b.clone()], &cam, 0, &mut lookup).0.is_empty());
        let mut r = board(0x0089, 0);
        r.rot = Quat::from_rotation_y(1.0).to_array();
        r.pos = [100.0, 50.0, 2.0];
        let (w, _) = quads(&[r], &cam, 0, &mut lookup);
        let q = &w[0];
        assert!((q.u - Quat::from_rotation_y(1.0) * Vec3::X * 16.0).length() < 1e-4);
        let (w, v) = quads(&[board(0x00c1, 0)], &cam, 0, &mut lookup);
        assert!(w.is_empty());
        assert_eq!((v[0].u, v[0].v), (Vec3::X * 16.0, Vec3::Y * 4.0));
        // no frame (missing texture): nothing
        let (w, _) = quads(&[board(0x0081, 0)], &cam, 0, &mut |_, _| None);
        assert!(w.is_empty());
    }
}
