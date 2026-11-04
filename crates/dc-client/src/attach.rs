//! The mod models drawn on a weapon (silencer, sight, magazine) and the muzzle effect a muzzle mod picks.
//!
//! The 2009 weapon object keeps three attachment records, one per mod socket (`Kinds` 0 muzzle, 1 sight, 2
//! magazine). When a part is filed (cshell+0x2378xx, the weapon object's attach step, reached from the grant's
//! `+0x104(part, 0, 1)`), the record's mod row is looked up; a row whose `Socket` names no socket draws nothing.
//! The socket is looked up by that name on the first-person model (model interface +0x18); when the model has it,
//! the attachment's model is the mod row's `AttachModel<k>` with its skins `AttachSkin<k>_<j>` and the row's
//! `AttachRenderStyle<j>`, where `k` is the weapon row's `ModModel<i>` for the row's `ModName<i>` that names this mod
//! (cshell+0x172b20; none, or `k` outside 0..9: no file), scaled by `AttachScale`. Every frame (cshell+0x234380) the
//! attachment sits at the socket's world transform moved by the record's offset in the socket's frame
//! (`position + right x off.x + up x off.y + forward x off.z`) and turned by the record's angle offset added to the
//! socket's Euler angles ([`turned`]); the muzzle
//! record's offset is the weapon row's `MuzzlePVPosOff` / `MuzzlePVRotOff`, the sight record's `ScopePVPosOff` /
//! `ScopePVRotOff`, the magazine record has none (cshell+0x23c330).
// EVIDENCE: cshell+0x176f10 (mod row loader: `Socket` -> +0x0c, `HHModelSocket` -> +0x54, `AttachModel<k>` -> +0x1c +
// 4 k, `AttachHHModel<k>` -> +0x58 + 4 k, `AttachSkin<k>_` -> +0x84 + 8 k, `AttachRenderStyle` -> +0xdc,
// `AttachScale` -> +0xf4, `FlashHiderLevel` -> +0x1ac; DISASM 0x177529..0x177a77), cshell+0x17bd20 (weapon row:
// `ModName<i>` -> mod ids +0x264, `ModModel<i>` -> +0x268, `MuzzlePVPosOff` -> +0x344, `MuzzlePVRotOff` -> +0x350,
// `ScopePVPosOff` -> +0x38c, `ScopePVRotOff` -> +0x398, `PVMuzzleFXName` -> +0xcc, `HHMuzzleFXName` -> +0xd0; DISASM
// 0x17c066..0x17c6c3, 0x17d64d), cshell+0x172b20 (the `ModModel` of a mod id), cshell+0x237983..0x237b9e (attach:
// socket by name, model / skins / styles by `ModModel`, scale), cshell+0x23c330..0x23c3a6 (record offsets),
// cshell+0x234380 (per-frame placement: socket transform, `cshell+0x1b800` right / `cshell+0x1b850` up /
// `cshell+0x1b8a0` forward times the offset, Euler angles plus the angle offset)
// OBSERVED: ROM WEAPONS.TXT (the user's loadout: the L96's `Silencer_C_SR` on socket "Flash" of PVMSH_L96A1, the
// pistol's `DotSight_HDS-4_Pistol` on "Breach", sockets "Flash" / "Breach" on every first-person and third-person
// model checked)

use dc_bute::{ButeDoc, SectionMap};
use std::collections::BTreeMap;

