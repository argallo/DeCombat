//! The camera jar of a hit on the local player (`GS_DAMAGE_TAKEN` 0x74), the way the 2009 client plays it.
//!
//! When a 0x74 arrives with a non-zero `damage`, the client turns the hit direction into one of 12 sectors of the
//! player's body frame (pitch 0, the view yaw): `k = trunc((atan2(d . F, d . R) + pi) x 12 / 2pi)` with `d` the
//! normalised `from_dir` (the shot's direction), `F` / `R` the body's forward and right axes. A shot from the front
//! (`d . F < 0`) lands in sectors 2..4, from behind in 8..10, from the right in 0, 1, 11, from the left in 5..7.
//! Then, if the damage type (the byte before `mode`) jars the camera (the ROM damage-type table, key `JarCamera`):
//! - **pitch**: front -2 degrees (the view kicks up), behind +2 degrees (down);
//! - **roll**: left +2 degrees, right -2 degrees;
//!
//! each a wave on the camera's angle offsets: a rise over `CamDamageTime1` (0.1 s) and a fall over
//! `CamDamageTime2` (0.25 s), both on the curve `c(x) = x` for `x < 0.5`, else `(1 - cos(pi x)) / 2`
//! (`v = amp x c(t / t1)` rising, `v = amp - amp x c(t / t2)` falling). A new hit replaces the running wave.
//!
//! The offsets move the camera, not the view angles the client reports: the 0x64 / 0x82 `aim` stay the view
//! controller's angles, while the shots leave along the camera's rotation (the 2009 fire code reads the camera
//! object), so a jarred view also throws its next shots up or down by the offset.
//!
//! The whole branch sits behind the client's "Reality" option (a console value registered with default 0, shown in
//! the options menu next to "Tracer Bullet" and "Blood Effect"): with it off the sector is never computed, so
//! neither the damage indicator nor the camera jar runs. DeCombat keeps the 2009 default (off,
//! [`ArmsConfig::reality`](crate::arms::ArmsConfig)); OBSERVED: the real 2009 client of capture rust6 (52 bullet
//! hits from the front) shot within 0.17 degrees of its aim right after the hits, i.e. its option was off.
//!
//! The wave runs on the session clock: its value is a function of the time since the hit (the 2009 client stepped it
//! once per frame, one frame late, and restarted the fall at the frame after the rise ended).
// EVIDENCE: cshell+0x1f7d20 (0x74: read order, the "Reality" gate at VA 0x375b47fc, the sector, the damage-type test
// cshell+0x282c20, the gates `damage` != 0, sector != 12, camera helper +0x54 == 0, CamDamage > 0; the parameter block
// built by cshell+0x1f1750, channel 0 filled for sectors 8..10 (+) / 2..4 (-) and handed to cshell+0x2b1530, channel 2
// for 5..7 (+) / 0, 1, 11 (-) to cshell+0x2b15a0); DISASM cshell+0x1f7eab..0x1f7f4b (the axes: cshell+0x11b8a0 third
// column = forward, cshell+0x11b800 first column = right of the body rotation cshell+0x1f5d10; fpatan, + pi,
// x 1.9098593, ftol) and cshell+0x1f80f4..0x1f8308 (the sector ranges and the console values used)
// DISASM: cshell+0x1f7322..0x1f7416 (console values and defaults: CamDamage 1, CamDamagePitch 1, CamDamageRoll 1,
// CamDamageTime1 0.1, CamDamageTime2 0.25, CamDamageMinPitchVal 2, CamDamageMaxPitchVal 20, CamDamageMinRollVal 2,
// CamDamageMaxRollVal 20), cshell+0x2b0012..0x2b0026 ("Reality", default 0)
// EVIDENCE: cshell+0x2b1530 / +0x2b15a0 (copy of the whole block when the slot's amplitudes sum below 2.0 / 1.0),
// cshell+0x2b0f40 (one channel: rise / fall phases), cshell+0x19f360 / +0x19f010 (curve 2), cshell+0x2b17e0 (sum of the
// slots: channels 0 / 1 / 2 = pitch / yaw / roll by the debug texts), cshell+0x2b1250 (clamp by CamMaxPitchOffset /
// CamMaxYawOffset / CamMaxRollOffset, defaults 15, cshell+0x2b1140)
// DISASM: cshell+0x1fafba..0x1fb266 (camera rotation = euler(view + offsets), the shot direction source
// cshell+0x234d50 reads that camera object's rotation)
// DISASM: cshell+0x282a20..0x282b04 (damage-type rows: section names from the table at VA 0x375573f0, key JarCamera)

