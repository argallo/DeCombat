//! Server constants the server pushes with `S2C_EVAR` at the world load (before `S2C_LOAD_WORLD`): the variables
//! outside the world-settings object that the 2009 client reads.
//!
//! The client reads server variables only through the engine's server-variable float read (client slot +0x134,
//! engine+0xd760), which writes 0 and reports "not found" when the server never sent the variable. Apart from the
//! world-settings rows (`evars.rs`), exactly two names are read that way: the step height (the largest riser the
//! local mover climbs without a jump; a missing variable pushes 0 into the physics, so 16-unit stair risers block)
//! and the world-north angle (copied into the client's console value of the same name at world entry, used by the
//! HUD's radar / compass rotation). Every other variable the 2009 server sent has no reader in the client.
//!
//! The names are client strings: they are found at run time in the user's own `Game/CShell.dll` by their hashes
//! (SHA-256 prefix and length). The tree holds only the hashes.
// SPEC: MOVEMENT_2009.md §2 (`STEP_HEIGHT`, key eda10d3a6476dd56/15, read through client slot +0x134 with the
// caller's default 30.0; a value other than 30.0 is written into the physics, engine+0xd6a20), §9.7 (stair move);
// UDP_SESSION_2009_A.md §4.5 (`S2C_EVAR` `[name:zstr8][value:zstr8]`), §8.2 (phase 1: variables, then
// `S2C_LOAD_WORLD`)
// EVIDENCE: engine+0xd760 (the server-variable float read stores 0 into the output before the lookup and returns
// not-found for a missing name); cshell+0x1fe6c0 (the mover's read of the step height, compare with 30.0);
// cshell+0x1a3910 (30.0 written at physics creation)
// OBSERVED: capture 20261025_002236_tdm4 (the reference stack sent this variable with the value text
// `30.000000` on both links, conn 5 t 56.234 and conn 6 t 62.297 just before its `S2C_LOAD_WORLD`; the real client
// ran up the SNOWVALLEY staircase on the ground, seq 3741-3777; REFERENCE); capture
// 20261025_095358_rust4 (no `S2C_EVAR` at all from our server: the same staircase blocked four times and needed
// jumps, seq 39278-39936)
// DC-DEFAULT: DSV-53 (proposed) send the step height 30 at every world load; `-stepheight <units>|off`
// EVIDENCE: engine+0xd760 is the reader of the client's server-variable table (CShell reaches it through slot +0x134 from several sites, e.g. cshell+0x2cba98 too; dserver-servervars-verify.md) (engine+0xa50c0 writes it,
// engine+0x15980 / engine+0x159d0 create and clear it; no binding, no enumeration); CShell calls the read through
// slot +0x134 only at cshell+0x1a55f2 and cshell+0x1fe738 (step height), cshell+0x1a687a (cshell+0x1a6840, called
// from cshell+0x1a6900 and cshell+0x1a9b00 with the world-settings names and the world-north name) and
// cshell+0x1a9c00 / 0x1a9c18 / 0x1a9c30 (wind); ClientFx.fxd has no such call
// DISASM: servervars scan 2026-10-25 (every `call [reg+0x134]` of CShell.dll, ClientFx.fxd and Engine.exe with the
// string pushed before it)
// DC-DEFAULT: DSV-60 (proposed) send the world-north angle 0 at every world load, in the reference stack's order

use std::path::Path;

use crate::world::PropHash;
use dc_wire::udp::BitWriter;

/// Hash of the client's step-height variable name (SHA-256 prefix, length).
// SPEC: MOVEMENT_2009.md §2 (`STEP_HEIGHT`)
pub const STEP_HEIGHT_VAR: PropHash = ([0xed, 0xa1, 0x0d, 0x3a, 0x64, 0x76, 0xdd, 0x56], 15);

/// The step height the server sends by default (world units).
// EVIDENCE: cshell+0x1fe6c0, cshell+0x1a3910 (the client's own value); OBSERVED: tdm4 (the reference stack's value text)
// DC-DEFAULT: DSV-53 (proposed) 30
pub const DEFAULT_STEP_HEIGHT: f32 = 30.0;

