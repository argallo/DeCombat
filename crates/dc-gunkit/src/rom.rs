//! Finding the player's game folder, so that the studio just opens when it can and asks once when it cannot.
// DC-DEFAULT: VAR-6 (proposed) the order: the folder named, CA_ROM, the one remembered, next to the program, a `rom` folder above it

use std::path::{Path, PathBuf};

/// A Combat Arms folder: `Game/` with the archives, and the key in `Engine.exe` or `Game/CShell.dll`.
pub fn is_rom(dir: &Path) -> bool {
    dir.join("Game").is_dir()
        && (dir.join("Engine.exe").is_file() || dir.join("Game").join("CShell.dll").is_file())
}

/// The game folders directly in `dir`, by name.
fn roms_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(list) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = list
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && is_rom(p))
        .collect();
    out.sort();
    out
}

/// Looks beside a program: its own folder, what is in it, and a `rom` folder in it or in a parent (a source checkout keeps it there).
fn near(exe_dir: &Path) -> Option<PathBuf> {
    for dir in exe_dir.ancestors().take(6) {
        if is_rom(dir) {
            return Some(dir.to_path_buf());
        }
        if let Some(r) = roms_in(dir).into_iter().next() {
            return Some(r);
        }
        if let Some(r) = roms_in(&dir.join("rom")).into_iter().next() {
            return Some(r);
        }
    }
    None
}

fn remembered_file(home: &Path) -> PathBuf {
    home.join("rom-path.txt")
}

/// Keeps the folder for the next start.
pub fn remember(home: &Path, rom: &Path) {
    let _ = std::fs::create_dir_all(home);
    let _ = std::fs::write(remembered_file(home), rom.display().to_string());
}

/// The game folder to open, when one can be found.
pub fn discover(named: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(n) = named {
        return is_rom(n).then(|| n.to_path_buf());
    }
    if let Some(d) = std::env::var_os("CA_ROM").map(PathBuf::from) {
        if is_rom(&d) {
            return Some(d);
        }
    }
    if let Some(h) = home {
        if let Ok(text) = std::fs::read_to_string(remembered_file(h)) {
            let d = PathBuf::from(text.trim());
            if is_rom(&d) {
                return Some(d);
            }
        }
    }
    let exe = std::env::current_exe().ok()?;
    near(exe.parent()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_rom(dir: &Path) {
        std::fs::create_dir_all(dir.join("Game")).unwrap();
        std::fs::write(dir.join("Engine.exe"), b"x").unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-gunkit-rom-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_game_folder_needs_its_archives_and_its_key() {
        let d = temp("is");
        assert!(!is_rom(&d));
        std::fs::create_dir_all(d.join("Game")).unwrap();
        assert!(!is_rom(&d));
        std::fs::write(d.join("Game/CShell.dll"), b"x").unwrap();
        assert!(is_rom(&d));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn it_is_found_beside_the_program_or_in_a_rom_folder_above_it() {
        let d = temp("near");
        let exe = d.join("target/debug");
        std::fs::create_dir_all(&exe).unwrap();
        assert_eq!(near(&exe), None);
        let rom = d.join("rom/EU");
        fake_rom(&rom);
        assert_eq!(near(&exe), Some(rom));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_named_folder_decides_and_a_remembered_one_is_kept() {
        let d = temp("named");
        let rom = d.join("game");
        fake_rom(&rom);
        assert_eq!(discover(Some(&rom), None), Some(rom.clone()));
        assert_eq!(discover(Some(&d), None), None, "not a game folder");
        let home = d.join("home");
        remember(&home, &rom);
        assert_eq!(
            std::fs::read_to_string(home.join("rom-path.txt")).unwrap(),
            rom.display().to_string()
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