/// Mod row keys read here.
// OBSERVED: ROM table keys WEAPONS.TXT [Mod<n>] (names read by the 2009 loader cshell+0x176f10)
pub mod key {
    pub const NAME: &str = "Name";
    pub const SOCKET: &str = "Socket";
    pub const HH_SOCKET: &str = "HHModelSocket";
    pub const MODEL: &str = "AttachModel";
    pub const SKIN: &str = "AttachSkin";
    pub const STYLE: &str = "AttachRenderStyle";
    pub const HH_MODEL: &str = "AttachHHModel";
    pub const HH_SKIN: &str = "AttachHHSkin";
    pub const HH_STYLE: &str = "HHRenderStyle";
    pub const PV_FX: &str = "PVMuzzlefxname";
    pub const HH_FX: &str = "HHMuzzlefxname";
    pub const SCALE: &str = "AttachScale";
    pub const HH_SCALE: &str = "AttachHHModelScale";
    pub const KINDS: &str = "Kinds";
    /// Weapon row keys.
    pub const MOD_NAME: &str = "ModName";
    pub const MOD_MODEL: &str = "ModModel";
    pub const MUZZLE_PV_POS: &str = "MuzzlePVPosOff";
    pub const MUZZLE_PV_ROT: &str = "MuzzlePVRotOff";
    pub const SCOPE_PV_POS: &str = "ScopePVPosOff";
    pub const SCOPE_PV_ROT: &str = "ScopePVRotOff";
    pub const MUZZLE_HH_POS: &str = "MuzzleHHPosOff";
    pub const MUZZLE_HH_ROT: &str = "MuzzleHHRotOff";
    pub const SCOPE_HH_POS: &str = "ScopeHHPosOff";
    pub const SCOPE_HH_ROT: &str = "ScopeHHRotOff";
    pub const ROW_PV_FX: &str = "PVMuzzleFXName";
    pub const ROW_HH_FX: &str = "HHMuzzleFXName";
    /// Weapon row keys of the built-in sight: the mod row the model already shows and whether it is drawn again.
    // DISASM: cshell+0x17c7ce..0x17c7fc (`DefaultScopeAttach` -> row +0xd8; `DefaultScopeVisible`, default 1 -> row
    // +0xdc)
    pub const DEFAULT_SCOPE: &str = "DefaultScopeAttach";
    pub const DEFAULT_SCOPE_VISIBLE: &str = "DefaultScopeVisible";
}

/// `AttachModel<k>` indices a mod row may carry (`ModModel` outside 0..9 draws nothing).
// EVIDENCE: cshell+0x237ad2..0x237ad9 (`0 <= k < 10`)
pub const MAX_MODELS: usize = 10;
/// Skins per attachment model read here.
// DC-DEFAULT: CLI-617 at most 8 skins per model (the ROM rows carry 1..3)
pub const MAX_SKINS: usize = 8;
/// Mod sockets: the muzzle record and the sight record carry the row's offsets.
pub const MUZZLE: usize = 0;
pub const SIGHT: usize = 1;

/// What one `[Mod<n>]` row says about its look.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModLook {
    pub name: String,
    /// `Socket`: the first-person model socket (empty: nothing drawn).
    pub socket: String,
    /// `AttachModel<k>` (empty strings kept so `k` indexes the list).
    pub models: Vec<String>,
    /// `AttachSkin<k>_<j>`.
    pub skins: Vec<Vec<String>>,
    /// `AttachRenderStyle<j>`.
    pub styles: Vec<String>,
    /// `HHModelSocket` and the third-person lists.
    pub hh_socket: String,
    pub hh_models: Vec<String>,
    pub hh_skins: Vec<Vec<String>>,
    pub hh_styles: Vec<String>,
    /// `PVMuzzlefxname` / `HHMuzzlefxname`: the muzzle effect group of a weapon with this mod.
    pub pv_fx: Option<String>,
    pub hh_fx: Option<String>,
    /// `AttachScale`: the per-axis scale of the first-person model (1, 1, 1 when the row has none).
    pub scale: [f32; 3],
    /// `AttachHHModelScale`: the per-axis scale of the third-person model (1, 1, 1 when the row has none).
    pub hh_scale: [f32; 3],
    /// `Kinds`: the weapon socket the mod is filed under (0 muzzle, 1 sight, 2 magazine).
    pub kind: Option<u8>,
}

