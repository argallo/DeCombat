//! The friend pack's double-click start (project decision, no 2009 counterpart). The Windows zip holds one thing to
//! start, `DeCombat.exe`, next to `pack.json` and the README. Started with no arguments in a folder whose `pack.json`
//! names a server, the exe finds the player's 2009 client folder beside it, then runs the game as a child process
//! with the pack's lobby, the found folder and the log files of the logs folder, the child's output going to
//! `dc-client.out` there (the last run's kept as `dc-client.prev.out`). A missing folder or a failed end is shown
//! in a message box: a double-clicked console closes before anyone reads it.
// DC-DEFAULT: CLI-1906 (proposed, OWNER REQUEST 2026-11-06), see DC_DEFAULTS.md

use crate::selfupdate::{self, Pack};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The run's output in the logs folder, and the run before it.
pub const OUT_FILE: &str = "dc-client.out";
pub const PREV_OUT_FILE: &str = "dc-client.prev.out";
/// The network log of the run (`--net-log`).
pub const NET_LOG_FILE: &str = "dc-client.net.jsonl";
/// The caption of every box of the start.
pub const TITLE: &str = "DeCombat";
/// The box when no client folder is next to the exe.
pub const NO_ROM: &str =
    "Put your Combat Arms folder, the one with Engine.exe and Game, next to DeCombat.exe, then start it again.";

/// The first folder in `dir` (by name, ignoring case) that holds a file `Engine.exe` and a folder `Game`.
pub fn find_rom(dir: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    dirs.into_iter()
        .find(|d| d.join("Engine.exe").is_file() && d.join("Game").is_dir())
}

/// Why the start cannot run the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// No client folder beside the exe.
    NoRom,
    /// The folder found is not a full client (the text says why).
    Rom(String),
}

impl Refused {
    pub fn text(&self) -> String {
        match self {
            Refused::NoRom => NO_ROM.to_string(),
            Refused::Rom(e) => e.clone(),
        }
    }
}

/// The game's command line for the pack in `dir` (`None`: the pack names no server, so this is no pack start):
/// `--menu lobby --lobby <server> --rom <folder> [--server-fingerprint <fp>] --net-log <logs>/dc-client.net.jsonl`.
/// No `--name`: the login screen's ID box starts empty or remembered (CLI-1490).
pub fn plan(dir: &Path, pack: &Pack, logs: &Path) -> Option<Result<Vec<OsString>, Refused>> {
    let server = pack.server.as_deref()?;
    let Some(rom) = find_rom(dir) else {
        return Some(Err(Refused::NoRom));
    };
    if let Err(e) = dc_rom::server_rom::refuse_for_client(&rom) {
        return Some(Err(Refused::Rom(e)));
    }
    let mut args: Vec<OsString> = vec![
        "--menu".into(),
        "lobby".into(),
        "--lobby".into(),
        server.into(),
        "--rom".into(),
        rom.into_os_string(),
    ];
    if let Some(fp) = pack.fingerprint.as_deref() {
        args.push("--server-fingerprint".into());
        args.push(fp.into());
    }
    args.push("--net-log".into());
    args.push(logs.join(NET_LOG_FILE).into_os_string());
    Some(Ok(args))
}

/// The box after a failed end of the game.
pub fn failed_text(code: i32, out: &Path) -> String {
    let code = if (0..=255).contains(&code) {
        code.to_string()
    } else {
        format!("{code} (0x{:08X})", code as u32)
    };
    format!(
        "DeCombat ended with an error, code {code}. The log is in {}; send it to the host.",
        out.display()
    )
}

/// A message box (dc-hangdump holds the Win32 call), and the same line on stderr. Tests only print it.
pub fn message_box(text: &str, error: bool) {
    eprintln!("dc-client: {text}");
    if !cfg!(test) {
        dc_hangdump::notice::message_box(TITLE, text, error);
    }
}

