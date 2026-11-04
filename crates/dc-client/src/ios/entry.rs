//! The iPad app's entry (CLI-1604): what `dc-client --menu lobby --lobby <server> --name <name> --rom <rom>
//! [--server-fingerprint <pin>] --net-log <logs>/dc-client.net.jsonl` does on the Mac (the friend pack's launcher),
//! from `Documents/decombat.toml` ([`super::config`]). Logs, the watchdog's reports and the crash folder go to
//! `Documents/logs`; the profile (options, key binds, known server certificates) lives in the app container's
//! `Library/Application Support/DeCombat`, as under the Mac's home. A missing or unreadable config or ROM folder is
//! shown in a message box over the login screen (without the ROM the menu draws its text-only screens in a system
//! font), never a crash.
// DC-DEFAULT: CLI-1604 (proposed)

use super::config::{self, IosConfig};
use crate::menu::link::LinkOptions;
use crate::menu::popup::{BoxTag, Popup};
use crate::menu::{Menu, MenuOptions};
use crate::net::{Connect, NetOptions};
use crate::window::{WindowLook, WindowOptions};
use dc_botclient::run::RoomChoice;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

/// The auth address the menu is given when the config names none or names one that does not resolve (the login then
/// fails with the 2009 connection box; the message box above it says why).
const NO_SERVER: &str = "127.0.0.1:10000";

/// Runs the client as the iPad app: reads the config, builds the menu as `run_menu` does and enters UIKit's event
/// loop (which never returns). The return value is for a start that fails before the loop.
pub fn ios_main() -> i32 {
    let docs = dc_ios_hid::documents_dir();
    let logs = docs.join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs) {
        eprintln!("dc-client ios: logs folder {}: {e}", logs.display());
    }
    // the working folder is the log folder (the crash-report folder is made there at every start, as the Mac
    // launcher does); the watchdog writes its reports there and never aborts the app (iPadOS ends a hung app
    // itself)
    let _ = std::env::set_current_dir(&logs);
    if std::env::var_os("DC_WATCHDOG_DIR").is_none() {
        std::env::set_var("DC_WATCHDOG_DIR", &logs);
    }
    if std::env::var_os("DC_WATCHDOG_ABORT").is_none() {
        std::env::set_var("DC_WATCHDOG_ABORT", "0");
    }
    let mut problems: Vec<String> = Vec::new();
    let cfg_path = docs.join(config::FILE);
    let cfg = match std::fs::read_to_string(&cfg_path) {
        Ok(text) => config::parse(&text),
        Err(e) => {
            problems.push(format!(
                "No {} in the app's Documents ({e}). Write server = \"host:port\" into it (name, fingerprint and rom are optional).",
                config::FILE
            ));
            IosConfig::default()
        }
    };
    for w in &cfg.warnings {
        eprintln!("dc-client ios: {}: {w}", config::FILE);
    }
    if cfg.debug_input {
        std::env::set_var("DC_IOS_INPUT_LOG", "1");
    }
    if !cfg.debug_frames {
        std::env::set_var("DC_IOS_NO_FRAMES", "1");
    }
    eprintln!(
        "dc-client ios: config {}: server {:?}, fingerprint {:?}, name {:?}, rom {:?}",
        cfg_path.display(),
        cfg.server,
        cfg.fingerprint,
        cfg.name,
        cfg.rom
    );
    match start(&docs, &logs, &cfg, problems) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("dc-client ios: {e}");
            1
        }
    }
}

/// The lobby address of the config (`problems` gets why there is none).
fn auth_of(cfg: &IosConfig, problems: &mut Vec<String>) -> SocketAddr {
    let fallback: SocketAddr = NO_SERVER.parse().expect("a socket address");
    let Some(server) = cfg.server.as_deref() else {
        if problems.is_empty() {
            problems.push(format!(
                "{} names no server (server = \"host:port\").",
                config::FILE
            ));
        }
        return fallback;
    };
    match server.to_socket_addrs().map(|mut a| a.next()) {
        Ok(Some(at)) => {
            // CLI-1473: the typed host name is what a CA-issued certificate names
            if let Some((host, _)) = server.rsplit_once(':') {
                dc_botclient::tls::note_host(
                    at.ip(),
                    host.trim_start_matches('[').trim_end_matches(']'),
                );
            }
            at
        }
        Ok(None) => {
            problems.push(format!("The server {server} has no address."));
            fallback
        }
        Err(e) => {
            problems.push(format!("The server {server} cannot be found: {e}."));
            fallback
        }
    }
}

/// The ROM folder of the config, checked as `--rom` is (`None`: missing, `problems` says why).
fn rom_of(docs: &Path, cfg: &IosConfig, problems: &mut Vec<String>) -> Option<PathBuf> {
    let rom = docs.join(&cfg.rom);
    if let Err(e) = crate::check_rom_dir(&rom) {
        problems.push(format!(
            "{e}. Copy the 2009 Combat Arms client folder into the app's Documents as {}.",
            cfg.rom
        ));
        return None;
    }
    if let Err(e) = dc_rom::server_rom::refuse_for_client(&rom) {
        problems.push(e);
        return None;
    }
    Some(rom)
}