/// What one `[Weapon<n>]` row says about its mods' look.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponLook {
    /// `(ModName<i>, ModModel<i>)` in row order.
    pub mods: Vec<(String, i32)>,
    pub muzzle_pos: [f32; 3],
    pub muzzle_rot: [f32; 3],
    pub scope_pos: [f32; 3],
    pub scope_rot: [f32; 3],
    /// `MuzzleHHPosOff` / `MuzzleHHRotOff` / `ScopeHHPosOff` / `ScopeHHRotOff`: the same offsets for the
    /// third-person model.
    pub muzzle_hh_pos: [f32; 3],
    pub muzzle_hh_rot: [f32; 3],
    pub scope_hh_pos: [f32; 3],
    pub scope_hh_rot: [f32; 3],
    /// `PVMuzzleFXName` / `HHMuzzleFXName`.
    pub pv_fx: Option<String>,
    pub hh_fx: Option<String>,
    /// `DefaultScopeAttach`: the sight mod row the weapon comes with (empty: none).
    pub default_scope: String,
    /// `DefaultScopeVisible` (1 when the row has none): 0 = the weapon's model already carries that sight, so the
    /// sight attachment is not drawn while the fitted sight is the default one.
    pub default_scope_visible: bool,
}

/// One mod model to draw on the weapon: the model with its skins and styles at a socket of the weapon's model,
/// moved by `pos_off` in the socket's frame and turned by `rot_off` (radians).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponAttach {
    pub mod_id: u16,
    pub socket: String,
    pub model: crate::world::ModelRef,
    pub styles: Vec<String>,
    pub pos_off: [f32; 3],
    pub rot_off: [f32; 3],
    /// Per-axis scale of the model (the mod row's `AttachScale`).
    pub scale: [f32; 3],
}

/// The look of every mod and weapon row.
#[derive(Debug, Clone, Default)]
pub struct AttachTable {
    pub mods: BTreeMap<u16, ModLook>,
    pub weapons: BTreeMap<u16, WeaponLook>,
}

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

fn get<'a>(sec: &'a SectionMap, k: &str) -> Option<&'a String> {
    sec.iter()
        .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
        .map(|(_, v)| v)
}