/// The start of an exe given no arguments: `None` when it is no pack start (no pack file beside it, or one without
/// a server), else the process code after the game ended.
pub fn run_if_pack() -> Option<i32> {
    let install = selfupdate::install_dir()?;
    let pack = selfupdate::pack()?;
    pack.server.as_ref()?;
    let running = selfupdate::running_name();
    // an older pack's exe name started by hand in a folder that has the new one: the new one starts instead
    if running
        .as_deref()
        .is_some_and(|r| r.eq_ignore_ascii_case(selfupdate::LEGACY_EXE))
        && selfupdate::EXE != selfupdate::LEGACY_EXE
        && install.join(selfupdate::EXE).is_file()
    {
        return Some(match std::process::Command::new(install.join(selfupdate::EXE)).spawn() {
            Ok(_) => 0,
            Err(e) => {
                message_box(&format!("{} does not start: {e}", selfupdate::EXE), true);
                2
            }
        });
    }
    let logs = crate::diag::default_logs_dir().unwrap_or_else(|| install.join("logs"));
    let args = match plan(&install, &pack, &logs)? {
        Ok(a) => a,
        Err(r) => {
            message_box(&r.text(), true);
            return Some(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&logs) {
        message_box(&format!("The log folder {} cannot be made: {e}", logs.display()), true);
        return Some(2);
    }
    let out = logs.join(OUT_FILE);
    if out.is_file() {
        let _ = std::fs::rename(&out, logs.join(PREV_OUT_FILE));
    }
    let file = match std::fs::File::create(&out) {
        Ok(f) => f,
        Err(e) => {
            message_box(&format!("{} cannot be written: {e}", out.display()), true);
            return Some(2);
        }
    };
    let err_file = match file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            message_box(&format!("{} cannot be written: {e}", out.display()), true);
            return Some(2);
        }
    };
    let me = std::env::current_exe().ok()?;
    let child = std::process::Command::new(&me)
        .args(&args)
        .env("DC_WATCHDOG_DIR", &logs)
        .env(selfupdate::PACK_START_ENV, "1")
        .stdout(file)
        .stderr(err_file)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            message_box(&format!("DeCombat does not start: {e}"), true);
            return Some(2);
        }
    };
    // the older starters go once this one runs (a moment's retry while an older process ends)
    for _ in 0..20 {
        selfupdate::tidy_legacy(&install, running.as_deref());
        if !install.join(selfupdate::LEGACY_EXE).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let code = match child.wait() {
        Ok(st) => st.code().unwrap_or(1),
        Err(_) => 1,
    };
    if code != 0 {
        message_box(&failed_text(code, &out), true);
    }
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_packstart_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn client_folder(at: &Path) {
        std::fs::create_dir_all(at.join("Game")).unwrap();
        std::fs::write(at.join("Engine.exe"), b"not a real client").unwrap();
    }

    fn pack(server: Option<&str>, fp: Option<&str>) -> Pack {
        Pack {
            commit: "abcd1234".into(),
            update_dir: None,
            server: server.map(str::to_string),
            fingerprint: fp.map(str::to_string),
        }
    }

    fn strings(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_client_folder_is_the_first_with_engine_and_game() {
        let d = temp("rom");
        assert_eq!(find_rom(&d), None);
        // an Engine.exe without Game, a Game without Engine.exe, a file: none of them
        std::fs::create_dir_all(d.join("a_half")).unwrap();
        std::fs::write(d.join("a_half").join("Engine.exe"), b"x").unwrap();
        std::fs::create_dir_all(d.join("b_half").join("Game")).unwrap();
        std::fs::write(d.join("Engine.exe"), b"x").unwrap();
        assert_eq!(find_rom(&d), None);
        client_folder(&d.join("Combat Arms"));
        client_folder(&d.join("z copy"));
        assert_eq!(find_rom(&d), Some(d.join("Combat Arms")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pack_start_joins_the_packs_server_with_the_found_folder() {
        let d = temp("plan");
        let logs = d.join("logs");
        // no server: no pack start, whatever is in the folder
        assert_eq!(plan(&d, &pack(None, None), &logs), None);
        assert_eq!(
            plan(&d, &pack(Some("play.example.org:10000"), None), &logs),
            Some(Err(Refused::NoRom))
        );
        assert!(Refused::NoRom.text().contains("next to DeCombat.exe"));
        client_folder(&d.join("Combat Arms"));
        let rom = d.join("Combat Arms").to_string_lossy().into_owned();
        let net = logs.join(NET_LOG_FILE).to_string_lossy().into_owned();
        let a = plan(&d, &pack(Some("play.example.org:10000"), None), &logs)
            .unwrap()
            .unwrap();
        assert_eq!(
            strings(&a),
            [
                "--menu",
                "lobby",
                "--lobby",
                "play.example.org:10000",
                "--rom",
                &rom,
                "--net-log",
                &net
            ]
        );
        let fp = "ab".repeat(32);
        let a = plan(&d, &pack(Some("10.0.0.2:10000"), Some(&fp)), &logs)
            .unwrap()
            .unwrap();
        assert_eq!(
            strings(&a),
            [
                "--menu",
                "lobby",
                "--lobby",
                "10.0.0.2:10000",
                "--rom",
                &rom,
                "--server-fingerprint",
                &fp,
                "--net-log",
                &net
            ]
        );
        assert!(!strings(&a).iter().any(|s| s == "--name"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_end_names_the_log() {
        let t = failed_text(3, Path::new("C:/logs/dc-client.out"));
        assert!(t.contains("code 3.") && t.contains("C:/logs/dc-client.out"), "{t}");
        let t = failed_text(-1073740791, Path::new("x"));
        assert!(t.contains("0xC0000409"), "{t}");
    }
}
