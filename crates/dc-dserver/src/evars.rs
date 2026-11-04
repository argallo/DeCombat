//! The world-settings engine variables (`S2C_EVAR`): one variable per property of the world's world-settings
//! object, under the name the 2009 client reads it by, sent at the world load before `S2C_LOAD_WORLD`.
//!
//! The values come from the user's world file at run time (the world-settings object's properties); the names come
//! from the user's ROM at run time too: where the client's name equals the property name, the property name of the
//! world file is used, otherwise the client name is found in the user's CShell.dll by its hash. The tree holds only
//! hashes (name hash and length), never the names or values.
// SPEC: UDP_SESSION_2009_A.md §4.5 (`S2C_EVAR` `[name:zstr8][value:zstr8]`), §8.2 (phase 1: every variable, then
// `S2C_LOAD_WORLD`); UDP_SESSION_2009_B.md §8.6; CAPTURE_rust3_RUSTSTACK_VALIDATION.md §3.3 (the 33 world-settings
// variables, their client names and the world-settings properties they come from)
// EVIDENCE: cshell+0x1a6900, cshell+0x1a6840, cshell+0x1a9b00 (the client copies these server variables into console
// values at world entry; a missing one is applied as 0)
// OBSERVED: capture 20261025_065016_tdm6 and 20261025_002236_tdm4 (the reference stack sent the 33 with
// values equal to the ROM properties on two worlds, 66 of 66; REFERENCE, accepted by the real client);
// capture 20261025_081704_rust3 (no variable at all: fog off, all-sky-portals 0)
// DC-DEFAULT: DSV-47 (proposed) the world-settings variables at the world load; no server-configuration variable
// unless the operator configures one

use std::path::Path;

use crate::world::PropHash;
use dc_wire::udp::BitWriter;
use dc_world::{names, Key};

/// One row of the world-settings mapping: our label, the hash of the client's variable name, the key of the
/// world-settings property (its ROM hash, and DeCombat's own name for the rows the map vocabulary covers, MAP-2),
/// and the component of a vector or colour property (`None` = the whole value).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldVarRow {
    pub label: &'static str,
    pub client: PropHash,
    pub prop: Key,
    pub part: Option<usize>,
}

/// A ROM-only property key of the table (no DeCombat name).
const fn rom(hash: [u8; 8], len: usize) -> Key {
    Key {
        hash,
        len,
        ours: "",
    }
}