use std::path::Path;

use dc_bute::ButeDoc;
use dc_rez::RezVfs;

use crate::recoil::rad;

/// The 2009 client's console values of the damage jar (code defaults; no ROM file sets them).
pub mod console {
    pub const CAM_DAMAGE: f32 = 1.0;
    pub const PITCH_ON: f32 = 1.0;
    pub const ROLL_ON: f32 = 1.0;
    pub const TIME1_S: f32 = 0.1;
    pub const TIME2_S: f32 = 0.25;
    pub const PITCH_DEG: f32 = 2.0;
    pub const ROLL_DEG: f32 = 2.0;
    /// `CamMaxPitchOffset` / `CamMaxYawOffset` / `CamMaxRollOffset`: the clamp of the summed offsets (a raw value
    /// compared with radians, so it never bites).
    pub const MAX_OFFSET: f32 = 15.0;
    /// "Reality" (the options-menu effect setting) registers as 0.
    pub const REALITY: bool = false;
    /// The curve kind the jar uses for both phases.
    pub const CURVE: u8 = 2;
    /// The sector value that means "no direction".
    pub const NO_SECTOR: u8 = 12;
}

/// Sectors per turn of the hit direction.
// DISASM: cshell+0x1f7f36 (x 1.9098593 = 12 / 2pi)
pub const SECTORS_PER_RAD: f32 = 1.909_859_3;

/// The curve of a wave phase at `x` in 0..1.
// EVIDENCE: cshell+0x19f010 (curve 2: x below 0.5, else (1 - sin(pi x + pi / 2)) / 2), cshell+0x19f050 (curve 3: the
// other half), cshell+0x19f360 (curves 0 and 1 are not read here)
pub fn curve(kind: u8, x: f32) -> f32 {
    let s = |x: f32| (1.0 - (std::f32::consts::PI * x + std::f32::consts::FRAC_PI_2).sin()) * 0.5;
    match kind {
        2 if x < 0.5 => x,
        2 => s(x),
        _ if x < 0.5 => s(x),
        _ => x,
    }
}

/// The sector (0..=12) of a hit with direction `from_dir` for a player whose body faces `body_yaw` (pitch 0);
/// `None` when the direction is empty.
pub fn hit_sector(from_dir: [f32; 3], body_yaw: f32) -> Option<u8> {
    let n =
        (from_dir[0] * from_dir[0] + from_dir[1] * from_dir[1] + from_dir[2] * from_dir[2]).sqrt();
    if !(n.is_finite() && n > 0.0) {
        return None;
    }
    let d = [from_dir[0] / n, from_dir[1] / n, from_dir[2] / n];
    // body pitch 0: forward (sin y, 0, cos y), right (cos y, 0, -sin y) (dc-sim: yaw 0 looks along +Z)
    let (s, c) = body_yaw.sin_cos();
    let f = d[0] * s + d[2] * c;
    let r = d[0] * c - d[2] * s;
    let a = f.atan2(r) + std::f32::consts::PI;
    Some(((a * SECTORS_PER_RAD) as u32).min(console::NO_SECTOR as u32) as u8)
}

/// One channel of a camera wave.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Channel {
    /// Peak offset (radians).
    pub amp: f32,
    /// Rise and fall times (s).
    pub t1: f32,
    pub t2: f32,
    pub curve: u8,
}

impl Channel {
    /// The offset `t` seconds after the wave started (0 once it ended).
    // DC-DEFAULT: CLI-314 (proposed) evaluated at the session time since the hit (the 2009 client stepped the wave per
    // frame, one frame late, and began the fall phase at the frame after the rise ended)
    pub fn value(&self, t: f32) -> f32 {
        if self.amp == 0.0 || t.is_nan() || t < 0.0 {
            return 0.0;
        }
        let rise = self.t1.max(0.0);
        if self.t1 > 0.0 && t <= rise {
            return self.amp * curve(self.curve, t / self.t1);
        }
        let tf = t - rise;
        if self.t2 > 0.0 && tf <= self.t2 {
            return self.amp - curve(self.curve, tf / self.t2) * self.amp;
        }
        0.0
    }

    /// The channel still runs `t` seconds after the start (the 2009 client zeroes its amplitude at the end).
    pub fn running(&self, t: f32) -> bool {
        self.amp != 0.0 && t <= self.t1.max(0.0) + self.t2.max(0.0)
    }
}

