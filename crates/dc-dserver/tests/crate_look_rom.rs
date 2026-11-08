//! ROM-gated check of CRATE-2 (skipped without `CA_ROM`): the Camo Crate's case table row names a model and skins
//! that all resolve in the archives, and it does not look like a plain HiSec drop (OWNER REPORT 2026-11-07 "camo
//! crates also appear as black hi-sec"). Every ROM value is read at run time.
// Provenance: ORIGINAL (our own check of our own default)
// OBSERVED: ROM table key HISECS.TXT [Hisec<n>] Filename, Skin<k>, RenderStyle<k>

use dc_dserver::game::hisec::HisecTuning;
use dc_dserver::rom_tables::{HisecRow, RomTables};

/// Whether `file` is in any archive of the ROM's game folder.
fn in_archives(vfs: &dc_rez::RezVfs, rezs: &[String], file: &str) -> bool {
    rezs.iter()
        .any(|r| matches!(vfs.entry_raw(r, file), Ok(Some(_))))
}

fn same_look(a: &HisecRow, b: &HisecRow) -> bool {
    let low = |v: &[String]| v.iter().map(|s| s.to_ascii_lowercase()).collect::<Vec<_>>();
    a.model.as_deref().map(str::to_ascii_lowercase)
        == b.model.as_deref().map(str::to_ascii_lowercase)
        && low(&a.skins) == low(&b.skins)
}

#[test]
fn rom_camo_crate_row_resolves_and_differs_from_the_plain_cases() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    let vfs = dc_rez::RezVfs::new(&rom);
    let rezs: Vec<String> = std::fs::read_dir(rom.join("Game"))
        .expect("game folder")
        .filter_map(|e| {
            let n = e.ok()?.file_name().to_string_lossy().to_string();
            n.to_ascii_lowercase().ends_with(".rez").then_some(n)
        })
        .collect();
    let h = HisecTuning::default();
    let rows = &tables.hisec.rows;
    let crate_row = rows.get(&h.camo_row).expect("the crate's case table row");
    let model = crate_row
        .model
        .as_deref()
        .expect("the crate row names a model");
    assert!(in_archives(&vfs, &rezs, model), "model {model}");
    assert!(!crate_row.skins.is_empty(), "the crate row names a skin");
    for s in &crate_row.skins {
        assert!(in_archives(&vfs, &rezs, s), "skin {s}");
    }
    for s in &crate_row.styles {
        assert!(in_archives(&vfs, &rezs, s), "render style {s}");
    }
    // every plain drop row (HS-15) has another model or other skins than the crate
    for &(row, w) in &h.case_rows {
        if w == 0 {
            continue;
        }
        let plain = rows.get(&row).expect("a plain case row");
        assert!(
            !same_look(plain, crate_row),
            "plain row {row} looks like the crate row {}",
            h.camo_row
        );
    }
    assert!(!h.case_rows.iter().any(|&(r, w)| w > 0 && r == h.camo_row));
}