/// The 33 rows (CAPTURE_rust3_RUSTSTACK_VALIDATION.md §3.3 table, hashes = SHA-256 prefix and length). The eleven
/// rows a `dc.world_settings` object may carry (`far_z`, `fog_enable`, `fog_near`, `fog_far`, `fog_color` as
/// `fog_r/g/b`, `sky_fog_*`, `all_sky_portals`, `wind` as `wind_x/y/z`) use the `dc_world::names` keys; the other
/// 22 are ROM-only.
// SPEC: CAPTURE_rust3_RUSTSTACK_VALIDATION.md §3.3
// DC-DEFAULT: MAP-20 (the vocabulary rows through the keys)
#[rustfmt::skip]
pub const WORLD_VARS: [WorldVarRow; 33] = [
    WorldVarRow { label: "far_z", client: ([0x12, 0x46, 0x2a, 0x18, 0x52, 0xd9, 0xfd, 0x08], 4), prop: names::PROP_FAR_Z, part: None },
    WorldVarRow { label: "fog_enable", client: ([0xb4, 0x2e, 0x42, 0x32, 0xc6, 0x6d, 0x28, 0xc4], 9), prop: names::PROP_FOG_ENABLE, part: None },
    WorldVarRow { label: "fog_near_z", client: ([0x63, 0xcb, 0x48, 0x90, 0x1b, 0x40, 0xa0, 0x7e], 8), prop: names::PROP_FOG_NEAR, part: None },
    WorldVarRow { label: "fog_far_z", client: ([0x06, 0x8c, 0x8f, 0x33, 0x71, 0x6e, 0xac, 0x9c], 7), prop: names::PROP_FOG_FAR, part: None },
    WorldVarRow { label: "fog_r", client: ([0x6a, 0x3a, 0xa1, 0x36, 0xd4, 0xa2, 0xc3, 0x41], 4), prop: names::PROP_FOG_COLOR, part: Some(0) },
    WorldVarRow { label: "fog_g", client: ([0xcb, 0x7c, 0xae, 0x2a, 0x3c, 0x30, 0x69, 0xb3], 4), prop: names::PROP_FOG_COLOR, part: Some(1) },
    WorldVarRow { label: "fog_b", client: ([0xcc, 0xd5, 0x5f, 0xe4, 0xa2, 0x19, 0x14, 0x54], 4), prop: names::PROP_FOG_COLOR, part: Some(2) },
    WorldVarRow { label: "sky_fog_enable", client: ([0xcd, 0x9c, 0x0c, 0x6e, 0x0d, 0x40, 0x40, 0x7e], 12), prop: names::PROP_SKY_FOG_ENABLE, part: None },
    WorldVarRow { label: "sky_fog_near", client: ([0x10, 0xeb, 0x82, 0x75, 0xf0, 0x99, 0x17, 0xef], 11), prop: names::PROP_SKY_FOG_NEAR, part: None },
    WorldVarRow { label: "sky_fog_far", client: ([0x38, 0x85, 0x26, 0x3f, 0x3d, 0x41, 0x06, 0x45], 10), prop: names::PROP_SKY_FOG_FAR, part: None },
    WorldVarRow { label: "vfog_on", client: ([0xbd, 0xaa, 0x1c, 0x3a, 0x0a, 0xcb, 0x61, 0xd2], 7), prop: rom([0xe3, 0xd7, 0x2a, 0xec, 0xf9, 0xae, 0xc8, 0xaa], 4), part: None },
    WorldVarRow { label: "vfog_min_y", client: ([0x0f, 0x39, 0x85, 0xea, 0x5b, 0x0a, 0xd7, 0xd7], 11), prop: rom([0x14, 0x93, 0x9e, 0xd7, 0x20, 0xe8, 0x9b, 0x77], 8), part: None },
    WorldVarRow { label: "vfog_max_y", client: ([0xbe, 0xba, 0x61, 0xa2, 0xf9, 0x59, 0x8f, 0x93], 11), prop: rom([0xa8, 0x14, 0x41, 0x7a, 0x42, 0xdf, 0x5d, 0x75], 8), part: None },
    WorldVarRow { label: "vfog_density", client: ([0x32, 0xa8, 0x29, 0xd7, 0x99, 0x25, 0x91, 0x2a], 14), prop: rom([0x93, 0xdc, 0x00, 0xc3, 0xfc, 0xa0, 0xf0, 0x25], 11), part: None },
    WorldVarRow { label: "vfog_max", client: ([0xf3, 0xff, 0x5d, 0xe2, 0x93, 0xb2, 0xa0, 0xab], 10), prop: rom([0x6b, 0x35, 0x2f, 0x22, 0x4b, 0x52, 0x83, 0xbe], 7), part: None },
    WorldVarRow { label: "vfog_max_y_val", client: ([0x6f, 0x10, 0x9a, 0x26, 0x68, 0xd3, 0x68, 0xba], 14), prop: rom([0x8a, 0x3a, 0x69, 0x21, 0xff, 0x5b, 0x98, 0x58], 11), part: None },
    WorldVarRow { label: "vfog_min_y_val", client: ([0x6c, 0x28, 0x2e, 0x53, 0x2e, 0xb2, 0x73, 0x96], 14), prop: rom([0x5e, 0xc6, 0xc1, 0xa5, 0xd9, 0xdd, 0xe9, 0xbf], 11), part: None },
    WorldVarRow { label: "shadow_alpha", client: ([0x8c, 0x71, 0x5d, 0xa3, 0x50, 0x76, 0xfc, 0x3a], 22), prop: rom([0x73, 0x23, 0xb5, 0xb4, 0xaf, 0x48, 0xb5, 0x97], 11), part: None },
    WorldVarRow { label: "shadow_min_color", client: ([0xab, 0x4e, 0x02, 0x77, 0xa6, 0x1f, 0x7f, 0xf4], 34), prop: rom([0x27, 0xd2, 0x30, 0x3e, 0x2d, 0x9f, 0xe4, 0x03], 23), part: None },
    WorldVarRow { label: "shadow_max_dist", client: ([0x96, 0xb2, 0xfe, 0x98, 0x73, 0xf4, 0x0d, 0x91], 28), prop: rom([0xd8, 0x08, 0x0b, 0x0c, 0x9b, 0x3f, 0xa1, 0xc4], 17), part: None },
    WorldVarRow { label: "all_sky_portals", client: ([0x5f, 0x48, 0x30, 0x04, 0xcf, 0x36, 0xe2, 0x38], 13), prop: names::PROP_ALL_SKY_PORTALS, part: None },
    WorldVarRow { label: "wind_x", client: ([0xce, 0x34, 0x3f, 0x3c, 0x97, 0xfa, 0x69, 0x2f], 5), prop: names::PROP_WIND, part: Some(0) },
    WorldVarRow { label: "wind_y", client: ([0x2d, 0xa1, 0x4a, 0x1d, 0x90, 0x93, 0x76, 0x9e], 5), prop: names::PROP_WIND, part: Some(1) },
    WorldVarRow { label: "wind_z", client: ([0xb2, 0xc9, 0xe2, 0xe1, 0x9a, 0x05, 0xa5, 0xc7], 5), prop: names::PROP_WIND, part: Some(2) },
    WorldVarRow { label: "pan_sky", client: ([0x6a, 0x90, 0x3f, 0xf8, 0x99, 0xd3, 0x57, 0xcb], 6), prop: rom([0x6a, 0x90, 0x3f, 0xf8, 0x99, 0xd3, 0x57, 0xcb], 6), part: None },
    WorldVarRow { label: "pan_sky_offset_x", client: ([0x84, 0xac, 0xf6, 0x9a, 0x55, 0xbf, 0xee, 0xe7], 13), prop: rom([0x84, 0xac, 0xf6, 0x9a, 0x55, 0xbf, 0xee, 0xe7], 13), part: None },
    WorldVarRow { label: "pan_sky_offset_z", client: ([0x23, 0x3f, 0xd6, 0x31, 0x07, 0xda, 0xdf, 0x13], 13), prop: rom([0x23, 0x3f, 0xd6, 0x31, 0x07, 0xda, 0xdf, 0x13], 13), part: None },
    WorldVarRow { label: "pan_sky_scale_x", client: ([0x63, 0x55, 0x1d, 0x8b, 0x8b, 0xbd, 0x9b, 0x38], 12), prop: rom([0x63, 0x55, 0x1d, 0x8b, 0x8b, 0xbd, 0x9b, 0x38], 12), part: None },
    WorldVarRow { label: "pan_sky_scale_z", client: ([0x86, 0xf2, 0x26, 0x3c, 0xad, 0xa6, 0xb1, 0x75], 12), prop: rom([0x86, 0xf2, 0x26, 0x3c, 0xad, 0xa6, 0xb1, 0x75], 12), part: None },
    WorldVarRow { label: "pan_sky_texture", client: ([0xc2, 0xe7, 0xdb, 0x2c, 0x84, 0xe5, 0xfe, 0x9c], 13), prop: rom([0xc2, 0xe7, 0xdb, 0x2c, 0x84, 0xe5, 0xfe, 0x9c], 13), part: None },
    WorldVarRow { label: "ai_max_number", client: ([0x98, 0xd6, 0x90, 0xc4, 0xf4, 0x05, 0x98, 0x27], 11), prop: rom([0x98, 0xd6, 0x90, 0xc4, 0xf4, 0x05, 0x98, 0x27], 11), part: None },
    WorldVarRow { label: "sound_filter", client: ([0x56, 0xe6, 0xfb, 0x14, 0xb5, 0x58, 0x82, 0xde], 17), prop: rom([0x56, 0xe6, 0xfb, 0x14, 0xb5, 0x58, 0x82, 0xde], 17), part: None },
    WorldVarRow { label: "music_mood_min", client: ([0x03, 0x65, 0x66, 0x6c, 0x46, 0xf4, 0xde, 0x53], 12), prop: rom([0x03, 0x65, 0x66, 0x6c, 0x46, 0xf4, 0xde, 0x53], 12), part: None },
];

