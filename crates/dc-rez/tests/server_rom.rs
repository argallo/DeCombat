//! A server ROM built from the user's client folder reads like the client folder for every entry it keeps
//! (`docs/SERVER_ROM.md`). Needs `CA_ROM` = a full client folder; `DC_SERVER_ROM` = an already built server ROM of it
//! (else one with a single world is built under the temp folder).
// Provenance: ORIGINAL (our own tooling; no 2009 behaviour)

use dc_rez::dc_rom::{self, server_rom};
use dc_rez::server_rom::{build, BuildOptions};
use dc_rez::RezVfs;
use std::path::PathBuf;

fn server_rom_of(full: &std::path::Path) -> (PathBuf, bool) {
    if let Some(p) = std::env::var_os("DC_SERVER_ROM").filter(|p| !p.is_empty()) {
        return (PathBuf::from(p), false);
    }
    let out = std::env::temp_dir().join(format!("dc_server_rom_test_{}", std::process::id()));
    let opts = BuildOptions {
        only_worlds: Some(vec!["JUNK_FLEA".into()]),
    };
    build(full, &out, &opts).expect("server ROM build");
    (out, true)
}

#[test]
fn a_server_rom_reads_like_the_client_folder() {
    let full = dc_rom::rom_or_skip!();
    if server_rom::is_server_rom(&full) {
        println!("skipped: CA_ROM is a server ROM");
        return;
    }
    let (srom, temp) = server_rom_of(&full);
    assert!(server_rom::is_server_rom(&srom));
    // identity, key and file hashes
    assert_eq!(
        dc_rom::identify_rom(&srom).unwrap(),
        dc_rom::identify_rom(&full).unwrap()
    );
    assert_eq!(
        dc_rom::crypt_key(&srom).unwrap(),
        dc_rom::crypt_key(&full).unwrap()
    );
    assert!(!srom.join("Engine.exe").exists() && !srom.join("Game/CShell.dll").exists());
    // the header stamp of the shell image
    let a = server_rom::shell_image(&full).unwrap();
    let b = server_rom::shell_image(&srom).unwrap();
    assert_eq!(&a[..0x100], &b[..0x100], "DOS header, stub, signature, file header");
    assert!(b.len() * 4 < a.len(), "strings only: {} of {}", b.len(), a.len());

    let m = server_rom::manifest(&srom).unwrap().unwrap();
    let vf = RezVfs::new(&full);
    let vs = RezVfs::new(&srom);
    let (mut whole, mut heads) = (0, 0);
    for (arch, rows) in &m.archives {
        let lf = vf.list_entries(arch).unwrap();
        let ls = vs.list_entries(arch).unwrap();
        assert_eq!(lf.len(), ls.len(), "{arch}: entry count");
        for (x, y) in lf.iter().zip(&ls) {
            assert_eq!((&x.0, x.2), (&y.0, y.2), "{arch}: names and sizes in archive order");
        }
        for e in rows {
            let Some(_) = &e.sha256 else {
                let r = vs.entry(arch, &e.name);
                assert!(
                    matches!(r, Err(_) | Ok(None)),
                    "{arch} {} is left out and must not read",
                    e.name
                );
                continue;
            };
            let got = vs.entry(arch, &e.name).unwrap().unwrap();
            let want = vf.entry(arch, &e.name).unwrap().unwrap();
            if e.head.is_some() {
                heads += 1;
                assert!(want.starts_with(&got), "{arch} {}: head", e.name);
            } else {
                whole += 1;
                // a name listed twice keeps the last one in both folders
                assert_eq!(got, want, "{arch} {}", e.name);
            }
        }
    }
    assert!(whole > 100 && heads > 100, "whole {whole} heads {heads}");
    // a changed file is refused
    let items = server_rom::entry_path(&srom, "ATTRIBUTES.rez", "ATTRIBUTES\\TEAMS.TXT");
    if temp && items.is_file() {
        let mut b = std::fs::read(&items).unwrap();
        b[0] ^= 1;
        std::fs::write(&items, &b).unwrap();
        assert!(RezVfs::new(&srom)
            .entry("ATTRIBUTES.rez", "ATTRIBUTES\\TEAMS.TXT")
            .is_err());
        assert!(dc_rom::identify_rom(&srom).is_err());
    }
    if temp {
        let _ = std::fs::remove_dir_all(&srom);
    }
}
