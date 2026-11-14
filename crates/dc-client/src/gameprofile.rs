//! The content profile of this run (`--game <dir>`): run with your own content and no original game folder.
//!
//! `main` loads the profile named by the flag ([`load`]) and [`install`]s it once at start-up; code that builds a
//! file system reads it with [`active`] (`None`: no profile, the run reads exactly what it read before profiles
//! existed). The profile format is [`dc_config::game`].
// Provenance: ORIGINAL (DeCombat's own content profiles; no 2009 behaviour)
// DC-DEFAULT: ENG-3 (proposed) the content profile flag of the client, see DC_DEFAULTS.md

use std::path::Path;
use std::sync::{Arc, RwLock};

pub use dc_config::game::GameProfile;

/// The flag that names a profile folder.
pub const FLAG: &str = "--game";

static ACTIVE: RwLock<Option<Arc<GameProfile>>> = RwLock::new(None);

/// Loads and checks the profile at `path` (a profile folder or its `game.json`).
pub fn load(path: &Path) -> Result<Arc<GameProfile>, String> {
    GameProfile::load(path).map(Arc::new)
}

/// Makes `profile` this run's profile (`None`: no profile).
pub fn install(profile: Option<Arc<GameProfile>>) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = profile;
}

/// This run's profile, if `--game` named one.
pub fn active() -> Option<Arc<GameProfile>> {
    ACTIVE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-gameprofile-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// One test owns the process-global slot (tests run in parallel threads).
    #[test]
    fn a_loaded_profile_is_what_active_returns_and_none_clears_it() {
        assert!(active().is_none(), "no profile unless --game names one");
        let d = temp("active");
        std::fs::create_dir_all(d.join("base")).unwrap();
        std::fs::write(d.join("base/readme.txt"), b"own content\n").unwrap();
        std::fs::write(
            d.join("game.json"),
            br#"{"format":1,"id":"own","roots":["base"]}"#,
        )
        .unwrap();
        let p = load(&d).unwrap();
        install(Some(p.clone()));
        let got = active().expect("installed");
        assert_eq!(got.id, "own");
        assert_eq!(got.summary(), "game: own, 1 roots, 0 packs");
        // what later code builds from it: a file system with no archive that reads the root
        let v = got.vfs(None);
        assert_eq!(
            v.entry_text("any.rez", "readme.txt").unwrap().as_deref(),
            Some("own content\n")
        );
        install(None);
        assert!(active().is_none());
        assert!(load(&d.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
