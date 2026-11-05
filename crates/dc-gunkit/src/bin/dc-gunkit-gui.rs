//! `dc-gunkit-gui`: the studio's window. On Windows a GUI-subsystem binary (no console), as `dc-mapkit-gui` is (MAP-66).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Err(e) =
        dc_gunkit::gui::launch(value("--rom").map(std::path::PathBuf::from), value("--gun"))
    {
        eprintln!("dc-gunkit-gui: {e}");
        std::process::exit(1);
    }
}
