//! `refdump --rom <CA_ROM> --out <dir> [--only entries,decrypt,bute,pe]`
use std::path::PathBuf;

fn main() {
    let mut rom: Option<PathBuf> = std::env::var_os("CA_ROM").map(PathBuf::from);
    let mut out: Option<PathBuf> = None;
    let mut only = "entries,decrypt,bute,pe".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--only" => only = args.next().unwrap_or_default(),
            _ => {
                eprintln!(
                    "usage: refdump --rom <CA_ROM> --out <dir> [--only entries,decrypt,bute,pe]"
                );
                std::process::exit(2);
            }
        }
    }
    let (Some(rom), Some(out)) = (rom, out) else {
        eprintln!("need --rom (or CA_ROM) and --out");
        std::process::exit(2);
    };
    match dc_xtask::run(&rom, &out, &only) {
        Ok(c) => println!("{}", c.to_json()),
        Err(e) => {
            eprintln!("refdump failed: {e}");
            std::process::exit(1);
        }
    }
}