/// A camera wave slot: pitch, yaw and roll channels (the position channels are not used by the jar).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CamWave {
    pub start_ms: f64,
    pub pitch: Channel,
    pub yaw: Channel,
    pub roll: Channel,
}

impl CamWave {
    fn amp_sum(&self, now_ms: f64) -> f32 {
        let t = ((now_ms - self.start_ms) / 1000.0) as f32;
        [self.pitch, self.yaw, self.roll]
            .iter()
            .filter(|c| c.running(t))
            .map(|c| c.amp.abs())
            .sum()
    }
}

/// The camera's angle offsets of a moment (radians; pitch positive looks down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CamAngles {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

/// Which damage types jar the camera (by damage-type number), read from the user's ROM.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DamageJar {
    jar: Vec<bool>,
}

/// Where the damage-type rows live.
// OBSERVED: ROM table ATTRIBUTES.rez entry ATTRIBUTES\DAMAGETYPES.TXT, key JarCamera (names only)
pub const DAMAGE_TYPES_REZ: &str = "ATTRIBUTES.rez";
pub const DAMAGE_TYPES_ENTRY: &str = "ATTRIBUTES\\DAMAGETYPES.TXT";
pub const KEY_JAR: &str = "JarCamera";
/// The client binary that names the damage types, its name table (VA, entries) and the pinned digest of the table's
/// pointer bytes (only the digest is committed).
// DISASM: cshell+0x282a6c (section name = the pointer at VA 0x375573f0 + 4 x type), cshell+0x282af2 (42 types)
pub const SHELL_FILE: &str = "Game/CShell.dll";
pub const NAME_TABLE_VA: u32 = 0x3755_73f0;
pub const NAME_COUNT: usize = 42;
pub const NAME_TABLE_SHA256: &str =
    "74bc66ce403bd62b305e7968869a5fad632164aa70b99b11ae6b1e897f882c3f";

impl DamageJar {
    /// A table from explicit flags (tests).
    pub fn from_flags(jar: Vec<bool>) -> DamageJar {
        DamageJar { jar }
    }

    /// The table of the user's ROM: the damage-type names from the client binary, their rows from the damage-type
    /// file. Empty (nothing jars) when either is missing or the binary is another build.
    // DC-DEFAULT: CLI-317 (proposed) without the two tables nothing jars (the 2009 lookup answers 0 for a type it has
    // no row for)
    pub fn open(rom: &Path) -> DamageJar {
        Self::try_open(rom).unwrap_or_default()
    }

    fn try_open(rom: &Path) -> Option<DamageJar> {
        let names = shell_names(rom)?;
        let vfs = RezVfs::new(rom);
        let doc = ButeDoc::try_from_vfs(&vfs, DAMAGE_TYPES_REZ, DAMAGE_TYPES_ENTRY)
            .ok()
            .flatten()?;
        Some(Self::from_names(&doc, &names))
    }

    /// The flags of `names` (one per damage type) read from a parsed damage-type file; a missing section or key
    /// reads 0, as the client's reader does.
    pub fn from_names(doc: &ButeDoc, names: &[String]) -> DamageJar {
        let jar = names
            .iter()
            .map(|n| {
                doc.iter()
                    .find(|(tag, _)| tag.eq_ignore_ascii_case(n))
                    .and_then(|(_, sec)| {
                        sec.iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case(KEY_JAR))
                            .map(|(_, v)| v.trim().parse::<f32>().unwrap_or(0.0) != 0.0)
                    })
                    .unwrap_or(false)
            })
            .collect();
        DamageJar { jar }
    }

    /// Does damage type `t` jar the camera.
    // EVIDENCE: cshell+0x282c20 (row lookup by type, 0 when no row)
    pub fn jars(&self, t: u8) -> bool {
        self.jar.get(t as usize).copied().unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.jar.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jar.is_empty()
    }
}

/// The damage-type names of the user's client binary (read at run time; `None` for another build).
pub fn shell_names(rom: &Path) -> Option<Vec<String>> {
    let pe = dc_rom::PeFile::from_file(&rom.join(SHELL_FILE)).ok()?;
    let rva = NAME_TABLE_VA.checked_sub(pe.base)?;
    let raw = pe.rva(rva, NAME_COUNT * 4).ok()?.to_vec();
    if dc_rom::sha256_hex(&raw) != NAME_TABLE_SHA256 {
        return None;
    }
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|c| pe.cstr(u32::from_le_bytes(*c)).ok())
        .collect()
}

