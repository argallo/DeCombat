//! `dc-server-rom`: makes a server ROM (`docs/SERVER_ROM.md`) out of the user's 2009 client folder, or checks one.
// Provenance: ORIGINAL (our own tooling; no 2009 behaviour)

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  dc-server-rom <client folder> <out folder> [--only-worlds NAME,NAME,...]
      writes the server ROM: what dc-lobby-server and dc-dserver read from the client folder, as plain files
      plus server-rom.json (out folder: new, empty, or an earlier server ROM, which is replaced)
  dc-server-rom --verify <server ROM folder>
      checks every file of a server ROM against its manifest";

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1_000_000.0)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--verify" {
        let dir = PathBuf::from(&args[1]);
        return match dc_rez::dc_rom::identify_rom(&dir) {
            Ok(id) => {
                let n = dc_rez::dc_rom::server_rom::verify(&dir).unwrap_or(0);
                println!("dc-server-rom: {} OK ({id:?}, {n} files match the manifest)", dir.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("dc-server-rom: {}: {e}", dir.display());
                ExitCode::FAILURE
            }
        };
    }
    let mut pos = Vec::new();
    let mut opts = dc_rez::server_rom::BuildOptions::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--only-worlds" => {
                let Some(v) = it.next() else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };
                opts.only_worlds = Some(
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => pos.push(PathBuf::from(a)),
        }
    }
    if pos.len() != 2 {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    match dc_rez::server_rom::build(&pos[0], &pos[1], &opts) {
        Ok(r) => {
            for (name, listed, whole, heads, bytes) in &r.archives {
                if whole + heads > 0 {
                    println!(
                        "  {name:<28} {listed:>6} listed {whole:>5} whole {heads:>5} heads {:>10}",
                        mb(*bytes)
                    );
                }
            }
            println!(
                "dc-server-rom: {} -> {}: {} files, {} (manifest {}); client folder {} files, {}",
                pos[0].display(),
                pos[1].display(),
                r.out_files + 1,
                mb(r.out_bytes),
                mb(r.manifest_bytes),
                r.client_files,
                mb(r.client_bytes)
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dc-server-rom: {e}");
            ExitCode::FAILURE
        }
    }
}
