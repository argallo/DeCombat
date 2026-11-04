//! ROM-gated: the first-person model of a rifle row of the user's ROM weapon table drawn with the player-view
//! camera (RENDER_2009.md §4.2 / §4.3) on the first map: it lands in the lower half of the frame, in front of the
//! world. Skipped when CA_ROM is unset or no graphics adapter exists. Nothing of the ROM is asserted by value
//! besides counts; the images are ROM-derived local files (folder from `DC_RENDER_OUT`, else the system temp
//! folder), never part of the tree.

use dc_render::*;

/// OBSERVED (2026-10-25, `ViewModels::from_vfs`): 297 weapon rows name a first-person model for the first hand
/// identifier of the table.
const ROM_VIEW_MODEL_ROWS: usize = 297;
/// A rifle row: weapon 145 (the assault rifle of the captures dc-client replays, CLI-28's OBSERVED bounds).
const RIFLE_ROW: u32 = 145;

fn viewer(rom: &std::path::Path) -> Option<RomViewer> {
    match RomViewer::open(rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => Some(v),
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn rom_rifle_first_person_model_in_the_lower_half() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let table = ViewModels::from_vfs(v.vfs()).expect("weapon table");
    assert_eq!(table.rows.len(), ROM_VIEW_MODEL_ROWS);
    let row = table.rows.get(&RIFLE_ROW).expect("rifle row");
    let map = v.map_names().first().cloned().expect("a map");
    let (w, h) = (640u32, 480u32);
    let mut req = ViewRequest::new(&map);
    req.width = w;
    req.height = h;
    let (plain, _) = v.render(&req).expect("frame without the view model");
    let mut vr = ViewModelRequest::new(&row.model);
    vr.textures = row.skins.clone();
    vr.styles = row.styles.clone();
    // DC-DEFAULT: REN-15 (proposed), see DC_DEFAULTS.md (viewer: origin 4 units below the eye)
    vr.offset = [0.0, -4.0, 0.0];
    req.view_model = Some(vr);
    let (img, report) = v.render(&req).expect("frame with the view model");
    let out = default_out_dir();
    let _ = std::fs::create_dir_all(&out);
    let _ = img.save_png(&out.join("rom_playerview_rifle.png"));
    println!("{}", report.summary());
    assert_eq!(report.frame.view_models, 1);
    let vm = report.view_model.as_ref().expect("view model report");
    assert!(vm.clip.is_some(), "the idle clip (or clip 0) is played");
    let rect = vm.screen_rect.expect("on screen");
    assert!(rect[3] > h / 2, "reaches the lower half {rect:?}");
    let (mut n, mut lower, mut sum_y) = (0u64, 0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let (p, q) = (plain.pixel(x, y), img.pixel(x, y));
            if (0..3).any(|k| (p[k] as i32 - q[k] as i32).abs() > 24) {
                n += 1;
                sum_y += y as u64;
                lower += (y >= h / 2) as u64;
            }
        }
    }
    let total = (w * h) as u64;
    println!(
        "changed {n} of {total} pixels, {lower} in the lower half, mean y {}",
        sum_y / n.max(1)
    );
    assert!(n > total / 50, "the view model covers {n} pixels");
    assert!(
        lower * 10 >= n * 8,
        "mostly in the lower half: {lower} of {n}"
    );
    assert!(sum_y / n > (h / 2) as u64, "centroid below the middle");
}