/// Hash of the client's world-north variable name (SHA-256 prefix, length).
// EVIDENCE: cshell+0x1a6900 (read at world entry through cshell+0x1a6840 and copied into the console value of the
// same name); cshell+0xaeab0, cshell+0xb60f0, cshell+0xc8620 (the HUD reads that console value, an angle in
// radians, default 0)
pub const WORLD_NORTH_VAR: PropHash = ([0x18, 0xe9, 0x1a, 0x62, 0x81, 0x02, 0x0c, 0x4a], 10);

/// The world-north angle the server sends by default (radians).
// OBSERVED: capture 20261025_002236_tdm4 and 20261025_065016_tdm6 (`0.000000` on both
// worlds and every link; also tdm1, tdm2, botduel1: a server constant, not a world property; REFERENCE)
// DC-DEFAULT: DSV-60 (proposed) 0, the reference stack value (project decision frame-rate independence: the server sends what
// the client reads)
pub const DEFAULT_WORLD_NORTH: f32 = 0.0;

/// Client names of the pushed variables, resolved from the user's ROM.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientVarNames {
    /// The step-height variable's name; `None` when the ROM did not give it (then nothing is sent).
    pub step_height: Option<Vec<u8>>,
    /// The world-north variable's name; `None` when the ROM did not give it (then nothing is sent).
    pub world_north: Option<Vec<u8>>,
}

/// A NUL-terminated printable string of `bytes` whose hash and length equal `key`. The string may follow other data
/// directly (in the shell library this name follows a pointer table without a NUL in front), so the window that
/// ends at each NUL is tested, not the whole NUL-delimited run.
pub fn find_name(bytes: &[u8], key: PropHash) -> Option<Vec<u8>> {
    let len = key.1;
    if len == 0 {
        return None;
    }
    bytes
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == 0 && i >= len)
        .map(|(i, _)| &bytes[i - len..i])
        .find(|w| w.iter().all(u8::is_ascii_graphic) && dc_world::hash16(w) == key.0)
        .map(<[u8]>::to_vec)
}

/// The names from the user's `Game/CShell.dll` (missing file or name: `None`).
pub fn resolve(rom: &Path) -> ClientVarNames {
    let bytes = dc_rez::dc_rom::server_rom::shell_image(rom);
    ClientVarNames {
        step_height: bytes.as_deref().and_then(|b| find_name(b, STEP_HEIGHT_VAR)),
        world_north: bytes.as_deref().and_then(|b| find_name(b, WORLD_NORTH_VAR)),
    }
}

/// A real value as the client's variables carry it: six decimals.
// OBSERVED: capture 20261025_002236_tdm4 (`30.000000`; REFERENCE)
pub fn real_text(v: f32) -> Vec<u8> {
    format!("{:.6}", v as f64).into_bytes()
}

/// The `S2C_EVAR` messages to send before `S2C_LOAD_WORLD`: the step height when it is configured and its name is
/// known.
// DC-DEFAULT: DSV-53 (proposed)
pub fn messages(names: &ClientVarNames, step_height: Option<f32>) -> Vec<BitWriter> {
    let mut out = Vec::new();
    if let (Some(n), Some(v)) = (&names.step_height, step_height) {
        out.push(crate::msg::s2c_evar(n, &real_text(v)));
    }
    out
}