/// The order in which the server sends its variables before `S2C_LOAD_WORLD`: the client names of the 33
/// world-settings rows and the two server constants (`clientvars.rs`), in the relative order of the 2009 server's
/// list. The client looks variables up by name, so the order is fidelity only.
// OBSERVED: capture 20261025_002236_tdm4 (conn 5, the 86 variables before `S2C_LOAD_WORLD`)
// and 20261025_065016_tdm6, 20261025_010543_botduel1 (the same order on the other world and run; REFERENCE)
// DC-DEFAULT: DSV-60 (proposed) the reference stack's order
#[rustfmt::skip]
pub const SEND_ORDER: [PropHash; 35] = [
    ([0x12, 0x46, 0x2a, 0x18, 0x52, 0xd9, 0xfd, 0x08], 4), // far_z
    ([0x18, 0xe9, 0x1a, 0x62, 0x81, 0x02, 0x0c, 0x4a], 10), // world_north
    ([0xb4, 0x2e, 0x42, 0x32, 0xc6, 0x6d, 0x28, 0xc4], 9), // fog_enable
    ([0x63, 0x55, 0x1d, 0x8b, 0x8b, 0xbd, 0x9b, 0x38], 12), // pan_sky_scale_x
    ([0x6c, 0x28, 0x2e, 0x53, 0x2e, 0xb2, 0x73, 0x96], 14), // vfog_min_y_val
    ([0x86, 0xf2, 0x26, 0x3c, 0xad, 0xa6, 0xb1, 0x75], 12), // pan_sky_scale_z
    ([0x6f, 0x10, 0x9a, 0x26, 0x68, 0xd3, 0x68, 0xba], 14), // vfog_max_y_val
    ([0xbd, 0xaa, 0x1c, 0x3a, 0x0a, 0xcb, 0x61, 0xd2], 7), // vfog_on
    ([0x06, 0x8c, 0x8f, 0x33, 0x71, 0x6e, 0xac, 0x9c], 7), // fog_far_z
    ([0x32, 0xa8, 0x29, 0xd7, 0x99, 0x25, 0x91, 0x2a], 14), // vfog_density
    ([0x5f, 0x48, 0x30, 0x04, 0xcf, 0x36, 0xe2, 0x38], 13), // all_sky_portals
    ([0x84, 0xac, 0xf6, 0x9a, 0x55, 0xbf, 0xee, 0xe7], 13), // pan_sky_offset_x
    ([0x63, 0xcb, 0x48, 0x90, 0x1b, 0x40, 0xa0, 0x7e], 8), // fog_near_z
    ([0x98, 0xd6, 0x90, 0xc4, 0xf4, 0x05, 0x98, 0x27], 11), // ai_max_number
    ([0x23, 0x3f, 0xd6, 0x31, 0x07, 0xda, 0xdf, 0x13], 13), // pan_sky_offset_z
    ([0xab, 0x4e, 0x02, 0x77, 0xa6, 0x1f, 0x7f, 0xf4], 34), // shadow_min_color
    ([0x6a, 0x90, 0x3f, 0xf8, 0x99, 0xd3, 0x57, 0xcb], 6), // pan_sky
    ([0xcd, 0x9c, 0x0c, 0x6e, 0x0d, 0x40, 0x40, 0x7e], 12), // sky_fog_enable
    ([0x10, 0xeb, 0x82, 0x75, 0xf0, 0x99, 0x17, 0xef], 11), // sky_fog_near
    ([0xc2, 0xe7, 0xdb, 0x2c, 0x84, 0xe5, 0xfe, 0x9c], 13), // pan_sky_texture
    ([0xce, 0x34, 0x3f, 0x3c, 0x97, 0xfa, 0x69, 0x2f], 5), // wind_x
    ([0x2d, 0xa1, 0x4a, 0x1d, 0x90, 0x93, 0x76, 0x9e], 5), // wind_y
    ([0x8c, 0x71, 0x5d, 0xa3, 0x50, 0x76, 0xfc, 0x3a], 22), // shadow_alpha
    ([0xb2, 0xc9, 0xe2, 0xe1, 0x9a, 0x05, 0xa5, 0xc7], 5), // wind_z
    ([0x96, 0xb2, 0xfe, 0x98, 0x73, 0xf4, 0x0d, 0x91], 28), // shadow_max_dist
    ([0x0f, 0x39, 0x85, 0xea, 0x5b, 0x0a, 0xd7, 0xd7], 11), // vfog_min_y
    ([0x56, 0xe6, 0xfb, 0x14, 0xb5, 0x58, 0x82, 0xde], 17), // sound_filter
    ([0xbe, 0xba, 0x61, 0xa2, 0xf9, 0x59, 0x8f, 0x93], 11), // vfog_max_y
    ([0xcc, 0xd5, 0x5f, 0xe4, 0xa2, 0x19, 0x14, 0x54], 4), // fog_b
    ([0xed, 0xa1, 0x0d, 0x3a, 0x64, 0x76, 0xdd, 0x56], 15), // step_height
    ([0xcb, 0x7c, 0xae, 0x2a, 0x3c, 0x30, 0x69, 0xb3], 4), // fog_g
    ([0x6a, 0x3a, 0xa1, 0x36, 0xd4, 0xa2, 0xc3, 0x41], 4), // fog_r
    ([0xf3, 0xff, 0x5d, 0xe2, 0x93, 0xb2, 0xa0, 0xab], 10), // vfog_max
    ([0x38, 0x85, 0x26, 0x3f, 0x3d, 0x41, 0x06, 0x45], 10), // sky_fog_far
    ([0x03, 0x65, 0x66, 0x6c, 0x46, 0xf4, 0xde, 0x53], 12), // music_mood_min
];