/// The camera jar state of the local player.
#[derive(Debug, Clone, Default)]
pub struct CameraFx {
    /// The "Reality" option.
    pub reality: bool,
    pub jar: DamageJar,
    /// The running wave (the camera effect slot the jar writes).
    pub slot: Option<CamWave>,
    /// Jars started (tests, reports).
    pub jars: u32,
    /// The jars started: (session ms, sector), the latest [`HISTORY`] kept.
    pub history: Vec<(u32, u8)>,
}

/// How many started jars [`CameraFx::history`] keeps.
pub const HISTORY: usize = 1024;

/// The wave a jar in sector `k` starts at `start_ms`: pitch for the front and rear sectors, roll for the sides
/// (`None` for sector 12).
pub fn wave_of(start_ms: f64, k: u8) -> Option<CamWave> {
    let phase = |amp: f32| Channel {
        amp,
        t1: console::TIME1_S,
        t2: console::TIME2_S,
        curve: console::CURVE,
    };
    let (p, r) = (rad(console::PITCH_DEG), rad(console::ROLL_DEG));
    let mut w = CamWave {
        start_ms,
        ..CamWave::default()
    };
    match k {
        8..=10 if console::PITCH_ON > 0.0 => w.pitch = phase(p),
        2..=4 if console::PITCH_ON > 0.0 => w.pitch = phase(-p),
        5..=7 if console::ROLL_ON > 0.0 => w.roll = phase(r),
        0 | 1 | 11 if console::ROLL_ON > 0.0 => w.roll = phase(-r),
        12.. => return None,
        _ => {}
    }
    Some(w)
}

impl CameraFx {
    pub fn new(reality: bool, jar: DamageJar) -> CameraFx {
        CameraFx {
            reality,
            jar,
            ..CameraFx::default()
        }
    }

    /// Writes `w` into the slot when the slot's running amplitudes sum below `limit`.
    fn place(&mut self, w: CamWave, now_ms: f64, limit: f32) {
        if self.slot.map_or(0.0, |s| s.amp_sum(now_ms)) < limit {
            self.slot = Some(w);
        }
    }

    /// A 0x74 at `now_ms` for a live first-person player whose body faces `body_yaw`: the sector (`None` without
    /// the option or a direction) and, when every gate passes, a new wave.
    pub fn jar_hit(
        &mut self,
        now_ms: f64,
        from_dir: [f32; 3],
        damage_type: u8,
        damage: u32,
        body_yaw: f32,
    ) -> Option<u8> {
        if !self.reality {
            return None;
        }
        let k = hit_sector(from_dir, body_yaw)?;
        if damage == 0
            || k == console::NO_SECTOR
            || !self.jar.jars(damage_type)
            || console::CAM_DAMAGE <= 0.0
        {
            return Some(k);
        }
        let Some(full) = wave_of(now_ms, k) else {
            return Some(k);
        };
        // the pitch block first (channel 0 only), then the whole block with the roll channel
        if console::PITCH_ON > 0.0 {
            let w = CamWave {
                roll: Channel::default(),
                ..full
            };
            self.place(w, now_ms, 2.0);
        }
        if console::ROLL_ON > 0.0 {
            self.place(full, now_ms, 1.0);
        }
        self.jars += 1;
        if self.history.len() >= HISTORY {
            self.history.remove(0);
        }
        self.history
            .push((now_ms.max(0.0).min(u32::MAX as f64) as u32, k));
        Some(k)
    }

    /// The camera offsets at `now_ms`.
    pub fn angles(&self, now_ms: f64) -> CamAngles {
        let Some(w) = self.slot else {
            return CamAngles::default();
        };
        let t = ((now_ms - w.start_ms) / 1000.0) as f32;
        let m = console::MAX_OFFSET;
        CamAngles {
            pitch: w.pitch.value(t).clamp(-m, m),
            yaw: w.yaw.value(t).clamp(-m, m),
            roll: w.roll.value(t).clamp(-m, m),
        }
    }

    /// Back to rest (spawn, death).
    pub fn clear(&mut self) {
        self.slot = None;
    }
}

/// The shot / camera rotation of view angles plus offsets: `Ry(yaw) x Rx(pitch) x Rz(roll)` as `[x, y, z, w]`.
// SPEC: MOVEMENT_2009.md §13.2 step 7 (the euler builder cshell+0x1b700, order PLAUSIBLY yaw, pitch, roll)
pub fn camera_quat(yaw: f32, pitch: f32, roll: f32) -> [f32; 4] {
    let q = dc_sim::Quat::from_yaw_pitch(yaw, pitch);
    let (s, c) = (roll * 0.5).sin_cos();
    // q x (0, 0, s, c)
    [
        q.x * c + q.y * s,
        q.y * c - q.x * s,
        q.z * c + q.w * s,
        q.w * c - q.z * s,
    ]
}

