//! ROM-gated census of every texture entry. Skipped when CA_ROM is unset.
//! Counts and hashes only; no names or pixels appear here. SPEC: FMT_DTX.md §8.

use dc_rez::RezArchive;
use dc_tex::{DtxFile, DtxHeader, PixelFormat, HEADER_LEN};
use std::collections::BTreeMap;

fn is_dtx(name: &str) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".dtx")
}

#[test]
fn rom_header_census_matches_spec() {
    let rom = dc_rom::rom_or_skip!();
    let mut rez_files: Vec<_> = std::fs::read_dir(rom.join("Game"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("rez"))
                .unwrap_or(false)
        })
        .collect();
    rez_files.sort();

    let (mut total, mut empty, mut exact, mut version_ok) = (0usize, 0usize, 0usize, 0usize);
    let mut by_format: BTreeMap<u8, usize> = BTreeMap::new();
    let mut by_levels: BTreeMap<u16, usize> = BTreeMap::new();
    let mut by_group: BTreeMap<u8, usize> = BTreeMap::new();
    let mut sections_nonzero = 0usize;
    let mut sample: Vec<(std::path::PathBuf, u64, u32)> = Vec::new();

    for path in &rez_files {
        let arch = RezArchive::open(path).unwrap_or_else(|e| panic!("{e}"));
        for (name, pos, size) in arch.entries() {
            if !is_dtx(name) {
                continue;
            }
            total += 1;
            if *size == 0 {
                empty += 1;
                continue;
            }
            // header-only read (the 0xA4 bytes), then the size rule against the directory size
            let head = arch.read_raw(*pos, (*size).min(HEADER_LEN as u32)).unwrap();
            let h =
                DtxHeader::parse(&head).expect("every non-empty entry passes the engine's checks");
            version_ok += 1;
            let fmt = PixelFormat::from_code(h.format).unwrap();
            let sum: u64 = (0..h.level_count)
                .map(|i| {
                    fmt.level_bytes((h.width as u32) >> i, (h.height as u32) >> i)
                        .unwrap()
                })
                .sum();
            if *size as u64 == HEADER_LEN as u64 + sum {
                exact += 1;
            }
            *by_format
                .entry(PixelFormat::from_code(h.format).unwrap().code())
                .or_default() += 1;
            *by_levels.entry(h.level_count).or_default() += 1;
            *by_group.entry(h.lod_group).or_default() += 1;
            sections_nonzero += (h.section_count != 0) as usize;
            if total % 97 == 0 {
                sample.push((path.clone(), *pos, *size));
            }
        }
    }

    println!("dtx census: total={total} empty={empty} exact={exact} formats={by_format:?} levels={by_levels:?} lod_groups={by_group:?} sections_nonzero={sections_nonzero}");
    assert_eq!(sections_nonzero, 0);
    assert_eq!(total, 11_272);
    assert_eq!(empty, 1);
    assert_eq!(version_ok, 11_271); // every non-empty entry has kind 0 and version -5
    assert_eq!(exact, 11_271);
    assert_eq!(by_format.get(&3), Some(&6_777));
    assert_eq!(by_format.get(&4), Some(&3_030));
    assert_eq!(by_format.get(&5), Some(&536));
    assert_eq!(by_format.get(&6), Some(&928));
    assert_eq!(by_format.len(), 4);
    assert_eq!(by_levels.get(&1), Some(&6_705));
    assert_eq!(by_levels.get(&8), Some(&27));

    // decode a sample end to end (no panic, correct output sizes, zero trailing bytes)
    assert!(sample.len() > 50);
    for (path, pos, size) in sample.iter().take(150) {
        let arch = RezArchive::open(path).unwrap();
        let bytes = arch.read_raw(*pos, *size).unwrap();
        let f = DtxFile::parse(&bytes).unwrap();
        assert_eq!(f.trailing(), 0);
        for l in 0..f.level_count() {
            let px = f.try_decode_rgba8(l, 0).unwrap();
            assert_eq!(
                px.len(),
                (f.level_width(l).unwrap() * f.level_height(l).unwrap() * 4) as usize
            );
        }
        let _ = f.commands();
    }
}

#[test]
fn rom_command_keyword_census() {
    let rom = dc_rom::rom_or_skip!();
    let (mut nonempty, mut alpha, mut env_alpha, mut detail) = (0usize, 0usize, 0usize, 0usize);
    for e in std::fs::read_dir(rom.join("Game")).unwrap() {
        let path = e.unwrap().path();
        if !path
            .extension()
            .map(|x| x.eq_ignore_ascii_case("rez"))
            .unwrap_or(false)
        {
            continue;
        }
        let arch = RezArchive::open(&path).unwrap();
        for (name, pos, size) in arch.entries() {
            if !is_dtx(name) || (*size as usize) < HEADER_LEN {
                continue;
            }
            let head = arch.read_raw(*pos, HEADER_LEN as u32).unwrap();
            let h = DtxHeader::parse(&head).unwrap();
            let cmd = h.command_str();
            if cmd.is_empty() {
                continue;
            }
            nonempty += 1;
            let set = dc_tex::parse_command(&cmd).expect("shipped commands parse");
            alpha += set.alpha_test_value.is_some() as usize;
            match set.material.map(|m| m.kind) {
                Some(dc_tex::MaterialKind::EnvMapAlpha) => env_alpha += 1,
                Some(dc_tex::MaterialKind::DetailTex) => detail += 1,
                _ => {}
            }
        }
    }
    println!(
        "command census: nonempty={nonempty} alpha={alpha} env_alpha={env_alpha} detail={detail}"
    );
    assert_eq!(nonempty, 624);
    assert_eq!(alpha, 617);
    assert_eq!(env_alpha, 3);
    assert_eq!(detail, 2);
}