/// One world variable of a loaded world: the client's name (`None` until it is found in the ROM), the name's hash,
/// and the value text.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldVar {
    pub label: &'static str,
    pub key: PropHash,
    pub name: Option<Vec<u8>>,
    pub value: Vec<u8>,
}

/// Which world variables the server sends (`-evars world|off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvarMode {
    World,
    Off,
}

/// A value as the 2009 server formatted it: reals and vector components with six decimals (`%f`), booleans `1` /
/// `0`, integers in decimal, text verbatim.
// OBSERVED: capture 20261025_065016_tdm6 (the value texts of the reference stack's 33 world-settings
// variables against the property types of the ROM world: reals and colour components `10000.000000`, booleans `1`
// / `0`, the integer property `0`, the texts as stored; REFERENCE)
pub fn value_form(v: &dc_world::FieldValue<'_>, part: Option<usize>) -> Option<Vec<u8>> {
    use dc_world::FieldValue as F;
    let real = |x: f32| format!("{:.6}", x as f64).into_bytes();
    Some(match (v, part) {
        (F::Vec3(c) | F::Rgb(c), Some(i)) => real(*c.get(i)?),
        (F::Real(r), None) => real(*r),
        (F::Bool(b), None) => {
            if *b {
                b"1".to_vec()
            } else {
                b"0".to_vec()
            }
        }
        (F::Int(i) | F::Bits(i), None) => i.to_string().into_bytes(),
        (F::Text(t), None) => t.to_vec(),
        _ => return None,
    })
}