/// A system font for the text-only menu (no ROM): a sans face the system has, else its first upright one.
fn system_font() -> Option<(Vec<u8>, usize)> {
    use crate::menu::sysfont;
    let faces: Vec<_> = sysfont::font_dirs()
        .iter()
        .flat_map(|d| sysfont::scan(d))
        .collect();
    for family in ["Helvetica", "Helvetica Neue", "Arial", "Avenir Next"] {
        if let Some(f) = sysfont::load(&faces, family, false) {
            return Some(f);
        }
    }
    let first = faces.iter().find(|f| !f.italic)?;
    sysfont::load(&faces, &first.family.clone(), false)
}

fn start(
    docs: &Path,
    logs: &Path,
    cfg: &IosConfig,
    mut problems: Vec<String>,
) -> Result<(), String> {
    // CLI-1470: the trust-on-first-use fingerprints live in the profile folder; CLI-1471: the config's pin wins
    let profile = crate::settings::own_profile_dir();
    if let Some(d) = &profile {
        eprintln!("dc-client ios: profile folder {}", d.display());
        dc_botclient::tls::set_store(d.join(dc_botclient::tls::KNOWN_SERVERS_FILE));
    }
    if let Some(fp) = cfg.fingerprint.as_deref() {
        if let Err(e) = dc_botclient::tls::set_pin(fp) {
            problems.push(format!(
                "The fingerprint in {} is refused: {e}.",
                config::FILE
            ));
        }
    }
    let auth = auth_of(cfg, &mut problems);
    let rom = rom_of(docs, cfg, &mut problems);
    // gate T1: the panic report and the crash folder go where the watchdog writes
    dc_hangdump::crumb::configure_from_env();
    crate::watchdog::install_crash_reports(crate::watchdog::report_dir(Some(logs.to_path_buf())));
    let net_log = logs.join("dc-client.net.jsonl");
    // `--menu lobby` without `--name`: the login screen's ID box starts empty (CLI-1490)
    let name = cfg.name.clone().unwrap_or_default();
    let mut base = NetOptions::new(
        Connect::Lobby {
            auth,
            room: RoomChoice::Any,
            password: String::new(),
            channel: None,
        },
        &name,
    );
    base.rom = rom.clone();
    base.log = Some(net_log.clone());
    let mut link = LinkOptions::new(&base.name);
    // CLI-1480: the player's client opens the nickname screen for a login without a character
    link.auto_character = std::env::var("DC_AUTO_CHARACTER").is_ok_and(|v| v == "1");
    link.log = Some(net_log.with_extension("menu.jsonl"));
    let mut menu = Menu::new(MenuOptions {
        auth,
        user: name.clone(),
        link,
        rom: rom.clone(),
        font: if rom.is_none() { system_font() } else { None },
        width: 1024,
        height: 768,
    });
    eprintln!("dc-client ios: menu {}", menu.source());
    // w3-options: the player's profile (options, key binds) is read at start-up
    let store = crate::settings::ProfileStore::for_client(rom.as_deref(), profile.as_deref());
    match menu.load_settings(store) {
        Some(p) => eprintln!("dc-client ios: settings from {}", p.display()),
        None => eprintln!("dc-client ios: no profile found, default settings"),
    }
    let mut wopts = WindowOptions {
        size: None,
        windowed_size: None,
        // the window is the screen: the full-screen look for the run, nothing saved
        start_look: Some(WindowLook::Borderless),
        keep_size: false,
        monitor: 0,
        seconds: None,
        scripted: false,
        walk_test: false,
        shoot_test: false,
        debug_boxes: false,
        tick_hz: dc_sim::DEFAULT_TICK_HZ,
        spawn: 0,
        view: crate::ViewSettings::with_reference_fov_x(0.0),
        params: dc_sim::MovementParams::default(),
        look: crate::LookParams::DEFAULT,
        vsync: false,
        no_vsync: false,
        font_rom: rom.clone(),
        adapter: dc_render::AdapterChoice::Auto,
        hud: match &rom {
            Some(r) => crate::hud::HudChoice::Rom(r.clone()),
            None => crate::hud::HudChoice::Fallback,
        },
        debug_panel: false,
        board_at: None,
        sound: crate::sound::SoundOptions {
            enabled: true,
            log: None,
        },
        audio: crate::sound::AudioConfig {
            out: crate::sound::AudioOut::Device,
            effects_volume: 100,
            music_volume: 100,
            ..Default::default()
        },
    };
    // w3-options: the profile's look settings and volumes, as the Mac's run without flags takes them (CLI-554)
    if menu.data.profile_doc.is_some() {
        let s = &menu.data.settings;
        wopts.look.sensitivity = s.sensitivity as f32;
        wopts.look.invert_y = s.invert_y;
        wopts.windowed_size = s.resolution;
        wopts.audio.effects_volume = s.effective_effects();
        wopts.audio.music_volume = s.effective_music();
    }
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("dc-client ios: {p}");
        }
        menu.show_popup(Popup::ok(problems.join("\n"), BoxTag::None));
    }
    crate::menu::window::run_ipad(menu, wopts, base).map_err(|e| e.to_string())
}