/// The server constants as variables (label, key, name, value text): the step height and the world-north angle when
/// each is configured and its name is known. `evars::preload_messages` puts them into the reference stack's order.
// DC-DEFAULT: DSV-53 (proposed), DSV-60 (proposed)
pub fn vars(
    names: &ClientVarNames,
    step_height: Option<f32>,
    world_north: Option<f32>,
) -> Vec<crate::evars::WorldVar> {
    let mut out = Vec::new();
    let rows = [
        (
            "step_height",
            STEP_HEIGHT_VAR,
            &names.step_height,
            step_height,
        ),
        (
            "world_north",
            WORLD_NORTH_VAR,
            &names.world_north,
            world_north,
        ),
    ];
    for (label, key, name, v) in rows {
        if let (Some(n), Some(v)) = (name, v) {
            out.push(crate::evars::WorldVar {
                label,
                key,
                name: Some(n.clone()),
                value: real_text(v),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_name_by_hash_even_without_a_leading_nul() {
        // made-up names
        let k = (dc_world::hash16(b"Ladder"), 6);
        // directly after binary data (0x37 is printable: the window, not the run, must match)
        let bin = b"\x10\x54\x2a\x37Ladder\0tail\0";
        assert_eq!(find_name(bin, k), Some(b"Ladder".to_vec()));
        // NUL-delimited as usual
        assert_eq!(find_name(b"\0Ladder\0", k), Some(b"Ladder".to_vec()));
        // not NUL-terminated, wrong case, or a non-printable byte inside: no match
        assert_eq!(find_name(b"\0Ladder", k), None);
        assert_eq!(find_name(b"\0ladder\0", k), None);
        assert_eq!(find_name(b"\0Lad\x01er\0", k), None);
        assert_eq!(find_name(b"", k), None);
    }

    #[test]
    fn step_height_message_bytes() {
        assert_eq!(real_text(30.0), b"30.000000");
        assert_eq!(real_text(18.5), b"18.500000");
        let names = ClientVarNames {
            step_height: Some(b"Stepper".to_vec()),
            world_north: None,
        };
        let m = messages(&names, Some(DEFAULT_STEP_HEIGHT));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].as_bytes(), b"\x0fStepper\x0030.000000\x00");
        // off, or no name from the ROM: nothing
        assert!(messages(&names, None).is_empty());
        assert!(messages(&ClientVarNames::default(), Some(30.0)).is_empty());
    }

    #[test]
    fn the_capture_form_of_the_reference_message() {
        // The reference stack's message in tdm4 is `0f <name> 00 "30.000000" 00`: our bytes for the same name are equal. The
        // name itself is not in the tree; the check runs against the user's ROM when it is present.
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let names = resolve(&rom);
        let Some(n) = &names.step_height else {
            panic!("the step-height name is in the ROM's CShell.dll");
        };
        assert_eq!(n.len(), STEP_HEIGHT_VAR.1);
        assert_eq!(dc_world::hash16(n), STEP_HEIGHT_VAR.0);
        let m = messages(&names, Some(30.0));
        let mut want = vec![0x0f];
        want.extend_from_slice(n);
        want.push(0);
        want.extend_from_slice(b"30.000000\0");
        assert_eq!(m[0].as_bytes(), want.as_slice());
        let Some(wn) = &names.world_north else {
            panic!("the world-north name is in the ROM's CShell.dll");
        };
        assert_eq!((dc_world::hash16(wn), wn.len()), WORLD_NORTH_VAR);
    }

    #[test]
    fn constants_as_variables() {
        // made-up names
        let names = ClientVarNames {
            step_height: Some(b"Stepper".to_vec()),
            world_north: Some(b"Northing".to_vec()),
        };
        let v = vars(&names, Some(30.0), Some(DEFAULT_WORLD_NORTH));
        let got: Vec<_> = v
            .iter()
            .map(|x| (x.label, x.key, x.name.clone().unwrap(), x.value.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "step_height",
                    STEP_HEIGHT_VAR,
                    b"Stepper".to_vec(),
                    b"30.000000".to_vec()
                ),
                (
                    "world_north",
                    WORLD_NORTH_VAR,
                    b"Northing".to_vec(),
                    b"0.000000".to_vec()
                ),
            ]
        );
        assert!(vars(&names, None, None).is_empty());
        assert!(vars(&ClientVarNames::default(), Some(30.0), Some(0.0)).is_empty());
        assert_eq!(vars(&names, None, Some(1.5))[0].value, b"1.500000");
    }
}