/// The world variables of a parsed world: one per row whose property the world-settings object has (file order of
/// the rows; the property by ROM hash or by its DeCombat name, `Key::matches`). The name is the property's own
/// name where the client's name hash equals it AND the property was matched by that hash; a row matched through
/// the DeCombat alias (`far_z` on a `dc.world_settings`) has no file name to send and gets `None`, resolved from
/// the user's CShell.dll by [`resolve_names`] (`load_from_rom`, and `-worldfile` with a ROM at hand).
// SPEC: FMT_DAT85.md §6, §6.2 (the world-settings role, properties by name hash)
// DC-DEFAULT: MAP-20
pub fn world_vars(w: &dc_world::World<'_>) -> Vec<WorldVar> {
    let Some(o) = w
        .objects
        .iter()
        .find(|o| o.role == Some(dc_world::ObjectRole::WorldSettings))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in &WORLD_VARS {
        let Some(p) = o.prop_named(&row.prop) else {
            continue;
        };
        let Some(value) = value_form(&p.value, row.part) else {
            continue;
        };
        let own_name = row.client == row.prop.hash_len() && row.prop.matches_hash(p.name);
        out.push(WorldVar {
            label: row.label,
            key: row.client,
            name: own_name.then(|| p.name.to_vec()),
            value,
        });
    }
    out
}

/// The NUL-delimited printable strings of a binary whose hash and length equal one of `keys`.
pub fn find_strings(bytes: &[u8], keys: &[PropHash]) -> Vec<(PropHash, Vec<u8>)> {
    let lens: std::collections::BTreeSet<usize> = keys.iter().map(|k| k.1).collect();
    let mut out: Vec<(PropHash, Vec<u8>)> = Vec::new();
    for run in bytes.split(|&b| b == 0) {
        if !lens.contains(&run.len()) || !run.iter().all(|b| b.is_ascii_graphic()) {
            continue;
        }
        let k = (dc_world::hash16(run), run.len());
        if keys.contains(&k) && !out.iter().any(|x| x.0 == k) {
            out.push((k, run.to_vec()));
        }
    }
    out
}

