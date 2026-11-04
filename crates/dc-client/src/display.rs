//! Display settings the client takes from the user's 2009 profile: the gamma of the finished frame.

use std::path::Path;

/// The gamma the client draws with: the gamma entry of the user's profile in the client folder `rom`, else the
/// game's default (1.22).
// EVIDENCE: cshell+0x1ea350 (the profile's display gamma, default 1.22, read at start), cshell+0x1e9640 (the three
// engine gamma values set to it), engine+0xf6f10 (ramp built from them)
// OBSERVED: CA_ROM profile gamma (decrypted locally) equals the gamma fitted to the 2009 frames of sbs4 within 0.01
// (reports/client-render2.md)
// DC-DEFAULT: CLI-210 (proposed), see the staged rows (the profile is read once at start; the options page that
// changes it is not implemented; no ROM: the code default)
pub fn client_gamma(rom: Option<&Path>) -> f32 {
    rom.and_then(dc_render::profile_gamma)
        .unwrap_or(dc_render::PROFILE_GAMMA_DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rom_is_the_code_default() {
        assert_eq!(client_gamma(None), dc_render::PROFILE_GAMMA_DEFAULT);
        let missing = std::env::temp_dir().join("dc_client_no_such_rom_for_gamma");
        assert_eq!(
            client_gamma(Some(&missing)),
            dc_render::PROFILE_GAMMA_DEFAULT
        );
    }
}