fn text(sec: &SectionMap, k: &str) -> String {
    get(sec, k)
        .map(|t| t.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default()
}

fn opt_text(sec: &SectionMap, k: &str) -> Option<String> {
    Some(text(sec, k)).filter(|t| !t.is_empty())
}

fn list(sec: &SectionMap, k: &str, max: usize) -> Vec<String> {
    (0..max)
        .map_while(|i| get(sec, &format!("{k}{i}")).map(|_| text(sec, &format!("{k}{i}"))))
        .collect()
}

fn models_and_skins(sec: &SectionMap, model: &str, skin: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let models: Vec<String> = (0..MAX_MODELS)
        .map(|k| text(sec, &format!("{model}{k}")))
        .collect();
    let skins = (0..MAX_MODELS)
        .map(|k| list(sec, &format!("{skin}{k}_"), MAX_SKINS))
        .collect();
    (models, skins)
}

/// `AttachScale` as three floats (a single number scales all three axes); 1, 1, 1 when absent or unreadable.
// EVIDENCE: cshell+0x176f10 (`AttachScale` -> mod row +0xf4, three floats; DISASM 0x177529..0x177a77)
// DC-DEFAULT: CLI-635 (proposed) a missing, unreadable or zero-axis value is 1, 1, 1 (the loader's default was not read)
fn scale_of(sec: &SectionMap, k: &str) -> [f32; 3] {
    let v = get(sec, k);
    let s = crate::fx::row_vec3(v).or_else(|| {
        v.and_then(|t| t.trim().trim_matches('"').trim().parse::<f32>().ok())
            .map(|f| [f; 3])
    });
    match s {
        Some(s) if s.iter().all(|f| f.is_finite() && *f > 0.0) => s,
        _ => [1.0; 3],
    }
}

fn vec3(sec: &SectionMap, k: &str) -> [f32; 3] {
    crate::fx::row_vec3(get(sec, k)).unwrap_or([0.0; 3])
}

impl ModLook {
    pub fn of_section(sec: &SectionMap) -> ModLook {
        let (models, skins) = models_and_skins(sec, key::MODEL, key::SKIN);
        let (hh_models, hh_skins) = models_and_skins(sec, key::HH_MODEL, key::HH_SKIN);
        ModLook {
            name: text(sec, key::NAME),
            socket: text(sec, key::SOCKET),
            models,
            skins,
            styles: list(sec, key::STYLE, MAX_SKINS),
            hh_socket: text(sec, key::HH_SOCKET),
            hh_models,
            hh_skins,
            hh_styles: list(sec, key::HH_STYLE, MAX_SKINS),
            pv_fx: opt_text(sec, key::PV_FX),
            hh_fx: opt_text(sec, key::HH_FX),
            scale: scale_of(sec, key::SCALE),
            hh_scale: scale_of(sec, key::HH_SCALE),
            kind: get(sec, key::KINDS).and_then(|v| v.trim().trim_matches('"').trim().parse().ok()),
        }
    }
}

impl WeaponLook {
    pub fn of_section(sec: &SectionMap) -> WeaponLook {
        let names = list(sec, key::MOD_NAME, 64);
        let mods = names
            .into_iter()
            .enumerate()
            .map(|(i, n)| {
                let m = get(sec, &format!("{}{i}", key::MOD_MODEL))
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .map_or(-1, |v| v as i32);
                (n, m)
            })
            .collect();
        WeaponLook {
            mods,
            muzzle_pos: vec3(sec, key::MUZZLE_PV_POS),
            muzzle_rot: vec3(sec, key::MUZZLE_PV_ROT),
            scope_pos: vec3(sec, key::SCOPE_PV_POS),
            scope_rot: vec3(sec, key::SCOPE_PV_ROT),
            muzzle_hh_pos: vec3(sec, key::MUZZLE_HH_POS),
            muzzle_hh_rot: vec3(sec, key::MUZZLE_HH_ROT),
            scope_hh_pos: vec3(sec, key::SCOPE_HH_POS),
            scope_hh_rot: vec3(sec, key::SCOPE_HH_ROT),
            pv_fx: opt_text(sec, key::ROW_PV_FX),
            hh_fx: opt_text(sec, key::ROW_HH_FX),
            default_scope: text(sec, key::DEFAULT_SCOPE),
            default_scope_visible: get(sec, key::DEFAULT_SCOPE_VISIBLE)
                .and_then(|v| v.trim().trim_matches('"').trim().parse::<f32>().ok())
                .is_none_or(|v| v != 0.0),
        }
    }
}

impl AttachTable {
    pub fn from_doc(doc: &ButeDoc) -> AttachTable {
        let mut t = AttachTable::default();
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, "Mod") {
                t.mods.insert(n, ModLook::of_section(sec));
            } else if let Some(n) = index_of(name, "Weapon") {
                t.weapons.insert(n, WeaponLook::of_section(sec));
            }
        }
        t
    }

    /// The weapon row's `ModModel` for mod `id` (by the mod row's `Name` among the row's `ModName<i>`); -1 when the
    /// row does not list it.
    // EVIDENCE: cshell+0x172b20 (the list index of the id, then +0x268 at that index; -1 when absent), cshell+0x17d62e
    // (`ModName<i>` resolved to the mod id by name)
    pub fn mod_model_of(&self, weapon: u16, id: u16) -> i32 {
        let (Some(w), Some(m)) = (self.weapons.get(&weapon), self.mods.get(&id)) else {
            return -1;
        };
        w.mods
            .iter()
            .find(|(n, _)| dc_bute::eq_fold(n, &m.name))
            .map_or(-1, |(_, k)| *k)
    }

    /// Whether the sight attachment `id` is hidden on `weapon`: the mod is the row's `DefaultScopeAttach` and the
    /// row says `DefaultScopeVisible = 0` (the weapon's model carries that sight itself, e.g. the M417's and the
    /// TAR-21's ITECH dot sight). Any other sight is drawn.
    // DISASM: cshell+0x232f30 (the weapon's attachment show/hide: all three records take the flag, except that the
    // sight record, mod id at +0x40 = record 1 +0x14 (cshell+0x237977 reads the record's mod id 4 bytes before the
    // object), takes row +0xdc when the row's +0xd8 names (cshell+0x172510, mod by name) that same mod id)
    pub fn sight_built_in(&self, weapon: u16, id: u16) -> bool {
        let (Some(w), Some(m)) = (self.weapons.get(&weapon), self.mods.get(&id)) else {
            return false;
        };
        !w.default_scope_visible && !w.default_scope.is_empty() && dc_bute::eq_fold(&w.default_scope, &m.name)
    }

    /// The first-person attachments of `weapon` with the mods of its three sockets (muzzle, sight, magazine): one
    /// per socket whose mod names a socket and a model file.
    pub fn view_attachments(&self, weapon: u16, mods: [Option<u16>; 3]) -> Vec<WeaponAttach> {
        let w = self.weapons.get(&weapon).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for (slot, id) in mods.iter().enumerate() {
            let Some(id) = *id else { continue };
            let Some(m) = self.mods.get(&id) else {
                continue;
            };
            if m.socket.is_empty() || (slot == SIGHT && self.sight_built_in(weapon, id)) {
                continue;
            }
            let k = self.mod_model_of(weapon, id);
            if !(0..MAX_MODELS as i32).contains(&k) {
                continue;
            }
            let k = k as usize;
            let Some(file) = m.models.get(k).filter(|f| !f.is_empty()) else {
                continue;
            };
            let (pos_off, rot_off) = match slot {
                MUZZLE => (w.muzzle_pos, w.muzzle_rot),
                SIGHT => (w.scope_pos, w.scope_rot),
                _ => ([0.0; 3], [0.0; 3]),
            };
            out.push(WeaponAttach {
                mod_id: id,
                socket: m.socket.clone(),
                model: crate::world::ModelRef {
                    model: file.clone(),
                    textures: m.skins.get(k).cloned().unwrap_or_default(),
                },
                styles: m.styles.clone(),
                pos_off,
                rot_off,
                // the attach step scales the new model by the row's three floats
                // DISASM: cshell+0x237b68..0x237b91 (mod row +0xf4..+0xfc to engine interface slot +0x224)
                scale: m.scale,
            });
        }
        out
    }

    /// The third-person attachments of `weapon` with `mods` (the mod numbers of a shot record; 0xff = none): the
    /// mod's `AttachHHModel<k>` at its `HHModelSocket`, `k` as for the first-person model, moved and turned in the
    /// socket's frame by the weapon row's `MuzzleHHPosOff` / `MuzzleHHRotOff` (a mod of `Kinds` 0) or
    /// `ScopeHHPosOff` / `ScopeHHRotOff` (`Kinds` 1), scaled by the mod row's `AttachHHModelScale`.
    // DC-DEFAULT: CLI-618 the third-person attachments: the remote's mods are the last shot record's three mod
    // numbers (the 2009 shot object keeps them, cshell+0x2c6a7e..0x2c6a96; the 2009 drawing of third-person mod models
    // was not found in the client), the same `ModModel` index
    // DC-DEFAULT: CLI-651 (proposed) the HH offsets apply as the first-person ones do (socket frame, Euler angles plus
    // the angle offset, cshell+0x234380). The only 2009 client use found is the tuning commands cshell+0x1f6a10 /
    // 0x1f6b70 / 0x1f6cd0 / 0x1f6e30 (registered by cshell+0x1f6fb0), PLAUSIBLY a message 0x8b with socket 0 / 1 and
    // three floats; the reader is not in the client
    // OBSERVED: ROM WEAPONS.TXT (`MuzzleHHPosOff` / `ScopeHHPosOff` / their `RotOff` on 217 weapon rows, the L96
    // `MuzzleHHPosOff = <0, 0, -50.5>`; `AttachHHModelScale` on the 27 mod rows with a third-person model)
    pub fn hh_attachments(&self, weapon: u16, mods: [u8; 3]) -> Vec<WeaponAttach> {
        let w = self.weapons.get(&weapon).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for id in mods.iter().filter(|m| **m != 0xff).map(|m| u16::from(*m)) {
            let Some(m) = self.mods.get(&id) else {
                continue;
            };
            // DC-DEFAULT: CLI-1720 (proposed, project decision) the third-person model hides a built-in default sight as the
            // first-person one does (cshell+0x232f30; the third-person models carry the same sight skin)
            if m.hh_socket.is_empty() || (m.kind == Some(SIGHT as u8) && self.sight_built_in(weapon, id)) {
                continue;
            }
            let k = self.mod_model_of(weapon, id);
            if !(0..MAX_MODELS as i32).contains(&k) {
                continue;
            }
            let k = k as usize;
            let Some(file) = m.hh_models.get(k).filter(|f| !f.is_empty()) else {
                continue;
            };
            let (pos_off, rot_off) = match m.kind.map(usize::from) {
                Some(MUZZLE) => (w.muzzle_hh_pos, w.muzzle_hh_rot),
                Some(SIGHT) => (w.scope_hh_pos, w.scope_hh_rot),
                _ => ([0.0; 3], [0.0; 3]),
            };
            out.push(WeaponAttach {
                mod_id: id,
                socket: m.hh_socket.clone(),
                model: crate::world::ModelRef {
                    model: file.clone(),
                    textures: m.hh_skins.get(k).cloned().unwrap_or_default(),
                },
                styles: m.hh_styles.clone(),
                pos_off,
                rot_off,
                scale: m.hh_scale,
            });
        }
        out
    }
}