/// Fills the names the world file could not give from the client's own shell library (`Game/CShell.dll` of the
/// user's ROM), matched by hash. Returns how many names are still missing (those variables are not sent).
pub fn resolve_names(vars: &mut [WorldVar], rom: &Path) -> usize {
    let keys: Vec<PropHash> = vars
        .iter()
        .filter(|v| v.name.is_none())
        .map(|v| v.key)
        .collect();
    if !keys.is_empty() {
        if let Some(bytes) = dc_rez::dc_rom::server_rom::shell_image(rom) {
            let found = find_strings(&bytes, &keys);
            for v in vars.iter_mut().filter(|v| v.name.is_none()) {
                v.name = found.iter().find(|f| f.0 == v.key).map(|f| f.1.clone());
            }
        }
    }
    vars.iter().filter(|v| v.name.is_none()).count()
}

/// The `S2C_EVAR` messages of the world variables with a known name, then the operator's own variables
/// (`-evar name=value`; a configured name replaces a world variable of the same name).
// DC-DEFAULT: DSV-47 (proposed)
pub fn evar_messages(vars: &[WorldVar], extra: &[(String, String)]) -> Vec<BitWriter> {
    let mut out = Vec::new();
    for v in vars {
        let Some(n) = &v.name else {
            continue;
        };
        if extra
            .iter()
            .any(|(k, _)| k.as_bytes().eq_ignore_ascii_case(n))
        {
            continue;
        }
        out.push(crate::msg::s2c_evar(n, &v.value));
    }
    for (k, v) in extra {
        out.push(crate::msg::s2c_evar(k.as_bytes(), v.as_bytes()));
    }
    out
}