/// The drawn first-person camera: the view plus the jar's offsets, the eye moved back along it by the recoil's
/// kick-back. The view the client reports is not changed.
// DISASM: cshell+0x1fafba..0x1fb266 (camera rotation = euler(view + offsets)); EVIDENCE: cshell+0x1f9c40 (eye -=
// forward x 10 x back, the recoil module's `eye_back`)
// DC-DEFAULT: CLI-315 (proposed) the 2009 camera's per-frame smoothing of the drawn pitch and yaw (cshell+0x1fafa0:
// 1 / 1.5 of the way per frame, snap after 500 ms) is not modelled: it is a frame-rate effect by construction
pub fn own_camera(
    eye: dc_sim::V3,
    view: &crate::look::ViewAngles,
    fx: CamAngles,
    back: f32,
) -> (dc_sim::V3, crate::look::ViewAngles) {
    let v = crate::look::ViewAngles {
        yaw: view.yaw + fx.yaw,
        pitch: view.pitch + fx.pitch,
    };
    if back == 0.0 || !back.is_finite() {
        return (eye, v);
    }
    let f = dc_sim::Quat::from_yaw_pitch(v.yaw, v.pitch).forward();
    (
        dc_sim::V3::new(eye.x - f.x * back, eye.y - f.y * back, eye.z - f.z * back),
        v,
    )
}

/// The camera place moved by an effect shake's offset ([`crate::fx::FxRunner::shake_at`], w2-fx): the offset adds to
/// the camera object's position for the frame drawn; the next frame starts again from the unshaken place. A
/// non-finite offset leaves the place as it is.
// DISASM: clientfx+0x4044..0x40c5 (the camera object's position + the offset, written back through engine method +0xc4)
pub fn shaken_eye(eye: glam::Vec3, offset: glam::Vec3) -> glam::Vec3 {
    if offset.is_finite() {
        eye + offset
    } else {
        eye
    }
}