/// The rotation of an attached mod model: the socket's rotation as Euler angles (yaw about +Y, pitch about +X, roll
/// about +Z, rotation = yaw x pitch x roll), each plus the record's angle offset, turned back into a rotation. The
/// offset's first component is added to the yaw, the second to the pitch, the third to the roll.
// DISASM: cshell+0x23462b (the socket rotation to Euler angles by cshell+0x1e7d00 with order code 0x11, the YXZ
// rotating-axes order of the engine's quaternion-to-Euler helper cshell+0x1e7b20), cshell+0x234630..0x234673 (angle k
// plus record +0x10 + 4 k; the rotation built by cshell+0x1b700 with (angle 1, angle 0, angle 2)), cshell+0x1b700
// (matrix rows [c0c2 + s1s0s2, s1s0c2 - c0s2, c1s0], [c1s2, c1c2, -s1], [c0s1s2 - s0c2, c0s1c2 + s0s2, c1c0] for
// yaw a0, pitch a1, roll a2: Ry(a0) Rx(a1) Rz(a2))
pub fn turned(socket: glam::Quat, r: [f32; 3]) -> glam::Quat {
    if r == [0.0; 3] {
        return socket;
    }
    let (yaw, pitch, roll) = socket.to_euler(glam::EulerRot::YXZ);
    glam::Quat::from_euler(glam::EulerRot::YXZ, yaw + r[0], pitch + r[1], roll + r[2]).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> ButeDoc {
        ButeDoc::parse(concat!(
            "[Mod3]\nName = \"Sil\"\nSocket = \"Flash\"\nAttachModel0 = \"a.ltb\"\nAttachSkin0_0 = \"a.dtx\"\n",
            "AttachModel1 = \"b.ltb\"\nAttachRenderStyle0 = \"rs.ltb\"\nHHModelSocket = \"Flash\"\n",
            "AttachHHModel0 = \"hh.ltb\"\nAttachScale = <2.0, 2.0, 2.0>\n",
            "[Mod4]\nName = \"Scope\"\nSocket = \"Scope\"\nAttachModel0 = \"\"\n",
            "[Mod5]\nName = \"Mag\"\nKinds = 2\n",
            "[Weapon9]\nModName0 = \"Scope\"\nModName1 = \"Sil\"\nModModel0 = 0\nModModel1 = 1\n",
            "MuzzlePVPosOff = <0.0, 0.0, 4.0>\n",
        ))
    }

    #[test]
    fn the_mod_model_is_picked_by_the_weapon_rows_index_and_empty_files_draw_nothing() {
        let t = AttachTable::from_doc(&doc());
        assert_eq!(t.mod_model_of(9, 3), 1);
        assert_eq!(t.mod_model_of(9, 5), -1);
        let a = t.view_attachments(9, [Some(3), Some(4), Some(5)]);
        assert_eq!(
            a.len(),
            1,
            "the integrated scope (empty file) and the magazine (no socket) draw nothing"
        );
        assert_eq!(a[0].model.model, "b.ltb");
        assert_eq!(a[0].socket, "Flash");
        assert_eq!(a[0].pos_off, [0.0, 0.0, 4.0]);
        assert!(t.view_attachments(9, [None; 3]).is_empty());
        // index 1 has no third-person model
        assert!(t.hh_attachments(9, [3, 0xff, 0xff]).is_empty());
    }

    /// CLI-635: the mod row's `AttachScale` goes with the first-person attachment; a row without it draws at 1.
    #[test]
    fn attach_scale_is_carried_and_defaults_to_one() {
        let t = AttachTable::from_doc(&doc());
        let a = t.view_attachments(9, [Some(3), None, None]);
        assert_eq!(a[0].scale, [2.0; 3]);
        assert_eq!(t.mods[&4].scale, [1.0; 3]);
        let one = ButeDoc::parse("[Mod1]\nAttachScale = 1.5\n[Mod2]\nAttachScale = <0, 1, 1>\n");
        let t = AttachTable::from_doc(&one);
        assert_eq!(t.mods[&1].scale, [1.5; 3]);
        assert_eq!(t.mods[&2].scale, [1.0; 3]);
    }

    /// A weapon whose model carries its default sight (`DefaultScopeVisible = 0`) does not draw that sight's mod
    /// model again; another sight, and a default sight with the flag unset or 1, is drawn (the user's "the M417 with
    /// the combat scope shows two scopes", also the TAR-21).
    #[test]
    fn a_built_in_default_sight_is_not_drawn_twice() {
        let rows = concat!(
            "[Mod1]
Name = \"Dot_ITECH\"
Socket = \"Breach\"
Kinds = 1
AttachModel0 = \"itech.ltb\"
",
            "HHModelSocket = \"Breach\"
AttachHHModel0 = \"hh_itech.ltb\"
",
            "[Mod2]
Name = \"Dot_HDS\"
Socket = \"Breach\"
Kinds = 1
AttachModel0 = \"hds.ltb\"
",
            "HHModelSocket = \"Breach\"
AttachHHModel0 = \"hh_hds.ltb\"
",
            "[Weapon1]
DefaultScopeAttach = \"dot_itech\"
DefaultScopeVisible = 0
",
            "ModName0 = \"Dot_ITECH\"
ModName1 = \"Dot_HDS\"
ModModel0 = 0
ModModel1 = 0
",
            "[Weapon2]
DefaultScopeAttach = \"Dot_ITECH\"
",
            "ModName0 = \"Dot_ITECH\"
ModModel0 = 0
",
        );
        let t = AttachTable::from_doc(&ButeDoc::parse(rows));
        assert!(t.sight_built_in(1, 1));
        assert!(!t.sight_built_in(1, 2));
        assert!(!t.sight_built_in(2, 1), "DefaultScopeVisible defaults to 1");
        assert!(t.view_attachments(1, [None, Some(1), None]).is_empty());
        assert!(t.hh_attachments(1, [0xff, 1, 0xff]).is_empty());
        let a = t.view_attachments(1, [None, Some(2), None]);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].model.model, "hds.ltb");
        assert_eq!(t.hh_attachments(1, [0xff, 2, 0xff]).len(), 1);
        assert_eq!(t.view_attachments(2, [None, Some(1), None]).len(), 1);
    }

    /// The angle offset adds to the socket's yaw / pitch / roll (Ry Rx Rz), component 0 to the yaw.
    #[test]
    fn the_angle_offset_adds_to_the_sockets_euler_angles() {
        use glam::{EulerRot, Quat};
        let socket = Quat::from_euler(EulerRot::YXZ, 0.3, 0.2, -0.1);
        assert_eq!(turned(socket, [0.0; 3]), socket);
        let got = turned(socket, [0.1, 0.05, 0.2]);
        let want = Quat::from_euler(EulerRot::YXZ, 0.4, 0.25, 0.1);
        assert!(got.angle_between(want) < 1e-5, "{got:?} vs {want:?}");
        // the matrix of cshell+0x1b700 for yaw a0, pitch a1, roll a2 (row 1: [c1 s2, c1 c2, -s1]; row 0 z: c1 s0)
        let (a0, a1, a2) = (0.4f32, 0.25f32, 0.1f32);
        let m = glam::Mat3::from_quat(want);
        let row1 = glam::Vec3::new(a1.cos() * a2.sin(), a1.cos() * a2.cos(), -a1.sin());
        assert!((m.row(1) - row1).length() < 1e-5);
        assert!((m.row(0).z - a1.cos() * a0.sin()).abs() < 1e-5);
    }
}
