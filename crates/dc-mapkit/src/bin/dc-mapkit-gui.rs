//! `dc-mapkit-gui`: the editor window (MAPKIT_PLAN.md 5.2; DC-DEFAULT: MAP-66). On Windows it is a GUI-subsystem
//! binary (no console), so the console `dc-mapkit` keeps stdout for `--json` and `mcp` and its `gui` verb spawns
//! this one. Usage: `dc-mapkit-gui [--smoke <out dir>] [<project dir>]`; `--smoke` runs the scripted check
//! (first build, place a spawn, rebuild, export, play, viewport read-back) and closes.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("dc-mapkit-gui [--smoke <out dir>] [<project dir>]: the DeCombat map editor (docs/MAPKIT_AGENTS.md section 16)");
        return;
    }
    let mut smoke = None;
    let mut dir = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--smoke" {
            smoke = args.get(i + 1).map(std::path::PathBuf::from);
            i += 2;
        } else {
            if !args[i].starts_with('-') {
                dir = Some(std::path::PathBuf::from(&args[i]));
            }
            i += 1;
        }
    }
    if let Err(e) = dc_mapkit::gui::launch_with(dir, smoke) {
        eprintln!("dc-mapkit-gui: {e}");
        std::process::exit(1);
    }
}