/// Turns a render camera about its own forward axis by the jar's roll.
// SPEC: MOVEMENT_2009.md §13.2 step 7 (roll applied first, about +Z)
pub fn roll_camera(c: &mut dc_render::Camera, roll: f32) {
    if roll != 0.0 && roll.is_finite() {
        c.rot *= glam::Quat::from_rotation_z(roll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx() -> CameraFx {
        let mut j = vec![false; NAME_COUNT];
        j[2] = true;
        CameraFx::new(true, DamageJar::from_flags(j))
    }

    #[test]
    fn sectors_follow_the_attacker_side() {
        // facing +Z (yaw 0): a shot from the front travels along -Z (directions off the sector borders)
        assert_eq!(hit_sector([0.1, 0.0, -1.0], 0.0), Some(3));
        assert_eq!(hit_sector([0.1, 0.0, 1.0], 0.0), Some(8));
        assert_eq!(hit_sector([-0.1, 0.0, 1.0], 0.0), Some(9));
        // from the right (+X side) the shot travels along -X
        assert_eq!(hit_sector([-1.0, 0.0, 0.1], 0.0), Some(11));
        assert_eq!(hit_sector([-1.0, 0.0, -0.1], 0.0), Some(0));
        assert_eq!(hit_sector([1.0, 0.0, 0.1], 0.0), Some(6));
        // turning the body turns the frame
        let y = 1.0f32;
        assert_eq!(
            hit_sector([0.1 * y.cos() - y.sin(), 0.0, -y.cos() - 0.1 * y.sin()], y),
            Some(3)
        );
        assert_eq!(hit_sector([0.0; 3], 0.0), None);
        // the magnitude does not matter
        assert_eq!(hit_sector([0.02, -0.01, -0.27], 0.0), Some(3));
    }

    #[test]
    fn a_front_hit_kicks_the_view_up_and_returns() {
        let mut f = fx();
        assert_eq!(f.jar_hit(1000.0, [0.02, 0.0, -0.27], 2, 27, 0.0), Some(3));
        let at = |ms: f64| f.angles(1000.0 + ms).pitch.to_degrees();
        assert_eq!(at(0.0), 0.0);
        assert!((at(25.0) + 0.5).abs() < 1e-4, "{}", at(25.0));
        assert!((at(100.0) + 2.0).abs() < 1e-4, "peak {}", at(100.0));
        assert!((at(225.0) + 1.0).abs() < 1e-3, "{}", at(225.0));
        assert_eq!(at(351.0), 0.0);
        assert_eq!(f.angles(1100.0).roll, 0.0);
    }

    #[test]
    fn side_hits_roll_and_rear_hits_kick_down() {
        let mut f = fx();
        f.jar_hit(0.0, [1.0, 0.0, 0.1], 2, 10, 0.0);
        assert!((f.angles(100.0).roll - rad(2.0)).abs() < 1e-6);
        assert_eq!(f.angles(100.0).pitch, 0.0);
        f.jar_hit(500.0, [0.1, 0.0, 1.0], 2, 10, 0.0);
        assert!((f.angles(600.0).pitch - rad(2.0)).abs() < 1e-6);
        assert_eq!(f.angles(600.0).roll, 0.0, "a new hit replaces the wave");
    }

    #[test]
    fn gates() {
        // the option off: nothing at all
        let mut off = CameraFx::new(false, fx().jar);
        assert_eq!(off.jar_hit(0.0, [0.1, 0.0, -1.0], 2, 27, 0.0), None);
        assert_eq!(off.angles(100.0), CamAngles::default());
        // a damage type that does not jar, zero damage
        let mut f = fx();
        assert_eq!(f.jar_hit(0.0, [0.1, 0.0, -1.0], 5, 27, 0.0), Some(3));
        assert_eq!(f.jar_hit(0.0, [0.1, 0.0, -1.0], 2, 0, 0.0), Some(3));
        assert_eq!(f.jars, 0);
        assert_eq!(f.angles(100.0), CamAngles::default());
    }

    #[test]
    fn the_drawn_camera_adds_the_offsets_and_moves_the_eye_back() {
        let view = crate::look::ViewAngles {
            yaw: 0.0,
            pitch: 0.0,
        };
        let fx = CamAngles {
            pitch: -0.03,
            yaw: 0.01,
            roll: 0.02,
        };
        let (eye, v) = own_camera(dc_sim::V3::new(1.0, 2.0, 3.0), &view, fx, 5.0);
        assert_eq!((v.yaw, v.pitch), (0.01, -0.03));
        // 5 units back along the drawn view (forward is about +Z, slightly up)
        assert!(
            (eye.z - (3.0 - 5.0 * (0.03f32.cos() * 0.01f32.cos()))).abs() < 1e-4,
            "{eye:?}"
        );
        assert!(eye.y < 2.0, "back and down when the view looks up: {eye:?}");
        let mut c = dc_render::Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
        let f0 = c.forward();
        roll_camera(&mut c, 0.3);
        assert!(
            (c.forward() - f0).length() < 1e-6,
            "roll keeps the forward axis"
        );
    }

    #[test]
    fn curve_two_is_continuous() {
        assert_eq!(curve(2, 0.0), 0.0);
        assert!((curve(2, 0.5) - 0.5).abs() < 1e-6);
        assert!((curve(2, 1.0) - 1.0).abs() < 1e-6);
        assert!((curve(2, 0.49) - 0.49).abs() < 1e-6);
    }

    #[test]
    fn camera_quat_keeps_the_forward_axis_under_roll() {
        let a = camera_quat(0.3, -0.2, 0.0);
        let b = camera_quat(0.3, -0.2, 0.5);
        let fwd = |q: [f32; 4]| {
            dc_sim::Quat {
                x: q[0],
                y: q[1],
                z: q[2],
                w: q[3],
            }
            .forward()
        };
        let (fa, fb) = (fwd(a), fwd(b));
        assert!(
            (fa.x - fb.x).abs() < 1e-6 && (fa.y - fb.y).abs() < 1e-6 && (fa.z - fb.z).abs() < 1e-6
        );
        let q = dc_sim::Quat::from_yaw_pitch(0.3, -0.2);
        assert!((a[0] - q.x).abs() < 1e-7 && (a[3] - q.w).abs() < 1e-7);
    }

    #[test]
    fn shake_moves_the_eye_by_the_offset_only() {
        let e = glam::Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(
            shaken_eye(e, glam::Vec3::new(0.5, -1.0, 0.0)),
            glam::Vec3::new(1.5, 1.0, 3.0)
        );
        assert_eq!(shaken_eye(e, glam::Vec3::ZERO), e);
        assert_eq!(shaken_eye(e, glam::Vec3::new(f32::NAN, 0.0, 0.0)), e);
    }
}
