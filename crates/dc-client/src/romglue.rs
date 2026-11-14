//! Values the client reads out of the user's own 2009 client binary and profile, handed to the renderer as plain
//! values (the renderer itself reads no original library): the arm placeholder texture of the first-person rows,
//! the profile key and the profile's display gamma.

use std::path::Path;

use dc_render::{
    gamma_of_profile_text, name_key, PROFILE_FILE, PROFILE_KEY_RVA, PROFILE_KEY_SHA256, PV_NONE_TEXTURE_KEY,
    PV_NONE_TEXTURE_RVA,
};

/// The user's CShell.dll in the client folder `rom`.
fn shell(rom: &Path) -> Option<dc_rom::PeFile> {
    dc_rom::PeFile::from_file(&rom.join("Game").join("CShell.dll")).ok()
}

/// The replacement texture name of the arm placeholders, read from the user's CShell.dll by RVA and accepted only
/// when its name key matches (nothing of the ROM in the tree).
// DISASM: cshell+0x23b956 (see dc_render::PV_NONE_TEXTURE_RVA)
pub fn pv_none_texture(rom: &Path) -> Option<String> {
    let s = shell(rom)?.cstr(PV_NONE_TEXTURE_RVA).ok()?;
    (name_key(s.as_bytes()) == PV_NONE_TEXTURE_KEY).then_some(s)
}

/// The client's own profile key (read from the user's CShell.dll by RVA, accepted by its pinned SHA-256); `None`
/// when the binary is missing or does not match. The options screen reads and writes the profile with it
/// (w3-options).
// EVIDENCE: cshell+0x259930 (see dc_render::PROFILE_KEY_RVA)
pub fn profile_key(rom: &Path) -> Option<Vec<u8>> {
    dc_rom::crypt_key_from_pe(&shell(rom)?, PROFILE_KEY_RVA, PROFILE_KEY_SHA256).ok()
}

/// The gamma of the user's profile in the client folder `rom` (decrypted with the client's own profile key);
/// `None` when anything is missing.
// SPEC: same framing as the archive attributes (Blowfish ECB over byte-swapped words, plain tail), dc-rez;
// OBSERVED: the four CA_ROM profiles decrypt this way to plain text with a display section
pub fn profile_gamma(rom: &Path) -> Option<f32> {
    let key = profile_key(rom)?;
    let raw = std::fs::read(rom.join(PROFILE_FILE)).ok()?;
    dc_rom::journal::note(
        || format!("file:{}", rom.join(PROFILE_FILE).display()),
        &raw,
    );
    let plain = dc_rez::decrypt_with_key(&raw, &key).ok()?;
    gamma_of_profile_text(&String::from_utf8_lossy(&plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_client_folder_gives_nothing() {
        let none = Path::new("/nowhere/at/all");
        assert_eq!(pv_none_texture(none), None);
        assert_eq!(profile_key(none), None);
        assert_eq!(profile_gamma(none), None);
    }
}