/// All variables to send before `S2C_LOAD_WORLD`: the world variables and the server constants with a known name, in
/// the reference stack's order (`SEND_ORDER`; a variable outside it keeps its place after the listed ones), then the
/// operator's own variables (`-evar name=value`; a configured name replaces a variable of the same name).
// DC-DEFAULT: DSV-47 (proposed), DSV-60 (proposed)
pub fn preload_messages(
    world: &[WorldVar],
    consts: &[WorldVar],
    extra: &[(String, String)],
) -> Vec<BitWriter> {
    let rank = |k: &PropHash| SEND_ORDER.iter().position(|o| o == k).unwrap_or(usize::MAX);
    let mut all: Vec<&WorldVar> = world.iter().chain(consts).collect();
    all.sort_by_key(|v| rank(&v.key));
    let vars: Vec<WorldVar> = all.into_iter().cloned().collect();
    evar_messages(&vars, extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_forms_and_hash_lookup() {
        use dc_world::FieldValue as F;
        assert_eq!(
            value_form(&F::Real(10000.0), None).unwrap(),
            b"10000.000000"
        );
        assert_eq!(
            value_form(&F::Real(-2000.0), None).unwrap(),
            b"-2000.000000"
        );
        assert_eq!(value_form(&F::Bool(true), None).unwrap(), b"1");
        assert_eq!(value_form(&F::Int(0), None).unwrap(), b"0");
        assert_eq!(value_form(&F::Text(b"abc"), None).unwrap(), b"abc");
        assert_eq!(
            value_form(&F::Rgb([1.0, 2.5, 3.0]), Some(1)).unwrap(),
            b"2.500000"
        );
        assert!(value_form(&F::Rgb([1.0, 2.5, 3.0]), None).is_none());
        // made-up names: found by hash between NUL bytes, printable only
        let k = (dc_world::hash16(b"Alpha"), 5);
        let bin = b"\0xx\0Alpha\0Beta\0\x01Alpha\0";
        assert_eq!(find_strings(bin, &[k]), vec![(k, b"Alpha".to_vec())]);
        let vars = vec![
            WorldVar {
                label: "a",
                key: k,
                name: Some(b"Alpha".to_vec()),
                value: b"1".to_vec(),
            },
            WorldVar {
                label: "b",
                key: k,
                name: None,
                value: b"2".to_vec(),
            },
        ];
        let m = evar_messages(&vars, &[("Gamma".into(), "3".into())]);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].as_bytes(), b"\x0fAlpha\x001\x00");
        assert_eq!(m[1].as_bytes(), b"\x0fGamma\x003\x00");
        // a configured name replaces the world's variable of that name
        let m = evar_messages(&vars, &[("alpha".into(), "9".into())]);
        assert_eq!(m.len(), 1);
    }

    /// MAP-20: the vocabulary rows carry DeCombat names, the rest are ROM-only; a `dc.world_settings` row is read
    /// through the alias and sends no file name (resolved from the ROM later), while a property that matches by the
    /// ROM hash keeps its own name where the client's name equals it.
    #[test]
    fn vocabulary_rows_send_no_name_until_resolved() {
        let own: Vec<&str> = WORLD_VARS
            .iter()
            .filter(|r| r.prop.has_ours())
            .map(|r| r.label)
            .collect();
        assert_eq!(own.len(), 14);
        assert!(own.contains(&"far_z") && own.contains(&"wind_z") && own.contains(&"fog_b"));
        assert!(!own.contains(&"vfog_on") && !own.contains(&"pan_sky"));
        let bytes = crate::world::synthetic_dc_world_bytes();
        let w =
            dc_world::World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        let vars = world_vars(&w);
        assert_eq!(vars.len(), 14);
        assert!(vars.iter().all(|v| v.name.is_none()));
        assert!(
            evar_messages(&vars, &[]).is_empty(),
            "nothing is sent unresolved"
        );
        // a made-up property under the far-plane row's ROM hash: not reachable without a ROM name, so the row's
        // own-name rule is exercised on a synthetic key instead
        let k = WORLD_VARS[0].prop;
        assert!(k.matches_ours(b"FAR_Z"));
        assert!(!k.matches_hash(b"far_z"));
        assert_eq!(WORLD_VARS[0].client, k.hash_len());
    }

    #[test]
    fn preload_order_follows_the_reference() {
        // every world row and both constants have a place; no key twice
        for r in &WORLD_VARS {
            assert!(SEND_ORDER.contains(&r.client), "{}", r.label);
        }
        assert!(SEND_ORDER.contains(&crate::clientvars::STEP_HEIGHT_VAR));
        assert!(SEND_ORDER.contains(&crate::clientvars::WORLD_NORTH_VAR));
        for (i, k) in SEND_ORDER.iter().enumerate() {
            assert!(!SEND_ORDER[..i].contains(k));
        }
        // made-up names under real keys: the far clip first, then world north, the step height after fog_b
        let v = |label, key, name: &[u8], value: &[u8]| WorldVar {
            label,
            key,
            name: Some(name.to_vec()),
            value: value.to_vec(),
        };
        let world = vec![
            v("fog_b", WORLD_VARS[6].client, b"B", b"1"),
            v("far_z", WORLD_VARS[0].client, b"Far", b"2"),
            v("other", (dc_world::hash16(b"Zz"), 2), b"Zz", b"3"),
        ];
        let consts = vec![
            v(
                "step_height",
                crate::clientvars::STEP_HEIGHT_VAR,
                b"Step",
                b"30.000000",
            ),
            v(
                "world_north",
                crate::clientvars::WORLD_NORTH_VAR,
                b"North",
                b"0.000000",
            ),
        ];
        let names = |m: &[BitWriter]| -> Vec<Vec<u8>> {
            m.iter()
                .map(|b| {
                    let mut r = b.reader();
                    r.read_u8();
                    r.read_zstr8()
                })
                .collect()
        };
        let m = preload_messages(&world, &consts, &[]);
        assert_eq!(
            names(&m),
            vec![
                b"Far".to_vec(),
                b"North".to_vec(),
                b"B".to_vec(),
                b"Step".to_vec(),
                b"Zz".to_vec()
            ]
        );
        // `-evar` replaces a constant too and goes last
        let m = preload_messages(&world, &consts, &[("step".into(), "18".into())]);
        assert_eq!(
            names(&m),
            vec![
                b"Far".to_vec(),
                b"North".to_vec(),
                b"B".to_vec(),
                b"Zz".to_vec(),
                b"step".to_vec()
            ]
        );
    }
}
